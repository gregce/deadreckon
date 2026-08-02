use super::super::*;
use crate::commands::orchestrate::recommend_child_count_for_goal;
use crate::commands::plan::{
    PlannerAccounting, build_full_plan_tasks_accounted, resolve_plan_providers,
};
use crate::commands::start::{
    classify_goal_shape_for_start, goal_shape_provider_route, write_goal_shape_preview_record,
};
use crate::plan_event_bus::JsonlTail;
use deadreckon_protocol::StopReason;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

// --- Campaign: sub-orchestrator spawn (P3) ---------------------------------
// A campaign launches each sub-goal as a full `orchestrate full-plan`
// subprocess, isolated by DEADRECKON_SCOPE_ROOT exactly like a plan child, plus
// the campaign lineage env so a depth-1 process refuses to fan out again. The
// sub reports back through a sub-result.json sidecar in its launch dir.

#[allow(dead_code)] // reserved campaign-fork launch shape; fields mirror the sub-orchestrator env/CLI contract
pub(crate) struct CampaignSubLaunch<'a> {
    pub(crate) home: &'a Path,
    pub(crate) source_dir: &'a Path,
    pub(crate) launch_dir: &'a Path,
    pub(crate) campaign_id: &'a str,
    pub(crate) sub_plan_id: &'a str,
    pub(crate) sub_goal: &'a str,
    pub(crate) sub_n: u8,
    pub(crate) sandbox: &'a str,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) plain: bool,
    pub(crate) planner_provider: Option<&'a str>,
    pub(crate) child_provider: Option<&'a str>,
    pub(crate) planner_model: Option<&'a str>,
    pub(crate) child_model: Option<&'a str>,
    pub(crate) narrate: bool,
    pub(crate) narrator_model: Option<&'a str>,
    pub(crate) ancestor_task_keys: &'a [String],
    pub(crate) ancestor_scopes: &'a [String],
}

enum CampaignSubChildProcess {
    Plain(std::process::Child),
    Durable(Box<commands::graph_job::CampaignSubProcess>),
}

impl CampaignSubChildProcess {
    fn try_wait(&mut self) -> Result<Option<Option<bool>>> {
        match self {
            Self::Plain(child) => Ok(child.try_wait()?.map(|status| Some(status.success()))),
            Self::Durable(child) => match child.try_wait()? {
                commands::graph_job::CampaignSubProcessPoll::Running => Ok(None),
                commands::graph_job::CampaignSubProcessPoll::Exited { success } => {
                    Ok(Some(success))
                }
            },
        }
    }

    fn revoke_pending(&self, paths: &DeadreckonPaths) -> Result<()> {
        if let Self::Durable(child) = self {
            child.revoke_pending(paths)?;
        }
        Ok(())
    }
}

#[allow(dead_code)] // reserved campaign-fork seam; tests pin the exact sub-orchestrator argv/env
pub(crate) fn build_sub_orchestrator_command(
    launch: &CampaignSubLaunch<'_>,
) -> Result<std::process::Command> {
    use deadreckon_core::campaign;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(launch.source_dir)
        .env("DEADRECKON_HOME", launch.home)
        .env("DEADRECKON_HINTS", "0")
        .env("DEADRECKON_SCOPE_ROOT", launch.launch_dir)
        .env(campaign::ENV_DEPTH, "1")
        .env(campaign::ENV_ROOT, launch.campaign_id)
        .env(campaign::ENV_SUB_RESULT, launch.launch_dir)
        .env(campaign::ENV_SUB_PLAN_ID, launch.sub_plan_id);
    if !launch.ancestor_task_keys.is_empty() {
        command.env(
            campaign::ENV_ANCESTOR_TASK_KEYS,
            launch.ancestor_task_keys.join(","),
        );
    }
    if !launch.ancestor_scopes.is_empty() {
        command.env(
            campaign::ENV_ANCESTOR_SCOPES,
            launch.ancestor_scopes.join(","),
        );
    }
    command
        .arg("orchestrate")
        .arg("full-plan")
        .arg(launch.sub_goal)
        .arg("--n")
        .arg(launch.sub_n.to_string())
        .arg("--yes")
        .arg("--no-hints")
        .arg("--sandbox")
        .arg(launch.sandbox);
    if let Some(acceptance) = commands::graph_job::current_acceptance_path() {
        command.arg("--acceptance").arg(acceptance);
    }
    if launch.plain {
        command.arg("--plain");
    }
    if let Some(max_spend) = launch.max_spend {
        command.arg("--max-spend").arg(format!("{max_spend:.6}"));
    }
    if let Some(max_wall_seconds) = launch.max_wall_seconds {
        command
            .arg("--max-wall-seconds")
            .arg(format!("{max_wall_seconds:.6}"));
    }
    if let Some(planner) = launch.planner_provider {
        command.arg("--planner-provider").arg(planner);
    }
    if let Some(provider) = launch.child_provider {
        command.arg("--provider").arg(provider);
    }
    if let Some(planner_model) = launch.planner_model {
        command.arg("--planner-model").arg(planner_model);
    }
    if let Some(child_model) = launch.child_model {
        command.arg("--model").arg(child_model);
    }
    if launch.narrate {
        command.arg("--narrate");
        if let Some(narrator_model) = launch.narrator_model {
            command.arg("--narrator-model").arg(narrator_model);
        }
    }
    Ok(command)
}

#[allow(dead_code)] // reserved campaign-fork sidecar seam; kept separate for result-discovery tests
pub(crate) fn discover_sub_result(
    launch_dir: &Path,
) -> Result<Option<deadreckon_core::campaign::SubResult>> {
    deadreckon_core::campaign::read_sub_result(launch_dir).map_err(CliError::Core)
}

fn recover_persisted_campaign_sub(
    paths: &DeadreckonPaths,
    source_dir: &Path,
    launch_dir: &Path,
    sub: &deadreckon_core::campaign::SubGoal,
    sandbox: &str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
) -> Result<Option<deadreckon_core::campaign::SubResult>> {
    use deadreckon_core::plan::PlanStatus;

    if let Some(result) = discover_sub_result(launch_dir)? {
        let expected_plan_id = sub.sub_plan_id.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "campaign sub-result {} has no protected reserved Plan identity",
                result.sub_id
            )))
        })?;
        if result.schema_version != 1
            || result.sub_id != sub.sub_id
            || result.plan_id.as_deref() != Some(expected_plan_id)
            || sub
                .result_run_id
                .as_ref()
                .zip(result.result_run_id.as_ref())
                .is_some_and(|(expected, actual)| expected != actual)
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "campaign sub-result identity {} does not match persisted {}",
                result.sub_id, sub.sub_id
            ))));
        }
        let plan = deadreckon_core::plan::load_plan(paths, expected_plan_id)?;
        if plan.plan_id != expected_plan_id || plan.root_goal != sub.goal {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "campaign sub-result {} does not match protected Plan {expected_plan_id}",
                result.sub_id
            ))));
        }
        if let Some(job_id) = commands::graph_job::current_parent_job_id() {
            if plan.owner_job_id.as_deref() != Some(job_id) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign sub-result Plan {expected_plan_id} does not retain Campaign Job {job_id}"
                ))));
            }
            commands::graph_job::record_owned_plan_tree(paths, &plan)?;
        }
        let plan_proves_result = if result.ok {
            plan.status == PlanStatus::Merged
                && result.result_run_id.as_deref() == plan.merged_run_id.as_deref()
                && result.result_run_id.is_some()
        } else {
            plan.status == PlanStatus::Failed
        };
        if !plan_proves_result {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "campaign sub-result {} is not corroborated by protected Plan {expected_plan_id}",
                result.sub_id
            ))));
        }
        return Ok(Some(result));
    }
    let Some(plan_id) = sub.sub_plan_id.clone() else {
        return Ok(None);
    };
    if read_sub_plan_id(launch_dir).is_some_and(|published| published != plan_id) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "published campaign sub-plan identity does not match persisted sub {} plan {plan_id}",
            sub.sub_id
        ))));
    }
    if !paths.plan_json(&plan_id).is_file() {
        return Ok(None);
    }
    let plan = deadreckon_core::plan::load_plan(paths, &plan_id)?;
    if plan.plan_id != plan_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "linked campaign sub-plan {plan_id} changed identity"
        ))));
    }
    if plan.root_goal != sub.goal {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "linked campaign sub-plan {plan_id} does not match sub {} goal",
            sub.sub_id
        ))));
    }
    if let Some(job_id) = commands::graph_job::current_parent_job_id() {
        if plan.owner_job_id.as_deref() != Some(job_id) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "linked campaign sub-plan {plan_id} does not retain Campaign Job {job_id}"
            ))));
        }
        commands::graph_job::record_owned_plan_tree(paths, &plan)?;
    }
    match plan.status {
        PlanStatus::Failed => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "linked campaign sub-plan {plan_id} failed and cannot be resumed automatically"
            ))));
        }
        PlanStatus::Merged => {}
        PlanStatus::Pending | PlanStatus::Forked => {
            let executable = std::env::current_exe()?;
            let mut fork = std::process::Command::new(&executable);
            fork.current_dir(source_dir)
                .env("DEADRECKON_HOME", paths.home())
                .env("DEADRECKON_SCOPE_ROOT", launch_dir)
                .arg("fork")
                .arg(&plan_id)
                .arg("--sandbox")
                .arg(sandbox)
                .arg("--no-hints")
                .arg("--quiet")
                .arg("--plain");
            if let Some(max_spend) = max_spend {
                fork.arg("--max-spend").arg(format!("{max_spend:.6}"));
            }
            if let Some(max_wall_seconds) = max_wall_seconds {
                fork.arg("--max-wall-seconds")
                    .arg(format!("{max_wall_seconds:.3}"));
            }
            let status = if commands::graph_job::current_parent_job_id().is_some() {
                let argv = fork.get_args().map(ToOwned::to_owned).collect::<Vec<_>>();
                let delegation = commands::graph_job::prepare_delegated_invocation(
                    paths,
                    commands::graph_job::DelegatedAction::PlanFork {
                        plan_id: plan_id.clone(),
                    },
                    &argv,
                    source_dir,
                    launch_dir,
                    Some(&plan),
                )?;
                let mut child =
                    commands::graph_job::spawn_delegated(paths, &mut fork, &delegation)?;
                let status = child.wait();
                let revoke = commands::graph_job::revoke_pending_delegation(paths, &delegation);
                let status = status?;
                revoke?;
                status
            } else {
                fork.status()?
            };
            if !status.success() {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "linked campaign sub-plan {plan_id} fork resume exited with {status}"
                ))));
            }
            let mut merge = std::process::Command::new(executable);
            merge
                .current_dir(source_dir)
                .env("DEADRECKON_HOME", paths.home())
                .env("DEADRECKON_SCOPE_ROOT", launch_dir)
                .arg("merge")
                .arg(&plan_id)
                .arg("--strategy")
                .arg("dag-aware")
                .arg("--yes")
                .arg("--no-hints")
                .arg("--quiet")
                .arg("--plain");
            let status = if commands::graph_job::current_parent_job_id().is_some() {
                let resumed_plan = deadreckon_core::plan::load_plan(paths, &plan_id)?;
                let argv = merge.get_args().map(ToOwned::to_owned).collect::<Vec<_>>();
                let delegation = commands::graph_job::prepare_delegated_invocation(
                    paths,
                    commands::graph_job::DelegatedAction::PlanMerge {
                        plan_id: plan_id.clone(),
                    },
                    &argv,
                    source_dir,
                    launch_dir,
                    Some(&resumed_plan),
                )?;
                let mut child =
                    commands::graph_job::spawn_delegated(paths, &mut merge, &delegation)?;
                let status = child.wait();
                let revoke = commands::graph_job::revoke_pending_delegation(paths, &delegation);
                let status = status?;
                revoke?;
                status
            } else {
                merge.status()?
            };
            if !status.success() {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "linked campaign sub-plan {plan_id} merge resume exited with {status}"
                ))));
            }
        }
    }
    let resumed = deadreckon_core::plan::load_plan(paths, &plan_id)?;
    if resumed.status != PlanStatus::Merged {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "linked campaign sub-plan {plan_id} did not persist a merged result"
        ))));
    }
    let result_run_id = resumed.merged_run_id.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "linked campaign sub-plan {plan_id} has no merged result run"
        )))
    })?;
    let result = deadreckon_core::campaign::SubResult {
        schema_version: 1,
        sub_id: sub.sub_id.clone(),
        plan_id: Some(plan_id),
        result_run_id: Some(result_run_id),
        ok: true,
    };
    deadreckon_core::campaign::write_sub_result(launch_dir, &result)?;
    Ok(Some(result))
}

/// Filename of the early plan-id marker a sub-orchestrator publishes to its
/// launch dir so the campaign parent can tail its grandchildren mid-run (before
/// the result sidecar exists). Plain text: the sub-plan id.
const SUB_PLAN_ID_FILE: &str = "plan-id";

/// Publish a sub-orchestrator's plan id to its launch dir as soon as the plan
/// exists, so the campaign parent's live aggregate can discover the grandchild
/// run roots before the sub finishes.
pub(crate) fn publish_sub_plan_id(launch_dir: &Path, plan_id: &str) -> Result<()> {
    fs::create_dir_all(launch_dir)?;
    if let Some(existing) = read_sub_plan_id(launch_dir) {
        if existing == plan_id {
            return Ok(());
        }
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "campaign launch already reserves Plan {existing}, not {plan_id}"
        ))));
    }
    let path = launch_dir.join(SUB_PLAN_ID_FILE);
    let temporary = launch_dir.join(format!(".{SUB_PLAN_ID_FILE}.{}.tmp", Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(plan_id.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, &path)?;
    #[cfg(unix)]
    fs::File::open(launch_dir)?.sync_all()?;
    Ok(())
}

/// Read a sub-orchestrator's published plan id, if it has reached plan creation.
pub(crate) fn read_sub_plan_id(launch_dir: &Path) -> Option<String> {
    fs::read_to_string(launch_dir.join(SUB_PLAN_ID_FILE))
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|plan_id| !plan_id.is_empty())
}

pub(crate) fn campaign_test_failpoint(name: &str) {
    if std::env::var("DEADRECKON_TEST_CAMPAIGN_FAILPOINTS").as_deref() == Ok("1")
        && std::env::var("DEADRECKON_TEST_CAMPAIGN_FAILPOINT").as_deref() == Ok(name)
    {
        std::process::exit(86);
    }
}

/// Write a sub-orchestrator's result sidecar. Called at the end of
/// `orchestrate full-plan` when launched by a campaign (DEADRECKON_CAMPAIGN_SUB_RESULT
/// is set): records the plan id and its merged result run for the meta-coordinator.
pub(crate) fn record_sub_orchestrator_result(
    plan_id: &str,
    launch_dir: &Path,
    ok: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let result_run_id = deadreckon_core::plan::load_plan(&paths, plan_id)
        .ok()
        .and_then(|plan| plan.merged_run_id);
    let sub_id = launch_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sub")
        .to_string();
    let result = deadreckon_core::campaign::SubResult {
        schema_version: 1,
        sub_id,
        plan_id: Some(plan_id.to_string()),
        result_run_id,
        ok,
    };
    deadreckon_core::campaign::write_sub_result(launch_dir, &result).map_err(CliError::Core)
}

pub(crate) struct CampaignArgs {
    pub(crate) goal: String,
    pub(crate) n: Option<u8>,
    pub(crate) planner_provider: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) planner_model: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) deadline: Option<DateTime<Utc>>,
    pub(crate) sandbox: Option<String>,
    pub(crate) acceptance: Option<PathBuf>,
    pub(crate) preview: bool,
    pub(crate) yes: bool,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) narrate: bool,
    pub(crate) no_narrate: bool,
    pub(crate) narrator_model: Option<String>,
}

pub(crate) struct CampaignRepairArgs {
    pub(crate) campaign_id: String,
    pub(crate) repair_provider: Option<String>,
    pub(crate) repair_mode: String,
    pub(crate) repair_attempts: u32,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
}

struct CampaignRemainingBudget {
    spend_usd: Option<f64>,
    wall_seconds: Option<f64>,
    exhausted_reason: Option<String>,
    exhausted_stop_reason: Option<StopReason>,
}

