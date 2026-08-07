import XCTest

@testable import DeadreckonKit

/// Decode coverage for the Chartroom read-models against literal fixtures of
/// the committed Rust surfaces.
final class DetailModelDecodingTests: XCTestCase {
    private func decode<T: Decodable>(_ json: String) throws -> T {
        try DeadreckonJSON.decoder().decode(T.self, from: Data(json.utf8))
    }

    // MARK: show --diff --json (G10)

    func testDiffSummaryDecodesWithPatchesAndTruncationHonesty() throws {
        let summary: DiffSummaryModel = try decode(
            """
            {
              "files_changed": 2, "added": 30, "removed": 4,
              "files": [
                {"path": "src/a.rs", "added": 25, "removed": 0, "status": "added"},
                {"path": "src/b.rs", "added": 5, "removed": 4, "status": "modified"}
              ],
              "patches": [
                {"path": "src/b.rs", "status": "modified",
                 "unified": "--- a/src/b.rs\\n+++ b/src/b.rs\\n@@ -1 +1 @@\\n-x\\n+y\\n",
                 "truncated": true},
                {"path": "img.png", "status": "added", "unified": "",
                 "truncated": false, "note": "binary or unreadable diff"}
              ]
            }
            """)
        XCTAssertEqual(summary.filesChanged, 2)
        XCTAssertEqual(summary.files[0].status, "added")
        XCTAssertEqual(summary.patches?.count, 2)
        XCTAssertEqual(summary.patches?[0].truncated, true)
        XCTAssertEqual(summary.patches?[1].note, "binary or unreadable diff")
    }

    func testDiffSummaryWithoutPatchesDecodes() throws {
        let summary: DiffSummaryModel = try decode(
            """
            {"files_changed": 0, "added": 0, "removed": 0, "files": []}
            """)
        XCTAssertNil(summary.patches)
    }

    // MARK: status <job> --json

    func testJobStatusEnvelopeExposesLastAttemptSteerable() throws {
        let envelope: JobStatusEnvelope = try decode(
            """
            {
              "kind": "job_status", "id": "job-1", "status": "running",
              "verified_proof": {"status": "not-applicable", "error": null},
              "work_clock": {"active_elapsed_seconds": 12.5, "remaining_seconds": 100.0,
                             "cutoff": "2026-08-06T12:00:00Z", "limiting_boundary": "wall"},
              "job": {
                "job": {"job_id": "job-1", "scope": "demo-1234", "goal": "fix it",
                        "source_cwd": "/tmp/src",
                        "policy": {"max_spend_usd": 25.0, "max_wall_seconds": 3600,
                                   "max_attempts": 3,
                                   "execution": {"gate": {"network": "deny"}}}},
                "projection": {"job_id": "job-1", "phase": "running"},
                "attempts": [
                  {"id": {"scope": "demo-1234", "run_id": "run-old", "short": "run-old"},
                   "status": "failed", "steerable": {"steerable": false, "reason": "not_executing"},
                   "provider": "cli:claude-code"},
                  {"id": {"scope": "demo-1234", "run_id": "run-new", "short": "run-new"},
                   "status": "executing", "steerable": {"steerable": true},
                   "provider": "cli:codex-server"}
                ]
              }
            }
            """)
        XCTAssertEqual(envelope.status, "running")
        XCTAssertEqual(envelope.verifiedProof?.status, .notApplicable)
        XCTAssertEqual(envelope.workClock?.limitingBoundary, "wall")
        XCTAssertEqual(envelope.currentAttempt?.id.runID, "run-new")
        XCTAssertEqual(envelope.currentSteerable, SteerEligibility(steerable: true, reason: nil))
        XCTAssertEqual(envelope.job?.job.policy?.execution?.gate?.network, "deny")
    }

    func testJobStatusEnvelopeWithoutAttemptsHasNoSteerableGuess() throws {
        let envelope: JobStatusEnvelope = try decode(
            """
            {"kind": "job_status", "id": "job-2", "status": "queued",
             "job": {"job": {"job_id": "job-2", "scope": "s", "goal": "g"},
                     "attempts": []}}
            """)
        XCTAssertNil(envelope.currentSteerable)
        XCTAssertNil(envelope.currentAttempt)
    }

    // MARK: report <job> --json

