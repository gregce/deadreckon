import AppKit
import DeadreckonKit
import SwiftUI

// The one drill-down grammar (VIZ-DRILLDOWN-SPEC §G/§D): rows expand
// INLINE, fixed-band data opens anchored POPOVERS, jumps are mono accent
// text-buttons that navigate within the run surface. Drill content renders
// durable files and CLI envelopes verbatim (mono, selectable) — reading,
// never authority; nothing here adds a write path or re-derives a status.

// MARK: - §G1 DrillTarget (view state, not Kit)

/// A jump's destination inside the run surface. RunSurfaceView owns the
/// state; setting it switches the center tab and the receiving pane
/// consumes it (expand/scroll/brush) and clears it. Never a new window,
/// never a third pane.
enum DrillTarget: Equatable {
    /// Activity tab -> Turns mode, expand + scroll to the turn.
    case turn(Int)
    /// Changes tab, expand that path (triggers the existing lazy load).
    case changesFile(String)
    /// Checks tab, expand that recorded-check row.
    case recordedCheck(kind: String, command: String?)
    /// Activity tab -> Stream, set the brush window.
    case activityWindow(ClosedRange<Date>)
}

// MARK: - Shared atoms

/// Popover chrome (§G rule 2): panel fill, 1px border, radius 8, overlay
/// shadow, padding 12, max-width 380. The body is the same component the
/// inline expansion renders where both exist.
struct DrillPopoverChrome<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            content()
        }
        .padding(12)
        .frame(maxWidth: 380, alignment: .leading)
        .background(Theme.panel)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .strokeBorder(Theme.border, lineWidth: 1))
        .shadow(color: Theme.overlayShadow, radius: 24, y: 8)
    }
}

/// A jump line (§G rule 3): mono accent text-button with a trailing arrow.
/// Links are a lawful accent use.
struct JumpLine: View {
    let title: String
    var size: CGFloat = 10.5
    let action: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Text("\(title) \u{2192}")
                .font(Theme.mono(size))
                .foregroundStyle(Theme.accent)
                .underline(hovering)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .buttonStyle(.tactile)
        .onHover { hovering = $0 }
    }
}

/// A mono well: the home of verbatim machine truth inside a drill. Well
/// fill, 1px border, radius 6, selectable, horizontal scroll; optional
/// vertical cap with scroll.
struct MonoWell: View {
    let text: String
    var label: String?
    var maxHeight: CGFloat?
    var copyButton = false

    /// The scroll viewport's width: content narrower than the viewport gets
    /// a leading-aligned frame at least this wide — a scroll view otherwise
    /// centers undersized content, and quoted evidence must read as a
    /// left-aligned document.
    @State private var viewportWidth: CGFloat = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if label != nil || copyButton {
                HStack(spacing: 6) {
                    if let label {
                        Text(label)
                            .font(Theme.body(8.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    if copyButton {
                        CopyTextButton(text: text)
                    }
                }
            }
            Group {
                if let maxHeight {
                    ScrollView([.vertical, .horizontal]) {
                        wellText
                    }
                    .frame(maxHeight: maxHeight)
                } else {
                    ScrollView(.horizontal) {
                        wellText
                    }
                }
            }
            .background(GeometryReader { geometry in
                Color.clear
                    .onAppear { viewportWidth = geometry.size.width }
                    .onChange(of: geometry.size.width) { _, width in
                        viewportWidth = width
                    }
            })
            .padding(6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.well, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))
        }
    }

    private var wellText: some View {
        Text(text)
            .font(Theme.monoS)
            .foregroundStyle(Theme.textSecondary)
            .textSelection(.enabled)
            .frame(minWidth: max(viewportWidth, 0), alignment: .leading)
    }
}

/// A small `copy` text-button (NSPasteboard) with a moment of confirmation.
struct CopyTextButton: View {
    let text: String
    var label = "copy"

    @State private var copied = false

