import XCTest

@testable import DeadreckonKit

/// Binnacle promote gating and the G4 preview: promote disabled unless the
/// shared proof classifier reads valid AND both recorded keys are present;
/// dry-run decode; the refusal path renders try_lines verbatim; and no code
/// path exists that retries finish with different authority.
@MainActor
final class PromoteFlowTests: XCTestCase {

    // MARK: - Fixture helpers

    private func receipt(_ verified: ProofStatus) -> FleetRow.Receipt {
        try! DeadreckonJSON.decoder().decode(FleetRow.Receipt.self, from: Data("""
        {"present": true, "verified": "\(verified.rawValue)", "error": null}
        """.utf8))
    }

    private func report(marker: Bool, judgment: Bool) -> JobReportEnvelope {
        let receiptBlock = marker
            ? """
              "receipt": {"status": "valid", "contained": true, "sandbox_backend": "sandbox-exec"},
              """
            : ""
        let semantic = judgment
            ? """
              "semantic": {"judgment": {"decision": "achieved",
                "summary": "ledger v2 written, migrations pass"}},
              """
            : ""
        return try! DeadreckonJSON.decoder().decode(JobReportEnvelope.self, from: Data("""
        {"id": "job-1", "goal": "g", "phase": "terminal", "outcome": "verified",
         "stop_reason": "verified",
         \(receiptBlock)
         \(semantic)
         "deterministic_checks": [], "attempts": []}
        """.utf8))
    }

    // MARK: - Gating

