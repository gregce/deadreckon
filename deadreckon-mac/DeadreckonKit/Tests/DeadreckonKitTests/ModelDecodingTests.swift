import XCTest

@testable import DeadreckonKit

/// Fixture JSON below is derived line-for-line from the committed M0 Rust
/// surfaces: `job_list_row`/`run_list_row` (commands/inspection.rs),
/// docs/schemas/job-lease.schema.json, job-event.schema.json,
/// spend-record.schema.json, projections/run-view.schema.json (steerable{}),
/// and docs/TAILING.md (notify.jsonl, acceptance-progress.jsonl).
final class ModelDecodingTests: XCTestCase {
    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try DeadreckonJSON.decoder().decode(type, from: Data(json.utf8))
    }

    // MARK: - FleetRow

    private let fullJobRow = """
        {
          "job_id": "job-20260806-021542-verify-ledger",
          "scope": "deadreckon",
          "goal": "Verify the spend ledger rollup",
          "status": "verified",
          "updated_at": "2026-08-06T02:19:42.913402Z",
          "attempts": 2,
          "outcome": "verified",
          "stop_reason": "verified",
          "projection": {
            "phase": "terminal",
            "outcome": "verified",
            "stop_reason": "verified",
            "attempt_count": 2,
            "caveats": ["semantic judge was optional and unavailable"]
          },
          "lease": {
            "owner_id": "supervisor-7f3a",
            "epoch": 4,
            "heartbeat_age_seconds": 3,
            "expires_at": "2026-08-06T02:20:12Z",
            "fresh": true
          },
          "spend": {
            "total_cost_usd": 1.8342,
            "cap_usd": 5.0,
            "subscription": false,
            "wall_time_seconds": 412.7
          },
          "provider": "cli:codex-server",
          "sandbox": "seatbelt",
          "receipt": {
            "present": true,
            "verified": "valid",
            "error": null
          },
          "gate": {
            "attempt": 2,
            "n_passed": 5,
            "n_total": 5
          }
        }
        """

    func testDecodesFullJobRow() throws {
        let row = try decode(FleetRow.self, fullJobRow)
        XCTAssertEqual(row.jobID, "job-20260806-021542-verify-ledger")
        XCTAssertEqual(row.status, "verified")
        XCTAssertEqual(row.projection.phase, .terminal)
        XCTAssertEqual(row.projection.outcome, .verified)
        XCTAssertEqual(row.projection.stopReason, .verified)
        XCTAssertEqual(row.projection.attemptCount, 2)
        XCTAssertEqual(row.projection.caveats.count, 1)
        XCTAssertEqual(row.lease?.ownerID, "supervisor-7f3a")
        XCTAssertEqual(row.lease?.epoch, 4)
        XCTAssertEqual(row.lease?.fresh, true)
        XCTAssertEqual(row.spend?.totalCostUSD, 1.8342)
        XCTAssertEqual(row.spend?.capUSD, 5.0)
        XCTAssertEqual(row.provider, "cli:codex-server")
        XCTAssertEqual(row.sandbox, "seatbelt")
        XCTAssertEqual(row.receipt?.present, true)
        XCTAssertEqual(row.receipt?.verified, .valid)
        XCTAssertNil(row.receipt?.error)
        XCTAssertEqual(row.gate?.nPassed, 5)
        XCTAssertEqual(row.gate?.nTotal, 5)
        // Fractional-second RFC 3339 must round-trip through the shared decoder.
        XCTAssertEqual(
            row.updatedAt.timeIntervalSince1970,
            DeadreckonJSON.date(from: "2026-08-06T02:19:42.913402Z")!.timeIntervalSince1970,
            accuracy: 0.001)
    }

    /// A queued Job with no lease checkpoint, no spend ledger, no receipt
    /// proof, and no signed gate marker: nulls stay nil, never fabricated.
    func testDecodesMinimalJobRow() throws {
        let json = """
            {
              "job_id": "job-20260806-030001-fresh",
              "scope": "deadreckon",
              "goal": "Fresh job",
              "status": "queued",
              "updated_at": "2026-08-06T03:00:01Z",
              "attempts": 0,
              "outcome": null,
              "stop_reason": null,
              "projection": {
                "phase": "queued",
                "outcome": null,
                "stop_reason": null,
                "attempt_count": 0,
                "caveats": []
              },
              "lease": null,
              "spend": null,
              "provider": null,
              "sandbox": null,
              "receipt": {
                "present": false,
                "verified": "not-applicable",
                "error": null
              },
              "gate": null
            }
            """
        let row = try decode(FleetRow.self, json)
        XCTAssertEqual(row.projection.phase, .queued)
        XCTAssertNil(row.outcome)
        XCTAssertNil(row.stopReason)
        XCTAssertNil(row.lease)
        XCTAssertNil(row.spend)
        XCTAssertNil(row.provider)
        XCTAssertNil(row.gate)
        XCTAssertEqual(row.receipt?.verified, .notApplicable)
    }

    /// A Verified outcome whose external proof no longer validates is its own
    /// state; the three-valued classifier must never collapse into a Bool.
    func testDecodesInvalidProofRow() throws {
        let json = """
            {
              "job_id": "job-x",
              "scope": "s",
              "goal": "g",
              "status": "verified_proof_invalid",
              "updated_at": "2026-08-06T02:19:42Z",
              "attempts": 1,
              "outcome": "verified",
              "stop_reason": "verified",
              "projection": {
                "phase": "terminal",
                "outcome": "verified",
                "stop_reason": "verified",
                "attempt_count": 1,
                "caveats": []
              },
              "lease": null,
              "spend": null,
              "provider": "cli:claude",
              "sandbox": "none",
              "receipt": {
                "present": true,
                "verified": "invalid",
                "error": "receipt sha256 mismatch for artifacts/report.md"
              },
              "gate": null
            }
            """
        let row = try decode(FleetRow.self, json)
        XCTAssertEqual(row.status, "verified_proof_invalid")
        XCTAssertEqual(row.receipt?.verified, .invalid)
        XCTAssertEqual(row.receipt?.error, "receipt sha256 mismatch for artifacts/report.md")
    }

    /// Unknown glossary words decode to .unknown, never a decode crash.
    func testUnknownEnumWordsDecodeToUnknown() throws {
        let json = """
            {
              "job_id": "job-y",
              "scope": "s",
              "goal": "g",
              "status": "hyperspace_jump",
              "updated_at": "2026-08-06T02:19:42Z",
              "attempts": 1,
              "outcome": "transcended",
              "stop_reason": "gravity_failure",
              "projection": {
                "phase": "warping",
                "outcome": "transcended",
                "stop_reason": "gravity_failure",
                "attempt_count": 1,
                "caveats": []
              },
              "lease": null,
              "spend": null,
              "provider": null,
              "sandbox": null,
              "receipt": {
                "present": false,
                "verified": "quantum",
                "error": null
              },
              "gate": null
            }
            """
        let row = try decode(FleetRow.self, json)
        XCTAssertEqual(row.projection.phase, .unknown)
        XCTAssertEqual(row.projection.outcome, .unknown)
        XCTAssertEqual(row.projection.stopReason, .unknown)
        XCTAssertEqual(row.receipt?.verified, .unknown)
    }

    func testDecodesRunRowAndListEnvelope() throws {
        let json = """
            {
              "kind": "list",
              "id": "all-scopes",
              "status": "ok",
              "next_actions": ["deadreckon status latest"],
              "try_lines": [],
              "paths": {"home": "/Users/op/.deadreckon"},
              "jobs": [\(fullJobRow)],
              "runs": [
                {
                  "run_id": "run-20260805-114210-legacy",
                  "scope": "deadreckon",
                  "goal": "Legacy run",
                  "status": "completed",
                  "updated_at": "2026-08-05T11:59:00.120Z",
                  "state_path": "/Users/op/.deadreckon/runstate/deadreckon/runs/run-20260805-114210-legacy/state.json",
                  "provider": "cli:claude",
                  "spend": {
                    "total_cost_usd": 0.42,
                    "cap_usd": null,
                    "subscription": true,
                    "wall_time_seconds": null
                  }
                }
              ],
              "plans": [],
              "chains": []
            }
            """
        let envelope = try decode(ListEnvelope.self, json)
        XCTAssertEqual(envelope.kind, "list")
        XCTAssertEqual(envelope.jobs.count, 1)
        XCTAssertEqual(envelope.runs.count, 1)
        let run = envelope.runs[0]
        XCTAssertEqual(run.runID, "run-20260805-114210-legacy")
        XCTAssertEqual(run.provider, "cli:claude")
        XCTAssertEqual(run.spend?.subscription, true)
        XCTAssertNil(run.spend?.capUSD)
    }

    // MARK: - JobLease

    func testDecodesJobLeaseCheckpoint() throws {
        let json = """
            {
              "schema_version": 1,
              "job_id": "job-20260806-021542-verify-ledger",
              "owner_id": "supervisor-7f3a",
              "epoch": 4,
              "pid": 48231,
              "process_group": 48231,
              "child_pid": 48377,
              "boot_id": "E4C2A9B1-77D2-4B0F-8461-52B9E2377A10",
              "process_start_identity": "48231:1754445600",
              "acquired_at": "2026-08-06T02:15:44Z",
              "heartbeat_at": "2026-08-06T02:19:39.001Z",
              "expires_at": "2026-08-06T02:20:09.001Z"
            }
            """
        let lease = try decode(JobLease.self, json)
        XCTAssertEqual(lease.schemaVersion, 1)
        XCTAssertEqual(lease.epoch, 4)
        XCTAssertEqual(lease.childPID, 48377)
        XCTAssertEqual(lease.processStartIdentity, "48231:1754445600")
    }

    /// Old checkpoints omit process_start_identity and child_pid.
    func testDecodesLegacyLeaseWithoutIdentity() throws {
        let json = """
            {
              "schema_version": 1,
              "job_id": "job-old",
              "owner_id": "supervisor-1",
              "epoch": 1,
              "pid": 100,
              "process_group": 100,
              "boot_id": "boot",
              "acquired_at": "2026-08-06T02:15:44Z",
              "heartbeat_at": "2026-08-06T02:19:39Z",
              "expires_at": "2026-08-06T02:20:09Z"
            }
            """
        let lease = try decode(JobLease.self, json)
        XCTAssertNil(lease.childPID)
        XCTAssertNil(lease.processStartIdentity)
    }

    // MARK: - JobEvent

    func testDecodesJobEventWithDetail() throws {
        let json = """
            {
              "schema_version": 1,
              "job_id": "job-20260806-021542-verify-ledger",
              "event_id": "evt-0007",
              "causation_id": "evt-0006",
              "sequence": 7,
              "kind": "deterministic_gate_passed",
              "lease_epoch": 4,
              "timestamp": "2026-08-06T02:19:40.771Z",
              "detail": {"attempt": 2, "n_passed": 5, "n_total": 5}
            }
            """
        let event = try decode(JobEvent.self, json)
        XCTAssertEqual(event.sequence, 7)
        XCTAssertEqual(event.kind, .deterministicGatePassed)
        XCTAssertEqual(event.leaseEpoch, 4)
        guard case .object(let detail) = event.detail else {
            return XCTFail("detail should be an object")
        }
        XCTAssertEqual(detail["n_passed"], .number(5))
    }

    func testUnknownJobEventKindDecodesToUnknown() throws {
        let json = """
            {
              "schema_version": 1,
              "job_id": "job-z",
              "event_id": "evt-1",
              "causation_id": "evt-0",
              "sequence": 1,
              "kind": "warp_core_engaged",
              "lease_epoch": 0,
              "timestamp": "2026-08-06T02:19:40Z",
              "detail": null
            }
            """
        let event = try decode(JobEvent.self, json)
        XCTAssertEqual(event.kind, .unknown)
        XCTAssertEqual(event.detail, .null)
    }

    // MARK: - SpendRecord

    func testDecodesSpendRecordWithDefaults() throws {
        // A legacy row: no kind, no subscription, no estimated.
        let json = """
            {
              "timestamp": "2026-08-06T02:18:11.402198Z",
              "turn": 3,
              "provider": "cli:codex-server",
              "model": "gpt-5.2-codex",
              "input_tokens": 48211,
              "output_tokens": 1922,
              "cost_usd": 0.211,
              "total_cost_usd": 1.104,
              "cap_usd": 5.0,
              "wall_time_seconds": 240.2,
              "wall_time_cap_seconds": null
            }
            """
        let record = try decode(SpendRecord.self, json)
        XCTAssertEqual(record.kind, "loop")
        XCTAssertFalse(record.subscription)
        XCTAssertFalse(record.estimated)
        XCTAssertEqual(record.totalCostUSD, 1.104)
        XCTAssertEqual(record.wallTimeSeconds, 240.2)
        XCTAssertNil(record.wallTimeCapSeconds)
    }

    func testDecodesNarratorSpendRow() throws {
        let json = """
            {
              "timestamp": "2026-08-06T02:18:12Z",
              "turn": 3,
              "provider": "api:anthropic",
              "model": "narrator",
              "input_tokens": 900,
              "output_tokens": 120,
              "cost_usd": 0.004,
              "total_cost_usd": 0.021,
              "kind": "narrator",
              "subscription": false,
              "estimated": true
            }
            """
        let record = try decode(SpendRecord.self, json)
        XCTAssertEqual(record.kind, "narrator")
        XCTAssertTrue(record.estimated)
        XCTAssertNil(record.capUSD)
    }

    // MARK: - SteerEligibility

    func testDecodesSteerable() throws {
        let json = #"{"steerable": true, "reason": null}"#
        let eligibility = try decode(SteerEligibility.self, json)
        XCTAssertTrue(eligibility.steerable)
        XCTAssertNil(eligibility.reason)
    }

    func testDecodesSteerRefusalReasons() throws {
        let fenced = try decode(
            SteerEligibility.self, #"{"steerable": false, "reason": "driver_fenced"}"#)
        XCTAssertEqual(fenced.reason, .driverFenced)
        let notExecuting = try decode(
            SteerEligibility.self, #"{"steerable": false, "reason": "not_executing"}"#)
        XCTAssertEqual(notExecuting.reason, .notExecuting)
        let provider = try decode(
            SteerEligibility.self, #"{"steerable": false, "reason": "provider_not_steerable"}"#)
        XCTAssertEqual(provider.reason, .providerNotSteerable)
        // Reason may be absent entirely (skip_serializing_if on the Rust side).
        let absent = try decode(SteerEligibility.self, #"{"steerable": true}"#)
        XCTAssertNil(absent.reason)
        // A widened future reason decodes to .unknown, still steer-disabled.
        let widened = try decode(
            SteerEligibility.self, #"{"steerable": false, "reason": "mid_turn_window_closed"}"#)
        XCTAssertEqual(widened.reason, .unknown)
    }

    // MARK: - NotifyRecord

    func testDecodesNotifyRecords() throws {
        // Historical delivery-attempt rows carry no `kind` field.
        guard case .deliveryAttempt(let delivered) = try decode(
            NotifyRecord.self,
            #"{"ts": "2026-08-06T02:19:42Z", "transition": "paused", "channel": "webhook", "ok": true}"#)
        else {
            return XCTFail("expected a delivery-attempt row")
        }
        XCTAssertEqual(delivered.transition, .paused)
        XCTAssertTrue(delivered.ok)
        XCTAssertNil(delivered.detail)

        guard case .deliveryAttempt(let failed) = try decode(
            NotifyRecord.self,
            #"{"ts": "2026-08-06T02:19:43Z", "transition": "failed", "channel": "webhook", "ok": false, "detail": "connection refused"}"#)
        else {
            return XCTFail("expected a delivery-attempt row")
        }
        XCTAssertEqual(failed.transition, .failed)
        XCTAssertFalse(failed.ok)
        XCTAssertEqual(failed.detail, "connection refused")
    }

    func testDecodesOperatorAttentionRows() throws {
        // M1 typed rows are discriminated by the presence of `kind`.
        guard case .attention(let attention) = try decode(
            NotifyRecord.self,
            #"{"schema_version": 1, "kind": "operator_attention", "reason": "verified_awaiting_promote", "job_id": "job-1", "run_id": "run-1", "scope": null, "at": "2026-08-06T02:20:00Z", "summary": "verified run awaits promote", "next_actions": ["deadreckon finish run-1"]}"#)
        else {
            return XCTFail("expected an operator-attention row")
        }
        XCTAssertEqual(attention.reason, .verifiedAwaitingPromote)
        XCTAssertEqual(attention.jobID, "job-1")
        XCTAssertEqual(attention.runID, "run-1")
        XCTAssertNil(attention.scope)
        XCTAssertEqual(attention.summary, "verified run awaits promote")
        XCTAssertEqual(attention.nextActions, ["deadreckon finish run-1"])

        // A widened future reason decodes to .unknown rather than failing.
        guard case .attention(let widened) = try decode(
            NotifyRecord.self,
            #"{"schema_version": 1, "kind": "operator_attention", "reason": "gate_flapping", "at": "2026-08-06T02:21:00Z", "summary": "gate is flapping", "next_actions": []}"#)
        else {
            return XCTFail("expected an operator-attention row")
        }
        XCTAssertEqual(widened.reason, .unknown)
    }

    // MARK: - AcceptanceProgressRow

    func testDecodesAcceptanceProgressRows() throws {
        let started = try decode(
            AcceptanceProgressRow.self,
            #"{"checked_at": "2026-08-06T02:19:10Z", "status": "started", "index": 1, "total": 5}"#)
        XCTAssertEqual(started.status, "started")
        XCTAssertNil(started.result)

        let passed = try decode(
            AcceptanceProgressRow.self,
            """
            {
              "checked_at": "2026-08-06T02:19:12.55Z",
              "status": "passed",
              "index": 1,
              "total": 5,
              "result": {
                "kind": "command",
                "passed": true,
                "must_pass": true,
                "detail": "cargo test --workspace",
                "command": "cargo test --workspace",
                "cwd": "/work/tree",
                "duration_ms": 2140,
                "stdout": "ok",
                "stderr": ""
              }
            }
            """)
        XCTAssertEqual(passed.result?.passed, true)
        XCTAssertEqual(passed.result?.mustPass, true)
        XCTAssertEqual(passed.result?.durationMS, 2140)
    }
}
