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

        public var id: Int { ordinal }

        public init(ordinal: Int, timestamp: Date, kind: EntryKind, text: String) {
            self.ordinal = ordinal
            self.timestamp = timestamp
            self.kind = kind
            self.text = text
        }
    }

    public let turn: Int
    public var startedAt: Date?
    public var inputTokens: Int
    public var outputTokens: Int
    public var costUSD: Double
    public var entries: [Entry]

    public var id: Int { turn }

    public init(turn: Int, startedAt: Date? = nil, inputTokens: Int = 0,
                outputTokens: Int = 0, costUSD: Double = 0, entries: [Entry] = []) {
        self.turn = turn
        self.startedAt = startedAt
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.costUSD = costUSD
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

        public init() {}

        /// Fold new rows and return the full ordered turn list. Events
        /// without a turn number (unknown kinds) are dropped from the
        /// grouping — they still appear in the Activity feed, which renders
        /// the raw ledger. Token usage and spend accumulate from the events
        /// ledger (token_usage_delta / spend_delta); trace rows land as
        /// entries with their event word and latency.
        public mutating func fold(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel] {
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
                var turnModel = byTurn[trace.turn] ?? TurnModel(turn: trace.turn)
                ordinal += 1
                let latency = trace.latencyMS.map { " \($0)ms" } ?? ""
                turnModel.entries.append(TurnModel.Entry(
                    ordinal: ordinal, timestamp: trace.timestamp, kind: .trace,
                    text: "\(trace.event)\(latency)"))
                byTurn[trace.turn] = turnModel
                touched.insert(trace.turn)
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
    public static func group(events: [RunEventRecord], traces: [TraceRow]) -> [TurnModel] {
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
