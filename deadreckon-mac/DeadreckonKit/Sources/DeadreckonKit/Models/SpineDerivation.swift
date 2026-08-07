import Foundation

// The 5-question status spine, recomputed from the same durable inputs the
// attach TUI uses. This mirrors the RUN surface of
// crates/deadreckon/src/tui/spine.rs (spine_for_run_with_events and the
// run_* helpers) faithfully; the plan/chain/campaign surfaces are not ported
// because the Chartroom drill-in observes a durable Job's current attempt,
// which is always a Run. Every simplification against spine.rs is documented
// in CONTRACTS.md.

/// Everything `spine_for_run_with_events` reads, projected to values the
/// JobDetailStore already holds from its file reads and tails. Pure input:
/// no file IO happens in the derivation.
public struct RunSpineInputs: Equatable, Sendable {
    public var runID: String
    /// kebab-case RunStatus word from state.json: pending | planned |
    /// executing | completed | failed | killed. Unrecognized words derive
    /// the honest unknown spine, never a guessed state.
    public var status: String
    public var turn: Int
    public var activePhaseName: String?
    public var totalSpendUSD: Double
    /// state.max_spend_usd (launch-plan ceiling wins when present, mirroring
    /// launch_plan_spend_ceiling(...).or(state.max_spend_usd)).
    public var stateMaxSpendUSD: Double?
    public var launchPlanCeilingUSD: Double?
    public var totalWallSeconds: Double
    public var stateMaxWallSeconds: Double?
    public var launchPlanWallSeconds: Double?
    public var pauseReason: String?
    public var failureReason: String?
    /// state.updated_at: the aliveness fallback when no event exists.
    public var updatedAt: Date
    /// Newest events.jsonl timestamp (from the activity tail).
    public var newestEventTimestamp: Date?
    /// Newest Error event's message (events.jsonl scanned newest-first).
    public var newestErrorMessage: String?
    /// reshape-proposal.json exists and parses as a JSON object. spine.rs
    /// additionally validates it as a launch plan; presence-plus-parse is
    /// the documented simplification.
    public var hasReshapeProposal: Bool
    /// steer-inbox.jsonl rows with status == "pending".
    public var pendingSteerCount: Int

    public init(runID: String, status: String, turn: Int, activePhaseName: String?,
                totalSpendUSD: Double, stateMaxSpendUSD: Double?,
                launchPlanCeilingUSD: Double?, totalWallSeconds: Double,
                stateMaxWallSeconds: Double?, launchPlanWallSeconds: Double?,
                pauseReason: String?, failureReason: String?, updatedAt: Date,
                newestEventTimestamp: Date?, newestErrorMessage: String?,
                hasReshapeProposal: Bool, pendingSteerCount: Int) {
        self.runID = runID
        self.status = status
        self.turn = turn
        self.activePhaseName = activePhaseName
        self.totalSpendUSD = totalSpendUSD
        self.stateMaxSpendUSD = stateMaxSpendUSD
        self.launchPlanCeilingUSD = launchPlanCeilingUSD
        self.totalWallSeconds = totalWallSeconds
        self.stateMaxWallSeconds = stateMaxWallSeconds
        self.launchPlanWallSeconds = launchPlanWallSeconds
        self.pauseReason = pauseReason
        self.failureReason = failureReason
        self.updatedAt = updatedAt
        self.newestEventTimestamp = newestEventTimestamp
        self.newestErrorMessage = newestErrorMessage
        self.hasReshapeProposal = hasReshapeProposal
        self.pendingSteerCount = pendingSteerCount
    }
}

public struct SpineSnapshot: Equatable, Sendable {
    public enum Aliveness: Equatable, Sendable {
        case live(lastEventAgeSeconds: Int)
        case stale(ageSeconds: Int)
        case dead(reason: String)
        case done
    }

    public enum AttentionKind: Equatable, Sendable {
        case pausedAtCap
        case failure
        case killed
        case providerError
        case stall
        case reshapeProposed
        case steerPending
    }

    public struct Attention: Equatable, Sendable {
        public let kind: AttentionKind
        public let summary: String

