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
        return Err(CliError::Surface {
            code: 1,
            surface: merge_unavailable_plan_surface(&paths, &plan)
                .render_plain(!completion_hints_enabled(no_hints)),
        });
    }
    if let Some(task) = plan
        .tasks
        .iter()
        .find(|task| task.status != PlanTaskStatus::Completed)
    {
        return Err(CliError::Surface {
            code: 1,
            surface: merge_incomplete_plan_surface(&plan, task)
                .render_plain(!completion_hints_enabled(no_hints)),
        });
    }
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::MergeStarted)?;
    let strategy = parse_merge_strategy(&plan, &strategy, prefer_child, no_hints)?;
    if let PlanMergeStrategy::PreferChild(chosen) = strategy
        && !plan.tasks.iter().any(|task| task.index == chosen)
    {
        let id = run_prefix(&plan.plan_id);
        return Err(merge_argument_surface_error(
            &plan,
            no_hints,
            format!("DeadReckon did not merge because unknown child index {chosen}."),
            "The prefer-child strategy must name one of the plan child indexes before DeadReckon can choose a winner.",
            vec![
                ("flag".to_string(), "--prefer-child".to_string()),
                ("value".to_string(), chosen.to_string()),
                (
                    "available children".to_string(),
                    plan.tasks
                        .iter()
                        .map(|task| task.index.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ],
            format!("deadreckon show {id}"),
        ));
    }
    let repair_mode = parse_merge_repair_mode(&plan, &repair_mode, no_hints)?;
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
            return Err(CliError::Surface {
                code: 1,
                surface: merge_repair_disabled_surface(
                    &paths,
                    &plan,
                    &unresolved_conflicts,
                    &reason,
                )
                .render_plain(!completion_hints_enabled(no_hints)),
            });
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
            return Err(CliError::Surface {
                code: 1,
                surface: merge_repair_provider_missing_surface(
                    &paths,
                    &plan,
                    &unresolved_conflicts,
                    &reason,
                )
                .render_plain(!completion_hints_enabled(no_hints)),
            });
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

fn merge_unavailable_plan_surface(paths: &DeadreckonPaths, plan: &Plan) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let status = merge_plan_status_word(plan.status);
    let kind = match plan.status {
        PlanStatus::Merged => VerdictKind::Noop,
        _ => VerdictKind::Blocked,
    };
    let primary = super::plan::plan_next_actions_with_context(paths, plan)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("deadreckon show {id}"));
    VerdictSurface::try_new(
        kind,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!("DeadReckon did not merge the plan because it is {status}."),
            "Merge only composes forked or failed plans; the recommended command follows the current plan state instead.",
            [
                ("plan".to_string(), id.clone()),
                ("status".to_string(), status.to_string()),
                ("tasks".to_string(), plan.tasks.len().to_string()),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
    .expect("merge unavailable plan surface must have one primary action")
}

fn merge_plan_status_word(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Merged => "merged",
        other => plan_status_label(other),
    }
}

fn merge_incomplete_plan_surface(plan: &Plan, task: &PlanTask) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let primary = format!("deadreckon attach {id}");
    let secondary = format!("deadreckon kill {id}");
    VerdictSurface::try_new(
        VerdictKind::Paused,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!(
                "DeadReckon cannot merge yet because child {} is {}.",
                task.index,
                task_status_label(task.status)
            ),
            "The merge step requires every child task to complete before composing a result run.",
            [
                ("plan".to_string(), id.clone()),
                (
                    "status".to_string(),
                    plan_status_label(plan.status).to_string(),
                ),
                ("child".to_string(), task.index.to_string()),
                (
                    "child status".to_string(),
                    task_status_label(task.status).to_string(),
                ),
                (
                    "tasks".to_string(),
                    format!("{completed}/{} completed", plan.tasks.len()),
                ),
            ],
        ),
        [("Recommended", primary.as_str())],
        [("Secondary", secondary.as_str())],
    )
    .expect("merge incomplete plan surface must have one primary action")
}

fn merge_repair_disabled_surface(
    paths: &DeadreckonPaths,
    plan: &Plan,
    conflicts: &[PlanMergeConflict],
    reason: &str,
) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let conflict_paths = conflicts
        .iter()
        .map(|conflict| conflict.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let conflicts_path = paths.merge_proofs(&plan.plan_id).join("conflicts.json");
    let primary = format!("deadreckon merge {id}");
    let secondary = format!("deadreckon show {id} --why-failed");
    VerdictSurface::try_new(
        VerdictKind::Failed,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!("DeadReckon could not merge the plan because {reason}."),
            "The plan has unresolved child artifact conflicts and automatic repair disabled for this attempt, so DeadReckon recorded the conflict bundle instead of choosing a result.",
            [
                ("plan".to_string(), id.clone()),
                ("conflict count".to_string(), conflicts.len().to_string()),
                ("conflict paths".to_string(), conflict_paths),
                ("automatic repair".to_string(), "disabled".to_string()),
                ("conflicts".to_string(), conflicts_path.display().to_string()),
            ],
        ),
        [("Recommended", primary.as_str())],
        [("Secondary", secondary.as_str())],
    )
    .expect("merge repair disabled surface must have one primary action")
}

