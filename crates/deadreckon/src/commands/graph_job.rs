//! Trusted drivers that place existing orchestration engines under one Job.
//!
//! The driver specification is embedded in the signed launch plan. The
//! mutable sidecar records which existing Plan or Campaign artifact belongs to
//! the parent Job; it is navigation evidence, never completion authority.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::Utc;
use deadreckon_protocol::{
    CompletionReceipt, JobId, JobShape, SemanticDecision, SpendRecord, StopReason, TraceRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::*;

const DRIVER_SIGNAL: &str = "watchkeeper_driver";
const DRIVER_STATE_FILE: &str = "driver.json";
const PLAN_PLANNER_ACCOUNTING_FILE: &str = "root-planner-accounting.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DriverKind {
    Review,
    FullPlan,
    Campaign,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DriverSpec {
    pub(crate) kind: DriverKind,
    pub(crate) child_count: Option<u8>,
    pub(crate) apply: deadreckon_core::plan::ApplyWhen,
    pub(crate) planner_provider: Option<String>,
    pub(crate) child_provider: Option<String>,
    pub(crate) child_provider_overrides: Vec<String>,
    pub(crate) coder_provider: Option<String>,
    pub(crate) reviewer_provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) source_init_git: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriverState {
    pub(crate) schema_version: u32,
    pub(crate) job_id: JobId,
    pub(crate) kind: DriverKind,
    pub(crate) artifact_kind: String,
    pub(crate) artifact_id: String,
    pub(crate) recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct DriverContext {
    job_id: String,
    acceptance_path: PathBuf,
}

static DRIVER_CONTEXT: OnceLock<DriverContext> = OnceLock::new();

#[derive(Debug)]
pub(crate) enum ParentCompletion {
    Verified(Box<CompletionReceipt>),
    NeedsReview {
        reason: String,
        decision: Option<SemanticDecision>,
        stop_reason: StopReason,
    },
    BudgetExhausted {
        reason: String,
        stop_reason: StopReason,
    },
    GateFailed(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ParentExecutionUsage {
    spend_usd: f64,
    wall_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PlanPlannerAccounting {
    schema_version: u32,
    planner_invoked: bool,
    provider: Option<String>,
    model: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    subscription: bool,
    wall_seconds: f64,
    recorded_at: chrono::DateTime<Utc>,
}

pub(crate) const fn semantic_decision_stop_reason(
    decision: Option<SemanticDecision>,
) -> Option<StopReason> {
    match decision {
        Some(SemanticDecision::Revise) => Some(StopReason::SemanticRevise),
        Some(SemanticDecision::Uncertain) => Some(StopReason::SemanticUncertain),
        Some(SemanticDecision::Achieved) | None => None,
    }
}

pub(crate) fn embed_driver_spec(
    plan: &mut commands::course::LaunchPlan,
    driver: &DriverSpec,
) -> Result<()> {
    let mut driver = driver.clone();
    if matches!(
        driver.kind,
        DriverKind::Review | DriverKind::FullPlan | DriverKind::Campaign
    ) {
        // A PerNode plan mutates the approved source while the durable parent
        // is still being verified. Strict Graph Jobs therefore always merge
        // in isolation and expose one receipt-bound parent result at the end.
        driver.apply = deadreckon_core::plan::ApplyWhen::AtEnd;
    }
    let mut signals = plan.signals.as_object().cloned().unwrap_or_default();
    signals.insert(DRIVER_SIGNAL.to_string(), serde_json::to_value(&driver)?);
    plan.signals = serde_json::Value::Object(signals);
    Ok(())
}

pub(crate) fn driver_spec(plan: &commands::course::LaunchPlan) -> Result<DriverSpec> {
    let value = plan.signals.get(DRIVER_SIGNAL).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "advanced job launch plan is missing its immutable driver specification".to_string(),
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(CliError::from)
}

pub(crate) fn driver_state_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(DRIVER_STATE_FILE)
}

pub(crate) fn record_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
    accounting: Option<&commands::plan::PlannerAccounting>,
) -> Result<()> {
    let record = PlanPlannerAccounting {
        schema_version: 1,
        planner_invoked: accounting.is_some(),
        provider: accounting.map(|value| value.spend.provider.clone()),
        model: accounting.map(|value| value.spend.model.clone()),
        input_tokens: accounting.map_or(0, |value| value.spend.input_tokens),
        output_tokens: accounting.map_or(0, |value| value.spend.output_tokens),
        cost_usd: accounting.map_or(0.0, |value| value.spend.cost_usd),
        subscription: accounting.is_some_and(|value| value.spend.subscription),
        wall_seconds: accounting.map_or(0.0, |value| value.wall_seconds),
        recorded_at: Utc::now(),
    };
    super::job::write_json_synced(
        &paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE),
        &record,
    )
}

pub(crate) fn load_driver_state(paths: &DeadreckonPaths, job_id: &str) -> Result<DriverState> {
    let path = driver_state_path(paths, job_id);
    serde_json::from_slice(&fs::read(&path)?).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.clone(),
            source,
        })
    })
}

pub(crate) fn current_parent_job_id() -> Option<&'static str> {
    DRIVER_CONTEXT.get().map(|context| context.job_id.as_str())
}

pub(crate) fn current_acceptance_path() -> Option<&'static Path> {
    DRIVER_CONTEXT
        .get()
        .map(|context| context.acceptance_path.as_path())
}

pub(crate) fn record_current_artifact(
    paths: &DeadreckonPaths,
    kind: DriverKind,
    artifact_kind: &str,
    artifact_id: &str,
) -> Result<()> {
    let Some(job_id) = current_parent_job_id() else {
        return Ok(());
    };
    if job_id != artifact_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced driver artifact {artifact_id} does not retain parent job identity {job_id}"
        ))));
    }
    let state = DriverState {
        schema_version: 1,
        job_id: JobId(job_id.to_string()),
        kind,
        artifact_kind: artifact_kind.to_string(),
        artifact_id: artifact_id.to_string(),
        recorded_at: Utc::now(),
    };
    let path = driver_state_path(paths, job_id);
    if path.exists() {
        let existing = load_driver_state(paths, job_id)?;
        if existing.job_id != state.job_id
            || existing.kind != state.kind
            || existing.artifact_kind != state.artifact_kind
            || existing.artifact_id != state.artifact_id
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "advanced artifact mapping for job {job_id} changed across attempts"
            ))));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &state)?;
    Ok(())
}

