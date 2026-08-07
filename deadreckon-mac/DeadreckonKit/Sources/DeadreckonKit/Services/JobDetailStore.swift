import Foundation

/// The Chartroom drill-in engine (APP-3): one store per SELECTED job,
/// created when the workbench opens and torn down when it closes. Only the
/// selected job holds active tails (the bounded-tails contract: the fleet
/// board needs no tailing, only the rollup; this store is the whole tail
/// budget). Composition:
///
/// - CLI reads through the FleetCLIRunning seam: `status <id> --json`,
///   `report <id> --json`, `show <run> --diff --json` (+ `--patch --file`
///   per file on demand). `verdict --receipt` is deliberately NOT invoked:
///   the committed binary's `verdict` accepts run references only and the
///   child run of a durable Job is driver-fenced, so the call can only
///   refuse. The receipt band derives from `report --json` until the
///   Rust-side verdict-on-JOB follow-up lands (registered in CONTRACTS.md).
/// - JSONLTailer tails per docs/TAILING.md over the blessed run/job ledgers.
///   narrative/snapshots.jsonl is NOT a TAILING.md-blessed file, so it rides
///   the restart-on-anomaly mode (self-healing, never sticky corruption)
///   until the Rust side blesses it (registered in CONTRACTS.md).
/// - Plain file polls for projection.json, lease.json, state.json,
///   narrative/state.json, flight-manifest.json, checkpoints, docs listing.
///
/// Trust rules held here: tailed rows are display only; the spine and every
/// pane derive from durable files or CLI envelopes; nothing writes under
/// DEADRECKON_HOME; no path touches gate-keys/ (all reads are rooted at
/// jobs/<id>/ and runstate/<scope>/runs/<id>/ by construction).
@MainActor
public final class JobDetailStore: ObservableObject {
    // MARK: Cadence

    public struct Cadence: Equatable, Sendable {
        /// The focused-job tick: file reads + tail polls + `status`.
        public var poll: TimeInterval
        /// Every Nth tick also refreshes `report` (the evidence rail).
        public var reportEveryTicks: Int

        public init(poll: TimeInterval = 2, reportEveryTicks: Int = 5) {
            self.poll = poll
            self.reportEveryTicks = reportEveryTicks
        }

        public static let standard = Cadence()
    }

    // MARK: Ceilings (documented in CONTRACTS.md)

    /// The drawer's raw-line pane keeps this many trailing lines; older raw
    /// lines are dropped WITH a visible count (the parsed Activity scrollback
    /// stays unbounded; the full raw history stays in events.jsonl on disk).
    public static let rawEventLineCeiling = 2_000
    static let rawEventLineSlack = 500
    /// supervisor.out/err are kept to roughly this many trailing characters
    /// each; trimming lands on a line boundary and is announced honestly.
    public static let supervisorTextCeiling = 262_144
    static let supervisorTextSlack = 65_536
    /// A turns fold larger than this (first open of a long history) runs off
    /// the main actor so the workbench does not stall.
    static let mainThreadFoldLimit = 1_500

    // MARK: Read-model surface

    public struct ActivityEntry: Equatable, Sendable, Identifiable {
        public let ordinal: Int
        public let timestamp: Date?
        public let line: String
        public var id: Int { ordinal }
    }

    public struct SpendMeter: Equatable, Sendable {
        /// The loop head: the LAST `kind == "loop"` row's running total.
        /// Never a sum across kinds (TAILING.md).
        public var loopTotalUSD: Double = 0
        /// App-side sum of narrator rows' per-row `cost_usd` (the narrator
        /// keeps no cross-row head in the shared ledger). Labeled as the
        /// narrator split, never folded into run spend.
        public var narratorTotalUSD: Double = 0
        public var capUSD: Double?
        public var lastLoopTurn: Int = 0
        public var recordCount: Int = 0
    }

    public struct FlightState: Equatable, Sendable {
        public var manifest: FlightManifestDoc?
        public var eventCount: Int = 0
        public var lastEventSummary: String?
        public var checkpoints: [CheckpointManifestDoc] = []
    }

    public struct NarrativePane: Equatable, Sendable {
        public var stateDoc: NarrativeStateDoc?
        /// Newest beat of any status; overlay labeling comes from
        /// `isUnverifiedOverlay` on the snapshot itself.
        public var latestSnapshot: NarrativeSnapshotDoc?
        /// Newest positively-deterministic beat: the projection the pane
        /// always renders, overlay or not.
        public var latestDeterministic: NarrativeSnapshotDoc?
        public var staleness: NarrativeStaleness = .unknown
        public var skippedMalformedRows: Int = 0
    }

