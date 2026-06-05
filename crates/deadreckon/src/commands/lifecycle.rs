use super::super::*;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ParentMarker {
    pub(crate) schema_version: u32,
    pub(crate) kind: String,
    pub(crate) parent_run_id: String,
    pub(crate) parent_scope: String,
    pub(crate) parent_goal: String,
    pub(crate) parent_completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) materialized_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) extended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) new_goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_turns_included: Option<u32>,
    pub(crate) deadreckon_version: String,
}

// SAFETY: Materialize arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn materialize_command(
    run_id: String,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let (state, plan_context, dest) = match load_cli_run(&paths, &run_id) {
        Ok(state) => (state, None, dest),
        Err(run_error) => match resolve_plan_result_run(&paths, &run_id, "export")? {
            Some(result) => {
                let dest = dest.or_else(|| Some(default_plan_materialize_dest(&result.plan)));
                (result.state, Some(result.plan), dest)
            }
            None => return Err(run_error),
        },
    };
    if let Some(plan) = plan_context.as_ref() {
        print_plan_result_context(plan, &state);
        let library_dir = paths.library_dir(&state.scope, &state.run_id);
        materialize_plan_docs_to_working(&paths, plan, &library_dir, None)?;
    }
    let materialized = materialize_completed_run(&paths, &state, dest, force, include_manifest)?;
    print_materialized(&materialized);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_command(
    run_id: Option<String>,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
    strategy: String,
    branch: Option<String>,
    autostash: bool,
    cleanup: bool,
    no_confirm: bool,
    message: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let requested = run_id.unwrap_or_else(|| "latest".to_string());
    let (state, plan_context, dest) = match load_cli_run(&paths, &requested) {
        Ok(state) => (state, None, dest),
        Err(run_error) => match resolve_plan_result_run(&paths, &requested, "finish")? {
            Some(result) => {
                if dest.is_none() && plan_apply_git_root(&result.plan)?.is_some() {
                    return apply_command_inner(
                        requested, strategy, branch, no_confirm, autostash, cleanup, message,
                        false, false,
                    );
                }
                let dest =
                    Some(dest.unwrap_or_else(|| default_plan_materialize_dest(&result.plan)));
                (result.state, Some(result.plan), dest)
            }
            None => return Err(run_error),
        },
    };
    if let Some(plan) = plan_context.as_ref() {
        print_plan_result_context(plan, &state);
        let library_dir = paths.library_dir(&state.scope, &state.run_id);
        materialize_plan_docs_to_working(&paths, plan, &library_dir, None)?;
    }
    match state.status {
        RunStatus::Completed => {}
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is still {}", state.run_id, state.status),
                &format!("deadreckon attach {}", run_prefix(&state.run_id)),
            )));
        }
        RunStatus::Failed | RunStatus::Killed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is {}", state.run_id, state.status),
                &format!("deadreckon resume {}", run_prefix(&state.run_id)),
            )));
        }
    }

    print_finish_consistency_summary(&state);

    let mode = read_codebase_record(&state.working_dir)
        .map(|record| record.mode)
        .unwrap_or(CodebaseMode::Fresh);
    match mode {
        CodebaseMode::Worktree => apply_command(
            state.run_id,
            strategy,
            branch,
            no_confirm,
            autostash,
            cleanup,
            message,
            false,
        ),
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            materialize_completed_run(&paths, &state, dest, force, include_manifest)
                .map(|materialized| print_materialized(&materialized))
        }
        CodebaseMode::InPlace => {
            let prefix = run_prefix(&state.run_id);
            println!(
                "{} {}",
                ui_ok("finished in-place run"),
                ui_id(&state.run_id)
            );
            println!("  working: {}", state.working_dir.display());
            println!(
                "{}",
                VerdictSurface::try_new(
                    VerdictKind::Completed,
                    "run",
                    Some(&prefix),
                    ExplanationPanel::new(
                        "DeadReckon finished the in-place run and left the checkout as the result.",
                        "The safest next command is inspection because the changes already live in the working tree.",
                        vec![
                            ("run".to_string(), prefix.clone()),
                            ("status".to_string(), run_status_label(state.status).to_string()),
                            ("working".to_string(), state.working_dir.display().to_string()),
                        ],
                    ),
                    vec![("Recommended", format!("deadreckon show {prefix}"))],
                    vec![
                        (
                            "Secondary",
                            format!("deadreckon doc {prefix} --kind decisions"),
                        ),
                        ("Secondary", format!("deadreckon undo --run {prefix}")),
                    ],
                )
                .expect("in-place finish verdict surface must have one primary action")
                .render_plain(false)
            );
            Ok(())
        }
    }
}

fn print_finish_consistency_summary(state: &deadreckon_core::PipelineState) {
    println!("{}", ui_heading("run summary"));
    println!("  spend: {}", run_spend_label(state, false));
    println!("  gate: {}", acceptance_status_value(state));
}

#[derive(Debug)]
pub(crate) struct MaterializedRun {
    run_id: String,
    source: PathBuf,
    dest: PathBuf,
}

pub(crate) fn materialize_completed_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<MaterializedRun> {
    ensure_completed_run(state, "materialize")?;
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        match record.mode {
            CodebaseMode::Worktree => {
                return Err(codebase_mode_refusal_error(
                    "materialize",
                    state,
                    record.mode,
                    "export is for copy/fresh runs; run was worktree",
                    format!("deadreckon apply {}", run_prefix(&state.run_id)),
                    "Materialize exports copy/fresh artifacts, while this run already has a worktree branch that should be applied instead.",
                ));
            }
            CodebaseMode::InPlace => {
                return Err(codebase_mode_refusal_error(
                    "materialize",
                    state,
                    record.mode,
                    "export is not needed; run edited the source in-place",
                    format!("deadreckon undo --run {}", run_prefix(&state.run_id)),
                    "Materialize would duplicate work already written in place; use undo only if those edits need to be reverted.",
                ));
            }
            CodebaseMode::Copy | CodebaseMode::Fresh => {}
        }
    }
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if !library_dir.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library missing for run {}; was promotion successful?",
            state.run_id
        ))));
    }

    let dest = absolute_dest(dest.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(run_prefix(&state.run_id))
    }))?;
    refuse_dest_inside_home(paths, &dest, "export")?;
    prepare_empty_dest(&dest, force)?;

    copy_tree(&library_dir, &dest)?;
    if !include_manifest {
        remove_if_exists(&dest.join("manifest.json"))?;
    }
    remove_if_exists(&dest.join(".materialized-to"))?;
    write_parent_marker(
        &dest.join(".deadreckon").join("parent.json"),
        &materialized_parent_marker(state),
    )?;
    normalize_permissions(&dest)?;
    append_materialized_marker(&library_dir, &dest)?;

    Ok(MaterializedRun {
        run_id: state.run_id.clone(),
        source: library_dir,
        dest,
    })
}

