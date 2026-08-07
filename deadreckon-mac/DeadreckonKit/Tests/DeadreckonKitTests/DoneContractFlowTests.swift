import XCTest

@testable import DeadreckonKit

/// The Lay Course done-contract step against the non-interactive def-done
/// slice: envelope decodes pinned to the REAL Rust shapes (acceptance.rs
/// def_done_contract_envelope, gate.rs AcceptanceCheck serde tags,
/// tests/lifecycle.rs pins), the argv discipline for the plain-English
/// positional, and the coordinator flow refusal -> declare -> auto
/// re-preview with fakes. No real binary anywhere: the Prove stage covers
/// reality.
@MainActor
final class DoneContractFlowTests: XCTestCase {

    // MARK: - Fixtures (the binary's real shapes)

    /// Declare/add/edit success (`status:"written"`), as pinned by
    /// lifecycle.rs done_declare_json_yes_is_non_interactive_and_emits_
    /// def_done_result: the checks array is the serialized AcceptanceSpec
    /// shape report --json emits (tagged kind + must_pass + kind fields).
    private let writtenFixture = """
    {
      "kind": "def_done_result",
      "status": "written",
      "contract_path": "/Users/op/code/app/.deadreckon/acceptance.yaml",
      "notes_path": "/Users/op/code/app/.deadreckon/acceptance.md",
      "name": "done",
      "check_count": 1,
      "checks": [
        {
          "kind": "shell",
          "command": "python3 -c \\"import pathlib; assert pathlib.Path('README.md').exists()\\"",
          "cwd": "{working_dir}",
          "must_pass": true
        }
      ],
      "capabilities": {"network": "deny"},
      "drafted_by": "mock / mock-model",
      "next_actions": ["deadreckon start \\"goal\\"", "deadreckon def-done check --json"]
    }
    """

    /// `def-done show --json` on a declared project (`status:"declared"`),
    /// per the lifecycle.rs show pin: two check kinds, a granted loopback
    /// capability, notes_path, no drafted_by (a read drafts nothing).
    private let declaredShowFixture = """
    {
      "kind": "def_done_result",
      "status": "declared",
      "contract_path": "/Users/op/code/app/.deadreckon/acceptance.yaml",
      "notes_path": "/Users/op/code/app/.deadreckon/acceptance.md",
      "name": "show contract",
      "check_count": 2,
      "checks": [
        {"kind": "shell", "command": "curl -sf http://127.0.0.1:4173",
         "cwd": "{working_dir}", "must_pass": true},
        {"kind": "file_exists", "path": "{working_dir}/README.md", "must_pass": true}
      ],
      "capabilities": {"network": "loopback"},
      "drafted_by": null,
      "next_actions": ["deadreckon start \\"goal\\"", "deadreckon def-done check --json"]
    }
    """

    /// `def-done show --json` with nothing declared: a NORMAL exit-0 read
    /// (`status:"default_gate"`, contract_path null, empty checks) — the
    /// sheet renders the declare affordance, never a refusal.
    private let defaultGateFixture = """
    {
      "kind": "def_done_result",
      "status": "default_gate",
      "contract_path": null,
      "notes_path": null,
      "name": null,
      "check_count": 0,
      "checks": [],
      "capabilities": {"network": "deny"},
      "drafted_by": null,
      "next_actions": ["deadreckon start \\"goal\\"", "deadreckon def-done check --json"]
    }
    """

    /// The --yes refusal (refuse_unapproved_machine_write), the shared G1
    /// error envelope with verb "def-done" and the historical exit code.
    private let yesRefusalFixture = """
    {"kind": "error", "code": 1, "verb": "def-done",
     "message": "def-done --json writes a done contract without prompts and needs --yes to approve it",
     "try_lines": ["deadreckon def-done \\"what should count as done\\" --yes --json"]}
    """

