use super::super::*;

const DURABLE_ORCHESTRATION_SIGNAL: &str = "watchkeeper_orchestration";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DurableOrchestrationSpec {
    pub(crate) planner_model: Option<String>,
    pub(crate) child_models: Vec<String>,
    pub(crate) coder_model: Option<String>,
    pub(crate) reviewer_model: Option<String>,
    pub(crate) no_repair: bool,
    pub(crate) narrate: bool,
    pub(crate) narrator_model: Option<String>,
}

pub(crate) fn durable_orchestration_spec(
    plan: &commands::course::LaunchPlan,
) -> Result<Option<DurableOrchestrationSpec>> {
    plan.signals
        .get(DURABLE_ORCHESTRATION_SIGNAL)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(CliError::from)
}

pub(crate) struct OrchestrateRunArgs {
    pub(crate) plan: PlanCommandArgs,
    /// The graph the launch classifier already drew. When present, plan
    /// creation uses it instead of asking a second planner for a child graph.
    pub(crate) seed_pieces: Vec<commands::course::CoursePiece>,
    /// An already accepted launch decision, used by reshape/replay callers
    /// that must preserve lineage and decision provenance in the durable Job.
    pub(crate) accepted_launch_plan: Option<commands::course::LaunchPlan>,
    pub(crate) deadline: Option<DateTime<Utc>>,
    pub(crate) preview: bool,
    pub(crate) yes: bool,
    pub(crate) no_repair: bool,
    pub(crate) completion_surface: bool,
    pub(crate) narrate: bool,
    pub(crate) narrator_model: Option<String>,
}

pub(crate) struct BareOrchestrateArgs {
    pub(crate) goal: Option<String>,
    pub(crate) goal_file: Option<PathBuf>,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) deadline: Option<DateTime<Utc>>,
    pub(crate) sandbox: Option<String>,
    pub(crate) preview: bool,
    pub(crate) init_git: bool,
    pub(crate) acceptance: Option<PathBuf>,
    pub(crate) yes: bool,
    pub(crate) no_repair: bool,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) narrate: bool,
    pub(crate) no_narrate: bool,
    pub(crate) narrator_model: Option<String>,
}

