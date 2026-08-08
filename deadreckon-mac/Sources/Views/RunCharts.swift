import Charts
import DeadreckonKit
import SwiftUI

// The plotted charts of the goal-run surfaces (VIZ-DRILLDOWN-SPEC §V):
// Swift Charts only, quiet instruments in the committed palette
// (Theme.Chart, §V0). Every series/bin/scale is a Kit derivation — these
// views map published values to marks and never compute. Charts render
// recorded rows only: no smoothing past the data, no extension to "now",
// no chart animation. `import Charts` lives in this file only.

// MARK: - V1: the run-header spend burn strip (140×18)

/// The shape of the burn against the cap — the one thing the printed
/// "$4.12 of $25.00" cannot say. Step interpolation: spend is a step
/// function between records; a smooth curve would invent spend.
struct SpendBurnStrip: View {
    let series: SpendSeries
    let live: Bool
    let onOpenTurn: (Int) -> Void

    @State private var hoverDate: Date?
    @State private var pinned: SpendSeries.Point?

    var body: some View {
        chart
            .frame(width: 140, height: 18)
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartXScale(domain: xDomain)
            .chartYScale(domain: 0 ... yMax)
            .chartXSelection(value: $hoverDate)
            .onTapGesture {
                if let point = nearestPoint { pinned = point }
            }
            .popover(item: $pinned, arrowEdge: .bottom) { point in
                SpendPointDetail(point: point) { turn in
                    pinned = nil
                    onOpenTurn(turn)
                }
                .presentationBackground(Theme.panel)
            }
            .overlay(alignment: .topTrailing) { hoverTip }
            .accessibilityLabel(summary)
    }

    private var chart: some View {
        Chart {
            ForEach(series.points) { point in
                AreaMark(x: .value("time", point.timestamp),
                         y: .value("spend", point.totalUSD))
                    .interpolationMethod(.stepEnd)
                    .foregroundStyle(Theme.Chart.markFill)
            }
            ForEach(series.points) { point in
                LineMark(x: .value("time", point.timestamp),
                         y: .value("spend", point.totalUSD))
                    .interpolationMethod(.stepEnd)
                    .lineStyle(StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                    .foregroundStyle(Theme.Chart.markLine)
            }
            if let cap = series.capUSD {
                RuleMark(y: .value("cap", cap))
                    .lineStyle(StrokeStyle(lineWidth: 1))
                    .foregroundStyle(Theme.Chart.capRule)
            }
            if let last = series.points.last {
                // End-dot with a 2px surface ring; accent ONLY while live.
                PointMark(x: .value("time", last.timestamp),
                          y: .value("spend", last.totalUSD))
                    .symbolSize(50)
                    .foregroundStyle(Theme.windowBg)
                PointMark(x: .value("time", last.timestamp),
                          y: .value("spend", last.totalUSD))
                    .symbolSize(14)
                    .foregroundStyle(live ? Theme.Chart.liveDatum : Theme.Chart.markQuiet)
            }
        }
    }

    /// First…last recorded point — never extended to "now" (law 3).
    private var xDomain: ClosedRange<Date> {
        let first = series.points.first?.timestamp ?? Date(timeIntervalSince1970: 0)
        let last = series.points.last?.timestamp ?? first
        return first ... max(last, first.addingTimeInterval(0.001))
    }

    private var yMax: Double {
        max(series.capUSD ?? 0, series.maxTotalUSD, 0.01)
    }

    private var nearestPoint: SpendSeries.Point? {
        guard let hoverDate else { return series.points.last }
        return series.points.min {
            abs($0.timestamp.timeIntervalSince(hoverDate))
                < abs($1.timestamp.timeIntervalSince(hoverDate))
        }
    }

    @ViewBuilder private var hoverTip: some View {
        if pinned == nil, hoverDate != nil, let point = nearestPoint {
            ChartTooltipChip(text: String(
                format: "turn %d \u{00B7} $%.2f \u{00B7} +$%.3f",
                point.turn, point.totalUSD, point.deltaUSD))
                .offset(y: -20)
                .allowsHitTesting(false)
        }
    }

    private var summary: String {
        let cap = series.capUSD.map { String(format: " of $%.2f", $0) } ?? ""
        let turns = series.points.last?.turn ?? 0
        return String(format: "Spend $%.2f%@ over %d turns", series.maxTotalUSD, cap, turns)
    }
}

/// The tiny floating value chip charts show on hover (values are also
/// always printed elsewhere — tooltips enhance, never gate).
struct ChartTooltipChip: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Theme.mono(9.5))
            .monospacedDigit()
            .foregroundStyle(Theme.textPrimary)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1))
            .fixedSize()
    }
}