    /// A preview blocked for the missing contract: start.rs sets
    /// done_criteria_source to "missing" (StartDoneCriteriaSource::Missing)
    /// and start_done_contract_json emits null capabilities/checks.
    private let missingContractPreviewFixture = """
    {
      "kind": "start",
      "goal": "keep the README correct",
      "selected_mode": "single",
      "selection_source": "auto",
      "reason": "done contract is missing for this repo",
      "provider": "cli:codex-server",
      "provider_source": "configured",
      "done_criteria": "missing done contract",
      "done_criteria_source": "missing",
      "done_contract": {"capabilities": null, "checks": null},
      "source_mode": "copy",
      "requires_confirmation": false,
      "will_start": false,
      "next_actions": [],
      "try_lines": ["deadreckon def-done \\"what should count as done\\" && deadreckon start \\"keep the README correct\\""]
    }
    """

    /// The re-preview after declaring: the project contract resolves
    /// (done_criteria_source "project") and the plan embeds, launchable.
    /// `done_contract.checks` is the COMPILED row shape start.rs emits
    /// (acceptance.rs CompiledCheck: index/summary/behavioral/can_fail/
    /// network_required with the raw AcceptanceCheck under "raw" — NOT the
    /// def_done_result raw shape, and no top-level must_pass).
    private let launchablePreviewFixture = """
    {
      "kind": "start",
      "goal": "keep the README correct",
      "selected_mode": "single",
      "provider": "cli:codex-server",
      "done_criteria": "1 check(s) from /Users/op/code/app/.deadreckon/acceptance.yaml",
      "done_criteria_source": "project",
      "done_contract": {"capabilities": {"network": "deny"},
                        "checks": [{"index": 1, "kind": "shell",
                                    "summary": "shell: python3 readme check",
                                    "behavioral": true, "can_fail": false,
                                    "network_required": "deny",
                                    "raw": {"kind": "shell",
                                            "command": "python3 readme check",
                                            "cwd": "{working_dir}",
                                            "must_pass": true}}]},
      "requires_confirmation": false,
      "will_start": false,
      "next_actions": [],
      "try_lines": [],
      "launch_plan": {"plan_id": "lp-2", "budget": {"ceiling_usd": 25, "wall_seconds": 28800}}
    }
    """

    // MARK: - Envelope decode (real shapes)

    func testWrittenEnvelopeDecodesTheDeclareShape() throws {
        let envelope = try XCTUnwrap(DefDoneResultEnvelope(data: Data(writtenFixture.utf8)))
        XCTAssertEqual(envelope.kind, "def_done_result")
        XCTAssertEqual(envelope.status, "written")
        XCTAssertEqual(envelope.contractPath, "/Users/op/code/app/.deadreckon/acceptance.yaml")
        XCTAssertEqual(envelope.notesPath, "/Users/op/code/app/.deadreckon/acceptance.md")
        XCTAssertEqual(envelope.name, "done")
        XCTAssertEqual(envelope.checkCount, 1)
        XCTAssertEqual(envelope.checks.count, 1)
        let check = try XCTUnwrap(envelope.checks.first)
        XCTAssertEqual(check.kind, "shell")
        XCTAssertTrue(check.mustPass)
        XCTAssertEqual(check.cwd, "{working_dir}")
        XCTAssertEqual(check.target, check.command, "a shell row's target is its command, verbatim")
        XCTAssertEqual(envelope.network, "deny")
        XCTAssertEqual(envelope.draftedBy, "mock / mock-model",
                       "who drafted renders from the envelope, never guessed")
        XCTAssertEqual(envelope.nextActions.first, "deadreckon start \"goal\"")
    }

    func testDeclaredShowEnvelopeDecodesKindSpecificFields() throws {
        let envelope = try XCTUnwrap(DefDoneResultEnvelope(data: Data(declaredShowFixture.utf8)))
        XCTAssertEqual(envelope.status, "declared")
        XCTAssertEqual(envelope.name, "show contract")
        XCTAssertEqual(envelope.checkCount, 2)
        XCTAssertEqual(envelope.checks[0].kind, "shell")
        XCTAssertEqual(envelope.checks[1].kind, "file_exists")
        XCTAssertEqual(envelope.checks[1].path, "{working_dir}/README.md")
        XCTAssertEqual(envelope.checks[1].target, "{working_dir}/README.md")
        XCTAssertNil(envelope.checks[1].command)
        XCTAssertEqual(envelope.network, "loopback")
        XCTAssertNil(envelope.draftedBy, "a read drafts nothing")
    }

