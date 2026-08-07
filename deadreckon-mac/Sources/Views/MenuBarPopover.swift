import DeadreckonKit
import SwiftUI

/// The menubar popover (design A2, revised by operator decision 4: the
/// popover carries write affordances, not triage only). Guide is inline
/// for eligible runs — eligibility checked lazily per row via
/// `status --json` steerable{} with the envelope's reason when disabled.
/// Stop / Review & Approve / Send back are items that open the app window
/// DIRECTLY onto the respective confirmation sheet: a destructive verb
/// always gets its full evidence surface; the popover never fires one
/// itself.
struct MenuBarPopover: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var router: WriteSurfaceRouter
    /// Deep-links: Open (and a running row tap) open the main window
    /// directly onto the run's view via `.openJob`, whose
    /// pending-while-loading resolution MainWindowView already owns.
    @ObservedObject var shell: ShellModel
    let openMainWindow: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            headerLine
            Divider().overlay(Theme.border)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    sections
                }
                .padding(8)
            }
            .frame(maxHeight: 380)
            Divider().overlay(Theme.border)
            footer
        }
        .frame(width: 360)
        .background(Theme.sidebarBg)
        // A just-summoned popover must not ride the 10 s menubar-only
        // cadence into the ten-second triage budget: refresh immediately.
        // FleetStore's in-flight/queued coalescing makes this safe against
        // poll overlap, mirroring showMainWindow's immediate refresh.
        .task { await store.refreshNow() }
        // .task alone re-fires only if SwiftUI unmounts the
        // MenuBarExtra(.window) content between opens — a lifecycle that
        // has been version-fragile historically (onAppear/task firing only
        // on the first open on some macOS builds). This bridge observes the
        // hosting panel's own show/key notifications (the
        // WindowVisibilityObserver pattern from RunDetailView), so every
        // open refreshes regardless; the coalescing absorbs the overlap
        // when both paths fire.
        .background(PopoverOpenObserver {
            Task { await store.refreshNow() }
        })
    }

    private var headerLine: some View {
        HStack {
            Text(statusLine)
                .font(Theme.body(11, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var statusLine: String {
        switch store.fleet {
        case .loading: return "reading your runs\u{2026}"
        case .unavailable: return "can\u{2019}t read your runs"
        case .loaded(let queue): return Lexicon.summaryLine(queue)
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
            sectionHeader("NEEDS YOU")
            // One accent primary per surface (DESIGN.md §5): only the top
            // decision's Review & Approve carries the accent; the rows
            // beneath keep standard chrome.
            let shown = Array(decisions.prefix(4))
            ForEach(shown) { item in
                if let row = item.row {
                    PopoverDecisionRow(item: item, row: row, router: router,
                                       shell: shell, openMainWindow: openMainWindow,
                                       isPrimary: item.id == shown.first?.id)
                }
            }
        }
        if !underway.isEmpty {
            sectionHeader("RUNNING")
            ForEach(underway.prefix(5)) { item in
                if let row = item.row {
                    PopoverUnderwayRow(item: item, row: row, router: router,
                                       shell: shell, openMainWindow: openMainWindow)
                }
            }
        }
        if decisions.isEmpty && underway.isEmpty {
            // Degraded-state honesty: the empty-state words key off the
            // FLEET state, not the derived queue — when the fleet is
            // unavailable or still loading, its queue is empty for reasons
            // that are anything but quiet, and this is the app's primary
            // trust surface.
            emptyStateLine
        }
    }

    @ViewBuilder private var emptyStateLine: some View {
        switch store.fleet {
        case .loading:
            Text("reading your runs\u{2026}")
                .font(Theme.small)
                .foregroundStyle(Theme.textTertiary)
                .padding(8)
        case .unavailable(let reason):
            Text("can\u{2019}t read your runs \u{2014} \(reason)")
                .font(Theme.small)
                .foregroundStyle(Theme.warn)
                .lineLimit(3)
                .padding(8)
        case .loaded:
            Text(store.queue.isEmpty ? "all quiet" : "nothing needs you right now")
                .font(Theme.small)
                .foregroundStyle(Theme.textTertiary)
                .padding(8)
        }
    }

    private func sectionHeader(_ title: String) -> some View {
        Theme.sectionTitle(title)
            .padding(.horizontal, 6)
            .padding(.top, 6)
    }

    /// The footer (design A2): the service fact from the health poll, then
    /// Open deadreckon / New Goal / Quit. The mock's "spend today" line is
    /// deliberately dropped: per-day spend is not derivable from the rollup
    /// heads and would need spend tails across every run (documented in
    /// CONTRACTS.md).
    private var footer: some View {
        VStack(alignment: .leading, spacing: 0) {
            supervisorLine
            Divider().overlay(Theme.border)
            HStack(spacing: 10) {
                Button("Open deadreckon") { openMainWindow() }
                    .buttonStyle(.themeStandard(compact: true))
                    .keyboardShortcut("o")
                Button("New Goal") {
                    openMainWindow()
                    router.pending = .newGoal
                }
                .buttonStyle(.themeStandard(compact: true))
                .keyboardShortcut("n")
                Spacer()
                Button("Quit") { NSApp.terminate(nil) }
                    .buttonStyle(.themeStandard(compact: true))
                    .keyboardShortcut("q")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }

    private var supervisorLine: some View {
        HStack(spacing: 5) {
            StateDot(color: supervisorColor)
            Text(supervisorText)
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
            Spacer()
            if let refreshed = store.lastRefreshed {
                Text("updated \(Lexicon.relativeTime(refreshed))")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    private var supervisorText: String {
        Lexicon.serviceWord(store.harbor.supervisor)
    }

    private var supervisorColor: Color {
        switch store.harbor.supervisor {
        case .running: return Theme.success
        case .stopped: return Theme.warn
        case .unknown: return Theme.textTertiary
        }
    }
}

/// A decision row: the fact chips plus the write items. Each destructive
/// verb opens the window onto its confirmation sheet (full evidence surface).
private struct PopoverDecisionRow: View {
    let item: QueueItem
    let row: FleetRow
    @ObservedObject var router: WriteSurfaceRouter
    @ObservedObject var shell: ShellModel
    let openMainWindow: () -> Void
    /// True only for the top decision row — the popover's one accent
    /// primary (DESIGN.md §5/§6).
    var isPrimary = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                RunStateDot(item: item)
                if let provider = row.provider {
                    ProviderIcon(provider: provider, size: 14)
                        .help(provider)
                }
                Text(row.goal)
                    .font(Theme.body(11.5, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Spacer()
                if row.receipt?.verified == .valid {
                    StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                        .help(Lexicon.verifiedHelp)
                }
            }
            // The same row anatomy as the sidebar (§B3): the plain state
            // word under the goal.
            Text(Lexicon.rowStateWord(item))
                .font(Theme.caption)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            HStack(spacing: 8) {
                if item.section == .atTheGate {
                    Button("Review & Approve\u{2026}") {
                        openMainWindow()
                        router.pending = .promote(row)
                    }
                    .buttonStyle(ThemeButtonStyle(kind: isPrimary ? .primary : .standard,
                                                  compact: true))
                    .help("Opens the full review sheet \u{2014} the popover never approves directly.")
                }
                if row.projection.phase == .terminal {
                    Button("Send back\u{2026}") {
                        openMainWindow()
                        router.pending = .sendBack(row)
                    }
                    .buttonStyle(.themeStandard(compact: true))
                } else {
                    Button("Stop\u{2026}") {
                        openMainWindow()
                        router.pending = .kill(row)
                    }
                    .buttonStyle(ThemeButtonStyle(kind: .quietDanger, compact: true))
                    .help("Opens the stop confirmation \u{2014} the popover never stops a run directly.")
                }
                Button("Open") {
                    // Deep-link onto THIS run's view, not whatever the
                    // NavigationStack last showed (design A2's per-row
                    // Open, the exemplar's two-click inspect path).
                    openMainWindow()
                    shell.request = .openJob(row.jobID)
                }
                .buttonStyle(.themeStandard(compact: true))
                Spacer()
            }
        }
        .padding(8)
        .cardChrome()
    }
}

/// A running row: live facts plus inline guide (decision 4). The guide
/// field appears on expand; eligibility is fetched lazily from
/// `status --json` steerable{}, and a later verb refusal stays authoritative
/// over that display (trust rule 4).
private struct PopoverUnderwayRow: View {
    let item: QueueItem
    let row: FleetRow
    @ObservedObject var router: WriteSurfaceRouter
    @ObservedObject var shell: ShellModel
    let openMainWindow: () -> Void

    @StateObject private var quickSteer: QuickSteerController
    @State private var expanded = false
    @State private var note = ""

    init(item: QueueItem, row: FleetRow, router: WriteSurfaceRouter,
         shell: ShellModel, openMainWindow: @escaping () -> Void) {
        self.item = item
        self.row = row
        self.router = router
        self.shell = shell
        self.openMainWindow = openMainWindow
        _quickSteer = StateObject(
            wrappedValue: QuickSteerController(targetID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            // The row tap IS the open path (design A2): the window opens
            // directly onto this run's view.
            Button {
                inspect()
            } label: {
                HStack(spacing: 6) {
                    RunStateDot(item: item)
                    if let provider = row.provider {
                        ProviderIcon(provider: provider, size: 14)
                            .help(provider)
                    }
                    Text(row.goal)
                        .font(Theme.body(11.5, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Spacer()
                    Text(Lexicon.rowStateWord(item))
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.tactile)
            .help("Open this run")
            HStack(spacing: 8) {
                if let spend = row.spend {
                    Text(Lexicon.spendLine(spend))
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textSecondary)
                        .monospacedDigit()
                }
                Button(expanded ? "close" : "Guide\u{2026}") {
                    expanded.toggle()
                    if expanded, quickSteer.eligibility == .unknown {
                        Task { await quickSteer.checkEligibility() }
                    }
                }
                .buttonStyle(.themeStandard(compact: true))
                Button("Stop\u{2026}") {
                    openMainWindow()
                    router.pending = .kill(row)
                }
                .buttonStyle(ThemeButtonStyle(kind: .quietDanger, compact: true))
                .help("Opens the stop confirmation \u{2014} the popover never stops a run directly.")
                Button("Open") { inspect() }
                    .buttonStyle(.themeStandard(compact: true))
                Spacer()
            }
            if expanded {
                quickSteerBody
            }
        }
        .padding(8)
        .cardChrome()
    }

    private func inspect() {
        openMainWindow()
        shell.request = .openJob(row.jobID)
    }

    @ViewBuilder private var quickSteerBody: some View {
        switch quickSteer.eligibility {
        case .unknown, .checking:
            HStack(spacing: 5) {
                ProgressView().controlSize(.mini)
                Text("checking if the run can take guidance\u{2026}")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
        case .ineligible(let reason):
            Text("guide unavailable \u{2014} \(Lexicon.guideReason(reason))")
                .font(Theme.caption)
                .foregroundStyle(Theme.textTertiary)
        case .unavailable(let words):
            Text("can\u{2019}t tell yet \u{2014} \(words)")
                .font(Theme.caption)
                .foregroundStyle(Theme.warn)
                .lineLimit(2)
        case .eligible:
            steerField
        }
        trackerLine
    }

    private var steerField: some View {
        HStack(spacing: 6) {
            TextField("Guide the agent\u{2026}", text: $note)
                .textFieldStyle(.plain)
                .font(Theme.small)
                .padding(.horizontal, 7)
                .padding(.vertical, 4)
                .inputChrome()
                .onSubmit { submit() }
            Button("Send") { submit() }
                .buttonStyle(.themeStandard(compact: true))
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
            Text("queued #\(facts.inboxSeq) \u{2014} delivers at the next turn; status shows in the run view")
                .font(Theme.caption)
                .foregroundStyle(Theme.textSecondary)
        case .delivered(let facts, let turn):
            Text("guidance delivered #\(facts.inboxSeq)\(turn.map { " on turn \($0)" } ?? "")")
                .font(Theme.caption)
                .foregroundStyle(Theme.success)
        case .refused(let refusal):
            // The refusal downgrades the control (trust rule 4).
            Text("refused: \(refusal.message)")
                .font(Theme.caption)
                .foregroundStyle(Theme.dangerText)
                .lineLimit(3)
                .textSelection(.enabled)
        case .envelopeFree(let words):
            Text(words)
                .font(Theme.caption)
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

/// Bridges the MenuBarExtra(.window) panel's own lifecycle into SwiftUI:
/// fires `onOpen` when the hosting panel becomes key or de-occludes (each
/// popover summon), independent of whether SwiftUI unmounts the content
/// between opens. Same NSViewRepresentable pattern as RunDetailView's
/// WindowVisibilityObserver.
private struct PopoverOpenObserver: NSViewRepresentable {
    let onOpen: () -> Void

    func makeNSView(context: Context) -> TrackingView {
        let view = TrackingView()
        view.onOpen = onOpen
        return view
    }

    func updateNSView(_ view: TrackingView, context: Context) {
        view.onOpen = onOpen
    }

    final class TrackingView: NSView {
        var onOpen: (() -> Void)?
        private var observers: [NSObjectProtocol] = []

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            removeObservers()
            guard let window else { return }
            let center = NotificationCenter.default
            observers.append(center.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main
            ) { [weak self] _ in
                self?.onOpen?()
            })
            // The panel may show without becoming key; occlusion flipping
            // to visible covers that path. Guarded so hide events fire
            // nothing.
            observers.append(center.addObserver(
                forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main
            ) { [weak self] _ in
                guard let self, let window = self.window,
                      window.occlusionState.contains(.visible) else { return }
                self.onOpen?()
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
