import AppKit
import DeadreckonKit
import SwiftUI

/// The selected run filling the window's center (REDESIGN-SPEC §D): header
/// with contextual actions, PLAN band, NOW band, DONE MEANS strip, the
/// evidence tabs, the guide bar, and the console drawer — all full-width
/// with seams between every band. Every fact comes from the existing
/// JobDetailStore publishers; this file is layout.
struct RunDetailView: View {
    @ObservedObject var fleet: FleetStore
    let item: QueueItem

    var body: some View {
        Group {
            switch item.kind {
            case .job(let row):
                // `.id` gives each run its own view identity: switching
                // selection tears the previous JobDetailStore down
                // (onDisappear -> close()) and builds a fresh one — the
                // per-selected-run lifecycle; only the SELECTED run holds
                // active tails.
                RunSurfaceView(row: row, fleet: fleet)
                    .id(row.jobID)
            case .quarantined(let inner):
                QuarantinedDetailView(row: inner)
            }
        }
        .background(Theme.windowBg)
    }
}

/// One run's surface: owns the JobDetailStore for exactly this run.
struct RunSurfaceView: View {
    let row: FleetRow
    @ObservedObject var fleet: FleetStore
    @StateObject private var detail: JobDetailStore
    @State private var drawerShown = false
    @State private var tab: DetailCenterTabsView.Tab = .activity

    init(row: FleetRow, fleet: FleetStore) {
        self.row = row
        self.fleet = fleet
        _detail = StateObject(wrappedValue: JobDetailStore(
            jobID: row.jobID, scope: row.scope, goal: row.goal,
            cli: WriteCLI.client))
    }

