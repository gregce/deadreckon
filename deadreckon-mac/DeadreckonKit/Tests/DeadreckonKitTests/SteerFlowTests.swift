import XCTest

@testable import DeadreckonKit

/// Steer queued -> delivered: the chip renders queued from the envelope's
/// inbox_seq and flips to delivered ONLY when the typed steer_delivered
/// event (matching queued_at) appears in the events tail.
@MainActor
final class SteerFlowTests: XCTestCase {

    private let queuedAt = "2026-08-07T10:00:00.500Z"

    private func steerEnvelope() -> String {
        """
        {"kind": "steer", "id": "run-1", "status": "completed",
         "next_actions": ["deadreckon attach run-1"], "try_lines": [],
         "queued_at": "\(queuedAt)", "inbox_seq": 4, "source": "cli",
         "delivery": "next turn boundary"}
        """
    }

    func testQueuedFromEnvelopeThenDeliveredOnMatchingTypedEvent() {
        var tracker = SteerDeliveryTracker()
        tracker.submitted()
        tracker.fold(result: MutationResult.classify(
            stdout: steerEnvelope(), stderr: "", exitCode: 0))
        guard case .queued(let facts) = tracker.phase else {
            return XCTFail("expected queued: \(tracker.phase)")
        }
        XCTAssertEqual(facts.inboxSeq, 4)

        // A steer_delivered for some OTHER queued note does not flip.
        tracker.fold(deliveries: [
            SteerDeliveredFact(turn: 6, queuedAtRaw: "2026-08-07T09:00:00Z", preview: "other")
        ])
        guard case .queued = tracker.phase else {
            return XCTFail("a non-matching delivery must not flip: \(tracker.phase)")
        }

        // The matching typed event flips to delivered.
        tracker.fold(deliveries: [
            SteerDeliveredFact(turn: 7, queuedAtRaw: queuedAt, preview: "cap backoff")
        ])
        guard case .delivered(_, let turn) = tracker.phase else {
            return XCTFail("expected delivered: \(tracker.phase)")
        }
        XCTAssertEqual(turn, 7)
    }

    func testRefusalDowngradesTheControl() {
        var tracker = SteerDeliveryTracker()
        tracker.submitted()
        tracker.fold(result: MutationResult.classify(stdout: """
        {"kind": "error", "code": 1, "verb": "steer",
         "message": "run run-1 is not executing", "try_lines": ["deadreckon status run-1"]}
        """, stderr: "", exitCode: 1))
        guard case .refused(let refusal) = tracker.phase else {
            return XCTFail("the verb refusal is authoritative: \(tracker.phase)")
        }
        XCTAssertEqual(refusal.message, "run run-1 is not executing")
    }

    func testCoordinatorSubmitsArgvAndObservesDeliveries() async {
        let cli = FakeCLI()
        cli.scriptStdout("steer", steerEnvelope())
        let coordinator = SteerCoordinator(targetID: "run-1", cli: cli)
        await coordinator.submit(note: "cap backoff at 5m")
        XCTAssertEqual(cli.calls.first, ["steer", "--json", "--", "run-1", "cap backoff at 5m"])
        guard case .queued = coordinator.tracker.phase else {
            return XCTFail("expected queued: \(coordinator.tracker.phase)")
        }
        coordinator.observe(deliveries: [
            SteerDeliveredFact(turn: 9, queuedAtRaw: queuedAt, preview: "cap backoff")
        ])
        guard case .delivered = coordinator.tracker.phase else {
            return XCTFail("expected delivered: \(coordinator.tracker.phase)")
        }
    }

    /// Arrival-order independence: a typed steer_delivered event tailed
    /// while the envelope is still in flight (fast mid-turn delivery racing
    /// the submit) is buffered, and the fold(result:) that lands .queued
    /// immediately re-matches it — the chip never sticks at "queued"
    /// against a file-backed delivered fact.
    func testDeliveryObservedBeforeQueuedStillFlips() {
        var tracker = SteerDeliveryTracker()
        tracker.submitted()
        tracker.fold(deliveries: [
            SteerDeliveredFact(turn: 7, queuedAtRaw: queuedAt, preview: "cap backoff")
        ])
        guard case .submitting = tracker.phase else {
            return XCTFail("the delivery buffers; nothing flips before the envelope: \(tracker.phase)")
        }
        tracker.fold(result: MutationResult.classify(
            stdout: steerEnvelope(), stderr: "", exitCode: 0))
        guard case .delivered(_, let turn) = tracker.phase else {
            return XCTFail("the buffered delivery must flip on queue: \(tracker.phase)")
        }
        XCTAssertEqual(turn, 7)
    }