pub(crate) fn orchestrate_request_from_cli(
    command: Option<OrchestrateCommand>,
    bare: BareOrchestrateArgs,
) -> Result<OrchestrateRunArgs> {
    match command {
        Some(OrchestrateCommand::Review(args)) => Ok(OrchestrateRunArgs {
            seed_pieces: Vec::new(),
            accepted_launch_plan: None,
            deadline: args.deadline.or(bare.deadline),
            plan: PlanCommandArgs {
                goal: resolve_required_goal_input(
                    "orchestrate review",
                    args.goal,
                    args.goal_file,
                    "deadreckon orchestrate review --goal-file docs/goal.md --yes",
                )?,
                n: 2,
                mode: CliPlanMode::Review,
                apply: deadreckon_core::plan::ApplyWhen::AtEnd,
                max_spend: args.max_spend.or(bare.max_spend),
                max_wall_seconds: args.max_wall_seconds.or(bare.max_wall_seconds),
                sandbox: args.sandbox.or(bare.sandbox),
                planner_provider: None,
                provider: None,
                child_provider: Vec::new(),
                coder_provider: args.coder_provider,
                reviewer_provider: args.reviewer_provider,
                planner_model: None,
                model: None,
                child_model: Vec::new(),
                coder_model: args.coder_model,
                reviewer_model: args.reviewer_model,
                init_git: args.init_git || bare.init_git,
                acceptance: args.acceptance.or(bare.acceptance),
                skip_acceptance_prompt: orchestration_acceptance_prompt_may_skip(
                    args.yes || bare.yes,
                    args.preview || bare.preview,
                    args.quiet || bare.quiet,
                ),
                no_hints: args.no_hints || bare.no_hints,
                quiet: args.quiet || bare.quiet,
                json: false,
                plain: args.plain || bare.plain,
            },
            preview: args.preview || bare.preview,
            yes: args.yes || bare.yes,
            no_repair: args.no_repair || bare.no_repair,
            completion_surface: true,
            narrate: (args.narrate || bare.narrate) && !(args.no_narrate || bare.no_narrate),
            narrator_model: args.narrator_model.or(bare.narrator_model),
        }),
        Some(OrchestrateCommand::FullPlan(args)) => Ok(OrchestrateRunArgs {
            seed_pieces: Vec::new(),
            accepted_launch_plan: None,
            deadline: args.deadline.or(bare.deadline),
            plan: PlanCommandArgs {
                goal: resolve_required_goal_input(
                    "orchestrate full-plan",
                    args.goal,
                    args.goal_file,
                    "deadreckon orchestrate full-plan --goal-file docs/goal.md --yes",
                )?,
                n: args.n,
                mode: CliPlanMode::FullPlan,
                apply: deadreckon_core::plan::ApplyWhen::AtEnd,
                max_spend: args.max_spend.or(bare.max_spend),
                max_wall_seconds: args.max_wall_seconds.or(bare.max_wall_seconds),
                sandbox: args.sandbox.or(bare.sandbox),
                planner_provider: args.planner_provider,
                provider: args.provider,
                child_provider: args.child_provider,
                coder_provider: None,
                reviewer_provider: None,
                planner_model: args.planner_model,
                model: args.model,
                child_model: args.child_model,
                coder_model: None,
                reviewer_model: None,
                init_git: args.init_git || bare.init_git,
                acceptance: args.acceptance.or(bare.acceptance),
                skip_acceptance_prompt: orchestration_acceptance_prompt_may_skip(
                    args.yes || bare.yes,
                    args.preview || bare.preview,
                    args.quiet || bare.quiet,
                ),
                no_hints: args.no_hints || bare.no_hints,
                quiet: args.quiet || bare.quiet,
                json: false,
                plain: args.plain || bare.plain,
            },
            preview: args.preview || bare.preview,
            yes: args.yes || bare.yes,
            no_repair: args.no_repair || bare.no_repair,
            completion_surface: true,
            narrate: (args.narrate || bare.narrate) && !(args.no_narrate || bare.no_narrate),
            narrator_model: args.narrator_model.or(bare.narrator_model),
        }),
        None => interactive_orchestrate_request(bare),
    }
}