fn campaign_remaining_after_planning(
    approved_spend_usd: Option<f64>,
    approved_wall_seconds: Option<f64>,
    accounting: Option<&PlannerAccounting>,
) -> Result<CampaignRemainingBudget> {
    let planner_spend = accounting.map_or(0.0, |value| value.spend.cost_usd);
    let planner_wall = accounting.map_or(0.0, |value| value.wall_seconds);
    if !planner_spend.is_finite()
        || planner_spend < 0.0
        || !planner_wall.is_finite()
        || planner_wall < 0.0
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign planner reported invalid spend or wall accounting".to_string(),
        )));
    }
    let spend_usd = approved_spend_usd.map(|cap| (cap - planner_spend).max(0.0));
    let wall_seconds = approved_wall_seconds.map(|cap| (cap - planner_wall).max(0.0));
    let spend_exhausted = approved_spend_usd.is_some_and(|cap| planner_spend >= cap);
    let wall_exhausted = approved_wall_seconds.is_some_and(|cap| planner_wall >= cap);
    let exhausted_stop_reason = if spend_exhausted {
        Some(StopReason::SpendCap)
    } else if wall_exhausted {
        Some(StopReason::WallCap)
    } else {
        None
    };
    let exhausted_reason = (spend_exhausted || wall_exhausted).then(|| {
        format!(
            "one token-bounded campaign planner completion used ${planner_spend:.6} and {planner_wall:.3}s, exhausting the approved campaign cap before child launch"
        )
    });
    Ok(CampaignRemainingBudget {
        spend_usd,
        wall_seconds,
        exhausted_reason,
        exhausted_stop_reason,
    })
}

fn merge_campaign_planner_accounting(
    current: &mut Option<PlannerAccounting>,
    next: Option<PlannerAccounting>,
) {
    let Some(next) = next else {
        return;
    };
    let Some(current) = current.as_mut() else {
        *current = Some(next);
        return;
    };
    if current.spend.provider != next.spend.provider {
        current.spend.provider = "multiple".to_string();
    }
    if current.spend.model != next.spend.model {
        current.spend.model = "multiple".to_string();
    }
    current.spend.input_tokens = current
        .spend
        .input_tokens
        .saturating_add(next.spend.input_tokens);
    current.spend.output_tokens = current
        .spend
        .output_tokens
        .saturating_add(next.spend.output_tokens);
    current.spend.cost_usd += next.spend.cost_usd;
    current.spend.subscription &= next.spend.subscription;
    current.wall_seconds += next.wall_seconds;
}

fn append_campaign_planner_accounting_snapshot(
    campaign_dir: &Path,
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    if !accounting.planner_invoked {
        return Ok(());
    }
    let detail = campaign_planner_accounting_detail(accounting);
    deadreckon_core::campaign::append_campaign_event(
        campaign_dir,
        "root_planner_accounting",
        detail,
    )
    .map_err(CliError::Core)
}

fn campaign_planner_accounting_detail(
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Value {
    json!({
        "schema_version": accounting.schema_version,
        "planner_invoked": accounting.planner_invoked,
        "provider": accounting.provider,
        "model": accounting.model,
        "input_tokens": accounting.input_tokens,
        "output_tokens": accounting.output_tokens,
        "cost_usd": accounting.cost_usd,
        "subscription": accounting.subscription,
        "wall_seconds": accounting.wall_seconds,
        "recorded_at": accounting.recorded_at,
        "cumulative": true,
        "overrun_policy": "one token-bounded planner completion may cross a very small cap; child launch is refused when planning exhausts the total",
    })
}

pub(crate) fn restore_campaign_planner_accounting_snapshot(
    campaign_dir: &Path,
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    if !accounting.planner_invoked {
        return Ok(());
    }
    let expected = campaign_planner_accounting_detail(accounting);
    let existing = deadreckon_core::campaign::read_campaign_events(campaign_dir)?
        .into_iter()
        .rev()
        .find(|event| event.kind == "root_planner_accounting");
    match existing {
        Some(event) if event.detail == expected => Ok(()),
        Some(_) => Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign root planner accounting disagrees with its crash-safe root snapshot"
                .to_string(),
        ))),
        None => deadreckon_core::campaign::append_campaign_event(
            campaign_dir,
            "root_planner_accounting",
            expected,
        )
        .map_err(CliError::Core),
    }
}

fn print_campaign_preflight(campaign: &deadreckon_core::campaign::Campaign, sandbox: Option<&str>) {
    let per_sub = campaign
        .tree_budget_usd
        .map(|budget| deadreckon_core::campaign::allocate_budget(budget, campaign.sub_goals.len()));
    let id = run_prefix(&campaign.campaign_id);
    let primary = format!(
        "deadreckon campaign \"{}\" --n {} --yes",
        shell_display_quote(&campaign.root_goal),
        campaign.n
    );

    println!("{} {}", ui_heading("Campaign preview"), ui_id(&id));
    println!();
    println!("{}", ui_heading("Goal"));
    print_campaign_wrapped("  ", &campaign.root_goal, CAMPAIGN_WRAP_WIDTH);
    println!();
    println!("{}", ui_heading("Plan"));
    print_campaign_fact("campaign", &id);
    print_campaign_fact("sub-goals", &campaign.n.to_string());
    print_campaign_fact(
        "depth cap",
        &deadreckon_core::campaign::CAMPAIGN_MAX_DEPTH.to_string(),
    );
    if let Some(sandbox) = sandbox {
        print_campaign_fact("sandbox", sandbox);
    }
    if let Some(planner) = campaign.providers.planner.as_deref() {
        print_campaign_fact("planner", planner);
    }
    if let Some(child) = campaign.providers.default_child.as_deref() {
        print_campaign_fact("workers", child);
    }
    if let (Some(total), Some(share)) = (
        campaign.tree_budget_usd,
        per_sub.as_ref().and_then(|s| s.first()),
    ) {
        print_campaign_fact(
            "budget",
            &format!("${total:.2} total (~${share:.2} per sub)"),
        );
    } else {
        print_campaign_fact("budget", "unbounded");
        print_campaign_wrapped(
            "             ",
            "Add --max-spend <usd> to cap the whole campaign tree.",
            CAMPAIGN_WRAP_WIDTH,
        );
    }
    if let Some(seconds) = campaign.tree_wall_seconds {
        print_campaign_fact("wall cap", &format_wall_cap(Some(seconds)));
    }

    println!();
    println!("{}", ui_heading("Next"));
    println!("  Press Enter or choose [1] to launch. Edit the split first if it looks wrong.");
    println!("  Launch without prompting:");
    for line in wrap_campaign_words(&primary, CAMPAIGN_WRAP_WIDTH.saturating_sub(4)) {
        println!("    {}", ui_command(line));
    }

    println!("{}", ui_heading("Sub-goals"));
    for sub in &campaign.sub_goals {
        print_campaign_sub_goal(sub);
    }
}

const CAMPAIGN_WRAP_WIDTH: usize = 88;

fn print_campaign_fact(label: &str, value: &str) {
    println!("  {} {value}", crate::ui::pad_visible(&ui_muted(label), 9));
}

fn print_campaign_sub_goal(sub: &deadreckon_core::campaign::SubGoal) {
    println!("  {}", ui_id(&sub.sub_id));
    if let Some((body, acceptance)) = split_acceptance_clause(&sub.goal) {
        print_campaign_wrapped("    ", body, CAMPAIGN_WRAP_WIDTH);
        println!("    {}", ui_heading("Acceptance"));
        print_campaign_wrapped("      ", acceptance, CAMPAIGN_WRAP_WIDTH);
    } else {
        print_campaign_wrapped("    ", &sub.goal, CAMPAIGN_WRAP_WIDTH);
    }
}

fn split_acceptance_clause(goal: &str) -> Option<(&str, &str)> {
    let (body, acceptance) = goal.split_once("Acceptance:")?;
    Some((body.trim(), acceptance.trim()))
}

fn print_campaign_wrapped(indent: &str, value: &str, width: usize) {
    for line in wrap_campaign_words(value, width.saturating_sub(indent.chars().count())) {
        println!("{indent}{line}");
    }
}

fn wrap_campaign_words(value: &str, width: usize) -> Vec<String> {
    if value.split_whitespace().next().is_none() {
        return vec!["-".to_string()];
    }
    crate::ui::wrap_words(value, width.max(16))
}

fn campaign_choice_detail(goal: &str) -> String {
    wrap_campaign_words(goal, 72)
        .into_iter()
        .next()
        .unwrap_or_else(|| "-".to_string())
}

fn confirm_campaign_start(campaign: &deadreckon_core::campaign::Campaign, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    let prompt = format!(
        "launch {} sub-orchestrators for \"{}\"?",
        campaign.n, campaign.root_goal
    );
    if prompt::confirm(&prompt, false)? {
        Ok(())
    } else {
        Err(CliError::Core(deadreckon_core::user_error(
            "campaign preflight cancelled",
            &format!(
                "deadreckon campaign \"{}\" --n {} --preview",
                shell_display_quote(&campaign.root_goal),
                campaign.n.clamp(2, 6)
            ),
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CampaignPreflightAction {
    Launch,
    NeedsConfirmation,
    ChangeCount(u8),
}

fn prompt_campaign_preflight_actions(
    campaign: &mut deadreckon_core::campaign::Campaign,
    sandbox: Option<&str>,
    yes: bool,
    quiet: bool,
) -> Result<CampaignPreflightAction> {
    if yes {
        return Ok(CampaignPreflightAction::Launch);
    }
    if quiet || !io::stdin().is_terminal() {
        return Ok(CampaignPreflightAction::NeedsConfirmation);
    }
    loop {
        let mut choices = vec![start_prompt_choice(
            "launch",
            "Launch now",
            format!("start {} sub-orchestrators", campaign.n),
        )];
        choices.push(start_prompt_choice(
            "edit",
            "Edit a sub-goal",
            "fix one item before launch",
        ));
        if campaign.sub_goals.len() > 2 {
            choices.push(start_prompt_choice(
                "drop",
                "Drop a sub-goal",
                "remove one item before launch",
            ));
        }
        choices.push(start_prompt_choice(
            "count",
            "Regenerate count",
            "ask the planner for a different split",
        ));
        choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
        let choice = prompt::select_one(&prompt::SelectPrompt {
            title: "Next step".to_string(),
            help: Some(
                "Enter launches this campaign. Edit first if the split is wrong.".to_string(),
            ),
            choices,
            default_index: 0,
        })?;
        match choice.id.as_str() {
            "launch" => return Ok(CampaignPreflightAction::Launch),
            "edit" => {
                let edit_choices = campaign
                    .sub_goals
                    .iter()
                    .map(|sub| {
                        start_prompt_choice(
                            sub.sub_id.clone(),
                            sub.sub_id.clone(),
                            campaign_choice_detail(&sub.goal),
                        )
                    })
                    .chain(std::iter::once(prompt::SelectChoice::new(
                        "cancel", "Cancel",
                    )))
                    .collect::<Vec<_>>();
                let edit = prompt::select_one(&prompt::SelectPrompt {
                    title: "Edit sub-goal".to_string(),
                    help: Some("Choose the sub-goal to update before launch.".to_string()),
                    choices: edit_choices,
                    default_index: 0,
                })?;
                if edit.id == "cancel" {
                    continue;
                }
                let current = campaign
                    .sub_goals
                    .iter()
                    .find(|sub| sub.sub_id == edit.id)
                    .map(|sub| sub.goal.clone())
                    .unwrap_or_default();
                let updated = prompt::open("sub-goal: ", Some(&current))?;
                campaign_edit_subgoal_before_launch(campaign, &edit.id, &updated)?;
                print_campaign_preflight(campaign, sandbox);
            }
            "drop" => {
                let drop_choices = campaign
                    .sub_goals
                    .iter()
                    .map(|sub| {
                        start_prompt_choice(
                            sub.sub_id.clone(),
                            sub.sub_id.clone(),
                            campaign_choice_detail(&sub.goal),
                        )
                    })
                    .chain(std::iter::once(prompt::SelectChoice::new(
                        "cancel", "Cancel",
                    )))
                    .collect::<Vec<_>>();
                let drop = prompt::select_one(&prompt::SelectPrompt {
                    title: "Drop sub-goal".to_string(),
                    help: Some("Choose the sub-goal to remove before launch.".to_string()),
                    choices: drop_choices,
                    default_index: 0,
                })?;
                if drop.id == "cancel" {
                    continue;
                }
                campaign_drop_subgoal_before_launch(campaign, &drop.id)?;
                print_campaign_preflight(campaign, sandbox);
            }
            "count" => {
                // Re-prompt on bad input instead of aborting the preflight.
                let count =
                    prompt::ask_number("sub-orchestrator count", 2..=6, campaign.n as usize)? as u8;
                return Ok(CampaignPreflightAction::ChangeCount(count));
            }
            _ => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "campaign preflight cancelled",
                    &format!(
                        "deadreckon campaign \"{}\" --n {} --preview",
                        shell_display_quote(&campaign.root_goal),
                        campaign.n.clamp(2, 6)
                    ),
                )));
            }
        }
    }
}

pub(crate) fn campaign_replace_sub_goals_before_launch(
    campaign: &mut deadreckon_core::campaign::Campaign,
    sub_goals: Vec<deadreckon_core::campaign::SubGoal>,
) -> Result<()> {
    if campaign.status != deadreckon_core::campaign::CampaignStatus::Pending {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign sub-goals can only be edited before launch",
            "deadreckon campaign <goal> --preview",
        )));
    }
    validate_task_count(sub_goals.len()).map_err(CliError::Core)?;
    campaign.n = sub_goals.len() as u32;
    campaign.sub_goals = sub_goals;
    Ok(())
}

pub(crate) fn campaign_edit_subgoal_before_launch(
    campaign: &mut deadreckon_core::campaign::Campaign,
    sub_id: &str,
    updated_goal: &str,
) -> Result<()> {
    let Some(index) = campaign
        .sub_goals
        .iter()
        .position(|sub| sub.sub_id == sub_id)
    else {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown campaign sub-goal {sub_id}"),
            "choose one of the sub ids shown in the campaign preflight",
        )));
    };
    let mut goals = campaign
        .sub_goals
        .iter()
        .map(|sub| sub.goal.clone())
        .collect::<Vec<_>>();
    goals[index] = updated_goal.trim().to_string();
    let sub_goals = deadreckon_core::campaign::build_sub_goals(goals, campaign.sub_goals.len())
        .map_err(CliError::Core)?;
    campaign_replace_sub_goals_before_launch(campaign, sub_goals)
}

pub(crate) fn campaign_drop_subgoal_before_launch(
    campaign: &mut deadreckon_core::campaign::Campaign,
    sub_id: &str,
) -> Result<()> {
    if campaign.status != deadreckon_core::campaign::CampaignStatus::Pending {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign sub-goals can only be dropped before launch",
            "deadreckon campaign <goal> --preview",
        )));
    }
    if campaign.sub_goals.len() <= 2 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign must keep at least 2 sub-goals",
            "drop a different sub-goal or cancel",
        )));
    }
    let Some(index) = campaign
        .sub_goals
        .iter()
        .position(|sub| sub.sub_id == sub_id)
    else {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown campaign sub-goal {sub_id}"),
            "choose one of the sub ids shown in the campaign preflight",
        )));
    };
    let mut goals = campaign
        .sub_goals
        .iter()
        .map(|sub| sub.goal.clone())
        .collect::<Vec<_>>();
    goals.remove(index);
    let sub_goals = deadreckon_core::campaign::build_sub_goals(goals, campaign.sub_goals.len() - 1)
        .map_err(CliError::Core)?;
    campaign_replace_sub_goals_before_launch(campaign, sub_goals)
}

