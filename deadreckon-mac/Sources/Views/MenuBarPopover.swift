import DeadreckonKit
import SwiftUI

/// The menubar popover (design A2, revised by operator decision 4: the
/// popover carries write affordances, not triage only). Quick-steer is
/// inline for steerable jobs — eligibility checked lazily per row via
/// `status --json` steerable{} with the envelope's reason when disabled.
/// Kill / Promote / Send back are items that open the app window DIRECTLY
/// onto the respective confirmation sheet: a destructive verb always gets
/// its full evidence surface; the popover never fires one itself.
struct MenuBarPopover: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var router: WriteSurfaceRouter
    let openMainWindow: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            headerLine
            Divider().overlay(Theme.hairline)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    sections
                }
                .padding(8)
            }
            .frame(maxHeight: 380)
            Divider().overlay(Theme.hairline)
            footer
        }
        .frame(width: 360)
        .background(Theme.paper)
    }

    private var headerLine: some View {
        HStack {
            Text(statusLine)
                .font(Theme.body(11, weight: .medium))
                .foregroundStyle(Theme.inkSecondary)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var statusLine: String {
        switch store.fleet {
        case .loading: return "Reading the fleet"
        case .unavailable: return "Fleet unavailable"
        case .loaded(let queue): return queue.summaryLine
        }
    }

    @ViewBuilder private var sections: some View {
        let queue = store.queue
        let decisions = queue.allItems.filter { $0.needsDecision || $0.section == .atTheGate }
        // Parenthesized on purpose: the needsDecision filter must cover BOTH
        // slices, or a decision-shaped approaching row appears in two
        // sections at once.
        let underway = (queue.items(in: .approaching) + queue.items(in: .underway))
            .filter { !$0.needsDecision }

        if !decisions.isEmpty {
            sectionHeader("NEEDS DECISION")
            ForEach(decisions.prefix(4)) { item in
                if let row = item.row {
                    PopoverDecisionRow(item: item, row: row, router: router,
                                       openMainWindow: openMainWindow)
                }
            }
        }
        if !underway.isEmpty {
            sectionHeader("UNDERWAY")
            ForEach(underway.prefix(5)) { item in
                if let row = item.row {
                    PopoverUnderwayRow(item: item, row: row, router: router,
                                       openMainWindow: openMainWindow)
                }
            }
        }
        if decisions.isEmpty && underway.isEmpty {
            Text(store.queue.isEmpty ? "fleet quiet" : "nothing needs you right now")
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkTertiary)
                .padding(8)
        }
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(Theme.body(9.5, weight: .bold))
            .kerning(0.6)
            .foregroundStyle(Theme.inkTertiary)
            .padding(.horizontal, 6)
            .padding(.top, 6)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Button("Open deadreckon") { openMainWindow() }
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .medium))
                .keyboardShortcut("o")
            Button("Lay Course") {
                openMainWindow()
                router.pending = .layCourse
            }
            .buttonStyle(.tactile)
            .font(Theme.body(11, weight: .medium))
            .keyboardShortcut("n")
            Spacer()
            Button("Quit") { NSApp.terminate(nil) }
                .buttonStyle(.tactile)
                .font(Theme.body(11))
                .keyboardShortcut("q")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

/// A decision row: the fact chips plus the write items. Each destructive
/// verb opens the window onto its confirmation sheet (full evidence surface).
private struct PopoverDecisionRow: View {
    let item: QueueItem
    let row: FleetRow
    @ObservedObject var router: WriteSurfaceRouter
    let openMainWindow: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(row.goal)
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
                Spacer()
                if row.receipt?.verified == .valid {
                    StatusChip(text: GlossaryText.verdictVerified, color: Theme.verified, filled: true)
                } else {
                    StatusChip(text: GlossaryText.statusWord(row.status), color: Theme.warn)
                }
            }
            HStack(spacing: 8) {
                if item.section == .atTheGate {
                    Button("Promote\u{2026}") {
                        openMainWindow()
                        router.pending = .promote(row)
                    }
                    .buttonStyle(.tactile)
                    .font(Theme.body(10.5, weight: .semibold))
                    .foregroundStyle(Theme.verified)
                    .help("Opens the Binnacle sheet — the full evidence surface. The popover never promotes directly.")
                }
                if row.projection.phase == .terminal {
                    Button("Send back\u{2026}") {
                        openMainWindow()
                        router.pending = .sendBack(row)
                    }
                    .buttonStyle(.tactile)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.accent)
                } else {
                    Button("Kill\u{2026}") {
                        openMainWindow()
                        router.pending = .kill(row)
                    }
                    .buttonStyle(.tactile)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.danger)
                    .help("Opens the kill confirmation with the real semantics. The popover never kills directly.")
                }
                Button("Inspect") { openMainWindow() }
                    .buttonStyle(.tactile)
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkSecondary)
                Spacer()
            }
        }
        .padding(8)
        .cardChrome()
    }
}

