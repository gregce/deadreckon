use super::super::*;

pub(crate) async fn plan_command(args: PlanCommandArgs) -> Result<()> {
    let quiet = args.quiet;
    let no_hints = args.no_hints;
    let json_output = args.json;
    if !prepare_orchestration_source(args.init_git, quiet)? {
        return Ok(());
    }
    let plan = create_orchestration_plan(args, &[]).await?;
    if json_output {
        print_plan_json(&plan)?;
        return Ok(());
    }
    if !quiet {
        print_plan_created(&plan, no_hints);
    }
    Ok(())
}

pub(crate) fn prepare_orchestration_source(init_git: bool, quiet: bool) -> Result<bool> {
    let cwd = std::env::current_dir()?;
    if init_git {
        init_git_repo(&cwd)?;
        return Ok(true);
    }
    if deadreckon_core::find_git_root(&cwd)?.is_some() || quiet || !io::stdin().is_terminal() {
        return Ok(true);
    }
    match prompt_non_git_mode()? {
        NonGitChoice::Init => {
            init_git_repo(&cwd)?;
            Ok(true)
        }
        NonGitChoice::Copy => Ok(true),
        NonGitChoice::Cancel => {
            println!("{}", ui_status("cancelled"));
            Ok(false)
        }
    }
}

pub(crate) async fn create_orchestration_plan(
    args: PlanCommandArgs,
    seed_pieces: &[commands::course::CoursePiece],
) -> Result<Plan> {
    let PlanCommandArgs {
        goal,
        n,
        mode,
        apply,
        max_spend: _,
        max_wall_seconds: _,
        sandbox: _,
        planner_provider,
        provider,
        child_provider,
        coder_provider,
        reviewer_provider,
        planner_model,
        model,
        child_model,
        coder_model,
        reviewer_model,
        init_git: _,
        acceptance,
        skip_acceptance_prompt,
        no_hints,
        quiet,
        json: json_output,
        plain,
    } = args;
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return Err(plan_refusal_error(
            "--goal must be non-empty",
            "DeadReckon did not create a plan because the plan command needs a non-empty goal.",
            "Planning without a goal would create misleading orchestration state, so DeadReckon refused before writing any plan files.",
            vec![
                ("command".to_string(), "plan".to_string()),
                ("goal".to_string(), "empty".to_string()),
            ],
            "deadreckon plan \"your goal\"".to_string(),
            Vec::new(),
            no_hints,
            json_output,
        ));
    }
    validate_plan_task_count(&goal, n, no_hints, json_output)?;
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let cwd = std::env::current_dir()?;
    let scope = workspace_scope(&cwd)?;
    let plan_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let plan_mode = match mode {
        CliPlanMode::FullPlan => PlanMode::FullPlan,
        CliPlanMode::Review => PlanMode::Review,
    };
    let mut providers = resolve_plan_providers(
        &paths,
        &defaults,
        plan_mode,
        planner_provider,
        provider,
        coder_provider,
        reviewer_provider,
        PlanModelOverrides {
            planner_model,
            model,
            coder_model,
            reviewer_model,
            child_models: parse_child_model_overrides(&child_model, n)?,
        },
    )?;
    let acceptance_provider = match plan_mode {
        PlanMode::FullPlan => providers
            .planner
            .clone()
            .or_else(|| providers.default_child.clone()),
        PlanMode::Review => providers
            .coder
            .clone()
            .or_else(|| providers.reviewer.clone())
            .or_else(|| providers.default_child.clone()),
    };
    let acceptance_source = commands::acceptance::ensure_acceptance_before_start(
        &cwd,
        acceptance.as_deref(),
        &goal,
        acceptance_provider,
        None,
        skip_acceptance_prompt || quiet,
        "orchestration",
    )
    .await?;
    let mut tasks = match plan_mode {
        PlanMode::FullPlan => {
            let overrides = parse_child_provider_overrides(&child_provider, n)?;
            providers.children = overrides.clone();
            match plan_tasks_from_seed(seed_pieces, n, &providers, &overrides) {
                Some(tasks) => tasks,
                None => {
                    build_full_plan_tasks(
                        &paths,
                        &goal,
                        n,
                        &providers,
                        &overrides,
                        &cwd,
                        plain,
                        no_hints,
                        json_output,
                    )
                    .await?
                }
            }
        }
        PlanMode::Review => build_review_plan_tasks(&goal, &providers),
    };
    for task in &mut tasks {
        task.worker_spec = deadreckon_core::worker_spec_relative_path(&task.task_id);
    }
    let mut plan = Plan::new(
        goal,
        plan_mode,
        tasks,
        providers,
        Some(scope),
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(CliError::Core)?;
    plan.parent_cwd = Some(plan_cwd);
    // Per-node apply serializes execution and lands work incrementally, so a
    // stop-on-failure policy matches what the operator asked for: a half-
    // applied branch should not keep growing after something breaks.
    plan.apply = apply;
    if apply == deadreckon_core::plan::ApplyWhen::PerNode {
        plan.on_fail = OnFail::Stop;
    }
    plan.acceptance_path = acceptance_source.as_ref().map(|source| source.path.clone());
    plan.capability_preview = infer_capability_preview(&plan.root_goal);
    for task in &plan.tasks {
        let spec = render_worker_spec(&plan, task);
        write_worker_spec(&paths, &plan.plan_id, &task.task_id, &spec)?;
    }
    save_plan(&paths, &plan)?;
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::PlanCreated {
            mode: plan.mode,
            task_count: plan.tasks.len(),
        },
    )?;
    Ok(plan)
}

#[derive(Default)]
pub(crate) struct PlanModelOverrides {
    pub(crate) planner_model: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) coder_model: Option<String>,
    pub(crate) reviewer_model: Option<String>,
    pub(crate) child_models: std::collections::BTreeMap<u32, String>,
}

/// Per-role model resolution: per-role flag -> generic --model -> config
/// defaults.model -> None (the provider's own default; no --model argv).
fn resolve_role_model(
    role_flag: Option<&String>,
    generic: Option<&String>,
    configured: Option<&String>,
) -> Option<String> {
    role_flag.or(generic).or(configured).cloned()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_plan_providers(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    mode: PlanMode,
    planner_provider: Option<String>,
    provider: Option<String>,
    coder_provider: Option<String>,
    reviewer_provider: Option<String>,
    models: PlanModelOverrides,
) -> Result<PlanProviders> {
    let default_child = resolve_provider_name(
        paths,
        setup::SetupProviderRoleRef::DefaultChild,
        provider.or(defaults.provider.clone()),
    )?;
    let planner = match mode {
        PlanMode::FullPlan => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Planner,
            planner_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::Review => None,
    };
    let coder = match mode {
        PlanMode::Review => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Coder,
            coder_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::FullPlan => None,
    };
    let reviewer = match mode {
        PlanMode::Review => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Reviewer,
            reviewer_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::FullPlan => None,
    };
    let configured_model = defaults.model.as_ref();
    Ok(PlanProviders {
        planner_model: planner.as_ref().and(resolve_role_model(
            models.planner_model.as_ref(),
            models.model.as_ref(),
            configured_model,
        )),
        default_child_model: resolve_role_model(None, models.model.as_ref(), configured_model),
        coder_model: coder.as_ref().and(resolve_role_model(
            models.coder_model.as_ref(),
            models.model.as_ref(),
            configured_model,
        )),
        reviewer_model: reviewer.as_ref().and(resolve_role_model(
            models.reviewer_model.as_ref(),
            models.model.as_ref(),
            configured_model,
        )),
        child_models: models.child_models,
        planner,
        default_child,
        coder,
        reviewer,
        children: BTreeMap::new(),
    })
}

pub(crate) fn resolve_provider_name(
    paths: &DeadreckonPaths,
    role: setup::SetupProviderRoleRef,
    provider: Option<String>,
) -> Result<Option<String>> {
    if provider
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        return Ok(provider);
    }
    let selection = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role,
            explicit_provider: provider.as_deref(),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: true,
            allow_auto_subscription: false,
            require_usable_route: false,
        },
    )?;
    Ok(selection.provider.or(provider))
}

/// `IDX=MODEL` pairs, exactly like `--child-provider`'s parser.
pub(crate) fn parse_child_model_overrides(
    values: &[String],
    n: u8,
) -> Result<std::collections::BTreeMap<u32, String>> {
    parse_child_provider_overrides(values, n)
}

pub(crate) fn parse_child_provider_overrides(
    values: &[String],
    n: u8,
) -> Result<BTreeMap<u32, String>> {
    let mut overrides = BTreeMap::new();
    for value in values {
        let Some((idx, provider)) = value.split_once('=') else {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("child provider override must be IDX=PROVIDER: {value}"),
                "--child-provider 1=cli:codex",
            )));
        };
        let index = idx.trim().parse::<u32>().map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                &format!("child provider index is not a number: {idx}"),
                "--child-provider 1=cli:codex",
            ))
        })?;
        if index >= u32::from(n) {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("child provider index {index} outside 0..{n}"),
                "--child-provider 1=cli:codex",
            )));
        }
        let provider = provider.trim();
        if provider.is_empty() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "child provider must be non-empty",
                "--child-provider 1=cli:codex",
            )));
        }
        overrides.insert(index, provider.to_string());
    }
    Ok(overrides)
}

/// Turn the launch classifier's graph into plan tasks.
///
/// This is what makes the classifier's answer the plan. Without it the shape
/// decision came from one planner and the executed child graph from a second,
/// so a goal the classifier read as ordered could still run as N independent
/// nodes — the preview and the execution disagreeing about the same launch.
///
/// Returns `None` when there is no usable seed, so plan creation falls back to
/// its own planner. The count must match `n` exactly: `n` is the number the
/// operator saw and confirmed on the preview, and silently planning a
/// different number would make the confirmation meaningless.
fn plan_tasks_from_seed(
    seed: &[commands::course::CoursePiece],
    n: u8,
    providers: &PlanProviders,
    overrides: &BTreeMap<u32, String>,
) -> Option<Vec<PlanTask>> {
    if seed.len() != usize::from(n) {
        return None;
    }
    if seed.iter().any(|piece| piece.goal.trim().is_empty()) {
        return None;
    }
    // Piece ids are positional (p1, p2, ...), so an edge maps to the task at
    // that position. Anything unresolvable is dropped rather than guessed.
    let positions: BTreeMap<&str, String> = seed
        .iter()
        .enumerate()
        .map(|(index, piece)| (piece.id.as_str(), format!("task-{index}")))
        .collect();
    let mut tasks = Vec::new();
    for (index, piece) in seed.iter().enumerate() {
        let task_index = index as u32;
        let provider = overrides
            .get(&task_index)
            .cloned()
            .or_else(|| providers.default_child.clone());
        let subject = commands::course::piece_subject(piece);
        let mut task = PlanTask::new(
            task_index,
            subject,
            piece.goal.trim(),
            PlanRole::Child,
            provider,
        );
        task.depends_on = piece
            .depends_on
            .iter()
            .filter_map(|dependency| positions.get(dependency.as_str()).cloned())
            .filter(|dependency| dependency != &task.task_id)
            .collect();
        tasks.push(task);
    }
    // A seed that does not form a valid DAG is discarded wholesale rather than
    // repaired; the planner fallback is the safer answer.
    deadreckon_core::validate_task_graph(&tasks).ok()?;
    Some(tasks)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_full_plan_tasks(
    paths: &DeadreckonPaths,
    goal: &str,
    n: u8,
    providers: &PlanProviders,
    overrides: &BTreeMap<u32, String>,
    cwd: &Path,
    plain: bool,
    no_hints: bool,
    json_output: bool,
) -> Result<Vec<PlanTask>> {
    let drafts = if providers
        .planner
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        deterministic_plan_drafts(goal, n)
    } else {
        provider_plan_drafts(paths, goal, n, providers.planner.as_deref(), cwd, plain).await?
    };
    if drafts.len() != usize::from(n) {
        return Err(plan_refusal_error(
            format!("provider returned {} children; need {n}", drafts.len()),
            "DeadReckon did not save the plan because the planner returned the wrong number of child tasks.",
            "The requested child count is part of the orchestration contract; saving a partial graph would make later fork/merge state misleading.",
            vec![
                ("requested children".to_string(), n.to_string()),
                ("returned children".to_string(), drafts.len().to_string()),
                (
                    "planner".to_string(),
                    providers
                        .planner
                        .as_deref()
                        .unwrap_or("default planner")
                        .to_string(),
                ),
            ],
            "deadreckon plan ... --provider <other>".to_string(),
            vec!["deadreckon providers list --all".to_string()],
            no_hints,
            json_output,
        ));
    }
    let mut tasks = Vec::new();
    for (index, draft) in drafts.into_iter().enumerate() {
        let index = index as u32;
        let provider = overrides
            .get(&index)
            .cloned()
            .or_else(|| providers.default_child.clone());
        let mut task = PlanTask::new(index, draft.subject, draft.goal, PlanRole::Child, provider);
        task.active_form = draft.active_form.unwrap_or_else(|| task.subject.clone());
        task.depends_on = draft.depends_on;
        tasks.push(task);
    }
    Ok(tasks)
}

