import XCTest

@testable import DeadreckonKit

/// K1 SpendSeries: the burn strip's retained fold over spend.jsonl.
final class SpendSeriesTests: XCTestCase {
    private func record(turn: Int, at offset: TimeInterval, cost: Double,
                        total: Double, cap: Double? = nil,
                        kind: String = "loop") -> SpendRecord {
        let capField = cap.map { ", \"cap_usd\": \($0)" } ?? ""
        let json = """
            {"timestamp": "2026-08-06T10:00:\(String(format: "%02.0f", offset))Z",
             "turn": \(turn), "provider": "cli:codex", "model": "gpt-5.6-sol",
             "input_tokens": 100, "output_tokens": 10, "cost_usd": \(cost),
             "total_cost_usd": \(total)\(capField), "kind": "\(kind)",
             "wall_time_seconds": 50.7}
            """
        return try! DeadreckonJSON.decoder().decode(SpendRecord.self, from: Data(json.utf8))
    }

    func testLoopRowsEnterAndNarratorRowsNever() {
        var series = SpendSeries()
        series.fold([
            record(turn: 1, at: 0, cost: 1.0, total: 1.0),
            record(turn: 1, at: 1, cost: 0.05, total: 0.05, kind: "narrator"),
            record(turn: 2, at: 2, cost: 2.0, total: 3.0),
        ])
        XCTAssertEqual(series.points.count, 2, "narrator rows are a split ledger, never folded")
        XCTAssertEqual(series.points.map(\.totalUSD), [1.0, 3.0])
        XCTAssertEqual(series.maxTotalUSD, 3.0)
        XCTAssertEqual(series.points.last?.wallSeconds ?? 0, 50.7, accuracy: 0.001)
    }

    func testCapAdoptionLastNonNilWins() {
        var series = SpendSeries()
        series.fold([
            record(turn: 1, at: 0, cost: 1, total: 1, cap: 25.0),
            record(turn: 2, at: 1, cost: 1, total: 2),
            record(turn: 3, at: 2, cost: 1, total: 3, cap: 30.0),
            record(turn: 4, at: 3, cost: 1, total: 4),
        ])
        XCTAssertEqual(series.capUSD, 30.0, "the last non-nil loop cap wins; a nil never clears it")
    }

    func testCeilingDropsOldestWithTheCounter() {
        var series = SpendSeries()
        var records: [SpendRecord] = []
        for turn in 1...(SpendSeries.pointCeiling + 3) {
            records.append(record(turn: turn, at: 0, cost: 0.01, total: Double(turn) * 0.01))
        }
        series.fold(records)
        XCTAssertEqual(series.points.count, SpendSeries.pointCeiling)
        XCTAssertEqual(series.droppedPoints, 3, "dropped points are counted, not hidden")
        XCTAssertEqual(series.points.first?.turn, 4, "the OLDEST points drop — the shape needs the tail")
    }

    func testIncrementalFoldEqualsOneShot() {
        let all = [
            record(turn: 1, at: 0, cost: 1, total: 1, cap: 10),
            record(turn: 2, at: 1, cost: 1, total: 2),
            record(turn: 3, at: 2, cost: 1, total: 3),
        ]
        var oneShot = SpendSeries()
        oneShot.fold(all)
        var incremental = SpendSeries()
        incremental.fold(Array(all.prefix(2)))
        incremental.fold(Array(all.suffix(1)))
        XCTAssertEqual(oneShot, incremental, "fold(a+b) == fold(a); fold(b)")
    }
}

/// K2 DensitySeries: the Activity strip's bins + the sparse law.
final class DensitySeriesTests: XCTestCase {
    private let base = Date(timeIntervalSinceReferenceDate: 800_000_000)

    private func events(_ offsets: [TimeInterval], kind: String = "tool_call_started")
        -> [(timestamp: Date, kind: String)] {
        offsets.map { (timestamp: base.addingTimeInterval($0), kind: kind) }
    }

    func testZeroEventsIsAbsent() {
        var accumulator = DensitySeries.Accumulator()
        let series = accumulator.fold([])
        XCTAssertEqual(series.presentation, .absent)
    }

    func testSparseBelowTheFloorRendersTicksAndFlipsToBinsExactlyAtIt() {
        var accumulator = DensitySeries.Accumulator()
        // 11 events across > 60 s: below the count floor -> ticks.
        let eleven = accumulator.fold(events((0..<11).map { Double($0) * 10 }))
        guard case .ticks(let stamps, _) = eleven.presentation else {
            return XCTFail("expected ticks, got \(eleven.presentation)")
        }
        XCTAssertEqual(stamps.count, 11)

        // The 12th event reaches the floor exactly -> bins.
        let twelve = accumulator.fold(events([115]))
        guard case .bins(let bins, let width) = twelve.presentation else {
            return XCTFail("expected bins at the floor, got \(twelve.presentation)")
        }
        XCTAssertGreaterThan(bins.count, 0)
        XCTAssertGreaterThan(width, 0)
    }