    var body: some View {
        VStack(spacing: 0) {
            RunHeaderView(row: currentRow, detail: detail, fleet: fleet)
            Divider().overlay(Theme.border)
            PlanBandView(row: currentRow, detail: detail)
            Divider().overlay(Theme.border)
            NowBandView(detail: detail)
            Divider().overlay(Theme.border)
            DoneMeansStrip(row: currentRow, detail: detail) { tab = .checks }
            Divider().overlay(Theme.border)
            DetailCenterTabsView(row: currentRow, detail: detail, tab: $tab)
            // The guide bar is the live run's tool; a terminal run's header
            // carries the review actions instead (§D6).
            if currentRow.projection.phase != .terminal {
                Divider().overlay(Theme.border)
                GuideBar(row: currentRow, detail: detail)
            }
            Divider().overlay(Theme.border)
            DetailDrawerView(detail: detail, shown: $drawerShown)
        }
        .onAppear { detail.open() }
        .onDisappear { detail.close() }
        // The retained window keeps this view alive across window close
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

    /// The freshest rollup row for this run (the fleet store keeps
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

// MARK: - Header (§D1)

/// Goal + contextual actions on row one; the dense fact line on row two.
/// Terminal runs get Review & Approve (primary) + Send back; live runs get
/// Stop (quiet-danger); a decision-shaped waiting run gets nothing extra —
/// the guide bar is the tool (Stop stays reachable in the overflow menu).
struct RunHeaderView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @ObservedObject var fleet: FleetStore
    @EnvironmentObject private var router: WriteSurfaceRouter
    @State private var copiedNote = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(row.goal)
                    .font(Theme.title)
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(2)
                Spacer(minLength: 12)
                actions
                overflowMenu
            }
            factsLine
            if copiedNote {
                Text("Automation denied \u{2014} command copied to clipboard")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.warn)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        // A menu-path terminal launch that degraded to the pasteboard
        // (TCC Automation denied) surfaces its note here — the header is
        // always present, terminal or live.
        .onReceive(NotificationCenter.default.publisher(for: .deadreckonTerminalFallback)) { _ in
            copiedNote = true
        }
    }

    /// Contextual actions (§D1). Destructive verbs always open their full
    /// confirmation sheet.
    @ViewBuilder private var actions: some View {
        if row.projection.phase == .terminal {
            Button("Send back\u{2026}") { router.pending = .sendBack(row) }
                .buttonStyle(.themeStandard(compact: true))
                .help("Start a follow-up run with your note attached")
            Button("Review & Approve\u{2026}") { router.pending = .promote(row) }
                .buttonStyle(ThemeButtonStyle(kind: .primary, compact: true))
                .help("Review the evidence and approve the result")
        } else if !QueueDerivation.isDecisionShaped(row) {
            Button("Stop\u{2026}") { router.pending = .kill(row) }
                .buttonStyle(ThemeButtonStyle(kind: .quietDanger, compact: true))
                .help("Stop this run \u{2014} asks first, then force-quits; full details in the confirm")
        }
    }

    private var overflowMenu: some View {
        Menu {
            Button("Open in Terminal") { openInTerminal() }
            Button("Copy run id") { copyToPasteboard(row.jobID) }
            Button("Copy project path") { copyToPasteboard(row.scope) }
            if row.projection.phase != .terminal, QueueDerivation.isDecisionShaped(row) {
                Divider()
                Button("Stop\u{2026}") { router.pending = .kill(row) }
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 12, weight: .medium))
        }
        .menuStyle(.button)
        .buttonStyle(.themeStandard(compact: true))
        .fixedSize()
        .help("Open in Terminal (\u{2318}T), copy run id or project path")
    }

    private func openInTerminal() {
        // Async: the AppleScript roundtrip (and its first-run TCC
        // Automation prompt) must not block the main thread.
        let command = "deadreckon attach \(row.jobID)"
        Task {
            let result = await TerminalLauncher.launch(command: command)
            copiedNote = (result == .copiedToPasteboard)
        }
    }

    private func copyToPasteboard(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    // MARK: Row 2 — the fact line

    private var factsLine: some View {
        HStack(spacing: 6) {
            // One chip carries the word: when the strong proof chip already
            // says Verified, a second neutral "Verified" beside it is
            // duplication, not information (§6: solve with weight, not
            // repetition).
            if !(stateWord == Lexicon.proofVerified && row.receipt?.verified == .valid) {
                StatusChip(text: stateWord, color: Theme.textSecondary)
            }
            proofChip
            if let provider = row.provider {
                ProviderIcon(provider: provider, size: 13)
                agentText(provider)
            }
            dot
            Text(row.jobID)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.textTertiary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
            if let runID = detail.currentRunID, runID != row.jobID {
                dot
                // The id speaks for itself; no "run" label (§A3.2). Shown
                // only when it differs from the durable job id — for a
                // single-run job the two are the same fact.
                Text(runID)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            dot
            Text(Lexicon.projectName(row.scope))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
                .help(row.scope)
            // The heartbeat is a live-run fact; a finished run's released
            // lease would otherwise read as a warning ("no signal 500s")
            // about a process that is honestly done.
            if let lease = row.lease, row.projection.phase != .terminal {
                dot
                LeaseDotView(
                    lease: lease,
                    confirmedStale: fleet.confirmedStaleLeaseJobIDs.contains(row.jobID))
                    .font(Theme.body(10.5))
            }
            clockText
            dot
            spendText
            if let spendIssue = detail.spendIssue {
                StatusChip(text: "spend feed stopped", color: Theme.warn)
                    .help(spendIssue)
            }
            if let gate = row.gate {
                StatusChip(text: GlossaryText.gateCounts(gate), color: Theme.textSecondary)
                    .help("From the signed record of check results, attempt \(gate.attempt)")
            }
            Spacer(minLength: 0)
        }
        .lineLimit(1)
    }

    private var stateWord: String {
        if row.projection.phase == .waiting, let reason = row.projection.stopReason {
            return "Waiting \u{2014} \(Lexicon.stopReasonWord(reason))"
        }
        return Lexicon.statusWord(row.status)
    }

    @ViewBuilder private var proofChip: some View {
        if let receipt = row.receipt {
            // Trust rule 6: Verified only from the shared proof classifier;
            // proof-invalid is its own state (rule 5).
            switch receipt.verified {
            case .valid:
                StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                    .help(Lexicon.verifiedHelp)
            case .invalid:
                StatusChip(text: Lexicon.proofInvalid, color: Theme.danger, strong: true,
                           textColor: Theme.dangerText)
                    .help(receipt.error ?? "the signed receipt did not validate")
            case .notApplicable, .unknown:
                EmptyView()
            }
        }
    }

    @ViewBuilder private var clockText: some View {
        if let clock = detail.status?.workClock {
            dot
            Text("active \(Lexicon.hours(clock.activeElapsedSeconds))")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
                .monospacedDigit()
                .help("\(Lexicon.hours(clock.remainingSeconds)) left before \(Self.boundaryWords(clock.limitingBoundary)) (\(clock.limitingBoundary))")
        } else if let state = detail.runState {
            dot
            Text("elapsed \(Lexicon.hours(state.totalWallSeconds))")
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
                .monospacedDigit()
        }
    }

    /// Plain words for the work clock's limiting boundary; the raw boundary
    /// word rides beside it in the tooltip.
    static func boundaryWords(_ boundary: String) -> String {
        let lowered = boundary.lowercased()
        if lowered.contains("spend") || lowered.contains("budget") { return "its budget" }
        if lowered.contains("wall") || lowered.contains("deadline") || lowered.contains("time") {
            return "its time limit"
        }
        return boundary
    }

    /// Known agents get plain display names; unknown ids stay verbatim mono.
    @ViewBuilder private func agentText(_ provider: String) -> some View {
        if let name = Lexicon.agentName(provider) {
            Text(name).font(Theme.body(10.5)).foregroundStyle(Theme.textSecondary)
        } else {
            Text(provider).font(Theme.mono(10.5)).foregroundStyle(Theme.textSecondary)
        }
    }

    @ViewBuilder private var spendText: some View {
        let meter = detail.spendMeter
        if meter.recordCount > 0 {
            let cap = meter.capUSD.map { String(format: "$%.2f", $0) } ?? "no budget cap"
            Text(String(format: "$%.2f of %@", meter.loopTotalUSD, cap))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
                .monospacedDigit()
                .help(String(format: "loop $%.2f \u{00B7} narrator $%.2f (split ledgers, never summed)",
                             meter.loopTotalUSD, meter.narratorTotalUSD))
        } else if let spend = row.spend {
            Text(Lexicon.spendLine(spend))
                .font(Theme.body(10.5))
                .foregroundStyle(Theme.textSecondary)
                .monospacedDigit()
        }
    }

    private var dot: some View {
        Text("\u{00B7}").font(Theme.body(10.5)).foregroundStyle(Theme.textTertiary)
    }
}

// MARK: - PLAN band (§D2)

/// The horizontal step strip from the run's own pipeline state (names
/// verbatim), or the five-stage lifecycle fallback derived from
/// `projection.phase` when the run hasn't posted a plan. Step grammar:
/// completed = success check, current = breathing accent (the live
/// marker), planned = quiet, failed = danger x.
struct PlanBandView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Theme.sectionTitle("PLAN")
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 16) {
                    if let state = detail.runState {
                        ForEach(state.phases, id: \.id.raw) { phase in
                            planStep(phase, currentID: state.currentPhaseID)
                        }
                    } else {
                        fallbackSteps
                        Text("The run hasn\u{2019}t posted its plan yet.")
                            .font(Theme.caption)
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
                .padding(.vertical, 2)
            }
            Spacer(minLength: 0)
            if row.projection.attemptCount > 1 {
                Text("attempt \(row.projection.attemptCount)")
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Theme.panel)
    }

    // One real pipeline step, status words from the file verbatim.
    private func planStep(_ phase: RunStateDoc.Phase, currentID: Int) -> some View {
        let current = phase.id.raw == currentID && phase.status == "executing"
        return HStack(spacing: 5) {
            stepGlyph(status: phase.status, current: current)
            Text("\(phase.id.raw)")
                .font(Theme.mono(9.5))
                .foregroundStyle(Theme.textTertiary)
            Text(phase.name)
                .font(Theme.body(11, weight: current ? .semibold : .regular))
                .foregroundStyle(stepNameColor(status: phase.status, current: current))
                .lineLimit(1)
        }
        .help("\(phase.name) \u{00B7} \(phase.status)")
    }

    @ViewBuilder private func stepGlyph(status: String, current: Bool) -> some View {
        switch status {
        case "completed":
            Text("\u{2713}")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(Theme.success)
        case "failed":
            Text("\u{2717}")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(Theme.danger)
        case "executing":
            BreathingDot(color: Theme.accent)
        default:
            StateDot(color: Theme.textTertiary)
        }
    }

    private func stepNameColor(status: String, current: Bool) -> Color {
        if current { return Theme.textPrimary }
        switch status {
        case "completed": return Theme.textSecondary
        case "failed": return Theme.dangerText
        case "executing": return Theme.textPrimary
        default: return Theme.textTertiary
        }
    }

    // The five-stage lifecycle fallback (§D2): Queued → Working → Checks →
    // Judge → Review, positioned from the durable projection phase.
    private static let lifecycleStages = ["Queued", "Working", "Checks", "Judge", "Review"]

    private enum StageMark {
        case done, current, planned, failed, review(JobOutcome?)
    }

    @ViewBuilder private var fallbackSteps: some View {
        let position = lifecyclePosition
        ForEach(Array(Self.lifecycleStages.enumerated()), id: \.offset) { index, name in
            let mark = stageMark(index: index, position: position)
            HStack(spacing: 5) {
                fallbackGlyph(mark)
                Text(name)
                    .font(Theme.body(11, weight: markIsCurrent(mark) ? .semibold : .regular))
                    .foregroundStyle(fallbackNameColor(mark))
            }
        }
    }

    /// The lifecycle index the projection phase maps to; nil when the phase
    /// is unknown (every stage stays quiet — never a guess).
    private var lifecyclePosition: Int? {
        switch row.projection.phase {
        case .queued: return 0
        case .running, .waiting: return 1
        case .verifyingChecks: return 2
        case .verifyingMeaning: return 3
        case .terminal: return 4
        case .unknown: return nil
        }
    }

    private func stageMark(index: Int, position: Int?) -> StageMark {
        guard let position else { return .planned }
        if index < position { return .done }
        if index > position { return .planned }
        if row.projection.phase == .terminal { return .review(row.projection.outcome) }
        return .current
    }

    private func markIsCurrent(_ mark: StageMark) -> Bool {
        if case .current = mark { return true }
        return false
    }

    @ViewBuilder private func fallbackGlyph(_ mark: StageMark) -> some View {
        switch mark {
        case .done:
            Text("\u{2713}").font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.success)
        case .current:
            // The lifecycle strip's current stage only breathes while the
            // run is actually executing (accent = live, DESIGN.md §6).
            if row.projection.phase == .waiting || row.projection.phase == .queued {
                StateDot(color: Theme.textTertiary)
            } else {
                BreathingDot(color: Theme.accent)
            }
        case .planned:
            StateDot(color: Theme.textTertiary)
        case .failed:
            Text("\u{2717}").font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.danger)
        case .review(let outcome):
            // The terminal Review stage says how the run arrived: verified
            // completed it, needs-review waits warm, everything else is over.
            switch outcome {
            case .verified:
                Text("\u{2713}").font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.success)
            case .needsReview:
                StateDot(color: Theme.warn)
            default:
                Text("\u{2717}").font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.danger)
            }
        }
    }

    private func fallbackNameColor(_ mark: StageMark) -> Color {
        switch mark {
        case .done: return Theme.textSecondary
        case .current: return Theme.textPrimary
        case .planned: return Theme.textTertiary
        case .failed: return Theme.dangerText
        case .review(let outcome):
            switch outcome {
            case .verified: return Theme.textSecondary
            case .needsReview: return Theme.textPrimary
            default: return Theme.dangerText
            }
        }
    }
}

