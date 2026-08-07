import Foundation

/// The fleet read-model for the Gate Queue home and the menubar: polls
/// `deadreckon list --all --json` (fast cadence while the window shows,
/// slow when only the menubar does), listens to FSEvents over
/// DEADRECKON_HOME/jobs as a wake-up hint, and polls the Harbor surfaces
/// (`providers list --json`, `supervisor status --json`, `--version`) at a
/// low cadence.
///
/// Everything rendered from here is a durable fact off the M0 rollup row or
/// an honest "unknown"/"unavailable". Degradation ladder: binary missing ->
/// typed unavailable state (never a crash, never fake rows); one row failing
/// to decode -> that row quarantined, siblings survive; a chip surface
/// failing -> that chip says unknown.
@MainActor
public final class FleetStore: ObservableObject {
    // MARK: Cadence

    /// Poll intervals in seconds, injectable for tests. Defaults per the
    /// design doc: ~2s while the window is visible, slower when only the
    /// menubar shows, 60s for the Harbor strip.
    public struct Cadence: Equatable, Sendable {
        public var windowVisible: TimeInterval
        public var menubarOnly: TimeInterval
        public var harbor: TimeInterval

        public init(windowVisible: TimeInterval = 2, menubarOnly: TimeInterval = 10, harbor: TimeInterval = 60) {
            self.windowVisible = windowVisible
            self.menubarOnly = menubarOnly
            self.harbor = harbor
        }

        public static let standard = Cadence()
    }

    // MARK: Fleet state

    public enum FleetState: Equatable {
        /// Nothing fetched yet this session.
        case loading
        /// The latest poll decoded; the queue is current as of `lastRefreshed`.
        case loaded(GateQueue)
        /// The typed no-fake-rows state: binary missing, launch failure,
        /// nonzero list exit, or an undecodable envelope. The reason is the
        /// error's own words, rendered verbatim.
        case unavailable(reason: String)
    }

    // MARK: Harbor state

    public struct HarborState: Equatable {
        public enum Providers: Equatable {
            case unknown(String)
            case counted(ok: Int, total: Int)
        }

        public enum Supervisor: Equatable {
            case unknown(String)
            case running
            case stopped(SupervisorInstallState)
        }

        public enum Doctor: Equatable {
            case unknown(String)
            case ok(warnings: Int)
            case failed(Int)
        }

        public var providers: Providers
        public var supervisor: Supervisor
        public var doctor: Doctor

        public init(providers: Providers, supervisor: Supervisor, doctor: Doctor) {
            self.providers = providers
            self.supervisor = supervisor
            self.doctor = doctor
        }

        public static let initial = HarborState(
            providers: .unknown("not yet polled"),
            supervisor: .unknown("not yet polled"),
            doctor: .unknown("not yet polled"))
    }

    // MARK: Published state

    @Published public private(set) var fleet: FleetState = .loading
    @Published public private(set) var harbor: HarborState = .initial
    @Published public private(set) var lastRefreshed: Date?
    @Published public private(set) var legacyRunCount = 0
    @Published public private(set) var binaryVersion: String?

    /// Jobs whose lease reported `fresh == false` for at least
    /// `staleLeaseConfirmationPolls` consecutive polls of the same epoch.
    /// This is the debounce the design doc demands: the rollup computes
    /// freshness at CLI-invocation time, so the first poll after Mac
    /// sleep/wake reports a huge heartbeat age until the supervisor's next
    /// beat. One raw stale poll never paints amber; persistence across a
    /// full poll interval does. An epoch change resets the streak.
    @Published public private(set) var confirmedStaleLeaseJobIDs: Set<String> = []

    /// Consecutive stale polls (same job, same epoch) before amber.
    public static let staleLeaseConfirmationPolls = 2

    public private(set) var windowVisible = false

    public var menuBarState: MenuBarFleetState {
        QueueDerivation.menuBarState(
            fleet,
            staleLeaseCount: confirmedStaleLeaseJobIDs.count,
            supervisorDown: supervisorDown)
    }

    /// True only for a positively-reported stopped Watchkeeper; an unknown
    /// chip stays a quiet chip and never badges the menubar.
    public var supervisorDown: Bool {
        if case .stopped = harbor.supervisor { return true }
        return false
    }

    /// Children still running (quit-time teardown polls this so the app
    /// outlives its children or their SIGKILL escalation).
    public var inFlightChildren: Int { cli.inFlightCount }

    /// The current queue when loaded, else empty. Views that need rows.
    public var queue: GateQueue {
        if case .loaded(let queue) = fleet { return queue }
        return .empty
    }

    // MARK: Dependencies

    private let cli: FleetCLIRunning
    private let watcher: FleetWatching?
    private let cadence: Cadence
    private let listTimeout: TimeInterval = 30

