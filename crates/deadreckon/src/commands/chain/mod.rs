use super::super::*;
use super::attach_runtime::*;
use crate::commands::acceptance::ensure_acceptance_before_start;
use crate::commands::attach::{attach_should_quit, resume_tui, suspend_tui};
use crate::tui::{ChainAttachTuiState, chain_event_read_hint, render_chain_attach};

fn print_chain_help(topic: Option<&str>) {
    let topic = topic.unwrap_or("overview");
    match topic {
        "plan" | "expand" => {
            println!("{}", ui_heading("deadreckon chain plan"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain plan \"large goal\" --n 4")
            );
            println!(
                "purpose: ask the configured provider to split a large goal into ordered steps"
            );
            print_chain_help_recommended("deadreckon chain run latest");
        }
        "run" | "resume" => {
            println!("{}", ui_heading("deadreckon chain run"));
            println!("usage: {}", ui_command("deadreckon chain run latest"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain resume latest --from-step 2")
            );
            println!("purpose: execute or continue the conductor for a chain");
            print_chain_help_recommended("deadreckon chain attach latest");
        }
        "attach" | "watch" => {
            println!("{}", ui_heading("deadreckon chain attach"));
            println!("usage: {}", ui_command("deadreckon chain attach latest"));
            println!(
                "purpose: open the chain TUI, including step timeline and live inner run status"
            );
            print_chain_help_recommended("deadreckon chain status latest");
        }
        "status" | "list" => {
            println!("{}", ui_heading("deadreckon chain status/list"));
            println!("usage: {}", ui_command("deadreckon chain status latest"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain list --all-scopes")
            );
            println!("purpose: find chains, summarize progress, and see the next action");
            print_chain_help_recommended("deadreckon chain show latest");
        }
        "show" => {
            println!("{}", ui_heading("deadreckon chain show"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain show latest --why-failed")
            );
            println!("purpose: inspect steps, policies, failures, applied SHAs, and run ids");
            print_chain_help_recommended("deadreckon chain resume latest");
        }
        "pause" | "kill" => {
            println!("{}", ui_heading("deadreckon chain pause/kill"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain pause latest --reason \"waiting on review\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain kill latest --escalate")
            );
            println!(
                "purpose: stop the conductor intentionally; kill also cascades to the live inner run"
            );
            print_chain_help_recommended("deadreckon chain resume latest");
        }
        "undo" | "redo" => {
            println!("{}", ui_heading("deadreckon chain undo/redo"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain undo latest --step 2")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain redo latest --step 2")
            );
            println!("purpose: back out or rerun an applied step with bounded chain state changes");
            print_chain_help_recommended("deadreckon chain show latest");
        }
        "extend" => {
            println!("{}", ui_heading("deadreckon chain extend"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\" --insert-at 2")
            );
            println!("purpose: add a new step to an existing chain");
            print_chain_help_recommended("deadreckon chain run latest");
        }
        "hooks" => {
            println!("{}", ui_heading("deadreckon chain hooks"));
            println!("usage: {}", ui_command("deadreckon chain hooks list"));
            println!("purpose: list lifecycle hook names supported by the conductor");
        }
        _ => {
            println!("{}", ui_heading("deadreckon chain"));
            println!("{CHAIN_HELP}");
            println!();
            println!("More help:");
            println!("  {}", ui_command("deadreckon chain help plan"));
            println!("  {}", ui_command("deadreckon chain help run"));
            println!("  {}", ui_command("deadreckon chain help pause"));
            println!("  {}", ui_command("deadreckon chain help undo"));
        }
    }
}

fn print_chain_help_recommended(command: &str) {
    println!("recommended: {}", ui_command(command));
}

pub(crate) async fn chain_command(args: ChainCommandArgs) -> Result<()> {
    let ChainCommandArgs {
        args,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n,
        no_hints,
        quiet,
        plain,
        reason,
        from_step,
        max_spend_add,
        reset_breaker,
        force,
        step,
        extend,
        reapply,
        insert_at,
        no_confirm,
        full,
        all,
        why_failed,
        json,
    } = args;
    let paths = DeadreckonPaths::discover();
    if args.first().is_some_and(|arg| arg == "help") {
        print_chain_help(args.get(1).map(String::as_str));
        return Ok(());
    }
    let Some(first) = args.first().map(String::as_str) else {
        if from_file.is_some() || from_stdin {
            let goals = collect_chain_goals(&[], from_file, from_stdin, no_hints)?;
            return chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ());
        }
        return chain_status_command(None, all, full, plain, json);
    };

    match first {
        "plan" | "expand" => {
            let root_goal = args.get(1).cloned().ok_or_else(|| {
                chain_create_refusal_surface(
                    VerdictKind::Blocked,
                    None,
                    "DeadReckon did not plan the chain because chain plan needs a goal.",
                    "The planner needs a root goal before it can decompose work into ordered chain steps.",
                    [
                        ("command".to_string(), "chain plan".to_string()),
                        ("goal".to_string(), "missing".to_string()),
                    ],
                    "deadreckon chain plan \"build the app\" --n 4".to_string(),
                    no_hints,
                )
            })?;
            chain_plan_command(ChainCreateOptions {
                paths,
                root_goal,
                goals: Vec::new(),
                from_file,
                from_stdin,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
        }
        "run" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            if id == "latest" || id == "last" {
                let latest = resolve_chain_id(&paths, id, all)?;
                return chain_run_command(
                    &paths,
                    &latest,
                    ChainRunOptions {
                        detach,
                        quiet,
                        plain,
                        from_step,
                        max_spend_add,
                        reset_breaker,
                        apply_mode: Some(apply_mode),
                        skip_acceptance_prompt: yes,
                    },
                )
                .await;
            }
            let id = resolve_chain_id(&paths, id, all)?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                    skip_acceptance_prompt: yes,
                },
            )
            .await
        }
        "resume" => {
            let id = resolve_chain_id(
                &paths,
                args.get(1).map(String::as_str).unwrap_or("latest"),
                all,
            )?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                    skip_acceptance_prompt: yes,
                },
            )
            .await
        }
        "status" => chain_status_command(args.get(1).map(String::as_str), all, full, plain, json),
        "list" => chain_list_command(all, full, json),
        "show" => chain_show_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            why_failed,
            json,
        ),
        "attach" => chain_attach_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            plain,
        ),
        "pause" => chain_pause_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            reason,
        ),
        "kill" => chain_kill_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            force,
        ),
        "undo" => chain_undo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            no_confirm,
        ),
        "extend" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            let step_goal = args.get(2).cloned().or(extend).ok_or_else(|| {
                chain_create_refusal_surface(
                    VerdictKind::Blocked,
                    None,
                    "DeadReckon did not extend the chain because chain extend needs a step goal.",
                    "Extending a chain mutates ordered chain state, so DeadReckon needs the new step goal before it can preview or write that change.",
                    [
                        ("command".to_string(), "chain extend".to_string()),
                        ("chain".to_string(), id.to_string()),
                        ("step goal".to_string(), "missing".to_string()),
                    ],
                    format!("deadreckon chain extend {id} \"add tests\""),
                    no_hints,
                )
            })?;
            chain_extend_command(&paths, id, step_goal, insert_at, max_spend_add)
        }
        "redo" => chain_redo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            extend,
            reapply,
        ),
        "hooks" if args.get(1).is_some_and(|arg| arg == "list") => chain_hooks_list_command(),
        maybe_id if args.len() == 1 && looks_like_chain_id(maybe_id) => {
            Err(chain_create_refusal_surface(
                VerdictKind::Blocked,
                Some(maybe_id),
                "DeadReckon did not run the chain because chain <id> is ambiguous.",
                "A chain id without an explicit verb could mean run, show, attach, pause, or another action, so DeadReckon refused before changing chain state.",
                [
                    ("command".to_string(), "chain <id>".to_string()),
                    ("chain".to_string(), maybe_id.to_string()),
                    ("verb".to_string(), "missing".to_string()),
                ],
                format!("deadreckon chain run {maybe_id}"),
                no_hints,
            ))
        }
        _ => {
            let goals = collect_chain_goals(&args, from_file, from_stdin, no_hints)?;
            chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ())
        }
    }
}

struct ChainCreateOptions {
    paths: DeadreckonPaths,
    root_goal: String,
    goals: Vec<String>,
    from_file: Option<PathBuf>,
    from_stdin: bool,
    draft: bool,
    yes: bool,
    detach: bool,
    branch_policy: String,
    apply_mode: String,
    apply_strategy: String,
    apply_allowlist: Vec<String>,
    on_fail: String,
    circuit_breaker_threshold: u32,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: String,
    base: Option<String>,
    n: u8,
    no_hints: bool,
    quiet: bool,
    plain: bool,
}

struct ChainRunOptions {
    detach: bool,
    quiet: bool,
    plain: bool,
    from_step: Option<u32>,
    max_spend_add: Option<f64>,
    reset_breaker: bool,
    apply_mode: Option<String>,
    skip_acceptance_prompt: bool,
}

async fn chain_plan_command(options: ChainCreateOptions) -> Result<()> {
    let n = options.n.clamp(2, 12);
    let paths = options.paths.clone();
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        options.provider.as_deref(),
        options.model.as_deref(),
    )?;
    let prompt = chain_planner_prompt(&options.root_goal, n);
    let response = with_cli_wait_status(
        "drafting chain plan",
        router.complete(&ProviderRequest {
            prompt,
            max_output_tokens: u32::from(n) * 96,
            cwd: Some(std::env::current_dir()?),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        }),
    )
    .await
    .map_err(|err| {
        chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            &format!("chain planner provider failed: {err}"),
            "DeadReckon could not get a valid decomposition from the configured provider, so it refused before writing chain state.",
            [
                ("goal".to_string(), options.root_goal.clone()),
                (
                    "provider".to_string(),
                    options.provider.as_deref().unwrap_or("default").to_string(),
                ),
            ],
            "deadreckon chain \"step one\" \"step two\"".to_string(),
            options.no_hints,
        )
    })?;
    let goals = parse_planner_goals(&response.content, n, &options.root_goal, options.no_hints)?;
    let chain_id = chain_create_command(ChainCreateOptions { goals, ..options }).await?;
    append_chain_planner_spend(&paths, &chain_id, &response)?;
    Ok(())
}

