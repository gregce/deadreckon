import DeadreckonKit
import SwiftUI

/// The Chartroom workbench drill-in (design B1/B2 per section 6.2 item 3):
/// fleet sidebar stays live on the left via the existing FleetStore, center
/// pane is the living surface you read, right rail is evidence, bottom is
/// the drawer (P5). All facts come from JobDetailStore; this file is layout.
struct JobDetailView: View {
    @ObservedObject var fleet: FleetStore
    @State private var selection: QueueItem

    init(fleet: FleetStore, item: QueueItem) {
        self.fleet = fleet
        _selection = State(initialValue: item)
    }

    var body: some View {
        HStack(spacing: 0) {
            FleetSidebarView(fleet: fleet, selection: $selection)
                .frame(width: 220)
            Divider().overlay(Theme.hairline)
            workbench
        }
        .background(Theme.paper)
        .navigationTitle(goalText)
    }

    @ViewBuilder private var workbench: some View {
        switch selection.kind {
        case .job(let row):
            // `.id` gives each job its own workbench identity: switching
            // selection tears the previous JobDetailStore down (onDisappear
            // -> close()) and builds a fresh one — the per-selected-job
            // lifecycle; only the SELECTED job holds active tails.
            JobWorkbenchView(row: row, fleet: fleet)
                .id(row.jobID)
        case .quarantined(let inner):
            QuarantinedDetailView(row: inner)
        }
    }

    private var goalText: String {
        switch selection.kind {
        case .job(let row): return row.goal
        case .quarantined(let inner): return inner.goal ?? "unreadable row"
        }
    }
}

/// The live fleet sidebar: the same queue the home renders, compact. Rows
/// derive from the rollup only; selection swaps the workbench in place.
struct FleetSidebarView: View {
    @ObservedObject var fleet: FleetStore
    @Binding var selection: QueueItem

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("FLEET")
                .font(Theme.body(10, weight: .bold))
                .kerning(0.8)
                .foregroundStyle(Theme.inkTertiary)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider().overlay(Theme.hairline)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(fleet.queue.nonEmptySections, id: \.self) { section in
                        Text(section.title)
                            .font(Theme.body(9, weight: .bold))
                            .kerning(0.5)
                            .foregroundStyle(Theme.inkTertiary)
                            .padding(.horizontal, 10)
                            .padding(.top, 8)
                        ForEach(fleet.queue.items(in: section)) { item in
                            sidebarRow(item)
                        }
                    }
                }
                .padding(.vertical, 6)
            }
        }
        .background(Theme.paper)
    }

    private func sidebarRow(_ item: QueueItem) -> some View {
        Button {
            selection = item
        } label: {
            VStack(alignment: .leading, spacing: 1) {
                Text(rowGoal(item))
                    .font(Theme.body(11, weight: item.id == selection.id ? .semibold : .regular))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
                Text(rowWord(item))
                    .font(Theme.body(9.5))
                    .foregroundStyle(item.needsDecision ? Theme.warn : Theme.inkTertiary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                item.id == selection.id ? Theme.accent.opacity(0.12) : .clear,
                in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func rowGoal(_ item: QueueItem) -> String {
        switch item.kind {
        case .job(let row): return row.goal
        case .quarantined(let inner): return inner.goal ?? "unreadable row"
        }
    }

    private func rowWord(_ item: QueueItem) -> String {
        switch item.kind {
        case .job(let row): return GlossaryText.statusWord(row.status)
        case .quarantined: return GlossaryText.unknownState
        }
    }
}

/// One job's workbench: owns the JobDetailStore for exactly this job.
struct JobWorkbenchView: View {
    let row: FleetRow
    @ObservedObject var fleet: FleetStore
    @StateObject private var detail: JobDetailStore
    @State private var drawerShown = false

    init(row: FleetRow, fleet: FleetStore) {
        self.row = row
        self.fleet = fleet
        _detail = StateObject(wrappedValue: JobDetailStore(
            jobID: row.jobID, scope: row.scope, goal: row.goal,
            cli: DeadreckonCLIClient()))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    DetailHeaderView(row: currentRow, detail: detail, fleet: fleet)
                    Divider().overlay(Theme.hairline)
                    SpineBandView(detail: detail)
                    Divider().overlay(Theme.hairline)
                    DetailCenterTabsView(detail: detail)
                    Divider().overlay(Theme.hairline)
                    SteerBarView(row: currentRow, detail: detail)
                }
                .frame(maxWidth: .infinity)
                Divider().overlay(Theme.hairline)
                EvidenceRailView(row: currentRow, detail: detail)
                    .frame(width: 320)
            }
            .frame(maxHeight: .infinity)

            Divider().overlay(Theme.hairline)
            DetailDrawerView(detail: detail, shown: $drawerShown)
        }
        .onAppear { detail.open() }
        .onDisappear { detail.close() }
        // The NavigationStack keeps this view alive across window close
        // (the AppDelegate retains the window), so onDisappear alone cannot
        // bound the tails: a closed window must not keep the 2s tail + CLI
        // cadence running while the app is menubar-only. The hosting
        // window's lifecycle drives the same open/close pair; both are
        // idempotent, and close() resets run resolution so a later open()
        // resumes every feed cleanly.
        .background(WindowVisibilityObserver(
            onWindowVisible: { detail.open() },
            onWindowHidden: { detail.close() }))
    }

    /// The freshest rollup row for this job (the sidebar store keeps
    /// polling); falls back to the row we were opened with.
    private var currentRow: FleetRow {
        fleet.queue.allItems.compactMap(\.row).first(where: { $0.jobID == row.jobID }) ?? row
    }
}