fn validate_plan_task_count(goal: &str, n: u8, no_hints: bool, json_output: bool) -> Result<()> {
    match usize::from(n) {
        0 | 1 => Err(plan_refusal_error(
            "plan must have >= 2 children",
            "DeadReckon did not create a plan because the requested child count is too small.",
            "A full plan needs at least two child tasks; for one task, a direct run is the truthful execution shape.",
            vec![("requested children".to_string(), n.to_string())],
            "deadreckon run \"<the only child>\"".to_string(),
            vec![format!(
                "deadreckon plan {} --n 2",
                plan_goal_argument(goal)
            )],
            no_hints,
            json_output,
        )),
        2..=6 => Ok(()),
        count => Err(plan_refusal_error(
            format!("plan capped at 6 children; got {count}"),
            "DeadReckon did not create a plan because the requested child count is above the full-plan cap.",
            "Full-plan orchestration is capped at six parallel children; a sequential chain is the safer shape for larger decompositions.",
            vec![("requested children".to_string(), n.to_string())],
            format!("deadreckon chain plan {} --n {n}", plan_goal_argument(goal)),
            vec![format!(
                "deadreckon plan {} --n 6",
                plan_goal_argument(goal)
            )],
            no_hints,
            json_output,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_refusal_error(
    message: impl Into<String>,
    what_happened: impl Into<String>,
    why_this_verdict: impl Into<String>,
    evidence: Vec<(String, String)>,
    primary: String,
    secondary: Vec<String>,
    no_hints: bool,
    json_output: bool,
) -> CliError {
    let message = message.into();
    let mut all_evidence = vec![("reason".to_string(), message)];
    all_evidence.extend(evidence);
    let secondary = secondary
        .into_iter()
        .map(|command| ("Secondary", command))
        .collect::<Vec<_>>();
    let surface = VerdictSurface::must_new(
        VerdictKind::Blocked,
        "plan",
        None,
        ExplanationPanel::new(what_happened, why_this_verdict, all_evidence),
        vec![("Recommended", primary)],
        secondary,
    );
    let rendered = if json_output {
        let payload = surface.add_to_json(json!({
            "kind": "plan_refusal",
            "error": surface.explanation.evidence[0].1.clone(),
            "next_actions": [surface.primary_action.command.clone()],
        }));
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
    } else {
        surface.render_plain(!completion_hints_enabled(no_hints))
    };
    CliError::Surface {
        code: 1,
        surface: rendered,
    }
}

fn plan_goal_argument(goal: &str) -> String {
    format!("\"{}\"", shell_display_quote(goal))
}

fn build_review_plan_tasks(goal: &str, providers: &PlanProviders) -> Vec<PlanTask> {
    let mut coder = PlanTask::new(
        0,
        "Implement requested change",
        goal,
        PlanRole::Coder,
        providers.coder.clone(),
    );
    coder.active_form = "Coding implementation".to_string();
    let mut reviewer = PlanTask::new(
        1,
        "Review and fix implementation",
        format!(
            "Review the completed implementation for: {goal}. Write .deadreckon/REVIEW.md first, then apply only fixes tied to findings and acceptance."
        ),
        PlanRole::Reviewer,
        providers.reviewer.clone(),
    );
    reviewer.active_form = "Reviewing implementation".to_string();
    reviewer.depends_on = vec![coder.task_id.clone()];
    vec![coder, reviewer]
}

#[derive(Debug, Deserialize)]
struct PlannerDraft {
    subject: String,
    goal: String,
    #[serde(default)]
    active_form: Option<String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PlannerObjectDraft {
    tasks: Vec<PlannerDraft>,
}

async fn provider_plan_drafts(
    paths: &DeadreckonPaths,
    goal: &str,
    n: u8,
    planner_provider: Option<&str>,
    cwd: &Path,
    plain: bool,
) -> Result<Vec<PlannerDraft>> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), planner_provider)?;
    let prompt = planner_prompt(goal, n);
    let request = ProviderRequest {
        prompt,
        max_output_tokens: 4096,
        cwd: Some(cwd.to_path_buf()),
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
        session_dir: None,
        output_schema: None,
        capability_posture: None,
    };
    let response =
        maybe_with_cli_wait_status(!plain, "planning child graph", router.complete(&request))
            .await?;
    parse_planner_response(&response.content)
}

fn planner_prompt(goal: &str, n: u8) -> String {
    format!(
        "You are a read-only planning agent for deadreckon. Do not write files, create temporary files, install packages, commit, delete, move, or mutate state. Inspect only if your provider supports read-only tools.\n\nReturn JSON only. Shape: {{\"tasks\":[{{\"subject\":\"imperative label\",\"goal\":\"self-contained child goal\",\"active_form\":\"present-progress text\",\"depends_on\":[\"task-0\"]}}]}}. Return exactly {n} child entries in the tasks array. Dependencies must refer to earlier child ids task-0..task-{} and form a DAG.\n\nChild hygiene:\n- Prefer child ids in execution order; earlier children should unblock later children.\n- For build/product goals, child goals must be implementation or verification slices that create or edit project files and move toward runnable behavior.\n- Do not return research-only, sourcing-only, architecture-only, or roadmap-only children unless the user explicitly asked for planning or research documentation.\n- Split independent implementation work into separate children; use research only as a dependency that directly unblocks concrete implementation.\n- Give each child enough context to run without seeing the user conversation, including likely files/modules/features and acceptance checks.\n- Never write \"based on the other worker\"; include the concrete dependency output the child will need.\n\nGoal: {goal}",
        n.saturating_sub(1)
    )
}

fn parse_planner_response(content: &str) -> Result<Vec<PlannerDraft>> {
    // Providers rarely return bare JSON: cli:claude-code (and others) wrap the
    // object in a ```json fence and often add prose around it. Try candidates in
    // robustness order — the fenced block first (immune to surrounding prose),
    // then the fence-stripped body, then the raw content, then the brace/bracket
    // slice as a last resort — so a chatty answer no longer fails the plan.
    for candidate in planner_json_candidates(content) {
        if let Ok(object) = serde_json::from_str::<PlannerObjectDraft>(&candidate) {
            return Ok(object.tasks);
        }
        if let Ok(tasks) = serde_json::from_str::<Vec<PlannerDraft>>(&candidate) {
            return Ok(tasks);
        }
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "planner provider did not return a valid child JSON object",
        "deadreckon plan ... --planner-provider <other>",
    )))
}

/// Ordered JSON-bearing candidate strings from a provider's free-text answer.
/// Reuses the same fence-aware extraction the acceptance-draft parser uses, so
/// the planner is no longer defeated by a ```json block wrapped in prose.
fn planner_json_candidates(content: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(block) = crate::commands::acceptance::extract_fenced_block(content, &["json"]) {
        candidates.push(block);
    }
    let stripped = crate::commands::acceptance::strip_code_fence(content.trim());
    if !stripped.is_empty() {
        candidates.push(stripped);
    }
    candidates.push(content.to_string());
    if let Some(slice) = json_slice(content, '{', '}') {
        candidates.push(slice.to_string());
    }
    if let Some(slice) = json_slice(content, '[', ']') {
        candidates.push(slice.to_string());
    }
    candidates
}

pub(crate) fn json_slice(content: &str, open: char, close: char) -> Option<&str> {
    let start = content.find(open)?;
    let end = content.rfind(close)?;
    (end >= start).then_some(&content[start..=end])
}

fn deterministic_plan_drafts(goal: &str, n: u8) -> Vec<PlannerDraft> {
    (0..n)
        .map(|index| PlannerDraft {
            subject: match index {
                0 => "Create foundation".to_string(),
                1 => "Add behavior".to_string(),
                _ => format!("Complete slice {}", index + 1),
            },
            goal: format!("{goal} (child {} of {n})", index + 1),
            active_form: Some(match index {
                0 => "Creating foundation".to_string(),
                1 => "Adding behavior".to_string(),
                _ => format!("Completing slice {}", index + 1),
            }),
            depends_on: Vec::new(),
        })
        .collect()
}

fn infer_capability_preview(goal: &str) -> deadreckon_core::CapabilityPreview {
    let lower = goal.to_ascii_lowercase();
    let deploy = ["deploy", "vercel", "netlify", "production"]
        .iter()
        .any(|needle| lower.contains(needle));
    let global_install = ["install globally", "global install", "npm -g"]
        .iter()
        .any(|needle| lower.contains(needle));
    let networked = [
        "api",
        "websocket",
        "web socket",
        "multiplayer",
        "online",
        "networked",
        "real-time",
        "real time",
        "realtime",
        "live",
        "server",
        "client/server",
        "asset source",
        "asset sourcing",
        "terrain data",
        "mapbox",
        "cesium",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let network = if deploy || networked {
        deadreckon_core::NetworkCapability::Allowlist
    } else {
        deadreckon_core::NetworkCapability::Deny
    };
    let mut notes = Vec::new();
    if deploy {
        notes.push(
            "goal mentions deployment; require explicit capability before deploy".to_string(),
        );
    }
    if global_install {
        notes.push("goal mentions global install; require explicit capability".to_string());
    }
    deadreckon_core::CapabilityPreview {
        network,
        deploy,
        global_install,
        filesystem: vec!["working directory".to_string()],
        notes,
    }
}

fn render_worker_spec(plan: &Plan, task: &PlanTask) -> String {
    let dependencies = if task.depends_on.is_empty() {
        "none".to_string()
    } else {
        task.depends_on.join(", ")
    };
    let acceptance_line = plan
        .acceptance_path
        .as_ref()
        .map(|path| {
            format!(
                "- dr-gate will enforce configured done criteria from {}.",
                path.display()
            )
        })
        .unwrap_or_else(|| {
            "- dr-gate will use the default local gate if no project done criteria exist."
                .to_string()
        });
    format!(
        "# deadreckon worker spec: {}\n\nRoot goal: {}\nChild id: {}\nRole: {:?}\nProvider: {}\nDependencies satisfied before start: {}\n\n## Scope\n{}\n\n## Capability constraints\n- network: {:?}\n- deploy: {}\n- global install: {}\n- filesystem: {}\n\n## Coordination rules\n- Treat this file as the complete brief; do not assume access to the parent conversation.\n- Do not inspect, tail, or summarize sibling child transcripts; wait for dependency summaries included below.\n- If correcting your own failed check, keep the same context and fix the root cause.\n- If acting as reviewer, approach the artifact with fresh assumptions and verify independently.\n- Report blockers as concrete file paths, command output, or acceptance failures.\n\n## Done criteria\n{}\n- Stay within this child's scope.\n- Verify relevant behavior before reporting done.\n- Do not spawn subagents or orchestrate more children.\n- Do not editorialize between tool calls.\n- Report scope, result, key files, files changed, and issues.\n",
        task.subject,
        plan.root_goal,
        task.task_id,
        task.role,
        task.provider.as_deref().unwrap_or("config default"),
        dependencies,
        task.goal,
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install,
        plan.capability_preview.filesystem.join(", "),
        acceptance_line
    )
}

fn render_launch_worker_spec(paths: &DeadreckonPaths, plan: &Plan, task: &PlanTask) -> String {
    let mut spec = render_worker_spec(plan, task);
    let dependency_summaries = task
        .depends_on
        .iter()
        .filter_map(|dependency| plan.task_by_id(dependency))
        .filter_map(|dependency| {
            let summary_path = dependency.summary_path.as_ref()?;
            let absolute = paths.plan_dir(&plan.plan_id).join(summary_path);
            let raw = fs::read_to_string(&absolute).ok()?;
            Some((dependency, absolute, truncate_for_worker_spec(&raw)))
        })
        .collect::<Vec<_>>();
    if dependency_summaries.is_empty() {
        return spec;
    }
    spec.push_str("\n## Dependency summaries\n");
    for (dependency, absolute, summary) in dependency_summaries {
        spec.push_str(&format!(
            "\n### {} - {}\n\nSummary path: {}\n\n{}\n",
            dependency.task_id,
            dependency.subject,
            absolute.display(),
            summary.trim()
        ));
    }
    spec
}

fn truncate_for_worker_spec(raw: &str) -> String {
    const MAX_CHARS: usize = 4_000;
    let mut chars = raw.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n\n... truncated ...")
    } else {
        truncated
    }
}

fn plan_source_label(plan: &Plan) -> String {
    let Some(cwd) = plan.parent_cwd.as_ref() else {
        return "current directory at fork time".to_string();
    };
    let git_label = match preview_git_state(cwd) {
        Ok(Some(git)) => format!("git branch={} @ {}", git.branch, git.head_sha),
        _ => "not git; child runs use copy mode".to_string(),
    };
    format!("{} ({git_label})", cwd.display())
}

fn plan_acceptance_label(plan: &Plan) -> String {
    let Some(path) = plan.acceptance_path.as_ref() else {
        return setup::DoneCriteriaSelection::default_gate().full_label();
    };
    let checks = fs::read_to_string(path)
        .ok()
        .and_then(|raw| commands::acceptance::acceptance_check_count(&raw).ok());
    setup::DoneCriteriaSelection::project(path.clone(), None, checks).full_label()
}

pub(crate) fn plan_next_actions(plan: &Plan) -> Vec<String> {
    let id = run_prefix(&plan.plan_id);
    match plan.status {
        PlanStatus::Pending => vec![format!("deadreckon fork {id}")],
        PlanStatus::Forked => {
            if plan
                .tasks
                .iter()
                .all(|task| task.status == PlanTaskStatus::Completed)
            {
                vec![format!("deadreckon merge {id}")]
            } else {
                vec![format!("deadreckon attach {id}")]
            }
        }
        PlanStatus::Merged => vec![format!("deadreckon finish {id}")],
        PlanStatus::Failed => vec![format!("deadreckon show {id} --why-failed")],
    }
}

pub(crate) fn plan_next_actions_with_context(paths: &DeadreckonPaths, plan: &Plan) -> Vec<String> {
    let id = run_prefix(&plan.plan_id);
    match plan.status {
        PlanStatus::Failed if plan_has_repair_evidence(paths, plan) => vec![
            format!("deadreckon merge {id}"),
            format!("deadreckon show {id} --why-failed"),
        ],
        _ => plan_next_actions(plan),
    }
}

pub(crate) fn plan_verdict_surface(paths: &DeadreckonPaths, plan: &Plan) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let total = plan.tasks.len();
    let repairable = plan_has_repair_evidence(paths, plan);
    let (kind, what, why) = match plan.status {
        PlanStatus::Pending => (
            VerdictKind::Preview,
            "DeadReckon wrote the plan graph, but no child run has started yet.",
            "This is a pre-fork plan state; forking is the one command that advances it.",
        ),
        PlanStatus::Forked
            if plan
                .tasks
                .iter()
                .all(|task| task.status == PlanTaskStatus::Completed) =>
        {
            (
                VerdictKind::Completed,
                "All child runs completed and the plan is ready to merge.",
                "The next state-changing command composes the child artifacts into one result.",
            )
        }
        PlanStatus::Forked => (
            VerdictKind::Paused,
            "The plan has launched child work and is not merged yet.",
            "Attaching is the safest next command because the plan still has active or pending child state.",
        ),
        PlanStatus::Merged => (
            VerdictKind::Completed,
            "The plan merged its child results into a promoted result run.",
            "DeadReckon has a merged artifact; the recommended command lands or exports that result.",
        ),
        PlanStatus::Failed if repairable => (
            VerdictKind::Failed,
            "The plan stopped before producing a merged result.",
            "Merge-repair evidence exists, so rerunning merge is the safest recovery path before inspection-only commands.",
        ),
        PlanStatus::Failed => (
            VerdictKind::Failed,
            "The plan stopped before producing a merged result.",
            "No repair evidence is available, so the why-failed view is the safest next inspection command.",
        ),
    };
    let mut evidence = vec![
        ("plan".to_string(), id.clone()),
        (
            "status".to_string(),
            plan_status_label(plan.status).to_string(),
        ),
        (
            "tasks".to_string(),
            format!("{completed}/{total} completed"),
        ),
        (
            "events".to_string(),
            paths.plan_events(&plan.plan_id).display().to_string(),
        ),
    ];
    if let Some(merged_run_id) = plan.merged_run_id.as_deref() {
        evidence.push(("result run".to_string(), run_prefix(merged_run_id)));
    }
    if repairable {
        evidence.push((
            "repair evidence".to_string(),
            paths.merge_proofs(&plan.plan_id).display().to_string(),
        ));
    }
    let primary = plan_next_actions_with_context(paths, plan)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("deadreckon show {id}"));
    let secondary = plan_secondary_actions(paths, plan, &primary);
    VerdictSurface::must_new(
        kind,
        "plan",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str())),
    )
}

fn plan_secondary_actions(paths: &DeadreckonPaths, plan: &Plan, primary: &str) -> Vec<String> {
    let id = run_prefix(&plan.plan_id);
    let mut actions = Vec::new();
    for command in plan_next_actions_with_context(paths, plan) {
        if command != primary && !actions.contains(&command) {
            actions.push(command);
        }
    }
    let inspection = if plan.status == PlanStatus::Failed {
        format!("deadreckon show {id} --why-failed")
    } else {
        format!("deadreckon show {id}")
    };
    for command in [format!("deadreckon attach {id}"), inspection] {
        if command != primary && !actions.contains(&command) {
            actions.push(command);
        }
    }
    actions
}