    func testJobReportDecodesContractChecksDigestsAndTwoKeys() throws {
        let report: JobReportEnvelope = try decode(
            """
            {
              "id": "job-1", "goal": "fix it", "phase": "terminal",
              "outcome": "verified", "stop_reason": "verified",
              "contract": {
                "path": "/home/jobs/job-1/acceptance.yaml",
                "approved_sha256": "abc", "current_sha256": "abc",
                "matches_approved_digest": true,
                "spec": {"name": "done", "checks": [
                  {"kind": "cargo_test", "args": ["--lib"], "must_pass": true},
                  {"kind": "shell", "command": "./scripts/smoke.sh", "must_pass": true},
                  {"kind": "content_match", "path": "CHANGELOG.md", "pattern": "retry",
                   "must_pass": false},
                  {"kind": "sonar_ping", "frequency": 7}
                ]}
              },
              "deterministic_checks": [
                {"kind": "cargo_test", "passed": true, "must_pass": true,
                 "detail": "212 passed", "duration_ms": 41300}
              ],
              "semantic": {"judgment": {"decision": "achieved",
                                        "summary": "ledger v2 written", "judged_at": "2026-08-06T07:42:00Z"}},
              "attempts": [
                {"run_id": "run-new", "status": "completed", "provider": "cli:codex-server",
                 "spend_usd": 9.12, "checks": []}
              ],
              "receipt": {"status": "valid", "contained": true,
                          "sandbox_backend": "sandbox-exec",
                          "signature_validation_error": null}
            }
            """)
        let rows = report.contract?.checkRows ?? []
        XCTAssertEqual(rows.count, 4)
        XCTAssertEqual(rows[0].kind, "cargo_test")
        XCTAssertEqual(rows[0].subject, "cargo test --lib")
        XCTAssertEqual(rows[1].subject, "./scripts/smoke.sh")
        XCTAssertEqual(rows[2].mustPass, false)
        XCTAssertEqual(rows[2].subject, "CHANGELOG.md =~ retry")
        // Unknown kind survives with its raw kind and an empty subject.
        XCTAssertEqual(rows[3].kind, "sonar_ping")
        XCTAssertEqual(report.contract?.matchesApprovedDigest, true)
        XCTAssertEqual(report.semantic?.judgment?.decision, "achieved")
        XCTAssertEqual(report.receipt?.contained, true)
        XCTAssertEqual(report.deterministicChecks.first?.durationMS, 41300)
    }

    // MARK: verdict --receipt --json (G7)

    func testVerdictEnvelopeCarriesPerDigestAuditFacts() throws {
        let verdict: VerdictEnvelope = try decode(
            """
            {
              "kind": "verdict", "id": "run-new", "status": "verified",
              "had_signed_marker": true, "marker_valid": true,
              "checks": [{"kind": "cargo_test", "passed": true, "must_pass": true,
                          "detail": "212 passed"}],
              "receipt_audit": {"facts": [
                {"name": "goal_sha256", "pass": true, "detail": "matches"},
                {"name": "result_tree_sha256", "pass": false,
                 "detail": "result tree drifted after signing"}
              ]}
            }
            """)
        XCTAssertEqual(verdict.status, "verified")
        XCTAssertEqual(verdict.receiptAudit?.facts.count, 2)
        XCTAssertEqual(verdict.receiptAudit?.facts[1].pass, false)
    }

    // MARK: narrative

    func testNarrativeSnapshotDeterministicVsOverlay() throws {
        let deterministic: NarrativeSnapshotDoc = try decode(
            """
            {"snapshot_id": "s1", "created_at": "2026-08-06T10:00:00Z",
             "status": "deterministic", "headline": "run created",
             "current_work": [], "risks": [], "next_likely": []}
            """)
        XCTAssertFalse(deterministic.isUnverifiedOverlay)

        let overlay: NarrativeSnapshotDoc = try decode(
            """
            {"snapshot_id": "s2", "created_at": "2026-08-06T10:01:00Z",
             "status": "fresh", "headline": "model prose",
             "current_work": [{"text": "claim", "evidence": ["e1"], "confidence": "high"}],
             "risks": [], "next_likely": [],
             "live": {"beat_seq": 31, "covers_turn": 12, "source": "live",
                      "rolling_summary": "so far"}}
            """)
        XCTAssertTrue(overlay.isUnverifiedOverlay)
        XCTAssertEqual(overlay.live?.beatSeq, 31)

        // Any unrecognized status fails TOWARD the unverified label.
        let odd: NarrativeSnapshotDoc = try decode(
            """
            {"snapshot_id": "s3", "created_at": "2026-08-06T10:02:00Z",
             "status": "hologram", "headline": "?", "current_work": [],
             "risks": [], "next_likely": []}
            """)
        XCTAssertTrue(odd.isUnverifiedOverlay)
    }

    func testNarrativeStalenessThresholds() {
        let now = Date(timeIntervalSince1970: 1_700_000_000)
        XCTAssertEqual(
            NarrativeStaleness.from(createdAt: now.addingTimeInterval(-12), now: now),
            .fresh(ageSeconds: 12))
        XCTAssertEqual(
            NarrativeStaleness.from(createdAt: now.addingTimeInterval(-91), now: now),
            .stale(ageSeconds: 91))
        XCTAssertEqual(NarrativeStaleness.from(createdAt: nil, now: now), .unknown)
    }