/// Bridges the hosting NSWindow's lifecycle into SwiftUI: willClose fires
/// the hidden callback, didBecomeKey the visible one. Needed because a
/// retained window that closes does NOT remove the SwiftUI hierarchy, so
/// onDisappear never fires for window close.
private struct WindowVisibilityObserver: NSViewRepresentable {
    let onWindowVisible: () -> Void
    let onWindowHidden: () -> Void

    func makeNSView(context: Context) -> TrackingView {
        let view = TrackingView()
        view.onWindowVisible = onWindowVisible
        view.onWindowHidden = onWindowHidden
        return view
    }

    func updateNSView(_ view: TrackingView, context: Context) {
        view.onWindowVisible = onWindowVisible
        view.onWindowHidden = onWindowHidden
    }

    final class TrackingView: NSView {
        var onWindowVisible: (() -> Void)?
        var onWindowHidden: (() -> Void)?
        private var observers: [NSObjectProtocol] = []

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            removeObservers()
            guard let window else { return }
            let center = NotificationCenter.default
            observers.append(center.addObserver(
                forName: NSWindow.willCloseNotification, object: window, queue: .main
            ) { [weak self] _ in
                self?.onWindowHidden?()
            })
            observers.append(center.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main
            ) { [weak self] _ in
                self?.onWindowVisible?()
            })
        }

        private func removeObservers() {
            for observer in observers {
                NotificationCenter.default.removeObserver(observer)
            }
            observers = []
        }

        deinit {
            removeObservers()
        }
    }
}