/// Create the evidence run from the composed campaign tree, binding the
/// roll-up into the marker signature. Legacy direct campaigns promote here;
/// durable parent Jobs wait for the parent receipt.
fn promote_campaign_result(
    paths: &DeadreckonPaths,
    campaign_id: &str,
    merge_dir: &Path,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> Result<deadreckon_core::PipelineState> {
    let cwd = std::env::current_dir()?;
    let run_options = RunOptions {
        goal: format!("campaign {}", run_prefix(campaign_id)),
        cwd,
        sandbox: "none".to_string(),
        provider: Some("deadreckon:campaign".to_string()),
        skill_name: "default-coding".to_string(),
        max_spend_usd: None,
        max_wall_seconds: None,
        run_id: None,
        codebase: None,
    };
    let mut state = if let Some(job_id) = commands::graph_job::current_parent_job_id() {
        if job_id != campaign_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Campaign {campaign_id} is being driven by unrelated durable Job {job_id}"
            ))));
        }
        deadreckon_core::create_owned_run(
            paths,
            run_options,
            deadreckon_core::RunOwnership::campaign_result(job_id, campaign_id),
        )?
    } else {
        create_run(paths, run_options)?
    };
    remove_if_exists(&state.working_dir)?;
    copy_deliverable_tree(merge_dir, &state.working_dir)?;
    deadreckon_core::campaign::write_campaign_rollup_at_run_root(&state.run_root, rollup)?;
    record_campaign_result_accounting(paths, campaign_id, &mut state)?;
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )?;
    state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
    save_state(&state)?;
    if commands::graph_job::current_parent_job_id().is_none() {
        promote_completed_run(paths, &mut state)?;
    }
    Ok(state)
}

#[derive(Deserialize)]
struct PersistedPlannerAccounting {
    provider: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    subscription: bool,
    wall_seconds: f64,
}

fn read_campaign_planner_accounting(
    campaign_dir: &Path,
) -> Result<Option<PersistedPlannerAccounting>> {
    let event = deadreckon_core::campaign::read_campaign_events(campaign_dir)?
        .into_iter()
        .rev()
        .find(|event| event.kind == "root_planner_accounting");
    event
        .map(|event| serde_json::from_value(event.detail).map_err(CliError::from))
        .transpose()
}

fn record_campaign_result_accounting(
    paths: &DeadreckonPaths,
    campaign_id: &str,
    state: &mut deadreckon_core::PipelineState,
) -> Result<()> {
    let campaign_dir = paths.plan_dir(campaign_id);
    let campaign = deadreckon_core::campaign::read_campaign(&campaign_dir)?;
    let planner = read_campaign_planner_accounting(&campaign_dir)?;
    let total_cap = campaign
        .tree_budget_usd
        .map(|remaining| remaining + planner.as_ref().map_or(0.0, |value| value.cost_usd));
    let total_wall_cap = campaign
        .tree_wall_seconds
        .map(|remaining| remaining + planner.as_ref().map_or(0.0, |value| value.wall_seconds));
    if let Some(planner) = planner {
        state.total_spend_usd += planner.cost_usd;
        state.total_wall_seconds += planner.wall_seconds;
        deadreckon_core::append_spend(
            state,
            &SpendRecord {
                timestamp: Utc::now(),
                turn: state.turn,
                provider: planner.provider,
                model: planner.model,
                input_tokens: planner.input_tokens,
                output_tokens: planner.output_tokens,
                cost_usd: planner.cost_usd,
                total_cost_usd: state.total_spend_usd,
                cap_usd: total_cap,
                subscription: planner.subscription,
                estimated: false,
                wall_time_seconds: Some(planner.wall_seconds),
                wall_time_cap_seconds: total_wall_cap,
                kind: "campaign_root_planner".to_string(),
            },
        )?;
    }
    for sub in &campaign.sub_goals {
        let Some(run_id) = sub.result_run_id.as_deref() else {
            continue;
        };
        let child = load_run(paths, run_id)?;
        state.total_spend_usd += child.total_spend_usd;
        state.total_wall_seconds += child.total_wall_seconds;
        deadreckon_core::append_spend(
            state,
            &SpendRecord {
                timestamp: Utc::now(),
                turn: state.turn,
                provider: "deadreckon:campaign-subtree".to_string(),
                model: sub.sub_id.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: child.total_spend_usd,
                total_cost_usd: state.total_spend_usd,
                cap_usd: total_cap,
                subscription: false,
                estimated: false,
                wall_time_seconds: Some(child.total_wall_seconds),
                wall_time_cap_seconds: total_wall_cap,
                kind: "campaign_subtree".to_string(),
            },
        )?;
    }
    Ok(())
}

fn write_campaign_manifest(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    result_state: &deadreckon_core::PipelineState,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> Result<()> {
    let library_dir = paths.library_dir(&result_state.scope, &result_state.run_id);
    let manifest = serde_json::json!({
        "schema_version": 1,
        "campaign_id": campaign.campaign_id,
        "root_goal": campaign.root_goal,
        "n": campaign.n,
        "depth": campaign.depth,
        "subs": campaign.sub_goals.iter().map(|sub| serde_json::json!({
            "sub_id": sub.sub_id,
            "goal": sub.goal,
            "sub_plan_id": sub.sub_plan_id,
            "result_run_id": sub.result_run_id,
        })).collect::<Vec<_>>(),
        "result_run_id": result_state.run_id,
        "rollup_verdict": rollup.rollup_verdict,
        "refused_subs": rollup.refused_subs,
        "caveat_subs": rollup.caveat_subs,
    });
    let path = library_dir.join("deadreckon-campaign-manifest.json");
    fs::create_dir_all(&library_dir)?;
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )?;
    Ok(())
}

fn campaign_sub_context_path(
    paths: &DeadreckonPaths,
    campaign_dir: &Path,
    sub: &deadreckon_core::campaign::SubGoal,
) -> PathBuf {
    sub.sub_plan_id
        .as_deref()
        .map(|plan_id| paths.plan_json(plan_id))
        .unwrap_or_else(|| {
            campaign_dir
                .join("launch")
                .join(&sub.sub_id)
                .join("sub-result.json")
        })
}

fn campaign_sub_summary_path(
    paths: &DeadreckonPaths,
    sub: &deadreckon_core::campaign::SubGoal,
) -> Option<PathBuf> {
    let plan_id = sub.sub_plan_id.as_deref()?;
    let docs = paths
        .plan_dir(plan_id)
        .join(deadreckon_core::plan::PLAN_DOCS_DIR)
        .join(deadreckon_core::plan::PLAN_NARRATIVE);
    if docs.is_file() {
        Some(docs)
    } else {
        Some(paths.plan_json(plan_id))
    }
}

fn campaign_as_merge_repair_plan(
    paths: &DeadreckonPaths,
    campaign_dir: &Path,
    campaign: &deadreckon_core::campaign::Campaign,
    parent_cwd: &Path,
) -> Result<Plan> {
    let mut tasks = Vec::new();
    for (index, sub) in campaign.sub_goals.iter().enumerate() {
        let Some(run_id) = sub.result_run_id.clone() else {
            continue;
        };
        let mut task = PlanTask::new(
            index as u32,
            format!("{} {}", sub.sub_id, sub.goal),
            sub.goal.clone(),
            PlanRole::Child,
            campaign.providers.default_child.clone(),
        );
        task.task_id = sub.sub_id.clone();
        task.active_form = sub.goal.clone();
        task.worker_spec = campaign_sub_context_path(paths, campaign_dir, sub);
        task.summary_path = campaign_sub_summary_path(paths, sub);
        task.child_run_id = Some(run_id.clone());
        task.child_scope = sub
            .scope
            .clone()
            .or_else(|| load_run(paths, &run_id).ok().map(|state| state.scope));
        task.status = PlanTaskStatus::Completed;
        tasks.push(task);
    }
    let mut plan = Plan::new(
        campaign.root_goal.clone(),
        PlanMode::FullPlan,
        tasks,
        campaign.providers.clone(),
        None,
        campaign.deadreckon_version.clone(),
    )
    .map_err(CliError::Core)?;
    plan.plan_id = campaign.campaign_id.clone();
    plan.status = PlanStatus::Forked;
    plan.forked_at = campaign.forked_at;
    plan.parent_cwd = Some(parent_cwd.to_path_buf());
    Ok(plan)
}

pub(crate) fn campaign_as_apply_plan(
    paths: &DeadreckonPaths,
    campaign_dir: &Path,
    campaign: &deadreckon_core::campaign::Campaign,
    parent_cwd: &Path,
) -> Result<Plan> {
    let mut plan = campaign_as_merge_repair_plan(paths, campaign_dir, campaign, parent_cwd)?;
    plan.status = PlanStatus::Merged;
    plan.merged_at = campaign.merged_at;
    plan.merged_run_id = campaign.merged_run_id.clone();
    Ok(plan)
}

fn parse_campaign_repair_mode(mode: &str) -> Result<MergeRepairMode> {
    match mode {
        "auto" => Ok(MergeRepairMode::Auto),
        "prefer" => Ok(MergeRepairMode::Prefer),
        "synthesize" => Ok(MergeRepairMode::Synthesize),
        "child" => Ok(MergeRepairMode::Child),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown repair mode {other}"),
            "use --repair-mode auto|prefer|synthesize|child",
        ))),
    }
}

fn campaign_rollup_refusal_message(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> String {
    if rollup.refused_subs.is_empty() {
        return "campaign failed: one or more sub-orchestrators did not merge".to_string();
    }
    let mut message = format!(
        "campaign failed: refused sub(s) {}",
        rollup.refused_subs.join(", ")
    );
    for sub_id in &rollup.refused_subs {
        if let Some(summary) = sub_failure_summary(paths, campaign, sub_id) {
            message.push('\n');
            message.push_str(&summary);
        }
    }
    message
}

/// The first failed child's stored failure reason for a refused sub —
/// resolved sub -> plan -> failed task -> child run state, so the campaign
/// surface can say WHY instead of only WHICH.
fn sub_failure_summary(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    sub_id: &str,
) -> Option<String> {
    sub_failure_details(paths, campaign, sub_id).map(|details| {
        format!(
            "{sub_id}: child {} — {}",
            details.task_index, details.reason
        )
    })
}

struct SubFailureDetails {
    task_index: u32,
    child_run_id: String,
    reason: String,
}

fn sub_failure_details(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    sub_id: &str,
) -> Option<SubFailureDetails> {
    let sub = campaign.sub_goals.iter().find(|sub| sub.sub_id == sub_id)?;
    // sub_plan_id is only persisted on merge; a refused sub's plan id lives in
    // the launch sidecar its sub-orchestrator wrote before failing.
    let plan_id = sub.sub_plan_id.clone().or_else(|| {
        let launch_dir = paths
            .home()
            .join("plans")
            .join(&campaign.campaign_id)
            .join("launch")
            .join(sub_id);
        deadreckon_core::campaign::read_sub_result(&launch_dir)
            .ok()
            .flatten()
            .and_then(|result| result.plan_id)
    })?;
    let plan = deadreckon_core::plan::load_plan(paths, &plan_id).ok()?;
    // A failed task is the usual shape, but a kill or cap can leave the task
    // recorded as running — so any non-completed task whose child run stored a
    // failure or pause reason explains the refusal.
    plan.tasks
        .iter()
        .filter(|task| task.status != deadreckon_core::plan::PlanTaskStatus::Completed)
        .find_map(|task| {
            let run_id = task.child_run_id.as_deref()?;
            child_failure_reason(paths, run_id).map(|reason| SubFailureDetails {
                task_index: task.index,
                child_run_id: run_id.to_string(),
                reason,
            })
        })
}

/// The resume commands that recover a refused roll-up: one per interrupted
/// child a refused sub traces to. Empty when nothing resolves (no paths, no
/// sidecars), in which case inspection is the only honest recommendation.
fn refused_sub_resume_commands(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> Vec<String> {
    let mut commands = Vec::new();
    for sub_id in &rollup.refused_subs {
        if let Some(details) = sub_failure_details(paths, campaign, sub_id) {
            let command = format!("deadreckon resume {}", run_prefix(&details.child_run_id));
            if !commands.contains(&command) {
                commands.push(command);
            }
        }
    }
    commands
}

/// Repair composes merged sub results; a refused roll-up means some subs never
/// merged, so repair has nothing to compose and must refuse with the resume
/// path instead of re-stating the roll-up error.
pub(crate) fn campaign_repair_unmerged_refusal(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> Option<VerdictSurface> {
    if rollup.refused_subs.is_empty() {
        return None;
    }
    let resumes = refused_sub_resume_commands(paths, campaign, rollup);
    let primary = resumes.first().cloned().unwrap_or_else(|| {
        format!(
            "deadreckon show {} --why-failed",
            run_prefix(&campaign.campaign_id)
        )
    });
    Some(campaign_repair_refusal_surface(
        campaign,
        VerdictKind::Blocked,
        format!(
            "DeadReckon cannot repair this campaign because {} never merged.",
            rollup.refused_subs.join(", ")
        ),
        "Repair composes merged sub results, and an unmerged sub has nothing to compose. Resume the interrupted children so their plans can complete, then re-run the campaign.",
        primary,
    ))
}

struct CampaignRepairExecution<'a> {
    paths: &'a DeadreckonPaths,
    campaign_dir: &'a Path,
    campaign_obj: &'a mut deadreckon_core::campaign::Campaign,
    rollup: &'a deadreckon_core::campaign::CampaignRollup,
    parent_cwd: &'a Path,
    repair_provider: Option<&'a str>,
    repair_mode: MergeRepairMode,
    repair_attempts: u32,
    quiet: bool,
}

async fn repair_and_promote_campaign_result(
    request: CampaignRepairExecution<'_>,
) -> Result<deadreckon_core::PipelineState> {
    use deadreckon_core::campaign;

    let CampaignRepairExecution {
        paths,
        campaign_dir,
        campaign_obj,
        rollup,
        parent_cwd,
        repair_provider,
        repair_mode,
        repair_attempts,
        quiet,
    } = request;

    if !campaign::campaign_can_complete(campaign_obj, rollup) {
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::write_campaign(campaign_dir, campaign_obj)?;
        campaign::append_campaign_event(
            campaign_dir,
            "rollup_refused",
            serde_json::json!({ "refused_subs": rollup.refused_subs }),
        )?;
        let id = run_prefix(&campaign_obj.campaign_id);
        return Err(CliError::Core(deadreckon_core::user_error(
            &campaign_rollup_refusal_message(paths, campaign_obj, rollup),
            &format!("deadreckon show {id} --why-failed"),
        )));
    }

    let campaign_id = campaign_obj.campaign_id.clone();
    let merge_plan = campaign_as_merge_repair_plan(paths, campaign_dir, campaign_obj, parent_cwd)?;
    let mut merge = compose_plan_merge_working(paths, &merge_plan, PlanMergeStrategy::DagAware)?;
    let unresolved_conflicts = merge.unresolved_conflicts();
    if !unresolved_conflicts.is_empty() {
        let repair_context = MergeRepairContext::final_merge(paths, &merge_plan);
        let provider = resolve_merge_repair_provider(paths, &merge_plan, repair_provider)?;
        campaign::append_campaign_event(
            campaign_dir,
            "campaign_merge_conflict",
            serde_json::json!({
                "conflicts": unresolved_conflicts.len(),
                "provider": provider,
            }),
        )?;
        write_merge_repair_request(
            paths,
            &merge_plan,
            &repair_context,
            provider.as_deref(),
            &unresolved_conflicts,
        )?;
        campaign::append_campaign_event(
            campaign_dir,
            "campaign_repair_planned",
            serde_json::json!({
                "conflicts": unresolved_conflicts.len(),
                "provider": provider,
            }),
        )?;
        let Some(provider) = provider else {
            campaign_obj.status = campaign::CampaignStatus::Failed;
            campaign::write_campaign(campaign_dir, campaign_obj)?;
            campaign::append_campaign_event(
                campaign_dir,
                "campaign_repair_failed",
                serde_json::json!({ "reason": "campaign merge repair needs a configured provider" }),
            )?;
            return Err(CliError::Core(deadreckon_core::user_error(
                "campaign failed: cross-sub file conflict; repair needs a configured provider",
                "deadreckon providers list --all",
            )));
        };
        campaign::append_campaign_event(
            campaign_dir,
            "campaign_repair_started",
            serde_json::json!({ "mode": repair_mode.as_str(), "provider": provider }),
        )?;
        match run_merge_repair(
            paths,
            &merge_plan,
            &repair_context,
            &MergeRepairOptions {
                provider: &provider,
                mode: repair_mode,
                attempts: repair_attempts,
                quiet,
            },
            &mut merge,
        )
        .await
        {
            Ok(repaired) => {
                campaign::append_campaign_event(
                    campaign_dir,
                    "campaign_repaired",
                    serde_json::json!({
                        "strategy": repaired.strategy,
                        "repair_run_id": repaired.repair_run_id,
                    }),
                )?;
                write_plan_merge_conflicts(
                    paths,
                    &merge_plan,
                    PlanMergeStrategy::DagAware,
                    &merge.conflicts,
                )?;
            }
            Err(error) => {
                let reason = error.to_string();
                campaign_obj.status = campaign::CampaignStatus::Failed;
                campaign::write_campaign(campaign_dir, campaign_obj)?;
                campaign::append_campaign_event(
                    campaign_dir,
                    "campaign_repair_failed",
                    serde_json::json!({ "reason": reason }),
                )?;
                return Err(error);
            }
        }
    }

    let merge_dir = paths.merge_working(&campaign_id);
    let result_state = promote_campaign_result(paths, &campaign_id, &merge_dir, rollup)?;
    campaign_obj.merged_run_id = Some(result_state.run_id.clone());
    campaign_obj.merged_at = Some(chrono::Utc::now());
    campaign_obj.status = campaign::CampaignStatus::Merged;
    campaign::write_campaign(campaign_dir, campaign_obj)?;
    if commands::graph_job::current_parent_job_id().is_none() {
        write_campaign_manifest(paths, campaign_obj, &result_state, rollup)?;
    }
    campaign::append_campaign_event(
        campaign_dir,
        "campaign_completed",
        serde_json::json!({ "merged_run_id": result_state.run_id }),
    )?;
    Ok(result_state)
}

fn print_campaign_completion(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: &deadreckon_core::campaign::CampaignRollup,
    result_state: &deadreckon_core::PipelineState,
    no_hints: bool,
) {
    let mut campaign_for_surface = campaign.clone();
    if campaign_for_surface.merged_run_id.is_none() {
        campaign_for_surface.merged_run_id = Some(result_state.run_id.clone());
    }
    print!(
        "{}",
        campaign_verdict_surface(Some(paths), &campaign_for_surface, Some(rollup))
            .render_plain(!completion_hints_enabled(no_hints))
    );
}

pub(crate) fn campaign_verdict_surface(
    paths: Option<&DeadreckonPaths>,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: Option<&deadreckon_core::campaign::CampaignRollup>,
) -> VerdictSurface {
    use deadreckon_core::campaign::{CampaignStatus, RollupVerdict, SubGoalStatus};

    let id = run_prefix(&campaign.campaign_id);
    let merged_subs = campaign
        .sub_goals
        .iter()
        .filter(|sub| sub.status == SubGoalStatus::Merged)
        .count();
    // Repair only composes merged sub results, so it is the recommendation
    // for the caveat shape alone; a refused roll-up means unmerged subs and
    // repair is guaranteed to refuse there.
    let repairable = campaign.status == CampaignStatus::Failed
        && rollup.is_some_and(|rollup| rollup.rollup_verdict == RollupVerdict::Caveat);
    let resume_commands = match (paths, rollup) {
        (Some(paths), Some(rollup)) if campaign.status == CampaignStatus::Failed => {
            refused_sub_resume_commands(paths, campaign, rollup)
        }
        _ => Vec::new(),
    };
    let (kind, what, why) = match campaign.status {
        CampaignStatus::Merged => (
            VerdictKind::Completed,
            "The campaign assembled its sub-orchestrator results into one result run.",
            "DeadReckon has a campaign artifact; the recommended command lands or inspects that result.",
        ),
        CampaignStatus::Failed if !resume_commands.is_empty() => (
            VerdictKind::Blocked,
            "The campaign stopped because interrupted sub-orchestrator children never completed.",
            "Each refused sub traces to a child run that was interrupted; resuming those children lets their plans finish so the campaign can roll up.",
        ),
        CampaignStatus::Failed if repairable => (
            VerdictKind::Blocked,
            "The campaign stopped after merged sub results produced a caveat roll-up.",
            "This is a deterministic campaign-level refusal, not a provider crash; repair can inspect the sub-results and produce a consolidated artifact.",
        ),
        CampaignStatus::Failed => (
            VerdictKind::Failed,
            "The campaign stopped before producing a merged result.",
            "No resumable child or repairable roll-up evidence resolved, so failure inspection is the safest next command.",
        ),
        CampaignStatus::Killed => (
            VerdictKind::Killed,
            "The campaign was stopped before all sub-orchestrators could finish.",
            "Killed campaign state should be inspected before cleanup or relaunch.",
        ),
        CampaignStatus::Forked => (
            VerdictKind::Paused,
            "The campaign has launched sub-orchestrators and is not merged yet.",
            "Attach is the safest next command because sub-plan state may still be active.",
        ),
        CampaignStatus::Pending => (
            VerdictKind::Preview,
            "The campaign record exists, but no sub-orchestrator has started yet.",
            "Attaching or launching the stored campaign state is the next non-destructive step.",
        ),
    };
    let primary = resume_commands
        .first()
        .cloned()
        .unwrap_or_else(|| campaign_primary_action(campaign, repairable));
    let mut evidence = vec![
        ("campaign".to_string(), id.clone()),
        (
            "status".to_string(),
            campaign_status_text(campaign.status).to_string(),
        ),
        (
            "subs".to_string(),
            format!("{merged_subs}/{} merged", campaign.n),
        ),
    ];
    if let Some(result_run_id) = campaign.merged_run_id.as_deref() {
        evidence.push(("result run".to_string(), run_prefix(result_run_id)));
    }
    if let Some(rollup) = rollup {
        evidence.push((
            "roll-up".to_string(),
            rollup_verdict_text(rollup.rollup_verdict).to_string(),
        ));
        if !rollup.refused_subs.is_empty() {
            evidence.push(("refused subs".to_string(), rollup.refused_subs.join(", ")));
            for sub_id in &rollup.refused_subs {
                if let Some(summary) =
                    paths.and_then(|paths| sub_failure_summary(paths, campaign, sub_id))
                {
                    let reason = summary
                        .split_once(": ")
                        .map_or(summary.as_str(), |(_, rest)| rest);
                    evidence.push((sub_id.clone(), reason.to_string()));
                    if let Some(quota) = provider_quota_note(reason) {
                        evidence.push(("resumable".to_string(), quota));
                    }
                }
            }
        }
        if !rollup.caveat_subs.is_empty() {
            evidence.push(("caveat subs".to_string(), rollup.caveat_subs.join(", ")));
        }
    }
    let mut secondary: Vec<String> = resume_commands
        .iter()
        .filter(|command| **command != primary)
        .cloned()
        .collect();
    for command in campaign_secondary_actions(campaign, &primary) {
        if !secondary.contains(&command) {
            secondary.push(command);
        }
    }
    VerdictSurface::must_new(
        kind,
        "campaign",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str())),
    )
}