pub(crate) fn print_materialized(materialized: &MaterializedRun) {
    println!(
        "{}",
        materialized_surface(materialized).render_plain(!completion_hints_enabled(false))
    );
}

fn materialized_surface(materialized: &MaterializedRun) -> VerdictSurface {
    let id = run_prefix(&materialized.run_id);
    let primary = format!("deadreckon show {id}");
    let secondary = "deadreckon status".to_string();
    VerdictSurface::try_new(
        VerdictKind::Completed,
        "materialize",
        Some(&id),
        ExplanationPanel::new(
            "DeadReckon exported run output into the requested destination.",
            "Materialize completed because the run was already completed, the destination was safe to write, and the library artifact was copied.",
            vec![
                ("run".to_string(), id.clone()),
                ("source".to_string(), materialized.source.display().to_string()),
                ("dest".to_string(), materialized.dest.display().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("materialize verdict surface must be valid")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_command(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
    plain: bool,
) -> Result<()> {
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        false,
        plain,
    )
}

pub(crate) fn apply_command_quiet(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
) -> Result<()> {
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        true,
        false,
    )
}

// SAFETY: Apply arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
fn apply_command_inner(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
    quiet: bool,
    _plain: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = match load_cli_run(&paths, &run_id) {
        Ok(state) => state,
        Err(run_error) => match resolve_plan_result_run(&paths, &run_id, "apply")? {
            Some(result) => {
                if !quiet {
                    print_plan_result_context(&result.plan, &result.state);
                }
                prepare_plan_result_apply_state(&paths, &result.plan, &result.state)?
            }
            None => return Err(run_error),
        },
    };
    ensure_completed_run(&state, "apply")?;
    let record = match read_codebase_record(&state.working_dir) {
        Ok(record) => record,
        Err(source) => match prepare_result_run_apply_state(&paths, &state, quiet)? {
            Some(prepared) => {
                state = prepared;
                read_codebase_record(&state.working_dir)?
            }
            None => return Err(apply_missing_codebase_error(&paths, &state, source)),
        },
    };
    if record.mode != CodebaseMode::Worktree {
        return Err(apply_mode_error(&state, record.mode));
    }
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
    let target =
        target_branch.unwrap_or(git_stdout(git_root, &["symbolic-ref", "--short", "HEAD"])?);
    let diff_stat = git_stdout(
        git_root,
        &["diff", "--stat", &format!("{target}..{branch}")],
    )
    .unwrap_or_default();
    if diff_stat.trim().is_empty() {
        if !quiet {
            print_already_applied(&state, branch, &target);
        }
        let cleaned = finish_apply_cleanup(&state, &record, cleanup, no_confirm)?;
        if !quiet {
            print!(
                "{}",
                apply_completed_surface(&state, &record, &target, cleaned)
                    .render_plain(!completion_hints_enabled(false))
            );
        }
        return Ok(());
    }
    if !quiet {
        eprintln!(
            "{}",
            ui::render(ui::Stream::Stderr, ui::Tone::Heading, "changes to apply:")
        );
        eprintln!("{diff_stat}");
    }

    if !no_confirm && io::stdin().is_terminal() {
        if !prompt::confirm("apply these changes?", true)? {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm && !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive apply requires --no-confirm",
            &format!("deadreckon apply {} --no-confirm", state.run_id),
        )));
    }

    let autostash = prepare_apply_autostash(git_root, &state.run_id, autostash, no_confirm)?;

    let (commit_subject, commit_body) = match message {
        Some(message) => (message, None),
        None => (
            format!(
                "{} (deadreckon run {})",
                state.goal.lines().next().unwrap_or("deadreckon run"),
                state.run_id.chars().take(8).collect::<String>()
            ),
            Some(apply_commit_body(&state)),
        ),
    };
    let full_merge_message = commit_body
        .as_ref()
        .map(|body| format!("{commit_subject}\n\n{body}"))
        .unwrap_or_else(|| commit_subject.clone());
    match strategy.as_str() {
        "merge" => git_status(
            git_root,
            &["merge", "--no-ff", branch, "-m", &full_merge_message],
        )
        .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?,
        "squash" => {
            git_status(git_root, &["merge", "--squash", branch])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?;
            let staged_stat = git_stdout(git_root, &["diff", "--cached", "--stat"])?;
            if staged_stat.trim().is_empty() {
                if let Some(stash) = autostash.as_ref() {
                    restore_apply_autostash(git_root, &state.run_id, stash)?;
                }
                if !quiet {
                    print_already_applied(&state, branch, &target);
                }
                let cleaned = finish_apply_cleanup(&state, &record, cleanup, no_confirm)?;
                if !quiet {
                    print!(
                        "{}",
                        apply_completed_surface(&state, &record, &target, cleaned)
                            .render_plain(!completion_hints_enabled(false))
                    );
                }
                return Ok(());
            }
            if let Some(body) = commit_body.as_deref() {
                git_status(git_root, &["commit", "-m", &commit_subject, "-m", body])?;
            } else {
                git_status(git_root, &["commit", "-m", &commit_subject])?;
            }
        }
        "cherry-pick" => {
            let base = record.base_sha.as_deref().unwrap_or("HEAD");
            git_status(git_root, &["cherry-pick", &format!("{base}..{branch}")])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?;
        }
        other => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown git apply strategy {other}"
            ))));
        }
    }
    if let Some(stash) = autostash.as_ref() {
        restore_apply_autostash(git_root, &state.run_id, stash)?;
    }
    if !quiet {
        println!(
            "{} {} into {}",
            ui_ok("applied"),
            ui_id(&state.run_id),
            target
        );
        println!("{}", git_stdout(git_root, &["log", "-1", "--stat"])?);
    }
    let cleaned = finish_apply_cleanup(&state, &record, cleanup, no_confirm)?;
    if !quiet {
        print!(
            "{}",
            apply_completed_surface(&state, &record, &target, cleaned)
                .render_plain(!completion_hints_enabled(false))
        );
    }
    Ok(())
}

fn prepare_result_run_apply_state(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    quiet: bool,
) -> Result<Option<deadreckon_core::PipelineState>> {
    let Some(plan_id) = result_plan_id(paths, state)? else {
        return Ok(None);
    };
    if let Ok(plan) = load_plan(paths, &plan_id) {
        if !quiet {
            print_plan_result_context(&plan, state);
        }
        return prepare_plan_result_apply_state(paths, &plan, state).map(Some);
    }
    if let Some((campaign_dir, campaign)) =
        crate::commands::campaign::resolve_campaign(paths, &plan_id)?
    {
        let plan = crate::commands::campaign::campaign_as_apply_plan(
            paths,
            &campaign_dir,
            &campaign,
            &state.cwd,
        )?;
        if plan.merged_run_id.as_deref() != Some(state.run_id.as_str()) {
            return Ok(None);
        }
        if !quiet {
            print_plan_result_context(&plan, state);
        }
        return prepare_plan_result_apply_state(paths, &plan, state).map(Some);
    }
    Ok(None)
}

