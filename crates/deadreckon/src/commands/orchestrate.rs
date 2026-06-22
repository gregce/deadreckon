use super::super::*;

pub(crate) struct OrchestrateRunArgs {
    pub(crate) plan: PlanCommandArgs,
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
            plan: PlanCommandArgs {
                goal: resolve_required_goal_input(
                    "orchestrate review",
                    args.goal,
                    args.goal_file,
                    "deadreckon orchestrate review --goal-file docs/goal.md --yes",
                )?,
                n: 2,
                mode: CliPlanMode::Review,
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
                skip_acceptance_prompt: args.yes
                    || args.preview
                    || args.quiet
                    || bare.yes
                    || bare.preview
                    || bare.quiet,
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
            plan: PlanCommandArgs {
                goal: resolve_required_goal_input(
                    "orchestrate full-plan",
                    args.goal,
                    args.goal_file,
                    "deadreckon orchestrate full-plan --goal-file docs/goal.md --yes",
                )?,
                n: args.n,
                mode: CliPlanMode::FullPlan,
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
                skip_acceptance_prompt: args.yes
                    || args.preview
                    || args.quiet
                    || bare.yes
                    || bare.preview
                    || bare.quiet,
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
        skip_acceptance_prompt: yes || preview || quiet,
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
        plan,
        preview,
        yes,
        no_repair,
        completion_surface: true,
        narrate: narrate && !no_narrate,
        narrator_model,
    })
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
    let quiet = args.plan.quiet;
    let plain = args.plan.plain;
    let no_hints = args.plan.no_hints;
    let max_spend = args.plan.max_spend;
    let max_wall_seconds = args.plan.max_wall_seconds;
    let sandbox = args.plan.sandbox.clone();
    if !commands::plan::prepare_orchestration_source(args.plan.init_git, quiet)? {
        return Ok(());
    }
    let plan = commands::plan::create_orchestration_plan(args.plan).await?;
    let plan_id = plan.plan_id.clone();
    if let Ok(launch_dir) = std::env::var(deadreckon_core::campaign::ENV_SUB_RESULT) {
        commands::campaign::publish_sub_plan_id(std::path::Path::new(&launch_dir), &plan_id);
    }
    if !quiet {
        commands::plan::print_orchestrate_preflight(
            &plan,
            max_spend,
            max_wall_seconds,
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
        sandbox,
        provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        no_repair: args.no_repair,
        repair_provider: None,
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
        commands::campaign::record_sub_orchestrator_result(
            &plan_id,
            std::path::Path::new(&launch_dir),
            merge_result.is_ok(),
        );
    }
    merge_result
}

#[cfg(test)]
mod tests {
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
}
