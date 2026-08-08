import DeadreckonKit
import SwiftUI

// The Story tab's MAP section (VIZ-DRILLDOWN-SPEC §V7): the narrative
// architecture graph, rendered by what the data IS. A run-scope star renders
// as an evidence strip (a node-link render of a star is a worse Changes list
// wearing circles); a plan-scope DAG renders as a deterministic layered
// Canvas. Every word verbatim; deterministic evidence — never inside or
// below the unverified overlay's frame.

struct StoryMapSection: View {
    @ObservedObject var detail: JobDetailStore
    let openChangesTab: () -> Void
    let openChangesFile: (String) -> Void

    @State private var selectedNode: SelectedGraphNode?

    struct SelectedGraphNode: Identifiable {
        let node: ArchitectureGraphDoc.Node
        var id: String { node.id }
    }

    var body: some View {
        if let issue = detail.archGraphIssue, detail.archGraph == nil {
            VStack(alignment: .leading, spacing: 4) {
                Theme.sectionTitle("MAP")
                Text("map unreadable: \(issue)")
                    .font(Theme.body(10.5))
                    .foregroundStyle(Theme.warn)
                    .textSelection(.enabled)
            }
        } else if let doc = detail.archGraph {
            VStack(alignment: .leading, spacing: 6) {
                header(doc)
                if let issue = detail.archGraphIssue {
                    Text("map unreadable this poll, showing the last good read: \(issue)")
                        .font(Theme.body(9.5))
                        .foregroundStyle(Theme.warn)
                        .textSelection(.enabled)
                }
                content(doc)
                warnings(doc)
            }
            .popover(item: $selectedNode, arrowEdge: .bottom) { selected in
                GraphNodeDetail(
                    node: selected.node,
                    diffPath: diffPath(for: selected.node),
                    onOpenChanges: { path in
                        selectedNode = nil
                        openChangesFile(path)
                    })
                    .presentationBackground(Theme.panel)
            }
        }
        // No graph file -> no MAP section: silence, not placeholder.
    }

    private func header(_ doc: ArchitectureGraphDoc) -> some View {
        HStack(spacing: 8) {
            Theme.sectionTitle("MAP")
            Spacer()
            // Freshness words stay with the narrative staleness chip; this
            // caption only states when the file was drawn.
            Text("\(drawnAgo(doc.generatedAt)) \u{00B7} flows left \u{2192} right")
                .font(Theme.caption)
                .foregroundStyle(Theme.textTertiary)
                .monospacedDigit()
        }
    }

    @ViewBuilder private func content(_ doc: ArchitectureGraphDoc) -> some View {
        if doc.scope == "plan" {
            switch GraphLayoutDerivation.layered(doc) {
            case .placed(let placed, let columns):
                GraphCanvasView(doc: doc, placed: placed, columns: columns) { node in
                    selectedNode = SelectedGraphNode(node: node)
                }
                legendRow(doc)
            case .tooLarge(let count):
                Text("graph too large to draw honestly \u{2014} \(count) nodes")
                    .font(Theme.body(10))
                    .foregroundStyle(Theme.textTertiary)
                evidenceStrip(doc)
            }
        } else {
            evidenceStrip(doc)
        }
    }

    // MARK: Tier 1 — the evidence strip (run-scope star)