// MARK: - V2: the Activity event-density strip (brushable)

/// Event density over the run's whole recorded span. Bins when the data
/// earns them (sparse law), honest ticks otherwise; error events are
/// full-height red rules; turn starts are recessive bottom ticks. Dragging
/// brushes a time window that filters the Stream.
struct ActivityDensityStrip: View {
    let series: DensitySeries
    let live: Bool
    @Binding var brush: ClosedRange<Date>?

    @State private var hoverDate: Date?

    var body: some View {
        switch series.presentation {
        case .absent:
            EmptyView()
        case .ticks(let events, let errors):
            strip(marks: { ticksContent(events: events, errors: errors) },
                  yTop: 1, hoverText: tickHoverText(events: events, errors: errors))
        case .bins(let bins, let width):
            strip(marks: { binsContent(bins: bins, width: width) },
                  yTop: Double(max(bins.map(\.count).max() ?? 1, 1)),
                  hoverText: binHoverText(bins: bins, width: width))
        }
    }

    private func strip(@ChartContentBuilder marks: () -> some ChartContent,
                       yTop: Double, hoverText: String?) -> some View {
        VStack(spacing: 2) {
            Chart {
                marks()
            }
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartXScale(domain: xDomain)
            .chartYScale(domain: 0 ... yTop)
            // Hover and the drag-brush are one explicit overlay layer:
            // attaching BOTH chartXSelection(value:) and (range:) leaves the
            // range gesture dead (the value selection wins the drag), so the
            // brush is a plain DragGesture translated through the proxy.
            .chartOverlay { proxy in
                interactionLayer(proxy: proxy)
            }
            .frame(height: 24)
            .frame(maxWidth: .infinity)

            captionRow(hoverText: hoverText)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .accessibilityLabel(
            "\(series.eventCount) events, \(series.errorStamps.count) errors over the recorded span")
    }

    @ChartContentBuilder
    private func binsContent(bins: [DensitySeries.Bin], width: TimeInterval) -> some ChartContent {
        let gap = width * 0.08   // the 2px surface gap, in time units
        ForEach(bins) { bin in
            BarMark(
                xStart: .value("start", bin.start.addingTimeInterval(gap)),
                xEnd: .value("end", bin.start.addingTimeInterval(width - gap)),
                y: .value("events", bin.count))
                .foregroundStyle(
                    live && bin.id == bins.last?.id
                        ? Theme.Chart.liveDatum : Theme.Chart.markQuiet)
        }
        let top = Double(max(bins.map(\.count).max() ?? 1, 1))
        ForEach(Array(series.turnStamps.enumerated()), id: \.offset) { _, stamp in
            RuleMark(x: .value("turn", stamp),
                     yStart: .value("y", 0), yEnd: .value("y", top * 0.25))
                .lineStyle(StrokeStyle(lineWidth: 1))
                .foregroundStyle(Theme.Chart.gridline)
        }
        ForEach(Array(series.errorStamps.enumerated()), id: \.offset) { _, stamp in
            RuleMark(x: .value("error", stamp))
                .lineStyle(StrokeStyle(lineWidth: 2))
                .foregroundStyle(Theme.Chart.fail)
        }
    }

    @ChartContentBuilder
    private func ticksContent(events: [Date], errors: [Date]) -> some ChartContent {
        // Tick mode: "events happened here", nothing more — no y encoding.
        let errorSet = Set(errors)
        ForEach(Array(events.enumerated()), id: \.offset) { _, stamp in
            RuleMark(x: .value("event", stamp))
                .lineStyle(StrokeStyle(lineWidth: 2))
                .foregroundStyle(errorSet.contains(stamp) ? Theme.Chart.fail : Theme.Chart.markQuiet)
        }
        ForEach(Array(series.turnStamps.enumerated()), id: \.offset) { _, stamp in
            RuleMark(x: .value("turn", stamp),
                     yStart: .value("y", 0), yEnd: .value("y", 0.25))
                .lineStyle(StrokeStyle(lineWidth: 1))
                .foregroundStyle(Theme.Chart.gridline)
        }
    }

    /// One overlay layer over the plot: the hover tracker + drag-brush
    /// surface underneath, the brush band (fill `well`, 1px `borderHover`
    /// edges) drawn above it from the same binding the filter reads.
    @ViewBuilder private func interactionLayer(proxy: ChartProxy) -> some View {
        GeometryReader { geometry in
            if let plotFrame = proxy.plotFrame.map({ geometry[$0] }) {
                Rectangle()
                    .fill(Color.clear)
                    .frame(width: plotFrame.width, height: plotFrame.height)
                    .offset(x: plotFrame.minX, y: plotFrame.minY)
                    .contentShape(Rectangle())
                    .onContinuousHover { phase in
                        switch phase {
                        case .active(let point):
                            hoverDate = proxy.value(atX: point.x, as: Date.self)
                        case .ended:
                            hoverDate = nil
                        }
                    }
                    // Click without drag clears the window (§V2).
                    .onTapGesture { brush = nil }
                    .gesture(DragGesture(minimumDistance: 4)
                        .onChanged { value in
                            setBrush(from: value, plotWidth: plotFrame.width, proxy: proxy)
                        })
                if let brush,
                   let lower = proxy.position(forX: brush.lowerBound),
                   let upper = proxy.position(forX: brush.upperBound) {
                    let x0 = min(lower, upper) + plotFrame.minX
                    let x1 = max(lower, upper) + plotFrame.minX
                    Rectangle()
                        .fill(Theme.Chart.brushFill.opacity(0.55))
                        .frame(width: max(x1 - x0, 1), height: plotFrame.height)
                        .offset(x: x0, y: plotFrame.minY)
                        .allowsHitTesting(false)
                    ForEach([x0, x1], id: \.self) { edge in
                        Rectangle()
                            .fill(Theme.Chart.brushEdge)
                            .frame(width: 1, height: plotFrame.height)
                            .offset(x: edge, y: plotFrame.minY)
                            .allowsHitTesting(false)
                    }
                }
            }
        }
    }

    /// Translate the drag's plot-local x span into the brushed date window.
    /// Locations clamp to the plot; a zero-width result clears instead of
    /// pinning a sliver.
    private func setBrush(from value: DragGesture.Value, plotWidth: CGFloat,
                          proxy: ChartProxy) {
        let x0 = min(max(value.startLocation.x, 0), plotWidth)
        let x1 = min(max(value.location.x, 0), plotWidth)
        guard let d0 = proxy.value(atX: x0, as: Date.self),
              let d1 = proxy.value(atX: x1, as: Date.self), d0 != d1 else { return }
        brush = min(d0, d1) ... max(d0, d1)
    }

    private func captionRow(hoverText: String?) -> some View {
        HStack(spacing: 8) {
            Text(series.domain.map { RunChartTime.hms($0.lowerBound) } ?? "")
                .frame(alignment: .leading)
            Spacer(minLength: 4)
            // The hover value prints in the caption center — no floating
            // overlay, nothing hover-only that isn't also printed.
            if let hoverText {
                Text(hoverText).foregroundStyle(Theme.textSecondary)
            }
            Spacer(minLength: 4)
            Text(series.domain.map { RunChartTime.hms($0.upperBound) } ?? "")
        }
        .font(Theme.mono(9))
        .monospacedDigit()
        .foregroundStyle(Theme.textTertiary)
        .lineLimit(1)
    }

    private var xDomain: ClosedRange<Date> {
        let lower = series.domain?.lowerBound ?? Date(timeIntervalSince1970: 0)
        let upper = series.domain?.upperBound ?? lower
        return lower ... max(upper, lower.addingTimeInterval(0.001))
    }

    private func binHoverText(bins: [DensitySeries.Bin], width: TimeInterval) -> String? {
        guard let hoverDate,
              let bin = bins.last(where: { $0.start <= hoverDate }) else { return nil }
        let end = bin.start.addingTimeInterval(width)
        let errors = bin.errorCount > 0 ? " \u{00B7} \(bin.errorCount) error\(bin.errorCount == 1 ? "" : "s")" : ""
        return "\(RunChartTime.hms(bin.start))\u{2013}\(RunChartTime.hms(end)) \u{00B7} \(bin.count) events\(errors)"
    }

    private func tickHoverText(events: [Date], errors: [Date]) -> String? {
        guard hoverDate != nil else { return nil }
        let errorNote = errors.isEmpty ? "" : " \u{00B7} \(errors.count) error\(errors.count == 1 ? "" : "s")"
        return "\(events.count) events\(errorNote)"
    }
}

// MARK: - V8: the Recorder checkpoint scrubber

/// One tick per checkpoint (full snapshots heavier), session starts as
/// bottom-lane boundary ticks. Chart-as-index: clicking a tick scrolls the
/// matching card into view — it scrubs the eye, never the run.
struct CheckpointScrubber: View {
    let timeline: CheckpointTimeline
    let live: Bool
    let onSelect: (String) -> Void

