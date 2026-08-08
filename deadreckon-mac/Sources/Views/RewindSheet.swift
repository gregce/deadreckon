import DeadreckonKit
import SwiftUI

/// The rewind sheet (SETTINGS-SCREENS-SPEC §R1): preview first, always.
/// Opening the sheet immediately runs `rewind … --preview --json` — a
/// read-only materialization binary-side — and only the destructive
/// confirm dispatches `--apply`. The hash guard is the CLI's, not the
/// app's: a drifted file refuses binary-side and the refusal renders
/// verbatim. Against the pre-arming vendored binary, a prose refusal
/// renders the established envelope-free pattern — never a guessed state.
struct RewindSheet: View {
    let row: FleetRow
    let runID: String
    let checkpoint: CheckpointManifestDoc

    @Environment(\.dismiss) private var dismiss
    @StateObject private var coordinator: RewindCoordinator

    init(row: FleetRow, runID: String, checkpoint: CheckpointManifestDoc) {
        self.row = row
        self.runID = runID
        self.checkpoint = checkpoint
        _coordinator = StateObject(wrappedValue: RewindCoordinator(
            runID: runID, checkpointID: checkpoint.checkpointID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.border)
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    checkpointBand
                    resultBand
                }
                .padding(20)
            }
            Divider().overlay(Theme.border)
            footer
        }
        .frame(width: 560, height: 480)
        .background(Theme.windowBg)
        .task { await coordinator.loadPreview() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Rewind this run?")
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text(row.goal)
                .font(Theme.body(11.5))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(2)
            Text(runID)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.textTertiary)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: Checkpoint facts (from the manifest the card showed)

    private var checkpointBand: some View {
        VStack(alignment: .leading, spacing: 4) {
            Theme.sectionTitle("CHECKPOINT")
            HStack(spacing: 6) {
                Text(checkpoint.checkpointID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
                Text("turn \(checkpoint.deadreckonTurn) \u{00B7} \(Lexicon.checkpointTrigger(checkpoint.trigger))")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textSecondary)
                if checkpoint.fullAnchor {
                    StatusChip(text: "full snapshot", color: Theme.textSecondary)
                }
            }
        }
    }

    // MARK: Preview / result

    @ViewBuilder private var resultBand: some View {
        switch coordinator.phase {
        case .idle, .previewing:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("previewing \u{2014} the CLI materializes the checkpoint without touching the workspace\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textSecondary)
            }
        case .previewed(let preview):
            previewBody(preview)
        case .applying(let preview):
            previewBody(preview)
        case .applied(let applied):
            appliedBody(applied)
        case .previewRefused(let refusal), .applyRefused(let refusal):
            RefusalView(refusal: refusal)
        case .previewEnvelopeFree(let exitCode, let words),
             .applyEnvelopeFree(let exitCode, let words):
            envelopeFreeBody(exitCode: exitCode, words: words)
        }
    }

    @ViewBuilder private func previewBody(_ preview: RewindEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("FILES THAT WOULD CHANGE")
            if preview.files.isEmpty {
                Text("no files differ from this checkpoint")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
            }
            ForEach(preview.files, id: \.self) { path in
                Text(path)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            // The guard caption (§R1): the guard belongs to the CLI. The
            // shipped preview carries no per-file guard state — drift
            // surfaces as an apply refusal, quoted verbatim above.
            Text("Rewind refuses to touch a file that changed since the checkpoint unless its hash still matches \u{2014} the guard is the CLI\u{2019}s, not the app\u{2019}s.")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textTertiary)
                .fixedSize(horizontal: false, vertical: true)
            if let explanation = preview.verdict?.explanation {
                Text(explanation)
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    /// The applied result: the envelope's own facts, verbatim. The
    /// Recorder re-reads the manifest from the files — nothing here paints
    /// optimistic state.
    @ViewBuilder private func appliedBody(_ applied: RewindEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "checkmark.seal")
                    .foregroundStyle(Theme.success)
                Text(applied.verdict?.label ?? "rewind applied")
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
            }
            ForEach(applied.verdict?.evidencePairs ?? [], id: \.0) { pair in
                HStack(alignment: .top, spacing: 6) {
                    Text(pair.0)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textTertiary)
                        .frame(width: 110, alignment: .leading)
                    Text(pair.1)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                }
            }
            Text("The run updates from its files, not from this sheet.")
                .font(Theme.caption)
                .foregroundStyle(Theme.textTertiary)
        }
    }

    /// The established pre-arming pattern: the CLI's words verbatim,
    /// selectable, never re-worded into a guessed state.
    private func envelopeFreeBody(exitCode: Int32, words: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("The CLI answered without a machine envelope (exit \(exitCode)); it said:")
                .font(Theme.body(11, weight: .medium))
                .foregroundStyle(Theme.warn)
            Text(words)
                .font(Theme.monoM)
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: Footer

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            CommandLineView(command: coordinator.applyCommandLine)
            HStack(spacing: 8) {
                Button(dismissTitle) { dismiss() }
                    .buttonStyle(.themeStandard)
                    .keyboardShortcut(.cancelAction)
                Spacer()
                if case .applying = coordinator.phase {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("applying \u{2014} hash-guarding, then restoring\u{2026}")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textSecondary)
                    }
                }
                Button("Rewind Run") {
                    Task { await coordinator.apply() }
                }
                .buttonStyle(.themeDangerConfirm)
                .disabled(!applyEnabled)
                .help("deadreckon rewind \(runID) --to-checkpoint \(checkpoint.checkpointID) --apply --json")
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// Apply arms ONLY from a successful preview (preview-first is also
    /// structural in the coordinator).
    private var applyEnabled: Bool {
        if case .previewed = coordinator.phase { return true }
        return false
    }

    private var dismissTitle: String {
        switch coordinator.phase {
        case .idle, .previewing, .previewed, .applying: return "Cancel"
        case .applied, .previewRefused, .applyRefused,
             .previewEnvelopeFree, .applyEnvelopeFree: return "Close"
        }
    }
}

