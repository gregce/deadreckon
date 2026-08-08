import XCTest

@testable import DeadreckonKit

// MARK: - Fakes

/// Scriptable FleetCLIRunning: routes on the leading argument, records every
/// call, and never touches a real process.
final class FakeCLI: FleetCLIRunning {
    enum Script {
        case result(CLIRunResult)
        case failure(Error)
    }

    private let lock = NSLock()
    private var scripts: [String: Script] = [:]
    private(set) var calls: [[String]] = []
    /// stdin payload per call, aligned with `calls` (nil for the read
    /// verbs). Lets the secret-over-stdin tests prove the bytes reached
    /// the CLI seam without ever putting them in argv.
    private(set) var stdins: [Data?] = []
    private(set) var terminateCalls: [TimeInterval] = []

    func script(_ leadingArgument: String, _ script: Script) {
        lock.lock()
        scripts[leadingArgument] = script
        lock.unlock()
    }

    func scriptStdout(_ leadingArgument: String, _ stdout: String, exitCode: Int32 = 0) {
        script(leadingArgument, .result(CLIRunResult(stdout: stdout, stderr: "", exitCode: exitCode)))
    }

    func callCount(leading: String) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return calls.filter { $0.first == leading }.count
    }

    func run(arguments: [String], timeout: TimeInterval, stdin: Data?) async throws -> CLIRunResult {
        lock.lock()
        calls.append(arguments)
        stdins.append(stdin)
        let script = scripts[arguments.first ?? ""]
        lock.unlock()
        switch script {
        case .result(let result):
            return result
        case .failure(let error):
            throw error
        case nil:
            return CLIRunResult(stdout: "", stderr: "unscripted: \(arguments)", exitCode: 2)
        }
    }

    @discardableResult
    func terminateInFlight(patience: TimeInterval) -> Int {
        lock.lock()
        terminateCalls.append(patience)
        lock.unlock()
        return 0
    }

    var inFlightCount: Int { 0 }
}

/// A CLI that parks calls mid-flight until `release()`: the double-submit
/// guard tests suspend the first dispatch here and prove a second dispatch
/// never reaches the binary. `serveImmediately` scripts set-up legs (e.g. a
/// launch preview) that must complete without gating.
final class GatedCLI: FleetCLIRunning, @unchecked Sendable {
    private let lock = NSLock()
    private(set) var calls: [[String]] = []
    private var immediate: [CLIRunResult] = []
    private var waiters: [CheckedContinuation<Void, Never>] = []
    var gatedResult = CLIRunResult(stdout: "", stderr: "", exitCode: 0)

    /// How many calls are currently suspended at the gate.
    var parkedCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return waiters.count
    }

    /// The next `times` calls return `result` without gating.
    func serveImmediately(_ result: CLIRunResult, times: Int = 1) {
        lock.lock()
        immediate.append(contentsOf: Array(repeating: result, count: times))
        lock.unlock()
    }

    func release() {
        lock.lock()
        let parked = waiters
        waiters = []
        lock.unlock()
        parked.forEach { $0.resume() }
    }

    func run(arguments: [String], timeout: TimeInterval, stdin: Data?) async throws -> CLIRunResult {
        lock.lock()
        calls.append(arguments)
        if !immediate.isEmpty {
            let result = immediate.removeFirst()
            lock.unlock()
            return result
        }
        lock.unlock()
        await withCheckedContinuation { continuation in
            lock.lock()
            waiters.append(continuation)
            lock.unlock()
        }
        return gatedResult
    }

    @discardableResult
    func terminateInFlight(patience: TimeInterval) -> Int { 0 }

    var inFlightCount: Int { 0 }
}

final class FakeWatcher: FleetWatching {
    private(set) var startCount = 0
    private(set) var stopCount = 0
    private var onChange: (() -> Void)?

    func start(onChange: @escaping () -> Void) {
        startCount += 1
        self.onChange = onChange
    }

    func stop() {
        stopCount += 1
        onChange = nil
    }

    func fire() {
        onChange?()
    }

    var isActive: Bool { onChange != nil }
}

/// A watcher whose start is always a no-op (the jobs directory does not
/// exist yet): isActive stays false so the store should retry on each
/// successful poll.
final class InertWatcher: FleetWatching {
    private(set) var startCount = 0
    private(set) var stopCount = 0

    func start(onChange: @escaping () -> Void) { startCount += 1 }
    func stop() { stopCount += 1 }
    var isActive: Bool { false }
}