/// Center-pane header: goal, phase, lease heartbeat with WHY, spend vs cap,
/// wall time (design B1 selected-job header).
struct DetailHeaderView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @ObservedObject var fleet: FleetStore

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(row.goal)
                    .font(Theme.display(17))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(2)
                Spacer()
                chips
            }
            HStack(spacing: 6) {
                Text(row.jobID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.inkTertiary)
                    .textSelection(.enabled)
                if let runID = detail.currentRunID {
                    dot
                    Text("run \(runID)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if let provider = row.provider {
                    dot
                    Text(provider).font(Theme.body(10.5)).foregroundStyle(Theme.inkSecondary)
                }
                if let lease = row.lease {
                    dot
                    LeaseDotView(
                        lease: lease,
                        confirmedStale: fleet.confirmedStaleLeaseJobIDs.contains(row.jobID))
                        .font(Theme.body(10.5))
                }
                dot
                spendText
                if let spendIssue = detail.spendIssue {
                    Text("spend tail stopped")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                        .help(spendIssue)
                }
                if let clock = detail.status?.workClock {
                    dot
                    Text("active \(Self.hours(clock.activeElapsedSeconds))")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                        .help("Work clock: \(Self.hours(clock.remainingSeconds)) remaining, limited by \(clock.limitingBoundary)")
                } else if let state = detail.runState {
                    dot
                    Text("wall \(Self.hours(state.totalWallSeconds))")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkSecondary)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private var dot: some View {
        Text("\u{00B7}").font(Theme.body(10.5)).foregroundStyle(Theme.inkTertiary)
    }

    @ViewBuilder private var chips: some View {
        HStack(spacing: 5) {
            StatusChip(text: GlossaryText.statusWord(row.status), color: Theme.inkSecondary)
            if let gate = row.gate {
                StatusChip(text: GlossaryText.gateCounts(gate), color: Theme.inkSecondary)
                    .help("From the signed acceptance marker, attempt \(gate.attempt)")
            }
            if let receipt = row.receipt {
                // Trust rule 6: VERIFIED only from the shared proof
                // classifier; proof-invalid is its own state (rule 5).
                switch receipt.verified {
                case .valid:
                    StatusChip(text: GlossaryText.verdictVerified, color: Theme.verified, filled: true)
                        .help(GlossaryText.phraseVerifiedByDrGate)
                case .invalid:
                    StatusChip(text: GlossaryText.proofWord(.invalid), color: Theme.danger, filled: true)
                        .help(receipt.error ?? "the signed receipt did not validate")
                case .notApplicable, .unknown:
                    EmptyView()
                }
            }
        }
    }

    @ViewBuilder private var spendText: some View {
        let meter = detail.spendMeter
        if meter.recordCount > 0 {
            let cap = meter.capUSD.map { String(format: "$%.2f", $0) } ?? "no cap"
            Text(String(format: "$%.2f / %@", meter.loopTotalUSD, cap))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.inkSecondary)
                .monospacedDigit()
                .help(String(format: "loop $%.2f \u{00B7} narrator $%.2f (split ledgers, never summed)",
                             meter.loopTotalUSD, meter.narratorTotalUSD))
        } else if let spend = row.spend {
            Text(GlossaryText.spendLine(spend))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.inkSecondary)
                .monospacedDigit()
        }
    }

    static func hours(_ seconds: Double) -> String {
        if seconds >= 3600 { return String(format: "%.1fh", seconds / 3600) }
        if seconds >= 60 { return String(format: "%.0fm", seconds / 60) }
        return String(format: "%.0fs", seconds)
    }
}

/// The 5-question SPINE band: alive / doing / on track / wrong / next,
/// recomputed from the same durable inputs the TUI uses (spine.rs parity).
struct SpineBandView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        Group {
            if let spine = detail.spine {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 14) { cells(spine) }
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 14) {
                            cell("alive", spine.aliveText, color: aliveColor(spine))
                            cell("doing", spine.doing)
                        }
                        HStack(spacing: 14) {
                            cell("on track", spine.onTrackText)
                            cell("wrong", spine.wrongText,
                                 color: spine.wrong.isEmpty ? Theme.inkSecondary : Theme.warn)
                            cell("next", spine.next, mono: true)
                        }
                    }
                }
            } else {
                Text("spine pending: no state.json for this job's attempt yet")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.inkTertiary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 7)
        .background(Theme.card)
    }

    @ViewBuilder private func cells(_ spine: DeadreckonKit.SpineSnapshot) -> some View {
        cell("alive", spine.aliveText, color: aliveColor(spine))
        cell("doing", spine.doing)
        cell("on track", spine.onTrackText)
        cell("wrong", spine.wrongText,
             color: spine.wrong.isEmpty ? Theme.inkSecondary : Theme.warn)
        cell("next", spine.next, mono: true)
    }

    private func cell(_ label: String, _ value: String,
                      color: Color = Theme.inkSecondary, mono: Bool = false) -> some View {
        HStack(spacing: 4) {
            Text(label)
                .font(Theme.body(9.5, weight: .bold))
                .foregroundStyle(Theme.inkTertiary)
            Text(value)
                .font(mono ? Theme.mono(10) : Theme.body(10.5))
                .foregroundStyle(color)
                .lineLimit(1)
        }
    }

    private func aliveColor(_ spine: DeadreckonKit.SpineSnapshot) -> Color {
        switch spine.alive {
        case .live: return Theme.verified
        case .stale: return Theme.warn
        case .dead: return Theme.danger
        case .done: return Theme.inkSecondary
        }
    }
}