    private var pollTask: Task<Void, Never>?
    private var harborTask: Task<Void, Never>?
    private var refreshInFlight = false
    private var refreshQueued = false
    private var started = false
    /// Set by shutdown(): after the SIGTERM sweep no code path may launch a
    /// fresh child (an in-flight refresh that resumes, a queued follow-up,
    /// or a mid-sweep Harbor stage).
    private var tornDown = false
    /// Per-job stale-lease streaks backing `confirmedStaleLeaseJobIDs`.
    private var staleLeaseStreaks: [String: (epoch: Int, polls: Int)] = [:]

    public init(cli: FleetCLIRunning, watcher: FleetWatching?, cadence: Cadence = .standard) {
        self.cli = cli
        self.watcher = watcher
        self.cadence = cadence
    }

    /// Production wiring: vendored binary + FSEvents over DEADRECKON_HOME/jobs.
    public convenience init() {
        self.init(cli: DeadreckonCLIClient(), watcher: JobsDirectoryWatcher())
    }

    // MARK: Lifecycle

    public func start() {
        guard !started else { return }
        started = true
        watcher?.start(onChange: makeWatcherHint())
        schedulePolling()
        scheduleHarborPolling()
    }

    /// FSEvents delivers on its own queue; state lives on the main actor.
    /// The hint coalesces into refreshNow's in-flight guard.
    private func makeWatcherHint() -> () -> Void {
        { [weak self] in
            Task { @MainActor [weak self] in
                await self?.refreshNow()
            }
        }
    }

    public func stop() {
        started = false
        pollTask?.cancel()
        pollTask = nil
        harborTask?.cancel()
        harborTask = nil
        watcher?.stop()
    }

    /// Quit-time teardown (exemplar applicationShouldTerminate pattern):
    /// stop polling, fence off every child-launching path, then SIGTERM
    /// in-flight children with patience so quit never orphans a child.
    /// Returns how many children were signaled; the caller must then keep
    /// the process alive until `inFlightChildren == 0` or past `patience`
    /// (so the SIGKILL escalation can fire for a SIGTERM-ignoring child).
    @discardableResult
    public func shutdown(patience: TimeInterval = 2) -> Int {
        tornDown = true
        stop()
        return cli.terminateInFlight(patience: patience)
    }

    /// Window visibility drives the poll cadence; becoming visible also
    /// triggers an immediate refresh so the queue is current when seen.
    public func setWindowVisible(_ visible: Bool) {
        guard windowVisible != visible else { return }
        windowVisible = visible
        guard started else { return }
        schedulePolling()
    }

    // MARK: Fleet polling