fn interactive_orchestrate_request(bare: BareOrchestrateArgs) -> Result<OrchestrateRunArgs> {
    let BareOrchestrateArgs {
        goal,
        goal_file,
        max_spend,
        max_wall_seconds,
        deadline,
        sandbox,
        preview,
        init_git,
        acceptance,
        yes,
        no_repair,
        no_hints,
        quiet,
        plain,
        narrate,
        no_narrate,
        narrator_model,
    } = bare;
    let Some(goal) = resolve_optional_goal_input("orchestrate", goal, goal_file)? else {
        return Err(CliError::Core(deadreckon_core::user_error(
            "orchestrate requires a mode or goal",
            "deadreckon orchestrate review --goal-file docs/goal.md --coder-provider cli:claude-code --reviewer-provider cli:codex --yes",
        )));
    };
    if !io::stdin().is_terminal() {
        return Err(orchestrate_mode_refusal_error(
            &goal,
            "non-interactive orchestrate requires an explicit mode",
            no_hints,
        ));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let default_provider = commands::plan::resolve_provider_name(
        &paths,
        setup::SetupProviderRoleRef::DefaultChild,
        defaults.provider,
    )?;
    let recommended_mode = recommend_orchestration_mode(&goal);
    println!("{}", ui_heading("Orchestration mode"));
    println!(
        "  recommendation: {} - {}",
        ui_command(plan_mode_label(match recommended_mode {
            CliPlanMode::FullPlan => PlanMode::FullPlan,
            CliPlanMode::Review => PlanMode::Review,
        })),
        orchestration_mode_recommendation_reason(&goal, recommended_mode)
    );
    println!(
        "  {} focused implementation with one coder provider, then a fresh reviewer/fixer",
        ui_command("review")
    );
    println!(
        "  {} planner provider decomposes broad product work into child implementation agents before fork and merge",
        ui_command("full-plan")
    );
    let mode = prompt_orchestration_mode(recommended_mode)?;
    print_orchestrate_provider_choices(&paths, default_provider.as_deref())?;
    let mut plan = PlanCommandArgs {
        goal,
        n: recommend_child_count_for_goal("", mode),
        mode,
        apply: deadreckon_core::plan::ApplyWhen::AtEnd,
        max_spend,
        max_wall_seconds,
        sandbox,
        planner_provider: None,
        provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        planner_model: None,
        model: None,
        child_model: Vec::new(),
        coder_model: None,
        reviewer_model: None,
        init_git,
        acceptance,
        skip_acceptance_prompt: orchestration_acceptance_prompt_may_skip(yes, preview, quiet),
        no_hints,
        quiet,
        json: false,
        plain,
    };
    match mode {
        CliPlanMode::FullPlan => {
            plan.n = prompt_child_count(recommend_child_count_for_goal(&plan.goal, mode))?;
            plan.planner_provider = prompt_provider_role("planner", default_provider.as_deref())?;
            plan.provider = prompt_provider_role("default child", default_provider.as_deref())?;
            plan.child_provider = prompt_child_provider_overrides(plan.n)?;
        }
        CliPlanMode::Review => {
            plan.coder_provider = prompt_provider_role("coder", default_provider.as_deref())?;
            plan.reviewer_provider = prompt_provider_role("reviewer", default_provider.as_deref())?;
        }
    }
    Ok(OrchestrateRunArgs {
        seed_pieces: Vec::new(),
        accepted_launch_plan: None,
        deadline,
        plan,
        preview,
        yes,
        no_repair,
        completion_surface: true,
        narrate: narrate && !no_narrate,
        narrator_model,
    })
}

fn orchestration_acceptance_prompt_may_skip(yes: bool, preview: bool, _quiet: bool) -> bool {
    yes || preview
}

fn orchestrate_mode_refusal_error(goal: &str, message: &str, no_hints: bool) -> CliError {
    let recommended_mode = recommend_orchestration_mode(goal);
    let primary = orchestrate_mode_command(goal, recommended_mode);
    let secondary_mode = match recommended_mode {
        CliPlanMode::FullPlan => CliPlanMode::Review,
        CliPlanMode::Review => CliPlanMode::FullPlan,
    };
    let secondary_command = orchestrate_mode_command(goal, secondary_mode);
    let secondary = [secondary_command.as_str()];
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "orchestrate",
            None,
            ExplanationPanel::new(
                "DeadReckon did not start orchestration because this shell cannot choose a mode interactively.",
                "Bare orchestrate needs a review or full-plan decision before it can create accurate plan state, so non-interactive execution must supply the mode explicitly.",
                vec![
                    ("reason".to_string(), message.to_string()),
                    ("command".to_string(), "orchestrate".to_string()),
                    ("stdin".to_string(), "non-interactive".to_string()),
                    (
                        "recommended mode".to_string(),
                        plan_mode_label(match recommended_mode {
                            CliPlanMode::FullPlan => PlanMode::FullPlan,
                            CliPlanMode::Review => PlanMode::Review,
                        })
                        .to_string(),
                    ),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            secondary
                .iter()
                .map(|command| ("Secondary", *command)),
        )
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}

fn orchestrate_mode_command(goal: &str, mode: CliPlanMode) -> String {
    let raw_goal = goal;
    let goal = format!("\"{}\"", shell_display_quote(goal));
    match mode {
        CliPlanMode::Review => {
            format!(
                "deadreckon orchestrate review {goal} --coder-provider cli:claude-code --reviewer-provider cli:codex --yes"
            )
        }
        CliPlanMode::FullPlan => format!(
            "deadreckon orchestrate full-plan {goal} --planner-provider cli:codex --provider cli:claude-code --n {} --yes",
            recommend_child_count_for_goal(raw_goal, CliPlanMode::FullPlan)
        ),
    }
}

pub(crate) fn recommend_orchestration_mode(goal: &str) -> CliPlanMode {
    let lower = goal.to_ascii_lowercase();
    let broad_product = [
        "make a full",
        "build a full",
        "create a full",
        "fully",
        "from scratch",
        "app",
        "game",
        "site",
        "multiplayer",
        "realtime",
        "real-time",
        "live",
        "server",
        "client",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let focused_change = [
        "fix ", "bug", "change ", "refactor", "review", "audit", "explain", "docs", "rename",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if broad_product && !focused_change {
        CliPlanMode::FullPlan
    } else {
        CliPlanMode::Review
    }
}

fn orchestration_mode_recommendation_reason(goal: &str, mode: CliPlanMode) -> &'static str {
    match mode {
        CliPlanMode::FullPlan => {
            if goal.to_ascii_lowercase().contains("multiplayer") {
                "goal looks like broad product work with separable implementation slices"
            } else {
                "goal looks broad enough to decompose before execution"
            }
        }
        CliPlanMode::Review => "goal looks focused enough for coder plus reviewer",
    }
}

pub(crate) fn recommend_child_count_for_goal(goal: &str, mode: CliPlanMode) -> u8 {
    if mode == CliPlanMode::Review {
        return 2;
    }
    let lower = goal.to_ascii_lowercase();
    let complexity = [
        "multiplayer",
        "realtime",
        "real-time",
        "live",
        "physics",
        "terrain",
        "server",
        "database",
        "auth",
        "deploy",
        "mobile",
        "game",
    ]
    .iter()
    .filter(|needle| lower.contains(*needle))
    .count();
    match complexity {
        0 | 1 => 3,
        2 | 3 => 4,
        _ => 5,
    }
}

fn prompt_orchestration_mode(default: CliPlanMode) -> Result<CliPlanMode> {
    let default_label = match default {
        CliPlanMode::Review => "review",
        CliPlanMode::FullPlan => "full-plan",
    };
    let answer = prompt::open(&format!("mode [{default_label}]: "), None)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "r" | "review" => Ok(CliPlanMode::Review),
        "f" | "full" | "full-plan" | "full_plan" | "plan" => Ok(CliPlanMode::FullPlan),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown orchestration mode {other}"),
            "choose review or full-plan",
        ))),
    }
}

