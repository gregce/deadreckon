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

    // MARK: - K4: wall time + trace raw retention

    func testWallSecondsAccumulateFromSpendDeltas() {
        let json = #"{"timestamp": "2026-08-06T10:00:03Z", "run_id": "r", "event": {"kind": "spend_delta", "turn": 3, "cost_usd": 0.12, "wall_time_seconds": 50.7}}"#
        let decoded = try! DeadreckonJSON.decoder().decode(RunEventRecord.self, from: Data(json.utf8))
        XCTAssertEqual(decoded.event.wallTimeSeconds ?? 0, 50.7, accuracy: 0.001)

        let events = [
            event("turn_started", turn: 3, at: 0),
            decoded,
            event("spend_delta", turn: 3, at: 5, cost: 0.08),   // no wall field: legacy row
        ]
        let turns = TurnsDerivation.group(events: events, traces: [])
        XCTAssertEqual(turns[0].wallSeconds, 50.7, accuracy: 0.001,
                       "wall time accumulates exactly as costUSD does; absent fields add nothing")
    }

    func testLegacyEventsWithoutWallFieldDecodeUnchanged() {
        let json = #"{"timestamp": "2026-08-06T10:00:03Z", "run_id": "r", "event": {"kind": "spend_delta", "turn": 1, "cost_usd": 0.5}}"#
        let decoded = try! DeadreckonJSON.decoder().decode(RunEventRecord.self, from: Data(json.utf8))
        XCTAssertNil(decoded.event.wallTimeSeconds)
        let turns = TurnsDerivation.group(events: [decoded], traces: [])
        XCTAssertEqual(turns[0].wallSeconds, 0, "legacy ledgers derive an honest zero, not a guess")
    }

    func testTraceRawLinesRideOntoTraceEntries() {
        let line = #"{"timestamp": "2026-08-06T10:00:00Z", "turn": 1, "event": "llm.complete"}"#
        let traces: [(row: TraceRow, raw: String?)] = [
            (row: TraceRow(timestamp: base, turn: 1, event: "llm.complete", latencyMS: 900),
             raw: line)
        ]
        let turns = TurnsDerivation.group(events: [], traces: traces)
        XCTAssertEqual(turns[0].entries[0].raw, line, "the verbatim source line rides the entry")
        XCTAssertEqual(turns[0].entries[0].kind, .trace)
    }

    func testTraceRawCeilingDropsTheOldestRawsOnly() {
        var accumulator = TurnsDerivation.Accumulator(traceRawCeiling: 2)
        let traces: [(row: TraceRow, raw: String?)] = (1...4).map { index in
            (row: TraceRow(timestamp: base.addingTimeInterval(Double(index)), turn: 1,
                           event: "llm.complete", latencyMS: nil),
             raw: "line-\(index)")
        }
        let turns = accumulator.fold(events: [], traces: traces)
        XCTAssertEqual(turns[0].entries.count, 4, "the parsed entries themselves are never dropped")
        XCTAssertEqual(turns[0].entries.map(\.raw), [nil, nil, "line-3", "line-4"],
                       "only the newest raws survive the ceiling; the ledger on disk stays whole")
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
