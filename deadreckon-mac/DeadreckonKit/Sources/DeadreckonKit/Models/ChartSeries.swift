import Foundation

// Pure chart derivations for the goal-run visualization wave
// (design/VIZ-DRILLDOWN-SPEC.md §K). Every series, bin, scale, and duration a
// chart renders is derived here — Sendable, tested, display-only. Charts
// render recorded rows only: no smoothing, no interpolation, no extension
// past the last record, no forecast (spec law 3). Nothing here confers
// authority or re-derives a status word.

// MARK: - K1 SpendSeries (spend.jsonl -> the burn strip)

/// Retained fold of `spend.jsonl` LOOP rows for the run-header burn strip.
/// Narrator rows are a split ledger and never enter the series (TAILING.md:
/// never summed across kinds). `capUSD` is the last non-nil loop cap.
public struct SpendSeries: Equatable, Sendable {
    public struct Point: Equatable, Sendable, Identifiable {
        /// Ledger order — stable identity across folds.
        public let ordinal: Int
        public let turn: Int
        public let timestamp: Date
        /// The row's own `cost_usd`.
        public let deltaUSD: Double
        /// The running head `total_cost_usd`.
        public let totalUSD: Double
        public let inputTokens: Int
        public let outputTokens: Int
        public let model: String
        public let provider: String
        public let wallSeconds: Double?
        public let estimated: Bool
        public let subscription: Bool

        public var id: Int { ordinal }

        public init(ordinal: Int, turn: Int, timestamp: Date, deltaUSD: Double,
                    totalUSD: Double, inputTokens: Int, outputTokens: Int,
                    model: String, provider: String, wallSeconds: Double?,
                    estimated: Bool, subscription: Bool) {
            self.ordinal = ordinal
            self.turn = turn
            self.timestamp = timestamp
            self.deltaUSD = deltaUSD
            self.totalUSD = totalUSD
            self.inputTokens = inputTokens
            self.outputTokens = outputTokens
            self.model = model
            self.provider = provider
            self.wallSeconds = wallSeconds
            self.estimated = estimated
            self.subscription = subscription
        }
    }

    /// Retention ceiling: the burn SHAPE needs the tail, the header prints
    /// the head regardless; oldest points drop WITH the counter.
    public static let pointCeiling = 5_000

    public private(set) var points: [Point] = []
    public private(set) var capUSD: Double?
    public private(set) var droppedPoints = 0
    private var nextOrdinal = 0

    public var maxTotalUSD: Double { points.last?.totalUSD ?? 0 }

    public init() {}

    /// Fold new spend records in ledger order. Loop rows only; the last
    /// non-nil `cap_usd` wins. fold(a + b) == fold(a); fold(b).
    public mutating func fold(_ records: [SpendRecord]) {
        for record in records where record.kind == "loop" {
            nextOrdinal += 1
            points.append(Point(
                ordinal: nextOrdinal, turn: record.turn, timestamp: record.timestamp,
                deltaUSD: record.costUSD, totalUSD: record.totalCostUSD,
                inputTokens: record.inputTokens, outputTokens: record.outputTokens,
                model: record.model, provider: record.provider,
                wallSeconds: record.wallTimeSeconds,
                estimated: record.estimated, subscription: record.subscription))
            if let cap = record.capUSD { capUSD = cap }
        }
        if points.count > Self.pointCeiling {
            let overflow = points.count - Self.pointCeiling
            points.removeFirst(overflow)
            droppedPoints += overflow
        }
    }
}

// MARK: - K2 DensitySeries (events.jsonl -> the Activity density strip)

/// Incremental fold of event stamps into time bins for the Activity density
/// strip, with the sparse law applied in `presentation`: no data -> absent,
/// sparse -> honest discrete ticks, enough -> bins on a fixed "nice" ladder.
/// A zero-count bin renders as zero — absence of events is a fact.
public struct DensitySeries: Equatable, Sendable {
    public struct Bin: Equatable, Sendable, Identifiable {
        public let start: Date
        public let count: Int
        public let errorCount: Int
        public var id: Date { start }

        public init(start: Date, count: Int, errorCount: Int) {
            self.start = start
            self.count = count
            self.errorCount = errorCount
        }
    }

