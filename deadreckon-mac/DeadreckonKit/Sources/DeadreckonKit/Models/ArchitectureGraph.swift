import Foundation

// The narrative architecture graph (VIZ-DRILLDOWN-SPEC §K7): decode of
// `narrative/architecture-graph.json` at plan scope (`plans/<jobID>/…`,
// driver jobs) or run scope (`<runRoot>/…`), plus the deterministic layered
// layout for the Story tab's MAP Canvas. Display data only: every word
// (kind, status, style_token, legend, warnings) renders verbatim; nothing
// here re-derives a status.

// MARK: - The document

public struct ArchitectureGraphDoc: Codable, Equatable, Sendable {
    public struct Node: Codable, Equatable, Sendable {
        public let id: String
        public let label: String
        /// Verbatim: run | task | provider | file | …
        public let kind: String
        /// Verbatim status word.
        public let status: String
        public let weight: Int
        public let evidence: [String]
        public let styleToken: String

        enum CodingKeys: String, CodingKey {
            case id, label, kind, status, weight, evidence
            case styleToken = "style_token"
        }

        public init(id: String, label: String, kind: String, status: String,
                    weight: Int, evidence: [String], styleToken: String) {
            self.id = id
            self.label = label
            self.kind = kind
            self.status = status
            self.weight = weight
            self.evidence = evidence
            self.styleToken = styleToken
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            id = try container.decode(String.self, forKey: .id)
            label = try container.decode(String.self, forKey: .label)
            kind = try container.decode(String.self, forKey: .kind)
            status = try container.decodeIfPresent(String.self, forKey: .status) ?? ""
            weight = try container.decodeIfPresent(Int.self, forKey: .weight) ?? 0
            evidence = try container.decodeIfPresent([String].self, forKey: .evidence) ?? []
            styleToken = try container.decodeIfPresent(String.self, forKey: .styleToken) ?? ""
        }
    }

    public struct Edge: Codable, Equatable, Sendable {
        public let from: String
        public let to: String
        /// Verbatim edge words: spawns, blocks, uses, touches, owns, …
        public let label: String
        public let kind: String

        public init(from: String, to: String, label: String, kind: String) {
            self.from = from
            self.to = to
            self.label = label
            self.kind = kind
        }

        enum CodingKeys: String, CodingKey {
            case from, to, label, kind
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            from = try container.decode(String.self, forKey: .from)
            to = try container.decode(String.self, forKey: .to)
            label = try container.decodeIfPresent(String.self, forKey: .label) ?? ""
            kind = try container.decodeIfPresent(String.self, forKey: .kind) ?? ""
        }
    }

    public struct Group: Codable, Equatable, Sendable {
        public let id: String
        public let label: String
        public let nodeIDs: [String]

        enum CodingKeys: String, CodingKey {
            case id, label
            case nodeIDs = "node_ids"
        }

        public init(id: String, label: String, nodeIDs: [String]) {
            self.id = id
            self.label = label
            self.nodeIDs = nodeIDs
        }
    }

    public struct Layout: Codable, Equatable, Sendable {
        /// Verbatim ("swimlane", "layered-tree", …): renders as the block's
        /// caption; both observed kinds are depth-layered left -> right.
        public let kind: String
        public let rootIDs: [String]
        public let warnings: [String]

        enum CodingKeys: String, CodingKey {
            case kind
            case rootIDs = "root_ids"
            case warnings
        }

        public init(kind: String, rootIDs: [String], warnings: [String]) {
            self.kind = kind
            self.rootIDs = rootIDs
            self.warnings = warnings
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            kind = try container.decodeIfPresent(String.self, forKey: .kind) ?? ""
            rootIDs = try container.decodeIfPresent([String].self, forKey: .rootIDs) ?? []
            warnings = try container.decodeIfPresent([String].self, forKey: .warnings) ?? []
        }
    }

    public struct LegendEntry: Codable, Equatable, Sendable {
        public let styleToken: String
        public let meaning: String

        enum CodingKeys: String, CodingKey {
            case styleToken = "style_token"
            case meaning
        }

        public init(styleToken: String, meaning: String) {
            self.styleToken = styleToken
            self.meaning = meaning
        }
    }

    public let version: Int
    public let graphID: String
    /// "run" | "plan" — the tier switch (§V7).
    public let scope: String
    public let targetID: String
    public let generatedAt: Date
    public let nodes: [Node]
    public let edges: [Edge]
    public let groups: [Group]
    public let layout: Layout
    public let legend: [LegendEntry]
    /// `source_window.files.count` when the file carries it: the honest
    /// total behind a truncated file-node list (the run star truncates
    /// nodes to ~10 while source_window lists every touched file).
    public let sourceFileCount: Int?

    enum CodingKeys: String, CodingKey {
        case version
        case graphID = "graph_id"
        case scope
        case targetID = "target_id"
        case generatedAt = "generated_at"
        case nodes, edges, groups, layout, legend
        case sourceWindow = "source_window"
    }

    private enum SourceWindowKeys: String, CodingKey {
        case files
    }

