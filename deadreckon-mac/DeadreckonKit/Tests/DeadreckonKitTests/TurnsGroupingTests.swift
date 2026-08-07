import XCTest

@testable import DeadreckonKit

/// Turns grouping from traces + events fixtures (design B2: sourced from
/// ledgers, not a chat buffer).
final class TurnsGroupingTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_700_000_000)

    private func event(_ kind: String, turn: Int?, at offset: TimeInterval,
                       tool: String? = nil, status: String? = nil,
                       preview: String? = nil, input: Int? = nil, output: Int? = nil,
                       cost: Double? = nil, message: String? = nil) -> RunEventRecord {
        RunEventRecord(
            timestamp: base.addingTimeInterval(offset),
            event: RunEventRecord.Detail(
                kind: kind, turn: turn, toolName: tool, toolCallID: nil,
                status: status, preview: preview, inputTokens: input,
                outputTokens: output, costUSD: cost, totalCostUSD: nil,
                message: message, path: nil))
    }

    func testGroupsToolCallsAndResultsByTurnInOrder() {
        let events = [
            event("turn_started", turn: 1, at: 0),
            event("tool_call_started", turn: 1, at: 1, tool: "bash"),
            event("tool_call_result", turn: 1, at: 2, status: "ok", preview: "exit 0"),
            event("turn_started", turn: 2, at: 10),
            event("tool_call_started", turn: 2, at: 11, tool: "edit"),
        ]
        let turns = TurnsDerivation.group(events: events, traces: [])

        XCTAssertEqual(turns.map(\.turn), [1, 2])
        XCTAssertEqual(turns[0].startedAt, base)
        XCTAssertEqual(turns[0].entries.map(\.text), ["tool bash", "ok \u{00B7} exit 0"])
        XCTAssertEqual(turns[1].entries.map(\.text), ["tool edit"])
    }

    func testTokenUsageAndSpendAccumulatePerTurn() {
        let events = [
            event("turn_started", turn: 3, at: 0),
            event("token_usage_delta", turn: 3, at: 1, input: 1000, output: 200),
            event("token_usage_delta", turn: 3, at: 2, input: 2100, output: 1600),
            event("spend_delta", turn: 3, at: 3, cost: 0.12),
            event("spend_delta", turn: 3, at: 4, cost: 0.08),
        ]
        let turns = TurnsDerivation.group(events: events, traces: [])

        XCTAssertEqual(turns.count, 1)
        XCTAssertEqual(turns[0].inputTokens, 3100)
        XCTAssertEqual(turns[0].outputTokens, 1800)
        XCTAssertEqual(turns[0].costUSD, 0.20, accuracy: 0.0001)
        // Usage deltas are counters, not entries.
        XCTAssertTrue(turns[0].entries.isEmpty)
    }

    func testTraceRowsInterleaveByTimestampWithinTheTurn() {
        let events = [
            event("turn_started", turn: 1, at: 0),
            event("tool_call_started", turn: 1, at: 5, tool: "bash"),
        ]
        let traces = [
            TraceRow(timestamp: base.addingTimeInterval(2), turn: 1,
                     event: "provider_exchange", latencyMS: 3100),
            TraceRow(timestamp: base.addingTimeInterval(8), turn: 1,
                     event: "provider_exchange", latencyMS: 900),
        ]
        let turns = TurnsDerivation.group(events: events, traces: traces)

        XCTAssertEqual(turns[0].entries.map(\.text), [
            "provider_exchange 3100ms",
            "tool bash",
            "provider_exchange 900ms",
        ])
        XCTAssertEqual(turns[0].entries.map(\.kind), [.trace, .toolCall, .trace])
    }

    func testErrorsAndUnknownKindsLandAsEntriesUnknownKeepsRawKind() {
        let events = [
            event("turn_started", turn: 1, at: 0),
            event("error", turn: 1, at: 1, message: "provider 429"),
            event("hologram_sync", turn: 1, at: 2),
        ]
        let turns = TurnsDerivation.group(events: events, traces: [])

        XCTAssertEqual(turns[0].entries.map(\.text), ["provider 429", "hologram_sync"])
        XCTAssertEqual(turns[0].entries.map(\.kind), [.error, .other])
    }

    func testEventsWithoutATurnNumberAreExcludedFromGrouping() {
        let events = [
            event("turn_started", turn: 1, at: 0),
            event("run_completed", turn: nil, at: 5),
        ]
        let turns = TurnsDerivation.group(events: events, traces: [])
        XCTAssertEqual(turns.count, 1)
        XCTAssertTrue(turns[0].entries.isEmpty)
    }

    func testTraceOnlyTurnStillAppears() {
        let traces = [
            TraceRow(timestamp: base, turn: 7, event: "provider_exchange", latencyMS: nil)
        ]
        let turns = TurnsDerivation.group(events: [], traces: traces)
        XCTAssertEqual(turns.map(\.turn), [7])
        XCTAssertEqual(turns[0].entries.map(\.text), ["provider_exchange"])
    }
}

/// Integrity-chip derivation over JSONLTailer's jobEvents verdicts.
final class JobEventsIntegrityTests: XCTestCase {
    func testNoRowsYetIsNone() {
        let chip = JobEventsIntegrity.derive(previous: .none, poll: .none, lastSequence: 0)
        XCTAssertEqual(chip, .none)
        XCTAssertEqual(chip.label, "no job events yet")
    }

    func testContiguousRowsReportTheHonestClaim() {
        let chip = JobEventsIntegrity.derive(
            previous: .none, poll: .lines(["{}", "{}"]), lastSequence: 4)
        XCTAssertEqual(chip, .contiguous(count: 4))
        XCTAssertEqual(chip.label, "events 1..4 contiguous")
    }

    func testGapReportsTheTailersWordsVerbatim() {
        let chip = JobEventsIntegrity.derive(
            previous: .contiguous(count: 3),
            poll: .corrupt("job event sequence gap: expected 4, read 6"),
            lastSequence: 3)
        XCTAssertEqual(chip, .corrupt("job event sequence gap: expected 4, read 6"))
        XCTAssertTrue(chip.label.contains("integrity failed"))
    }

    func testCorruptionIsStickyAcrossLaterCleanPolls() {
        let corrupt = JobEventsIntegrity.corrupt("gap")
        let chip = JobEventsIntegrity.derive(
            previous: corrupt, poll: .lines(["{}"]), lastSequence: 9)
        XCTAssertEqual(chip, corrupt)
    }
}