    var body: some View {
        Button(copied ? "copied" : label) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            copied = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { copied = false }
        }
        .buttonStyle(.themeText(size: 9))
    }
}

// MARK: - D3: spend point -> that turn's cost breakdown (popover body)

struct SpendPointDetail: View {
    let point: SpendSeries.Point
    let onOpenTurn: (Int) -> Void

    var body: some View {
        DrillPopoverChrome {
            Text("turn \(point.turn) \u{00B7} \(RunChartTime.hms(point.timestamp))")
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textPrimary)
            Text(String(format: "$%.3f this turn \u{00B7} $%.2f total",
                        point.deltaUSD, point.totalUSD))
                .font(Theme.mono(10.5))
                .monospacedDigit()
                .foregroundStyle(Theme.textPrimary)
            Text("in \(formatted(point.inputTokens)) \u{00B7} out \(formatted(point.outputTokens)) tokens")
                .font(Theme.mono(10))
                .monospacedDigit()
                .foregroundStyle(Theme.textSecondary)
            Text("\(point.model) \u{00B7} \(point.provider)")
                .font(Theme.mono(10))
                .foregroundStyle(Theme.textSecondary)
                .textSelection(.enabled)
            HStack(spacing: 4) {
                if let wall = point.wallSeconds {
                    Text(String(format: "wall %.1fs", wall))
                        .font(Theme.mono(10))
                        .monospacedDigit()
                        .foregroundStyle(Theme.textSecondary)
                }
                if point.subscription {
                    Text("\u{00B7} subscription")
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.textSecondary)
                }
                if point.estimated {
                    // A flagged degradation of fact quality, per the schema.
                    Text("\u{00B7} estimated")
                        .font(Theme.mono(10))
                        .foregroundStyle(Theme.warn)
                }
            }
            JumpLine(title: "open turn \(point.turn)") { onOpenTurn(point.turn) }
        }
    }

    private func formatted(_ count: Int) -> String {
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        return formatter.string(from: NSNumber(value: count)) ?? "\(count)"
    }
}

// MARK: - D4: event row -> raw record inspector (inline body)

/// The verbatim events.jsonl line behind one Activity row. The pretty form
/// is a view; the line is the fact — `raw` shows the untouched bytes and
/// copy copies them.
struct EventRawInspector: View {
    let timestamp: Date?
    let raw: String?

    @State private var showRawLine = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let raw {
                HStack(spacing: 6) {
                    if let kind = Self.kindWord(raw) {
                        Text("kind \(kind)")
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textPrimary)
                    }
                    if let timestamp {
                        Text(RunChartTime.hms(timestamp))
                            .font(Theme.mono(9.5))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Spacer()
                    CopyTextButton(text: raw)
                    Button(showRawLine ? "pretty" : "raw") { showRawLine.toggle() }
                        .buttonStyle(.themeText(size: 9))
                }
                MonoWell(text: showRawLine ? raw : (Self.prettyPrinted(raw) ?? raw),
                         maxHeight: 180)
            } else {
                Text("raw line no longer in memory \u{2014} full ledger in events.jsonl")
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.textTertiary)
                Text("CONSOLE \u{2192} Raw events keeps the last \(JobDetailStore.rawEventLineCeiling) lines; the file on disk is the durable record.")
                    .font(Theme.body(9))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .padding(.top, 2)
    }

    /// Re-indent of the SAME bytes for reading; never a rewrite.
    static func prettyPrinted(_ line: String) -> String? {
        guard let data = line.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let pretty = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted]),
              let text = String(data: pretty, encoding: .utf8) else { return nil }
        return text
    }

    static func kindWord(_ line: String) -> String? {
        guard let data = line.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let event = object["event"] as? [String: Any] else { return nil }
        return event["kind"] as? String
    }
}

// MARK: - D1 level 2: trace entry -> full tool I/O (inline body)

