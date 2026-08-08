import Foundation

// Implementer B (SETTINGS-SCREENS-SPEC §R1): the run-disposition engines —
// rewind (preview-first, always) and undo (envelope-offered, capability-
// gated) — plus the flag-capability probe that arms both against the
// binary actually in the bundle. Kit logic, thin views, fake-runner tested.

// MARK: - Verb flag capability (probe, never a hardcoded gap label)

/// Probes whether the located binary's VERB speaks `--json` by reading the
/// verb's own `--help` (read-only, no side effects): clap generates the
/// flag listing from the real flag set, so the presence of the literal
/// token `--json` IS the capability fact. FULL-DRIVE finding 6 discipline:
/// the surface arms itself from this probe at open — when a newer binary
/// lands the affordance appears with zero label edits, and against the
/// vendored 0.8.4 binary (whose `undo` predates `--json`) the control
/// never renders.
@MainActor
public final class VerbCapabilityProbe: ObservableObject {
    public enum State: Equatable {
        case unknown
        case probing
        /// The verb lists `--json`: the affordance arms.
        case armed
        /// The verb's help does not list `--json`; the probe's own words
        /// (the help text) back the degraded rendering.
        case missing
        /// The probe itself failed (locator/launch); words verbatim.
        case failed(String)
    }

    @Published public private(set) var state: State = .unknown

    private let cli: FleetCLIRunning
    private let verb: [String]

    /// `verb` is the subcommand path to probe, e.g. ["rewind"] or ["undo"].
    public init(cli: FleetCLIRunning, verb: [String]) {
        self.cli = cli
        self.verb = verb
    }

    public var isArmed: Bool { state == .armed }

    public func probe() async {
        if case .probing = state { return }
        if case .armed = state { return }
        state = .probing
        do {
            let result = try await cli.run(arguments: verb + ["--help"], timeout: 30)
            let listing = result.stdout + "\n" + result.stderr
            state = listing.contains("--json") ? .armed : .missing
        } catch {
            state = .failed(
                (error as? FleetCLIError)?.errorDescription ?? error.localizedDescription)
        }
    }
}

// MARK: - Rewind (preview first, always)

/// The RewindSheet engine (§R1). Flow, enforced by construction:
/// 1. `loadPreview()` — `rewind RUN --to-checkpoint C --preview --json`,
///    dispatched the moment the sheet opens. Read-only binary-side.
/// 2. `apply()` — reachable ONLY from a successful preview; dispatches the
///    `--apply` form. The hash guard is the CLI's: a drifted file refuses
///    binary-side and the refusal renders verbatim.
/// A prose refusal (the vendored 0.8.4 binary pre-dates the rewind refusal
/// arming) classifies as envelope-free and renders the established pattern:
/// "The CLI answered without a machine envelope (exit N); it said:" —
/// words verbatim, never a guessed state.
@MainActor
public final class RewindCoordinator: ObservableObject {
    public enum Phase: Equatable {
        case idle
        case previewing
        case previewed(RewindEnvelope)
        case previewRefused(ErrorEnvelope)
        case previewEnvelopeFree(exitCode: Int32, words: String)
        /// Applying; the preview stays visible (its facts are what the
        /// operator confirmed).
        case applying(RewindEnvelope)
        case applied(RewindEnvelope)
        case applyRefused(ErrorEnvelope)
        case applyEnvelopeFree(exitCode: Int32, words: String)
    }

    public let runID: String
    public let checkpointID: String

    @Published public private(set) var phase: Phase = .idle

    private let runner: MutationRunner

    public init(runID: String, checkpointID: String, cli: FleetCLIRunning) {
        self.runID = runID
        self.checkpointID = checkpointID
        runner = MutationRunner(cli: cli)
    }

    public var previewCommandLine: String {
        runner.literalCommandLine(for: .rewindPreview(runID: runID, checkpoint: checkpointID))
    }

    public var applyCommandLine: String {
        runner.literalCommandLine(for: .rewindApply(runID: runID, checkpoint: checkpointID))
    }

    public func loadPreview() async {
        // Preview runs once per sheet open; re-entry dispatches nothing.
        guard case .idle = phase else { return }
        phase = .previewing
        let result = await runner.run(.rewindPreview(runID: runID, checkpoint: checkpointID))
        if let refusal = result.refusal {
            phase = .previewRefused(refusal)
        } else if let object = result.rawObjects.last,
                  let envelope = RewindEnvelope(data: object) {
            phase = .previewed(envelope)
        } else {
            phase = .previewEnvelopeFree(
                exitCode: result.exitCode, words: result.envelopeFreeWords)
        }
    }

    /// Preview-first is structural: apply dispatches ONLY from a previewed
    /// phase (tested); a double-click's second task dispatches nothing.
    public func apply() async {
        guard case .previewed(let preview) = phase else { return }
        phase = .applying(preview)
        let result = await runner.run(.rewindApply(runID: runID, checkpoint: checkpointID))
        if let refusal = result.refusal {
            phase = .applyRefused(refusal)
        } else if let object = result.rawObjects.last,
                  let envelope = RewindEnvelope(data: object) {
            phase = .applied(envelope)
        } else {
            phase = .applyEnvelopeFree(
                exitCode: result.exitCode, words: result.envelopeFreeWords)
        }
    }
}

// MARK: - Undo (envelope-offered, one dispatch per sheet)

/// The UndoSheet engine (§R1): `undo ID --no-confirm --json`, where the
/// sheet's destructive confirm IS the confirmation `--no-confirm`
/// formalizes (undo.rs refuses non-interactive Job undo without it — the
/// same pattern as finish's `--yes`). One undo per sheet: any terminal
/// state disarms re-dispatch; the operator opens a fresh surface to try
/// again, with fresh facts.
@MainActor
public final class UndoCoordinator: ObservableObject {
    public enum Phase: Equatable {
        case idle
        case running
        case done(UndoEnvelope)
        case refused(ErrorEnvelope)
        case envelopeFree(exitCode: Int32, words: String)
    }

    public let targetID: String

    @Published public private(set) var phase: Phase = .idle

    private let runner: MutationRunner

    public init(targetID: String, cli: FleetCLIRunning) {
        self.targetID = targetID
        runner = MutationRunner(cli: cli)
    }

    public var commandLine: String {
        runner.literalCommandLine(for: .undo(id: targetID))
    }

    public func run() async {
        // One dispatch per sheet: .idle is the only armed state.
        guard case .idle = phase else { return }
        phase = .running
        let result = await runner.run(.undo(id: targetID))
        if let refusal = result.refusal {
            phase = .refused(refusal)
        } else if let object = result.rawObjects.last,
                  let envelope = UndoEnvelope(data: object) {
            phase = .done(envelope)
        } else {
            phase = .envelopeFree(exitCode: result.exitCode, words: result.envelopeFreeWords)
        }
    }
}
