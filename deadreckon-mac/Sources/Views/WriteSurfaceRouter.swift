import DeadreckonKit
import SwiftUI

/// Routes every write surface to its sheet (APP-4, operator decision 4):
/// the workbench, the queue rows, AND the menubar popover all open the same
/// confirmation sheets through this one object — the popover never fires a
/// destructive verb without its full evidence surface.
@MainActor
final class WriteSurfaceRouter: ObservableObject {
    enum PendingSurface: Identifiable, Equatable {
        case layCourse
        case kill(FleetRow)
        case promote(FleetRow)
        case sendBack(FleetRow)

        var id: String {
            switch self {
            case .layCourse: return "lay-course"
            case .kill(let row): return "kill-\(row.jobID)"
            case .promote(let row): return "promote-\(row.jobID)"
            case .sendBack(let row): return "send-back-\(row.jobID)"
            }
        }
    }

    @Published var pending: PendingSurface?
}

/// Shared verbatim rendering for a typed machine refusal (trust rule 2):
/// message and try lines exactly as the binary said them, selectable, with
/// NO override control. The try lines are the only recovery affordances.
struct RefusalView: View {
    let refusal: ErrorEnvelope

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "xmark.octagon")
                    .foregroundStyle(Theme.danger)
                Text("\(refusal.verb) refused (exit \(refusal.code))")
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.danger)
            }
            Text(refusal.message)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(refusal.tryLines, id: \.self) { line in
                HStack(spacing: 5) {
                    Text("try:")
                        .font(Theme.body(10, weight: .semibold))
                        .foregroundStyle(Theme.inkTertiary)
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
                    in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .strokeBorder(Theme.danger.opacity(0.35), lineWidth: 1))
    }
}

/// The literal CLI line a sheet is about to run (design 2.4.3), displayed
/// verbatim and selectable.
struct CommandLineView: View {
    let command: String

    var body: some View {
        Text(command)
            .font(Theme.mono(10.5))
            .foregroundStyle(Theme.inkSecondary)
            .textSelection(.enabled)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(Theme.hairline, lineWidth: 1))
    }
}
