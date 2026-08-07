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
            case .activity: ActivityPaneView(detail: detail)
            case .story: NarrativePaneView(detail: detail)
            case .changes: ChangesView(detail: detail)
            case .checks: ContractChecksView(row: row, detail: detail)
            case .docs: DocsView(detail: detail)
            case .recorder: FlightView(detail: detail)
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
}

// MARK: - Story

/// The story pane. Trust split (design 2.4.4): the deterministic
/// projection renders as the pane's body; provider-refreshed beats render
/// ONLY behind the visible "AI summary — unverified" label. Neither ever
/// appears on an evidence surface or anywhere near a decision.
struct NarrativePaneView: View {
    @ObservedObject var detail: JobDetailStore

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
/// traces.jsonl grouping. Header right: the activity sparkbar (per-turn
/// entry counts) + the event count.
struct ActivityPaneView: View {
    enum Mode: String, CaseIterable {
        case stream = "Stream"
        case turns = "Turns"
    }

    @ObservedObject var detail: JobDetailStore
    @State private var mode: Mode = .stream
    @State private var query = ""
    /// Whether the operator is at (or near) the tail: tracked by the
    /// sentinel row's visibility. Auto-follow on append only when pinned —
    /// this pane's purpose is unbounded scrollback READING, and yanking a
    /// scrolled-up operator to the bottom every 2 s tick defeats it
    /// (Console.app/Terminal behavior; the drawer's terminal panes stay
    /// tail-convention always-follow).
    @State private var pinnedToTail = true

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.border)

