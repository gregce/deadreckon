import DeadreckonKit
import SwiftUI

/// The bottom DRAWER (P5): Terminal (supervisor.out/err plain-text tails) |
/// Raw events | Job events (torn-tail badge + integrity chip). Execution
/// surfaces live adjacent to evidence, not in a separate window.
struct DetailDrawerView: View {
    enum Tab: String, CaseIterable {
        case terminal = "Terminal"
        case rawEvents = "Raw events"
        case jobEvents = "Job events"
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
                        Text("DRAWER")
                            .font(Theme.body(9.5, weight: .bold))
                            .kerning(0.6)
                    }
                    .foregroundStyle(Theme.inkTertiary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.tactile)

                if shown {
                    ForEach(Tab.allCases, id: \.self) { candidate in
                        Button {
                            tab = candidate
                        } label: {
                            Text(candidate.rawValue)
                                .font(Theme.body(10.5, weight: tab == candidate ? .semibold : .regular))
                                .foregroundStyle(tab == candidate ? Theme.ink : Theme.inkSecondary)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 4)
                                .background(
                                    tab == candidate ? Theme.card : .clear,
                                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        }
                        .buttonStyle(.tactile)
                    }
                }
                Spacer()
                integrityChip
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)

            if shown {
                Divider().overlay(Theme.hairline)
                content
                    .frame(height: 180)
            }
        }
        .background(Theme.card)
    }

    /// The visible integrity claim over job-events.jsonl: contiguity or the
    /// tailer's own failure words, plus the torn-tail badge. Always shown,
    /// drawer open or not.
    @ViewBuilder private var integrityChip: some View {
        HStack(spacing: 5) {
            if detail.jobEventsTornTail {
                StatusChip(text: "torn tail", color: Theme.inkTertiary)
                    .help("Unterminated final line retained and retried; an in-flight append, never corruption (TAILING.md).")
            }
            switch detail.integrity {
            case .none:
                StatusChip(text: detail.integrity.label, color: Theme.inkTertiary)
            case .contiguous:
                StatusChip(text: detail.integrity.label, color: Theme.verified)
                    .help("Strict sequence 1..N verified continuously against job-events.jsonl.")
            case .corrupt:
                StatusChip(text: detail.integrity.label, color: Theme.danger, filled: true)
                    .help("The strict ledger failed verification; rendering unknown, never a guessed state.")
            }
        }
    }

    @ViewBuilder private var content: some View {
        switch tab {
        case .terminal:
            HStack(spacing: 0) {
                terminalPane(title: "supervisor.out", text: detail.supervisorOut,
                             truncated: detail.supervisorOutTruncated)
                Divider().overlay(Theme.hairline)
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
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(title)
                    .font(Theme.mono(9))
                    .foregroundStyle(Theme.inkTertiary)
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
                    Text(text.isEmpty ? "(empty)" : text)
                        .font(Theme.mono(9.5))
                        .foregroundStyle(text.isEmpty ? Theme.inkTertiary : Theme.inkSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.bottom, 8)
                        .id("tail")
                }
                .onChange(of: text.count) { _ in
                    proxy.scrollTo("tail", anchor: .bottom)
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
                            .foregroundStyle(Theme.inkSecondary)
                            .textSelection(.enabled)
                            .lineLimit(1)
                            .id(base + index)
                    }
                    if detail.rawEventLines.isEmpty {
                        Text("(no events yet)")
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.inkTertiary)
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
                        .foregroundStyle(Theme.inkTertiary)
                }
            }
            Text("The strictly-sequenced Job lifecycle ledger (job-events.jsonl): sequence 1..N with no gaps, fsynced before the projection checkpoint. This chip is the app continuously verifying that contract; a gap renders the failure, never a guessed state.")
                .font(Theme.body(10))
                .foregroundStyle(Theme.inkTertiary)
            if let issue = detail.projectionIssue {
                Text(issue)
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
            if let projection = detail.projection {
                Text("projection: phase \(projection.phase.rawValue) \u{00B7} last_sequence \(projection.lastSequence) \u{00B7} lease epoch \(projection.currentLeaseEpoch) \u{00B7} attempts \(projection.attemptCount)")
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.inkSecondary)
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
                    .foregroundStyle(Theme.inkSecondary)
                    .textSelection(.enabled)
            }
            Spacer()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
}
