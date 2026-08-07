import Foundation

// APP-4 write-flow state machines. Pure values, unit-tested: the trust rules
// they encode (kill resolves only on the file-backed terminal event; steer
// flips to delivered only on the typed steer_delivered event; the >$50
// acknowledgment flag can only be armed by the typed amount matching;
// promote is gated on the shared proof classifier plus both recorded keys)
// live here so no view can re-derive them loosely.

// MARK: - Spend acknowledgment (G2)

/// The GUI-honest `--i-know-its-a-lot`: a launch budget above $50 swaps the
/// Start button for a type-the-amount confirmation. Bypass is impossible by
/// construction — `armsFlag` is the ONLY expression that authorizes passing
/// the flag, and it is computed from the typed text matching the resolved
/// plan ceiling; there is no settable override.
public struct SpendAcknowledgement: Equatable, Sendable {
    public static let ceilingUSD: Double = 50

    public var capUSD: Double?
    public var typedAmount: String = ""

    public init(capUSD: Double?) {
        self.capUSD = capUSD
    }

    /// The launch needs the acknowledgment at all.
    public var required: Bool {
        guard let capUSD else { return false }
        return capUSD > Self.ceilingUSD
    }

    /// The typed text names the exact cap ("60", "60.00", "$60.00" all
    /// match a $60.00 cap; anything else does not).
    public var typedMatches: Bool {
        guard let capUSD, let typed = Self.parseAmount(typedAmount) else { return false }
        return abs(typed - capUSD) < 0.005
    }

    /// True exactly when the flag may be passed: required AND typed matches.
    public var armsFlag: Bool { required && typedMatches }

    /// The launch may proceed (with or without the flag).
    public var readyToStart: Bool { !required || typedMatches }

    /// Public so the Lay Course sheet can parse the operator's cap field
    /// with identical semantics to the acknowledgment match.
    public static func parseAmount(_ text: String) -> Double? {
        var trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("$") { trimmed.removeFirst() }
        trimmed = trimmed.replacingOccurrences(of: ",", with: "")
        guard !trimmed.isEmpty else { return nil }
        return Double(trimmed)
    }
}

// MARK: - Kill progress (honest kill)

extension JobEventKind {
    /// The supervisor's terminal classification kinds in job-events.jsonl.
    /// Kill resolution accepts any of these (a job being killed can still
    /// terminate as failed/blocked if the cancel raced something else): the
    /// EVENT is the resolution, whatever word it carries.
    public var isTerminalClassification: Bool {
        switch self {
        case .cancelled, .failed, .blocked, .needsReview, .verified,
             .budgetExhausted, .deadlineReached:
            return true
        default:
            return false
        }
    }
}

/// The kill state machine. Amber "cancel requested" enters ONLY from the
/// envelope acceptance; terminal resolution enters ONLY from a job-events
/// terminal event. There is deliberately no fold over an exit code: a kill
/// child exiting 0 proves nothing about the supervised job's cleanup
/// (CancelRequested is sticky, the supervisor writes terminal Cancelled
/// after proven teardown), so no such API exists on this type.
public struct KillProgress: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case idle
        case dispatching
        /// The envelope accepted the cancel: render amber, keep watching.
        case cancelRequested(KillFacts)
        /// Typed machine refusal, authoritative (trust rule 4).
        case refused(ErrorEnvelope)
        /// No envelope landed (G1 carve-out / launch failure): the words are
        /// all there is. NOT amber, NOT terminal.
        case envelopeFree(exitCode: Int32, words: String)
        /// job-events.jsonl reported corrupt (strict-sequence violation or
        /// the file vanished beneath the tail): file-backed resolution is
        /// impossible and the sheet says so instead of waiting forever.
        case resolutionUnavailable(reason: String)
        /// The file-backed terminal event arrived: resolved.
        case terminal(JobEventKind)
    }

    public private(set) var phase: Phase = .idle
    /// Campaign kill cascade: the sub-plan envelopes that preceded the
    /// campaign envelope, surfaced so the sheet can show every kill.
    public private(set) var cascadeEnvelopes: [MutationEnvelope] = []

    public init() {}

    public mutating func dispatched() {
        phase = .dispatching
    }

    /// Fold the verb's machine result. Note what is absent: `.terminal` is
    /// not reachable from here — only `fold(jobEventKind:)` resolves.
    public mutating func fold(result: MutationResult) {
        if case .terminal = phase { return }
        if let refusal = result.refusal {
            phase = .refused(refusal)
            return
        }
        guard let primary = result.primary else {
            phase = .envelopeFree(exitCode: result.exitCode, words: result.envelopeFreeWords)
            return
        }
        cascadeEnvelopes = Array(result.envelopes.dropLast())
        let facts = primary.kill
            ?? KillFacts(signal: "unknown", escalated: false, terminalPhaseObserved: false)
        phase = .cancelRequested(facts)
    }

    /// The ONLY path to `.terminal`: a terminal-classification event read
    /// from job-events.jsonl. A refusal stays sticky (the kill never
    /// happened; whatever terminal event arrives belongs to another story).
    public mutating func fold(jobEventKind: JobEventKind) {
        guard jobEventKind.isTerminalClassification else { return }
        switch phase {
        case .refused, .idle, .terminal:
            return
        case .dispatching, .cancelRequested, .envelopeFree, .resolutionUnavailable:
            phase = .terminal(jobEventKind)
        }
    }

    /// The tail's corrupt verdict (sticky in JSONLTailer): resolution is
    /// impossible and the sheet must say so. Never overwrites a refusal or
    /// an already-observed terminal event.
    public mutating func fold(tailCorruption reason: String) {
        switch phase {
        case .refused, .idle, .terminal:
            return
        case .dispatching, .cancelRequested, .envelopeFree, .resolutionUnavailable:
            phase = .resolutionUnavailable(reason: reason)
        }
    }
}