        public init(kind: AttentionKind, summary: String) {
            self.kind = kind
            self.summary = summary
        }
    }

    public struct OnTrack: Equatable, Sendable {
        public let spendUsedUSD: Double
        public let spendCeilingUSD: Double?
        public let wallSecondsUsed: Double
        public let wallSecondsCeiling: Double?
        public let turnsUsed: Int?

        public init(spendUsedUSD: Double, spendCeilingUSD: Double?,
                    wallSecondsUsed: Double, wallSecondsCeiling: Double?,
                    turnsUsed: Int?) {
            self.spendUsedUSD = spendUsedUSD
            self.spendCeilingUSD = spendCeilingUSD
            self.wallSecondsUsed = wallSecondsUsed
            self.wallSecondsCeiling = wallSecondsCeiling
            self.turnsUsed = turnsUsed
        }
    }

    public let alive: Aliveness
    public let doing: String
    public let onTrack: OnTrack
    public let wrong: [Attention]
    /// Exactly one command (spine.rs invariant: no chains, no pipes).
    public let next: String

    public init(alive: Aliveness, doing: String, onTrack: OnTrack,
                wrong: [Attention], next: String) {
        self.alive = alive
        self.doing = doing
        self.onTrack = onTrack
        self.wrong = wrong
        self.next = next
    }

    // MARK: Band text (mirrors spine_plain_lines wording exactly: plain
    // hyphens, matching aliveness_text/on_track_text/attention_text in
    // spine.rs; the parity claim in CONTRACTS.md is load-bearing)

    public var aliveText: String {
        switch alive {
        case .live(let age): return "live \(age)s"
        case .stale(let age): return "stale \(age)s"
        case .dead(let reason): return "dead - \(reason)"
        case .done: return "done"
        }
    }

    public var onTrackText: String {
        // Built from the same match as spine.rs on_track_text so a future
        // job-level gate count slots straight in; runs always carry no gate
        // counts today (documented simplification 4).
        let gate = "gate -"
        let spend: String
        if let ceiling = onTrack.spendCeilingUSD {
            spend = String(format: "$%.2f/$%.2f", onTrack.spendUsedUSD, ceiling)
        } else if onTrack.spendUsedUSD > 0 {
            spend = String(format: "$%.2f/unbounded", onTrack.spendUsedUSD)
        } else {
            spend = "spend unbounded"
        }
        let turns = onTrack.turnsUsed.map { "turn \($0)" } ?? "turn -"
        return "\(gate) - \(spend) - \(turns)"
    }

    public var wrongText: String {
        guard let first = wrong.first else { return "none" }
        return "\(wrong.count) attention - \(first.summary)"
    }

    /// The same snapshot with a different NEXT command. Used by the
    /// JobDetailStore's job-altitude override: the run-surface fallback can
    /// suggest verbs the ownership fence refuses for job-owned runs, and the
    /// job envelope's own next_actions are the friendliness contract.
    public func replacingNext(_ newNext: String) -> SpineSnapshot {
        SpineSnapshot(alive: alive, doing: doing, onTrack: onTrack, wrong: wrong, next: newNext)
    }
}

public enum SpineDerivation {
    /// spine.rs SPINE_STALE_AFTER_SECONDS.
    public static let staleAfterSeconds = 30

    public static func deriveRun(_ inputs: RunSpineInputs, now: Date) -> SpineSnapshot {
        let alive = aliveness(inputs, now: now)
        var wrong = attention(inputs, alive: alive)

        if inputs.hasReshapeProposal {
            wrong.append(SpineSnapshot.Attention(
                kind: .reshapeProposed,
                summary: "reshape proposal waiting for explicit operator acceptance"))
        }
        if inputs.pendingSteerCount > 0 {
            let noun = inputs.pendingSteerCount == 1 ? "steer" : "steers"
            wrong.append(SpineSnapshot.Attention(
                kind: .steerPending,
                summary: "\(inputs.pendingSteerCount) pending \(noun) waiting for delivery"))
        }

        return SpineSnapshot(
            alive: alive,
            doing: doing(inputs),
            onTrack: onTrack(inputs),
            wrong: wrong,
            next: inputs.hasReshapeProposal
                ? "deadreckon reshape \(inputs.runID)"
                : primaryAction(inputs))
    }

