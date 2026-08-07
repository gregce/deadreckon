import XCTest

@testable import DeadreckonKit

/// The single verb dispatcher: argv arrays (never shell strings), --json
/// always on, the literal display line, and the trust-rule guarantees that
/// live in the type system (no dr-gate constructor, no override parameter).
final class MutationRunnerTests: XCTestCase {

    func testEveryVerbAppendsJSON() {
        let verbs: [PlannedVerb] = [
            .steer(id: "j1", note: "note"),
            .kill(id: "j1", escalate: false),
            .kill(id: "j1", escalate: true),
            .finishDryRun(id: "j1", destination: .apply(autostash: true, cleanup: false)),
            .finish(id: "j1", destination: .export(path: "/tmp/out")),
            .extendJob(parentID: "j1", goal: "again", note: "why"),
            .startPreview(StartRequest(goal: "g")),
            .startExecute(planFilePath: "/tmp/plan.json", spendAcknowledged: false),
        ]
        for verb in verbs {
            XCTAssertTrue(verb.arguments.contains("--json"), "\(verb) must always speak JSON")
        }
    }

    func testKillArgvCarriesExplicitEscalateOnly() {
        XCTAssertEqual(PlannedVerb.kill(id: "j1", escalate: false).arguments,
                       ["kill", "j1", "--json"])
        XCTAssertEqual(PlannedVerb.kill(id: "j1", escalate: true).arguments,
                       ["kill", "j1", "--escalate", "--json"])
    }

    func testSteerArgvTerminatesFlagsBeforeOperatorText() {
        XCTAssertEqual(PlannedVerb.steer(id: "run-1", note: "cap backoff at 5m").arguments,
                       ["steer", "--json", "--", "run-1", "cap backoff at 5m"])
    }

    /// Flag-injection pin: operator-typed text beginning with "-" (a pasted
    /// bullet, or a token shaped like a real flag) must reach clap AFTER the
    /// `--` terminator — literal text, never parsed as a flag.
    func testDashPrefixedOperatorTextRoundTripsAsText() {
        let evilGoal = "--plan=/tmp/evil.json"
        let preview = PlannedVerb.startPreview(StartRequest(goal: evilGoal)).arguments
        XCTAssertEqual(preview, ["start", "--json", "--", evilGoal])
        let separator = preview.firstIndex(of: "--")!
        XCTAssertTrue(preview.index(after: separator) < preview.endIndex
                      && preview.last == evilGoal,
                      "the goal must ride after -- as a positional, byte-identical")

        XCTAssertEqual(
            PlannedVerb.steer(id: "run-1", note: "- fix login first").arguments,
            ["steer", "--json", "--", "run-1", "- fix login first"])

        // The extend note rides a single =-token so a dash-value survives
        // clap value parsing; parent and goal ride after --.
        XCTAssertEqual(
            PlannedVerb.extendJob(parentID: "run-p", goal: "--goal-file=/x",
                                  note: "--use the cached index").arguments,
            ["extend", "--note=--use the cached index", "--yes", "--json",
             "--", "run-p", "--goal-file=/x"])
    }

    func testFinishDestinationsMapOneToOneToDocumentedFlags() {
        XCTAssertEqual(
            PlannedVerb.finish(id: "j1", destination: .apply(autostash: true, cleanup: true)).arguments,
            ["finish", "j1", "--autostash", "--cleanup", "--yes", "--json"])
        XCTAssertEqual(
            PlannedVerb.finish(id: "j1", destination: .apply(autostash: false, cleanup: false)).arguments,
            ["finish", "j1", "--yes", "--json"])
        XCTAssertEqual(
            PlannedVerb.finish(id: "j1", destination: .export(path: "/Users/op/reviews/x")).arguments,
            ["finish", "j1", "--dest", "/Users/op/reviews/x", "--yes", "--json"])
    }

    /// The runner executes argv directly (no shell) and the Rust side takes
    /// the path verbatim: a typed "~" must expand app-side or the binary
    /// creates a directory literally named "~".
    func testExportDestinationExpandsTildeToAnAbsolutePath() {
        let argv = PlannedVerb.finish(
            id: "j1", destination: .export(path: "~/deadreckon-out")).arguments
        let dest = argv[argv.firstIndex(of: "--dest")!.advanced(by: 1)]
        XCTAssertEqual(dest, NSHomeDirectory() + "/deadreckon-out")
        XCTAssertTrue(dest.hasPrefix("/"), "--dest must always be absolute")
    }

    func testFinishDryRunHasNoYesFlagAndFinishHasNoOverrideFlags() {
        let dryRun = PlannedVerb.finishDryRun(
            id: "j1", destination: .export(path: "/tmp/x")).arguments
        XCTAssertTrue(dryRun.contains("--dry-run"))
        XCTAssertFalse(dryRun.contains("--yes"), "the preview leg is report-only, nothing mutates")

        // No override affordance exists anywhere (trust rule 2): the finish
        // argv can never carry a force/overwrite flag because PlannedVerb
        // has no parameter to express one.
        for destination: FinishDestinationChoice in [
            .apply(autostash: true, cleanup: true), .export(path: "/tmp/x"),
        ] {
            let argv = PlannedVerb.finish(id: "j1", destination: destination).arguments
            XCTAssertFalse(argv.contains("--force"))
            XCTAssertFalse(argv.contains("--overwrite"))
        }
    }