    @State private var hoverDate: Date?

    var body: some View {
        if let domain = timeline.domain {
            if domain.lowerBound == domain.upperBound {
                degenerate
            } else {
                strip(domain: domain)
            }
        }
    }

    /// All stamps equal: ticks at center, the stamp printed — no axis
    /// pretense.
    private var degenerate: some View {
        VStack(spacing: 2) {
            HStack(spacing: 3) {
                Spacer()
                ForEach(timeline.ticks) { tick in
                    Rectangle()
                        .fill(tickColor(tick))
                        .frame(width: tick.fullAnchor ? 2 : 1, height: 18)
                }
                Spacer()
            }
            Text(timeline.ticks.first.map { RunChartTime.hms($0.at) } ?? "")
                .font(Theme.mono(9))
                .foregroundStyle(Theme.textTertiary)
        }
        .accessibilityLabel(summaryLine)
    }

    private func strip(domain: ClosedRange<Date>) -> some View {
        VStack(spacing: 2) {
            Chart {
                ForEach(Array(timeline.sessions.enumerated()), id: \.offset) { _, session in
                    RuleMark(x: .value("session", session.at),
                             yStart: .value("y", 0.0), yEnd: .value("y", 0.22))
                        .lineStyle(StrokeStyle(lineWidth: 1))
                        .foregroundStyle(
                            session.status == "failed" || session.status == "killed"
                                ? Theme.Chart.fail : Theme.Chart.gridline)
                }
                ForEach(timeline.ticks) { tick in
                    RuleMark(x: .value("checkpoint", tick.at))
                        .lineStyle(StrokeStyle(lineWidth: tick.fullAnchor ? 2 : 1))
                        .foregroundStyle(tickColor(tick))
                }
            }
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartXScale(domain: domain)
            .chartYScale(domain: 0 ... 1)
            .chartXSelection(value: $hoverDate)
            .onTapGesture {
                if let tick = nearestTick { onSelect(tick.id) }
            }
            .frame(height: 28)
            .frame(maxWidth: .infinity)

            HStack(spacing: 8) {
                Text(RunChartTime.hms(domain.lowerBound))
                Spacer(minLength: 4)
                Text(centerCaption)
                    .foregroundStyle(hoverDate == nil ? Theme.textTertiary : Theme.textSecondary)
                Spacer(minLength: 4)
                Text(RunChartTime.hms(domain.upperBound))
            }
            .font(Theme.mono(9))
            .monospacedDigit()
            .foregroundStyle(Theme.textTertiary)
            .lineLimit(1)
        }
        .accessibilityLabel(summaryLine)
    }

