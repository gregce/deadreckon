//! Trusted drivers that place existing orchestration engines under one Job.
//!
//! The driver specification is embedded in the signed launch plan. The
//! mutable sidecar records which existing Plan or Campaign artifact belongs to
//! the parent Job; it is navigation evidence, never completion authority.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use chrono::Utc;
use deadreckon_protocol::{
    CompletionReceipt, JobId, JobShape, SemanticDecision, SpendRecord, StopReason, TraceRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::*;

const DRIVER_SIGNAL: &str = "watchkeeper_driver";
const DRIVER_STATE_FILE: &str = "driver.json";
const PLAN_PLANNER_ACCOUNTING_FILE: &str = "root-planner-accounting.json";
const DELEGATION_JOB_ENV: &str = "DEADRECKON_DELEGATION_JOB";
const DELEGATION_ID_ENV: &str = "DEADRECKON_DELEGATION_ID";
const MAX_DELEGATION_TOKEN_BYTES: u64 = 512;
const MAX_DELEGATION_RECORD_BYTES: u64 = 128 * 1024;

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
    authority: commands::supervisor::GuardedDriverAuthority,
    root_artifact: bool,
}

static DRIVER_CONTEXT: OnceLock<DriverContext> = OnceLock::new();
static DELEGATED_PLAN_CHILD: OnceLock<DelegatedAction> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DelegatedAction {
    PlanChild {
        plan_id: String,
        task_id: String,
        task_index: u32,
        task_attempt: u32,
    },
    PlanFork {
        plan_id: String,
    },
    PlanMerge {
        plan_id: String,
    },
    CampaignSub {
        campaign_id: String,
        sub_id: String,
        plan_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DelegatedInvocation {
    schema_version: u32,
    capability_id: String,
    job_id: String,
    authority: commands::supervisor::GuardedDriverAuthority,
    action: DelegatedAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    immutable_plan_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    immutable_campaign_sha256: Option<String>,
    argv_sha256: String,
    cwd: PathBuf,
    scope_root: PathBuf,
    token_sha256: String,
    issued_at: chrono::DateTime<Utc>,
}

pub(crate) struct PreparedDelegation {
    capability_id: String,
    job_id: String,
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OwnedPlanRecord {
    schema_version: u32,
    job_id: String,
    root_plan_id: String,
    plan_id: String,
    parent_plan_id: Option<String>,
    immutable_definition_sha256: String,
    recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OwnedCampaignRecord {
    schema_version: u32,
    job_id: String,
    immutable_definition_sha256: String,
    recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPlanOwner {
    pub(crate) job: deadreckon_protocol::Job,
    pub(crate) root_plan_id: String,
    pub(crate) lineage: Vec<String>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingDriverRecovery {
    Unchanged,
    Recovered,
    BudgetExhausted {
        stop_reason: StopReason,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RootPlannerBudgetExhaustion {
    pub(crate) dimension: deadreckon_core::plan::BudgetDimension,
    pub(crate) stop_reason: StopReason,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ParentExecutionUsage {
    spend_usd: f64,
    wall_seconds: f64,
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

#[cfg(test)]
pub(crate) fn record_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
    accounting: Option<&commands::plan::PlannerAccounting>,
) -> Result<()> {
    let record = root_planner_accounting(accounting);
    record_plan_planner_accounting_snapshot(paths, plan_id, &record)
}

pub(crate) fn root_planner_accounting(
    accounting: Option<&commands::plan::PlannerAccounting>,
) -> deadreckon_core::plan::RootPlannerAccounting {
    deadreckon_core::plan::RootPlannerAccounting {
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
    }
}

pub(crate) fn record_plan_planner_accounting_snapshot(
    paths: &DeadreckonPaths,
    plan_id: &str,
    record: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    super::job::write_json_synced(
        &paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE),
        record,
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

pub(crate) fn current_driver_owns_root_artifact() -> bool {
    DRIVER_CONTEXT
        .get()
        .is_some_and(|context| context.root_artifact)
}

pub(crate) fn delegated_plan_child_authorized() -> bool {
    matches!(
        DELEGATED_PLAN_CHILD.get(),
        Some(DelegatedAction::PlanChild { .. })
    )
}

fn install_driver_context(
    paths: &DeadreckonPaths,
    authority: commands::supervisor::GuardedDriverAuthority,
    root_artifact: bool,
) -> Result<()> {
    let job_id = authority.job_id.clone();
    DRIVER_CONTEXT
        .set(DriverContext {
            acceptance_path: commands::job::job_acceptance_path(paths, &job_id),
            job_id,
            authority,
            root_artifact,
        })
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "this process is already driving another durable job".to_string(),
            ))
        })
}

pub(crate) fn resolve_plan_owner(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<Option<ResolvedPlanOwner>> {
    let owner_job_id = plan.owner_job_id.clone().or_else(|| {
        paths
            .job_json(&plan.plan_id)
            .is_file()
            .then(|| plan.plan_id.clone())
    });
    let Some(owner_job_id) = owner_job_id else {
        return Ok(None);
    };
    let job = deadreckon_core::load_job(paths, &owner_job_id)?;
    if job.job_id.as_ref() != owner_job_id
        || !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan {} names incompatible durable Job {} as its owner",
            plan.plan_id, owner_job_id
        ))));
    }

    let mut lineage = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = plan.clone();
    loop {
        if !seen.insert(current.plan_id.clone()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan ownership lineage contains a cycle at {}",
                current.plan_id
            ))));
        }
        lineage.push(current.plan_id.clone());
        if lineage.len() as u32 > deadreckon_core::plan::MAX_SUBPLAN_DEPTH {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan ownership lineage exceeds nesting cap {}",
                deadreckon_core::plan::MAX_SUBPLAN_DEPTH
            ))));
        }
        let compatible_owner = current.owner_job_id.as_deref() == Some(owner_job_id.as_str())
            || (current.owner_job_id.is_none() && current.plan_id == owner_job_id);
        if !compatible_owner {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} does not retain durable Job owner {}",
                current.plan_id, owner_job_id
            ))));
        }
        let Some(parent_id) = current.parent_plan_id.as_deref() else {
            break;
        };
        let parent = deadreckon_core::load_plan(paths, parent_id)?;
        let reverse_links = parent
            .tasks
            .iter()
            .filter(|task| task.subplan.as_deref() == Some(current.plan_id.as_str()))
            .count();
        if reverse_links != 1 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} has no unique parent-task link from {}",
                current.plan_id, parent.plan_id
            ))));
        }
        current = parent;
    }
    let root_plan_id = current.plan_id;
    if job.shape == JobShape::Graph && root_plan_id != owner_job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Graph Job {} does not own root Plan {}",
            owner_job_id, root_plan_id
        ))));
    }
    Ok(Some(ResolvedPlanOwner {
        job,
        root_plan_id,
        lineage,
    }))
}

