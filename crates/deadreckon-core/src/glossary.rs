//! User-facing vocabulary for deadreckon surfaces.
//!
//! Stored enum variants keep their historical schema names
//! compatibility. These helpers own the words shown to a person.

use deadreckon_protocol::{JobOutcome, JobPhase, StopReason};

use crate::chain::{ChainStatus, ChainStepStatus};
use crate::plan::{PlanStatus, PlanTaskStatus};
use crate::state::{PhaseStatus, RunStatus};

pub const NOUN_RUN: &str = "run";
pub const NOUN_CHAIN: &str = "chain";
pub const NOUN_PLAN: &str = "plan";
pub const NOUN_CHILD: &str = "child";
pub const NOUN_VERIFIED_RUN: &str = "verified run";
pub const PHRASE_VERIFIED_BY_DR_GATE: &str = "verified by dr-gate";
pub const DR_GATE_DESCRIPTION: &str = "the process that verifies the run";
pub const NOUN_DONE_CONTRACT: &str = "done contract";
pub const VERDICT_VERIFIED: &str = "VERIFIED";

pub trait StatusLabel {
    fn status_label(self) -> &'static str;
}

pub fn status_label<S: StatusLabel>(status: S) -> &'static str {
    status.status_label()
}

pub fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Planned => "planned",
        RunStatus::Executing => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Killed => "killed",
    }
}

pub fn phase_status_label(status: PhaseStatus) -> &'static str {
    match status {
        PhaseStatus::Pending => "pending",
        PhaseStatus::Planned => "planned",
        PhaseStatus::Executing => "running",
        PhaseStatus::Completed => "completed",
        PhaseStatus::Failed => "failed",
    }
}

pub fn chain_status_label(status: ChainStatus) -> &'static str {
    match status {
        ChainStatus::Pending => "pending",
        ChainStatus::Running => "running",
        ChainStatus::Paused => "paused",
        ChainStatus::Completed => "completed",
        ChainStatus::Failed => "failed",
        ChainStatus::Killed => "killed",
        ChainStatus::Undone => "undone",
    }
}

pub fn chain_step_status_label(status: ChainStepStatus) -> &'static str {
    match status {
        ChainStepStatus::Pending => "pending",
        ChainStepStatus::Running => "running",
        ChainStepStatus::Completed => "completed",
        ChainStepStatus::Failed => "failed",
        ChainStepStatus::Skipped => "skipped",
        ChainStepStatus::Applied => "applied",
        ChainStepStatus::Undone => "undone",
    }
}

pub fn plan_status_label(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => "pending",
        PlanStatus::Forked => "running",
        PlanStatus::Merged => "completed",
        PlanStatus::Failed => "failed",
    }
}

/// User words for the durable Job lifecycle phase. Machine surfaces emit
/// these alongside the serialized snake_case value so app translations can
/// mirror this table instead of re-authoring vocabulary.
pub fn job_phase_label(phase: JobPhase) -> &'static str {
    match phase {
        JobPhase::Queued => "queued",
        JobPhase::Running => "running",
        JobPhase::VerifyingChecks => "verifying checks",
        JobPhase::VerifyingMeaning => "verifying meaning",
        JobPhase::Waiting => "waiting",
        // No softer word exists for a Job whose lifecycle is over but whose
        // outcome is unset; the serialized label is the honest render.
        JobPhase::Terminal => "terminal",
    }
}

/// User words for the terminal classification of a durable Job.
pub fn job_outcome_label(outcome: JobOutcome) -> &'static str {
    match outcome {
        JobOutcome::Verified => "verified",
        JobOutcome::NeedsReview => "needs review",
        JobOutcome::Blocked => "blocked",
        JobOutcome::BudgetExhausted => "budget exhausted",
        JobOutcome::DeadlineReached => "deadline reached",
        JobOutcome::RetryExhausted => "retry exhausted",
        JobOutcome::Cancelled => "cancelled",
        JobOutcome::Failed => "failed",
    }
}

/// User words for the causal stop reason recorded on a durable Job.
pub fn stop_reason_label(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Verified => "verified",
        StopReason::SemanticRevise => "judge asked for revision",
        StopReason::SemanticUncertain => "judge uncertain",
        StopReason::SemanticUnavailable => "judge unavailable",
        StopReason::OperatorInputRequired => "operator input required",
        StopReason::SpendCap => "paused at spend cap",
        StopReason::WallCap => "paused at wall-clock cap",
        StopReason::Deadline => "deadline",
        StopReason::AttemptLimit => "attempt limit",
        StopReason::CancelRequested => "cancel requested",
        StopReason::DeterministicRevise => "checks asked for revision",
        StopReason::TransientProvider => "transient provider error",
        StopReason::FatalProvider => "fatal provider error",
        StopReason::FatalGate => "fatal gate error",
        StopReason::LostContainment => "lost containment",
        StopReason::SupervisorFailure => "supervisor failure",
        StopReason::CorruptHistory => "corrupt history",
        StopReason::LegacyUnknown => "legacy (unknown reason)",
    }
}

pub fn plan_task_status_label(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Pending => "pending",
        PlanTaskStatus::Running => "running",
        PlanTaskStatus::Completed => "completed",
        PlanTaskStatus::Skipped => "skipped",
        PlanTaskStatus::Failed => "failed",
        PlanTaskStatus::Killed => "killed",
    }
}

impl StatusLabel for RunStatus {
    fn status_label(self) -> &'static str {
        run_status_label(self)
    }
}

impl StatusLabel for PhaseStatus {
    fn status_label(self) -> &'static str {
        phase_status_label(self)
    }
}

impl StatusLabel for ChainStatus {
    fn status_label(self) -> &'static str {
        chain_status_label(self)
    }
}

impl StatusLabel for ChainStepStatus {
    fn status_label(self) -> &'static str {
        chain_step_status_label(self)
    }
}

impl StatusLabel for PlanStatus {
    fn status_label(self) -> &'static str {
        plan_status_label(self)
    }
}

impl StatusLabel for PlanTaskStatus {
    fn status_label(self) -> &'static str {
        plan_task_status_label(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_reason_has_a_user_word_without_raw_enum_casing() {
        for reason in StopReason::ALL {
            let word = stop_reason_label(reason);
            assert!(!word.is_empty());
            assert!(
                !word.contains('_') && word == word.to_lowercase(),
                "stop reason user words must never leak serialized casing: {word:?}"
            );
        }
    }

    #[test]
    fn job_phase_and_outcome_words_carry_no_serialized_casing() {
        let phases = [
            JobPhase::Queued,
            JobPhase::Running,
            JobPhase::VerifyingChecks,
            JobPhase::VerifyingMeaning,
            JobPhase::Waiting,
            JobPhase::Terminal,
        ];
        for phase in phases {
            let word = job_phase_label(phase);
            assert!(!word.is_empty() && !word.contains('_'), "{word:?}");
        }
        let outcomes = [
            JobOutcome::Verified,
            JobOutcome::NeedsReview,
            JobOutcome::Blocked,
            JobOutcome::BudgetExhausted,
            JobOutcome::DeadlineReached,
            JobOutcome::RetryExhausted,
            JobOutcome::Cancelled,
            JobOutcome::Failed,
        ];
        for outcome in outcomes {
            let word = job_outcome_label(outcome);
            assert!(!word.is_empty() && !word.contains('_'), "{word:?}");
        }
    }
}