pub(crate) async fn drive_job_command(job_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if std::env::var(commands::run::TRUSTED_SUPERVISOR_JOB_ID_ENV).as_deref() != Ok(job_id.as_str())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced job drivers may only be launched by the durable supervisor".to_string(),
        )));
    }
    let job = deadreckon_core::load_job(&paths, &job_id)?;
    if job.job_id.as_ref() != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced driver job identity mismatch".to_string(),
        )));
    }
    let plan = commands::course::load_launch_plan(&paths.job_launch_plan(&job_id))?;
    let driver = driver_spec(&plan)?;
    DRIVER_CONTEXT
        .set(DriverContext {
            job_id: job_id.clone(),
            acceptance_path: commands::job::job_acceptance_path(&paths, &job_id),
        })
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "this process is already driving another durable job".to_string(),
            ))
        })?;
    std::env::set_current_dir(&job.source_cwd)?;

    match driver.kind {
        DriverKind::Review | DriverKind::FullPlan => {
            if job.shape != JobShape::Graph {
                return Err(driver_shape_error(&job_id));
            }
            drive_plan(&paths, &job, plan, driver).await
        }
        DriverKind::Campaign => {
            if job.shape != JobShape::LegacyCampaign {
                return Err(driver_shape_error(&job_id));
            }
            drive_campaign(&paths, &job, driver).await
        }
    }
}

async fn drive_plan(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    plan: commands::course::LaunchPlan,
    driver: DriverSpec,
) -> Result<()> {
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(paths.job_authority(job.job_id.as_ref()))?)?;
    let execution = commands::orchestrate::durable_orchestration_spec(&plan)?.unwrap_or_default();
    if driver_state_path(paths, job.job_id.as_ref()).is_file() {
        return resume_plan(paths, job, &authority, driver, execution).await;
    }
    let mode = match driver.kind {
        DriverKind::Review => CliPlanMode::Review,
        DriverKind::FullPlan => CliPlanMode::FullPlan,
        DriverKind::Campaign => unreachable!("campaign has its own driver"),
    };
    let n = match mode {
        CliPlanMode::Review => 2,
        CliPlanMode::FullPlan => driver.child_count.unwrap_or(3),
    };
    commands::orchestrate::orchestrate_command(commands::orchestrate::OrchestrateRunArgs {
        seed_pieces: plan.pieces,
        accepted_launch_plan: None,
        plan: PlanCommandArgs {
            goal: job.goal.clone(),
            n,
            mode,
            apply: driver.apply,
            max_spend: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            sandbox: Some(authority.sandbox_requested),
            planner_provider: driver.planner_provider,
            provider: driver.child_provider,
            child_provider: driver.child_provider_overrides,
            coder_provider: driver.coder_provider,
            reviewer_provider: driver.reviewer_provider,
            planner_model: execution.planner_model,
            model: driver.model,
            child_model: execution.child_models,
            coder_model: execution.coder_model,
            reviewer_model: execution.reviewer_model,
            init_git: driver.source_init_git,
            acceptance: Some(commands::job::job_acceptance_path(
                paths,
                job.job_id.as_ref(),
            )),
            skip_acceptance_prompt: true,
            no_hints: true,
            quiet: true,
            json: false,
            plain: true,
        },
        preview: false,
        yes: true,
        no_repair: execution.no_repair,
        completion_surface: false,
        narrate: execution.narrate,
        narrator_model: execution.narrator_model,
    })
    .await
}

async fn resume_plan(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    driver: DriverSpec,
    execution: commands::orchestrate::DurableOrchestrationSpec,
) -> Result<()> {
    use deadreckon_core::plan::{PlanStatus, PlanTaskStatus};

    let plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
    match plan.status {
        PlanStatus::Merged => return Ok(()),
        PlanStatus::Failed => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "graph job {} has a failed plan and cannot be resumed automatically",
                job.job_id
            ))));
        }
        PlanStatus::Pending | PlanStatus::Forked => {}
    }
    let all_children_completed = plan
        .tasks
        .iter()
        .all(|task| task.status == PlanTaskStatus::Completed);
    if !all_children_completed {
        commands::plan::fork_command(ForkCommandArgs {
            plan_id: job.job_id.as_ref().to_string(),
            max_spend: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            sandbox: Some(authority.sandbox_requested.clone()),
            provider: driver.child_provider,
            child_provider: driver.child_provider_overrides,
            coder_provider: driver.coder_provider,
            reviewer_provider: driver.reviewer_provider,
            no_repair: execution.no_repair,
            repair_provider: None,
            yes: true,
            no_hints: true,
            quiet: true,
            plain: true,
            completion_surface: false,
            narrate: execution.narrate,
            narrator_model: execution.narrator_model.clone(),
        })
        .await?;
    }
    commands::merge::merge_command(MergeCommandArgs {
        plan_id: job.job_id.as_ref().to_string(),
        strategy: "dag-aware".to_string(),
        prefer_child: None,
        no_repair: execution.no_repair,
        repair_provider: None,
        repair_mode: "auto".to_string(),
        repair_attempts: 1,
        yes: true,
        no_gate: false,
        no_hints: true,
        quiet: true,
        plain: true,
        completion_surface: false,
    })
    .await
}

async fn drive_campaign(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    driver: DriverSpec,
) -> Result<()> {
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(paths.job_authority(job.job_id.as_ref()))?)?;
    let args = commands::campaign::CampaignArgs {
        goal: job.goal.clone(),
        n: driver.child_count,
        planner_provider: driver.planner_provider,
        provider: driver.child_provider,
        planner_model: None,
        model: driver.model,
        max_spend: Some(job.policy.max_spend_usd),
        max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
        sandbox: Some(authority.sandbox_requested),
        acceptance: Some(commands::job::job_acceptance_path(
            paths,
            job.job_id.as_ref(),
        )),
        preview: false,
        yes: true,
        no_hints: true,
        quiet: true,
        plain: true,
        narrate: false,
        no_narrate: true,
        narrator_model: None,
    };
    if driver_state_path(paths, job.job_id.as_ref()).is_file() {
        commands::campaign::resume_campaign_job(paths, job, &args).await
    } else {
        commands::campaign::campaign_command(args).await
    }
}