// MARK: - Fixtures

private let listFixture = """
    {
      "kind": "list", "id": "all", "status": "ok",
      "next_actions": [], "try_lines": [],
      "jobs": [
        {"job_id": "job-gate", "scope": "s", "goal": "Ship the ledger", "status": "verified",
         "updated_at": "2026-08-06T07:42:00Z", "attempts": 4,
         "outcome": "verified", "stop_reason": "verified",
         "projection": {"phase": "terminal", "outcome": "verified", "stop_reason": "verified",
                        "attempt_count": 4, "caveats": []},
         "lease": null,
         "spend": {"total_cost_usd": 9.12, "cap_usd": 25.0, "subscription": false, "wall_time_seconds": 22440.0},
         "provider": "cli:claude-code", "sandbox": "seatbelt",
         "receipt": {"present": true, "verified": "valid", "error": null},
         "gate": {"attempt": 4, "n_passed": 5, "n_total": 5}},
        {"job_id": "job-running", "scope": "s", "goal": "Fuzz the tailer", "status": "running",
         "updated_at": "2026-08-06T09:40:00Z", "attempts": 1,
         "outcome": null, "stop_reason": null,
         "projection": {"phase": "running", "outcome": null, "stop_reason": null,
                        "attempt_count": 1, "caveats": []},
         "lease": {"owner_id": "sup", "epoch": 2, "heartbeat_age_seconds": 1,
                   "expires_at": "2026-08-06T09:41:00Z", "fresh": true},
         "spend": null, "provider": "cli:codex", "sandbox": null,
         "receipt": null, "gate": null},
        {"job_id": "job-mangled", "goal": "Half decoded"}
      ],
      "runs": [
        {"run_id": "run-legacy", "scope": "s", "goal": "old", "status": "completed",
         "updated_at": "2026-08-06T01:00:00Z", "state_path": "/tmp/s.json",
         "provider": null, "spend": null}
      ]
    }
    """

private let emptyListFixture = """
    {"kind": "list", "id": "all", "status": "ok",
     "next_actions": [], "try_lines": [], "jobs": [], "runs": []}
    """

/// One running job holding a lease, with scriptable freshness and epoch, for
/// the stale-lease debounce tests.
private func leaseListFixture(fresh: Bool, epoch: Int, age: Int = 71) -> String {
    """
    {
      "kind": "list", "id": "all", "status": "ok",
      "next_actions": [], "try_lines": [],
      "jobs": [
        {"job_id": "job-leased", "scope": "s", "goal": "Leased job", "status": "running",
         "updated_at": "2026-08-06T09:40:00Z", "attempts": 1,
         "outcome": null, "stop_reason": null,
         "projection": {"phase": "running", "outcome": null, "stop_reason": null,
                        "attempt_count": 1, "caveats": []},
         "lease": {"owner_id": "sup", "epoch": \(epoch), "heartbeat_age_seconds": \(age),
                   "expires_at": "2026-08-06T09:41:00Z", "fresh": \(fresh)},
         "spend": null, "provider": "cli:codex", "sandbox": null,
         "receipt": null, "gate": null}
      ],
      "runs": []
    }
    """
}

private let providersFixture = """
    {
      "kind": "providers", "id": "configured", "status": "ok",
      "next_actions": [], "try_lines": [],
      "providers": [
        {"id": "cli:claude-code", "display_name": "Claude Code", "status": "ok"},
        {"id": "cli:codex", "display_name": "Codex", "status": "ok"},
        {"id": "cli:gemini", "display_name": "Gemini", "status": "failed"}
      ],
      "missing_providers": ["cli:droid"],
      "warnings": [],
      "active": "cli:codex"
    }
    """

private let supervisorRunningFixture = """
    {
      "schema_version": 1, "manager": "launchd", "installed": "current",
      "loaded": true, "enabled": null, "active": null,
      "checkpoint": null, "current_boot_id": "boot-1",
      "boot_identity_source": "macos_sysctl", "test_override": false
    }
    """

private let supervisorStoppedFixture = """
    {
      "schema_version": 1, "manager": "launchd", "installed": "not_installed",
      "loaded": null, "enabled": null, "active": null,
      "checkpoint": null, "current_boot_id": "boot-1",
      "boot_identity_source": "macos_sysctl", "test_override": false
    }
    """

