import XCTest

@testable import DeadreckonKit

/// Envelope decode for every verb APP-4 dispatches, success and error, with
/// fixtures shaped exactly like the committed machine_json.rs emitter and
/// the per-verb fact builders (kill_outcome_facts, steer.rs,
/// print_materialized, apply_outcome_facts, extend_queue_facts).
final class MutationEnvelopeTests: XCTestCase {

    // MARK: - Error envelope (G1 global refusal)

    func testErrorEnvelopeDecodesVerbatimWithTryLines() throws {
        let fixture = """
        {
          "kind": "error",
          "code": 1,
          "verb": "steer",
          "message": "run abc12345 is executing and cannot accept steering",
          "try_lines": ["deadreckon extend abc12345 \\"goal\\""]
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 1)
        let refusal = try XCTUnwrap(result.refusal)
        XCTAssertEqual(refusal.code, 1)
        XCTAssertEqual(refusal.verb, "steer")
        XCTAssertEqual(refusal.message, "run abc12345 is executing and cannot accept steering")
        XCTAssertEqual(refusal.tryLines, ["deadreckon extend abc12345 \"goal\""])
        XCTAssertFalse(result.isSuccess)
        XCTAssertTrue(result.envelopes.isEmpty)
    }

    // MARK: - steer

    func testSteerSuccessEnvelopeCarriesQueuedFacts() throws {
        let fixture = """
        {
          "kind": "steer",
          "id": "abc12345",
          "status": "completed",
          "next_actions": ["deadreckon attach abc12345"],
          "try_lines": [],
          "queued_at": "2026-08-07T00:00:00.123456Z",
          "inbox_seq": 2,
          "source": "cli",
          "delivery": "next turn boundary",
          "primary_action": "deadreckon attach abc12345",
          "verdict": {"kind": "completed"}
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        XCTAssertTrue(result.isSuccess)
        let envelope = try XCTUnwrap(result.primary)
        XCTAssertEqual(envelope.kind, "steer")
        XCTAssertEqual(envelope.id, "abc12345")
        XCTAssertEqual(envelope.status, "completed")
        let steer = try XCTUnwrap(envelope.steer)
        XCTAssertEqual(steer.inboxSeq, 2)
        XCTAssertEqual(steer.queuedAtRaw, "2026-08-07T00:00:00.123456Z")
        XCTAssertNotNil(steer.queuedAt)
        XCTAssertEqual(steer.source, "cli")
        XCTAssertEqual(steer.delivery, "next turn boundary")
    }

    // MARK: - kill

    func testKillRunEnvelopeCarriesSignalFacts() throws {
        let fixture = """
        {
          "kind": "kill",
          "id": "run-7be410",
          "status": "killed",
          "next_actions": ["deadreckon show run-7be410 --why-failed"],
          "try_lines": [],
          "signal": "SIGTERM",
          "escalated": false,
          "terminal_phase_observed": true,
          "verdict": {"kind": "killed"}
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let kill = try XCTUnwrap(result.primary?.kill)
        XCTAssertEqual(kill.signal, "SIGTERM")
        XCTAssertFalse(kill.escalated)
        XCTAssertTrue(kill.terminalPhaseObserved)
        XCTAssertNil(kill.processesSignalled)
    }

    func testJobKillEnvelopeWithSignalNoneDecodes() throws {
        let fixture = """
        {
          "kind": "kill",
          "id": "job-1",
          "status": "cancel requested",
          "next_actions": ["deadreckon status job-1"],
          "try_lines": [],
          "signal": "none",
          "escalated": false,
          "terminal_phase_observed": false,
          "verified_proof": {"status": "not-applicable", "error": null},
          "queued": null
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let kill = try XCTUnwrap(result.primary?.kill)
        XCTAssertEqual(kill.signal, "none")
        XCTAssertFalse(kill.terminalPhaseObserved)
    }

    // MARK: - campaign kill: concatenated envelope stream

    func testCampaignKillStreamDecodesEveryEnvelope() throws {
        let subPlan = """
        {
          "kind": "kill",
          "id": "plan-sub-%d",
          "status": "killed",
          "next_actions": ["deadreckon show plan-sub-%d --why-failed"],
          "try_lines": [],
          "signal": "SIGTERM",
          "escalated": false,
          "terminal_phase_observed": true,
          "processes_signalled": %d
        }
        """
        let campaign = """
        {
          "kind": "kill",
          "id": "campaign-b33f10",
          "status": "killed",
          "next_actions": ["deadreckon status campaign-b33f10"],
          "try_lines": [],
          "signal": "SIGTERM",
          "escalated": false,
          "terminal_phase_observed": true
        }
        """
        let stdout = String(format: subPlan, 1, 1, 3) + "\n"
            + String(format: subPlan, 2, 2, 2) + "\n" + campaign
        let objects = EnvelopeStreamParser.objects(in: stdout)
        XCTAssertEqual(objects.count, 3, "the stream is concatenated objects, not one document")

        let result = MutationResult.classify(stdout: stdout, stderr: "", exitCode: 0)
        XCTAssertEqual(result.envelopes.count, 3, "every envelope is surfaced")
        XCTAssertEqual(result.primary?.id, "campaign-b33f10",
                       "the campaign envelope comes last and is the primary")
        XCTAssertEqual(result.envelopes[0].kill?.processesSignalled, 3)
        XCTAssertEqual(result.envelopes[1].kill?.processesSignalled, 2)
    }

    func testStreamParserSurvivesBracesInsideStrings() {
        let tricky = #"{"kind":"kill","id":"x","message":"a {brace\"} inside"}{"kind":"kill","id":"y"}"#
        XCTAssertEqual(EnvelopeStreamParser.objects(in: tricky).count, 2)
    }

    // MARK: - finish / materialize / apply delivery facts

    func testFinishInPlaceEnvelopeDecodesDestination() throws {
        let fixture = """
        {
          "kind": "finish",
          "id": "run-90ab1e",
          "status": "completed",
          "next_actions": ["deadreckon show run-90ab1e"],
          "try_lines": [],
          "destination": {"kind": "in-place", "path": "/Users/op/src/proj"},
          "staged_file_count": null,
          "receipt_validated": true
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let delivery = try XCTUnwrap(result.primary?.delivery)
        XCTAssertEqual(delivery.destinationKind, "in-place")
        XCTAssertEqual(delivery.destination, "/Users/op/src/proj")
        XCTAssertNil(delivery.stagedFileCount, "null staged count stays nil, never guessed")
        XCTAssertEqual(delivery.receiptValidated, true)
    }

    func testMaterializeExportEnvelopeDecodesStagedCount() throws {
        let fixture = """
        {
          "kind": "finish",
          "id": "run-a3f9e2",
          "status": "completed",
          "next_actions": ["deadreckon show run-a3f9e2"],
          "try_lines": [],
          "destination": {"kind": "export", "path": "/Users/op/reviews/ledger-v2"},
          "source": "library",
          "staged_file_count": 142,
          "receipt_validated": true
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let delivery = try XCTUnwrap(result.primary?.delivery)
        XCTAssertEqual(delivery.destinationKind, "export")
        XCTAssertEqual(delivery.stagedFileCount, 142)
        XCTAssertEqual(delivery.source, "library")
    }

    func testApplyEnvelopeDecodesStrategyFacts() throws {
        let fixture = """
        {
          "kind": "finish",
          "id": "run-9f3e21",
          "status": "completed",
          "next_actions": ["deadreckon undo"],
          "try_lines": [],
          "destination": {"kind": "git-branch", "target": "main"},
          "strategy": "squash",
          "cleaned": true,
          "receipt_validated": true,
          "already_applied": false,
          "staged_file_count": 7
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let delivery = try XCTUnwrap(result.primary?.delivery)
        XCTAssertEqual(delivery.destinationKind, "git-branch")
        XCTAssertEqual(delivery.destination, "main")
        XCTAssertEqual(delivery.strategy, "squash")
        XCTAssertEqual(delivery.cleaned, true)
        XCTAssertEqual(delivery.alreadyApplied, false)
        XCTAssertEqual(delivery.stagedFileCount, 7)
    }

    // MARK: - extend (G9)

    func testExtendEnvelopeCarriesSendBackFacts() throws {
        let fixture = """
        {
          "kind": "extend",
          "id": "job-new-1",
          "status": "queued",
          "next_actions": ["deadreckon attach job-new-1"],
          "try_lines": [],
          "queued": true,
          "parent_id": "run-parent-1",
          "parent_run_id": "run-parent-1",
          "contract": "inherited",
          "note_recorded": true,
          "verified_proof": {"status": "not-applicable", "error": null}
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        let envelope = try XCTUnwrap(result.primary)
        XCTAssertEqual(envelope.kind, "extend",
                       "G9 as built: the kind stays extend, no extend_result kind exists")
        let extend = try XCTUnwrap(envelope.extend)
        XCTAssertEqual(extend.parentRunID, "run-parent-1")
        XCTAssertEqual(extend.contract, "inherited")
        XCTAssertEqual(extend.noteRecorded, true)
        XCTAssertEqual(extend.queued, true)
    }

    // MARK: - launch execute envelope

    func testLaunchEnvelopeDecodesGenerically() throws {
        let fixture = """
        {
          "kind": "launch",
          "id": "job-9f3e21",
          "status": "queued",
          "next_actions": ["deadreckon attach job-9f3e21"],
          "try_lines": []
        }
        """
        let result = MutationResult.classify(stdout: fixture, stderr: "", exitCode: 0)
        XCTAssertEqual(result.primary?.kind, "launch")
        XCTAssertEqual(result.primary?.id, "job-9f3e21")
    }

    // MARK: - The G1 carve-out: no envelope at all

    func testClapParseFailureIsEnvelopeFreeNeverInvented() {
        let result = MutationResult.classify(
            stdout: "",
            stderr: "error: unexpected argument '--dry-run' found",
            exitCode: 2)
        XCTAssertTrue(result.isEnvelopeFree)
        XCTAssertNil(result.refusal)
        XCTAssertTrue(result.envelopeFreeWords.contains("--dry-run"))
    }

    // MARK: - start preview (G2)

    func testStartPreviewDecodesAndExtractsReplayablePlanBytes() throws {
        let fixture = """
        {
          "kind": "start",
          "goal": "add retry queue",
          "selected_mode": "single",
          "selection_source": "auto",
          "reason": "single durable job",
          "provider": "cli:codex-server",
          "provider_source": "configured",
          "done_criteria": ".deadreckon/acceptance.yaml",
          "done_criteria_source": "detected",
          "done_contract": {
            "capabilities": {"network": "deny"},
            "checks": [
              {"kind": "cargo_test", "must_pass": true},
              {"kind": "shell", "command": "./scripts/smoke.sh"}
            ],
            "divergence": null
          },
          "source_mode": "worktree",
          "requires_confirmation": true,
          "will_start": false,
          "next_actions": ["deadreckon start --plan launch-plan.json --yes"],
          "try_lines": [],
          "launch_plan": {
            "plan_id": "lp-1",
            "budget": {"ceiling_usd": 60, "wall_seconds": 28800},
            "pieces": [{"id": "run", "budget_usd": 60}]
          }
        }
        """
        let preview = try XCTUnwrap(StartPreviewEnvelope(data: Data(fixture.utf8)))
        XCTAssertFalse(preview.willStart)
        XCTAssertTrue(preview.isLaunchable)
        XCTAssertEqual(preview.provider, "cli:codex-server")
        XCTAssertEqual(preview.doneContract?.network, "deny")
        XCTAssertEqual(preview.doneContract?.checks.map(\.kind), ["cargo_test", "shell"])
        XCTAssertEqual(preview.planCeilingUSD, 60)

        // The replayable payload survives byte-honest: integers stay
        // integers (never laundered through a Double), and the re-parsed
        // structure equals the envelope's own launch_plan subobject.
        let planData = try XCTUnwrap(preview.launchPlanData)
        let planText = String(decoding: planData, as: UTF8.self)
        XCTAssertTrue(planText.contains("\"ceiling_usd\":60"),
                      "integer cap must not become 60.0: \(planText)")
        let reparsed = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: planData) as? NSDictionary)
        let original = try XCTUnwrap(
            (try JSONSerialization.jsonObject(with: Data(fixture.utf8)) as? [String: Any])?["launch_plan"]
                as? NSDictionary)
        XCTAssertEqual(reparsed, original)
    }

    func testBlockedStartPreviewOmitsPlanAndIsNotLaunchable() throws {
        let fixture = """
        {
          "kind": "start",
          "goal": "add retry queue",
          "requires_confirmation": false,
          "will_start": false,
          "next_actions": [],
          "try_lines": ["deadreckon def-done \\"what should count as done\\""]
        }
        """
        let preview = try XCTUnwrap(StartPreviewEnvelope(data: Data(fixture.utf8)))
        XCTAssertFalse(preview.isLaunchable)
        XCTAssertEqual(preview.tryLines.count, 1)
    }

    // MARK: - finish_plan (G4, spec-true decode)

    func testFinishPlanDecodesExactlyTheG4Shape() throws {
        let fixture = """
        {
          "kind": "finish_plan",
          "id": "job-9f2c31",
          "staged": [
            {"path": "src/dispatch/drain.rs", "bytes": 4096, "sha256": "9c41aa00"},
            {"path": "src/dispatch/retry.rs", "bytes": 8192, "sha256": "e0c194aa"}
          ],
          "diffstat": {"files_changed": 7, "added": 312, "removed": 64},
          "destination": {"kind": "export", "path": "/Users/op/reviews/webhook"},
          "irreversible_steps": ["publish", "cleanup"]
        }
        """
        let plan = try DeadreckonJSON.decoder().decode(
            FinishPlanEnvelope.self, from: Data(fixture.utf8))
        XCTAssertEqual(plan.kind, "finish_plan")
        XCTAssertEqual(plan.staged.count, 2)
        XCTAssertEqual(plan.staged[0].path, "src/dispatch/drain.rs")
        XCTAssertEqual(plan.staged[0].bytes, 4096)
        XCTAssertEqual(plan.staged[0].sha256, "9c41aa00")
        XCTAssertEqual(plan.diffstat?.filesChanged, 7)
        XCTAssertEqual(plan.destination?.kind, "export")
        XCTAssertEqual(plan.irreversibleSteps, ["publish", "cleanup"])
    }
}