fn campaign_primary_action(
    campaign: &deadreckon_core::campaign::Campaign,
    repairable: bool,
) -> String {
    let id = run_prefix(&campaign.campaign_id);
    if repairable {
        return format!("deadreckon campaign repair {id}");
    }
    if let Some(result_run_id) = campaign.merged_run_id.as_deref() {
        return format!("deadreckon apply {}", run_prefix(result_run_id));
    }
    match campaign.status {
        deadreckon_core::campaign::CampaignStatus::Failed => {
            format!("deadreckon show {id} --why-failed")
        }
        deadreckon_core::campaign::CampaignStatus::Killed => {
            format!("deadreckon show {id} --why-failed")
        }
        deadreckon_core::campaign::CampaignStatus::Forked
        | deadreckon_core::campaign::CampaignStatus::Pending => format!("deadreckon attach {id}"),
        deadreckon_core::campaign::CampaignStatus::Merged => format!("deadreckon show {id}"),
    }
}

fn campaign_secondary_actions(
    campaign: &deadreckon_core::campaign::Campaign,
    primary: &str,
) -> Vec<String> {
    let id = run_prefix(&campaign.campaign_id);
    let mut actions = Vec::new();
    for command in [
        format!("deadreckon attach {id}"),
        format!("deadreckon show {id} --why-failed"),
        campaign
            .merged_run_id
            .as_deref()
            .map(|run_id| format!("deadreckon show {}", run_prefix(run_id)))
            .unwrap_or_else(|| format!("deadreckon show {id}")),
    ] {
        if command != primary && !actions.contains(&command) {
            actions.push(command);
        }
    }
    actions
}

pub(crate) async fn campaign_command(args: CampaignArgs) -> Result<()> {
    use deadreckon_core::campaign;

    let goal = args.goal.trim().to_string();
    if goal.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign goal must be non-empty",
            "deadreckon campaign \"your goal\"",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let planner_sandbox = args
        .sandbox
        .as_deref()
        .or(defaults.sandbox.as_deref())
        .unwrap_or("auto")
        .parse::<deadreckon_sandbox::SandboxBackend>()?;
    let cwd = std::env::current_dir()?;
    let scope = workspace_scope(&cwd)?;
    let mut n = if let Some(n) = args.n {
        n
    } else {
        let provider = args
            .planner_provider
            .as_deref()
            .or(args.provider.as_deref())
            .map(ToString::to_string)
            .or_else(|| goal_shape_provider_route(&paths, &defaults, None));
        let recommendation = classify_goal_shape_for_start(
            &paths,
            &cwd,
            &goal,
            provider.as_deref(),
            planner_sandbox,
            args.plain,
        )
        .await;
        write_goal_shape_preview_record(&paths, &scope, &recommendation)?;
        recommendation
            .n
            .unwrap_or_else(|| recommend_child_count_for_goal(&goal, CliPlanMode::FullPlan))
    };
    deadreckon_core::plan::validate_task_count(usize::from(n)).map_err(CliError::Core)?;

    let lineage = campaign::lineage_from_env();
    if lineage.depth + 1 >= campaign::CAMPAIGN_MAX_DEPTH {
        return Err(CliError::Surface {
            code: 1,
            surface: campaign_depth_refusal_surface(
                &goal,
                n,
                lineage.depth,
                args.planner_provider.as_deref(),
                args.provider.as_deref(),
            )
            .render_plain(!completion_hints_enabled(args.no_hints)),
        });
    }

    let providers = resolve_plan_providers(
        &paths,
        &defaults,
        PlanMode::FullPlan,
        args.planner_provider.clone(),
        args.provider.clone(),
        None,
        None,
        commands::plan::PlanModelOverrides {
            planner_model: args.planner_model.clone(),
            model: args.model.clone(),
            coder_model: None,
            reviewer_model: None,
            child_models: BTreeMap::new(),
        },
    )?;
    let overrides = BTreeMap::new();
    let planned_tasks = build_full_plan_tasks_accounted(
        &paths,
        &goal,
        n,
        &providers,
        &overrides,
        &cwd,
        planner_sandbox,
        args.plain,
        args.no_hints,
        false,
    )
    .await?;
    let tasks = planned_tasks.tasks;
    let mut planner_accounting = planned_tasks.planner_accounting;
    let sub_goal_strings: Vec<String> = tasks.iter().map(|task| task.goal.clone()).collect();
    let sub_goals =
        campaign::build_sub_goals(sub_goal_strings, usize::from(n)).map_err(CliError::Core)?;
    let sub_keys: Vec<String> = sub_goals.iter().map(|sub| sub.task_key.clone()).collect();
    campaign::guard(
        lineage.depth,
        &lineage.ancestor_task_keys,
        &lineage.ancestor_scopes,
        &sub_keys,
        &[],
    )?;

    let remaining = campaign_remaining_after_planning(
        args.max_spend,
        args.max_wall_seconds,
        planner_accounting.as_ref(),
    )?;
    let mut campaign_obj = campaign::Campaign::new(
        goal.clone(),
        sub_goals,
        providers.clone(),
        lineage.depth,
        remaining.spend_usd,
        remaining.wall_seconds,
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(CliError::Core)?;
    if let Some(job_id) = commands::graph_job::current_parent_job_id() {
        campaign_obj.campaign_id = job_id.to_string();
    }
    let mut root_planner_accounting =
        commands::graph_job::root_planner_accounting(planner_accounting.as_ref());
    campaign_obj.root_planner_accounting = Some(root_planner_accounting.clone());
    let campaign_id = campaign_obj.campaign_id.clone();
    let campaign_dir = paths.plan_dir(&campaign_id);
    fs::create_dir_all(&campaign_dir)?;
    campaign::write_campaign(&campaign_dir, &campaign_obj)?;
    if commands::graph_job::current_driver_owns_root_artifact() {
        campaign_test_failpoint("after_root_campaign_saved_before_driver_state");
    }
    commands::graph_job::record_owned_campaign(&paths, &campaign_obj)?;
    append_campaign_planner_accounting_snapshot(&campaign_dir, &root_planner_accounting)?;
    commands::graph_job::record_current_artifact(
        &paths,
        commands::graph_job::DriverKind::Campaign,
        "campaign",
        &campaign_id,
    )?;
    if let Some(reason) = remaining.exhausted_reason {
        let stop_reason = remaining
            .exhausted_stop_reason
            .unwrap_or(StopReason::SpendCap);
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::append_campaign_event(
            &campaign_dir,
            "budget_exhausted",
            json!({
                "reason": reason,
                "phase": "root_planner",
                "child_launches": 0,
                "stop_reason": stop_reason,
            }),
        )?;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        return Err(CliError::Core(deadreckon_core::user_error(
            &reason,
            "raise --max-spend/--max-wall-seconds or use a deterministic pre-approved decomposition",
        )));
    }
    let mut lineage_scopes = lineage.ancestor_scopes.clone();
    lineage_scopes.push(scope.clone());
    campaign::write_lineage(
        &campaign_dir,
        &campaign::Lineage {
            schema_version: 1,
            depth: lineage.depth,
            campaign_root_id: Some(campaign_id.clone()),
            ancestor_task_keys: lineage.ancestor_task_keys.clone(),
            ancestor_scopes: lineage_scopes,
        },
    )?;
    campaign::append_campaign_event(
        &campaign_dir,
        "campaign_created",
        serde_json::json!({ "n": campaign_obj.n, "root_goal": campaign_obj.root_goal }),
    )?;

    if args.preview {
        if !args.quiet {
            print_campaign_preflight(&campaign_obj, args.sandbox.as_deref());
            print_campaign_contract_preview(&campaign_contract_preview(&args, &cwd)?);
        }
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        return Ok(());
    }
    if !args.quiet {
        print_campaign_preflight(&campaign_obj, args.sandbox.as_deref());
    }
    loop {
        match prompt_campaign_preflight_actions(
            &mut campaign_obj,
            args.sandbox.as_deref(),
            args.yes,
            args.quiet,
        )? {
            CampaignPreflightAction::Launch => break,
            CampaignPreflightAction::NeedsConfirmation => {
                confirm_campaign_start(&campaign_obj, args.yes)?;
                break;
            }
            CampaignPreflightAction::ChangeCount(new_n) => {
                n = new_n;
                let planned = build_full_plan_tasks_accounted(
                    &paths,
                    &goal,
                    n,
                    &providers,
                    &overrides,
                    &cwd,
                    planner_sandbox,
                    args.plain,
                    args.no_hints,
                    false,
                )
                .await?;
                merge_campaign_planner_accounting(
                    &mut planner_accounting,
                    planned.planner_accounting,
                );
                let remaining = campaign_remaining_after_planning(
                    args.max_spend,
                    args.max_wall_seconds,
                    planner_accounting.as_ref(),
                )?;
                campaign_obj.tree_budget_usd = remaining.spend_usd;
                campaign_obj.tree_wall_seconds = remaining.wall_seconds;
                root_planner_accounting =
                    commands::graph_job::root_planner_accounting(planner_accounting.as_ref());
                campaign_obj.root_planner_accounting = Some(root_planner_accounting.clone());
                campaign::write_campaign(&campaign_dir, &campaign_obj)?;
                append_campaign_planner_accounting_snapshot(
                    &campaign_dir,
                    &root_planner_accounting,
                )?;
                if let Some(reason) = remaining.exhausted_reason {
                    let stop_reason = remaining
                        .exhausted_stop_reason
                        .unwrap_or(StopReason::SpendCap);
                    campaign_obj.status = campaign::CampaignStatus::Failed;
                    campaign::append_campaign_event(
                        &campaign_dir,
                        "budget_exhausted",
                        json!({
                            "reason": reason,
                            "phase": "root_planner_replan",
                            "child_launches": 0,
                            "stop_reason": stop_reason,
                        }),
                    )?;
                    campaign::write_campaign(&campaign_dir, &campaign_obj)?;
                    return Err(CliError::Core(deadreckon_core::user_error(
                        &reason,
                        "raise --max-spend/--max-wall-seconds or keep the current decomposition",
                    )));
                }
                let tasks = planned.tasks;
                let sub_goal_strings: Vec<String> =
                    tasks.iter().map(|task| task.goal.clone()).collect();
                let sub_goals = campaign::build_sub_goals(sub_goal_strings, usize::from(n))
                    .map_err(CliError::Core)?;
                let sub_keys: Vec<String> =
                    sub_goals.iter().map(|sub| sub.task_key.clone()).collect();
                campaign::guard(
                    lineage.depth,
                    &lineage.ancestor_task_keys,
                    &lineage.ancestor_scopes,
                    &sub_keys,
                    &[],
                )?;
                campaign_replace_sub_goals_before_launch(&mut campaign_obj, sub_goals)?;
                campaign::append_campaign_event(
                    &campaign_dir,
                    "campaign_count_changed",
                    serde_json::json!({ "n": campaign_obj.n }),
                )?;
                campaign::write_campaign(&campaign_dir, &campaign_obj)?;
                if !args.quiet {
                    print_campaign_preflight(&campaign_obj, args.sandbox.as_deref());
                }
            }
        }
    }
    campaign::write_campaign(&campaign_dir, &campaign_obj)?;
    commands::graph_job::record_owned_campaign(&paths, &campaign_obj)?;

    execute_campaign_state(&paths, &cwd, &scope, &lineage, &args, campaign_obj).await
}

struct CampaignContractPreview {
    source: String,
    name: String,
    checks: Vec<String>,
}

fn campaign_contract_preview(args: &CampaignArgs, cwd: &Path) -> Result<CampaignContractPreview> {
    if let Some(path) = args.acceptance.as_deref() {
        let spec: deadreckon_core::AcceptanceSpec = serde_yaml::from_slice(&fs::read(path)?)
            .map_err(|source| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "could not read campaign acceptance contract {}: {source}",
                    path.display()
                )))
            })?;
        return Ok(CampaignContractPreview {
            source: format!("selected {}", path.display()),
            name: spec
                .name
                .unwrap_or_else(|| "unnamed acceptance contract".to_string()),
            checks: spec
                .checks
                .iter()
                .map(campaign_acceptance_check_label)
                .collect(),
        });
    }
    let kind = deadreckon_core::acceptance_defaults::detect_project_kind(cwd);
    let checks = deadreckon_core::acceptance_defaults::default_checks_for(&kind, cwd);
    Ok(CampaignContractPreview {
        source: format!(
            "detected {}",
            deadreckon_core::acceptance_defaults::kind_label(&kind)
        ),
        name: format!(
            "deadreckon detected {}",
            deadreckon_core::acceptance_defaults::kind_label(&kind)
        ),
        checks: checks.iter().map(campaign_acceptance_check_label).collect(),
    })
}