/// The breathing 6px dot for the executing step / live marker (DESIGN.md
/// §7); Reduce Motion disables the breathe.
struct BreathingDot: View {
    let color: Color
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dimmed = false

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 6, height: 6)
            .opacity(dimmed ? 0.55 : 1)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                    dimmed = true
                }
            }
            .accessibilityHidden(true)
    }
}

// MARK: - NOW band (§D3)

/// WORKING ON / ON TRACK / ATTENTION / LAST ACTIVITY, recomputed from the
/// same durable inputs the TUI uses (spine.rs parity — values from
/// SpineSnapshot's structured fields, never string-parsed), with the
/// spine's next command as a quiet mono CLI line.
struct NowBandView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        Group {
            if let spine = detail.spine {
                VStack(alignment: .leading, spacing: 6) {
                    ViewThatFits(in: .horizontal) {
                        HStack(alignment: .top, spacing: 28) { cells(spine) }
                        VStack(alignment: .leading, spacing: 8) {
                            HStack(alignment: .top, spacing: 28) {
                                cell("WORKING ON", workingOnText(spine), color: Theme.textPrimary)
                                cell("LAST ACTIVITY", Lexicon.lastActivity(spine.alive),
                                     color: aliveColor(spine))
                            }
                            HStack(alignment: .top, spacing: 28) {
                                cell("ON TRACK", Lexicon.onTrackLine(spine.onTrack))
                                cell("ATTENTION", Lexicon.attentionLine(spine.wrong),
                                     color: spine.wrong.isEmpty ? Theme.textSecondary : Theme.warn)
                            }
                        }
                    }
                    HStack(spacing: 6) {
                        Text("CLI")
                            .font(.system(size: 9.5, weight: .bold))
                            .kerning(0.6)
                            .foregroundStyle(Theme.textTertiary)
                        Text(spine.next)
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(1)
                            .textSelection(.enabled)
                    }
                }
            } else {
                Text("Status pending \u{2014} the run hasn\u{2019}t written its first state file yet")
                    .font(Theme.small)
                    .foregroundStyle(Theme.textTertiary)
                    .help("no state.json for this run's attempt yet")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    @ViewBuilder private func cells(_ spine: DeadreckonKit.SpineSnapshot) -> some View {
        cell("WORKING ON", workingOnText(spine), color: Theme.textPrimary)
        cell("ON TRACK", Lexicon.onTrackLine(spine.onTrack))
        cell("ATTENTION", Lexicon.attentionLine(spine.wrong),
             color: spine.wrong.isEmpty ? Theme.textSecondary : Theme.warn)
        cell("LAST ACTIVITY", Lexicon.lastActivity(spine.alive), color: aliveColor(spine))
    }

    /// A1 spine table: "Working on {phase} · turn N" — built from the run's
    /// own STRUCTURED state (phase name verbatim, turn count), never parsed
    /// out of the spine's display string. Without a state file the spine's
    /// composed line renders verbatim (machine truth over silence).
    private func workingOnText(_ spine: DeadreckonKit.SpineSnapshot) -> String {
        guard let state = detail.runState else { return spine.doing }
        if let phase = state.activePhaseName {
            return "\(phase) \u{00B7} turn \(state.turn)"
        }
        return spine.doing
    }

    private func cell(_ label: String, _ value: String,
                      color: Color = Theme.textSecondary) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Theme.sectionTitle(label)
            Text(value)
                .font(Theme.body(12.5))
                .foregroundStyle(color)
                .monospacedDigit()
                .lineLimit(1)
        }
    }

    private func aliveColor(_ spine: DeadreckonKit.SpineSnapshot) -> Color {
        switch spine.alive {
        case .live: return Theme.success
        case .stale: return Theme.warn
        case .dead: return Theme.dangerText
        case .done: return Theme.textSecondary
        }
    }
}

