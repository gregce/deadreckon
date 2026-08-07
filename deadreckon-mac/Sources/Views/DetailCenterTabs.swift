import DeadreckonKit
import SwiftUI

/// Center pane tabs: Narrative | Activity | Turns | Timeline (design B1/B2).
struct DetailCenterTabsView: View {
    enum Tab: String, CaseIterable {
        case narrative = "Narrative"
        case activity = "Activity"
        case turns = "Turns"
        case timeline = "Timeline"
    }

    @ObservedObject var detail: JobDetailStore
    @State private var tab: Tab = .narrative

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                ForEach(Tab.allCases, id: \.self) { candidate in
                    Button {
                        tab = candidate
                    } label: {
                        Text(candidate.rawValue)
                            .font(Theme.body(11, weight: tab == candidate ? .semibold : .regular))
                            .foregroundStyle(tab == candidate ? Theme.ink : Theme.inkSecondary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(
                                tab == candidate ? Theme.card : .clear,
                                in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    }
                    .buttonStyle(.tactile)
                }
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            Divider().overlay(Theme.hairline)

            switch tab {
            case .narrative: NarrativePaneView(detail: detail)
            case .activity: ActivityPaneView(detail: detail)
            case .turns: TurnsPaneView(detail: detail)
            case .timeline: TimelinePaneView(detail: detail)
            }
        }
    }
}

// MARK: - Narrative

/// The narrative pane. Trust split (design 2.4.4): the deterministic
/// projection renders as the pane's body; provider-refreshed beats render
/// ONLY behind the visible "overlay — unverified" label. Neither ever
/// appears in the evidence rail or any promote-adjacent surface.
struct NarrativePaneView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                chipsRow

                if let deterministic = detail.narrative.latestDeterministic {
                    snapshotBody(deterministic)
                } else if detail.narrative.latestSnapshot == nil {
                    Text("No narrative beats yet for this attempt.")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.inkTertiary)
                }

                if let overlay = detail.narrative.latestSnapshot, overlay.isUnverifiedOverlay {
                    overlayBlock(overlay)
                }

                if let error = detail.narrative.stateDoc?.lastError {
                    Text("narrator error: \(error)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
                if detail.narrative.skippedMalformedRows > 0 {
                    Text("\(detail.narrative.skippedMalformedRows) malformed snapshot rows skipped")
                        .font(Theme.body(10))
                        .foregroundStyle(Theme.inkTertiary)
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
                    StatusChip(text: "snapshot #\(beat.beatSeq)", color: Theme.inkSecondary)
                        .help("covers turn \(beat.coversTurn) \u{00B7} source \(beat.source)")
                } else {
                    StatusChip(text: snapshot.snapshotID, color: Theme.inkSecondary)
                }
            }
            switch detail.narrative.staleness {
            case .fresh(let age):
                StatusChip(text: "refreshed \(age)s ago", color: Theme.verified)
            case .stale(let age):
                StatusChip(text: "stale \(ageWords(age))", color: Theme.warn)
            case .unknown:
                StatusChip(text: "no snapshot yet", color: Theme.inkTertiary)
            }
            if let status = detail.narrative.stateDoc?.latestStatus {
                StatusChip(text: status, color: Theme.inkTertiary)
            }
        }
    }

    private func ageWords(_ seconds: Int) -> String {
        seconds >= 3600 ? "\(seconds / 3600)h" : seconds >= 60 ? "\(seconds / 60)m" : "\(seconds)s"
    }

    @ViewBuilder private func snapshotBody(_ snapshot: NarrativeSnapshotDoc) -> some View {
        Text(snapshot.headline)
            .font(Theme.body(13))
            .foregroundStyle(Theme.ink)
            .textSelection(.enabled)
        claims("current work", snapshot.currentWork)
        claims("risks", snapshot.risks)
        claims("next likely", snapshot.nextLikely)
    }

    /// The provider-refreshed overlay: visibly labeled, amber-bordered, and
    /// never anywhere near a decision surface.
    private func overlayBlock(_ snapshot: NarrativeSnapshotDoc) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                StatusChip(text: "overlay \u{2014} unverified", color: Theme.warn, filled: true)
                Text("provider-refreshed prose; not evidence")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
            }
            Text(snapshot.headline)
                .font(Theme.body(12.5))
                .foregroundStyle(Theme.ink)
                .textSelection(.enabled)
            if let rolling = snapshot.live?.rollingSummary, !rolling.isEmpty {
                Text(rolling)
                    .font(Theme.body(11.5))
                    .foregroundStyle(Theme.inkSecondary)
                    .textSelection(.enabled)
            }
            claims("current work", snapshot.currentWork)
            claims("risks", snapshot.risks)
            claims("next likely", snapshot.nextLikely)
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
                Text(title.uppercased())
                    .font(Theme.body(9, weight: .bold))
                    .kerning(0.5)
                    .foregroundStyle(Theme.inkTertiary)
                ForEach(Array(claims.enumerated()), id: \.offset) { _, claim in
                    HStack(alignment: .top, spacing: 6) {
                        Text("\u{2022}").foregroundStyle(Theme.inkTertiary)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(claim.text)
                                .font(Theme.body(11.5))
                                .foregroundStyle(Theme.inkSecondary)
                                .textSelection(.enabled)
                            if !claim.evidence.isEmpty {
                                Text(claim.evidence.joined(separator: " \u{00B7} "))
                                    .font(Theme.mono(9))
                                    .foregroundStyle(Theme.inkTertiary)
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Activity

/// Live tail of events.jsonl: unbounded scrollback plus text search — the
/// two things attach's 1000-event cap cannot give (design 1.4).
struct ActivityPaneView: View {
    @ObservedObject var detail: JobDetailStore
    @State private var query = ""

    var body: some View {
        // Computed ONCE per body pass: the counter and the ForEach share the
        // same filtered array instead of scanning the scrollback twice.
        let filtered = self.filtered
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.inkTertiary)
                TextField("Search the whole scrollback", text: $query)
                    .textFieldStyle(.plain)
                    .font(Theme.body(11))
                Text("\(filtered.count) / \(detail.activity.count)")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.inkTertiary)
                    .monospacedDigit()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            Divider().overlay(Theme.hairline)

            if let issue = detail.activityIssue {
                Text("events tail stopped: \(issue)")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.danger)
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
                                    .foregroundStyle(Theme.inkTertiary)
                                    .frame(width: 58, alignment: .leading)
                                Text(entry.line)
                                    .font(Theme.mono(10.5))
                                    .foregroundStyle(Theme.inkSecondary)
                                    .textSelection(.enabled)
                            }
                            .id(entry.id)
                        }
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: detail.activity.count) { _ in
                    if query.isEmpty, let last = detail.activity.last {
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
struct TurnsPaneView: View {
    @ObservedObject var detail: JobDetailStore
    @State private var expanded: Set<Int> = []

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 4) {
                if let issue = detail.tracesIssue {
                    Text("traces tail stopped: \(issue)")
                        .font(Theme.body(10.5))
                        .foregroundStyle(Theme.danger)
                        .textSelection(.enabled)
                        .padding(4)
                }
                if detail.turns.isEmpty {
                    Text("No turns recorded yet.")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.inkTertiary)
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
                        .foregroundStyle(Theme.inkTertiary)
                    Text("turn \(turn.turn)")
                        .font(Theme.body(11.5, weight: .semibold))
                        .foregroundStyle(Theme.ink)
                    if let started = turn.startedAt {
                        Text(ActivityPaneView.time(started))
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.inkTertiary)
                    }
                    Spacer()
                    Text("in \(Self.tokens(turn.inputTokens)) out \(Self.tokens(turn.outputTokens))")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.inkTertiary)
                    if turn.costUSD > 0 {
                        Text(String(format: "$%.3f", turn.costUSD))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.inkSecondary)
                    }
                    Text("\(turn.entries.count) entries")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.inkTertiary)
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
                                .foregroundStyle(Theme.inkTertiary)
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
        case .toolCall: return Theme.ink
        case .toolResult, .docs, .trace, .other: return Theme.inkSecondary
        }
    }

    static func tokens(_ count: Int) -> String {
        count >= 1000 ? String(format: "%.1fk", Double(count) / 1000) : "\(count)"
    }
}