private let doctorHealthyFixture = """
    {
      "kind": "doctor", "id": "config", "status": "verified",
      "next_actions": [], "try_lines": [],
      "findings": [
        {"status": "passed", "subject": "config", "detail": "config present"},
        {"status": "warning", "subject": "provider cli:gemini", "detail": "version probe failed"}
      ]
    }
    """

private let doctorFailedFixture = """
    {
      "kind": "doctor", "id": "config", "status": "blocked",
      "next_actions": [], "try_lines": [],
      "findings": [
        {"status": "failed", "subject": "sandbox", "detail": "sandbox-exec missing"},
        {"status": "passed", "subject": "config", "detail": "config present"}
      ]
    }
    """

// MARK: - Tests

@MainActor
final class FleetStoreTests: XCTestCase {
    /// Long-cadence store: only explicit refreshes and watcher hints poll.
    private func makeStore(cli: FakeCLI, watcher: FleetWatching? = nil) -> FleetStore {
        FleetStore(
            cli: cli, watcher: watcher,
            cadence: FleetStore.Cadence(windowVisible: 3600, menubarOnly: 3600, harbor: 3600))
    }

    private func waitUntil(
        _ timeout: TimeInterval = 2, _ condition: @escaping () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline {
                XCTFail("condition not met within \(timeout)s")
                return
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    // MARK: Fleet refresh

    func testRefreshBuildsQueueFromFixtureAndQuarantinesTheMangledRow() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        let store = makeStore(cli: cli)

        await store.refreshNow()

        guard case .loaded(let queue) = store.fleet else {
            return XCTFail("expected loaded fleet, got \(store.fleet)")
        }
        XCTAssertEqual(queue.atTheGate.map(\.id), ["job-gate"])
        XCTAssertEqual(queue.underway.map(\.id), ["job-running"])
        XCTAssertEqual(queue.unknown.count, 1)
        XCTAssertEqual(store.legacyRunCount, 1)
        XCTAssertNotNil(store.lastRefreshed)
        XCTAssertEqual(cli.calls.first, ["list", "--all", "--json"])
    }

    func testBinaryMissingBecomesTypedUnavailableStateNotACrash() async {
        let cli = FakeCLI()
        cli.script("list", .failure(FleetCLIError.binaryUnavailable("The bundled CLI binary deadreckon_darwin_arm64 is missing")))
        let store = makeStore(cli: cli)

        await store.refreshNow()

        guard case .unavailable(let reason) = store.fleet else {
            return XCTFail("expected unavailable, got \(store.fleet)")
        }
        XCTAssertTrue(reason.contains("missing"))
        XCTAssertEqual(store.menuBarState, .unavailable)
        XCTAssertTrue(store.queue.isEmpty, "unavailable must never present fake rows")
    }

    func testNonzeroListExitBecomesUnavailableWithStderrWords() async {
        let cli = FakeCLI()
        cli.script("list", .result(CLIRunResult(stdout: "", stderr: "scan failed: permission denied", exitCode: 2)))
        let store = makeStore(cli: cli)

        await store.refreshNow()

        guard case .unavailable(let reason) = store.fleet else {
            return XCTFail("expected unavailable, got \(store.fleet)")
        }
        XCTAssertTrue(reason.contains("exited 2"))
        XCTAssertTrue(reason.contains("permission denied"))
    }

    func testUndecodableEnvelopeBecomesUnavailable() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", "not json at all")
        let store = makeStore(cli: cli)

        await store.refreshNow()

        guard case .unavailable = store.fleet else {
            return XCTFail("expected unavailable, got \(store.fleet)")
        }
    }

    func testMenuBarStateLoadingBeforeFirstRefresh() {
        let store = makeStore(cli: FakeCLI())
        XCTAssertEqual(store.menuBarState, .loading)
    }

    // MARK: Watcher orchestration

    func testWatcherHintTriggersImmediateRefresh() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let watcher = FakeWatcher()
        let store = makeStore(cli: cli, watcher: watcher)

        store.start()
        try await waitUntil { cli.callCount(leading: "list") >= 1 }
        let before = cli.callCount(leading: "list")

        watcher.fire()
        try await waitUntil { cli.callCount(leading: "list") > before }

