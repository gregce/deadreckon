import Foundation

/// The posting seam between the AttentionCenter and the platform. The app's
/// adapter wraps UNUserNotificationCenter (per-post authorization resolution,
/// stable identifiers — the exemplar pattern); tests record intents. All
/// derivation logic runs Kit-side against this protocol, so nothing here
/// needs the UserNotifications framework.
public protocol UserNotifying: AnyObject {
    /// Post one intent. The adapter uses `deliveryIdentity` as the platform
    /// notification identifier so repeats replace rather than stack, and
    /// resolves authorization per post (an unauthorized post is a silent
    /// no-op; a grant made later in System Settings takes effect on the
    /// next post without a relaunch).
    func post(_ intent: NotificationIntent)
}

/// Discovers and tails `notify.jsonl` across the fleet, deriving user
/// notifications from typed `operator_attention` rows (APP-5, operator
/// decision 6; docs/TAILING.md notify entry).
///
/// Mechanics, each pinned by AttentionCenterTests:
/// - **Run root resolution** reuses the JobDetailStore convention: the
///   current attempt is `jobs/<id>/projection.json` `child_run_ids.last`,
///   its root `home/runstate/<scope>/runs/<id>`; when no child is linked the
///   job's own id stands in (the same fallback `verdict <job>` uses — a
///   Single-shape job's attempt run IS the job id). `notify.jsonl` lives in
///   that run root.
/// - **Bounded tails** (AttentionTailPlan, pure and tested): only rows in
///   non-terminal or recently-terminal states hold a live tail, capped at
///   `activeTailLimit`, newest first. Every other row is covered by a slow
///   full-scan sweep (`fallbackSweepEveryTicks`); the seen-set makes the
///   re-read safe.
/// - **Stable identity** (AttentionDerivation, pure and tested): event
///   identity comes from the record's own fields, never the time of
///   observation, so tails rebuilt from offset 0 and app relaunch cannot
///   re-fire. The seen-set persists in UserDefaults with a bounded LRU, and
///   every observation refreshes an identity's LRU position — an identity
///   still present in a swept file can never age out and re-fire.
/// - **Launch catch-up:** `start()` full-scans the current fleet's notify
///   files and fires only unseen identities whose `at` is within
///   `catchUpWindow` (48 h). Older unseen rows are marked seen without
///   firing — stale news is not a banner. The same recency rule applies at
///   every fire site, uniformly, with ONE deliberate waiver: approval-class
///   reasons (verified_awaiting_promote, waiting_input) fire regardless of
///   age when the current rollup row still shows the decision waiting —
///   summoning the returning operator is the product, and a decision-queue
///   row is a decision-queue row however old (design A1). A record whose
///   decision has since been resolved stays silent.
/// - **Per-reason preferences** are consulted at post time via the injected
///   closure, so Settings toggles apply immediately. Filtered rows are
///   still marked seen (re-enabling a reason does not replay old news).
/// - **Honest degradation:** notify.jsonl is a blessed standard tail, so a
///   corrupt verdict is sticky per (job, runID): the run's file stops
///   producing notifications — it is neither tailed nor swept — until the
///   projection points at a NEW attempt, and `issues[jobID]` carries the
///   reason verbatim for the views to render. Siblings are unaffected.
///   Delivery-attempt rows and unknown-reason rows are observed, never
///   posted. Sweeps skip torn final lines (retried next sweep) and skip
///   undecodable lines without a corruption verdict — a sweep is a catch-up
///   read, not a corruption judge; the live tail keeps the strict verdict.
/// - **Main-actor hygiene:** every file read (projection resolution, tail
///   polls, sweep reads) runs off the main actor in an awaited detached
///   task; only derivation over the gathered lines and the `@Published`
///   writes run on the MainActor. `pollOnce` is serialized by an in-flight
///   guard, so the single-owner tailer contract holds even though polls
///   leave the actor.
/// - Trust rule 7 stands: these rows confer no authority. The notification
///   opens the app onto the real surfaces; nothing acts on the row itself.
@MainActor
public final class AttentionCenter: ObservableObject {
    public struct Cadence: Equatable, Sendable {
        /// Active-tail poll interval.
        public var poll: TimeInterval
        /// Fallback rows get one full scan every this many ticks.
        public var fallbackSweepEveryTicks: Int
        public init(poll: TimeInterval = 5, fallbackSweepEveryTicks: Int = 12) {
            self.poll = poll
            self.fallbackSweepEveryTicks = fallbackSweepEveryTicks
        }
        public static let standard = Cadence()
    }