    private func schedulePolling() {
        pollTask?.cancel()
        let interval = windowVisible ? cadence.windowVisible : cadence.menubarOnly
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                // Re-bind self per iteration and release it before sleeping:
                // a store discarded without stop() must not keep an immortal
                // sleep loop alive (the loop exits on the next wake instead).
                do {
                    guard let self else { return }
                    await self.refreshNow()
                }
                do {
                    try await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                } catch {
                    return
                }
            }
        }
    }

    /// One `list --all --json` poll. Reentrancy coalesces: a call while a
    /// refresh is in flight queues exactly one follow-up so an FSEvents
    /// burst cannot stack child processes. After shutdown() this is a no-op:
    /// nothing may launch a child once the SIGTERM sweep ran.
    public func refreshNow() async {
        guard !tornDown else { return }
        if refreshInFlight {
            refreshQueued = true
            return
        }
        refreshInFlight = true
        defer { refreshInFlight = false }

        await pollFleetOnce()

        if refreshQueued {
            refreshQueued = false
            guard !tornDown else { return }
            await pollFleetOnce()
        }
    }

    private func pollFleetOnce() async {
        do {
            let result = try await cli.run(arguments: ["list", "--all", "--json"], timeout: listTimeout)
            guard result.exitCode == 0 else {
                let firstError = result.stderr.split(separator: "\n").first.map(String.init)
                fleet = .unavailable(
                    reason: "deadreckon list exited \(result.exitCode)"
                        + (firstError.map { ": \($0)" } ?? ""))
                return
            }
            let decoded = try FleetDecoder.decodeList(Data(result.stdout.utf8))
            fleet = .loaded(QueueDerivation.derive(rows: decoded.rows, quarantined: decoded.quarantined))
            legacyRunCount = decoded.runs.count
            lastRefreshed = Date()
            updateStaleLeaseStreaks(rows: decoded.rows)
            retryWatcherIfInert()
        } catch let error as FleetCLIError {
            fleet = .unavailable(reason: error.errorDescription ?? "deadreckon binary unavailable")
        } catch let error as FleetDecodeError {
            fleet = .unavailable(reason: error.errorDescription ?? "list --json did not decode")
        } catch {
            fleet = .unavailable(reason: (error as? LocalizedError)?.errorDescription ?? String(describing: error))
        }
    }

    /// The stale-lease debounce (design doc Bridge risk: "if operators learn
    /// to ignore amber, the attention product is dead"). A lease counts as
    /// confirmed stale only after `staleLeaseConfirmationPolls` consecutive
    /// stale reports for the same epoch; a fresh report, a vanished lease,
    /// or an epoch change resets the streak.
    private func updateStaleLeaseStreaks(rows: [FleetRow]) {
        var next: [String: (epoch: Int, polls: Int)] = [:]
        for row in rows {
            guard let lease = row.lease, !lease.fresh else { continue }
            if let prior = staleLeaseStreaks[row.jobID], prior.epoch == lease.epoch {
                next[row.jobID] = (epoch: lease.epoch, polls: prior.polls + 1)
            } else {
                next[row.jobID] = (epoch: lease.epoch, polls: 1)
            }
        }
        staleLeaseStreaks = next
        let confirmed = Set(
            next.filter { $0.value.polls >= Self.staleLeaseConfirmationPolls }.map(\.key))
        if confirmed != confirmedStaleLeaseJobIDs {
            confirmedStaleLeaseJobIDs = confirmed
        }
    }

    /// A watcher started before the first job exists stays inert (jobs/ was
    /// absent). Retry after successful polls so FSEvents hints kick in the
    /// moment the directory appears, instead of degrading the whole session
    /// to the slow cadence.
    private func retryWatcherIfInert() {
        guard started, !tornDown, let watcher, !watcher.isActive else { return }
        watcher.start(onChange: makeWatcherHint())
    }

    // MARK: Harbor polling

    private func scheduleHarborPolling() {
        harborTask?.cancel()
        let interval = cadence.harbor
        harborTask = Task { [weak self] in
            while !Task.isCancelled {
                // Same discipline as the fleet loop: never outlive the store.
                do {
                    guard let self else { return }
                    await self.refreshHarborNow()
                }
                do {
                    try await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                } catch {
                    return
                }
            }
        }
    }

    /// One low-frequency Harbor sweep. Each chip degrades independently:
    /// a failed surface reads "unknown" with the failure's own words
    /// (a timeout says timeout, a decode failure says decode), never a
    /// guessed count. Each stage re-checks the teardown fence so a sweep in
    /// flight during quit cannot launch a child after the SIGTERM sweep.
    public func refreshHarborNow() async {
        guard !tornDown else { return }
        var next = harbor

        do {
            let result = try await cli.run(arguments: ["providers", "list", "--json"], timeout: listTimeout)
            if result.exitCode == 0 {
                let envelope = try DeadreckonJSON.decoder()
                    .decode(ProvidersEnvelope.self, from: Data(result.stdout.utf8))
                next.providers = .counted(ok: envelope.okCount, total: envelope.totalCount)
            } else {
                next.providers = .unknown("providers list exited \(result.exitCode)")
            }
        } catch let error as FleetCLIError {
            next.providers = .unknown(error.errorDescription ?? "binary unavailable")
        } catch is DecodingError {
            next.providers = .unknown("providers list --json did not decode")
        } catch {
            next.providers = .unknown(
                (error as? LocalizedError)?.errorDescription ?? String(describing: error))
        }

        guard !tornDown else { return }
        do {
            let result = try await cli.run(arguments: ["supervisor", "status", "--json"], timeout: listTimeout)
            if result.exitCode == 0 {
                let report = try DeadreckonJSON.decoder()
                    .decode(SupervisorStatusReport.self, from: Data(result.stdout.utf8))
                next.supervisor = report.isRunning ? .running : .stopped(report.installed)
            } else {
                next.supervisor = .unknown("supervisor status exited \(result.exitCode)")
            }
        } catch let error as FleetCLIError {
            next.supervisor = .unknown(error.errorDescription ?? "binary unavailable")
        } catch is DecodingError {
            next.supervisor = .unknown("supervisor status --json did not decode")
        } catch {
            next.supervisor = .unknown(
                (error as? LocalizedError)?.errorDescription ?? String(describing: error))
        }

        guard !tornDown else { return }
        do {
            // Doctor exits nonzero when checks fail but still prints the
            // envelope; decode stdout first and only fall back to the exit
            // code when there is nothing to decode.
            let result = try await cli.run(arguments: ["doctor", "--json"], timeout: listTimeout)
            if let envelope = try? DeadreckonJSON.decoder()
                .decode(DoctorEnvelope.self, from: Data(result.stdout.utf8))
            {
                next.doctor = envelope.failedCount > 0
                    ? .failed(envelope.failedCount)
                    : .ok(warnings: envelope.warningCount)
            } else {
                next.doctor = .unknown("doctor exited \(result.exitCode) without a decodable report")
            }
        } catch let error as FleetCLIError {
            next.doctor = .unknown(error.errorDescription ?? "binary unavailable")
        } catch {
            next.doctor = .unknown(
                (error as? LocalizedError)?.errorDescription ?? String(describing: error))
        }

        harbor = next

        if binaryVersion == nil, !tornDown {
            if let result = try? await cli.run(arguments: ["--version"], timeout: 10),
                result.exitCode == 0,
                let line = result.stdout.split(separator: "\n").first
            {
                binaryVersion = String(line)
            }
        }
    }
}
