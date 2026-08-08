import DeadreckonKit
import SwiftUI

/// The zero-selection center (REDESIGN-SPEC §B2): answers "does anything
/// need me?" in ten seconds. Needs-you decision cards, running rows,
/// recently finished, health footer — every fact durable and file-backed,
/// nothing estimated. Loading, unavailable, and fresh-install states render
/// here too (§B4).
struct OverviewView: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var attention: AttentionCenter
    let onOpen: (QueueItem) -> Void
    let onReview: (FleetRow) -> Void
    /// Opens the New Goal sheet; `true` arms the folder chooser (the
    /// fresh-install invitation).
    let onNewGoal: (_ armFolderChooser: Bool) -> Void
    /// Shows the Library browser in the center (§R3).
    let onLibrary: () -> Void
    /// Session-scoped "Set up later" for the first-run panel (§R2): the
    /// panel returns on next launch only while setup is incomplete and the
    /// fleet is still empty.
    @Binding var setupDismissed: Bool

    var body: some View {
        Group {
            switch store.fleet {
            case .loading:
                VStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Reading your runs\u{2026}")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .unavailable(let reason):
                UnavailableView(reason: reason)
            case .loaded(let queue):
                if queue.isEmpty {
                    // The one setup panel (§R2) while setup is incomplete;
                    // the standard empty state otherwise — FirstRunGate
                    // probes and decides.
                    FirstRunGate(onNewGoal: onNewGoal, dismissed: $setupDismissed)
                } else {
                    overview(queue)
                }
            }
        }
        .background(Theme.windowBg)
    }

    // MARK: The loaded Overview

    private func overview(_ queue: GateQueue) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                headline(queue)
                needsYouPanel(queue)
                runningPanel(queue)
                finishedPanel(queue)
                footerLine
            }
            .padding(24)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    private func headline(_ queue: GateQueue) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Right now")
                .font(Theme.display)
                .foregroundStyle(Theme.textPrimary)
            Text(summaryLine(queue))
                .font(Theme.small)
                .foregroundStyle(Theme.textSecondary)
                .monospacedDigit()
        }
    }

    private func summaryLine(_ queue: GateQueue) -> String {
        var line = Lexicon.summaryLine(queue)
        if store.legacyRunCount > 0 {
            line += " \u{00B7} \(Lexicon.olderCLIRuns(store.legacyRunCount))"
        }
        return line
    }

    // MARK: Needs you (decision cards — absent when empty)

    private var needsYouItems: [QueueItem] {
        store.queue.allItems.filter { $0.needsDecision || $0.section == .atTheGate }
    }

    @ViewBuilder private func needsYouPanel(_ queue: GateQueue) -> some View {
        let items = needsYouItems
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Theme.sectionTitle("NEEDS YOU", color: Theme.textSecondary)
                VStack(spacing: 0) {
                    ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                        if index > 0 { Divider().overlay(Theme.border) }
                        DecisionCardRow(item: item, onOpen: onOpen, onReview: onReview)
                    }
                }
                .cardChrome()
            }
        }
    }

    // MARK: Running

    private var runningItems: [QueueItem] {
        (store.queue.items(in: .approaching) + store.queue.items(in: .underway))
            .filter { !$0.needsDecision }
    }

    @ViewBuilder private func runningPanel(_ queue: GateQueue) -> some View {
        let items = runningItems
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Theme.sectionTitle("RUNNING", color: Theme.textSecondary)
                VStack(spacing: 0) {
                    ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                        if index > 0 { Divider().overlay(Theme.border) }
                        RunningRow(item: item, onOpen: onOpen)
                    }
                }
                .cardChrome()
            }
        }
    }

    // MARK: Recently finished

    @ViewBuilder private func finishedPanel(_ queue: GateQueue) -> some View {
        let items = Array(queue.items(in: .wrecked).prefix(5))
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Theme.sectionTitle("RECENTLY FINISHED")
                    Spacer()
                    // §R3: the quiet doorway to the promoted-artifact
                    // inventory.
                    Button("Library \u{2192}") { onLibrary() }
                        .buttonStyle(.themeText(size: 10.5))
                        .help("Approved results, promoted \u{2014} View > Library (\u{2318}L)")
                }
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(items) { item in
                        FinishedRow(item: item, onOpen: onOpen)
                    }
                }
            }
        }
    }

    // MARK: Footer

    private var footerLine: some View {
        let summary = FleetHealth.summarize(store: store, attention: attention)
        return HStack(spacing: 6) {
            StateDot(color: summary.color)
            Text(summary.text)
                .foregroundStyle(Theme.textTertiary)
            if let refreshed = store.lastRefreshed {
                Text("\u{00B7} updated \(Lexicon.relativeTime(refreshed))")
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            }
            Spacer()
        }
        .font(Theme.body(10.5))
        .help(summary.tooltip ?? "")
    }
}

// MARK: - Rows