fn prompt_child_count(default: u8) -> Result<u8> {
    // Re-prompt on bad input instead of aborting the whole command.
    let n = prompt::ask_number("children", 2..=6, usize::from(default))?;
    Ok(n as u8)
}

fn prompt_child_provider_overrides(n: u8) -> Result<Vec<String>> {
    println!(
        "  optional: route specific child indices 0..{} to another provider, e.g. 1=cli:codex",
        n.saturating_sub(1)
    );
    let answer = prompt::open("child provider overrides []: ", None)?;
    if answer.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn prompt_provider_role(role: &str, default: Option<&str>) -> Result<Option<String>> {
    let prompt_text = match default {
        Some(default) => format!("{role} provider [{default}]: "),
        None => format!("{role} provider: "),
    };
    let answer = prompt::open(&prompt_text, None)?;
    let provider = answer.trim();
    if provider.is_empty() {
        return Ok(default.map(ToString::to_string));
    }
    Ok(Some(provider.to_string()))
}

fn print_orchestrate_provider_choices(
    paths: &DeadreckonPaths,
    default_provider: Option<&str>,
) -> Result<()> {
    let configured = configured_provider_ids(paths)?;
    for line in orchestrate_provider_choice_lines(default_provider, &configured) {
        println!("{line}");
    }
    Ok(())
}

fn orchestrate_provider_choice_lines(
    default_provider: Option<&str>,
    configured: &[String],
) -> Vec<String> {
    let mut lines = vec![
        ui_heading("Providers"),
        format!(
            "  default: {}",
            default_provider
                .map(ui_command)
                .unwrap_or_else(|| ui_muted("none configured"))
        ),
    ];
    if configured.is_empty() {
        lines.push("  configured: none".to_string());
        lines.push(format!(
            "  {} {}",
            ui_command("setup:"),
            ui_command("deadreckon providers list --all")
        ));
    } else {
        lines.push(format!("  configured: {}", configured.join(", ")));
    }
    lines.push(
        "  planner creates the child graph; child/coder/reviewer providers execute work."
            .to_string(),
    );
    lines
}

pub(crate) async fn orchestrate_command(args: OrchestrateRunArgs) -> Result<()> {
    if commands::graph_job::current_parent_job_id().is_none()
        && !commands::plan::internal_characterization_requested()
        && !args.preview
    {
        return schedule_direct_orchestration(args).await;
    }
    let quiet = args.plan.quiet;
    let plain = args.plan.plain;
    let no_hints = args.plan.no_hints;
    let max_spend = args.plan.max_spend;
    let max_wall_seconds = args.plan.max_wall_seconds;
    let deadline = args.deadline;
    let sandbox = args.plan.sandbox.clone();
    let plan = if args.preview {
        commands::plan::preview_orchestration_plan(args.plan, &args.seed_pieces).await?
    } else {
        if !commands::plan::prepare_orchestration_source(args.plan.init_git, quiet)? {
            return Ok(());
        }
        commands::plan::create_orchestration_plan(args.plan, &args.seed_pieces).await?
    };
    let plan_id = plan.plan_id.clone();
    if !args.preview
        && let Ok(launch_dir) = std::env::var(deadreckon_core::campaign::ENV_SUB_RESULT)
    {
        commands::campaign::publish_sub_plan_id(std::path::Path::new(&launch_dir), &plan_id)?;
        commands::campaign::campaign_test_failpoint("after_sub_plan_created_before_execution");
    }
    if !quiet {
        commands::plan::print_orchestrate_preflight(
            &plan,
            max_spend,
            max_wall_seconds,
            deadline.as_ref(),
            sandbox.as_deref(),
            args.no_repair,
        );
    }
    if args.preview {
        return Ok(());
    }
    commands::plan::confirm_orchestration_start(&plan, args.yes, no_hints)?;
    if !quiet {
        commands::plan::print_orchestrate_started(
            &plan,
            max_spend,
            max_wall_seconds,
            sandbox.as_deref(),
            args.no_repair,
        );
    }
    commands::plan::fork_command(ForkCommandArgs {
        plan_id: plan_id.clone(),
        max_spend,
        max_wall_seconds,
        deadline,
        sandbox,
        provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        no_repair: args.no_repair,
        repair_provider: None,
        yes: true,
        no_hints,
        quiet,
        plain,
        completion_surface: false,
        narrate: args.narrate,
        narrator_model: args.narrator_model.clone(),
    })
    .await?;
    let sub_result_launch_dir = std::env::var(deadreckon_core::campaign::ENV_SUB_RESULT).ok();
    let merge_result = commands::merge::merge_command(MergeCommandArgs {
        plan_id: plan_id.clone(),
        strategy: "dag-aware".to_string(),
        prefer_child: None,
        no_repair: args.no_repair,
        repair_provider: None,
        repair_mode: "auto".to_string(),
        repair_attempts: 1,
        yes: true,
        no_gate: false,
        no_hints,
        quiet,
        plain,
        completion_surface: args.completion_surface,
    })
    .await;
    if let Some(launch_dir) = sub_result_launch_dir {
        commands::campaign::campaign_test_failpoint("after_sub_merge_before_result");
        let record_result = commands::campaign::record_sub_orchestrator_result(
            &plan_id,
            std::path::Path::new(&launch_dir),
            merge_result.is_ok(),
        );
        if merge_result.is_ok() {
            record_result?;
        }
    }
    merge_result
}

pub(crate) async fn schedule_direct_orchestration(args: OrchestrateRunArgs) -> Result<()> {
    let quiet = args.plan.quiet;
    let deadline = args.deadline;
    let explicitly_approved = orchestration_approval_policy(args.yes, quiet, prompt::is_tty())?;
    if !commands::plan::prepare_orchestration_source(args.plan.init_git, quiet)? {
        return Ok(());
    }
    if let Some(model) = args.narrator_model.as_deref() {
        let paths = DeadreckonPaths::discover();
        if let Ok(registry) =
            deadreckon_providers::registry::ProviderRegistry::with_overrides(paths.home())
            && !crate::narrator::narrator_model_known(&registry, model)
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                crate::narrator::narrator_model_refusal(model),
            )));
        }
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let cwd = std::env::current_dir()?;
    let max_spend_usd = args.plan.max_spend.or(defaults.max_spend).unwrap_or(10.0);
    let max_wall_seconds = commands::job::checked_job_wall_seconds(
        args.plan
            .max_wall_seconds
            .or(defaults.cli_max_wall_seconds)
            .unwrap_or(36_000.0),
    )?;
    let sandbox_requested = args
        .plan
        .sandbox
        .clone()
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    if sandbox_requested == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable orchestration requires a containment backend; sandbox `none` cannot produce a trusted receipt",
            "omit `--sandbox none` or choose auto, sandbox-exec, bwrap, or docker",
        )));
    }
    let contract_provider = args
        .plan
        .planner_provider
        .clone()
        .or_else(|| args.plan.provider.clone())
        .or_else(|| args.plan.coder_provider.clone());
    let contract_model = args
        .plan
        .planner_model
        .clone()
        .or_else(|| args.plan.model.clone())
        .or_else(|| args.plan.coder_model.clone());
    let contract = commands::acceptance::ensure_acceptance_before_start(
        &cwd,
        args.plan.acceptance.as_deref(),
        &args.plan.goal,
        contract_provider,
        contract_model,
        explicitly_approved,
        "orchestrate",
    )
    .await?;
    if !explicitly_approved && !prompt::confirm("queue this durable orchestration job?", true)? {
        eprintln!("cancelled before job creation");
        return Ok(());
    }

    let kind = match args.plan.mode {
        CliPlanMode::Review => commands::graph_job::DriverKind::Review,
        CliPlanMode::FullPlan => commands::graph_job::DriverKind::FullPlan,
    };
    let driver = commands::graph_job::DriverSpec {
        kind,
        child_count: Some(args.plan.n),
        apply: args.plan.apply,
        planner_provider: args.plan.planner_provider.clone(),
        child_provider: args.plan.provider.clone(),
        child_provider_overrides: args.plan.child_provider.clone(),
        coder_provider: args.plan.coder_provider.clone(),
        reviewer_provider: args.plan.reviewer_provider.clone(),
        model: args.plan.model.clone(),
        source_init_git: args.plan.init_git,
    };
    let accepted_launch_plan = args.accepted_launch_plan.clone();
    let mut launch_plan = accepted_launch_plan.clone().unwrap_or_else(|| {
        commands::course::trivial_operator_plan(
            &args.plan.goal,
            commands::course::CourseShape::Plan,
            "orchestrate",
        )
    });
    launch_plan.n = Some(args.plan.n);
    launch_plan.pieces = args.seed_pieces;
    launch_plan.providers = commands::course::CourseProviders {
        planner: args.plan.planner_provider.clone(),
        coder: args.plan.coder_provider.clone(),
        reviewer: args.plan.reviewer_provider.clone(),
    };
    launch_plan.budget.ceiling_usd = Some(max_spend_usd);
    launch_plan.budget.wall_seconds = Some(max_wall_seconds);
    launch_plan.budget.deadline = deadline;
    if accepted_launch_plan.is_none() {
        launch_plan.accepted_by = Some(if explicitly_approved {
            "yes-flag-guardrail".to_string()
        } else {
            "operator".to_string()
        });
    }
    let execution = DurableOrchestrationSpec {
        planner_model: args.plan.planner_model,
        child_models: args.plan.child_model,
        coder_model: args.plan.coder_model,
        reviewer_model: args.plan.reviewer_model,
        no_repair: args.no_repair,
        narrate: args.narrate,
        narrator_model: args.narrator_model,
    };
    let mut signals = launch_plan.signals.as_object().cloned().unwrap_or_default();
    signals.insert(
        DURABLE_ORCHESTRATION_SIGNAL.to_string(),
        serde_json::to_value(execution)?,
    );
    launch_plan.signals = serde_json::Value::Object(signals);

    let source = if deadreckon_core::find_git_root(&cwd)?.is_some() {
        commands::job::DurableSource {
            mode: commands::job::DurableSourceMode::Worktree,
            from: None,
            allow_dirty: false,
        }
    } else {
        commands::job::DurableSource {
            mode: commands::job::DurableSourceMode::Copy,
            from: Some(cwd.clone()),
            allow_dirty: false,
        }
    };
    let accepted_by = if explicitly_approved {
        deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
    } else {
        deadreckon_protocol::AuthorityAcceptedBy::Operator
    };
    let job = persist_direct_orchestration_job(
        &paths,
        &cwd,
        launch_plan,
        driver,
        contract.as_ref().map(|source| source.path.as_path()),
        source,
        max_spend_usd,
        max_wall_seconds,
        deadline,
        sandbox_requested,
        accepted_by,
    )?;
    commands::job::launch_detached_supervisor(&paths, &job.job_id)?;
    if !quiet {
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref())?;
        commands::job::print_job_status(&view, false)?;
    }
    Ok(())
}

