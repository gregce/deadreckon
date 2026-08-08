import Foundation

// Turns grouping for the Chartroom center pane (design B2): traces.jsonl
// gives the per-turn provider exchanges; events.jsonl interleaves the tool
// calls and token usage in ledger order. Sourced from ledgers, not a chat
// buffer (L2), with unbounded scrollback.

/// One collapsible turn in the Turns tab.
public struct TurnModel: Equatable, Sendable, Identifiable {
    public enum EntryKind: Equatable, Sendable {
        case toolCall
        case toolResult
        case error
        case docs
        case trace
        case other
    }

    public struct Entry: Equatable, Sendable, Identifiable {
        public let ordinal: Int
        public let timestamp: Date
        public let kind: EntryKind
        public let text: String
        /// The verbatim ledger line this entry decoded from — retained for
        /// trace-kind entries under the raw-retention ceiling (K10) so the
        /// tool-I/O drill can decode the full exchange on demand. nil past
        /// the ceiling: the drill then names the ledger file on disk.
        public var raw: String?

        public var id: Int { ordinal }

        public init(ordinal: Int, timestamp: Date, kind: EntryKind, text: String,
                    raw: String? = nil) {
            self.ordinal = ordinal
            self.timestamp = timestamp
            self.kind = kind
            self.text = text
            self.raw = raw
        }
    }

    public let turn: Int
    public var startedAt: Date?
    public var inputTokens: Int
    public var outputTokens: Int
    public var costUSD: Double
    /// Accumulated from the events ledger's spend_delta rows'
    /// `wall_time_seconds` (exactly as costUSD accumulates); 0 on legacy
    /// ledgers without the field — the view renders that as an honest dash.
    public var wallSeconds: Double
    public var entries: [Entry]

    public var id: Int { turn }

    public init(turn: Int, startedAt: Date? = nil, inputTokens: Int = 0,
                outputTokens: Int = 0, costUSD: Double = 0,
                wallSeconds: Double = 0, entries: [Entry] = []) {
        self.turn = turn
        self.startedAt = startedAt
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.costUSD = costUSD
        self.wallSeconds = wallSeconds
        self.entries = entries
    }
}

public enum TurnsDerivation {
    /// Persistent incremental grouping state: fold newly polled ledger rows
    /// as they arrive instead of re-deriving the whole run history every
    /// tick. Per-fold cost is proportional to the NEW rows (entries re-sort
    /// only inside the turns those rows touched), so a multi-hour run's 2s
    /// tick stays cheap. `group` remains the one-shot convenience.
    public struct Accumulator: Equatable, Sendable {
        private var byTurn: [Int: TurnModel] = [:]
        private var ordinal = 0
        /// Trace entries currently holding their raw line, in arrival order
        /// (K10 ceiling: newest `traceRawCeiling` trace lines keep raw;
        /// older entries' raw drops to nil and the drill names the ledger).
        private var traceRawRefs: [(turn: Int, ordinal: Int)] = []
        private let traceRawCeiling: Int

        public init(traceRawCeiling: Int = 1_000) {
            self.traceRawCeiling = traceRawCeiling
        }

        public static func == (lhs: Accumulator, rhs: Accumulator) -> Bool {
            lhs.byTurn == rhs.byTurn && lhs.ordinal == rhs.ordinal
                && lhs.traceRawRefs.map(\.ordinal) == rhs.traceRawRefs.map(\.ordinal)
                && lhs.traceRawCeiling == rhs.traceRawCeiling
        }

        /// Backwards-compatible fold without trace raw lines (raw = nil).
        /// Disfavored so an empty-literal `traces: []` resolves to the
        /// raw-carrying primary; both are semantically identical there.
        @_disfavoredOverload
        public mutating func fold(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel] {
            fold(events: events, traces: traces.map { (row: $0, raw: nil) })
        }