    func testShortSpanStaysTicksEvenWithManyEvents() {
        var accumulator = DensitySeries.Accumulator()
        // 20 events inside 30 s: span below the sparse floor -> ticks.
        let series = accumulator.fold(events((0..<20).map { Double($0) }))
        guard case .ticks = series.presentation else {
            return XCTFail("a sub-minute span renders honest ticks, got \(series.presentation)")
        }
    }

    func testLadderWidthSelectionPerSpan() {
        XCTAssertEqual(DensitySeries.ladderWidth(span: 60, maxBins: 72), 1)
        XCTAssertEqual(DensitySeries.ladderWidth(span: 300, maxBins: 72), 5)
        XCTAssertEqual(DensitySeries.ladderWidth(span: 3_600, maxBins: 72), 60)
        XCTAssertEqual(DensitySeries.ladderWidth(span: 7_200, maxBins: 72), 120)
        XCTAssertEqual(DensitySeries.ladderWidth(span: 86_400, maxBins: 72), 1_800)
        // Beyond the ladder the width doubles until the span fits.
        let huge = DensitySeries.ladderWidth(span: 3_600 * 24 * 365, maxBins: 72)
        XCTAssertLessThanOrEqual(3_600 * 24 * 365 / huge, 71)
    }

    func testIncrementalEqualsOneShotAcrossARebin() {
        // First fold fits 1 s bins; the second stretches the span so the
        // width steps up the ladder and bins rebuild from retained stamps.
        let first = events((0..<30).map { Double($0) * 2 })          // 58 s span
        let second = events((0..<30).map { 100 + Double($0) * 20 })  // to ~11 min

        var incremental = DensitySeries.Accumulator()
        _ = incremental.fold(first)
        let stepped = incremental.fold(second)

        var oneShot = DensitySeries.Accumulator()
        let whole = oneShot.fold(first + second)

        XCTAssertEqual(stepped.presentation, whole.presentation,
                       "rebinning must not change what renders")
        XCTAssertEqual(stepped.domain, whole.domain)
        XCTAssertEqual(stepped.eventCount, whole.eventCount)
    }

    func testErrorAndTurnStampsRouteToTheirOwnSeries() {
        var accumulator = DensitySeries.Accumulator()
        let series = accumulator.fold([
            (timestamp: base, kind: "turn_started"),
            (timestamp: base.addingTimeInterval(1), kind: "error"),
            (timestamp: base.addingTimeInterval(2), kind: "tool_call_result"),
        ])
        XCTAssertEqual(series.turnStamps, [base])
        XCTAssertEqual(series.errorStamps, [base.addingTimeInterval(1)])
        XCTAssertEqual(series.eventCount, 3)
    }

    func testZeroCountBinsRenderAsZeroBetweenBursts() {
        var accumulator = DensitySeries.Accumulator()
        // Two bursts with a quiet gap; span 300 s -> 5 s bins.
        let series = accumulator.fold(
            events((0..<12).map { Double($0) }) + events([300]))
        guard case .bins(let bins, _) = series.presentation else {
            return XCTFail("expected bins")
        }
        XCTAssertTrue(bins.contains { $0.count == 0 },
                      "absence of events is a fact: the gap renders as zero bins, never elided")
    }

    func testStampCeilingDropsOldestWithTheCounter() {
        var accumulator = DensitySeries.Accumulator()
        let overflow = 500
        var batch: [(timestamp: Date, kind: String)] = []
        batch.reserveCapacity(DensitySeries.stampCeiling + overflow)
        for index in 0..<(DensitySeries.stampCeiling + overflow) {
            batch.append((timestamp: base.addingTimeInterval(Double(index) / 100), kind: "x"))
        }
        let series = accumulator.fold(batch)
        XCTAssertEqual(series.droppedStamps, overflow)
        XCTAssertEqual(series.stamps.count, DensitySeries.stampCeiling)
        XCTAssertEqual(series.domain?.lowerBound, base,
                       "the domain start stays pinned honest after drops")
    }
}

/// K4 TurnScale: the shared maxima for the Turns micro-bars.
final class TurnScaleTests: XCTestCase {
    func testMaximaAcrossTurns() {
        let turns = [
            TurnModel(turn: 1, inputTokens: 1_000, outputTokens: 200, wallSeconds: 10),
            TurnModel(turn: 2, inputTokens: 5_000, outputTokens: 1_500, wallSeconds: 80.5),
            TurnModel(turn: 3, inputTokens: 100, outputTokens: 50, wallSeconds: 3),
        ]
        let scale = TurnScale.derive(turns: turns)
        XCTAssertEqual(scale.maxTokens, 6_500, "max of in+out per turn")
        XCTAssertEqual(scale.maxWallSeconds, 80.5)
    }