/// One decision card (§B2): dot, goal + Verified chip, facts line, the
/// contextual verb. Verbs route through the confirmation sheets — nothing
/// fires from the card.
private struct DecisionCardRow: View {
    let item: QueueItem
    let onOpen: (QueueItem) -> Void
    let onReview: (FleetRow) -> Void

    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            RunStateDot(item: item)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(item.row?.goal ?? Lexicon.unreadableEntry)
                        .font(Theme.baseMedium)
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    if item.row?.receipt?.verified == .valid {
                        StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                            .help(Lexicon.verifiedHelp)
                    }
                }
                factsLine
            }
            Spacer(minLength: 12)
            if item.section == .atTheGate, let row = item.row {
                Button("Review & Approve") { onReview(row) }
                    .buttonStyle(.themeStandard(compact: true))
                    .help("Review the evidence and approve the result")
            } else {
                Button("Open") { onOpen(item) }
                    .buttonStyle(.themeStandard(compact: true))
            }
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 56)
        .background(hovering ? Theme.panelHover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { onOpen(item) }
    }

    private var factsLine: some View {
        HStack(spacing: 6) {
            Text(Lexicon.rowStateWord(item))
                .foregroundStyle(Theme.textSecondary)
            if let row = item.row {
                if let provider = row.provider {
                    dot
                    ProviderIcon(provider: provider, size: 12)
                    agentText(provider)
                }
                if let spend = row.spend {
                    dot
                    Text(Lexicon.spendLine(spend))
                        .foregroundStyle(Theme.textSecondary)
                        .monospacedDigit()
                }
                if let gate = row.gate {
                    dot
                    Text(GlossaryText.gateCounts(gate))
                        .foregroundStyle(Theme.textSecondary)
                        .monospacedDigit()
                        .help("From the signed record of check results, attempt \(gate.attempt)")
                }
                dot
                Text(Lexicon.relativeTime(row.updatedAt))
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            }
        }
        .font(Theme.small)
        .lineLimit(1)
    }

    @ViewBuilder private func agentText(_ provider: String) -> some View {
        if let name = Lexicon.agentName(provider) {
            Text(name).foregroundStyle(Theme.textSecondary)
        } else {
            Text(provider).font(Theme.mono(10.5)).foregroundStyle(Theme.textSecondary)
        }
    }

    private var dot: some View {
        Text("\u{00B7}").foregroundStyle(Theme.textTertiary)
    }
}

/// One running row (§B2): breathing dot, goal, plain phase word, spend,
/// quiet Open.
private struct RunningRow: View {
    let item: QueueItem
    let onOpen: (QueueItem) -> Void

    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            RunStateDot(item: item)
            Text(item.row?.goal ?? Lexicon.unreadableEntry)
                .font(Theme.body(12.5, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(1)
            Text(Lexicon.rowStateWord(item))
                .font(Theme.small)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            if let spend = item.row?.spend {
                Text(Lexicon.spendLine(spend))
                    .font(Theme.small)
                    .foregroundStyle(Theme.textSecondary)
                    .monospacedDigit()
            }
            Spacer(minLength: 12)
            Button("Open") { onOpen(item) }
                .buttonStyle(.themeStandard(compact: true))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 40)
        .background(hovering ? Theme.panelHover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { onOpen(item) }
    }
}

/// One recently-finished row: outcome word + goal + time, quiet.
private struct FinishedRow: View {
    let item: QueueItem
    let onOpen: (QueueItem) -> Void

    var body: some View {
        Button {
            onOpen(item)
        } label: {
            HStack(spacing: 8) {
                Text(item.row.map { Lexicon.statusWord($0.status) } ?? Lexicon.unreadableEntry)
                    .font(Theme.small)
                    .foregroundStyle(Theme.textSecondary)
                Text(item.row?.goal ?? Lexicon.unreadableEntry)
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if let row = item.row {
                    Text(Lexicon.relativeTime(row.updatedAt))
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
            }
            .padding(.horizontal, 2)
            .padding(.vertical, 3)
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
    }
}

// MARK: - Degraded and empty states (§B4)

/// The typed unavailable state: the reason verbatim, no fake rows, and the
/// CLI escape hatch (the CLI remains first-class).
struct UnavailableView: View {
    let reason: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 26, weight: .medium))
                .foregroundStyle(Theme.warn)
            Text("Can\u{2019}t read your runs")
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text(reason)
                .font(Theme.monoM)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 460)
                .textSelection(.enabled)
            HStack(spacing: 5) {
                Text("The CLI still works:")
                    .font(Theme.base)
                    .foregroundStyle(Theme.textSecondary)
                Text("deadreckon list")
                    .font(Theme.monoL)
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}

/// The fresh-install empty state (§B4): teach the surface — never the word
/// "nothing" alone, never fake rows. The primary opens New Goal with the
/// folder chooser armed.
struct EmptyFleetView: View {
    let onChooseProject: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Text("Start your first goal")
                .font(Theme.display)
                .foregroundStyle(Theme.textPrimary)
            Text("Pick a project folder, say what you want done and what done means, and watch the run work. Everything the agent does lands here as evidence.")
                .font(Theme.base)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)

            // Standard chrome: the sidebar's [+ New Goal] is the window's
            // ONE accent action (§B1); a second orange button on the same
            // surface would break the one-accent budget (§6).
            Button("Choose a Project Folder\u{2026}") { onChooseProject() }
                .buttonStyle(.themeStandard)

            VStack(alignment: .leading, spacing: 6) {
                Text("deadreckon start \"your goal\"")
                Text("deadreckon list")
            }
            .font(Theme.monoL)
            .foregroundStyle(Theme.textPrimary)
            .textSelection(.enabled)
            .padding(14)
            .background(Theme.well,
                        in: RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.controlRadius, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))

            Text("The CLI works too \u{2014} runs started there appear here.")
                .font(Theme.caption)
                .foregroundStyle(Theme.textTertiary)
        }
        .frame(maxWidth: 480)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}