/// Verify a merged graph as the parent Job's result.
///
/// The orchestration merge run is a compatibility artifact with its own
/// random identity and a synthetic marker. It is useful evidence, but it is
/// not completion authority. This function copies that actual merged result
/// into a run whose identity is the durable Job, executes the native gate
/// there, asks a fresh read-only semantic judge, and seals with the existing
/// two-key receipt implementation.
pub(crate) async fn complete_merged_plan_parent(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ParentCompletion> {
    let merged_run_id = plan.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged graph {} has no evidence-bearing result run",
            job.job_id
        )))
    })?;
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref())
        && let Ok(receipt) = deadreckon_core::validate_completion_receipt(paths, &existing)
    {
        let receipt = validate_and_promote_parent(paths, &mut existing, &receipt)?;
        return Ok(ParentCompletion::Verified(Box::new(receipt)));
    }

    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    if merged.status != deadreckon_core::RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged graph result {merged_run_id} is not completed"
        ))));
    }
    if let Ok(existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        verify_parent_result_identity(job, &existing, &merged)?;
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("NEEDS_REVIEW: "))
        {
            let decision = persisted_semantic_judgment(&existing)
                .ok()
                .flatten()
                .map(|judgment| judgment.decision);
            let stop_reason = semantic_decision_stop_reason(decision).unwrap_or_else(|| {
                if reason.contains("not contained") {
                    StopReason::LostContainment
                } else {
                    StopReason::SemanticUnavailable
                }
            });
            return Ok(ParentCompletion::NeedsReview {
                reason: reason.to_string(),
                decision,
                stop_reason,
            });
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("deterministic graph gate failed: "))
        {
            return Ok(ParentCompletion::GateFailed(reason.to_string()));
        }
    }
    let mut parent = prepare_parent_result_run(paths, job, authority, &merged)?;
    let backend = authority
        .sandbox_requested
        .parse::<deadreckon_sandbox::SandboxBackend>()
        .map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "approved sandbox backend is invalid: {error}"
            )))
        })?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let marker = if marker_path.exists() {
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic graph proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    } else {
        if let Err(error) = deadreckon_runtime::run_deterministic_completion_gate(&parent, backend)
        {
            parent.failure_reason = Some(format!("deterministic graph gate failed: {error}"));
            parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
            deadreckon_core::save_state(&parent)?;
            return Ok(ParentCompletion::GateFailed(error.to_string()));
        }
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic graph proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    };
    if !marker.is_native_gate_proof() {
        return Ok(ParentCompletion::GateFailed(
            "merged graph did not produce a native dr-gate proof".to_string(),
        ));
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return parent_needs_review(
            &mut parent,
            "deterministic checks passed, but the graph result was not contained",
            None,
            StopReason::LostContainment,
        );
    }
    let persisted_achieved = persisted_semantic_judgment(&parent)?
        .filter(|judgment| judgment.decision == SemanticDecision::Achieved);
    let execution_usage = match plan_execution_usage(paths, plan) {
        Ok(usage) => usage,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("graph budget accounting is incomplete or corrupt: {error}"),
                None,
                StopReason::CorruptHistory,
            );
        }
    };
    let current_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(judgment) = persisted_achieved {
        if let Some((stop_reason, reason)) = semantic_budget_overrun(job, current_usage) {
            return parent_budget_exhausted(&mut parent, stop_reason, &reason);
        }
        return seal_achieved_parent(paths, &mut parent, authority, &marker, &judgment);
    }
    if let Some((stop_reason, reason)) = semantic_budget_exhaustion(job, current_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }

    let router = match semantic_router(paths, plan) {
        Ok(router) => router,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    let semantic = match deadreckon_runtime::run_semantic_judge_against_source_with_budget(
        &parent,
        &marker,
        &router,
        backend,
        &job.source_cwd,
        remaining_semantic_budget(job, current_usage),
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    record_parent_semantic_accounting(&mut parent, job, &semantic)?;
    let final_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(dimension) = semantic.budget_exhaustion {
        let (stop_reason, reason) = semantic_judge_budget_exhaustion(job, final_usage, dimension);
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some((stop_reason, reason)) = semantic_budget_overrun(job, final_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some(judgment) = semantic.result.judgment()
        && let Err(error) =
            deadreckon_runtime::persist_semantic_judgment(&parent.run_root, judgment)
    {
        return parent_needs_review(
            &mut parent,
            &format!("strict semantic judgment could not be persisted after accounting: {error}"),
            None,
            StopReason::SemanticUnavailable,
        );
    }
    match semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(judgment) => {
            seal_achieved_parent(paths, &mut parent, authority, &marker, &judgment)
        }
        deadreckon_runtime::SemanticJudgeResult::Revise(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge requested revision: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Revise),
            StopReason::SemanticRevise,
        ),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge was uncertain: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Uncertain),
            StopReason::SemanticUncertain,
        ),
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            parent_needs_review(&mut parent, &reason, None, StopReason::SemanticUnavailable)
        }
    }
}