    func testZeroSafeOnEmpty() {
        let scale = TurnScale.derive(turns: [])
        XCTAssertEqual(scale.maxTokens, 0)
        XCTAssertEqual(scale.maxWallSeconds, 0)
    }
}

/// K5 CheckDurations: duration bars + attempt history matching.
final class CheckDurationsTests: XCTestCase {
    private func check(kind: String, command: String? = nil, cwd: String? = nil,
                       durationMS: Int? = nil, passed: Bool = true)
        -> AcceptanceProgressRow.CheckResult {
        AcceptanceProgressRow.CheckResult(
            kind: kind, passed: passed, mustPass: true, detail: "d",
            command: command, cwd: cwd, durationMS: durationMS,
            stdout: nil, stderr: nil)
    }

    func testTripleKeyDistinguishesSameKindDifferentCommand() {
        let a = CheckDurations.CheckKey(kind: "shell", command: "make test", cwd: "/w")
        let b = CheckDurations.CheckKey(kind: "shell", command: "make lint", cwd: "/w")
        XCTAssertNotEqual(a, b, "same kind, different command is a different check")
    }

    func testBarsFloorIsExactlyTwoDurations() {
        let one = CheckDurations.derive(results: [
            check(kind: "shell", durationMS: 4_100),
            check(kind: "file_exists"),
        ])
        XCTAssertFalse(one.showBars, "a lone duration is a number, not a comparison")

        let two = CheckDurations.derive(results: [
            check(kind: "shell", durationMS: 4_100),
            check(kind: "build_success", durationMS: 900),
        ])
        XCTAssertTrue(two.showBars)
        XCTAssertEqual(two.maxMS, 4_100)
    }

    func testHistoryIsNewestFirstWithAbsenceStated() {
        let key = CheckDurations.CheckKey(kind: "shell", command: "make test", cwd: nil)
        let attempts = [
            JobReportEnvelope.Attempt(runID: "r1", status: "failed", provider: "p",
                                      spendUSD: 1, checks: []),
            JobReportEnvelope.Attempt(runID: "r2", status: "completed", provider: "p",
                                      spendUSD: 2,
                                      checks: [check(kind: "shell", command: "make test",
                                                     durationMS: 4_100)]),
        ]
        let history = CheckDurations.history(for: key, attempts: attempts)
        XCTAssertEqual(history.map(\.attemptIndex), [2, 1], "newest first")
        XCTAssertEqual(history[0].result?.durationMS, 4_100)
        XCTAssertNil(history[1].result, "an attempt with no matching record states absence")
    }
}

