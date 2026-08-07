import DeadreckonKit
import SwiftUI

/// The window's home surface: the Quarterdeck Gate Queue (design doc C1)
/// composed per section 6.2 — Bridge's column discipline inside each row,
/// ranked by durable file-backed facts only (operator decision 8). Every
/// word comes from the glossary or the binary's own output; nothing here
/// estimates, predicts, or overrides.
struct GateQueueView: View {
    @ObservedObject var store: FleetStore
    /// APP-4: every write surface (queue rows, workbench, popover) opens its
    /// confirmation sheet through this router.
    @ObservedObject var router: WriteSurfaceRouter
    /// APP-5: pending navigation requests (menu bar, notification actions)
    /// consumed here — this view owns the NavigationStack — and the opened
    /// item published back so menu enablement can follow the live row.
    @ObservedObject var shell: ShellModel
    /// APP-5 fix pass: per-job notify-tail trouble surfaces as a header
    /// chip — a corrupt notify.jsonl silences that job's notifications
    /// (sticky per attempt), and silence needs a visible signal.
    @ObservedObject var attention: AttentionCenter
    @State private var path: [QueueItem] = []
    @State private var paletteShown = false

    var body: some View {
        ZStack {
            NavigationStack(path: $path) {
                ZStack {
                    Theme.paper.ignoresSafeArea()

                    VStack(spacing: 0) {
                        QueueHeaderView(store: store, attention: attention,
                                        onLayCourse: { router.pending = .layCourse },
                                        onPalette: { paletteShown = true })
                        Divider().overlay(Theme.hairline)
                        content
                        Divider().overlay(Theme.hairline)
                        HarborStripView(store: store)
                    }
                }
                .navigationDestination(for: QueueItem.self) { item in
                    JobDetailView(fleet: store, item: item)
                        .environmentObject(router)
                }
            }

            // The window-level overlay layer (the exemplar MainWindowView
            // pattern): the palette renders ABOVE the NavigationStack, so
            // Command-K or View > Search Fleet from the workbench shows a
            // visible palette instead of arming an invisible one under a
            // pushed view. Opening a match lands on that job's workbench.
            if paletteShown {
                CommandPalette(store: store, isShown: $paletteShown) { item in
                    path = [item]
                }
            }
        }
        // Layered Escape at the window level: the palette's own handler
        // consumes it first while shown (its field holds focus); otherwise
        // Escape steps the workbench back toward the queue.
        .onExitCommand {
            if paletteShown {
                paletteShown = false
            } else if !path.isEmpty {
                path.removeLast()
            }
        }
        .sheet(item: $router.pending) { surface in
            switch surface {
            case .layCourse:
                LayCourseSheet()
            case .kill(let row):
                KillSheet(row: row)
            case .promote(let row):
                PromoteSheet(
                    row: row,
                    fleet: store,
                    onSendBack: { router.pending = .sendBack(row) },
                    onKill: { router.pending = .kill(row) })
            case .sendBack(let row):
                SendBackSheet(row: row)
            }
        }
        .background(shortcutButtons)
        // The opened workbench item, published for menu enablement (APP-5).
        .onChange(of: path) { _, newPath in
            shell.openedItem = newPath.last
        }
        // Navigation requests from the menu bar and notification actions.
        // openJob/reviewAtGate stay pending while the fleet is loading and
        // resolve (or drop, honestly) once a loaded queue can answer.
        .onChange(of: shell.request) { _, _ in consumeShellRequest() }
        .onChange(of: store.lastRefreshed) { _, _ in consumeShellRequest() }
        .onAppear { consumeShellRequest() }
    }

    private func consumeShellRequest() {
        guard let request = shell.request else { return }
        switch request {
        case .gateQueue:
            path = []
            shell.request = nil
        case .search:
            paletteShown = true
            shell.request = nil
        case .focusSteer:
            if path.last != nil {
                NotificationCenter.default.post(name: .deadreckonFocusSteer, object: nil)
            }
            shell.request = nil
        case .openJob(let jobID):
            guard case .loaded = store.fleet else { return }  // retry on next poll
            if let item = store.queue.allItems.first(where: { $0.id == jobID }) {
                path = [item]
            }
            shell.request = nil
        case .reviewAtGate(let jobID):
            guard case .loaded = store.fleet else { return }  // retry on next poll
            if let item = store.queue.allItems.first(where: { $0.id == jobID }),
               let row = item.row {
                path = [item]
                // The promote sheet's own gate (PromoteGate, live row) stays
                // authoritative; this only navigates onto it.
                router.pending = .promote(row)
            }
            shell.request = nil
        }
    }

