import XCTest

@testable import DeadreckonKit

/// K7: architecture-graph decode (both live scopes) + the deterministic
/// layered layout. Fixture shapes are the real files observed under
/// ~/.deadreckon on 2026-08-08, abbreviated but structurally faithful.
final class ArchitectureGraphTests: XCTestCase {
    /// The plan-scope DAG (plans/<jobID>/narrative/architecture-graph.json,
    /// driver jobs): a real graph with spawns/owns/blocks edges.
    static let planFixture = """
        {
          "version": 1,
          "graph_id": "arch-04369aabb75d",
          "scope": "plan",
          "target_id": "aa49e5aa3f7a460bb574539472c8265d",
          "generated_at": "2026-08-06T02:07:43.035800Z",
          "default_visual": "agents",
          "nodes": [
            {"id": "plan:aa49e5aa", "label": "plan aa49e5aa", "kind": "run", "status": "running", "weight": 5, "evidence": ["plan:aa49e5aa"], "style_token": "primary"},
            {"id": "task:task-0", "label": "task-0 child", "kind": "task", "status": "running", "weight": 3, "evidence": ["task:task-0"], "style_token": "primary"},
            {"id": "provider:cli:codex", "label": "cli:codex", "kind": "provider", "status": "active", "weight": 2, "evidence": ["task:task-0"], "style_token": "primary"},
            {"id": "run:d7524b52", "label": "run d7524b52", "kind": "run", "status": "active", "weight": 2, "evidence": ["child-run:d7524b52b0f71218709967c41b07b392"], "style_token": "primary"},
            {"id": "task:task-1", "label": "task-1 child", "kind": "task", "status": "pending", "weight": 3, "evidence": ["task:task-1"], "style_token": "muted"},
            {"id": "task:task-2", "label": "task-2 child", "kind": "task", "status": "pending", "weight": 3, "evidence": ["task:task-2"], "style_token": "muted"}
          ],
          "edges": [
            {"from": "plan:aa49e5aa", "to": "task:task-0", "label": "spawns", "kind": "spawns", "evidence": ["task:task-0"]},
            {"from": "task:task-0", "to": "provider:cli:codex", "label": "uses", "kind": "depends_on", "evidence": ["task:task-0"]},
            {"from": "task:task-0", "to": "run:d7524b52", "label": "owns", "kind": "owns", "evidence": ["child-run:d7524b52b0f71218709967c41b07b392"]},
            {"from": "plan:aa49e5aa", "to": "task:task-1", "label": "spawns", "kind": "spawns", "evidence": ["task:task-1"]},
            {"from": "task:task-0", "to": "task:task-1", "label": "blocks", "kind": "depends_on", "evidence": ["task:task-1:deps"]},
            {"from": "plan:aa49e5aa", "to": "task:task-2", "label": "spawns", "kind": "spawns", "evidence": ["task:task-2"]},
            {"from": "task:task-0", "to": "task:task-2", "label": "blocks", "kind": "depends_on", "evidence": ["task:task-2:deps"]}
          ],
          "groups": [
            {"id": "group:tasks", "label": "Plan tasks", "node_ids": ["task:task-0", "task:task-1", "task:task-2"], "evidence": ["plan:aa49e5aa"]}
          ],
          "layout": {"kind": "swimlane", "root_ids": ["plan:aa49e5aa"], "warnings": []},
          "legend": [
            {"style_token": "primary", "meaning": "active work"},
            {"style_token": "success", "meaning": "done"},
            {"style_token": "warning", "meaning": "risk or stale evidence"},
            {"style_token": "danger", "meaning": "blocked or failed"}
          ]
        }
        """