/// The full tool exchange behind one trace entry, decoded on expansion from
/// the retained raw line (TraceDetailDoc, K11). Verbatim commands and
/// outputs in mono wells; changed paths jump into Changes.
struct ToolIOView: View {
    let raw: String?
    /// The on-disk ledger named when the raw line is past the ceiling.
    let ledgerPath: String
    var onOpenChangesFile: ((String) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let raw {
                if let doc = TraceDetailDoc.decode(rawTraceLine: raw), !doc.isEmpty {
                    decoded(doc)
                } else {
                    // Undecodable — or decodable but carrying nothing this
                    // drill can render: the raw line itself, verbatim.
                    // Never a guess, never an empty block.
                    MonoWell(text: raw, label: "raw trace line", maxHeight: 160, copyButton: true)
                }
            } else {
                Text("raw line no longer in memory \u{2014} the full ledger is on disk")
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.textTertiary)
                Text(ledgerPath)
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.top, 2)
    }

    @ViewBuilder private func decoded(_ doc: TraceDetailDoc) -> some View {
        factLine(doc)
        if let warning = doc.sandboxWarning, !warning.isEmpty {
            Text(warning)
                .font(Theme.mono(9.5))
                .foregroundStyle(Theme.warn)
                .textSelection(.enabled)
        }
        ForEach(doc.flightRows) { row in
            flightRow(row)
        }
        if doc.flightRows.isEmpty, let prompt = doc.promptArg {
            MonoWell(text: prompt, label: "prompt", maxHeight: 160)
        }
        let changed = doc.flightRows.flatMap(\.changedPaths)
        if !changed.isEmpty, let onOpenChangesFile {
            HStack(alignment: .top, spacing: 6) {
                Text("changes:")
                    .font(Theme.body(9.5))
                    .foregroundStyle(Theme.textTertiary)
                VStack(alignment: .leading, spacing: 1) {
                    ForEach(Array(Set(changed)).sorted(), id: \.self) { path in
                        JumpLine(title: path, size: 9.5) { onOpenChangesFile(path) }
                    }
                }
            }
        }
    }

    private func factLine(_ doc: TraceDetailDoc) -> some View {
        // `codex · gpt-5.6-sol · 50.7s · exit 0 · sandboxed: sandbox-exec · read-write`
        var parts: [String] = []
        if let binary = doc.binary { parts.append(binary) }
        if let model = doc.model { parts.append(model) }
        if let duration = doc.durationMS {
            parts.append(String(format: "%.1fs", Double(duration) / 1000))
        }
        if let exit = doc.exitCode { parts.append("exit \(exit)") }
        if let sandbox = doc.sandboxBackend { parts.append("sandboxed: \(sandbox)") }
        if let access = doc.workspaceAccess { parts.append(access) }
        let failed = (doc.exitCode ?? 0) != 0
        return Text(parts.joined(separator: " \u{00B7} "))
            .font(Theme.mono(10))
            .foregroundStyle(failed ? Theme.dangerText : Theme.textPrimary)
            .textSelection(.enabled)
    }

    @ViewBuilder private func flightRow(_ row: TraceDetailDoc.FlightRow) -> some View {
        // Failed = the recorded status word, or a recorded nonzero exit code
        // (the loop's tool shape carries no status word — the exit code is
        // the recorded fact).
        let failed = row.status == "failed" || (row.exitCode ?? 0) != 0
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 5) {
                Text(failed ? "\u{2717}" : "\u{2713}")
                    .font(Theme.body(10, weight: .bold))
                    .foregroundStyle(failed ? Theme.danger : Theme.success)
                Text(row.toolName ?? row.toolCategory ?? "tool")
                    .font(Theme.mono(10))
                    .foregroundStyle(Theme.textPrimary)
                if let exit = row.exitCode, exit != 0 {
                    Text("exit \(exit)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.dangerText)
                }
            }
            if let command = row.command {
                MonoWell(text: command, label: "command", copyButton: true)
            } else if let summary = row.summary, !summary.isEmpty {
                Text(summary)
                    .font(Theme.monoS)
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(2)
                    .textSelection(.enabled)
            }
            if let output = row.aggregatedOutput, !output.isEmpty {
                MonoWell(text: output, label: "output", maxHeight: 160, copyButton: true)
            }
            if let stderr = row.stderrOutput, !stderr.isEmpty {
                MonoWell(text: stderr, label: "stderr", maxHeight: 160, copyButton: true)
            }
        }
        .padding(.leading, 2)
    }
}