async fn chain_create_command(options: ChainCreateOptions) -> Result<String> {
    let ChainCreateOptions {
        paths,
        root_goal,
        mut goals,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n: _,
        no_hints,
        quiet,
        plain,
    } = options;
    if goals.is_empty() {
        goals = collect_chain_goals(&[], from_file, from_stdin, no_hints)?;
    }
    if goals.len() < 2 {
        let primary = goals
            .first()
            .map(|goal| format!("deadreckon run {}", quote_chain_goal_arg(goal)))
            .unwrap_or_else(|| "deadreckon run \"<goal>\"".to_string());
        return Err(chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            "DeadReckon did not create the chain because chain must have >= 2 steps.",
            "A single goal should run as a normal run; chain state is only for ordered multi-step work.",
            [
                ("requested steps".to_string(), goals.len().to_string()),
                ("minimum steps".to_string(), "2".to_string()),
            ],
            primary,
            no_hints,
        ));
    }
    if goals.len() > 12 {
        return Err(chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            "DeadReckon did not create the chain because chain capped at 12 steps.",
            "Longer step lists need to be split into multiple chains or replanned so one chain stays reviewable and recoverable.",
            [
                ("requested steps".to_string(), goals.len().to_string()),
                ("maximum steps".to_string(), "12".to_string()),
            ],
            "deadreckon chain plan \"<larger goal>\" --n 12".to_string(),
            no_hints,
        ));
    }
    let cwd = std::env::current_dir()?;
    let git_root = deadreckon_core::find_git_root(&cwd)?.ok_or_else(|| {
        chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            "DeadReckon did not create the chain because chains require a git repo.",
            "Chain runs coordinate branches, checkpoints, and step application against a git repository, so DeadReckon refused before writing chain state.",
            [
                ("cwd".to_string(), cwd.display().to_string()),
                ("git root".to_string(), "not found".to_string()),
            ],
            "git init".to_string(),
            no_hints,
        )
    })?;
    let scope = workspace_scope(&cwd).map_err(CliError::from)?;
    let base_ref = base.unwrap_or_else(|| "HEAD".to_string());
    let base_sha = git_stdout(&git_root, &["rev-parse", &base_ref])?;
    let base_branch = git_stdout(&git_root, &["symbolic-ref", "--short", "HEAD"])
        .unwrap_or_else(|_| base_ref.clone());
    let chain = Chain::new(ChainNewOptions {
        root_goal,
        goals,
        scope,
        base_branch,
        base_sha,
        cwd: git_root.clone(),
        provider,
        model,
        sandbox,
        branch_policy: parse_branch_policy(&branch_policy)?,
        apply_mode: parse_apply_mode(&apply_mode)?,
        apply_strategy: parse_apply_strategy(&apply_strategy)?,
        apply_allowlist,
        on_fail: parse_on_fail(&on_fail)?,
        circuit_breaker_threshold,
        max_spend_usd: max_spend,
        max_wall_seconds,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .map_err(CliError::from)?;
    save_chain(&paths, &chain)?;
    append_chain_event(
        &paths,
        &chain.chain_id,
        ChainEventKind::ChainCreated,
        None,
        json!({ "steps": chain.steps.len(), "draft": draft }),
    )?;
    if !quiet && (draft || !yes) {
        println!("{}", chain_preview(&chain));
    }
    if draft {
        if completion_hints_enabled(no_hints) && !quiet {
            println!(
                "drafted: {} with {} steps",
                chain.chain_id,
                chain.steps.len()
            );
            println!(
                "edit:    vim {}",
                paths.chain_json(&chain.chain_id).display()
            );
            println!(
                "run:     deadreckon chain run {}",
                chain_prefix(&chain.chain_id)
            );
        }
        return Ok(chain.chain_id);
    }
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(chain_create_refusal_surface(
                VerdictKind::Blocked,
                Some(&chain.chain_id),
                "DeadReckon did not start the chain because non-interactive chain start requires --yes.",
                "The chain was drafted, but this session cannot ask for launch confirmation, so DeadReckon stopped before starting the conductor.",
                [
                    ("chain".to_string(), chain_prefix(&chain.chain_id)),
                    ("steps".to_string(), chain.steps.len().to_string()),
                    ("stdin".to_string(), "non-interactive".to_string()),
                ],
                "deadreckon chain --yes \"step one\" \"step two\"".to_string(),
                no_hints,
            ));
        }
        if !prompt::confirm("start the chain?", true)? {
            println!("cancelled");
            return Ok(chain.chain_id);
        }
    }
    let chain_id = chain.chain_id.clone();
    let auto_attach = chain_should_auto_attach(io::stdout().is_terminal(), detach, quiet, plain);
    chain_run_command(
        &paths,
        &chain_id,
        ChainRunOptions {
            detach: detach || auto_attach,
            quiet,
            plain,
            from_step: None,
            max_spend_add: None,
            reset_breaker: false,
            apply_mode: None,
            skip_acceptance_prompt: yes,
        },
    )
    .await?;
    if auto_attach {
        chain_attach_command(&paths, &chain_id, false)?;
    }
    Ok(chain_id)
}

fn chain_create_refusal_surface<K, V>(
    kind: VerdictKind,
    chain_id: Option<&str>,
    what_happened: impl Into<String>,
    why_this_verdict: impl Into<String>,
    evidence: impl IntoIterator<Item = (K, V)>,
    primary: String,
    no_hints: bool,
) -> CliError
where
    K: Into<String>,
    V: Into<String>,
{
    let subject = chain_id.map(chain_prefix);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            kind,
            "chain",
            subject.as_deref(),
            ExplanationPanel::new(what_happened, why_this_verdict, evidence),
            [("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .expect("chain creation refusal surface must have one primary action")
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}

fn quote_chain_goal_arg(goal: &str) -> String {
    format!("\"{}\"", goal.replace('\\', "\\\\").replace('"', "\\\""))
}

async fn chain_run_command(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let chain_id = resolve_chain_id(paths, chain_id, false)?;
    ensure_chain_acceptance_before_start(paths, &chain_id, &options).await?;
    if options.detach {
        return detach_chain_conductor(paths, &chain_id, &options);
    }
    run_chain_conductor(paths, &chain_id, options).await
}

async fn ensure_chain_acceptance_before_start(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    if options.skip_acceptance_prompt || options.quiet || !io::stdin().is_terminal() {
        return Ok(());
    }
    let chain = load_chain(paths, chain_id)?;
    let _ = ensure_acceptance_before_start(
        &chain.cwd,
        None,
        &chain.root_goal,
        chain.provider.clone(),
        chain.model.clone(),
        false,
        "chain",
    )
    .await?;
    Ok(())
}

async fn run_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let mut chain = load_chain(paths, chain_id)?;
    if let Some(add) = options.max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    if options.reset_breaker {
        chain.circuit_breaker_consecutive_failures = 0;
    }
    if let Some(mode) = options.apply_mode.as_deref()
        && mode != "auto"
    {
        chain.apply_mode = parse_apply_mode(mode)?;
    }
    match chain.status {
        ChainStatus::Completed => {
            let id = chain_prefix(&chain.chain_id);
            return Err(CliError::Surface {
                code: 1,
                surface: chain_transition_surface(
                    paths,
                    &chain,
                    VerdictKind::Blocked,
                    "DeadReckon did not run the chain because the chain is completed.",
                    "Completed chain state is terminal, so inspection is the safest next command before redo or extension.",
                    vec![("requested action".to_string(), "chain run".to_string())],
                    format!("deadreckon chain show {id}"),
                    Vec::new(),
                )
                .render_plain(false),
            });
        }
        ChainStatus::Running if chain.conductor_pid.is_some_and(pid_is_alive) => {
            let id = chain_prefix(&chain.chain_id);
            let pid = chain.conductor_pid.unwrap_or_default();
            return Err(CliError::Surface {
                code: 1,
                surface: chain_transition_surface(
                    paths,
                    &chain,
                    VerdictKind::Blocked,
                    "DeadReckon did not start another conductor because the chain is already running.",
                    "Starting a second conductor would race the live chain state, so attaching to the existing run is the safest next command.",
                    vec![
                        ("requested action".to_string(), "chain run".to_string()),
                        ("conductor pid".to_string(), pid.to_string()),
                    ],
                    format!("deadreckon chain attach {id}"),
                    Vec::new(),
                )
                .render_plain(false),
            });
        }
        _ => {}
    }
    let mut lock = acquire_lock(
        paths,
        &chain.task_key(),
        &chain.chain_id,
        &chain.scope,
        "chain",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    chain.status = ChainStatus::Running;
    chain.started_at.get_or_insert_with(Utc::now);
    chain.paused_reason = None;
    chain.conductor_pid = Some(std::process::id());
    save_chain(paths, &chain)?;
    let conductor = ConductorState {
        schema_version: 1,
        chain_id: chain.chain_id.clone(),
        conductor_pid: std::process::id(),
        started_at: Utc::now(),
        live_step: None,
        live_run_id: None,
        live_child_pid: None,
    };
    fs::create_dir_all(paths.chain_dir(&chain.chain_id))?;
    fs::write(
        paths.conductor_json(&chain.chain_id),
        serde_json::to_vec_pretty(&conductor)?,
    )?;

    let start_index = options.from_step.unwrap_or(0);
    let mut completed = true;
    for index in 0..chain.steps.len() {
        if (index as u32) < start_index {
            continue;
        }
        if matches!(
            chain.steps[index].status,
            ChainStepStatus::Applied | ChainStepStatus::Skipped
        ) {
            continue;
        }
        lock.heartbeat(format!("step-{index}"))?;
        let step_cap = per_step_spend_cap(&chain, index);
        let step_wall_cap = per_step_wall_cap(&chain, index);
        let base_ref = chain_step_base_ref(&chain)?;
        match invoke_chain_hook(
            paths,
            &chain,
            "pre-step",
            Some(index as u32),
            json!({
                "chain_id": chain.chain_id,
                "step_index": index,
                "step_goal": chain.steps[index].goal,
                "base_ref": base_ref
            }),
        )? {
            1 => {
                chain.steps[index].status = ChainStepStatus::Skipped;
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainStepFailed,
                    Some(index as u32),
                    json!({ "reason": "skipped_by_pre_step_hook" }),
                )?;
                save_chain(paths, &chain)?;
                continue;
            }
            2 => {
                pause_chain_at_step(
                    paths,
                    &mut chain,
                    index,
                    "paused_by_pre_step_hook".to_string(),
                )?;
                completed = false;
                break;
            }
            _ => {}
        }
        let state = if chain.steps[index].status == ChainStepStatus::Completed {
            let run_id = chain.steps[index].run_id.clone().ok_or_else(|| {
                let id = chain_prefix(&chain.chain_id);
                let step_number = index + 1;
                CliError::Surface {
                    code: 1,
                    surface: chain_transition_surface(
                        paths,
                        &chain,
                        VerdictKind::Blocked,
                        &format!(
                            "DeadReckon did not run the chain because step {step_number} is completed but has no run id."
                        ),
                        "Completed chain steps must point at a recorded run before they can be replayed or applied. DeadReckon refused before continuing from inconsistent chain state.",
                        vec![
                            ("step".to_string(), step_number.to_string()),
                            ("step status".to_string(), "completed".to_string()),
                            ("run id".to_string(), "missing".to_string()),
                        ],
                        format!("deadreckon chain redo {id} --step {step_number}"),
                        Vec::new(),
                    )
                    .render_plain(false),
                }
            })?;
            load_run(paths, &run_id)?
        } else {
            chain.steps[index].status = ChainStepStatus::Running;
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainStepStarted,
                Some(index as u32),
                json!({ "goal": chain.steps[index].goal, "base": base_ref, "max_spend": step_cap, "max_wall_seconds": step_wall_cap }),
            )?;
            save_chain(paths, &chain)?;
            let run_id = match run_chain_step(
                paths,
                &chain,
                index,
                &base_ref,
                step_cap,
                step_wall_cap,
                options.quiet,
            )
            .await
            {
                Ok(run_id) => run_id,
                Err(err) => {
                    completed =
                        handle_chain_step_failure(paths, &mut chain, index, err.to_string())?;
                    if !completed {
                        break;
                    }
                    continue;
                }
            };
            chain.steps[index].run_id = Some(run_id.clone());
            let state = load_run(paths, &run_id)?;
            chain.steps[index].spend_usd = state.total_spend_usd;
            chain.total_spend_usd += state.total_spend_usd;
            chain.total_wall_seconds += state.total_wall_seconds;
            write_chain_step_marker(
                &state.working_dir,
                &ChainStepMarker::new(
                    &chain,
                    &chain.steps[index],
                    latest_applied_sha_before(&chain, index),
                ),
            )?;
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainRunCompleted,
                Some(index as u32),
                json!({ "run_id": run_id, "status": state.status.to_string() }),
            )?;
            if state.status != RunStatus::Completed {
                completed = handle_chain_step_failure(
                    paths,
                    &mut chain,
                    index,
                    format!("inner run {} ended {}", state.run_id, state.status),
                )?;
                if !completed {
                    break;
                }
                continue;
            }
            match invoke_chain_hook(
                paths,
                &chain,
                "post-step",
                Some(index as u32),
                json!({
                    "chain_id": chain.chain_id,
                    "step_index": index,
                    "run_id": state.run_id,
                    "status": state.status.to_string(),
                    "library_dir": state.promoted_library_dir
                }),
            )? {
                1 => {
                    pause_chain_at_step(
                        paths,
                        &mut chain,
                        index,
                        "paused_by_post_step_hook".to_string(),
                    )?;
                    completed = false;
                    break;
                }
                2 => {
                    completed = handle_chain_step_failure(
                        paths,
                        &mut chain,
                        index,
                        "refused_by_post_step_hook".to_string(),
                    )?;
                    if !completed {
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            chain.steps[index].status = ChainStepStatus::Completed;
            save_chain(paths, &chain)?;
            state
        };
        match chain.apply_mode {
            ApplyMode::Auto => {
                if let Err(err) =
                    auto_apply_chain_step(paths, &mut chain, index, &state.run_id, options.quiet)
                {
                    pause_chain_at_step(
                        paths,
                        &mut chain,
                        index,
                        chain_apply_refusal_pause_reason(&err.to_string()),
                    )?;
                    completed = false;
                    break;
                }
            }
            ApplyMode::Preview => {
                let diff_summary = preview_diff_summary_for_run(&state).unwrap_or_default();
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainApplyRefused,
                    Some(index as u32),
                    json!({ "reason": "apply_mode_preview", "diff_summary": diff_summary }),
                )?;
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_preview".to_string())?;
                completed = false;
                break;
            }
            ApplyMode::Manual => {
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_manual".to_string())?;
                completed = false;
                break;
            }
        }
        if chain_spend_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "cap".to_string())?;
            completed = false;
            break;
        }
        if chain_wall_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "wall_clock_cap".to_string())?;
            completed = false;
            break;
        }
    }
    if completed {
        let hook_status = invoke_chain_hook(
            paths,
            &chain,
            "on-chain-end",
            None,
            json!({
                "chain_id": chain.chain_id,
                "status": "completed",
                "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(),
                "total_spend_usd": chain.total_spend_usd
            }),
        )
        .unwrap_or_default();
        chain.status = ChainStatus::Completed;
        chain.completed_at = Some(Utc::now());
        chain.conductor_pid = None;
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainCompleted,
            None,
            json!({ "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(), "total_spend_usd": chain.total_spend_usd, "on_chain_end_status": hook_status }),
        )?;
        save_chain(paths, &chain)?;
        if !options.quiet {
            print!(
                "{}",
                chain_verdict_surface(paths, &chain).render_plain(false)
            );
        }
    } else if !options.quiet {
        print_chain_paused_footer(paths, &chain);
    }
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
    let _ = lock.release();
    Ok(())
}