    /// The run-scope STAR (<runRoot>/narrative/architecture-graph.json):
    /// one run node -> provider + file nodes, nodes truncated while
    /// source_window.files carries the full list.
    static let runFixture = """
        {
          "version": 1,
          "graph_id": "arch-1939cfabb3af",
          "scope": "run",
          "target_id": "f3529e49e20b43ceb31b647385a8c54f",
          "generated_at": "2026-07-08T15:11:37.341466Z",
          "source_window": {
            "run_events": {"from_seq": 1, "to_seq": 8},
            "files": [".data/slack-clone.sqlite", ".gitignore", ".next/BUILD_ID", "app.py", "lib/db.ts"]
          },
          "nodes": [
            {"id": "run:f3529e49", "label": "run f3529e49", "kind": "run", "status": "completed", "weight": 5, "evidence": ["file:/tmp/state.json"], "style_token": "success"},
            {"id": "provider:cli:codex", "label": "cli:codex", "kind": "provider", "status": "active", "weight": 3, "evidence": ["file:/tmp/state.json"], "style_token": "primary"},
            {"id": "file:.gitignore", "label": ".gitignore", "kind": "file", "status": "active", "weight": 2, "evidence": ["file:.gitignore"], "style_token": "primary"},
            {"id": "file:app.py", "label": "app.py", "kind": "file", "status": "neutral", "weight": 2, "evidence": ["file:app.py"], "style_token": "muted"}
          ],
          "edges": [
            {"from": "run:f3529e49", "to": "provider:cli:codex", "label": "uses", "kind": "depends_on", "evidence": []},
            {"from": "run:f3529e49", "to": "file:.gitignore", "label": "touches", "kind": "writes", "evidence": []},
            {"from": "run:f3529e49", "to": "file:app.py", "label": "touches", "kind": "writes", "evidence": []}
          ],
          "groups": [
            {"id": "group:changed-files", "label": "Changed files", "node_ids": ["file:.gitignore", "file:app.py"], "evidence": []}
          ],
          "layout": {"kind": "layered-tree", "root_ids": ["run:f3529e49"], "warnings": []},
          "legend": [{"style_token": "primary", "meaning": "active work"}]
        }
        """

    private func decode(_ json: String) throws -> ArchitectureGraphDoc {
        try DeadreckonJSON.decoder().decode(ArchitectureGraphDoc.self, from: Data(json.utf8))
    }

    // MARK: - Decode

    func testDecodesThePlanScopeFixture() throws {
        let doc = try decode(Self.planFixture)
        XCTAssertEqual(doc.scope, "plan")
        XCTAssertEqual(doc.graphID, "arch-04369aabb75d")
        XCTAssertEqual(doc.nodes.count, 6)
        XCTAssertEqual(doc.edges.count, 7)
        XCTAssertEqual(doc.layout.kind, "swimlane")
        XCTAssertEqual(doc.layout.rootIDs, ["plan:aa49e5aa"])
        XCTAssertEqual(doc.legend.count, 4)
        XCTAssertEqual(doc.legend.first?.meaning, "active work")
        XCTAssertEqual(doc.groups.first?.nodeIDs.count, 3)
        XCTAssertNil(doc.sourceFileCount, "the plan window carries no files list")
        XCTAssertEqual(doc.edges.first { $0.kind == "owns" }?.label, "owns")
    }

    func testDecodesTheRunScopeFixtureWithSourceFileCount() throws {
        let doc = try decode(Self.runFixture)
        XCTAssertEqual(doc.scope, "run")
        XCTAssertEqual(doc.layout.kind, "layered-tree")
        XCTAssertEqual(doc.sourceFileCount, 5,
                       "source_window.files is the honest total behind the truncated node list")
        XCTAssertEqual(doc.nodes.filter { $0.kind == "file" }.count, 2)
    }

    // MARK: - Style tokens

    func testUnknownStyleTokenPreservesTheRawWord() {
        XCTAssertEqual(GraphStyleToken(raw: "primary"), .primary)
        XCTAssertEqual(GraphStyleToken(raw: "danger"), .danger)
        XCTAssertEqual(GraphStyleToken(raw: "hologram"), .unknown("hologram"),
                       "unknown vocabulary is preserved verbatim, rendered muted")
    }