    public enum Presentation: Equatable, Sendable {
        /// No events: the strip is absent (the feed's empty words carry it).
        case absent
        /// Sparse: one honest tick per recorded row, no bars, no y encoding.
        case ticks(events: [Date], errors: [Date])
        /// Enough data: bins at the chosen ladder width, zero bins included.
        case bins([Bin], width: TimeInterval)
    }

    /// Stamp retention ceiling; oldest stamps drop WITH the counter and the
    /// domain start stays pinned honest.
    public static let stampCeiling = 200_000

    /// The fixed "nice" bin-width ladder (seconds); beyond the last rung the
    /// width doubles until the span fits.
    public static let binLadder: [TimeInterval] = [
        1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1_800, 3_600,
    ]

    public private(set) var eventCount = 0
    public private(set) var errorStamps: [Date] = []
    /// `turn_started` boundary stamps (structure ticks).
    public private(set) var turnStamps: [Date] = []
    public private(set) var domain: ClosedRange<Date>?
    public private(set) var droppedStamps = 0

    // Fold internals (accumulator-owned; read by `presentation`).
    var sparseFloor = 12
    var sparseSpanSeconds: TimeInterval = 60
    var maxBins = 72
    var stamps: [Date] = []
    var binWidth: TimeInterval = 1
    var binCounts: [Date: Int] = [:]
    var binErrors: [Date: Int] = [:]

    public init() {}

    /// The sparse law, applied.
    public var presentation: Presentation {
        guard eventCount > 0, let domain else { return .absent }
        let span = domain.upperBound.timeIntervalSince(domain.lowerBound)
        if eventCount < sparseFloor || span < sparseSpanSeconds {
            return .ticks(events: stamps, errors: errorStamps)
        }
        var bins: [Bin] = []
        var cursor = Self.binStart(for: domain.lowerBound, width: binWidth)
        // Bounded: the ladder choice guarantees the domain spans <= maxBins
        // widths (belt: hard stop at maxBins + 2).
        while cursor <= domain.upperBound, bins.count < maxBins + 2 {
            bins.append(Bin(start: cursor,
                            count: binCounts[cursor] ?? 0,
                            errorCount: binErrors[cursor] ?? 0))
            cursor = cursor.addingTimeInterval(binWidth)
        }
        return .bins(bins, width: binWidth)
    }

    static func binStart(for date: Date, width: TimeInterval) -> Date {
        let interval = date.timeIntervalSinceReferenceDate
        return Date(timeIntervalSinceReferenceDate: (interval / width).rounded(.down) * width)
    }

    /// Smallest ladder width whose bin count over `span` stays <= maxBins;
    /// past the ladder's last rung the width doubles until it fits.
    static func ladderWidth(span: TimeInterval, maxBins: Int) -> TimeInterval {
        let budget = Double(max(maxBins - 1, 1))
        for width in binLadder where span / width <= budget {
            return width
        }
        var width = binLadder.last ?? 3_600
        while span / width > budget {
            width *= 2
        }
        return width
    }

    /// The incremental folder: maintains bins at the current ladder width;
    /// when the span outgrows `maxBins` the width steps up the ladder and
    /// bins rebuild once from the retained stamps (O(n), amortized rare).
    public struct Accumulator: Equatable, Sendable {
        private var series: DensitySeries

        public init(sparseFloor: Int = 12, sparseSpanSeconds: TimeInterval = 60,
                    maxBins: Int = 72) {
            series = DensitySeries()
            series.sparseFloor = sparseFloor
            series.sparseSpanSeconds = sparseSpanSeconds
            series.maxBins = maxBins
        }

        /// Fold (timestamp, kind) pairs from newly decoded events and return
        /// the updated series snapshot.
        public mutating func fold(_ events: [(timestamp: Date, kind: String)]) -> DensitySeries {
            guard !events.isEmpty else { return series }
            for event in events {
                series.eventCount += 1
                series.stamps.append(event.timestamp)
                if event.kind == "error" { series.errorStamps.append(event.timestamp) }
                if event.kind == "turn_started" { series.turnStamps.append(event.timestamp) }
                if let domain = series.domain {
                    series.domain = min(domain.lowerBound, event.timestamp)
                        ... max(domain.upperBound, event.timestamp)
                } else {
                    series.domain = event.timestamp ... event.timestamp
                }
            }
            if series.stamps.count > DensitySeries.stampCeiling {
                let overflow = series.stamps.count - DensitySeries.stampCeiling
                series.stamps.removeFirst(overflow)
                series.droppedStamps += overflow
                // The domain start stays pinned: droppedStamps carries the
                // honesty; bins beyond the retained window rebuild empty.
            }
            let span = series.domain.map {
                $0.upperBound.timeIntervalSince($0.lowerBound)
            } ?? 0
            let width = DensitySeries.ladderWidth(span: span, maxBins: series.maxBins)
            if width != series.binWidth || series.binCounts.isEmpty {
                series.binWidth = width
                rebuildBins()
            } else {
                for event in events {
                    let start = DensitySeries.binStart(for: event.timestamp, width: width)
                    series.binCounts[start, default: 0] += 1
                    if event.kind == "error" {
                        series.binErrors[start, default: 0] += 1
                    }
                }
            }
            return series
        }

