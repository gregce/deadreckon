import XCTest

@testable import DeadreckonKit

/// Implementer B (§R1): rewind preview→apply, undo, and the verb-flag
/// capability probe. Fixtures are byte-shaped from the SHIPPED Rust
/// emitters (main.rs rewind_command / machine_json.rs success_envelope /
/// undo.rs · chain/mod.rs undo facts) and from live 0.8.4 probes
/// (2026-08-07): the vendored binary's prose refusal paths are pinned
/// verbatim so the degraded renderings never drift from reality.
@MainActor
final class DispositionFlowTests: XCTestCase {
    // MARK: - argv (the complete literal contract)

    func testRewindPreviewArgv() {
        XCTAssertEqual(
            PlannedVerb.rewindPreview(runID: "8f2c31a0", checkpoint: "cp-000005").arguments,
            ["rewind", "8f2c31a0", "--to-checkpoint", "cp-000005", "--preview", "--json"])
    }

    func testRewindApplyArgv() {
        XCTAssertEqual(
            PlannedVerb.rewindApply(runID: "8f2c31a0", checkpoint: "cp-000005").arguments,
            ["rewind", "8f2c31a0", "--to-checkpoint", "cp-000005", "--apply", "--json"])
    }

    /// The shipped binary refuses non-interactive Job undo without
    /// --no-confirm (undo.rs); the sheet's destructive confirm IS that
    /// confirmation, so the flag is always present — same pattern as
    /// finish's --yes.
    func testUndoArgvCarriesNoConfirm() {
        XCTAssertEqual(
            PlannedVerb.undo(id: "aa49e5aa").arguments,
            ["undo", "aa49e5aa", "--no-confirm", "--json"])
    }

    func testTryArgv() {
        XCTAssertEqual(PlannedVerb.tryProof.arguments, ["try", "--json"])
    }

    func testVerbWords() {
        XCTAssertEqual(PlannedVerb.rewindPreview(runID: "r", checkpoint: "c").verbWord, "rewind")
        XCTAssertEqual(PlannedVerb.undo(id: "j").verbWord, "undo")
        XCTAssertEqual(PlannedVerb.tryProof.verbWord, "try")
    }

    // MARK: - RewindEnvelope decode (shipped bespoke payload)

    /// CORRECTED from spec §P7's guess: `files` is a plain array of paths —
    /// no per-file change word, no per-file hash-guard state. The decode
    /// must carry exactly what the emitter wrote.
    func testRewindEnvelopeDecodesShippedShape() throws {
        let envelope = try XCTUnwrap(
            RewindEnvelope(data: Data(Fixtures.rewindPreview.utf8)))
        XCTAssertEqual(envelope.runID, "8f2c31a09b7d4e21aa10cc4dd21b1f10")
        XCTAssertEqual(envelope.mode, "preview")
        XCTAssertEqual(envelope.targetKind, "checkpoint")
        XCTAssertEqual(envelope.targetID, "cp-000005")
        XCTAssertEqual(envelope.checkpointID, "cp-000005")
        XCTAssertEqual(envelope.files, ["src/ledger.rs", "src/tail.rs"])
        XCTAssertEqual(envelope.verdict?.kind, "preview")
        XCTAssertEqual(envelope.verdict?.evidencePairs.first?.0, "run")
        XCTAssertNotNil(envelope.previewDir)
    }

    /// A document without the required facts decodes to nil (fail closed),
    /// never a guessed preview.
    func testRewindEnvelopeFailsClosedOnForeignShape() {
        XCTAssertNil(RewindEnvelope(data: Data("{\"kind\":\"job_status\"}".utf8)))
        XCTAssertNil(RewindEnvelope(data: Data("not json".utf8)))
    }

    // MARK: - UndoEnvelope decode (armed G1 scaffold, three kinds)