// MARK: - DONE MEANS strip (§D4)

/// Always visible — the anchor for "what does done mean here?". Criteria
/// one-liner + rollup chips; clicking anywhere lands on the Checks tab.
struct DoneMeansStrip: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    let onOpenChecks: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: onOpenChecks) {
            HStack(spacing: 8) {
                Text("Done means:")
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.textTertiary)
                content
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Theme.textTertiary)
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 40)
            .background(hovering ? Theme.panelHover : Theme.panel)
            .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .onHover { hovering = $0 }
        .accessibilityLabel("What done means \u{2014} opens the Checks tab")
    }

    @ViewBuilder private var content: some View {
        if let contract = detail.report?.contract {
            HStack(spacing: 6) {
                Text(criteriaLine(contract))
                    .font(Theme.base)
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(criteriaLine(contract))
                let rows = contract.checkRows
                if !rows.isEmpty {
                    StatusChip(text: "\(rows.count) check\(rows.count == 1 ? "" : "s")",
                               color: Theme.textSecondary)
                }
                if let gate = row.gate {
                    StatusChip(text: "\(gate.nPassed) passed",
                               color: gate.nPassed == gate.nTotal ? Theme.success : Theme.textSecondary)
                        .help("From the signed record of check results, attempt \(gate.attempt)")
                }
                digestChip(contract)
                networkText
            }
        } else if let issue = detail.reportIssue {
            Text("report unavailable: \(issue)")
                .font(Theme.small)
                .foregroundStyle(Theme.warn)
                .lineLimit(1)
        } else if detail.report != nil {
            Text("No definition of done recorded \u{2014} the CLI\u{2019}s preview would refuse to start this today")
                .font(Theme.small)
                .foregroundStyle(Theme.warn)
                .lineLimit(1)
        } else {
            Text("reading the run\u{2019}s report\u{2026}")
                .font(Theme.small)
                .foregroundStyle(Theme.textTertiary)
                .help("report --json")
        }
    }

    /// The plain criteria one-liner when the frozen spec names one; the
    /// frozen file fact (mono truth) otherwise.
    private func criteriaLine(_ contract: JobReportEnvelope.Contract) -> String {
        if let name = contract.spec?.name, !name.isEmpty { return name }
        return "acceptance.yaml (frozen)"
    }

    @ViewBuilder private func digestChip(_ contract: JobReportEnvelope.Contract) -> some View {
        switch contract.matchesApprovedDigest {
        case true?:
            StatusChip(text: "matches the approved version", color: Theme.success)
                .help("sha256 approved \(contract.approvedSHA256 ?? "")")
        case false?:
            StatusChip(text: "CHANGED SINCE APPROVAL", color: Theme.danger, strong: true,
                       textColor: Theme.dangerText)
                .help("sha256 approved \(contract.approvedSHA256 ?? "-") \u{00B7} current \(contract.currentSHA256 ?? "-")")
        case nil:
            StatusChip(text: "approval status unknown", color: Theme.textTertiary)
        }
    }

    @ViewBuilder private var networkText: some View {
        if let network = detail.status?.job?.job.policy?.execution?.gate?.network {
            Text(network == "deny" ? "network: not allowed" : "network: \(network)")
                .font(Theme.small)
                .foregroundStyle(network == "deny" ? Theme.textSecondary : Theme.warn)
                .help("capability compiled into the run's policy (gate.network)")
        }
    }
}