    public init(version: Int, graphID: String, scope: String, targetID: String,
                generatedAt: Date, nodes: [Node], edges: [Edge], groups: [Group],
                layout: Layout, legend: [LegendEntry], sourceFileCount: Int?) {
        self.version = version
        self.graphID = graphID
        self.scope = scope
        self.targetID = targetID
        self.generatedAt = generatedAt
        self.nodes = nodes
        self.edges = edges
        self.groups = groups
        self.layout = layout
        self.legend = legend
        self.sourceFileCount = sourceFileCount
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        version = try container.decodeIfPresent(Int.self, forKey: .version) ?? 0
        graphID = try container.decode(String.self, forKey: .graphID)
        scope = try container.decode(String.self, forKey: .scope)
        targetID = try container.decode(String.self, forKey: .targetID)
        generatedAt = try container.decode(Date.self, forKey: .generatedAt)
        nodes = try container.decodeIfPresent([Node].self, forKey: .nodes) ?? []
        edges = try container.decodeIfPresent([Edge].self, forKey: .edges) ?? []
        groups = try container.decodeIfPresent([Group].self, forKey: .groups) ?? []
        layout = try container.decodeIfPresent(Layout.self, forKey: .layout)
            ?? Layout(kind: "", rootIDs: [], warnings: [])
        legend = try container.decodeIfPresent([LegendEntry].self, forKey: .legend) ?? []
        if let window = try? container.nestedContainer(
            keyedBy: SourceWindowKeys.self, forKey: .sourceWindow),
            let files = try? window.decodeIfPresent([String].self, forKey: .files) {
            sourceFileCount = files.count
        } else {
            sourceFileCount = nil
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(version, forKey: .version)
        try container.encode(graphID, forKey: .graphID)
        try container.encode(scope, forKey: .scope)
        try container.encode(targetID, forKey: .targetID)
        try container.encode(generatedAt, forKey: .generatedAt)
        try container.encode(nodes, forKey: .nodes)
        try container.encode(edges, forKey: .edges)
        try container.encode(groups, forKey: .groups)
        try container.encode(layout, forKey: .layout)
        try container.encode(legend, forKey: .legend)
    }
}

// MARK: - Style tokens

/// The file's own `style_token` vocabulary, mapped to the app's fixed
/// meanings; an unknown token is preserved verbatim and renders muted with
/// the raw word in the tooltip — never a guessed color.
public enum GraphStyleToken: Equatable, Sendable {
    case primary
    case success
    case warning
    case danger
    case muted
    case unknown(String)

    public init(raw: String) {
        switch raw {
        case "primary": self = .primary
        case "success": self = .success
        case "warning": self = .warning
        case "danger": self = .danger
        case "muted": self = .muted
        default: self = .unknown(raw)
        }
    }
}

// MARK: - Layered layout (deterministic, no physics)

public enum GraphLayoutDerivation {
    /// Above this the Canvas refuses (§V7: never a hairball) and Tier 1's
    /// strip renders instead with the count printed.
    public static let nodeCeiling = 40

    public struct Placed: Equatable, Sendable {
        public let node: ArchitectureGraphDoc.Node
        /// BFS depth from `layout.root_ids` (min over roots); unreachable
        /// nodes land in a final overflow column — drawn, never dropped
        /// silently.
        public let column: Int
        /// Within a column: weight desc, then id asc — deterministic.
        public let row: Int

        public init(node: ArchitectureGraphDoc.Node, column: Int, row: Int) {
            self.node = node
            self.column = column
            self.row = row
        }
    }

    public enum Result: Equatable, Sendable {
        case placed([Placed], columns: Int)
        case tooLarge(nodeCount: Int)
    }

    public static func layered(_ doc: ArchitectureGraphDoc) -> Result {
        guard doc.nodes.count <= nodeCeiling else {
            return .tooLarge(nodeCount: doc.nodes.count)
        }
        let nodeIDs = Set(doc.nodes.map(\.id))
        var adjacency: [String: [String]] = [:]
        for edge in doc.edges {
            adjacency[edge.from, default: []].append(edge.to)
        }
        // BFS from all roots at once: min depth over roots; cycle-safe via
        // the improves-only relaxation.
        var depth: [String: Int] = [:]
        var queue: [(id: String, depth: Int)] = []
        for root in doc.layout.rootIDs where nodeIDs.contains(root) && depth[root] == nil {
            depth[root] = 0
            queue.append((root, 0))
        }
        var head = 0
        while head < queue.count {
            let (id, currentDepth) = queue[head]
            head += 1
            for next in adjacency[id] ?? [] where nodeIDs.contains(next) {
                if depth[next] == nil || currentDepth + 1 < depth[next]! {
                    depth[next] = currentDepth + 1
                    queue.append((next, currentDepth + 1))
                }
            }
        }
        let maxDepth = depth.values.max()
        let overflowColumn = maxDepth.map { $0 + 1 } ?? 0
        var byColumn: [Int: [ArchitectureGraphDoc.Node]] = [:]
        for node in doc.nodes {
            byColumn[depth[node.id] ?? overflowColumn, default: []].append(node)
        }
        var placed: [Placed] = []
        for (column, nodes) in byColumn.sorted(by: { $0.key < $1.key }) {
            let ordered = nodes.sorted {
                $0.weight != $1.weight ? $0.weight > $1.weight : $0.id < $1.id
            }
            for (row, node) in ordered.enumerated() {
                placed.append(Placed(node: node, column: column, row: row))
            }
        }
        let columnCount = (byColumn.keys.max() ?? 0) + 1
        return .placed(placed, columns: columnCount)
    }
}