        private mutating func rebuildBins() {
            series.binCounts = [:]
            series.binErrors = [:]
            let width = series.binWidth
            for stamp in series.stamps {
                series.binCounts[DensitySeries.binStart(for: stamp, width: width), default: 0] += 1
            }
            let firstRetained = series.stamps.first
            for stamp in series.errorStamps {
                if let firstRetained, stamp < firstRetained { continue }
                series.binErrors[DensitySeries.binStart(for: stamp, width: width), default: 0] += 1
            }
        }
    }
}

// MARK: - K4 TurnScale (shared maxima for the turn micro-bars)

/// The shared scale for the Turns list's micro-bars: every row draws against
/// the same maxima so turns are comparable down the list. Zero-safe: empty
/// input yields zeros and the view renders empty tracks.
public enum TurnScale {
    public static func derive(turns: [TurnModel]) -> (maxTokens: Int, maxWallSeconds: Double) {
        let maxTokens = turns.map { $0.inputTokens + $0.outputTokens }.max() ?? 0
        let maxWall = turns.map(\.wallSeconds).max() ?? 0
        return (maxTokens, maxWall)
    }
}

// MARK: - K5 CheckDurations (recorded check rows -> duration bars + history)

public enum CheckDurations {
    /// Identity of one check across attempts: the exact (kind, command, cwd)
    /// triple. Same kind with a different command is a different check.
    public struct CheckKey: Equatable, Hashable, Sendable {
        public let kind: String
        public let command: String?
        public let cwd: String?

        public init(kind: String, command: String?, cwd: String?) {
            self.kind = kind
            self.command = command
            self.cwd = cwd
        }
    }

    public struct Row: Equatable, Sendable {
        public let key: CheckKey
        public let durationMS: Int?
        public let passed: Bool

        public init(key: CheckKey, durationMS: Int?, passed: Bool) {
            self.key = key
            self.durationMS = durationMS
            self.passed = passed
        }
    }

    /// Bars render only when 2+ rows carry a duration (§V4 floor: a lone
    /// duration is a number, not a comparison).
    public static func derive(results: [AcceptanceProgressRow.CheckResult])
        -> (rows: [Row], maxMS: Int, showBars: Bool) {
        let rows = results.map {
            Row(key: CheckKey(kind: $0.kind, command: $0.command, cwd: $0.cwd),
                durationMS: $0.durationMS, passed: $0.passed)
        }
        let durations = rows.compactMap(\.durationMS)
        return (rows, durations.max() ?? 0, durations.count >= 2)
    }

    /// Newest-first per-attempt history for one check identity. An attempt
    /// with no matching record yields a nil result — absence stated, never
    /// inferred. `attemptIndex` is the 1-based ledger position.
    public static func history(for key: CheckKey, attempts: [JobReportEnvelope.Attempt])
        -> [(attemptIndex: Int, result: AcceptanceProgressRow.CheckResult?)] {
        attempts.enumerated().map { index, attempt in
            (attemptIndex: index + 1,
             result: attempt.checks.first {
                 CheckKey(kind: $0.kind, command: $0.command, cwd: $0.cwd) == key
             })
        }.reversed()
    }
}

// MARK: - K6 PhaseDurations (state.json phase stamps -> PLAN captions)

/// Elapsed-per-phase marks DERIVED from each phase's status-change stamp
/// (`updated_at`), not a recorded duration — the honesty caveat rides every
/// rendered caption. A negative interval, an out-of-order completion, or an
/// unrecognized status yields `.none`: never a negative, never a guess.
public enum PhaseDurations {
    public enum Mark: Equatable, Sendable {
        case completed(seconds: Double)
        /// The executing phase's live clock (recomputed each tick via `now`
        /// only while the RUN status word is "executing"; killed/failed runs
        /// freeze at the phase's own stamp).
        case current(secondsSoFar: Double)
        case none
    }