fn immutable_plan_sha256(plan: &deadreckon_core::plan::Plan) -> Result<String> {
    let tasks = plan
        .tasks
        .iter()
        .map(|task| {
            json!({
                "index": task.index,
                "task_id": task.task_id,
                "subject": task.subject,
                "goal": task.goal,
                "active_form": task.active_form,
                "provider": task.provider,
                "role": task.role,
                "depends_on": task.depends_on,
                "subplan": task.subplan,
                "worker_spec": task.worker_spec,
            })
        })
        .collect::<Vec<_>>();
    let definition = json!({
        "schema_version": plan.schema_version,
        "plan_id": plan.plan_id,
        "owner_job_id": plan.owner_job_id,
        "parent_plan_id": plan.parent_plan_id,
        "root_goal": plan.root_goal,
        "mode": plan.mode,
        "n": plan.n,
        "providers": plan.providers,
        "capability_preview": plan.capability_preview,
        "parent_scope": plan.parent_scope,
        "parent_cwd": plan.parent_cwd,
        "acceptance_path": plan.acceptance_path,
        "apply": plan.apply,
        "branch_policy": plan.branch_policy,
        "apply_strategy": plan.apply_strategy,
        "apply_allowlist": plan.apply_allowlist,
        "on_fail": plan.on_fail,
        "max_attempts": plan.max_attempts,
        "circuit_breaker_threshold": plan.circuit_breaker_threshold,
        "tasks": tasks,
    });
    Ok(deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&definition)?,
    ))
}

fn immutable_campaign_sha256(campaign: &deadreckon_core::campaign::Campaign) -> Result<String> {
    let sub_goals = campaign
        .sub_goals
        .iter()
        .map(|sub| {
            json!({
                "sub_id": sub.sub_id,
                "goal": sub.goal,
                "task_key": sub.task_key,
            })
        })
        .collect::<Vec<_>>();
    let definition = json!({
        "schema_version": campaign.schema_version,
        "campaign_id": campaign.campaign_id,
        "root_goal": campaign.root_goal,
        "n": campaign.n,
        "depth": campaign.depth,
        "providers": campaign.providers,
        "tree_budget_usd": campaign.tree_budget_usd,
        "tree_wall_seconds": campaign.tree_wall_seconds,
        "sub_goals": sub_goals,
    });
    Ok(deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&definition)?,
    ))
}

fn owned_campaign_record_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join("owned-campaign.json")
}

fn write_owned_campaign_record(
    paths: &DeadreckonPaths,
    job_id: &str,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<()> {
    if campaign.campaign_id != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} does not retain durable Job identity {job_id}",
            campaign.campaign_id
        ))));
    }
    let record = OwnedCampaignRecord {
        schema_version: 1,
        job_id: job_id.to_string(),
        immutable_definition_sha256: immutable_campaign_sha256(campaign)?,
        recorded_at: Utc::now(),
    };
    let path = owned_campaign_record_path(paths, job_id);
    if path.is_file() {
        let existing: OwnedCampaignRecord = serde_json::from_slice(&fs::read(&path)?)?;
        if existing.schema_version != 1
            || existing.job_id != record.job_id
            || existing.immutable_definition_sha256 != record.immutable_definition_sha256
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "Campaign no longer matches its protected Job-owned definition".to_string(),
            )));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &record)
}

pub(crate) fn record_owned_campaign(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<()> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(());
    };
    if context.job_id != campaign.campaign_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} does not retain current durable Job identity {}",
            campaign.campaign_id, context.job_id
        ))));
    }
    let job = deadreckon_core::load_job(paths, &context.job_id)?;
    if job.shape != JobShape::LegacyCampaign {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} belongs to incompatible durable Job shape {:?}",
            campaign.campaign_id, job.shape
        ))));
    }
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign definition cannot be frozen without current fenced authority".to_string(),
        )));
    }
    write_owned_campaign_record(paths, &context.job_id, campaign)
}

pub(crate) fn validate_owned_campaign(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    job_id: &str,
) -> Result<()> {
    if campaign.campaign_id != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign changed its durable Job identity".to_string(),
        )));
    }
    let path = owned_campaign_record_path(paths, job_id);
    let record: OwnedCampaignRecord =
        serde_json::from_slice(&fs::read(&path).map_err(|source| {
            CliError::Core(DeadreckonError::Io {
                path: path.clone(),
                source,
            })
        })?)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.immutable_definition_sha256 != immutable_campaign_sha256(campaign)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign no longer matches its protected Job-owned definition".to_string(),
        )));
    }
    Ok(())
}

fn owned_plan_record_path(paths: &DeadreckonPaths, job_id: &str, plan_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("owned-plans")
        .join(format!("{plan_id}.json"))
}

fn record_owned_plan(
    paths: &DeadreckonPaths,
    root_plan_id: &str,
    plan: &deadreckon_core::plan::Plan,
) -> Result<()> {
    let Some(job_id) = plan.owner_job_id.as_deref() else {
        return Ok(());
    };
    let record = OwnedPlanRecord {
        schema_version: 1,
        job_id: job_id.to_string(),
        root_plan_id: root_plan_id.to_string(),
        plan_id: plan.plan_id.clone(),
        parent_plan_id: plan.parent_plan_id.clone(),
        immutable_definition_sha256: immutable_plan_sha256(plan)?,
        recorded_at: Utc::now(),
    };
    let path = owned_plan_record_path(paths, job_id, &plan.plan_id);
    if path.is_file() {
        let existing: OwnedPlanRecord = serde_json::from_slice(&fs::read(&path)?)?;
        if existing.job_id != record.job_id
            || existing.root_plan_id != record.root_plan_id
            || existing.plan_id != record.plan_id
            || existing.parent_plan_id != record.parent_plan_id
            || existing.immutable_definition_sha256 != record.immutable_definition_sha256
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "protected ownership definition changed for Plan {}",
                plan.plan_id
            ))));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &record)
}