    // MARK: Published state

    @Published public private(set) var status: JobStatusEnvelope?
    @Published public private(set) var statusIssue: String?
    @Published public private(set) var report: JobReportEnvelope?
    @Published public private(set) var reportIssue: String?

    @Published public private(set) var projection: JobProjectionDoc?
    /// Set when projection.json exists but could not be read/decoded this
    /// tick (mid-write or corruption). The last good checkpoint is KEPT: a
    /// transient read failure never fabricates "no attempt yet" and never
    /// churns the run tailers.
    @Published public private(set) var projectionIssue: String?
    @Published public private(set) var lease: JobLease?
    @Published public private(set) var runState: RunStateDoc?
    @Published public private(set) var spine: SpineSnapshot?

    @Published public private(set) var activity: [ActivityEntry] = []
    @Published public private(set) var rawEventLines: [String] = []
    /// Raw lines dropped from the head of `rawEventLines` (bounded drawer
    /// pane); also the stable ordinal base for the pane's row identity.
    @Published public private(set) var rawEventsDropped = 0
    @Published public private(set) var activityIssue: String?
    /// Per-tail corruption verdicts for the strict blessed files, in the
    /// tailer's own words (the owning pane renders them; nothing freezes
    /// silently).
    @Published public private(set) var tracesIssue: String?
    @Published public private(set) var spendIssue: String?
    @Published public private(set) var flightIssue: String?
    @Published public private(set) var turns: [TurnModel] = []
    @Published public private(set) var spendMeter = SpendMeter()
    @Published public private(set) var flight = FlightState()
    @Published public private(set) var narrative = NarrativePane()
    @Published public private(set) var liveChecks: [AcceptanceProgressRow] = []
    @Published public private(set) var docs: [DocEntry] = []

    @Published public private(set) var integrity: JobEventsIntegrity = .none
    @Published public private(set) var jobEventsTornTail = false
    @Published public private(set) var supervisorOut = ""
    @Published public private(set) var supervisorErr = ""
    @Published public private(set) var supervisorOutTruncated = false
    @Published public private(set) var supervisorErrTruncated = false

    @Published public private(set) var changes: DiffSummaryModel?
    @Published public private(set) var changesIssue: String?
    @Published public private(set) var patches: [String: PatchModel] = [:]
    @Published public private(set) var patchIssues: [String: String] = [:]

    @Published public private(set) var currentRunID: String?
    @Published public private(set) var isOpen = false

    public let jobID: String
    public let scope: String
    public let goal: String

    /// Injectable clock so staleness/spine tests are deterministic.
    public var nowProvider: () -> Date = Date.init

    // MARK: Internals

    private let cli: FleetCLIRunning
    private let home: URL
    private let cadence: Cadence
    private var pollTask: Task<Void, Never>?
    private var tick = 0
    private var generation = 0

    private var jobEventsTailer: JSONLTailer?
    private var eventsTailer: JSONLTailer?
    private var tracesTailer: JSONLTailer?
    private var spendTailer: JSONLTailer?
    private var flightEventsTailer: JSONLTailer?
    private var snapshotsTailer: JSONLTailer?
    private var progressTailer: JSONLTailer?
    private var supervisorOutTailer: TextFileTailer?
    private var supervisorErrTailer: TextFileTailer?

    // Per-tick derivation state, all O(new rows) per tick: newly decoded
    // ledger rows queue here and fold into the persistent turns accumulator;
    // the newest event timestamp and error message are running values.
    private var pendingTurnEvents: [RunEventRecord] = []
    private var pendingTurnTraces: [TraceRow] = []
    private var turnsAccumulator = TurnsDerivation.Accumulator()
    private var lastEventTimestamp: Date?
    private var newestErrorMessage: String?
    private var steerInboxCache: (size: UInt64, count: Int)?
    private var launchPlanCeilings: (runID: String, spendUSD: Double?, wallSeconds: Double?)?

    /// Test observability: how many tailers are currently live. The
    /// open/close lifecycle contract is `close()` -> 0.
    public var activeTailCount: Int {
        [jobEventsTailer, eventsTailer, tracesTailer, spendTailer,
         flightEventsTailer, snapshotsTailer, progressTailer].compactMap { $0 }.count
            + [supervisorOutTailer, supervisorErrTailer].compactMap { $0 }.count
    }