fn plan_has_repair_evidence(paths: &DeadreckonPaths, plan: &Plan) -> bool {
    let proofs = paths.merge_proofs(&plan.plan_id);
    [
        "conflicts.json",
        "repair-request.json",
        "repair-plan.json",
        "repair-run.json",
    ]
    .iter()
    .any(|name| proofs.join(name).is_file())
}

pub(crate) fn plan_paths_json(plan: &Plan) -> Value {
    let paths = DeadreckonPaths::discover();
    json!({
        "plan": paths.plan_json(&plan.plan_id),
        "events": paths.plan_events(&plan.plan_id),
        "directory": paths.plan_dir(&plan.plan_id),
    })
}

fn print_plan_json(plan: &Plan) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let value = plan_verdict_surface(&paths, plan).add_to_json(json!({
        "kind": "plan",
        "id": &plan.plan_id,
        "status": plan_status_label(plan.status),
        "next_actions": plan_next_actions_with_context(&paths, plan),
        "try_lines": Vec::<String>::new(),
        "paths": plan_paths_json(plan),
        "plan": plan,
    }));
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestrationRoleRow {
    pub(crate) role: String,
    pub(crate) route: String,
    pub(crate) model: String,
    pub(crate) source: String,
    pub(crate) notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestrationDependencyRow {
    pub(crate) child: String,
    pub(crate) status: String,
    pub(crate) starts: String,
    pub(crate) waits_for: String,
    pub(crate) unblocks: String,
}

pub(crate) fn orchestration_provider_role_rows(
    plan: &Plan,
    repair_enabled: bool,
    repair_provider: Option<&str>,
) -> Vec<OrchestrationRoleRow> {
    let mut rows = Vec::new();
    match plan.mode {
        PlanMode::FullPlan => {
            rows.push(orchestration_role_row(
                "planner",
                plan.providers.planner.as_deref(),
                plan.providers.planner_model.as_deref(),
                "plan",
                "writes child graph",
            ));
            rows.push(orchestration_role_row(
                "default child",
                plan.providers.default_child.as_deref(),
                plan.providers.default_child_model.as_deref(),
                "plan",
                "runs ready children",
            ));
            let mut seen_overrides = BTreeSet::new();
            let mut override_indexes: BTreeSet<u32> =
                plan.providers.children.keys().copied().collect();
            override_indexes.extend(plan.providers.child_models.keys().copied());
            for index in override_indexes {
                seen_overrides.insert(index);
                let route = plan.providers.children.get(&index);
                rows.push(orchestration_role_row(
                    format!("child task-{index}"),
                    route.map(String::as_str),
                    plan.providers
                        .child_models
                        .get(&index)
                        .or(plan.providers.default_child_model.as_ref())
                        .map(String::as_str),
                    "override",
                    "per-child route",
                ));
            }
            for task in &plan.tasks {
                let default_route = plan.providers.default_child.as_deref();
                if task.provider.as_deref().is_some()
                    && task.provider.as_deref() != default_route
                    && !seen_overrides.contains(&task.index)
                {
                    rows.push(orchestration_role_row(
                        format!("child {}", task.task_id),
                        task.provider.as_deref(),
                        plan.providers
                            .child_models
                            .get(&task.index)
                            .or(plan.providers.default_child_model.as_ref())
                            .map(String::as_str),
                        "task",
                        one_line(&task.subject, 30),
                    ));
                }
            }
        }
        PlanMode::Review => {
            rows.push(orchestration_role_row(
                "coder",
                plan.providers.coder.as_deref(),
                plan.providers.coder_model.as_deref(),
                "plan",
                "implementation pass",
            ));
            rows.push(orchestration_role_row(
                "reviewer",
                plan.providers.reviewer.as_deref(),
                plan.providers.reviewer_model.as_deref(),
                "plan",
                "independent review",
            ));
        }
    }
    if repair_enabled {
        let derived = repair_provider
            .or(plan.providers.planner.as_deref())
            .or(plan.providers.default_child.as_deref())
            .or(plan.providers.reviewer.as_deref())
            .or(plan.providers.coder.as_deref());
        rows.push(orchestration_role_row(
            "repair",
            derived,
            plan.providers.planner_model.as_deref(),
            if repair_provider.is_some() {
                "flag"
            } else {
                "derived"
            },
            "merge repair planning",
        ));
    } else {
        rows.push(OrchestrationRoleRow {
            role: "repair".to_string(),
            route: "disabled".to_string(),
            model: "-".to_string(),
            source: "--no-repair".to_string(),
            notes: "raw conflict refusal".to_string(),
        });
    }
    rows
}

/// The model a child run launches with: per-index override, else the
/// default-child model. None (or the literal "provider default") means the
/// provider's own default — no --model argument reaches the child.
pub(crate) fn child_model_for_task<'a>(
    providers: &'a PlanProviders,
    task: &deadreckon_core::plan::PlanTask,
) -> Option<&'a str> {
    providers
        .child_models
        .get(&task.index)
        .or(providers.default_child_model.as_ref())
        .map(String::as_str)
        .filter(|model| *model != "provider default" && !model.trim().is_empty())
}

fn orchestration_role_row(
    role: impl Into<String>,
    route: Option<&str>,
    model: Option<&str>,
    source: impl Into<String>,
    notes: impl Into<String>,
) -> OrchestrationRoleRow {
    OrchestrationRoleRow {
        role: role.into(),
        route: route.unwrap_or("config default").to_string(),
        model: model.unwrap_or("-").to_string(),
        source: if route.is_some() {
            source.into()
        } else {
            "config".to_string()
        },
        notes: notes.into(),
    }
}

pub(crate) fn orchestration_role_table_lines(rows: &[OrchestrationRoleRow]) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} {} {} {}",
        ui::pad_visible(&ui_muted("role"), 14),
        ui::pad_visible(&ui_muted("route"), 22),
        ui::pad_visible(&ui_muted("model"), 8),
        ui::pad_visible(&ui_muted("source"), 12),
        ui_muted("notes")
    )];
    lines.extend(rows.iter().map(|row| {
        format!(
            "{:<14} {:<22} {:<8} {:<12} {}",
            row.role, row.route, row.model, row.source, row.notes
        )
    }));
    lines
}

pub(crate) fn print_orchestration_role_table(
    plan: &Plan,
    repair_enabled: bool,
    repair_provider: Option<&str>,
) {
    println!("{}", ui_heading("provider roles"));
    for line in orchestration_role_table_lines(&orchestration_provider_role_rows(
        plan,
        repair_enabled,
        repair_provider,
    )) {
        println!("  {line}");
    }
}

pub(crate) fn orchestration_dependency_rows(plan: &Plan) -> Vec<OrchestrationDependencyRow> {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .map(|task| task.task_id.as_str())
        .collect::<BTreeSet<_>>();
    plan.tasks
        .iter()
        .map(|task| {
            let blockers = task
                .depends_on
                .iter()
                .filter(|dependency| !completed.contains(dependency.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let starts = match task.status {
                PlanTaskStatus::Pending if blockers.is_empty() => "now".to_string(),
                PlanTaskStatus::Pending => format!("after {}", blockers.join(",")),
                PlanTaskStatus::Running => "already running".to_string(),
                PlanTaskStatus::Completed => "done".to_string(),
                PlanTaskStatus::Failed => "failed".to_string(),
                PlanTaskStatus::Killed => "killed".to_string(),
            };
            let unblocks = plan
                .tasks
                .iter()
                .filter(|candidate| candidate.depends_on.iter().any(|dep| dep == &task.task_id))
                .map(|candidate| candidate.task_id.clone())
                .collect::<Vec<_>>();
            OrchestrationDependencyRow {
                child: task.task_id.clone(),
                status: task_status_label(task.status).to_string(),
                starts,
                waits_for: if blockers.is_empty() {
                    "-".to_string()
                } else {
                    blockers.join(",")
                },
                unblocks: if unblocks.is_empty() {
                    "-".to_string()
                } else {
                    unblocks.join(",")
                },
            }
        })
        .collect()
}

pub(crate) fn orchestration_parallelism_lines(plan: &Plan) -> Vec<String> {
    let rows = orchestration_dependency_rows(plan);
    let starts_now = rows
        .iter()
        .filter(|row| row.starts == "now")
        .map(|row| row.child.clone())
        .collect::<Vec<_>>();
    let waits = rows
        .iter()
        .filter(|row| row.waits_for != "-")
        .map(|row| format!("{} after {}", row.child, row.waits_for))
        .collect::<Vec<_>>();
    vec![
        format!(
            "starts now: {}",
            if starts_now.is_empty() {
                "-".to_string()
            } else {
                starts_now.join(", ")
            }
        ),
        format!(
            "waits: {}",
            if waits.is_empty() {
                "-".to_string()
            } else {
                waits.join("; ")
            }
        ),
    ]
}

pub(crate) fn print_orchestration_dependency_summary(plan: &Plan) {
    println!("{}", ui_heading("parallelism"));
    for line in orchestration_parallelism_lines(plan) {
        println!("  {line}");
    }
    println!("{}", ui_heading("dependencies"));
    println!(
        "  {} {} {} {} {}",
        ui::pad_visible(&ui_muted("child"), 10),
        ui::pad_visible(&ui_muted("status"), 10),
        ui::pad_visible(&ui_muted("starts"), 18),
        ui::pad_visible(&ui_muted("waits_for"), 18),
        ui_muted("unblocks")
    );
    for row in orchestration_dependency_rows(plan) {
        println!(
            "  {} {} {:<18} {:<18} {}",
            ui::pad_visible(&ui_id(&row.child), 10),
            ui::pad_visible(&ui_status(&row.status), 10),
            row.starts,
            row.waits_for,
            row.unblocks
        );
    }
}

fn print_plan_created(plan: &Plan, no_hints: bool) {
    let paths = DeadreckonPaths::discover();
    println!(
        "{}",
        plan_verdict_surface(&paths, plan).render_plain(!completion_hints_enabled(no_hints))
    );
    let ready = plan.ready_pending_task_indices().len();
    let pending = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Pending)
        .count();
    let blocked = pending.saturating_sub(ready);
    let children = format!(
        "{} ({} ready / {} blocked)",
        plan.tasks.len(),
        ready,
        blocked
    );
    let providers = match plan.mode {
        PlanMode::FullPlan => format!(
            "planner={} default-child={}",
            plan.providers.planner.as_deref().unwrap_or("-"),
            plan.providers.default_child.as_deref().unwrap_or("-")
        ),
        PlanMode::Review => format!(
            "coder={} reviewer={}",
            plan.providers.coder.as_deref().unwrap_or("-"),
            plan.providers.reviewer.as_deref().unwrap_or("-")
        ),
    };
    let capabilities = format!(
        "network={:?} deploy={} install={}",
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install
    );
    let source = plan_source_label(plan);
    let gate = plan_acceptance_label(plan);
    let plan_path = paths.plan_json(&plan.plan_id);
    let plan_path_display = plan_path.to_string_lossy().to_string();
    let items = [
        ("status", plan_status_label(plan.status)),
        ("mode", plan_mode_label(plan.mode)),
        ("children", children.as_str()),
        ("providers", providers.as_str()),
        ("source", source.as_str()),
        (NOUN_DONE_CONTRACT, gate.as_str()),
        ("capabilities", capabilities.as_str()),
        ("plan", plan_path_display.as_str()),
    ];
    print_kv_block(&items);
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            "-".to_string()
        } else {
            task.depends_on.join(",")
        };
        println!(
            "  {} {} [{}] provider={} deps={}",
            ui_id(&task.task_id),
            task.subject,
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-"),
            deps
        );
    }
}

pub(crate) fn print_orchestrate_preflight(
    plan: &Plan,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<&str>,
    no_repair: bool,
) {
    println!(
        "{} {} ({})",
        ui_heading("orchestrate preflight"),
        ui_id(run_prefix(&plan.plan_id)),
        plan.plan_id
    );
    let children = format!(
        "{} ({})",
        plan.tasks.len(),
        orchestration_mode_summary(plan)
    );
    let spend = max_spend
        .map(|value| format!("${value:.2} per child"))
        .unwrap_or_else(|| "config default".to_string());
    let wall = max_wall_seconds
        .map(|value| format!("{value:.0}s per child"))
        .unwrap_or_else(|| "config default".to_string());
    let sandbox = sandbox.unwrap_or("config default").to_string();
    let capabilities = format!(
        "network={:?} deploy={} install={}",
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install
    );
    let providers = plan_provider_summary(plan);
    let source = plan_source_label(plan);
    let gate = plan_acceptance_label(plan);
    let repair = plan_repair_label(plan, no_repair);
    let plan_ref = run_prefix(&plan.plan_id);
    let path = orchestration_path_label(plan.mode);
    let launch_rows = launch_preview_rows(&LaunchPreviewFacts {
        goal: &plan.root_goal,
        path,
        suggestion: None,
        provider: &providers,
        model: plan.providers.default_child_model.clone(),
        roles: None,
        base: None,
        history: None,
        done: &gate,
        workspace: &source,
        watch: format!("deadreckon attach {plan_ref}"),
        stop: format!("deadreckon kill {plan_ref}"),
        finish: format!("deadreckon finish {plan_ref}"),
        override_command: Some("deadreckon start <goal> --mode run".to_string()),
    });
    let mut items = vec![
        ("goal".to_string(), plan.root_goal.clone()),
        ("path".to_string(), path.to_string()),
        ("mode".to_string(), plan_mode_label(plan.mode).to_string()),
        ("children".to_string(), children),
        ("provider".to_string(), providers),
        ("workspace".to_string(), source),
        (NOUN_DONE_CONTRACT.to_string(), gate),
        ("merge repair".to_string(), repair),
        ("sandbox".to_string(), sandbox),
        ("spend".to_string(), spend),
        ("wall".to_string(), wall),
        ("capabilities".to_string(), capabilities),
    ];
    items.extend(
        launch_rows
            .into_iter()
            .filter(|(key, _)| matches!(key.as_str(), "watch" | "stop" | "finish" | "override")),
    );
    print_launch_preview_rows(&items);
    print_orchestration_role_table(plan, !no_repair, None);
    print_orchestration_dependency_summary(plan);
    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            "-".to_string()
        } else {
            task.depends_on.join(",")
        };
        println!(
            "  {} {} [{}] provider={} deps={}",
            ui_id(&task.task_id),
            task.subject,
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-"),
            deps
        );
    }
    let warnings = implementation_plan_warnings(plan);
    if !warnings.is_empty() {
        println!("{}", ui_warn("preflight warnings"));
        for warning in warnings {
            println!("  - {warning}");
        }
        println!(
            "{}",
            orchestrate_preflight_warning_recovery_line(&plan.plan_id)
        );
    }
    println!(
        "{} {}/plans/{}/plan.json",
        ui_command("plan:"),
        DeadreckonPaths::discover().home().display(),
        plan.plan_id
    );
}

