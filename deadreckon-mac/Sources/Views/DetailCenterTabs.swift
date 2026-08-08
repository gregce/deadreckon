import AppKit
import DeadreckonKit
import SwiftUI

/// The evidence tabs filling the run's remaining height (REDESIGN-SPEC
/// §D5): Activity · Story · Changes · Checks · Docs · Recorder. Everything
/// here is file-backed or a CLI envelope; story prose is visibly labeled
/// and never sits on a decision surface (the 2.4.4 trust rule).
struct DetailCenterTabsView: View {
    enum Tab: String, CaseIterable {
        case activity = "Activity"
        case story = "Story"
        case changes = "Changes"
        case checks = "Checks"
        case docs = "Docs"
        case recorder = "Recorder"
    }

    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @Binding var tab: Tab
    /// The drill-jump channel (§G1): RunSurfaceView switches the tab; the
    /// owning pane consumes the target and clears it.
    @Binding var drill: DrillTarget?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                ForEach(Tab.allCases, id: \.self) { candidate in
                    TabButton(title: title(candidate), active: tab == candidate) {
                        tab = candidate
                        if candidate == .changes, detail.changes == nil {
                            Task { await detail.refreshChanges() }
                        }
                    }
                }
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Theme.panel)
            Divider().overlay(Theme.border)

            switch tab {
            case .activity:
                ActivityPaneView(detail: detail, live: live, drill: $drill)
            case .story:
                NarrativePaneView(detail: detail, drill: $drill,
                                  openChangesTab: { tab = .changes })
            case .changes:
                ChangesView(detail: detail, drill: $drill)
            case .checks:
                ContractChecksView(row: row, detail: detail, drill: $drill)
            case .docs:
                DocsView(detail: detail)
            case .recorder:
                FlightView(row: row, detail: detail)
            }
        }
        // The DONE MEANS strip can land here on .checks; the Changes lazy
        // load must fire on that path too.
        .onChange(of: tab) { _, newTab in
            if newTab == .changes, detail.changes == nil {
                Task { await detail.refreshChanges() }
            }
        }
    }

    private func title(_ tab: Tab) -> String {
        if tab == .changes, let changes = detail.changes {
            return "Changes \(changes.filesChanged)"
        }
        return tab.rawValue
    }

    private var live: Bool {
        row.projection.phase != .terminal
    }
}

// MARK: - Story

/// The story pane. Trust split (design 2.4.4): the deterministic
/// projection renders as the pane's body; provider-refreshed beats render
/// ONLY behind the visible "AI summary — unverified" label. Neither ever
/// appears on an evidence surface or anywhere near a decision.
struct NarrativePaneView: View {
    @ObservedObject var detail: JobDetailStore
    @Binding var drill: DrillTarget?
    let openChangesTab: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                chipsRow