fn campaign_acceptance_check_label(check: &deadreckon_core::AcceptanceCheck) -> String {
    serde_json::to_value(check)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "acceptance_check".to_string())
}

fn print_campaign_contract_preview(preview: &CampaignContractPreview) {
    println!("  done contract: {} — {}", preview.source, preview.name);
    if preview.checks.is_empty() {
        println!("    checks: none (semantic judge remains required)");
    } else {
        println!("    checks: {}", preview.checks.join(", "));
    }
}

fn campaign_accepted_by(yes: bool) -> deadreckon_protocol::AuthorityAcceptedBy {
    if yes {
        deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
    } else {
        deadreckon_protocol::AuthorityAcceptedBy::Operator
    }
}

pub(crate) fn schedule_campaign_job(args: CampaignArgs) -> Result<()> {
    let goal = args.goal.trim().to_string();
    if goal.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign goal must be non-empty",
            "deadreckon campaign \"your goal\" --yes",
        )));
    }
    if let Some(n) = args.n {
        deadreckon_core::plan::validate_task_count(usize::from(n)).map_err(CliError::Core)?;
    }
    if args.planner_model.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable campaign Jobs do not yet preserve a separate planner model; no Job was created",
            "omit --planner-model or use --model for the shared campaign model",
        )));
    }
    if args.narrate || args.narrator_model.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable campaign Jobs do not yet run the optional legacy narrator; no Job was created",
            "omit --narrate and inspect the authenticated Job receipt with `deadreckon report latest`",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let cwd = std::env::current_dir()?;
    let scope = workspace_scope(&cwd)?;
    let contract_preview = campaign_contract_preview(&args, &cwd)?;
    let sandbox = args
        .sandbox
        .clone()
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    if sandbox == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable campaign Jobs require an isolated sandbox; no Job was created",
            "use --sandbox auto or an available isolated sandbox backend",
        )));
    }
    let max_spend_usd = args.max_spend.or(defaults.max_spend).unwrap_or(10.0);
    let max_wall_seconds = args
        .max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .unwrap_or(36_000.0);
    if !max_spend_usd.is_finite() || max_spend_usd <= 0.0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable campaign max spend must be a positive finite value".to_string(),
        )));
    }
    let max_wall_seconds = commands::job::checked_job_wall_seconds(max_wall_seconds)?;
    if !args.yes {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive durable campaign start requires --yes; no Job was created",
                "deadreckon campaign \"your goal\" --yes",
            )));
        }
        println!(
            "durable campaign\n  goal: {}\n  children: {}\n  spend cap: ${max_spend_usd:.2}\n  wall cap: {max_wall_seconds}s\n  deadline: {}",
            goal,
            args.n
                .map_or_else(|| "planner-selected".to_string(), |n| n.to_string()),
            args.deadline
                .as_ref()
                .map(DateTime::to_rfc3339)
                .unwrap_or_else(|| "none".to_string())
        );
        print_campaign_contract_preview(&contract_preview);
        if !prompt::confirm("create and start this durable campaign Job?", true)? {
            println!("{}", ui_status("cancelled"));
            return Ok(());
        }
    }
    let mut launch = commands::course::trivial_operator_plan(
        &goal,
        commands::course::CourseShape::Campaign,
        "direct_campaign_job",
    );
    launch.n = args.n;
    launch.budget.ceiling_usd = Some(max_spend_usd);
    launch.budget.wall_seconds = Some(max_wall_seconds);
    launch.budget.deadline = args.deadline;
    let mut signals = launch.signals.as_object().cloned().unwrap_or_default();
    signals.insert(
        "watchkeeper_campaign_budget".to_string(),
        json!({
            "approved_total_spend_usd": max_spend_usd,
            "approved_total_wall_seconds": max_wall_seconds,
            "root_planner_policy": "measure the token-bounded root planner completion, persist it before child launch, and subtract its exact spend/wall from the campaign tree budget; refuse child launch if either cap is exhausted",
        }),
    );
    signals.insert(
        "watchkeeper_contract_approval".to_string(),
        json!({
            "source": contract_preview.source,
            "name": contract_preview.name,
            "checks": contract_preview.checks,
            "approval": if args.yes { "yes_flag_guardrail" } else { "interactive_operator" },
        }),
    );
    launch.signals = serde_json::Value::Object(signals);
    launch.accepted_by = Some(if args.yes {
        "yes-flag-guardrail".to_string()
    } else {
        "operator".to_string()
    });
    let accepted_by = campaign_accepted_by(args.yes);
    let job = commands::job::create_job(commands::job::CreateJob {
        paths: &paths,
        source_cwd: &cwd,
        scope,
        launch_plan: launch,
        shape: deadreckon_protocol::JobShape::LegacyCampaign,
        driver: Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::Campaign,
            child_count: args.n,
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: args.planner_provider,
            child_provider: args.provider,
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: args.planner_model,
            child_model: args.model,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        }),
        contract_source: args.acceptance.as_deref(),
        source: commands::job::DurableSource {
            mode: commands::job::DurableSourceMode::Worktree,
            from: Some(cwd.clone()),
            allow_dirty: false,
        },
        max_spend_usd,
        max_wall_seconds,
        max_attempts: 3,
        deadline: args.deadline,
        sandbox_requested: sandbox,
        accepted_by,
    })?;
    commands::job::launch_detached_supervisor(&paths, &job.job_id)?;
    if args.quiet {
        println!("{}", job.job_id);
        Ok(())
    } else {
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref())?;
        commands::job::print_job_status(&view, false)
    }
}

pub(crate) async fn resume_campaign_job(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    args: &CampaignArgs,
) -> Result<()> {
    use deadreckon_core::campaign::{self, CampaignStatus};

    let campaign_dir = paths.plan_dir(job.job_id.as_ref());
    let mut campaign_obj = campaign::read_campaign(&campaign_dir)?;
    if campaign_obj.campaign_id != job.job_id.as_ref()
        || campaign_obj.root_goal != job.goal
        || campaign_obj.status == CampaignStatus::Killed
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "campaign job {} does not have resumable persisted campaign state",
            job.job_id
        ))));
    }
    if campaign_obj.status == CampaignStatus::Pending {
        let accounting = campaign_obj
            .root_planner_accounting
            .as_ref()
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign job {} has no root planner accounting",
                    job.job_id
                )))
            })?;
        if let Some(exhaustion) = commands::graph_job::root_planner_budget_exhaustion(
            accounting,
            job.policy.max_spend_usd,
            job.policy.max_wall_seconds as f64,
        )? {
            if !campaign::read_campaign_events(&campaign_dir)?
                .iter()
                .any(|event| {
                    event.kind == "budget_exhausted"
                        && event.detail.get("phase").and_then(Value::as_str) == Some("root_planner")
                })
            {
                campaign::append_campaign_event(
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
            campaign_obj.status = CampaignStatus::Failed;
            campaign::write_campaign(&campaign_dir, &campaign_obj)?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &exhaustion.reason,
                "raise the approved Job budget or use a deterministic pre-approved decomposition",
            )));
        }
    }
    if campaign_obj.status == CampaignStatus::Failed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "campaign job {} has a failed campaign and cannot be resumed automatically",
            job.job_id
        ))));
    }
    commands::graph_job::validate_owned_campaign(paths, &campaign_obj, job.job_id.as_ref())?;
    if campaign_obj.status == CampaignStatus::Merged {
        return Ok(());
    }
    commands::graph_job::record_current_artifact(
        paths,
        commands::graph_job::DriverKind::Campaign,
        "campaign",
        job.job_id.as_ref(),
    )?;
    let lineage = campaign::read_lineage(&campaign_dir)?;
    let scope = workspace_scope(&job.source_cwd)?;
    execute_campaign_state(paths, &job.source_cwd, &scope, &lineage, args, campaign_obj).await
}

