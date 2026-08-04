use super::super::*;

pub(crate) struct DocCommandArgs {
    pub(crate) run_id: String,
    pub(crate) kind: CliDocKind,
    pub(crate) export: Option<PathBuf>,
    pub(crate) polish: bool,
    pub(crate) no_confirm: bool,
    pub(crate) force: bool,
    pub(crate) doc_skill: Option<String>,
    pub(crate) doc_provider: Option<String>,
    pub(crate) budget_cap: Option<f64>,
}

struct DocPlanCommandArgs {
    target: PlanDocTarget,
    kind: CliDocKind,
    export: Option<PathBuf>,
    polish: bool,
    force: bool,
    doc_provider: Option<String>,
    budget_cap: Option<f64>,
}

pub(crate) async fn doc_command(args: DocCommandArgs) -> Result<()> {
    let DocCommandArgs {
        run_id,
        kind,
        export,
        polish,
        no_confirm,
        force,
        doc_skill,
        doc_provider,
        budget_cap,
    } = args;
    let paths = DeadreckonPaths::discover();
    // `doc` maps a plan reference onto that plan's doc target, so resolution must
    // not refuse a non-run kind before the mapping gets its turn.
    let loaded_state = super::reference::try_resolve_run(&paths, &run_id, "doc")?;
    if let Some(target) = resolve_plan_doc_target(&paths, &run_id, loaded_state.as_ref())? {
        return doc_plan_command(
            &paths,
            DocPlanCommandArgs {
                target,
                kind,
                export,
                polish,
                force,
                doc_provider,
                budget_cap,
            },
        )
        .await;
    }
    let Some(mut state) = loaded_state else {
        return Err(super::reference::refusal_for_reference(
            &paths, &run_id, "doc",
        ));
    };
    let kind_arg = cli_doc_kind_arg(kind);
    let kind = run_doc_kind(kind)?;
    if polish {
        super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "doc polish")?;
        if state.status != RunStatus::Completed {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "run {} is {}; docs are not yet polished",
                    state.run_id, state.status
                ),
                &format!("deadreckon resume {} or omit --polish", state.run_id),
            )));
        }
        let defaults = config_defaults(&paths)?;
        let setup_selection = doc_provider_setup_selection(
            &paths,
            &defaults,
            doc_provider.as_deref(),
            state.provider.as_deref(),
            false,
        )?;
        let selection = doc_provider_selection_from_setup(&setup_selection);
        let Some(provider) = selection.provider.clone() else {
            return Err(missing_doc_provider_error(
                &paths,
                state.provider.as_deref(),
                &setup_selection,
            ));
        };
        let subskills = effective_doc_subskills(&defaults);
        let token_budget = defaults
            .doc_polish_token_budget
            .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
        let budget_cap = budget_cap.or(defaults.doc_polish_budget_cap_usd);
        let router = ProviderRouter::from_config_path(&paths.config_path(), Some(&provider))?;
        let estimated_spend =
            estimate_doc_polish_spend(&router, &provider, token_budget, subskills.len())?;
        if let Some(cap) = budget_cap
            && estimated_spend.cost_usd > cap
        {
            return Err(doc_polish_budget_cap_error(
                &state,
                &provider,
                selection.source.as_str(),
                &estimated_spend,
                cap,
            ));
        }
        if !no_confirm && completion_hints_enabled(false) && io::stdin().is_terminal() {
            print_doc_polish_preview(
                &state,
                &provider,
                selection.source.as_str(),
                &subskills,
                token_budget,
                budget_cap,
                &estimated_spend,
            )?;
            if !prompt::confirm("polish docs now?", true)? {
                println!("{}", ui_status("cancelled"));
                return Ok(());
            }
        } else if !no_confirm && !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive doc polish requires --no-confirm",
                &format!("deadreckon doc {} --polish --no-confirm", state.run_id),
            )));
        }
        with_cli_wait_status(
            "polishing run docs",
            polish_run_docs(
                &mut state,
                &router,
                &PolishConfig {
                    home: paths.home().to_path_buf(),
                    doc_skill: doc_skill
                        .or(defaults.doc_skill)
                        .unwrap_or_else(|| "run-narrator".to_string()),
                    doc_provider: Some(provider),
                    doc_provider_source: Some(selection.source.as_str().to_string()),
                    doc_subskills: subskills,
                    token_budget,
                    budget_cap_usd: budget_cap,
                    sandbox_backend: deadreckon_sandbox::SandboxBackend::Auto,
                    commit_docs: true,
                    no_llm: false,
                    force,
                    max_wall_seconds: None,
                    phase_deadline: None,
                    cancellation_token: None,
                },
            ),
        )
        .await?;
        if completion_hints_enabled(false)
            && let Some(record) = deadreckon_runtime::read_polish_record(&state)?
        {
            print_doc_polish_summary(&state, &record);
        }
        save_state(&state)?;
    }
    let view = deadreckon_core::RunView::from_state(&state)?;
    let Some(path) = run_view_doc_path(&state, &view, kind) else {
        if kind == DocKind::Delta {
            return Err(CliError::Core(deadreckon_core::user_error(
                "no delta produced; this run did not affect a project AS-BUILT",
                "deadreckon doc <run-id> --kind narrative",
            )));
        }
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "{} for run {}",
            kind.file_name(),
            state.run_id
        ))));
    };
    if let Some(dest) = export {
        if dest.exists() && !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("dest {} exists", dest.display()),
                "--overwrite or pick a fresh path",
            )));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &dest)?;
        print_doc_export_surface(
            "run",
            &state.run_id,
            kind_arg,
            kind.file_name(),
            &path,
            &dest,
        );
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
}