async fn run_chain_step(
    paths: &DeadreckonPaths,
    chain: &Chain,
    index: usize,
    base_ref: &str,
    step_cap: Option<f64>,
    step_wall_cap: Option<f64>,
    quiet: bool,
) -> Result<String> {
    let step = &chain.steps[index];
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&chain.cwd)
        .env("DEADRECKON_HOME", paths.home())
        .arg("run")
        .arg(&step.goal)
        .arg("--worktree")
        .arg("--base")
        .arg(base_ref)
        .arg("--yes")
        .arg("--no-confirm")
        .arg("--no-hints")
        .arg("--sandbox")
        .arg(&chain.sandbox);
    if let Some(provider) = chain.provider.as_deref() {
        command.arg("--provider").arg(provider);
    }
    if let Some(model) = chain.model.as_deref() {
        command.arg("--model").arg(model);
    }
    if let Some(max_wall) = step_wall_cap {
        command.arg("--max-wall-seconds").arg(max_wall.to_string());
    }
    if let Some(step_cap) = step_cap {
        command.arg("--max-spend").arg(format!("{step_cap:.6}"));
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let child_pid = child.id();
    update_conductor_live(paths, chain, Some(index as u32), None, Some(child_pid))?;

    let stdout = child.stdout.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stdout".to_string(),
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stderr".to_string(),
        ))
    })?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let stdout_thread = spawn_chain_step_reader(stdout, true, tx.clone());
    let stderr_thread = spawn_chain_step_reader(stderr, false, tx);
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut live_run_id: Option<String> = None;
    let status = loop {
        while let Ok((is_stdout, line)) = rx.try_recv() {
            if let Some(run_id) = capture_chain_step_output(
                is_stdout,
                &line,
                &mut stdout_text,
                &mut stderr_text,
                quiet,
            )? && live_run_id.as_deref() != Some(run_id.as_str())
            {
                update_conductor_live(
                    paths,
                    chain,
                    Some(index as u32),
                    Some(run_id.clone()),
                    Some(child_pid),
                )?;
                live_run_id = Some(run_id);
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
        if let Some(run_id) =
            capture_chain_step_output(is_stdout, &line, &mut stdout_text, &mut stderr_text, quiet)?
            && live_run_id.as_deref() != Some(run_id.as_str())
        {
            update_conductor_live(
                paths,
                chain,
                Some(index as u32),
                Some(run_id.clone()),
                Some(child_pid),
            )?;
            live_run_id = Some(run_id);
        }
    }
    update_conductor_live(paths, chain, None, None, None)?;
    if !status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "step {} run failed: {}{}",
            index + 1,
            stdout_text,
            stderr_text
        ))));
    }
    live_run_id
        .or_else(|| parse_started_run_id(&stdout_text))
        .ok_or_else(missing_inner_run_id_error)
}

fn missing_inner_run_id_error() -> CliError {
    CliError::Exit {
        code: 1,
        message: "could not find inner run id in run output".to_string(),
        hint: "deadreckon list".to_string(),
    }
}