fn result_plan_id(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<Option<String>> {
    let library = paths.library_dir(&state.scope, &state.run_id);
    if let Some(plan_id) =
        result_manifest_id(&library.join("deadreckon-plan-manifest.json"), "plan_id")?
    {
        return Ok(Some(plan_id));
    }
    result_manifest_id(
        &library.join("deadreckon-campaign-manifest.json"),
        "campaign_id",
    )
}

fn result_manifest_id(path: &Path, key: &str) -> Result<Option<String>> {
    match fs::read(&path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)?;
            Ok(value.get(key).and_then(Value::as_str).map(str::to_string))
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        })),
    }
}

fn apply_missing_codebase_error(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    source: DeadreckonError,
) -> CliError {
    let id = run_prefix(&state.run_id);
    let library = paths.library_dir(&state.scope, &state.run_id);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "apply",
            Some(&id),
            ExplanationPanel::new(
                "DeadReckon could not find a worktree record for this completed run.",
                "Apply only merges worktree-backed runs or completed plan/campaign results with recoverable source metadata; use finish/export to copy this library result.",
                [
                    ("run".to_string(), id.clone()),
                    ("status".to_string(), run_status_label(state.status).to_string()),
                    ("library".to_string(), library.display().to_string()),
                    ("missing".to_string(), source.to_string()),
                ],
            ),
            vec![("Recommended", format!("deadreckon finish {id}"))],
            vec![("Secondary", format!("deadreckon export {id} --dest <path>"))],
        )
        .expect("apply missing-codebase refusal surface must be valid")
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn print_already_applied(state: &deadreckon_core::PipelineState, branch: &str, target: &str) {
    println!(
        "{} {} into {}",
        ui_ok("already applied"),
        ui_id(&state.run_id),
        target
    );
    println!("  run branch:    {branch}");
    println!("  target branch: {target}");
    println!("  reason: no file changes remain between the run branch and target branch");
}

fn apply_completed_surface(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    target: &str,
    cleaned: bool,
) -> VerdictSurface {
    let id = run_prefix(&state.run_id);
    let primary = if cleaned {
        format!("deadreckon show {id}")
    } else {
        format!("deadreckon cleanup {id}")
    };
    let secondary = if cleaned {
        "deadreckon status".to_string()
    } else {
        format!("deadreckon show {id}")
    };
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        ("target branch".to_string(), target.to_string()),
        (
            "run branch".to_string(),
            record
                .branch_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        ),
        ("mode".to_string(), record.mode.to_string()),
        (
            "cleanup".to_string(),
            if cleaned {
                "completed".to_string()
            } else {
                "pending".to_string()
            },
        ),
    ];
    if let Some(worktree) = record.worktree_path.as_ref() {
        evidence.push(("worktree".to_string(), worktree.display().to_string()));
    }
    if let Some(source) = record.source_git_root.as_ref() {
        evidence.push(("source git root".to_string(), source.display().to_string()));
    }
    let why = if cleaned {
        "The apply transition is complete and temporary resources were removed; inspect the run record before further cleanup or recovery."
    } else {
        "The apply transition is complete, but the temporary worktree resources remain; cleanup is the safest next command."
    };
    VerdictSurface::try_new(
        VerdictKind::Completed,
        "apply",
        Some(&id),
        ExplanationPanel::new(
            "DeadReckon applied or confirmed the run branch in the target checkout.",
            why,
            evidence,
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("apply completion verdict surface must be valid")
}

fn finish_apply_cleanup(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    cleanup: bool,
    no_confirm: bool,
) -> Result<bool> {
    let cleanup_now = cleanup || should_prompt_cleanup(no_confirm)?;
    if cleanup_now {
        cleanup_worktree_run(state, record, false, false, CleanupReason::Applied)?;
    }
    Ok(cleanup_now)
}

#[derive(Debug, Clone)]
struct ApplyAutoStash {
    refname: String,
}

fn prepare_apply_autostash(
    git_root: &Path,
    run_id: &str,
    requested: bool,
    no_confirm: bool,
) -> Result<Option<ApplyAutoStash>> {
    let dirty = git_stdout(git_root, &["status", "--porcelain"])?;
    if dirty.trim().is_empty() {
        return Ok(None);
    }

    eprintln!(
        "{}",
        ui::render(
            ui::Stream::Stderr,
            ui::Tone::Warn,
            "working tree has uncommitted changes:",
        )
    );
    for line in dirty.lines().take(30) {
        eprintln!("  {line}");
    }
    if dirty.lines().count() > 30 {
        eprintln!("  ...");
    }

    let mut should_stash = requested;
    if !should_stash && !no_confirm && io::stdin().is_terminal() {
        should_stash =
            prompt::confirm("stash these changes during apply and restore after?", true)?;
    }

    if !should_stash {
        return Err(apply_dirty_error(
            git_root,
            run_id,
            no_confirm,
            dirty.as_str(),
        ));
    }

    let marker = format!(
        "deadreckon apply {} autostash {}",
        run_prefix(run_id),
        Utc::now().timestamp_millis()
    );
    git_status(git_root, &["stash", "push", "-u", "-m", &marker])?;
    let refname = find_stash_by_marker(git_root, &marker)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "git stash succeeded but the new stash could not be found".to_string(),
        ))
    })?;
    eprintln!("stashed local changes as {refname}");
    Ok(Some(ApplyAutoStash { refname }))
}

fn apply_dirty_hint(run_id: &str, no_confirm: bool) -> String {
    let mut hint = format!("deadreckon apply {} --autostash", run_prefix(run_id));
    if no_confirm {
        hint.push_str(" --no-confirm");
    }
    hint
}

fn apply_dirty_error(git_root: &Path, run_id: &str, no_confirm: bool, dirty: &str) -> CliError {
    let id = run_prefix(run_id);
    let dirty_count = dirty.lines().count();
    let primary = apply_dirty_hint(run_id, no_confirm);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            "apply",
            Some(&id),
            ExplanationPanel::new(
                "your working tree has uncommitted changes",
                "Apply would merge a run branch into a checkout that already has local changes; autostash is required to preserve and restore those changes around the merge.",
                [
                    ("run".to_string(), id.clone()),
                    ("working".to_string(), git_root.display().to_string()),
                    ("dirty paths".to_string(), dirty_count.to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .expect("apply dirty refusal verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn find_stash_by_marker(git_root: &Path, marker: &str) -> Result<Option<String>> {
    let output = git_stdout(git_root, &["stash", "list", "--format=%gd%x00%s"])?;
    for line in output.lines() {
        if let Some((refname, subject)) = line.split_once('\0')
            && subject.contains(marker)
        {
            return Ok(Some(refname.to_string()));
        }
    }
    Ok(None)
}

fn restore_apply_autostash(git_root: &Path, run_id: &str, stash: &ApplyAutoStash) -> Result<()> {
    git_status(git_root, &["stash", "pop", &stash.refname]).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("applied {run_id}, but restoring autostash produced conflicts: {err}"),
            &format!(
                "resolve conflicts, then inspect `git stash list` before dropping {}",
                stash.refname
            ),
        ))
    })
}