fn run_view_doc_path(
    state: &deadreckon_core::PipelineState,
    view: &deadreckon_core::RunView,
    kind: DocKind,
) -> Option<PathBuf> {
    match kind {
        DocKind::Narrative => view
            .why
            .narrative_path
            .clone()
            .or_else(|| doc_path_for_kind(&state.working_dir, kind)),
        DocKind::Decisions => view
            .why
            .decisions_path
            .clone()
            .or_else(|| doc_path_for_kind(&state.working_dir, kind)),
        DocKind::AsBuilt | DocKind::Delta => doc_path_for_kind(&state.working_dir, kind),
    }
}

async fn doc_plan_command(paths: &DeadreckonPaths, args: DocPlanCommandArgs) -> Result<()> {
    let DocPlanCommandArgs {
        target,
        kind,
        export,
        polish,
        force,
        doc_provider,
        budget_cap,
    } = args;
    let Some(file_name) = plan_doc_kind_file_name(kind) else {
        return Err(CliError::Core(deadreckon_core::user_error(
            "plan docs do not produce AS-BUILT-DELTA.md",
            "deadreckon doc <plan-id> --kind narrative",
        )));
    };
    super::graph_job::require_current_driver_for_job_artifact(
        paths,
        &target.plan.plan_id,
        deadreckon_protocol::JobShape::Graph,
        if polish { "doc polish" } else { "doc" },
    )?;
    if polish {
        let selection = select_plan_doc_provider(paths, &target.plan, doc_provider.as_deref())?;
        let defaults = config_defaults(paths)?;
        refresh_plan_docs(
            paths,
            &target.plan,
            PlanDocRefreshOptions {
                provider: selection.provider,
                provider_source: selection.source.as_str().to_string(),
                budget_cap_usd: budget_cap.or(defaults.doc_polish_budget_cap_usd),
                force: true,
            },
        )
        .await?;
    } else {
        ensure_plan_docs_deterministic(paths, &target.plan)?;
    }
    if let Some(wrapper) = target.wrapper.as_ref()
        && let Ok(state) = load_run(paths, &wrapper.wrapper_run_id)
    {
        let _ = materialize_plan_docs_to_working(
            paths,
            &target.plan,
            &state.working_dir,
            Some(wrapper),
        );
    }
    let path = plan_doc_path(paths, &target.plan.plan_id, file_name);
    let kind_arg = cli_doc_kind_arg(kind);
    if let Some(dest) = export {
        if dest.exists() && !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("dest {} exists", dest.display()),
                "--overwrite or pick a fresh path",
            )));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &dest)?;
        print_doc_export_surface(
            "plan",
            &target.plan.plan_id,
            kind_arg,
            file_name,
            &path,
            &dest,
        );
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
}

