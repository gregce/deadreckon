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
    } = args;
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
        .or(Some(3600.0));
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
                println!("cancelled");
                return Ok(());
            }
        }
    }
    let resolved_mode = resolve_mode(&mode_flags, &cwd, io::stdin().is_terminal())?;
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
        )?,
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
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive without --yes",
                "--yes, --quiet, or run interactively",
            )));
        }
        if !prompt::confirm("continue?", true)? {
            println!("cancelled");
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
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
        }
        return Ok(());
    }
    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
        }
        return Ok(());
    }
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
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
            event_sender: None,
            cancellation_token: None,
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
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    if !quiet {
        print_exit_summary_card(&state, &outcome, plain);
    }
    super::lifecycle::fire_lifecycle_notification(&paths, &state, &outcome).await;
    if completed && completion_hints_enabled(effective_no_hints) {
        complete_run_actions(&state, !auto_confirm).await?;
    }
    Ok(())
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
