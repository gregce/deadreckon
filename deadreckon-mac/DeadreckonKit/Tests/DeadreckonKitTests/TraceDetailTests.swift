import XCTest

@testable import DeadreckonKit

/// K11 TraceDetailDoc: the on-demand decode of one retained traces.jsonl
/// line into the full tool exchange. Lenient by law: missing branches yield
/// nils, never a throw; nil only when the line is not a JSON object at all.
final class TraceDetailTests: XCTestCase {
    /// Structurally faithful to the live codex trace observed on 2026-08-08
    /// (turn 1 of run ea6701b0…): a command_execution row with its embedded
    /// raw item, a failed row, and a file_change row with changed paths.
    private var liveFixture: String {
        let commandRaw = """
            {"type":"item.completed","item":{"id":"item_1","type":"command_execution",\
            "command":"/bin/zsh -lc \\"pwd && rg --files\\"",\
            "aggregated_output":"/Users/gdc/.deadreckon/worktrees/task-0\\napp.py\\n",\
            "exit_code":0,"status":"completed"}}
            """
        let failedRaw = """
            {"type":"item.completed","item":{"id":"item_2","type":"command_execution",\
            "command":"/bin/zsh -lc \\"swift build\\"",\
            "aggregated_output":"error: no such module",\
            "exit_code":1,"status":"failed"}}
            """
        let changeRaw = """
            {"type":"item.completed","item":{"id":"item_3","type":"file_change",\
            "changes":[{"path":"/w/app.py","kind":"update"},\
            {"path":"/w/implementation-notes.html","kind":"update"}],"status":"completed"}}
            """
        let encoder = JSONEncoder()
        func jsonString(_ string: String) -> String {
            String(data: try! encoder.encode(string), encoding: .utf8)!
        }
        return """
            {"timestamp": "2026-08-02T00:07:45.686204Z", "run_id": "ea6701b0", "turn": 1,
             "event": "llm.complete", "latency_ms": 452509,
             "detail": {
               "model": "gpt-5.6-sol", "provider": "cli:codex", "tool_call_id": "llm-turn-1",
               "trace": {
                 "args": ["--ask-for-approval", "--", "You are a deadreckon CLI sub-agent."],
                 "binary": "codex",
                 "duration_ms": 450974, "exit_code": 0,
                 "flight_rows": [
                   {"id": "item_1", "raw": \(jsonString(commandRaw)), "status": "completed",
                    "summary": "pwd && rg --files", "tool_category": "shell",
                    "tool_name": "command_execution"},
                   {"id": "item_2", "raw": \(jsonString(failedRaw)), "status": "failed",
                    "summary": "swift build", "tool_category": "shell",
                    "tool_name": "command_execution"},
                   {"id": "item_3", "raw": \(jsonString(changeRaw)), "status": "completed",
                    "summary": "2 files", "tool_category": "edit", "tool_name": "file_change"}
                 ],
                 "kind": "cli_subagent", "pid": 47740,
                 "sandbox_backend": "sandbox-exec", "sandbox_warning": null,
                 "stdout_path": "/tmp/turns/turn-1/codex.out",
                 "workspace_access": "read-write"
               }
             }}
            """
    }

    func testDecodesTheLiveCodexExchange() throws {
        let doc = try XCTUnwrap(TraceDetailDoc.decode(rawTraceLine: liveFixture))
        XCTAssertEqual(doc.provider, "cli:codex")
        XCTAssertEqual(doc.model, "gpt-5.6-sol")
        XCTAssertEqual(doc.binary, "codex")
        XCTAssertEqual(doc.durationMS, 450_974)
        XCTAssertEqual(doc.exitCode, 0)
        XCTAssertEqual(doc.sandboxBackend, "sandbox-exec")
        XCTAssertNil(doc.sandboxWarning)
        XCTAssertEqual(doc.workspaceAccess, "read-write")
        XCTAssertEqual(doc.stdoutPath, "/tmp/turns/turn-1/codex.out")
        XCTAssertEqual(doc.promptArg, "You are a deadreckon CLI sub-agent.",
                       "the prompt is the last args element")

        XCTAssertEqual(doc.flightRows.count, 3)
        let command = doc.flightRows[0]
        XCTAssertEqual(command.toolName, "command_execution")
        XCTAssertEqual(command.command, "/bin/zsh -lc \"pwd && rg --files\"")
        XCTAssertEqual(command.aggregatedOutput, "/Users/gdc/.deadreckon/worktrees/task-0\napp.py\n")
        XCTAssertEqual(command.exitCode, 0)

        let failed = doc.flightRows[1]
        XCTAssertEqual(failed.status, "failed")
        XCTAssertEqual(failed.exitCode, 1)
        XCTAssertEqual(failed.aggregatedOutput, "error: no such module")

        let change = doc.flightRows[2]
        XCTAssertEqual(change.changedPaths, ["/w/app.py", "/w/implementation-notes.html"])
    }