async fn execute_campaign_state(
    paths: &DeadreckonPaths,
    cwd: &Path,
    scope: &str,
    lineage: &deadreckon_core::campaign::Lineage,
    args: &CampaignArgs,
    mut campaign_obj: deadreckon_core::campaign::Campaign,
) -> Result<()> {
    use deadreckon_core::campaign;

    let campaign_id = campaign_obj.campaign_id.clone();
    let campaign_dir = paths.plan_dir(&campaign_id);
    let pre_child_exhaustion = if campaign_obj
        .tree_budget_usd
        .is_some_and(|remaining| remaining <= 0.0)
    {
        Some((
            StopReason::SpendCap,
            "campaign root planner left no approved spend for child work".to_string(),
        ))
    } else if campaign_obj
        .tree_wall_seconds
        .is_some_and(|remaining| remaining <= 0.0)
    {
        Some((
            StopReason::WallCap,
            "campaign root planner left no approved wall time for child work".to_string(),
        ))
    } else {
        None
    };
    if let Some((stop_reason, reason)) = pre_child_exhaustion {
        if !campaign::read_campaign_events(&campaign_dir)?
            .iter()
            .any(|event| {
                event.kind == "budget_exhausted"
                    && event.detail.get("phase").and_then(Value::as_str) == Some("root_planner")
            })
        {
            campaign::append_campaign_event(
                &campaign_dir,
                "budget_exhausted",
                json!({
                    "reason": reason,
                    "phase": "root_planner",
                    "child_launches": 0,
                    "stop_reason": stop_reason,
                }),
            )?;
        }
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        return Err(CliError::Core(deadreckon_core::user_error(
            &reason,
            "raise --max-spend/--max-wall-seconds or use a deterministic pre-approved decomposition",
        )));
    }
    let providers = campaign_obj.providers.clone();
    let sandbox = args.sandbox.clone().unwrap_or_else(|| "auto".to_string());
    let per_sub = campaign_obj
        .tree_budget_usd
        .map(|budget| campaign::allocate_budget(budget, campaign_obj.sub_goals.len()));
    let per_sub_wall = campaign_obj
        .tree_wall_seconds
        .map(|budget| campaign::allocate_budget(budget, campaign_obj.sub_goals.len()));
    let home = paths.home().to_path_buf();
    let planner = providers.planner.clone();
    let child_provider = providers.default_child.clone();
    let plain = args.plain;
    let final_sub_keys: Vec<String> = campaign_obj
        .sub_goals
        .iter()
        .map(|sub| sub.task_key.clone())
        .collect();
    let mut ancestor_task_keys = lineage.ancestor_task_keys.clone();
    ancestor_task_keys.extend(final_sub_keys.iter().cloned());
    let mut ancestor_scopes = lineage.ancestor_scopes.clone();
    ancestor_scopes.push(scope.to_string());
    let sub_positions = campaign_obj
        .sub_goals
        .iter()
        .enumerate()
        .map(|(index, sub)| (sub.sub_id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let campaign_narrate = args.narrate && !args.no_narrate;
    let campaign_narrator_model = args.narrator_model.clone();
    let campaign_quiet = args.quiet;
    let n_subs = campaign_obj.sub_goals.len();
    let campaign_prefix = run_prefix(&campaign_id);

    run_campaign_fork_with_recovery(
        &campaign_dir,
        &mut campaign_obj,
        |sub, launch_dir| {
            let position = sub_positions.get(&sub.sub_id).copied().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign sub {} is absent from its approved schedule",
                    sub.sub_id
                )))
            })?;
            if discover_sub_result(launch_dir)?.is_some() {
                // A finalized result is authoritative recovery evidence even
                // if the launch PID has since been reused or can no longer be
                // verified on this boot.
                return recover_persisted_campaign_sub(
                    paths,
                    cwd,
                    launch_dir,
                    sub,
                    &sandbox,
                    per_sub
                        .as_ref()
                        .and_then(|shares| shares.get(position).copied()),
                    per_sub_wall
                        .as_ref()
                        .and_then(|shares| shares.get(position).copied()),
                );
            }
            if let Some(plan_id) = sub.sub_plan_id.as_deref()
                && commands::graph_job::current_parent_job_id().is_some()
                && commands::graph_job::campaign_sub_launch_process_is_live(
                    paths,
                    &campaign_id,
                    &sub.sub_id,
                    plan_id,
                )?
            {
                // A prior driver may have left the exact guarded
                // sub-orchestrator alive. Let the launch closure adopt it
                // instead of racing it through Plan-level recovery.
                return Ok(None);
            }
            recover_persisted_campaign_sub(
                paths,
                cwd,
                launch_dir,
                sub,
                &sandbox,
                per_sub
                    .as_ref()
                    .and_then(|shares| shares.get(position).copied()),
                per_sub_wall
                    .as_ref()
                    .and_then(|shares| shares.get(position).copied()),
            )
        },
        |sub, launch_dir| {
            let sub_index = sub_positions.get(&sub.sub_id).copied().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign sub {} is absent from its approved schedule",
                    sub.sub_id
                )))
            })?;
            let share = per_sub
                .as_ref()
                .and_then(|shares| shares.get(sub_index).copied());
            let wall_share = per_sub_wall
                .as_ref()
                .and_then(|shares| shares.get(sub_index).copied());
            let sub_plan_id = sub.sub_plan_id.as_deref().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign sub {} has no reserved Plan identity",
                    sub.sub_id
                )))
            })?;
            let position = sub_index + 1;
            let aggregate_on = campaign_narrate && !campaign_quiet;
            let mut command = build_sub_orchestrator_command(&CampaignSubLaunch {
                home: &home,
                source_dir: cwd,
                launch_dir,
                campaign_id: &campaign_id,
                sub_plan_id,
                sub_goal: &sub.goal,
                sub_n: 2,
                sandbox: &sandbox,
                max_spend: share,
                max_wall_seconds: wall_share,
                plain,
                planner_provider: planner.as_deref(),
                child_provider: child_provider.as_deref(),
                planner_model: providers.planner_model.as_deref(),
                child_model: providers.default_child_model.as_deref(),
                narrate: campaign_narrate,
                narrator_model: campaign_narrator_model.as_deref(),
                ancestor_task_keys: &ancestor_task_keys,
                ancestor_scopes: &ancestor_scopes,
            })?;
            if aggregate_on {
                let goal_snippet = sub.goal.chars().take(60).collect::<String>();
                eprintln!(
                    "campaign {campaign_prefix} · {} ({position}/{n_subs}) started — {goal_snippet}",
                    sub.sub_id
                );
            }
            let mut child = if commands::graph_job::current_parent_job_id().is_some() {
                match commands::graph_job::recover_campaign_sub_launch(
                    paths,
                    &campaign_id,
                    &sub.sub_id,
                    sub_plan_id,
                )? {
                    commands::graph_job::CampaignSubLaunchRecovery::Adopted(child) => {
                        CampaignSubChildProcess::Durable(child)
                    }
                    commands::graph_job::CampaignSubLaunchRecovery::RecoverLinkedArtifacts => {
                        if let Some(result) = recover_persisted_campaign_sub(
                            paths,
                            cwd,
                            launch_dir,
                            sub,
                            &sandbox,
                            share,
                            per_sub_wall
                                .as_ref()
                                .and_then(|shares| shares.get(sub_index).copied()),
                        )? {
                            return Ok(result);
                        }
                        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                            "Campaign sub-process {} crossed its durable execution boundary but left no recoverable reserved Plan {sub_plan_id}; refusing a duplicate launch",
                            sub.sub_id
                        ))));
                    }
                    commands::graph_job::CampaignSubLaunchRecovery::Relaunch => {
                        let argv = command
                            .get_args()
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>();
                        let prepared = commands::graph_job::prepare_delegated_invocation(
                            paths,
                            commands::graph_job::DelegatedAction::CampaignSub {
                                campaign_id: campaign_id.clone(),
                                sub_id: sub.sub_id.clone(),
                                plan_id: sub_plan_id.to_string(),
                            },
                            &argv,
                            cwd,
                            launch_dir,
                            None,
                        )?;
                        CampaignSubChildProcess::Durable(Box::new(
                            commands::graph_job::spawn_campaign_sub_delegated(
                                paths, command, prepared,
                            )?,
                        ))
                    }
                }
            } else {
                CampaignSubChildProcess::Plain(command.spawn()?)
            };
            let mut last_tick = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(2))
                .unwrap_or_else(std::time::Instant::now);
            let success = loop {
                if let Some(success) = child.try_wait()? {
                    break success;
                }
                if aggregate_on && last_tick.elapsed() >= std::time::Duration::from_secs(2) {
                    let headline = read_sub_plan_id(launch_dir)
                        .and_then(|plan_id| deadreckon_core::plan::load_plan(paths, &plan_id).ok())
                        .map(|plan| {
                            plan.tasks
                                .iter()
                                .filter_map(|task| task.child_run_id.clone())
                                .collect::<Vec<_>>()
                        })
                        .and_then(|run_ids| narrative::freshest_child_headline(paths, &run_ids));
                    let line = narrative::campaign_aggregate_line(
                        &campaign_prefix,
                        &sub.sub_id,
                        position,
                        n_subs,
                        "running",
                        headline.as_deref(),
                        100,
                    );
                    let mut out = std::io::sink();
                    let mut err = std::io::stderr();
                    let mut sinks = narrative::AggregateSinks {
                        out: &mut out,
                        err: &mut err,
                    };
                    let _ = narrative::emit_campaign_aggregate(&mut sinks, &line);
                    last_tick = std::time::Instant::now();
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            };
            child.revoke_pending(paths)?;
            if success == Some(false) {
                if aggregate_on {
                    eprintln!(
                        "campaign {campaign_prefix} · {} ({position}/{n_subs}) failed",
                        sub.sub_id
                    );
                }
                if let Some(result) = recover_persisted_campaign_sub(
                    paths,
                    cwd,
                    launch_dir,
                    sub,
                    &sandbox,
                    share,
                    per_sub_wall
                        .as_ref()
                        .and_then(|shares| shares.get(sub_index).copied()),
                )? {
                    return Ok(result);
                }
                if !paths.plan_json(sub_plan_id).is_file() {
                    return Err(CliError::RetryableInterruption {
                        message: format!(
                            "campaign sub-orchestrator {} was interrupted before reserved Plan {sub_plan_id} was created",
                            sub.sub_id
                        ),
                    });
                }
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "sub-orchestrator {} exited unsuccessfully",
                    sub.sub_id
                ))));
            }
            let result = match discover_sub_result(launch_dir)? {
                Some(result) => result,
                None if success.is_none() => recover_persisted_campaign_sub(
                    paths,
                    cwd,
                    launch_dir,
                    sub,
                    &sandbox,
                    share,
                    per_sub_wall
                        .as_ref()
                        .and_then(|shares| shares.get(sub_index).copied()),
                )?
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "adopted sub-orchestrator {} exited without a recoverable result",
                        sub.sub_id
                    )))
                })?,
                None => {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "sub-orchestrator {} produced no result",
                        sub.sub_id
                    ))));
                }
            };
            if aggregate_on {
                let run = result
                    .result_run_id
                    .as_deref()
                    .map(run_prefix)
                    .unwrap_or_else(|| "-".to_string());
                eprintln!(
                    "campaign {campaign_prefix} · {} ({position}/{n_subs}) merged → run {run}",
                    sub.sub_id
                );
            }
            Ok(result)
        },
        |result| {
            result
                .result_run_id
                .as_deref()
                .and_then(|run_id| load_run(paths, run_id).ok())
                .map(|state| state.total_spend_usd)
                .unwrap_or(0.0)
        },
    )?;

    let final_tree_spend_usd = campaign_obj
        .sub_goals
        .iter()
        .filter_map(|sub| sub.result_run_id.as_deref())
        .filter_map(|run_id| load_run(paths, run_id).ok())
        .map(|state| state.total_spend_usd)
        .sum::<f64>();
    if campaign::tree_budget_exhausted(campaign_obj.tree_budget_usd, final_tree_spend_usd) {
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::append_campaign_event(
            &campaign_dir,
            "budget_exhausted",
            json!({
                "spent_usd": final_tree_spend_usd,
                "tree_budget_usd": campaign_obj.tree_budget_usd,
                "phase": "post_children_pre_merge",
                "stop_reason": StopReason::SpendCap,
            }),
        )?;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign children exhausted the remaining approved spend before parent verification",
            "inspect `deadreckon status` and restart with a larger approved cap",
        )));
    }
    let final_tree_wall_seconds = campaign_obj
        .sub_goals
        .iter()
        .filter_map(|sub| sub.result_run_id.as_deref())
        .filter_map(|run_id| load_run(paths, run_id).ok())
        .map(|state| state.total_wall_seconds)
        .sum::<f64>();
    if campaign_obj
        .tree_wall_seconds
        .is_some_and(|cap| final_tree_wall_seconds >= cap)
    {
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::append_campaign_event(
            &campaign_dir,
            "budget_exhausted",
            json!({
                "wall_seconds": final_tree_wall_seconds,
                "tree_wall_seconds": campaign_obj.tree_wall_seconds,
                "phase": "post_children_pre_merge",
                "stop_reason": StopReason::WallCap,
            }),
        )?;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign children exhausted the remaining approved wall time before parent verification",
            "inspect `deadreckon status` and restart with a larger approved cap",
        )));
    }

    let rollup = campaign::build_rollup(&campaign_obj, |run_id| match load_run(paths, run_id) {
        Ok(state) => {
            let tamper =
                deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)
                    .ok()
                    .flatten();
            let verdict = tamper
                .as_ref()
                .map(|tamper| tamper.verdict)
                .unwrap_or(deadreckon_core::tamper::AcceptanceTamperVerdict::Clean);
            let caveats = tamper.map(|tamper| tamper.caveats).unwrap_or_default();
            let gate = if deadreckon_core::gate::validate_acceptance_marker(&state).is_ok() {
                "signed".to_string()
            } else {
                "refused".to_string()
            };
            (gate, verdict, caveats)
        }
        Err(_) => (
            "missing".to_string(),
            deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse,
            Vec::new(),
        ),
    });
    campaign::write_campaign_rollup(&campaign_dir, &rollup)?;

    let result_state = repair_and_promote_campaign_result(CampaignRepairExecution {
        paths,
        campaign_dir: &campaign_dir,
        campaign_obj: &mut campaign_obj,
        rollup: &rollup,
        parent_cwd: cwd,
        repair_provider: None,
        repair_mode: MergeRepairMode::Auto,
        repair_attempts: 1,
        quiet: args.quiet,
    })
    .await?;
    if !args.quiet {
        print_campaign_completion(paths, &campaign_obj, &rollup, &result_state, args.no_hints);
    }
    Ok(())
}

fn campaign_depth_refusal_surface(
    goal: &str,
    n: u8,
    current_depth: u32,
    planner_provider: Option<&str>,
    child_provider: Option<&str>,
) -> VerdictSurface {
    let mut primary = format!(
        "deadreckon orchestrate full-plan \"{}\" --n {}",
        shell_display_quote(goal),
        n
    );
    if let Some(provider) = planner_provider {
        primary.push_str(&format!(" --planner-provider {provider}"));
    }
    if let Some(provider) = child_provider {
        primary.push_str(&format!(" --provider {provider}"));
    }

    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "campaign",
        None,
        ExplanationPanel::new(
            "DeadReckon refused to start a nested campaign because depth cap 2 reached.",
            "This is a controlled recursion guard; use an orchestrated full-plan inside the sub-orchestrator instead of starting another campaign coordinator.",
            [
                ("goal", goal.to_string()),
                ("current depth", current_depth.to_string()),
                (
                    "depth cap",
                    deadreckon_core::campaign::CAMPAIGN_MAX_DEPTH.to_string(),
                ),
                ("requested subs", n.to_string()),
                (
                    "reason",
                    "sub-orchestrators cannot campaign again".to_string(),
                ),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
}

fn campaign_repair_refusal_surface(
    campaign: &deadreckon_core::campaign::Campaign,
    kind: VerdictKind,
    what: impl Into<String>,
    why: impl Into<String>,
    primary: String,
) -> VerdictSurface {
    let id = run_prefix(&campaign.campaign_id);
    let mut evidence = vec![
        ("campaign".to_string(), id.clone()),
        (
            "status".to_string(),
            campaign_status_text(campaign.status).to_string(),
        ),
        ("subs".to_string(), campaign.n.to_string()),
    ];
    if let Some(result_run_id) = campaign.merged_run_id.as_deref() {
        evidence.push(("result run".to_string(), run_prefix(result_run_id)));
    }
    let mut secondary = Vec::new();
    for command in [
        format!("deadreckon show {id}"),
        format!("deadreckon attach {id}"),
        campaign
            .merged_run_id
            .as_deref()
            .map(|run_id| format!("deadreckon show {}", run_prefix(run_id)))
            .unwrap_or_else(|| format!("deadreckon show {id} --why-failed")),
    ] {
        if command != primary && !secondary.contains(&command) {
            secondary.push(command);
        }
    }
    VerdictSurface::must_new(
        kind,
        "campaign",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary)],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str())),
    )
}

pub(crate) async fn campaign_repair_command(args: CampaignRepairArgs) -> Result<()> {
    use deadreckon_core::campaign;

    let reference = args.campaign_id.trim();
    if reference.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign id required",
            "deadreckon campaign repair <campaign-id>",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let Some((campaign_dir, mut campaign_obj)) = resolve_campaign(&paths, reference)? else {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown campaign {reference}"),
            "deadreckon list --all",
        )));
    };
    let job_owned = commands::graph_job::require_current_driver_for_job_artifact(
        &paths,
        &campaign_obj.campaign_id,
        deadreckon_protocol::JobShape::LegacyCampaign,
        "campaign repair",
    )?;
    if !job_owned && !commands::plan::internal_characterization_requested() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "campaign repair cannot mutate legacy Campaign {} because it has no durable Job owner",
                run_prefix(&campaign_obj.campaign_id)
            ),
            "start a new durable Job with `deadreckon start`",
        )));
    }
    match campaign_obj.status {
        campaign::CampaignStatus::Failed => {}
        campaign::CampaignStatus::Merged => {
            let primary = campaign_obj
                .merged_run_id
                .as_deref()
                .map(|run_id| format!("deadreckon apply {}", run_prefix(run_id)))
                .unwrap_or_else(|| "deadreckon show <campaign-id>".to_string());
            return Err(CliError::Surface {
                code: 1,
                surface: campaign_repair_refusal_surface(
                    &campaign_obj,
                    VerdictKind::Noop,
                    "DeadReckon did not run campaign repair because this campaign is already merged.",
                    "Repair is only needed for failed campaign roll-ups; the merged result is ready for the normal apply or inspection path.",
                    primary,
                )
                .render_plain(!completion_hints_enabled(args.no_hints)),
            });
        }
        status => {
            return Err(CliError::Surface {
                code: 1,
                surface: campaign_repair_refusal_surface(
                    &campaign_obj,
                    VerdictKind::Blocked,
                    format!(
                        "DeadReckon did not run campaign repair because the campaign is {}.",
                        campaign_status_text(status)
                    ),
                    "Campaign repair needs a failed campaign with a completed roll-up before it can inspect sub-results.",
                    format!("deadreckon attach {}", run_prefix(&campaign_obj.campaign_id)),
                )
                .render_plain(!completion_hints_enabled(args.no_hints)),
            });
        }
    }

    let rollup = campaign::read_campaign_rollup(&campaign_dir).map_err(|_| {
        CliError::Surface {
            code: 1,
            surface: campaign_repair_refusal_surface(
                &campaign_obj,
                VerdictKind::Blocked,
                "DeadReckon did not run campaign repair because no completed roll-up was found.",
                "Repair needs the sub-orchestrator roll-up evidence before it can decide whether to synthesize or promote a result.",
                format!("deadreckon attach {}", run_prefix(&campaign_obj.campaign_id)),
            )
            .render_plain(!completion_hints_enabled(args.no_hints)),
        }
    })?;
    if let Some(surface) = campaign_repair_unmerged_refusal(&paths, &campaign_obj, &rollup) {
        return Err(CliError::Surface {
            code: 1,
            surface: surface.render_plain(!completion_hints_enabled(args.no_hints)),
        });
    }
    let repair_mode = parse_campaign_repair_mode(&args.repair_mode)?;
    let cwd = std::env::current_dir()?;
    let result_state = repair_and_promote_campaign_result(CampaignRepairExecution {
        paths: &paths,
        campaign_dir: &campaign_dir,
        campaign_obj: &mut campaign_obj,
        rollup: &rollup,
        parent_cwd: &cwd,
        repair_provider: args.repair_provider.as_deref(),
        repair_mode,
        repair_attempts: args.repair_attempts,
        quiet: args.quiet,
    })
    .await?;

    if !args.quiet {
        print_campaign_completion(&paths, &campaign_obj, &rollup, &result_state, args.no_hints);
    }
    Ok(())
}

pub(crate) fn campaign_status_text(
    status: deadreckon_core::campaign::CampaignStatus,
) -> &'static str {
    use deadreckon_core::campaign::CampaignStatus;
    match status {
        CampaignStatus::Pending => "pending",
        CampaignStatus::Forked => "forked",
        CampaignStatus::Merged => "merged",
        CampaignStatus::Failed => "failed",
        CampaignStatus::Killed => "killed",
    }
}

fn sub_status_text(status: deadreckon_core::campaign::SubGoalStatus) -> &'static str {
    use deadreckon_core::campaign::SubGoalStatus;
    match status {
        SubGoalStatus::Pending => "pending",
        SubGoalStatus::Running => "running",
        SubGoalStatus::Merged => "merged",
        SubGoalStatus::Failed => "failed",
        SubGoalStatus::Killed => "killed",
    }
}