fn orchestrate_preflight_warning_recovery_line(plan_id: &str) -> String {
    format!(
        "  {} {}",
        ui_command("inspect:"),
        ui_command(format!("deadreckon attach {} --plain", run_prefix(plan_id)))
    )
}

fn orchestration_path_label(mode: PlanMode) -> &'static str {
    match mode {
        PlanMode::Review => "review orchestration",
        PlanMode::FullPlan => "full-plan orchestration",
    }
}

/// Printed immediately after the preflight in the same invocation, so it
/// carries only the launch delta (durable artifact paths + watch handle) —
/// the shape, providers, contract, and dependency tables are already on
/// screen from `print_orchestrate_preflight`.
pub(crate) fn print_orchestrate_started(
    plan: &Plan,
    _max_spend: Option<f64>,
    _max_wall_seconds: Option<f64>,
    _sandbox: Option<&str>,
    _no_repair: bool,
) {
    println!(
        "{} {}",
        ui_ok("started orchestration"),
        ui_id(format!("{} ({})", run_prefix(&plan.plan_id), plan.plan_id))
    );
    let paths = DeadreckonPaths::discover();
    let children = plan.tasks.len().to_string();
    let plan_path_display = paths.plan_json(&plan.plan_id).to_string_lossy().to_string();
    let events_path_display = paths
        .plan_events(&plan.plan_id)
        .to_string_lossy()
        .to_string();
    let watch = format!("deadreckon attach {}", run_prefix(&plan.plan_id));
    let items = [
        ("children", children.as_str()),
        ("plan", plan_path_display.as_str()),
        ("events", events_path_display.as_str()),
        ("watch", watch.as_str()),
    ];
    print_kv_block(&items);
    let _ = io::stdout().flush();
}

pub(crate) fn implementation_plan_warnings(plan: &Plan) -> Vec<String> {
    if plan.mode != PlanMode::FullPlan || user_requested_planning(&plan.root_goal) {
        return Vec::new();
    }
    let weak_tasks = plan
        .tasks
        .iter()
        .filter(|task| task_looks_non_implementation(task))
        .map(|task| task.task_id.clone())
        .collect::<Vec<_>>();
    if weak_tasks.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "{} task(s) look research/design/roadmap-only for a build goal: {}. Preview/edit/re-plan before starting if these should build working software.",
        weak_tasks.len(),
        weak_tasks.join(", ")
    )]
}

fn user_requested_planning(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    ["research", "plan", "roadmap", "architecture", "design doc"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn task_looks_non_implementation(task: &PlanTask) -> bool {
    let text = format!("{} {}", task.subject, task.goal).to_ascii_lowercase();
    let planning_terms = [
        "research",
        "source ",
        "sourcing",
        "architecture",
        "design ",
        "roadmap",
        "document",
        "decision record",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    let implementation_terms = [
        "implement ",
        "build",
        "create",
        "add",
        "wire",
        "test",
        "verify",
        "fix",
        "scaffold",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    planning_terms && !implementation_terms
}

fn orchestration_mode_summary(plan: &Plan) -> &'static str {
    match plan.mode {
        PlanMode::FullPlan => "planner -> children -> merge -> final gate",
        PlanMode::Review => "coder -> reviewer/fixer -> final gate",
    }
}

pub(crate) fn confirm_orchestration_start(plan: &Plan, yes: bool, no_hints: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(orchestrate_confirmation_refusal_error(plan, no_hints));
    }
    if !prompt::confirm("start this orchestration?", true)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "orchestration cancelled by user".to_string(),
        )));
    }
    Ok(())
}

fn orchestrate_confirmation_refusal_error(plan: &Plan, no_hints: bool) -> CliError {
    let id = run_prefix(&plan.plan_id);
    let primary = format!("deadreckon fork {id}");
    let secondary = [
        format!("deadreckon show {id}"),
        format!("deadreckon kill {id}"),
    ];
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "orchestrate",
            Some(&id),
            ExplanationPanel::new(
                "DeadReckon wrote the orchestration preflight plan but did not launch child work because this shell cannot confirm the launch.",
                "Starting child agents mutates plan and run state; without an interactive confirmation or --yes, the saved pending plan must be advanced explicitly.",
                vec![
                    (
                        "reason".to_string(),
                        "non-interactive orchestrate requires --yes after reviewing preflight"
                            .to_string(),
                    ),
                    ("plan".to_string(), id.clone()),
                    ("status".to_string(), plan_status_label(plan.status).to_string()),
                    ("mode".to_string(), plan_mode_label(plan.mode).to_string()),
                    ("children".to_string(), plan.tasks.len().to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            secondary
                .iter()
                .map(|command| ("Secondary", command.as_str())),
        )
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}

/// Whether the orchestrate parent should print its own per-child aggregate
/// line. Suppressed when this orchestrate is itself a campaign sub-orchestrator
/// (`is_campaign_sub`) — the campaign parent owns the live surface there and
/// prints one consolidated line per sub-goal instead.
fn orchestrate_aggregate_enabled(narrate: bool, quiet: bool, is_campaign_sub: bool) -> bool {
    narrate && !quiet && !is_campaign_sub
}

/// Per-child snapshot tails the parent polls to render its live aggregate
/// stderr line (Option D1). Keyed by task index.
#[derive(Default)]
struct ParentAggregateState {
    tails: BTreeMap<usize, crate::plan_event_bus::JsonlTail<crate::narrative::NarrativeSnapshot>>,
    headlines: BTreeMap<usize, String>,
}

/// Tail each running child's `snapshots.jsonl` and refresh its latest headline.
fn refresh_parent_aggregate(
    paths: &DeadreckonPaths,
    plan: &Plan,
    state: &mut ParentAggregateState,
    running_indices: &[usize],
) {
    for &task_index in running_indices {
        let Some(run_id) = plan.tasks[task_index].child_run_id.clone() else {
            continue;
        };
        if let std::collections::btree_map::Entry::Vacant(slot) = state.tails.entry(task_index) {
            let Ok(run_state) = load_run(paths, &run_id) else {
                continue;
            };
            let path = crate::narrative::child_snapshots_path(&run_state.run_root);
            slot.insert(crate::plan_event_bus::JsonlTail::new(path));
        }
        if let Some(tail) = state.tails.get_mut(&task_index)
            && let Ok(rows) = tail.read_new()
            && let Some(headline) = crate::narrative::latest_headline_from(&rows)
        {
            state.headlines.insert(task_index, headline);
        }
    }
}

pub(crate) async fn fork_command(args: ForkCommandArgs) -> Result<()> {
    let ForkCommandArgs {
        plan_id,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        child_provider,
        coder_provider,
        reviewer_provider,
        no_repair,
        repair_provider,
        no_hints,
        quiet,
        plain,
        completion_surface,
        narrate,
        narrator_model,
    } = args;
    let paths = DeadreckonPaths::discover();
    let resolved_id = resolve_plan_id(&paths, &plan_id)?;
    let mut plan = load_plan(&paths, &resolved_id)?;
    if plan.status != PlanStatus::Pending {
        return Err(CliError::Surface {
            code: 1,
            surface: fork_refusal_surface(&paths, &plan)
                .render_plain(!completion_hints_enabled(no_hints)),
        });
    }
    apply_fork_provider_overrides(
        &mut plan,
        provider,
        &child_provider,
        coder_provider,
        reviewer_provider,
    )?;
    let defaults = config_defaults(&paths)?;
    let sandbox = sandbox
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    let parent_cwd = std::env::current_dir()?;

    plan.status = PlanStatus::Forked;
    plan.forked_at = Some(Utc::now());
    save_plan(&paths, &plan)?;
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)?;
    write_coordinator_snapshot(&paths, &plan, None)?;

    // Self-healing bookkeeping. A node that misses its done contract is put
    // back to Pending with its failure recorded, so the loop below picks it up
    // again and `child_argv` extends the failed run instead of restarting it.
    // `halt` is set when the failure policy is Stop or the breaker trips —
    // in-flight children still finish and are recorded, but nothing new starts.
    let mut consecutive_failures: u32 = 0;
    let mut halt: Option<String> = None;

    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        let mut ready = if halt.is_some() {
            Vec::new()
        } else {
            plan.ready_pending_task_indices()
        };
        // Per-node apply lands each node on the branch before the next starts,
        // so the next node's source tree contains it. Running siblings in
        // parallel would race on that same base, so this shape is serial by
        // construction — one ready node per pass.
        if plan.apply == deadreckon_core::plan::ApplyWhen::PerNode {
            ready.truncate(1);
        }
        let ready = ready;
        if !ready.is_empty() {
            made_progress = true;
        }
        for &task_index in &ready {
            let task_id = plan.tasks[task_index].task_id.clone();
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::TaskReady {
                    task_id: task_id.clone(),
                    task_index,
                },
            )?;
            mark_plan_task_status(&mut plan, task_index, PlanTaskStatus::Running)?;
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::TaskStarted {
                    task_id: task_id.clone(),
                    task_index,
                },
            )?;
            append_plan_message(
                &paths,
                &plan.plan_id,
                &PlanMessage::new(
                    "coordinator",
                    &task_id,
                    PlanMessageKind::Progress,
                    format!("{task_id} started"),
                    json!({ "task_index": task_index }),
                )?,
            )?;
        }
        if ready.is_empty() {
            continue;
        }
        save_plan(&paths, &plan)?;
        write_coordinator_snapshot(&paths, &plan, None)?;

        let mut outcomes = Vec::new();
        let (signal_tx, signal_rx) = std::sync::mpsc::channel::<PlanChildSignal>();
        let mut handles = Vec::new();
        for task_index in ready {
            let source_dir = match plan_child_source_dir(
                &paths,
                &plan,
                task_index,
                &parent_cwd,
                DependencyComposeRepair {
                    disabled: no_repair,
                    provider: repair_provider.as_deref(),
                    quiet,
                },
            )
            .await
            {
                Ok(source_dir) => source_dir,
                Err(error) => {
                    outcomes.push((task_index, Err(error)));
                    continue;
                }
            };
            // Stack policy branches each node off the branch tip as it stands
            // now, so it sees every earlier node that has landed. Base policy
            // pins every node to the ref the plan started from.
            let per_node_base = if plan.apply == deadreckon_core::plan::ApplyWhen::PerNode {
                let repo = plan
                    .parent_cwd
                    .clone()
                    .unwrap_or_else(|| parent_cwd.clone());
                match plan.branch_policy {
                    BranchPolicy::Base => Some("HEAD".to_string()),
                    BranchPolicy::Stack | BranchPolicy::Merge => Some(
                        git_stdout(&repo, &["rev-parse", "HEAD"])?
                            .trim()
                            .to_string(),
                    ),
                }
            } else {
                None
            };
            let paths_for_child = paths.clone();
            let plan_for_child = plan.clone();
            let sandbox_for_child = sandbox.clone();
            let signal_tx_for_child = signal_tx.clone();
            let narrator_model_for_child = narrator_model.clone();
            handles.push((
                task_index,
                tokio::task::spawn_blocking(move || {
                    run_plan_child(PlanChildLaunch {
                        paths: &paths_for_child,
                        plan: &plan_for_child,
                        task_index,
                        source_dir: &source_dir,
                        per_node_base,
                        sandbox: &sandbox_for_child,
                        max_spend,
                        max_wall_seconds,
                        quiet,
                        plain,
                        forward_output: false,
                        narrate,
                        narrator_model: narrator_model_for_child,
                        signal_sender: Some(signal_tx_for_child),
                    })
                }),
            ));
        }
        drop(signal_tx);
        let mut live_children = BTreeMap::new();
        let mut running = handles;
        let started = std::time::Instant::now();
        let mut tick = 0usize;
        let mut last_plain_status = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(2))
            .unwrap_or_else(std::time::Instant::now);
        let mut aggregate_state = ParentAggregateState::default();
        let is_campaign_sub = std::env::var_os(deadreckon_core::campaign::ENV_SUB_RESULT).is_some();
        let aggregate_enabled = orchestrate_aggregate_enabled(narrate, quiet, is_campaign_sub);
        let mut last_aggregate = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(2))
            .unwrap_or_else(std::time::Instant::now);
        while !running.is_empty() {
            drain_plan_child_signals(
                &paths,
                &mut plan,
                &signal_rx,
                &mut live_children,
                quiet,
                plain,
            )?;
            let mut index = 0;
            while index < running.len() {
                if running[index].1.is_finished() {
                    let (task_index, handle) = running.remove(index);
                    let outcome = handle.await.map_err(|err| {
                        CliError::Core(DeadreckonError::InvalidInput(format!(
                            "child join failed: {err}"
                        )))
                    })?;
                    outcomes.push((task_index, outcome));
                } else {
                    index += 1;
                }
            }
            if running.is_empty() {
                break;
            }
            if aggregate_enabled && last_aggregate.elapsed() >= std::time::Duration::from_secs(2) {
                let running_indices: Vec<usize> =
                    running.iter().map(|(task_index, _)| *task_index).collect();
                refresh_parent_aggregate(&paths, &plan, &mut aggregate_state, &running_indices);
                let children: Vec<crate::narrative::ChildHeadline> = running_indices
                    .iter()
                    .filter_map(|task_index| {
                        aggregate_state.headlines.get(task_index).map(|headline| {
                            crate::narrative::ChildHeadline {
                                task_id: plan.tasks[*task_index].task_id.clone(),
                                headline: headline.clone(),
                            }
                        })
                    })
                    .collect();
                if !children.is_empty() {
                    if !plain {
                        clear_cli_wait_status();
                    }
                    let mut out = std::io::sink();
                    let mut err = std::io::stderr();
                    let mut sinks = crate::narrative::AggregateSinks {
                        out: &mut out,
                        err: &mut err,
                    };
                    let _ = crate::narrative::emit_parent_aggregate(&mut sinks, &children, 100);
                }
                last_aggregate = std::time::Instant::now();
            }
            if !quiet {
                if plain {
                    if last_plain_status.elapsed() >= std::time::Duration::from_secs(2) {
                        eprintln!("{}", plain_plan_progress_line(&plan, started.elapsed()));
                        last_plain_status = std::time::Instant::now();
                    }
                } else {
                    tick = tick.wrapping_add(1);
                    print_cli_wait_status(&plan_wait_status_label(&plan), started.elapsed(), tick);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(if plain {
                500
            } else {
                180
            }))
            .await;
        }
        drain_plan_child_signals(
            &paths,
            &mut plan,
            &signal_rx,
            &mut live_children,
            quiet,
            plain,
        )?;
        if !quiet && !plain {
            clear_cli_wait_status();
        }
        for (task_index, outcome) in outcomes {
            let task_id = plan.tasks[task_index].task_id.clone();
            match outcome {
                Ok(run_id) => {
                    let state = load_run(&paths, &run_id)?;
                    let status = plan_status_from_run_status(state.status);
                    // A missed done contract arrives here, not in the Err arm:
                    // the child process exits cleanly and the run carries the
                    // failure. That is the case worth another attempt, because
                    // there is a run to extend and a reason to seed it with.
                    if status == PlanTaskStatus::Failed
                        && let NodeFailureOutcome::Retrying =
                            record_node_failure(RecordNodeFailure {
                                paths: &paths,
                                plan: &mut plan,
                                task_index,
                                run_id: Some(run_id.as_str()),
                                failure_reason: state.failure_reason.clone(),
                                spend_usd: state.total_spend_usd,
                                consecutive_failures: &mut consecutive_failures,
                                halt: &mut halt,
                                quiet,
                                plain,
                            })?
                    {
                        made_progress = true;
                        save_plan(&paths, &plan)?;
                        write_coordinator_snapshot(&paths, &plan, None)?;
                        continue;
                    }
                    if status == PlanTaskStatus::Completed {
                        consecutive_failures = 0;
                    }
                    let summary =
                        summarize_child_run(&paths, &plan, &plan.tasks[task_index], &state);
                    write_child_summary(&paths, &plan.plan_id, &task_id, &summary)?;
                    let marker = plan_child_marker(&paths, &plan, &plan.tasks[task_index], &state);
                    write_plan_child_marker(&state.working_dir, &marker)?;
                    let library_dir = paths.library_dir(&state.scope, &state.run_id);
                    if library_dir.is_dir() {
                        write_plan_child_marker(&library_dir, &marker)?;
                    }
                    {
                        let task = &mut plan.tasks[task_index];
                        task.child_run_id = Some(run_id.clone());
                        task.child_scope = Some(state.scope.clone());
                        task.summary_path =
                            Some(deadreckon_core::child_summary_relative_path(&task.task_id));
                        task.status = status;
                    }
                    append_plan_event(
                        &paths,
                        &plan.plan_id,
                        PlanEventKind::TaskRunDiscovered {
                            task_id: task_id.clone(),
                            task_index,
                            run_id: Some(run_id.clone()),
                            pid: live_children.get(&task_index).copied(),
                        },
                    )?;
                    append_task_terminal_plan_event(&paths, &plan, task_index, status, &run_id)?;
                    append_plan_message(
                        &paths,
                        &plan.plan_id,
                        &PlanMessage::new(
                            "coordinator",
                            &task_id,
                            if status == PlanTaskStatus::Completed {
                                PlanMessageKind::Progress
                            } else {
                                PlanMessageKind::Blocker
                            },
                            format!("{task_id} {}", task_status_label(status)),
                            json!({
                                "task_index": task_index,
                                "run_id": run_id,
                                "run_status": state.status.to_string(),
                            }),
                        )?,
                    )?;
                    save_plan(&paths, &plan)?;
                    write_coordinator_snapshot(&paths, &plan, None)?;
                    if !quiet {
                        print_plan_child_finished_line(&plan, task_index, status, &run_id, plain);
                    }
                    if plan.apply == deadreckon_core::plan::ApplyWhen::PerNode
                        && status == PlanTaskStatus::Completed
                    {
                        // A landing failure is not a gate failure: the node did
                        // its work and passed. Retrying it would re-run good
                        // work against the same conflict, so the plan halts
                        // with the reason instead, leaving earlier nodes
                        // applied and the rest unstarted.
                        if let Err(error) = apply_node(&paths, &plan, task_index, &run_id) {
                            let reason = error.to_string();
                            append_plan_event(
                                &paths,
                                &plan.plan_id,
                                PlanEventKind::TaskBlocked {
                                    task_id: task_id.clone(),
                                    task_index,
                                    reason: reason.clone(),
                                },
                            )?;
                            halt = Some(reason);
                        } else {
                            append_plan_event(
                                &paths,
                                &plan.plan_id,
                                PlanEventKind::TaskApplied {
                                    task_id: task_id.clone(),
                                    task_index,
                                    run_id: run_id.clone(),
                                },
                            )?;
                            if !quiet {
                                println!(
                                    "{}",
                                    ui_status(format!("{task_id} landed on the branch"))
                                );
                            }
                        }
                        save_plan(&paths, &plan)?;
                    }
                }
                Err(error) => {
                    // The child never produced a run (spawn or source-prep
                    // failure). Still an attempt, and still retryable — but a
                    // retry starts fresh, because there is nothing to extend.
                    if let NodeFailureOutcome::Retrying = record_node_failure(RecordNodeFailure {
                        paths: &paths,
                        plan: &mut plan,
                        task_index,
                        run_id: None,
                        failure_reason: Some(error.to_string()),
                        spend_usd: 0.0,
                        consecutive_failures: &mut consecutive_failures,
                        halt: &mut halt,
                        quiet,
                        plain,
                    })? {
                        made_progress = true;
                        save_plan(&paths, &plan)?;
                        write_coordinator_snapshot(&paths, &plan, None)?;
                        continue;
                    }
                    mark_plan_task_status(&mut plan, task_index, PlanTaskStatus::Failed)?;
                    append_plan_event(
                        &paths,
                        &plan.plan_id,
                        PlanEventKind::TaskFailed {
                            task_id: task_id.clone(),
                            task_index,
                            reason: error.to_string(),
                        },
                    )?;
                    append_plan_message(
                        &paths,
                        &plan.plan_id,
                        &PlanMessage::new(
                            "coordinator",
                            &task_id,
                            PlanMessageKind::Blocker,
                            format!("{task_id} failed"),
                            json!({ "task_index": task_index, "error": error.to_string() }),
                        )?,
                    )?;
                    save_plan(&paths, &plan)?;
                    write_coordinator_snapshot(&paths, &plan, None)?;
                }
            }
        }
    }

    mark_blocked_pending_tasks(&paths, &mut plan, halt.as_deref())?;
    if let Some(reason) = halt.as_deref()
        && plan.status != PlanStatus::Failed
    {
        // Record why the plan stopped launching before the generic terminal
        // sweep does — "circuit breaker: 2 nodes failed in a row" is a usable
        // account; "one or more child tasks failed" is not.
        plan.status = PlanStatus::Failed;
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::PlanFailed {
                reason: reason.to_string(),
            },
        )?;
    }
    mark_failed_fork_plan_terminal(&paths, &mut plan)?;
    save_plan(&paths, &plan)?;
    let _ = fs::remove_file(paths.coordinator_json(&plan.plan_id));
    if !quiet && completion_surface {
        print_fork_finished(&plan, no_hints);
    }
    Ok(())
}