fn orchestration_approval_policy(yes: bool, _quiet: bool, is_tty: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !is_tty {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable orchestration needs explicit approval before its immutable Job is queued",
            "rerun with --yes after reviewing the goal, definition of done, child graph, budget and sandbox",
        )));
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn persist_direct_orchestration_job(
    paths: &DeadreckonPaths,
    cwd: &Path,
    launch_plan: commands::course::LaunchPlan,
    driver: commands::graph_job::DriverSpec,
    contract_source: Option<&Path>,
    source: commands::job::DurableSource,
    max_spend_usd: f64,
    max_wall_seconds: u64,
    deadline: Option<DateTime<Utc>>,
    sandbox_requested: String,
    accepted_by: deadreckon_protocol::AuthorityAcceptedBy,
) -> Result<deadreckon_protocol::Job> {
    commands::job::create_job(commands::job::CreateJob {
        paths,
        source_cwd: cwd,
        scope: workspace_scope(cwd)?,
        launch_plan,
        shape: deadreckon_protocol::JobShape::Graph,
        driver: Some(driver),
        contract_source,
        source,
        max_spend_usd,
        max_wall_seconds,
        max_attempts: 3,
        deadline,
        sandbox_requested,
        accepted_by,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn provider_choices_without_configured_providers_use_setup_recovery_rows() {
        let rendered = orchestrate_provider_choice_lines(None, &[]).join("\n");

        assert!(rendered.contains("Providers"), "{rendered}");
        assert!(rendered.contains("default: none configured"), "{rendered}");
        assert!(rendered.contains("configured: none"), "{rendered}");
        assert!(!rendered.contains("recommended:"), "{rendered}");
        assert!(
            rendered.contains("setup: deadreckon providers list --all"),
            "{rendered}"
        );
        assert!(!rendered.contains("try:"), "{rendered}");
    }

    #[test]
    fn provider_choices_with_configured_providers_does_not_add_recovery_command() {
        let configured = vec!["cli:codex".to_string(), "cli:claude-code".to_string()];
        let rendered = orchestrate_provider_choice_lines(Some("cli:codex"), &configured).join("\n");

        assert!(rendered.contains("default: cli:codex"), "{rendered}");
        assert!(
            rendered.contains("configured: cli:codex, cli:claude-code"),
            "{rendered}"
        );
        assert!(!rendered.contains("recommended:"), "{rendered}");
        assert!(!rendered.contains("try:"), "{rendered}");
    }

    #[test]
    fn direct_orchestration_persists_one_bounded_graph_job_with_parent_identity() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable graph").expect("source file");
        let mut launch = commands::course::trivial_operator_plan(
            "finish the graph task",
            commands::course::CourseShape::Plan,
            "orchestrate",
        );
        launch.n = Some(2);
        let execution = DurableOrchestrationSpec {
            planner_model: Some("planner-model".to_string()),
            child_models: vec!["1=review-model".to_string()],
            coder_model: Some("coder-model".to_string()),
            reviewer_model: Some("reviewer-model".to_string()),
            no_repair: true,
            narrate: false,
            narrator_model: None,
        };
        launch.signals = serde_json::json!({
            DURABLE_ORCHESTRATION_SIGNAL: execution
        });
        let driver = commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::Review,
            child_count: Some(2),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: None,
            child_provider: None,
            child_provider_overrides: Vec::new(),
            coder_provider: Some("coder".to_string()),
            reviewer_provider: Some("reviewer".to_string()),
            model: None,
            source_init_git: false,
        };
        let deadline = Utc::now() + chrono::TimeDelta::hours(2);

        let job = persist_direct_orchestration_job(
            &paths,
            &source,
            launch,
            driver.clone(),
            None,
            commands::job::DurableSource {
                mode: commands::job::DurableSourceMode::Copy,
                from: Some(source.clone()),
                allow_dirty: false,
            },
            12.0,
            600,
            Some(deadline),
            "auto".to_string(),
            deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail,
        )
        .expect("persist graph job");
        let authority: deadreckon_protocol::JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("launch plan");

        assert_eq!(job.shape, deadreckon_protocol::JobShape::Graph);
        assert_eq!(job.job_id.as_ref(), authority.run_id.as_ref());
        assert_eq!(job.policy.max_attempts, 3);
        assert_eq!(job.policy.deadline, Some(deadline));
        assert_eq!(frozen.budget.deadline, Some(deadline));
        assert_eq!(
            commands::graph_job::driver_spec(&frozen).expect("driver"),
            driver
        );
        assert_eq!(
            durable_orchestration_spec(&frozen)
                .expect("execution spec")
                .expect("execution"),
            DurableOrchestrationSpec {
                planner_model: Some("planner-model".to_string()),
                child_models: vec!["1=review-model".to_string()],
                coder_model: Some("coder-model".to_string()),
                reviewer_model: Some("reviewer-model".to_string()),
                no_repair: true,
                narrate: false,
                narrator_model: None,
            }
        );
    }

    #[test]
    fn quiet_non_tty_orchestration_is_not_operator_approval() {
        let error = orchestration_approval_policy(false, true, false)
            .expect_err("quiet cannot approve a graph job");
        assert!(error.to_string().contains("needs explicit approval"));
        assert!(orchestration_approval_policy(true, true, false).expect("yes approves"));
    }

    #[test]
    fn quiet_does_not_skip_orchestration_contract_approval() {
        assert!(!orchestration_acceptance_prompt_may_skip(
            false, false, true
        ));
        assert!(orchestration_acceptance_prompt_may_skip(false, true, true));
        assert!(orchestration_acceptance_prompt_may_skip(true, false, true));
    }
}
