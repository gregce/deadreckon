//! User-facing vocabulary for deadreckon surfaces.
//!
//! Stored enum variants keep their historical schema names
//! compatibility. These helpers own the words shown to a person.

use crate::chain::{ChainStatus, ChainStepStatus};
use crate::plan::{PlanStatus, PlanTaskStatus};
use crate::state::{PhaseStatus, RunStatus};

pub const NOUN_RUN: &str = "run";
pub const NOUN_CHAIN: &str = "chain";
pub const NOUN_PLAN: &str = "plan";
pub const NOUN_CHILD: &str = "child";

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

pub fn plan_task_status_label(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Pending => "pending",
        PlanTaskStatus::Running => "running",
        PlanTaskStatus::Completed => "completed",
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