                if let deterministic = detail.narrative.latestDeterministic {
                    snapshotBody(deterministic)
                } else if detail.narrative.latestSnapshot == nil {
                    Text("No story yet for this run.")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.textTertiary)
                }

                // V7: MAP is deterministic evidence — after the snapshot
                // body, never inside or below the unverified overlay.
                StoryMapSection(
                    detail: detail,
                    openChangesTab: openChangesTab,
                    openChangesFile: { drill = .changesFile($0) })

                if let overlay = detail.narrative.latestSnapshot, overlay.isUnverifiedOverlay {
                    overlayBlock(overlay)
                }

                if let error = detail.narrative.stateDoc?.lastError {
                    Text("story writer error: \(error)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
                if detail.narrative.skippedMalformedRows > 0 {
                    Text("\(detail.narrative.skippedMalformedRows) unreadable story rows skipped")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private var chipsRow: some View {
        HStack(spacing: 6) {
            if let snapshot = detail.narrative.latestSnapshot {
                if let beat = snapshot.live {
                    StatusChip(text: "update #\(beat.beatSeq)", color: Theme.textSecondary)
                        .help("covers turn \(beat.coversTurn) \u{00B7} source \(beat.source)")
                } else {
                    StatusChip(text: snapshot.snapshotID, color: Theme.textSecondary)
                }
            }
            switch detail.narrative.staleness {
            case .fresh(let age):
                StatusChip(text: "written \(age)s ago", color: Theme.success)
            case .stale(let age):
                StatusChip(text: "stale \(ageWords(age))", color: Theme.warn)
            case .unknown:
                StatusChip(text: "no updates yet", color: Theme.textTertiary)
            }
            if let status = detail.narrative.stateDoc?.latestStatus {
                StatusChip(text: status, color: Theme.textTertiary)
            }
        }
    }

    private func ageWords(_ seconds: Int) -> String {
        seconds >= 3600 ? "\(seconds / 3600)h" : seconds >= 60 ? "\(seconds / 60)m" : "\(seconds)s"
    }

    @ViewBuilder private func snapshotBody(_ snapshot: NarrativeSnapshotDoc) -> some View {
        Text(snapshot.headline)
            .font(Theme.body(13))
            .foregroundStyle(Theme.textPrimary)
            .textSelection(.enabled)
        claims("WORKING ON", snapshot.currentWork)
        claims("RISKS", snapshot.risks)
        claims("LIKELY NEXT", snapshot.nextLikely)
    }

    /// The agent-model-refreshed overlay: visibly labeled, warn-bordered,
    /// and never anywhere near a decision surface.
    private func overlayBlock(_ snapshot: NarrativeSnapshotDoc) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                StatusChip(text: "AI summary \u{2014} unverified", color: Theme.warn)
                Text("written by the agent\u{2019}s model; not evidence")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
            }
            Text(snapshot.headline)
                .font(Theme.body(12.5))
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
            if let rolling = snapshot.live?.rollingSummary, !rolling.isEmpty {
                Text(rolling)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }
            claims("WORKING ON", snapshot.currentWork)
            claims("RISKS", snapshot.risks)
            claims("LIKELY NEXT", snapshot.nextLikely)
        }
        .padding(12)
        .background(Theme.warn.opacity(0.05),
                    in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
            .strokeBorder(Theme.warn.opacity(0.5), lineWidth: 1))
    }

    @ViewBuilder private func claims(_ title: String, _ claims: [NarrativeSnapshotDoc.Claim]) -> some View {
        if !claims.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Theme.sectionTitle(title)
                ForEach(Array(claims.enumerated()), id: \.offset) { _, claim in
                    HStack(alignment: .top, spacing: 6) {
                        Text("\u{2022}").foregroundStyle(Theme.textTertiary)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(claim.text)
                                .font(Theme.body(11.5))
                                .foregroundStyle(Theme.textSecondary)
                                .textSelection(.enabled)
                            if !claim.evidence.isEmpty {
                                Text(claim.evidence.joined(separator: " \u{00B7} "))
                                    .font(Theme.mono(9))
                                    .foregroundStyle(Theme.textTertiary)
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Activity (Stream | Turns)

/// Live tail of events.jsonl with the Turns grouping one toggle away
/// (§D5): Stream is the unbounded scrollback plus text search — the two
/// things attach's 1000-event cap cannot give (design 1.4); Turns is the
/// traces.jsonl grouping. Between header and body: the event-density strip
/// (V2), brushable — a drag selects a time window that filters the Stream,
/// composing AND with the text search. Rows expand inline to the raw
/// record inspector (D4).
struct ActivityPaneView: View {
    enum Mode: String, CaseIterable {
        case stream = "Stream"
        case turns = "Turns"
    }

    @ObservedObject var detail: JobDetailStore
    let live: Bool
    @Binding var drill: DrillTarget?
    @State private var mode: Mode = .stream
    @State private var query = ""
    /// Whether the operator is at (or near) the tail: tracked by the
    /// sentinel row's visibility. Auto-follow on append only when pinned —
    /// this pane's purpose is unbounded scrollback READING, and yanking a
    /// scrolled-up operator to the bottom every 2 s tick defeats it
    /// (Console.app/Terminal behavior; the drawer's terminal panes stay
    /// tail-convention always-follow).
    @State private var pinnedToTail = true
    /// The density strip's brush window (V2). Filters the Stream; the chip
    /// survives a switch to Turns and re-applies on return.
    @State private var brush: ClosedRange<Date>?
    @State private var expandedEvents: Set<Int> = []
    /// A `.turn(n)` drill landing: consumed by TurnsListView.
    @State private var pendingTurn: Int?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.border)
            // V2: the density strip renders in both modes and brushes the
            // Stream (it replaces the old header sparkbar).
            ActivityDensityStrip(series: detail.density, live: live, brush: $brush)

            switch mode {
            case .stream: streamBody
            case .turns: TurnsListView(detail: detail, drill: $drill, expandTurn: $pendingTurn)
            }
        }
        .onAppear { consumeDrill() }
        .onChange(of: drill) { _, _ in consumeDrill() }
    }

    /// §G1 consumption: land the jump, then clear it.
    private func consumeDrill() {
        switch drill {
        case .turn(let turn):
            mode = .turns
            pendingTurn = turn
            drill = nil
        case .activityWindow(let window):
            mode = .stream
            brush = window
            drill = nil
        default:
            break
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            HStack(spacing: 2) {
                ForEach(Mode.allCases, id: \.self) { candidate in
                    TabButton(title: candidate.rawValue, active: mode == candidate) {
                        mode = candidate
                    }
                }
            }
            if mode == .stream {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
                TextField("Search the whole scrollback", text: $query)
                    .textFieldStyle(.plain)
                    .font(Theme.body(11))
            }
            Spacer()
            brushChip
            Text("\(detail.activity.count) events")
                .font(Theme.body(10))
                .foregroundStyle(Theme.textTertiary)
                .monospacedDigit()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    /// The active brush window as a neutral chip + its clear button.
    @ViewBuilder private var brushChip: some View {
        if let brush {
            HStack(spacing: 3) {
                StatusChip(
                    text: "\(RunChartTime.hm(brush.lowerBound))\u{2013}\(RunChartTime.hm(brush.upperBound)) \u{00B7} \(filtered.count) shown",
                    color: Theme.textSecondary)
                Button("\u{2715}") { self.brush = nil }
                    .buttonStyle(.themeText(size: 10))
                    .help("clear the time window")
            }
        }
    }

    @ViewBuilder private var streamBody: some View {
        // Computed ONCE per body pass: the counter and the ForEach share the
        // same filtered array instead of scanning the scrollback twice.
        let filtered = self.filtered
        VStack(spacing: 0) {
            if !query.isEmpty {
                HStack {
                    Text("\(filtered.count) / \(detail.activity.count) match")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                        .monospacedDigit()
                    Spacer()
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 3)
            }
            if let issue = detail.activityIssue {
                Text("activity feed stopped: \(issue)")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.dangerText)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 4)
            }

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(filtered) { entry in
                            eventRow(entry)
                                .id(entry.id)
                        }
                        // The tail sentinel: instantiated by the LazyVStack
                        // only when the bottom is on screen, so its
                        // appear/disappear IS the was-at-bottom fact.
                        Color.clear
                            .frame(height: 1)
                            .onAppear { pinnedToTail = true }
                            .onDisappear { pinnedToTail = false }
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: detail.activity.count) { _ in
                    if query.isEmpty, brush == nil, pinnedToTail,
                       let last = detail.activity.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    /// One stream row: the fact line stays selectable; the trailing
    /// chevron expands the raw record inspector inline (D4).
    private func eventRow(_ entry: JobDetailStore.ActivityEntry) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .top, spacing: 8) {
                Text(Self.time(entry.timestamp))
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.textTertiary)
                    .frame(width: 58, alignment: .leading)
                Text(entry.line)
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
                Spacer(minLength: 4)
                Button {
                    if expandedEvents.contains(entry.id) {
                        expandedEvents.remove(entry.id)
                    } else {
                        expandedEvents.insert(entry.id)
                    }
                } label: {
                    Image(systemName: expandedEvents.contains(entry.id)
                        ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                        .frame(width: 16, height: 14)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("show the raw ledger record")
            }
            if expandedEvents.contains(entry.id) {
                EventRawInspector(timestamp: entry.timestamp, raw: entry.raw)
                    .padding(.leading, 66)
                    .padding(.trailing, 8)
            }
        }
    }

    /// Brush window AND text search, composed.
    private var filtered: [JobDetailStore.ActivityEntry] {
        var entries = detail.activity
        if let brush {
            entries = entries.filter { entry in
                entry.timestamp.map { brush.contains($0) } ?? false
            }
        }
        guard !query.isEmpty else { return entries }
        return entries.filter { $0.line.localizedCaseInsensitiveContains(query) }
    }

    private static let formatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    static func time(_ date: Date?) -> String {
        date.map { formatter.string(from: $0) } ?? "\u{2014}"
    }
}

// MARK: - Turns

/// traces.jsonl turn grouping with interleaved tool calls, collapsible
/// (design B2 TURNS pane): ledgers, not a chat buffer. Each row carries the
/// token/duration micro-bars against the shared TurnScale (V3 — the row
/// list IS the chart); trace entries expand to the full tool I/O (D1
/// level 2).
struct TurnsListView: View {
    @ObservedObject var detail: JobDetailStore
    @Binding var drill: DrillTarget?
    /// A `.turn(n)` drill landing: expand + scroll, then clear.
    @Binding var expandTurn: Int?

    @State private var expanded: Set<Int> = []
    @State private var expandedEntries: Set<Int> = []

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 4) {
                    if let issue = detail.tracesIssue {
                        Text("turn feed stopped: \(issue)")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.dangerText)
                            .textSelection(.enabled)
                            .padding(4)
                    }
                    if detail.turns.isEmpty {
                        Text("No turns recorded yet.")
                            .font(Theme.body(12))
                            .foregroundStyle(Theme.textTertiary)
                            .padding(4)
                    } else {
                        legendRow
                    }
                    let scale = TurnScale.derive(turns: detail.turns)
                    ForEach(detail.turns.reversed()) { turn in
                        turnCard(turn, scale: scale)
                            .id("turn-\(turn.turn)")
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onAppear { consumePendingTurn(proxy: proxy) }
            .onChange(of: expandTurn) { _, _ in consumePendingTurn(proxy: proxy) }
        }
    }

    private func consumePendingTurn(proxy: ScrollViewProxy) {
        guard let turn = expandTurn else { return }
        expanded.insert(turn)
        withAnimation(.easeOut(duration: 0.2)) {
            proxy.scrollTo("turn-\(turn)", anchor: .top)
        }
        expandTurn = nil
    }

    /// The micro-bar key, printed once (a legend, not per-row labels).
    private var legendRow: some View {
        HStack(spacing: 4) {
            Spacer()
            Rectangle().fill(Theme.Chart.markQuiet).frame(width: 7, height: 5)
            Text("in")
            Text("\u{00B7}")
            Rectangle().fill(Theme.Chart.markLine).frame(width: 7, height: 5)
            Text("out")
        }
        .font(Theme.body(10))
        .foregroundStyle(Theme.textTertiary)
        .accessibilityHidden(true)
    }

    private func turnCard(_ turn: TurnModel, scale: (maxTokens: Int, maxWallSeconds: Double)) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if expanded.contains(turn.turn) {
                    expanded.remove(turn.turn)
                } else {
                    expanded.insert(turn.turn)
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: expanded.contains(turn.turn) ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                    Text("turn \(turn.turn)")
                        .font(Theme.body(11.5, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                    if let started = turn.startedAt {
                        Text(ActivityPaneView.time(started))
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Spacer()
                    // V3: the fixed-width micro-mark column — every row
                    // draws against the same Kit-derived maxima, so turns
                    // compare at a glance. The printed numbers beside them
                    // are the table twin.
                    VStack(alignment: .leading, spacing: 2) {
                        TokenMicroBar(inTokens: turn.inputTokens,
                                      outTokens: turn.outputTokens,
                                      maxTokens: scale.maxTokens)
                        DurationMicroBar(seconds: turn.wallSeconds,
                                         maxSeconds: scale.maxWallSeconds)
                    }
                    Text("in \(Self.tokens(turn.inputTokens)) out \(Self.tokens(turn.outputTokens))")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    Text(turn.wallSeconds > 0 ? Self.duration(turn.wallSeconds) : "\u{2014}")
                        .font(Theme.mono(9.5))
                        .monospacedDigit()
                        .foregroundStyle(Theme.textTertiary)
                        .frame(width: 46, alignment: .trailing)
                    if turn.costUSD > 0 {
                        Text(String(format: "$%.3f", turn.costUSD))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textSecondary)
                    }
                    Text("\(turn.entries.count) entries")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.textTertiary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded.contains(turn.turn) {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(turn.entries) { entry in
                        entryRow(entry)
                    }
                    // The turn's window in the actual ledger, filtered —
                    // not a paraphrase.
                    if let started = turn.startedAt {
                        JumpLine(title: "activity in this turn", size: 9.5) {
                            drill = .activityWindow(started ... windowEnd(for: turn, started: started))
                        }
                        .padding(.top, 2)
                    }
                }
                .padding(.leading, 18)
                .padding(.bottom, 4)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .cardChrome()
    }

    /// One interleaved entry; trace entries expand inline to the full tool
    /// exchange (D1 level 2).
    private func entryRow(_ entry: TurnModel.Entry) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .top, spacing: 8) {
                Text(ActivityPaneView.time(entry.timestamp))
                    .font(Theme.mono(9))
                    .foregroundStyle(Theme.textTertiary)
                    .frame(width: 54, alignment: .leading)
                Text(entry.text)
                    .font(Theme.mono(10))
                    .foregroundStyle(entryColor(entry.kind))
                    .textSelection(.enabled)
                if entry.kind == .trace {
                    Spacer(minLength: 4)
                    Button {
                        if expandedEntries.contains(entry.ordinal) {
                            expandedEntries.remove(entry.ordinal)
                        } else {
                            expandedEntries.insert(entry.ordinal)
                        }
                    } label: {
                        Image(systemName: expandedEntries.contains(entry.ordinal)
                            ? "chevron.down" : "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(Theme.textTertiary)
                            .frame(width: 16, height: 14)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("show the full tool exchange")
                }
            }
            if entry.kind == .trace, expandedEntries.contains(entry.ordinal) {
                ToolIOView(raw: entry.raw, ledgerPath: tracesLedgerPath) { path in
                    drill = .changesFile(path)
                }
                .padding(.leading, 62)
                .padding(.trailing, 8)
            }
        }
    }

    /// The turn's honest upper bound: the next turn's start, else this
    /// turn's last recorded entry, else its own start — never "now".
    private func windowEnd(for turn: TurnModel, started: Date) -> Date {
        if let next = detail.turns.first(where: { $0.turn > turn.turn })?.startedAt {
            return max(next, started)
        }
        let lastEntry = turn.entries.map(\.timestamp).max()
        return max(lastEntry ?? started, started)
    }

    private var tracesLedgerPath: String {
        guard let runID = detail.currentRunID else { return "traces.jsonl" }
        return DeadreckonHome.url()
            .appendingPathComponent("runstate")
            .appendingPathComponent(detail.scope)
            .appendingPathComponent("runs")
            .appendingPathComponent(runID)
            .appendingPathComponent("traces.jsonl").path
    }

    private func entryColor(_ kind: TurnModel.EntryKind) -> Color {
        switch kind {
        case .error: return Theme.danger
        case .toolCall: return Theme.textPrimary
        case .toolResult, .docs, .trace, .other: return Theme.textSecondary
        }
    }

    static func tokens(_ count: Int) -> String {
        count >= 1000 ? String(format: "%.1fk", Double(count) / 1000) : "\(count)"
    }

    static func duration(_ seconds: Double) -> String {
        if seconds >= 60 {
            let total = Int(seconds.rounded())
            return "\(total / 60)m \(total % 60)s"
        }
        return String(format: "%.1fs", seconds)
    }
}