/// K6 PhaseDurations: derived stamps, never a guess.
final class PhaseDurationsTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_700_000_000)

    private func phase(_ id: Int, _ status: String, at offset: TimeInterval)
        -> RunStateDoc.Phase {
        RunStateDoc.Phase(id: .init(raw: id), name: "phase-\(id)", status: status,
                          updatedAt: start.addingTimeInterval(offset))
    }

    func testHappyPathDerivesCompletedAndTicksTheCurrentPhase() {
        let phases = [
            phase(1, "completed", at: 161),
            phase(2, "completed", at: 413),
            phase(3, "executing", at: 500),
            phase(4, "pending", at: 500),
        ]
        let marks = PhaseDurations.derive(
            phases: phases, runStartedAt: start, currentPhaseID: 3,
            status: "executing", now: start.addingTimeInterval(595))
        XCTAssertEqual(marks[0], .completed(seconds: 161))
        XCTAssertEqual(marks[1], .completed(seconds: 252))
        XCTAssertEqual(marks[2], .current(secondsSoFar: 182),
                       "the executing phase clocks now - previous stamp")
        XCTAssertEqual(marks[3], .none, "pending phases show nothing")
    }

    func testNonMonotonicStampsYieldNoneNeverANegative() {
        let phases = [
            phase(1, "completed", at: 100),
            phase(2, "completed", at: 40),   // stamp went backwards
            phase(3, "completed", at: 200),
        ]
        let marks = PhaseDurations.derive(
            phases: phases, runStartedAt: start, currentPhaseID: 3,
            status: "executing", now: start.addingTimeInterval(300))
        XCTAssertEqual(marks[0], .completed(seconds: 100))
        XCTAssertEqual(marks[1], .none, "never a negative")
        XCTAssertEqual(marks[2], .none, "a broken chain never resumes guessing")
    }

    func testOutOfOrderCompletionYieldsNone() {
        let phases = [
            phase(1, "failed", at: 50),
            phase(2, "completed", at: 100),
        ]
        let marks = PhaseDurations.derive(
            phases: phases, runStartedAt: start, currentPhaseID: 2,
            status: "executing", now: start.addingTimeInterval(300))
        XCTAssertEqual(marks, [.none, .none],
                       "a completion after a non-completed phase is out of order")
    }

    /// The shape real ledgers actually carry (observed 2026-08-08, run
    /// d902985e…): `init planned · plan pending · provider/sandbox executing`
    /// rows the writer stopped updating, then cleanly completed work phases.
    /// Not-started and stale-executing rows show no duration of their own,
    /// but their real stamps stay the chain's baseline — they never poison
    /// the completed phases behind them (spec §K6 is per-phase).
    func testStalePendingAndExecutingRowsDoNotPoisonLaterCompletions() {
        let phases = [
            phase(0, "planned", at: 0),
            phase(10, "pending", at: 0),
            phase(20, "executing", at: 1),     // stale — never the current phase
            phase(30, "executing", at: 2),     // stale
            phase(40, "completed", at: 91),
            phase(50, "completed", at: 105),
            phase(60, "completed", at: 113),
        ]
        let marks = PhaseDurations.derive(
            phases: phases, runStartedAt: start, currentPhaseID: 60,
            status: "completed", now: start.addingTimeInterval(9_999))
        XCTAssertEqual(marks[0], .none)
        XCTAssertEqual(marks[1], .none)
        XCTAssertEqual(marks[2], .none, "a stale executing row claims no duration")
        XCTAssertEqual(marks[3], .none)
        XCTAssertEqual(marks[4], .completed(seconds: 89),
                       "execute measures from the last real stamp before it")
        XCTAssertEqual(marks[5], .completed(seconds: 14))
        XCTAssertEqual(marks[6], .completed(seconds: 8))
    }

    func testKilledRunFreezesTheCurrentMarkAtItsOwnStamp() {
        let phases = [
            phase(1, "completed", at: 100),
            phase(2, "executing", at: 250),
        ]
        let marks = PhaseDurations.derive(
            phases: phases, runStartedAt: start, currentPhaseID: 2,
            status: "killed", now: start.addingTimeInterval(9_999))
        XCTAssertEqual(marks[1], .current(secondsSoFar: 150),
                       "the status word gates the ticking: killed freezes at the phase stamp")
    }
}

/// K9 CheckpointTimeline: the Recorder scrubber's honest domain.
final class CheckpointTimelineTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_700_000_000)

    private func checkpoint(_ id: String, at offset: TimeInterval,
                            turn: Int = 1, fullAnchor: Bool = false) -> CheckpointManifestDoc {
        let json = """
            {"checkpoint_id": "\(id)", "deadreckon_turn": \(turn),
             "created_at": "\(ISO8601DateFormatter().string(from: start.addingTimeInterval(offset)))",
             "trigger": "provider_tool", "full_anchor": \(fullAnchor), "files": []}
            """
        return try! DeadreckonJSON.decoder().decode(CheckpointManifestDoc.self, from: Data(json.utf8))
    }

    func testDomainNeverExceedsRecordedStamps() {
        let timeline = CheckpointTimeline.derive(
            checkpoints: [checkpoint("cp-000001", at: 60), checkpoint("cp-000002", at: 120)],
            sessions: [.init(flightSessionID: "s1", provider: "cli:codex", deadreckonTurn: 1,
                             status: "running", startedAt: start.addingTimeInterval(10))],
            runStartedAt: start)
        XCTAssertEqual(timeline.domain?.lowerBound, start)
        XCTAssertEqual(timeline.domain?.upperBound, start.addingTimeInterval(120),
                       "never extended to now — the last RECORDED stamp is the edge")
        XCTAssertEqual(timeline.ticks.map(\.id), ["cp-000001", "cp-000002"])
        XCTAssertEqual(timeline.sessions.count, 1)
    }

    func testSingleCheckpointDegenerate() {
        let timeline = CheckpointTimeline.derive(
            checkpoints: [checkpoint("cp-000001", at: 30)], sessions: [], runStartedAt: start)
        XCTAssertEqual(timeline.ticks.count, 1)
        XCTAssertEqual(timeline.domain?.lowerBound, start)
        XCTAssertEqual(timeline.domain?.upperBound, start.addingTimeInterval(30))
    }

    func testMissingRunStartDomainStartsAtFirstStamp() {
        let timeline = CheckpointTimeline.derive(
            checkpoints: [checkpoint("cp-000001", at: 30), checkpoint("cp-000002", at: 90)],
            sessions: [], runStartedAt: nil)
        XCTAssertEqual(timeline.domain?.lowerBound, start.addingTimeInterval(30))
    }

    func testNothingRecordedYieldsNilDomain() {
        let timeline = CheckpointTimeline.derive(
            checkpoints: [], sessions: [], runStartedAt: start)
        XCTAssertNil(timeline.domain, "an empty recorder draws nothing, not a fake axis")
    }
}