/// The Rudder (P6) and Kill as HONEST placeholders: eligibility comes from
/// `status --json steerable{}` (G6), but the verbs are APP-4's — submit and
/// Kill are disabled and say so. 'Open in Terminal' IS live now.
struct SteerBarView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @State private var draft = ""
    @State private var copiedNote = false

    var body: some View {
        HStack(spacing: 8) {
            Text("\u{27E9} RUDDER")
                .font(Theme.body(9.5, weight: .bold))
                .foregroundStyle(Theme.inkTertiary)

            TextField(steerPrompt, text: $draft)
                .textFieldStyle(.plain)
                .font(Theme.body(11.5))
                .disabled(true)
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
                .background(Theme.paper, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1))

            Button("Steer") {}
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .medium))
                .disabled(true)
                .help("Steer submit arrives with APP-4. \(steerReason)")

            Button("Kill\u{2026}") {}
                .buttonStyle(.tactile)
                .font(Theme.body(11, weight: .medium))
                .foregroundStyle(Theme.inkTertiary)
                .disabled(true)
                .help("Kill arrives with APP-4. Real mechanics: CancelRequested, SIGTERM to the process group, 2s grace, SIGKILL, supervisor-proven Cancelled.")

            Button {
                // Async: the AppleScript roundtrip (and its first-run TCC
                // Automation prompt) must not block the main thread.
                let command = "deadreckon attach \(row.jobID)"
                Task {
                    let result = await TerminalLauncher.launch(command: command)
                    copiedNote = (result == .copiedToPasteboard)
                }
            } label: {
                Label("Open in Terminal", systemImage: "terminal")
                    .font(Theme.body(11, weight: .medium))
            }
            .buttonStyle(.tactile)
            .help("deadreckon attach \(row.jobID)")

            if copiedNote {
                Text("Automation denied \u{2014} command copied to clipboard")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.warn)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    /// The eligibility fact, in the binary's vocabulary. A refusal from the
    /// verb stays authoritative over this display (trust rule 4).
    private var steerPrompt: String {
        guard let eligibility = detail.status?.currentSteerable else {
            return "steer: no run attempt yet \u{00B7} submit arrives with APP-4"
        }
        if eligibility.steerable {
            return "steerable \u{00B7} submit arrives with APP-4"
        }
        return "steer unavailable \u{2014} \(reasonWords(eligibility.reason)) \u{00B7} APP-4"
    }

    private var steerReason: String {
        guard let eligibility = detail.status?.currentSteerable else {
            return "No run attempt yet."
        }
        return eligibility.steerable
            ? "The run reports steerable right now."
            : "Not steerable: \(reasonWords(eligibility.reason))."
    }

    private func reasonWords(_ reason: SteerIneligibleReason?) -> String {
        switch reason {
        case .driverFenced: return "driver fenced"
        case .notExecuting: return "not executing"
        case .providerNotSteerable: return "provider not steerable"
        case .unknown, nil: return GlossaryText.unknownState
        }
    }
}

/// A quarantined row has no durable facts to drill into: say so, verbatim.
struct QuarantinedDetailView: View {
    let row: QuarantinedRow

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(row.goal ?? "unreadable row")
                .font(Theme.display(18))
                .foregroundStyle(Theme.ink)
            Text(row.jobID ?? "unknown id")
                .font(Theme.mono(11))
                .foregroundStyle(Theme.inkTertiary)
            Text("This row failed to decode from `list --json`; its facts cannot be trusted enough to open a workbench. The decoder said:")
                .font(Theme.body(12))
                .foregroundStyle(Theme.inkSecondary)
            Text(row.reason)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.warn)
                .textSelection(.enabled)
                .padding(12)
                .cardChrome()
            Spacer()
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