pub(crate) fn rollup_verdict_text(
    verdict: deadreckon_core::campaign::RollupVerdict,
) -> &'static str {
    use deadreckon_core::campaign::RollupVerdict;
    match verdict {
        RollupVerdict::Clean => "clean",
        RollupVerdict::Caveat => "caveat",
        RollupVerdict::Refused => "refused",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CampaignFeedEvent {
    Campaign {
        event: deadreckon_core::campaign::CampaignEvent,
    },
    SubPlan {
        sub_id: String,
        event: PlanEvent,
    },
    Snapshot {
        campaign: Box<deadreckon_core::campaign::Campaign>,
    },
    Warning {
        message: String,
    },
}

pub(crate) struct CampaignEventFeed {
    paths: DeadreckonPaths,
    campaign_dir: PathBuf,
    campaign_id: String,
    campaign_tail: JsonlTail<deadreckon_core::campaign::CampaignEvent>,
    sub_tails: BTreeMap<String, (String, JsonlTail<PlanEvent>)>,
    seen: BTreeSet<String>,
    last_snapshot_key: Option<String>,
}

impl CampaignEventFeed {
    pub(crate) fn new(
        paths: DeadreckonPaths,
        campaign_dir: impl Into<PathBuf>,
        campaign_id: impl Into<String>,
    ) -> Self {
        let campaign_dir = campaign_dir.into();
        Self {
            campaign_tail: JsonlTail::new(deadreckon_core::campaign::campaign_events_path(
                &campaign_dir,
            )),
            paths,
            campaign_dir,
            campaign_id: campaign_id.into(),
            sub_tails: BTreeMap::new(),
            seen: BTreeSet::new(),
            last_snapshot_key: None,
        }
    }

    pub(crate) async fn refresh(&mut self, wait: Duration) -> Vec<CampaignFeedEvent> {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let mut events = Vec::new();
        match self.campaign_tail.read_new() {
            Ok(campaign_events) => {
                events.extend(
                    campaign_events
                        .into_iter()
                        .map(|event| CampaignFeedEvent::Campaign { event }),
                );
            }
            Err(error) => events.push(CampaignFeedEvent::Warning {
                message: format!("campaign event replay failed: {error}"),
            }),
        }
        match deadreckon_core::campaign::read_campaign(&self.campaign_dir) {
            Ok(campaign) => {
                self.discover_sub_plans(&campaign);
                self.maybe_push_snapshot(campaign, &mut events);
            }
            Err(error) => events.push(CampaignFeedEvent::Warning {
                message: format!("campaign snapshot failed: {error}"),
            }),
        }
        self.read_sub_plan_events(&mut events);
        self.dedupe(events)
    }

    #[cfg(test)]
    pub(crate) fn sub_plan_ids(&self) -> Vec<String> {
        self.sub_tails
            .values()
            .map(|(plan_id, _)| plan_id.clone())
            .collect()
    }

    fn discover_sub_plans(&mut self, campaign: &deadreckon_core::campaign::Campaign) {
        for sub in &campaign.sub_goals {
            let Some(plan_id) = sub.sub_plan_id.as_deref() else {
                continue;
            };
            if self.sub_tails.contains_key(&sub.sub_id) {
                continue;
            }
            self.sub_tails.insert(
                sub.sub_id.clone(),
                (
                    plan_id.to_string(),
                    JsonlTail::new(self.paths.plan_events(plan_id)),
                ),
            );
        }
    }

    fn read_sub_plan_events(&mut self, events: &mut Vec<CampaignFeedEvent>) {
        for (sub_id, (_plan_id, tail)) in &mut self.sub_tails {
            match tail.read_new() {
                Ok(plan_events) => {
                    events.extend(
                        plan_events
                            .into_iter()
                            .map(|event| CampaignFeedEvent::SubPlan {
                                sub_id: sub_id.clone(),
                                event,
                            }),
                    )
                }
                Err(error) => events.push(CampaignFeedEvent::Warning {
                    message: format!("sub-plan {sub_id} event replay failed: {error}"),
                }),
            }
        }
    }

    fn maybe_push_snapshot(
        &mut self,
        campaign: deadreckon_core::campaign::Campaign,
        events: &mut Vec<CampaignFeedEvent>,
    ) {
        if campaign.campaign_id != self.campaign_id {
            events.push(CampaignFeedEvent::Warning {
                message: format!(
                    "campaign id changed on disk: expected {}, found {}",
                    self.campaign_id, campaign.campaign_id
                ),
            });
        }
        let key = campaign_snapshot_key(&campaign);
        if self.last_snapshot_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.last_snapshot_key = Some(key);
        events.push(CampaignFeedEvent::Snapshot {
            campaign: Box::new(campaign),
        });
    }

    fn dedupe(&mut self, events: Vec<CampaignFeedEvent>) -> Vec<CampaignFeedEvent> {
        events
            .into_iter()
            .filter(|event| {
                let key = format!("{event:?}");
                self.seen.insert(key)
            })
            .collect()
    }
}

fn campaign_snapshot_key(campaign: &deadreckon_core::campaign::Campaign) -> String {
    let subs = campaign
        .sub_goals
        .iter()
        .map(|sub| {
            format!(
                "{}:{:?}:{:?}:{:?}",
                sub.sub_id, sub.status, sub.sub_plan_id, sub.result_run_id
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "{}:{:?}:{:?}:{}",
        campaign.campaign_id, campaign.status, campaign.merged_run_id, subs
    )
}

#[derive(Debug, Clone)]
pub(crate) struct CampaignAttachState {
    pub(crate) paths: DeadreckonPaths,
    pub(crate) campaign_dir: PathBuf,
    pub(crate) campaign: deadreckon_core::campaign::Campaign,
    pub(crate) rollup: Option<deadreckon_core::campaign::CampaignRollup>,
    pub(crate) aggregate_spend_usd: f64,
    pub(crate) sub_spend_usd: BTreeMap<String, f64>,
    pub(crate) feed: VecDeque<CampaignFeedEvent>,
    pub(crate) selected: usize,
    pub(crate) selected_node: Option<crate::tui::tree::NodeId>,
    pub(crate) zoomed_node: Option<crate::tui::tree::NodeId>,
}

impl CampaignAttachState {
    pub(crate) fn new(
        paths: &DeadreckonPaths,
        campaign_dir: impl Into<PathBuf>,
        campaign: deadreckon_core::campaign::Campaign,
    ) -> Self {
        let campaign_dir = campaign_dir.into();
        let rollup = deadreckon_core::campaign::read_campaign_rollup(&campaign_dir).ok();
        let (aggregate_spend_usd, sub_spend_usd) = campaign_spend(paths, &campaign);
        Self {
            paths: paths.clone(),
            campaign_dir,
            campaign,
            rollup,
            aggregate_spend_usd,
            sub_spend_usd,
            feed: VecDeque::new(),
            selected: 0,
            selected_node: None,
            zoomed_node: None,
        }
    }

    pub(crate) fn refresh(&mut self, paths: &DeadreckonPaths) -> Result<()> {
        self.campaign = deadreckon_core::campaign::read_campaign(&self.campaign_dir)?;
        self.rollup = deadreckon_core::campaign::read_campaign_rollup(&self.campaign_dir).ok();
        let (aggregate_spend_usd, sub_spend_usd) = campaign_spend(paths, &self.campaign);
        self.aggregate_spend_usd = aggregate_spend_usd;
        self.sub_spend_usd = sub_spend_usd;
        self.clamp_selection();
        Ok(())
    }

    pub(crate) fn apply_feed_events(&mut self, events: Vec<CampaignFeedEvent>) {
        for event in events {
            if let CampaignFeedEvent::Snapshot { campaign } = &event {
                self.campaign = (**campaign).clone();
            }
            self.feed.push_back(event);
        }
        while self.feed.len() > 1_000 {
            self.feed.pop_front();
        }
        self.clamp_selection();
    }

    pub(crate) fn clamp_selection(&mut self) {
        if self.campaign.sub_goals.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.campaign.sub_goals.len() - 1);
        }
    }

    pub(crate) fn selected_sub_plan(&self) -> Option<(String, String)> {
        self.campaign.sub_goals.get(self.selected).and_then(|sub| {
            sub.sub_plan_id
                .as_ref()
                .map(|plan_id| (sub.sub_id.clone(), plan_id.clone()))
        })
    }
}

#[cfg(test)]
pub(crate) fn campaign_attach_state_from_dir(
    paths: &DeadreckonPaths,
    campaign_dir: &Path,
) -> Result<CampaignAttachState> {
    let campaign = deadreckon_core::campaign::read_campaign(campaign_dir)?;
    Ok(CampaignAttachState::new(paths, campaign_dir, campaign))
}

fn campaign_spend(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> (f64, BTreeMap<String, f64>) {
    let mut total = 0.0_f64;
    let mut by_sub = BTreeMap::new();
    for sub in &campaign.sub_goals {
        let spend = sub
            .result_run_id
            .as_deref()
            .and_then(|run_id| load_run(paths, run_id).ok())
            .map(|state| state.total_spend_usd)
            .unwrap_or(0.0);
        total += spend;
        by_sub.insert(sub.sub_id.clone(), spend);
    }
    (total, by_sub)
}

pub(crate) fn campaign_attach_json_text(
    paths: Option<&DeadreckonPaths>,
    state: &CampaignAttachState,
) -> Result<String> {
    let rollup = state.rollup.as_ref();
    let surface = campaign_verdict_surface(paths, &state.campaign, rollup);
    let value = surface.add_to_json(serde_json::json!({
        "kind": "campaign",
        "id": &state.campaign.campaign_id,
        "status": campaign_status_text(state.campaign.status),
        "goal": &state.campaign.root_goal,
        "tree_budget_usd": state.campaign.tree_budget_usd,
        "aggregate_spend_usd": state.aggregate_spend_usd,
        "next_actions": [surface.primary_action.command.clone()],
        "rollup": rollup.map(|rollup| serde_json::json!({
            "verdict": rollup_verdict_text(rollup.rollup_verdict),
            "refused_subs": &rollup.refused_subs,
            "caveat_subs": &rollup.caveat_subs,
        })),
        "subs": state.campaign.sub_goals.iter().map(|sub| serde_json::json!({
            "sub_id": &sub.sub_id,
            "goal": &sub.goal,
            "status": sub_status_text(sub.status),
            "sub_plan_id": &sub.sub_plan_id,
            "result_run_id": &sub.result_run_id,
            "spend_usd": state.sub_spend_usd.get(&sub.sub_id).copied().unwrap_or(0.0),
        })).collect::<Vec<_>>()
    }));
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

/// Read-only attach summary for a campaign: breadcrumb, sub rows, and roll-up.
/// This is the plain summary that works off TTY or with `--plain`; the live TUI
/// uses the same campaign state but omits the retype hint.
pub(crate) fn campaign_attach_summary(
    paths: Option<&DeadreckonPaths>,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: Option<&deadreckon_core::campaign::CampaignRollup>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(
        out,
        "{}",
        campaign_verdict_surface(paths, campaign, rollup).render_plain(false)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", ui_heading("Details"));
    let _ = writeln!(
        out,
        "{} {} ({})",
        ui_muted("campaign:"),
        campaign.root_goal,
        ui_status(campaign_status_text(campaign.status))
    );
    if let Some(rollup) = rollup {
        let _ = writeln!(
            out,
            "roll-up {}",
            ui_status(rollup_verdict_text(rollup.rollup_verdict))
        );
    }
    let _ = writeln!(out, "{}", ui_heading("Status spine"));
    for line in crate::tui::spine::spine_plain_lines(
        &crate::tui::spine::spine_for_campaign_with_events(campaign, &[], Utc::now()),
    ) {
        let _ = writeln!(out, "{line}");
    }
    for sub in &campaign.sub_goals {
        let result = sub
            .result_run_id
            .as_deref()
            .map(run_prefix)
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            out,
            "  {} {} result={} {}",
            ui_id(&sub.sub_id),
            ui_status(sub_status_text(sub.status)),
            ui_id(&result),
            sub.goal
        );
        if let Some(paths) = paths
            && let Some(run_id) = sub.result_run_id.as_deref()
            && let Ok(state) = load_run(paths, run_id)
        {
            let _ = writeln!(out, "    spend {}", run_spend_label(&state, false));
            let _ = writeln!(
                out,
                "    {} {}",
                ui_muted("gate:"),
                ui_status(acceptance_status_value(&state))
            );
        }
    }
    let _ = writeln!(
        out,
        "(each sub is its own plan; deadreckon attach <sub-plan-id> drills in)"
    );
    out
}

/// `show <campaign-id> --why-failed` report: surfaces refused and caveat subs.
pub(crate) fn campaign_why_failed_report(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: Option<&deadreckon_core::campaign::CampaignRollup>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(
        out,
        "{}",
        campaign_verdict_surface(Some(paths), campaign, rollup).render_plain(false)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", ui_heading("Details"));
    let _ = writeln!(
        out,
        "campaign {} {}",
        ui_id(run_prefix(&campaign.campaign_id)),
        ui_status(campaign_status_text(campaign.status))
    );
    if let Some(rollup) = rollup {
        let _ = writeln!(
            out,
            "roll-up {}",
            ui_status(rollup_verdict_text(rollup.rollup_verdict))
        );
        if !rollup.refused_subs.is_empty() {
            let _ = writeln!(
                out,
                "{} {}",
                ui_muted("refused subs:"),
                ui_id(rollup.refused_subs.join(", "))
            );
            for sub_id in &rollup.refused_subs {
                if let Some(summary) = sub_failure_summary(paths, campaign, sub_id) {
                    let _ = writeln!(out, "  {summary}");
                }
            }
        }
        if !rollup.caveat_subs.is_empty() {
            let _ = writeln!(
                out,
                "{} {}",
                ui_muted("caveat subs:"),
                ui_id(rollup.caveat_subs.join(", "))
            );
        }
    }
    for sub in &campaign.sub_goals {
        if sub.status == deadreckon_core::campaign::SubGoalStatus::Merged {
            continue;
        }
        let _ = writeln!(
            out,
            "  {} {} {}",
            ui_id(&sub.sub_id),
            ui_status(sub_status_text(sub.status)),
            sub.goal
        );
    }
    out
}

/// The sub-plan ids a `kill <campaign-id>` must cascade into (each runs its own
/// coordinator and children). Killing each reuses the existing plan-kill path.
pub(crate) fn campaign_kill_targets(campaign: &deadreckon_core::campaign::Campaign) -> Vec<String> {
    campaign
        .sub_goals
        .iter()
        .filter_map(|sub| sub.sub_plan_id.clone())
        .collect()
}

/// Resolve a campaign by id prefix: a plan dir whose `campaign.json` exists and
/// whose id starts with `reference`. Returns the campaign dir and the campaign.
pub(crate) fn resolve_campaign(
    paths: &DeadreckonPaths,
    reference: &str,
) -> Result<Option<(PathBuf, deadreckon_core::campaign::Campaign)>> {
    let plans = paths.home().join("plans");
    if !plans.is_dir() {
        return Ok(None);
    }
    let mut campaigns = Vec::new();
    for entry in std::fs::read_dir(&plans).map_err(|source| DeadreckonError::Io {
        path: plans.clone(),
        source,
    })? {
        let dir = entry
            .map_err(|source| DeadreckonError::Io {
                path: plans.clone(),
                source,
            })?
            .path();
        if !deadreckon_core::campaign::campaign_path_for_plan_dir(&dir).is_file() {
            continue;
        }
        let campaign = deadreckon_core::campaign::read_campaign(&dir)?;
        campaigns.push((dir, campaign));
    }
    if matches!(reference, "latest" | "last") {
        campaigns.sort_by_key(|(_, campaign)| campaign.created_at);
        return Ok(campaigns.pop());
    }
    let matches = campaigns
        .into_iter()
        .filter(|(_, campaign)| campaign.campaign_id.starts_with(reference))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous campaign id prefix {reference}; matches {}",
                matches
                    .iter()
                    .map(|(_, campaign)| campaign.campaign_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "use a longer campaign id prefix",
        ))),
    }
}

/// Drive a campaign's sub-orchestrators sequentially, recording events and status
/// transitions. Sequential (not concurrent like plan `fork`) is deliberate: the
/// tree-budget ceiling (P5) must be checked *before* launching the next sub, which
/// a concurrent batch cannot guarantee. The `launch` closure performs the actual
/// spawn; tests inject a fake. A failed sub never aborts its siblings.
#[allow(dead_code)] // reserved campaign-fork engine; command wiring and tests share this private seam
pub(crate) fn run_campaign_fork<F, S>(
    campaign_dir: &Path,
    campaign: &mut deadreckon_core::campaign::Campaign,
    launch: F,
    spend_of: S,
) -> Result<()>
where
    F: FnMut(
        &deadreckon_core::campaign::SubGoal,
        &Path,
    ) -> Result<deadreckon_core::campaign::SubResult>,
    S: Fn(&deadreckon_core::campaign::SubResult) -> f64,
{
    run_campaign_fork_with_recovery(
        campaign_dir,
        campaign,
        |_sub, _launch_dir| Ok(None),
        launch,
        spend_of,
    )
}

pub(crate) fn run_campaign_fork_with_recovery<R, F, S>(
    campaign_dir: &Path,
    campaign: &mut deadreckon_core::campaign::Campaign,
    mut recover: R,
    mut launch: F,
    spend_of: S,
) -> Result<()>
where
    R: FnMut(
        &deadreckon_core::campaign::SubGoal,
        &Path,
    ) -> Result<Option<deadreckon_core::campaign::SubResult>>,
    F: FnMut(
        &deadreckon_core::campaign::SubGoal,
        &Path,
    ) -> Result<deadreckon_core::campaign::SubResult>,
    S: Fn(&deadreckon_core::campaign::SubResult) -> f64,
{
    use deadreckon_core::campaign::{
        self, CampaignStatus, SubGoalStatus, append_campaign_event, tree_budget_exhausted,
        write_campaign,
    };

    campaign.status = CampaignStatus::Forked;
    campaign.forked_at = Some(chrono::Utc::now());
    let tree_budget = campaign.tree_budget_usd;
    write_campaign(campaign_dir, campaign)?;
    append_campaign_event(
        campaign_dir,
        "campaign_started",
        serde_json::json!({ "n": campaign.n }),
    )?;

    let mut spent_usd = 0.0_f64;
    for index in 0..campaign.sub_goals.len() {
        let persisted_sub = campaign.sub_goals[index].clone();
        if persisted_sub.status == SubGoalStatus::Merged {
            spent_usd += spend_of(&deadreckon_core::campaign::SubResult {
                schema_version: 1,
                sub_id: persisted_sub.sub_id,
                plan_id: persisted_sub.sub_plan_id,
                result_run_id: persisted_sub.result_run_id,
                ok: true,
            });
            continue;
        }
        if tree_budget_exhausted(tree_budget, spent_usd) {
            append_campaign_event(
                campaign_dir,
                "budget_exhausted",
                serde_json::json!({
                    "spent_usd": spent_usd,
                    "tree_budget_usd": tree_budget,
                    "stop_reason": StopReason::SpendCap,
                }),
            )?;
            break;
        }
        let launch_dir = campaign_dir
            .join("launch")
            .join(&campaign.sub_goals[index].sub_id);
        fs::create_dir_all(&launch_dir)?;
        if campaign.sub_goals[index].sub_plan_id.is_none() {
            let plan_id = read_sub_plan_id(&launch_dir)
                .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
            campaign.sub_goals[index].sub_plan_id = Some(plan_id.clone());
            campaign.sub_goals[index].status = SubGoalStatus::Running;
            campaign::write_campaign(campaign_dir, campaign)?;
            publish_sub_plan_id(&launch_dir, &plan_id)?;
            append_campaign_event(
                campaign_dir,
                "sub_launch_prepared",
                serde_json::json!({
                    "sub_id": campaign.sub_goals[index].sub_id,
                    "plan_id": plan_id,
                }),
            )?;
            campaign_test_failpoint("after_sub_launch_intent_before_spawn");
        } else {
            let plan_id = campaign.sub_goals[index]
                .sub_plan_id
                .clone()
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "Campaign lost its reserved sub Plan identity".to_string(),
                    ))
                })?;
            publish_sub_plan_id(&launch_dir, &plan_id)?;
            campaign.sub_goals[index].status = SubGoalStatus::Running;
            campaign::write_campaign(campaign_dir, campaign)?;
        }
        let sub = campaign.sub_goals[index].clone();
        if let Some(result) = recover(&sub, &launch_dir)? {
            validate_campaign_sub_result(&sub, &result)?;
            spent_usd += spend_of(&result);
            let target = &mut campaign.sub_goals[index];
            target.sub_plan_id = result.plan_id.clone();
            target.result_run_id = result.result_run_id.clone();
            target.status = if result.ok {
                SubGoalStatus::Merged
            } else {
                SubGoalStatus::Failed
            };
            append_campaign_event(
                campaign_dir,
                "sub_recovered",
                serde_json::json!({
                    "sub_id": sub.sub_id,
                    "plan_id": result.plan_id,
                    "result_run_id": result.result_run_id,
                    "ok": result.ok,
                }),
            )?;
            campaign::write_campaign(campaign_dir, campaign)?;
            continue;
        }
        append_campaign_event(
            campaign_dir,
            "sub_launched",
            serde_json::json!({
                "sub_id": sub.sub_id,
                "plan_id": sub.sub_plan_id,
            }),
        )?;
        match launch(&sub, &launch_dir).and_then(|result| {
            validate_campaign_sub_result(&sub, &result)?;
            Ok(result)
        }) {
            Ok(result) => {
                spent_usd += spend_of(&result);
                let target = &mut campaign.sub_goals[index];
                target.sub_plan_id = result.plan_id.clone();
                target.result_run_id = result.result_run_id.clone();
                target.status = if result.ok {
                    SubGoalStatus::Merged
                } else {
                    SubGoalStatus::Failed
                };
                append_campaign_event(
                    campaign_dir,
                    if result.ok {
                        "sub_merged"
                    } else {
                        "sub_failed"
                    },
                    serde_json::json!({
                        "sub_id": sub.sub_id,
                        "plan_id": result.plan_id,
                        "result_run_id": result.result_run_id,
                    }),
                )?;
            }
            Err(err @ CliError::RetryableInterruption { .. }) => return Err(err),
            Err(err) => {
                campaign.sub_goals[index].status = SubGoalStatus::Failed;
                append_campaign_event(
                    campaign_dir,
                    "sub_failed",
                    serde_json::json!({ "sub_id": sub.sub_id, "reason": err.to_string() }),
                )?;
            }
        }
        campaign::write_campaign(campaign_dir, campaign)?;
    }
    Ok(())
}