        store.stop()
        XCTAssertEqual(watcher.startCount, 1)
        XCTAssertEqual(watcher.stopCount, 1)
    }

    // MARK: Cadence

    func testWindowVisibilitySpeedsUpThePollCadence() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let store = FleetStore(
            cli: cli, watcher: nil,
            cadence: FleetStore.Cadence(windowVisible: 0.05, menubarOnly: 3600, harbor: 3600))

        store.start()
        try await waitUntil { cli.callCount(leading: "list") >= 1 }
        // Menubar-only cadence is an hour: the count must stay put.
        try await Task.sleep(nanoseconds: 200_000_000)
        let menubarOnlyPolls = cli.callCount(leading: "list")
        XCTAssertLessThanOrEqual(menubarOnlyPolls, 2)

        store.setWindowVisible(true)
        try await waitUntil { cli.callCount(leading: "list") >= menubarOnlyPolls + 3 }
        store.stop()
    }

    // MARK: Harbor

    func testHarborChipsCountProvidersAndSupervisorHonestly() async {
        let cli = FakeCLI()
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.scriptStdout("doctor", doctorHealthyFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        // 2 ok of 4 answered-for routes (3 probed + 1 missing).
        XCTAssertEqual(store.harbor.providers, .counted(ok: 2, total: 4))
        XCTAssertEqual(store.harbor.supervisor, .running)
        XCTAssertEqual(store.harbor.doctor, .ok(warnings: 1))
        XCTAssertEqual(store.binaryVersion, "deadreckon 0.8.4")
    }

    /// Doctor exits nonzero when checks fail but still prints the envelope:
    /// the chip counts the failed findings from the report itself.
    func testDoctorFailuresCountedEvenOnNonzeroExit() async {
        let cli = FakeCLI()
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.script("doctor", .result(CLIRunResult(stdout: doctorFailedFixture, stderr: "", exitCode: 1)))
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        XCTAssertEqual(store.harbor.doctor, .failed(1))
    }

    func testSupervisorStoppedReportsInstallState() async {
        let cli = FakeCLI()
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorStoppedFixture)
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        XCTAssertEqual(store.harbor.supervisor, .stopped(.notInstalled))
    }

    func testHarborDegradesToUnknownWhenBinaryUnavailable() async {
        let cli = FakeCLI()
        cli.script("providers", .failure(FleetCLIError.binaryUnavailable("no binary")))
        cli.script("supervisor", .failure(FleetCLIError.binaryUnavailable("no binary")))
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        guard case .unknown(let providersReason) = store.harbor.providers else {
            return XCTFail("expected unknown providers chip")
        }
        XCTAssertTrue(providersReason.contains("no binary"))
        guard case .unknown = store.harbor.supervisor else {
            return XCTFail("expected unknown supervisor chip")
        }
    }

    func testHarborChipDegradesIndependentlyOnOneBadSurface() async {
        let cli = FakeCLI()
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", "not json")
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        XCTAssertEqual(store.harbor.providers, .counted(ok: 2, total: 4))
        guard case .unknown = store.harbor.supervisor else {
            return XCTFail("expected unknown supervisor chip")
        }
    }

    // MARK: Stale-lease debounce

    /// One raw stale poll never paints amber: the rollup computes freshness
    /// at CLI-invocation time, so the first poll after sleep/wake would flash
    /// a false amber and teach the operator to ignore it.
    func testStaleLeaseConfirmedOnlyAfterConsecutiveStalePolls() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", leaseListFixture(fresh: false, epoch: 2))
        let store = makeStore(cli: cli)

        await store.refreshNow()
        XCTAssertTrue(store.confirmedStaleLeaseJobIDs.isEmpty, "one stale poll must not confirm")

        await store.refreshNow()
        XCTAssertEqual(store.confirmedStaleLeaseJobIDs, ["job-leased"])
    }

    func testStaleLeaseStreakResetsOnEpochChangeAndOnFreshReport() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", leaseListFixture(fresh: false, epoch: 2))
        let store = makeStore(cli: cli)
        await store.refreshNow()

        // The supervisor reclaimed the lease (new epoch): streak restarts.
        cli.scriptStdout("list", leaseListFixture(fresh: false, epoch: 3))
        await store.refreshNow()
        XCTAssertTrue(store.confirmedStaleLeaseJobIDs.isEmpty)

        // A fresh heartbeat clears the streak entirely.
        cli.scriptStdout("list", leaseListFixture(fresh: true, epoch: 3, age: 1))
        await store.refreshNow()
        cli.scriptStdout("list", leaseListFixture(fresh: false, epoch: 3))
        await store.refreshNow()
        XCTAssertTrue(store.confirmedStaleLeaseJobIDs.isEmpty)
        await store.refreshNow()
        XCTAssertEqual(store.confirmedStaleLeaseJobIDs, ["job-leased"])
    }

    /// Design 2.4.1: the menubar badges on stale-lease. Unconfirmed reads
    /// live; confirmed reads degraded (attention-class, below decisions).
    func testConfirmedStaleLeaseDrivesDegradedMenuBarState() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", leaseListFixture(fresh: false, epoch: 2))
        let store = makeStore(cli: cli)

        await store.refreshNow()
        XCTAssertEqual(store.menuBarState, .live(1))

        await store.refreshNow()
        XCTAssertEqual(store.menuBarState, .degraded(staleLeases: 1, supervisorDown: false))
    }

    /// Design 2.4.1: the menubar badges on supervisor-down. Only a
    /// positively-reported stopped Watchkeeper badges; an unknown chip
    /// (surface failed) stays a quiet chip.
    func testSupervisorStoppedDrivesDegradedMenuBarState() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", emptyListFixture)
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorStoppedFixture)
        cli.scriptStdout("doctor", doctorHealthyFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let store = makeStore(cli: cli)

        await store.refreshNow()
        await store.refreshHarborNow()
        XCTAssertEqual(store.menuBarState, .degraded(staleLeases: 0, supervisorDown: true))
    }

    func testSupervisorUnknownDoesNotBadgeTheMenuBar() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", emptyListFixture)
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", "not json")
        cli.scriptStdout("doctor", doctorHealthyFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let store = makeStore(cli: cli)

        await store.refreshNow()
        await store.refreshHarborNow()
        XCTAssertEqual(store.menuBarState, .idle)
    }

    // MARK: Harbor error honesty

    /// A timeout must read as a timeout, not as a decode failure: the chip
    /// carries the error's own words verbatim.
    func testHarborChipReportsTimeoutWordsNotDecodeWords() async {
        let cli = FakeCLI()
        cli.script("providers", .failure(CLIRunnerError.timeout(30)))
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.scriptStdout("doctor", doctorHealthyFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let store = makeStore(cli: cli)

        await store.refreshHarborNow()

        guard case .unknown(let reason) = store.harbor.providers else {
            return XCTFail("expected unknown providers chip")
        }
        XCTAssertTrue(reason.contains("did not finish within 30 seconds"), reason)
        XCTAssertFalse(reason.contains("decode"), reason)
    }

    // MARK: Watcher retry on a fresh home

    /// A watcher that started before the first job exists stays inert
    /// (jobs/ absent); the store must retry start after successful polls so
    /// FSEvents hints kick in the moment the directory appears.
    func testInertWatcherIsRetriedAfterSuccessfulPoll() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        cli.scriptStdout("providers", providersFixture)
        cli.scriptStdout("supervisor", supervisorRunningFixture)
        cli.scriptStdout("doctor", doctorHealthyFixture)
        cli.scriptStdout("--version", "deadreckon 0.8.4")
        let watcher = InertWatcher()
        let store = makeStore(cli: cli, watcher: watcher)

        store.start()
        try await waitUntil { watcher.startCount >= 2 }
        store.stop()
    }

    // MARK: Teardown

    func testShutdownStopsPollingAndTerminatesInFlightChildren() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        let watcher = FakeWatcher()
        let store = makeStore(cli: cli, watcher: watcher)

        store.start()
        store.shutdown(patience: 2)

        XCTAssertEqual(cli.terminateCalls, [2])
        XCTAssertEqual(watcher.stopCount, 1)
    }

    /// After shutdown's SIGTERM sweep, no code path may launch a fresh
    /// child: queued refreshes, direct refresh calls, and Harbor sweeps are
    /// all fenced off.
    func testNoChildLaunchesAfterShutdown() async {
        let cli = FakeCLI()
        cli.scriptStdout("list", listFixture)
        let store = makeStore(cli: cli)

        await store.refreshNow()
        let listCallsBefore = cli.callCount(leading: "list")

        store.shutdown(patience: 2)
        await store.refreshNow()
        await store.refreshHarborNow()

        XCTAssertEqual(cli.callCount(leading: "list"), listCallsBefore)
        XCTAssertEqual(cli.callCount(leading: "providers"), 0)
        XCTAssertEqual(cli.callCount(leading: "supervisor"), 0)
        XCTAssertEqual(cli.callCount(leading: "doctor"), 0)
    }
}