// MARK: - Timeline

/// Phase progression from PipelineState.phases plus event density per turn.
struct TimelinePaneView: View {
    @ObservedObject var detail: JobDetailStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if let state = detail.runState {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("PHASES")
                            .font(Theme.body(9, weight: .bold))
                            .kerning(0.5)
                            .foregroundStyle(Theme.inkTertiary)
                        ForEach(state.phases, id: \.id.raw) { phase in
                            HStack(spacing: 8) {
                                Circle()
                                    .fill(phaseColor(phase.status))
                                    .frame(width: 7, height: 7)
                                Text("\(phase.id.raw)")
                                    .font(Theme.mono(10))
                                    .foregroundStyle(Theme.inkTertiary)
                                    .frame(width: 24, alignment: .trailing)
                                Text(phase.name)
                                    .font(Theme.body(11.5,
                                        weight: phase.id.raw == state.currentPhaseID ? .semibold : .regular))
                                    .foregroundStyle(Theme.ink)
                                Text(phase.status)
                                    .font(Theme.body(10))
                                    .foregroundStyle(phaseColor(phase.status))
                                Spacer()
                                Text(ActivityPaneView.time(phase.updatedAt))
                                    .font(Theme.mono(9.5))
                                    .foregroundStyle(Theme.inkTertiary)
                            }
                        }
                    }
                } else {
                    Text("No pipeline state yet for this attempt.")
                        .font(Theme.body(12))
                        .foregroundStyle(Theme.inkTertiary)
                }

                densityBand
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// Event density: entries per turn as a quiet histogram, from the same
    /// grouping the Turns tab renders. Counts, not prose (P8).
    @ViewBuilder private var densityBand: some View {
        if !detail.turns.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("EVENT DENSITY \u{00B7} \(detail.activity.count) events")
                    .font(Theme.body(9, weight: .bold))
                    .kerning(0.5)
                    .foregroundStyle(Theme.inkTertiary)
                HStack(alignment: .bottom, spacing: 2) {
                    let maxCount = max(detail.turns.map { $0.entries.count }.max() ?? 1, 1)
                    ForEach(detail.turns.suffix(60)) { turn in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(Theme.accent.opacity(0.7))
                            .frame(width: 6,
                                   height: max(3, CGFloat(turn.entries.count) / CGFloat(maxCount) * 40))
                            .help("turn \(turn.turn): \(turn.entries.count) entries")
                    }
                }
            }
        }
    }

    private func phaseColor(_ status: String) -> Color {
        switch status {
        case "completed": return Theme.verified
        case "executing": return Theme.live
        case "failed": return Theme.danger
        case "planned": return Theme.accent
        default: return Theme.inkTertiary
        }
    }
}