fn apply_merge_error(run_id: &str, autostash: &Option<ApplyAutoStash>, err: &CliError) -> CliError {
    let id = run_prefix(run_id);
    let cleanup = format!("deadreckon cleanup {id}");
    let mut secondary = vec![("Secondary", cleanup.as_str())];
    let stash_hint;
    if let Some(stash) = autostash {
        stash_hint = format!("git stash pop {}", stash.refname);
        secondary.push(("Secondary", stash_hint.as_str()));
    }
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Failed,
            "apply",
            Some(&id),
            ExplanationPanel::new(
                format!("merge produced conflicts: {err}"),
                "Git stopped during apply with conflict markers in the target checkout; inspect the conflicted paths, resolve them, commit the result, then clean up the run worktree.",
                [
                    ("run".to_string(), id.clone()),
                    ("primary evidence".to_string(), "git merge failed".to_string()),
                    (
                        "autostash".to_string(),
                        autostash
                            .as_ref()
                            .map(|stash| stash.refname.clone())
                            .unwrap_or_else(|| "none".to_string()),
                    ),
                ],
            ),
            vec![("Recommended", "git status")],
            secondary,
        )
        .expect("apply conflict verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn should_prompt_cleanup(no_confirm: bool) -> Result<bool> {
    if no_confirm || !io::stdin().is_terminal() {
        return Ok(false);
    }
    prompt::confirm("remove deadreckon worktree and temporary branch now?", true)
}

// SAFETY: Abandon arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn abandon_command(run_id: String, keep_branch: bool, force: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        print!(
            "{}",
            cleanup_noop_surface(
                "abandon",
                Some(&run_prefix(&state.run_id)),
                "DeadReckon did not find a temporary worktree record for this run.",
                "There is no worktree or temporary branch for abandon to remove; inspect the run before choosing another cleanup command.",
                vec![
                    ("run".to_string(), run_prefix(&state.run_id)),
                    ("status".to_string(), run_status_label(state.status).to_string()),
                    (
                        "workspace".to_string(),
                        state.working_dir.display().to_string(),
                    ),
                ],
                format!("deadreckon show {}", run_prefix(&state.run_id)),
                Vec::<String>::new(),
            )
            .render_plain(!completion_hints_enabled(false))
        );
        return Ok(());
    };
    if record.mode == CodebaseMode::InPlace {
        return Err(codebase_mode_refusal_error(
            "abandon",
            &state,
            record.mode,
            "cannot abandon in-place edits",
            format!("deadreckon undo --run {}", run_prefix(&state.run_id)),
            "Abandon removes temporary worktrees; this run changed the source checkout directly, so undo is the safe recovery command.",
        ));
    }
    if state.status == RunStatus::Executing {
        if !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is {}", state.run_id, run_status_label(state.status)),
                &format!("deadreckon kill {} --escalate", state.run_id),
            )));
        }
        let _ = kill_loaded_run(&paths, &mut state, true);
    }
    let result = cleanup_worktree_run(
        &state,
        &record,
        keep_branch,
        force,
        CleanupReason::Abandoned,
    )?;
    print_cleanup_results(&[result]);
    Ok(())
}

pub(crate) struct CleanupCommandRequest {
    pub(crate) run_id: Option<String>,
    pub(crate) all: bool,
    pub(crate) completed: bool,
    pub(crate) stale: bool,
    pub(crate) no_confirm: bool,
    pub(crate) escalate: bool,
    pub(crate) overwrite: bool,
    pub(crate) keep_branch: bool,
}

pub(crate) fn cleanup_command(args: CleanupCommandRequest) -> Result<()> {
    let CleanupCommandRequest {
        run_id,
        all,
        completed,
        stale,
        no_confirm,
        escalate,
        overwrite,
        keep_branch,
    } = args;
    let paths = DeadreckonPaths::discover();
    if let Some(run_id) = run_id {
        let mut state = load_cli_run(&paths, &run_id)?;
        if state.status == RunStatus::Executing {
            if !escalate {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("run {} is {}", state.run_id, run_status_label(state.status)),
                    &format!("deadreckon cleanup {} --escalate", state.run_id),
                )));
            }
            let _ = kill_loaded_run(&paths, &mut state, escalate);
        }
        let record = read_codebase_record(&state.working_dir)?;
        let result = cleanup_worktree_run(
            &state,
            &record,
            keep_branch,
            overwrite,
            CleanupReason::Cleaned,
        )?;
        print_cleanup_results(&[result]);
        return Ok(());
    }

    let candidates = cleanup_candidates(&paths, all, completed, stale)?;
    if candidates.is_empty() {
        print!(
            "{}",
            cleanup_no_candidates_surface(completed, all)
                .render_plain(!completion_hints_enabled(false))
        );
        return Ok(());
    }

    println!("cleanup candidates:");
    for candidate in &candidates {
        println!(
            "  {:<8} {:<10} {:<16} {}",
            run_prefix(&candidate.state.run_id),
            candidate.state.status,
            candidate.reason,
            one_line(&candidate.state.goal, 72)
        );
    }
    if !no_confirm {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive cleanup requires --no-confirm",
                "deadreckon cleanup --no-confirm",
            )));
        }
        if !prompt::confirm("clean these runs?", false)? {
            println!("cancelled");
            return Ok(());
        }
    }

    let mut results = Vec::new();
    for mut candidate in candidates {
        if candidate.state.status == RunStatus::Executing {
            let _ = kill_loaded_run(&paths, &mut candidate.state, escalate);
        }
        let result = cleanup_worktree_run(
            &candidate.state,
            &candidate.record,
            keep_branch,
            overwrite,
            CleanupReason::Cleaned,
        )?;
        results.push(result);
    }
    print_cleanup_results(&results);
    Ok(())
}

#[derive(Debug)]
struct CleanupCandidate {
    state: deadreckon_core::PipelineState,
    record: CodebaseRecord,
    reason: String,
}

fn cleanup_candidates(
    paths: &DeadreckonPaths,
    all: bool,
    include_completed: bool,
    include_stale: bool,
) -> Result<Vec<CleanupCandidate>> {
    let scope = if all { None } else { Some(current_scope()?) };
    let mut candidates = Vec::new();
    for run in list_runs(paths, scope.as_deref())? {
        let Ok(state) = load_run(paths, &run.run_id) else {
            continue;
        };
        let Ok(record) = read_codebase_record(&state.working_dir) else {
            continue;
        };
        if record.mode != CodebaseMode::Worktree {
            continue;
        }
        let abandoned = state.run_root.join("abandoned.json").exists();
        let stale = include_stale && is_stale_executing(&state);
        let completed = include_completed && state.status == RunStatus::Completed && !abandoned;
        if abandoned || stale || completed {
            let reason = if abandoned {
                "cleaned".to_string()
            } else if stale {
                "stale".to_string()
            } else {
                "completed".to_string()
            };
            candidates.push(CleanupCandidate {
                state,
                record,
                reason,
            });
        }
    }
    Ok(candidates)
}