// MARK: - Steer delivery (queued -> delivered)

/// One typed `steer_delivered` event from events.jsonl (turn_loop.rs: the
/// run loop echoes the inbox entry's `queued_at` back on delivery, which is
/// the correlator against the steer envelope's `queued_at`).
public struct SteerDeliveredFact: Equatable, Sendable {
    public let turn: Int?
    public let queuedAtRaw: String?
    public let preview: String?

    public init(turn: Int?, queuedAtRaw: String?, preview: String?) {
        self.turn = turn
        self.queuedAtRaw = queuedAtRaw
        self.preview = preview
    }
}

/// The steer chip's state machine: queued renders from the envelope facts
/// (inbox_seq), and flips to delivered ONLY when the typed steer_delivered
/// event with the matching queued_at appears in the events tail. A verb
/// refusal after `steerable: true` is authoritative and downgrades the
/// control (trust rule 4). Observed deliveries are BUFFERED, so a typed
/// steer_delivered event tailed while the envelope is still in flight
/// (fast mid-turn delivery racing the submit) still flips the chip the
/// moment the envelope's queued_at lands — order-independent, tested.
public struct SteerDeliveryTracker: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case idle
        case submitting
        case queued(SteerFacts)
        case delivered(SteerFacts, turn: Int?)
        case refused(ErrorEnvelope)
        case envelopeFree(words: String)
    }

    public private(set) var phase: Phase = .idle
    /// Every typed steer_delivered fact observed so far (the store's
    /// cumulative per-run array); matching happens whenever phase and
    /// buffer allow, in either arrival order.
    private var observedDeliveries: [SteerDeliveredFact] = []

    public init() {}

    public mutating func submitted() {
        phase = .submitting
    }

    public mutating func fold(result: MutationResult) {
        if let refusal = result.refusal {
            phase = .refused(refusal)
            return
        }
        guard let primary = result.primary, let facts = primary.steer else {
            phase = .envelopeFree(words: result.envelopeFreeWords)
            return
        }
        phase = .queued(facts)
        // A delivery observed before the envelope landed is already
        // buffered: re-fold immediately so the chip never sticks at queued
        // against a file-backed delivered fact.
        attemptDeliveryMatch()
    }

    /// Observe the typed steer_delivered events (cumulative). Flips queued
    /// -> delivered when one matches this steer's queued_at; otherwise the
    /// facts stay buffered for the fold(result:) that is still in flight.
    public mutating func fold(deliveries: [SteerDeliveredFact]) {
        observedDeliveries = deliveries
        attemptDeliveryMatch()
    }

    /// Raw-string equality first (both sides serialize the same chrono
    /// timestamp); parsed-date equality within 1 ms as the fallback.
    private mutating func attemptDeliveryMatch() {
        guard case .queued(let facts) = phase else { return }
        for delivered in observedDeliveries {
            guard let raw = delivered.queuedAtRaw else { continue }
            let matches: Bool
            if raw == facts.queuedAtRaw {
                matches = true
            } else if let lhs = DeadreckonJSON.date(from: raw), let rhs = facts.queuedAt {
                matches = abs(lhs.timeIntervalSince(rhs)) < 0.001
            } else {
                matches = false
            }
            if matches {
                phase = .delivered(facts, turn: delivered.turn)
                return
            }
        }
    }

    /// Start a fresh steer from any settled state. The delivery buffer is
    /// kept: it mirrors the store's cumulative per-run array, and matching
    /// is by queued_at, so stale entries can never claim a new steer.
    public mutating func reset() {
        phase = .idle
    }
}

// MARK: - Promote gate (Binnacle)

/// The two-key promote gating, from durable facts only: the shared proof
/// classifier off the rollup row (trust rule 6 — the ONLY source of
/// VERIFIED), and the two keys as recorded by `report --json` (APP-3's
/// receipt fallback: the committed binary's `verdict` refuses JOB refs, a
/// registered Rust gap, so the band says "recorded by report", not "freshly
/// re-validated"). PROMOTE stays disabled until all three facts hold, with
/// the missing fact named. The real authority is unchanged either way:
/// `finish` re-validates fail-closed from scratch, and a refusal renders
/// verbatim with no override.
public struct PromoteGate: Equatable, Sendable {
    public let proofValid: Bool
    public let markerKeyPresent: Bool
    public let judgmentKeyPresent: Bool
    /// The recorded semantic decision word (achieved | revise | uncertain),
    /// rendered verbatim next to the key.
    public let judgmentDecision: String?
    public let promoteEnabled: Bool
    /// The first missing fact, named; nil when promote is enabled.
    public let disabledReason: String?

    public static func evaluate(receipt: FleetRow.Receipt?, report: JobReportEnvelope?) -> PromoteGate {
        let proofValid = receipt?.verified == .valid
        let markerPresent = report?.receipt != nil
        let judgment = report?.semantic?.judgment
        let enabled = proofValid && markerPresent && judgment != nil
        var reason: String?
        if !proofValid {
            reason = "the shared proof classifier does not read valid for this job"
        } else if !markerPresent {
            reason = "report --json records no completion receipt block (key 1)"
        } else if judgment == nil {
            reason = "report --json records no semantic judgment (key 2)"
        }
        return PromoteGate(
            proofValid: proofValid,
            markerKeyPresent: markerPresent,
            judgmentKeyPresent: judgment != nil,
            judgmentDecision: judgment?.decision,
            promoteEnabled: enabled,
            disabledReason: reason)
    }
}