    func testNarrativeStateDocDecodes() throws {
        let doc: NarrativeStateDoc = try decode(
            """
            {"version": 2, "scope": "run", "target_id": "run-new",
             "latest_snapshot_id": "s2", "latest_status": "stale",
             "latest_created_at": "2026-08-06T10:01:00.250Z",
             "latest_covered": {}, "cadence": {"mode": "event-driven",
             "min_seconds_between_provider_calls": 45, "quiet_seconds": 30,
             "max_provider_calls_per_attach": 20},
             "provider": {"route": null, "source": "none", "model": null,
             "calls": 0, "cost_usd": 0.0, "subscription_seconds": 0.0},
             "last_error": "provider timeout"}
            """)
        XCTAssertEqual(doc.latestStatus, "stale")
        XCTAssertEqual(doc.lastError, "provider timeout")
        XCTAssertNotNil(doc.latestCreatedAt)
    }

    // MARK: state.json

    func testRunStateDocDecodesPhasesAndActivePhase() throws {
        let state: RunStateDoc = try decode(
            """
            {"version": 1, "goal": "fix it", "task_key": "fix-it-1234",
             "run_id": "run-new", "scope": "demo-1234", "status": "executing",
             "current_phase_id": 20, "started_at": "2026-08-06T09:00:00Z",
             "updated_at": "2026-08-06T10:00:00Z", "cwd": "/tmp",
             "run_root": "/home/runstate/demo-1234/runs/run-new",
             "working_dir": "/tmp/work", "skill_name": "deadreckon",
             "skill_path": "/tmp/skill", "sandbox": "sandbox-exec",
             "provider": "cli:codex-server", "max_spend_usd": 25.0,
             "total_spend_usd": 4.83, "total_wall_seconds": 9660.0,
             "turn": 14, "pause_reason": null, "failure_reason": null,
             "child_pids": [], "killed_at": null,
             "phases": [
               {"id": 10, "name": "plan", "status": "completed", "plan_path": null,
                "updated_at": "2026-08-06T09:10:00Z"},
               {"id": 20, "name": "implement", "status": "executing", "plan_path": null,
                "updated_at": "2026-08-06T10:00:00Z"}
             ]}
            """)
        XCTAssertEqual(state.status, "executing")
        XCTAssertEqual(state.activePhaseName, "implement")
        XCTAssertEqual(state.phases.count, 2)
        XCTAssertEqual(state.phases[0].status, "completed")
    }

    // MARK: flight

    func testCheckpointManifestCountsFilesWithoutModelingThem() throws {
        let doc: CheckpointManifestDoc = try decode(
            """
            {"version": 1, "checkpoint_id": "ckpt-041", "run_id": "run-new",
             "flight_session_id": "fs-1", "deadreckon_turn": 12, "attempt": 1,
             "created_at": "2026-08-06T10:00:00Z", "trigger": "provider_tool",
             "base": {"kind": "turn_snapshot", "id": "t12"}, "full_anchor": false,
             "files": [{"path": "a.rs", "change": "modified"},
                        {"path": "b.rs", "change": "created"}],
             "working_tree_hash": "abc"}
            """)
        XCTAssertEqual(doc.checkpointID, "ckpt-041")
        XCTAssertEqual(doc.fileCount, 2)
        XCTAssertEqual(doc.trigger, "provider_tool")
    }

    func testFlightManifestDecodesSessions() throws {
        let doc: FlightManifestDoc = try decode(
            """
            {"version": 1, "run_id": "run-new",
             "sessions": [{"flight_session_id": "fs-1", "provider": "cli:codex-server",
                           "schema": "codex-v1", "deadreckon_turn": 3, "attempt": 1,
                           "status": "running", "started_at": "2026-08-06T09:00:00Z"}],
             "checkpoint_policy": {"mode": "delta-with-anchors", "quiet_ms": 750,
                                   "poll_ms": 500, "anchor_every": 20}}
            """)
        XCTAssertEqual(doc.sessions.first?.status, "running")
    }

    // MARK: projection.json

    func testJobProjectionDocResolvesCurrentRun() throws {
        let doc: JobProjectionDoc = try decode(
            """
            {"schema_version": 1, "job_id": "job-1", "phase": "running",
             "outcome": null, "stop_reason": null, "last_sequence": 42,
             "current_lease_epoch": 7, "attempt_count": 2,
             "child_run_ids": ["run-old", "run-new"],
             "updated_at": "2026-08-06T10:00:00Z", "caveats": []}
            """)
        XCTAssertEqual(doc.childRunIDs.last, "run-new")
        XCTAssertEqual(doc.phase, .running)
        XCTAssertEqual(doc.lastSequence, 42)
    }
}