    private func tickColor(_ tick: CheckpointTimeline.Tick) -> Color {
        if live, tick.id == timeline.ticks.last?.id { return Theme.Chart.liveDatum }
        return tick.fullAnchor ? Theme.Chart.markLine : Theme.Chart.markQuiet
    }

    private var nearestTick: CheckpointTimeline.Tick? {
        guard let hoverDate else { return nil }
        return timeline.ticks.min {
            abs($0.at.timeIntervalSince(hoverDate)) < abs($1.at.timeIntervalSince(hoverDate))
        }
    }

    /// Hovering swaps the centered counts for the hovered tick's facts —
    /// printed in place, never a floating overlay.
    private var centerCaption: String {
        if let tick = nearestTick {
            return "\(tick.id) \u{00B7} turn \(tick.turn) \u{00B7} \(Lexicon.checkpointTrigger(tick.trigger)) \u{00B7} \(RunChartTime.hms(tick.at))"
        }
        return summaryLine
    }

    private var summaryLine: String {
        "\(timeline.ticks.count) checkpoint\(timeline.ticks.count == 1 ? "" : "s") \u{00B7} \(timeline.sessions.count) session\(timeline.sessions.count == 1 ? "" : "s")"
    }
}

// MARK: - V3/V4 micro-marks (plain rectangles; the Kit scale keeps them honest)

/// One 96×5 token track: `in` segment quiet, 2px surface gap, `out` segment
/// line-ink; both against the shared turn maximum. Zero tokens renders the
/// empty track — honest absence, not a zero-width sliver pretending.
struct TokenMicroBar: View {
    let inTokens: Int
    let outTokens: Int
    let maxTokens: Int
    var width: CGFloat = 96