    /// Hidden buttons so the shortcuts work regardless of focus.
    private var shortcutButtons: some View {
        Group {
            Button("") { router.pending = .layCourse }
                .keyboardShortcut("n", modifiers: .command)
            Button("") { paletteShown.toggle() }
                .keyboardShortcut("k", modifiers: .command)
        }
        .opacity(0)
        .frame(width: 0, height: 0)
        .accessibilityHidden(true)
    }

    @ViewBuilder private var content: some View {
        switch store.fleet {
        case .loading:
            VStack(spacing: 10) {
                ProgressView().controlSize(.small)
                Text("Reading the fleet")
                    .font(Theme.body(12))
                    .foregroundStyle(Theme.inkSecondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .unavailable(let reason):
            UnavailableView(reason: reason)
        case .loaded(let queue):
            if queue.isEmpty {
                EmptyFleetView()
            } else {
                queueList(queue)
            }
        }
    }

    private func queueList(_ queue: GateQueue) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 8) {
                ForEach(queue.nonEmptySections, id: \.self) { section in
                    QueueSectionHeader(section: section, count: queue.items(in: section).count)
                        .padding(.top, section == queue.nonEmptySections.first ? 0 : 10)
                    ForEach(queue.items(in: section)) { item in
                        QueueRowView(
                            item: item,
                            leaseStaleConfirmed: store.confirmedStaleLeaseJobIDs.contains(item.id),
                            onOpen: { path.append(item) })
                            .contextMenu { rowContextMenu(item) }
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: Theme.queueWidth)
            .frame(maxWidth: .infinity)
        }
    }

    /// APP-4 row verbs, contextual on durable facts: Promote for gate rows,
    /// Send back for terminal rows, Kill for live ones. Each opens its full
    /// confirmation sheet — no verb fires from the menu itself.
    @ViewBuilder private func rowContextMenu(_ item: QueueItem) -> some View {
        if let row = item.row {
            if item.section == .atTheGate {
                Button("Promote\u{2026}") { router.pending = .promote(row) }
            }
            if row.projection.phase == .terminal {
                Button("Send back + note\u{2026}") { router.pending = .sendBack(row) }
            } else {
                Button("Kill\u{2026}") { router.pending = .kill(row) }
            }
            Button("Inspect") { path.append(item) }
        }
    }
}

// MARK: - Header

/// Fleet counts plus the Harbor chips (P8: counts and states, not prose).
struct QueueHeaderView: View {
    @ObservedObject var store: FleetStore
    @ObservedObject var attention: AttentionCenter
    let onLayCourse: () -> Void
    let onPalette: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Gate Queue")
                    .font(Theme.display(20))
                    .foregroundStyle(Theme.ink)
                Text(summary)
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkSecondary)
                    .monospacedDigit()
            }

            Spacer()

            notifyTailChip
            HarborChips(store: store)

            Button(action: onPalette) {
                Label("Filter", systemImage: "magnifyingglass")
                    .font(Theme.body(11, weight: .medium))
                    .foregroundStyle(Theme.inkSecondary)
            }
            .buttonStyle(.tactile)
            .help("Filter the queue (Command-K)")

            Button(action: onLayCourse) {
                Label("Lay Course", systemImage: "plus")
                    .font(Theme.body(11, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Theme.accent.opacity(0.10), in: Capsule())
            }
            .buttonStyle(.tactile)
            .help("Start a voyage (Command-N): preview before launch, the plan is the decision")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    /// AttentionCenter honest degradation, rendered: a corrupt notify.jsonl
    /// stops that job's notifications (sticky per attempt), so the silence
    /// gets a chip here — count in the chip, per-job verbatim reasons in the
    /// tooltip; the full list also renders in Settings > Notifications.
    @ViewBuilder private var notifyTailChip: some View {
        if !attention.issues.isEmpty {
            let count = attention.issues.count
            StatusChip(text: "notify tail \(count) corrupt", color: Theme.warn)
                .help(attention.issues.sorted(by: { $0.key < $1.key })
                    .map { "\($0.key): \($0.value)" }
                    .joined(separator: "\n"))
        }
    }

    private var summary: String {
        switch store.fleet {
        case .loading:
            return "reading the fleet"
        case .unavailable:
            return "fleet unavailable"
        case .loaded(let queue):
            var line = queue.summaryLine
            if store.legacyRunCount > 0 {
                line += " \u{00B7} \(store.legacyRunCount) legacy \(store.legacyRunCount == 1 ? "run" : "runs")"
            }
            return line
        }
    }
}