/// Land one completed node's work on the operator's branch.
///
/// Lifted from `chain`'s auto_apply_chain_step, which had exactly this logic
/// bound to chain state. The guards are the point and are kept whole: the
/// acceptance marker must validate (an unsigned or forged result never
/// lands), the target must be clean, and every changed file must sit inside
/// the plan's allowlist when one is set.
///
/// Applying to the parent repo is also what lets the next node see this work:
/// nodes source from `plan.parent_cwd`, so a node that starts after this
/// returns copies a tree that already contains it.
fn apply_node(paths: &DeadreckonPaths, plan: &Plan, task_index: usize, run_id: &str) -> Result<()> {
    let state = load_run(paths, run_id)?;
    validate_acceptance_marker(&state)?;
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    let task_id = plan.tasks[task_index].task_id.clone();
    if !git_stdout(git_root, &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("{task_id} refused to land (target has uncommitted changes)"),
            &format!(
                "git -C {} stash && deadreckon fork {}",
                git_root.display(),
                run_prefix(&plan.plan_id)
            ),
        )));
    }
    if !plan.apply_allowlist.is_empty() {
        let branch = record.branch_name.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "missing branch_name".to_string(),
            ))
        })?;
        let files = git_stdout(
            git_root,
            &["diff", "--name-only", &format!("HEAD..{branch}")],
        )?;
        for file in files.lines().filter(|line| !line.trim().is_empty()) {
            if !plan
                .apply_allowlist
                .iter()
                .any(|pattern| apply_allowlist_matches(pattern, file))
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("{task_id} refused to land (outside_allowlist {file})"),
                    &format!("deadreckon show {}", run_prefix(run_id)),
                )));
            }
        }
    }
    super::lifecycle::apply_command_quiet(
        run_id.to_string(),
        apply_strategy_label(plan.apply_strategy).to_string(),
        None,
        true,
        true,
        false,
        None,
    )
}

pub(crate) fn apply_strategy_label(value: ApplyStrategy) -> &'static str {
    match value {
        ApplyStrategy::Squash => "squash",
        ApplyStrategy::Merge => "merge",
        ApplyStrategy::CherryPick => "cherry-pick",
    }
}

fn apply_allowlist_matches(pattern: &str, file: &str) -> bool {
    pattern == "*"
        || pattern == file
        || file.starts_with(pattern.trim_end_matches('*'))
        || file.starts_with(pattern.trim_end_matches('/'))
}

/// What the plan does with a node whose attempt just failed.
#[derive(Debug, PartialEq, Eq)]
enum NodeFailureOutcome {
    /// Attempts remain. The node is Pending again and will be relaunched,
    /// extending its failed run where there is one.
    Retrying,
    /// Out of attempts. The caller records the terminal failure as before.
    Exhausted,
}

struct RecordNodeFailure<'a> {
    paths: &'a DeadreckonPaths,
    plan: &'a mut Plan,
    task_index: usize,
    run_id: Option<&'a str>,
    failure_reason: Option<String>,
    spend_usd: f64,
    consecutive_failures: &'a mut u32,
    halt: &'a mut Option<String>,
    quiet: bool,
    plain: bool,
}

/// Record a failed attempt and decide whether the node gets another run.
///
/// This is what keeps an unattended plan moving. Before this existed, a node
/// that missed its done contract ended the plan with `paused plan <id>` and
/// `Recommended: deadreckon attach` — advice for an operator who walked away.
fn record_node_failure(context: RecordNodeFailure<'_>) -> Result<NodeFailureOutcome> {
    let RecordNodeFailure {
        paths,
        plan,
        task_index,
        run_id,
        failure_reason,
        spend_usd,
        consecutive_failures,
        halt,
        quiet,
        plain,
    } = context;

    let max_attempts = plan.max_attempts;
    let on_fail = plan.on_fail;
    let threshold = plan.circuit_breaker_threshold;
    let plan_id = plan.plan_id.clone();
    let task_id = plan.tasks[task_index].task_id.clone();

    let attempt_number = plan.tasks[task_index].attempts_used() + 1;
    plan.tasks[task_index]
        .attempts
        .push(deadreckon_core::plan::TaskAttempt::failed(
            attempt_number,
            run_id.map(ToString::to_string),
            failure_reason.clone(),
            spend_usd,
        ));

    let reason =
        failure_reason.unwrap_or_else(|| "child did not satisfy the done contract".to_string());

    if plan.tasks[task_index].may_retry(max_attempts) {
        let next_attempt = attempt_number + 1;
        // Back to Pending so the fork loop relaunches it; child_argv reads the
        // recorded attempt to extend the failed run rather than start over.
        mark_plan_task_status(plan, task_index, PlanTaskStatus::Pending)?;
        append_plan_event(
            paths,
            &plan_id,
            PlanEventKind::TaskRetrying {
                task_id: task_id.clone(),
                task_index,
                attempt: next_attempt,
                max_attempts,
                reason: reason.clone(),
                parent_run_id: run_id.map(ToString::to_string),
            },
        )?;
        append_plan_message(
            paths,
            &plan_id,
            &PlanMessage::new(
                "coordinator",
                &task_id,
                PlanMessageKind::Progress,
                format!("{task_id} retrying ({next_attempt}/{max_attempts})"),
                json!({
                    "task_index": task_index,
                    "attempt": next_attempt,
                    "max_attempts": max_attempts,
                    "reason": reason,
                    "parent_run_id": run_id,
                }),
            )?,
        )?;
        if !quiet {
            print_plan_child_retry_line(
                &task_id,
                next_attempt,
                max_attempts,
                &reason,
                run_id,
                plain,
            );
        }
        return Ok(NodeFailureOutcome::Retrying);
    }

    // Out of attempts. Count it against the breaker, then let the failure
    // policy decide whether the rest of the graph keeps going.
    *consecutive_failures += 1;
    if halt.is_some() {
        // Already halted; siblings still in flight land here as they finish.
        // Recording the breaker again would double-count the same event.
    } else if threshold > 0 && *consecutive_failures >= threshold {
        append_plan_event(
            paths,
            &plan_id,
            PlanEventKind::CircuitBreakerTripped {
                consecutive_failures: *consecutive_failures,
                threshold,
            },
        )?;
        *halt = Some(format!(
            "circuit breaker: {consecutive} nodes failed in a row (threshold {threshold})",
            consecutive = *consecutive_failures
        ));
    } else if on_fail == OnFail::Stop {
        *halt = Some(format!(
            "{task_id} failed and the plan's failure policy is stop"
        ));
    }

    Ok(NodeFailureOutcome::Exhausted)
}

fn print_plan_child_retry_line(
    task_id: &str,
    attempt: u32,
    max_attempts: u32,
    reason: &str,
    parent_run_id: Option<&str>,
    plain: bool,
) {
    let continues = parent_run_id
        .map(|run_id| format!(" continuing {}", run_prefix(run_id)))
        .unwrap_or_default();
    let line = format!(
        "{task_id} retry {attempt}/{max_attempts}{continues}: {}",
        one_line_reason(reason)
    );
    if plain {
        println!("{line}");
    } else {
        println!("{}", ui_status(&line));
    }
}

/// Failure reasons arrive multi-line from the gate; the progress line wants one.
fn one_line_reason(reason: &str) -> String {
    let compact = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 160 {
        return compact;
    }
    let clipped = compact.chars().take(160).collect::<String>();
    format!("{clipped}...")
}

fn fork_refusal_surface(paths: &DeadreckonPaths, plan: &Plan) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let primary = plan_next_actions_with_context(paths, plan)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("deadreckon show {id}"));
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!(
                "DeadReckon did not fork the plan because it is already {}.",
                plan_status_label(plan.status)
            ),
            "Fork only starts pending plans; the recommended command follows the current plan state instead.",
            [
                ("plan".to_string(), id.clone()),
                (
                    "status".to_string(),
                    plan_status_label(plan.status).to_string(),
                ),
                (
                    "tasks".to_string(),
                    format!("{completed}/{} completed", plan.tasks.len()),
                ),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
}

#[derive(Debug)]
enum PlanChildSignal {
    Pid { task_index: usize, pid: u32 },
    RunId { task_index: usize, run_id: String },
}