    var body: some View {
        HStack(spacing: 2) {
            segment(inTokens, Theme.Chart.markQuiet)
            segment(outTokens, Theme.Chart.markLine)
            Spacer(minLength: 0)
        }
        .frame(width: width, height: 5, alignment: .leading)
        .background(Theme.well)
        .accessibilityHidden(true)   // the row prints its own numbers
    }

    @ViewBuilder private func segment(_ value: Int, _ color: Color) -> some View {
        if value > 0, maxTokens > 0 {
            Rectangle()
                .fill(color)
                .frame(width: max(1, width * CGFloat(value) / CGFloat(maxTokens)), height: 5)
        }
    }
}

/// The 96×3 duration track beneath, quiet ink at 45%.
struct DurationMicroBar: View {
    let seconds: Double
    let maxSeconds: Double
    var width: CGFloat = 96

    var body: some View {
        HStack(spacing: 0) {
            if seconds > 0, maxSeconds > 0 {
                Rectangle()
                    .fill(Theme.Chart.markQuiet.opacity(0.45))
                    .frame(width: max(1, width * CGFloat(seconds / maxSeconds)), height: 3)
            }
            Spacer(minLength: 0)
        }
        .frame(width: width, height: 3, alignment: .leading)
        .background(Theme.well)
        .accessibilityHidden(true)
    }
}

/// The 80×5 recorded-check duration bar: quiet for passed, fail ink for
/// failed (state, fixed meaning); minimum 2px when a duration exists.
struct CheckDurationBar: View {
    let durationMS: Int
    let maxMS: Int
    let passed: Bool
    var width: CGFloat = 80

    var body: some View {
        HStack(spacing: 0) {
            Spacer(minLength: 0)
            Rectangle()
                .fill((passed ? Theme.Chart.markQuiet : Theme.Chart.fail).opacity(0.35))
                .frame(width: max(2, width * CGFloat(durationMS) / CGFloat(max(maxMS, 1))),
                       height: 5)
        }
        .frame(width: width, height: 5, alignment: .trailing)
        .accessibilityHidden(true)   // the printed duration is the content
    }
}

// MARK: - Shared time formatting

enum RunChartTime {
    private static let formatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private static let shortFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter
    }()

    static func hms(_ date: Date) -> String { formatter.string(from: date) }
    static func hm(_ date: Date) -> String { shortFormatter.string(from: date) }
}