fn validate_campaign_sub_result(
    sub: &deadreckon_core::campaign::SubGoal,
    result: &deadreckon_core::campaign::SubResult,
) -> Result<()> {
    if result.sub_id != sub.sub_id || result.plan_id != sub.sub_plan_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign sub {} returned a result for a different reserved Plan",
            sub.sub_id
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod sub_plan_marker_tests {
    use super::{publish_sub_plan_id, read_sub_plan_id};
    use tempfile::TempDir;

    #[test]
    fn sub_plan_id_marker_roundtrips_for_mid_run_grandchild_discovery() {
        let temp = TempDir::new().expect("temp");
        assert!(
            read_sub_plan_id(temp.path()).is_none(),
            "no marker before the sub-orchestrator creates its plan"
        );
        publish_sub_plan_id(temp.path(), "plan-abc123").expect("publish");
        assert_eq!(
            read_sub_plan_id(temp.path()).as_deref(),
            Some("plan-abc123"),
            "campaign parent can discover the sub-plan id mid-run"
        );
    }
}

#[cfg(test)]
mod model_argv_tests {
    use super::{CampaignSubLaunch, build_sub_orchestrator_command};
    use std::path::Path;

    #[test]
    fn campaign_planner_model_flows_to_sub_orchestrator_argv() {
        let command = build_sub_orchestrator_command(&CampaignSubLaunch {
            home: Path::new("/tmp/home"),
            source_dir: Path::new("/tmp/src"),
            launch_dir: Path::new("/tmp/launch"),
            campaign_id: "cmp-1",
            sub_plan_id: "00000000000000000000000000000001",
            sub_goal: "sub goal",
            sub_n: 2,
            sandbox: "none",
            max_spend: None,
            max_wall_seconds: None,
            plain: false,
            planner_provider: Some("smoke"),
            child_provider: Some("smoke"),
            planner_model: Some("planner-mx"),
            child_model: Some("child-mx"),
            narrate: false,
            narrator_model: None,
            ancestor_task_keys: &[],
            ancestor_scopes: &[],
        })
        .expect("command");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let pair = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|at| args[at + 1].clone())
        };
        assert_eq!(pair("--planner-model").as_deref(), Some("planner-mx"));
        assert_eq!(pair("--model").as_deref(), Some("child-mx"));
    }

    #[test]
    fn campaign_narrate_propagates_through_sub_orchestrator_to_leaf_run_argv() {
        let command = build_sub_orchestrator_command(&CampaignSubLaunch {
            home: Path::new("/tmp/home"),
            source_dir: Path::new("/tmp/src"),
            launch_dir: Path::new("/tmp/launch"),
            campaign_id: "cmp-1",
            sub_plan_id: "00000000000000000000000000000001",
            sub_goal: "sub goal",
            sub_n: 2,
            sandbox: "none",
            max_spend: None,
            max_wall_seconds: None,
            plain: false,
            planner_provider: Some("smoke"),
            child_provider: Some("smoke"),
            planner_model: None,
            child_model: None,
            narrate: true,
            narrator_model: Some("haiku"),
            ancestor_task_keys: &[],
            ancestor_scopes: &[],
        })
        .expect("command");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(
            args.iter().any(|a| a == "--narrate"),
            "campaign passes --narrate to the sub-orchestrator: {args:?}"
        );
        let model = args
            .iter()
            .position(|a| a == "--narrator-model")
            .map(|at| args[at + 1].clone());
        assert_eq!(model.as_deref(), Some("haiku"));

        // Building the argv is not enough — the receiving `orchestrate full-plan`
        // subcommand must actually ACCEPT these flags. (A real campaign run found
        // the subcommand rejecting `--narrate` though the argv looked correct.)
        // The full CLI command tree is large; parse it on a roomy stack because
        // the default 2 MiB test-harness thread stack overflows building it
        // (the real binary parses on the 8 MiB main thread).
        let mut argv = vec![std::ffi::OsString::from("deadreckon")];
        argv.extend(command.get_args().map(|arg| arg.to_owned()));
        let narrate_parsed = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                use clap::Parser;
                let cli = crate::cli::Cli::try_parse_from(&argv)
                    .expect("sub-orchestrator argv must parse against the real CLI");
                matches!(
                    &cli.command,
                    Some(crate::cli::Commands::Orchestrate {
                        command: Some(crate::cli::OrchestrateCommand::FullPlan(parsed)),
                        ..
                    }) if parsed.narrate && parsed.narrator_model.as_deref() == Some("haiku")
                )
            })
            .expect("spawn parse thread")
            .join()
            .expect("parse thread");
        assert!(
            narrate_parsed,
            "orchestrate full-plan must parse --narrate/--narrator-model from the campaign argv"
        );
    }
}

#[cfg(test)]
mod durable_campaign_launch_tests {
    use std::fs;

    use super::{
        CampaignArgs, PlannerAccounting, campaign_accepted_by, campaign_contract_preview,
        campaign_remaining_after_planning, schedule_campaign_job,
    };
    use deadreckon_protocol::StopReason;

    fn args() -> CampaignArgs {
        CampaignArgs {
            goal: "complete the campaign".to_string(),
            n: Some(2),
            planner_provider: None,
            provider: None,
            planner_model: None,
            model: None,
            max_spend: Some(2.0),
            max_wall_seconds: Some(120.0),
            deadline: None,
            sandbox: Some("auto".to_string()),
            acceptance: None,
            preview: false,
            yes: true,
            no_hints: true,
            quiet: true,
            plain: true,
            narrate: false,
            no_narrate: true,
            narrator_model: None,
        }
    }

    #[test]
    fn durable_campaign_refuses_unpreserved_planner_model() {
        let mut request = args();
        request.planner_model = Some("planner-only".to_string());

        let error = schedule_campaign_job(request).expect_err("planner model must be explicit");

        assert!(error.to_string().contains("separate planner model"));
        assert!(error.to_string().contains("no Job was created"));
    }

    #[test]
    fn durable_campaign_refuses_legacy_narration_instead_of_ignoring_it() {
        let mut request = args();
        request.narrate = true;

        let error = schedule_campaign_job(request).expect_err("narration must not be ignored");

        assert!(error.to_string().contains("optional legacy narrator"));
        assert!(error.to_string().contains("authenticated Job receipt"));
    }

    #[test]
    fn durable_campaign_refuses_unsandboxed_trusted_execution() {
        let mut request = args();
        request.sandbox = Some("none".to_string());

        let error = schedule_campaign_job(request).expect_err("sandbox none must fail closed");

        assert!(error.to_string().contains("require an isolated sandbox"));
        assert!(error.to_string().contains("no Job was created"));
    }

    #[test]
    fn root_planner_accounting_is_subtracted_before_child_launch() {
        let accounting = PlannerAccounting {
            spend: deadreckon_providers::SpendEstimate {
                provider: "test".to_string(),
                model: "planner".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 1.25,
                subscription: false,
                wall_time_seconds: Some(5.0),
            },
            wall_seconds: 5.0,
        };

        let remaining =
            campaign_remaining_after_planning(Some(10.0), Some(100.0), Some(&accounting))
                .expect("remaining budget");

        assert_eq!(remaining.spend_usd, Some(8.75));
        assert_eq!(remaining.wall_seconds, Some(95.0));
        assert!(remaining.exhausted_reason.is_none());
    }

    #[test]
    fn root_planner_exhaustion_is_a_typed_pre_child_refusal() {
        let accounting = PlannerAccounting {
            spend: deadreckon_providers::SpendEstimate {
                provider: "test".to_string(),
                model: "planner".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 2.0,
                subscription: false,
                wall_time_seconds: Some(5.0),
            },
            wall_seconds: 5.0,
        };

        let remaining =
            campaign_remaining_after_planning(Some(1.0), Some(100.0), Some(&accounting))
                .expect("bounded accounting");

        assert_eq!(remaining.spend_usd, Some(0.0));
        assert_eq!(remaining.exhausted_stop_reason, Some(StopReason::SpendCap));
        assert!(
            remaining
                .exhausted_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("before child launch"))
        );
    }

    #[test]
    fn root_planner_wall_exhaustion_keeps_the_wall_dimension() {
        let accounting = PlannerAccounting {
            spend: deadreckon_providers::SpendEstimate {
                provider: "test".to_string(),
                model: "planner".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 0.25,
                subscription: false,
                wall_time_seconds: Some(10.0),
            },
            wall_seconds: 10.0,
        };

        let remaining = campaign_remaining_after_planning(Some(1.0), Some(5.0), Some(&accounting))
            .expect("bounded accounting");

        assert_eq!(remaining.wall_seconds, Some(0.0));
        assert_eq!(remaining.exhausted_stop_reason, Some(StopReason::WallCap));
    }

    #[test]
    fn campaign_yes_and_interactive_approval_have_distinct_authority() {
        assert_eq!(
            campaign_accepted_by(true),
            deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
        );
        assert_eq!(
            campaign_accepted_by(false),
            deadreckon_protocol::AuthorityAcceptedBy::Operator
        );
    }

    #[test]
    fn selected_campaign_contract_is_parsed_for_pre_approval_display() {
        let temp = tempfile::tempdir().expect("temp");
        let acceptance = temp.path().join("acceptance.yaml");
        fs::write(
            &acceptance,
            "name: explicit campaign done\nchecks:\n  - kind: file_exists\n    path: README.md\n",
        )
        .expect("acceptance");
        let mut request = args();
        request.acceptance = Some(acceptance.clone());

        let preview = campaign_contract_preview(&request, temp.path()).expect("preview");

        assert_eq!(preview.source, format!("selected {}", acceptance.display()));
        assert_eq!(preview.name, "explicit campaign done");
        assert_eq!(preview.checks, vec!["file_exists"]);
    }
}