    /// Ceiling on simultaneously live notify tails across the fleet.
    public static let activeTailLimit = 12
    /// Catch-up recency window: an unseen record older than this is marked
    /// seen without firing (approval-class waiver aside, see above).
    public static let catchUpWindow: TimeInterval = 48 * 3600

    /// Per-job tail trouble, the failing surface's own words. Sticky per
    /// (job, runID) — cleared only when the projection points at a new
    /// attempt, or when the job leaves the fleet. Rendered by the Gate
    /// Queue header chip and Settings > Notifications.
    @Published public private(set) var issues: [String: String] = [:]

    /// Test observability: which jobs currently hold a live tail.
    public var activeTailJobIDs: Set<String> { Set(tails.keys) }

    public var nowProvider: () -> Date = { Date() }

    private let home: URL
    private let notifier: UserNotifying
    private let seenStore: NotificationSeenStore
    private let preferencesProvider: () -> AttentionPreferences
    private let rowsProvider: () -> [FleetRow]
    private let cadence: Cadence

    private struct TailState {
        let runID: String
        let tailer: JSONLTailer
    }

    private var tails: [String: TailState] = [:]
    /// Sticky corruption verdicts: jobID -> the runID whose notify.jsonl
    /// earned a corrupt verdict. Consulted by BOTH the tail reconcile and
    /// the sweeps, so leaving the active tail set cannot un-stick a verdict.
    private var corruptRuns: [String: String] = [:]
    private var running = false
    private var tick = 0
    private var didCatchUp = false
    private var pollInFlight = false
    private var pollTask: Task<Void, Never>?

    public init(home: URL = DeadreckonHome.url(),
                notifier: UserNotifying,
                seenStore: NotificationSeenStore,
                preferences: @escaping () -> AttentionPreferences,
                rowsProvider: @escaping () -> [FleetRow],
                cadence: Cadence = .standard) {
        self.home = home
        self.notifier = notifier
        self.seenStore = seenStore
        self.preferencesProvider = preferences
        self.rowsProvider = rowsProvider
        self.cadence = cadence
    }

    /// Launch-time catch-up scan, then the periodic tick loop. Idempotent.
    public func start() {
        guard !running else { return }
        running = true
        let interval = cadence.poll
        pollTask = Task { [weak self] in
            await self?.catchUpScan()
            while true {
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                // Re-bind weakly each iteration: a discarded center leaks no
                // immortal loop, and the strong reference dies with the scope.
                guard let self, self.running, !Task.isCancelled else { return }
                await self.pollOnce()
            }
        }
    }

    public func stop() {
        running = false
        pollTask?.cancel()
        pollTask = nil
        tails = [:]
        // issues and corruptRuns survive stop(): a sticky verdict is a fact
        // about the file, not about the loop.
    }

    /// One deterministic tick (tests drive this directly). Serialized: a
    /// tick arriving while one is in flight is dropped, so the single-owner
    /// tailer contract holds across the off-actor reads.
    public func pollOnce() async {
        guard !pollInFlight else { return }
        pollInFlight = true
        defer { pollInFlight = false }

        tick += 1
        let rows = rowsProvider()
        pruneDepartedJobs(rows: rows)
        // Deferred launch catch-up: at app launch the fleet is usually still
        // loading, so start()'s scan sees no rows. The first non-empty
        // snapshot completes the catch-up immediately — fallback-only rows
        // must not wait out a whole sweep interval for their missed
        // notifications.
        if !didCatchUp, !rows.isEmpty {
            didCatchUp = true
            await sweep(rows: rows)
        }
        let plan = AttentionTailPlan.select(
            rows: rows, limit: Self.activeTailLimit, now: nowProvider())
        await reconcileTails(active: plan.active)
        await pollActiveTails(rows: rows)
        if tick % max(1, cadence.fallbackSweepEveryTicks) == 0 {
            await sweep(rows: plan.fallback)
        }
    }

    /// Full-scan the whole current fleet, firing only unseen recent records.
    public func catchUpScan() async {
        let rows = rowsProvider()
        if !rows.isEmpty { didCatchUp = true }
        await sweep(rows: rows)
    }