    func testDefaultGateEnvelopeIsANormalRead() throws {
        let envelope = try XCTUnwrap(DefDoneResultEnvelope(data: Data(defaultGateFixture.utf8)))
        XCTAssertEqual(envelope.status, "default_gate")
        XCTAssertNil(envelope.contractPath)
        XCTAssertNil(envelope.notesPath)
        XCTAssertNil(envelope.name)
        XCTAssertEqual(envelope.checkCount, 0)
        XCTAssertTrue(envelope.checks.isEmpty)
        XCTAssertEqual(envelope.network, "deny", "absence is deny-by-default in the compiled contract")
    }

    func testWrongKindNeverDecodes() {
        XCTAssertNil(DefDoneResultEnvelope(data: Data(yesRefusalFixture.utf8)),
                     "an error envelope is not a def_done_result")
        XCTAssertNil(DefDoneResultEnvelope(data: Data(launchablePreviewFixture.utf8)),
                     "a start preview is not a def_done_result")
    }

    func testYesRefusalClassifiesAsTheTypedErrorEnvelope() throws {
        let result = MutationResult.classify(stdout: yesRefusalFixture, stderr: "", exitCode: 1)
        let refusal = try XCTUnwrap(result.refusal)
        XCTAssertEqual(refusal.verb, "def-done")
        XCTAssertTrue(refusal.message.contains("--yes"))
        XCTAssertEqual(refusal.tryLines.count, 1)
    }

    func testMissingContractSignalOnTheBlockedPreview() throws {
        let blocked = try XCTUnwrap(
            StartPreviewEnvelope(data: Data(missingContractPreviewFixture.utf8)))
        XCTAssertTrue(blocked.missingDoneContract)
        XCTAssertFalse(blocked.isLaunchable)
        XCTAssertNil(blocked.doneContract, "null capabilities/checks decode as no contract")
        let resolved = try XCTUnwrap(
            StartPreviewEnvelope(data: Data(launchablePreviewFixture.utf8)))
        XCTAssertFalse(resolved.missingDoneContract)
    }

    func testCompiledCheckRowsCarryMustPassWithoutATopLevelField() throws {
        // The compiled row has no top-level must_pass: the decoder must read
        // raw.must_pass (or the can_fail inverse), never assume.
        let resolved = try XCTUnwrap(
            StartPreviewEnvelope(data: Data(launchablePreviewFixture.utf8)))
        XCTAssertEqual(resolved.doneContract?.checks.map(\.kind), ["shell"])
        XCTAssertEqual(resolved.doneContract?.checks.first?.mustPass, true)

        let optionalRow = """
        {"kind": "start",
         "done_contract": {"capabilities": {"network": "deny"},
                           "checks": [{"index": 1, "kind": "shell",
                                       "summary": "optional smoke",
                                       "behavioral": true, "can_fail": true,
                                       "network_required": "deny"}]},
         "will_start": false}
        """
        let preview = try XCTUnwrap(StartPreviewEnvelope(data: Data(optionalRow.utf8)))
        XCTAssertEqual(preview.doneContract?.checks.first?.mustPass, false,
                       "can_fail:true is the compiled spelling of must_pass:false")
    }

    // MARK: - Argv discipline

    func testDeclareArgvSeparatesTheOperatorTextWithDoubleDash() {
        let verb = PlannedVerb.defDoneDeclare(
            directory: "/Users/op/code/app",
            criteria: "--overwrite everything and call it done",
            provider: "mock", model: "mock-model")
        XCTAssertEqual(verb.arguments, [
            "def-done", "--dir", "/Users/op/code/app",
            "--provider", "mock", "--model", "mock-model",
            "--yes", "--json", "--", "--overwrite everything and call it done",
        ], "flag-shaped operator text reaches clap as literal text, after --")
        XCTAssertEqual(verb.verbWord, "def-done")

        let minimal = PlannedVerb.defDoneDeclare(directory: nil, criteria: "README stays correct")
        XCTAssertEqual(minimal.arguments,
                       ["def-done", "--yes", "--json", "--", "README stays correct"],
                       "no --dir declares in the client's working directory, the binary's default")
    }

