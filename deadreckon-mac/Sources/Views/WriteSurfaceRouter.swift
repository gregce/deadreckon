import DeadreckonKit
import SwiftUI

/// Routes every write surface to its sheet (APP-4, operator decision 4):
/// the workbench, the queue rows, AND the menubar popover all open the same
/// confirmation sheets through this one object — the popover never fires a
/// destructive verb without its full evidence surface.
@MainActor
final class WriteSurfaceRouter: ObservableObject {
    enum PendingSurface: Identifiable, Equatable {
        case newGoal
        case kill(FleetRow)
        case promote(FleetRow)
        case sendBack(FleetRow)

        var id: String {
            switch self {
            case .newGoal: return "new-goal"
            case .kill(let row): return "kill-\(row.jobID)"
            case .promote(let row): return "promote-\(row.jobID)"
            case .sendBack(let row): return "send-back-\(row.jobID)"
            }
        }
    }

    @Published var pending: PendingSurface?
}

/// The refusal card (DESIGN.md §5, trust rule 2): title in plain words,
/// the binary's message verbatim in mono, each try line as a `try:` label +
/// mono accent text — exactly as the binary said them, selectable, with NO
/// override control. The try lines are the only recovery affordances.
struct RefusalView: View {
    let refusal: ErrorEnvelope

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(refusal.verb) refused (exit \(refusal.code))")
                .font(Theme.body(13, weight: .semibold))
                .foregroundStyle(Theme.dangerText)
            Text(refusal.message)
                .font(Theme.monoM)
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(refusal.tryLines, id: \.self) { line in
                HStack(spacing: 5) {
                    Text("try:")
                        .font(Theme.body(10, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                    Text(line)
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.accent)
                        .textSelection(.enabled)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.danger.opacity(0.06),
                    in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
            .strokeBorder(Theme.danger.opacity(0.35), lineWidth: 1))
    }
}

/// The command well (DESIGN.md §5): the literal CLI line a surface is about
/// to run, verbatim and selectable. This is the home of CLI truth; UI words
/// never leak into it and its words never get translated.
struct CommandLineView: View {
    let command: String

    var body: some View {
        Text(command)
            .font(Theme.monoL)
            .foregroundStyle(Theme.textSecondary)
            .textSelection(.enabled)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.well,
                        in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))
    }
}