    // MARK: - run_aliveness

    private static func aliveness(_ inputs: RunSpineInputs, now: Date) -> SpineSnapshot.Aliveness {
        switch inputs.status {
        case "completed":
            return .done
        case "failed":
            return .dead(reason: inputs.failureReason ?? "run failed")
        case "killed":
            return .dead(reason: inputs.failureReason ?? "run killed")
        case "pending", "planned", "executing":
            let timestamp = inputs.newestEventTimestamp ?? inputs.updatedAt
            let age = ageSeconds(now: now, timestamp: timestamp)
            return age <= staleAfterSeconds
                ? .live(lastEventAgeSeconds: age)
                : .stale(ageSeconds: age)
        default:
            // An unrecognized status word is rendered as a stale unknown,
            // never a guessed live state (TAILING.md discipline).
            return .stale(ageSeconds: ageSeconds(now: now, timestamp: inputs.updatedAt))
        }
    }

    // MARK: - run_doing

    private static func doing(_ inputs: RunSpineInputs) -> String {
        let phase = inputs.activePhaseName ?? "no active phase"
        return "\(statusWord(inputs.status)) - turn \(inputs.turn) - \(phase)"
    }

    /// glossary.rs run_status_label. Unrecognized words render raw (the
    /// evidence-pane rule), never a guessed label.
    static func statusWord(_ raw: String) -> String {
        switch raw {
        case "pending": return "pending"
        case "planned": return "planned"
        case "executing": return "running"
        case "completed": return "completed"
        case "failed": return "failed"
        case "killed": return "killed"
        default: return raw
        }
    }

    // MARK: - run_on_track

    private static func onTrack(_ inputs: RunSpineInputs) -> SpineSnapshot.OnTrack {
        SpineSnapshot.OnTrack(
            spendUsedUSD: inputs.totalSpendUSD,
            spendCeilingUSD: inputs.launchPlanCeilingUSD ?? inputs.stateMaxSpendUSD,
            wallSecondsUsed: inputs.totalWallSeconds,
            wallSecondsCeiling: inputs.launchPlanWallSeconds ?? inputs.stateMaxWallSeconds,
            turnsUsed: inputs.turn)
    }

    // MARK: - run_attention (same order as spine.rs)

    private static func attention(
        _ inputs: RunSpineInputs, alive: SpineSnapshot.Aliveness
    ) -> [SpineSnapshot.Attention] {
        var wrong: [SpineSnapshot.Attention] = []
        if let reason = inputs.pauseReason {
            wrong.append(SpineSnapshot.Attention(kind: .pausedAtCap, summary: reason))
        }
        if inputs.status == "failed" {
            wrong.append(SpineSnapshot.Attention(
                kind: .failure, summary: inputs.failureReason ?? "run failed"))
        }
        if inputs.status == "killed" {
            wrong.append(SpineSnapshot.Attention(
                kind: .killed, summary: inputs.failureReason ?? "run killed"))
        }
        if let message = inputs.newestErrorMessage {
            wrong.append(SpineSnapshot.Attention(kind: .providerError, summary: message))
        }
        if inputs.status == "executing", case .stale(let age) = alive {
            wrong.append(SpineSnapshot.Attention(
                kind: .stall, summary: "no run event for \(age)s"))
        }
        return wrong
    }

    // MARK: - run_primary_action

    private static func primaryAction(_ inputs: RunSpineInputs) -> String {
        switch inputs.status {
        case "pending", "planned", "executing":
            return "deadreckon attach \(inputs.runID)"
        case "failed", "killed":
            return "deadreckon resume \(inputs.runID)"
        case "completed":
            return "deadreckon finish \(inputs.runID)"
        default:
            // The honest fallback for an unknown status: observe, never act.
            return "deadreckon attach \(inputs.runID)"
        }
    }

    private static func ageSeconds(now: Date, timestamp: Date) -> Int {
        max(0, Int(now.timeIntervalSince(timestamp)))
    }
}
