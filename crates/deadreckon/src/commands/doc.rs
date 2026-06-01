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
    let loaded_state = load_cli_run(&paths, &run_id);
    if let Some(target) = match loaded_state.as_ref() {
        Ok(state) => resolve_plan_doc_target(&paths, &run_id, Some(state))?,
        Err(_) => resolve_plan_doc_target(&paths, &run_id, None)?,
    } {
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
    let mut state = loaded_state?;
    let kind = run_doc_kind(kind)?;
    if polish {
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
            return Err(CliError::Exit {
                code: 1,
                message: "deadreckon: no doc provider available; configure a doc provider after installing codex or claude".to_string(),
                hint: "deadreckon config set defaults.doc_provider cli:codex".to_string(),
            });
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
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "doc polish would cost about ${:.6}, above cap ${cap:.6}",
                    estimated_spend.cost_usd
                ),
                &format!(
                    "deadreckon doc {} --polish --max-spend {:.2} --no-confirm",
                    state.run_id, estimated_spend.cost_usd
                ),
            )));
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
                println!("cancelled");
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
                    no_llm: false,
                    force,
                },
            ),
        )
        .await?;
        if completion_hints_enabled(false)
            && let Some(record) = deadreckon_runtime::read_polish_record(&state)?
        {
            print_doc_polish_summary(&record);
        }
        save_state(&state)?;
    }
    let Some(path) = doc_path_for_kind(&state.working_dir, kind) else {
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
        println!("exported {} to {}", kind.file_name(), dest.display());
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
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
        println!("exported {file_name} to {}", dest.display());
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
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
        "  provider:  {} ({provider_source})\n",
        ui_id(provider)
    ));
    out.push_str(&format!("  subskills: {}\n", subskills.join(", ")));
    out.push_str(&format!(
        "  budget:    {} tokens per subcall\n",
        token_budget
    ));
    out.push_str(&format!(
        "  estimate:  {}\n",
        doc_polish_cost_label(estimated_spend)
    ));
    out.push_str(&format!(
        "  cost cap:  {}\n",
        budget_cap
            .map(|cap| format!("${cap:.2}"))
            .unwrap_or_else(|| "provider/account default".to_string())
    ));
    out.push_str(&format!("  inputs:    {}\n", &hash[..hash.len().min(12)]));
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
        "$0.00 (subscription)".to_string()
    } else {
        format!(
            "${:.6} for up to {} output tokens",
            estimate.cost_usd, estimate.output_tokens
        )
    }
}

fn print_doc_polish_summary(record: &deadreckon_runtime::PolishRecord) {
    println!("{}", ui_heading("doc polish:"));
    println!("  status:   {}", record.status);
    if let Some(provider) = record.provider.as_deref() {
        println!("  provider: {provider}");
    }
    println!("  cost:     ${:.6}", record.cost_usd);
    if !record.subcalls.is_empty() {
        println!("  subcalls:");
        for subcall in &record.subcalls {
            println!(
                "    {} {:<18} {} in / {} out ${:.6}",
                ui_status(&subcall.status),
                subcall.skill,
                subcall.tokens_in,
                subcall.tokens_out,
                subcall.cost_usd
            );
        }
    }
}