            switch mode {
            case .stream: streamBody
            case .turns: TurnsListView(detail: detail)
            }
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
            sparkbar
            Text("\(detail.activity.count) events")
                .font(Theme.body(10))
                .foregroundStyle(Theme.textTertiary)
                .monospacedDigit()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    /// Per-turn entry counts as quiet bars (§D5 grammar: border-colored
    /// bars, the current turn accent). Counts, not prose.
    @ViewBuilder private var sparkbar: some View {
        if !detail.turns.isEmpty {
            let recent = Array(detail.turns.suffix(30))
            let maxCount = max(recent.map { $0.entries.count }.max() ?? 1, 1)
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(recent) { turn in
                    RoundedRectangle(cornerRadius: 1)
                        .fill(turn.id == recent.last?.id ? Theme.accent : Theme.border)
                        .frame(width: 3,
                               height: max(2, CGFloat(turn.entries.count) / CGFloat(maxCount) * 14))
                        .help("turn \(turn.turn): \(turn.entries.count) entries")
                }
            }
            .accessibilityHidden(true)
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
                            HStack(alignment: .top, spacing: 8) {
                                Text(Self.time(entry.timestamp))
                                    .font(Theme.mono(9.5))
                                    .foregroundStyle(Theme.textTertiary)
                                    .frame(width: 58, alignment: .leading)
                                Text(entry.line)
                                    .font(Theme.mono(10.5))
                                    .foregroundStyle(Theme.textSecondary)
                                    .textSelection(.enabled)
                            }
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
                    if query.isEmpty, pinnedToTail, let last = detail.activity.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    private var filtered: [JobDetailStore.ActivityEntry] {
        guard !query.isEmpty else { return detail.activity }
        return detail.activity.filter { $0.line.localizedCaseInsensitiveContains(query) }
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
/// (design B2 TURNS pane): ledgers, not a chat buffer.
struct TurnsListView: View {
    @ObservedObject var detail: JobDetailStore
    @State private var expanded: Set<Int> = []

    var body: some View {
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
                }
                ForEach(detail.turns.reversed()) { turn in
                    turnCard(turn)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func turnCard(_ turn: TurnModel) -> some View {
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
                    Text("in \(Self.tokens(turn.inputTokens)) out \(Self.tokens(turn.outputTokens))")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
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
                        HStack(alignment: .top, spacing: 8) {
                            Text(ActivityPaneView.time(entry.timestamp))
                                .font(Theme.mono(9))
                                .foregroundStyle(Theme.textTertiary)
                                .frame(width: 54, alignment: .leading)
                            Text(entry.text)
                                .font(Theme.mono(10))
                                .foregroundStyle(entryColor(entry.kind))
                                .textSelection(.enabled)
                        }
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
            ForEach(Array(detail.liveChecks.enumerated()), id: \.offset) { _, progressRow in
                liveRow(progressRow)
            }
        }
    }

    @ViewBuilder private func liveRow(_ progressRow: AcceptanceProgressRow) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text(glyph(progressRow))
                .font(Theme.body(10, weight: .bold))
                .foregroundStyle(color(progressRow))
                .frame(width: 12)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text("\(progressRow.index)/\(progressRow.total)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    Text(progressRow.result?.kind ?? progressRow.status)
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textPrimary)
                    if let duration = progressRow.result?.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
                if let result = progressRow.result {
                    CheckResultDetail(result: result)
                }
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
                ForEach(Array(report.deterministicChecks.enumerated()), id: \.offset) { _, check in
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 6) {
                            Text(check.passed ? "\u{2713}" : "\u{2717}")
                                .font(Theme.body(10, weight: .bold))
                                .foregroundStyle(check.passed ? Theme.success : Theme.danger)
                                .frame(width: 12)
                            Text(check.kind)
                                .font(Theme.mono(10))
                                .foregroundStyle(Theme.textPrimary)
                            if let duration = check.durationMS {
                                Text(String(format: "%.1fs", Double(duration) / 1000))
                                    .font(Theme.mono(9.5))
                                    .foregroundStyle(Theme.textTertiary)
                            }
                        }
                        CheckResultDetail(result: check)
                            .padding(.leading, 18)
                    }
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
}

/// One check result's detail with expandable clipped output when present.
struct CheckResultDetail: View {
    let result: AcceptanceProgressRow.CheckResult
    @State private var outputShown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !result.detail.isEmpty {
                Text(result.detail)
                    .font(Theme.body(9.5))
                    .foregroundStyle(result.passed ? Theme.textTertiary : Theme.danger)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
            if hasOutput {
                Button(outputShown ? "hide output" : "show output") {
                    outputShown.toggle()
                }
                .buttonStyle(.themeText(size: 9))
                if outputShown {
                    if let stdout = result.stdout, !stdout.isEmpty {
                        clipped(stdout, label: "stdout")
                    }
                    if let stderr = result.stderr, !stderr.isEmpty {
                        clipped(stderr, label: "stderr")
                    }
                }
            }
        }
    }

    private var hasOutput: Bool {
        !(result.stdout ?? "").isEmpty || !(result.stderr ?? "").isEmpty
    }

    private func clipped(_ text: String, label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("\(label) (clipped when recorded)")
                .font(Theme.body(8.5))
                .foregroundStyle(Theme.textTertiary)
            ScrollView(.horizontal) {
                Text(text)
                    .font(Theme.mono(9))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 120)
        }
        .padding(6)
        .background(Theme.well, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(Theme.border, lineWidth: 1))
    }
}

// MARK: - Changes

/// Diffstat list from `show --diff --json` (G10); per-file unified patch
/// loaded on demand via `--patch --file` with truncation honesty.
struct ChangesView: View {
    @ObservedObject var detail: JobDetailStore
    @State private var expandedPath: String?

    var body: some View {
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
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
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

/// The recorder: checkpoint cards from the manifest tree. PREVIEW facts
/// only — the rewind-apply verb has no machine envelope yet and its
/// affordance says so.
struct FlightView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
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

                if detail.flight.checkpoints.isEmpty {
                    Text("No checkpoints captured yet.")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.textTertiary)
                }
                ForEach(detail.flight.checkpoints.reversed(), id: \.checkpointID) { checkpoint in
                    checkpointCard(checkpoint)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
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
            Text("turn \(checkpoint.deadreckonTurn) \u{00B7} \(checkpoint.trigger) \u{00B7} \(checkpoint.fileCount) files")
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.textSecondary)
            Button("Rewind\u{2026}") {}
                .buttonStyle(.plain)
                .font(Theme.body(9.5, weight: .medium))
                .foregroundStyle(Theme.textTertiary)
                .disabled(true)
                .help("Rewind isn't available from the app yet (no machine envelope in this CLI version) \u{2014} use deadreckon in Terminal.")
        }
        .padding(8)
        .cardChrome()
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
