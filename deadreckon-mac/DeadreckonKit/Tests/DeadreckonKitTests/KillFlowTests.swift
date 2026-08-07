import XCTest

@testable import DeadreckonKit

/// The kill state machine: envelope acceptance -> amber cancel-requested;
/// terminal ONLY from the job-events.jsonl terminal event; NEVER from an
/// exit code (there is no exit-code fold on the type, and this suite pins
/// that a clean exit alone cannot resolve).
@MainActor
final class KillFlowTests: XCTestCase {

    private func killEnvelope(id: String = "job-1") -> String {
        """
        {"kind": "kill", "id": "\(id)", "status": "cancel requested",
         "next_actions": ["deadreckon status \(id)"], "try_lines": [],
         "signal": "SIGTERM", "escalated": false, "terminal_phase_observed": false}
        """
    }

    func testEnvelopeAcceptanceEntersAmberNotTerminal() {
        var progress = KillProgress()
        progress.dispatched()
        progress.fold(result: MutationResult.classify(
            stdout: killEnvelope(), stderr: "", exitCode: 0))
        guard case .cancelRequested(let facts) = progress.phase else {
            return XCTFail("expected amber cancel-requested: \(progress.phase)")
        }
        XCTAssertEqual(facts.signal, "SIGTERM")
    }

    func testExitCodeAloneNeverResolvesTerminal() {
        var progress = KillProgress()
        progress.dispatched()
        // A successful exit with an envelope claiming terminal_phase_observed
        // is still only amber: resolution belongs to job-events.jsonl.
        let observed = killEnvelope().replacingOccurrences(
            of: "\"terminal_phase_observed\": false",
            with: "\"terminal_phase_observed\": true")
        progress.fold(result: MutationResult.classify(stdout: observed, stderr: "", exitCode: 0))
        if case .terminal = progress.phase {
            XCTFail("the envelope (and exit 0) must never resolve terminal")
        }
        // An envelope-free clean exit is even less: not amber, not terminal.
        var bare = KillProgress()
        bare.dispatched()
        bare.fold(result: MutationResult.classify(stdout: "", stderr: "", exitCode: 0))
        guard case .envelopeFree = bare.phase else {
            return XCTFail("an envelope-free exit renders its words only: \(bare.phase)")
        }
    }

    func testTerminalOnlyFromJobEvent() {
        var progress = KillProgress()
        progress.dispatched()
        progress.fold(result: MutationResult.classify(
            stdout: killEnvelope(), stderr: "", exitCode: 0))
        progress.fold(jobEventKind: .attemptStopped)
        if case .terminal = progress.phase {
            XCTFail("attempt_stopped is not a terminal classification")
        }
        progress.fold(jobEventKind: .cancelled)
        guard case .terminal(.cancelled) = progress.phase else {
            return XCTFail("the cancelled job event resolves the kill: \(progress.phase)")
        }
    }

    func testRefusalIsAuthoritativeAndSticky() {
        var progress = KillProgress()
        progress.dispatched()
        progress.fold(result: MutationResult.classify(stdout: """
        {"kind": "error", "code": 1, "verb": "kill",
         "message": "job job-1 is already terminal", "try_lines": ["deadreckon status job-1"]}
        """, stderr: "", exitCode: 1))
        guard case .refused(let refusal) = progress.phase else {
            return XCTFail("expected the typed refusal: \(progress.phase)")
        }
        XCTAssertEqual(refusal.tryLines, ["deadreckon status job-1"])
        // A later terminal event belongs to another story; refusal stays.
        progress.fold(jobEventKind: .cancelled)
        guard case .refused = progress.phase else {
            return XCTFail("a refusal never silently upgrades: \(progress.phase)")
        }
    }

    func testCampaignCascadeSurfacesEveryEnvelope() {
        let stream = """
        {"kind": "kill", "id": "plan-1", "status": "killed", "next_actions": [], "try_lines": [],
         "signal": "SIGTERM", "escalated": false, "terminal_phase_observed": true,
         "processes_signalled": 3}
        {"kind": "kill", "id": "campaign-1", "status": "killed", "next_actions": [], "try_lines": [],
         "signal": "SIGTERM", "escalated": false, "terminal_phase_observed": true}
        """
        var progress = KillProgress()
        progress.dispatched()
        progress.fold(result: MutationResult.classify(stdout: stream, stderr: "", exitCode: 0))
        XCTAssertEqual(progress.cascadeEnvelopes.count, 1)
        XCTAssertEqual(progress.cascadeEnvelopes.first?.id, "plan-1")
        guard case .cancelRequested = progress.phase else {
            return XCTFail("the campaign envelope is the primary: \(progress.phase)")
        }
    }

    // MARK: - Coordinator: file-backed resolution end to end

