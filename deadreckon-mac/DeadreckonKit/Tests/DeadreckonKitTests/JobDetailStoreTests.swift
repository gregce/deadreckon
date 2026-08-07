import XCTest

@testable import DeadreckonKit

/// JobDetailStore: per-selected-job lifecycle, tail composition, and the
/// derived panes — against a fixture DEADRECKON_HOME and an injected FakeCLI.
/// No real binary is ever invoked.
@MainActor
final class JobDetailStoreTests: XCTestCase {
    private var home: URL!
    private let jobID = "job-1"
    private let scope = "demo-1234"
    private let runID = "run-new"

    override func setUp() async throws {
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("drk-detail-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: runRoot.appendingPathComponent("narrative"),
            withIntermediateDirectories: true)
        try FileManager.default.createDirectory(
            at: runRoot.appendingPathComponent("proofs"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: jobDir, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: home)
    }

    private var jobDir: URL {
        home.appendingPathComponent("jobs").appendingPathComponent(jobID)
    }

    private var runRoot: URL {
        home.appendingPathComponent("runstate").appendingPathComponent(scope)
            .appendingPathComponent("runs").appendingPathComponent(runID)
    }

    private func write(_ relative: String, in root: URL, _ content: String) throws {
        let url = root.appendingPathComponent(relative)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(content.utf8).write(to: url)
    }

    private func append(_ relative: String, in root: URL, _ content: String) throws {
        let url = root.appendingPathComponent(relative)
        if !FileManager.default.fileExists(atPath: url.path) {
            try write(relative, in: root, content)
            return
        }
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: Data(content.utf8))
    }

    private func writeProjection(runIDs: [String] = ["run-new"], phase: String = "running",
                                 attemptCount: Int = 1) throws {
        let ids = runIDs.map { "\"\($0)\"" }.joined(separator: ",")
        try write("projection.json", in: jobDir, """
            {"schema_version": 1, "job_id": "\(jobID)", "phase": "\(phase)",
             "outcome": null, "stop_reason": null, "last_sequence": 3,
             "current_lease_epoch": 1, "attempt_count": \(attemptCount),
             "child_run_ids": [\(ids)], "updated_at": "2026-08-06T10:00:00Z",
             "caveats": []}
            """)
    }

    private func writeRunState(status: String = "executing", turn: Int = 3) throws {
        try write("state.json", in: runRoot, """
            {"version": 1, "goal": "fix it", "task_key": "fix-it", "run_id": "\(runID)",
             "scope": "\(scope)", "status": "\(status)", "current_phase_id": 20,
             "started_at": "2026-08-06T09:00:00Z", "updated_at": "2026-08-06T10:00:00Z",
             "cwd": "/tmp", "run_root": "\(runRoot.path)",
             "working_dir": "\(runRoot.appendingPathComponent("work").path)",
             "skill_name": "deadreckon", "skill_path": "/tmp/s", "sandbox": "sandbox-exec",
             "provider": "cli:codex-server", "max_spend_usd": 25.0,
             "total_spend_usd": 4.83, "total_wall_seconds": 120.0, "turn": \(turn),
             "pause_reason": null, "failure_reason": null, "child_pids": [],
             "killed_at": null, "phases": [
               {"id": 20, "name": "implement", "status": "executing",
                "plan_path": null, "updated_at": "2026-08-06T10:00:00Z"}]}
            """)
    }

    private func makeStore(cli: FakeCLI = FakeCLI()) -> JobDetailStore {
        JobDetailStore(jobID: jobID, scope: scope, goal: "fix it", cli: cli, home: home)
    }

    // MARK: - Lifecycle

    func testOpenBuildsTailsAndCloseTearsEveryTailDown() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        XCTAssertEqual(store.activeTailCount, 0)