    func testShowArgvIsReadOnly() {
        XCTAssertEqual(PlannedVerb.defDoneShow(directory: "/p").arguments,
                       ["def-done", "show", "--json", "--dir", "/p"])
        XCTAssertEqual(PlannedVerb.defDoneShow(directory: nil).arguments,
                       ["def-done", "show", "--json"])
        XCTAssertFalse(PlannedVerb.defDoneShow(directory: "/p").arguments.contains("--yes"),
                       "show never carries the approval flag: nothing may write")
    }

    // MARK: - Coordinator flow: refusal -> declare -> auto re-preview

    func testMissingContractDeclareAutoRerunsThePreview() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", missingContractPreviewFixture)
        cli.scriptStdout("def-done", writtenFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(
            goal: "keep the README correct",
            provider: "cli:codex-server",
            projectDirectory: "/Users/op/code/app")

        await controller.runPreview()
        guard case .blocked = controller.preview else {
            return XCTFail("expected the blocked preview: \(controller.preview)")
        }
        XCTAssertTrue(controller.previewNeedsContract,
                      "the typed missing signal swaps the try-line for the editor")

        // The declared contract makes the SAME request launchable next time.
        cli.scriptStdout("start", launchablePreviewFixture)
        await controller.declareContract(criteria: "README stays correct")

        guard case .declared(let declared) = controller.contract else {
            return XCTFail("expected the declared envelope: \(controller.contract)")
        }
        XCTAssertEqual(declared.status, "written")
        XCTAssertEqual(declared.contractPath, "/Users/op/code/app/.deadreckon/acceptance.yaml")
        guard case .ready = controller.preview else {
            return XCTFail("the preview must re-run to launchable by itself: \(controller.preview)")
        }
        XCTAssertFalse(controller.previewNeedsContract)

        // Exactly three children: preview, declare, re-preview. No chained
        // show — the declare envelope already holds the richer facts.
        XCTAssertEqual(cli.calls.map { $0.first ?? "" }, ["start", "def-done", "start"])
        let declare = try XCTUnwrap(cli.calls.first(where: { $0.first == "def-done" }))
        XCTAssertEqual(declare, [
            "def-done", "--dir", "/Users/op/code/app",
            "--provider", "cli:codex-server",
            "--yes", "--json", "--", "README stays correct",
        ], "the declare rides the chosen project dir and the -- discipline")
    }

    func testDeclareDoubleSubmitDispatchesOnce() async throws {
        let cli = GatedCLI()
        cli.serveImmediately(
            CLIRunResult(stdout: missingContractPreviewFixture, stderr: "", exitCode: 0))
        cli.gatedResult = CLIRunResult(stdout: writtenFixture, stderr: "", exitCode: 0)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g", projectDirectory: "/p")
        await controller.runPreview()

        let first = Task { await controller.declareContract(criteria: "README stays correct") }
        while cli.parkedCount == 0 { await Task.yield() }
        // The double-click's second task: in flight already, dispatches
        // nothing (the round-2 discipline — drafting calls the provider and
        // the write overwrites).
        await controller.declareContract(criteria: "README stays correct")
        XCTAssertEqual(cli.calls.filter { $0.first == "def-done" }.count, 1)

        cli.release()
        // The declare resumes and auto re-runs the preview, which parks in
        // turn: release it with a launchable envelope.
        while cli.parkedCount == 0 { await Task.yield() }
        cli.gatedResult = CLIRunResult(stdout: launchablePreviewFixture, stderr: "", exitCode: 0)
        cli.release()
        await first.value

        XCTAssertEqual(cli.calls.filter { $0.first == "def-done" }.count, 1,
                       "ONE declare for the double-click")
        guard case .declared = controller.contract else {
            return XCTFail("the first declare still lands: \(controller.contract)")
        }
        guard case .ready = controller.preview else {
            return XCTFail("and the auto re-preview completes: \(controller.preview)")
        }
    }