pub(crate) fn record_owned_plan_tree(
    paths: &DeadreckonPaths,
    root: &deadreckon_core::plan::Plan,
) -> Result<()> {
    if root.owner_job_id.is_none() {
        return Ok(());
    }
    let root_plan_id = root.plan_id.clone();
    let mut pending = vec![root.clone()];
    let mut seen = BTreeSet::new();
    while let Some(plan) = pending.pop() {
        if !seen.insert(plan.plan_id.clone()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "owned Plan tree contains a cycle at {}",
                plan.plan_id
            ))));
        }
        if plan.owner_job_id != root.owner_job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "subplan {} does not retain root Job owner",
                plan.plan_id
            ))));
        }
        let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} lost its durable owner while freezing its definition",
                plan.plan_id
            )))
        })?;
        if owner.root_plan_id != root_plan_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} resolves to root {}, not approved root {root_plan_id}",
                plan.plan_id, owner.root_plan_id
            ))));
        }
        record_owned_plan(paths, &root_plan_id, &plan)?;
        for task in &plan.tasks {
            let Some(subplan_id) = task.subplan.as_deref() else {
                continue;
            };
            let child = deadreckon_core::load_plan(paths, subplan_id)?;
            if child.parent_plan_id.as_deref() != Some(plan.plan_id.as_str()) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "subplan {subplan_id} does not point back to parent {}",
                    plan.plan_id
                ))));
            }
            pending.push(child);
        }
    }
    Ok(())
}

fn validate_owned_plan_if_present(
    paths: &DeadreckonPaths,
    plan_id: &str,
    job_id: &str,
    root_plan_id: &str,
) -> Result<()> {
    if !paths.plan_json(plan_id).is_file() {
        return Ok(());
    }
    let plan = deadreckon_core::load_plan(paths, plan_id)?;
    let path = owned_plan_record_path(paths, job_id, plan_id);
    let record: OwnedPlanRecord = serde_json::from_slice(&fs::read(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.root_plan_id != root_plan_id
        || record.plan_id != plan_id
        || record.parent_plan_id != plan.parent_plan_id
        || record.immutable_definition_sha256 != immutable_plan_sha256(&plan)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan {plan_id} no longer matches its protected Job-owned definition"
        ))));
    }
    Ok(())
}

fn validate_owned_plan_lineage(paths: &DeadreckonPaths, owner: &ResolvedPlanOwner) -> Result<()> {
    for plan_id in &owner.lineage {
        validate_owned_plan_if_present(
            paths,
            plan_id,
            owner.job.job_id.as_ref(),
            &owner.root_plan_id,
        )?;
    }
    Ok(())
}

fn delegation_pending_path(paths: &DeadreckonPaths, job_id: &str, capability_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("delegations")
        .join("pending")
        .join(format!("{capability_id}.json"))
}

fn delegation_consumed_path(paths: &DeadreckonPaths, job_id: &str, capability_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("delegations")
        .join("consumed")
        .join(format!("{capability_id}.json"))
}

fn sync_delegation_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn invocation_argv_sha256<I, S>(argv: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut hasher = Sha256::new();
    for argument in argv {
        let argument = argument.as_ref();
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            argument.as_bytes().to_vec()
        };
        #[cfg(windows)]
        let bytes = {
            use std::os::windows::ffi::OsStrExt;
            argument
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        #[cfg(not(any(unix, windows)))]
        let bytes = argument.to_string_lossy().as_bytes().to_vec();
        let length = u64::try_from(bytes.len()).map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation argument is too large to hash".to_string(),
            ))
        })?;
        hasher.update(length.to_le_bytes());
        hasher.update(bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn canonical_invocation_path(path: &Path, label: &str) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "delegated {label} path {} is unavailable: {source}",
            path.display()
        )))
    })
}

pub(crate) fn prepare_delegated_invocation<S: AsRef<OsStr>>(
    paths: &DeadreckonPaths,
    action: DelegatedAction,
    argv: &[S],
    cwd: &Path,
    scope_root: &Path,
    plan: Option<&deadreckon_core::plan::Plan>,
) -> Result<PreparedDelegation> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "only an authenticated Job driver can delegate executable work".to_string(),
        ))
    })?;
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "the Job driver cannot delegate work without a current fenced lease".to_string(),
        )));
    }
    let immutable_plan_sha256 = if let Some(plan) = plan {
        let owner = resolve_plan_owner(paths, plan)?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} has no durable Job owner",
                plan.plan_id
            )))
        })?;
        if owner.job.job_id.as_ref() != context.job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} belongs to Job {}, not current Job {}",
                plan.plan_id, owner.job.job_id, context.job_id
            ))));
        }
        validate_owned_plan_lineage(paths, &owner)?;
        Some(immutable_plan_sha256(plan)?)
    } else {
        None
    };
    let immutable_campaign_sha256 = match &action {
        DelegatedAction::CampaignSub { campaign_id, .. } => {
            if campaign_id != &context.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "Campaign delegation changed its durable Job identity".to_string(),
                )));
            }
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(campaign_id))?;
            validate_owned_campaign(paths, &campaign, &context.job_id)?;
            Some(immutable_campaign_sha256(&campaign)?)
        }
        DelegatedAction::PlanChild { .. }
        | DelegatedAction::PlanFork { .. }
        | DelegatedAction::PlanMerge { .. } => None,
    };
    let capability_id = Uuid::new_v4().to_string();
    let token = format!("{capability_id}:{}", Uuid::new_v4());
    let record = DelegatedInvocation {
        schema_version: 1,
        capability_id: capability_id.clone(),
        job_id: context.job_id.clone(),
        authority: context.authority.clone(),
        action,
        immutable_plan_sha256,
        immutable_campaign_sha256,
        argv_sha256: invocation_argv_sha256(argv)?,
        cwd: canonical_invocation_path(cwd, "working directory")?,
        scope_root: canonical_invocation_path(scope_root, "scope root")?,
        token_sha256: deadreckon_core::flight::sha256_text(&token),
        issued_at: Utc::now(),
    };
    commands::job::write_json_synced(
        &delegation_pending_path(paths, &context.job_id, &capability_id),
        &record,
    )?;
    Ok(PreparedDelegation {
        capability_id,
        job_id: context.job_id.clone(),
        token,
    })
}

pub(crate) fn apply_delegation(command: &mut Command, prepared: &PreparedDelegation) {
    commands::supervisor::remove_guarded_driver_metadata(command);
    command
        .env(DELEGATION_JOB_ENV, &prepared.job_id)
        .env(DELEGATION_ID_ENV, &prepared.capability_id)
        .stdin(Stdio::piped());
}

pub(crate) fn release_delegation(child: &mut Child, prepared: &PreparedDelegation) -> Result<()> {
    let mut input = child.stdin.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated child did not expose its private capability pipe".to_string(),
        ))
    })?;
    input.write_all(prepared.token.as_bytes())?;
    input.write_all(b"\n")?;
    input.flush()?;
    drop(input);
    Ok(())
}