    public init(jobID: String, scope: String, goal: String,
                cli: FleetCLIRunning, home: URL = DeadreckonHome.url(),
                cadence: Cadence = .standard) {
        self.jobID = jobID
        self.scope = scope
        self.goal = goal
        self.cli = cli
        self.home = home
        self.cadence = cadence
    }

    // MARK: - Lifecycle

    /// Start the focused-job poll loop. Idempotent.
    public func open() {
        guard !isOpen else { return }
        isOpen = true
        generation += 1
        let myGeneration = generation
        buildJobTailers()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.generation == myGeneration else { return }
                await self.pollOnce()
                let interval = self.cadence.poll
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
    }

    /// Tear down: cancel the loop, SIGTERM any in-flight CLI child of this
    /// workbench, and drop every tailer. After close() the store holds zero
    /// active tails (the bounded-tails contract) and no code path polls
    /// again unless open() is called anew.
    ///
    /// Run resolution and the supervisor text are reset here so a LATER
    /// open() on the same store resumes cleanly: resolveRunRootIfNeeded sees
    /// a nil currentRunID and rebuilds the run tailers (fresh offset-0
    /// re-reads), and the re-read supervisor text lands in cleared strings
    /// instead of appending a duplicate copy.
    public func close() {
        generation += 1
        pollTask?.cancel()
        pollTask = nil
        isOpen = false
        tick = 0
        cli.terminateInFlight(patience: 2)
        jobEventsTailer = nil
        eventsTailer = nil
        tracesTailer = nil
        spendTailer = nil
        flightEventsTailer = nil
        snapshotsTailer = nil
        progressTailer = nil
        supervisorOutTailer = nil
        supervisorErrTailer = nil
        currentRunID = nil
        runState = nil
        spine = nil
        launchPlanCeilings = nil
        pendingTurnEvents = []
        pendingTurnTraces = []
        turnsAccumulator = TurnsDerivation.Accumulator()
        lastEventTimestamp = nil
        newestErrorMessage = nil
        steerInboxCache = nil
        supervisorOut = ""
        supervisorErr = ""
        supervisorOutTruncated = false
        supervisorErrTruncated = false
        integrity = .none
        jobEventsTornTail = false
    }

    // MARK: - One tick (public for deterministic tests)

    public func pollOnce() async {
        let myGeneration = generation
        pollJobFiles()
        resolveRunRootIfNeeded()
        pollRunFiles()
        pollTails()
        deriveSpine()
        await foldPendingTurns(myGeneration: myGeneration)
        guard generation == myGeneration else { return }

        tick += 1
        await refreshStatus()
        guard generation == myGeneration else { return }
        if tick == 1 || tick % cadence.reportEveryTicks == 0 || (projection?.phase == .terminal && report == nil) {
            await refreshReport()
        }
        // NOTE deliberately absent: `verdict <job> --receipt`. The committed
        // binary's `verdict` accepts RUN_LIKE references only, a Single-shape
        // job's id resolves to the Job kind, and the child run is
        // driver-fenced, so the call can only produce a typed refusal. The
        // receipt band derives from `report --json` until the Rust-side
        // verdict-on-JOB follow-up lands (CONTRACTS.md gap register).
    }

    // MARK: - Paths

    private var jobDir: URL { home.appendingPathComponent("jobs").appendingPathComponent(jobID) }

    private var runRoot: URL? {
        guard let currentRunID else { return nil }
        return home.appendingPathComponent("runstate")
            .appendingPathComponent(scope)
            .appendingPathComponent("runs")
            .appendingPathComponent(currentRunID)
    }

    private func buildJobTailers() {
        if jobEventsTailer == nil {
            jobEventsTailer = JSONLTailer(
                url: jobDir.appendingPathComponent("job-events.jsonl"), mode: .jobEvents)
        }
        if supervisorOutTailer == nil {
            supervisorOutTailer = TextFileTailer(url: jobDir.appendingPathComponent("supervisor.out"))
        }
        if supervisorErrTailer == nil {
            supervisorErrTailer = TextFileTailer(url: jobDir.appendingPathComponent("supervisor.err"))
        }
    }