fn merge_repair_provider_missing_surface(
    paths: &DeadreckonPaths,
    plan: &Plan,
    conflicts: &[PlanMergeConflict],
    reason: &str,
) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let conflict_paths = conflicts
        .iter()
        .map(|conflict| conflict.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let conflicts_path = paths.merge_proofs(&plan.plan_id).join("conflicts.json");
    let primary = "deadreckon providers list --all";
    let secondary = format!("deadreckon show {id} --why-failed");
    VerdictSurface::try_new(
        VerdictKind::Failed,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!("DeadReckon could not repair the merge because {reason}; conflicts remain."),
            "The plan has unresolved child artifact conflicts, but no repair provider was available from the flag, plan routes, child defaults, or config.",
            [
                ("plan".to_string(), id.clone()),
                ("conflict count".to_string(), conflicts.len().to_string()),
                ("conflict paths".to_string(), conflict_paths),
                ("provider".to_string(), "none".to_string()),
                ("conflicts".to_string(), conflicts_path.display().to_string()),
            ],
        ),
        [("Recommended", primary)],
        [("Secondary", secondary.as_str())],
    )
    .expect("merge missing repair provider surface must have one primary action")
}

fn parse_merge_strategy(
    plan: &Plan,
    strategy: &str,
    prefer_child: Option<u32>,
    no_hints: bool,
) -> Result<PlanMergeStrategy> {
    match strategy {
        "fail-on-conflict" => Ok(PlanMergeStrategy::FailOnConflict),
        "dag-aware" => Ok(PlanMergeStrategy::DagAware),
        "prefer-child" => prefer_child
            .map(PlanMergeStrategy::PreferChild)
            .ok_or_else(|| {
                let id = run_prefix(&plan.plan_id);
                merge_argument_surface_error(
                    plan,
                    no_hints,
                    "DeadReckon did not merge because prefer-child needs --prefer-child <idx>.",
                    "The prefer-child strategy requires a concrete child index so DeadReckon knows which completed child result to keep for conflicting files.",
                    vec![
                        ("flag".to_string(), "--prefer-child".to_string()),
                        ("strategy".to_string(), "prefer-child".to_string()),
                    ],
                    format!("deadreckon merge {id} --strategy prefer-child --prefer-child 1"),
                )
            }),
        other => Err({
            let id = run_prefix(&plan.plan_id);
            merge_argument_surface_error(
                plan,
                no_hints,
                format!("DeadReckon did not merge because unknown plan merge strategy {other}."),
                "Merge only supports dag-aware, fail-on-conflict, or prefer-child strategies.",
                vec![
                    ("flag".to_string(), "--strategy".to_string()),
                    ("value".to_string(), other.to_string()),
                ],
                format!("deadreckon merge {id} --strategy dag-aware"),
            )
        }),
    }
}

fn parse_merge_repair_mode(plan: &Plan, mode: &str, no_hints: bool) -> Result<MergeRepairMode> {
    match mode {
        "auto" => Ok(MergeRepairMode::Auto),
        "prefer" => Ok(MergeRepairMode::Prefer),
        "synthesize" => Ok(MergeRepairMode::Synthesize),
        "child" => Ok(MergeRepairMode::Child),
        other => Err({
            let id = run_prefix(&plan.plan_id);
            merge_argument_surface_error(
                plan,
                no_hints,
                format!("DeadReckon did not merge because unknown repair mode {other}."),
                "Merge repair only supports auto, prefer, synthesize, or child modes.",
                vec![
                    ("flag".to_string(), "--repair-mode".to_string()),
                    ("value".to_string(), other.to_string()),
                ],
                format!("deadreckon merge {id} --repair-mode auto"),
            )
        }),
    }
}

fn merge_argument_surface_error(
    plan: &Plan,
    no_hints: bool,
    what_happened: impl Into<String>,
    why_this_verdict: impl Into<String>,
    mut evidence: Vec<(String, String)>,
    primary: String,
) -> CliError {
    let id = run_prefix(&plan.plan_id);
    evidence.insert(0, ("plan".to_string(), id.clone()));
    evidence.push((
        "status".to_string(),
        plan_status_label(plan.status).to_string(),
    ));
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "plan",
            Some(&id),
            ExplanationPanel::new(what_happened, why_this_verdict, evidence),
            [("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .expect("merge argument surface must have one primary action")
        .render_plain(!completion_hints_enabled(no_hints)),
    }
}