// MARK: - Checks (first-class)

/// The frozen acceptance contract rendered check-by-check, crossed with the
/// live acceptance-progress band; when terminal, the two sign-offs and the
/// recorded check results. VERIFIED language only from the shared proof
/// classifier (trust rule 6); live rows are display data, never evidence
/// (TAILING.md).
struct ContractChecksView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore
    @Binding var drill: DrillTarget?

    @State private var expandedChecks: Set<Int> = []
    @State private var expandedLive: Set<Int> = []

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if row.projection.phase == .terminal {
                    twoKeysBand
                }
                contractBand
                liveBand
                if row.projection.phase == .terminal {
                    receiptAuditBand
                }
                if let issue = detail.reportIssue {
                    Text("report unavailable: \(issue)")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .onAppear { consumeDrill() }
        .onChange(of: drill) { _, _ in consumeDrill() }
        .onChange(of: detail.report) { _, _ in consumeDrill() }
    }

    /// `.recordedCheck` landing (§G1): expand the matching recorded row.
    private func consumeDrill() {
        guard case .recordedCheck(let kind, let command) = drill,
              let checks = detail.report?.deterministicChecks else { return }
        if let index = checks.firstIndex(where: { $0.kind == kind && $0.command == command }) {
            expandedChecks.insert(index)
        }
        drill = nil
    }

    @ViewBuilder private var contractBand: some View {
        VStack(alignment: .leading, spacing: 6) {
            Theme.sectionTitle("WHAT DONE MEANS")
            HStack(spacing: 6) {
                Text("acceptance.yaml")
                    .font(Theme.mono(10.5))
                    .foregroundStyle(Theme.textPrimary)
                Text("(frozen)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
            }
            digestChip
            networkLine

            if let contract = detail.report?.contract {
                let rows = contract.checkRows
                if rows.isEmpty {
                    Text("no checks could be read from the frozen definition")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, check in
                    HStack(alignment: .top, spacing: 6) {
                        Text(check.kind)
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.textPrimary)
                        if check.mustPass {
                            StatusChip(text: "must pass", color: Theme.textSecondary)
                        }
                        Text(check.subject)
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(2)
                            .textSelection(.enabled)
                    }
                }
            } else {
                Text("reading the run\u{2019}s report\u{2026}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textTertiary)
                    .help("report --json")
            }
        }
    }

    @ViewBuilder private var digestChip: some View {
        if let contract = detail.report?.contract {
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
    }

    @ViewBuilder private var networkLine: some View {
        if let network = detail.status?.job?.job.policy?.execution?.gate?.network {
            Text(network == "deny" ? "network: not allowed" : "network: \(network)")
                .font(Theme.body(10))
                .foregroundStyle(network == "deny" ? Theme.textSecondary : Theme.warn)
                .help("capability compiled into the run's policy (gate.network)")
        }
    }

    /// The live acceptance-progress band with the restart rule: rows are
    /// scoped to the current gate attempt (the store already discards on
    /// restart). Display data only, never evidence.
    @ViewBuilder private var liveBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Theme.sectionTitle("CHECKS RUNNING NOW")
                Text("as they stream \u{2014} not evidence")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.textTertiary)
            }
            if detail.liveChecks.isEmpty {
                Text("nothing streaming \u{2014} strict checks appear all at once when the record is signed")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
            }
            ForEach(Array(detail.liveChecks.enumerated()), id: \.offset) { index, progressRow in
                liveRow(progressRow, index: index)
            }
        }
    }

    /// One live-band row: expandable to the same full evidence component
    /// as recorded rows, minus history (D2) — display data, never evidence.
    @ViewBuilder private func liveRow(_ progressRow: AcceptanceProgressRow, index: Int) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Button {
                guard progressRow.result != nil else { return }
                if expandedLive.contains(index) {
                    expandedLive.remove(index)
                } else {
                    expandedLive.insert(index)
                }
            } label: {
                HStack(alignment: .top, spacing: 6) {
                    if progressRow.result != nil {
                        Image(systemName: expandedLive.contains(index)
                            ? "chevron.down" : "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Text(glyph(progressRow))
                        .font(Theme.body(10, weight: .bold))
                        .foregroundStyle(color(progressRow))
                        .frame(width: 12)
                    Text("\(progressRow.index)/\(progressRow.total)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    Text(progressRow.result?.kind ?? progressRow.status)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textPrimary)
                    if let detailText = progressRow.result?.detail, !detailText.isEmpty {
                        Text(detailText)
                            .font(Theme.body(9.5))
                            .foregroundStyle(progressRow.result?.passed == false
                                ? Theme.dangerText : Theme.textTertiary)
                            .lineLimit(1)
                    }
                    if let duration = progressRow.result?.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expandedLive.contains(index), let result = progressRow.result {
                CheckEvidenceView(result: result)
                    .padding(.leading, 18)
            }
        }
    }

    private func glyph(_ progressRow: AcceptanceProgressRow) -> String {
        guard let result = progressRow.result else { return "\u{25CC}" }
        return result.passed ? "\u{2713}" : "\u{2717}"
    }

    private func color(_ progressRow: AcceptanceProgressRow) -> Color {
        guard let result = progressRow.result else { return Theme.textTertiary }
        return result.passed ? Theme.success : Theme.danger
    }

    /// Two sign-offs (design B1): the signed record of checks and the
    /// judge's call. Words come from the report; the Verified chip only
    /// from the rollup's proof classifier.
    @ViewBuilder private var twoKeysBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            Theme.sectionTitle("TWO SIGN-OFFS")

            HStack(spacing: 6) {
                if let receipt = detail.report?.receipt {
                    Text("Checks record: \(receipt.status)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(Theme.textPrimary)
                        .help("signed")
                    if receipt.contained == true {
                        StatusChip(text: "sandboxed", color: Theme.textSecondary)
                            .help(receipt.sandboxBackend ?? "")
                    }
                } else {
                    Text("Checks record: unknown")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            if let error = detail.report?.receipt?.signatureValidationError {
                Text(error)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.dangerText)
                    .textSelection(.enabled)
            }

            HStack(spacing: 6) {
                if let judgment = detail.report?.semantic?.judgment {
                    Text(judgment.decision == "achieved"
                        ? "Judged: achieved" : "Judged: \(judgment.decision)")
                        .font(Theme.mono(10.5))
                        .foregroundStyle(judgment.decision == "achieved" ? Theme.textPrimary : Theme.warn)
                } else {
                    Text("Judge: pending")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            if let summary = detail.report?.semantic?.judgment?.summary, !summary.isEmpty {
                // The judge's reason is quoted verbatim, never paraphrased.
                Text("\u{201C}\(summary)\u{201D}")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }

            if row.receipt?.verified == .valid {
                StatusChip(text: Lexicon.proofVerified, color: Theme.success, strong: true)
                    .help(Lexicon.verifiedHelp)
            }
            if row.receipt?.verified == .invalid {
                StatusChip(text: Lexicon.proofInvalid, color: Theme.danger, strong: true,
                           textColor: Theme.dangerText)
                    .help(row.receipt?.error ?? "the signed receipt did not validate")
            }
        }
    }

    /// Receipt evidence for terminal jobs, sourced from `report --json`: the
    /// deterministic checks the signed marker recorded, check by check.
    /// Inspection only; the strict fail-closed path in the binary stays the
    /// sole promotion authority.
    ///
    /// Deliberately NOT `verdict --receipt --json`: the committed binary's
    /// `verdict` accepts run references only, a Single-shape job's id
    /// resolves to the Job kind, and the child run is driver-fenced, so a
    /// fresh checks re-run cannot exist for job-owned attempts today. The
    /// Rust-side follow-up (verdict accepting JOB refs) is registered in
    /// CONTRACTS.md; the G7 per-digest audit facts land here with it.
    @ViewBuilder private var receiptAuditBand: some View {
        VStack(alignment: .leading, spacing: 5) {
            Theme.sectionTitle("RECORDED CHECK RESULTS")

            if let report = detail.report {
                if report.deterministicChecks.isEmpty {
                    Text("no check results recorded in the report")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.textTertiary)
                }
                // V4: duration bars against the shared maximum, only when
                // 2+ results carry a duration (a lone duration is a number,
                // not a comparison). The printed seconds stay — table twin.
                let durations = CheckDurations.derive(results: report.deterministicChecks)
                ForEach(Array(report.deterministicChecks.enumerated()), id: \.offset) { index, check in
                    recordedCheckRow(check, index: index, report: report, durations: durations)
                }
                Text("Recorded when the checks ran (`report --json`). The app can\u{2019}t re-run checks for a run today \u{2014} a registered CLI gap.")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.textTertiary)
            } else if let issue = detail.reportIssue {
                Text("report unavailable: \(issue)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            } else {
                Text("reading the run\u{2019}s report\u{2026}")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
                    .help("report --json")
            }
        }
    }

    /// One recorded check row (D2): the row is the drill — click expands to
    /// the full evidence (command/cwd/outputs + per-attempt history).
    private func recordedCheckRow(
        _ check: AcceptanceProgressRow.CheckResult, index: Int,
        report: JobReportEnvelope,
        durations: (rows: [CheckDurations.Row], maxMS: Int, showBars: Bool)
    ) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Button {
                if expandedChecks.contains(index) {
                    expandedChecks.remove(index)
                } else {
                    expandedChecks.insert(index)
                }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: expandedChecks.contains(index)
                        ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                    Text(check.passed ? "\u{2713}" : "\u{2717}")
                        .font(Theme.body(10, weight: .bold))
                        .foregroundStyle(check.passed ? Theme.success : Theme.danger)
                        .frame(width: 12)
                    Text(check.kind)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textPrimary)
                    if !check.detail.isEmpty {
                        Text(check.detail)
                            .font(Theme.body(9.5))
                            .foregroundStyle(check.passed ? Theme.textTertiary : Theme.dangerText)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if let duration = check.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(9.5))
                            .monospacedDigit()
                            .foregroundStyle(Theme.textTertiary)
                        if durations.showBars {
                            CheckDurationBar(durationMS: duration,
                                             maxMS: durations.maxMS,
                                             passed: check.passed)
                        }
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expandedChecks.contains(index) {
                CheckEvidenceView(result: check, attempts: report.attempts)
                    .padding(.leading, 18)
                    .padding(.top, 2)
            }
        }
    }
}

// MARK: - Changes

/// Diffstat list from `show --diff --json` (G10); per-file unified patch
/// loaded on demand via `--patch --file` with truncation honesty.
struct ChangesView: View {
    @ObservedObject var detail: JobDetailStore
    @Binding var drill: DrillTarget?
    @State private var expandedPath: String?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        if let changes = detail.changes {
                            Text("\u{0394} \(changes.filesChanged) files \u{00B7} +\(changes.added) \u{2212}\(changes.removed)")
                                .font(Theme.body(11, weight: .medium))
                                .foregroundStyle(Theme.textPrimary)
                                .monospacedDigit()
                        }
                        Spacer()
                        Button {
                            Task { await detail.refreshChanges() }
                        } label: {
                            Image(systemName: "arrow.clockwise").font(.system(size: 10))
                        }
                        .buttonStyle(.tactile)
                        .help("Refresh the diff \u{2014} show --diff --json")
                    }

                    if let issue = detail.changesIssue {
                        Text(issue)
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.warn)
                            .textSelection(.enabled)
                    } else if detail.changes == nil {
                        Text("Reading the run diff \u{2026}")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textTertiary)
                    } else if detail.changes?.files.isEmpty == true {
                        Text("No source changes recorded.")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textTertiary)
                    }

                    ForEach(detail.changes?.files ?? [], id: \.path) { file in
                        fileRow(file)
                            .id(file.path)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onAppear { consumeDrill(proxy: proxy) }
            .onChange(of: drill) { _, _ in consumeDrill(proxy: proxy) }
            .onChange(of: detail.changes) { _, _ in consumeDrill(proxy: proxy) }
        }
    }

    /// `.changesFile` landing (§G1): select + expand + trigger the
    /// existing lazy patch load — no new fetch path. Waits for the diff
    /// list when the jump raced the on-demand `show --diff` read; jump
    /// paths from trace exchanges are absolute worktree paths, so suffix
    /// matching bridges them to the diff's repo-relative paths.
    private func consumeDrill(proxy: ScrollViewProxy) {
        guard case .changesFile(let target) = drill else { return }
        guard let files = detail.changes?.files else { return }
        let match = files.first { $0.path == target }
            ?? files.first { target.hasSuffix("/" + $0.path) || $0.path.hasSuffix(target) }
        if let match {
            expandedPath = match.path
            if detail.patches[match.path] == nil {
                Task { await detail.loadPatch(path: match.path) }
            }
            withAnimation(.easeOut(duration: 0.2)) {
                proxy.scrollTo(match.path, anchor: .top)
            }
        }
        drill = nil
    }

    @ViewBuilder private func fileRow(_ file: DiffSummaryModel.FileDelta) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if expandedPath == file.path {
                    expandedPath = nil
                } else {
                    expandedPath = file.path
                    if detail.patches[file.path] == nil {
                        Task { await detail.loadPatch(path: file.path) }
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Text(statusGlyph(file.status))
                        .font(Theme.mono(10))
                        .foregroundStyle(statusColor(file.status))
                        .frame(width: 12)
                    Text(file.path)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text("+\(file.added)")
                        .font(Theme.mono(9.5)).foregroundStyle(Theme.success)
                    Text("\u{2212}\(file.removed)")
                        .font(Theme.mono(9.5)).foregroundStyle(Theme.danger)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expandedPath == file.path {
                patchBody(path: file.path)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .cardChrome()
    }

    @ViewBuilder private func patchBody(path: String) -> some View {
        // D6: the expanded header shows the FULL path (the collapsed row
        // may middle-truncate with no recourse) + a copy affordance.
        HStack(spacing: 6) {
            Text(path)
                .font(Theme.mono(10))
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
            CopyTextButton(text: path, label: "copy path")
            Spacer(minLength: 0)
        }
        if let patch = detail.patches[path] {
            VStack(alignment: .leading, spacing: 3) {
                if let note = patch.note {
                    Text(note).font(Theme.body(9.5)).foregroundStyle(Theme.textTertiary)
                }
                if patch.truncated {
                    Text("patch truncated by the CLI\u{2019}s size limit")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.warn)
                }
                ScrollView(.horizontal) {
                    Text(patch.unified.isEmpty ? "(empty patch)" : patch.unified)
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(6)
            .background(Theme.well, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))
        } else if let issue = detail.patchIssues[path] {
            Text(issue).font(Theme.body(9.5)).foregroundStyle(Theme.warn)
        } else {
            Text("loading patch \u{2026}").font(Theme.body(9.5)).foregroundStyle(Theme.textTertiary)
        }
    }

    private func statusGlyph(_ status: String) -> String {
        switch status {
        case "added": return "A"
        case "removed": return "D"
        case "modified": return "M"
        default: return "?"
        }
    }

    private func statusColor(_ status: String) -> Color {
        switch status {
        case "added": return Theme.success
        case "removed": return Theme.danger
        case "modified": return Theme.accent
        default: return Theme.warn
        }
    }
}

// MARK: - Recorder

/// The recorder: checkpoint cards from the manifest tree. [Rewind…] arms
/// via a capability probe (SETTINGS-SCREENS-SPEC §R1): the vendored 0.8.4
/// binary speaks `rewind --json`, so the probe arms it; an older binary
/// degrades to the probe's honest words — no hardcoded gap label. The flow
/// is preview-first, always (RewindSheet).
struct FlightView: View {
    let row: FleetRow
    @ObservedObject var detail: JobDetailStore

    @EnvironmentObject private var router: WriteSurfaceRouter
    @StateObject private var rewindCapability = VerbCapabilityProbe(
        cli: WriteCLI.client, verb: ["rewind"])
    /// The card briefly flashed by a scrubber click (border swap, 250ms).
    @State private var flashedCheckpointID: String?
    /// A scrubber click's landing card, consumed inside the card scroller.
    @State private var scrollTarget: String?

    var body: some View {
        // The facts + scrubber stay a FIXED band above the scrolling cards:
        // the scrubber is chart-as-index (§G rule 2 — bands must not
        // reflow), and an index that scrolls itself out of view on use
        // loses the position it exists to give.
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                if let issue = detail.flightIssue {
                    Text("recorder feed stopped: \(issue)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.dangerText)
                        .textSelection(.enabled)
                }
                if let manifest = detail.flight.manifest {
                    VStack(alignment: .leading, spacing: 3) {
                        Theme.sectionTitle("RECORDER")
                        Text("\(detail.flight.eventCount) events this session \u{00B7} \(manifest.sessions.count) sessions")
                            .font(Theme.body(10.5))
                            .foregroundStyle(Theme.textSecondary)
                            .monospacedDigit()
                        if let last = detail.flight.lastEventSummary {
                            Text(last)
                                .font(Theme.body(10))
                                .foregroundStyle(Theme.textTertiary)
                                .lineLimit(2)
                        }
                    }
                } else {
                    Text("No recordings for this run yet.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }

                // V8: the scrubber — chart-as-index over the recorded
                // stamps; it scrubs the eye, never the run.
                let timeline = CheckpointTimeline.derive(
                    checkpoints: detail.flight.checkpoints,
                    sessions: detail.flight.manifest?.sessions ?? [],
                    runStartedAt: detail.runState?.startedAt)
                if !timeline.ticks.isEmpty {
                    CheckpointScrubber(
                        timeline: timeline,
                        live: row.projection.phase != .terminal
                    ) { checkpointID in
                        scrollTarget = checkpointID
                        flashedCheckpointID = checkpointID
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                            withAnimation(.easeOut(duration: 0.25)) {
                                if flashedCheckpointID == checkpointID {
                                    flashedCheckpointID = nil
                                }
                            }
                        }
                    }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            Divider().overlay(Theme.border)

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        if detail.flight.checkpoints.isEmpty {
                            Text("No checkpoints captured yet.")
                                .font(Theme.body(10.5))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        ForEach(detail.flight.checkpoints.reversed(), id: \.checkpointID) { checkpoint in
                            checkpointCard(checkpoint)
                                .id(checkpoint.checkpointID)
                        }
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: scrollTarget) { _, target in
                    guard let target else { return }
                    withAnimation(.easeOut(duration: 0.25)) {
                        proxy.scrollTo(target, anchor: .center)
                    }
                    scrollTarget = nil
                }
            }
        }
        .task { await rewindCapability.probe() }
    }

    private func checkpointCard(_ checkpoint: CheckpointManifestDoc) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(checkpoint.checkpointID)
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textPrimary)
                if checkpoint.fullAnchor {
                    StatusChip(text: "full snapshot", color: Theme.textSecondary)
                }
                Spacer()
                Text(ActivityPaneView.time(checkpoint.createdAt))
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.textTertiary)
            }
            Text("turn \(checkpoint.deadreckonTurn) \u{00B7} \(Lexicon.checkpointTrigger(checkpoint.trigger)) \u{00B7} \(checkpoint.fileCount) file\(checkpoint.fileCount == 1 ? "" : "s")")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textSecondary)
            rewindAffordance(checkpoint)
        }
        .padding(8)
        .cardChrome(hovering: flashedCheckpointID == checkpoint.checkpointID)
    }

    /// Armed by the capability probe, never a dead control: when the
    /// binary's rewind speaks --json the button routes to the preview-first
    /// RewindSheet; otherwise the degraded words carry the probe's own
    /// finding + the Terminal escape hatch.
    @ViewBuilder private func rewindAffordance(_ checkpoint: CheckpointManifestDoc) -> some View {
        switch rewindCapability.state {
        case .armed:
            Button("Rewind\u{2026}") {
                router.pending = .rewind(
                    row,
                    runID: detail.currentRunID ?? row.jobID,
                    checkpoint: checkpoint)
            }
            .buttonStyle(.themeStandard(compact: true))
            .help("Preview first \u{2014} deadreckon rewind \(detail.currentRunID ?? row.jobID) --to-checkpoint \(checkpoint.checkpointID) --preview --json")
        case .unknown, .probing:
            disabledRewind(help: "checking whether this CLI's rewind speaks a machine envelope\u{2026}")
        case .missing:
            disabledRewind(help: "This CLI\u{2019}s rewind lists no --json envelope \u{2014} use deadreckon rewind in Terminal.")
        case .failed(let words):
            disabledRewind(help: words)
        }
    }

    private func disabledRewind(help: String) -> some View {
        Button("Rewind\u{2026}") {}
            .buttonStyle(.plain)
            .font(Theme.body(9.5, weight: .medium))
            .foregroundStyle(Theme.textTertiary)
            .disabled(true)
            .help(help)
    }
}

// MARK: - Docs

/// Run docs listing from `<working_dir>/.deadreckon/docs`; an honest empty
/// state otherwise.
struct DocsView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 6) {
                if detail.docs.isEmpty {
                    Text("No documents written by this run yet.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                        .help(".deadreckon/docs")
                }
                ForEach(detail.docs) { doc in
                    Button {
                        NSWorkspace.shared.open(URL(fileURLWithPath: doc.path))
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "doc.text")
                                .font(.system(size: 10))
                                .foregroundStyle(Theme.textTertiary)
                            Text(doc.name)
                                .font(Theme.mono(10))
                                .foregroundStyle(Theme.textPrimary)
                            Spacer()
                            Text(Self.bytes(doc.bytes))
                                .font(Theme.mono(9))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .cardChrome()
                    .help("Open in the default editor")
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    static func bytes(_ count: Int) -> String {
        count >= 1024 * 1024 ? String(format: "%.1f MB", Double(count) / 1048576)
            : count >= 1024 ? String(format: "%.0f KB", Double(count) / 1024)
            : "\(count) B"
    }
}