#[derive(Debug, Clone, Copy)]
enum CleanupReason {
    Abandoned,
    Applied,
    Cleaned,
}

impl CleanupReason {
    fn marker(self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::Applied => "applied",
            Self::Cleaned => "cleaned",
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Self::Abandoned => "abandon",
            Self::Applied | Self::Cleaned => "cleanup",
        }
    }
}

#[derive(Debug)]
struct CleanupRunResult {
    run_id: String,
    status: RunStatus,
    mode: CodebaseMode,
    removed: Vec<String>,
    keep_branch: bool,
    force: bool,
    reason: CleanupReason,
}

fn cleanup_worktree_run(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    keep_branch: bool,
    force: bool,
    reason: CleanupReason,
) -> Result<CleanupRunResult> {
    let mut removed = Vec::new();
    if record.mode == CodebaseMode::Worktree
        && let (Some(git_root), Some(worktree)) = (
            record.source_git_root.as_ref(),
            record.worktree_path.as_ref(),
        )
    {
        if worktree.exists() {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(path_to_str(worktree)?);
            let _ = git_status(git_root, &args);
            removed.push(worktree.display().to_string());
        }
        if !keep_branch
            && let Some(branch) = record.branch_name.as_deref()
            && git_stdout(git_root, &["rev-parse", "--verify", branch]).is_ok()
        {
            let _ = git_status(git_root, &["branch", "-D", branch]);
            removed.push(format!("branch {branch}"));
        }
    }
    write_abandoned_marker(state, reason)?;
    Ok(CleanupRunResult {
        run_id: state.run_id.clone(),
        status: state.status,
        mode: record.mode,
        removed,
        keep_branch,
        force,
        reason,
    })
}

