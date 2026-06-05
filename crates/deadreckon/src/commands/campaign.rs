use super::super::*;
use crate::commands::orchestrate::recommend_child_count_for_goal;
use crate::commands::plan::{build_full_plan_tasks, resolve_plan_providers};
use crate::commands::start::{
    classify_goal_shape_for_start, goal_shape_provider_route, write_goal_shape_preview_record,
};
use crate::plan_event_bus::JsonlTail;
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
    pub(crate) sub_goal: &'a str,
    pub(crate) sub_n: u8,
    pub(crate) sandbox: &'a str,
    pub(crate) max_spend: Option<f64>,
    pub(crate) plain: bool,
    pub(crate) planner_provider: Option<&'a str>,
    pub(crate) child_provider: Option<&'a str>,
    pub(crate) ancestor_task_keys: &'a [String],
    pub(crate) ancestor_scopes: &'a [String],
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
        .env(campaign::ENV_SUB_RESULT, launch.launch_dir);
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
    if launch.plain {
        command.arg("--plain");
    }
    if let Some(max_spend) = launch.max_spend {
        command.arg("--max-spend").arg(format!("{max_spend:.6}"));
    }
    if let Some(planner) = launch.planner_provider {
        command.arg("--planner-provider").arg(planner);
    }
    if let Some(provider) = launch.child_provider {
        command.arg("--provider").arg(provider);
    }
    Ok(command)
}

#[allow(dead_code)] // reserved campaign-fork sidecar seam; kept separate for result-discovery tests
pub(crate) fn discover_sub_result(
    launch_dir: &Path,
) -> Result<Option<deadreckon_core::campaign::SubResult>> {
    deadreckon_core::campaign::read_sub_result(launch_dir).map_err(CliError::Core)
}

/// Write a sub-orchestrator's result sidecar. Called at the end of
/// `orchestrate full-plan` when launched by a campaign (DEADRECKON_CAMPAIGN_SUB_RESULT
/// is set): records the plan id and its merged result run for the meta-coordinator.
pub(crate) fn record_sub_orchestrator_result(plan_id: &str, launch_dir: &Path, ok: bool) {
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
    let _ = deadreckon_core::campaign::write_sub_result(launch_dir, &result);
}

pub(crate) struct CampaignArgs {
    pub(crate) goal: String,
    pub(crate) n: Option<u8>,
    pub(crate) planner_provider: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) sandbox: Option<String>,
    pub(crate) preview: bool,
    pub(crate) yes: bool,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
}

pub(crate) struct CampaignRepairArgs {
    pub(crate) campaign_id: String,
    pub(crate) repair_provider: Option<String>,
    pub(crate) repair_mode: String,
    pub(crate) repair_attempts: u32,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
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

    println!("Campaign preview {id}");
    println!();
    println!("Goal");
    print_campaign_wrapped("  ", &campaign.root_goal, CAMPAIGN_WRAP_WIDTH);
    println!();
    println!("Plan");
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
    match (
        campaign.tree_budget_usd,
        per_sub.as_ref().and_then(|s| s.first()),
    ) {
        (Some(total), Some(share)) => {
            print_campaign_fact(
                "budget",
                &format!("${total:.2} total (~${share:.2} per sub)"),
            );
        }
        _ => {
            print_campaign_fact("budget", "unbounded");
            print_campaign_wrapped(
                "             ",
                "Add --max-spend <usd> to cap the whole campaign tree.",
                CAMPAIGN_WRAP_WIDTH,
            );
        }
    }
    if let Some(seconds) = campaign.tree_wall_seconds {
        print_campaign_fact("wall cap", &format_wall_cap(Some(seconds)));
    }

    println!();
    println!("Next");
    println!("  Press Enter or choose [1] to launch. Edit the split first if it looks wrong.");
    println!("  Launch without prompting:");
    print_campaign_wrapped("    ", &primary, CAMPAIGN_WRAP_WIDTH);

    println!("Sub-goals");
    for sub in &campaign.sub_goals {
        print_campaign_sub_goal(sub);
    }
}

const CAMPAIGN_WRAP_WIDTH: usize = 88;