/// Quiet chips for the low-frequency Harbor surfaces. Degrades to the word
/// "unknown" with the failure's words in the tooltip, never a guessed count.
struct HarborChips: View {
    @ObservedObject var store: FleetStore

    var body: some View {
        HStack(spacing: 6) {
            if let version = store.binaryVersion {
                StatusChip(text: version, color: Theme.inkTertiary)
            }
            doctorChip
            providersChip
            supervisorChip
        }
    }

    @ViewBuilder private var doctorChip: some View {
        switch store.harbor.doctor {
        case .ok(let warnings) where warnings == 0:
            StatusChip(text: "doctor ok", color: Theme.verified)
        case .ok(let warnings):
            StatusChip(text: "doctor \(warnings) warn", color: Theme.warn)
        case .failed(let count):
            StatusChip(text: "doctor \(count) failed", color: Theme.danger)
        case .unknown(let reason):
            StatusChip(text: "doctor unknown", color: Theme.inkTertiary)
                .help(reason)
        }
    }

    @ViewBuilder private var providersChip: some View {
        switch store.harbor.providers {
        case .counted(let ok, let total):
            StatusChip(
                text: "providers \(ok)/\(total) ok",
                color: ok == total ? Theme.verified : Theme.warn)
        case .unknown(let reason):
            StatusChip(text: "providers unknown", color: Theme.inkTertiary)
                .help(reason)
        }
    }

    @ViewBuilder private var supervisorChip: some View {
        switch store.harbor.supervisor {
        case .running:
            StatusChip(text: "supervisor running", color: Theme.verified)
        case .stopped(let installState):
            StatusChip(text: "supervisor \(stoppedWord(installState))", color: Theme.warn)
        case .unknown(let reason):
            StatusChip(text: "supervisor unknown", color: Theme.inkTertiary)
                .help(reason)
        }
    }

    private func stoppedWord(_ state: SupervisorInstallState) -> String {
        switch state {
        case .notInstalled: return "not installed"
        case .stale: return "stale"
        case .unsupported: return "unsupported"
        case .current: return "stopped"
        case .unknown: return "stopped"
        }
    }
}

// MARK: - Sections

struct QueueSectionHeader: View {
    let section: QueueSection
    let count: Int

    var body: some View {
        HStack(spacing: 8) {
            Text(section.title)
                .font(Theme.body(11, weight: .bold))
                .kerning(0.6)
                .foregroundStyle(headerColor)
            if let subtitle = section.subtitle {
                Text("\u{2014} \(subtitle)")
                    .font(Theme.body(11))
                    .foregroundStyle(Theme.inkTertiary)
            }
            Text("(\(count))")
                .font(Theme.body(11))
                .foregroundStyle(Theme.inkTertiary)
                .monospacedDigit()
        }
        .padding(.horizontal, 2)
    }

    private var headerColor: Color {
        switch section {
        case .atTheGate: return Theme.verified
        case .needsReview: return Theme.warn
        case .approaching: return Theme.accent
        case .underway: return Theme.inkSecondary
        case .wrecked: return Theme.danger
        case .unknown: return Theme.warn
        }
    }
}

// MARK: - Rows