pub(crate) fn spawn_chain_step_reader<R: Read + Send + 'static>(
    reader: R,
    is_stdout: bool,
    tx: std::sync::mpsc::Sender<(bool, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(reader);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx.send((is_stdout, line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

pub(crate) fn capture_chain_step_output(
    is_stdout: bool,
    line: &str,
    stdout_text: &mut String,
    stderr_text: &mut String,
    quiet: bool,
) -> Result<Option<String>> {
    if is_stdout {
        stdout_text.push_str(line);
        if !quiet {
            print!("{line}");
            io::stdout().flush()?;
        }
        Ok(parse_started_run_id(stdout_text))
    } else {
        stderr_text.push_str(line);
        if !quiet {
            eprint!("{line}");
            io::stderr().flush()?;
        }
        Ok(None)
    }
}

fn auto_apply_chain_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    run_id: &str,
    quiet: bool,
) -> Result<()> {
    let state = load_run(paths, run_id)?;
    validate_acceptance_marker(&state)?;
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    if !git_stdout(git_root, &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("step '{}' refused auto-apply (dirty target)", index + 1),
            &format!(
                "git -C {} stash && deadreckon chain resume {}",
                git_root.display(),
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    if !chain.apply_allowlist.is_empty() {
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
            if !chain
                .apply_allowlist
                .iter()
                .any(|pattern| allowlist_matches(pattern, file))
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "step '{}' refused auto-apply (outside_allowlist {file})",
                        index + 1
                    ),
                    &format!(
                        "deadreckon chain resume {} --apply-mode preview",
                        chain_prefix(&chain.chain_id)
                    ),
                )));
            }
        }
    }
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    let files_changed = git_stdout(
        git_root,
        &["diff", "--name-only", &format!("HEAD..{branch}")],
    )?
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    let diff_stat =
        git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")]).unwrap_or_default();
    match invoke_chain_hook(
        paths,
        chain,
        "on-promote",
        Some(index as u32),
        json!({
            "chain_id": chain.chain_id,
            "step_index": index,
            "run_id": run_id,
            "diff_stat": diff_stat,
            "files_changed": files_changed
        }),
    )? {
        1 => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' paused by hook on-promote", index + 1),
                &format!("deadreckon chain resume {}", chain_prefix(&chain.chain_id)),
            )));
        }
        2 => {
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainApplyRefused,
                Some(index as u32),
                json!({ "reason": "refused_by_hook_on_promote" }),
            )?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' refused by hook on-promote", index + 1),
                "inspect ~/.deadreckon/hooks/chain/on-promote",
            )));
        }
        _ => {}
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplyStarted,
        Some(index as u32),
        json!({ "run_id": run_id }),
    )?;
    if quiet {
        super::lifecycle::apply_command_quiet(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
        )?;
    } else {
        super::lifecycle::apply_command(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
            false,
        )?;
    }
    let applied_sha = git_stdout(git_root, &["rev-parse", "HEAD"])?;
    chain.steps[index].status = ChainStepStatus::Applied;
    chain.steps[index].applied_at = Some(Utc::now());
    chain.steps[index].applied_sha = Some(applied_sha.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplied,
        Some(index as u32),
        json!({ "run_id": run_id, "applied_sha": applied_sha }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

// SAFETY: Chain failure reasons are owned when they are persisted and emitted as JSON.
#[allow(clippy::needless_pass_by_value)]
fn handle_chain_step_failure(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<bool> {
    chain.steps[index].status = ChainStepStatus::Failed;
    chain.steps[index].fail_reason = Some(reason.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepFailed,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    match chain.on_fail {
        OnFail::Stop => {
            pause_chain_at_step(paths, chain, index, "step_failed".to_string())?;
            Ok(false)
        }
        OnFail::Skip => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            chain.circuit_breaker_consecutive_failures += 1;
            if chain.circuit_breaker_consecutive_failures >= chain.circuit_breaker_threshold {
                pause_chain_at_step(paths, chain, index, "circuit_breaker_open".to_string())?;
                return Ok(false);
            }
            save_chain(paths, chain)?;
            Ok(true)
        }
        OnFail::Continue => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            save_chain(paths, chain)?;
            Ok(true)
        }
    }
}

// SAFETY: Pause reasons are command-boundary values that are stored and emitted atomically.
#[allow(clippy::needless_pass_by_value)]
fn pause_chain_at_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<()> {
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.clone());
    chain.conductor_pid = None;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

fn detach_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    fs::create_dir_all(paths.chain_dir(chain_id))?;
    let stdout = fs::File::create(paths.chain_dir(chain_id).join("conductor.out"))?;
    let stderr = fs::File::create(paths.chain_dir(chain_id).join("conductor.err"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("chain")
        .arg("run")
        .arg(chain_id)
        .arg("--quiet")
        .env("DEADRECKON_HOME", paths.home())
        .stdout(stdout)
        .stderr(stderr)
        .stdin(std::process::Stdio::null());
    if options.plain {
        command.arg("--plain");
    }
    let child = command.spawn()?;
    if !options.quiet {
        print!(
            "{}",
            chain_detached_surface(paths, chain_id, child.id()).render_plain(false)
        );
    }
    Ok(())
}

fn chain_detached_surface(
    paths: &DeadreckonPaths,
    chain_id: &str,
    conductor_pid: u32,
) -> VerdictSurface {
    let id = chain_prefix(chain_id);
    let primary = format!("deadreckon chain attach {id}");
    let secondary = [
        format!("deadreckon chain status {id}"),
        format!("deadreckon chain show {id}"),
    ];
    VerdictSurface::try_new(
        VerdictKind::Verified,
        "chain",
        Some(&id),
        ExplanationPanel::new(
            "DeadReckon started the chain conductor in the background.",
            "The detached conductor process was spawned successfully, so attaching is the primary next command to watch progress.",
            vec![
                ("chain", id.clone()),
                ("conductor pid", conductor_pid.to_string()),
                (
                    "state",
                    paths.chain_json(chain_id).display().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("chain detached verdict surface must have one primary action")
}

fn read_conductor_state(paths: &DeadreckonPaths, chain_id: &str) -> Result<Option<ConductorState>> {
    let path = paths.conductor_json(chain_id);
    match fs::read(&path) {
        Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn write_conductor_state(paths: &DeadreckonPaths, state: &ConductorState) -> Result<()> {
    let path = paths.conductor_json(&state.chain_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn update_conductor_live(
    paths: &DeadreckonPaths,
    chain: &Chain,
    live_step: Option<u32>,
    live_run_id: Option<String>,
    live_child_pid: Option<u32>,
) -> Result<()> {
    let mut conductor =
        read_conductor_state(paths, &chain.chain_id)?.unwrap_or_else(|| ConductorState {
            schema_version: 1,
            chain_id: chain.chain_id.clone(),
            conductor_pid: chain.conductor_pid.unwrap_or_else(std::process::id),
            started_at: chain.started_at.unwrap_or_else(Utc::now),
            live_step: None,
            live_run_id: None,
            live_child_pid: None,
        });
    conductor.live_step = live_step;
    conductor.live_run_id = live_run_id;
    conductor.live_child_pid = live_child_pid;
    write_conductor_state(paths, &conductor)
}

// SAFETY: Hook payloads are owned JSON messages written once to child process stdin.
#[allow(clippy::needless_pass_by_value)]
fn invoke_chain_hook(
    paths: &DeadreckonPaths,
    chain: &Chain,
    hook: &str,
    step_index: Option<u32>,
    payload: Value,
) -> Result<i32> {
    let Some(path) = resolve_chain_hook(paths, &chain.cwd, hook) else {
        return Ok(0);
    };
    let mut child = std::process::Command::new(&path)
        .env("DEADRECKON_CHAIN_ID", &chain.chain_id)
        .env("DEADRECKON_HOME", paths.home())
        .env(
            "DEADRECKON_STEP_INDEX",
            step_index
                .map(|index| index.to_string())
                .unwrap_or_else(|| "-".to_string()),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        serde_json::to_writer(&mut *stdin, &payload)?;
        stdin.write_all(b"\n")?;
    }
    let output = child.wait_with_output()?;
    let stdout = truncate_text(&String::from_utf8_lossy(&output.stdout), 4096);
    let stderr = truncate_text(&String::from_utf8_lossy(&output.stderr), 4096);
    let code = output.status.code().unwrap_or(1);
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainHookInvoked,
        step_index,
        json!({
            "hook": hook,
            "path": path,
            "status": code,
            "stdout": stdout,
            "stderr": stderr
        }),
    )?;
    Ok(code)
}

fn resolve_chain_hook(paths: &DeadreckonPaths, cwd: &Path, hook: &str) -> Option<PathBuf> {
    [
        cwd.join(".deadreckon/hooks/chain").join(hook),
        paths.home().join("hooks/chain").join(hook),
        PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn chain_status_command(
    id: Option<&str>,
    all: bool,
    full: bool,
    _plain: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if let Some(id) = id {
        return chain_show_command(&paths, id, false, json_output);
    }
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if json_output {
        print_chains_json(&chains)?;
        return Ok(());
    }
    if chains.is_empty() {
        println!(
            "{}",
            chain_empty_surface("status", !all).render_plain(!completion_hints_enabled(false))
        );
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_list_command(all: bool, full: bool, json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if json_output {
        print_chains_json(&chains)?;
        return Ok(());
    }
    if chains.is_empty() {
        println!(
            "{}",
            chain_empty_surface("list", !all).render_plain(!completion_hints_enabled(false))
        );
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_empty_surface(command: &str, scoped: bool) -> VerdictSurface {
    let scope = if scoped {
        "current scope"
    } else {
        "all scopes"
    };
    VerdictSurface::try_new(
        VerdictKind::Noop,
        format!("chain {command}"),
        None,
        ExplanationPanel::new(
            format!("DeadReckon found no chains in {scope}."),
            "The command was read-only and there is no chain state to inspect yet.",
            vec![
                ("command", format!("deadreckon chain {command}")),
                ("scope", scope.to_string()),
                ("chains", "0".to_string()),
            ],
        ),
        vec![("Recommended", "deadreckon chain \"step one\" \"step two\"")],
        Vec::<(&str, &str)>::new(),
    )
    .expect("empty chain verdict surface must have one primary action")
}

fn chain_show_command(
    paths: &DeadreckonPaths,
    id: &str,
    why_failed: bool,
    json_output: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    if json_output {
        let surface = chain_verdict_surface(paths, &chain);
        let value = surface.add_to_json(json!({
            "kind": "chain",
            "id": &chain.chain_id,
            "status": chain_status_label(&chain),
            "next_actions": [surface.primary_action.command.clone()],
            "try_lines": Vec::<String>::new(),
            "paths": {
                "chain": paths.chain_json(&chain.chain_id),
            },
            "chain": chain,
        }));
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if why_failed {
        print!(
            "{}",
            chain_verdict_surface(paths, &chain).render_plain(false)
        );
        return Ok(());
    }
    print!(
        "{}",
        chain_verdict_surface(paths, &chain).render_plain(false)
    );
    println!();
    println!("Steps");
    print_chain_header(paths, &chain);
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}{}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 72),
            step.run_id
                .as_deref()
                .map(|run_id| format!(" run={}", run_prefix(run_id)))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn chain_verdict_surface(paths: &DeadreckonPaths, chain: &Chain) -> VerdictSurface {
    let id = chain_prefix(&chain.chain_id);
    let failed_step = chain
        .steps
        .iter()
        .find(|step| step.status == ChainStepStatus::Failed || step.fail_reason.is_some());
    let reason = chain
        .failure_reason
        .as_deref()
        .or(chain.paused_reason.as_deref());
    let (kind, what, why) = match chain.status {
        ChainStatus::Completed => (
            VerdictKind::Completed,
            "The chain reached a terminal completed state.",
            "All required chain steps are complete, so the safest next command is inspection.",
        ),
        ChainStatus::Failed => (
            VerdictKind::Failed,
            "The chain stopped before all required steps completed.",
            "Failure inspection is the safest next command before resuming, skipping, or applying any step.",
        ),
        ChainStatus::Paused => (
            VerdictKind::Paused,
            chain_paused_what(reason),
            chain_paused_why(reason),
        ),
        ChainStatus::Killed => (
            VerdictKind::Killed,
            "The chain was stopped before reaching a terminal result.",
            "Killed chain state should be inspected before cleanup or relaunch.",
        ),
        ChainStatus::Running => (
            VerdictKind::Paused,
            "The chain is still running.",
            "Attaching is the safest next command because active conductor state may still exist.",
        ),
        ChainStatus::Pending => (
            VerdictKind::Preview,
            "The chain has been planned but has not started running.",
            "Resume starts or continues the stored chain state.",
        ),
        ChainStatus::Undone => (
            VerdictKind::Noop,
            "The chain has already been undone.",
            "Inspection is the safest next command because there is no active chain work to advance.",
        ),
    };
    let mut evidence = chain_base_evidence(paths, chain);
    if let Some(reason) = reason {
        evidence.push(("reason".to_string(), reason.to_string()));
    }
    if chain.status == ChainStatus::Paused
        && reason == Some("apply_mode_preview")
        && let Some(diff_summary) = latest_chain_preview_diff_summary(paths, chain)
    {
        evidence.push(("diff summary".to_string(), diff_summary));
    }
    if let Some(step) = failed_step {
        evidence.push((
            "failed step".to_string(),
            format!(
                "{} {}",
                step.index + 1,
                chain_step_status_label(step.status)
            ),
        ));
        if let Some(run_id) = step.run_id.as_deref() {
            evidence.push(("failed run".to_string(), run_prefix(run_id)));
        }
        if let Some(reason) = step.fail_reason.as_deref() {
            evidence.push(("step reason".to_string(), reason.to_string()));
        }
    }
    let primary = chain_primary_action(chain);
    let secondary = chain_secondary_actions(chain, &primary);
    VerdictSurface::try_new(
        kind,
        "chain",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("chain verdict surface must have one primary action")
}

fn latest_chain_preview_diff_summary(paths: &DeadreckonPaths, chain: &Chain) -> Option<String> {
    let raw = fs::read_to_string(paths.chain_events(&chain.chain_id)).ok()?;
    raw.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<ChainEvent>(line).ok())
        .find(|event| {
            event.event == ChainEventKind::ChainApplyRefused
                && event
                    .detail
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    == Some("apply_mode_preview")
        })
        .map(|event| {
            event
                .detail
                .get("diff_summary")
                .and_then(serde_json::Value::as_str)
                .map(compact_chain_diff_summary)
                .unwrap_or_else(|| "no diff".to_string())
        })
}

fn compact_chain_diff_summary(summary: &str) -> String {
    let mut lines = summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "no diff".to_string();
    }
    if let Some(index) = lines
        .iter()
        .rposition(|line| line.contains("file changed") || line.contains("files changed"))
    {
        return lines.swap_remove(index).to_string();
    }
    lines[0].to_string()
}

fn chain_transition_surface(
    paths: &DeadreckonPaths,
    chain: &Chain,
    kind: VerdictKind,
    what: &str,
    why: &str,
    mut evidence: Vec<(String, String)>,
    primary: String,
    secondary: Vec<String>,
) -> VerdictSurface {
    let id = chain_prefix(&chain.chain_id);
    let mut all_evidence = chain_base_evidence(paths, chain);
    all_evidence.append(&mut evidence);
    VerdictSurface::try_new(
        kind,
        "chain",
        Some(&id),
        ExplanationPanel::new(what, why, all_evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|command| ("Secondary", command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("chain transition verdict surface must have one primary action")
}

fn chain_base_evidence(paths: &DeadreckonPaths, chain: &Chain) -> Vec<(String, String)> {
    let completed = chain
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step.status,
                ChainStepStatus::Completed | ChainStepStatus::Applied
            )
        })
        .count();
    vec![
        ("chain".to_string(), chain_prefix(&chain.chain_id)),
        ("status".to_string(), chain_status_label(chain).to_string()),
        (
            "steps".to_string(),
            format!("{completed}/{} complete", chain.steps.len()),
        ),
        (
            "state".to_string(),
            paths.chain_json(&chain.chain_id).display().to_string(),
        ),
    ]
}

fn chain_primary_action(chain: &Chain) -> String {
    let id = chain_prefix(&chain.chain_id);
    match chain.status {
        ChainStatus::Failed => format!("deadreckon chain show {id} --why-failed"),
        ChainStatus::Paused if chain_paused_for_dirty_target(chain) => {
            format!("git -C {} status --short", chain.cwd.display())
        }
        ChainStatus::Paused if chain_paused_for_hook_refusal(chain) => {
            "deadreckon chain hooks list".to_string()
        }
        ChainStatus::Paused if chain_paused_for_outside_allowlist(chain) => {
            format!("deadreckon chain resume {id} --apply-mode preview")
        }
        ChainStatus::Paused | ChainStatus::Pending => format!("deadreckon chain resume {id}"),
        ChainStatus::Running => format!("deadreckon chain attach {id}"),
        ChainStatus::Completed | ChainStatus::Undone => format!("deadreckon chain show {id}"),
        ChainStatus::Killed => format!("deadreckon chain show {id} --why-failed"),
    }
}

fn chain_paused_for_dirty_target(chain: &Chain) -> bool {
    chain
        .paused_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("apply_refused_dirty_target"))
}

fn chain_paused_for_hook_refusal(chain: &Chain) -> bool {
    chain
        .paused_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("apply_refused_by_hook_on_promote"))
}

fn chain_paused_for_outside_allowlist(chain: &Chain) -> bool {
    chain
        .paused_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("apply_refused_outside_allowlist"))
}

fn chain_paused_what(reason: Option<&str>) -> &'static str {
    if reason.is_some_and(|reason| reason.starts_with("apply_refused_by_hook_on_promote")) {
        "The chain paused because the on-promote hook refused auto-apply."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_paused_by_hook_on_promote")) {
        "The chain paused because the on-promote hook requested an operator pause."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_refused_dirty_target")) {
        "The chain paused because auto-apply found uncommitted target changes."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_refused_outside_allowlist")) {
        "The chain paused because auto-apply refused a file outside the apply allowlist."
    } else {
        "The chain is paused before reaching a terminal result."
    }
}

fn chain_paused_why(reason: Option<&str>) -> &'static str {
    if reason.is_some_and(|reason| reason.starts_with("apply_refused_by_hook_on_promote")) {
        "The hook policy blocked promotion, so inspecting configured chain hooks is the primary next command before resuming."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_paused_by_hook_on_promote")) {
        "The hook requested a resumable pause, so resuming the chain is the primary next command after the operator check."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_refused_dirty_target")) {
        "The source repo has local changes, so inspecting the dirty state is the primary next command before stashing, committing, or resuming."
    } else if reason.is_some_and(|reason| reason.starts_with("apply_refused_outside_allowlist")) {
        "Previewing the diff is the primary next command before widening the allowlist or manually applying the step."
    } else {
        "The chain still has resumable state, so resume is the primary next command."
    }
}

fn chain_secondary_actions(chain: &Chain, primary: &str) -> Vec<String> {
    let id = chain_prefix(&chain.chain_id);
    let mut actions = Vec::new();
    let mut candidates = vec![
        format!("deadreckon chain attach {id}"),
        format!("deadreckon chain show {id}"),
        format!("deadreckon chain show {id} --why-failed"),
        format!("deadreckon chain resume {id}"),
    ];
    if chain.status == ChainStatus::Paused {
        candidates.push(format!("deadreckon chain resume {id} --apply-mode preview"));
        candidates.push(format!("deadreckon chain undo {id}"));
    }
    for command in candidates {
        if command != primary && !actions.contains(&command) {
            actions.push(command);
        }
    }
    actions
}

fn print_chains_json(chains: &[Chain]) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "kind": "chain_list",
            "id": "chains",
            "status": "ok",
            "next_actions": ["deadreckon chain status latest"],
            "try_lines": Vec::<String>::new(),
            "paths": {
                "home": paths.home(),
            },
            "chains": chains,
        }))?
    );
    Ok(())
}

fn chain_progress(chain: &Chain) -> String {
    format!(
        "{}/{}",
        chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count(),
        chain.steps.len()
    )
}

fn chain_spend_label(chain: &Chain) -> String {
    format!(
        "${:.6} / {}",
        chain.total_spend_usd,
        chain
            .max_spend_usd
            .map(|value| format!("${value:.6}"))
            .unwrap_or_else(|| "uncapped".to_string())
    )
}

fn chain_policy_label(chain: &Chain) -> String {
    format!(
        "branch={} apply={} strategy={} on-fail={} base={}@{}",
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        apply_strategy_label(chain_apply_strategy(chain)),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha)
    )
}

fn chain_header_items(paths: &DeadreckonPaths, chain: &Chain) -> Vec<(&'static str, String)> {
    vec![
        (
            "chain",
            format!("{} ({})", chain_prefix(&chain.chain_id), chain.chain_id),
        ),
        ("status", chain_status_label(chain).to_string()),
        ("steps", chain_progress(chain)),
        ("spend", chain_spend_label(chain)),
        ("policy", chain_policy_label(chain)),
        ("cwd", chain.cwd.display().to_string()),
        (
            "path",
            paths.chain_json(&chain.chain_id).display().to_string(),
        ),
    ]
}

fn print_chain_header(paths: &DeadreckonPaths, chain: &Chain) {
    println!("{}", ui_heading("chain"));
    let items = chain_header_items(paths, chain);
    print_kv_block(&items);
}

pub(crate) fn chain_attach_command(paths: &DeadreckonPaths, id: &str, plain: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    if io::stdout().is_terminal() && !plain {
        print_attach_banner("chain", &id);
        return chain_attach_tui(paths, &id);
    }
    print_chain_attach_snapshot(&chain);
    Ok(())
}

fn print_chain_attach_snapshot(chain: &Chain) {
    let paths = DeadreckonPaths::discover();
    print_chain_header(&paths, chain);
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 80)
        );
    }
    println!("[r] redo  [e] extend  [p] pause  [k] kill  [Ctrl-D] detach  [q] quit");
}

pub(crate) fn chain_attach_summary_line(chain: &Chain) -> String {
    let spend = chain_spend_label(chain).replace(" / ", "/");
    format!(
        "{}  status {}  steps {}  spend {}",
        chain_prefix(&chain.chain_id),
        chain_status_label(chain),
        chain_progress(chain),
        spend
    )
}

fn chain_attach_tui(paths: &DeadreckonPaths, chain_id: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = ChainAttachTuiState::default();
    let mut event_tail = AttachJsonlTail::<ChainEvent>::new(paths.chain_events(chain_id));

    let result = loop {
        let budget = AttachTickBudget::default();
        let mut tick = AttachTickTiming::new(AttachSurface::Chain, budget);
        let stage_started = Instant::now();
        let chain = load_chain(paths, chain_id)?;
        tick.record_since(AttachLoopStage::LoadState, stage_started);
        let stage_started = Instant::now();
        event_tail.reset_to_path(paths.chain_events(chain_id));
        let event_refresh_error = event_tail.refresh().err();
        let event_read_elapsed = stage_started.elapsed();
        tick.record_since(AttachLoopStage::ReadJsonl, stage_started);
        tui_state.event_status_hint = chain_event_read_hint(
            event_tail.rows().len(),
            event_tail.last_appended_rows,
            event_tail.partial_bytes,
            event_read_elapsed,
            budget,
            event_refresh_error.as_ref(),
        );
        let events = event_tail.rows();
        tui_state.clamp(&chain);
        let stage_started = Instant::now();
        terminal.draw(|frame| render_chain_attach(frame, &chain, events, &tui_state))?;
        tick.record_since(AttachLoopStage::Draw, stage_started);

        let stage_started = Instant::now();
        let input_ready = event::poll(Duration::from_millis(200))?;
        tick.record_since(AttachLoopStage::InputPoll, stage_started);
        drop(tick.slow_sync_stages());
        drop(tick.slow_stage_labels());
        let _ = tick.frame_exceeded();
        if input_ready {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key) => match key.code {
                    KeyCode::Enter => {
                        suspend_tui(&mut terminal)?;
                        if let Some(run_id) = chain
                            .steps
                            .get(tui_state.selected_step)
                            .and_then(|step| step.run_id.clone())
                        {
                            let _ = show_command(&run_id, None, false, false, false, false, None);
                        } else {
                            eprintln!("selected step has no run yet");
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('r') => {
                        suspend_tui(&mut terminal)?;
                        let action = chain_redo_command(
                            paths,
                            chain_id,
                            Some(tui_state.selected_step as u32 + 1),
                            None,
                            false,
                        );
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('e') => {
                        suspend_tui(&mut terminal)?;
                        let goal = prompt::open("new chain step goal: ", None)?;
                        if !goal.trim().is_empty() {
                            let action = chain_extend_command(paths, chain_id, goal, None, None);
                            if let Err(err) = &action {
                                print_error(err);
                            }
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('p') => {
                        suspend_tui(&mut terminal)?;
                        let action =
                            chain_pause_command(paths, chain_id, Some("user_paused".to_string()));
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('k') => {
                        suspend_tui(&mut terminal)?;
                        if prompt::confirm("kill chain?", false)?
                            && let Err(err) = chain_kill_command(paths, chain_id, false)
                        {
                            print_error(&err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    _ => tui_state.handle_key(key, &chain),
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => tui_state.scroll(1, &chain),
                    MouseEventKind::ScrollUp => tui_state.scroll(-1, &chain),
                    _ => {}
                },
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

fn chain_pause_command(paths: &DeadreckonPaths, id: &str, reason: Option<String>) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status != ChainStatus::Running {
        let status = chain_status_label(&chain).to_string();
        let primary = format!("deadreckon chain status {}", chain_prefix(&chain.chain_id));
        return Err(CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Blocked,
                &format!("DeadReckon did not pause the chain because cannot pause '{status}' chain."),
                "Only a running chain can be paused; this chain is already outside the active conductor state.",
                vec![
                    ("requested transition".to_string(), "pause".to_string()),
                    ("required status".to_string(), "running".to_string()),
                ],
                primary,
                Vec::new(),
            )
            .render_plain(false),
        });
    }
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.unwrap_or_else(|| "user_paused".to_string()));
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        None,
        json!({ "reason": chain.paused_reason }),
    )?;
    print!(
        "{}",
        chain_verdict_surface(paths, &chain).render_plain(false)
    );
    Ok(())
}

pub(crate) fn chain_kill_command(paths: &DeadreckonPaths, id: &str, force: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let conductor = read_conductor_state(paths, &id)?;
    let mut signaled_pids = BTreeSet::new();
    if let Some(run_id) = conductor
        .as_ref()
        .and_then(|state| state.live_run_id.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Running)
                .and_then(|step| step.run_id.clone())
        })
        && let Ok(mut state) = load_run(paths, &run_id)
    {
        kill_loaded_run(paths, &mut state, force)?;
        signaled_pids.extend(supervised_pids(&state));
    }
    if let Some(pid) = conductor.as_ref().and_then(|state| state.live_child_pid) {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if let Some(pid) = conductor
        .as_ref()
        .map(|state| state.conductor_pid)
        .or(chain.conductor_pid)
        && pid != std::process::id()
    {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if !force {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while signaled_pids
            .iter()
            .any(|pid| *pid != std::process::id() && pid_is_alive(*pid))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for pid in &signaled_pids {
            if *pid != std::process::id() && pid_is_alive(*pid) {
                terminate_pid(*pid, true)?;
            }
        }
    }
    chain.status = ChainStatus::Killed;
    chain.failure_reason = Some("killed by user".to_string());
    chain.conductor_pid = None;
    for step in &mut chain.steps {
        if step.status == ChainStepStatus::Running {
            step.status = ChainStepStatus::Failed;
            step.fail_reason = Some("killed by user".to_string());
        }
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainKilled,
        None,
        json!({ "force": force }),
    )?;
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
    print!(
        "{}",
        chain_verdict_surface(paths, &chain).render_plain(false)
    );
    Ok(())
}

fn chain_undo_command(
    paths: &DeadreckonPaths,
    id: &str,
    through_step: Option<u32>,
    no_confirm: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let mut applied = chain
        .steps
        .iter()
        .filter(|step| step.status == ChainStepStatus::Applied)
        .filter(|step| through_step.is_none_or(|limit| step.index < limit))
        .filter_map(|step| {
            step.applied_sha
                .as_deref()
                .map(|sha| (step.index, sha.to_string()))
        })
        .collect::<Vec<_>>();
    if applied.is_empty() {
        let id = chain_prefix(&chain.chain_id);
        return Err(CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Noop,
                "DeadReckon did not undo the chain because there is nothing to undo.",
                "Undo only applies to chain steps with applied commits, so inspection is the safest next command.",
                vec![("applied steps".to_string(), "0".to_string())],
                format!("deadreckon chain show {id}"),
                Vec::new(),
            )
            .render_plain(false),
        });
    }
    if !no_confirm && io::stdin().is_terminal() {
        if !prompt::confirm("undo applied chain commits?", false)? {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm {
        let id = chain_prefix(&chain.chain_id);
        return Err(CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Blocked,
                "DeadReckon did not undo the chain because non-interactive chain undo requires --no-confirm.",
                "Undo reverts applied commits, and this session cannot ask for confirmation, so DeadReckon stopped before changing git state.",
                vec![("stdin".to_string(), "non-interactive".to_string())],
                format!("deadreckon chain undo {id} --no-confirm"),
                Vec::new(),
            )
            .render_plain(false),
        });
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainUndoStarted,
        None,
        json!({ "count": applied.len() }),
    )?;
    let undo_count = applied.len();
    applied.reverse();
    for (index, sha) in applied {
        git_status(&chain.cwd, &["revert", "--no-edit", &sha])?;
        if let Some(step) = chain.steps.iter_mut().find(|step| step.index == index) {
            step.status = ChainStepStatus::Undone;
        }
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainUndoneStep,
            Some(index),
            json!({ "sha": sha }),
        )?;
    }
    chain.status = ChainStatus::Undone;
    save_chain(paths, &chain)?;
    let id = chain_prefix(&chain.chain_id);
    print!(
        "{}",
        chain_transition_surface(
            paths,
            &chain,
            VerdictKind::Noop,
            "DeadReckon reverted the applied chain commits and marked the chain undone.",
            "There is no active chain work to advance, so inspection is the safest next command.",
            vec![("undone steps".to_string(), undo_count.to_string())],
            format!("deadreckon chain show {id}"),
            vec![format!("deadreckon chain redo {id}")],
        )
        .render_plain(false)
    );
    Ok(())
}

fn chain_extend_command(
    paths: &DeadreckonPaths,
    id: &str,
    step_goal: String,
    insert_at: Option<u32>,
    max_spend_add: Option<f64>,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status == ChainStatus::Completed && insert_at.is_none() {
        let id = chain_prefix(&chain.chain_id);
        return Err(CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Blocked,
                "DeadReckon did not extend the chain because it cannot extend completed chain at end.",
                "A completed chain can only be reopened by inserting a step at a specific position, so DeadReckon refused before mutating chain state.",
                vec![("insert-at".to_string(), "missing".to_string())],
                format!("deadreckon chain extend {id} \"...\" --insert-at <N>"),
                Vec::new(),
            )
            .render_plain(false),
        });
    }
    if let Some(add) = max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    let insert = insert_at
        .map(|value| value.saturating_sub(1) as usize)
        .unwrap_or(chain.steps.len())
        .min(chain.steps.len());
    chain.steps.insert(
        insert,
        deadreckon_core::ChainStep::new(insert as u32, step_goal),
    );
    for (index, step) in chain.steps.iter_mut().enumerate() {
        step.index = index as u32;
    }
    if chain.status == ChainStatus::Completed {
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("extended".to_string());
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepExtended,
        Some(insert as u32),
        json!({ "insert_at": insert }),
    )?;
    let id = chain_prefix(&chain.chain_id);
    print!(
        "{}",
        chain_transition_surface(
            paths,
            &chain,
            VerdictKind::Preview,
            "DeadReckon queued a new chain step.",
            "The chain has stored work that has not run yet, so resume is the safest next command.",
            vec![("inserted step".to_string(), (insert + 1).to_string())],
            format!("deadreckon chain resume {id}"),
            vec![format!("deadreckon chain show {id}")],
        )
        .render_plain(false)
    );
    Ok(())
}

fn chain_redo_command(
    paths: &DeadreckonPaths,
    id: &str,
    step: Option<u32>,
    extend: Option<String>,
    reapply: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let index = step
        .map(|step| step.saturating_sub(1))
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Failed)
                .map(|step| step.index)
        })
        .or_else(|| {
            chain
                .steps
                .iter()
                .rev()
                .find(|step| step.status == ChainStepStatus::Applied)
                .map(|step| step.index)
        })
        .ok_or_else(|| {
            let id = chain_prefix(&chain.chain_id);
            CliError::Surface {
                code: 1,
                surface: chain_transition_surface(
                    paths,
                    &chain,
                    VerdictKind::Noop,
                    "DeadReckon did not redo a chain step because no failed or applied step to redo.",
                    "The default redo target is the first failed step or the latest applied step. This chain has no eligible redo candidate, so DeadReckon left chain state unchanged.",
                    vec![("redo candidate".to_string(), "none".to_string())],
                    format!("deadreckon chain show {id}"),
                    Vec::new(),
                )
                .render_plain(false),
            }
        })? as usize;
    let selected_step = chain.steps.get(index).ok_or_else(|| {
        let id = chain_prefix(&chain.chain_id);
        CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Blocked,
                &format!("DeadReckon did not redo the step because step {} does not exist.", index + 1),
                "The requested step is outside the chain's stored step list, so DeadReckon refused before mutating chain state.",
                vec![
                    ("requested step".to_string(), (index + 1).to_string()),
                    ("step count".to_string(), chain.steps.len().to_string()),
                ],
                format!("deadreckon chain show {id}"),
                Vec::new(),
            )
            .render_plain(false),
        }
    })?;
    if selected_step.status == ChainStepStatus::Applied && !reapply {
        let step_number = index + 1;
        let primary = format!(
            "deadreckon chain redo {} --step {step_number} --reapply",
            chain_prefix(&chain.chain_id)
        );
        return Err(CliError::Surface {
            code: 1,
            surface: chain_transition_surface(
                paths,
                &chain,
                VerdictKind::Blocked,
                &format!("DeadReckon did not redo the step because step '{step_number}' already applied; redo needs --reapply."),
                "Redoing an applied step requires explicit reapply consent because DeadReckon may need to revert a previously applied commit before replaying the step.",
                vec![
                    ("requested step".to_string(), step_number.to_string()),
                    (
                        "step status".to_string(),
                        chain_step_status_label(selected_step.status).to_string(),
                    ),
                ],
                primary,
                Vec::new(),
            )
            .render_plain(false),
        });
    }
    let step = chain.steps.get_mut(index).expect("validated step index");
    if reapply && let Some(sha) = step.applied_sha.as_deref() {
        git_status(&chain.cwd, &["revert", "--no-edit", sha])?;
    }
    let prior_goal = step.goal.clone();
    if let Some(extend) = extend {
        step.goal = extend;
    }
    step.status = ChainStepStatus::Pending;
    step.run_id = None;
    step.applied_at = None;
    step.applied_sha = None;
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepRedone,
        Some(index as u32),
        json!({ "prior_goal": prior_goal, "new_goal": chain.steps[index].goal }),
    )?;
    let id = chain_prefix(&chain.chain_id);
    print!(
        "{}",
        chain_transition_surface(
            paths,
            &chain,
            VerdictKind::Preview,
            "DeadReckon reset a chain step for redo.",
            "The selected step is pending again, so resume is the safest next command.",
            vec![("redo step".to_string(), (index + 1).to_string())],
            format!("deadreckon chain resume {id}"),
            vec![format!("deadreckon chain show {id}")],
        )
        .render_plain(false)
    );
    Ok(())
}

