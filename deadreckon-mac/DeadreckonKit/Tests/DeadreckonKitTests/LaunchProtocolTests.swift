import XCTest

@testable import DeadreckonKit

/// The G2 launch protocol round trip with fakes: preview -> plan file
/// content -> execute argv, with the >$50 acknowledgment armed ONLY by the
/// typed amount matching the resolved plan ceiling.
@MainActor
final class LaunchProtocolTests: XCTestCase {

    private let previewFixture = """
    {
      "kind": "start",
      "goal": "add retry queue",
      "selected_mode": "single",
      "provider": "cli:codex-server",
      "done_contract": {"capabilities": {"network": "deny"},
                        "checks": [{"kind": "cargo_test", "must_pass": true}]},
      "requires_confirmation": true,
      "will_start": false,
      "next_actions": [],
      "try_lines": [],
      "launch_plan": {
        "plan_id": "lp-1",
        "budget": {"ceiling_usd": 60, "wall_seconds": 28800},
        "pieces": [{"id": "run", "budget_usd": 60}]
      }
    }
    """

    private func launchFixture() -> String {
        """
        {"kind": "launch", "id": "job-new", "status": "queued",
         "next_actions": ["deadreckon attach job-new"], "try_lines": []}
        """
    }

    func testPreviewThenExecuteRoundTrip() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", previewFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "add retry queue", provider: "cli:codex-server")

        await controller.runPreview()
        guard case .ready(let envelope) = controller.preview else {
            return XCTFail("preview should be launchable: \(controller.preview)")
        }
        XCTAssertFalse(envelope.willStart, "the preview leg never starts anything")
        // The preview argv had no --yes.
        XCTAssertEqual(cli.calls.first?.contains("--yes"), false)

        // The resolved plan ceiling ($60) drives the acknowledgment.
        XCTAssertTrue(controller.acknowledgement.required)
        controller.acknowledgement.typedAmount = "60"
        XCTAssertTrue(controller.acknowledgement.armsFlag)

        cli.scriptStdout("start", launchFixture())
        await controller.execute()

        guard case .launched(let launched) = controller.execution else {
            return XCTFail("execute should have launched: \(controller.execution)")
        }
        XCTAssertEqual(launched.kind, "launch")

        // The execute argv replays the plan file with --yes and the armed
        // acknowledgment flag.
        let executeCall = try XCTUnwrap(cli.calls.last)
        let planPath = try XCTUnwrap(controller.lastPlanFileURL?.path)
        XCTAssertEqual(executeCall,
                       ["start", "--plan", planPath, "--yes", "--i-know-its-a-lot", "--json"])

