import XCTest

@testable import DeadreckonKit

/// Behavioral tests for the docs/TAILING.md reader algorithm over real temp
/// files: torn appends, strict job-event sequencing, and the
/// acceptance-progress restart special rule.
final class JSONLTailerTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("jsonl-tailer-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func file(_ name: String) -> URL {
        directory.appendingPathComponent(name)
    }

    private func write(_ text: String, to url: URL) throws {
        try Data(text.utf8).write(to: url)
    }

    private func append(_ text: String, to url: URL) throws {
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: Data(text.utf8))
    }

    // MARK: - Standard mode

    func testMissingFileIsNothingYet() {
        let tailer = JSONLTailer(url: file("absent.jsonl"), mode: .standard)
        XCTAssertEqual(tailer.poll(), .none)
        XCTAssertEqual(tailer.offset, 0)
    }

    func testReadsCompleteLinesAndRetainsTornTail() throws {
        let url = file("events.jsonl")
        try write("{\"a\":1}\n{\"b\":2}\n{\"c\":", to: url)
        let tailer = JSONLTailer(url: url, mode: .standard)

        // The torn final line is ignored and retried, never parsed.
        XCTAssertEqual(tailer.poll(), .lines(["{\"a\":1}", "{\"b\":2}"]))
        XCTAssertEqual(tailer.poll(), .none)

        // Completing the append yields exactly the completed line.
        try append("3}\n", to: url)
        XCTAssertEqual(tailer.poll(), .lines(["{\"c\":3}"]))
    }

    func testTornAppendSplitAcrossThreePolls() throws {
        let url = file("events.jsonl")
        try write("{\"a\"", to: url)
        let tailer = JSONLTailer(url: url, mode: .standard)
        XCTAssertEqual(tailer.poll(), .none)
        try append(":1", to: url)
        XCTAssertEqual(tailer.poll(), .none)
        try append("}\n", to: url)
        XCTAssertEqual(tailer.poll(), .lines(["{\"a\":1}"]))
    }

    func testCompleteInvalidLineIsStickyCorruption() throws {
        let url = file("events.jsonl")
        try write("{\"a\":1}\nnot json\n{\"b\":2}\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .standard)

        guard case .corrupt = tailer.poll() else {
            return XCTFail("damage to a completed line is corruption, not a torn append")
        }
        // Sticky: the file is no longer trusted, even after more appends.
        try append("{\"c\":3}\n", to: url)
        guard case .corrupt = tailer.poll() else {
            return XCTFail("corruption must be sticky")
        }
    }

    func testShrinkingStandardFileIsCorruption() throws {
        let url = file("events.jsonl")
        try write("{\"a\":1}\n{\"b\":2}\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .standard)
        XCTAssertEqual(tailer.poll(), .lines(["{\"a\":1}", "{\"b\":2}"]))

        try write("{\"a\":1}\n", to: url)
        guard case .corrupt = tailer.poll() else {
            return XCTFail("append-only files must never shrink")
        }
    }

    func testVanishingStandardFileIsCorruption() throws {
        let url = file("events.jsonl")
        try write("{\"a\":1}\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .standard)
        XCTAssertEqual(tailer.poll(), .lines(["{\"a\":1}"]))

        try FileManager.default.removeItem(at: url)
        guard case .corrupt = tailer.poll() else {
            return XCTFail("blessed files are never rotated or removed")
        }
    }

    // MARK: - jobEvents mode (strict sequence)

    private func jobEventLine(sequence: Int) -> String {
        """
        {"schema_version":1,"job_id":"job-1","event_id":"evt-\(sequence)","causation_id":"evt-\(sequence - 1)","sequence":\(sequence),"kind":"queued","lease_epoch":0,"timestamp":"2026-08-06T02:00:0\(sequence % 10)Z","detail":null}
        """
    }

    func testJobEventsSequenceContinuityAccepted() throws {
        let url = file("job-events.jsonl")
        try write(jobEventLine(sequence: 1) + "\n" + jobEventLine(sequence: 2) + "\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .jobEvents)

        guard case .lines(let lines) = tailer.poll() else {
            return XCTFail("sequential rows must be accepted")
        }
        XCTAssertEqual(lines.count, 2)
        XCTAssertEqual(tailer.lastSequence, 2)

        try append(jobEventLine(sequence: 3) + "\n", to: url)
        guard case .lines(let more) = tailer.poll() else {
            return XCTFail("continuation row must be accepted")
        }
        XCTAssertEqual(more.count, 1)
        XCTAssertEqual(tailer.lastSequence, 3)
    }

    func testJobEventsSequenceGapIsCorruption() throws {
        let url = file("job-events.jsonl")
        try write(jobEventLine(sequence: 1) + "\n" + jobEventLine(sequence: 3) + "\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .jobEvents)

        guard case .corrupt(let reason) = tailer.poll() else {
            return XCTFail("a sequence gap is corruption; render unknown, never a guessed state")
        }
        XCTAssertTrue(reason.contains("expected 2"), reason)
    }

    func testJobEventsMissingSequenceFieldIsCorruption() throws {
        let url = file("job-events.jsonl")
        try write("{\"kind\":\"queued\"}\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .jobEvents)
        guard case .corrupt = tailer.poll() else {
            return XCTFail("a job event without a sequence is corruption")
        }
    }

    func testJobEventsTornFinalLineDoesNotBreakSequencing() throws {
        let url = file("job-events.jsonl")
        let torn = jobEventLine(sequence: 2)
        let cut = torn.index(torn.startIndex, offsetBy: 40)
        try write(jobEventLine(sequence: 1) + "\n" + String(torn[..<cut]), to: url)
        let tailer = JSONLTailer(url: url, mode: .jobEvents)

        guard case .lines(let lines) = tailer.poll() else {
            return XCTFail("the complete first row must be delivered")
        }
        XCTAssertEqual(lines.count, 1)
        XCTAssertEqual(tailer.lastSequence, 1)

        try append(String(torn[cut...]) + "\n", to: url)
        guard case .lines(let completed) = tailer.poll() else {
            return XCTFail("the completed torn row must be delivered")
        }
        XCTAssertEqual(completed.count, 1)
        XCTAssertEqual(tailer.lastSequence, 2)
    }

    // MARK: - acceptanceProgress mode (restart special rule)

    private func progressLine(index: Int, status: String) -> String {
        """
        {"checked_at":"2026-08-06T02:19:1\(index)Z","status":"\(status)","index":\(index),"total":5}
        """
    }

    func testAcceptanceProgressShrinkIsRestartNotCorruption() throws {
        let url = file("acceptance-progress.jsonl")
        try write(
            progressLine(index: 1, status: "started") + "\n"
                + progressLine(index: 1, status: "passed") + "\n"
                + progressLine(index: 2, status: "started") + "\n",
            to: url)
        let tailer = JSONLTailer(url: url, mode: .acceptanceProgress)
        guard case .lines(let first) = tailer.poll() else {
            return XCTFail("live rows must be delivered")
        }
        XCTAssertEqual(first.count, 3)

        // New gate attempt: the trusted controller removed and restarted the
        // file with fewer bytes.
        try write(progressLine(index: 1, status: "started") + "\n", to: url)
        guard case .restarted(let reread) = tailer.poll() else {
            return XCTFail("a shrunk acceptance-progress file is a restart, never corruption")
        }
        XCTAssertEqual(reread.count, 1)
    }

    func testAcceptanceProgressMidLineRewriteIsRestart() throws {
        let url = file("acceptance-progress.jsonl")
        try write(progressLine(index: 1, status: "started") + "\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .acceptanceProgress)
        guard case .lines = tailer.poll() else {
            return XCTFail("live rows must be delivered")
        }

        // Sign-time rewrite reuses the inode and lands LONGER than the
        // offset, so the next read starts mid-line inside new content: a
        // parse anomaly, which for this file means restart and re-read.
        let rewritten =
            progressLine(index: 1, status: "passed") + "\n"
            + progressLine(index: 2, status: "passed") + "\n"
            + progressLine(index: 3, status: "failed") + "\n"
        try write(rewritten, to: url)
        guard case .restarted(let reread) = tailer.poll() else {
            return XCTFail("any parse anomaly on acceptance-progress is a restart")
        }
        XCTAssertEqual(reread.count, 3)
        XCTAssertTrue(reread[2].contains("\"failed\""), "re-read must come from the top")
    }

    func testAcceptanceProgressDisappearingFileIsRestart() throws {
        let url = file("acceptance-progress.jsonl")
        try write(progressLine(index: 1, status: "started") + "\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .acceptanceProgress)
        guard case .lines = tailer.poll() else {
            return XCTFail("live rows must be delivered")
        }

        // clear_stale_gate_attempt_evidence removed the file at gate entry.
        try FileManager.default.removeItem(at: url)
        XCTAssertEqual(tailer.poll(), .restarted([]))
        XCTAssertEqual(tailer.offset, 0)

        // The next attempt's rows stream in fresh.
        try write(progressLine(index: 1, status: "started") + "\n", to: url)
        guard case .lines(let fresh) = tailer.poll() else {
            return XCTFail("rows after a restart must flow again")
        }
        XCTAssertEqual(fresh.count, 1)
    }

    func testAcceptanceProgressUnparseableRereadStaysReset() throws {
        let url = file("acceptance-progress.jsonl")
        try write(progressLine(index: 1, status: "started") + "\n", to: url)
        let tailer = JSONLTailer(url: url, mode: .acceptanceProgress)
        guard case .lines = tailer.poll() else {
            return XCTFail("live rows must be delivered")
        }

        // A rewrite caught mid-flight: shorter AND still invalid at re-read.
        try write("garbage\n", to: url)
        XCTAssertEqual(tailer.poll(), .restarted([]))
        XCTAssertEqual(tailer.offset, 0, "must stay reset so the next poll retries from the top")

        // Once the writer finishes, the next poll delivers the whole file.
        try write(progressLine(index: 1, status: "passed") + "\n", to: url)
        guard case .lines(let settled) = tailer.poll() else {
            return XCTFail("a settled rewrite must be readable on the next poll")
        }
        XCTAssertEqual(settled.count, 1)
    }
}