    private func buildRunTailers(root: URL) {
        eventsTailer = JSONLTailer(url: root.appendingPathComponent("events.jsonl"), mode: .standard)
        tracesTailer = JSONLTailer(url: root.appendingPathComponent("traces.jsonl"), mode: .standard)
        spendTailer = JSONLTailer(url: root.appendingPathComponent("spend.jsonl"), mode: .standard)
        flightEventsTailer = JSONLTailer(
            url: root.appendingPathComponent("flight-events.jsonl"), mode: .standard)
        // narrative/snapshots.jsonl is NOT blessed by docs/TAILING.md, so it
        // gets no strict corruption verdict: restart-on-anomaly re-reads the
        // file honestly instead of freezing the pane forever on a rewrite.
        snapshotsTailer = JSONLTailer(
            url: root.appendingPathComponent("narrative").appendingPathComponent("snapshots.jsonl"),
            mode: .acceptanceProgress)
        progressTailer = JSONLTailer(
            url: root.appendingPathComponent("proofs").appendingPathComponent("acceptance-progress.jsonl"),
            mode: .acceptanceProgress)
        // Fresh attempt, fresh scrollback: the ledgers are per run.
        pendingTurnEvents = []
        pendingTurnTraces = []
        turnsAccumulator = TurnsDerivation.Accumulator()
        lastEventTimestamp = nil
        newestErrorMessage = nil
        steerInboxCache = nil
        activity = []
        rawEventLines = []
        rawEventsDropped = 0
        activityIssue = nil
        tracesIssue = nil
        spendIssue = nil
        flightIssue = nil
        turns = []
        spendMeter = SpendMeter()
        flight = FlightState()
        narrative = NarrativePane()
        liveChecks = []
        changes = nil
        patches = [:]
        patchIssues = [:]
    }

    // MARK: - File polls

    private func pollJobFiles() {
        let outcome: JSONReadOutcome<JobProjectionDoc> =
            readJSONOutcome(jobDir.appendingPathComponent("projection.json"))
        switch outcome {
        case .absent:
            projection = nil
            projectionIssue = nil
        case .value(let doc):
            projection = doc
            projectionIssue = nil
        case .unreadable(let reason):
            // Mid-write or corruption: keep the last good checkpoint and say
            // so. Regressing to nil would fabricate "no attempt yet" and
            // wipe/rebuild every run tailer over a one-tick fs hiccup.
            projectionIssue = "projection.json unreadable this poll, keeping the last good read: \(reason)"
        }
        lease = readJSON(jobDir.appendingPathComponent("lease.json"))
    }

    private func resolveRunRootIfNeeded() {
        let resolved = projection?.childRunIDs.last
        guard resolved != currentRunID else { return }
        currentRunID = resolved
        runState = nil
        launchPlanCeilings = nil
        if let root = runRoot {
            buildRunTailers(root: root)
        } else {
            eventsTailer = nil
            tracesTailer = nil
            spendTailer = nil
            flightEventsTailer = nil
            snapshotsTailer = nil
            progressTailer = nil
        }
    }

    private func pollRunFiles() {
        guard let root = runRoot else { return }
        runState = readJSON(root.appendingPathComponent("state.json"))

        if let runID = currentRunID, launchPlanCeilings?.runID != runID {
            let plan = readRawJSON(root.appendingPathComponent("launch-plan.json"))
            let budget = (plan?["budget"] as? [String: Any])
            launchPlanCeilings = (
                runID: runID,
                spendUSD: (budget?["ceiling_usd"] as? NSNumber)?.doubleValue,
                wallSeconds: (budget?["wall_seconds"] as? NSNumber)?.doubleValue)
        }

        var pane = narrative
        pane.stateDoc = readJSON(
            root.appendingPathComponent("narrative").appendingPathComponent("state.json"))
        pane.staleness = NarrativeStaleness.from(
            createdAt: pane.latestSnapshot?.createdAt ?? pane.stateDoc?.latestCreatedAt,
            now: nowProvider())
        narrative = pane

        var flightState = flight
        flightState.manifest = readJSON(root.appendingPathComponent("flight-manifest.json"))
        flightState.checkpoints = listCheckpoints(root: root)
        flight = flightState

        docs = listDocs()
    }

    private func listCheckpoints(root: URL) -> [CheckpointManifestDoc] {
        let dir = root.appendingPathComponent("checkpoints")
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else {
            return []
        }
        return names.sorted().suffix(60).compactMap { name in
            readJSON(dir.appendingPathComponent(name).appendingPathComponent("manifest.json"))
                as CheckpointManifestDoc?
        }
    }