    func testUndoEnvelopeRunSnapshot() throws {
        let envelope = try XCTUnwrap(UndoEnvelope(data: Data(Fixtures.undoRunSnapshot.utf8)))
        XCTAssertEqual(envelope.status, "completed")
        XCTAssertEqual(envelope.undoKind, "run-snapshot")
        XCTAssertEqual(envelope.restoredTurn, 3)
        XCTAssertEqual(envelope.snapshot, "/Users/op/.deadreckon/runstate/s/runs/r1/snapshots/turn-3")
        XCTAssertEqual(envelope.workspace, "/Users/op/project")
    }

    func testUndoEnvelopeJobDelivery() throws {
        let envelope = try XCTUnwrap(UndoEnvelope(data: Data(Fixtures.undoJobDelivery.utf8)))
        XCTAssertEqual(envelope.undoKind, "job-delivery")
        XCTAssertEqual(envelope.destination, "/Users/op/project")
        XCTAssertEqual(envelope.targetRef, "refs/heads/main")
        XCTAssertEqual(envelope.revertedRevision, "96346a06")
        XCTAssertEqual(envelope.undoRevision, "b007322e")
        XCTAssertEqual(envelope.alreadyUndone, false)
        XCTAssertEqual(envelope.nextActions.first, "deadreckon show aa49e5aa")
    }

    func testUndoEnvelopeChain() throws {
        let envelope = try XCTUnwrap(UndoEnvelope(data: Data(Fixtures.undoChain.utf8)))
        XCTAssertEqual(envelope.undoKind, "chain")
        XCTAssertEqual(envelope.undoneSteps, 4)
        XCTAssertEqual(envelope.status, "no-op")
    }

    func testUndoEnvelopeFailsClosedOnWrongKind() {
        XCTAssertNil(UndoEnvelope(data: Data("{\"kind\":\"kill\",\"id\":\"x\"}".utf8)))
    }

    // MARK: - RewindCoordinator (preview first, always)

    func testRewindPreviewSuccessThenApply() async {
        let cli = SettingsFakeCLI()
        cli.script("rewind r1 --to-checkpoint cp-000005 --preview",
                   stdout: Fixtures.rewindPreview)
        cli.script("rewind r1 --to-checkpoint cp-000005 --apply",
                   stdout: Fixtures.rewindApplied)
        let coordinator = RewindCoordinator(runID: "r1", checkpointID: "cp-000005", cli: cli)
        await coordinator.loadPreview()
        guard case .previewed(let preview) = coordinator.phase else {
            return XCTFail("expected previewed, got \(coordinator.phase)")
        }
        XCTAssertEqual(preview.files.count, 2)
        await coordinator.apply()
        guard case .applied(let applied) = coordinator.phase else {
            return XCTFail("expected applied, got \(coordinator.phase)")
        }
        XCTAssertEqual(applied.mode, "apply")
    }

    /// Preview-first is structural: apply from any non-previewed phase
    /// dispatches NOTHING.
    func testApplyWithoutPreviewDispatchesNothing() async {
        let cli = SettingsFakeCLI()
        let coordinator = RewindCoordinator(runID: "r1", checkpointID: "cp-1", cli: cli)
        await coordinator.apply()
        XCTAssertEqual(coordinator.phase, .idle)
        XCTAssertTrue(cli.calls.isEmpty, "apply must not reach the binary without a preview")
    }

    /// The vendored 0.8.4 binary's rewind refusals are PROSE on stderr
    /// (live-pinned: the driver fence on a job-owned run) — the coordinator
    /// classifies envelope-free and keeps the words verbatim; nothing is
    /// invented.
    func testProseRefusalRendersEnvelopeFreeVerbatim() async {
        let cli = SettingsFakeCLI()
        let prose = """
        error: invalid input: rewind cannot mutate d7524b52 because it belongs to durable Job aa49e5aa
        try: deadreckon attach aa49e5aa
        """
        cli.script("rewind", stdout: "", stderr: prose, exitCode: 1)
        let coordinator = RewindCoordinator(runID: "d7524b52", checkpointID: "cp-1", cli: cli)
        await coordinator.loadPreview()
        guard case .previewEnvelopeFree(let exitCode, let words) = coordinator.phase else {
            return XCTFail("expected envelope-free, got \(coordinator.phase)")
        }
        XCTAssertEqual(exitCode, 1)
        XCTAssertTrue(words.contains("rewind cannot mutate d7524b52"))
    }