pub(crate) async fn complete_merged_campaign_parent(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<ParentCompletion> {
    let merged_run_id = campaign.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged campaign {} has no evidence-bearing result run",
            job.job_id
        )))
    })?;
    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    if merged.status != deadreckon_core::RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged campaign result {merged_run_id} is not completed"
        ))));
    }
    let rollup = match validate_campaign_rollup(paths, campaign, &merged) {
        Ok(rollup) => rollup,
        Err(error) => {
            return Ok(ParentCompletion::GateFailed(format!(
                "campaign no-laundering rollup refused: {error}"
            )));
        }
    };
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        verify_parent_result_identity(job, &existing, &merged)?;
        let parent_rollup_path =
            deadreckon_core::campaign::rollup_path_at_run_root(&existing.run_root);
        if parent_rollup_path.is_file() {
            let parent_rollup: deadreckon_core::campaign::CampaignRollup =
                serde_json::from_slice(&fs::read(&parent_rollup_path)?).map_err(|source| {
                    CliError::Core(DeadreckonError::Json {
                        path: parent_rollup_path.clone(),
                        source,
                    })
                })?;
            if parent_rollup != rollup {
                return Ok(ParentCompletion::GateFailed(
                    "campaign parent receipt is bound to a different rollup".to_string(),
                ));
            }
        } else {
            deadreckon_core::campaign::write_campaign_rollup_at_run_root(
                &existing.run_root,
                &rollup,
            )?;
        }
        if let Ok(receipt) = deadreckon_core::validate_completion_receipt(paths, &existing) {
            let receipt = validate_and_promote_parent(paths, &mut existing, &receipt)?;
            return Ok(ParentCompletion::Verified(Box::new(receipt)));
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("NEEDS_REVIEW: "))
        {
            let decision = persisted_semantic_judgment(&existing)
                .ok()
                .flatten()
                .map(|judgment| judgment.decision);
            let stop_reason = semantic_decision_stop_reason(decision).unwrap_or_else(|| {
                if reason.contains("not contained") {
                    StopReason::LostContainment
                } else {
                    StopReason::SemanticUnavailable
                }
            });
            return Ok(ParentCompletion::NeedsReview {
                reason: reason.to_string(),
                decision,
                stop_reason,
            });
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("deterministic campaign gate failed: "))
        {
            return Ok(ParentCompletion::GateFailed(reason.to_string()));
        }
    }

    let mut parent = prepare_parent_result_run(paths, job, authority, &merged)?;
    let parent_rollup = deadreckon_core::campaign::rollup_path_at_run_root(&parent.run_root);
    if parent_rollup.is_file() {
        let persisted: deadreckon_core::campaign::CampaignRollup =
            serde_json::from_slice(&fs::read(&parent_rollup)?).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: parent_rollup.clone(),
                    source,
                })
            })?;
        if persisted != rollup {
            return Ok(ParentCompletion::GateFailed(
                "campaign parent rollup changed across completion attempts".to_string(),
            ));
        }
    } else {
        deadreckon_core::campaign::write_campaign_rollup_at_run_root(&parent.run_root, &rollup)?;
    }
    let backend = authority
        .sandbox_requested
        .parse::<deadreckon_sandbox::SandboxBackend>()
        .map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "approved sandbox backend is invalid: {error}"
            )))
        })?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let marker = if marker_path.exists() {
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic campaign proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    } else {
        if let Err(error) = deadreckon_runtime::run_deterministic_completion_gate(&parent, backend)
        {
            parent.failure_reason = Some(format!("deterministic campaign gate failed: {error}"));
            parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
            deadreckon_core::save_state(&parent)?;
            return Ok(ParentCompletion::GateFailed(error.to_string()));
        }
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic campaign proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    };
    if !marker.is_native_gate_proof() {
        return Ok(ParentCompletion::GateFailed(
            "merged campaign did not produce a native dr-gate proof".to_string(),
        ));
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return parent_needs_review(
            &mut parent,
            "deterministic checks passed, but the campaign result was not contained",
            None,
            StopReason::LostContainment,
        );
    }
    let persisted_achieved = persisted_semantic_judgment(&parent)?
        .filter(|judgment| judgment.decision == SemanticDecision::Achieved);
    let execution_usage = match campaign_execution_usage(paths, campaign) {
        Ok(usage) => usage,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("campaign budget accounting is incomplete or corrupt: {error}"),
                None,
                StopReason::CorruptHistory,
            );
        }
    };
    let current_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(judgment) = persisted_achieved {
        if let Some((stop_reason, reason)) = semantic_budget_overrun(job, current_usage) {
            return parent_budget_exhausted(&mut parent, stop_reason, &reason);
        }
        return seal_achieved_parent(paths, &mut parent, authority, &marker, &judgment);
    }
    if let Some((stop_reason, reason)) = semantic_budget_exhaustion(job, current_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }

    let router = match campaign_semantic_router(paths, &campaign.providers) {
        Ok(router) => router,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    let semantic = match deadreckon_runtime::run_semantic_judge_against_source_with_budget(
        &parent,
        &marker,
        &router,
        backend,
        &job.source_cwd,
        remaining_semantic_budget(job, current_usage),
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    record_parent_semantic_accounting(&mut parent, job, &semantic)?;
    let final_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(dimension) = semantic.budget_exhaustion {
        let (stop_reason, reason) = semantic_judge_budget_exhaustion(job, final_usage, dimension);
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some((stop_reason, reason)) = semantic_budget_overrun(job, final_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some(judgment) = semantic.result.judgment()
        && let Err(error) =
            deadreckon_runtime::persist_semantic_judgment(&parent.run_root, judgment)
    {
        return parent_needs_review(
            &mut parent,
            &format!("strict semantic judgment could not be persisted after accounting: {error}"),
            None,
            StopReason::SemanticUnavailable,
        );
    }
    match semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(judgment) => {
            seal_achieved_parent(paths, &mut parent, authority, &marker, &judgment)
        }
        deadreckon_runtime::SemanticJudgeResult::Revise(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge requested revision: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Revise),
            StopReason::SemanticRevise,
        ),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge was uncertain: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Uncertain),
            StopReason::SemanticUncertain,
        ),
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            parent_needs_review(&mut parent, &reason, None, StopReason::SemanticUnavailable)
        }
    }
}

fn validate_campaign_rollup(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    merged: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::campaign::CampaignRollup> {
    use deadreckon_core::campaign::{self, RollupVerdict};
    use deadreckon_core::tamper::AcceptanceTamperVerdict;

    deadreckon_core::validate_acceptance_marker(merged)?;
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let persisted = campaign::read_campaign_rollup(&campaign_dir)?;
    let merged_path = campaign::rollup_path_at_run_root(&merged.run_root);
    let merged_rollup: campaign::CampaignRollup = serde_json::from_slice(&fs::read(&merged_path)?)
        .map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: merged_path.clone(),
                source,
            })
        })?;
    if persisted != merged_rollup || persisted.campaign_id != campaign.campaign_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign rollup does not match the actual merged result".to_string(),
        )));
    }
    let fresh = campaign::build_rollup(campaign, |run_id| {
        let Ok(state) = deadreckon_core::load_run(paths, run_id) else {
            return (
                "missing".to_string(),
                AcceptanceTamperVerdict::Refuse,
                Vec::new(),
            );
        };
        let tamper = deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)
            .ok()
            .flatten();
        let verdict = tamper
            .as_ref()
            .map(|tamper| tamper.verdict)
            .unwrap_or(AcceptanceTamperVerdict::Clean);
        let caveats = tamper.map(|tamper| tamper.caveats).unwrap_or_default();
        let gate = if deadreckon_core::validate_acceptance_marker(&state).is_ok() {
            "signed".to_string()
        } else {
            "refused".to_string()
        };
        (gate, verdict, caveats)
    });
    if persisted.leaves != fresh.leaves
        || persisted.rollup_verdict != fresh.rollup_verdict
        || persisted.refused_subs != fresh.refused_subs
        || persisted.caveat_subs != fresh.caveat_subs
        || persisted.rollup_verdict == RollupVerdict::Refused
        || !campaign::campaign_can_complete(campaign, &fresh)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign worst-of rollup refuses parent completion".to_string(),
        )));
    }
    Ok(persisted)
}

fn campaign_semantic_router(
    paths: &DeadreckonPaths,
    providers: &deadreckon_core::plan::PlanProviders,
) -> Result<ProviderRouter> {
    let selection = [
        (
            providers.reviewer.as_deref(),
            providers.reviewer_model.as_deref(),
        ),
        (providers.coder.as_deref(), providers.coder_model.as_deref()),
        (
            providers.planner.as_deref(),
            providers.planner_model.as_deref(),
        ),
        (
            providers.default_child.as_deref(),
            providers.default_child_model.as_deref(),
        ),
    ]
    .into_iter()
    .find(|(provider, _)| provider.is_some())
    .unwrap_or((None, None));
    if selection
        .0
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        Ok(ProviderRouter::smoke())
    } else {
        Ok(ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            selection.0,
            selection.1,
        )?)
    }
}

