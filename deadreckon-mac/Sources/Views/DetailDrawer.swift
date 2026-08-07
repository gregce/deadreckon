import DeadreckonKit
import SwiftUI

/// The bottom CONSOLE (P5): Terminal (supervisor.out/err plain-text tails) |
/// Raw events | Run log (partial-line badge + integrity chip). Execution
/// surfaces live adjacent to evidence, not in a separate window.
struct DetailDrawerView: View {
    enum Tab: String, CaseIterable {
        case terminal = "Terminal"
        case rawEvents = "Raw events"
        case jobEvents = "Run log"
    }

    @ObservedObject var detail: JobDetailStore
    @Binding var shown: Bool
    @State private var tab: Tab = .terminal

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                Button {
                    shown.toggle()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: shown ? "chevron.down" : "chevron.up")
                            .font(.system(size: 9, weight: .semibold))
                        Theme.sectionTitle("CONSOLE")
                    }
                    .foregroundStyle(Theme.textTertiary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.tactile)

                if shown {
                    ForEach(Tab.allCases, id: \.self) { candidate in
                        TabButton(title: candidate.rawValue, active: tab == candidate) {
                            tab = candidate
                        }
                    }
                }
                Spacer()
                integrityChip
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)

            if shown {
                Divider().overlay(Theme.border)
                content
                    .frame(height: 180)
            }
        }
        .background(Theme.panel)
    }

    /// The visible integrity claim over job-events.jsonl: contiguity or the
    /// tailer's own failure words, plus the partial-line badge. Always
    /// shown, console open or not. Display words from Lexicon; the Kit's
    /// own labels stay byte-identical for tests.
    @ViewBuilder private var integrityChip: some View {
        HStack(spacing: 5) {
            if detail.jobEventsTornTail {
                StatusChip(text: "partial line", color: Theme.textTertiary)
                    .help("An unterminated final line, kept and retried \u{2014} an in-flight write, not corruption.")
            }
            switch detail.integrity {
            case .none:
                StatusChip(text: Lexicon.integrityWord(detail.integrity), color: Theme.textTertiary)
            case .contiguous:
                StatusChip(text: Lexicon.integrityWord(detail.integrity), color: Theme.success)
                    .help("Sequence 1..N verified continuously against job-events.jsonl.")
            case .corrupt(let reason):
                StatusChip(text: Lexicon.integrityWord(detail.integrity), color: Theme.danger,
                           strong: true, textColor: Theme.dangerText)
                    .help("The strict ledger failed verification; showing unknown, never a guess. \(reason)")
            }
        }
    }

    @ViewBuilder private var content: some View {
        switch tab {
        case .terminal:
            HStack(spacing: 0) {
                terminalPane(title: "supervisor.out", text: detail.supervisorOut,
                             truncated: detail.supervisorOutTruncated)
                Divider().overlay(Theme.border)
                terminalPane(title: "supervisor.err", text: detail.supervisorErr,
                             truncated: detail.supervisorErrTruncated)
            }
        case .rawEvents:
            rawEventsPane
        case .jobEvents:
            jobEventsPane
        }
    }

    private func terminalPane(title: String, text: String, truncated: Bool) -> some View {
        // Rendered as line rows in a LazyVStack (the raw-events pane's
        // pattern), not one 256 KB Text: SwiftUI re-laying out a
        // quarter-megabyte Text on every 2 s append stutters the drawer
        // with a chatty supervisor. The store trims at line boundaries, so
        // splitting is cheap; head-trims are rare (only at the ceiling), so
        // index identity stays stable across ordinary appends.
        let lines = text.isEmpty
            ? [] : text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(title)
                    .font(Theme.mono(9))
                    .foregroundStyle(Theme.textTertiary)
                if truncated {
                    // The documented ceiling (CONTRACTS.md): the pane keeps
                    // the trailing ~256 KB; the full file stays on disk.
                    Text("older output dropped; showing the last 256 KB (full file on disk)")
                        .font(Theme.body(8.5))
                        .foregroundStyle(Theme.warn)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        if lines.isEmpty {
                            Text("(empty)")
                                .font(Theme.mono(9.5))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
                            Text(line.isEmpty ? " " : line)
                                .font(Theme.mono(9.5))
                                .foregroundStyle(Theme.textSecondary)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .id(index)
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: text.count) { _ in
                    // Tail-convention always-follow (terminal semantics; the
                    // Activity pane is the pinned-aware reading surface).
                    // `lines` is the latest body pass's split of this text.
                    if !lines.isEmpty { proxy.scrollTo(lines.count - 1, anchor: .bottom) }
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var rawEventsPane: some View {
        // Row identity is the stable ledger ordinal (dropped + index), so
        // the bounded window sliding forward does not re-identify rows.
        let base = detail.rawEventsDropped
        return ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    if base > 0 {
                        // The documented ceiling (CONTRACTS.md): raw lines
                        // are a bounded drawer window; the parsed Activity
                        // scrollback stays unbounded and the full raw
                        // history stays in events.jsonl.
                        Text("\(base) older raw lines dropped; showing the last \(detail.rawEventLines.count) (full ledger in events.jsonl)")
                            .font(Theme.body(9))
                            .foregroundStyle(Theme.warn)
                    }
                    ForEach(Array(detail.rawEventLines.enumerated()), id: \.offset) { index, line in
                        Text(line)
                            .font(Theme.mono(9))
                            .foregroundStyle(Theme.textSecondary)
                            .textSelection(.enabled)
                            .lineLimit(1)
                            .id(base + index)
                    }
                    if detail.rawEventLines.isEmpty {
                        Text("(no events yet)")
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onChange(of: detail.rawEventLines.count) { count in
                if count > 0 { proxy.scrollTo(detail.rawEventsDropped + count - 1, anchor: .bottom) }
            }
        }
    }

    private var jobEventsPane: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                integrityChip
                if case .contiguous(let count) = detail.integrity {
                    Text("last sequence \(count)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            Text("The run\u{2019}s strict lifecycle ledger (`job-events.jsonl`): sequence 1..N, no gaps. The app verifies it continuously; a gap shows as a failure, never a guess.")
                .font(Theme.body(10))
                .foregroundStyle(Theme.textTertiary)
            if let issue = detail.projectionIssue {
                Text(issue)
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
            if let projection = detail.projection {
                Text("projection: phase \(projection.phase.rawValue) \u{00B7} last_sequence \(projection.lastSequence) \u{00B7} lease epoch \(projection.currentLeaseEpoch) \u{00B7} attempts \(projection.attemptCount)")
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
                ForEach(projection.caveats, id: \.self) { caveat in
                    Text("caveat: \(caveat)")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
            }
            if let lease = detail.lease {
                // The full lease.json checkpoint as evidence facts (owner,
                // epoch, pids, heartbeat). The FRESHNESS verdict stays with
                // the rollup row + FleetStore's confirmed-stale debounce in
                // the header; this line never re-derives it.
                Text("lease: owner \(lease.ownerID) \u{00B7} epoch \(lease.epoch) \u{00B7} pid \(lease.pid) \u{00B7} heartbeat \(ActivityPaneView.time(lease.heartbeatAt)) \u{00B7} expires \(ActivityPaneView.time(lease.expiresAt))")
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }
            Spacer()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
}