    /// The in-flight guard: a Return-key auto-repeat enqueues a second
    /// submit before the first resolves; it must not reach the binary.
    func testInFlightSubmitGuardDispatchesOnce() async {
        let cli = GatedCLI()
        cli.gatedResult = CLIRunResult(stdout: steerEnvelope(), stderr: "", exitCode: 0)
        let coordinator = SteerCoordinator(targetID: "run-1", cli: cli)
        let first = Task { await coordinator.submit(note: "cap backoff at 5m") }
        while cli.parkedCount == 0 { await Task.yield() }
        await coordinator.submit(note: "cap backoff at 5m")
        cli.release()
        await first.value
        XCTAssertEqual(cli.calls.count, 1,
                       "a second submit while one is in flight must not queue a duplicate steer")
    }

    func testEmptyNoteNeverDispatches() async {
        let cli = FakeCLI()
        let coordinator = SteerCoordinator(targetID: "run-1", cli: cli)
        await coordinator.submit(note: "   ")
        XCTAssertTrue(cli.calls.isEmpty)
    }

    func testQuickSteerEligibilityFromStatusEnvelope() async {
        let cli = FakeCLI()
        cli.scriptStdout("status", """
        {"kind": "job_status", "id": "job-1", "status": "running",
         "next_actions": ["deadreckon attach job-1"],
         "job": {"job": {"job_id": "job-1", "scope": "s", "goal": "g"},
                 "attempts": [{"id": {"scope": "s", "run_id": "run-1"},
                               "status": "executing",
                               "steerable": {"steerable": true}}]}}
        """)
        let quick = QuickSteerController(targetID: "job-1", cli: cli)
        await quick.checkEligibility()
        XCTAssertEqual(quick.eligibility, .eligible)

        cli.scriptStdout("status", """
        {"kind": "job_status", "id": "job-1", "status": "waiting",
         "next_actions": [],
         "job": {"job": {"job_id": "job-1", "scope": "s", "goal": "g"},
                 "attempts": [{"id": {"scope": "s", "run_id": "run-1"},
                               "status": "paused",
                               "steerable": {"steerable": false, "reason": "not_executing"}}]}}
        """)
        await quick.checkEligibility()
        XCTAssertEqual(quick.eligibility, .ineligible("not executing"),
                       "disabled states name the envelope's reason")
    }

    /// JobDetailStore surfaces typed steer_delivered events off the events
    /// tail for the workbench chip.
    func testDetailStorePublishesSteerDeliveries() async throws {
        let home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-steer-\(UUID().uuidString)")
        let jobDir = home.appendingPathComponent("jobs").appendingPathComponent("job-1")
        let runRoot = home.appendingPathComponent("runstate").appendingPathComponent("s")
            .appendingPathComponent("runs").appendingPathComponent("run-1")
        try FileManager.default.createDirectory(at: jobDir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: runRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }

        try Data("""
        {"schema_version": 1, "job_id": "job-1", "phase": "running", "outcome": null,
         "stop_reason": null, "last_sequence": 1, "current_lease_epoch": 1,
         "attempt_count": 1, "child_run_ids": ["run-1"],
         "updated_at": "2026-08-07T10:00:00Z", "caveats": []}
        """.utf8).write(to: jobDir.appendingPathComponent("projection.json"))
        try Data("""
        {"timestamp": "2026-08-07T10:01:00Z", "event": {"kind": "steer_delivered", \
        "turn": 7, "source": "cli", "queued_at": "\(queuedAt)", "preview": "cap backoff"}}\n
        """.utf8).write(to: runRoot.appendingPathComponent("events.jsonl"))

        let store = JobDetailStore(
            jobID: "job-1", scope: "s", goal: "g", cli: FakeCLI(), home: home)
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.steerDeliveries.count, 1)
        XCTAssertEqual(store.steerDeliveries.first?.queuedAtRaw, queuedAt)
        XCTAssertEqual(store.steerDeliveries.first?.turn, 7)
        store.close()
        XCTAssertTrue(store.steerDeliveries.isEmpty, "close resets the per-run chip evidence")
    }
}