    /// A future binary's armed refusal (the hash guard) is the shared error
    /// envelope — authoritative, rendered verbatim, no override.
    func testApplyHashGuardRefusalIsTyped() async {
        let cli = SettingsFakeCLI()
        cli.script("rewind r1 --to-checkpoint cp-1 --preview", stdout: Fixtures.rewindPreview)
        cli.script("rewind r1 --to-checkpoint cp-1 --apply",
                   stdout: Fixtures.rewindGuardRefusal, exitCode: 1)
        let coordinator = RewindCoordinator(runID: "r1", checkpointID: "cp-1", cli: cli)
        await coordinator.loadPreview()
        await coordinator.apply()
        guard case .applyRefused(let refusal) = coordinator.phase else {
            return XCTFail("expected applyRefused, got \(coordinator.phase)")
        }
        XCTAssertTrue(refusal.message.contains("unrelated edits"))
        XCTAssertEqual(refusal.verb, "rewind")
    }

    // MARK: - UndoCoordinator (one dispatch per sheet)

    func testUndoSuccessDecodesEnvelope() async {
        let cli = SettingsFakeCLI()
        cli.script("undo", stdout: Fixtures.undoJobDelivery)
        let coordinator = UndoCoordinator(targetID: "aa49e5aa", cli: cli)
        await coordinator.run()
        guard case .done(let envelope) = coordinator.phase else {
            return XCTFail("expected done, got \(coordinator.phase)")
        }
        XCTAssertEqual(envelope.undoKind, "job-delivery")
        XCTAssertEqual(cli.calls, [["undo", "aa49e5aa", "--no-confirm", "--json"]])
    }

    func testUndoIsOneDispatchPerSheet() async {
        let cli = SettingsFakeCLI()
        cli.script("undo", stdout: Fixtures.undoJobDelivery)
        let coordinator = UndoCoordinator(targetID: "aa49e5aa", cli: cli)
        await coordinator.run()
        await coordinator.run()
        XCTAssertEqual(cli.calls.count, 1, "a terminal phase disarms re-dispatch")
    }

    /// The vendored 0.8.4 binary rejects `undo --json` at clap (exit 2,
    /// live-pinned): envelope-free, words verbatim. The affordance is
    /// capability-gated so this path exists only as a belt-and-braces
    /// honesty floor.
    func testUndoAgainstOlderBinaryStaysHonest() async {
        let cli = SettingsFakeCLI()
        let clap = """
        error: unexpected argument '--json' found

          tip: to pass '--json' as a value, use '-- --json'

        Usage: deadreckon undo <ID>
        """
        cli.script("undo", stdout: "", stderr: clap, exitCode: 2)
        let coordinator = UndoCoordinator(targetID: "aa49e5aa", cli: cli)
        await coordinator.run()
        guard case .envelopeFree(let exitCode, let words) = coordinator.phase else {
            return XCTFail("expected envelopeFree, got \(coordinator.phase)")
        }
        XCTAssertEqual(exitCode, 2)
        XCTAssertTrue(words.contains("unexpected argument '--json'"))
    }

    // MARK: - VerbCapabilityProbe (no envelope → no control)

    /// The live 0.8.4 `undo --help` (pinned): no `--json` listed → the
    /// control never renders.
    func testUndoProbeAgainstOlderBinaryIsMissing() async {
        let cli = SettingsFakeCLI()
        cli.script("undo --help", stdout: Fixtures.undoHelp084)
        let probe = VerbCapabilityProbe(cli: cli, verb: ["undo"])
        await probe.probe()
        XCTAssertEqual(probe.state, .missing)
        XCTAssertFalse(probe.isArmed)
    }

