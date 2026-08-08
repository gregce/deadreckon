import XCTest

@testable import DeadreckonKit

/// §R2 row 5: the keyless smoke proof. The fixture is the LIVE 0.8.4
/// `try --json` document (scratch home, 2026-08-07) — a bare proof doc,
/// not a kind-scaffold envelope. The trust-words contract: `trust` and
/// `gate` render verbatim, and `trusted_job_receipt` stays the binary's
/// own boolean.
@MainActor
final class TryControllerTests: XCTestCase {
    /// Live 0.8.4 shape, paths shortened.
    private static let proof = """
    {
      "gate": "local smoke gate evidence only; not a trusted Job receipt",
      "lineage": "README.md \u{2190} turn 2 \u{00B7} smoke \u{00B7} smoke-bash-2",
      "next": "deadreckon start \\"build the real thing\\"",
      "proof": "/Users/op/.deadreckon/runstate/s/runs/812cc98b/proofs/turn-acceptance.json",
      "run_id": "812cc98b09924f078098bedef4f4417f",
      "story": "/Users/op/.deadreckon/library/s/812cc98b/docs/RUN-NARRATIVE.md",
      "trust": "untrusted local smoke diagnostic",
      "trusted_job_receipt": false
    }
    """

    func testDispatchesTryJSON() async {
        let cli = SettingsFakeCLI()
        cli.script("try", stdout: Self.proof)
        let controller = TryController(cli: cli)
        await controller.run()
        XCTAssertEqual(cli.calls, [["try", "--json"]])
    }

    func testProofDecodesWithTrustWordsVerbatim() async throws {
        let cli = SettingsFakeCLI()
        cli.script("try", stdout: Self.proof)
        let controller = TryController(cli: cli)
        await controller.run()
        guard case .proof(let envelope) = controller.state else {
            return XCTFail("expected proof, got \(controller.state)")
        }
        XCTAssertEqual(envelope.trust, "untrusted local smoke diagnostic")
        XCTAssertEqual(envelope.gate,
                       "local smoke gate evidence only; not a trusted Job receipt")
        XCTAssertFalse(envelope.trustedJobReceipt)
        XCTAssertEqual(envelope.runID, "812cc98b09924f078098bedef4f4417f")
        XCTAssertTrue(envelope.proofPath?.contains("turn-acceptance.json") == true)
    }

    func testFailureKeepsTheWordsVerbatim() async {
        let cli = SettingsFakeCLI()
        cli.script("try", stdout: "",
                   stderr: "error: invalid input: workspace scratch dir unavailable",
                   exitCode: 1)
        let controller = TryController(cli: cli)
        await controller.run()
        guard case .failed(let words) = controller.state else {
            return XCTFail("expected failed, got \(controller.state)")
        }
        XCTAssertTrue(words.contains("workspace scratch dir unavailable"))
    }

    func testTypedRefusalIsAuthoritative() async {
        let cli = SettingsFakeCLI()
        cli.script("try", stdout: """
        {"kind": "error", "code": 1, "verb": "try",
         "message": "invalid input: home is owned by a newer binary",
         "try_lines": ["deadreckon doctor"]}
        """, exitCode: 1)
        let controller = TryController(cli: cli)
        await controller.run()
        guard case .refused(let refusal) = controller.state else {
            return XCTFail("expected refused, got \(controller.state)")
        }
        XCTAssertEqual(refusal.verb, "try")
        XCTAssertEqual(refusal.tryLines, ["deadreckon doctor"])
    }

    /// A failed proof can be retried (a new dispatch); only an in-flight
    /// run guards.
    func testRetryAfterFailureDispatchesAgain() async {
        let cli = SettingsFakeCLI()
        cli.script("try", stdout: "", stderr: "boom", exitCode: 1)
        cli.script("try", stdout: Self.proof)
        let controller = TryController(cli: cli)
        await controller.run()
        await controller.run()
        XCTAssertEqual(cli.calls.count, 2)
        guard case .proof = controller.state else {
            return XCTFail("expected proof after retry, got \(controller.state)")
        }
    }
}