fn chain_hooks_list_command() -> Result<()> {
    let paths = DeadreckonPaths::discover();
    for hook in ["pre-step", "post-step", "on-promote", "on-chain-end"] {
        let project = std::env::current_dir()?
            .join(".deadreckon/hooks/chain")
            .join(hook);
        let user = paths.home().join("hooks/chain").join(hook);
        let repo = PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook);
        let (tier, path) = if project.exists() {
            ("project", project)
        } else if user.exists() {
            ("user", user)
        } else if repo.exists() {
            ("repo", repo)
        } else {
            ("missing", user)
        };
        println!("{hook}\t{tier}\t{}", path.display());
    }
    Ok(())
}

fn collect_chain_goals(
    args: &[String],
    from_file: Option<PathBuf>,
    from_stdin: bool,
    no_hints: bool,
) -> Result<Vec<String>> {
    let mut goals = Vec::new();
    goals.extend(args.iter().cloned());
    if let Some(path) = from_file {
        goals.extend(parse_goal_lines(&fs::read_to_string(&path).map_err(
            |source| {
                CliError::Core(DeadreckonError::Io {
                    path: path.clone(),
                    source,
                })
            },
        )?));
    }
    if from_stdin {
        if io::stdin().is_terminal() {
            return Err(chain_create_refusal_surface(
                VerdictKind::Blocked,
                None,
                "DeadReckon did not read chain steps because --from-stdin needs a pipe.",
                "The command requested stdin input, but stdin is an interactive terminal. DeadReckon refused before reading goals or writing chain state.",
                [
                    ("stdin".to_string(), "terminal".to_string()),
                    ("from-stdin".to_string(), "true".to_string()),
                ],
                "printf 'g1\\ng2\\n' | deadreckon chain --from-stdin --yes".to_string(),
                no_hints,
            ));
        }
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        goals.extend(parse_goal_lines(&raw));
    }
    Ok(goals
        .into_iter()
        .map(|goal| goal.trim().to_string())
        .filter(|goal| !goal.is_empty())
        .collect())
}

