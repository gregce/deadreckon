//! Steer eligibility as data (operator-console gap G6).
//!
//! The predicate that decides whether a Run can accept live steering lives
//! here, in exactly one place. The `steer` command guard and the JSON
//! projections (`status --json`, `show --json`, `RunView`) all call it, so
//! the CLI and any app reading the JSON can never disagree. Widening
//! steering to more providers later is a change to this module only.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::state::{PipelineState, RunStatus};

/// The only provider route that accepts live mid-turn steering today.
pub const STEERABLE_PROVIDER_ROUTE: &str = "cli:codex-server";

/// Why a Run cannot accept steering right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SteerIneligibleReason {
    /// The Run is owned by a durable Job, so a public caller's steer is
    /// refused; only the live job driver may mutate it.
    DriverFenced,
    /// The Run is not currently executing a provider turn.
    NotExecuting,
    /// The Run's provider route cannot accept live steering.
    ProviderNotSteerable,
}

/// Whether a Run can accept steering, as data.
///
/// Serializes as `{"steerable": bool, "reason": string|null}` with
/// snake_case reason labels, the shape published on every JSON surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SteerEligibility {
    pub steerable: bool,
    pub reason: Option<SteerIneligibleReason>,
}

/// Steer eligibility as seen by a public (non-driver) caller.
///
/// This is what the JSON projections publish: a Job-owned Run (stamped
/// ownership) reports `driver_fenced` because the public `steer` verb would
/// refuse it. The steer command itself resolves the fence with full plan
/// lineage (so its refusal can name the owning Job) and then calls
/// [`steer_eligibility_with_driver_fence`] with the fence outcome.
pub fn steer_eligibility(state: &PipelineState) -> SteerEligibility {
    steer_eligibility_with_driver_fence(state, state.ownership.is_some())
}

/// Steer eligibility with the driver-fence outcome supplied by the caller.
///
/// Reason precedence mirrors the historical steer guard: driver fence,
/// then run status, then provider route.
pub fn steer_eligibility_with_driver_fence(
    state: &PipelineState,
    driver_fenced: bool,
) -> SteerEligibility {
    let reason = if driver_fenced {
        Some(SteerIneligibleReason::DriverFenced)
    } else if state.status != RunStatus::Executing {
        Some(SteerIneligibleReason::NotExecuting)
    } else if state.provider.as_deref() != Some(STEERABLE_PROVIDER_ROUTE) {
        Some(SteerIneligibleReason::ProviderNotSteerable)
    } else {
        None
    };
    SteerEligibility {
        steerable: reason.is_none(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::state::{RunOptions, RunOwnership, RunStatus, create_run};
    use crate::{DeadreckonPaths, PipelineState};

    use super::{
        STEERABLE_PROVIDER_ROUTE, SteerIneligibleReason, steer_eligibility,
        steer_eligibility_with_driver_fence,
    };

    fn fixture_state(temp: &TempDir) -> PipelineState {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        create_run(
            &paths,
            RunOptions {
                goal: "steer eligibility fixture".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some(STEERABLE_PROVIDER_ROUTE.to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("steer-eligibility-fixture".to_string()),
                codebase: None,
            },
        )
        .expect("run")
    }

    #[test]
    fn executing_codex_server_run_is_steerable_with_no_reason() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;

        let eligibility = steer_eligibility(&state);

        assert!(eligibility.steerable);
        assert_eq!(eligibility.reason, None);
    }

    #[test]
    fn non_executing_run_reports_not_executing() {
        let temp = TempDir::new().expect("tempdir");
        let state = fixture_state(&temp);
        assert_ne!(state.status, RunStatus::Executing);

        let eligibility = steer_eligibility(&state);

        assert!(!eligibility.steerable);
        assert_eq!(
            eligibility.reason,
            Some(SteerIneligibleReason::NotExecuting)
        );
    }

    #[test]
    fn non_codex_server_provider_reports_provider_not_steerable() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;
        state.provider = Some("smoke".to_string());

        let eligibility = steer_eligibility(&state);

        assert!(!eligibility.steerable);
        assert_eq!(
            eligibility.reason,
            Some(SteerIneligibleReason::ProviderNotSteerable)
        );
    }

    #[test]
    fn missing_provider_route_reports_provider_not_steerable() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;
        state.provider = None;

        let eligibility = steer_eligibility(&state);

        assert_eq!(
            eligibility.reason,
            Some(SteerIneligibleReason::ProviderNotSteerable)
        );
    }

    #[test]
    fn job_owned_run_reports_driver_fenced_to_public_callers() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;
        state.ownership = Some(RunOwnership::plan_task("job-1", "plan-1", "task-1", 0, 1));

        let eligibility = steer_eligibility(&state);

        assert!(!eligibility.steerable);
        assert_eq!(
            eligibility.reason,
            Some(SteerIneligibleReason::DriverFenced)
        );
    }

    #[test]
    fn caller_supplied_fence_outcome_overrides_the_ownership_stamp() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;
        state.ownership = Some(RunOwnership::plan_task("job-1", "plan-1", "task-1", 0, 1));

        // The steer command resolves the fence itself; once it passed, the
        // ownership stamp must not re-fence the live driver.
        let eligibility = steer_eligibility_with_driver_fence(&state, false);

        assert!(eligibility.steerable);
        assert_eq!(eligibility.reason, None);
    }

    #[test]
    fn eligibility_serializes_as_the_published_json_shape() {
        let temp = TempDir::new().expect("tempdir");
        let mut state = fixture_state(&temp);
        state.status = RunStatus::Executing;

        let steerable = serde_json::to_value(steer_eligibility(&state)).expect("json");
        assert_eq!(
            steerable,
            serde_json::json!({"steerable": true, "reason": null})
        );

        state.status = RunStatus::Completed;
        let blocked = serde_json::to_value(steer_eligibility(&state)).expect("json");
        assert_eq!(
            blocked,
            serde_json::json!({"steerable": false, "reason": "not_executing"})
        );
    }
}