    public static func derive(phases: [RunStateDoc.Phase], runStartedAt: Date,
                              currentPhaseID: Int, status: String, now: Date) -> [Mark] {
        var marks: [Mark] = []
        var baseline = runStartedAt
        var orderHolds = true
        for phase in phases {
            switch phase.status {
            case "completed":
                let elapsed = phase.updatedAt.timeIntervalSince(baseline)
                if orderHolds, elapsed >= 0 {
                    marks.append(.completed(seconds: elapsed))
                } else {
                    marks.append(.none)
                    orderHolds = false
                }
                baseline = max(baseline, phase.updatedAt)
            case "executing" where phase.id.raw == currentPhaseID:
                let reference = status == "executing" ? now : phase.updatedAt
                let elapsed = reference.timeIntervalSince(baseline)
                marks.append(orderHolds && elapsed >= 0 ? .current(secondsSoFar: elapsed) : .none)
                orderHolds = false
            case "planned", "pending", "executing":
                // Not-started rows and stale "executing" rows the writer
                // stopped updating: no duration of their own, but their real
                // stamps stay the chain's baseline (the spec formula measures
                // updatedAt(i) − updatedAt(i−1)); they never poison the
                // durations of later completed phases. Real ledgers routinely
                // carry `init planned · plan pending` before the work phases.
                marks.append(.none)
                baseline = max(baseline, phase.updatedAt)
            default:
                // failed / unrecognized words: a completion after these is
                // out of order — never a guess.
                marks.append(.none)
                orderHolds = false
            }
        }
        return marks
    }
}

// MARK: - K9 CheckpointTimeline (recorder manifests -> the scrubber)

/// The Recorder scrubber's model: one tick per checkpoint, one boundary mark
/// per session START (sessions carry no end stamp, so no span is ever
/// drawn). The domain never extends past the last RECORDED stamp — a live
/// run's strip freezes exactly where the manifests stop (law 3).
public struct CheckpointTimeline: Equatable, Sendable {
    public struct Tick: Equatable, Sendable, Identifiable {
        public let id: String
        public let at: Date
        public let turn: Int
        public let trigger: String
        public let fullAnchor: Bool

        public init(id: String, at: Date, turn: Int, trigger: String, fullAnchor: Bool) {
            self.id = id
            self.at = at
            self.turn = turn
            self.trigger = trigger
            self.fullAnchor = fullAnchor
        }
    }

    public struct SessionMark: Equatable, Sendable {
        /// startedAt — boundaries only, no invented ends.
        public let at: Date
        public let status: String
        public let provider: String

        public init(at: Date, status: String, provider: String) {
            self.at = at
            self.status = status
            self.provider = provider
        }
    }

    /// runStartedAt ... last recorded stamp; nil when nothing is recorded.
    public let domain: ClosedRange<Date>?
    public let ticks: [Tick]
    public let sessions: [SessionMark]

    init(domain: ClosedRange<Date>?, ticks: [Tick], sessions: [SessionMark]) {
        self.domain = domain
        self.ticks = ticks
        self.sessions = sessions
    }

    public static func derive(checkpoints: [CheckpointManifestDoc],
                              sessions: [FlightManifestDoc.Session],
                              runStartedAt: Date?) -> CheckpointTimeline {
        let ticks = checkpoints.map {
            Tick(id: $0.checkpointID, at: $0.createdAt, turn: $0.deadreckonTurn,
                 trigger: $0.trigger, fullAnchor: $0.fullAnchor)
        }.sorted { $0.at < $1.at }
        let marks = sessions.map {
            SessionMark(at: $0.startedAt, status: $0.status, provider: $0.provider)
        }.sorted { $0.at < $1.at }
        let stamps = ticks.map(\.at) + marks.map(\.at)
        guard let lastStamp = stamps.max(), let firstStamp = stamps.min() else {
            return CheckpointTimeline(domain: nil, ticks: [], sessions: [])
        }
        // Clamped both ways: never before the earliest recorded stamp when
        // runStartedAt is missing/askew, never past the last recorded stamp.
        let start = min(runStartedAt ?? firstStamp, firstStamp)
        return CheckpointTimeline(domain: start ... max(start, lastStamp),
                                  ticks: ticks, sessions: marks)
    }
}
