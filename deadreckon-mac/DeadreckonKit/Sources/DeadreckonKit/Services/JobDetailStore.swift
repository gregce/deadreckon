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
    /// The newest this-many Activity entries keep their verbatim raw line
    /// for the inline raw-record inspector (VIZ-DRILLDOWN-SPEC K10); older
    /// entries' raw drops to nil and the drill names events.jsonl on disk.
    /// The parsed scrollback itself stays unbounded — this ceiling bounds
    /// only duplicated raw bytes.
    public static let rawInspectorCeiling = 4_000
    /// The newest this-many trace entries keep their verbatim ledger line
    /// for the tool-I/O drill (trace lines are large — ~11 KB each — but
    /// arrive ~2/turn; 1,000 covers a 500-turn run at ~11 MB worst case).
    public static let traceRawCeiling = 1_000

    // MARK: Read-model surface

    public struct ActivityEntry: Equatable, Sendable, Identifiable {
        public let ordinal: Int
        public let timestamp: Date?
        public let line: String
        /// The verbatim events.jsonl line, retained for the newest
        /// `rawInspectorCeiling` entries (K10); nil past the ceiling.
        public internal(set) var raw: String?
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
    /// K1: the burn-strip series (loop rows only, folded off-main).
    @Published public private(set) var spendSeries = SpendSeries()
    /// K2: the Activity density series (event stamps, folded off-main).
    @Published public private(set) var density = DensitySeries()
    /// K7/K8: the narrative architecture graph — plan-scope
    /// (`plans/<jobID>/…`) preferred over run-scope; mtime-cached read.
    @Published public private(set) var archGraph: ArchitectureGraphDoc?
    /// Set when a graph file exists but did not decode this poll; the last
    /// good doc is KEPT (the projection.json pattern).
    @Published public private(set) var archGraphIssue: String?
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

    /// APP-4: typed steer_delivered events observed in this attempt's events
    /// tail, for the queued -> delivered chip (SteerDeliveryTracker matches
    /// on queued_at). Per-run, reset with the scrollback.
    ///
    /// NOTE deliberately absent: no terminal-job-event surface lives here.
    /// Kill resolution belongs to KillCoordinator's dispatch-primed
    /// sheet-scoped tail alone — one resolution source, so no future caller
    /// can bind kill state to this store's full-history, per-attempt tail.
    @Published public private(set) var steerDeliveries: [SteerDeliveredFact] = []

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
    /// Serializes ticks (loop and direct test calls alike) so the
    /// single-owner tailer contract holds across the off-main-actor reads.
    private var pollChain: Task<Void, Never>?
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
    private var pendingTurnTraces: [(row: TraceRow, raw: String?)] = []
    private var turnsAccumulator = TurnsDerivation.Accumulator(
        traceRawCeiling: JobDetailStore.traceRawCeiling)
    /// K2 fold state; the fold itself runs in the stage-2 detached hop.
    private var densityAccumulator = DensitySeries.Accumulator()
    /// Index below which Activity entries have already had raw trimmed
    /// (K10): the trim walk never rescans the unbounded scrollback.
    private var activityRawTrimIndex = 0
    /// Mtime cache for the architecture-graph read (the checkpoints-cache
    /// pattern): an unchanged mtime on the same path skips the read.
    private var archGraphCache: (path: String, mtime: Date)?
    private var lastEventTimestamp: Date?
    private var newestErrorMessage: String?
    private var steerInboxCache: (size: UInt64, count: Int)?
    /// Mtime-keyed (the steerInboxCache pattern): checkpoint creation adds
    /// a subdirectory, bumping the directory mtime, so an unchanged mtime
    /// skips up to 60 manifest re-reads per tick.
    private var checkpointsCache: (mtime: Date, values: [CheckpointManifestDoc])?
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
        turnsAccumulator = TurnsDerivation.Accumulator(traceRawCeiling: Self.traceRawCeiling)
        densityAccumulator = DensitySeries.Accumulator()
        spendSeries = SpendSeries()
        density = DensitySeries()
        archGraph = nil
        archGraphIssue = nil
        archGraphCache = nil
        activityRawTrimIndex = 0
        lastEventTimestamp = nil
        newestErrorMessage = nil
        steerInboxCache = nil
        checkpointsCache = nil
        supervisorOut = ""
        supervisorErr = ""
        supervisorOutTruncated = false
        supervisorErrTruncated = false
        integrity = .none
        jobEventsTornTail = false
        steerDeliveries = []
    }

    // MARK: - One tick (public for deterministic tests)

    /// One tick, serialized through `pollChain`: the loop and any direct
    /// test call run strictly one after another, so the single-owner tailer
    /// contract holds across the off-main-actor reads. The generation is
    /// captured at CALL time, so a tick queued behind an in-flight one when
    /// close() lands becomes a no-op instead of launching a CLI child after
    /// close.
    public func pollOnce() async {
        let myGeneration = generation
        let previous = pollChain
        let task = Task { [weak self] in
            await previous?.value
            await self?.performPoll(myGeneration: myGeneration)
        }
        pollChain = task
        await task.value
    }

    private func performPoll(myGeneration: Int) async {
        guard generation == myGeneration else { return }

        // Stage 1, off the main actor: the job-level file reads.
        let jobDir = self.jobDir
        let jobReads = await Task.detached(priority: .userInitiated) {
            () -> (projection: JSONReadOutcome<JobProjectionDoc>, lease: JobLease?) in
            (Self.readJSONOutcome(jobDir.appendingPathComponent("projection.json")),
             Self.readJSON(jobDir.appendingPathComponent("lease.json")))
        }.value
        guard generation == myGeneration else { return }
        applyProjectionOutcome(jobReads.projection)
        lease = jobReads.lease
        resolveRunRootIfNeeded()

        // Stage 2, off the main actor: run-file reads, every tail poll, and
        // all per-line JSONL decoding in one detached hop. Only derivation
        // and the @Published assignments run back on the main actor — a
        // multi-MB ledger's first read never beachballs workbench open.
        let reads = await gatherTickReads()
        guard generation == myGeneration else { return }
        apply(reads)
        deriveSpine(hasReshape: reads.hasReshape,
                    pendingSteerCount: reads.pendingSteerCount)
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
        turnsAccumulator = TurnsDerivation.Accumulator(traceRawCeiling: Self.traceRawCeiling)
        densityAccumulator = DensitySeries.Accumulator()
        activityRawTrimIndex = 0
        archGraphCache = nil
        lastEventTimestamp = nil
        newestErrorMessage = nil
        steerInboxCache = nil
        checkpointsCache = nil
        activity = []
        rawEventLines = []
        rawEventsDropped = 0
        activityIssue = nil
        tracesIssue = nil
        spendIssue = nil
        flightIssue = nil
        turns = []
        spendMeter = SpendMeter()
        spendSeries = SpendSeries()
        density = DensitySeries()
        archGraph = nil
        archGraphIssue = nil
        flight = FlightState()
        narrative = NarrativePane()
        liveChecks = []
        changes = nil
        patches = [:]
        patchIssues = [:]
        steerDeliveries = []
    }

    // MARK: - File polls

    private func applyProjectionOutcome(_ outcome: JSONReadOutcome<JobProjectionDoc>) {
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

    // MARK: - The gathered tick (all file I/O and line decode off-main)

    /// Everything one stage-2 hop reads and decodes: run-root JSON docs,
    /// all nine tail polls with their lines already decoded, and the
    /// spine's file facts. Values only; the @Published assignments happen
    /// back on the main actor in `apply(_:)`.
    private func gatherTickReads() async -> DetailTickReads {
        let root = runRoot
        let runID = currentRunID
        let home = self.home
        let jobID = self.jobID
        let needsLaunchPlan = runID != nil && launchPlanCeilings?.runID != runID
        let priorLaunchPlan = launchPlanCeilings
        let priorCheckpointsCache = checkpointsCache
        let priorSteerCache = steerInboxCache
        let priorArchGraphCache = archGraphCache
        let priorSpendSeries = spendSeries
        let priorDensityAccumulator = densityAccumulator
        let jobEventsTailer = self.jobEventsTailer
        let eventsTailer = self.eventsTailer
        let tracesTailer = self.tracesTailer
        let spendTailer = self.spendTailer
        let flightEventsTailer = self.flightEventsTailer
        let snapshotsTailer = self.snapshotsTailer
        let progressTailer = self.progressTailer
        let supervisorOutTailer = self.supervisorOutTailer
        let supervisorErrTailer = self.supervisorErrTailer

        return await Task.detached(priority: .userInitiated) { () -> DetailTickReads in
            var reads = DetailTickReads()
            if let root {
                reads.hasRunRoot = true
                reads.runState = Self.readJSON(root.appendingPathComponent("state.json"))
                if needsLaunchPlan, let runID {
                    let plan = Self.readRawJSON(root.appendingPathComponent("launch-plan.json"))
                    let budget = plan?["budget"] as? [String: Any]
                    reads.launchPlanCeilings = (
                        runID: runID,
                        spendUSD: (budget?["ceiling_usd"] as? NSNumber)?.doubleValue,
                        wallSeconds: (budget?["wall_seconds"] as? NSNumber)?.doubleValue)
                } else {
                    reads.launchPlanCeilings = priorLaunchPlan
                }
                reads.narrativeStateDoc = Self.readJSON(
                    root.appendingPathComponent("narrative").appendingPathComponent("state.json"))
                reads.flightManifest = Self.readJSON(root.appendingPathComponent("flight-manifest.json"))
                let checkpoints = Self.listCheckpoints(root: root, cache: priorCheckpointsCache)
                reads.checkpoints = checkpoints.values
                reads.checkpointsCache = checkpoints.cache
                reads.docs = Self.listDocs(workingDir: reads.runState?.workingDir)
                reads.hasReshape = Self.hasReshapeProposal(root: root)
                let steers = Self.pendingSteers(root: root, cache: priorSteerCache)
                reads.pendingSteerCount = steers.count
                reads.steerInboxCache = steers.cache
            }
            if let tailer = jobEventsTailer {
                let poll = tailer.poll()
                reads.jobEvents = (poll, tailer.lastSequence, tailer.hasRetainedTail)
            }
            if let tailer = eventsTailer { reads.events = Self.decodeEventsPoll(tailer.poll()) }
            if let tailer = tracesTailer { reads.traces = Self.decodeTracesPoll(tailer.poll()) }
            if let tailer = spendTailer { reads.spend = Self.decodeSpendPoll(tailer.poll()) }
            if let tailer = flightEventsTailer {
                reads.flightEvents = Self.decodeFlightPoll(tailer.poll())
            }
            if let tailer = snapshotsTailer {
                reads.snapshots = Self.decodeSnapshotsPoll(tailer.poll())
            }
            if let tailer = progressTailer {
                reads.progress = Self.decodeProgressPoll(tailer.poll())
            }
            reads.supervisorOut = supervisorOutTailer?.poll()
            reads.supervisorErr = supervisorErrTailer?.poll()
            // Chart-series folds ride the same off-main hop (K8): only the
            // @Published assignment happens back on the main actor.
            if let spend = reads.spend, !spend.records.isEmpty {
                var series = priorSpendSeries
                series.fold(spend.records)
                reads.spendSeriesUpdate = series
            }
            if let events = reads.events {
                let pairs = events.rows.compactMap { row in
                    row.record.map { (timestamp: $0.timestamp, kind: $0.event.kind) }
                }
                if !pairs.isEmpty {
                    var accumulator = priorDensityAccumulator
                    let series = accumulator.fold(pairs)
                    reads.densityUpdate = (accumulator: accumulator, series: series)
                }
            }
            reads.archGraph = Self.readArchGraph(
                home: home, jobID: jobID, runRoot: root, cache: priorArchGraphCache)
            return reads
        }.value
    }

    /// Fold one tick's gathered values into the published state (main
    /// actor, O(new rows), no file I/O and no JSON parsing here).
    private func apply(_ reads: DetailTickReads) {
        if reads.hasRunRoot {
            runState = reads.runState
            launchPlanCeilings = reads.launchPlanCeilings
            checkpointsCache = reads.checkpointsCache
            steerInboxCache = reads.steerInboxCache
            docs = reads.docs

            var pane = narrative
            pane.stateDoc = reads.narrativeStateDoc
            pane.staleness = NarrativeStaleness.from(
                createdAt: pane.latestSnapshot?.createdAt ?? pane.stateDoc?.latestCreatedAt,
                now: nowProvider())
            narrative = pane

            var flightState = flight
            flightState.manifest = reads.flightManifest
            flightState.checkpoints = reads.checkpoints
            flight = flightState
        }
        if let jobEvents = reads.jobEvents {
            integrity = JobEventsIntegrity.derive(
                previous: integrity, poll: jobEvents.poll, lastSequence: jobEvents.lastSequence)
            jobEventsTornTail = jobEvents.tornTail
        }
        if let events = reads.events { applyEvents(events) }
        if let traces = reads.traces { applyTraces(traces) }
        if let spend = reads.spend { applySpend(spend) }
        if let series = reads.spendSeriesUpdate { spendSeries = series }
        if let densityUpdate = reads.densityUpdate {
            densityAccumulator = densityUpdate.accumulator
            density = densityUpdate.series
        }
        switch reads.archGraph {
        case .unchanged:
            break
        case .absent:
            // Honest absence: no MAP section (silence, not placeholder).
            archGraph = nil
            archGraphIssue = nil
            archGraphCache = nil
        case .value(let doc, let cache):
            archGraph = doc
            archGraphIssue = nil
            archGraphCache = cache
        case .unreadable(let reason):
            // Keep the last good doc; say why (the projection.json pattern).
            archGraphIssue = reason
        }
        if let flightEvents = reads.flightEvents { applyFlightEvents(flightEvents) }
        if let snapshots = reads.snapshots { applySnapshots(snapshots) }
        if let progress = reads.progress { applyProgress(progress) }
        applySupervisor(out: reads.supervisorOut, err: reads.supervisorErr)
    }

    private func applyEvents(_ payload: DetailEventsPayload) {
        if let corrupt = payload.corrupt {
            // Strict file: report, stop trusting; existing scrollback stays.
            activityIssue = corrupt
        }
        guard !payload.rows.isEmpty else { return }
        for row in payload.rows {
            rawEventLines.append(row.line)
            let ordinal = activity.count + 1
            if let record = row.record {
                pendingTurnEvents.append(record)
                lastEventTimestamp = record.timestamp
                if record.event.kind == "error" {
                    newestErrorMessage = record.event.message
                }
                if record.event.kind == "steer_delivered" {
                    steerDeliveries.append(SteerDeliveredFact(
                        turn: record.event.turn,
                        queuedAtRaw: record.event.queuedAt,
                        preview: record.event.preview))
                }
                activity.append(ActivityEntry(
                    ordinal: ordinal, timestamp: record.timestamp,
                    line: record.activityLine, raw: row.line))
            } else {
                // A schema-conformant ledger line we cannot model yet:
                // show the raw fact rather than dropping or guessing.
                activity.append(ActivityEntry(
                    ordinal: ordinal, timestamp: nil, line: row.line, raw: row.line))
            }
        }
        trimRawEventLines()
        trimActivityRaws()
    }

    private func trimRawEventLines() {
        guard rawEventLines.count > Self.rawEventLineCeiling + Self.rawEventLineSlack else { return }
        let overflow = rawEventLines.count - Self.rawEventLineCeiling
        rawEventLines.removeFirst(overflow)
        rawEventsDropped += overflow
    }

    /// K10: only the newest `rawInspectorCeiling` entries keep their raw
    /// copy; the walk starts where the last trim stopped (never a rescan).
    private func trimActivityRaws() {
        while activity.count - activityRawTrimIndex > Self.rawInspectorCeiling {
            activity[activityRawTrimIndex].raw = nil
            activityRawTrimIndex += 1
        }
    }

    private func applyTraces(_ payload: DetailTracesPayload) {
        if let corrupt = payload.corrupt { tracesIssue = corrupt }
        pendingTurnTraces.append(contentsOf: payload.rows.map { ($0.row, $0.raw) })
    }

    private func applySpend(_ payload: DetailSpendPayload) {
        if let corrupt = payload.corrupt { spendIssue = corrupt }
        guard !payload.records.isEmpty else { return }
        var meter = spendMeter
        for record in payload.records {
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

    private func applyFlightEvents(_ payload: DetailFlightPayload) {
        if let corrupt = payload.corrupt {
            flightIssue = corrupt
            return
        }
        guard payload.lineCount > 0 else { return }
        var flightState = flight
        flightState.eventCount += payload.lineCount
        if let summary = payload.lastSummary {
            flightState.lastEventSummary = summary
        }
        flight = flightState
    }

    private func applySnapshots(_ payload: DetailSnapshotsPayload) {
        var pane = narrative
        if payload.restarted {
            // The unblessed file was rewritten or shrank: re-fold the whole
            // fresh read instead of freezing (self-healing, still honest:
            // the pane reflects exactly what the file now says).
            pane.latestSnapshot = nil
            pane.latestDeterministic = nil
            pane.skippedMalformedRows = 0
        }
        for row in payload.rows {
            guard let snapshot = row else {
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
        narrative = pane
    }

    private func applyProgress(_ payload: DetailProgressPayload) {
        if payload.restarted {
            // New gate attempt or sign-time rewrite: discard retained rows,
            // the fresh read is the whole current attempt (TAILING.md rule).
            liveChecks = payload.rows
        } else {
            liveChecks.append(contentsOf: payload.rows)
        }
    }

    private func applySupervisor(out: TextFileTailer.PollResult?,
                                 err: TextFileTailer.PollResult?) {
        if let out {
            switch out {
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
        if let err {
            switch err {
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

    // MARK: - Off-main decode helpers (values in, values out)

    nonisolated private static func decodeEventsPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailEventsPayload? {
        switch poll {
        case .none, .restarted:
            return nil
        case .corrupt(let reason):
            return DetailEventsPayload(corrupt: reason, rows: [])
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            let rows = lines.map { line -> DetailEventsPayload.Row in
                guard let data = line.data(using: .utf8),
                      let record = try? decoder.decode(RunEventRecord.self, from: data) else {
                    return DetailEventsPayload.Row(line: line, record: nil)
                }
                return DetailEventsPayload.Row(line: line, record: record)
            }
            return DetailEventsPayload(corrupt: nil, rows: rows)
        }
    }

    nonisolated private static func decodeTracesPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailTracesPayload? {
        switch poll {
        case .none, .restarted:
            return nil
        case .corrupt(let reason):
            return DetailTracesPayload(corrupt: reason, rows: [])
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            let rows = lines.compactMap { line -> DetailTracesPayload.Row? in
                guard let data = line.data(using: .utf8),
                      let row = try? decoder.decode(TraceRow.self, from: data) else { return nil }
                // The source line rides along so trace entries can retain
                // their verbatim ledger line for the tool-I/O drill (K10).
                return DetailTracesPayload.Row(row: row, raw: line)
            }
            return DetailTracesPayload(corrupt: nil, rows: rows)
        }
    }

    nonisolated private static func decodeSpendPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailSpendPayload? {
        switch poll {
        case .none, .restarted:
            return nil
        case .corrupt(let reason):
            return DetailSpendPayload(corrupt: reason, records: [])
        case .lines(let lines):
            let decoder = DeadreckonJSON.decoder()
            let records = lines.compactMap { line -> SpendRecord? in
                guard let data = line.data(using: .utf8) else { return nil }
                return try? decoder.decode(SpendRecord.self, from: data)
            }
            return DetailSpendPayload(corrupt: nil, records: records)
        }
    }

    nonisolated private static func decodeFlightPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailFlightPayload? {
        switch poll {
        case .none, .restarted:
            return nil
        case .corrupt(let reason):
            return DetailFlightPayload(corrupt: reason)
        case .lines(let lines):
            var payload = DetailFlightPayload()
            payload.lineCount = lines.count
            if let last = lines.last,
               let data = last.data(using: .utf8),
               let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let summary = object["summary"] as? String {
                payload.lastSummary = summary
            }
            return payload
        }
    }

    nonisolated private static func decodeSnapshotsPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailSnapshotsPayload? {
        switch poll {
        case .none, .corrupt:
            // .corrupt cannot happen in restart-on-anomaly mode; ignore.
            return nil
        case .lines(let lines):
            return DetailSnapshotsPayload(restarted: false, rows: decodeSnapshotRows(lines))
        case .restarted(let lines):
            return DetailSnapshotsPayload(restarted: true, rows: decodeSnapshotRows(lines))
        }
    }

    nonisolated private static func decodeSnapshotRows(_ lines: [String]) -> [NarrativeSnapshotDoc?] {
        let decoder = DeadreckonJSON.decoder()
        return lines.map { line -> NarrativeSnapshotDoc? in
            guard let data = line.data(using: .utf8) else { return nil }
            return try? decoder.decode(NarrativeSnapshotDoc.self, from: data)
        }
    }

    nonisolated private static func decodeProgressPoll(
        _ poll: JSONLTailer.PollResult
    ) -> DetailProgressPayload? {
        switch poll {
        case .none, .corrupt:
            // .corrupt cannot happen in acceptanceProgress mode; ignore.
            return nil
        case .lines(let lines):
            return DetailProgressPayload(restarted: false, rows: decodeProgress(lines))
        case .restarted(let lines):
            return DetailProgressPayload(restarted: true, rows: decodeProgress(lines))
        }
    }

    nonisolated private static func decodeProgress(_ lines: [String]) -> [AcceptanceProgressRow] {
        let decoder = DeadreckonJSON.decoder()
        return lines.compactMap { line in
            guard let data = line.data(using: .utf8) else { return nil }
            return try? decoder.decode(AcceptanceProgressRow.self, from: data)
        }
    }

    // MARK: - Off-main file facts

    nonisolated private static func listCheckpoints(
        root: URL, cache: (mtime: Date, values: [CheckpointManifestDoc])?
    ) -> (values: [CheckpointManifestDoc], cache: (mtime: Date, values: [CheckpointManifestDoc])?) {
        let dir = root.appendingPathComponent("checkpoints")
        let mtime = (try? FileManager.default.attributesOfItem(atPath: dir.path))?[
            .modificationDate] as? Date
        // Mtime-keyed cache (the steerInboxCache pattern): a new checkpoint
        // adds a subdirectory and bumps the directory mtime, so an unchanged
        // mtime skips up to 60 manifest re-reads per tick.
        if let cache, let mtime, cache.mtime == mtime {
            return (cache.values, cache)
        }
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else {
            return ([], nil)
        }
        let considered = names.sorted().suffix(60)
        let values = considered.compactMap { name in
            Self.readJSON(dir.appendingPathComponent(name).appendingPathComponent("manifest.json"))
                as CheckpointManifestDoc?
        }
        // Cache only a fully-decoded listing: a checkpoint whose manifest is
        // still landing keeps being re-read until it decodes (the directory
        // mtime moved at subdirectory creation, not at manifest write).
        if let mtime, values.count == considered.count {
            return (values, (mtime, values))
        }
        return (values, nil)
    }

    /// K8: the architecture-graph read. Plan scope
    /// (`plans/<jobID>/narrative/architecture-graph.json`, driver jobs)
    /// wins over run scope (`<runRoot>/narrative/architecture-graph.json`);
    /// an unchanged mtime on the same path skips the read entirely.
    nonisolated private static func readArchGraph(
        home: URL, jobID: String, runRoot: URL?,
        cache: (path: String, mtime: Date)?
    ) -> DetailTickReads.GraphRead {
        let planURL = home.appendingPathComponent("plans")
            .appendingPathComponent(jobID)
            .appendingPathComponent("narrative")
            .appendingPathComponent("architecture-graph.json")
        var candidates = [planURL]
        if let runRoot {
            candidates.append(runRoot.appendingPathComponent("narrative")
                .appendingPathComponent("architecture-graph.json"))
        }
        for url in candidates {
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else {
                continue
            }
            let mtime = attributes[.modificationDate] as? Date
            if let cache, let mtime, cache.path == url.path, cache.mtime == mtime {
                return .unchanged
            }
            guard let data = try? Data(contentsOf: url) else {
                return .unreadable("architecture-graph.json exists but could not be read this poll")
            }
            do {
                let doc = try DeadreckonJSON.decoder().decode(ArchitectureGraphDoc.self, from: data)
                return .value(doc, cache: mtime.map { (url.path, $0) })
            } catch {
                return .unreadable("architecture-graph.json did not decode: \(error.localizedDescription)")
            }
        }
        return .absent
    }

    nonisolated private static func listDocs(workingDir: String?) -> [DocEntry] {
        guard let workingDir, !workingDir.isEmpty else { return [] }
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

    /// Main-actor derivation over this tick's gathered file facts (the
    /// reshape presence and pending-steer count were read off-main).
    private func deriveSpine(hasReshape: Bool, pendingSteerCount: Int) {
        guard let state = runState else {
            spine = nil
            return
        }
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
            pendingSteerCount: pendingSteerCount)
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

    nonisolated private static func hasReshapeProposal(root: URL) -> Bool {
        let url = root.appendingPathComponent("reshape-proposal.json")
        guard let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data) else { return false }
        return object is [String: Any]
    }

    nonisolated private static func pendingSteers(
        root: URL, cache: (size: UInt64, count: Int)?
    ) -> (count: Int, cache: (size: UInt64, count: Int)?) {
        let url = root.appendingPathComponent("steer-inbox.jsonl")
        guard let size = ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size]
            as? NSNumber)?.uint64Value else {
            return (0, nil)
        }
        // Size-keyed cache: the inbox is append-only and status flips change
        // the byte count, so an unchanged size skips the full re-read.
        if let cache, cache.size == size { return (cache.count, cache) }
        guard let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .utf8) else { return (0, nil) }
        let count = text.split(separator: "\n").reduce(into: 0) { count, line in
            guard let lineData = line.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: lineData) as? [String: Any],
                  (object["status"] as? String) == "pending" else { return }
            count += 1
        }
        return (count, (size, count))
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

    // MARK: - JSON file helpers (nonisolated: they run in the detached hops)

    /// Typed read distinguishing "file does not exist" (an honest absence)
    /// from "exists but unreadable/undecodable" (a transient to ride out
    /// with the last good value, never a fabricated absence).
    nonisolated private static func readJSONOutcome<T: Decodable>(_ url: URL) -> JSONReadOutcome<T> {
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

    nonisolated private static func readJSON<T: Decodable>(_ url: URL) -> T? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? DeadreckonJSON.decoder().decode(T.self, from: data)
    }

    nonisolated private static func readRawJSON(_ url: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }
}

// MARK: - Tick payloads (file scope: constructed inside the detached hops,
// so they carry no main-actor isolation)

private enum JSONReadOutcome<T> {
    case absent
    case value(T)
    case unreadable(String)
}

/// One stage-2 hop's gathered values (see JobDetailStore.gatherTickReads).
private struct DetailTickReads {
    /// Architecture-graph read outcome (K8): unchanged = mtime cache hit.
    enum GraphRead {
        case unchanged
        case absent
        case value(ArchitectureGraphDoc, cache: (path: String, mtime: Date)?)
        case unreadable(String)
    }

    var hasRunRoot = false
    var runState: RunStateDoc?
    var launchPlanCeilings: (runID: String, spendUSD: Double?, wallSeconds: Double?)?
    var narrativeStateDoc: NarrativeStateDoc?
    var flightManifest: FlightManifestDoc?
    var checkpoints: [CheckpointManifestDoc] = []
    var checkpointsCache: (mtime: Date, values: [CheckpointManifestDoc])?
    var docs: [DocEntry] = []
    var hasReshape = false
    var pendingSteerCount = 0
    var steerInboxCache: (size: UInt64, count: Int)?
    var jobEvents: (poll: JSONLTailer.PollResult, lastSequence: UInt64, tornTail: Bool)?
    var events: DetailEventsPayload?
    var traces: DetailTracesPayload?
    var spend: DetailSpendPayload?
    var spendSeriesUpdate: SpendSeries?
    var densityUpdate: (accumulator: DensitySeries.Accumulator, series: DensitySeries)?
    var archGraph: GraphRead = .unchanged
    var flightEvents: DetailFlightPayload?
    var snapshots: DetailSnapshotsPayload?
    var progress: DetailProgressPayload?
    var supervisorOut: TextFileTailer.PollResult?
    var supervisorErr: TextFileTailer.PollResult?
}

private struct DetailEventsPayload {
    struct Row {
        let line: String
        /// nil when the line does not model yet: shown raw rather than
        /// dropped or guessed.
        let record: RunEventRecord?
    }

    var corrupt: String?
    var rows: [Row] = []
}

private struct DetailTracesPayload {
    struct Row {
        let row: TraceRow
        /// The verbatim source line (rides into the turns accumulator for
        /// the K10 trace-raw retention).
        let raw: String?
    }

    var corrupt: String?
    var rows: [Row] = []
}

private struct DetailSpendPayload {
    var corrupt: String?
    var records: [SpendRecord] = []
}

private struct DetailFlightPayload {
    var corrupt: String?
    var lineCount = 0
    var lastSummary: String?
}

private struct DetailSnapshotsPayload {
    var restarted = false
    /// nil element = malformed row (counted, never guessed at).
    var rows: [NarrativeSnapshotDoc?] = []
}

private struct DetailProgressPayload {
    var restarted = false
    var rows: [AcceptanceProgressRow] = []
}