fn parse_goal_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect()
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn chain_planner_prompt(goal: &str, n: u8) -> String {
    format!(
        "You are decomposing a coding goal into an ordered serial chain.\n\
Output a JSON array of <= {n} strings, each <= 160 chars, each a concrete next step. \
Each step builds on the previous step's result. No prose, no commentary. Goal: {goal:?}."
    )
}

fn parse_planner_goals(raw: &str, n: u8, root_goal: &str, no_hints: bool) -> Result<Vec<String>> {
    let raw = raw.trim();
    let json_text = if raw.starts_with("```") {
        raw.lines()
            .filter(|line| !line.trim_start().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        raw.to_string()
    };
    let value = serde_json::from_str::<Value>(json_text.trim()).map_err(|err| {
        chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            &format!("chain plan returned invalid JSON: {err}"),
            "DeadReckon refused the provider-produced plan because the planner response could not be parsed as the required JSON step list.",
            [
                ("parse error".to_string(), err.to_string()),
                ("response bytes".to_string(), json_text.len().to_string()),
            ],
            chain_plan_retry_command(root_goal),
            no_hints,
        )
    })?;
    let array = value.as_array().ok_or_else(|| {
        chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            "chain plan must return a JSON array of strings",
            "DeadReckon refused the provider-produced plan because the planner response had the wrong JSON shape for ordered chain steps.",
            [
                ("expected".to_string(), "array".to_string()),
                ("actual".to_string(), json_value_kind(&value).to_string()),
            ],
            chain_plan_retry_command(root_goal),
            no_hints,
        )
    })?;
    let mut seen = BTreeSet::new();
    let mut goals = Vec::new();
    for item in array.iter().take(usize::from(n)) {
        let Some(goal) = item.as_str().map(str::trim).filter(|goal| !goal.is_empty()) else {
            continue;
        };
        if goal.chars().count() > 160 {
            return Err(chain_create_refusal_surface(
                VerdictKind::Blocked,
                None,
                "chain plan produced a step longer than 160 chars",
                "DeadReckon refused the provider-produced plan because one step is too large to review and recover as a chain step.",
                [
                    ("step chars".to_string(), goal.chars().count().to_string()),
                    ("maximum chars".to_string(), "160".to_string()),
                ],
                chain_plan_retry_command(root_goal),
                no_hints,
            ));
        }
        let key = goal
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !seen.insert(key.clone()) {
            return Err(chain_create_refusal_surface(
                VerdictKind::Blocked,
                None,
                "chain plan produced duplicate steps",
                "DeadReckon refused the provider-produced plan because duplicate steps make chain progress and recovery ambiguous.",
                [
                    ("duplicate step".to_string(), goal.to_string()),
                    ("normalized step".to_string(), key),
                ],
                chain_plan_retry_command(root_goal),
                no_hints,
            ));
        }
        goals.push(goal.to_string());
    }
    deadreckon_core::validate_goal_count(goals.len()).map_err(|_| {
        chain_create_refusal_surface(
            VerdictKind::Blocked,
            None,
            &format!("decomposition produced {} goals; need >= 2", goals.len()),
            "DeadReckon refused the provider-produced plan because a chain needs at least two ordered steps.",
            [
                ("produced goals".to_string(), goals.len().to_string()),
                ("minimum goals".to_string(), "2".to_string()),
            ],
            chain_plan_retry_command(root_goal),
            no_hints,
        )
    })?;
    Ok(goals)
}