    func testDeclareRefusalRendersVerbatimAndStops() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", missingContractPreviewFixture)
        cli.scriptStdout("def-done", yesRefusalFixture, exitCode: 1)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g", projectDirectory: "/p")
        await controller.runPreview()

        await controller.declareContract(criteria: "README stays correct")
        guard case .refused(let refusal) = controller.contract else {
            return XCTFail("expected the typed refusal: \(controller.contract)")
        }
        XCTAssertEqual(
            refusal.message,
            "def-done --json writes a done contract without prompts and needs --yes to approve it")
        XCTAssertEqual(cli.callCount(leading: "start"), 1,
                       "a refusal is authoritative: no auto re-preview after it")
    }

    func testDeclareEnvelopeFreeFailureSaysItsOwnWords() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", missingContractPreviewFixture)
        cli.script("def-done", .result(CLIRunResult(stdout: "", stderr: "", exitCode: -9)))
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g", projectDirectory: "/p")
        await controller.runPreview()

        await controller.declareContract(criteria: "README stays correct")
        guard case .failed(let words) = controller.contract else {
            return XCTFail("expected the honest failure: \(controller.contract)")
        }
        XCTAssertTrue(words.contains("-9"), words)
        XCTAssertEqual(cli.callCount(leading: "start"), 1, "no re-preview on a failed declare")
    }

    func testEmptyCriteriaDeclaresNothing() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", missingContractPreviewFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g")
        await controller.runPreview()
        await controller.declareContract(criteria: "   \n")
        XCTAssertEqual(cli.callCount(leading: "def-done"), 0)
        guard case .idle = controller.contract else {
            return XCTFail("nothing dispatched, nothing claimed: \(controller.contract)")
        }
    }

    // MARK: - Existing contracts (show) and reset discipline

    func testProjectResolvedPreviewChainsOneShowForTheDeclaredRows() async throws {
        let cli = FakeCLI()
        cli.scriptStdout("start", launchablePreviewFixture)
        cli.scriptStdout("def-done", declaredShowFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g", projectDirectory: "/Users/op/code/app")

        await controller.runPreview()
        guard case .ready = controller.preview else {
            return XCTFail("expected the launchable preview: \(controller.preview)")
        }
        guard case .declared(let declared) = controller.contract else {
            return XCTFail("the project contract renders read-only: \(controller.contract)")
        }
        XCTAssertEqual(declared.status, "declared")
        XCTAssertEqual(declared.checks.count, 2)
        let show = try XCTUnwrap(cli.calls.first(where: { $0.first == "def-done" }))
        XCTAssertEqual(show, ["def-done", "show", "--json", "--dir", "/Users/op/code/app"])
    }

    func testDefaultGateShowLeavesTheDeclareAffordance() async {
        let cli = FakeCLI()
        cli.scriptStdout("def-done", defaultGateFixture)
        let controller = LayCourseController(cli: cli)
        await controller.loadDeclaredContract()
        guard case .idle = controller.contract else {
            return XCTFail("default_gate is a normal read, not a declared contract: \(controller.contract)")
        }
    }

    func testOperatorPreviewResetsAStaleContractBand() async {
        let cli = FakeCLI()
        cli.scriptStdout("start", launchablePreviewFixture)
        cli.scriptStdout("def-done", declaredShowFixture)
        let controller = LayCourseController(cli: cli)
        controller.request = StartRequest(goal: "g", projectDirectory: "/Users/op/code/app")
        await controller.runPreview()
        guard case .declared = controller.contract else {
            return XCTFail("precondition: \(controller.contract)")
        }

        // The operator re-points the sheet at ANOTHER project and previews:
        // the old project's declared rows must not survive next to it.
        controller.request.projectDirectory = "/Users/op/code/other"
        cli.scriptStdout("start", missingContractPreviewFixture)
        await controller.runPreview()
        guard case .blocked = controller.preview else {
            return XCTFail("expected the blocked preview: \(controller.preview)")
        }
        guard case .idle = controller.contract else {
            return XCTFail("stale declared rows from another project are a lie: \(controller.contract)")
        }
        XCTAssertTrue(controller.previewNeedsContract)
    }
}