fn print_campaign_fact(label: &str, value: &str) {
    println!("  {label:<9} {value}");
}

fn print_campaign_sub_goal(sub: &deadreckon_core::campaign::SubGoal) {
    println!("  {}", sub.sub_id);
    if let Some((body, acceptance)) = split_acceptance_clause(&sub.goal) {
        print_campaign_wrapped("    ", body, CAMPAIGN_WRAP_WIDTH);
        println!("    Acceptance");
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
    let width = width.max(16);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        push_campaign_word(&mut lines, &mut current, word, width);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push("-".to_string());
    }
    lines
}

fn push_campaign_word(lines: &mut Vec<String>, current: &mut String, word: &str, width: usize) {
    let word_len = word.chars().count();
    if current.is_empty() {
        if word_len <= width {
            current.push_str(word);
        } else {
            push_campaign_word_chunks(lines, current, word, width);
        }
        return;
    }
    let current_len = current.chars().count();
    if current_len + 1 + word_len <= width {
        current.push(' ');
        current.push_str(word);
        return;
    }
    lines.push(std::mem::take(current));
    if word_len <= width {
        current.push_str(word);
    } else {
        push_campaign_word_chunks(lines, current, word, width);
    }
}

fn push_campaign_word_chunks(
    lines: &mut Vec<String>,
    current: &mut String,
    word: &str,
    width: usize,
) {
    for ch in word.chars() {
        current.push(ch);
        if current.chars().count() >= width {
            lines.push(std::mem::take(current));
        }
    }
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
                let answer =
                    prompt::open("sub-orchestrator count: ", Some(&campaign.n.to_string()))?;
                let count = answer.trim().parse::<u8>().map_err(|_| {
                    CliError::Core(deadreckon_core::user_error(
                        &format!("campaign count is not a number: {answer}"),
                        "enter a value from 2 through 6",
                    ))
                })?;
                validate_task_count(usize::from(count)).map_err(CliError::Core)?;
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

/// Create a promoted result run from the composed campaign tree, binding the
/// roll-up into the marker signature. Mirrors `create_merged_plan_run`.
fn promote_campaign_result(
    paths: &DeadreckonPaths,
    campaign_id: &str,
    merge_dir: &Path,
    rollup: &deadreckon_core::campaign::CampaignRollup,
) -> Result<deadreckon_core::PipelineState> {
    let cwd = std::env::current_dir()?;
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!("campaign {}", run_prefix(campaign_id)),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("deadreckon:campaign".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )?;
    remove_if_exists(&state.working_dir)?;
    copy_tree(merge_dir, &state.working_dir)?;
    deadreckon_core::campaign::write_campaign_rollup_at_run_root(&state.run_root, rollup)?;
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )?;
    state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
    save_state(&state)?;
    promote_completed_run(paths, &mut state)?;
    Ok(state)
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

fn campaign_rollup_refusal_message(rollup: &deadreckon_core::campaign::CampaignRollup) -> String {
    if rollup.refused_subs.is_empty() {
        "campaign failed: one or more sub-orchestrators did not merge".to_string()
    } else {
        format!(
            "campaign failed: refused sub(s) {}",
            rollup.refused_subs.join(", ")
        )
    }
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
        return Err(CliError::Core(deadreckon_core::user_error(
            &campaign_rollup_refusal_message(rollup),
            "deadreckon show <campaign-id> --why-failed",
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
    write_campaign_manifest(paths, campaign_obj, &result_state, rollup)?;
    campaign::append_campaign_event(
        campaign_dir,
        "campaign_completed",
        serde_json::json!({ "merged_run_id": result_state.run_id }),
    )?;
    Ok(result_state)
}

fn print_campaign_completion(
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
        campaign_verdict_surface(&campaign_for_surface, Some(rollup)).render_plain(no_hints)
    );
}

pub(crate) fn campaign_verdict_surface(
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
    let repairable = campaign.status == CampaignStatus::Failed
        && rollup.is_some_and(|rollup| rollup.rollup_verdict != RollupVerdict::Clean);
    let (kind, what, why) = match campaign.status {
        CampaignStatus::Merged => (
            VerdictKind::Completed,
            "The campaign assembled its sub-orchestrator results into one result run.",
            "DeadReckon has a campaign artifact; the recommended command lands or inspects that result.",
        ),
        CampaignStatus::Failed if repairable => (
            VerdictKind::Blocked,
            "The campaign stopped after sub-orchestrator work produced a refused roll-up.",
            "This is a deterministic campaign-level refusal, not a provider crash; repair can inspect the sub-results and produce a consolidated artifact.",
        ),
        CampaignStatus::Failed => (
            VerdictKind::Failed,
            "The campaign stopped before producing a merged result.",
            "No repairable roll-up evidence is available, so failure inspection is the safest next command.",
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
    let primary = campaign_primary_action(campaign, repairable);
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
        }
        if !rollup.caveat_subs.is_empty() {
            evidence.push(("caveat subs".to_string(), rollup.caveat_subs.join(", ")));
        }
    }
    let secondary = campaign_secondary_actions(campaign, &primary);
    VerdictSurface::try_new(
        kind,
        "campaign",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("campaign verdict surface must have one primary action")
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
        let recommendation =
            classify_goal_shape_for_start(&paths, &cwd, &goal, provider.as_deref(), args.plain)
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
    )?;
    let overrides = BTreeMap::new();
    let tasks = build_full_plan_tasks(
        &paths,
        &goal,
        n,
        &providers,
        &overrides,
        &cwd,
        args.plain,
        args.no_hints,
        false,
    )
    .await?;
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

    let mut campaign_obj = campaign::Campaign::new(
        goal.clone(),
        sub_goals,
        providers.clone(),
        lineage.depth,
        args.max_spend,
        args.max_wall_seconds,
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(CliError::Core)?;
    let campaign_id = campaign_obj.campaign_id.clone();
    let campaign_dir = paths.plan_dir(&campaign_id);
    fs::create_dir_all(&campaign_dir)?;
    campaign::write_campaign(&campaign_dir, &campaign_obj)?;
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
                let tasks = build_full_plan_tasks(
                    &paths,
                    &goal,
                    n,
                    &providers,
                    &overrides,
                    &cwd,
                    args.plain,
                    args.no_hints,
                    false,
                )
                .await?;
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

    let sandbox = args.sandbox.clone().unwrap_or_else(|| "auto".to_string());
    let per_sub = campaign_obj
        .tree_budget_usd
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
    ancestor_scopes.push(scope.clone());
    let launch_paths = &paths;
    let mut sub_index = 0usize;

    run_campaign_fork(
        &campaign_dir,
        &mut campaign_obj,
        |sub, launch_dir| {
            let share = per_sub
                .as_ref()
                .and_then(|shares| shares.get(sub_index).copied());
            sub_index += 1;
            let mut command = build_sub_orchestrator_command(&CampaignSubLaunch {
                home: &home,
                source_dir: &cwd,
                launch_dir,
                campaign_id: &campaign_id,
                sub_goal: &sub.goal,
                sub_n: 2,
                sandbox: &sandbox,
                max_spend: share,
                plain,
                planner_provider: planner.as_deref(),
                child_provider: child_provider.as_deref(),
                ancestor_task_keys: &ancestor_task_keys,
                ancestor_scopes: &ancestor_scopes,
            })?;
            let status = command.status()?;
            if !status.success() {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "sub-orchestrator {} exited with {status}",
                    sub.sub_id
                ))));
            }
            discover_sub_result(launch_dir)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "sub-orchestrator {} produced no result",
                    sub.sub_id
                )))
            })
        },
        |result| {
            result
                .result_run_id
                .as_deref()
                .and_then(|run_id| load_run(launch_paths, run_id).ok())
                .map(|state| state.total_spend_usd)
                .unwrap_or(0.0)
        },
    )?;

    let rollup = campaign::build_rollup(&campaign_obj, |run_id| match load_run(&paths, run_id) {
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
        paths: &paths,
        campaign_dir: &campaign_dir,
        campaign_obj: &mut campaign_obj,
        rollup: &rollup,
        parent_cwd: &cwd,
        repair_provider: None,
        repair_mode: MergeRepairMode::Auto,
        repair_attempts: 1,
        quiet: args.quiet,
    })
    .await?;
    if !args.quiet {
        print_campaign_completion(&campaign_obj, &rollup, &result_state, args.no_hints);
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

    VerdictSurface::try_new(
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
                ("reason", "sub-orchestrators cannot campaign again".to_string()),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
    .expect("campaign depth refusal must have one primary action")
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
    VerdictSurface::try_new(
        kind,
        "campaign",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("campaign repair refusal surface must have one primary action")
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
        print_campaign_completion(&campaign_obj, &rollup, &result_state, args.no_hints);
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
    pub(crate) campaign_dir: PathBuf,
    pub(crate) campaign: deadreckon_core::campaign::Campaign,
    pub(crate) rollup: Option<deadreckon_core::campaign::CampaignRollup>,
    pub(crate) aggregate_spend_usd: f64,
    pub(crate) sub_spend_usd: BTreeMap<String, f64>,
    pub(crate) feed: VecDeque<CampaignFeedEvent>,
    pub(crate) selected: usize,
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
            campaign_dir,
            campaign,
            rollup,
            aggregate_spend_usd,
            sub_spend_usd,
            feed: VecDeque::new(),
            selected: 0,
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

pub(crate) fn campaign_attach_json_text(state: &CampaignAttachState) -> Result<String> {
    let rollup = state.rollup.as_ref();
    let surface = campaign_verdict_surface(&state.campaign, rollup);
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
        campaign_verdict_surface(campaign, rollup).render_plain(false)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Details");
    let _ = writeln!(
        out,
        "campaign: {} ({})",
        campaign.root_goal,
        campaign_status_text(campaign.status)
    );
    if let Some(rollup) = rollup {
        let _ = writeln!(
            out,
            "roll-up {}",
            rollup_verdict_text(rollup.rollup_verdict)
        );
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
            sub.sub_id,
            sub_status_text(sub.status),
            result,
            sub.goal
        );
        if let Some(paths) = paths
            && let Some(run_id) = sub.result_run_id.as_deref()
            && let Ok(state) = load_run(paths, run_id)
        {
            let _ = writeln!(out, "    spend {}", run_spend_label(&state, false));
            let _ = writeln!(out, "    gate: {}", acceptance_status_value(&state));
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
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: Option<&deadreckon_core::campaign::CampaignRollup>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(
        out,
        "{}",
        campaign_verdict_surface(campaign, rollup).render_plain(false)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Details");
    let _ = writeln!(
        out,
        "campaign {} {}",
        run_prefix(&campaign.campaign_id),
        campaign_status_text(campaign.status)
    );
    if let Some(rollup) = rollup {
        let _ = writeln!(
            out,
            "roll-up {}",
            rollup_verdict_text(rollup.rollup_verdict)
        );
        if !rollup.refused_subs.is_empty() {
            let _ = writeln!(out, "refused subs: {}", rollup.refused_subs.join(", "));
        }
        if !rollup.caveat_subs.is_empty() {
            let _ = writeln!(out, "caveat subs: {}", rollup.caveat_subs.join(", "));
        }
    }
    for sub in &campaign.sub_goals {
        if sub.status == deadreckon_core::campaign::SubGoalStatus::Merged {
            continue;
        }
        let _ = writeln!(
            out,
            "  {} {} {}",
            sub.sub_id,
            sub_status_text(sub.status),
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
    mut launch: F,
    spend_of: S,
) -> Result<()>
where
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
        if tree_budget_exhausted(tree_budget, spent_usd) {
            append_campaign_event(
                campaign_dir,
                "budget_exhausted",
                serde_json::json!({ "spent_usd": spent_usd, "tree_budget_usd": tree_budget }),
            )?;
            break;
        }
        let sub = campaign.sub_goals[index].clone();
        let launch_dir = campaign_dir.join("launch").join(&sub.sub_id);
        fs::create_dir_all(&launch_dir)?;
        append_campaign_event(
            campaign_dir,
            "sub_launched",
            serde_json::json!({ "sub_id": sub.sub_id }),
        )?;
        match launch(&sub, &launch_dir) {
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