    // MARK: - Housekeeping

    /// Issues and corruption verdicts for jobs no longer in the fleet are
    /// pruned — but only against a non-empty snapshot; a still-loading fleet
    /// is not evidence of absence.
    private func pruneDepartedJobs(rows: [FleetRow]) {
        guard !rows.isEmpty else { return }
        let ids = Set(rows.map(\.jobID))
        for jobID in issues.keys where !ids.contains(jobID) {
            issues[jobID] = nil
        }
        for jobID in corruptRuns.keys where !ids.contains(jobID) {
            corruptRuns[jobID] = nil
        }
    }

    // MARK: - Tails

    private func reconcileTails(active: [FleetRow]) async {
        let activeIDs = Set(active.map(\.jobID))
        for jobID in tails.keys where !activeIDs.contains(jobID) {
            tails[jobID] = nil
        }
        guard !active.isEmpty else { return }
        let home = self.home
        let targets = active.map { (jobID: $0.jobID, scope: $0.scope) }
        // Run-root resolution reads projection.json: off the main actor.
        let resolved: [(jobID: String, runID: String, root: URL)] =
            await Task.detached(priority: .utility) {
                targets.map { target in
                    let run = AttentionCenter.resolveRun(
                        home: home, jobID: target.jobID, scope: target.scope)
                    return (jobID: target.jobID, runID: run.runID, root: run.root)
                }
            }.value
        for entry in resolved {
            if corruptRuns[entry.jobID] == entry.runID {
                // Sticky verdict: the corrupt run is never re-tailed (and
                // never swept); re-entering the active set changes nothing.
                tails[entry.jobID] = nil
                continue
            }
            if let existing = tails[entry.jobID], existing.runID == entry.runID {
                continue
            }
            // New attempt (or first sight): a fresh tail from offset 0. The
            // seen-set absorbs the re-read of history. A NEW runID is the
            // only thing that clears a sticky corruption verdict.
            if corruptRuns[entry.jobID] != nil {
                corruptRuns[entry.jobID] = nil
                issues[entry.jobID] = nil
            }
            tails[entry.jobID] = TailState(
                runID: entry.runID,
                tailer: JSONLTailer(
                    url: entry.root.appendingPathComponent(AttentionCenter.notifyFileName),
                    mode: .standard))
        }
    }

    private func pollActiveTails(rows: [FleetRow]) async {
        guard !tails.isEmpty else { return }
        let polls = tails.map { (jobID: $0.key, runID: $0.value.runID, tailer: $0.value.tailer) }
        // File I/O off the main actor; pollInFlight keeps each tailer
        // single-owner across the await.
        let results: [(jobID: String, runID: String, result: JSONLTailer.PollResult)] =
            await Task.detached(priority: .utility) {
                polls.map { (jobID: $0.jobID, runID: $0.runID, result: $0.tailer.poll()) }
            }.value
        let rowsByID = Dictionary(rows.map { ($0.jobID, $0) }, uniquingKeysWith: { first, _ in first })
        for entry in results {
            guard tails[entry.jobID]?.runID == entry.runID else { continue }
            switch entry.result {
            case .none:
                continue
            case .lines(let lines), .restarted(let lines):
                process(lines: lines, owningJobID: entry.jobID, row: rowsByID[entry.jobID])
            case .corrupt(let reason):
                // Sticky, per the blessed-file contract: this run's file
                // stops producing notifications (tail AND sweep) and the
                // reason is recorded for the rendering surfaces.
                corruptRuns[entry.jobID] = entry.runID
                issues[entry.jobID] = reason
                tails[entry.jobID] = nil
            }
        }
    }

    // MARK: - Sweeps

