use super::super::*;

pub(crate) async fn run_command(args: RunCommandArgs) -> Result<()> {
    let RunCommandArgs {
        goal,
        fresh,
        worktree,
        from,
        in_place,
        base,
        branch,
        allow_dirty,
        init_git,
        yes,
        preview,
        brief,
        no_seams,
        plain,
        prevent_sleep,
        quiet,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        model,
        doc_provider,
        acceptance,
        skill,
        smoke,
        i_know_its_a_lot,
        no_confirm,
        no_hints,
        no_docs,
        doc_skill,
        narrate,
        no_narrate,
        narrator_model,
    } = args;
    if let Err(message) = crate::narrator::validate_narration_flags(narrate, no_narrate) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
    }
    let auto_confirm = yes || no_confirm || quiet;
    let effective_no_hints = no_hints || quiet;
    if smoke && provider.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --provider".to_string(),
        )));
    }
    if smoke && model.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --model".to_string(),
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    if let Some(model) = narrator_model.as_deref()
        && let Ok(registry) =
            deadreckon_providers::registry::ProviderRegistry::with_overrides(paths.home())
        && !crate::narrator::narrator_model_known(&registry, model)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            crate::narrator::narrator_model_refusal(model),
        )));
    }
    // NOTE: piped runs intentionally keep rich rendering (the project opts out
    // of color only via NO_COLOR/--plain), so we do NOT force plain off-TTY.
    // The silent-pipe progress decision lives in narrator::effective_plain and
    // is exercised as a unit; wiring it to the run surface without regressing
    // the rich-when-piped card contract is a V1 candidate.
    let plain = plain || defaults.plain.unwrap_or(false) || std::env::var_os("NO_COLOR").is_some();
    ui::set_plain_output(plain);
    let prevent_sleep_prefs =
        SleepPrefs::parse(prevent_sleep.as_deref(), defaults.prevent_sleep.as_deref())
            .map_err(|err| CliError::Core(DeadreckonError::InvalidInput(err)))?;
    if !preview
        && let Some(exit_code) =
            sleep::maybe_reexec_for_linux(prevent_sleep_prefs, io::stdin().is_terminal())?
    {
        std::process::exit(exit_code);
    }
    let primary_setup = if smoke {
        provider_setup_selection(
            &paths,
            setup::ProviderSetupRequest {
                role: setup::SetupProviderRoleRef::PrimaryRun,
                explicit_provider: Some("smoke"),
                explicit_model: model.as_deref(),
                config_default_provider: defaults.provider.as_deref(),
                config_doc_provider: defaults.doc_provider.as_deref(),
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )?
    } else {
        provider_setup_selection(
            &paths,
            setup::ProviderSetupRequest {
                role: setup::SetupProviderRoleRef::PrimaryRun,
                explicit_provider: provider.as_deref(),
                explicit_model: model.as_deref(),
                config_default_provider: defaults.provider.as_deref(),
                config_doc_provider: defaults.doc_provider.as_deref(),
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: true,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )?
    };
    let provider_override = provider_override_from_setup(&primary_setup);
    let router = if smoke {
        ProviderRouter::smoke()
    } else {
        ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            provider_override.as_deref(),
            model.as_deref(),
        )?
    };
    let selected_route = router.selected_route_info();
    let effective_provider = selected_route
        .as_ref()
        .map(|route| route.name.clone())
        .or(primary_setup.provider.clone());
    let effective_max_spend = max_spend.or(defaults.max_spend).or(Some(10.0));
    let effective_max_wall_seconds = max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .or(Some(36_000.0));
    let effective_doc_skill = doc_skill
        .or(defaults.doc_skill.clone())
        .unwrap_or_else(|| "run-narrator".to_string());
    let doc_provider_setup = doc_provider_setup_selection(
        &paths,
        &defaults,
        doc_provider.as_deref(),
        effective_provider.as_deref(),
        false,
    )?;
    let mut doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup);
    let effective_no_docs = no_docs || (smoke && doc_provider.is_none());
    if effective_no_docs {
        doc_provider_selection = DocProviderSelection {
            provider: None,
            source: DocProviderSource::None,
        };
    }
    if max_spend.is_none() && !quiet {
        let cap = effective_max_spend.unwrap_or(10.0);
        println!(
            "using default --max-spend ${cap:.0} (override with --max-spend or in config defaults.max_spend)"
        );
    }
    confirm_spend_cap(effective_max_spend, i_know_its_a_lot, no_confirm)?;
    let cwd = std::env::current_dir()?;
    let acceptance_source = commands::acceptance::ensure_acceptance_before_start(
        &cwd,
        acceptance.as_deref(),
        &goal,
        provider.clone(),
        model.clone(),
        auto_confirm || preview,
        "run",
    )
    .await?;
    let acceptance_preview = commands::acceptance::done_criteria_selection(&acceptance_source)?;
    let sleep_preview = sleep::preview(prevent_sleep_prefs, io::stdin().is_terminal());
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    let seams = read_seams_config(&paths.config_path(), no_seams)?;
    let seams_label = seam_preview_label(&seams);
    if init_git {
        init_git_repo(&cwd)?;
    }
    let run_id = Uuid::new_v4().simple().to_string();
    let mut mode_flags = ModeFlags {
        fresh,
        worktree,
        from,
        in_place,
        i_know_its_a_lot,
    };
    let explicit_mode =
        mode_flags.fresh || mode_flags.worktree || mode_flags.from.is_some() || mode_flags.in_place;
    if !explicit_mode
        && deadreckon_core::find_git_root(&cwd)?.is_none()
        && io::stdin().is_terminal()
    {
        match prompt_non_git_mode()? {
            NonGitChoice::Init => {
                init_git_repo(&cwd)?;
                mode_flags.worktree = true;
            }
            NonGitChoice::Copy => mode_flags.from = Some(cwd.clone()),
            NonGitChoice::Cancel => {
                print!(
                    "{}",
                    cancelled_run_surface().render_plain(!completion_hints_enabled(false))
                );
                return Ok(());
            }
        }
    }
    let resolved_mode = resolve_mode(&mode_flags, &cwd, io::stdin().is_terminal())
        .map_err(|err| run_codebase_refusal_error(err, &goal, effective_no_hints))?;
    let mut codebase = match &resolved_mode {
        ResolvedMode::Worktree { source_path, .. } => prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: run_id.clone(),
                task_key: deadreckon_core::paths::task_key(&goal),
                source_path: source_path.clone(),
                base_ref: base,
                branch_name: branch,
                allow_dirty,
            },
        )
        .map_err(|err| run_codebase_refusal_error(err, &goal, effective_no_hints))?,
        _ => record_for_resolved_mode(resolved_mode.clone()),
    };
    if codebase.mode == CodebaseMode::Fresh {
        codebase.source_path = None;
    }
    let preview_text = run_preview(&RunPreview {
        goal: &goal,
        cwd: &cwd,
        codebase: &codebase,
        provider: effective_provider.as_deref(),
        provider_source: primary_setup.source.as_str(),
        route: selected_route.as_ref(),
        sandbox: &backend.to_string(),
        doc_provider: doc_provider_selection.provider.as_deref(),
        doc_provider_source: doc_provider_selection.source.as_str(),
        max_spend: effective_max_spend,
        max_wall_seconds: effective_max_wall_seconds,
        acceptance: &acceptance_preview,
        sleep: &sleep_preview,
        seams: &seams_label,
        brief,
        plain,
        run_id: &run_id,
    });
    if preview {
        eprintln!("{preview_text}");
        return Ok(());
    }
    if !quiet {
        eprintln!("{preview_text}");
    }
    if !auto_confirm {
        if !io::stdin().is_terminal() {
            return Err(run_confirmation_refusal_error(
                &goal,
                &run_id,
                effective_no_hints,
            ));
        }
        if !prompt::confirm("continue?", true)? {
            print!(
                "{}",
                cancelled_run_surface().render_plain(!completion_hints_enabled(effective_no_hints))
            );
            return Ok(());
        }
    }
    if codebase.mode == CodebaseMode::Worktree {
        create_worktree(&codebase)?;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal,
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: skill,
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase.clone()),
        },
    )?;
    if let Some(source_path) = codebase
        .source_path
        .as_ref()
        .filter(|_| codebase.mode == CodebaseMode::Copy)
    {
        copy_source_to_working(source_path, &state.working_dir)?;
        deadreckon_core::write_codebase_record(&state.working_dir, &codebase)?;
    }
    commands::acceptance::copy_acceptance_into_run(&state, &acceptance_source)?;
    let mut lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "run",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    state.child_pids = vec![std::process::id()];
    save_state(&state)?;

    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(
                &state,
                &RunLoopOutcome::Killed,
                plain,
                completion_hints_enabled(effective_no_hints),
            );
        }
        return Ok(());
    }
    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(
                &state,
                &RunLoopOutcome::Killed,
                plain,
                completion_hints_enabled(effective_no_hints),
            );
        }
        return Ok(());
    }
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(
                &state,
                &RunLoopOutcome::Killed,
                plain,
                completion_hints_enabled(effective_no_hints),
            );
        }
        return Ok(());
    }
    lock.heartbeat("turn-loop")?;
    let _sleep_handle = match sleep::arm(prevent_sleep_prefs, &state.working_dir)? {
        SleepPrevention::Active { handle } => Some(handle),
        SleepPrevention::Skipped { reason } => {
            if prevent_sleep_prefs == SleepPrefs::On && !quiet {
                eprint!(
                    "{}",
                    sleep_skipped_surface(&state.run_id, &state.working_dir, reason)
                        .render_plain(!completion_hints_enabled(effective_no_hints))
                );
            }
            None
        }
        SleepPrevention::Reexeced { exit_code } => {
            std::process::exit(exit_code);
        }
    };
    let router = if smoke {
        router
    } else {
        provider_router_for_run_with_catalog_seam(
            &paths,
            &state,
            backend,
            provider_override.as_deref(),
            model.as_deref(),
            no_seams,
        )
        .await?
    };
    let selected_route = router.selected_route_info();
    if !quiet {
        print_run_started(
            &state,
            selected_route.as_ref(),
            primary_setup.source.as_str(),
            doc_provider_selection.provider.as_deref(),
            doc_provider_selection.source.as_str(),
        );
    }
    let wait_label = format!(
        "run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let run_id_for_plain = state.run_id.clone();
    // Live narrator: on a TTY, narration is on by default; the bus sender feeds
    // the loop and the sidecar engine turns events into beats. Off-TTY without
    // --narrate, resolve returns None and the run is wired exactly as before.
    let narrator_config = crate::narrator::resolve_narrator_config(
        io::stdin().is_terminal(),
        narrate,
        no_narrate,
        narrator_model,
    );
    let (narrate_event_sender, narrator_handle) = match narrator_config.clone() {
        Some(config) => {
            // A smoke run makes no real provider calls — narrate via the
            // deterministic floor so tests and dry runs stay hermetic and fast.
            let backend = if smoke {
                deadreckon_providers::NarratorBackend::DeterministicFloor
            } else {
                crate::narrator::resolve_narrator_backend(
                    paths.home(),
                    config.model_override.as_deref(),
                )
            };
            let ctx = crate::narrator::NarratorCtx {
                run_id: state.run_id.clone(),
                run_root: state.run_root.clone(),
                config_path: Some(paths.config_path()),
                backend,
                config,
            };
            let (sender, handle) = crate::narrator::build_narration(ctx);
            (Some(sender), Some(handle))
        }
        None => (None, None),
    };
    let turn_loop = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: effective_provider.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            no_seams,
            max_turns: 12,
            from_turn: None,
            event_sender: narrate_event_sender,
            cancellation_token: None,
            narrate: narrator_config,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider: doc_provider_selection.provider.clone(),
                doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                doc_subskills: effective_doc_subskills(&defaults),
                token_budget: defaults
                    .doc_polish_token_budget
                    .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET),
                budget_cap_usd: defaults.doc_polish_budget_cap_usd,
                doc_skill: effective_doc_skill,
                no_docs: effective_no_docs,
            },
        },
    );
    let outcome = if plain && !quiet {
        with_plain_run_wait_status(paths.clone(), run_id_for_plain, turn_loop).await?
    } else {
        maybe_with_cli_wait_status(!plain && !quiet, &wait_label, turn_loop).await?
    };
    if let Some(handle) = narrator_handle {
        handle.shutdown().await;
    }
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    if !quiet {
        print_exit_summary_card(
            &state,
            &outcome,
            plain,
            completion_hints_enabled(effective_no_hints),
        );
    }
    super::lifecycle::fire_lifecycle_notification(&paths, &state, &outcome).await;
    if completed && completion_hints_enabled(effective_no_hints) {
        complete_run_actions(&state, !auto_confirm, false).await?;
    }
    Ok(())
}