/// An underway row: live facts plus inline quick-steer (decision 4). The
/// steer field appears on expand; eligibility is fetched lazily from
/// `status --json` steerable{}, and a later verb refusal stays authoritative
/// over that display (trust rule 4).
private struct PopoverUnderwayRow: View {
    let item: QueueItem
    let row: FleetRow
    @ObservedObject var router: WriteSurfaceRouter
    let openMainWindow: () -> Void

    @StateObject private var quickSteer: QuickSteerController
    @State private var expanded = false
    @State private var note = ""

    init(item: QueueItem, row: FleetRow, router: WriteSurfaceRouter,
         openMainWindow: @escaping () -> Void) {
        self.item = item
        self.row = row
        self.router = router
        self.openMainWindow = openMainWindow
        _quickSteer = StateObject(
            wrappedValue: QuickSteerController(targetID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(row.goal)
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
                Spacer()
                StatusChip(text: GlossaryText.phaseWord(row.projection.phase),
                           color: Theme.inkSecondary)
            }
            HStack(spacing: 8) {
                if let spend = row.spend {
                    Text(GlossaryText.spendLine(spend))
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkSecondary)
                        .monospacedDigit()
                }
                Button(expanded ? "close steer" : "Steer\u{2026}") {
                    expanded.toggle()
                    if expanded, quickSteer.eligibility == .unknown {
                        Task { await quickSteer.checkEligibility() }
                    }
                }
                .buttonStyle(.tactile)
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.accent)
                Button("Kill\u{2026}") {
                    openMainWindow()
                    router.pending = .kill(row)
                }
                .buttonStyle(.tactile)
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.danger)
                Spacer()
            }
            if expanded {
                quickSteerBody
            }
        }
        .padding(8)
        .cardChrome()
    }

    @ViewBuilder private var quickSteerBody: some View {
        switch quickSteer.eligibility {
        case .unknown, .checking:
            HStack(spacing: 5) {
                ProgressView().controlSize(.mini)
                Text("checking steerable{} via status --json\u{2026}")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
        case .ineligible(let reason):
            Text("steer unavailable \u{2014} \(reason)")
                .font(Theme.body(10))
                .foregroundStyle(Theme.inkTertiary)
        case .unavailable(let words):
            Text("eligibility unknown \u{2014} \(words)")
                .font(Theme.body(10))
                .foregroundStyle(Theme.warn)
                .lineLimit(2)
        case .eligible:
            steerField
        }
        trackerLine
    }

    private var steerField: some View {
        HStack(spacing: 6) {
            TextField("advisory note for the next turn\u{2026}", text: $note)
                .textFieldStyle(.plain)
                .font(Theme.body(11))
                .padding(.horizontal, 7)
                .padding(.vertical, 4)
                .background(Theme.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1))
                .onSubmit { submit() }
            Button("Steer") { submit() }
                .buttonStyle(.tactile)
                .font(Theme.body(10.5, weight: .semibold))
                .foregroundStyle(Theme.accent)
                .disabled(note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    @ViewBuilder private var trackerLine: some View {
        switch quickSteer.tracker.phase {
        case .idle:
            EmptyView()
        case .submitting:
            ProgressView().controlSize(.mini)
        case .queued(let facts):
            Text("queued #\(facts.inboxSeq) \u{00B7} \(facts.delivery ?? "next turn") \u{00B7} delivery shows in the workbench")
                .font(Theme.body(10))
                .foregroundStyle(Theme.inkSecondary)
        case .delivered(let facts, let turn):
            Text("delivered #\(facts.inboxSeq)\(turn.map { " on turn \($0)" } ?? "")")
                .font(Theme.body(10))
                .foregroundStyle(Theme.verified)
        case .refused(let refusal):
            // The refusal downgrades the control (trust rule 4).
            Text("refused: \(refusal.message)")
                .font(Theme.body(10))
                .foregroundStyle(Theme.danger)
                .lineLimit(3)
                .textSelection(.enabled)
        case .envelopeFree(let words):
            Text(words)
                .font(Theme.body(10))
                .foregroundStyle(Theme.warn)
                .lineLimit(2)
        }
    }

    private func submit() {
        let text = note
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        Task {
            await quickSteer.submit(note: text)
            if case .queued = quickSteer.tracker.phase { note = "" }
        }
    }
}
