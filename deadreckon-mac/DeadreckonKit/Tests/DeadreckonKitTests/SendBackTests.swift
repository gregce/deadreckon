import XCTest

@testable import DeadreckonKit

/// Send back (G9): the extend --note round trip — argv, envelope
/// acknowledgment (note_recorded, contract inherited/replaced), and the
/// typed refusal for an empty note.
@MainActor
final class SendBackTests: XCTestCase {

    func testNoteRoundTrip() async {
        let cli = FakeCLI()
        cli.scriptStdout("extend", """
        {"kind": "extend", "id": "job-new", "status": "queued",
         "next_actions": ["deadreckon attach job-new"], "try_lines": [],
         "queued": true, "parent_id": "run-parent", "parent_run_id": "run-parent",
         "contract": "inherited", "note_recorded": true}
        """)
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "fix the stubbed check"
        coordinator.note = "gate accepted a stub; the smoke test must hit the real server"
        XCTAssertEqual(
            coordinator.commandLine,
            "deadreckon extend '--note=gate accepted a stub; the smoke test must hit the real server' "
            + "--yes --json -- run-parent 'fix the stubbed check'")
        await coordinator.submit()

        XCTAssertEqual(cli.calls.first, [
            "extend", "--note=gate accepted a stub; the smoke test must hit the real server",
            "--yes", "--json", "--", "run-parent", "fix the stubbed check",
        ])
        guard case .queued(let envelope) = coordinator.state else {
            return XCTFail("expected queued: \(coordinator.state)")
        }
        XCTAssertEqual(envelope.extend?.noteRecorded, true)
        XCTAssertEqual(envelope.extend?.contract, "inherited")
        XCTAssertEqual(envelope.id, "job-new")
    }

    func testSendBackWithoutNoteOmitsTheFlag() async {
        let cli = FakeCLI()
        cli.scriptStdout("extend", """
        {"kind": "extend", "id": "job-new", "status": "queued", "next_actions": [],
         "try_lines": [], "queued": true, "parent_id": "run-parent",
         "parent_run_id": "run-parent", "contract": "inherited", "note_recorded": false}
        """)
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "continue"
        coordinator.note = "   "
        await coordinator.submit()
        XCTAssertEqual(cli.calls.first, ["extend", "--yes", "--json", "--", "run-parent", "continue"])
        guard case .queued(let envelope) = coordinator.state else {
            return XCTFail("expected queued: \(coordinator.state)")
        }
        XCTAssertEqual(envelope.extend?.noteRecorded, false,
                       "no note sent, none recorded — the envelope says so")
    }

    func testEmptyGoalNeverDispatches() async {
        let cli = FakeCLI()
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "  "
        coordinator.note = "a note"
        await coordinator.submit()
        XCTAssertTrue(cli.calls.isEmpty, "extend requires a follow-up goal")
    }

    /// A binary that died mid-verb with no envelope may have queued the
    /// continuation Job BEFORE dying: the coordinator says so, points at the
    /// file-backed fleet, and DISARMS the one-click dispatch until the
    /// operator explicitly re-arms — no duplicate spend from a confused
    /// retry click.
    func testEnvelopeFreeFailureDisarmsUntilExplicitRearm() async {
        let cli = FakeCLI()
        cli.script("extend", .result(CLIRunResult(stdout: "", stderr: "", exitCode: -11)))
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "continue"
        await coordinator.submit()
        guard case .failed(let words) = coordinator.state else {
            return XCTFail("expected the honest failure: \(coordinator.state)")
        }
        XCTAssertTrue(words.contains("exit -11"), words)
        XCTAssertTrue(words.contains("may or may not have been queued"), words)
        XCTAssertTrue(coordinator.mayHaveQueued)
        XCTAssertFalse(coordinator.canSubmit, "the dispatch stays disarmed")

        await coordinator.submit()
        XCTAssertEqual(cli.calls.count, 1, "disarmed: no second extend without a fresh decision")

        coordinator.rearmAfterPossibleQueue()
        XCTAssertTrue(coordinator.canSubmit, "the explicit re-arm restores the dispatch")
    }

    /// The in-flight guard: a double-click's second submit while the first
    /// extend is still running must not queue a second continuation Job.
    func testInFlightSubmitGuardDispatchesOnce() async {
        let cli = GatedCLI()
        cli.gatedResult = CLIRunResult(stdout: """
        {"kind": "extend", "id": "job-new", "status": "queued", "next_actions": [],
         "try_lines": [], "queued": true, "parent_id": "run-parent",
         "parent_run_id": "run-parent", "contract": "inherited", "note_recorded": false}
        """, stderr: "", exitCode: 0)
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "continue"
        let first = Task { await coordinator.submit() }
        while cli.parkedCount == 0 { await Task.yield() }
        await coordinator.submit()
        cli.release()
        await first.value
        XCTAssertEqual(cli.calls.count, 1,
                       "a second submit while one is in flight must not queue a duplicate Job")
    }

    func testTypedRefusalRendersVerbatim() async {
        let cli = FakeCLI()
        cli.script("extend", .result(CLIRunResult(stdout: """
        {"kind": "error", "code": 1, "verb": "extend",
         "message": "--note must not be empty",
         "try_lines": ["deadreckon extend run-parent \\"goal\\" --note \\"why\\""]}
        """, stderr: "", exitCode: 1)))
        let coordinator = SendBackCoordinator(parentID: "run-parent", cli: cli)
        coordinator.goal = "continue"
        coordinator.note = "x"
        await coordinator.submit()
        guard case .refused(let refusal) = coordinator.state else {
            return XCTFail("expected refusal: \(coordinator.state)")
        }
        XCTAssertEqual(refusal.message, "--note must not be empty")
        XCTAssertEqual(refusal.tryLines.count, 1)
    }
}