pub(crate) fn prepare_parent_result_run(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    merged: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        verify_parent_result_identity(job, &existing, merged)?;
        let contract = super::job::job_acceptance_path(paths, job.job_id.as_ref());
        let parent_contract =
            deadreckon_core::acceptance_spec_path_for_run_root(&existing.run_root);
        if parent_contract.is_file() {
            let digest = deadreckon_core::flight::sha256_file(&parent_contract)?;
            if digest != authority.contract_sha256 {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "parent result contract changed after graph job {} was approved",
                    job.job_id
                ))));
            }
        } else {
            fs::copy(contract, parent_contract)?;
        }
        existing.turn = existing.turn.max(1);
        if existing.status != deadreckon_core::RunStatus::Completed
            && !existing
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("NEEDS_REVIEW:"))
        {
            existing.set_phase_status(PhaseId(50), PhaseStatus::Executing)?;
            deadreckon_core::save_state(&existing)?;
        }
        return Ok(existing);
    }

    let mut state = deadreckon_core::create_run(
        paths,
        deadreckon_core::RunOptions {
            goal: job.goal.clone(),
            cwd: job.source_cwd.clone(),
            sandbox: authority.sandbox_requested.clone(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            run_id: Some(job.job_id.as_ref().to_string()),
            codebase: None,
        },
    )?;
    if state.scope != job.scope || state.run_id != job.job_id.as_ref() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent result run identity does not match graph job {}",
            job.job_id
        ))));
    }
    if state.working_dir.exists() {
        fs::remove_dir_all(&state.working_dir)?;
    }
    deadreckon_core::copy_tree(&merged.working_dir, &state.working_dir)?;
    // A promoted child run's manifest is lifecycle metadata for that child.
    // The parent receipt binds only the merged result tree; parent promotion
    // writes its own manifest later.
    match fs::remove_file(state.working_dir.join("manifest.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    deadreckon_core::write_codebase_record(
        &state.working_dir,
        &deadreckon_core::CodebaseRecord::fresh(),
    )?;
    fs::copy(
        super::job::job_acceptance_path(paths, job.job_id.as_ref()),
        deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
    )?;
    state.turn = 1;
    state.set_phase_status(PhaseId(50), PhaseStatus::Executing)?;
    deadreckon_core::save_state(&state)?;
    Ok(state)
}

fn verify_parent_result_identity(
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
    merged: &deadreckon_core::PipelineState,
) -> Result<()> {
    if parent.run_id != job.job_id.as_ref()
        || parent.scope != job.scope
        || parent.goal != job.goal
        || parent.cwd != job.source_cwd
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "existing parent result run does not retain graph job {} identity",
            job.job_id
        ))));
    }
    let mut parent_index = deadreckon_core::flight::build_working_file_index(&parent.working_dir)?;
    parent_index.files.remove(Path::new("manifest.json"));
    let parent_hash = parent_index.tree_hash();
    let mut merged_index = deadreckon_core::flight::build_working_file_index(&merged.working_dir)?;
    merged_index.files.remove(Path::new("manifest.json"));
    let merged_hash = merged_index.tree_hash();
    if parent_hash != merged_hash {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "existing parent result for graph job {} does not match merged result {}",
            job.job_id, merged.run_id
        ))));
    }
    Ok(())
}

fn persisted_semantic_judgment(
    state: &deadreckon_core::PipelineState,
) -> Result<Option<deadreckon_protocol::SemanticJudgment>> {
    let path = state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let judgment: deadreckon_protocol::SemanticJudgment =
        serde_json::from_slice(&raw).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?;
    if judgment.job_id != deadreckon_protocol::JobId(state.run_id.clone())
        || judgment.run_id != deadreckon_protocol::RunId(state.run_id.clone())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "persisted semantic judgment identity does not match parent result {}",
            state.run_id
        ))));
    }
    Ok(Some(judgment))
}

fn plan_execution_usage(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ParentExecutionUsage> {
    let mut seen_runs = std::collections::BTreeSet::new();
    let mut seen_plans = std::collections::BTreeSet::new();
    let mut usage = ParentExecutionUsage::default();
    add_plan_execution_usage(paths, plan, &mut seen_runs, &mut seen_plans, &mut usage)?;
    Ok(usage)
}

fn add_plan_execution_usage(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    seen_runs: &mut std::collections::BTreeSet<String>,
    seen_plans: &mut std::collections::BTreeSet<String>,
    usage: &mut ParentExecutionUsage,
) -> Result<()> {
    if !seen_plans.insert(plan.plan_id.clone()) {
        return Ok(());
    }
    let planner = load_plan_planner_accounting(paths, &plan.plan_id)?;
    add_usage(
        "plan root planner",
        ParentExecutionUsage {
            spend_usd: planner.cost_usd,
            wall_seconds: planner.wall_seconds,
        },
        usage,
    )?;
    for task in &plan.tasks {
        if let Some(subplan_id) = task.subplan.as_deref() {
            let subplan = deadreckon_core::load_plan(paths, subplan_id).map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot verify parent budget because nested sub-plan {subplan_id} is unreadable: {error}"
                )))
            })?;
            add_plan_execution_usage(paths, &subplan, seen_runs, seen_plans, usage)?;
        }
        for run_id in task
            .attempts
            .iter()
            .filter_map(|attempt| attempt.run_id.as_deref())
            .chain(task.child_run_id.as_deref())
        {
            if !seen_runs.insert(run_id.to_string()) {
                continue;
            }
            let state = deadreckon_core::load_run(paths, run_id).map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot verify parent budget because task run {run_id} is unreadable: {error}"
                )))
            })?;
            add_usage(
                &format!("task run {run_id}"),
                ParentExecutionUsage {
                    spend_usd: state.total_spend_usd,
                    wall_seconds: state.total_wall_seconds,
                },
                usage,
            )?;
        }
        if task.attempts.iter().any(|attempt| attempt.run_id.is_none()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify parent budget because graph task {} has an attempt without a run ID",
                task.task_id
            ))));
        }
        if task.child_run_id.is_none() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify parent budget because graph task {} has no result run",
                task.task_id
            ))));
        }
    }
    for event in deadreckon_core::read_plan_events(paths, &plan.plan_id)? {
        let run_id = match event.event {
            deadreckon_core::PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => Some(run_id),
            deadreckon_core::PlanEventKind::MergeRepaired {
                repair_run_id: Some(run_id),
                ..
            } => Some(run_id),
            _ => None,
        };
        let Some(run_id) = run_id else {
            continue;
        };
        if !seen_runs.insert(run_id.clone()) {
            continue;
        }
        let state = deadreckon_core::load_run(paths, &run_id).map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify parent budget because merge-repair run {run_id} is unreadable: {error}"
            )))
        })?;
        add_usage(
            &format!("merge-repair run {run_id}"),
            ParentExecutionUsage {
                spend_usd: state.total_spend_usd,
                wall_seconds: state.total_wall_seconds,
            },
            usage,
        )?;
    }
    Ok(())
}