fn drain_plan_child_signals(
    paths: &DeadreckonPaths,
    plan: &mut Plan,
    signal_rx: &std::sync::mpsc::Receiver<PlanChildSignal>,
    live_children: &mut BTreeMap<usize, u32>,
    quiet: bool,
    plain: bool,
) -> Result<()> {
    while let Ok(signal) = signal_rx.try_recv() {
        match signal {
            PlanChildSignal::Pid { task_index, pid } => {
                live_children.insert(task_index, pid);
                if let Some(task) = plan.tasks.get(task_index) {
                    append_plan_event(
                        paths,
                        &plan.plan_id,
                        PlanEventKind::TaskRunDiscovered {
                            task_id: task.task_id.clone(),
                            task_index,
                            run_id: task.child_run_id.clone(),
                            pid: Some(pid),
                        },
                    )?;
                }
                write_coordinator_snapshot_live(paths, plan, live_children)?;
            }
            PlanChildSignal::RunId { task_index, run_id } => {
                let mut newly_discovered = false;
                if let Some(task) = plan.tasks.get_mut(task_index)
                    && task.child_run_id.as_deref() != Some(run_id.as_str())
                {
                    task.child_run_id = Some(run_id.clone());
                    newly_discovered = true;
                }
                if let Some(task) = plan.tasks.get(task_index) {
                    append_plan_event(
                        paths,
                        &plan.plan_id,
                        PlanEventKind::TaskRunDiscovered {
                            task_id: task.task_id.clone(),
                            task_index,
                            run_id: Some(run_id.clone()),
                            pid: live_children.get(&task_index).copied(),
                        },
                    )?;
                }
                save_plan(paths, plan)?;
                write_coordinator_snapshot_live(paths, plan, live_children)?;
                if newly_discovered && !quiet {
                    print_plan_child_run_line(plan, task_index, &run_id, plain);
                }
            }
        }
    }
    Ok(())
}

fn print_plan_child_run_line(plan: &Plan, task_index: usize, run_id: &str, plain: bool) {
    if !plain {
        clear_cli_wait_status();
    }
    let Some(task) = plan.tasks.get(task_index) else {
        return;
    };
    println!(
        "{} {} {} -> run {}  {}",
        ui_heading("plan"),
        ui_id(run_prefix(&plan.plan_id)),
        ui_id(&task.task_id),
        ui_id(run_prefix(run_id)),
        ui_command(format!(
            "deadreckon attach {}:{}",
            run_prefix(&plan.plan_id),
            task.task_id
        ))
    );
    let _ = io::stdout().flush();
}

fn print_plan_child_finished_line(
    plan: &Plan,
    task_index: usize,
    status: PlanTaskStatus,
    run_id: &str,
    plain: bool,
) {
    if !plain {
        clear_cli_wait_status();
    }
    let Some(task) = plan.tasks.get(task_index) else {
        return;
    };
    println!(
        "{} {} {} {} run {}",
        ui_heading("plan"),
        ui_id(run_prefix(&plan.plan_id)),
        ui_id(&task.task_id),
        ui_status(task_status_label(status)),
        ui_id(run_prefix(run_id))
    );
    let _ = io::stdout().flush();
}

fn plan_wait_status_label(plan: &Plan) -> String {
    format!(
        "plan {} running; {}; attach deadreckon attach {}",
        run_prefix(&plan.plan_id),
        plan_progress_summary(plan),
        run_prefix(&plan.plan_id)
    )
}

fn plain_plan_progress_line(plan: &Plan, elapsed: std::time::Duration) -> String {
    format!(
        "[plan {}] {} elapsed={}s attach=deadreckon attach {}",
        run_prefix(&plan.plan_id),
        plan_progress_summary(plan),
        elapsed.as_secs(),
        run_prefix(&plan.plan_id)
    )
}

fn plan_progress_summary(plan: &Plan) -> String {
    let total = plan.tasks.len();
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let failed = plan
        .tasks
        .iter()
        .filter(|task| matches!(task.status, PlanTaskStatus::Failed | PlanTaskStatus::Killed))
        .count();
    let pending = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Pending)
        .count();
    let running = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Running)
        .map(|task| match task.child_run_id.as_deref() {
            Some(run_id) => format!("{}:{}", task.task_id, run_prefix(run_id)),
            None => task.task_id.clone(),
        })
        .collect::<Vec<_>>();
    let running = if running.is_empty() {
        "-".to_string()
    } else {
        running.join(",")
    };
    format!("done={completed}/{total} running={running} pending={pending} failed={failed}")
}

fn apply_fork_provider_overrides(
    plan: &mut Plan,
    provider: Option<String>,
    child_provider: &[String],
    coder_provider: Option<String>,
    reviewer_provider: Option<String>,
) -> Result<()> {
    match plan.mode {
        PlanMode::FullPlan => {
            if let Some(provider) = provider {
                plan.providers.default_child = Some(provider.clone());
                for task in &mut plan.tasks {
                    task.provider = Some(provider.clone());
                }
            }
            let overrides = parse_child_provider_overrides(child_provider, plan.n as u8)?;
            for (index, provider) in overrides {
                plan.providers.children.insert(index, provider.clone());
                let task = plan.tasks.get_mut(index as usize).ok_or_else(|| {
                    CliError::Core(deadreckon_core::user_error(
                        &format!("child provider index {index} outside 0..{}", plan.n),
                        "--child-provider 1=cli:codex",
                    ))
                })?;
                task.provider = Some(provider);
            }
        }
        PlanMode::Review => {
            if let Some(provider) = coder_provider {
                plan.providers.coder = Some(provider.clone());
                if let Some(task) = plan
                    .tasks
                    .iter_mut()
                    .find(|task| task.role == PlanRole::Coder)
                {
                    task.provider = Some(provider);
                }
            }
            if let Some(provider) = reviewer_provider {
                plan.providers.reviewer = Some(provider.clone());
                if let Some(task) = plan
                    .tasks
                    .iter_mut()
                    .find(|task| task.role == PlanRole::Reviewer)
                {
                    task.provider = Some(provider);
                }
            }
        }
    }
    Ok(())
}

fn append_task_terminal_plan_event(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task_index: usize,
    status: PlanTaskStatus,
    run_id: &str,
) -> Result<()> {
    let task = plan.tasks.get(task_index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("no child index {task_index}"),
            "deadreckon plan \"your goal\"",
        ))
    })?;
    let event = match status {
        PlanTaskStatus::Completed => PlanEventKind::TaskCompleted {
            task_id: task.task_id.clone(),
            task_index,
            run_id: Some(run_id.to_string()),
            status: "completed".to_string(),
        },
        PlanTaskStatus::Failed => PlanEventKind::TaskFailed {
            task_id: task.task_id.clone(),
            task_index,
            reason: format!("run {run_id} failed"),
        },
        PlanTaskStatus::Killed => PlanEventKind::TaskKilled {
            task_id: task.task_id.clone(),
            task_index,
            run_id: Some(run_id.to_string()),
        },
        PlanTaskStatus::Pending | PlanTaskStatus::Running => PlanEventKind::TaskBlocked {
            task_id: task.task_id.clone(),
            task_index,
            reason: format!("run {run_id} ended {}", task_status_label(status)),
        },
    };
    append_plan_event(paths, &plan.plan_id, event)?;
    Ok(())
}

fn mark_plan_task_status(plan: &mut Plan, task_index: usize, status: PlanTaskStatus) -> Result<()> {
    let task = plan.tasks.get_mut(task_index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("no child index {task_index}"),
            "deadreckon plan \"your goal\"",
        ))
    })?;
    task.status = status;
    Ok(())
}

fn write_coordinator_snapshot(
    paths: &DeadreckonPaths,
    plan: &Plan,
    live_child: Option<(usize, u32)>,
) -> Result<()> {
    let live_children = live_child.into_iter().collect::<BTreeMap<_, _>>();
    write_coordinator_snapshot_live(paths, plan, &live_children)
}

fn write_coordinator_snapshot_live(
    paths: &DeadreckonPaths,
    plan: &Plan,
    live_children: &BTreeMap<usize, u32>,
) -> Result<()> {
    let children = plan
        .tasks
        .iter()
        .enumerate()
        .map(|(index, task)| CoordinatorChild {
            child_index: task.index,
            task_id: task.task_id.clone(),
            run_id: task.child_run_id.clone(),
            pid: live_children.get(&index).copied(),
            scope: task.child_scope.clone(),
            provider: task.provider.clone(),
            role: task.role,
            status: task.status,
        })
        .collect::<Vec<_>>();
    write_coordinator_state(
        paths,
        &plan.plan_id,
        &CoordinatorState {
            schema_version: 1,
            plan_id: plan.plan_id.clone(),
            coordinator_pid: std::process::id(),
            started_at: plan.forked_at.unwrap_or_else(Utc::now),
            children,
        },
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct DependencyComposeRepair<'a> {
    disabled: bool,
    provider: Option<&'a str>,
    quiet: bool,
}

async fn plan_child_source_dir(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task_index: usize,
    parent_cwd: &Path,
    repair: DependencyComposeRepair<'_>,
) -> Result<PathBuf> {
    let task = &plan.tasks[task_index];
    let plan_cwd = plan.parent_cwd.as_deref().unwrap_or(parent_cwd);
    // A retry resumes from the tree the failed attempt left behind, so the
    // agent fixes its own near-miss rather than starting the node over. Falls
    // back to the normal source when that run is unreadable (or the attempt
    // never produced one), which keeps a retry possible either way.
    if let Some(parent_run_id) = task.retry_parent_run_id()
        && let Ok(parent_state) = load_run(paths, parent_run_id)
        && parent_state.working_dir.is_dir()
    {
        return Ok(parent_state.working_dir);
    }
    if plan.apply == deadreckon_core::plan::ApplyWhen::PerNode {
        // Earlier nodes have already landed on this branch, so the parent repo
        // *is* the composed source. Composing dependency artifacts here would
        // reapply work that is already present.
        return Ok(plan_cwd.to_path_buf());
    }
    if task.depends_on.is_empty() {
        return Ok(plan_cwd.to_path_buf());
    }

    let dependencies = plan_dependency_artifacts(paths, plan, task)?;
    if task.role == PlanRole::Reviewer && dependencies.len() == 1 {
        Ok(dependencies[0].root.clone())
    } else {
        compose_dependency_source_dir(paths, plan, task, &dependencies, repair).await
    }
}

#[derive(Debug, Clone)]
struct PlanDependencyArtifact {
    task_id: String,
    index: u32,
    run_id: String,
    root: PathBuf,
}

fn plan_dependency_artifacts(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
) -> Result<Vec<PlanDependencyArtifact>> {
    let mut dependencies = Vec::new();
    for dependency in &task.depends_on {
        let dependency_task = plan.task_by_id(dependency).ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                &format!("task {} depends on unknown {dependency}", task.task_id),
                "edit the plan so depends_on references earlier task ids",
            ))
        })?;
        if dependency_task.status != PlanTaskStatus::Completed {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "task {} dependency {} is {}",
                    task.task_id,
                    dependency_task.task_id,
                    task_status_label(dependency_task.status)
                ),
                "wait for dependencies to complete before forking dependent children",
            )));
        }
        let run_id = dependency_task.child_run_id.as_deref().ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                &format!(
                    "task {} dependency {} has no run id",
                    task.task_id, dependency
                ),
                "deadreckon fork <plan-id>",
            ))
        })?;
        let state = load_run(paths, run_id)?;
        dependencies.push(PlanDependencyArtifact {
            task_id: dependency_task.task_id.clone(),
            index: dependency_task.index,
            run_id: run_id.to_string(),
            root: child_artifact_root(paths, &state),
        });
    }
    dependencies.sort_by_key(|dependency| dependency.index);
    Ok(dependencies)
}

async fn compose_dependency_source_dir(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    dependencies: &[PlanDependencyArtifact],
    repair: DependencyComposeRepair<'_>,
) -> Result<PathBuf> {
    let source_dir = paths
        .plan_dir(&plan.plan_id)
        .join("launch")
        .join(&task.task_id)
        .join("source");
    let sources = dependencies
        .iter()
        .map(|dependency| ComposeFileSource {
            root: dependency.root.clone(),
            data: dependency.clone(),
            prefix_error: "dependency source prefix error",
        })
        .collect::<Vec<_>>();
    let conflicts = compose_merge_sources(
        &source_dir,
        &sources,
        |dependency, _relative, file, hash| PlanMergeSeenFile {
            task_id: dependency.task_id.clone(),
            task_index: dependency.index,
            run_id: dependency.run_id.clone(),
            artifact_root: dependency.root.clone(),
            artifact_path: file.to_path_buf(),
            hash,
        },
        |relative, previous, current| {
            if plan_task_depends_on(plan, &current.task_id, &previous.task_id) {
                ComposeMergeDecision::UseCurrent
            } else if plan_task_depends_on(plan, &previous.task_id, &current.task_id) {
                ComposeMergeDecision::KeepExisting
            } else {
                ComposeMergeDecision::RecordConflict {
                    conflict: plan_merge_conflict(plan, relative, previous, current, None),
                    use_current: false,
                }
            }
        },
    )?;
    if !conflicts.is_empty() {
        repair_dependency_source_conflicts(
            paths,
            plan,
            task,
            repair,
            source_dir.clone(),
            conflicts,
        )
        .await?;
    }
    Ok(source_dir)
}

async fn repair_dependency_source_conflicts(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    repair: DependencyComposeRepair<'_>,
    source_dir: PathBuf,
    conflicts: Vec<PlanMergeConflict>,
) -> Result<()> {
    let mut merge = PlanMergeOutcome {
        working_dir: source_dir,
        conflicts,
    };
    let unresolved_conflicts = merge.unresolved_conflicts();
    let context = MergeRepairContext::dependency(paths, plan, task);
    write_plan_merge_conflicts_to(
        &context.proof_dir,
        plan,
        "dependency-compose",
        &unresolved_conflicts,
    )?;
    let worker_spec = render_launch_worker_spec(paths, plan, task);
    write_worker_spec(paths, &plan.plan_id, &task.task_id, &worker_spec)?;
    let provider = if repair.disabled {
        None
    } else {
        resolve_merge_repair_provider(paths, plan, repair.provider)?
    };
    write_merge_repair_request(
        paths,
        plan,
        &context,
        provider.as_deref(),
        &unresolved_conflicts,
    )?;
    if repair.disabled {
        return Err(dependency_source_conflict_error(
            task,
            &unresolved_conflicts,
            &format!(
                "automatic repair disabled; inspect {}",
                context.proof_dir.join("conflicts.json").display()
            ),
        ));
    }
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::MergeConflict {
            conflict_count: unresolved_conflicts.len(),
        },
    )?;
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::MergeRepairPlanned {
            conflict_count: unresolved_conflicts.len(),
            provider: provider.clone(),
        },
    )?;
    let Some(provider) = provider else {
        let reason = format!(
            "dependency merge repair for {} needs a configured provider",
            task.task_id
        );
        append_plan_event(
            paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairFailed {
                reason: reason.clone(),
            },
        )?;
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("{reason}; conflicts remain"),
            "deadreckon providers list --all",
        )));
    };
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::MergeRepairStarted {
            mode: MergeRepairMode::Auto.as_str().to_string(),
        },
    )?;
    match run_merge_repair(
        paths,
        plan,
        &context,
        &MergeRepairOptions {
            provider: &provider,
            mode: MergeRepairMode::Auto,
            attempts: 1,
            quiet: repair.quiet,
        },
        &mut merge,
    )
    .await
    {
        Ok(repaired) => {
            append_plan_event(
                paths,
                &plan.plan_id,
                PlanEventKind::MergeRepaired {
                    strategy: format!("dependency-{}", repaired.strategy),
                    repair_run_id: repaired.repair_run_id,
                },
            )?;
            write_plan_merge_conflicts_to(
                &context.proof_dir,
                plan,
                "dependency-compose",
                &merge.conflicts,
            )?;
            Ok(())
        }
        Err(error) => {
            let reason = error.to_string();
            append_plan_event(
                paths,
                &plan.plan_id,
                PlanEventKind::MergeRepairFailed { reason },
            )?;
            Err(error)
        }
    }
}