pub(crate) fn revoke_pending_delegation(
    paths: &DeadreckonPaths,
    prepared: &PreparedDelegation,
) -> Result<()> {
    let pending = delegation_pending_path(paths, &prepared.job_id, &prepared.capability_id);
    match fs::remove_file(&pending) {
        Ok(()) => {
            if let Some(parent) = pending.parent() {
                sync_delegation_directory(parent)?;
            }
            Ok(())
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Core(DeadreckonError::Io {
            path: pending,
            source,
        })),
    }
}

pub(crate) fn spawn_delegated(
    paths: &DeadreckonPaths,
    command: &mut Command,
    prepared: &PreparedDelegation,
) -> Result<Child> {
    apply_delegation(command, prepared);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            revoke_pending_delegation(paths, prepared)?;
            return Err(source.into());
        }
    };
    if let Err(error) = release_delegation(&mut child, prepared) {
        let _ = child.kill();
        let _ = child.wait();
        if let Err(revoke_error) = revoke_pending_delegation(paths, prepared) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "delegated child release failed ({error}); capability revocation also failed ({revoke_error})"
            ))));
        }
        return Err(error);
    }
    Ok(child)
}

fn validate_delegated_action(paths: &DeadreckonPaths, record: &DelegatedInvocation) -> Result<()> {
    match &record.action {
        DelegatedAction::PlanChild {
            plan_id,
            task_id,
            task_index,
            task_attempt,
        } => {
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan is no longer Job-owned".to_string(),
                ))
            })?;
            if owner.job.job_id.as_ref() != record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan changed Job owner".to_string(),
                )));
            }
            validate_owned_plan_lineage(paths, &owner)?;
            if record.immutable_plan_sha256.as_deref()
                != Some(immutable_plan_sha256(&plan)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan definition changed after approval".to_string(),
                )));
            }
            let task = plan.tasks.get(*task_index as usize).ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child task index is absent".to_string(),
                ))
            })?;
            if task.task_id != *task_id
                || task.status != deadreckon_core::PlanTaskStatus::Running
                || task.attempts.len() as u32 + 1 != *task_attempt
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child does not match the exact running task attempt".to_string(),
                )));
            }
            let expected_scope = canonical_invocation_path(
                &paths.plan_dir(plan_id).join("launch").join(task_id),
                "task launch scope",
            )?;
            if expected_scope != record.scope_root {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child scope does not match its exact Plan task".to_string(),
                )));
            }
        }
        DelegatedAction::PlanFork { plan_id } | DelegatedAction::PlanMerge { plan_id } => {
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan operation is no longer Job-owned".to_string(),
                ))
            })?;
            if owner.job.job_id.as_ref() != record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan operation changed Job owner".to_string(),
                )));
            }
            validate_owned_plan_lineage(paths, &owner)?;
            if record.immutable_plan_sha256.as_deref()
                != Some(immutable_plan_sha256(&plan)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan definition changed after approval".to_string(),
                )));
            }
        }
        DelegatedAction::CampaignSub {
            campaign_id,
            sub_id,
            plan_id,
        } => {
            if campaign_id != &record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated campaign does not match its durable Job".to_string(),
                )));
            }
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(campaign_id))?;
            validate_owned_campaign(paths, &campaign, &record.job_id)?;
            if record.immutable_campaign_sha256.as_deref()
                != Some(immutable_campaign_sha256(&campaign)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Campaign definition changed after approval".to_string(),
                )));
            }
            if !campaign.sub_goals.iter().any(|sub| {
                sub.sub_id == *sub_id && sub.sub_plan_id.as_deref() == Some(plan_id.as_str())
            }) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated campaign sub-goal or reserved Plan is absent from the approved schedule"
                        .to_string(),
                )));
            }
            if std::env::var(deadreckon_core::campaign::ENV_SUB_PLAN_ID).as_deref()
                != Ok(plan_id.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Campaign child did not receive its exact reserved Plan identity"
                        .to_string(),
                )));
            }
        }
    }
    Ok(())
}

fn claim_delegation_record(pending: &Path, consumed: &Path, raw: &[u8]) -> Result<()> {
    let pending_parent = pending.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation pending path has no parent".to_string(),
        ))
    })?;
    let consumed_parent = consumed.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation consumed path has no parent".to_string(),
        ))
    })?;
    fs::create_dir_all(consumed_parent)?;
    let mut tombstone = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(consumed)
    {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation capability was already consumed".to_string(),
            )));
        }
        Err(source) => {
            return Err(CliError::Core(DeadreckonError::Io {
                path: consumed.to_path_buf(),
                source,
            }));
        }
    };
    tombstone.write_all(raw)?;
    tombstone.sync_all()?;
    sync_delegation_directory(consumed_parent)?;
    fs::remove_file(pending)?;
    sync_delegation_directory(pending_parent)?;
    Ok(())
}

pub(crate) fn authorize_delegated_invocation_if_present() -> Result<bool> {
    let Some(job_id) = std::env::var_os(DELEGATION_JOB_ENV).filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let job_id = job_id.to_str().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation Job identity is not UTF-8".to_string(),
        ))
    })?;
    let capability_id = std::env::var(DELEGATION_ID_ENV).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation is missing its capability identity".to_string(),
        ))
    })?;
    Uuid::parse_str(&capability_id).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation capability identity is malformed".to_string(),
        ))
    })?;
    let paths = DeadreckonPaths::discover();
    let pending = delegation_pending_path(&paths, job_id, &capability_id);
    let metadata = fs::metadata(&pending).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation has no pending protected capability record".to_string(),
            ))
        } else {
            CliError::from(source)
        }
    })?;
    if metadata.len() > MAX_DELEGATION_RECORD_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation record exceeded its bounded size".to_string(),
        )));
    }
    let raw = fs::read(&pending)?;
    let record: DelegatedInvocation = serde_json::from_slice(&raw)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.capability_id != capability_id
        || record.authority.job_id != job_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation identity does not match its protected record".to_string(),
        )));
    }

    let mut token_bytes = Vec::new();
    std::io::stdin()
        .take(MAX_DELEGATION_TOKEN_BYTES + 1)
        .read_to_end(&mut token_bytes)?;
    if token_bytes.len() as u64 > MAX_DELEGATION_TOKEN_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation token exceeded its bounded size".to_string(),
        )));
    }
    let token = std::str::from_utf8(&token_bytes)
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation token was not UTF-8".to_string(),
            ))
        })?
        .trim();
    if token.is_empty() || deadreckon_core::flight::sha256_text(token) != record.token_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation did not receive its private one-time capability".to_string(),
        )));
    }
    if invocation_argv_sha256(std::env::args_os().skip(1))? != record.argv_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation arguments changed after authorization".to_string(),
        )));
    }
    if canonical_invocation_path(&std::env::current_dir()?, "working directory")? != record.cwd {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation working directory changed after authorization".to_string(),
        )));
    }
    let scope_root = std::env::var_os("DEADRECKON_SCOPE_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation is missing its exact scope root".to_string(),
            ))
        })?;
    if canonical_invocation_path(&scope_root, "scope root")? != record.scope_root {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation scope changed after authorization".to_string(),
        )));
    }
    if !commands::supervisor::guarded_driver_authority_is_live(&paths, &record.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation is not bound to the current live Job driver".to_string(),
        )));
    }
    validate_delegated_action(&paths, &record)?;

    let consumed = delegation_consumed_path(&paths, job_id, &capability_id);
    claim_delegation_record(&pending, &consumed, &raw)?;
    match &record.action {
        action @ DelegatedAction::PlanChild { .. } => {
            DELEGATED_PLAN_CHILD.set(action.clone()).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "this process already consumed another Plan child capability".to_string(),
                ))
            })?;
        }
        DelegatedAction::PlanFork { .. }
        | DelegatedAction::PlanMerge { .. }
        | DelegatedAction::CampaignSub { .. } => {
            install_driver_context(&paths, record.authority, false)?;
        }
    }
    Ok(true)
}