fn print_cleanup_results(results: &[CleanupRunResult]) {
    if results.len() == 1 {
        print!(
            "{}",
            cleanup_result_surface(&results[0]).render_plain(!completion_hints_enabled(false))
        );
        return;
    }
    let removed_count = results
        .iter()
        .map(|result| result.removed.len())
        .sum::<usize>();
    let subject = format!("{} runs", results.len());
    let primary = "deadreckon status".to_string();
    let secondary = "deadreckon list --all".to_string();
    let mut evidence = vec![
        ("runs".to_string(), results.len().to_string()),
        ("removed entries".to_string(), removed_count.to_string()),
    ];
    for result in results.iter().take(5) {
        evidence.push((
            format!("run {}", run_prefix(&result.run_id)),
            format!("{} removed", result.removed.len()),
        ));
    }
    print!(
        "{}",
        VerdictSurface::try_new(
            VerdictKind::Completed,
            "cleanup",
            Some(&subject),
            ExplanationPanel::new(
                "DeadReckon removed the selected temporary run worktrees and branches.",
                "Cleanup completed across multiple runs; inspect status before starting another destructive cleanup.",
                evidence,
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .expect("aggregate cleanup verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false))
    );
}

fn cleanup_result_surface(result: &CleanupRunResult) -> VerdictSurface {
    let id = run_prefix(&result.run_id);
    let primary = format!("deadreckon show {id}");
    let secondary = "deadreckon status".to_string();
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        (
            "status".to_string(),
            run_status_label(result.status).to_string(),
        ),
        ("mode".to_string(), result.mode.to_string()),
        (
            "removed entries".to_string(),
            result.removed.len().to_string(),
        ),
        (
            "branch".to_string(),
            if result.keep_branch {
                "kept".to_string()
            } else {
                "removed when present".to_string()
            },
        ),
    ];
    if result.force {
        evidence.push(("force".to_string(), "true".to_string()));
    }
    for (index, item) in result.removed.iter().take(5).enumerate() {
        evidence.push((format!("removed {}", index + 1), item.clone()));
    }
    if result.removed.len() > 5 {
        evidence.push((
            "additional removed entries".to_string(),
            (result.removed.len() - 5).to_string(),
        ));
    }
    let what = match result.reason {
        CleanupReason::Abandoned => {
            "DeadReckon marked the run abandoned and removed its temporary worktree resources."
        }
        CleanupReason::Applied => {
            "DeadReckon removed temporary worktree resources after applying the run."
        }
        CleanupReason::Cleaned => {
            "DeadReckon removed the selected temporary run worktree resources."
        }
    };
    let why = match result.reason {
        CleanupReason::Abandoned => {
            "This is completed abandon cleanup; inspect the run record if you need provenance or docs."
        }
        CleanupReason::Applied | CleanupReason::Cleaned => {
            "This is completed cleanup; inspect the run record before further recovery or deletion."
        }
    };
    VerdictSurface::try_new(
        VerdictKind::Completed,
        result.reason.subject(),
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("cleanup verdict surface must be valid")
}

fn cleanup_no_candidates_surface(completed: bool, all: bool) -> VerdictSurface {
    let mut secondary = Vec::new();
    let primary = if !completed {
        if !all {
            secondary.push("deadreckon cleanup --all-scopes".to_string());
        }
        "deadreckon cleanup --completed".to_string()
    } else if !all {
        "deadreckon cleanup --all-scopes".to_string()
    } else {
        "deadreckon status".to_string()
    };
    cleanup_noop_surface(
        "cleanup",
        None,
        "DeadReckon did not find any temporary run worktrees or branches matching this cleanup filter.",
        "No cleanup mutation was made; broaden the safe filter if you expected completed or cross-scope candidates.",
        vec![
            ("completed filter".to_string(), completed.to_string()),
            ("all scopes".to_string(), all.to_string()),
        ],
        primary,
        secondary,
    )
}

fn cleanup_noop_surface(
    subject_kind: &str,
    subject: Option<&str>,
    what: impl Into<String>,
    why: impl Into<String>,
    evidence: Vec<(String, String)>,
    primary: String,
    secondary: Vec<String>,
) -> VerdictSurface {
    let secondary = secondary
        .iter()
        .map(|command| ("Secondary", command.as_str()))
        .collect::<Vec<_>>();
    VerdictSurface::try_new(
        VerdictKind::Noop,
        subject_kind,
        subject,
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary,
    )
    .expect("cleanup no-op verdict surface must be valid")
}

fn codebase_mode_refusal_error(
    subject_kind: &str,
    state: &deadreckon_core::PipelineState,
    mode: CodebaseMode,
    what: &str,
    primary: String,
    why: &str,
) -> CliError {
    let id = run_prefix(&state.run_id);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::try_new(
            VerdictKind::Blocked,
            subject_kind,
            Some(&id),
            ExplanationPanel::new(
                what,
                why,
                [
                    ("run".to_string(), id.clone()),
                    (
                        "status".to_string(),
                        run_status_label(state.status).to_string(),
                    ),
                    ("mode".to_string(), mode.to_string()),
                    (
                        "working".to_string(),
                        state.working_dir.display().to_string(),
                    ),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .expect("codebase mode refusal verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn apply_mode_error(state: &deadreckon_core::PipelineState, mode: CodebaseMode) -> CliError {
    let run_id = run_prefix(&state.run_id);
    let primary = match mode {
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            format!("deadreckon export {run_id} --dest <path>")
        }
        CodebaseMode::InPlace => format!("deadreckon undo --run {run_id}"),
        CodebaseMode::Worktree => format!("deadreckon apply {run_id}"),
    };
    codebase_mode_refusal_error(
        "apply",
        state,
        mode,
        &format!("apply requires worktree mode; run was {mode}"),
        primary,
        "Apply only lands isolated worktree branches; choose the recovery command that matches this run's source mode.",
    )
}

pub(crate) async fn extend_command(args: ExtendCommandArgs) -> Result<()> {
    let ExtendCommandArgs {
        parent_run_id,
        new_goal,
        dest,
        max_context_turns,
        no_context,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        no_docs,
        doc_skill,
        post_actions,
    } = args;
    let new_goal = new_goal.trim().to_string();
    if new_goal.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--goal must be non-empty".to_string(),
        )));
    }

    let paths = DeadreckonPaths::discover();
    let parent = load_cli_run(&paths, &parent_run_id)?;
    if parent.status != RunStatus::Completed {
        return Err(CliError::Surface {
            code: 1,
            surface: incomplete_parent_extend_surface(&parent)
                .render_plain(!completion_hints_enabled(false)),
        });
    }
    let parent_codebase = read_run_codebase_record(&paths, &parent).ok();
    if parent_codebase
        .as_ref()
        .is_some_and(|record| record.mode == CodebaseMode::InPlace)
    {
        return Err(CliError::Surface {
            code: 1,
            surface: in_place_parent_extend_surface(&parent, &new_goal)
                .render_plain(!completion_hints_enabled(false)),
        });
    }
    let parent_library = paths.library_dir(&parent.scope, &parent.run_id);
    if !parent_library.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(
            "parent library missing; cannot extend".to_string(),
        )));
    }

    let defaults = config_defaults(&paths)?;
    let primary_setup = provider_setup_selection(
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
    )?;
    let provider_override = provider_override_from_setup(&primary_setup);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        provider_override.as_deref(),
        model.as_deref(),
    )?;
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
    let doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup_selection(
        &paths,
        &defaults,
        None,
        effective_provider.as_deref(),
        false,
    )?);
    let doc_subskills = effective_doc_subskills(&defaults);
    let doc_token_budget = defaults
        .doc_polish_token_budget
        .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
    let doc_budget_cap = defaults.doc_polish_budget_cap_usd;
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    let cwd = if parent.cwd.exists() {
        parent.cwd.clone()
    } else {
        std::env::current_dir()?
    };
    let context_turns = context_turns(max_context_turns, no_context);
    if let Some(parent_record) = parent_codebase
        .as_ref()
        .filter(|record| record.mode == CodebaseMode::Worktree)
    {
        return extend_worktree_command(ExtendWorktreeArgs {
            paths,
            parent,
            parent_record: parent_record.clone(),
            new_goal,
            effective_provider,
            effective_max_spend,
            effective_max_wall_seconds,
            doc_provider: doc_provider_selection.provider.clone(),
            doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
            doc_subskills: doc_subskills.clone(),
            doc_token_budget,
            doc_budget_cap,
            doc_skill: effective_doc_skill,
            no_docs,
            backend,
            provider_override: provider_override.clone(),
            model: model.clone(),
            provider_source: primary_setup.source.as_str().to_string(),
            post_actions,
            context_turns,
        })
        .await;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: None,
            codebase: None,
        },
    )?;
    align_extended_run_with_parent(&paths, &mut state, &parent)?;

    let mut lock = match acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    ) {
        Ok(lock) => lock,
        Err(error) => {
            cleanup_new_run(&state);
            return Err(error.into());
        }
    };
    deadreckon_core::state::write_current_pointer(&paths, &state)?;

    if let Some(dest) = dest {
        let dest = absolute_dest(dest)?;
        refuse_dest_inside_home(&paths, &dest, "extend")?;
        prepare_empty_dest(&dest, false)?;
        remove_if_exists(&state.working_dir)?;
        state.working_dir = dest;
    }
    seed_working_from_library(&parent_library, &state.working_dir)?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        &extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    commands::acceptance::copy_existing_acceptance_into_run(
        &state,
        &[&state.cwd, &state.working_dir],
    )?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let router = provider_router_for_run_with_catalog_seam(
        &paths,
        &state,
        backend,
        provider_override.as_deref(),
        model.as_deref(),
        false,
    )
    .await?;
    let selected_route = router.selected_route_info();
    print_run_started(
        &state,
        selected_route.as_ref(),
        primary_setup.source.as_str(),
        doc_provider_selection.provider.as_deref(),
        doc_provider_selection.source.as_str(),
    );
    let wait_label = format!(
        "extended run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let outcome = with_cli_wait_status(
        &wait_label,
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: effective_provider,
                max_spend_usd: effective_max_spend,
                max_wall_seconds: effective_max_wall_seconds,
                sandbox_backend: backend,
                no_seams: false,
                max_turns: 12,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(paths.config_path()),
                    doc_provider: doc_provider_selection.provider.clone(),
                    doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                    doc_subskills,
                    token_budget: doc_token_budget,
                    budget_cap_usd: doc_budget_cap,
                    doc_skill: effective_doc_skill,
                    no_docs,
                },
            },
        ),
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    print_extended_run_outcome(&state, &outcome);
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    fire_lifecycle_notification(&paths, &state, &outcome).await;
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true, true)).await?;
    }
    Ok(())
}