    // MARK: - Layout

    func testBFSDepthsForThePlanFixture() throws {
        let doc = try decode(Self.planFixture)
        guard case .placed(let placed, let columns) = GraphLayoutDerivation.layered(doc) else {
            return XCTFail("expected a placed layout")
        }
        let byID = Dictionary(uniqueKeysWithValues: placed.map { ($0.node.id, $0) })
        XCTAssertEqual(byID["plan:aa49e5aa"]?.column, 0)
        XCTAssertEqual(byID["task:task-0"]?.column, 1)
        XCTAssertEqual(byID["task:task-1"]?.column, 1, "min depth over roots wins over blocks paths")
        XCTAssertEqual(byID["task:task-2"]?.column, 1)
        XCTAssertEqual(byID["provider:cli:codex"]?.column, 2)
        XCTAssertEqual(byID["run:d7524b52"]?.column, 2)
        XCTAssertEqual(columns, 3)
    }

    func testLayoutIsDeterministic() throws {
        let doc = try decode(Self.planFixture)
        XCTAssertEqual(GraphLayoutDerivation.layered(doc), GraphLayoutDerivation.layered(doc))
    }

    func testRowsOrderByWeightDescThenID() throws {
        let doc = try decode(Self.planFixture)
        guard case .placed(let placed, _) = GraphLayoutDerivation.layered(doc) else {
            return XCTFail("expected a placed layout")
        }
        let columnOne = placed.filter { $0.column == 1 }.sorted { $0.row < $1.row }
        XCTAssertEqual(columnOne.map(\.node.id),
                       ["task:task-0", "task:task-1", "task:task-2"],
                       "equal weights order by id asc — deterministic, no physics")
    }

    private func syntheticDoc(nodeCount: Int, rootIDs: [String],
                              edges: [ArchitectureGraphDoc.Edge] = []) -> ArchitectureGraphDoc {
        let nodes = (0..<nodeCount).map {
            ArchitectureGraphDoc.Node(id: "n\($0)", label: "n\($0)", kind: "file",
                                      status: "neutral", weight: 1, evidence: [],
                                      styleToken: "muted")
        }
        return ArchitectureGraphDoc(
            version: 1, graphID: "g", scope: "run", targetID: "t",
            generatedAt: Date(timeIntervalSince1970: 0), nodes: nodes, edges: edges,
            groups: [], layout: .init(kind: "layered-tree", rootIDs: rootIDs, warnings: []),
            legend: [], sourceFileCount: nil)
    }

    func testCanvasRefusesAtFortyOneNodes() {
        let forty = syntheticDoc(nodeCount: 40, rootIDs: ["n0"])
        if case .tooLarge = GraphLayoutDerivation.layered(forty) {
            XCTFail("40 nodes still draws")
        }
        let fortyOne = syntheticDoc(nodeCount: 41, rootIDs: ["n0"])
        XCTAssertEqual(GraphLayoutDerivation.layered(fortyOne), .tooLarge(nodeCount: 41),
                       "above the ceiling the Canvas refuses — never a hairball")
    }

    func testUnreachableNodesLandInTheOverflowColumnNeverDropped() {
        let doc = syntheticDoc(
            nodeCount: 4, rootIDs: ["n0"],
            edges: [.init(from: "n0", to: "n1", label: "uses", kind: "depends_on")])
        guard case .placed(let placed, _) = GraphLayoutDerivation.layered(doc) else {
            return XCTFail("expected a placed layout")
        }
        XCTAssertEqual(placed.count, 4, "unreachable nodes are drawn, never dropped silently")
        let byID = Dictionary(uniqueKeysWithValues: placed.map { ($0.node.id, $0) })
        XCTAssertEqual(byID["n1"]?.column, 1)
        XCTAssertEqual(byID["n2"]?.column, 2, "overflow column = max BFS depth + 1")
        XCTAssertEqual(byID["n3"]?.column, 2)
    }
}