fn campaign_execution_usage(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<ParentExecutionUsage> {
    let mut usage = ParentExecutionUsage::default();
    if let Some(accounting) =
        deadreckon_core::campaign::read_campaign_events(&paths.plan_dir(&campaign.campaign_id))?
            .into_iter()
            .rev()
            .find(|event| event.kind == "root_planner_accounting")
    {
        usage.spend_usd += accounting
            .detail
            .get("cost_usd")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "campaign root planner accounting has no valid cost_usd".to_string(),
                ))
            })?;
        usage.wall_seconds += accounting
            .detail
            .get("wall_seconds")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "campaign root planner accounting has no valid wall_seconds".to_string(),
                ))
            })?;
    }
    let mut seen_runs = std::collections::BTreeSet::new();
    let mut seen_plans = std::collections::BTreeSet::new();
    for sub in &campaign.sub_goals {
        let plan_id = sub.sub_plan_id.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify campaign budget because sub-goal {} has no plan ID",
                sub.sub_id
            )))
        })?;
        let plan = deadreckon_core::load_plan(paths, plan_id).map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify campaign budget because sub-plan {plan_id} is unreadable: {error}"
            )))
        })?;
        add_plan_execution_usage(paths, &plan, &mut seen_runs, &mut seen_plans, &mut usage)?;
    }
    validate_parent_usage(usage)?;
    Ok(usage)
}

fn load_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
) -> Result<PlanPlannerAccounting> {
    let path = paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE);
    let raw = fs::read(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?;
    let accounting: PlanPlannerAccounting = serde_json::from_slice(&raw).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.clone(),
            source,
        })
    })?;
    if accounting.schema_version != 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "plan {plan_id} has unsupported root planner accounting schema {}",
            accounting.schema_version
        ))));
    }
    Ok(accounting)
}

fn add_usage(
    source: &str,
    increment: ParentExecutionUsage,
    usage: &mut ParentExecutionUsage,
) -> Result<()> {
    validate_parent_usage(increment).map_err(|error| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "cannot verify parent budget because {source} has invalid accounting: {error}"
        )))
    })?;
    usage.spend_usd += increment.spend_usd;
    usage.wall_seconds += increment.wall_seconds;
    validate_parent_usage(*usage)
}

fn validate_parent_usage(usage: ParentExecutionUsage) -> Result<()> {
    if !usage.spend_usd.is_finite()
        || usage.spend_usd < 0.0
        || !usage.wall_seconds.is_finite()
        || usage.wall_seconds < 0.0
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent execution accounting must be finite and non-negative".to_string(),
        )));
    }
    Ok(())
}

fn combined_parent_usage(
    execution: ParentExecutionUsage,
    parent: &deadreckon_core::PipelineState,
) -> Result<ParentExecutionUsage> {
    let combined = ParentExecutionUsage {
        spend_usd: execution.spend_usd + parent.total_spend_usd,
        wall_seconds: execution.wall_seconds + parent.total_wall_seconds,
    };
    validate_parent_usage(combined)?;
    Ok(combined)
}

