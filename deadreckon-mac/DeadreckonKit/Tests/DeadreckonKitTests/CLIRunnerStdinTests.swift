import XCTest

@testable import DeadreckonKit

/// The stdin-write path on the real runner, proven against real children:
/// bytes go to the child's stdin and the pipe closes (EOF) so a
/// read-to-end consumer (the `config set-key` contract) terminates.
final class CLIRunnerStdinTests: XCTestCase {

    /// One-shot leg: /bin/cat echoes stdin, so stdout equals the payload
    /// exactly — and only because the pipe was closed after the write.
    func testRunDetailedFeedsStdinToTheChildAndClosesIt() async throws {
        let payload = "sk-test-stdin-roundtrip\n"
        let result = try await CLIRunner.runDetailed(
            binary: URL(fileURLWithPath: "/bin/cat"), arguments: [],
            workingDirectory: NSTemporaryDirectory(), environment: [:],
            timeout: 10, stdin: Data(payload.utf8))
        XCTAssertEqual(result.exitCode, 0)
        XCTAssertEqual(result.stdout, payload)
    }

    /// nil stdin keeps the historical null device: cat sees immediate EOF
    /// and exits cleanly instead of hanging on an open pipe.
    func testNilStdinStaysNullDevice() async throws {
        let result = try await CLIRunner.runDetailed(
            binary: URL(fileURLWithPath: "/bin/cat"), arguments: [],
            workingDirectory: NSTemporaryDirectory(), environment: [:],
            timeout: 10)
        XCTAssertEqual(result.exitCode, 0)
        XCTAssertEqual(result.stdout, "")
    }

    /// Streaming leg (the path DeadreckonCLIClient actually runs): the
    /// child reads stdin to EOF and the event stream carries it back.
    func testStreamingRunnerFeedsStdin() async throws {
        let runner = CLIRunner(
            binary: URL(fileURLWithPath: "/bin/cat"), arguments: [],
            workingDirectory: NSTemporaryDirectory(), environment: [:],
            stdin: Data("line-one\nline-two\n".utf8))
        try runner.launch()
        var lines: [String] = []
        var exit: Int32 = -1
        for await event in runner.events {
            switch event {
            case .stdoutLine(let line): lines.append(line)
            case .stderrLine: break
            case .terminated(let code): exit = code
            }
        }
        XCTAssertEqual(exit, 0)
        XCTAssertEqual(lines, ["line-one", "line-two"])
    }

    /// A child that exits without reading stdin must not signal or hang
    /// the app (F_SETNOSIGPIPE + swallowed write error, payload never
    /// echoed anywhere).
    func testChildThatIgnoresStdinStillTerminatesCleanly() async throws {
        let result = try await CLIRunner.runDetailed(
            binary: URL(fileURLWithPath: "/usr/bin/true"), arguments: [],
            workingDirectory: NSTemporaryDirectory(), environment: [:],
            timeout: 10, stdin: Data(repeating: 0x61, count: 4 << 20))
        XCTAssertEqual(result.exitCode, 0)
    }
}