    func testPromoteDisabledUnlessProofValidAndBothKeysPresent() {
        // Everything present: enabled.
        let enabled = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: true))
        XCTAssertTrue(enabled.promoteEnabled)
        XCTAssertNil(enabled.disabledReason)
        XCTAssertEqual(enabled.judgmentDecision, "achieved")

        // Proof not valid: disabled, named.
        let invalid = PromoteGate.evaluate(
            receipt: receipt(.invalid), report: report(marker: true, judgment: true))
        XCTAssertFalse(invalid.promoteEnabled)
        XCTAssertTrue(invalid.disabledReason?.contains("proof classifier") == true)

        // No receipt at all: disabled.
        XCTAssertFalse(PromoteGate.evaluate(
            receipt: nil, report: report(marker: true, judgment: true)).promoteEnabled)

        // Missing marker key: disabled, named.
        let noMarker = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: false, judgment: true))
        XCTAssertFalse(noMarker.promoteEnabled)
        XCTAssertTrue(noMarker.disabledReason?.contains("key 1") == true)

        // Missing judgment key: disabled, named.
        let noJudgment = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: false))
        XCTAssertFalse(noJudgment.promoteEnabled)
        XCTAssertTrue(noJudgment.disabledReason?.contains("key 2") == true)

        // No report yet: disabled.
        XCTAssertFalse(PromoteGate.evaluate(receipt: receipt(.valid), report: nil).promoteEnabled)
    }

    // MARK: - Dry-run preview

    func testDryRunDecodesTheFinishPlan() async {
        let cli = FakeCLI()
        cli.scriptStdout("finish", """
        {"kind": "finish_plan", "id": "job-1", "status": "deliverable",
         "receipt": {"validated": true, "error": null},
         "staged": [{"path": "a.rs", "bytes": 10, "sha256": "aa"}],
         "diffstat": {"files_changed": 1, "added": 5, "removed": 2},
         "destination": {"kind": "export", "path": "/tmp/out"},
         "result_tree_sha256": "cafe1234",
         "irreversible_steps": ["publish", "cleanup"]}
        """)
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        coordinator.destination = .export(path: "/tmp/out")
        await coordinator.loadPreview()
        guard case .plan(let plan) = coordinator.preview else {
            return XCTFail("expected the plan: \(coordinator.preview)")
        }
        XCTAssertEqual(plan.staged.count, 1)
        XCTAssertEqual(plan.irreversibleSteps, ["publish", "cleanup"])
        XCTAssertEqual(plan.status, "deliverable")
        XCTAssertFalse(plan.isBlocked)
        XCTAssertEqual(plan.receipt?.validated, true)
        XCTAssertEqual(plan.resultTreeSHA256, "cafe1234")
        XCTAssertEqual(cli.calls.first,
                       ["finish", "job-1", "--dry-run", "--dest", "/tmp/out", "--json"])
        XCTAssertEqual(coordinator.previewDestination, .export(path: "/tmp/out"),
                       "the sheet can name the destination this plan was computed for")
    }

    /// The G4 "As built" blocked shape: receipt tamper / digest mismatch /
    /// staging refusal still exits 0 with the plan on stdout. The decoder
    /// must carry status and receipt.error so the sheet renders the exact
    /// fail-closed message verbatim — never "0 files · +0 −0".
    func testBlockedFinishPlanDecodesStatusAndReceiptError() async {
        let cli = FakeCLI()
        cli.scriptStdout("finish", """
        {"kind": "finish_plan", "id": "job-1", "status": "blocked",
         "receipt": {"validated": false,
                     "error": "receipt digest mismatch: source-tree drifted"},
         "staged": [], "diffstat": {"files_changed": 0, "added": 0, "removed": 0},
         "next_actions": ["deadreckon report job-1"]}
        """)
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        await coordinator.loadPreview()
        guard case .plan(let plan) = coordinator.preview else {
            return XCTFail("blocked plans still decode as plans: \(coordinator.preview)")
        }
        XCTAssertTrue(plan.isBlocked)
        XCTAssertEqual(plan.receipt?.validated, false)
        XCTAssertEqual(plan.receipt?.error, "receipt digest mismatch: source-tree drifted")
        XCTAssertEqual(plan.nextActions, ["deadreckon report job-1"],
                       "the envelope's own next actions are the only recovery affordances")
    }

    func testM1BinaryDegradesHonestlyWhenDryRunIsUnknown() async {
        let cli = FakeCLI()
        cli.script("finish", .result(CLIRunResult(
            stdout: "",
            stderr: "error: unexpected argument '--dry-run' found",
            exitCode: 2)))
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        await coordinator.loadPreview()
        guard case .unsupported(let words) = coordinator.preview else {
            return XCTFail("expected the honest degrade: \(coordinator.preview)")
        }
        XCTAssertTrue(words.contains("M2 binary"), words)
        XCTAssertTrue(words.contains("--dry-run"), "the binary's own words are kept: \(words)")
    }

    /// The M2-gap diagnosis is reserved for the G1 carve-out signature
    /// (exit 2, clap prose). Every OTHER envelope-free result — a watchdog
    /// SIGTERM, a crashed binary, a locator failure — is a plain failure
    /// rendered with its own words and exit code, never a fabricated
    /// version-gap story.
    func testNonParseEnvelopeFreeFailureIsNeverDiagnosedAsTheM2Gap() async {
        let cli = FakeCLI()
        cli.script("finish", .result(CLIRunResult(stdout: "", stderr: "", exitCode: -1)))
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        await coordinator.loadPreview()
        guard case .failed(let words) = coordinator.preview else {
            return XCTFail("a crash is a failure, not a version gap: \(coordinator.preview)")
        }
        XCTAssertTrue(words.contains("exit -1"), words)
        XCTAssertFalse(words.contains("M2"), "no fabricated diagnosis: \(words)")
    }

    /// The in-flight guard: a Return-key auto-repeat on PROMOTE enqueues a
    /// second promote before the first sets .running renders; the second
    /// must never reach the binary (two concurrent finish --yes mutations).
    func testInFlightPromoteGuardDispatchesOnce() async {
        let cli = GatedCLI()
        cli.gatedResult = CLIRunResult(stdout: """
        {"kind": "finish", "id": "job-1", "status": "completed",
         "next_actions": [], "try_lines": [],
         "destination": {"kind": "in-place", "path": "/w"}, "receipt_validated": true}
        """, stderr: "", exitCode: 0)
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        let gate = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: true))
        let first = Task { await coordinator.promote(gate: gate) }
        while cli.parkedCount == 0 { await Task.yield() }
        await coordinator.promote(gate: gate)
        cli.release()
        await first.value
        XCTAssertEqual(cli.calls.count, 1,
                       "a second promote while one finish is in flight must not dispatch")
    }

    func testRefusalRendersTryLinesVerbatimWithNoOverride() async {
        let cli = FakeCLI()
        cli.script("finish", .result(CLIRunResult(stdout: """
        {"kind": "error", "code": 1, "verb": "finish",
         "message": "receipt digest mismatch: source-tree drifted",
         "try_lines": ["deadreckon verdict job-1", "deadreckon report job-1"]}
        """, stderr: "", exitCode: 1)))
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        let gate = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: true))
        await coordinator.promote(gate: gate)
        guard case .refused(let refusal) = coordinator.promotion else {
            return XCTFail("expected the verbatim refusal: \(coordinator.promotion)")
        }
        XCTAssertEqual(refusal.message, "receipt digest mismatch: source-tree drifted")
        XCTAssertEqual(refusal.tryLines,
                       ["deadreckon verdict job-1", "deadreckon report job-1"])
    }

    /// No code path retries finish with different authority: a refused
    /// promote leaves the argv identical on any subsequent dispatch (same
    /// flags, no force/overwrite — PlannedVerb cannot even express one).
    func testNoRetryWithDifferentAuthorityExists() async {
        let cli = FakeCLI()
        cli.script("finish", .result(CLIRunResult(stdout: """
        {"kind": "error", "code": 1, "verb": "finish", "message": "refused", "try_lines": []}
        """, stderr: "", exitCode: 1)))
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        let gate = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: true))
        await coordinator.promote(gate: gate)
        await coordinator.promote(gate: gate)
        let finishCalls = cli.calls.filter { $0.first == "finish" }
        XCTAssertEqual(finishCalls.count, 2)
        XCTAssertEqual(finishCalls[0], finishCalls[1],
                       "a refusal never changes the authority of the next attempt")
        for call in finishCalls {
            XCTAssertFalse(call.contains("--force"))
            XCTAssertFalse(call.contains("--overwrite"))
        }
    }

    func testDisabledGateNeverDispatches() async {
        let cli = FakeCLI()
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        let gate = PromoteGate.evaluate(receipt: receipt(.invalid), report: nil)
        await coordinator.promote(gate: gate)
        XCTAssertTrue(cli.calls.isEmpty, "a disabled gate must not reach the binary")
        guard case .failed(let words) = coordinator.promotion else {
            return XCTFail("the disabled reason is named: \(coordinator.promotion)")
        }
        XCTAssertTrue(words.contains("proof classifier"))
    }

    func testPromoteSuccessCarriesTheEnvelope() async {
        let cli = FakeCLI()
        cli.scriptStdout("finish", """
        {"kind": "finish", "id": "job-1", "status": "completed",
         "next_actions": ["deadreckon undo"], "try_lines": [],
         "destination": {"kind": "git-branch", "target": "main"},
         "strategy": "squash", "cleaned": false, "receipt_validated": true,
         "already_applied": false, "staged_file_count": 7}
        """)
        let coordinator = PromoteCoordinator(jobID: "job-1", cli: cli)
        let gate = PromoteGate.evaluate(
            receipt: receipt(.valid), report: report(marker: true, judgment: true))
        await coordinator.promote(gate: gate)
        guard case .succeeded(let envelope) = coordinator.promotion else {
            return XCTFail("expected success: \(coordinator.promotion)")
        }
        XCTAssertEqual(envelope.delivery?.strategy, "squash")
        XCTAssertEqual(envelope.nextActions.first, "deadreckon undo",
                       "the undo hint rides the envelope's own next_actions")
    }
}