fn chain_plan_retry_command(root_goal: &str) -> String {
    format!(
        "deadreckon chain plan {} --n 3",
        quote_chain_goal_arg(root_goal)
    )
}

fn append_chain_planner_spend(
    paths: &DeadreckonPaths,
    chain_id: &str,
    response: &deadreckon_providers::ProviderResponse,
) -> Result<()> {
    let path = paths.chain_dir(chain_id).join("spend.jsonl");
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    serde_json::to_writer(
        &mut file,
        &json!({
            "timestamp": Utc::now(),
            "kind": "chain.planner",
            "provider": response.provider,
            "model": response.model,
            "input_tokens": response.spend.input_tokens,
            "output_tokens": response.spend.output_tokens,
            "cost_usd": response.spend.cost_usd,
        }),
    )?;
    file.write_all(b"\n")?;
    Ok(())
}

fn chain_preview(chain: &Chain) -> String {
    let cap = chain
        .max_spend_usd
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "uncapped".to_string());
    let mut lines = vec![
        format!("chain preview {}", chain_prefix(&chain.chain_id)),
        format!("scope {}", chain.scope),
        format!("cwd {}", chain.cwd.display()),
        format!("base {}@{}", chain.base_branch, short_sha(&chain.base_sha)),
        format!(
            "policy branch={} apply={} strategy={} on-fail={}",
            branch_policy_label(chain.branch_policy),
            apply_mode_label(chain.apply_mode),
            apply_strategy_label(chain.apply_strategy),
            on_fail_label(chain.on_fail)
        ),
        format!(
            "provider {} model {} sandbox {} max-spend {}",
            chain.provider.as_deref().unwrap_or("default"),
            chain.model.as_deref().unwrap_or("default"),
            chain.sandbox,
            cap
        ),
        "steps".to_string(),
    ];
    for step in &chain.steps {
        lines.push(format!("  {}. {}", step.index + 1, step.goal));
    }
    lines.join("\n")
}

fn print_chain_table(chains: &[Chain], full: bool) {
    println!("CHAIN     STATUS     STEPS  SPEND       UPDATED                  GOAL");
    for chain in chains {
        let id = if full {
            chain.chain_id.clone()
        } else {
            chain_prefix(&chain.chain_id)
        };
        let done = chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count();
        let updated = chain
            .completed_at
            .or(chain.started_at)
            .unwrap_or(chain.created_at);
        println!(
            "{:<9} {:<10} {:>2}/{:<2} ${:<9.6} {:<24} {}",
            id,
            chain_status_label(chain),
            done,
            chain.steps.len(),
            chain.total_spend_usd,
            updated,
            truncate_text(&chain.root_goal, 80)
        );
    }
}

// SAFETY: Chain list filters are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn list_chain_records(paths: &DeadreckonPaths, scope: Option<String>) -> Result<Vec<Chain>> {
    if !paths.chains_dir().exists() {
        return Ok(Vec::new());
    }
    let mut chains = Vec::new();
    for entry in fs::read_dir(paths.chains_dir())? {
        let entry = entry?;
        let path = entry.path().join("chain.json");
        if !path.exists() {
            continue;
        }
        let chain = serde_json::from_slice::<Chain>(&fs::read(&path)?)
            .map_err(|source| DeadreckonError::Json { path, source })?;
        if scope.as_deref().is_some_and(|scope| chain.scope != scope) {
            continue;
        }
        chains.push(chain);
    }
    chains.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(chains)
}

pub(crate) fn resolve_chain_id(paths: &DeadreckonPaths, id: &str, all: bool) -> Result<String> {
    let scope = if all { None } else { Some(current_scope()?) };
    let chains = list_chain_records(paths, scope.clone())?;
    if matches!(id, "latest" | "last") {
        return chains
            .first()
            .map(|chain| chain.chain_id.clone())
            .ok_or_else(|| chain_missing_scope_surface(scope.as_deref()));
    }
    let matches = chains
        .iter()
        .filter(|chain| chain.chain_id.starts_with(id))
        .map(|chain| chain.chain_id.clone())
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(chain_id_resolution_surface(
            id,
            &format!("DeadReckon could not resolve the chain because no chain '{id}' was found."),
            "The command needs one concrete chain id, but no chain in the selected scope starts with the requested id.",
            [
                ("requested id".to_string(), id.to_string()),
                (
                    "scope".to_string(),
                    scope.as_deref().unwrap_or("all").to_string(),
                ),
                ("matching chains".to_string(), "0".to_string()),
            ],
            "deadreckon chain list",
        )),
        _ => Err(chain_id_resolution_surface(
            id,
            &format!(
                "DeadReckon could not resolve the chain because chain id prefix {id} is ambiguous."
            ),
            "The command needs one concrete chain id, but the requested prefix matches multiple chains.",
            [
                ("requested prefix".to_string(), id.to_string()),
                (
                    "scope".to_string(),
                    scope.as_deref().unwrap_or("all").to_string(),
                ),
                ("matches".to_string(), matches.join(", ")),
            ],
            "deadreckon chain list --full",
        )),
    }
}

