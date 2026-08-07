import DeadreckonKit
import SwiftUI

/// The honest stop confirmation (design 2.4.5): states the real semantics
/// precisely, offers escalation as a separate explicit option, dispatches
/// `kill <id> --json`, renders the "stop requested" chip ONLY from the
/// envelope acceptance, and resolves ONLY on the terminal event in
/// job-events.jsonl — never on the exit code (KillProgress encodes this;
/// the tests pin it).
struct StopSheet: View {
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
                Text("Stop this run?")
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

            // The real mechanics, precise — no euphemism (design 1.2).
            VStack(alignment: .leading, spacing: 4) {
                mechanicsLine("1", "A cancel request is written to the run\u{2019}s ledger (`CancelRequested`, sticky) and `cancel.marker` is written.")
                mechanicsLine("2", "The service sends SIGTERM to the run\u{2019}s process groups.")
                mechanicsLine("3", "2 seconds of grace, then SIGKILL.")
                mechanicsLine("4", "The service records the final Stopped event only after proven cleanup.")
                Text("This sheet finishes only when that final event lands in `job-events.jsonl` \u{2014} never on an exit code.")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.top, 2)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .cardChrome()

            Toggle(isOn: $coordinator.escalate) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Also force-stop child processes (--escalate)")
                        .font(Theme.body(11.5, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    Text("a separate, explicit choice \u{2014} not the default")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            .toggleStyle(.checkbox)
            .disabled(dispatched)

            CommandLineView(command: coordinator.commandLine)

            progressView

            HStack {
                Spacer()
                Button(dismissTitle) {
                    coordinator.stop()
                    dismiss()
                }
                .buttonStyle(.themeStandard)
                .keyboardShortcut(.cancelAction)
                if !dispatched {
                    Button("Stop Run") {
                        Task { await coordinator.dispatch() }
                    }
                    .buttonStyle(.themeDangerConfirm)
                }
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(Theme.windowBg)
        .onDisappear { coordinator.stop() }
    }

    private var dispatched: Bool {
        if case .idle = coordinator.progress.phase { return false }
        return true
    }

    /// Sheet-dismiss word: "Cancel" before dispatch, "Close" after.
    private var dismissTitle: String {
        dispatched ? "Close" : "Cancel"
    }

    @ViewBuilder private var progressView: some View {
        switch coordinator.progress.phase {
        case .idle:
            EmptyView()
        case .dispatching:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("sending stop\u{2026}")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textSecondary)
            }
        case .cancelRequested(let facts):
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    StatusChip(text: "stop requested", color: Theme.warn)
                    Text("signal \(facts.signal)\(facts.escalated ? " (escalated)" : "")"
                        + (facts.processesSignalled.map { " \u{00B7} \($0) processes" } ?? ""))
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .monospacedDigit()
                }
                if !coordinator.progress.cascadeEnvelopes.isEmpty {
                    ForEach(Array(coordinator.progress.cascadeEnvelopes.enumerated()), id: \.offset) { _, sub in
                        Text("stopped \(sub.id ?? "?")"
                            + (sub.kill?.processesSignalled.map { " \u{00B7} \($0) processes" } ?? ""))
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.textSecondary)
                    }
                }
                Text("waiting for the service\u{2019}s final event in the run log\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
            }
        case .refused(let refusal):
            RefusalView(refusal: refusal)
        case .envelopeFree(let exitCode, let words):
            VStack(alignment: .leading, spacing: 3) {
                Text("The CLI answered without a machine envelope (exit \(exitCode)); it said:")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.warn)
                Text(words)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
            }
        case .resolutionUnavailable(let reason):
            VStack(alignment: .leading, spacing: 3) {
                Text("Can\u{2019}t confirm the stop \u{2014} \(reason)")
                    .font(Theme.body(10.5, weight: .medium))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Text("the ledger this sheet relies on can\u{2019}t be trusted; check `deadreckon status \(row.jobID)` instead")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
        case .terminal(let kind):
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle")
                    .foregroundStyle(Theme.success)
                // Plain words; the event's own word stays in the tooltip
                // (machine truth, never translated there).
                Text("Stopped \u{2014} confirmed by the run log")
                    .font(Theme.body(11, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .help("\(GlossaryText.jobEventWord(kind)) \u{00B7} from job-events.jsonl")
            }
        }
    }

    private func mechanicsLine(_ number: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(number)
                .font(Theme.monoS)
                .foregroundStyle(Theme.textTertiary)
            Text(.init(text))
                .font(Theme.small)
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