fn run_confirmation_refusal_error(goal: &str, run_id: &str, no_hints: bool) -> CliError {
    let primary = format!("deadreckon run {} --yes", run_goal_argument(goal));
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "run",
            None,
            ExplanationPanel::new(
                "non-interactive without --yes",
                "DeadReckon printed the launch preview, then refused to create run state because this shell cannot answer the confirmation prompt.",
                [
                    ("command".to_string(), "run".to_string()),
                    ("goal".to_string(), goal.to_string()),
                    ("preview run".to_string(), run_id.to_string()),
                ],
            ),
            [("Recommended", primary)],
            std::iter::empty::<(&str, String)>(),
        )
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}

fn run_codebase_refusal_error(err: DeadreckonError, goal: &str, no_hints: bool) -> CliError {
    let DeadreckonError::InvalidInput(message) = err else {
        return CliError::Core(err);
    };
    let Some((primary, why)) = run_codebase_refusal_primary(&message, goal) else {
        return CliError::Core(DeadreckonError::InvalidInput(message));
    };
    let first_reason_line = message.lines().next().unwrap_or(message.as_str());
    let evidence_reason = run_codebase_refusal_reason(&message);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "run",
            None,
            ExplanationPanel::new(
                first_reason_line.to_string(),
                why.to_string(),
                [
                    ("command".to_string(), "run".to_string()),
                    ("goal".to_string(), goal.to_string()),
                    ("reason".to_string(), evidence_reason),
                ],
            ),
            [("Recommended", primary)],
            std::iter::empty::<(&str, String)>(),
        )
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}