fn chain_id_resolution_surface<K, V>(
    id: &str,
    what_happened: impl Into<String>,
    why_this_verdict: impl Into<String>,
    evidence: impl IntoIterator<Item = (K, V)>,
    primary: &str,
) -> CliError
where
    K: Into<String>,
    V: Into<String>,
{
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "chain",
            Some(id),
            ExplanationPanel::new(what_happened, why_this_verdict, evidence),
            [("Recommended", primary)],
            Vec::<(&str, &str)>::new(),
        )
        .expect("chain id resolution surface must have one primary action")
        .render_plain(false),
    }
}

fn chain_missing_scope_surface(scope: Option<&str>) -> CliError {
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "chain",
            None,
            ExplanationPanel::new(
                "DeadReckon could not resolve a chain because no chains in scope.",
                "The requested command needs an existing chain id or latest chain, but the current scope has no chain state to operate on.",
                [
                    ("scope".to_string(), scope.unwrap_or("all").to_string()),
                    ("matching chains".to_string(), "0".to_string()),
                ],
            ),
            [("Recommended", "deadreckon chain \"step one\" \"step two\"")],
            Vec::<(&str, &str)>::new(),
        )
        .expect("missing chain scope surface must have one primary action")
        .render_plain(false),
    }
}

fn chain_option_parse_surface(
    flag: &str,
    value: &str,
    allowed: &str,
    what_happened: String,
    primary: &str,
) -> CliError {
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "chain",
            None,
            ExplanationPanel::new(
                what_happened,
                "DeadReckon refused before writing chain state because the option value is outside the supported policy set.",
                [
                    ("option".to_string(), flag.to_string()),
                    ("value".to_string(), value.to_string()),
                    ("allowed".to_string(), allowed.to_string()),
                ],
            ),
            [("Recommended", primary)],
            Vec::<(&str, &str)>::new(),
        )
        .expect("chain option parse surface must have one primary action")
        .render_plain(false),
    }
}

fn parse_branch_policy(value: &str) -> Result<BranchPolicy> {
    match value {
        "stack" => Ok(BranchPolicy::Stack),
        "base" => Ok(BranchPolicy::Base),
        "linear-merge" | "merge" => Ok(BranchPolicy::Merge),
        other => Err(chain_option_parse_surface(
            "--branch-policy",
            other,
            "stack|base|linear-merge",
            format!(
                "DeadReckon did not accept the chain option because unknown branch policy {other}."
            ),
            "deadreckon chain --branch-policy stack \"step one\" \"step two\"",
        )),
    }
}

fn parse_apply_mode(value: &str) -> Result<ApplyMode> {
    match value {
        "auto" => Ok(ApplyMode::Auto),
        "preview" => Ok(ApplyMode::Preview),
        "manual" => Ok(ApplyMode::Manual),
        other => Err(chain_option_parse_surface(
            "--apply-mode",
            other,
            "auto|preview|manual",
            format!(
                "DeadReckon did not accept the chain option because unknown apply mode {other}."
            ),
            "deadreckon chain --apply-mode auto \"step one\" \"step two\"",
        )),
    }
}

fn parse_apply_strategy(value: &str) -> Result<ApplyStrategy> {
    match value {
        "squash" => Ok(ApplyStrategy::Squash),
        "merge" => Ok(ApplyStrategy::Merge),
        "cherry-pick" => Ok(ApplyStrategy::CherryPick),
        other => Err(chain_option_parse_surface(
            "--apply-strategy",
            other,
            "squash|merge|cherry-pick",
            format!(
                "DeadReckon did not accept the chain option because unknown chain git apply strategy {other}."
            ),
            "deadreckon chain --apply-strategy squash \"step one\" \"step two\"",
        )),
    }
}

fn parse_on_fail(value: &str) -> Result<OnFail> {
    match value {
        "stop" => Ok(OnFail::Stop),
        "skip" => Ok(OnFail::Skip),
        "continue" => Ok(OnFail::Continue),
        other => Err(chain_option_parse_surface(
            "--on-fail",
            other,
            "stop|skip|continue",
            format!(
                "DeadReckon did not accept the chain option because unknown on-fail policy {other}."
            ),
            "deadreckon chain --on-fail stop \"step one\" \"step two\"",
        )),
    }
}

fn chain_step_base_ref(chain: &Chain) -> Result<String> {
    match chain.branch_policy {
        BranchPolicy::Base => Ok(chain.base_sha.clone()),
        BranchPolicy::Stack | BranchPolicy::Merge => git_stdout(&chain.cwd, &["rev-parse", "HEAD"]),
    }
}

pub(crate) fn chain_should_auto_attach(
    stdout_is_terminal: bool,
    detach: bool,
    quiet: bool,
    plain: bool,
) -> bool {
    stdout_is_terminal && !detach && !quiet && !plain
}

fn per_step_spend_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_spend_usd?;
    let remaining = (max - chain.total_spend_usd).max(0.0);
    let pending = chain
        .steps
        .iter()
        .skip(index)
        .filter(|step| {
            !matches!(
                step.status,
                ChainStepStatus::Applied | ChainStepStatus::Skipped
            )
        })
        .count()
        .max(1);
    Some(remaining / pending as f64)
}

pub(crate) fn per_step_wall_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_wall_seconds?;
    let remaining = (max - chain.total_wall_seconds).max(0.0);
    let pending = chain
        .steps
        .iter()
        .skip(index)
        .filter(|step| {
            !matches!(
                step.status,
                ChainStepStatus::Applied | ChainStepStatus::Skipped
            )
        })
        .count()
        .max(1);
    Some(remaining / pending as f64)
}

fn chain_spend_cap_hit(chain: &Chain) -> bool {
    chain
        .max_spend_usd
        .is_some_and(|max| chain.total_spend_usd >= max)
}

pub(crate) fn chain_wall_cap_hit(chain: &Chain) -> bool {
    chain
        .max_wall_seconds
        .is_some_and(|max| chain.total_wall_seconds >= max)
}

fn preview_diff_summary_for_run(state: &deadreckon_core::PipelineState) -> Result<String> {
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")])
}

fn latest_applied_sha_before(chain: &Chain, index: usize) -> Option<String> {
    chain
        .steps
        .iter()
        .take(index)
        .rev()
        .find_map(|step| step.applied_sha.clone())
}

pub(crate) fn parse_started_run_id(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(start) = line.find('(')
            && let Some(end) = line[start + 1..].find(')')
        {
            let value = &line[start + 1..start + 1 + end];
            if value.len() >= 16 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn allowlist_matches(pattern: &str, file: &str) -> bool {
    pattern == "*"
        || pattern == file
        || file.starts_with(pattern.trim_end_matches('*'))
        || file.starts_with(pattern.trim_end_matches('/'))
}

fn looks_like_chain_id(value: &str) -> bool {
    value.len() >= 6 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn chain_apply_refusal_pause_reason(message: &str) -> String {
    if let Some(file) = message
        .split("outside_allowlist ")
        .nth(1)
        .and_then(|tail| tail.split(')').next())
        .map(str::trim)
        .filter(|file| !file.is_empty())
    {
        return format!("apply_refused_outside_allowlist {file}");
    }
    if message.contains("dirty target") {
        return "apply_refused_dirty_target".to_string();
    }
    if message.contains("paused by hook on-promote") {
        return "apply_paused_by_hook_on_promote".to_string();
    }
    if message.contains("refused by hook on-promote") {
        return "apply_refused_by_hook_on_promote".to_string();
    }
    format!("apply_refused_{}", compact_reason(message))
}

fn compact_reason(reason: &str) -> String {
    reason
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(96)
        .collect()
}

pub(crate) fn chain_prefix(chain_id: &str) -> String {
    id_prefix(chain_id)
}

pub(crate) fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

pub(crate) fn branch_policy_label(value: BranchPolicy) -> &'static str {
    match value {
        BranchPolicy::Stack => "stack",
        BranchPolicy::Base => "base",
        BranchPolicy::Merge => "linear-merge",
    }
}

pub(crate) fn apply_mode_label(value: ApplyMode) -> &'static str {
    match value {
        ApplyMode::Auto => "auto",
        ApplyMode::Preview => "preview",
        ApplyMode::Manual => "manual",
    }
}

pub(crate) fn apply_strategy_label(value: ApplyStrategy) -> &'static str {
    match value {
        ApplyStrategy::Squash => "squash",
        ApplyStrategy::Merge => "merge",
        ApplyStrategy::CherryPick => "cherry-pick",
    }
}

pub(crate) fn chain_apply_strategy(chain: &Chain) -> ApplyStrategy {
    if chain.branch_policy == BranchPolicy::Merge {
        ApplyStrategy::Merge
    } else {
        chain.apply_strategy
    }
}

pub(crate) fn on_fail_label(value: OnFail) -> &'static str {
    match value {
        OnFail::Stop => "stop",
        OnFail::Skip => "skip",
        OnFail::Continue => "continue",
    }
}

pub(crate) fn chain_status_label(chain: &Chain) -> &'static str {
    glossary_chain_status_label(chain.status)
}

pub(crate) fn chain_step_status_label(status: ChainStepStatus) -> &'static str {
    glossary_chain_step_status_label(status)
}

pub(crate) fn chain_step_dot(status: ChainStepStatus) -> &'static str {
    match status {
        ChainStepStatus::Pending => "○",
        ChainStepStatus::Running => "●",
        ChainStepStatus::Completed => "◐",
        ChainStepStatus::Failed => "✗",
        ChainStepStatus::Skipped => "↷",
        ChainStepStatus::Applied => "◉",
        ChainStepStatus::Undone => "↶",
    }
}

fn print_chain_paused_footer(paths: &DeadreckonPaths, chain: &Chain) {
    print!(
        "{}",
        chain_verdict_surface(paths, chain).render_plain(false)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_inner_run_id_error_has_one_primary_hint() {
        match missing_inner_run_id_error() {
            CliError::Exit {
                code,
                message,
                hint,
            } => {
                assert_eq!(code, 1);
                assert_eq!(message, "could not find inner run id in run output");
                assert_eq!(hint, "deadreckon list");
                assert!(!message.contains("try:"));
            }
            other => panic!("expected exit error, got {other:?}"),
        }
    }
}
