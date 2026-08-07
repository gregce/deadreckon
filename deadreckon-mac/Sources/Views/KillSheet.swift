import DeadreckonKit
import SwiftUI

/// The honest kill confirmation (design 2.4.5): states the real semantics
/// verbatim, offers escalation as a separate explicit option, dispatches
/// `kill <id> --json`, renders amber "cancel requested" ONLY from the
/// envelope acceptance, and resolves ONLY on the terminal event in
/// job-events.jsonl — never on the exit code (KillProgress encodes this;
/// the tests pin it).
struct KillSheet: View {
    let row: FleetRow
    @Environment(\.dismiss) private var dismiss
    @StateObject private var coordinator: KillCoordinator

    init(row: FleetRow) {
        self.row = row
        _coordinator = StateObject(
            wrappedValue: KillCoordinator(jobID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Kill \(row.jobID)")
                    .font(Theme.display(18))
                    .foregroundStyle(Theme.ink)
                Text(row.goal)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.inkSecondary)
                    .lineLimit(2)
            }

            // The real mechanics, verbatim — no euphemism (design 1.2).
            VStack(alignment: .leading, spacing: 4) {
                mechanicsLine("1", "CancelRequested is appended to job-events.jsonl (sticky) and cancel.marker is written.")
                mechanicsLine("2", "SIGTERM is sent to the supervised process groups.")
                mechanicsLine("3", "2 seconds of grace, then SIGKILL.")
                mechanicsLine("4", "The supervisor writes the terminal Cancelled event only after proven cleanup.")
                Text("This sheet resolves only when that terminal event lands in job-events.jsonl \u{2014} never on an exit code.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
                    .padding(.top, 2)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .cardChrome()

            Toggle(isOn: $coordinator.escalate) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Escalate subprocess termination (--escalate)")
                        .font(Theme.body(11.5, weight: .medium))
                        .foregroundStyle(Theme.ink)
                    Text("a separate, explicit choice \u{2014} not the default")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
                }
            }
            .toggleStyle(.checkbox)
            .disabled(dispatched)

            CommandLineView(command: coordinator.commandLine)

            progressView

            HStack {
                Spacer()
                Button("Close") {
                    coordinator.stop()
                    dismiss()
                }
                .buttonStyle(.tactile)
                .keyboardShortcut(.cancelAction)
                if !dispatched {
                    Button {
                        Task { await coordinator.dispatch() }
                    } label: {
                        Text("Kill")
                            .font(Theme.body(12, weight: .semibold))
                            .foregroundStyle(Theme.onFill)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 6)
                            .background(Theme.danger, in: Capsule())
                    }
                    .buttonStyle(.tactile)
                }
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.paper)
        .onDisappear { coordinator.stop() }
    }

    private var dispatched: Bool {
        if case .idle = coordinator.progress.phase { return false }
        return true
    }

    @ViewBuilder private var progressView: some View {
        switch coordinator.progress.phase {
        case .idle:
            EmptyView()
        case .dispatching:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("dispatching kill\u{2026}")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkSecondary)
            }
        case .cancelRequested(let facts):
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    StatusChip(text: "cancel requested", color: Theme.warn, filled: true)
                    Text("signal \(facts.signal)\(facts.escalated ? " (escalated)" : "")"
                        + (facts.processesSignalled.map { " \u{00B7} \($0) processes" } ?? ""))
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                }
                if !coordinator.progress.cascadeEnvelopes.isEmpty {
                    ForEach(Array(coordinator.progress.cascadeEnvelopes.enumerated()), id: \.offset) { _, sub in
                        Text("killed \(sub.id ?? "?")"
                            + (sub.kill?.processesSignalled.map { " \u{00B7} \($0) processes" } ?? ""))
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.inkSecondary)
                    }
                }
                Text("waiting for the supervisor's terminal event in job-events.jsonl\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
            }
        case .refused(let refusal):
            RefusalView(refusal: refusal)
        case .envelopeFree(let exitCode, let words):
            VStack(alignment: .leading, spacing: 3) {
                Text("no machine envelope landed (exit \(exitCode)); the binary said:")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.warn)
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.ink)
                    .textSelection(.enabled)
            }
        case .resolutionUnavailable(let reason):
            VStack(alignment: .leading, spacing: 3) {
                Text("resolution unavailable \u{2014} \(reason)")
                    .font(Theme.body(10.5, weight: .medium))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Text("the ledger this sheet resolves on cannot be trusted; check `deadreckon status \(row.jobID)` instead")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
        case .terminal(let kind):
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle")
                    .foregroundStyle(Theme.verified)
                // Glossary user words only (design B1) — never the raw
                // enum name.
                Text("terminal \u{00B7} \(GlossaryText.jobEventWord(kind)) (from job-events.jsonl)")
                    .font(Theme.body(11, weight: .medium))
                    .foregroundStyle(Theme.ink)
            }
        }
    }

    private func mechanicsLine(_ number: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(number)
                .font(Theme.mono(10))
                .foregroundStyle(Theme.inkTertiary)
            Text(text)
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
