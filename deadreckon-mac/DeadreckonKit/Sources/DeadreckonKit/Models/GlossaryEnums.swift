import Foundation

/// String-backed enums decoded from deadreckon JSON surfaces.
///
/// Decoding is forgiving by contract: an unrecognized string decodes to
/// `.unknown`, never a decode failure. The binary's vocabulary may grow ahead
/// of the app (a newer vendored CLI, or a newer `DEADRECKON_HOME`); an unknown
/// word must degrade to an honest "unknown" render, not a crash or a guessed
/// state. A non-string value in the field still throws, because that is a
/// schema violation, not a vocabulary extension.
public protocol ForgivingStringEnum: RawRepresentable, Codable, Equatable, Sendable
where RawValue == String {
    static var unknown: Self { get }
}

extension ForgivingStringEnum {
    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// `JobPhase` (deadreckon-protocol job.rs, serde snake_case).
public enum JobPhase: String, ForgivingStringEnum, CaseIterable {
    case queued
    case running
    case verifyingChecks = "verifying_checks"
    case verifyingMeaning = "verifying_meaning"
    case waiting
    case terminal
    case unknown
}

/// `JobOutcome` (deadreckon-protocol job.rs). Terminal classification,
/// separate from both phase and causal stop reason.
public enum JobOutcome: String, ForgivingStringEnum, CaseIterable {
    case verified
    case needsReview = "needs_review"
    case blocked
    case budgetExhausted = "budget_exhausted"
    case deadlineReached = "deadline_reached"
    case retryExhausted = "retry_exhausted"
    case cancelled
    case failed
    case unknown
}

/// `StopReason` (deadreckon-protocol job.rs). Why execution stopped.
public enum StopReason: String, ForgivingStringEnum, CaseIterable {
    case verified
    case semanticRevise = "semantic_revise"
    case semanticUncertain = "semantic_uncertain"
    case semanticUnavailable = "semantic_unavailable"
    case operatorInputRequired = "operator_input_required"
    case spendCap = "spend_cap"
    case wallCap = "wall_cap"
    case deadline
    case attemptLimit = "attempt_limit"
    case cancelRequested = "cancel_requested"
    case deterministicRevise = "deterministic_revise"
    case transientProvider = "transient_provider"
    case fatalProvider = "fatal_provider"
    case fatalGate = "fatal_gate"
    case lostContainment = "lost_containment"
    case supervisorFailure = "supervisor_failure"
    case corruptHistory = "corrupt_history"
    case legacyUnknown = "legacy_unknown"
    case unknown
}

/// The shared proof-status classifier (`job_proof_status`, commands/job.rs):
/// whether a Verified outcome still has a valid signed receipt behind it.
/// NOT a boolean. `list --json` receipt.verified and `status --json` ride the
/// same Rust helper, so the two can never disagree; this enum is the Swift
/// mirror of that single vocabulary.
public enum ProofStatus: String, ForgivingStringEnum, CaseIterable {
    case valid
    case invalid
    case notApplicable = "not-applicable"
    case unknown
}

/// `SteerIneligibleReason` (run-view.schema.json).
public enum SteerIneligibleReason: String, ForgivingStringEnum, CaseIterable {
    case driverFenced = "driver_fenced"
    case notExecuting = "not_executing"
    case providerNotSteerable = "provider_not_steerable"
    case unknown
}

/// `JobEventKind` (job-event.schema.json definitions).
public enum JobEventKind: String, ForgivingStringEnum, CaseIterable {
    case created
    case contractApproved = "contract_approved"
    case queued
    case leaseAcquired = "lease_acquired"
    case leaseReclaimed = "lease_reclaimed"
    case workspacePrepared = "workspace_prepared"
    case childLaunchPrepared = "child_launch_prepared"
    case attemptStarted = "attempt_started"
    case childLinked = "child_linked"
    case campaignSubAuthorityChanged = "campaign_sub_authority_changed"
    case repairChildAuthorityChanged = "repair_child_authority_changed"
    case attemptStopped = "attempt_stopped"
    case retryScheduled = "retry_scheduled"
    case deterministicGatePassed = "deterministic_gate_passed"
    case deterministicGateFailed = "deterministic_gate_failed"
    case semanticJudgeAchieved = "semantic_judge_achieved"
    case semanticJudgeRevise = "semantic_judge_revise"
    case semanticJudgeUncertain = "semantic_judge_uncertain"
    case needsReview = "needs_review"
    case blocked
    case budgetExhausted = "budget_exhausted"
    case deadlineReached = "deadline_reached"
    case cancelRequested = "cancel_requested"
    case cancelled
    case failed
    case verified
    case resultApplied = "result_applied"
    case resultExported = "result_exported"
    case undoStarted = "undo_started"
    case undoCompleted = "undo_completed"
    case undoFailed = "undo_failed"
    case unknown
}

/// `notify.jsonl` transition labels (docs/TAILING.md).
public enum NotifyTransition: String, ForgivingStringEnum, CaseIterable {
    case accepted
    case paused
    case failed
    case unknown
}