// MARK: - Guide bar (§D6)

/// The guide bar (live runs only): eligibility comes from `status --json`
/// steerable{} (G6, widened in M1 to any provider while Executing); submit
/// runs `steer <id> "note" --json`; the queued chip renders the envelope's
/// inbox_seq and flips to delivered only when the typed steer_delivered
/// event appears in the events tail. A verb refusal after
/// `steerable: true` is authoritative and downgrades the control (trust
/// rule 4).
struct GuideBar: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @StateObject private var steer: SteerCoordinator
    @State private var draft = ""
    /// APP-5: Run > Guide… (⌘G) focuses this field via the shell
    /// notification.
    @FocusState private var steerFieldFocused: Bool

    init(row: FleetRow, detail: JobDetailStore) {
        self.row = row
        self.detail = detail
        _steer = StateObject(
            wrappedValue: SteerCoordinator(targetID: row.jobID, cli: WriteCLI.client))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Theme.sectionTitle("GUIDE")

                TextField(steerPrompt, text: $draft)
                    .textFieldStyle(.plain)
                    .font(Theme.body(11.5))
                    .focused($steerFieldFocused)
                    .disabled(!steerEnabled)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .inputChrome(focused: steerFieldFocused)
                    .onSubmit { submitSteer() }

                Button("Send") { submitSteer() }
                    .buttonStyle(.themeStandard(compact: true))
                    .disabled(!steerEnabled
                        || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .help(steerEnabled
                        ? steer.commandLine(note: draft.isEmpty ? "<note>" : draft)
                        : "Guide unavailable \u{2014} \(ineligibleReason)")
            }
            steerStateLine
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        // The delivered flip rides the events tail (typed steer_delivered
        // rows surfaced by JobDetailStore), never a timer or a guess.
        .onChange(of: detail.steerDeliveries) { _, deliveries in
            steer.observe(deliveries: deliveries)
        }
        // Run > Guide… (APP-5): focus lands here; the field's steerable{}
        // gating (and any verb refusal after it) stays authoritative.
        .onReceive(NotificationCenter.default.publisher(for: .deadreckonFocusSteer)) { _ in
            steerFieldFocused = true
        }
    }

    /// The queued/delivered/refused line under the bar, from the tracker.
    @ViewBuilder private var steerStateLine: some View {
        switch steer.tracker.phase {
        case .idle:
            EmptyView()
        case .submitting:
            HStack(spacing: 5) {
                ProgressView().controlSize(.mini)
                Text("queueing guidance\u{2026}")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
        case .queued(let facts):
            HStack(spacing: 6) {
                StatusChip(text: "guidance queued #\(facts.inboxSeq)", color: Theme.accent)
                Text(facts.delivery ?? "delivers at the next turn")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.textTertiary)
            }
        case .delivered(let facts, let turn):
            HStack(spacing: 6) {
                StatusChip(text: "guidance delivered #\(facts.inboxSeq)", color: Theme.success)
                if let turn {
                    Text("turn \(turn)")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                }
            }
            .help("from the `steer_delivered` event")
        case .refused(let refusal):
            RefusalView(refusal: refusal)
        case .envelopeFree(let words):
            HStack(spacing: 5) {
                Text("The CLI answered without a machine envelope:")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.warn)
                Text(words)
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textPrimary)
                    .textSelection(.enabled)
            }
        }
    }

    private func submitSteer() {
        let note = draft
        guard steerEnabled,
              !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        Task {
            await steer.submit(note: note)
            // Replay the deliveries already observed by the store: a typed
            // steer_delivered event tailed while the envelope was in flight
            // must still flip the chip (onChange never re-fires for an
            // unchanged array; the tracker also buffers, this is belt and
            // braces at the view seam).
            steer.observe(deliveries: detail.steerDeliveries)
            if case .queued = steer.tracker.phase { draft = "" }
        }
    }

    /// Display eligibility from the status envelope; the tracker's refused
    /// state additionally downgrades the control (trust rule 4).
    private var steerEnabled: Bool {
        if case .refused = steer.tracker.phase { return false }
        return detail.status?.currentSteerable?.steerable == true
    }

    private var ineligibleReason: String {
        guard let eligibility = detail.status?.currentSteerable else {
            return "no run attempt yet"
        }
        return eligibility.steerable
            ? "ready"
            : Lexicon.guideReason(QuickSteerController.reasonWords(eligibility.reason))
    }

    private var steerPrompt: String {
        guard let eligibility = detail.status?.currentSteerable else {
            return "Guide available once the run starts"
        }
        if eligibility.steerable {
            return "Guide the agent \u{2014} read at the next turn\u{2026}"
        }
        return "Guide unavailable \u{2014} \(Lexicon.guideReason(QuickSteerController.reasonWords(eligibility.reason)))"
    }
}