fn run_codebase_refusal_primary(message: &str, goal: &str) -> Option<(String, &'static str)> {
    let quoted_goal = run_goal_argument(goal);
    if message.contains("working tree has uncommitted changes") {
        return Some((
            format!("git stash && deadreckon run {quoted_goal} --yes"),
            "DeadReckon refused to create a worktree from a dirty source because the run would not have a clean base to apply or clean up from.",
        ));
    }
    if message.contains("git repo has no commits") {
        return Some((
            "git commit -m initial".to_string(),
            "DeadReckon needs a committed Git base before it can create an isolated worktree branch.",
        ));
    }
    if message.contains("HEAD is detached") {
        return Some((
            "git switch -c <branch>".to_string(),
            "DeadReckon needs a named source branch so the run branch has an unambiguous base.",
        ));
    }
    if message.contains("git is in the middle of a merge") {
        return Some((
            "git merge --abort".to_string(),
            "DeadReckon will not start a run while Git has an unresolved merge state.",
        ));
    }
    if message.contains("git is in the middle of a rebase") {
        return Some((
            "git rebase --abort".to_string(),
            "DeadReckon will not start a run while Git has an unresolved rebase state.",
        ));
    }
    if message.contains("branch ") && message.contains(" already exists") {
        return Some((
            format!("deadreckon run {quoted_goal} --branch-name <other-name> --yes"),
            "DeadReckon refused to reuse an existing branch name because that would blur this run's provenance.",
        ));
    }
    if message.contains("non-interactive without a mode flag") {
        return Some((
            format!("deadreckon run {quoted_goal} --from . --yes"),
            "DeadReckon cannot ask for a source-mode choice in non-interactive output, so one explicit source mode is required.",
        ));
    }
    if message.contains("--in-place requires --i-know-its-a-lot") {
        return Some((
            format!("deadreckon run {quoted_goal} --in-place --i-know-its-a-lot --yes"),
            "In-place runs can mutate the current checkout, so DeadReckon requires the stronger acknowledgement before launching.",
        ));
    }
    if message.contains(" is not a git repo") {
        return Some((
            "git init".to_string(),
            "Worktree mode requires a Git repository; initialize Git or choose an explicit copy/fresh source mode.",
        ));
    }
    None
}