fn dependency_source_conflict_error(
    task: &PlanTask,
    conflicts: &[PlanMergeConflict],
    hint: &str,
) -> CliError {
    let paths = conflicts
        .iter()
        .map(|conflict| conflict.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    CliError::Core(deadreckon_core::user_error(
        &format!(
            "dependency source conflict while preparing {} at {}",
            task.task_id, paths
        ),
        hint,
    ))
}

pub(crate) fn plan_task_depends_on(plan: &Plan, task_id: &str, dependency_id: &str) -> bool {
    let mut stack = vec![task_id.to_string()];
    let mut seen = BTreeSet::new();
    while let Some(next) = stack.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let Some(task) = plan.task_by_id(&next) else {
            continue;
        };
        if task
            .depends_on
            .iter()
            .any(|dependency| dependency == dependency_id)
        {
            return true;
        }
        stack.extend(task.depends_on.iter().cloned());
    }
    false
}

struct PlanChildLaunch<'a> {
    paths: &'a DeadreckonPaths,
    plan: &'a Plan,
    task_index: usize,
    source_dir: &'a Path,
    /// The git ref a per-node child branches from. `None` for at-end plans,
    /// which copy a source tree instead of branching.
    per_node_base: Option<String>,
    sandbox: &'a str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    quiet: bool,
    plain: bool,
    forward_output: bool,
    narrate: bool,
    narrator_model: Option<String>,
    signal_sender: Option<std::sync::mpsc::Sender<PlanChildSignal>>,
}

/// Build the spawned child's argument vector (the `run`/`extend` invocation
/// plus flags). Pure so the `--narrate` propagation is unit-testable without
/// spawning a subprocess.
#[allow(clippy::too_many_arguments)] // mirrors run_plan_child's resolved locals
fn child_argv(
    plan: &Plan,
    task: &PlanTask,
    prompt: &str,
    source_dir: &Path,
    plain: bool,
    sandbox: &str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    narrate: bool,
    narrator_model: Option<&str>,
    per_node_base: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    // A retry stays a `run`, not an `extend`: `extend` requires a completed
    // parent with promoted artifacts, and a node that missed its gate is
    // Failed by definition. The partial work is carried instead through the
    // source dir (see plan_child_source_dir), which hands the retry the failed
    // attempt's working tree.
    let review_parent = review_parent_run_id(plan, task);
    if let Some(parent_run_id) = review_parent.as_deref() {
        argv.extend([
            "extend".to_string(),
            parent_run_id.to_string(),
            prompt.to_string(),
            "--no-docs".to_string(),
        ]);
    } else if plan.apply == deadreckon_core::plan::ApplyWhen::PerNode {
        // Per-node apply lands each node with `deadreckon apply`, which needs
        // real git ancestry: a branch off a known base, not a copied tree.
        // This is the same shape chain steps have always used.
        argv.extend([
            "run".to_string(),
            prompt.to_string(),
            "--worktree".to_string(),
            "--base".to_string(),
            per_node_base.unwrap_or("HEAD").to_string(),
            "--yes".to_string(),
            "--no-confirm".to_string(),
            "--no-hints".to_string(),
            "--no-docs".to_string(),
        ]);
        if plain {
            argv.push("--plain".to_string());
        }
        if let Some(acceptance_path) = plan.acceptance_path.as_deref() {
            argv.push("--acceptance".to_string());
            argv.push(acceptance_path.display().to_string());
        }
    } else {
        argv.extend([
            "run".to_string(),
            prompt.to_string(),
            "--from".to_string(),
            source_dir.display().to_string(),
            "--yes".to_string(),
            "--no-confirm".to_string(),
            "--no-hints".to_string(),
            "--no-docs".to_string(),
        ]);
        if plain {
            argv.push("--plain".to_string());
        }
        if let Some(acceptance_path) = plan.acceptance_path.as_deref() {
            argv.push("--acceptance".to_string());
            argv.push(acceptance_path.display().to_string());
        }
    }
    if narrate {
        argv.push("--narrate".to_string());
        if let Some(model) = narrator_model {
            argv.push("--narrator-model".to_string());
            argv.push(model.to_string());
        }
    }
    argv.push("--sandbox".to_string());
    argv.push(sandbox.to_string());
    if let Some(max_spend) = max_spend {
        argv.push("--max-spend".to_string());
        argv.push(format!("{max_spend:.6}"));
    }
    if let Some(max_wall_seconds) = max_wall_seconds {
        argv.push("--max-wall-seconds".to_string());
        argv.push(max_wall_seconds.to_string());
    }
    if task
        .provider
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        if review_parent.is_some() {
            argv.push("--provider".to_string());
            argv.push("smoke".to_string());
        } else {
            argv.push("--smoke".to_string());
        }
    } else if let Some(provider) = task.provider.as_deref() {
        argv.push("--provider".to_string());
        argv.push(provider.to_string());
    }
    if let Some(model) = child_model_for_task(&plan.providers, task) {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    argv
}

fn run_plan_child(launch: PlanChildLaunch<'_>) -> Result<String> {
    let PlanChildLaunch {
        paths,
        plan,
        task_index,
        source_dir,
        per_node_base,
        sandbox,
        max_spend,
        max_wall_seconds,
        quiet,
        plain,
        forward_output,
        narrate,
        narrator_model,
        signal_sender,
    } = launch;
    let task = &plan.tasks[task_index];
    let worker_spec_path = paths.worker_spec(&plan.plan_id, &task.task_id);
    let worker_spec = render_launch_worker_spec(paths, plan, task);
    write_worker_spec(paths, &plan.plan_id, &task.task_id, &worker_spec)?;
    let prompt = plan_child_prompt(plan, task, &worker_spec, &worker_spec_path);
    let launch_dir = paths
        .plan_dir(&plan.plan_id)
        .join("launch")
        .join(&task.task_id);
    fs::create_dir_all(&launch_dir)?;

    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(source_dir)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_HINTS", "0")
        .env("DEADRECKON_SCOPE_ROOT", &launch_dir);
    if narrate {
        // The child narrates FILE-ONLY (parent owns its stdout/stderr); the env
        // also skips the per-child auth probe — the parent resolved the model.
        command
            .env(crate::narrator::NARRATE_CHILD_ENV, "1")
            .env("DEADRECKON_AUTH_PROBE", "0");
    }
    command.args(child_argv(
        plan,
        task,
        &prompt,
        source_dir,
        plain,
        sandbox,
        max_spend,
        max_wall_seconds,
        narrate,
        narrator_model.as_deref(),
        per_node_base.as_deref(),
    ));
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(sender) = signal_sender.as_ref() {
        let _ = sender.send(PlanChildSignal::Pid {
            task_index,
            pid: child.id(),
        });
    } else {
        write_coordinator_snapshot(paths, plan, Some((task_index, child.id())))?;
    }
    let stdout = child.stdout.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture child stdout".to_string(),
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture child stderr".to_string(),
        ))
    })?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let stdout_thread = commands::chain::spawn_chain_step_reader(stdout, true, tx.clone());
    let stderr_thread = commands::chain::spawn_chain_step_reader(stderr, false, tx);
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut live_run_id = None;
    let suppress_child_output = quiet || !forward_output;
    let status = loop {
        while let Ok((is_stdout, line)) = rx.try_recv() {
            if let Some(run_id) = commands::chain::capture_chain_step_output(
                is_stdout,
                &line,
                &mut stdout_text,
                &mut stderr_text,
                suppress_child_output,
            )? {
                note_plan_child_run_id(
                    &launch_dir,
                    signal_sender.as_ref(),
                    task_index,
                    &mut live_run_id,
                    run_id,
                );
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok((is_stdout, line)) = rx.try_recv() {
        if let Some(run_id) = commands::chain::capture_chain_step_output(
            is_stdout,
            &line,
            &mut stdout_text,
            &mut stderr_text,
            suppress_child_output,
        )? {
            note_plan_child_run_id(
                &launch_dir,
                signal_sender.as_ref(),
                task_index,
                &mut live_run_id,
                run_id,
            );
        }
    }
    let run_id = live_run_id.or_else(|| commands::chain::parse_started_run_id(&stdout_text));
    if let Some(run_id) = run_id.as_ref() {
        let _ = fs::write(launch_dir.join("run-id"), run_id);
        if let Some(sender) = signal_sender.as_ref() {
            let _ = sender.send(PlanChildSignal::RunId {
                task_index,
                run_id: run_id.clone(),
            });
        }
    }
    if !status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "child {} failed: {}{}",
            task.task_id, stdout_text, stderr_text
        ))));
    }
    run_id.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("could not find run id for child {}", task.task_id),
            "deadreckon list",
        ))
    })
}

fn note_plan_child_run_id(
    launch_dir: &Path,
    sender: Option<&std::sync::mpsc::Sender<PlanChildSignal>>,
    task_index: usize,
    live_run_id: &mut Option<String>,
    run_id: String,
) {
    if live_run_id.as_deref() == Some(run_id.as_str()) {
        return;
    }
    let _ = fs::write(launch_dir.join("run-id"), &run_id);
    if let Some(sender) = sender {
        let _ = sender.send(PlanChildSignal::RunId {
            task_index,
            run_id: run_id.clone(),
        });
    }
    *live_run_id = Some(run_id);
}

fn review_parent_run_id(plan: &Plan, task: &PlanTask) -> Option<String> {
    if task.role != PlanRole::Reviewer {
        return None;
    }
    task.depends_on
        .first()
        .and_then(|dependency| plan.task_by_id(dependency))
        .and_then(|parent_task| parent_task.child_run_id.clone())
}

fn plan_child_prompt(plan: &Plan, task: &PlanTask, spec: &str, spec_path: &Path) -> String {
    let role_note = match task.role {
        PlanRole::Reviewer => {
            "This is a fresh review/fix lane. Write .deadreckon/REVIEW.md first, then apply only fixes tied to findings and acceptance."
        }
        PlanRole::Coder => "This is the coding lane for review-mode orchestration.",
        PlanRole::Child => "This is one full-plan child run in a larger plan.",
    };
    // On a retry, lead with why the last attempt was refused. This mirrors
    // what turn_loop already does inside a single run when the gate fails —
    // the agent is told the specific complaint and told not to declare done
    // until dr-gate passes. Here the same feedback crosses a run boundary.
    let retry_note = match (task.attempts_used(), task.last_failure_reason()) {
        (0, _) | (_, None) => String::new(),
        (used, Some(reason)) => format!(
            "RETRY {attempt} of {max}. The previous attempt did not satisfy the done contract.\n\n\
             Gate failure: {reason}\n\n\
             Fix the root cause of that failure. Do not weaken, edit, or delete the done \
             criteria, and do not declare done until dr-gate passes. Stay within this \
             child's scope.\n\n",
            attempt = used + 1,
            max = plan.max_attempts,
        ),
    };
    format!(
        "{retry_note}{role_note}\n\nRoot goal: {}\nPlan: {}\nTask: {}\nWorker spec path: {}\n\n{}\n",
        plan.root_goal,
        plan.plan_id,
        task.task_id,
        spec_path.display(),
        spec
    )
}

fn plan_status_from_run_status(status: RunStatus) -> PlanTaskStatus {
    match status {
        RunStatus::Completed => PlanTaskStatus::Completed,
        RunStatus::Killed => PlanTaskStatus::Killed,
        RunStatus::Failed => PlanTaskStatus::Failed,
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => PlanTaskStatus::Running,
    }
}