// MARK: - Quarantined + shared atoms

/// A quarantined entry has no durable facts to drill into: say so, verbatim.
struct QuarantinedDetailView: View {
    let row: QuarantinedRow

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(row.goal ?? Lexicon.unreadableEntry)
                .font(Theme.title)
                .foregroundStyle(Theme.textPrimary)
            Text(row.jobID ?? Lexicon.unknownID)
                .font(Theme.monoM)
                .foregroundStyle(Theme.textTertiary)
            Text("This entry could not be read from `list --json`, so its details can\u{2019}t be trusted enough to open. The decoder said:")
                .font(Theme.body(12))
                .foregroundStyle(Theme.textSecondary)
            Text(row.reason)
                .font(Theme.monoM)
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

/// The lease heartbeat dot: green when the rollup says fresh, warn only
/// when staleness is CONFIRMED (persisted across consecutive FleetStore
/// polls; a single stale poll right after Mac sleep/wake would otherwise
/// flash false warn). An unconfirmed stale report still says WHY with the
/// recorded heartbeat age, in neutral ink: honest words, no warn yet,
/// never a green "fresh" lie. Display data only; the binary's
/// `claim_job_lease` remains the only judge of ownership.
struct LeaseDotView: View {
    let lease: FleetRow.Lease
    var confirmedStale = false

    var body: some View {
        HStack(spacing: 4) {
            StateDot(color: dotColor)
            if lease.fresh {
                Text(Lexicon.heartbeatAge(lease.heartbeatAgeSeconds))
                    .foregroundStyle(Theme.textTertiary)
                    .monospacedDigit()
            } else {
                Text(Lexicon.noSignal(lease.heartbeatAgeSeconds))
                    .foregroundStyle(confirmedStale ? Theme.warn : Theme.textSecondary)
                    .monospacedDigit()
            }
        }
        .help("Last heartbeat \(lease.heartbeatAgeSeconds)s ago \u{2014} lease owner \(lease.ownerID), epoch \(lease.epoch)")
    }

    private var dotColor: Color {
        if lease.fresh { return Theme.success }
        return confirmedStale ? Theme.warn : Theme.textTertiary
    }
}