fn cli_doc_kind_arg(kind: CliDocKind) -> &'static str {
    match kind {
        CliDocKind::Narrative => "narrative",
        CliDocKind::AsBuilt => "as-built",
        CliDocKind::Decisions => "decisions",
        CliDocKind::Children => "children",
        CliDocKind::Delta => "delta",
    }
}

fn print_doc_export_surface(
    target_kind: &str,
    target_id: &str,
    kind_arg: &str,
    file_name: &str,
    source: &std::path::Path,
    dest: &std::path::Path,
) {
    let id = run_prefix(target_id);
    let primary = format!("deadreckon doc {id} --kind {kind_arg}");
    let secondary = format!("deadreckon show {id}");
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "doc",
            Some(&id),
            ExplanationPanel::new(
                format!("Exported {file_name} to {}.", dest.display()),
                "The document copy completed successfully; the recommended command reopens the source document through DeadReckon for inspection.",
                vec![
                    ("target".to_string(), format!("{target_kind} {id}")),
                    ("kind".to_string(), kind_arg.to_string()),
                    ("file".to_string(), file_name.to_string()),
                    ("source".to_string(), source.display().to_string()),
                    ("dest".to_string(), dest.display().to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .render_plain(!completion_hints_enabled(false))
    );
}

fn print_doc_polish_preview(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<()> {
    print!(
        "{}",
        doc_polish_preview_text(
            state,
            provider,
            provider_source,
            subskills,
            token_budget,
            budget_cap,
            estimated_spend,
        )?
    );
    Ok(())
}

fn missing_doc_provider_error(
    paths: &DeadreckonPaths,
    run_provider: Option<&str>,
    setup_selection: &setup::ProviderSetupSelection,
) -> CliError {
    let primary = "deadreckon config set defaults.doc_provider cli:codex";
    let mut secondary = vec![("Secondary", "deadreckon doctor".to_string())];
    if let Some(setup_command) = setup_selection.try_lines.first()
        && setup_command != primary
    {
        secondary.push(("Secondary", setup_command.clone()));
    }
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "doc",
            Some("polish"),
            ExplanationPanel::new(
                "DeadReckon could not polish run documentation because no doc provider is available.",
                "Documentation polish needs an installed or configured provider route; setting defaults.doc_provider is the safest next step.",
                vec![
                    ("provider source".to_string(), setup_selection.source.as_str().to_string()),
                    (
                        "run provider".to_string(),
                        run_provider.unwrap_or("none").to_string(),
                    ),
                    ("config".to_string(), paths.config_path().display().to_string()),
                ],
            ),
            vec![("Recommended", primary)],
            secondary,
        )
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn doc_polish_budget_cap_error(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    estimated_spend: &SpendEstimate,
    cap: f64,
) -> CliError {
    let primary = format!(
        "deadreckon doc {} --polish --max-spend {:.2} --no-confirm",
        state.run_id, estimated_spend.cost_usd
    );
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "doc",
            Some("polish"),
            ExplanationPanel::new(
                "DeadReckon did not start documentation polish because the estimated provider spend exceeds the budget cap.",
                "The budget cap is a preflight guard, so increasing the explicit max-spend to the estimate is the safest command if this polish is intended.",
                vec![
                    ("run".to_string(), state.run_id.clone()),
                    ("provider".to_string(), provider.to_string()),
                    ("provider source".to_string(), provider_source.to_string()),
                    (
                        "estimated spend".to_string(),
                        format!("${:.6}", estimated_spend.cost_usd),
                    ),
                    ("budget cap".to_string(), format!("${cap:.6}")),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", "deadreckon doctor")],
        )
        .render_plain(!completion_hints_enabled(false)),
    }
}

pub(crate) fn doc_polish_preview_text(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<String> {
    let hash = deadreckon_runtime::inputs_hash(state)?;
    let mut out = String::new();
    out.push_str(&format!("{}\n", ui_heading("polish preview:")));
    out.push_str(&format!(
        "  {}  {} ({provider_source})\n",
        ui_muted("provider:"),
        ui_id(provider)
    ));
    out.push_str(&format!(
        "  {} {}\n",
        ui_muted("subskills:"),
        subskills.join(", ")
    ));
    out.push_str(&format!(
        "  {}    {} tokens per subcall\n",
        ui_muted("budget:"),
        token_budget
    ));
    out.push_str(&format!(
        "  {}  {}\n",
        ui_muted("estimate:"),
        doc_polish_cost_label(estimated_spend)
    ));
    out.push_str(&format!(
        "  {}  {}\n",
        ui_muted("cost cap:"),
        budget_cap
            .map(|cap| format!("${cap:.2}"))
            .unwrap_or_else(|| "provider/account default".to_string())
    ));
    out.push_str(&format!(
        "  {}    {}\n",
        ui_muted("inputs:"),
        &hash[..hash.len().min(12)]
    ));
    Ok(out)
}

pub(crate) fn estimate_doc_polish_spend(
    router: &ProviderRouter,
    provider: &str,
    token_budget: u32,
    subskill_count: usize,
) -> Result<SpendEstimate> {
    let calls = subskill_count.max(1) as u64;
    router
        .estimate_for_route(
            Some(provider),
            ProviderUsage {
                input_tokens: 0,
                output_tokens: u64::from(token_budget) * calls,
            },
        )
        .map_err(CliError::from)
}

fn doc_polish_cost_label(estimate: &SpendEstimate) -> String {
    if estimate.subscription {
        format!(
            "not metered (subscription) for up to {} output tokens",
            estimate.output_tokens
        )
    } else {
        format!(
            "${:.6} for up to {} output tokens",
            estimate.cost_usd, estimate.output_tokens
        )
    }
}

fn print_doc_polish_summary(
    state: &deadreckon_core::PipelineState,
    record: &deadreckon_runtime::PolishRecord,
) {
    let id = run_prefix(&state.run_id);
    let successful = record.status == "polished";
    let kind = if successful {
        VerdictKind::Completed
    } else {
        VerdictKind::Failed
    };
    let what = if successful {
        "DeadReckon polished the run documentation."
    } else {
        "Doc polish did not complete cleanly."
    };
    let why = if successful {
        "The polish record completed successfully; the recommended command opens the refreshed narrative through DeadReckon."
    } else {
        "The polish record contains an error, so rerunning polish is the primary recovery command while fallback docs remain available."
    };
    let primary = if successful {
        format!("deadreckon doc {id} --kind narrative")
    } else {
        format!("deadreckon doc {id} --polish --no-confirm --force")
    };
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        ("status".to_string(), record.status.clone()),
        ("cost".to_string(), polish_record_cost_label(record)),
        ("completed".to_string(), record.completed_at.clone()),
    ];
    if let Some(provider) = record.provider.as_deref() {
        evidence.push(("provider".to_string(), provider.to_string()));
    }
    if let Some(source) = record.doc_provider_source.as_deref() {
        evidence.push(("provider source".to_string(), source.to_string()));
    }
    if let Some(error) = record.error.as_deref() {
        evidence.push(("error".to_string(), error.to_string()));
    }
    for subcall in &record.subcalls {
        evidence.push((
            format!("subcall {}", subcall.skill),
            format!(
                "{} {} in / {} out {}",
                subcall.status,
                subcall.tokens_in,
                subcall.tokens_out,
                polish_subcall_cost_label(record, subcall)
            ),
        ));
    }
    print!(
        "{}",
        VerdictSurface::must_new(
            kind,
            "doc",
            Some(&id),
            ExplanationPanel::new(what, why, evidence),
            vec![("Recommended", primary.as_str())],
            vec![
                ("Secondary", format!("deadreckon doc {id} --kind as-built")),
                ("Secondary", format!("deadreckon doc {id} --kind decisions")),
            ],
        )
        .render_plain(!completion_hints_enabled(false))
    );
}

fn polish_record_cost_label(record: &deadreckon_runtime::PolishRecord) -> String {
    if record.doc_provider_source.as_deref() == Some("auto_subscription") && record.cost_usd == 0.0
    {
        "not metered (subscription)".to_string()
    } else {
        format!("${:.6}", record.cost_usd)
    }
}

fn polish_subcall_cost_label(
    record: &deadreckon_runtime::PolishRecord,
    subcall: &deadreckon_core::PolishSubcallRecord,
) -> String {
    if record.doc_provider_source.as_deref() == Some("auto_subscription") && subcall.cost_usd == 0.0
    {
        "not metered (subscription)".to_string()
    } else {
        format!("${:.6}", subcall.cost_usd)
    }
}

#[cfg(test)]
mod job_plan_doc_tests {
    use chrono::Utc;
    use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};
    use deadreckon_protocol::{
        Job, JobId, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode,
    };
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn plain_doc_refuses_a_job_owned_plan_before_creating_docs() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let job_id = "89898989898989898989898989898989";
        deadreckon_core::write_job(
            &paths,
            &Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                scope: "doc-ownership-test".to_string(),
                goal: "keep sealed docs immutable".to_string(),
                shape: JobShape::Graph,
                created_at: Utc::now(),
                source_cwd: workspace.clone(),
                launch_plan_sha256: "launch".to_string(),
                authority_sha256: "authority".to_string(),
                policy: JobPolicy {
                    max_spend_usd: 1.0,
                    max_wall_seconds: 60,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: SemanticJudgeMode::Required,
                    execution: None,
                },
            },
        )
        .expect("Job identity");
        let mut plan = Plan::new(
            "keep sealed docs immutable",
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "first", "first task", PlanRole::Child, None),
                PlanTask::new(1, "second", "second task", PlanRole::Child, None),
            ],
            PlanProviders::default(),
            Some("doc-ownership-test".to_string()),
            "test",
        )
        .expect("Plan");
        plan.plan_id = job_id.to_string();
        plan.owner_job_id = Some(job_id.to_string());
        plan.parent_cwd = Some(workspace);
        save_plan(&paths, &plan).expect("owned Plan");
        let plan_before = fs::read(paths.plan_json(job_id)).expect("Plan bytes before");

        let error = doc_plan_command(
            &paths,
            DocPlanCommandArgs {
                target: PlanDocTarget {
                    plan,
                    wrapper: None,
                },
                kind: CliDocKind::Narrative,
                export: None,
                polish: false,
                force: false,
                doc_provider: None,
                budget_cap: None,
            },
        )
        .await
        .expect_err("plain doc must not mutate a sealed Plan");

        assert!(
            error.to_string().contains("belongs to durable Job"),
            "{error}"
        );
        assert_eq!(
            fs::read(paths.plan_json(job_id)).expect("Plan bytes after"),
            plan_before
        );
        assert!(
            !plan_docs_dir(&paths, job_id).exists(),
            "refused plain doc created deterministic docs"
        );
    }
}