    func testCoordinatorResolvesFromJobEventsFile() async throws {
        let home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-kill-\(UUID().uuidString)")
        let jobDir = home.appendingPathComponent("jobs").appendingPathComponent("job-1")
        try FileManager.default.createDirectory(at: jobDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }

        let cli = FakeCLI()
        cli.scriptStdout("kill", killEnvelope())
        let coordinator = KillCoordinator(jobID: "job-1", cli: cli, home: home)
        await coordinator.dispatch()
        coordinator.stop()
        guard case .cancelRequested = coordinator.progress.phase else {
            return XCTFail("envelope acceptance renders amber: \(coordinator.progress.phase)")
        }
        XCTAssertEqual(cli.calls.first, ["kill", "job-1", "--json"])

        func event(_ seq: Int, _ kind: String) -> String {
            """
            {"schema_version": 1, "job_id": "job-1", "event_id": "e\(seq)", \
            "causation_id": "c\(seq)", "sequence": \(seq), "kind": "\(kind)", \
            "lease_epoch": 1, "timestamp": "2026-08-07T00:0\(seq):00Z", "detail": {}}\n
            """
        }
        var ledger = event(1, "created") + event(2, "cancel_requested")
        try Data(ledger.utf8).write(to: jobDir.appendingPathComponent("job-events.jsonl"))
        coordinator.pollOnce()
        if case .terminal = coordinator.progress.phase {
            XCTFail("cancel_requested is not terminal")
        }

        ledger += event(3, "cancelled")
        try Data(ledger.utf8).write(to: jobDir.appendingPathComponent("job-events.jsonl"))
        coordinator.pollOnce()
        guard case .terminal(.cancelled) = coordinator.progress.phase else {
            return XCTFail("the file-backed terminal event resolves: \(coordinator.progress.phase)")
        }
        coordinator.stop()
    }

    /// Design 2.4.5: the sheet resolves ONLY when THAT terminal event lands.
    /// A ledger that already carries a terminal classification from another
    /// story (a prior attempt's budget_exhausted on a paused job — the
    /// "raise or kill" flow) must never resolve this kill: the tail is
    /// primed past the existing history at dispatch time.
    func testHistoricalTerminalRowNeverResolvesThisKill() async throws {
        let home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-kill-\(UUID().uuidString)")
        let jobDir = home.appendingPathComponent("jobs").appendingPathComponent("job-1")
        try FileManager.default.createDirectory(at: jobDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }

        func event(_ seq: Int, _ kind: String) -> String {
            """
            {"schema_version": 1, "job_id": "job-1", "event_id": "e\(seq)", \
            "causation_id": "c\(seq)", "sequence": \(seq), "kind": "\(kind)", \
            "lease_epoch": 1, "timestamp": "2026-08-07T00:0\(seq):00Z", "detail": {}}\n
            """
        }
        // History BEFORE the kill: a terminal classification already on disk.
        var ledger = event(1, "created") + event(2, "budget_exhausted")
        let ledgerURL = jobDir.appendingPathComponent("job-events.jsonl")
        try Data(ledger.utf8).write(to: ledgerURL)

        let cli = FakeCLI()
        cli.scriptStdout("kill", killEnvelope())
        let coordinator = KillCoordinator(jobID: "job-1", cli: cli, home: home)
        await coordinator.dispatch()
        coordinator.pollOnce()
        guard case .cancelRequested = coordinator.progress.phase else {
            coordinator.stop()
            return XCTFail("historical budget_exhausted must not resolve this kill: "
                + "\(coordinator.progress.phase)")
        }

        // The terminal event appended AFTER this dispatch resolves it.
        ledger += event(3, "cancelled")
        try Data(ledger.utf8).write(to: ledgerURL)
        coordinator.pollOnce()
        guard case .terminal(.cancelled) = coordinator.progress.phase else {
            coordinator.stop()
            return XCTFail("the post-dispatch terminal event resolves: \(coordinator.progress.phase)")
        }
        coordinator.stop()
    }

    /// A strict-sequence violation in job-events.jsonl (crashed supervisor,
    /// corrupted file) makes file-backed resolution impossible: the sheet
    /// must say so instead of waiting in amber forever.
    func testCorruptLedgerSurfacesResolutionUnavailable() async throws {
        let home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-kill-\(UUID().uuidString)")
        let jobDir = home.appendingPathComponent("jobs").appendingPathComponent("job-1")
        try FileManager.default.createDirectory(at: jobDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }

        let cli = FakeCLI()
        cli.scriptStdout("kill", killEnvelope())
        let coordinator = KillCoordinator(jobID: "job-1", cli: cli, home: home)
        await coordinator.dispatch()

        // Sequence starts at 2: a strict gap (expected 1).
        try Data("""
        {"schema_version": 1, "job_id": "job-1", "event_id": "e2", "causation_id": "c2", \
        "sequence": 2, "kind": "cancelled", "lease_epoch": 1, \
        "timestamp": "2026-08-07T00:02:00Z", "detail": {}}\n
        """.utf8).write(to: jobDir.appendingPathComponent("job-events.jsonl"))
        coordinator.pollOnce()
        guard case .resolutionUnavailable(let reason) = coordinator.progress.phase else {
            coordinator.stop()
            return XCTFail("a corrupt ledger must surface honestly: \(coordinator.progress.phase)")
        }
        XCTAssertTrue(reason.contains("sequence gap"), reason)
        coordinator.stop()
    }

    /// The dispatch guard: a double-click's second task finds the phase
    /// already past idle and dispatches nothing.
    func testSecondDispatchIsANoOp() async throws {
        let home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-kill-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: home) }
        let cli = FakeCLI()
        cli.scriptStdout("kill", killEnvelope())
        let coordinator = KillCoordinator(jobID: "job-1", cli: cli, home: home)
        await coordinator.dispatch()
        await coordinator.dispatch()
        coordinator.stop()
        XCTAssertEqual(cli.callCount(leading: "kill"), 1,
                       "kill dispatches at most once per sheet")
    }

    func testEscalateFlagIsExplicitInTheCommandLine() {
        let coordinator = KillCoordinator(jobID: "job-1", cli: FakeCLI())
        XCTAssertEqual(coordinator.commandLine, "deadreckon kill job-1 --json")
        coordinator.escalate = true
        XCTAssertEqual(coordinator.commandLine, "deadreckon kill job-1 --escalate --json")
    }
}