        // The plan file's content is the preview's embedded payload,
        // byte-honest: integers survive, and the parsed structure equals the
        // fixture's launch_plan subobject.
        let written = try Data(contentsOf: URL(fileURLWithPath: planPath))
        XCTAssertTrue(String(decoding: written, as: UTF8.self).contains("\"ceiling_usd\":60"))
        let reparsed = try XCTUnwrap(try JSONSerialization.jsonObject(with: written) as? NSDictionary)
        let original = try XCTUnwrap(
            (try JSONSerialization.jsonObject(with: Data(previewFixture.utf8)) as? [String: Any])?["launch_plan"]
                as? NSDictionary)
        XCTAssertEqual(reparsed, original)
        try? FileManager.default.removeItem(atPath: planPath)
    }

    func testAckFlagIsArmedOnlyByTheMatchingTypedAmount() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", previewFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "add retry queue")
        await controller.runPreview()

        // Wrong amount: execute refuses to launch anything at all.
        controller.acknowledgement.typedAmount = "6"
        XCTAssertFalse(controller.acknowledgement.armsFlag)
        let callsBefore = cli.calls.count
        await controller.execute()
        XCTAssertEqual(cli.calls.count, callsBefore,
                       "an unmatched acknowledgment must not dispatch the execute leg")
        if case .failed = controller.execution {} else {
            XCTFail("execution should explain the required acknowledgment")
        }

        // Matching amount (with $ and decimals): armed.
        controller.acknowledgement.typedAmount = "$60.00"
        XCTAssertTrue(controller.acknowledgement.armsFlag)
        cli.scriptStdout("start", launchFixture())
        await controller.execute()
        let executeCall = try XCTUnwrap(cli.calls.last)
        XCTAssertTrue(executeCall.contains("--i-know-its-a-lot"))
        if let path = controller.lastPlanFileURL?.path {
            try? FileManager.default.removeItem(atPath: path)
        }
    }

    func testCapAtOrBelowFiftyNeverPassesTheFlag() async throws {
        let smallPlanPreview = previewFixture.replacingOccurrences(
            of: "\"ceiling_usd\": 60", with: "\"ceiling_usd\": 25")
            .replacingOccurrences(of: "\"budget_usd\": 60", with: "\"budget_usd\": 25")
        let cli = FakeCLI()
        cli.scriptStdout("start", smallPlanPreview)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g")
        await controller.runPreview()
        XCTAssertFalse(controller.acknowledgement.required)

        cli.scriptStdout("start", launchFixture())
        await controller.execute()
        let executeCall = try XCTUnwrap(cli.calls.last)
        XCTAssertFalse(executeCall.contains("--i-know-its-a-lot"),
                       "no acknowledgment flag below the ceiling, even unrequested")
        if let path = controller.lastPlanFileURL?.path {
            try? FileManager.default.removeItem(atPath: path)
        }
    }

    /// A binary that crashed mid-execute with no envelope may have queued
    /// the durable Job before dying: the failure says so, points at the
    /// file-backed fleet, and DROPS the armed preview so the same one-click
    /// Start is not left loaded — a fresh preview is required first.
    func testEnvelopeFreeExecuteFailureDropsTheArmedPreview() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", previewFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "add retry queue")
        await controller.runPreview()
        controller.acknowledgement.typedAmount = "60"

        cli.script("start", .result(CLIRunResult(stdout: "", stderr: "", exitCode: -9)))
        await controller.execute()
        let planPath = controller.lastPlanFileURL?.path
        defer { if let planPath { try? FileManager.default.removeItem(atPath: planPath) } }
        guard case .failed(let words) = controller.execution else {
            return XCTFail("expected the honest failure: \(controller.execution)")
        }
        XCTAssertTrue(words.contains("exit -9"), words)
        XCTAssertTrue(words.contains("may or may not have been queued"), words)
        guard case .idle = controller.preview else {
            return XCTFail("the armed preview must drop after an ambiguous failure: \(controller.preview)")
        }

        // With the preview dropped, another execute dispatches nothing.
        let callsBefore = cli.calls.count
        await controller.execute()
        XCTAssertEqual(cli.calls.count, callsBefore,
                       "no re-dispatch without a fresh preview")
    }

    /// Success disarms too: Start is Return-bound in the sheet, so a
    /// preview left .ready after a successful launch would replay the same
    /// plan file into a DUPLICATE paid Job with one keypress. After
    /// .launched the preview drops and a second execute dispatches nothing
    /// until a fresh preview lands.
    func testSecondExecuteAfterSuccessDispatchesNothing() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", previewFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "add retry queue")
        await controller.runPreview()
        controller.acknowledgement.typedAmount = "60"

        cli.scriptStdout("start", launchFixture())
        await controller.execute()
        defer {
            if let path = controller.lastPlanFileURL?.path {
                try? FileManager.default.removeItem(atPath: path)
            }
        }
        guard case .launched = controller.execution else {
            return XCTFail("the first execute should launch: \(controller.execution)")
        }
        guard case .idle = controller.preview else {
            return XCTFail("success must drop the armed preview: \(controller.preview)")
        }

        let callsBefore = cli.calls.count
        await controller.execute()
        XCTAssertEqual(cli.calls.count, callsBefore,
                       "one launch per previewed plan; a second Start (Return) must not replay it")
        guard case .launched = controller.execution else {
            return XCTFail("the queued acknowledgment stays visible: \(controller.execution)")
        }
    }

    /// The in-flight guard: a double-click on Start enqueues two execute
    /// tasks; the second must not launch a second real Job.
    func testInFlightExecuteGuardDispatchesOnce() async throws {
        let cli = GatedCLI()
        cli.serveImmediately(CLIRunResult(stdout: previewFixture, stderr: "", exitCode: 0))
        cli.gatedResult = CLIRunResult(stdout: launchFixture(), stderr: "", exitCode: 0)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "add retry queue")
        await controller.runPreview()
        controller.acknowledgement.typedAmount = "60"

        let first = Task { await controller.execute() }
        while cli.parkedCount == 0 { await Task.yield() }
        await controller.execute()
        cli.release()
        await first.value
        if let planPath = controller.lastPlanFileURL?.path {
            try? FileManager.default.removeItem(atPath: planPath)
        }
        XCTAssertEqual(cli.calls.count, 2,
                       "one preview + ONE execute; the double-click's second task dispatches nothing")
        guard case .launched = controller.execution else {
            return XCTFail("the first execute still lands: \(controller.execution)")
        }
    }

    func testPreviewRefusalRendersTheEnvelopeVerbatim() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", """
        {"kind": "error", "code": 1, "verb": "start",
         "message": "durable start needs a done contract",
         "try_lines": ["deadreckon def-done \\"what should count as done\\""]}
        """, exitCode: 1)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g")
        await controller.runPreview()
        guard case .refused(let refusal) = controller.preview else {
            return XCTFail("expected the typed refusal: \(controller.preview)")
        }
        XCTAssertEqual(refusal.message, "durable start needs a done contract")
        XCTAssertEqual(refusal.tryLines.count, 1)
    }

    func testBlockedPreviewWithoutPlanCannotExecute() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", """
        {"kind": "start", "goal": "g", "will_start": false,
         "requires_confirmation": false, "next_actions": [],
         "try_lines": ["deadreckon def-done check"]}
        """)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g")
        await controller.runPreview()
        guard case .blocked = controller.preview else {
            return XCTFail("expected blocked: \(controller.preview)")
        }
        let callsBefore = cli.calls.count
        await controller.execute()
        XCTAssertEqual(cli.calls.count, callsBefore, "a blocked preview has nothing to replay")
    }

    func testCatalogSurfacesFailedProbesWithTryLines() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("providers", """
        {"kind": "providers", "id": "configured", "status": "ok",
         "next_actions": [], "try_lines": [],
         "providers": [
           {"id": "cli:codex-server", "display_name": "Codex", "status": "ok"},
           {"id": "anthropic", "display_name": "Anthropic API", "status": "failed",
            "message": "ANTHROPIC_API_KEY is not set",
            "try_lines": ["export ANTHROPIC_API_KEY=..."]}
         ],
         "missing_providers": [], "active": "cli:codex-server"}
        """)
        cli.scriptStdout("models", """
        {"providers": [
          {"provider": "cli:codex-server", "display_name": "Codex",
           "models": [{"id": "gpt-5.3-codex", "recommended": true, "default": true}]}
        ]}
        """)
        let catalog = LayCourseCatalog(cli: cli)
        await catalog.load()
        guard case .loaded(let providers) = catalog.providers else {
            return XCTFail("providers should load: \(catalog.providers)")
        }
        let failed = try XCTUnwrap(providers.providers.first(where: { $0.status == .failed }))
        XCTAssertEqual(failed.message, "ANTHROPIC_API_KEY is not set")
        XCTAssertEqual(failed.tryLines, ["export ANTHROPIC_API_KEY=..."],
                       "failed probes stay visible with their fix hints verbatim")
        XCTAssertEqual(catalog.modelChoices(for: "cli:codex-server").first?.id, "gpt-5.3-codex")
    }
}