fn remaining_semantic_budget(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> deadreckon_runtime::SemanticJudgeBudget {
    deadreckon_runtime::SemanticJudgeBudget {
        remaining_spend_usd: Some(job.policy.max_spend_usd - usage.spend_usd),
        remaining_wall_seconds: Some(job.policy.max_wall_seconds as f64 - usage.wall_seconds),
    }
}

fn semantic_budget_exhaustion(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> Option<(StopReason, String)> {
    if usage.spend_usd >= job.policy.max_spend_usd {
        return Some((
            StopReason::SpendCap,
            format!(
                "approved spend cap was exhausted before semantic judging (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ));
    }
    if usage.wall_seconds >= job.policy.max_wall_seconds as f64 {
        return Some((
            StopReason::WallCap,
            format!(
                "approved wall-time cap was exhausted before semantic judging ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ));
    }
    None
}

fn semantic_budget_overrun(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> Option<(StopReason, String)> {
    if usage.spend_usd > job.policy.max_spend_usd {
        return Some((
            StopReason::SpendCap,
            format!(
                "semantic judging exceeded the approved spend cap (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ));
    }
    if usage.wall_seconds > job.policy.max_wall_seconds as f64 {
        return Some((
            StopReason::WallCap,
            format!(
                "semantic judging exceeded the approved wall-time cap ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ));
    }
    None
}

fn semantic_judge_budget_exhaustion(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
    dimension: deadreckon_runtime::SemanticBudgetExhaustion,
) -> (StopReason, String) {
    match dimension {
        deadreckon_runtime::SemanticBudgetExhaustion::Spend => (
            StopReason::SpendCap,
            format!(
                "semantic judging exhausted the approved spend cap (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ),
        deadreckon_runtime::SemanticBudgetExhaustion::Wall => (
            StopReason::WallCap,
            format!(
                "semantic judging exhausted the approved wall-time cap ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ),
    }
}

fn parent_budget_exhausted(
    parent: &mut deadreckon_core::PipelineState,
    stop_reason: StopReason,
    reason: &str,
) -> Result<ParentCompletion> {
    parent.pause_reason = Some(reason.to_string());
    parent.failure_reason = Some(reason.to_string());
    parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    deadreckon_core::save_state(parent)?;
    Ok(ParentCompletion::BudgetExhausted {
        reason: reason.to_string(),
        stop_reason,
    })
}

fn seal_achieved_parent(
    paths: &DeadreckonPaths,
    parent: &mut deadreckon_core::PipelineState,
    authority: &deadreckon_protocol::JobAuthority,
    marker: &deadreckon_core::AcceptanceMarker,
    judgment: &deadreckon_protocol::SemanticJudgment,
) -> Result<ParentCompletion> {
    let receipt = match deadreckon_core::seal_completion_receipt(
        paths, parent, authority, marker, judgment,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            return parent_needs_review(
                parent,
                &format!(
                    "semantic judgment achieved, but the parent receipt could not be sealed: {error}"
                ),
                Some(SemanticDecision::Achieved),
                StopReason::SemanticUnavailable,
            );
        }
    };
    let receipt = validate_and_promote_parent(paths, parent, &receipt)?;
    Ok(ParentCompletion::Verified(Box::new(receipt)))
}

fn validate_and_promote_parent(
    paths: &DeadreckonPaths,
    parent: &mut deadreckon_core::PipelineState,
    expected: &CompletionReceipt,
) -> Result<CompletionReceipt> {
    // Receipt validation is deliberately before promotion: no unverified
    // parent tree may enter the library that `finish` delivers.
    let validated = deadreckon_core::validate_completion_receipt(paths, parent)?;
    if &validated != expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "sealed graph receipt did not round-trip through the finish validator".to_string(),
        )));
    }
    parent.failure_reason = None;
    if parent.status != deadreckon_core::RunStatus::Completed {
        parent.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
        deadreckon_core::save_state(parent)?;
    }
    deadreckon_core::promote_completed_run(paths, parent)?;
    let promoted = deadreckon_core::validate_completion_receipt(paths, parent)?;
    if promoted != validated {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "graph receipt changed while promoting the parent result".to_string(),
        )));
    }
    Ok(promoted)
}

fn semantic_router(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ProviderRouter> {
    let selection = [
        (
            plan.providers.reviewer.as_deref(),
            plan.providers.reviewer_model.as_deref(),
        ),
        (
            plan.providers.coder.as_deref(),
            plan.providers.coder_model.as_deref(),
        ),
        (
            plan.providers.planner.as_deref(),
            plan.providers.planner_model.as_deref(),
        ),
        (
            plan.providers.default_child.as_deref(),
            plan.providers.default_child_model.as_deref(),
        ),
    ]
    .into_iter()
    .find(|(provider, _)| provider.is_some())
    .unwrap_or((None, None));
    if selection
        .0
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        Ok(ProviderRouter::smoke())
    } else {
        Ok(ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            selection.0,
            selection.1,
        )?)
    }
}

fn record_parent_semantic_accounting(
    state: &mut deadreckon_core::PipelineState,
    job: &deadreckon_protocol::Job,
    semantic: &deadreckon_runtime::semantic_judge::SemanticJudgeRun,
) -> Result<()> {
    let accounting = &semantic.accounting;
    state.total_spend_usd += accounting.cost_usd;
    state.total_wall_seconds += accounting.wall_time_seconds;
    deadreckon_core::append_spend(
        state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: state.turn,
            provider: accounting.provider.clone(),
            model: accounting.model.clone(),
            input_tokens: accounting.input_tokens,
            output_tokens: accounting.output_tokens,
            cost_usd: accounting.cost_usd,
            total_cost_usd: state.total_spend_usd,
            cap_usd: Some(job.policy.max_spend_usd),
            subscription: accounting.subscription,
            estimated: false,
            wall_time_seconds: Some(accounting.wall_time_seconds),
            wall_time_cap_seconds: Some(job.policy.max_wall_seconds as f64),
            kind: "semantic_judge".to_string(),
        },
    )?;
    let (event, reason) = match &semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(_) => ("semantic_judge.achieved", None),
        deadreckon_runtime::SemanticJudgeResult::Revise(_) => ("semantic_judge.revise", None),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(_) => {
            ("semantic_judge.uncertain", None)
        }
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            ("semantic_judge.unavailable", Some(reason.as_str()))
        }
    };
    deadreckon_core::append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: state.turn,
            event: event.to_string(),
            latency_ms: Some((accounting.wall_time_seconds.max(0.0) * 1_000.0).round() as u128),
            detail: json!({
                "provider": accounting.provider,
                "model": accounting.model,
                "input_tokens": accounting.input_tokens,
                "output_tokens": accounting.output_tokens,
                "spend_usd": accounting.cost_usd,
                "wall_time_seconds": accounting.wall_time_seconds,
                "sandbox_backend": accounting.sandbox_backend,
                "workspace_access": "read-only",
                "worker_session": false,
                "decision": semantic.result.judgment().map(|judgment| judgment.decision),
                "reason": reason,
                "graph_parent": true,
            }),
        },
    )?;
    deadreckon_core::save_state(state)?;
    Ok(())
}

fn parent_needs_review(
    state: &mut deadreckon_core::PipelineState,
    reason: &str,
    decision: Option<SemanticDecision>,
    stop_reason: StopReason,
) -> Result<ParentCompletion> {
    state.failure_reason = Some(format!("NEEDS_REVIEW: {reason}"));
    state.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    deadreckon_core::save_state(state)?;
    Ok(ParentCompletion::NeedsReview {
        reason: reason.to_string(),
        decision,
        stop_reason,
    })
}

fn driver_shape_error(job_id: &str) -> CliError {
    CliError::Core(DeadreckonError::InvalidInput(format!(
        "advanced driver kind does not match job {job_id}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_revise_has_its_own_typed_stop_reason() {
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Revise)),
            Some(StopReason::SemanticRevise)
        );
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Uncertain)),
            Some(StopReason::SemanticUncertain)
        );
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Achieved)),
            None
        );
    }

    #[test]
    fn parent_semantic_budget_boundary_returns_typed_cap_reasons() {
        let temp = tempfile::TempDir::new().expect("temp");
        let source_cwd = temp.path().join("source");
        std::fs::create_dir_all(&source_cwd).expect("source");
        let job = deadreckon_protocol::Job {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: JobId("budget-parent".to_string()),
            scope: "scope".to_string(),
            goal: "bounded parent".to_string(),
            shape: JobShape::Graph,
            created_at: Utc::now(),
            source_cwd,
            launch_plan_sha256: "sha256:launch".to_string(),
            authority_sha256: "sha256:authority".to_string(),
            policy: deadreckon_protocol::JobPolicy {
                max_spend_usd: 2.0,
                max_wall_seconds: 30,
                max_attempts: 1,
                deadline: None,
                semantic_judge: deadreckon_protocol::SemanticJudgeMode::Required,
            },
        };

        let spend = semantic_budget_exhaustion(
            &job,
            ParentExecutionUsage {
                spend_usd: 2.0,
                wall_seconds: 1.0,
            },
        )
        .expect("spend cap");
        assert_eq!(spend.0, StopReason::SpendCap);

        let wall = semantic_budget_exhaustion(
            &job,
            ParentExecutionUsage {
                spend_usd: 1.0,
                wall_seconds: 30.0,
            },
        )
        .expect("wall cap");
        assert_eq!(wall.0, StopReason::WallCap);

        assert!(
            semantic_budget_exhaustion(
                &job,
                ParentExecutionUsage {
                    spend_usd: 1.99,
                    wall_seconds: 29.99,
                },
            )
            .is_none()
        );
        assert!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 2.0,
                    wall_seconds: 30.0,
                },
            )
            .is_none(),
            "exactly using a cap remains within the approved policy"
        );
        assert_eq!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 2.01,
                    wall_seconds: 1.0,
                },
            )
            .expect("spend overrun")
            .0,
            StopReason::SpendCap
        );
        assert_eq!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 1.0,
                    wall_seconds: 30.01,
                },
            )
            .expect("wall overrun")
            .0,
            StopReason::WallCap
        );
    }

    #[test]
    fn graph_usage_loads_run_totals_and_deduplicates_attempt_and_result_ids() {
        use deadreckon_core::plan::{
            Plan, PlanMode, PlanProviders, PlanRole, PlanTask, TaskAttempt,
        };

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("source");
        std::fs::create_dir_all(&cwd).expect("source");
        let mut run = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "child".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "test".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        run.total_spend_usd = 1.25;
        run.total_wall_seconds = 4.5;
        deadreckon_core::save_state(&run).expect("state");

        let mut task = PlanTask::new(0, "child", "do work", PlanRole::Child, None);
        task.child_run_id = Some(run.run_id.clone());
        task.attempts.push(TaskAttempt::failed(
            1,
            Some(run.run_id.clone()),
            Some("retry reused the same durable run".to_string()),
            1.25,
        ));
        let mut second_task =
            PlanTask::new(1, "same child", "reuse durable work", PlanRole::Child, None);
        second_task.child_run_id = Some(run.run_id.clone());
        let plan = Plan::new(
            "bounded graph",
            PlanMode::FullPlan,
            vec![task, second_task],
            PlanProviders::default(),
            None,
            "test",
        )
        .expect("plan");
        let missing = plan_execution_usage(&paths, &plan)
            .expect_err("missing planner accounting must fail closed");
        assert!(
            missing.to_string().contains(PLAN_PLANNER_ACCOUNTING_FILE),
            "{missing}"
        );
        record_plan_planner_accounting(
            &paths,
            &plan.plan_id,
            Some(&commands::plan::PlannerAccounting {
                spend: deadreckon_providers::SpendEstimate {
                    provider: "planner".to_string(),
                    model: "planner-model".to_string(),
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: 0.25,
                    subscription: false,
                    wall_time_seconds: Some(1.0),
                },
                wall_seconds: 1.0,
            }),
        )
        .expect("planner accounting");

        let usage = plan_execution_usage(&paths, &plan).expect("usage");
        assert_eq!(
            usage,
            ParentExecutionUsage {
                spend_usd: 1.5,
                wall_seconds: 5.5,
            }
        );
    }

    #[test]
    fn campaign_usage_reads_subplan_runs_instead_of_zero_cost_merge_runs() {
        use deadreckon_core::campaign::{
            Campaign, SubGoalStatus, append_campaign_event, build_sub_goals,
        };
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("source");
        std::fs::create_dir_all(&cwd).expect("source");
        let mut plan_ids = Vec::new();
        for (index, (spend_usd, wall_seconds)) in [(0.6, 2.0), (0.7, 3.0)].into_iter().enumerate() {
            let mut run = deadreckon_core::create_run(
                &paths,
                deadreckon_core::RunOptions {
                    goal: format!("child {index}"),
                    cwd: cwd.clone(),
                    sandbox: "none".to_string(),
                    provider: Some("smoke".to_string()),
                    skill_name: "test".to_string(),
                    max_spend_usd: None,
                    max_wall_seconds: None,
                    run_id: None,
                    codebase: None,
                },
            )
            .expect("run");
            run.total_spend_usd = spend_usd;
            run.total_wall_seconds = wall_seconds;
            deadreckon_core::save_state(&run).expect("state");

            let mut first = PlanTask::new(0, "first", "work", PlanRole::Child, None);
            first.child_run_id = Some(run.run_id.clone());
            let mut second = PlanTask::new(1, "second", "reuse", PlanRole::Child, None);
            second.child_run_id = Some(run.run_id.clone());
            let mut plan = Plan::new(
                format!("subplan {index}"),
                PlanMode::FullPlan,
                vec![first, second],
                PlanProviders::default(),
                None,
                "test",
            )
            .expect("plan");
            plan.plan_id = format!("subplan-{index}");
            save_plan(&paths, &plan).expect("plan state");
            record_plan_planner_accounting(&paths, &plan.plan_id, None)
                .expect("planner accounting");
            plan_ids.push(plan.plan_id);
        }

        let mut campaign = Campaign::new(
            "bounded campaign",
            build_sub_goals(vec!["one".to_string(), "two".to_string()], 2).expect("sub-goals"),
            PlanProviders::default(),
            0,
            Some(2.0),
            Some(30.0),
            "test",
        )
        .expect("campaign");
        for (sub, plan_id) in campaign.sub_goals.iter_mut().zip(plan_ids) {
            sub.sub_plan_id = Some(plan_id);
            sub.status = SubGoalStatus::Merged;
        }
        append_campaign_event(
            &paths.plan_dir(&campaign.campaign_id),
            "root_planner_accounting",
            json!({
                "cost_usd": 0.2,
                "wall_seconds": 1.0,
            }),
        )
        .expect("planner accounting");

        let usage = campaign_execution_usage(&paths, &campaign).expect("usage");
        assert!((usage.spend_usd - 1.5).abs() < f64::EPSILON);
        assert!((usage.wall_seconds - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn driver_spec_round_trips_through_the_immutable_launch_plan() {
        let mut plan = commands::course::trivial_operator_plan(
            "durable graph",
            commands::course::CourseShape::Plan,
            "test",
        );
        let spec = DriverSpec {
            kind: DriverKind::FullPlan,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: vec!["1=reviewer".to_string()],
            coder_provider: None,
            reviewer_provider: None,
            model: Some("model".to_string()),
            source_init_git: false,
        };
        embed_driver_spec(&mut plan, &spec).expect("embed");
        assert_eq!(driver_spec(&plan).expect("read"), spec);
    }

    #[test]
    fn durable_graph_driver_never_persists_per_node_apply() {
        let mut plan = commands::course::trivial_operator_plan(
            "verify only after the isolated graph is merged",
            commands::course::CourseShape::Plan,
            "test",
        );
        let requested = DriverSpec {
            kind: DriverKind::FullPlan,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            model: None,
            source_init_git: false,
        };

        embed_driver_spec(&mut plan, &requested).expect("embed");
        let frozen = driver_spec(&plan).expect("frozen driver");
        assert_eq!(requested.apply, deadreckon_core::plan::ApplyWhen::PerNode);
        assert_eq!(frozen.apply, deadreckon_core::plan::ApplyWhen::AtEnd);
    }

    #[test]
    fn durable_campaign_driver_also_freezes_apply_at_end() {
        let mut plan = commands::course::trivial_operator_plan(
            "verify only after the isolated campaign is merged",
            commands::course::CourseShape::Campaign,
            "test",
        );
        let requested = DriverSpec {
            kind: DriverKind::Campaign,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            model: None,
            source_init_git: false,
        };

        embed_driver_spec(&mut plan, &requested).expect("embed");
        assert_eq!(
            driver_spec(&plan).expect("frozen driver").apply,
            deadreckon_core::plan::ApplyWhen::AtEnd
        );
    }
}