pub(crate) fn require_current_driver_for_job_artifact(
    paths: &DeadreckonPaths,
    artifact_id: &str,
    expected_shape: JobShape,
    operation: &str,
) -> Result<()> {
    let owner = if expected_shape == JobShape::Graph {
        let plan = deadreckon_core::load_plan(paths, artifact_id)?;
        resolve_plan_owner(paths, &plan)?
    } else if paths.job_json(artifact_id).is_file() {
        let job = deadreckon_core::load_job(paths, artifact_id)?;
        if job.job_id.as_ref() != artifact_id || job.shape != expected_shape {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{operation} found artifact {artifact_id} colliding with incompatible durable Job {}",
                job.job_id
            ))));
        }
        Some(ResolvedPlanOwner {
            root_plan_id: artifact_id.to_string(),
            lineage: vec![artifact_id.to_string()],
            job,
        })
    } else {
        None
    };
    let Some(owner) = owner else {
        return Ok(());
    };
    if owner.root_plan_id.trim().is_empty() || owner.lineage.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{operation} found incomplete durable Plan ownership for {artifact_id}"
        ))));
    }
    if let Some(context) = DRIVER_CONTEXT
        .get()
        .filter(|context| context.job_id == owner.job.job_id.as_ref())
    {
        if context.authority.job_id != owner.job.job_id.as_ref()
            || !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)?
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{operation} refused stale authority for durable Job {}",
                owner.job.job_id
            ))));
        }
        validate_owned_plan_lineage(paths, &owner)?;
        return Ok(());
    }
    Err(CliError::Core(deadreckon_core::user_error(
        &format!(
            "{operation} cannot mutate {} because it belongs to durable Job {}",
            run_prefix(artifact_id),
            run_prefix(owner.job.job_id.as_ref())
        ),
        &format!(
            "deadreckon attach {}",
            run_prefix(owner.job.job_id.as_ref())
        ),
    )))
}

pub(crate) fn record_current_artifact(
    paths: &DeadreckonPaths,
    kind: DriverKind,
    artifact_kind: &str,
    artifact_id: &str,
) -> Result<()> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(());
    };
    if !context.root_artifact {
        return Ok(());
    }
    let job_id = context.job_id.as_str();
    if job_id != artifact_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced driver artifact {artifact_id} does not retain parent job identity {job_id}"
        ))));
    }
    write_driver_state(paths, job_id, kind, artifact_kind, artifact_id)
}

fn write_driver_state(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: DriverKind,
    artifact_kind: &str,
    artifact_id: &str,
) -> Result<()> {
    if job_id != artifact_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced artifact {artifact_id} does not retain parent Job identity {job_id}"
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

fn validate_root_planner_accounting(
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    let has_identity = accounting
        .provider
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && accounting
            .model
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let numeric_valid = accounting.cost_usd.is_finite()
        && accounting.cost_usd >= 0.0
        && accounting.wall_seconds.is_finite()
        && accounting.wall_seconds >= 0.0;
    let empty_snapshot = accounting.provider.is_none()
        && accounting.model.is_none()
        && accounting.input_tokens == 0
        && accounting.output_tokens == 0
        && accounting.cost_usd == 0.0
        && !accounting.subscription
        && accounting.wall_seconds == 0.0;
    if accounting.schema_version != 1
        || !numeric_valid
        || (accounting.planner_invoked && !has_identity)
        || (!accounting.planner_invoked && !empty_snapshot)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "root planner accounting snapshot is malformed or internally inconsistent".to_string(),
        )));
    }
    Ok(())
}

pub(crate) fn root_planner_budget_exhaustion(
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
    max_spend_usd: f64,
    max_wall_seconds: f64,
) -> Result<Option<RootPlannerBudgetExhaustion>> {
    validate_root_planner_accounting(accounting)?;
    if accounting.cost_usd >= max_spend_usd {
        return Ok(Some(RootPlannerBudgetExhaustion {
            dimension: deadreckon_core::plan::BudgetDimension::Spend,
            stop_reason: StopReason::SpendCap,
            reason: format!(
                "root planner exhausted the approved spend cap before child launch (${:.6} used of ${max_spend_usd:.6})",
                accounting.cost_usd
            ),
        }));
    }
    if accounting.wall_seconds >= max_wall_seconds {
        return Ok(Some(RootPlannerBudgetExhaustion {
            dimension: deadreckon_core::plan::BudgetDimension::Wall,
            stop_reason: StopReason::WallCap,
            reason: format!(
                "root planner exhausted the approved wall-time cap before child launch ({:.3}s used of {max_wall_seconds:.3}s)",
                accounting.wall_seconds
            ),
        }));
    }
    Ok(None)
}

fn restore_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    validate_root_planner_accounting(accounting)?;
    let path = paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE);
    if path.is_file() {
        let existing = load_plan_planner_accounting(paths, plan_id)?;
        if existing != *accounting {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {plan_id} root planner accounting disagrees with its crash-safe snapshot"
            ))));
        }
        return Ok(());
    }
    record_plan_planner_accounting_snapshot(paths, plan_id, accounting)
}