    /// A binary whose undo lists `--json` arms the affordance with zero
    /// label edits.
    func testUndoProbeAgainstArmedBinaryArms() async {
        let cli = SettingsFakeCLI()
        cli.script("undo --help",
                   stdout: Fixtures.undoHelp084 + "\n      --json  Emit machine-readable JSON\n")
        let probe = VerbCapabilityProbe(cli: cli, verb: ["undo"])
        await probe.probe()
        XCTAssertTrue(probe.isArmed)
    }

    /// The live 0.8.4 `rewind --help` (pinned) DOES list `--json`: the
    /// Recorder's [Rewind…] arms against the vendored binary.
    func testRewindProbeAgainstVendoredBinaryArms() async {
        let cli = SettingsFakeCLI()
        cli.script("rewind --help", stdout: Fixtures.rewindHelp084)
        let probe = VerbCapabilityProbe(cli: cli, verb: ["rewind"])
        await probe.probe()
        XCTAssertTrue(probe.isArmed)
    }

    func testProbeFailureCarriesWords() async {
        let cli = SettingsFakeCLI()
        cli.scriptFailure("undo --help", FleetCLIError.binaryUnavailable("no trusted binary"))
        let probe = VerbCapabilityProbe(cli: cli, verb: ["undo"])
        await probe.probe()
        guard case .failed(let words) = probe.state else {
            return XCTFail("expected failed, got \(probe.state)")
        }
        XCTAssertTrue(words.contains("no trusted binary"))
        XCTAssertFalse(probe.isArmed)
    }

    // MARK: - Fixtures

    enum Fixtures {
        /// main.rs rewind_command --json (preview): surface.add_to_json over
        /// {run_id, mode, target, checkpoint_id, preview_dir, files}.
        static let rewindPreview = """
        {
          "run_id": "8f2c31a09b7d4e21aa10cc4dd21b1f10",
          "mode": "preview",
          "target": { "kind": "checkpoint", "id": "cp-000005" },
          "checkpoint_id": "cp-000005",
          "preview_dir": "/Users/op/.deadreckon/runstate/s/runs/8f2c31a09b7d4e21aa10cc4dd21b1f10/rewind-preview/cp-000005",
          "files": ["src/ledger.rs", "src/tail.rs"],
          "primary_action": "deadreckon rewind 8f2c31a0 --to-checkpoint cp-000005 --apply",
          "verdict": {
            "kind": "preview",
            "label": "preview rewind 8f2c31a0",
            "subject": "8f2c31a0",
            "recommended_command": "deadreckon rewind 8f2c31a0 --to-checkpoint cp-000005 --apply",
            "explanation": "DeadReckon materialized checkpoint cp-000005 into a preview directory without changing the run workspace.",
            "evidence": [
              ["run", "8f2c31a0"],
              ["checkpoint", "cp-000005"],
              ["target", "Checkpoint cp-000005"],
              ["changed files", "2"],
              ["preview", "/Users/op/.deadreckon/runstate/s/runs/8f2c31a09b7d4e21aa10cc4dd21b1f10/rewind-preview/cp-000005"],
              ["file 1", "src/ledger.rs"],
              ["file 2", "src/tail.rs"]
            ]
          }
        }
        """

        static let rewindApplied = rewindPreview
            .replacingOccurrences(of: "\"mode\": \"preview\"", with: "\"mode\": \"apply\"")
            .replacingOccurrences(of: "\"kind\": \"preview\"", with: "\"kind\": \"completed\"")

        /// The armed hash-guard refusal (future binary): the shared error
        /// envelope carrying the guard's own sentence.
        static let rewindGuardRefusal = """
        {"kind": "error", "code": 1, "verb": "rewind",
         "message": "invalid input: refusing rewind because src/tail.rs has unrelated edits",
         "try_lines": ["deadreckon rewind 8f2c31a0 --to-checkpoint cp-000005 --preview"]}
        """