fn run_goal_argument(goal: &str) -> String {
    format!("\"{}\"", shell_display_quote(goal))
}

fn run_codebase_refusal_reason(message: &str) -> String {
    message
        .lines()
        .filter(|line| !line.trim_start().starts_with("try:"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_cancelled_before_turn_loop(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
) -> Result<bool> {
    if !cancel_marker_present(state) {
        return Ok(false);
    }
    if let Ok(latest) = load_run(paths, &state.run_id)
        && latest.status == RunStatus::Killed
    {
        *state = latest;
        return Ok(true);
    }
    state.status = RunStatus::Killed;
    state.failure_reason = Some("run cancelled before provider turn".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(state)?;
    emit_event(
        state,
        None,
        deadreckon_core::RunEventKind::RunCompleted {
            status: "killed".to_string(),
        },
    )?;
    Ok(true)
}

/// A cancel outcome rendered through the shared verdict surface (one verdict,
/// one Recommended next step) instead of a bare `println!("cancelled")`.
fn cancelled_run_surface() -> VerdictSurface {
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "run",
        Some("cancelled"),
        ExplanationPanel::new(
            "Run cancelled before launch.",
            "This is a no-op because you declined to continue; no run state was created.",
            [("state".to_string(), "no run started".to_string())],
        ),
        [("Recommended", "deadreckon run \"<goal>\"")],
        [("Secondary", "deadreckon start \"<goal>\"")],
    )
}

#[cfg(test)]
mod cancel_tests {
    use super::cancelled_run_surface;

    #[test]
    fn cancelled_run_renders_surface_with_next_step() {
        let rendered = cancelled_run_surface().render_plain(true);
        assert!(rendered.contains("cancelled"), "{rendered}");
        assert!(rendered.contains("Recommended"), "{rendered}");
        assert!(rendered.contains("deadreckon run"), "{rendered}");
        // It is a full surface, not a bare "cancelled" line.
        assert!(rendered.contains("Explanation"), "{rendered}");
    }
}