fn incomplete_parent_extend_surface(parent: &deadreckon_core::PipelineState) -> VerdictSurface {
    let id = run_prefix(&parent.run_id);
    let primary = format!("deadreckon resume {id}");
    let secondary = format!("deadreckon show {id}");
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        "extend",
        Some(&id),
        ExplanationPanel::new(
            format!("Parent run {id} is {} and cannot be extended yet.", parent.status),
            "Extend requires a completed parent with promoted artifacts; an incomplete run may still change if it is resumed.",
            vec![
                ("run".to_string(), id.clone()),
                ("status".to_string(), parent.status.to_string()),
                ("state".to_string(), parent.state_path().display().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("incomplete parent extend verdict surface must be valid")
}

fn in_place_parent_extend_surface(
    parent: &deadreckon_core::PipelineState,
    new_goal: &str,
) -> VerdictSurface {
    let id = run_prefix(&parent.run_id);
    let primary = format!("deadreckon run --in-place --i-know-its-a-lot {new_goal:?}");
    let secondary = format!("deadreckon show {id}");
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        "extend",
        Some("in-place"),
        ExplanationPanel::new(
            format!("Parent run {id} is in-place and cannot be extended."),
            "Extend creates a follow-up from promoted copy or worktree artifacts; in-place work should continue as a new in-place run from the current checkout.",
            vec![
                ("run".to_string(), id.clone()),
                ("mode".to_string(), "in-place".to_string()),
                ("new goal".to_string(), new_goal.to_string()),
                ("state".to_string(), parent.state_path().display().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("in-place parent extend verdict surface must be valid")
}

struct ExtendWorktreeArgs {
    paths: DeadreckonPaths,
    parent: deadreckon_core::PipelineState,
    parent_record: CodebaseRecord,
    new_goal: String,
    effective_provider: Option<String>,
    effective_max_spend: Option<f64>,
    effective_max_wall_seconds: Option<f64>,
    doc_provider: Option<String>,
    doc_provider_source: Option<String>,
    doc_subskills: Vec<String>,
    doc_token_budget: u32,
    doc_budget_cap: Option<f64>,
    doc_skill: String,
    no_docs: bool,
    backend: SandboxBackend,
    provider_override: Option<String>,
    model: Option<String>,
    provider_source: String,
    post_actions: bool,
    context_turns: Option<u32>,
}

async fn extend_worktree_command(args: ExtendWorktreeArgs) -> Result<()> {
    let ExtendWorktreeArgs {
        paths,
        parent,
        parent_record,
        new_goal,
        effective_provider,
        effective_max_spend,
        effective_max_wall_seconds,
        doc_provider,
        doc_provider_source,
        doc_subskills,
        doc_token_budget,
        doc_budget_cap,
        doc_skill,
        no_docs,
        backend,
        provider_override,
        model,
        provider_source,
        post_actions,
        context_turns,
    } = args;
    let parent_branch = parent_record.branch_name.clone();
    let source_git_root = parent_record.source_git_root.clone().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent worktree record missing source_git_root".to_string(),
        ))
    })?;
    let base_ref = parent_branch
        .as_deref()
        .filter(|branch| git_ref_exists(&source_git_root, &format!("refs/heads/{branch}")))
        .map(str::to_string);
    let run_id = Uuid::new_v4().simple().to_string();
    let mut codebase = prepare_worktree_record(
        &paths,
        WorktreeOptions {
            run_id: run_id.clone(),
            task_key: deadreckon_core::paths::task_key(&new_goal),
            source_path: source_git_root.clone(),
            base_ref,
            branch_name: None,
            allow_dirty: false,
        },
    )?;
    codebase.parent_branch = parent_branch.or_else(|| codebase.base_ref.clone());
    create_worktree(&codebase)?;
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd: source_git_root,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase.clone()),
        },
    )?;

    let mut lock = acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        &extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    commands::acceptance::copy_existing_acceptance_into_run(
        &state,
        &[&state.cwd, &state.working_dir],
    )?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
                "mode": "worktree",
                "base_ref": codebase.base_ref.clone(),
                "parent_branch": codebase.parent_branch.clone(),
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let router = provider_router_for_run_with_catalog_seam(
        &paths,
        &state,
        backend,
        provider_override.as_deref(),
        model.as_deref(),
        false,
    )
    .await?;
    let selected_route = router.selected_route_info();
    print_run_started(
        &state,
        selected_route.as_ref(),
        &provider_source,
        doc_provider.as_deref(),
        doc_provider_source.as_deref().unwrap_or("none"),
    );
    let wait_label = format!(
        "extended run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let outcome = with_cli_wait_status(
        &wait_label,
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: effective_provider,
                max_spend_usd: effective_max_spend,
                max_wall_seconds: effective_max_wall_seconds,
                sandbox_backend: backend,
                no_seams: false,
                max_turns: 12,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(paths.config_path()),
                    doc_provider,
                    doc_provider_source,
                    doc_subskills,
                    token_budget: doc_token_budget,
                    budget_cap_usd: doc_budget_cap,
                    doc_skill,
                    no_docs,
                },
            },
        ),
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    print_extended_run_outcome(&state, &outcome);
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    fire_lifecycle_notification(&paths, &state, &outcome).await;
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true, true)).await?;
    }
    Ok(())
}

fn run_loop_outcome_status(outcome: &RunLoopOutcome) -> &'static str {
    match outcome {
        RunLoopOutcome::Done => "completed",
        RunLoopOutcome::PausedAtCap => "paused",
        RunLoopOutcome::Killed => "killed",
        RunLoopOutcome::Failed => "failed",
    }
}

fn notify_transition_for_outcome(outcome: &RunLoopOutcome) -> Option<NotifyTransition> {
    match outcome {
        RunLoopOutcome::Done => Some(NotifyTransition::Accepted),
        RunLoopOutcome::PausedAtCap => Some(NotifyTransition::Paused),
        RunLoopOutcome::Failed => Some(NotifyTransition::Failed),
        RunLoopOutcome::Killed => None,
    }
}

pub(crate) async fn fire_lifecycle_notification(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
) {
    let Some(transition) = notify_transition_for_outcome(outcome) else {
        return;
    };
    let Ok(config) = load_notify_config(paths) else {
        return;
    };
    if !config.enabled_for(transition) {
        return;
    }
    let channels = channels_for_config(&config);
    if channels.is_empty() {
        return;
    }
    let context = NotifyContext {
        transition,
        run_id: state.run_id.clone(),
        verdict: notification_verdict(state, outcome),
        spend: run_spend_label(state, false),
        narrative_path: doc_path_for_kind(&state.working_dir, DocKind::Narrative),
    };
    let _attempts = notify_run(state, &config, &context, &channels).await;
}

fn notification_verdict(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
) -> String {
    match outcome {
        RunLoopOutcome::Done => format!("{NOUN_VERIFIED_RUN} ({})", acceptance_status_value(state)),
        RunLoopOutcome::PausedAtCap => "paused at cap".to_string(),
        RunLoopOutcome::Failed => format!("failed run ({})", acceptance_status_value(state)),
        RunLoopOutcome::Killed => "killed run".to_string(),
    }
}

fn print_extended_run_outcome(state: &deadreckon_core::PipelineState, outcome: &RunLoopOutcome) {
    let status = run_loop_outcome_status(outcome);
    println!(
        "{} extended run {}",
        ui_status(status),
        ui_id(&state.run_id)
    );
}

fn align_extended_run_with_parent(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
) -> Result<()> {
    let old_scope = state.scope.clone();
    let old_task_key = state.task_key.clone();
    let old_pointer = paths.current_pointer_path(&old_scope, &old_task_key);
    let desired_root = paths.run_root(&parent.scope, &state.run_id);
    if state.run_root != desired_root {
        if let Some(parent_dir) = desired_root.parent() {
            fs::create_dir_all(parent_dir)?;
        }
        fs::rename(&state.run_root, &desired_root)?;
        state.run_root = desired_root;
        state.working_dir = state.run_root.join("working");
        state.scope = parent.scope.clone();
    }
    state.task_key = parent.task_key.clone();
    state.cwd = parent.cwd.clone();
    state.updated_at = Utc::now();
    let new_pointer = paths.current_pointer_path(&state.scope, &state.task_key);
    if old_pointer != new_pointer {
        remove_if_exists(&old_pointer)?;
    }
    save_state(state)?;
    Ok(())
}