fn summarize_child_run(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    state: &deadreckon_core::PipelineState,
) -> String {
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let files = inventory_files(&state.working_dir).unwrap_or_default();
    let file_lines = files
        .iter()
        .take(20)
        .map(|file| format!("- {}", file.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Child Summary: {}\n\nPlan: {}\nTask: {}\nRole: {:?}\nProvider: {}\nRun: {}\nStatus: {}\nWorking: {}\nLibrary: {}\n\n## Goal\n\n{}\n\n## Files\n\n{}\n",
        task.subject,
        plan.plan_id,
        task.task_id,
        task.role,
        task.provider.as_deref().unwrap_or("config default"),
        state.run_id,
        state.status,
        state.working_dir.display(),
        library_dir.display(),
        task.goal,
        if file_lines.is_empty() {
            "- no files recorded".to_string()
        } else {
            file_lines
        }
    )
}

fn plan_child_marker(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    state: &deadreckon_core::PipelineState,
) -> PlanChildMarker {
    PlanChildMarker {
        schema_version: 1,
        kind: "plan_child".to_string(),
        parent_plan_id: plan.plan_id.clone(),
        parent_scope: plan
            .parent_scope
            .clone()
            .unwrap_or_else(|| state.scope.clone()),
        parent_goal: plan.root_goal.clone(),
        task_id: task.task_id.clone(),
        child_index: task.index,
        task_goal: task.goal.clone(),
        worker_spec: paths.worker_spec(&plan.plan_id, &task.task_id),
        provider: task.provider.clone(),
        role: task.role,
        created_at: Utc::now(),
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Mark whatever never ran. `halt` carries the reason the plan stopped
/// launching work, so a node stranded by a stop policy or a tripped breaker
/// says so instead of reporting "missing dependencies: unknown".
fn mark_blocked_pending_tasks(
    paths: &DeadreckonPaths,
    plan: &mut Plan,
    halt: Option<&str>,
) -> Result<()> {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .map(|task| task.task_id.clone())
        .collect::<BTreeSet<_>>();
    let blockers = plan
        .tasks
        .iter()
        .filter(|task| task.status != PlanTaskStatus::Completed)
        .map(|task| task.task_id.clone())
        .collect::<BTreeSet<_>>();
    let pending = plan
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| task.status == PlanTaskStatus::Pending)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in pending {
        let missing = plan.tasks[index]
            .depends_on
            .iter()
            .filter(|dependency| !completed.contains(*dependency))
            .cloned()
            .collect::<Vec<_>>();
        let blocked_by = missing
            .iter()
            .filter(|dependency| blockers.contains(*dependency))
            .cloned()
            .collect::<Vec<_>>();
        let task_id = plan.tasks[index].task_id.clone();
        plan.tasks[index].status = PlanTaskStatus::Failed;
        append_plan_event(
            paths,
            &plan.plan_id,
            PlanEventKind::TaskBlocked {
                task_id: task_id.clone(),
                task_index: index,
                reason: match (missing.is_empty(), halt) {
                    (true, Some(halt)) => format!("never started: {halt}"),
                    (true, None) => "missing dependencies: unknown".to_string(),
                    (false, _) => format!("missing dependencies: {}", missing.join(", ")),
                },
            },
        )?;
        append_plan_message(
            paths,
            &plan.plan_id,
            &PlanMessage::new(
                "coordinator",
                &task_id,
                PlanMessageKind::Blocker,
                format!("{task_id} blocked"),
                json!({ "missing_dependencies": missing, "blocked_by": blocked_by }),
            )?,
        )?;
    }
    Ok(())
}

fn mark_failed_fork_plan_terminal(paths: &DeadreckonPaths, plan: &mut Plan) -> Result<()> {
    let all_terminal = plan.tasks.iter().all(|task| {
        !matches!(
            task.status,
            PlanTaskStatus::Pending | PlanTaskStatus::Running
        )
    });
    let has_failure = plan
        .tasks
        .iter()
        .any(|task| matches!(task.status, PlanTaskStatus::Failed | PlanTaskStatus::Killed));
    if all_terminal && has_failure && plan.status != PlanStatus::Failed {
        plan.status = PlanStatus::Failed;
        append_plan_event(
            paths,
            &plan.plan_id,
            PlanEventKind::PlanFailed {
                reason: "one or more child tasks failed or were blocked".to_string(),
            },
        )?;
    }
    Ok(())
}

fn print_fork_finished(plan: &Plan, no_hints: bool) {
    let paths = DeadreckonPaths::discover();
    println!(
        "{}",
        plan_verdict_surface(&paths, plan).render_plain(!completion_hints_enabled(no_hints))
    );
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_warning_footer_uses_inspection_guidance_not_primary_action() {
        let rendered =
            orchestrate_preflight_warning_recovery_line("1234567890abcdef1234567890abcdef");

        assert!(!rendered.contains("recommended:"), "{rendered}");
        assert!(
            rendered.contains("inspect: deadreckon attach 12345678 --plain"),
            "{rendered}"
        );
        assert!(!rendered.contains("try:"), "{rendered}");
    }

    #[test]
    fn orchestrate_aggregate_suppressed_when_running_as_campaign_sub() {
        // A normal narrating orchestrate prints its aggregate.
        assert!(orchestrate_aggregate_enabled(true, false, false));
        // The same orchestrate as a campaign sub-orchestrator stays silent so
        // the campaign parent owns the live surface.
        assert!(!orchestrate_aggregate_enabled(true, false, true));
        // --no-narrate / --quiet still win regardless.
        assert!(!orchestrate_aggregate_enabled(false, false, false));
        assert!(!orchestrate_aggregate_enabled(true, true, false));
    }

    const PLAN_JSON: &str = r#"{"tasks":[{"subject":"scaffold","goal":"g0","active_form":"scaffolding","depends_on":[]},{"subject":"sync","goal":"g1","active_form":"syncing","depends_on":["task-0"]}]}"#;

    #[test]
    fn parse_planner_response_reads_bare_object() {
        let tasks = parse_planner_response(PLAN_JSON).expect("bare object");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].depends_on, vec!["task-0".to_string()]);
    }

    #[test]
    fn parse_planner_response_survives_fenced_json_with_surrounding_prose() {
        // The real cli:claude-code failure shape: a ```json block wrapped in
        // prose whose braces defeat the old first-{-to-last-} slice.
        let content = format!(
            "Here's the plan — I split it into two slices:\n\n```json\n{PLAN_JSON}\n```\n\ntask-0 unblocks task-1 {{like so}}."
        );
        let tasks = parse_planner_response(&content).expect("fence-aware parse");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].subject, "scaffold");
    }

    #[test]
    fn parse_planner_response_rejects_non_json() {
        assert!(parse_planner_response("I could not produce a plan.").is_err());
    }
}

#[cfg(test)]
mod seed_graph_tests {
    use super::plan_tasks_from_seed;
    use crate::commands::course::CoursePiece;
    use deadreckon_core::plan::PlanProviders;
    use std::collections::BTreeMap;

    fn piece(id: &str, goal: &str, depends_on: &[&str]) -> CoursePiece {
        CoursePiece {
            id: id.to_string(),
            goal: goal.to_string(),
            done_hint: None,
            role: None,
            provider: None,
            model: None,
            budget_usd: None,
            depends_on: depends_on.iter().map(ToString::to_string).collect(),
            is_subplan: false,
        }
    }

    fn providers() -> PlanProviders {
        PlanProviders {
            default_child: Some("smoke".to_string()),
            ..Default::default()
        }
    }

    /// The edges the classifier drew must be the edges the executor runs.
    /// Before this, plan creation asked a *second* planner for the child
    /// graph, so an ordered goal could be previewed as ordered and then run
    /// as N independent nodes.
    #[test]
    fn the_classifier_graph_becomes_the_plan_tasks() {
        let seed = [
            piece("p1", "migrate the schema", &[]),
            piece("p2", "update the callers", &["p1"]),
            piece("p3", "delete the shim", &["p2"]),
        ];

        let tasks = plan_tasks_from_seed(&seed, 3, &providers(), &BTreeMap::new())
            .expect("seed becomes tasks");

        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].goal, "migrate the schema");
        assert!(tasks[0].depends_on.is_empty());
        assert_eq!(tasks[1].depends_on, vec!["task-0".to_string()]);
        assert_eq!(tasks[2].depends_on, vec!["task-1".to_string()]);
        assert!(
            tasks
                .iter()
                .all(|task| task.provider.as_deref() == Some("smoke")),
            "child provider routing still applies"
        );
    }

    /// `n` is the number the operator saw and confirmed on the preview.
    /// Planning a different number would make that confirmation meaningless,
    /// so a mismatched seed is refused and the planner fallback runs.
    #[test]
    fn a_seed_that_disagrees_with_the_confirmed_count_is_refused() {
        let seed = [piece("p1", "one", &[]), piece("p2", "two", &[])];

        assert!(plan_tasks_from_seed(&seed, 3, &providers(), &BTreeMap::new()).is_none());
    }

    #[test]
    fn a_seed_with_an_empty_goal_is_refused() {
        let seed = [piece("p1", "one", &[]), piece("p2", "   ", &[])];

        assert!(plan_tasks_from_seed(&seed, 2, &providers(), &BTreeMap::new()).is_none());
    }

    /// A cyclic seed is discarded wholesale rather than repaired; falling back
    /// to the planner is safer than executing a guess.
    #[test]
    fn a_cyclic_seed_is_refused() {
        let seed = [piece("p1", "one", &["p2"]), piece("p2", "two", &["p1"])];

        assert!(plan_tasks_from_seed(&seed, 2, &providers(), &BTreeMap::new()).is_none());
    }

    #[test]
    fn no_seed_falls_back_to_the_planner() {
        assert!(plan_tasks_from_seed(&[], 3, &providers(), &BTreeMap::new()).is_none());
    }
}

#[cfg(test)]
mod model_routing_tests {
    use super::{child_argv, child_model_for_task};
    use deadreckon_core::plan::{Plan, PlanProviders, PlanTask, PlanTaskStatus, TaskAttempt};

    fn task(index: u32) -> PlanTask {
        PlanTask {
            index,
            task_id: format!("task-{index}"),
            subject: "subject".to_string(),
            goal: "goal".to_string(),
            active_form: "working".to_string(),
            provider: None,
            role: deadreckon_core::plan::PlanRole::Child,
            depends_on: Vec::new(),
            subplan: None,
            attempts: Vec::new(),
            worker_spec: std::path::PathBuf::new(),
            summary_path: None,
            review_status: None,
            child_run_id: None,
            child_scope: None,
            status: PlanTaskStatus::Pending,
        }
    }

    fn providers(default_model: Option<&str>, overrides: &[(u32, &str)]) -> PlanProviders {
        PlanProviders {
            planner: None,
            default_child: Some("smoke".to_string()),
            coder: None,
            reviewer: None,
            children: Default::default(),
            planner_model: None,
            default_child_model: default_model.map(ToString::to_string),
            coder_model: None,
            reviewer_model: None,
            child_models: overrides
                .iter()
                .map(|(index, model)| (*index, (*model).to_string()))
                .collect(),
        }
    }

    #[test]
    fn child_model_idx_flag_overrides_default_child_model_in_spawn_argv() {
        let providers = providers(Some("child-default"), &[(1, "override-mx")]);
        assert_eq!(
            child_model_for_task(&providers, &task(1)),
            Some("override-mx")
        );
        assert_eq!(
            child_model_for_task(&providers, &task(0)),
            Some("child-default")
        );
    }

    #[test]
    fn provider_default_model_means_no_model_argv() {
        let providers = providers(Some("provider default"), &[]);
        assert_eq!(child_model_for_task(&providers, &task(0)), None);
    }

    fn narrate_plan() -> Plan {
        let mut coder = task(0);
        coder.subject = "coder".to_string();
        let mut reviewer = task(1);
        reviewer.subject = "reviewer".to_string();
        Plan::new(
            "goal",
            deadreckon_core::plan::PlanMode::FullPlan,
            vec![coder, reviewer],
            providers(None, &[]),
            None,
            "test",
        )
        .expect("plan")
    }

    fn failed_attempt(attempt: u32, run_id: Option<&str>, reason: &str) -> TaskAttempt {
        TaskAttempt::failed(
            attempt,
            run_id.map(ToString::to_string),
            Some(reason.to_string()),
            0.4,
        )
    }

    /// A retry must stay a `run`. `extend` refuses a parent that is not
    /// Completed, and a node that missed its gate is Failed by definition —
    /// an extend-based retry dies instantly with "cannot be extended yet".
    /// The partial work is carried by the source dir instead.
    #[test]
    fn a_retry_is_a_run_not_an_extend() {
        let mut plan = narrate_plan();
        plan.tasks[0].attempts.push(failed_attempt(
            1,
            Some("0abf0f7a"),
            "acceptance failed after turn 12",
        ));

        let argv = child_argv(
            &plan,
            &plan.tasks[0],
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            None,
        );

        assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
        assert!(
            argv.iter().any(|arg| arg == "--from"),
            "the retry is seeded from a source tree: {argv:?}"
        );
    }

    /// Per-node apply lands work with `deadreckon apply`, which needs real git
    /// ancestry. A copied source tree has none — the first cut of this failed
    /// on a live run with "missing source_git_root" — so a per-node child
    /// branches off the tip instead, exactly as a chain step does.
    #[test]
    fn a_per_node_child_branches_instead_of_copying() {
        let mut plan = narrate_plan();
        plan.apply = deadreckon_core::plan::ApplyWhen::PerNode;

        let argv = child_argv(
            &plan,
            &plan.tasks[0],
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            Some("abc123"),
        );

        assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
        assert!(argv.iter().any(|arg| arg == "--worktree"), "{argv:?}");
        assert!(
            argv.windows(2)
                .any(|pair| pair[0] == "--base" && pair[1] == "abc123"),
            "the child branches off the tip the parent measured: {argv:?}"
        );
        assert!(
            !argv.iter().any(|arg| arg == "--from"),
            "a per-node child must not copy a source tree: {argv:?}"
        );
    }

    #[test]
    fn an_at_end_child_still_copies_its_source() {
        let plan = narrate_plan();

        let argv = child_argv(
            &plan,
            &plan.tasks[0],
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            None,
        );

        assert!(argv.iter().any(|arg| arg == "--from"), "{argv:?}");
        assert!(!argv.iter().any(|arg| arg == "--worktree"), "{argv:?}");
    }

    #[test]
    fn the_apply_allowlist_bounds_what_may_land() {
        assert!(super::apply_allowlist_matches("*", "anything.rs"));
        assert!(super::apply_allowlist_matches("src/", "src/main.rs"));
        assert!(super::apply_allowlist_matches("src/*", "src/main.rs"));
        assert!(super::apply_allowlist_matches("Cargo.toml", "Cargo.toml"));
        assert!(!super::apply_allowlist_matches("src/", "docs/README.md"));
    }

    #[test]
    fn a_first_attempt_still_starts_a_fresh_run() {
        let plan = narrate_plan();

        let argv = child_argv(
            &plan,
            &plan.tasks[0],
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            None,
        );

        assert_eq!(argv.first().map(String::as_str), Some("run"), "{argv:?}");
    }

    #[test]
    fn a_reviewer_node_still_extends_its_coder_dependency() {
        let mut plan = narrate_plan();
        plan.tasks[1].role = deadreckon_core::plan::PlanRole::Reviewer;
        plan.tasks[1].depends_on = vec!["task-0".to_string()];
        plan.tasks[0].child_run_id = Some("0abf0f7a".to_string());

        let argv = child_argv(
            &plan,
            &plan.tasks[1],
            "review it",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            None,
        );

        assert_eq!(argv.first().map(String::as_str), Some("extend"), "{argv:?}");
        assert_eq!(
            argv.get(1).map(String::as_str),
            Some("0abf0f7a"),
            "{argv:?}"
        );
    }

    #[test]
    fn a_retry_prompt_leads_with_the_gate_complaint() {
        let mut plan = narrate_plan();
        plan.max_attempts = 3;
        plan.tasks[0].attempts.push(failed_attempt(
            1,
            Some("0abf0f7a"),
            "acceptance failed after turn 12: billing.rs missing",
        ));

        let prompt = super::plan_child_prompt(
            &plan,
            &plan.tasks[0],
            "spec body",
            std::path::Path::new("/spec.md"),
        );

        assert!(prompt.starts_with("RETRY 2 of 3"), "{prompt}");
        assert!(prompt.contains("billing.rs missing"), "{prompt}");
        assert!(
            prompt.contains("do not declare done until dr-gate passes"),
            "the retry must not be allowed to self-certify: {prompt}"
        );
        assert!(
            prompt.contains("Do not weaken, edit, or delete the done"),
            "a retry must not be able to pass by loosening the contract: {prompt}"
        );
    }

    #[test]
    fn a_first_attempt_prompt_has_no_retry_preamble() {
        let plan = narrate_plan();

        let prompt = super::plan_child_prompt(
            &plan,
            &plan.tasks[0],
            "spec body",
            std::path::Path::new("/spec.md"),
        );

        assert!(!prompt.contains("RETRY"), "{prompt}");
    }

    #[test]
    fn orchestrate_full_plan_with_narrate_appends_narrate_flag_to_each_child_argv() {
        let plan = narrate_plan();
        let task0 = &plan.tasks[0];
        let with = child_argv(
            &plan,
            task0,
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            true,
            None,
            None,
        );
        assert!(with.iter().any(|a| a == "--narrate"), "{with:?}");
        let without = child_argv(
            &plan,
            task0,
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            false,
            None,
            None,
        );
        assert!(
            !without.iter().any(|a| a == "--narrate"),
            "no --narrate without opt-in: {without:?}"
        );
    }

    #[test]
    fn plan_child_argv_pinned_test_updated_for_narrate_flag() {
        let plan = narrate_plan();
        let task0 = &plan.tasks[0];
        let argv = child_argv(
            &plan,
            task0,
            "do the thing",
            std::path::Path::new("/src"),
            false,
            "none",
            None,
            None,
            true,
            Some("haiku"),
            None,
        );
        // run child: leads with run/--from/--no-docs, then --narrate + model.
        assert_eq!(argv[0], "run");
        assert!(argv.iter().any(|a| a == "--no-docs"));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--narrator-model" && w[1] == "haiku"),
            "{argv:?}"
        );
    }
}