    func testExtendArgvCarriesNoteAndYes() {
        XCTAssertEqual(
            PlannedVerb.extendJob(parentID: "run-p", goal: "fix the stub", note: "gate accepted a stub").arguments,
            ["extend", "--note=gate accepted a stub", "--yes", "--json",
             "--", "run-p", "fix the stub"])
        XCTAssertEqual(
            PlannedVerb.extendJob(parentID: "run-p", goal: "fix", note: nil).arguments,
            ["extend", "--yes", "--json", "--", "run-p", "fix"])
    }

    func testStartPreviewHasNoYesAndExecuteReplaysThePlan() {
        let preview = PlannedVerb.startPreview(
            StartRequest(goal: "g", provider: "cli:codex-server", model: "gpt-5.3", maxSpendUSD: 25))
        XCTAssertEqual(preview.arguments,
                       ["start", "--provider", "cli:codex-server",
                        "--model", "gpt-5.3", "--max-spend", "25", "--json", "--", "g"])
        XCTAssertFalse(preview.arguments.contains("--yes"), "the preview leg is read-only")

        XCTAssertEqual(
            PlannedVerb.startExecute(planFilePath: "/tmp/p.json", spendAcknowledged: false).arguments,
            ["start", "--plan", "/tmp/p.json", "--yes", "--json"])
        XCTAssertEqual(
            PlannedVerb.startExecute(planFilePath: "/tmp/p.json", spendAcknowledged: true).arguments,
            ["start", "--plan", "/tmp/p.json", "--yes", "--i-know-its-a-lot", "--json"])
    }

    func testProjectDirectoryRidesFromOnBothLegs() {
        XCTAssertEqual(
            PlannedVerb.startPreview(
                StartRequest(goal: "g", projectDirectory: "/Users/op/itavero/billing")).arguments,
            ["start", "--from", "/Users/op/itavero/billing", "--json", "--", "g"])
        XCTAssertEqual(
            PlannedVerb.startExecute(planFilePath: "/tmp/p.json", spendAcknowledged: false,
                                     fromPath: "/Users/op/itavero/billing").arguments,
            ["start", "--plan", "/tmp/p.json", "--yes",
             "--from", "/Users/op/itavero/billing", "--json"],
            "the launch plan does not embed the source; --from must repeat on execute")
    }

    func testLiteralCommandLineQuotesUnsafeWordsOnly() {
        let runner = MutationRunner(cli: FakeCLI())
        XCTAssertEqual(
            runner.literalCommandLine(for: .steer(id: "run-1", note: "cap backoff at 5m")),
            "deadreckon steer --json -- run-1 'cap backoff at 5m'")
        XCTAssertEqual(
            runner.literalCommandLine(for: .kill(id: "job-1", escalate: true)),
            "deadreckon kill job-1 --escalate --json")
        XCTAssertEqual(
            MutationRunner.shellQuote("it's"), "'it'\\''s'")
    }

    func testRunClassifiesRefusalAndSuccess() async {
        let cli = FakeCLI()
        cli.scriptStdout("steer", """
        {"kind": "error", "code": 1, "verb": "steer", "message": "refused", "try_lines": ["deadreckon list"]}
        """, exitCode: 1)
        let runner = MutationRunner(cli: cli)
        let refused = await runner.run(.steer(id: "x", note: "n"))
        XCTAssertEqual(refused.refusal?.message, "refused")
        XCTAssertEqual(refused.exitCode, 1)

        cli.scriptStdout("kill", """
        {"kind": "kill", "id": "x", "status": "killed", "next_actions": [], "try_lines": [],
         "signal": "SIGTERM", "escalated": false, "terminal_phase_observed": true}
        """)
        let killed = await runner.run(.kill(id: "x", escalate: false))
        XCTAssertTrue(killed.isSuccess)
        XCTAssertEqual(killed.primary?.kill?.signal, "SIGTERM")
    }

    func testBinaryUnavailableComesBackAsEnvelopeFreeWords() async {
        final class UnavailableCLI: FleetCLIRunning {
            func run(arguments: [String], timeout: TimeInterval) async throws -> CLIRunResult {
                throw FleetCLIError.binaryUnavailable("no trusted binary")
            }
            func terminateInFlight(patience: TimeInterval) -> Int { 0 }
            var inFlightCount: Int { 0 }
        }
        let runner = MutationRunner(cli: UnavailableCLI())
        let result = await runner.run(.kill(id: "x", escalate: false))
        XCTAssertTrue(result.isEnvelopeFree)
        XCTAssertEqual(result.envelopeFreeWords, "no trusted binary")
    }

    /// A crashed binary can leave BOTH streams empty (SIGKILL before any
    /// output): the degraded surfaces must still say the one honest fact
    /// (the exit code), never render a blank failure.
    func testEnvelopeFreeWordsNameTheExitCodeWhenBothStreamsAreEmpty() {
        let result = MutationResult.classify(stdout: "", stderr: "", exitCode: 9)
        XCTAssertTrue(result.isEnvelopeFree)
        XCTAssertEqual(result.envelopeFreeWords, "exit 9 with no output")
    }

    func testPlanFileWriterStaysOutOfDeadreckonHome() throws {
        let url = try MutationRunner.writeLaunchPlanFile(Data("{\"a\":1}".utf8))
        defer { try? FileManager.default.removeItem(at: url) }
        XCTAssertFalse(url.path.contains(".deadreckon"),
                       "the plan scratch file must never live under DEADRECKON_HOME")
        XCTAssertEqual(try Data(contentsOf: url), Data("{\"a\":1}".utf8))
    }
}