/// The undo sheet (§R1): restores the project files to the snapshot taken
/// before the result was applied. Offered ONLY where the approve success
/// envelope's own next actions advertised `deadreckon undo` AND the
/// capability probe confirmed the binary speaks `undo --json`. The
/// destructive confirm click IS the confirmation `--no-confirm` formalizes.
struct UndoSheet: View {
    let row: FleetRow

    @Environment(\.dismiss) private var dismiss
    @StateObject private var coordinator: UndoCoordinator

    init(row: FleetRow) {
        self.row = row
        _coordinator = StateObject(wrappedValue: UndoCoordinator(
            targetID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Undo this approval?")
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                Text(row.goal)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
                Text(row.jobID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .textSelection(.enabled)
            }
            Text("Restores the project files to the snapshot taken before the result was applied. The run stays finished \u{2014} only the applied files revert.")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textTertiary)
                .fixedSize(horizontal: false, vertical: true)

            CommandLineView(command: coordinator.commandLine)

            switch coordinator.phase {
            case .idle:
                EmptyView()
            case .running:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("reverting\u{2026}")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                }
            case .done(let envelope):
                doneBody(envelope)
            case .refused(let refusal):
                RefusalView(refusal: refusal)
            case .envelopeFree(let exitCode, let words):
                VStack(alignment: .leading, spacing: 4) {
                    Text("The CLI answered without a machine envelope (exit \(exitCode)); it said:")
                        .font(Theme.body(11, weight: .medium))
                        .foregroundStyle(Theme.warn)
                    Text(words)
                        .font(Theme.monoM)
                        .foregroundStyle(Theme.textPrimary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            HStack {
                Spacer()
                Button(dismissTitle) { dismiss() }
                    .buttonStyle(.themeStandard)
                    .keyboardShortcut(.cancelAction)
                Button("Undo") {
                    Task { await coordinator.run() }
                }
                .buttonStyle(.themeDangerConfirm)
                .disabled(!undoEnabled)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
    }

    /// The envelope's own facts: what was reverted, where — undo_kind
    /// discriminated, nothing invented.
    @ViewBuilder private func doneBody(_ envelope: UndoEnvelope) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "checkmark.seal")
                    .foregroundStyle(Theme.success)
                Text(envelope.status == "no-op" ? "Already undone" : "Undone")
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
            }
            ForEach(undoFacts(envelope), id: \.0) { fact in
                HStack(alignment: .top, spacing: 6) {
                    Text(fact.0)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textTertiary)
                        .frame(width: 110, alignment: .leading)
                    Text(fact.1)
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                }
            }
            ForEach(envelope.nextActions, id: \.self) { action in
                Text("next: \(action)")
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.accent)
                    .textSelection(.enabled)
            }
            Text("The run updates from its files, not from this sheet.")
                .font(Theme.caption)
                .foregroundStyle(Theme.textTertiary)
        }
    }

    private func undoFacts(_ envelope: UndoEnvelope) -> [(String, String)] {
        var facts: [(String, String)] = []
        if let kind = envelope.undoKind { facts.append(("undo kind", kind)) }
        if let turn = envelope.restoredTurn { facts.append(("restored turn", "\(turn)")) }
        if let snapshot = envelope.snapshot { facts.append(("snapshot", snapshot)) }
        if let destination = envelope.destination { facts.append(("destination", destination)) }
        if let ref = envelope.targetRef { facts.append(("target ref", ref)) }
        if let reverted = envelope.revertedRevision { facts.append(("reverted", reverted)) }
        if let undoRevision = envelope.undoRevision { facts.append(("undo revision", undoRevision)) }
        if let steps = envelope.undoneSteps { facts.append(("undone steps", "\(steps)")) }
        if let workspace = envelope.workspace { facts.append(("workspace", workspace)) }
        return facts
    }

    /// One undo per sheet: any terminal phase disarms (the coordinator
    /// enforces it too).
    private var undoEnabled: Bool {
        if case .idle = coordinator.phase { return true }
        return false
    }

    private var dismissTitle: String {
        switch coordinator.phase {
        case .idle, .running: return "Cancel"
        case .done, .refused, .envelopeFree: return "Close"
        }
    }
}