    /// Full read of each row's notify.jsonl; unseen + recent + allowed rows
    /// fire, everything derived is marked seen. Torn final lines are skipped
    /// (they will complete by the next sweep); undecodable complete lines
    /// are skipped — a sweep is a catch-up read, not a corruption judge (the
    /// live tail keeps the strict verdict). Runs with a sticky corruption
    /// verdict are skipped entirely. All file reads run off the main actor.
    private func sweep(rows: [FleetRow]) async {
        guard !rows.isEmpty else { return }
        let home = self.home
        let corrupt = corruptRuns
        let targets = rows.map { (jobID: $0.jobID, scope: $0.scope) }
        let gathered: [(jobID: String, lines: [String])] =
            await Task.detached(priority: .utility) {
                var out: [(jobID: String, lines: [String])] = []
                for target in targets {
                    let resolved = AttentionCenter.resolveRun(
                        home: home, jobID: target.jobID, scope: target.scope)
                    guard corrupt[target.jobID] != resolved.runID else { continue }
                    let url = resolved.root.appendingPathComponent(AttentionCenter.notifyFileName)
                    guard let data = try? Data(contentsOf: url),
                          let text = String(data: data, encoding: .utf8) else { continue }
                    var lines = text.split(separator: "\n", omittingEmptySubsequences: true)
                        .map(String.init)
                    if !text.hasSuffix("\n"), !lines.isEmpty {
                        // Torn append (TAILING.md): retained unparsed, retried later.
                        lines.removeLast()
                    }
                    guard !lines.isEmpty else { continue }
                    out.append((jobID: target.jobID, lines: lines))
                }
                return out
            }.value
        let rowsByID = Dictionary(rows.map { ($0.jobID, $0) }, uniquingKeysWith: { first, _ in first })
        for entry in gathered {
            process(lines: entry.lines, owningJobID: entry.jobID, row: rowsByID[entry.jobID])
        }
    }

    // MARK: - Derivation

    private func process(lines: [String], owningJobID: String, row: FleetRow?) {
        guard !lines.isEmpty else { return }
        let decoder = DeadreckonJSON.decoder()
        let now = nowProvider()
        let preferences = preferencesProvider()
        for line in lines {
            guard let data = line.data(using: .utf8),
                  let record = try? decoder.decode(NotifyRecord.self, from: data) else { continue }
            guard case .attention(let attentionRow) = record else { continue }
            guard let intent = AttentionDerivation.intent(
                from: attentionRow, owningJobID: owningJobID) else { continue }
            let alreadySeen = seenStore.contains(intent.recordIdentity)
            // Mark (or re-mark) unconditionally: re-observation refreshes the
            // LRU position, so an identity still present in a swept file can
            // never age out of the bounded store and re-fire.
            seenStore.markSeen(intent.recordIdentity)
            guard !alreadySeen else { continue }
            let recent = now.timeIntervalSince(intent.at) <= Self.catchUpWindow
            guard recent || Self.recencyWaived(reason: intent.reason, row: row) else { continue }
            guard preferences.allows(intent.reason) else { continue }
            notifier.post(intent)
        }
    }

    /// The approval-class waiver on the catch-up recency window: a
    /// verified_awaiting_promote or waiting_input record fires however old
    /// it is, PROVIDED the current rollup row still shows that decision
    /// waiting (terminal + verified + valid receipt, or phase waiting).
    /// Return-because-it-notified-me must survive a weekend-plus absence;
    /// a decision resolved since the record landed stays silent.
    private static func recencyWaived(reason: OperatorAttentionReason, row: FleetRow?) -> Bool {
        guard let row else { return false }
        switch reason {
        case .verifiedAwaitingPromote:
            return row.projection.phase == .terminal
                && row.projection.outcome == .verified
                && row.receipt?.verified == .valid
        case .waitingInput:
            return row.projection.phase == .waiting
        default:
            return false
        }
    }

    // MARK: - Paths

    static let notifyFileName = "notify.jsonl"

    /// The JobDetailStore run-root convention, with the verdict-on-JOB
    /// fallback: current attempt = projection child_run_ids.last, else the
    /// job's own id (Single-shape jobs' attempt run IS the job id). Total:
    /// an unreadable projection falls back rather than failing. Nonisolated
    /// so resolution can run off the main actor (it reads projection.json).
    nonisolated static func resolveRun(home: URL, jobID: String, scope: String)
        -> (runID: String, root: URL) {
        let projectionURL = home
            .appendingPathComponent("jobs")
            .appendingPathComponent(jobID)
            .appendingPathComponent("projection.json")
        var runID = jobID
        if let data = try? Data(contentsOf: projectionURL),
           let doc = try? DeadreckonJSON.decoder().decode(JobProjectionDoc.self, from: data),
           let last = doc.childRunIDs.last {
            runID = last
        }
        let root = home
            .appendingPathComponent("runstate")
            .appendingPathComponent(scope)
            .appendingPathComponent("runs")
            .appendingPathComponent(runID)
        return (runID, root)
    }
}