// MARK: - D2: check row -> full evidence (inline body; absorbs CheckResultDetail)

/// Everything the gate recorded for one check: the exact command in a mono
/// well, cwd, duration, must-pass, outputs with copy, and (when attempts
/// exist) the per-attempt history matched on the exact (kind, command, cwd)
/// triple — `not recorded` where true, absence stated, never inferred.
struct CheckEvidenceView: View {
    let result: AcceptanceProgressRow.CheckResult
    var attempts: [JobReportEnvelope.Attempt] = []
    /// History renders only at depth 0 (recursion depth 1 per spec).
    var depth = 0

    @State private var expandedAttempts: Set<Int> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            factsRow
            if !result.detail.isEmpty {
                Text(result.detail)
                    .font(Theme.body(9.5))
                    .foregroundStyle(result.passed ? Theme.textTertiary : Theme.dangerText)
                    .textSelection(.enabled)
            }
            if let stdout = result.stdout, !stdout.isEmpty {
                MonoWell(text: stdout, label: "stdout (clipped when recorded)",
                         maxHeight: 120, copyButton: true)
            }
            if let stderr = result.stderr, !stderr.isEmpty {
                MonoWell(text: stderr, label: "stderr (clipped when recorded)",
                         maxHeight: 120, copyButton: true)
            }
            if depth == 0, !attempts.isEmpty {
                history
            }
        }
    }

    @ViewBuilder private var factsRow: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let command = result.command, !command.isEmpty {
                MonoWell(text: command, label: "command", copyButton: true)
            }
            HStack(spacing: 6) {
                if let cwd = result.cwd, !cwd.isEmpty {
                    Text("cwd \(cwd)")
                        .font(Theme.monoS)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
                if let duration = result.durationMS {
                    Text(String(format: "%.1fs", Double(duration) / 1000))
                        .font(Theme.mono(9.5))
                        .monospacedDigit()
                        .foregroundStyle(Theme.textTertiary)
                }
                if result.mustPass {
                    StatusChip(text: "must pass", color: Theme.textSecondary)
                }
            }
        }
    }

    /// Newest-first attempts for THIS check identity; each expands to its
    /// own recorded outputs (same component, depth 1).
    @ViewBuilder private var history: some View {
        let key = CheckDurations.CheckKey(
            kind: result.kind, command: result.command, cwd: result.cwd)
        let rows = CheckDurations.history(for: key, attempts: attempts)
        if rows.count > 1 || (rows.count == 1 && rows[0].result == nil) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text("attempts:")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    ForEach(rows, id: \.attemptIndex) { row in
                        attemptChip(row)
                    }
                }
                ForEach(rows, id: \.attemptIndex) { row in
                    if expandedAttempts.contains(row.attemptIndex), let recorded = row.result {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("attempt \(row.attemptIndex)")
                                .font(Theme.mono(9))
                                .foregroundStyle(Theme.textTertiary)
                            CheckEvidenceView(result: recorded, depth: 1)
                        }
                        .padding(.leading, 10)
                    }
                }
            }
        }
    }

    @ViewBuilder private func attemptChip(
        _ row: (attemptIndex: Int, result: AcceptanceProgressRow.CheckResult?)
    ) -> some View {
        if let recorded = row.result {
            Button {
                if expandedAttempts.contains(row.attemptIndex) {
                    expandedAttempts.remove(row.attemptIndex)
                } else {
                    expandedAttempts.insert(row.attemptIndex)
                }
            } label: {
                HStack(spacing: 3) {
                    Text("#\(row.attemptIndex)")
                        .font(Theme.mono(9.5))
                        .foregroundStyle(Theme.textSecondary)
                    Text(recorded.passed ? "\u{2713}" : "\u{2717}")
                        .font(Theme.body(9.5, weight: .bold))
                        .foregroundStyle(recorded.passed ? Theme.success : Theme.danger)
                    if let duration = recorded.durationMS {
                        Text(String(format: "%.1fs", Double(duration) / 1000))
                            .font(Theme.mono(9))
                            .monospacedDigit()
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Text(expandedAttempts.contains(row.attemptIndex) ? "\u{25BE}" : "\u{25B8}")
                        .font(Theme.body(8))
                        .foregroundStyle(Theme.textTertiary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } else {
            Text("#\(row.attemptIndex) \u{00B7} not recorded")
                .font(Theme.mono(9))
                .foregroundStyle(Theme.textTertiary)
        }
    }
}

// MARK: - D5: plan phase -> timestamps + duration (popover body)

struct PhaseDetail: View {
    let name: String
    let status: String
    let updatedAt: Date?
    let mark: PhaseDurations.Mark
    var onViewActivity: (() -> Void)?

    var body: some View {
        DrillPopoverChrome {
            Text(name)
                .font(Theme.body(12.5, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            Text("status \(status)")
                .font(Theme.mono(10.5))
                .foregroundStyle(Theme.textSecondary)
            if let updatedAt {
                Text("updated \(Self.absolute(updatedAt))")
                    .font(Theme.mono(10.5))
                    .monospacedDigit()
                    .foregroundStyle(Theme.textSecondary)
            }
            switch mark {
            case .completed(let seconds):
                elapsedLine(seconds)
            case .current(let seconds):
                elapsedLine(seconds)
            case .none:
                EmptyView()
            }
            if let onViewActivity {
                JumpLine(title: "view activity in this window") { onViewActivity() }
            }
        }
    }

    private func elapsedLine(_ seconds: Double) -> some View {
        Text("elapsed \(PlanBandView.durationWords(seconds)) \u{2014} derived from status-change stamps, not a recorded duration")
            .font(Theme.body(9.5))
            .foregroundStyle(Theme.textTertiary)
    }

    private static let formatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter
    }()

    static func absolute(_ date: Date) -> String {
        formatter.string(from: date)
    }
}

// MARK: - D7: graph node -> files/components detail (popover body)

struct GraphNodeDetail: View {
    let node: ArchitectureGraphDoc.Node
    /// Set when this file node's label matched a path in the loaded diff.
    var diffPath: String?
    var onOpenChanges: ((String) -> Void)?

    var body: some View {
        DrillPopoverChrome {
            Text(node.id)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.textPrimary)
                .textSelection(.enabled)
            Text("\(node.kind) \u{00B7} \(node.status)")
                .font(Theme.mono(10))
                .foregroundStyle(Theme.textSecondary)
            Text("weight \(node.weight)")
                .font(Theme.mono(10))
                .monospacedDigit()
                .foregroundStyle(Theme.textTertiary)
            if case .unknown(let raw) = GraphStyleToken(raw: node.styleToken) {
                Text("style_token \(raw)")
                    .font(Theme.mono(9.5))
                    .foregroundStyle(Theme.textTertiary)
            }
            if !node.evidence.isEmpty {
                VStack(alignment: .leading, spacing: 1) {
                    Text("evidence:")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.textTertiary)
                    ForEach(node.evidence, id: \.self) { line in
                        Text(line)
                            .font(Theme.monoS)
                            .foregroundStyle(Theme.textSecondary)
                            .textSelection(.enabled)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
            }
            if let diffPath, let onOpenChanges {
                JumpLine(title: "open in Changes") { onOpenChanges(diffPath) }
            }
        }
    }
}