        store.open()
        XCTAssertTrue(store.isOpen)
        // Job-side tails exist immediately; run tails appear once the
        // projection resolves the current attempt.
        XCTAssertEqual(store.activeTailCount, 3)
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, runID)
        XCTAssertEqual(store.activeTailCount, 9)

        store.close()
        XCTAssertFalse(store.isOpen)
        XCTAssertEqual(store.activeTailCount, 0, "close() must drop every tail")
    }

    func testCloseStopsThePollLoop() async throws {
        try writeProjection()
        let cli = FakeCLI()
        let store = JobDetailStore(
            jobID: jobID, scope: scope, goal: "fix it", cli: cli, home: home,
            cadence: .init(poll: 0.01, reportEveryTicks: 1000))
        store.open()
        try await Task.sleep(nanoseconds: 60_000_000)
        store.close()
        // Let any tick already in flight at close() finish before counting.
        try await Task.sleep(nanoseconds: 50_000_000)
        let countAtClose = cli.callCount(leading: "status")
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertEqual(cli.callCount(leading: "status"), countAtClose,
                       "no CLI child may launch after close()")
    }

    func testRunSwitchRebuildsTailsAndResetsScrollback() async throws {
        try writeProjection(runIDs: ["run-old"])
        let oldRoot = home.appendingPathComponent("runstate").appendingPathComponent(scope)
            .appendingPathComponent("runs").appendingPathComponent("run-old")
        try write("events.jsonl", in: oldRoot,
                  #"{"timestamp": "2026-08-06T09:00:00Z", "run_id": "run-old", "event": {"kind": "turn_started", "turn": 1}}"# + "\n")
        let store = makeStore()
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, "run-old")
        XCTAssertEqual(store.activity.count, 1)

        // A new attempt starts: the projection now points at run-new.
        try writeProjection(runIDs: ["run-old", "run-new"], attemptCount: 2)
        try writeRunState()
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, "run-new")
        XCTAssertEqual(store.activity.count, 0, "scrollback is per attempt")
        store.close()
    }

    // MARK: - Activity + turns

    func testActivityFeedGrowsUnboundedAndKeepsRawLines() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        for turn in 1...3 {
            try append("events.jsonl", in: runRoot,
                       #"{"timestamp": "2026-08-06T10:00:0\#(turn)Z", "run_id": "run-new", "event": {"kind": "turn_started", "turn": \#(turn)}}"# + "\n")
        }
        await store.pollOnce()
        XCTAssertEqual(store.activity.map(\.line),
                       ["turn 1 started", "turn 2 started", "turn 3 started"])
        XCTAssertEqual(store.rawEventLines.count, 3)
        XCTAssertEqual(store.turns.map(\.turn), [1, 2, 3])
        store.close()
    }

    // MARK: - Acceptance progress: live band + restart rule

    func testLiveChecksAppendWithinOneAttempt() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        try append("proofs/acceptance-progress.jsonl", in: runRoot,
                   #"{"checked_at": "2026-08-06T10:00:00Z", "status": "started", "index": 1, "total": 5}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.liveChecks.count, 1)

        try append("proofs/acceptance-progress.jsonl", in: runRoot,
                   #"{"checked_at": "2026-08-06T10:00:02Z", "status": "passed", "index": 1, "total": 5, "result": {"kind": "cargo_test", "passed": true, "must_pass": true, "detail": "ok"}}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.liveChecks.count, 2)
        XCTAssertEqual(store.liveChecks.last?.result?.passed, true)
        store.close()
    }

    func testProgressRestartReplacesTheBandInsteadOfMixingAttempts() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        for index in 1...3 {
            try append("proofs/acceptance-progress.jsonl", in: runRoot,
                       #"{"checked_at": "2026-08-06T10:00:0\#(index)Z", "status": "passed", "index": \#(index), "total": 5}"# + "\n")
        }
        await store.pollOnce()
        XCTAssertEqual(store.liveChecks.count, 3)

        // Gate-attempt entry clears the file (clear_stale_gate_attempt_evidence),
        // then the next attempt streams one fresh row: shrink -> restart.
        try write("proofs/acceptance-progress.jsonl", in: runRoot,
                  #"{"checked_at": "2026-08-06T10:05:00Z", "status": "started", "index": 1, "total": 5}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.liveChecks.count, 1, "restart discards the stale attempt's rows")
        XCTAssertEqual(store.liveChecks.first?.status, "started")
        store.close()
    }

    // MARK: - Integrity chip

    private func jobEventLine(sequence: Int) -> String {
        #"{"schema_version": 1, "job_id": "\#(jobID)", "event_id": "e\#(sequence)", "causation_id": "c", "sequence": \#(sequence), "kind": "created", "lease_epoch": 0, "timestamp": "2026-08-06T10:00:00Z", "detail": {}}"# + "\n"
    }

    func testContiguousJobEventsReportTheHonestClaim() async throws {
        try writeProjection()
        for sequence in 1...3 {
            try append("job-events.jsonl", in: jobDir, jobEventLine(sequence: sequence))
        }
        let store = makeStore()
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.integrity, .contiguous(count: 3))
        XCTAssertFalse(store.jobEventsTornTail)
        store.close()
    }

    func testSequenceGapIsTheHonestFailureAndSticks() async throws {
        try writeProjection()
        try append("job-events.jsonl", in: jobDir, jobEventLine(sequence: 1))
        try append("job-events.jsonl", in: jobDir, jobEventLine(sequence: 3))
        let store = makeStore()
        store.open()
        await store.pollOnce()
        guard case .corrupt(let reason) = store.integrity else {
            return XCTFail("expected corrupt, got \(store.integrity)")
        }
        XCTAssertTrue(reason.contains("expected 2"), reason)

        // Later clean appends never rehabilitate a corrupt ledger.
        try append("job-events.jsonl", in: jobDir, jobEventLine(sequence: 2))
        await store.pollOnce()
        guard case .corrupt = store.integrity else {
            return XCTFail("corruption must be sticky")
        }
        store.close()
    }

    func testTornTailBadgesWithoutBreakingContiguity() async throws {
        try writeProjection()
        try append("job-events.jsonl", in: jobDir, jobEventLine(sequence: 1))
        // A torn append: no trailing newline.
        try append("job-events.jsonl", in: jobDir,
                   #"{"schema_version": 1, "job_id": "job-1", "event_id": "e2""#)
        let store = makeStore()
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.integrity, .contiguous(count: 1))
        XCTAssertTrue(store.jobEventsTornTail, "retained unterminated bytes badge the drawer")
        store.close()
    }

    // MARK: - Spend meter

    func testSpendMeterKeepsLoopHeadAndNarratorSplitSeparate() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        try append("spend.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:00Z", "turn": 1, "provider": "p", "model": "m", "input_tokens": 10, "output_tokens": 5, "cost_usd": 1.0, "total_cost_usd": 1.0, "cap_usd": 25.0, "kind": "loop"}"# + "\n")
        try append("spend.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:05Z", "turn": 1, "provider": "p", "model": "m", "input_tokens": 4, "output_tokens": 2, "cost_usd": 0.05, "total_cost_usd": 0.05, "kind": "narrator"}"# + "\n")
        try append("spend.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:10Z", "turn": 2, "provider": "p", "model": "m", "input_tokens": 10, "output_tokens": 5, "cost_usd": 2.0, "total_cost_usd": 3.0, "cap_usd": 25.0, "kind": "loop"}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.spendMeter.loopTotalUSD, 3.0, "loop head is the LAST loop row's total")
        XCTAssertEqual(store.spendMeter.narratorTotalUSD, 0.05, accuracy: 0.0001)
        XCTAssertEqual(store.spendMeter.capUSD, 25.0)
        XCTAssertEqual(store.spendMeter.lastLoopTurn, 2)
        store.close()
    }

    // MARK: - Narrative

    func testNarrativeKeepsDeterministicProjectionAndLabelsOverlay() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        store.nowProvider = { now }
        store.open()
        try append("narrative/snapshots.jsonl", in: runRoot,
                   #"{"snapshot_id": "s1", "created_at": "2026-08-06T10:00:00Z", "status": "deterministic", "headline": "run created", "current_work": [], "risks": [], "next_likely": []}"# + "\n")
        try append("narrative/snapshots.jsonl", in: runRoot,
                   #"{"snapshot_id": "s2", "created_at": "2026-08-06T10:01:00Z", "status": "fresh", "headline": "model prose", "current_work": [], "risks": [], "next_likely": []}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.narrative.latestSnapshot?.snapshotID, "s2")
        XCTAssertEqual(store.narrative.latestSnapshot?.isUnverifiedOverlay, true)
        XCTAssertEqual(store.narrative.latestDeterministic?.snapshotID, "s1",
                       "the deterministic projection survives overlay beats")
        // 2026 snapshots against a 2027 clock: stale, honestly.
        guard case .stale = store.narrative.staleness else {
            return XCTFail("expected stale, got \(store.narrative.staleness)")
        }
        store.close()
    }

    // MARK: - CLI composition

    private var statusJSON: String {
        """
        {"kind": "job_status", "id": "\(jobID)", "status": "running",
         "verified_proof": {"status": "not-applicable", "error": null},
         "job": {"job": {"job_id": "\(jobID)", "scope": "\(scope)", "goal": "fix it"},
                 "attempts": [{"id": {"scope": "\(scope)", "run_id": "\(runID)", "short": "r"},
                               "status": "executing",
                               "steerable": {"steerable": false, "reason": "provider_not_steerable"},
                               "provider": "cli:claude-code"}]}}
        """
    }

    func testStatusEnvelopeFeedsSteerEligibility() async throws {
        try writeProjection()
        try writeRunState()
        let cli = FakeCLI()
        cli.scriptStdout("status", statusJSON)
        let store = makeStore(cli: cli)
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.status?.currentSteerable,
                       SteerEligibility(steerable: false, reason: .providerNotSteerable))
        XCTAssertNil(store.statusIssue)
        store.close()
    }

    func testCLIFailureIsTypedNotFatal() async throws {
        try writeProjection()
        try writeRunState()
        let cli = FakeCLI()
        cli.script("status", .failure(FleetCLIError.binaryUnavailable("no trusted binary")))
        let store = makeStore(cli: cli)
        store.open()
        try append("events.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:01Z", "run_id": "run-new", "event": {"kind": "turn_started", "turn": 1}}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.statusIssue, "no trusted binary")
        XCTAssertEqual(store.activity.count, 1, "file-backed panes stay live")
        store.close()
    }

    func testChangesAndPatchLoadOnDemand() async throws {
        try writeProjection()
        try writeRunState()
        let cli = FakeCLI()
        cli.scriptStdout("show", """
            {"files_changed": 1, "added": 2, "removed": 1,
             "files": [{"path": "src/a.rs", "added": 2, "removed": 1, "status": "modified"}],
             "patches": [{"path": "src/a.rs", "status": "modified",
                          "unified": "@@ -1 +1 @@", "truncated": false}]}
            """)
        let store = makeStore(cli: cli)
        store.open()
        await store.pollOnce()
        await store.refreshChanges()
        XCTAssertEqual(store.changes?.filesChanged, 1)
        await store.loadPatch(path: "src/a.rs")
        XCTAssertEqual(store.patches["src/a.rs"]?.unified, "@@ -1 +1 @@")
        store.close()
    }

    func testTerminalJobNeverLaunchesAVerdictChild() async throws {
        // The committed binary's `verdict` accepts run references only and
        // the child run of a durable Job is driver-fenced: the call could
        // only produce a typed refusal, so the store must not issue it. The
        // receipt band derives from `report --json` instead (Rust-side
        // verdict-on-JOB follow-up registered in CONTRACTS.md).
        try writeProjection(phase: "terminal")
        try writeRunState(status: "completed")
        let cli = FakeCLI()
        cli.scriptStdout("status", statusJSON)
        cli.scriptStdout("report", #"{"id": "job-1", "goal": "fix it", "phase": "terminal", "deterministic_checks": [{"kind": "cargo_test", "passed": true, "must_pass": true, "detail": "ok"}], "attempts": []}"#)
        let store = makeStore(cli: cli)
        store.open()
        await store.pollOnce()
        await store.pollOnce()
        XCTAssertEqual(cli.callCount(leading: "verdict"), 0,
                       "verdict on a job ref can only refuse in the committed binary; never launched")
        XCTAssertEqual(store.report?.deterministicChecks.count, 1,
                       "the receipt evidence rides the report envelope")
        store.close()
    }

    // MARK: - Drawer terminals

    func testSupervisorTailsArePlainTextAppends() async throws {
        try writeProjection()
        let store = makeStore()
        store.open()
        try append("supervisor.out", in: jobDir, "[supervisor] lease renewed epoch 7\n")
        await store.pollOnce()
        XCTAssertEqual(store.supervisorOut, "[supervisor] lease renewed epoch 7\n")
        try append("supervisor.out", in: jobDir, "[gate] evaluate deferred\n")
        await store.pollOnce()
        XCTAssertTrue(store.supervisorOut.hasSuffix("[gate] evaluate deferred\n"))
        store.close()
    }

    // MARK: - Reopen (open -> close -> open on the SAME store)

    func testReopenAfterCloseResumesFeedsWithoutDuplication() async throws {
        try writeProjection()
        try writeRunState()
        try append("events.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:01Z", "run_id": "run-new", "event": {"kind": "turn_started", "turn": 1}}"# + "\n")
        try append("supervisor.out", in: jobDir, "[supervisor] first line\n")
        let store = makeStore()
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, runID)
        XCTAssertEqual(store.activity.count, 1)
        XCTAssertEqual(store.supervisorOut, "[supervisor] first line\n")

        store.close()
        XCTAssertEqual(store.activeTailCount, 0)
        XCTAssertNil(store.currentRunID, "close() resets run resolution so reopen can rebuild")
        XCTAssertEqual(store.supervisorOut, "", "close() clears supervisor text so reopen cannot duplicate it")

        // More activity lands while the workbench is closed.
        try append("events.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:05Z", "run_id": "run-new", "event": {"kind": "turn_started", "turn": 2}}"# + "\n")

        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, runID, "reopen re-resolves the run root")
        XCTAssertEqual(store.activeTailCount, 9, "reopen rebuilds every tail")
        XCTAssertEqual(store.activity.map(\.line), ["turn 1 started", "turn 2 started"],
                       "the run feeds resume after reopen instead of freezing")
        XCTAssertEqual(store.turns.map(\.turn), [1, 2])
        XCTAssertEqual(store.supervisorOut, "[supervisor] first line\n",
                       "supervisor text re-reads exactly once, never appended twice")
        store.close()
    }

    // MARK: - Per-tail corruption is surfaced, never a silent freeze

    func testCorruptStrictTailsSurfaceTheirOwnIssues() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        try append("spend.jsonl", in: runRoot,
                   #"{"timestamp": "2026-08-06T10:00:00Z", "turn": 1, "provider": "p", "model": "m", "input_tokens": 10, "output_tokens": 5, "cost_usd": 1.0, "total_cost_usd": 1.0, "cap_usd": 25.0, "kind": "loop"}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.spendMeter.loopTotalUSD, 1.0)

        // Complete lines that are not valid JSON: strict-file corruption.
        try append("spend.jsonl", in: runRoot, "not json\n")
        try append("traces.jsonl", in: runRoot, "not json\n")
        try append("flight-events.jsonl", in: runRoot, "not json\n")
        await store.pollOnce()
        XCTAssertNotNil(store.spendIssue, "a corrupt spend tail must not freeze silently")
        XCTAssertNotNil(store.tracesIssue, "a corrupt traces tail must not freeze silently")
        XCTAssertNotNil(store.flightIssue, "a corrupt flight tail must not freeze silently")
        XCTAssertTrue(store.spendIssue?.contains("not valid JSON") == true, store.spendIssue ?? "")
        XCTAssertEqual(store.spendMeter.loopTotalUSD, 1.0,
                       "already-read data stays visible with the reason")
        store.close()
    }

    // MARK: - Snapshots: unblessed file rides restart-on-anomaly, never sticky

    func testSnapshotsRewriteRefoldsInsteadOfFreezing() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        try append("narrative/snapshots.jsonl", in: runRoot,
                   #"{"snapshot_id": "s1", "created_at": "2026-08-06T10:00:00Z", "status": "deterministic", "headline": "one", "current_work": [], "risks": [], "next_likely": []}"# + "\n")
        try append("narrative/snapshots.jsonl", in: runRoot,
                   #"{"snapshot_id": "s2", "created_at": "2026-08-06T10:01:00Z", "status": "fresh", "headline": "two", "current_work": [], "risks": [], "next_likely": []}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.narrative.latestSnapshot?.snapshotID, "s2")

        // The file shrinks (rewrite): narrative/snapshots.jsonl is NOT a
        // TAILING.md-blessed ledger, so this must re-fold the fresh content
        // rather than freezing the pane on a sticky corruption verdict.
        try write("narrative/snapshots.jsonl", in: runRoot,
                  #"{"snapshot_id": "s3", "created_at": "2026-08-06T10:02:00Z", "status": "deterministic", "headline": "three", "current_work": [], "risks": [], "next_likely": []}"# + "\n")
        await store.pollOnce()
        XCTAssertEqual(store.narrative.latestSnapshot?.snapshotID, "s3")
        XCTAssertEqual(store.narrative.latestDeterministic?.snapshotID, "s3",
                       "the pane reflects exactly what the file now says")
        store.close()
    }

    // MARK: - Projection transient failures keep the last good checkpoint

    func testProjectionTransientUnreadableKeepsLastGoodAndSurfacesIssue() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.currentRunID, runID)
        XCTAssertEqual(store.activeTailCount, 9)

        // Mid-write torn JSON: a one-tick fs hiccup must not fabricate
        // "no attempt yet" or churn the run tailers.
        try write("projection.json", in: jobDir, "{ torn mid-write")
        await store.pollOnce()
        XCTAssertNotNil(store.projection, "the last good checkpoint is kept")
        XCTAssertNotNil(store.projectionIssue, "the transient failure is surfaced, not guessed around")
        XCTAssertEqual(store.currentRunID, runID, "run resolution rides the last good read")
        XCTAssertEqual(store.activeTailCount, 9, "no tailer churn on a transient read failure")
        XCTAssertNotNil(store.spine, "the spine band does not regress to pending")

        try writeProjection()
        await store.pollOnce()
        XCTAssertNil(store.projectionIssue, "a successful re-read clears the issue")
        store.close()
    }

    // MARK: - Spine NEXT rides the job envelope, never a fenced verb

    private var failedStatusJSON: String {
        """
        {"kind": "job_status", "id": "\(jobID)", "status": "failed",
         "next_actions": ["deadreckon report \(jobID)"],
         "verified_proof": {"status": "not-applicable", "error": null},
         "job": {"job": {"job_id": "\(jobID)", "scope": "\(scope)", "goal": "fix it"},
                 "attempts": []}}
        """
    }

    func testSpineNextPrefersJobEnvelopeNextActions() async throws {
        try writeProjection()
        try writeRunState(status: "failed")
        let cli = FakeCLI()
        cli.scriptStdout("status", failedStatusJSON)
        let store = makeStore(cli: cli)
        store.open()
        await store.pollOnce()   // fetches the status envelope
        await store.pollOnce()   // derives the spine with it
        XCTAssertEqual(store.spine?.next, "deadreckon report \(jobID)",
                       "NEXT is the job envelope's own next_actions (the friendliness contract)")
        store.close()
    }

    func testSpineNextForFailedAttemptWithoutEnvelopeObservesInsteadOfResume() async throws {
        // Public `resume` on a job-owned run is refused by the ownership
        // fence: the workbench must never suggest it. With no envelope, the
        // honest fallback observes via `status`.
        try writeProjection()
        try writeRunState(status: "failed")
        let store = makeStore()   // unscripted FakeCLI: status exits 2
        store.open()
        await store.pollOnce()
        XCTAssertEqual(store.spine?.next, "deadreckon status \(jobID)")
        XCTAssertNotEqual(store.spine?.next, "deadreckon resume \(runID)")
        store.close()
    }

    // MARK: - Changes: the Single-shape aliasing is named, not a generic error

    func testShowReturningJobStatusExplainsTheAliasing() async throws {
        try writeProjection()
        try writeRunState()
        let cli = FakeCLI()
        cli.scriptStdout("show", #"{"kind": "job_status", "id": "job-1", "status": "running"}"#)
        let store = makeStore(cli: cli)
        store.open()
        await store.pollOnce()
        await store.refreshChanges()
        XCTAssertNil(store.changes)
        XCTAssertTrue(store.changesIssue?.contains("resolved to the Job itself") == true,
                      store.changesIssue ?? "nil")
        store.close()
    }

    // MARK: - Documented ceilings (CONTRACTS.md)

    func testRawEventLinesAreBoundedWithAnHonestDroppedCount() async throws {
        try writeProjection()
        try writeRunState()
        let store = makeStore()
        store.open()
        var lines = ""
        for turn in 1...2_600 {
            lines += #"{"timestamp": "2026-08-06T10:00:00Z", "run_id": "run-new", "event": {"kind": "turn_started", "turn": \#(turn)}}"# + "\n"
        }
        try append("events.jsonl", in: runRoot, lines)
        await store.pollOnce()
        XCTAssertEqual(store.activity.count, 2_600, "the parsed Activity scrollback stays unbounded")
        XCTAssertEqual(store.rawEventLines.count, JobDetailStore.rawEventLineCeiling)
        XCTAssertEqual(store.rawEventsDropped, 600, "dropped raw lines are counted, not hidden")
        XCTAssertTrue(store.rawEventLines.first?.contains("\"turn\": 601") == true,
                      "the window keeps the TRAILING lines")
        store.close()
    }

    func testSupervisorTextIsBoundedWithTruncationHonesty() async throws {
        try writeProjection()
        let store = makeStore()
        store.open()
        let line = String(repeating: "x", count: 99) + "\n"
        let big = String(repeating: line, count: 3_300)   // 330,000 chars
        try append("supervisor.out", in: jobDir, big + "final line\n")
        await store.pollOnce()
        XCTAssertLessThanOrEqual(store.supervisorOut.count, JobDetailStore.supervisorTextCeiling)
        XCTAssertTrue(store.supervisorOutTruncated, "trimming is announced, never silent")
        XCTAssertTrue(store.supervisorOut.hasSuffix("final line\n"), "the tail is what survives")
        XCTAssertFalse(store.supervisorErrTruncated, "the quiet err pane is untouched")
        store.close()
    }
}
