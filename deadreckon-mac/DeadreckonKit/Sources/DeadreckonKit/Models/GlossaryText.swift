import Foundation

/// The user-facing vocabulary for every deadreckon surface in the app. Raw
/// enum names never reach the UI: views ask this helper for words.
///
/// Two tiers of provenance, kept honest:
/// - The named constants below mirror `crates/deadreckon-core/src/glossary.rs`
///   verbatim.
/// - The phase/outcome/stop-reason words are APP-AUTHORED translations of the
///   binary's serialized labels: glossary.rs has no vocabulary for
///   JobPhase/JobOutcome/StopReason yet (the CLI renders raw snake_case), so
///   these phrasings originate here, one word-per-label, never predictive.
///   When `job_phase_label`/`job_outcome_label`/`stop_reason_label` land in
///   glossary.rs (tracked in CONTRACTS.md as a Rust-side gap), this file
///   mirrors them and any drift is an app bug.
public enum GlossaryText {
    // MARK: Glossary constants (glossary.rs verbatim)

    public static let nounRun = "run"
    public static let nounChain = "chain"
    public static let nounPlan = "plan"
    public static let nounChild = "child"
    public static let nounVerifiedRun = "verified run"
    public static let phraseVerifiedByDrGate = "verified by dr-gate"
    public static let nounDoneContract = "done contract"
    /// The one word that may claim verification, and only from the shared
    /// proof classifier (`receipt.verified == valid`). Trust rule 6.
    public static let verdictVerified = "VERIFIED"

    /// The honest render for any `.unknown` enum case (CONTRACTS invariant:
    /// never a guessed state).
    public static let unknownState = "unknown state"

    // MARK: Phase words (app-authored translations; see the type comment)

    public static func phaseWord(_ phase: JobPhase) -> String {
        switch phase {
        case .queued: return "queued"
        case .running: return "running"
        case .verifyingChecks: return "verifying checks"
        case .verifyingMeaning: return "verifying meaning"
        case .waiting: return "waiting"
        // The binary's own serialized label; the glossary has no softer word
        // for a Job whose lifecycle is over but whose outcome is unset.
        case .terminal: return "terminal"
        case .unknown: return unknownState
        }
    }

    // MARK: Outcome words (app-authored translations; see the type comment)

    public static func outcomeWord(_ outcome: JobOutcome) -> String {
        switch outcome {
        case .verified: return "verified"
        case .needsReview: return "needs review"
        case .blocked: return "blocked"
        case .budgetExhausted: return "budget exhausted"
        case .deadlineReached: return "deadline reached"
        case .retryExhausted: return "retry exhausted"
        case .cancelled: return "cancelled"
        case .failed: return "failed"
        case .unknown: return unknownState
        }
    }

    // MARK: Stop-reason words (app-authored translations; see the type comment)

    public static func stopReasonWord(_ reason: StopReason) -> String {
        switch reason {
        case .verified: return "verified"
        case .semanticRevise: return "judge asked for revision"
        case .semanticUncertain: return "judge uncertain"
        case .semanticUnavailable: return "judge unavailable"
        case .operatorInputRequired: return "operator input required"
        case .spendCap: return "paused at spend cap"
        case .wallCap: return "paused at wall-clock cap"
        case .deadline: return "deadline"
        case .attemptLimit: return "attempt limit"
        case .cancelRequested: return "cancel requested"
        case .deterministicRevise: return "checks asked for revision"
        case .transientProvider: return "transient provider error"
        case .fatalProvider: return "fatal provider error"
        case .fatalGate: return "fatal gate error"
        case .lostContainment: return "lost containment"
        case .supervisorFailure: return "supervisor failure"
        case .corruptHistory: return "corrupt history"
        case .legacyUnknown: return "legacy (unknown reason)"
        case .unknown: return unknownState
        }
    }

    // MARK: Proof words

    /// Words for the three-valued proof classifier. `valid` is the ONLY path
    /// to the word VERIFIED anywhere in the app (trust rule 6); `invalid` is
    /// its own rendered state, never collapsed into Verified (trust rule 5).
    public static func proofWord(_ proof: ProofStatus) -> String {
        switch proof {
        case .valid: return verdictVerified
        case .invalid: return "proof invalid"
        case .notApplicable: return "no proof expected"
        case .unknown: return unknownState
        }
    }

    // MARK: Rust status labels

    /// Translates a `job_status_label` glossary word (FleetRow.status) into
    /// user words. The Rust side emits serialized outcome/phase labels plus
    /// the special "verified_proof_invalid"; anything unrecognized renders
    /// with underscores spaced, never dropped and never re-worded.
    public static func statusWord(_ raw: String) -> String {
        if raw == "verified_proof_invalid" { return "proof invalid" }
        if let outcome = JobOutcome(rawValue: raw), outcome != .unknown {
            return outcomeWord(outcome)
        }
        if let phase = JobPhase(rawValue: raw), phase != .unknown {
            return phaseWord(phase)
        }
        return raw.replacingOccurrences(of: "_", with: " ")
    }

    // MARK: Durable-fact strings (decision 8: no forward-looking estimates)

    /// "$9.12 / $25.00" when a cap exists, "$9.12" otherwise. Facts only.
    public static func spendLine(_ spend: FleetRow.Spend) -> String {
        if let cap = spend.capUSD {
            return String(format: "$%.2f / $%.2f", spend.totalCostUSD, cap)
        }
        return String(format: "$%.2f", spend.totalCostUSD)
    }

    /// A stale lease dot must say WHY (design doc debounce caution): the
    /// durable fact is the recorded heartbeat age, so that is the sentence.
    public static func leaseStaleReason(_ lease: FleetRow.Lease) -> String {
        "no heartbeat \(lease.heartbeatAgeSeconds)s"
    }

    /// "5/5 checks" from a signed acceptance marker. Callers must only pass
    /// a gate that is present on the row (trust rule 6: no marker, no counts).
    public static func gateCounts(_ gate: FleetRow.Gate) -> String {
        "\(gate.nPassed)/\(gate.nTotal) checks"
    }

    // MARK: Job event words (B1: glossary user words, never raw enum names)

    /// User words for a job-events.jsonl kind (the kill sheet's terminal
    /// line). Underscores spaced for the known vocabulary — one mechanical
    /// translation in one place, so "budget_exhausted" can never reach the
    /// UI raw; `.unknown` renders the honest unknown-state words. Note this
    /// deliberately does NOT route through `verdictVerified`: a verified
    /// terminal EVENT word is lowercase "verified" — the VERIFIED chip
    /// stays with the proof classifier alone (trust rule 6).
    public static func jobEventWord(_ kind: JobEventKind) -> String {
        if kind == .unknown { return unknownState }
        return kind.rawValue.replacingOccurrences(of: "_", with: " ")
    }

    /// User words for a provider probe status (Lay Course route rows).
    public static func providerProbeWord(_ status: ProviderProbeStatus) -> String {
        if status == .unknown { return unknownState }
        return status.rawValue
    }
}