        /// Fold new rows and return the full ordered turn list. Events
        /// without a turn number (unknown kinds) are dropped from the
        /// grouping — they still appear in the Activity feed, which renders
        /// the raw ledger. Token usage, spend, and wall time accumulate from
        /// the events ledger (token_usage_delta / spend_delta); trace rows
        /// land as entries with their event word and latency, carrying their
        /// verbatim source line under the raw-retention ceiling.
        public mutating func fold(events: [RunEventRecord],
                                  traces: [(row: TraceRow, raw: String?)]) -> [TurnModel] {
            var touched: Set<Int> = []

            for record in events {
                guard let turn = record.event.turn else { continue }
                var turnModel = byTurn[turn] ?? TurnModel(turn: turn)
                ordinal += 1
                switch record.event.kind {
                case "turn_started":
                    // First TurnStarted wins; the ledger appends in order.
                    if turnModel.startedAt == nil { turnModel.startedAt = record.timestamp }
                case "tool_call_started":
                    turnModel.entries.append(TurnModel.Entry(
                        ordinal: ordinal, timestamp: record.timestamp, kind: .toolCall,
                        text: "tool \(record.event.toolName ?? "?")"))
                case "tool_call_result":
                    let status = record.event.status ?? "?"
                    let preview = (record.event.preview ?? "")
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                    turnModel.entries.append(TurnModel.Entry(
                        ordinal: ordinal, timestamp: record.timestamp, kind: .toolResult,
                        text: preview.isEmpty ? status : "\(status) \u{00B7} \(preview)"))
                case "token_usage_delta":
                    turnModel.inputTokens += record.event.inputTokens ?? 0
                    turnModel.outputTokens += record.event.outputTokens ?? 0
                case "spend_delta":
                    turnModel.costUSD += record.event.costUSD ?? 0
                    turnModel.wallSeconds += record.event.wallTimeSeconds ?? 0
                case "error":
                    turnModel.entries.append(TurnModel.Entry(
                        ordinal: ordinal, timestamp: record.timestamp, kind: .error,
                        text: record.event.message ?? "error"))
                case "docs_checkpoint":
                    turnModel.entries.append(TurnModel.Entry(
                        ordinal: ordinal, timestamp: record.timestamp, kind: .docs,
                        text: "docs \(record.event.status ?? "") \(record.event.path ?? "")"))
                default:
                    turnModel.entries.append(TurnModel.Entry(
                        ordinal: ordinal, timestamp: record.timestamp, kind: .other,
                        text: record.event.kind))
                }
                byTurn[turn] = turnModel
                touched.insert(turn)
            }

            for trace in traces {
                var turnModel = byTurn[trace.row.turn] ?? TurnModel(turn: trace.row.turn)
                ordinal += 1
                let latency = trace.row.latencyMS.map { " \($0)ms" } ?? ""
                turnModel.entries.append(TurnModel.Entry(
                    ordinal: ordinal, timestamp: trace.row.timestamp, kind: .trace,
                    text: "\(trace.row.event)\(latency)", raw: trace.raw))
                if trace.raw != nil {
                    traceRawRefs.append((turn: trace.row.turn, ordinal: ordinal))
                }
                byTurn[trace.row.turn] = turnModel
                touched.insert(trace.row.turn)
            }

            // K10 ceiling: the OLDEST trace entries lose their raw copy (the
            // parsed entry itself stays; the ledger on disk stays whole).
            while traceRawRefs.count > traceRawCeiling {
                let dropped = traceRawRefs.removeFirst()
                if var turnModel = byTurn[dropped.turn],
                   let index = turnModel.entries.firstIndex(where: { $0.ordinal == dropped.ordinal }) {
                    turnModel.entries[index].raw = nil
                    byTurn[dropped.turn] = turnModel
                    touched.insert(dropped.turn)
                }
            }

            // Within a turn, entries interleave by timestamp (stable on ties
            // by arrival ordinal, so ledger order survives equal stamps).
            // Only turns that received new rows re-sort.
            for turn in touched {
                byTurn[turn]?.entries.sort { lhs, rhs in
                    lhs.timestamp != rhs.timestamp
                        ? lhs.timestamp < rhs.timestamp
                        : lhs.ordinal < rhs.ordinal
                }
            }

            return byTurn.values.sorted { $0.turn < $1.turn }
        }
    }

    /// One-shot grouping over full ledgers (kept for tests and callers that
    /// hold complete histories); identical semantics to a single fold.
    @_disfavoredOverload
    public static func group(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel] {
        var accumulator = Accumulator()
        return accumulator.fold(events: events, traces: traces)
    }

    /// One-shot grouping carrying trace raw lines (K4 overload).
    public static func group(events: [RunEventRecord],
                             traces: [(row: TraceRow, raw: String?)]) -> [TurnModel] {
        var accumulator = Accumulator()
        return accumulator.fold(events: events, traces: traces)
    }
}

// MARK: - job-events.jsonl integrity chip

/// The drawer's integrity chip over the strictly-sequenced job-events ledger:
/// either the honest contiguity claim ("events 1..N contiguous") or the
/// tailer's own corruption words. Derived exclusively from JSONLTailer's
/// `.jobEvents` verification state; the chip never re-checks bytes itself.
public enum JobEventsIntegrity: Equatable, Sendable {
    /// No rows read yet (file absent or empty).
    case none
    /// Every row so far continued sequence exactly last + 1.
    case contiguous(count: Int)
    /// The tailer reported corruption (gap, shrink, vanish, bad JSON):
    /// sticky, rendered verbatim. "Render unknown, never a guessed state."
    case corrupt(String)

    public var label: String {
        switch self {
        case .none: return "no job events yet"
        case .contiguous(let count): return "events 1..\(count) contiguous"
        case .corrupt(let reason): return "integrity failed: \(reason)"
        }
    }

    /// Fold the latest poll into the chip. Corruption is sticky here exactly
    /// as it is inside the tailer.
    public static func derive(previous: JobEventsIntegrity,
                              poll: JSONLTailer.PollResult,
                              lastSequence: UInt64) -> JobEventsIntegrity {
        if case .corrupt(let reason) = previous { return .corrupt(reason) }
        switch poll {
        case .corrupt(let reason):
            return .corrupt(reason)
        case .none, .lines, .restarted:
            return lastSequence > 0 ? .contiguous(count: Int(lastSequence)) : .none
        }
    }
}
