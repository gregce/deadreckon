use super::super::*;
use crate::commands::orchestrate::recommend_child_count_for_goal;
use crate::commands::plan::{build_full_plan_tasks, resolve_plan_providers};

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

fn print_campaign_preflight(campaign: &deadreckon_core::campaign::Campaign, sandbox: Option<&str>) {
    let per_sub = campaign
        .tree_budget_usd
        .map(|budget| deadreckon_core::campaign::allocate_budget(budget, campaign.sub_goals.len()));
    println!("campaign: {} sub-orchestrators", campaign.n);
    println!(
        "  depth cap {} (sub-orchestrators cannot campaign again)",
        deadreckon_core::campaign::CAMPAIGN_MAX_DEPTH
    );
    match (
        campaign.tree_budget_usd,
        per_sub.as_ref().and_then(|s| s.first()),
    ) {
        (Some(total), Some(share)) => {
            println!("  tree budget ${total:.2} (~${share:.2}/sub)");
        }
        _ => {
            if let Some(warning) =
                deadreckon_core::campaign::unbounded_budget_warning(campaign.tree_budget_usd)
            {
                println!("  tree budget: {warning}");
            }
        }
    }
    if let Some(sandbox) = sandbox {
        println!("  sandbox {sandbox}");
    }
    if let Some(planner) = campaign.providers.planner.as_deref() {
        println!("  planner {planner}");
    }
    for sub in &campaign.sub_goals {
        println!("  {} {}", sub.sub_id, sub.goal);
    }
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
            "Launch campaign",
            "starts the sub-orchestrators",
        )];
        choices.push(start_prompt_choice(
            "edit",
            "Edit a sub-goal",
            "updates one pending sub-goal before launch",
        ));
        if campaign.sub_goals.len() > 2 {
            choices.push(start_prompt_choice(
                "drop",
                "Drop a sub-goal",
                "removes one pending sub-goal before launch",
            ));
        }
        choices.push(start_prompt_choice(
            "count",
            "Change count",
            "regenerates the proposed sub-goals",
        ));
        choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
        let choice = prompt::select_one(&prompt::SelectPrompt {
            title: "Review campaign preflight".to_string(),
            help: Some("Launch, edit/drop a sub-goal, change count, or cancel.".to_string()),
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
                            sub.goal.clone(),
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
                            sub.goal.clone(),
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
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign refused: depth cap 2 reached\n\
             try: run `orchestrate full-plan` (not campaign) inside a sub-orchestrator"
                .to_string(),
        )));
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
    let tasks =
        build_full_plan_tasks(&paths, &goal, n, &providers, &overrides, &cwd, args.plain).await?;
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
                    &paths, &goal, n, &providers, &overrides, &cwd, args.plain,
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

    let result_run_ids: Vec<String> = campaign_obj
        .sub_goals
        .iter()
        .filter_map(|sub| sub.result_run_id.clone())
        .collect();
    let merge_dir = campaign_dir.join("merge-working");
    let compose = compose_result_runs(&paths, &result_run_ids, &merge_dir)?;
    if !compose.conflicts.is_empty() {
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        campaign::append_campaign_event(
            &campaign_dir,
            "campaign_failed",
            serde_json::json!({ "reason": "cross-sub file conflict", "conflicts": compose.conflicts.len() }),
        )?;
        return Err(CliError::Core(deadreckon_core::user_error(
            "campaign failed: cross-sub file conflict",
            "narrow sub-goals so they touch disjoint files (cross-level repair is a V1 candidate)",
        )));
    }

    if !campaign::campaign_can_complete(&campaign_obj, &rollup) {
        campaign_obj.status = campaign::CampaignStatus::Failed;
        campaign::write_campaign(&campaign_dir, &campaign_obj)?;
        campaign::append_campaign_event(
            &campaign_dir,
            "rollup_refused",
            serde_json::json!({ "refused_subs": rollup.refused_subs }),
        )?;
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "campaign failed: refused sub(s) {}",
                rollup.refused_subs.join(", ")
            ),
            "deadreckon show <campaign-id> --why-failed",
        )));
    }

    let result_state = promote_campaign_result(&paths, &campaign_id, &merge_dir, &rollup)?;
    campaign_obj.merged_run_id = Some(result_state.run_id.clone());
    campaign_obj.merged_at = Some(chrono::Utc::now());
    campaign_obj.status = campaign::CampaignStatus::Merged;
    campaign::write_campaign(&campaign_dir, &campaign_obj)?;
    write_campaign_manifest(&paths, &campaign_obj, &result_state, &rollup)?;
    campaign::append_campaign_event(
        &campaign_dir,
        "campaign_completed",
        serde_json::json!({ "merged_run_id": result_state.run_id }),
    )?;
    if !args.quiet {
        let verdict = match rollup.rollup_verdict {
            campaign::RollupVerdict::Clean => "clean",
            campaign::RollupVerdict::Caveat => "caveat",
            campaign::RollupVerdict::Refused => "refused",
        };
        println!(
            "campaign {} complete: {}/{} subs · roll-up {verdict} · result {}",
            run_prefix(&campaign_id),
            campaign_obj
                .sub_goals
                .iter()
                .filter(|sub| sub.status == campaign::SubGoalStatus::Merged)
                .count(),
            campaign_obj.n,
            run_prefix(&result_state.run_id)
        );
        if !args.no_hints {
            println!("try: deadreckon apply {}", run_prefix(&result_state.run_id));
        }
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

/// Read-only attach summary for a campaign: breadcrumb, sub rows, and roll-up.
/// Full TUI drill-in into a selected sub-plan is a V1 candidate; this is the plain
/// summary that works on and off TTY.
pub(crate) fn campaign_attach_summary(
    paths: Option<&DeadreckonPaths>,
    campaign: &deadreckon_core::campaign::Campaign,
    rollup: Option<&deadreckon_core::campaign::CampaignRollup>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
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
        let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.starts_with(reference) {
            continue;
        }
        if deadreckon_core::campaign::campaign_path_for_plan_dir(&dir).is_file() {
            let campaign = deadreckon_core::campaign::read_campaign(&dir)?;
            return Ok(Some((dir, campaign)));
        }
    }
    Ok(None)
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
