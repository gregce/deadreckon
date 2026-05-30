use super::super::*;

pub(crate) async fn merge_command(args: MergeCommandArgs) -> Result<()> {
    let MergeCommandArgs {
        plan_id,
        strategy,
        prefer_child,
        no_repair,
        repair_provider,
        repair_mode,
        repair_attempts,
        yes: _yes,
        no_gate,
        no_hints,
        quiet,
        plain: _plain,
    } = args;
    let paths = DeadreckonPaths::discover();
    let resolved_id = resolve_plan_id(&paths, &plan_id)?;
    let mut plan = load_plan(&paths, &resolved_id)?;
    if !matches!(plan.status, PlanStatus::Forked | PlanStatus::Failed) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} is {}",
                run_prefix(&plan.plan_id),
                plan_status_label(plan.status)
            ),
            "deadreckon fork <plan-id>",
        )));
    }
    if let Some(task) = plan
        .tasks
        .iter()
        .find(|task| task.status != PlanTaskStatus::Completed)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("child {} is {}", task.index, task_status_label(task.status)),
            "wait, or run deadreckon kill <plan-id>",
        )));
    }
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::MergeStarted)?;
    let strategy = parse_merge_strategy(&strategy, prefer_child)?;
    if let PlanMergeStrategy::PreferChild(chosen) = strategy
        && !plan.tasks.iter().any(|task| task.index == chosen)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown child index {chosen}"),
            "deadreckon merge <plan-id> --strategy prefer-child --prefer-child 1",
        )));
    }
    let repair_mode = parse_merge_repair_mode(&repair_mode)?;
    let mut merge = compose_plan_merge_working(&paths, &plan, strategy)?;
    let unresolved_conflicts = merge.unresolved_conflicts();
    if !unresolved_conflicts.is_empty() {
        let repair_context = MergeRepairContext::final_merge(&paths, &plan);
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeConflict {
                conflict_count: unresolved_conflicts.len(),
            },
        )?;
        let repair_disabled = no_repair
            || repair_attempts == 0
            || matches!(strategy, PlanMergeStrategy::FailOnConflict);
        let provider = if repair_disabled {
            None
        } else {
            resolve_merge_repair_provider(&paths, &plan, repair_provider.as_deref())?
        };
        write_merge_repair_request(
            &paths,
            &plan,
            &repair_context,
            provider.as_deref(),
            &unresolved_conflicts,
        )?;
        if repair_disabled {
            let reason = format!(
                "merge conflict at {}",
                unresolved_conflicts
                    .iter()
                    .map(|conflict| conflict.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            record_plan_merge_failure(&paths, &mut plan, &reason)?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("{reason}; automatic repair disabled"),
                &format!(
                    "inspect {}",
                    paths
                        .merge_proofs(&plan.plan_id)
                        .join("conflicts.json")
                        .display()
                ),
            )));
        }
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairPlanned {
                conflict_count: unresolved_conflicts.len(),
                provider: provider.clone(),
            },
        )?;
        let Some(provider) = provider else {
            let reason = "merge repair needs a configured provider".to_string();
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::MergeRepairFailed {
                    reason: reason.clone(),
                },
            )?;
            record_plan_merge_failure(&paths, &mut plan, &reason)?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("{reason}; conflicts remain"),
                "deadreckon providers list --all",
            )));
        };
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairStarted {
                mode: repair_mode.as_str().to_string(),
            },
        )?;
        match run_merge_repair(
            &paths,
            &plan,
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
                append_plan_event(
                    &paths,
                    &plan.plan_id,
                    PlanEventKind::MergeRepaired {
                        strategy: repaired.strategy,
                        repair_run_id: repaired.repair_run_id,
                    },
                )?;
                write_plan_merge_conflicts(&paths, &plan, strategy, &merge.conflicts)?;
            }
            Err(error) => {
                let reason = error.to_string();
                append_plan_event(
                    &paths,
                    &plan.plan_id,
                    PlanEventKind::MergeRepairFailed {
                        reason: reason.clone(),
                    },
                )?;
                record_plan_merge_failure(&paths, &mut plan, &reason)?;
                return Err(error);
            }
        }
    }
    let merged_run = create_merged_plan_run(&paths, &plan, no_gate)?;
    plan.status = PlanStatus::Merged;
    plan.merged_at = Some(Utc::now());
    plan.merged_run_id = Some(merged_run.run_id.clone());
    save_plan(&paths, &plan)?;
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::MergeCompleted {
            merged_run_id: merged_run.run_id.clone(),
        },
    )?;
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanCompleted)?;
    let library_dir = paths.library_dir(&merged_run.scope, &merged_run.run_id);
    write_plan_merge_manifest(&paths, &library_dir, &plan, &merge.conflicts)?;
    let doc_provider = select_plan_doc_provider(&paths, &plan, None)?;
    let defaults = config_defaults(&paths)?;
    let _manifest = maybe_with_cli_wait_status(
        !quiet,
        "consolidating plan docs",
        refresh_plan_docs(
            &paths,
            &plan,
            PlanDocRefreshOptions {
                provider: doc_provider.provider.clone(),
                provider_source: doc_provider.source.as_str().to_string(),
                budget_cap_usd: defaults.doc_polish_budget_cap_usd,
                force: true,
            },
        ),
    )
    .await?;
    materialize_plan_docs_to_working(&paths, &plan, &library_dir, None)?;
    if !quiet {
        print_merge_finished(&paths, &plan, &merged_run, &library_dir, no_hints);
    }
    Ok(())
}

fn parse_merge_strategy(strategy: &str, prefer_child: Option<u32>) -> Result<PlanMergeStrategy> {
    match strategy {
        "fail-on-conflict" => Ok(PlanMergeStrategy::FailOnConflict),
        "dag-aware" => Ok(PlanMergeStrategy::DagAware),
        "prefer-child" => prefer_child
            .map(PlanMergeStrategy::PreferChild)
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "plan merge strategy prefer-child needs --prefer-child <idx>",
                    "deadreckon merge <plan-id> --strategy prefer-child --prefer-child 1",
                ))
            }),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown plan merge strategy {other}"),
            "use --strategy dag-aware, fail-on-conflict, or prefer-child --prefer-child <idx>",
        ))),
    }
}

fn parse_merge_repair_mode(mode: &str) -> Result<MergeRepairMode> {
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