    private func listDocs() -> [DocEntry] {
        guard let workingDir = runState?.workingDir, !workingDir.isEmpty else { return [] }
        let dir = URL(fileURLWithPath: workingDir)
            .appendingPathComponent(".deadreckon").appendingPathComponent("docs")
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else {
            return []
        }
        return names.sorted().compactMap { name in
            let path = dir.appendingPathComponent(name).path
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: path),
                  (attributes[.type] as? FileAttributeType) == .typeRegular else { return nil }
            return DocEntry(
                name: name,
                path: path,
                bytes: (attributes[.size] as? NSNumber)?.intValue ?? 0,
                modifiedAt: attributes[.modificationDate] as? Date)
        }
    }

    // MARK: - Tail polls

    private func pollTails() {
        pollJobEvents()
        pollActivity()
        pollTraces()
        pollSpend()
        pollFlightEvents()
        pollSnapshots()
        pollAcceptanceProgress()
        pollSupervisorTails()
    }

    private func pollJobEvents() {
        guard let tailer = jobEventsTailer else { return }
        let result = tailer.poll()
        integrity = JobEventsIntegrity.derive(
            previous: integrity, poll: result, lastSequence: tailer.lastSequence)
        jobEventsTornTail = tailer.hasRetainedTail
    }

    private func pollActivity() {
        guard let tailer = eventsTailer else { return }
        switch tailer.poll() {
        case .none, .restarted:
            break
        case .corrupt(let reason):
            // Strict file: report, stop trusting; existing scrollback stays.
            activityIssue = reason
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            for line in lines {
                rawEventLines.append(line)
                let ordinal = activity.count + 1
                if let data = line.data(using: .utf8),
                   let record = try? decoder.decode(RunEventRecord.self, from: data) {
                    pendingTurnEvents.append(record)
                    lastEventTimestamp = record.timestamp
                    if record.event.kind == "error" {
                        newestErrorMessage = record.event.message
                    }
                    activity.append(ActivityEntry(
                        ordinal: ordinal, timestamp: record.timestamp, line: record.activityLine))
                } else {
                    // A schema-conformant ledger line we cannot model yet:
                    // show the raw fact rather than dropping or guessing.
                    activity.append(ActivityEntry(ordinal: ordinal, timestamp: nil, line: line))
                }
            }
            trimRawEventLines()
        }
    }

    private func trimRawEventLines() {
        guard rawEventLines.count > Self.rawEventLineCeiling + Self.rawEventLineSlack else { return }
        let overflow = rawEventLines.count - Self.rawEventLineCeiling
        rawEventLines.removeFirst(overflow)
        rawEventsDropped += overflow
    }

    private func pollTraces() {
        guard let tailer = tracesTailer else { return }
        switch tailer.poll() {
        case .none, .restarted:
            break
        case .corrupt(let reason):
            tracesIssue = reason
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            for line in lines {
                if let data = line.data(using: .utf8),
                   let row = try? decoder.decode(TraceRow.self, from: data) {
                    pendingTurnTraces.append(row)
                }
            }
        }
    }

    private func pollSpend() {
        guard let tailer = spendTailer else { return }
        switch tailer.poll() {
        case .none, .restarted:
            break
        case .corrupt(let reason):
            spendIssue = reason
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            var meter = spendMeter
            for line in lines {
                guard let data = line.data(using: .utf8),
                      let record = try? decoder.decode(SpendRecord.self, from: data) else { continue }
                meter.recordCount += 1
                if record.kind == "narrator" {
                    meter.narratorTotalUSD += record.costUSD
                } else if record.kind == "loop" {
                    meter.loopTotalUSD = record.totalCostUSD
                    meter.lastLoopTurn = record.turn
                    if let cap = record.capUSD { meter.capUSD = cap }
                }
            }
            spendMeter = meter
        }
    }

    private func pollFlightEvents() {
        guard let tailer = flightEventsTailer else { return }
        switch tailer.poll() {
        case .none, .restarted:
            break
        case .corrupt(let reason):
            flightIssue = reason
        case .lines(let lines):
            var flightState = flight
            flightState.eventCount += lines.count
            if let last = lines.last,
               let data = last.data(using: .utf8),
               let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let summary = object["summary"] as? String {
                flightState.lastEventSummary = summary
            }
            flight = flightState
        }
    }

    private func pollSnapshots() {
        guard let tailer = snapshotsTailer else { return }
        switch tailer.poll() {
        case .none, .corrupt:
            // .corrupt cannot happen in restart-on-anomaly mode; ignore.
            break
        case .lines(let lines):
            var pane = narrative
            foldSnapshots(lines, into: &pane)
            narrative = pane
        case .restarted(let lines):
            // The unblessed file was rewritten or shrank: re-fold the whole
            // fresh read instead of freezing (self-healing, still honest:
            // the pane reflects exactly what the file now says).
            var pane = narrative
            pane.latestSnapshot = nil
            pane.latestDeterministic = nil
            pane.skippedMalformedRows = 0
            foldSnapshots(lines, into: &pane)
            narrative = pane
        }
    }

    private func foldSnapshots(_ lines: [String], into pane: inout NarrativePane) {
        let decoder = DeadreckonJSON.decoder()
        for line in lines {
            guard let data = line.data(using: .utf8),
                  let snapshot = try? decoder.decode(NarrativeSnapshotDoc.self, from: data) else {
                pane.skippedMalformedRows += 1
                continue
            }
            pane.latestSnapshot = snapshot
            if !snapshot.isUnverifiedOverlay {
                pane.latestDeterministic = snapshot
            }
        }
        pane.staleness = NarrativeStaleness.from(
            createdAt: pane.latestSnapshot?.createdAt ?? pane.stateDoc?.latestCreatedAt,
            now: nowProvider())
    }

    private func pollAcceptanceProgress() {
        guard let tailer = progressTailer else { return }
        switch tailer.poll() {
        case .none, .corrupt:
            // .corrupt cannot happen in acceptanceProgress mode; ignore.
            break
        case .lines(let lines):
            liveChecks.append(contentsOf: decodeProgress(lines))
        case .restarted(let lines):
            // New gate attempt or sign-time rewrite: discard retained rows,
            // the fresh read is the whole current attempt (TAILING.md rule).
            liveChecks = decodeProgress(lines)
        }
    }

    private func decodeProgress(_ lines: [String]) -> [AcceptanceProgressRow] {
        let decoder = DeadreckonJSON.decoder()
        return lines.compactMap { line in
            guard let data = line.data(using: .utf8) else { return nil }
            return try? decoder.decode(AcceptanceProgressRow.self, from: data)
        }
    }

    private func pollSupervisorTails() {
        if let tailer = supervisorOutTailer {
            switch tailer.poll() {
            case .none: break
            case .appended(let text):
                let (trimmed, dropped) = Self.trimSupervisorText(supervisorOut + text)
                supervisorOut = trimmed
                supervisorOutTruncated = supervisorOutTruncated || dropped
            case .reset(let text):
                let (trimmed, dropped) = Self.trimSupervisorText(text)
                supervisorOut = trimmed
                supervisorOutTruncated = dropped
            }
        }
        if let tailer = supervisorErrTailer {
            switch tailer.poll() {
            case .none: break
            case .appended(let text):
                let (trimmed, dropped) = Self.trimSupervisorText(supervisorErr + text)
                supervisorErr = trimmed
                supervisorErrTruncated = supervisorErrTruncated || dropped
            case .reset(let text):
                let (trimmed, dropped) = Self.trimSupervisorText(text)
                supervisorErr = trimmed
                supervisorErrTruncated = dropped
            }
        }
    }

    /// Keep roughly the last `supervisorTextCeiling` characters, cutting at
    /// a line boundary. Returns the kept text and whether anything dropped.
    static func trimSupervisorText(_ text: String) -> (String, Bool) {
        guard text.count > supervisorTextCeiling + supervisorTextSlack else { return (text, false) }
        var tail = String(text.suffix(supervisorTextCeiling))
        if let newline = tail.firstIndex(of: "\n") {
            tail = String(tail[tail.index(after: newline)...])
        }
        return (tail, true)
    }

    // MARK: - Turns (incremental fold, O(new rows) per tick)

    private func foldPendingTurns(myGeneration: Int) async {
        guard !pendingTurnEvents.isEmpty || !pendingTurnTraces.isEmpty else { return }
        let events = pendingTurnEvents
        let traces = pendingTurnTraces
        pendingTurnEvents = []
        pendingTurnTraces = []
        if events.count + traces.count > Self.mainThreadFoldLimit {
            // First open of a long history: fold off the main actor so the
            // workbench does not beachball; only the @Published assignment
            // happens back here.
            let seed = turnsAccumulator
            let folded = await Task.detached(priority: .userInitiated) {
                () -> (TurnsDerivation.Accumulator, [TurnModel]) in
                var accumulator = seed
                let models = accumulator.fold(events: events, traces: traces)
                return (accumulator, models)
            }.value
            guard generation == myGeneration else { return }
            turnsAccumulator = folded.0
            turns = folded.1
        } else {
            turns = turnsAccumulator.fold(events: events, traces: traces)
        }
    }

    // MARK: - Spine

    private func deriveSpine() {
        guard let state = runState else {
            spine = nil
            return
        }
        let hasReshape = hasReshapeProposal()
        let inputs = RunSpineInputs(
            runID: state.runID,
            status: state.status,
            turn: state.turn,
            activePhaseName: state.activePhaseName,
            totalSpendUSD: state.totalSpendUSD,
            stateMaxSpendUSD: state.maxSpendUSD,
            launchPlanCeilingUSD: launchPlanCeilings?.spendUSD,
            totalWallSeconds: state.totalWallSeconds,
            stateMaxWallSeconds: state.maxWallSeconds,
            launchPlanWallSeconds: launchPlanCeilings?.wallSeconds,
            pauseReason: state.pauseReason,
            failureReason: state.failureReason,
            updatedAt: state.updatedAt,
            newestEventTimestamp: lastEventTimestamp,
            newestErrorMessage: newestErrorMessage,
            hasReshapeProposal: hasReshape,
            pendingSteerCount: pendingSteerCount())
        var snapshot = SpineDerivation.deriveRun(inputs, now: nowProvider())
        // Job altitude for NEXT (design 1.2): every Chartroom attempt is a
        // job-owned run, and public `resume` on a job-owned run is refused
        // by the ownership fence, so the run-spine fallback would suggest a
        // command that refuses. Prefer the job envelope's own next_actions
        // (the friendliness contract); when the envelope is unavailable and
        // the run is failed/killed, observe via `status` instead of
        // suggesting a fenced verb. Reshape still wins (spine.rs invariant).
        if !hasReshape {
            if let action = status?.nextActions.first, !action.isEmpty {
                snapshot = snapshot.replacingNext(action)
            } else if state.status == "failed" || state.status == "killed" {
                snapshot = snapshot.replacingNext("deadreckon status \(jobID)")
            }
        }
        spine = snapshot
    }

    private func hasReshapeProposal() -> Bool {
        guard let root = runRoot else { return false }
        let url = root.appendingPathComponent("reshape-proposal.json")
        guard let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data) else { return false }
        return object is [String: Any]
    }

    private func pendingSteerCount() -> Int {
        guard let root = runRoot else { return 0 }
        let url = root.appendingPathComponent("steer-inbox.jsonl")
        guard let size = ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size]
            as? NSNumber)?.uint64Value else {
            steerInboxCache = nil
            return 0
        }
        // Size-keyed cache: the inbox is append-only and status flips change
        // the byte count, so an unchanged size skips the full re-read.
        if let cache = steerInboxCache, cache.size == size { return cache.count }
        guard let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .utf8) else { return 0 }
        let count = text.split(separator: "\n").reduce(into: 0) { count, line in
            guard let lineData = line.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: lineData) as? [String: Any],
                  (object["status"] as? String) == "pending" else { return }
            count += 1
        }
        steerInboxCache = (size, count)
        return count
    }

    // MARK: - CLI reads

    private func refreshStatus() async {
        let myGeneration = generation
        switch await runCLI(["status", jobID, "--json"]) {
        case .success(let data):
            guard generation == myGeneration else { return }
            do {
                status = try DeadreckonJSON.decoder().decode(JobStatusEnvelope.self, from: data)
                statusIssue = nil
            } catch {
                statusIssue = "status --json did not decode: \(error.localizedDescription)"
            }
        case .failure(let reason):
            guard generation == myGeneration else { return }
            statusIssue = reason
        }
    }

    private func refreshReport() async {
        let myGeneration = generation
        switch await runCLI(["report", jobID, "--json"]) {
        case .success(let data):
            guard generation == myGeneration else { return }
            do {
                report = try DeadreckonJSON.decoder().decode(JobReportEnvelope.self, from: data)
                reportIssue = nil
            } catch {
                reportIssue = "report --json did not decode: \(error.localizedDescription)"
            }
        case .failure(let reason):
            guard generation == myGeneration else { return }
            reportIssue = reason
        }
    }

    /// Changes tab entry point: the full-run diffstat (G10). On demand, not
    /// on the poll loop, because snapshot diffing is not free.
    public func refreshChanges() async {
        guard let runID = currentRunID else {
            changesIssue = "no run attempt yet"
            return
        }
        let myGeneration = generation
        switch await runCLI(["show", runID, "--diff", "--json"], timeout: 60) {
        case .success(let data):
            guard generation == myGeneration else { return }
            do {
                changes = try DeadreckonJSON.decoder().decode(DiffSummaryModel.self, from: data)
                changesIssue = nil
            } catch {
                changesIssue = Self.diffDecodeIssue(data: data, runID: runID, error: error)
            }
        case .failure(let reason):
            guard generation == myGeneration else { return }
            changesIssue = reason
        }
    }

    /// Per-file unified patch, loaded on demand (G10 `--patch --file`): a
    /// single selected file exports in full; truncation honesty rides the
    /// model's own `truncated` flag.
    public func loadPatch(path: String) async {
        guard let runID = currentRunID else { return }
        let myGeneration = generation
        switch await runCLI(
            ["show", runID, "--diff", "--patch", "--file", path, "--json"], timeout: 60) {
        case .success(let data):
            guard generation == myGeneration else { return }
            do {
                let summary = try DeadreckonJSON.decoder().decode(DiffSummaryModel.self, from: data)
                if let patch = summary.patches?.first(where: { $0.path == path }) ?? summary.patches?.first {
                    patches[path] = patch
                    patchIssues[path] = nil
                } else {
                    patchIssues[path] = "no patch returned for \(path)"
                }
            } catch {
                patchIssues[path] = Self.diffDecodeIssue(data: data, runID: runID, error: error)
            }
        case .failure(let reason):
            guard generation == myGeneration else { return }
            patchIssues[path] = reason
        }
    }

    /// Name the known aliasing case instead of a generic decode error: for a
    /// Single-shape job the attempt's run id IS the job id, the resolver
    /// hands `show` the Job, and the Job branch returns job status with no
    /// diff. Rust-side follow-up (`show --diff` Job delegation) is
    /// registered in CONTRACTS.md.
    static func diffDecodeIssue(data: Data, runID: String, error: Error) -> String {
        if let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
           (object["kind"] as? String) == "job_status" {
            return "show \(runID) resolved to the Job itself (a single-attempt job shares its run id) "
                + "and job references carry no diff surface in the committed binary; "
                + "the run diff needs the Rust-side `show --diff` Job delegation registered in CONTRACTS.md"
        }
        return "show --diff --json did not decode: \(error.localizedDescription)"
    }

    private enum CLIOutcome {
        case success(Data)
        case failure(String)
    }

    private func runCLI(_ arguments: [String], timeout: TimeInterval = 15)
        async -> CLIOutcome {
        do {
            let result = try await cli.run(arguments: arguments, timeout: timeout)
            guard result.exitCode == 0 else {
                let words = result.stderr.isEmpty ? result.stdout : result.stderr
                return .failure("deadreckon \(arguments.first ?? "") exited \(result.exitCode): \(words.prefix(300))")
            }
            return .success(Data(result.stdout.utf8))
        } catch {
            let reason = (error as? FleetCLIError)?.errorDescription
                ?? error.localizedDescription
            return .failure(reason)
        }
    }

    // MARK: - JSON file helpers

    private enum JSONReadOutcome<T> {
        case absent
        case value(T)
        case unreadable(String)
    }

    /// Typed read distinguishing "file does not exist" (an honest absence)
    /// from "exists but unreadable/undecodable" (a transient to ride out
    /// with the last good value, never a fabricated absence).
    private func readJSONOutcome<T: Decodable>(_ url: URL) -> JSONReadOutcome<T> {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            let nsError = error as NSError
            if nsError.domain == NSCocoaErrorDomain,
               nsError.code == NSFileReadNoSuchFileError || nsError.code == NSFileNoSuchFileError {
                return .absent
            }
            return .unreadable(nsError.localizedDescription)
        }
        do {
            return .value(try DeadreckonJSON.decoder().decode(T.self, from: data))
        } catch {
            return .unreadable("did not decode: \(error.localizedDescription)")
        }
    }

    private func readJSON<T: Decodable>(_ url: URL) -> T? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? DeadreckonJSON.decoder().decode(T.self, from: data)
    }

    private func readRawJSON(_ url: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }
}