fn cleanup_new_run(state: &deadreckon_core::PipelineState) {
    let _ = remove_if_exists(&state.run_root);
}

fn seed_working_from_library(library_dir: &Path, working_dir: &Path) -> Result<()> {
    copy_tree(library_dir, working_dir)?;
    remove_if_exists(&working_dir.join("manifest.json"))?;
    remove_if_exists(&working_dir.join(".materialized-to"))?;
    Ok(())
}

fn context_turns(max_context_turns: Option<u32>, no_context: bool) -> Option<u32> {
    if no_context {
        return None;
    }
    let turns = max_context_turns.unwrap_or(5);
    if turns == 0 { None } else { Some(turns) }
}

fn extended_parent_marker(
    parent: &deadreckon_core::PipelineState,
    new_goal: &str,
    context_turns: Option<u32>,
) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "extended".to_string(),
        parent_run_id: parent.run_id.clone(),
        parent_scope: parent.scope.clone(),
        parent_goal: parent.goal.clone(),
        parent_completed_at: parent.updated_at,
        materialized_at: None,
        extended_at: Some(Utc::now()),
        new_goal: Some(new_goal.to_string()),
        context_turns_included: context_turns,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_history(
    state: &deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
    context_turns: Option<u32>,
) -> Result<()> {
    let history = vec![parent_summary(parent, context_turns)];
    fs::write(
        state.run_root.join("history.json"),
        serde_json::to_vec_pretty(&history)?,
    )?;
    Ok(())
}

fn parent_summary(parent: &deadreckon_core::PipelineState, context_turns: Option<u32>) -> String {
    let spend = read_jsonl::<SpendRecord>(&parent.run_root.join("spend.jsonl")).unwrap_or_default();
    let traces =
        read_jsonl::<TraceRecord>(&parent.run_root.join("traces.jsonl")).unwrap_or_default();
    let spend_label = if spend.iter().any(|record| record.subscription)
        || parent
            .provider
            .as_deref()
            .is_some_and(|provider| provider.starts_with("cli:"))
    {
        "subscription".to_string()
    } else {
        format!("${:.6}", parent.total_spend_usd)
    };
    let acceptance = if parent.run_root.join("proofs/turn-acceptance.json").exists() {
        "dr-gate accepted"
    } else {
        "not recorded"
    };
    let mut summary = format!(
        "# Previous run summary ({})\n\n**Original goal.** {}\n**Completed.** {}\n**Total turns.** {}\n**Total spend.** {}\n**Acceptance.** {}\n",
        parent.run_id,
        parent.goal,
        parent.updated_at.to_rfc3339(),
        parent.turn,
        spend_label,
        acceptance
    );
    if let Some(max_turns) = context_turns {
        let mut recent = traces
            .iter()
            .filter(|trace| trace.turn > 0)
            .rev()
            .take(max_turns as usize)
            .map(trace_one_liner)
            .collect::<Vec<_>>();
        recent.reverse();
        summary.push_str(&format!(
            "\n## Recent activity (last {} turns)\n\n",
            max_turns
        ));
        if recent.is_empty() {
            summary.push_str("- no trace activity recorded\n");
        } else {
            for line in recent {
                summary.push_str(&format!("- {line}\n"));
            }
        }
    }
    summary
}

fn trace_one_liner(trace: &TraceRecord) -> String {
    let detail = trace
        .detail
        .get("tool_call_id")
        .and_then(Value::as_str)
        .or_else(|| trace.detail.get("summary").and_then(Value::as_str))
        .unwrap_or("");
    if detail.is_empty() {
        format!("turn {}: {}", trace.turn, trace.event)
    } else {
        format!(
            "turn {}: {} {}",
            trace.turn,
            trace.event,
            one_line(detail, 90)
        )
    }
}

fn ensure_completed_run(state: &deadreckon_core::PipelineState, verb: &str) -> Result<()> {
    if state.status != RunStatus::Completed {
        return Err(CliError::Surface {
            code: 1,
            surface: incomplete_run_required_surface(state, verb)
                .render_plain(!completion_hints_enabled(false)),
        });
    }
    Ok(())
}

fn incomplete_run_required_surface(
    state: &deadreckon_core::PipelineState,
    verb: &str,
) -> VerdictSurface {
    let id = run_prefix(&state.run_id);
    let primary = format!("deadreckon resume {id}");
    let secondary = format!("deadreckon show {id}");
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        verb,
        Some(&id),
        ExplanationPanel::new(
            format!("{verb} requires a completed run, but run {id} is {}.", state.status),
            "This command needs stable completed artifacts; an incomplete run may still change if it is resumed.",
            vec![
                ("run".to_string(), id.clone()),
                ("status".to_string(), state.status.to_string()),
                ("state".to_string(), state.state_path().display().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
    .expect("incomplete run requirement verdict surface must be valid")
}

fn materialized_parent_marker(state: &deadreckon_core::PipelineState) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "materialized".to_string(),
        parent_run_id: state.run_id.clone(),
        parent_scope: state.scope.clone(),
        parent_goal: state.goal.clone(),
        parent_completed_at: state.updated_at,
        materialized_at: Some(Utc::now()),
        extended_at: None,
        new_goal: None,
        context_turns_included: None,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_marker(path: &Path, marker: &ParentMarker) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(marker)?)?;
    Ok(())
}

fn append_materialized_marker(library_dir: &Path, dest: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(library_dir.join(".materialized-to"))?;
    writeln!(file, "{}\t{}", Utc::now().to_rfc3339(), dest.display())?;
    Ok(())
}

fn write_abandoned_marker(
    state: &deadreckon_core::PipelineState,
    reason: CleanupReason,
) -> Result<()> {
    let path = state.run_root.join("abandoned.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "run_id": state.run_id,
            "abandoned_at": Utc::now(),
            "reason": reason.marker(),
        }))?,
    )?;
    Ok(())
}

fn prepare_empty_dest(dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        let non_empty = !path_is_empty_dir(dest)?;
        if non_empty && !force {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "dest {} is not empty (use --overwrite or pass a fresh path)",
                dest.display()
            ))));
        }
        if force {
            remove_if_exists(dest)?;
        }
    }
    fs::create_dir_all(dest)?;
    Ok(())
}

pub(crate) fn path_is_empty_dir(path: &Path) -> Result<bool> {
    if path.is_dir() {
        Ok(fs::read_dir(path)?.next().is_none())
    } else {
        Ok(false)
    }
}

pub(crate) fn default_materialize_dest(state: &deadreckon_core::PipelineState) -> PathBuf {
    state
        .cwd
        .join(state.task_key.chars().take(24).collect::<String>())
}