/// One queue row with Bridge's column discipline: goal, short id, provider,
/// spend, lease heartbeat, then the fact chips (phase, gate counts, proof).
struct QueueRowView: View {
    let item: QueueItem
    /// FleetStore's debounced verdict for this row's lease: amber only after
    /// staleness persisted across consecutive polls, never off one raw poll.
    var leaseStaleConfirmed = false
    let onOpen: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            HStack(alignment: .center, spacing: 12) {
                sectionGlyph
                middle
                Spacer(minLength: 12)
                trailing
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
            .cardChrome(hovering: hovering)
            .onHover { hovering = $0 }
        }
        .buttonStyle(.tactileCard)
        .accessibilityLabel(accessibilityText)
    }

    @ViewBuilder private var middle: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(goalText)
                    .font(Theme.body(13, weight: .medium))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
                if item.needsDecision {
                    StatusChip(text: "decision needed", color: Theme.warn)
                }
            }
            contextLine
        }
    }

    private var contextLine: some View {
        HStack(spacing: 5) {
            Text(idText)
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.inkTertiary)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 180, alignment: .leading)
                .fixedSize(horizontal: true, vertical: false)

            if let row = item.row {
                if let provider = row.provider {
                    dotSeparator
                    ProviderIcon(provider: provider, size: 13)
                    Text(provider)
                        .foregroundStyle(Theme.inkSecondary)
                }
                if let spend = row.spend {
                    dotSeparator
                    Text(GlossaryText.spendLine(spend))
                        .foregroundStyle(Theme.inkSecondary)
                        .monospacedDigit()
                }
                if let lease = row.lease {
                    dotSeparator
                    LeaseDotView(lease: lease, confirmedStale: leaseStaleConfirmed)
                }
            } else if case .quarantined(let inner) = item.kind {
                dotSeparator
                Text(inner.reason)
                    .foregroundStyle(Theme.warn)
                    .lineLimit(1)
            }
        }
        .font(Theme.body(11))
    }

    private var dotSeparator: some View {
        Text("\u{00B7}").foregroundStyle(Theme.inkTertiary)
    }

    @ViewBuilder private var trailing: some View {
        HStack(spacing: 8) {
            chips
            VStack(alignment: .trailing, spacing: 3) {
                if let row = item.row {
                    Text(Self.relativeTime(row.updatedAt))
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.inkTertiary)
                        .monospacedDigit()
                }
                Text(actionTitle)
                    .font(Theme.body(10.5, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .opacity(hovering ? 1 : 0.65)
            }
        }
    }

    /// The fact chips. VERIFIED appears only from receipt.verified == valid
    /// (trust rule 6); an invalid proof is its own red state (trust rule 5);
    /// gate counts render only when the signed marker's rollup is present.
    @ViewBuilder private var chips: some View {
        if let row = item.row {
            HStack(spacing: 5) {
                StatusChip(text: phaseText(row), color: phaseColor(row))
                if let gate = row.gate {
                    StatusChip(text: GlossaryText.gateCounts(gate), color: Theme.inkSecondary)
                        .help("From the signed acceptance marker, attempt \(gate.attempt)")
                }
                if let receipt = row.receipt {
                    switch receipt.verified {
                    case .valid:
                        StatusChip(text: GlossaryText.verdictVerified, color: Theme.verifiedFill, filled: true)
                            .help(GlossaryText.phraseVerifiedByDrGate)
                    case .invalid:
                        StatusChip(text: GlossaryText.proofWord(.invalid), color: Theme.danger, filled: true)
                            .help(receipt.error ?? "the signed receipt did not validate")
                    case .notApplicable, .unknown:
                        EmptyView()
                    }
                }
            }
        } else {
            StatusChip(text: GlossaryText.unknownState, color: Theme.warn)
        }
    }

    private func phaseText(_ row: FleetRow) -> String {
        let phase = row.projection.phase
        if phase == .terminal {
            return GlossaryText.statusWord(row.status)
        }
        if phase == .waiting, let reason = row.projection.stopReason {
            return "waiting \u{00B7} \(GlossaryText.stopReasonWord(reason))"
        }
        return GlossaryText.phaseWord(phase)
    }

    private func phaseColor(_ row: FleetRow) -> Color {
        switch item.section {
        case .atTheGate: return Theme.verified
        case .needsReview: return Theme.warn
        case .approaching: return Theme.accent
        case .underway: return item.needsDecision ? Theme.warn : Theme.inkSecondary
        case .wrecked: return Theme.danger
        case .unknown: return Theme.warn
        }
    }

    private var goalText: String {
        switch item.kind {
        case .job(let row): return row.goal
        case .quarantined(let inner): return inner.goal ?? "unreadable row"
        }
    }

    private var idText: String {
        switch item.kind {
        case .job(let row): return row.jobID
        case .quarantined(let inner): return inner.jobID ?? "unknown id"
        }
    }

    /// No ⏎ glyph: the LazyVStack has no selection model or Return
    /// handling, and a key hint that does nothing teaches distrust — only
    /// keys that work get advertised (the exemplar's discipline).
    private var actionTitle: String {
        switch item.section {
        case .atTheGate, .needsReview: return "Review at Gate"
        default: return "Inspect"
        }
    }

    private var sectionGlyph: some View {
        Text(glyphText)
            .font(Theme.body(12, weight: .bold))
            .foregroundStyle(glyphColor)
            .frame(width: 16)
    }

    private var glyphText: String {
        switch item.section {
        case .atTheGate: return "\u{25CF}"      // filled circle
        case .needsReview: return "\u{25CE}"    // bullseye
        case .approaching: return "\u{25D0}"    // half circle
        case .underway: return "\u{25D4}"       // quarter circle
        case .wrecked: return "\u{2717}"        // ballot x
        case .unknown: return "?"
        }
    }

    private var glyphColor: Color {
        switch item.section {
        case .atTheGate: return Theme.verified
        case .needsReview: return Theme.warn
        case .approaching: return Theme.accent
        case .underway: return item.needsDecision ? Theme.warn : Theme.live
        case .wrecked: return Theme.danger
        case .unknown: return Theme.warn
        }
    }

    private var accessibilityText: String {
        switch item.kind {
        case .job(let row):
            return "\(row.goal), \(GlossaryText.statusWord(row.status))"
        case .quarantined(let inner):
            return "unreadable row \(inner.jobID ?? "")"
        }
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    static func relativeTime(_ date: Date) -> String {
        if Date().timeIntervalSince(date) < 90 { return "now" }
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }
}

/// The lease heartbeat dot: green when the rollup says fresh, amber only
/// when staleness is CONFIRMED (persisted across consecutive FleetStore
/// polls; the design doc's debounce caution — a single stale poll right
/// after Mac sleep/wake would otherwise flash false amber and teach the
/// operator to ignore it). An unconfirmed stale report still says WHY with
/// the recorded heartbeat age, in neutral ink: honest words, no amber yet,
/// never a green "fresh" lie. Display data only; the binary's
/// `claim_job_lease` remains the only judge of ownership.
struct LeaseDotView: View {
    let lease: FleetRow.Lease
    var confirmedStale = false

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(dotColor)
                .frame(width: 6, height: 6)
            if lease.fresh {
                Text("hb \(lease.heartbeatAgeSeconds)s")
                    .foregroundStyle(Theme.inkTertiary)
                    .monospacedDigit()
            } else {
                Text(GlossaryText.leaseStaleReason(lease))
                    .foregroundStyle(confirmedStale ? Theme.warn : Theme.inkSecondary)
                    .monospacedDigit()
            }
        }
        .help("Lease owner \(lease.ownerID), epoch \(lease.epoch)")
    }

    private var dotColor: Color {
        if lease.fresh { return Theme.verified }
        return confirmedStale ? Theme.warn : Theme.inkTertiary
    }
}

