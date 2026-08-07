import DeadreckonKit
import SwiftUI

/// APP-1 placeholder proving the shell compiles and the window flip works.
/// The glossary line below renders a real DeadreckonKit symbol so the app
/// target compiles against the Kit's public API, not just its build graph.
/// APP-2 replaces this with the real home surface (Gate Queue, Harbor, and
/// the command palette). No real screens live here by design.
struct PhasePlanView: View {
    private struct Phase: Identifiable {
        let id: String
        let scope: String
        let done: Bool
    }

    private let phases: [Phase] = [
        Phase(id: "APP-1", scope: "Scaffold: project, DeadreckonKit, CONTRACTS.md, vendor script", done: true),
        Phase(id: "APP-2", scope: "Menubar shell, Gate Queue home, Harbor, command palette", done: false),
        Phase(id: "APP-3", scope: "Chartroom workbench: narrative, spine, turns, evidence rail, drawer", done: false),
        Phase(id: "APP-4", scope: "Write parity: Lay Course, steer, kill, Binnacle promote, send-back", done: false),
        Phase(id: "APP-5", scope: "User notifications, popover, polish", done: false),
        Phase(id: "VALIDATE", scope: "Build and tests green; UX benchmark against specstory-mac", done: false),
        Phase(id: "RELEASE", scope: "Sign and notarize via the deadreckon release-trust pipeline", done: false),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("deadreckon")
                .font(.largeTitle.weight(.semibold))
            Text("Operator console build program (docs/design/MAC-APP-OPERATOR-CONSOLE.md, section 6.2 composite). This placeholder window proves the menubar shell, window flip, and Kit link; real screens land in APP-2.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: 560, alignment: .leading)

            Divider()

            ForEach(phases) { phase in
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Image(systemName: phase.done ? "checkmark.circle.fill" : "circle")
                        .foregroundStyle(phase.done ? Color.green : Color.secondary)
                    Text(phase.id)
                        .font(.system(.body, design: .monospaced).weight(.medium))
                        .frame(width: 90, alignment: .leading)
                    Text(phase.scope)
                        .foregroundStyle(.secondary)
                }
            }

            Spacer()

            Text("DeadreckonKit linked: JobPhase glossary carries \(JobPhase.allCases.count) words (\(JobPhase.allCases.map(\.rawValue).joined(separator: ", "))).")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .frame(maxWidth: 560, alignment: .leading)

            Text("Trust rules hold from the first commit: the app never invokes dr-gate, never signs, offers no override affordances, and never reads gate-keys.")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .frame(maxWidth: 560, alignment: .leading)
        }
        .padding(32)
        .frame(minWidth: 760, minHeight: 480, alignment: .topLeading)
    }
}

#Preview {
    PhasePlanView()
}