fn validated_driver_spec_for_recovery(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<DriverSpec> {
    let launch_path = paths.job_launch_plan(job.job_id.as_ref());
    if deadreckon_core::flight::sha256_file(&launch_path)? != job.launch_plan_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced Job launch plan changed before artifact recovery".to_string(),
        )));
    }
    let authority_path = paths.job_authority(job.job_id.as_ref());
    if deadreckon_core::flight::sha256_file(&authority_path)? != job.authority_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced Job authority changed before artifact recovery".to_string(),
        )));
    }
    let launch = commands::course::load_launch_plan(&launch_path)?;
    driver_spec(&launch)
}

/// Repair only the mapping side of a crash-partial advanced root creation.
///
/// The root ID is deterministic: it is the durable Job ID. Recovery therefore
/// validates that exact pending artifact and reconstructs its derived
/// ownership/accounting/mapping records. It never asks a planner to create a
/// replacement and never invents zero planner usage.
pub(crate) fn recover_pending_driver_state(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<PendingDriverRecovery> {
    if !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign) {
        return Ok(PendingDriverRecovery::Unchanged);
    }
    let view = deadreckon_core::JobView::load(paths, job.job_id.as_ref())?;
    if view.projection.is_terminal() || view.projection.attempt_count == 0 {
        return Ok(PendingDriverRecovery::Unchanged);
    }
    let mapping_exists = driver_state_path(paths, job.job_id.as_ref()).exists();
    let driver = validated_driver_spec_for_recovery(paths, job)?;
    match job.shape {
        JobShape::Graph => {
            if !paths.plan_json(job.job_id.as_ref()).is_file() {
                if mapping_exists {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "Graph Job {} has an artifact mapping but no root Plan",
                        job.job_id
                    ))));
                }
                return Ok(PendingDriverRecovery::Unchanged);
            }
            let mut plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
            let expected_kind = match plan.mode {
                deadreckon_core::plan::PlanMode::Review => DriverKind::Review,
                deadreckon_core::plan::PlanMode::FullPlan => DriverKind::FullPlan,
            };
            if plan.plan_id != job.job_id.as_ref()
                || plan.owner_job_id.as_deref() != Some(job.job_id.as_ref())
                || plan.parent_plan_id.is_some()
                || plan.root_goal != job.goal
                || plan.parent_scope.as_deref() != Some(job.scope.as_str())
                || plan.parent_cwd.as_deref() != Some(job.source_cwd.as_path())
                || plan.acceptance_path.as_deref()
                    != Some(
                        commands::job::job_acceptance_path(paths, job.job_id.as_ref()).as_path(),
                    )
                || driver.kind != expected_kind
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Plan {} does not match durable Graph Job {}",
                    plan.plan_id, job.job_id
                ))));
            }
            if plan.status != deadreckon_core::plan::PlanStatus::Pending {
                if mapping_exists {
                    return Ok(PendingDriverRecovery::Unchanged);
                }
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "unmapped root Plan {} is no longer pending",
                    plan.plan_id
                ))));
            }
            let accounting = plan.root_planner_accounting.as_ref().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Plan {} has no crash-safe root planner accounting",
                    plan.plan_id
                )))
            })?;
            restore_plan_planner_accounting(paths, &plan.plan_id, accounting)?;
            record_owned_plan_tree(paths, &plan)?;
            write_driver_state(
                paths,
                job.job_id.as_ref(),
                expected_kind,
                "plan",
                &plan.plan_id,
            )?;
            if let Some(exhaustion) = root_planner_budget_exhaustion(
                accounting,
                job.policy.max_spend_usd,
                job.policy.max_wall_seconds as f64,
            )? {
                if !deadreckon_core::read_plan_events(paths, &plan.plan_id)?
                    .iter()
                    .any(|event| {
                        matches!(
                            event.event,
                            deadreckon_core::PlanEventKind::RootBudgetExhausted { .. }
                        )
                    })
                {
                    deadreckon_core::append_plan_event(
                        paths,
                        &plan.plan_id,
                        deadreckon_core::PlanEventKind::RootBudgetExhausted {
                            dimension: exhaustion.dimension,
                            reason: exhaustion.reason.clone(),
                        },
                    )?;
                }
                plan.status = deadreckon_core::plan::PlanStatus::Failed;
                deadreckon_core::plan::save_plan(paths, &plan)?;
                return Ok(PendingDriverRecovery::BudgetExhausted {
                    stop_reason: exhaustion.stop_reason,
                    reason: exhaustion.reason,
                });
            }
            Ok(if mapping_exists {
                PendingDriverRecovery::Unchanged
            } else {
                PendingDriverRecovery::Recovered
            })
        }
        JobShape::LegacyCampaign => {
            let campaign_dir = paths.plan_dir(job.job_id.as_ref());
            if !campaign_dir.join("campaign.json").is_file() {
                if mapping_exists {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "Campaign Job {} has an artifact mapping but no root Campaign",
                        job.job_id
                    ))));
                }
                return Ok(PendingDriverRecovery::Unchanged);
            }
            let mut campaign = deadreckon_core::campaign::read_campaign(&campaign_dir)?;
            if driver.kind != DriverKind::Campaign
                || campaign.campaign_id != job.job_id.as_ref()
                || campaign.root_goal != job.goal
                || campaign.depth != 0
                || driver
                    .child_count
                    .is_some_and(|count| u32::from(count) != campaign.n)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Campaign {} does not match durable Campaign Job {}",
                    campaign.campaign_id, job.job_id
                ))));
            }
            if campaign.status != deadreckon_core::campaign::CampaignStatus::Pending {
                if mapping_exists {
                    return Ok(PendingDriverRecovery::Unchanged);
                }
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "unmapped root Campaign {} is no longer pending",
                    campaign.campaign_id
                ))));
            }
            let accounting = campaign.root_planner_accounting.as_ref().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Campaign {} has no crash-safe root planner accounting",
                    campaign.campaign_id
                )))
            })?;
            validate_root_planner_accounting(accounting)?;
            commands::campaign::restore_campaign_planner_accounting_snapshot(
                &campaign_dir,
                accounting,
            )?;
            write_owned_campaign_record(paths, job.job_id.as_ref(), &campaign)?;
            write_driver_state(
                paths,
                job.job_id.as_ref(),
                DriverKind::Campaign,
                "campaign",
                &campaign.campaign_id,
            )?;
            if let Some(exhaustion) = root_planner_budget_exhaustion(
                accounting,
                job.policy.max_spend_usd,
                job.policy.max_wall_seconds as f64,
            )? {
                if !deadreckon_core::campaign::read_campaign_events(&campaign_dir)?
                    .iter()
                    .any(|event| {
                        event.kind == "budget_exhausted"
                            && event
                                .detail
                                .get("phase")
                                .and_then(serde_json::Value::as_str)
                                == Some("root_planner")
                    })
                {
                    deadreckon_core::campaign::append_campaign_event(
                        &campaign_dir,
                        "budget_exhausted",
                        json!({
                            "reason": exhaustion.reason,
                            "phase": "root_planner",
                            "child_launches": 0,
                            "stop_reason": exhaustion.stop_reason,
                        }),
                    )?;
                }
                campaign.status = deadreckon_core::campaign::CampaignStatus::Failed;
                deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)?;
                return Ok(PendingDriverRecovery::BudgetExhausted {
                    stop_reason: exhaustion.stop_reason,
                    reason: exhaustion.reason,
                });
            }
            Ok(if mapping_exists {
                PendingDriverRecovery::Unchanged
            } else {
                PendingDriverRecovery::Recovered
            })
        }
        JobShape::Single | JobShape::LegacyChain => Ok(PendingDriverRecovery::Unchanged),
    }
}