// MARK: - Degraded and empty states

/// The typed unavailable state: the reason verbatim, no fake rows, and the
/// CLI escape hatch (the TUI remains first-class).
struct UnavailableView: View {
    let reason: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 26))
                .foregroundStyle(Theme.warn)
            Text("The fleet is unavailable")
                .font(Theme.display(16))
                .foregroundStyle(Theme.ink)
            Text(reason)
                .font(Theme.body(12))
                .foregroundStyle(Theme.inkSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 460)
                .textSelection(.enabled)
            Text("The CLI remains authoritative: deadreckon list")
                .font(Theme.mono(11))
                .foregroundStyle(Theme.inkTertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}

/// P7 empty state: a CTA that teaches the CLI start path (Lay Course lands
/// in APP-4), instead of a wizard.
struct EmptyFleetView: View {
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "water.waves")
                .font(.system(size: 30))
                .foregroundStyle(Theme.inkTertiary)
            Text("No jobs in the fleet")
                .font(Theme.display(18))
                .foregroundStyle(Theme.ink)
            Text("Lay a course (Command-N) or start from the CLI; either way the job surfaces here from the durable files and queues for your decision at the gate.")
                .font(Theme.body(12))
                .foregroundStyle(Theme.inkSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)

            VStack(alignment: .leading, spacing: 6) {
                Text("deadreckon start \"your goal\"")
                Text("deadreckon list")
            }
            .font(Theme.mono(12))
            .foregroundStyle(Theme.ink)
            .textSelection(.enabled)
            .padding(14)
            .cardChrome()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}

// MARK: - Harbor strip

/// The quiet bottom strip: refresh recency only (chips live in the header).
struct HarborStripView: View {
    @ObservedObject var store: FleetStore

    var body: some View {
        HStack(spacing: 8) {
            if case .loaded(let queue) = store.fleet {
                Text(queue.summaryLine)
                    .foregroundStyle(Theme.inkTertiary)
            }
            Spacer()
            if let refreshed = store.lastRefreshed {
                Text("updated \(QueueRowView.relativeTime(refreshed))")
                    .foregroundStyle(Theme.inkTertiary)
                    .monospacedDigit()
            }
        }
        .font(Theme.body(10.5))
        .padding(.horizontal, 16)
        .padding(.vertical, 7)
    }
}

// The APP-2/APP-3 LayCoursePlaceholderSheet was replaced by the real
// LayCourseSheet (LayCourseSheet.swift) in APP-4.