    func testNonJSONReturnsNil() {
        XCTAssertNil(TraceDetailDoc.decode(rawTraceLine: "not json at all"))
        XCTAssertNil(TraceDetailDoc.decode(rawTraceLine: ""))
        XCTAssertNil(TraceDetailDoc.decode(rawTraceLine: "[1, 2, 3]"),
                     "a non-object line renders raw verbatim, never a guessed doc")
    }

    func testPartialShapesYieldFactsOnlyDocs() throws {
        // No detail.trace at all: facts-only, no throw.
        let noTrace = try XCTUnwrap(TraceDetailDoc.decode(
            rawTraceLine: #"{"timestamp": "2026-08-02T00:00:00Z", "detail": {"provider": "cli:claude"}}"#))
        XCTAssertEqual(noTrace.provider, "cli:claude")
        XCTAssertNil(noTrace.binary)
        XCTAssertTrue(noTrace.flightRows.isEmpty)

        // A trace without flight_rows.
        let noRows = try XCTUnwrap(TraceDetailDoc.decode(
            rawTraceLine: #"{"detail": {"trace": {"binary": "codex", "exit_code": 2}}}"#))
        XCTAssertEqual(noRows.binary, "codex")
        XCTAssertEqual(noRows.exitCode, 2)
        XCTAssertTrue(noRows.flightRows.isEmpty)

        // A flight row whose raw is not JSON: the row survives facts-only.
        let badRaw = try XCTUnwrap(TraceDetailDoc.decode(rawTraceLine:
            #"{"detail": {"trace": {"flight_rows": [{"id": "x", "tool_name": "t", "raw": "not json"}]}}}"#))
        XCTAssertEqual(badRaw.flightRows.count, 1)
        XCTAssertEqual(badRaw.flightRows[0].toolName, "t")
        XCTAssertNil(badRaw.flightRows[0].command)
        XCTAssertEqual(badRaw.flightRows[0].changedPaths, [])
    }

    /// The built-in loop's tool traces (`event: tool.*`) carry the exchange
    /// directly on `detail` — command / stdout / stderr / status_code, no
    /// `trace` envelope. Observed live on 2026-08-08 (mock-provider run
    /// d902985e…, turn 6). The decode maps that recorded shape into one
    /// flight row; nothing is merged, nothing re-worded.
    func testDecodesTheBuiltInLoopToolShape() throws {
        let line = """
            {"timestamp": "2026-08-08T06:07:05.153251Z", "run_id": "d902985e", "turn": 6,
             "event": "tool.bash", "latency_ms": 11132,
             "detail": {"command": "echo 'stage 6: begin'\\nsleep 11",
                        "status_code": 0, "stderr": "",
                        "stdout": "stage 6: begin\\nstage 6: done\\n",
                        "tool_call_id": "mock-stage-6"}}
            """
        let doc = try XCTUnwrap(TraceDetailDoc.decode(rawTraceLine: line))
        XCTAssertFalse(doc.isEmpty)
        XCTAssertEqual(doc.durationMS, 11_132, "top-level latency_ms is the recorded duration")
        XCTAssertEqual(doc.flightRows.count, 1)
        let row = doc.flightRows[0]
        XCTAssertEqual(row.id, "mock-stage-6")
        XCTAssertEqual(row.toolName, "tool.bash", "the event word verbatim")
        XCTAssertEqual(row.command, "echo 'stage 6: begin'\nsleep 11")
        XCTAssertEqual(row.aggregatedOutput, "stage 6: begin\nstage 6: done\n")
        XCTAssertNil(row.stderrOutput, "an empty recorded stream renders nothing, not an empty well")
        XCTAssertEqual(row.exitCode, 0)
        XCTAssertNil(row.status, "the loop shape carries no status word and none is invented")
    }

    /// A failed loop tool call: stderr is its own stream (never merged into
    /// stdout), and the nonzero status_code is the recorded failure fact.
    func testLoopShapeKeepsStderrSeparateAndCarriesTheExitCode() throws {
        let line = """
            {"event": "tool.bash", "latency_ms": 90,
             "detail": {"command": "exit 1", "status_code": 1,
                        "stderr": "stage 3: simulated flake", "stdout": ""}}
            """
        let doc = try XCTUnwrap(TraceDetailDoc.decode(rawTraceLine: line))
        let row = try XCTUnwrap(doc.flightRows.first)
        XCTAssertEqual(row.exitCode, 1)
        XCTAssertNil(row.aggregatedOutput)
        XCTAssertEqual(row.stderrOutput, "stage 3: simulated flake")
    }

    /// A decodable line carrying nothing renderable: `isEmpty` is the view's
    /// cue to show the raw line verbatim instead of an empty expansion.
    func testFactlessDocReportsEmpty() throws {
        let doc = try XCTUnwrap(TraceDetailDoc.decode(
            rawTraceLine: #"{"timestamp": "2026-08-08T00:00:00Z", "turn": 2}"#))
        XCTAssertTrue(doc.isEmpty)
    }
}