pub(crate) async fn drive_job_command(job_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let authority = commands::supervisor::require_guarded_driver_launch(&paths, &job_id)?;
    let job = deadreckon_core::load_job(&paths, &job_id)?;
    if job.job_id.as_ref() != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced driver job identity mismatch".to_string(),
        )));
    }
    let plan = commands::course::load_launch_plan(&paths.job_launch_plan(&job_id))?;
    let driver = driver_spec(&plan)?;
    install_driver_context(&paths, authority, true)?;
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
        let launch_owner = parent_gate_launch_owner(paths, job)?;
        if let Err(error) = deadreckon_runtime::run_deterministic_completion_gate(
            &parent,
            backend,
            Some(&launch_owner),
            None,
        )
        .await
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
        let launch_owner = parent_gate_launch_owner(paths, job)?;
        if let Err(error) = deadreckon_runtime::run_deterministic_completion_gate(
            &parent,
            backend,
            Some(&launch_owner),
            None,
        )
        .await
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
    deadreckon_core::copy_deliverable_tree(&merged.working_dir, &state.working_dir)?;
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
    let mut parent_index =
        deadreckon_core::flight::build_deliverable_file_index(&parent.working_dir)?;
    parent_index.files.remove(Path::new("manifest.json"));
    let parent_hash = parent_index.tree_hash();
    let mut merged_index =
        deadreckon_core::flight::build_deliverable_file_index(&merged.working_dir)?;
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
) -> Result<deadreckon_core::plan::RootPlannerAccounting> {
    let path = paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE);
    let raw = fs::read(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?;
    let accounting: deadreckon_core::plan::RootPlannerAccounting = serde_json::from_slice(&raw)
        .map_err(|source| {
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

fn parent_gate_launch_owner(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<deadreckon_runtime::GateLaunchOwner> {
    let view = deadreckon_core::JobView::load(paths, job.job_id.as_ref())?;
    let attempt = view.projection.attempt_count;
    let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))?;
    let linked = history
        .events()
        .iter()
        .rev()
        .find(|event| {
            event.kind == deadreckon_protocol::JobEventKind::ChildLinked
                && event
                    .detail
                    .get("attempt")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(attempt))
        })
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "strict parent gate has no durable outer launch for Job attempt {attempt}"
            )))
        })?;
    let outer_launch_id = linked
        .detail
        .get("launch_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "strict parent gate has no launch identity for Job attempt {attempt}"
            )))
        })?;
    deadreckon_runtime::GateLaunchOwner::new(attempt, outer_launch_id).map_err(CliError::Core)
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

    fn owned_plan_fixture() -> (
        tempfile::TempDir,
        DeadreckonPaths,
        deadreckon_core::plan::Plan,
        deadreckon_core::plan::Plan,
    ) {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source_cwd = temp.path().join("source");
        std::fs::create_dir_all(&source_cwd).expect("source");
        let job_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        deadreckon_core::write_job(
            &paths,
            &deadreckon_protocol::Job {
                schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                scope: "owned-plan-test".to_string(),
                goal: "complete the owned graph".to_string(),
                shape: JobShape::Graph,
                created_at: Utc::now(),
                source_cwd,
                launch_plan_sha256: "sha256:launch".to_string(),
                authority_sha256: "sha256:authority".to_string(),
                policy: deadreckon_protocol::JobPolicy {
                    max_spend_usd: 1.0,
                    max_wall_seconds: 60,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: deadreckon_protocol::SemanticJudgeMode::Required,
                    execution: None,
                },
            },
        )
        .expect("Job");

        let tasks = || {
            vec![
                PlanTask::new(0, "first", "complete first", PlanRole::Child, None),
                PlanTask::new(1, "second", "complete second", PlanRole::Child, None),
            ]
        };
        let mut child = Plan::new(
            "complete the nested work",
            PlanMode::FullPlan,
            tasks(),
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("child Plan");
        child.plan_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        child.owner_job_id = Some(job_id.to_string());
        child.parent_plan_id = Some(job_id.to_string());

        let mut root = Plan::new(
            "complete the owned graph",
            PlanMode::FullPlan,
            tasks(),
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("root Plan");
        root.plan_id = job_id.to_string();
        root.owner_job_id = Some(job_id.to_string());
        root.tasks[0].subplan = Some(child.plan_id.clone());
        save_plan(&paths, &child).expect("save child");
        save_plan(&paths, &root).expect("save root");
        (temp, paths, root, child)
    }

    #[test]
    fn owned_plan_lineage_and_definition_fail_closed() {
        use deadreckon_core::plan::{PlanStatus, PlanTaskStatus, save_plan};

        let (_temp, paths, root, mut child) = owned_plan_fixture();
        let owner = resolve_plan_owner(&paths, &child)
            .expect("resolve")
            .expect("owned");
        assert_eq!(owner.job.job_id.as_ref(), root.plan_id);
        assert_eq!(owner.root_plan_id, root.plan_id);
        assert_eq!(
            owner.lineage,
            vec![child.plan_id.clone(), root.plan_id.clone()]
        );

        record_owned_plan_tree(&paths, &root).expect("freeze definitions");
        validate_owned_plan_if_present(&paths, &root.plan_id, &root.plan_id, &root.plan_id)
            .expect("root definition");
        validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
            .expect("child definition");

        let mut tampered_root = root.clone();
        tampered_root.tasks[0].goal = "tampered parent scheduling goal".to_string();
        save_plan(&paths, &tampered_root).expect("save tampered parent");
        let owner = resolve_plan_owner(&paths, &child)
            .expect("resolve tampered lineage")
            .expect("owned");
        let parent_error =
            validate_owned_plan_lineage(&paths, &owner).expect_err("parent tampering must fail");
        assert!(
            parent_error
                .to_string()
                .contains("protected Job-owned definition"),
            "{parent_error}"
        );
        save_plan(&paths, &root).expect("restore parent");

        child.status = PlanStatus::Forked;
        child.conductor_pid = Some(123);
        child.tasks[0].status = PlanTaskStatus::Running;
        save_plan(&paths, &child).expect("save mutable lifecycle");
        validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
            .expect("mutable lifecycle fields are not authority");

        child.tasks[0].goal = "tampered approved goal".to_string();
        save_plan(&paths, &child).expect("save tampered definition");
        let error =
            validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
                .expect_err("definition tampering must fail");
        assert!(
            error.to_string().contains("protected Job-owned definition"),
            "{error}"
        );
    }

    #[test]
    fn owned_plan_lineage_rejects_broken_links_cycles_and_excess_depth() {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let (_temp, paths, mut root, child) = owned_plan_fixture();
        root.tasks[0].subplan = None;
        save_plan(&paths, &root).expect("broken root");
        let broken = resolve_plan_owner(&paths, &child).expect_err("broken reverse link");
        assert!(
            broken.to_string().contains("unique parent-task link"),
            "{broken}"
        );

        let (_temp, paths, _root, mut child) = owned_plan_fixture();
        child.parent_plan_id = Some(child.plan_id.clone());
        child.tasks[0].subplan = Some(child.plan_id.clone());
        save_plan(&paths, &child).expect("cyclic child");
        let cycle = resolve_plan_owner(&paths, &child).expect_err("cycle");
        assert!(cycle.to_string().contains("cycle"), "{cycle}");

        let (_temp, paths, root, mut child) = owned_plan_fixture();
        let tasks = vec![
            PlanTask::new(
                0,
                "deep first",
                "complete deep first",
                PlanRole::Child,
                None,
            ),
            PlanTask::new(
                1,
                "deep second",
                "complete deep second",
                PlanRole::Child,
                None,
            ),
        ];
        let mut grandchild = Plan::new(
            "unapproved excessive nesting",
            PlanMode::FullPlan,
            tasks,
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("grandchild");
        grandchild.plan_id = "cccccccccccccccccccccccccccccccc".to_string();
        grandchild.owner_job_id = root.owner_job_id.clone();
        grandchild.parent_plan_id = Some(child.plan_id.clone());
        child.tasks[0].subplan = Some(grandchild.plan_id.clone());
        save_plan(&paths, &child).expect("deep child");
        save_plan(&paths, &grandchild).expect("grandchild");
        let depth = resolve_plan_owner(&paths, &grandchild).expect_err("excess depth");
        assert!(depth.to_string().contains("nesting cap"), "{depth}");
    }

    #[test]
    fn owned_campaign_definition_excludes_lifecycle_but_detects_schedule_tampering() {
        use deadreckon_core::campaign::{Campaign, CampaignStatus, SubGoalStatus, build_sub_goals};
        use deadreckon_core::plan::PlanProviders;

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "dddddddddddddddddddddddddddddddd";
        let mut campaign = Campaign::new(
            "complete the protected campaign",
            build_sub_goals(
                vec!["complete first".to_string(), "complete second".to_string()],
                2,
            )
            .expect("sub-goals"),
            PlanProviders::default(),
            0,
            Some(2.0),
            Some(60.0),
            "test",
        )
        .expect("Campaign");
        campaign.campaign_id = job_id.to_string();
        let record = OwnedCampaignRecord {
            schema_version: 1,
            job_id: job_id.to_string(),
            immutable_definition_sha256: immutable_campaign_sha256(&campaign).expect("digest"),
            recorded_at: Utc::now(),
        };
        commands::job::write_json_synced(&owned_campaign_record_path(&paths, job_id), &record)
            .expect("protected Campaign record");
        validate_owned_campaign(&paths, &campaign, job_id).expect("initial definition");

        campaign.status = CampaignStatus::Forked;
        campaign.sub_goals[0].status = SubGoalStatus::Merged;
        campaign.sub_goals[0].sub_plan_id = Some("runtime-plan".to_string());
        campaign.sub_goals[0].result_run_id = Some("runtime-result".to_string());
        validate_owned_campaign(&paths, &campaign, job_id)
            .expect("mutable lifecycle remains valid");

        campaign.sub_goals[0].goal = "tampered scheduled work".to_string();
        let error = validate_owned_campaign(&paths, &campaign, job_id)
            .expect_err("schedule tampering must fail");
        assert!(
            error.to_string().contains("protected Job-owned definition"),
            "{error}"
        );
    }

    #[test]
    fn delegation_claim_refuses_replay_when_pending_and_consumed_both_exist() {
        let temp = tempfile::TempDir::new().expect("temp");
        let pending = temp.path().join("pending").join("capability.json");
        let consumed = temp.path().join("consumed").join("capability.json");
        fs::create_dir_all(pending.parent().expect("pending parent")).expect("pending directory");
        fs::write(&pending, b"first protected record").expect("pending record");

        claim_delegation_record(&pending, &consumed, b"first protected record")
            .expect("first claim");
        assert!(!pending.exists());
        assert_eq!(
            fs::read(&consumed).expect("consumed record"),
            b"first protected record"
        );

        fs::write(&pending, b"replayed protected record").expect("replayed pending record");
        let error = claim_delegation_record(&pending, &consumed, b"replayed protected record")
            .expect_err("consumed tombstone must fail closed");
        assert!(error.to_string().contains("already consumed"), "{error}");
        assert!(
            pending.exists(),
            "the replay record remains available for recovery diagnostics"
        );
        assert_eq!(
            fs::read(&consumed).expect("original tombstone"),
            b"first protected record",
            "a replay must not replace the original consumed evidence"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delegated_argv_hash_accepts_non_utf8_arguments_without_loss() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first = vec![
            OsString::from("run"),
            OsString::from_vec(vec![b'p', b'a', b't', b'h', 0xff]),
        ];
        let second = vec![
            OsString::from("run"),
            OsString::from_vec(vec![b'p', b'a', b't', b'h', 0xfe]),
        ];
        assert_eq!(
            invocation_argv_sha256(&first).expect("first digest"),
            invocation_argv_sha256(first.iter()).expect("stable digest")
        );
        assert_ne!(
            invocation_argv_sha256(&first).expect("first digest"),
            invocation_argv_sha256(&second).expect("second digest")
        );
    }

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
                execution: None,
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