    /// The star's actual information content as a sentence of chips — no
    /// invented geometry.
    @ViewBuilder private func evidenceStrip(_ doc: ArchitectureGraphDoc) -> some View {
        let root = rootNode(doc)
        let provider = doc.nodes.first { $0.kind == "provider" }
        let files = doc.nodes.filter { $0.kind == "file" }.sorted {
            $0.weight != $1.weight ? $0.weight > $1.weight : $0.id < $1.id
        }
        let fileTotal = doc.sourceFileCount ?? files.count

        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                if let root {
                    nodeChip(root, suffix: root.status.isEmpty ? nil : "((\(root.status)))")
                }
                if provider != nil {
                    edgeWord("uses")
                }
                if let provider {
                    nodeChip(provider)
                }
                if fileTotal > 0 {
                    edgeWord("touches")
                    Button {
                        openChangesTab()
                    } label: {
                        StatusChip(text: "\(fileTotal) file\(fileTotal == 1 ? "" : "s")",
                                   color: Theme.textSecondary)
                    }
                    .buttonStyle(.tactile)
                    .help("open the Changes tab")
                }
                Spacer(minLength: 0)
            }
            if !files.isEmpty {
                HStack(spacing: 6) {
                    ForEach(files.prefix(4), id: \.id) { node in
                        nodeChip(node, mono: true)
                    }
                    if fileTotal > 4 {
                        JumpLine(title: "+\(fileTotal - 4) more", size: 9.5) {
                            openChangesTab()
                        }
                    }
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func edgeWord(_ word: String) -> some View {
        Text("\u{2014}\(word)\u{2192}")
            .font(Theme.mono(9.5))
            .foregroundStyle(Theme.textTertiary)
    }

    /// The chip atom (DESIGN §5) tinted by the node's own style token;
    /// hover shows its evidence paths, click shares the D7 detail.
    private func nodeChip(_ node: ArchitectureGraphDoc.Node,
                          suffix: String? = nil, mono: Bool = false) -> some View {
        let color = GraphTokenInk.color(for: node.styleToken)
        let text = suffix.map { "\(node.label) \($0)" } ?? node.label
        return Button {
            selectedNode = SelectedGraphNode(node: node)
        } label: {
            Text(mono ? middleTruncated(text) : text)
                .font(mono ? Theme.mono(10) : .system(size: 10.5, weight: .semibold))
                .foregroundStyle(color)
                .padding(.horizontal, 6)
                .frame(height: 18)
                .background(color.opacity(0.10),
                            in: RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Theme.chipRadius, style: .continuous)
                    .strokeBorder(color.opacity(0.45), lineWidth: 1))
                .lineLimit(1)
        }
        .buttonStyle(.tactile)
        .help(node.evidence.isEmpty
            ? "\(node.label) \u{00B7} \(node.status) \u{00B7} \(node.kind)"
            : node.evidence.joined(separator: "\n"))
    }

    private func rootNode(_ doc: ArchitectureGraphDoc) -> ArchitectureGraphDoc.Node? {
        for rootID in doc.layout.rootIDs {
            if let node = doc.nodes.first(where: { $0.id == rootID }) { return node }
        }
        return doc.nodes.max { $0.weight < $1.weight }
    }

    // MARK: Legend + warnings (the file's own words)

    private func legendRow(_ doc: ArchitectureGraphDoc) -> some View {
        HStack(spacing: 10) {
            ForEach(doc.legend, id: \.styleToken) { entry in
                HStack(spacing: 4) {
                    Circle()
                        .fill(GraphTokenInk.color(for: entry.styleToken))
                        .frame(width: 6, height: 6)
                    Text(entry.meaning)
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            Spacer(minLength: 0)
        }
        .accessibilityLabel(doc.legend.map(\.meaning).joined(separator: ", "))
    }

    @ViewBuilder private func warnings(_ doc: ArchitectureGraphDoc) -> some View {
        ForEach(doc.layout.warnings, id: \.self) { warning in
            Text(warning)
                .font(Theme.body(9.5))
                .foregroundStyle(Theme.warn)
                .textSelection(.enabled)
        }
    }

    // MARK: Helpers

    private func drawnAgo(_ generatedAt: Date) -> String {
        let age = max(0, Int(Date().timeIntervalSince(generatedAt)))
        if age >= 3600 { return "drawn \(age / 3600)h ago" }
        if age >= 60 { return "drawn \(age / 60)m ago" }
        return "drawn \(age)s ago"
    }

    private func diffPath(for node: ArchitectureGraphDoc.Node) -> String? {
        guard node.kind == "file", let files = detail.changes?.files else { return nil }
        if let exact = files.first(where: { $0.path == node.label }) { return exact.path }
        return files.first {
            node.label.hasSuffix($0.path) || $0.path.hasSuffix(node.label)
        }?.path
    }

    private func middleTruncated(_ text: String, limit: Int = 18) -> String {
        guard text.count > limit else { return text }
        let keep = (limit - 1) / 2
        return "\(text.prefix(keep))\u{2026}\(text.suffix(keep))"
    }
}

/// style_token -> ink, per the graph's own legend ("primary" is the file's
/// word for active work — the live marker, a lawful accent use); unknown
/// tokens render muted with the raw word available in the D7 detail.
enum GraphTokenInk {
    static func color(for raw: String) -> Color {
        switch GraphStyleToken(raw: raw) {
        case .primary: return Theme.accent
        case .success: return Theme.success
        case .warning: return Theme.warn
        case .danger: return Theme.danger
        case .muted, .unknown: return Theme.textTertiary
        }
    }
}

// MARK: - Tier 2: the plan-scope Canvas DAG

/// Deterministic layered drawing: columns = BFS depth, edges as 1px cubic
/// curves left -> right (no arrowheads — flow is stated once in the
/// caption). No pan, no zoom, no drag: the graph fits or the layout refused
/// upstream. Nodes are real views so hover/click/accessibility come free;
/// only the edges are Canvas strokes.
struct GraphCanvasView: View {
    let doc: ArchitectureGraphDoc
    let placed: [GraphLayoutDerivation.Placed]
    let columns: Int
    let onSelectNode: (ArchitectureGraphDoc.Node) -> Void

    private static let height: CGFloat = 220
    private static let padding: CGFloat = 16

    var body: some View {
        GeometryReader { geometry in
            let frames = nodeFrames(in: geometry.size)
            ZStack(alignment: .topLeading) {
                Canvas { context, _ in
                    for edge in doc.edges {
                        guard let from = frames[edge.from], let to = frames[edge.to] else { continue }
                        var path = Path()
                        let start = CGPoint(x: from.maxX, y: from.midY)
                        let end = CGPoint(x: to.minX, y: to.midY)
                        let dx = max((end.x - start.x) / 2, 24)
                        path.move(to: start)
                        path.addCurve(
                            to: end,
                            control1: CGPoint(x: start.x + dx, y: start.y),
                            control2: CGPoint(x: end.x - dx, y: end.y))
                        context.stroke(path, with: .color(Theme.Chart.gridline), lineWidth: 1)
                    }
                }
                // Edge labels surface on hover at the midpoint (words like
                // `blocks` / `spawns` / `uses`, verbatim).
                ForEach(Array(doc.edges.enumerated()), id: \.offset) { _, edge in
                    if let from = frames[edge.from], let to = frames[edge.to] {
                        Color.clear
                            .frame(width: 14, height: 14)
                            .contentShape(Rectangle())
                            .position(x: (from.maxX + to.minX) / 2,
                                      y: (from.midY + to.midY) / 2)
                            .help("\(edge.from) \(edge.label) \(edge.to)")
                    }
                }
                ForEach(placed, id: \.node.id) { item in
                    if let frame = frames[item.node.id] {
                        nodeView(item.node)
                            .frame(width: frame.width, height: frame.height)
                            .position(x: frame.midX, y: frame.midY)
                    }
                }
            }
        }
        .frame(height: Self.height)
        .frame(maxWidth: .infinity)
        .background(Theme.panel)
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous)
            .strokeBorder(Theme.border, lineWidth: 1))
        .overlay(alignment: .topTrailing) {
            // The file's own layout kind, verbatim.
            Text(doc.layout.kind)
                .font(Theme.mono(9))
                .foregroundStyle(Theme.textTertiary)
                .padding(6)
        }
        .accessibilityLabel(
            "Architecture map: \(doc.nodes.count) nodes, \(doc.edges.count) edges, flows left to right")
    }

    private func nodeView(_ node: ArchitectureGraphDoc.Node) -> some View {
        let color = GraphTokenInk.color(for: node.styleToken)
        let isFile = node.kind == "file"
        return Button {
            onSelectNode(node)
        } label: {
            Text(middleTruncated(node.label))
                .font(isFile ? Theme.mono(10) : .system(size: 10.5, weight: .medium))
                .foregroundStyle(color)
                .lineLimit(1)
                .padding(.horizontal, 6)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(color.opacity(0.10),
                            in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .strokeBorder(color.opacity(0.45), lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(.tactile)
        .help("\(node.label) \u{00B7} \(node.status) \u{00B7} \(node.kind)")
        .accessibilityLabel("\(node.label), \(node.status), \(node.kind)")
    }

    /// Deterministic frames: columns spread across the width, rows spread
    /// down each column. Pure geometry from the Kit layout.
    private func nodeFrames(in size: CGSize) -> [String: CGRect] {
        guard columns > 0 else { return [:] }
        let usableWidth = size.width - Self.padding * 2
        let columnWidth = usableWidth / CGFloat(columns)
        let nodeWidth = min(150, columnWidth - 20)
        let nodeHeight: CGFloat = 22
        var rowsPerColumn: [Int: Int] = [:]
        for item in placed {
            rowsPerColumn[item.column, default: 0] += 1
        }
        var frames: [String: CGRect] = [:]
        for item in placed {
            let rows = max(rowsPerColumn[item.column] ?? 1, 1)
            let x = Self.padding + columnWidth * CGFloat(item.column)
                + (columnWidth - nodeWidth) / 2
            let laneHeight = (Self.height - Self.padding * 2) / CGFloat(rows)
            let y = Self.padding + laneHeight * (CGFloat(item.row) + 0.5) - nodeHeight / 2
            frames[item.node.id] = CGRect(x: x, y: y, width: nodeWidth, height: nodeHeight)
        }
        return frames
    }

    private func middleTruncated(_ text: String, limit: Int = 18) -> String {
        guard text.count > limit else { return text }
        let keep = (limit - 1) / 2
        return "\(text.prefix(keep))\u{2026}\(text.suffix(keep))"
    }
}