        /// machine_json.rs success_envelope + main.rs undo facts.
        static let undoRunSnapshot = """
        {
          "kind": "undo", "id": "r1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6", "status": "completed",
          "next_actions": ["deadreckon show r1a2b3c4"],
          "try_lines": [],
          "undo_kind": "run-snapshot",
          "restored_turn": 3,
          "snapshot": "/Users/op/.deadreckon/runstate/s/runs/r1/snapshots/turn-3",
          "workspace": "/Users/op/project",
          "primary_action": "deadreckon show r1a2b3c4",
          "verdict": {
            "kind": "completed",
            "label": "completed undo r1a2b3c4",
            "subject": "r1a2b3c4",
            "recommended_command": "deadreckon show r1a2b3c4",
            "explanation": "DeadReckon restored the run workspace to snapshot turn 3.",
            "evidence": [["run", "r1a2b3c4"], ["turn", "3"]]
          }
        }
        """

        /// commands/undo.rs print_undo_result facts.
        static let undoJobDelivery = """
        {
          "kind": "undo", "id": "aa49e5aa", "status": "completed",
          "next_actions": ["deadreckon show aa49e5aa"],
          "try_lines": [],
          "undo_kind": "job-delivery",
          "destination": "/Users/op/project",
          "target_ref": "refs/heads/main",
          "reverted_revision": "96346a06",
          "undo_revision": "b007322e",
          "already_undone": false,
          "primary_action": "deadreckon show aa49e5aa",
          "verdict": {
            "kind": "completed",
            "label": "completed undo aa49e5aa",
            "subject": "aa49e5aa",
            "recommended_command": "deadreckon show aa49e5aa",
            "explanation": "DeadReckon reverted the verified applied Job delivery with one exact revert commit.",
            "evidence": [["job", "aa49e5aa"], ["destination", "/Users/op/project"]]
          }
        }
        """

        /// commands/chain/mod.rs chain undo facts (VerdictKind::Noop).
        static let undoChain = """
        {
          "kind": "undo", "id": "chain-9", "status": "no-op",
          "next_actions": ["deadreckon chain show chain-9"],
          "try_lines": [],
          "undo_kind": "chain",
          "undone_steps": 4,
          "workspace": "/Users/op/project",
          "primary_action": "deadreckon chain show chain-9",
          "verdict": {
            "kind": "no-op",
            "label": "no-op undo chain-9",
            "subject": "chain-9",
            "recommended_command": "deadreckon chain show chain-9",
            "explanation": "DeadReckon reverted the applied chain commits and marked the chain undone.",
            "evidence": [["undone steps", "4"]]
          }
        }
        """

        /// Live 0.8.4 `undo --help` (2026-08-07): no --json anywhere.
        static let undoHelp084 = """
        Restore an in-place run snapshot

        Usage: deadreckon undo [OPTIONS] [ID]

        Options:
          -h, --help  Print help

        Cleanup And Recovery:
              --run <RUN>    Deprecated alias for the positional id; every other lifecycle verb takes it positionally
              --turn <TURN>  Snapshot turn to restore
              --no-confirm   Skip confirmation when reverting a verified applied Job or chain delivery [aliases: --yes]
          [ID]               Job, run, or chain id, unique prefix, or latest; defaults to current project's latest
        """

        /// Live 0.8.4 `rewind --help` (2026-08-07): --json IS listed.
        static let rewindHelp084 = """
        Preview or apply a provider checkpoint rewind

        Usage: deadreckon rewind [OPTIONS] <RUN_ID>

        Options:
          -h, --help  Print help

        Cleanup And Recovery:
              --to-turn <TO_TURN>
                  Rewind to the last provider checkpoint for this turn
              --to-checkpoint <TO_CHECKPOINT>
                  Rewind to this checkpoint id
              --preview
                  Preview the rewind without changing files
              --apply
                  Apply the rewind after hash-guarding changed files
              --json
                  Emit machine-readable JSON
        """
    }
}
