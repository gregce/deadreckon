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
    materialize_command_with_paths(&paths, &run_id, dest, force, include_manifest)
}

fn materialize_command_with_paths(
    paths: &DeadreckonPaths,
    run_id: &str,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<()> {
    let resolved = super::reference::resolve_ref(
        paths,
        super::reference::RefQuery {
            reference: Some(run_id),
            all_scopes: false,
            verb: "export",
        },
    )?;
    let (state, plan_context, dest, authority) = match resolved {
        super::reference::ResolvedRef::Job(job) => {
            let state = finish_job_state(paths, &job)?;
            let authority = MaterializeDeliveryAuthority::Verified(Box::new(
                VerifiedDeliveryAuthority::from_finished_job(paths, &job, &state)?,
            ));
            (state, None, dest, authority)
        }
        super::reference::ResolvedRef::Run(state)
        | super::reference::ResolvedRef::PlanChild { state, .. } => {
            let state = *state;
            let authority = materialize_delivery_authority(paths, &state)?;
            (state, None, dest, authority)
        }
        super::reference::ResolvedRef::Plan(plan) => {
            let plan_id = plan.plan_id.clone();
            let Some(result) = resolve_plan_result_run(paths, &plan_id, "export")? else {
                return Err(super::reference::refusal_for(
                    super::reference::RefKind::Plan,
                    "export",
                    &plan_id,
                ));
            };
            let authority = materialize_delivery_authority(paths, &result.state)?;
            let dest = dest.or_else(|| Some(default_plan_materialize_dest(&result.plan)));
            (result.state, Some(result.plan), dest, authority)
        }
        other => {
            return Err(super::reference::refusal_for(
                other.kind(),
                "export",
                &super::reference::resolved_id(&other),
            ));
        }
    };
    if let Some(plan) = plan_context.as_ref() {
        print_plan_result_context(plan, &state);
        let library_dir = paths.library_dir(&state.scope, &state.run_id);
        materialize_plan_docs_to_working(paths, plan, &library_dir, None)?;
    }
    let materialized = materialize_completed_run_with_authority(
        paths,
        &state,
        dest,
        force,
        include_manifest,
        &authority,
    )?;
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
    let resolved = super::reference::resolve_ref(
        &paths,
        super::reference::RefQuery {
            reference: Some(&requested),
            all_scopes: false,
            verb: "finish",
        },
    )?;
    let (state, plan_context, dest, finished_job_id) = match resolved {
        super::reference::ResolvedRef::Job(job) => {
            let job_id = job.job.job_id.as_ref().to_string();
            (finish_job_state(&paths, &job)?, None, dest, Some(job_id))
        }
        super::reference::ResolvedRef::Run(state) => {
            super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "finish")?;
            (*state, None, dest, None)
        }
        super::reference::ResolvedRef::PlanChild { state, .. } => {
            super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "finish")?;
            (*state, None, dest, None)
        }
        super::reference::ResolvedRef::Plan(plan) => {
            let plan_id = plan.plan_id.clone();
            let Some(result) = resolve_plan_result_run(&paths, &plan_id, "finish")? else {
                return Err(super::reference::refusal_for(
                    super::reference::RefKind::Plan,
                    "finish",
                    &plan_id,
                ));
            };
            super::graph_job::require_current_driver_for_job_owned_run(
                &paths,
                &result.state,
                "finish",
            )?;
            if dest.is_none() && plan_apply_git_root(&result.plan)?.is_some() {
                return apply_command_inner(
                    plan_id, strategy, branch, no_confirm, autostash, cleanup, message, false,
                    false, None, None,
                );
            }
            let dest = Some(dest.unwrap_or_else(|| default_plan_materialize_dest(&result.plan)));
            (result.state, Some(result.plan), dest, None)
        }
        other => {
            return Err(super::reference::refusal_for(
                other.kind(),
                "finish",
                &super::reference::resolved_id(&other),
            ));
        }
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

    let mode = lifecycle_codebase_record(&paths, &state)?.mode;
    match mode {
        CodebaseMode::Worktree => {
            apply_command_inner(
                state.run_id.clone(),
                strategy,
                branch,
                no_confirm,
                autostash,
                cleanup,
                message,
                false,
                false,
                Some(state),
                finished_job_id.as_deref(),
            )?;
            Ok(())
        }
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            let materialized =
                materialize_completed_run(&paths, &state, dest, force, include_manifest)?;
            print_materialized(&materialized);
            Ok(())
        }
        CodebaseMode::InPlace => {
            let prefix = run_prefix(&state.run_id);
            println!(
                "{} {}",
                ui_ok("finished in-place run"),
                ui_id(&state.run_id)
            );
            println!("  {} {}", ui_muted("working:"), state.working_dir.display());
            println!(
                "{}",
                VerdictSurface::must_new(
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
                .render_plain(false)
            );
            Ok(())
        }
    }
}

fn delivered_git_revision(destination: &Path) -> Option<String> {
    deadreckon_core::git::run_git(destination, &["rev-parse", "HEAD"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

fn lifecycle_codebase_record(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> deadreckon_core::Result<CodebaseRecord> {
    if paths.job_json(&state.run_id).is_file() {
        return deadreckon_core::read_trusted_codebase_record(&state.run_root);
    }
    deadreckon_core::read_run_codebase_record(&state.run_root, &state.working_dir)
}

pub(crate) fn finish_job_state(
    paths: &DeadreckonPaths,
    job: &deadreckon_core::JobView,
) -> Result<deadreckon_core::PipelineState> {
    if job.projection.outcome != Some(deadreckon_protocol::JobOutcome::Verified)
        || job.projection.stop_reason != Some(deadreckon_protocol::StopReason::Verified)
    {
        let status = super::job::job_status_label(job);
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "job {} is {status}; no verified receipt is available",
                job.job.job_id
            ),
            &format!("deadreckon attach {}", run_prefix(job.job.job_id.as_ref())),
        )));
    }
    let receipt_path = paths.job_receipt(job.job.job_id.as_ref());
    let receipt_raw = fs::read(&receipt_path).map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("job {} has no sealed completion receipt", job.job.job_id),
            &format!("deadreckon attach {}", run_prefix(job.job.job_id.as_ref())),
        ))
    })?;
    let receipt: deadreckon_protocol::CompletionReceipt = serde_json::from_slice(&receipt_raw)
        .map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                &format!("job {} completion receipt is unreadable", job.job.job_id),
                &format!("deadreckon attach {}", run_prefix(job.job.job_id.as_ref())),
            ))
        })?;
    if !receipt.contained || receipt.sandbox_backend == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "job {} receipt does not prove contained execution",
                job.job.job_id
            ),
            &format!("deadreckon attach {}", run_prefix(job.job.job_id.as_ref())),
        )));
    }
    let state = load_run(paths, job.job.job_id.as_ref())?;
    deadreckon_core::validate_completion_receipt(paths, &state)?;
    Ok(state)
}

fn print_finish_consistency_summary(state: &deadreckon_core::PipelineState) {
    println!("{}", ui_heading("run summary"));
    println!("  {} {}", ui_muted("spend:"), run_spend_label(state, false));
    println!("  {} {}", ui_muted("gate:"), acceptance_status_value(state));
}

#[derive(Debug)]
pub(crate) struct MaterializedRun {
    run_id: String,
    source: PathBuf,
    dest: PathBuf,
}

#[derive(Debug)]
pub(crate) struct VerifiedDeliveryAuthority {
    job_id: String,
    run_id: String,
    receipt: deadreckon_protocol::CompletionReceipt,
    receipt_sha256: String,
}

impl VerifiedDeliveryAuthority {
    fn from_finished_job(
        paths: &DeadreckonPaths,
        job: &deadreckon_core::JobView,
        state: &deadreckon_core::PipelineState,
    ) -> Result<Self> {
        if job.job.job_id.as_ref() != state.run_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "verified Job {} resolved a different delivery Run {}",
                job.job.job_id, state.run_id
            ))));
        }
        let receipt = deadreckon_core::validate_completion_receipt(paths, state)?;
        let receipt_sha256 =
            deadreckon_core::flight::sha256_file(&paths.job_receipt(job.job.job_id.as_ref()))?;
        Ok(Self {
            job_id: job.job.job_id.as_ref().to_string(),
            run_id: state.run_id.clone(),
            receipt,
            receipt_sha256,
        })
    }

    fn revalidate(
        &self,
        paths: &DeadreckonPaths,
        state: &deadreckon_core::PipelineState,
    ) -> Result<()> {
        let receipt = deadreckon_core::validate_completion_receipt(paths, state)?;
        let receipt_sha256 =
            deadreckon_core::flight::sha256_file(&paths.job_receipt(&self.job_id))?;
        if receipt != self.receipt || receipt_sha256 != self.receipt_sha256 {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "job {} completion receipt changed while its result was being exported",
                    self.job_id
                ),
                &format!("deadreckon verdict {}", run_prefix(&self.job_id)),
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
enum MaterializeDeliveryAuthority {
    LegacyUnowned,
    Verified(Box<VerifiedDeliveryAuthority>),
}

fn materialize_delivery_authority(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<MaterializeDeliveryAuthority> {
    let owner_job_id = if paths.job_json(&state.run_id).is_file() {
        Some(state.run_id.clone())
    } else {
        super::graph_job::resolve_run_owner(paths, state)?
            .map(|owner| owner.job.job_id.as_ref().to_string())
    };
    let Some(owner_job_id) = owner_job_id else {
        return Ok(MaterializeDeliveryAuthority::LegacyUnowned);
    };
    if owner_job_id != state.run_id {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "delivery cannot use {} because it belongs to durable Job {}",
                run_prefix(&state.run_id),
                run_prefix(&owner_job_id)
            ),
            &format!("deadreckon attach {}", run_prefix(&owner_job_id)),
        )));
    }
    let job = deadreckon_core::JobView::load(paths, &owner_job_id)?;
    let finished = finish_job_state(paths, &job)?;
    if finished.run_id != state.run_id {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "export cannot deliver Run {} because Job {} verified a different result",
                run_prefix(&state.run_id),
                run_prefix(&owner_job_id)
            ),
            &format!("deadreckon finish {}", run_prefix(&owner_job_id)),
        )));
    }
    Ok(MaterializeDeliveryAuthority::Verified(Box::new(
        VerifiedDeliveryAuthority::from_finished_job(paths, &job, &finished)?,
    )))
}

pub(crate) fn materialize_completed_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<MaterializedRun> {
    let authority = materialize_delivery_authority(paths, state)?;
    materialize_completed_run_with_authority(
        paths,
        state,
        dest,
        force,
        include_manifest,
        &authority,
    )
}

fn materialize_completed_run_with_authority(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
    authority: &MaterializeDeliveryAuthority,
) -> Result<MaterializedRun> {
    if let MaterializeDeliveryAuthority::Verified(authority) = authority
        && authority.run_id != state.run_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "delivery authority for Job {} does not authorize Run {}",
            authority.job_id, state.run_id
        ))));
    }
    ensure_completed_run(state, "materialize")?;
    let record = lifecycle_codebase_record(paths, state)?;
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
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if !library_dir.is_dir() {
        // A bare `NotFound` renders the generic "try: deadreckon list to find
        // valid run ids" hint, which reads as "your id is wrong" when the id is
        // fine and the promotion is what is missing. The run resolved; point at
        // the command that explains why it has nothing to export.
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "library missing for run {}; was promotion successful?",
                run_prefix(&state.run_id)
            ),
            &format!("deadreckon show {}", run_prefix(&state.run_id)),
        )));
    }

    let dest = absolute_dest(dest.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(run_prefix(&state.run_id))
    }))?;
    refuse_dest_inside_home(paths, &dest, "export")?;

    if let MaterializeDeliveryAuthority::Verified(authority) = authority {
        return materialize_verified_completed_run(
            paths,
            state,
            &library_dir,
            &dest,
            force,
            include_manifest,
            authority,
            VerifiedExportFailpoint::None,
            None,
        );
    }

    prepare_empty_dest(&dest, force)?;

    copy_deliverable_tree(&library_dir, &dest)?;
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

    let materialized = MaterializedRun {
        run_id: state.run_id.clone(),
        source: library_dir,
        dest,
    };
    if let MaterializeDeliveryAuthority::Verified(authority) = authority {
        let revision = delivered_git_revision(&materialized.dest);
        super::job::record_job_delivery(
            paths,
            &authority.job_id,
            super::job::JobDeliveryKind::Exported,
            &materialized.dest,
            revision.as_deref(),
        )?;
    }
    Ok(materialized)
}

const VERIFIED_EXPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VerifiedExportPhase {
    Prepared,
    BackupMoved,
    Published,
    Recorded,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifiedExportMarker {
    schema_version: u32,
    kind: String,
    transaction_id: String,
    job_id: String,
    run_id: String,
    destination: PathBuf,
    receipt_sha256: String,
    result_tree_sha256: String,
    include_manifest: bool,
    manifest_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifiedExportTransaction {
    schema_version: u32,
    transaction_id: String,
    job_id: String,
    run_id: String,
    destination: PathBuf,
    destination_parent_identity: String,
    receipt_sha256: String,
    result_tree_sha256: String,
    include_manifest: bool,
    manifest_sha256: Option<String>,
    previous_destination_sha256: Option<String>,
    stage: PathBuf,
    backup: PathBuf,
    phase: VerifiedExportPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct VerifiedExportBinding<'a> {
    schema_version: u32,
    job_id: &'a str,
    run_id: &'a str,
    destination: &'a Path,
    receipt_sha256: &'a str,
    result_tree_sha256: &'a str,
    include_manifest: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifiedExportFailpoint {
    None,
    AfterStageSync,
    AfterBackupRename,
    AfterPublish,
    AfterEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifiedDestinationState {
    Missing,
    Previous,
    Exported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifiedStageState {
    Missing,
    PartialOwned,
    Complete,
}

#[allow(clippy::too_many_arguments)]
fn materialize_verified_completed_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    library_dir: &Path,
    dest: &Path,
    force: bool,
    include_manifest: bool,
    authority: &VerifiedDeliveryAuthority,
    failpoint: VerifiedExportFailpoint,
    after_initial_validation: Option<&mut dyn FnMut()>,
) -> Result<MaterializedRun> {
    authority.revalidate(paths, state)?;
    // `absolute_dest` can retain an empty unresolved suffix when a previously
    // absent directory now exists. Normalize that representation so the same
    // canonical destination produces the same durable transaction on retry.
    let dest = lexical_normalize_path(dest);
    let parent = dest.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "export destination {} has no parent directory",
            dest.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if canonical_parent != parent {
        return Err(verified_export_refusal(
            authority,
            &format!(
                "destination parent {} changed identity while export was being prepared",
                parent.display()
            ),
        ));
    }
    let parent_identity = verified_directory_identity(parent)?;
    let binding = VerifiedExportBinding {
        schema_version: VERIFIED_EXPORT_SCHEMA_VERSION,
        job_id: &authority.job_id,
        run_id: &authority.run_id,
        destination: &dest,
        receipt_sha256: &authority.receipt_sha256,
        result_tree_sha256: &authority.receipt.result_tree_sha256,
        include_manifest,
    };
    let transaction_id = deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&binding).map_err(CliError::from)?,
    );
    let transaction_id = transaction_id
        .strip_prefix("sha256:")
        .unwrap_or(&transaction_id)
        .to_string();
    let stage = parent.join(format!(".deadreckon-export-{transaction_id}.stage"));
    let backup = parent.join(format!(".deadreckon-export-{transaction_id}.backup"));
    let journal_path = paths
        .job_dir(&authority.job_id)
        .join("export-transactions")
        .join(format!("{transaction_id}.json"));
    let lock_key = format!("export-{transaction_id}");
    let lock = acquire_lock(
        paths,
        &lock_key,
        &authority.run_id,
        &state.scope,
        "verified-export",
        Duration::from_secs(30 * 60),
    )?;
    let result = materialize_verified_completed_run_locked(
        paths,
        state,
        library_dir,
        &dest,
        force,
        include_manifest,
        authority,
        failpoint,
        after_initial_validation,
        transaction_id,
        parent_identity,
        stage,
        backup,
        &journal_path,
    );
    match (result, lock.release()) {
        (Ok(materialized), Ok(())) => Ok(materialized),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(CliError::Core(error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn materialize_verified_completed_run_locked(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    library_dir: &Path,
    dest: &Path,
    force: bool,
    include_manifest: bool,
    authority: &VerifiedDeliveryAuthority,
    failpoint: VerifiedExportFailpoint,
    mut after_initial_validation: Option<&mut dyn FnMut()>,
    transaction_id: String,
    parent_identity: String,
    stage: PathBuf,
    backup: PathBuf,
    journal_path: &Path,
) -> Result<MaterializedRun> {
    let marker = VerifiedExportMarker {
        schema_version: VERIFIED_EXPORT_SCHEMA_VERSION,
        kind: "verified_export".to_string(),
        transaction_id: transaction_id.clone(),
        job_id: authority.job_id.clone(),
        run_id: authority.run_id.clone(),
        destination: dest.to_path_buf(),
        receipt_sha256: authority.receipt_sha256.clone(),
        result_tree_sha256: authority.receipt.result_tree_sha256.clone(),
        include_manifest,
        manifest_sha256: None,
    };
    let mut transaction = if path_lexically_present(journal_path)? {
        let transaction = read_verified_export_transaction(journal_path, authority)?;
        validate_verified_export_transaction(
            &transaction,
            &marker,
            &parent_identity,
            &stage,
            &backup,
            authority,
        )?;
        transaction
    } else {
        if path_lexically_present(&stage)? || path_lexically_present(&backup)? {
            return Err(verified_export_refusal(
                authority,
                "derived export staging paths exist without a trusted transaction journal",
            ));
        }
        let previous_destination_sha256 = if path_lexically_present(dest)? {
            if !force && !path_is_empty_dir(dest)? {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "dest {} is not empty (use --overwrite or pass a fresh path)",
                    dest.display()
                ))));
            }
            Some(verified_path_identity(dest)?)
        } else {
            None
        };
        let manifest_sha256 = if include_manifest {
            regular_file_sha256_if_present(&library_dir.join("manifest.json"))?
        } else {
            None
        };
        let transaction = VerifiedExportTransaction {
            schema_version: VERIFIED_EXPORT_SCHEMA_VERSION,
            transaction_id,
            job_id: authority.job_id.clone(),
            run_id: authority.run_id.clone(),
            destination: dest.to_path_buf(),
            destination_parent_identity: parent_identity,
            receipt_sha256: authority.receipt_sha256.clone(),
            result_tree_sha256: authority.receipt.result_tree_sha256.clone(),
            include_manifest,
            manifest_sha256,
            previous_destination_sha256,
            stage,
            backup,
            phase: VerifiedExportPhase::Prepared,
        };
        write_verified_export_transaction(journal_path, &transaction)?;
        transaction
    };
    let marker = VerifiedExportMarker {
        manifest_sha256: transaction.manifest_sha256.clone(),
        ..marker
    };
    validate_verified_export_parent(&transaction)?;
    if let Some(hook) = after_initial_validation.as_mut() {
        hook();
    }

    let mut destination_state =
        inspect_verified_destination(dest, &transaction, &marker, authority)?;
    let mut stage_state = inspect_verified_stage(&transaction.stage, &marker, authority)?;
    let backup_present = inspect_verified_backup(&transaction, authority)?;

    if destination_state == VerifiedDestinationState::Exported {
        return finish_published_verified_export(
            paths,
            state,
            library_dir,
            authority,
            failpoint,
            journal_path,
            &marker,
            &mut transaction,
            stage_state,
            backup_present,
        );
    }
    if destination_state == VerifiedDestinationState::Previous && backup_present {
        return Err(verified_export_refusal(
            authority,
            "both the destination and backup contain the pre-export tree",
        ));
    }
    if destination_state == VerifiedDestinationState::Missing
        && transaction.previous_destination_sha256.is_some()
        && !backup_present
    {
        return Err(verified_export_refusal(
            authority,
            "the prior destination disappeared without the receipt-bound backup",
        ));
    }

    if stage_state == VerifiedStageState::PartialOwned {
        remove_owned_verified_export_path(&transaction.stage, &marker, authority)?;
        sync_verified_export_parent(&transaction)?;
        stage_state = VerifiedStageState::Missing;
    }
    if stage_state == VerifiedStageState::Missing {
        create_verified_export_stage(&transaction.stage, &marker)?;
        let stage_result = (|| {
            copy_deliverable_tree(library_dir, &transaction.stage)?;
            if !include_manifest {
                remove_if_exists(&transaction.stage.join("manifest.json"))?;
            }
            remove_if_exists(&transaction.stage.join(".materialized-to"))?;
            write_parent_marker(
                &transaction.stage.join(".deadreckon").join("parent.json"),
                &materialized_parent_marker(state),
            )?;
            normalize_permissions(&transaction.stage)?;
            sync_verified_tree(&transaction.stage)?;
            authority.revalidate(paths, state)?;
            validate_verified_export_manifest(library_dir, &transaction)?;
            require_verified_export_identity(&transaction.stage, &marker, authority)
        })();
        if let Err(error) = stage_result {
            if verified_export_marker_matches(&transaction.stage, &marker)? {
                remove_owned_verified_export_path(&transaction.stage, &marker, authority)?;
                sync_verified_export_parent(&transaction)?;
            }
            if destination_state == VerifiedDestinationState::Missing && backup_present {
                restore_verified_export_backup(&mut transaction, journal_path, authority)?;
            }
            return Err(error);
        }
        verified_export_fail(failpoint, VerifiedExportFailpoint::AfterStageSync)?;
    }

    if let Err(error) = (|| {
        authority.revalidate(paths, state)?;
        validate_verified_export_manifest(library_dir, &transaction)?;
        require_verified_export_identity(&transaction.stage, &marker, authority)
    })() {
        if destination_state == VerifiedDestinationState::Missing && backup_present {
            restore_verified_export_backup(&mut transaction, journal_path, authority)?;
        }
        if verified_export_marker_matches(&transaction.stage, &marker)? {
            remove_owned_verified_export_path(&transaction.stage, &marker, authority)?;
            sync_verified_export_parent(&transaction)?;
        }
        return Err(error);
    }
    validate_verified_export_parent(&transaction)?;

    if destination_state == VerifiedDestinationState::Previous {
        require_previous_destination_identity(&transaction, authority)?;
        fs::rename(dest, &transaction.backup)?;
        sync_verified_export_parent(&transaction)?;
        verified_export_fail(failpoint, VerifiedExportFailpoint::AfterBackupRename)?;
        transaction.phase = VerifiedExportPhase::BackupMoved;
        write_verified_export_transaction(journal_path, &transaction)?;
        destination_state = VerifiedDestinationState::Missing;
    }
    if destination_state != VerifiedDestinationState::Missing {
        return Err(verified_export_refusal(
            authority,
            "destination changed before atomic export publication",
        ));
    }
    if path_lexically_present(dest)? {
        return Err(verified_export_refusal(
            authority,
            "destination appeared before atomic export publication",
        ));
    }
    fs::rename(&transaction.stage, dest)?;
    sync_verified_export_parent(&transaction)?;
    verified_export_fail(failpoint, VerifiedExportFailpoint::AfterPublish)?;
    transaction.phase = VerifiedExportPhase::Published;
    write_verified_export_transaction(journal_path, &transaction)?;

    if let Err(error) = (|| {
        authority.revalidate(paths, state)?;
        validate_verified_export_manifest(library_dir, &transaction)?;
        require_verified_export_identity(dest, &marker, authority)
    })() {
        rollback_published_verified_export(&mut transaction, journal_path, &marker, authority)?;
        return Err(error);
    }
    let backup_present = transaction.previous_destination_sha256.is_some();
    finish_published_verified_export(
        paths,
        state,
        library_dir,
        authority,
        failpoint,
        journal_path,
        &marker,
        &mut transaction,
        VerifiedStageState::Missing,
        backup_present,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_published_verified_export(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    library_dir: &Path,
    authority: &VerifiedDeliveryAuthority,
    failpoint: VerifiedExportFailpoint,
    journal_path: &Path,
    marker: &VerifiedExportMarker,
    transaction: &mut VerifiedExportTransaction,
    stage_state: VerifiedStageState,
    backup_present: bool,
) -> Result<MaterializedRun> {
    validate_verified_export_parent(transaction)?;
    require_verified_export_identity(&transaction.destination, marker, authority)?;
    authority.revalidate(paths, state)?;
    validate_verified_export_manifest(library_dir, transaction)?;
    append_materialized_marker_idempotent(library_dir, &transaction.destination)?;
    let revision = delivered_git_revision(&transaction.destination);
    super::job::record_job_delivery(
        paths,
        &authority.job_id,
        super::job::JobDeliveryKind::Exported,
        &transaction.destination,
        revision.as_deref(),
    )?;
    verified_export_fail(failpoint, VerifiedExportFailpoint::AfterEvent)?;
    transaction.phase = VerifiedExportPhase::Recorded;
    write_verified_export_transaction(journal_path, transaction)?;

    if backup_present {
        require_verified_export_backup_identity(transaction, authority)?;
        remove_if_exists(&transaction.backup)?;
        sync_verified_export_parent(transaction)?;
    }
    if stage_state != VerifiedStageState::Missing {
        remove_owned_verified_export_path(&transaction.stage, marker, authority)?;
        sync_verified_export_parent(transaction)?;
    }
    transaction.phase = VerifiedExportPhase::Completed;
    write_verified_export_transaction(journal_path, transaction)?;
    Ok(MaterializedRun {
        run_id: state.run_id.clone(),
        source: library_dir.to_path_buf(),
        dest: transaction.destination.clone(),
    })
}

fn rollback_published_verified_export(
    transaction: &mut VerifiedExportTransaction,
    journal_path: &Path,
    marker: &VerifiedExportMarker,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    validate_verified_export_parent(transaction)?;
    require_verified_export_identity(&transaction.destination, marker, authority)?;
    remove_if_exists(&transaction.destination)?;
    sync_verified_export_parent(transaction)?;
    if transaction.previous_destination_sha256.is_some() {
        require_verified_export_backup_identity(transaction, authority)?;
        if path_lexically_present(&transaction.destination)? {
            return Err(verified_export_refusal(
                authority,
                "destination appeared while restoring the prior export target",
            ));
        }
        fs::rename(&transaction.backup, &transaction.destination)?;
        sync_verified_export_parent(transaction)?;
    }
    transaction.phase = VerifiedExportPhase::Prepared;
    write_verified_export_transaction(journal_path, transaction)
}

fn restore_verified_export_backup(
    transaction: &mut VerifiedExportTransaction,
    journal_path: &Path,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    validate_verified_export_parent(transaction)?;
    require_verified_export_backup_identity(transaction, authority)?;
    if path_lexically_present(&transaction.destination)? {
        return Err(verified_export_refusal(
            authority,
            "destination appeared while restoring the prior export target",
        ));
    }
    fs::rename(&transaction.backup, &transaction.destination)?;
    sync_verified_export_parent(transaction)?;
    transaction.phase = VerifiedExportPhase::Prepared;
    write_verified_export_transaction(journal_path, transaction)
}

fn create_verified_export_stage(stage: &Path, marker: &VerifiedExportMarker) -> Result<()> {
    if path_lexically_present(stage)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "refusing to replace unexpected export staging path {}",
            stage.display()
        ))));
    }
    fs::create_dir(stage)?;
    write_verified_export_marker(stage, marker)?;
    sync_verified_tree(stage)
}

fn write_verified_export_marker(root: &Path, marker: &VerifiedExportMarker) -> Result<()> {
    let path = root.join(".deadreckon").join("export.json");
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "export marker {} has no parent directory",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    fs::write(&path, serde_json::to_vec_pretty(marker)?)?;
    fs::File::open(&path)?.sync_all()?;
    sync_directory(parent)
}

fn verified_export_marker_matches(root: &Path, expected: &VerifiedExportMarker) -> Result<bool> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(CliError::Io(source)),
    };
    if !metadata.file_type().is_dir() {
        return Ok(false);
    }
    let path = root.join(".deadreckon").join("export.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(CliError::Io(source)),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(CliError::Io(source)),
    };
    Ok(
        serde_json::from_slice::<VerifiedExportMarker>(&raw)
            .is_ok_and(|marker| marker == *expected),
    )
}

fn verified_export_identity_matches(root: &Path, marker: &VerifiedExportMarker) -> Result<bool> {
    if !verified_export_marker_matches(root, marker)? {
        return Ok(false);
    }
    if marker.include_manifest {
        match marker.manifest_sha256.as_deref() {
            Some(expected)
                if regular_file_sha256_if_present(&root.join("manifest.json"))?.as_deref()
                    == Some(expected) => {}
            None if regular_file_sha256_if_present(&root.join("manifest.json"))?.is_none() => {}
            _ => return Ok(false),
        }
    } else if path_lexically_present(&root.join("manifest.json"))? {
        return Ok(false);
    }
    if path_lexically_present(&root.join(".materialized-to"))? {
        return Ok(false);
    }
    if !verified_export_has_only_expected_paths(root)? {
        return Ok(false);
    }
    let mut index = deadreckon_core::flight::build_deliverable_file_index(root)?;
    index.files.remove(Path::new("manifest.json"));
    index.files.remove(Path::new(".materialized-to"));
    Ok(index.tree_hash() == marker.result_tree_sha256)
}

fn verified_export_has_only_expected_paths(root: &Path) -> Result<bool> {
    fn inspect(root: &Path, directory: &Path) -> Result<bool> {
        let mut entries = fs::read_dir(directory)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(CliError::Io)?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "export path escaped its root: {error}"
                )))
            })?;
            let metadata = fs::symlink_metadata(&path)?;
            match deadreckon_core::classify_workspace_path(relative) {
                deadreckon_core::WorkspacePathClass::Deliverable => {}
                deadreckon_core::WorkspacePathClass::LifecycleMetadata
                    if relative == Path::new(".deadreckon")
                        || relative == Path::new(".deadreckon/export.json")
                        || relative == Path::new(".deadreckon/parent.json") => {}
                deadreckon_core::WorkspacePathClass::LifecycleMetadata
                | deadreckon_core::WorkspacePathClass::EvidenceOnly
                | deadreckon_core::WorkspacePathClass::RuntimeOnly => return Ok(false),
            }
            if metadata.file_type().is_dir() && !inspect(root, &path)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    inspect(root, root)
}

fn require_verified_export_identity(
    root: &Path,
    marker: &VerifiedExportMarker,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    if verified_export_identity_matches(root, marker)? {
        return Ok(());
    }
    Err(verified_export_refusal(
        authority,
        &format!(
            "export tree {} does not match the exact signed receipt",
            root.display()
        ),
    ))
}

fn validate_verified_export_manifest(
    library_dir: &Path,
    transaction: &VerifiedExportTransaction,
) -> Result<()> {
    let path = library_dir.join("manifest.json");
    let current = regular_file_sha256_if_present(&path)?;
    if transaction.include_manifest && current != transaction.manifest_sha256 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "the requested library manifest changed while verified export was running",
            &format!("deadreckon verdict {}", run_prefix(&transaction.job_id)),
        )));
    }
    Ok(())
}

fn inspect_verified_destination(
    dest: &Path,
    transaction: &VerifiedExportTransaction,
    marker: &VerifiedExportMarker,
    authority: &VerifiedDeliveryAuthority,
) -> Result<VerifiedDestinationState> {
    if !path_lexically_present(dest)? {
        return Ok(VerifiedDestinationState::Missing);
    }
    if verified_export_identity_matches(dest, marker)? {
        return Ok(VerifiedDestinationState::Exported);
    }
    if let Some(expected) = transaction.previous_destination_sha256.as_deref() {
        let actual = verified_path_identity(dest)?;
        if actual == expected {
            return Ok(VerifiedDestinationState::Previous);
        }
    }
    Err(verified_export_refusal(
        authority,
        &format!(
            "destination {} changed outside the receipt-bound export transaction",
            dest.display()
        ),
    ))
}

fn inspect_verified_stage(
    stage: &Path,
    marker: &VerifiedExportMarker,
    authority: &VerifiedDeliveryAuthority,
) -> Result<VerifiedStageState> {
    if !path_lexically_present(stage)? {
        return Ok(VerifiedStageState::Missing);
    }
    if verified_export_identity_matches(stage, marker)? {
        return Ok(VerifiedStageState::Complete);
    }
    if verified_export_marker_matches(stage, marker)? {
        return Ok(VerifiedStageState::PartialOwned);
    }
    Err(verified_export_refusal(
        authority,
        &format!(
            "staging path {} was substituted outside the trusted export transaction",
            stage.display()
        ),
    ))
}

fn inspect_verified_backup(
    transaction: &VerifiedExportTransaction,
    authority: &VerifiedDeliveryAuthority,
) -> Result<bool> {
    if !path_lexically_present(&transaction.backup)? {
        return Ok(false);
    }
    require_verified_export_backup_identity(transaction, authority)?;
    Ok(true)
}

fn require_previous_destination_identity(
    transaction: &VerifiedExportTransaction,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    let Some(expected) = transaction.previous_destination_sha256.as_deref() else {
        return Err(verified_export_refusal(
            authority,
            "export transaction has no prior destination to move",
        ));
    };
    if verified_path_identity(&transaction.destination)? != expected {
        return Err(verified_export_refusal(
            authority,
            "destination changed before its transactional backup was created",
        ));
    }
    Ok(())
}

fn require_verified_export_backup_identity(
    transaction: &VerifiedExportTransaction,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    let Some(expected) = transaction.previous_destination_sha256.as_deref() else {
        return Err(verified_export_refusal(
            authority,
            "an export backup exists for a transaction that had no prior destination",
        ));
    };
    if verified_path_identity(&transaction.backup)? != expected {
        return Err(verified_export_refusal(
            authority,
            &format!(
                "backup {} changed outside the trusted export transaction",
                transaction.backup.display()
            ),
        ));
    }
    Ok(())
}

fn remove_owned_verified_export_path(
    path: &Path,
    marker: &VerifiedExportMarker,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    if !verified_export_marker_matches(path, marker)? {
        return Err(verified_export_refusal(
            authority,
            &format!(
                "refusing to clean staging path {} without its exact transaction marker",
                path.display()
            ),
        ));
    }
    remove_if_exists(path)
}

fn validate_verified_export_transaction(
    transaction: &VerifiedExportTransaction,
    marker: &VerifiedExportMarker,
    parent_identity: &str,
    stage: &Path,
    backup: &Path,
    authority: &VerifiedDeliveryAuthority,
) -> Result<()> {
    let valid = transaction.schema_version == VERIFIED_EXPORT_SCHEMA_VERSION
        && transaction.transaction_id == marker.transaction_id
        && transaction.job_id == marker.job_id
        && transaction.run_id == marker.run_id
        && transaction.destination == marker.destination
        && transaction.destination_parent_identity == parent_identity
        && transaction.receipt_sha256 == marker.receipt_sha256
        && transaction.result_tree_sha256 == marker.result_tree_sha256
        && transaction.include_manifest == marker.include_manifest
        && transaction.stage == stage
        && transaction.backup == backup;
    if valid {
        Ok(())
    } else {
        Err(verified_export_refusal(
            authority,
            "trusted export transaction journal does not match this exact request",
        ))
    }
}

fn validate_verified_export_parent(transaction: &VerifiedExportTransaction) -> Result<()> {
    let parent = transaction.destination.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "verified export destination has no parent".to_string(),
        ))
    })?;
    if fs::canonicalize(parent)? != parent
        || verified_directory_identity(parent)? != transaction.destination_parent_identity
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "verified export destination parent changed identity",
            &format!("deadreckon verdict {}", run_prefix(&transaction.job_id)),
        )));
    }
    Ok(())
}

fn read_verified_export_transaction(
    path: &Path,
    authority: &VerifiedDeliveryAuthority,
) -> Result<VerifiedExportTransaction> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(verified_export_refusal(
            authority,
            "trusted export transaction journal is not a regular file",
        ));
    }
    let raw = fs::read(path)?;
    let after = fs::symlink_metadata(path)?;
    if !same_verified_file_identity(&before, &after) {
        return Err(verified_export_refusal(
            authority,
            "trusted export transaction journal changed while it was read",
        ));
    }
    Ok(serde_json::from_slice(&raw)?)
}

#[cfg(unix)]
fn same_verified_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_verified_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn regular_file_sha256_if_present(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(deadreckon_core::flight::sha256_file(path)?))
        }
        Ok(_) => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "verified export metadata is not a regular file: {}",
            path.display()
        )))),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn write_verified_export_transaction(
    path: &Path,
    transaction: &VerifiedExportTransaction,
) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "verified export journal has no parent".to_string(),
        ))
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, transaction)?;
    temp.write_all(b"\n")?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path)
        .map_err(|error| CliError::Io(error.error))?;
    sync_directory(parent)
}

fn sync_verified_export_parent(transaction: &VerifiedExportTransaction) -> Result<()> {
    let parent = transaction.destination.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "verified export destination has no parent".to_string(),
        ))
    })?;
    sync_directory(parent)
}

fn sync_verified_tree(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "verified export tree is not a directory: {}",
            root.display()
        ))));
    }
    let mut entries = fs::read_dir(root)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(CliError::Io)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            sync_verified_tree(&path)?;
        } else if metadata.file_type().is_file() {
            fs::File::open(&path)?.sync_all()?;
        }
    }
    sync_directory(root)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn path_lexically_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn verified_path_identity(path: &Path) -> Result<String> {
    fn update(hasher: &mut Sha256, root: &Path, path: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let relative = path.strip_prefix(root).unwrap_or(Path::new(""));
        let relative = relative.as_os_str().as_encoded_bytes();
        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            hasher.update(metadata.mode().to_le_bytes());
            hasher.update(metadata.dev().to_le_bytes());
            hasher.update(metadata.ino().to_le_bytes());
            hasher.update(metadata.uid().to_le_bytes());
            hasher.update(metadata.gid().to_le_bytes());
        }
        if metadata.file_type().is_dir() {
            hasher.update(b"directory\0");
            let mut entries = fs::read_dir(path)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(CliError::Io)?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                update(hasher, root, &entry.path())?;
            }
        } else if metadata.file_type().is_file() {
            hasher.update(b"file\0");
            hasher.update(deadreckon_core::flight::sha256_file(path)?.as_bytes());
        } else if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            let target = fs::read_link(path)?;
            let target = target.as_os_str().as_encoded_bytes();
            hasher.update((target.len() as u64).to_le_bytes());
            hasher.update(target);
        } else {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unsupported filesystem entry in export destination: {}",
                path.display()
            ))));
        }
        Ok(())
    }

    let mut hasher = Sha256::new();
    hasher.update(b"deadreckon-verified-export-path-v1\0");
    update(&mut hasher, path, path)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn verified_directory_identity(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "verified export parent is not a directory: {}",
            path.display()
        ))));
    }
    let canonical = fs::canonicalize(path)?;
    let mut hasher = Sha256::new();
    hasher.update(b"deadreckon-verified-export-parent-v1\0");
    let encoded = canonical.as_os_str().as_encoded_bytes();
    hasher.update((encoded.len() as u64).to_le_bytes());
    hasher.update(encoded);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn verified_export_fail(
    selected: VerifiedExportFailpoint,
    current: VerifiedExportFailpoint,
) -> Result<()> {
    if selected == current {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "injected verified export crash after {current:?}"
        ))));
    }
    Ok(())
}

fn verified_export_refusal(authority: &VerifiedDeliveryAuthority, detail: &str) -> CliError {
    CliError::Core(deadreckon_core::user_error(
        detail,
        &format!("deadreckon verdict {}", run_prefix(&authority.job_id)),
    ))
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
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "materialize",
        Some(&id),
        ExplanationPanel::new(
            "DeadReckon exported run output into the requested destination.",
            "Materialize completed because the run was already completed, the destination was safe to write, and the library artifact was copied.",
            vec![
                ("run".to_string(), id.clone()),
                (
                    "source".to_string(),
                    materialized.source.display().to_string(),
                ),
                ("dest".to_string(), materialized.dest.display().to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        vec![("Secondary", secondary.as_str())],
    )
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
        None,
        None,
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
        None,
        None,
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
    verified_job_state: Option<deadreckon_core::PipelineState>,
    verified_job_id: Option<&str>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = resolve_apply_state(&paths, &run_id, quiet, verified_job_state)?;
    ensure_completed_run(&state, "apply")?;
    let record = match lifecycle_codebase_record(&paths, &state) {
        Ok(record) if record.mode == CodebaseMode::Worktree => record,
        Ok(record) => match prepare_result_run_apply_state(&paths, &state, quiet)? {
            Some(prepared) => {
                state = prepared;
                lifecycle_codebase_record(&paths, &state)?
            }
            None => record,
        },
        Err(source) => match prepare_result_run_apply_state(&paths, &state, quiet)? {
            Some(prepared) => {
                state = prepared;
                lifecycle_codebase_record(&paths, &state)?
            }
            None => return Err(apply_missing_codebase_error(&paths, &state, &source)),
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
    refuse_non_deliverable_result_history(&state, &record, git_root, branch)?;
    let target =
        target_branch.unwrap_or(git_stdout(git_root, &["symbolic-ref", "--short", "HEAD"])?);
    let delivery_before = delivered_git_revision(git_root).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "target checkout has no Git revision before apply".to_string(),
        ))
    })?;
    let diff_stat = git_stdout(
        git_root,
        &["diff", "--stat", &format!("{target}..{branch}")],
    )
    .unwrap_or_default();
    if diff_stat.trim().is_empty() {
        if !quiet {
            print_already_applied(&state, branch, &target);
        }
        record_applied_job_delivery(
            &paths,
            verified_job_id,
            &state,
            &record,
            git_root,
            &delivery_before,
        )?;
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
            println!("{}", ui_muted("cancelled"));
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
                record_applied_job_delivery(
                    &paths,
                    verified_job_id,
                    &state,
                    &record,
                    git_root,
                    &delivery_before,
                )?;
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
    let verified_revision = match verify_applied_job_delivery(
        &paths,
        verified_job_id,
        &state,
        &record,
        git_root,
        &delivery_before,
    ) {
        Ok(revision) => revision,
        Err(error) => {
            return Err(rollback_refused_job_delivery(
                git_root,
                &state.run_id,
                &delivery_before,
                autostash.as_ref(),
                error,
            ));
        }
    };
    if let Some(stash) = autostash.as_ref() {
        restore_apply_autostash(git_root, &state.run_id, stash)?;
    }
    if let (Some(job_id), Some(revision)) = (verified_job_id, verified_revision.as_deref()) {
        super::job::record_job_delivery(
            &paths,
            job_id,
            super::job::JobDeliveryKind::Applied,
            git_root,
            Some(revision),
        )?;
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

fn record_applied_job_delivery(
    paths: &DeadreckonPaths,
    verified_job_id: Option<&str>,
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    destination: &Path,
    delivery_before: &str,
) -> Result<()> {
    let Some(job_id) = verified_job_id else {
        return Ok(());
    };
    let Some(revision) = verify_applied_job_delivery(
        paths,
        Some(job_id),
        state,
        record,
        destination,
        delivery_before,
    )?
    else {
        return Ok(());
    };
    super::job::record_job_delivery(
        paths,
        job_id,
        super::job::JobDeliveryKind::Applied,
        destination,
        Some(&revision),
    )
}

fn verify_applied_job_delivery(
    paths: &DeadreckonPaths,
    verified_job_id: Option<&str>,
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    destination: &Path,
    delivery_before: &str,
) -> Result<Option<String>> {
    let Some(job_id) = verified_job_id else {
        return Ok(None);
    };
    let receipt = deadreckon_core::validate_completion_receipt(paths, state)?;
    verify_applied_receipt_identity(job_id, &receipt, record, destination, delivery_before)?;
    let revision = delivered_git_revision(destination).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("applied job {job_id}, but the delivered Git revision is unavailable"),
            &format!("deadreckon show {}", run_prefix(job_id)),
        ))
    })?;
    Ok(Some(revision))
}

fn rollback_refused_job_delivery(
    git_root: &Path,
    run_id: &str,
    delivery_before: &str,
    autostash: Option<&ApplyAutoStash>,
    validation_error: CliError,
) -> CliError {
    if let Err(rollback_error) = git_status(git_root, &["reset", "--hard", delivery_before]) {
        return CliError::Core(deadreckon_core::user_error(
            &format!(
                "verified delivery was refused ({validation_error}), and restoring the target revision also failed: {rollback_error}"
            ),
            "inspect `git status`, preserve any autostash, and restore the target branch manually before retrying finish",
        ));
    }
    if let Some(stash) = autostash
        && let Err(stash_error) = restore_apply_autostash(git_root, run_id, stash)
    {
        return CliError::Core(deadreckon_core::user_error(
            &format!(
                "verified delivery was refused and the target revision was restored, but restoring {} failed: {stash_error}",
                stash.refname
            ),
            &format!(
                "inspect `git status`, then recover the operator changes with `git stash pop {}`",
                stash.refname
            ),
        ));
    }
    validation_error
}

fn refuse_non_deliverable_result_history(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    git_root: &Path,
    branch: &str,
) -> Result<()> {
    let base = record.base_sha.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing base_sha for result artifact boundary".to_string(),
        ))
    })?;
    let paths =
        deadreckon_core::completion::non_deliverable_git_history_paths(git_root, base, branch)?;
    if paths.is_empty() {
        return Ok(());
    }
    let listed = paths
        .iter()
        .take(8)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(CliError::Core(deadreckon_core::user_error(
        &format!(
            "run {} result history contains non-deliverable paths: {listed}",
            run_prefix(&state.run_id)
        ),
        &format!(
            "rebuild {branch} without provider-private, lifecycle, or runtime artifacts and re-verify the result"
        ),
    )))
}

fn verify_applied_receipt_identity(
    job_id: &str,
    receipt: &deadreckon_protocol::CompletionReceipt,
    record: &CodebaseRecord,
    destination: &Path,
    delivery_before: &str,
) -> Result<()> {
    let base = receipt.source_revision.as_deref().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("job {job_id} receipt has no signed source revision"),
            &format!("deadreckon verdict {}", run_prefix(job_id)),
        ))
    })?;
    if record.base_sha.as_deref() != Some(base) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("job {job_id} apply base does not match its signed receipt"),
            &format!("deadreckon verdict {}", run_prefix(job_id)),
        )));
    }
    let result = receipt.result_revision.as_deref().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("job {job_id} receipt has no signed result revision"),
            &format!("deadreckon verdict {}", run_prefix(job_id)),
        ))
    })?;
    let delivered = delivered_git_revision(destination).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("applied job {job_id}, but the delivered Git revision is unavailable"),
            &format!("deadreckon show {}", run_prefix(job_id)),
        ))
    })?;
    let changed =
        deadreckon_core::completion::deliverable_git_delta_paths(destination, base, result)?;
    let signed_paths = changed
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for path in changed {
        let signed = git_tree_entry(destination, result, &path)?;
        let applied = git_tree_entry(destination, &delivered, &path)?;
        if signed != applied {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "applied job {job_id}, but delivered path {} does not match the signed result",
                    path.display()
                ),
                &format!("deadreckon verdict {}", run_prefix(job_id)),
            )));
        }
    }
    let delivered_paths = deadreckon_core::completion::git_delivery_history_paths(
        destination,
        delivery_before,
        &delivered,
    )?;
    let unexpected = delivered_paths
        .into_iter()
        .filter(|path| !signed_paths.contains(path))
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        let listed = unexpected
            .iter()
            .take(8)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "applied job {job_id}, but delivery committed paths outside the signed result: {listed}"
            ),
            &format!("deadreckon verdict {}", run_prefix(job_id)),
        )));
    }
    Ok(())
}

fn git_tree_entry(git_root: &Path, revision: &str, path: &Path) -> Result<Vec<u8>> {
    let output = deadreckon_core::git::run_git(
        git_root,
        &["ls-tree", "-r", "-z", "--full-tree", revision, "--"],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            if stderr.is_empty() {
                format!(
                    "could not inspect {} at Git revision {revision}",
                    path.display()
                )
            } else {
                stderr
            },
        )));
    }
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if git_path_matches(path, &entry[tab + 1..])? {
            return Ok(entry[..tab].to_vec());
        }
    }
    Ok(Vec::new())
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn git_path_matches(path: &Path, raw: &[u8]) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;

    Ok(path.as_os_str().as_bytes() == raw)
}

#[cfg(not(unix))]
fn git_path_matches(path: &Path, raw: &[u8]) -> Result<bool> {
    let raw = std::str::from_utf8(raw).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "Git returned a result path that cannot be represented on this platform".to_string(),
        ))
    })?;
    Ok(path == Path::new(raw))
}

fn resolve_apply_state(
    paths: &DeadreckonPaths,
    run_id: &str,
    quiet: bool,
    verified_job_state: Option<deadreckon_core::PipelineState>,
) -> Result<deadreckon_core::PipelineState> {
    let public_mutation = verified_job_state.is_none();
    let state = match verified_job_state {
        Some(state) => state,
        None => match super::reference::try_resolve_run(paths, run_id, "apply")? {
            Some(state) => state,
            None => match resolve_plan_result_run(paths, run_id, "apply")? {
                Some(result) => {
                    super::graph_job::require_current_driver_for_job_artifact(
                        paths,
                        &result.plan.plan_id,
                        deadreckon_protocol::JobShape::Graph,
                        "apply",
                    )?;
                    if !quiet {
                        print_plan_result_context(&result.plan, &result.state);
                    }
                    prepare_plan_result_apply_state(paths, &result.plan, &result.state)?
                }
                None => {
                    return Err(super::reference::refusal_for_reference(
                        paths, run_id, "apply",
                    ));
                }
            },
        },
    };
    if public_mutation {
        super::graph_job::require_current_driver_for_job_owned_run(paths, &state, "apply")?;
    }
    Ok(state)
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
    match fs::read(path) {
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
    source: &DeadreckonError,
) -> CliError {
    let id = run_prefix(&state.run_id);
    let library = paths.library_dir(&state.scope, &state.run_id);
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
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
    println!("  {}    {branch}", ui_muted("run branch:"));
    println!("  {} {target}", ui_muted("target branch:"));
    println!(
        "  {} no file changes remain between the run branch and target branch",
        ui_muted("reason:")
    );
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
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "apply",
        Some(&id),
        ExplanationPanel::new(
            "DeadReckon applied or confirmed the run branch in the target checkout.",
            why,
            evidence,
        ),
        vec![("Recommended", primary)],
        vec![("Secondary", secondary.as_str())],
    )
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
    eprintln!(
        "stashed local changes as {}",
        ui::render(ui::Stream::Stderr, ui::Tone::Id, &refname)
    );
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
        surface: VerdictSurface::must_new(
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
        surface: VerdictSurface::must_new(
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
    let mut state = super::reference::resolve_run_like(&paths, Some(&run_id), "abandon")?;
    super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "abandon")?;
    let record = match lifecycle_codebase_record(&paths, &state) {
        Ok(record) => record,
        Err(error) if paths.job_json(&state.run_id).is_file() => {
            return Err(CliError::Core(error));
        }
        Err(_) => {
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
        }
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
        let mut state = resolve_cleanup_state(&paths, &run_id)?;
        super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "cleanup")?;
        if state.status == RunStatus::Executing {
            if !escalate {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("run {} is {}", state.run_id, run_status_label(state.status)),
                    &format!("deadreckon cleanup {} --escalate", state.run_id),
                )));
            }
            let _ = kill_loaded_run(&paths, &mut state, escalate);
        }
        let record = lifecycle_codebase_record(&paths, &state)?;
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

    println!("{}", ui_heading("cleanup candidates:"));
    for candidate in &candidates {
        println!(
            "  {} {} {} {}",
            ui::pad_visible(&ui_id(run_prefix(&candidate.state.run_id)), 8),
            ui::pad_visible(&ui_status(candidate.state.status.to_string()), 10),
            ui::pad_visible(&ui_status(&candidate.reason), 16),
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
            println!("{}", ui_muted("cancelled"));
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

fn resolve_cleanup_state(
    paths: &DeadreckonPaths,
    reference: &str,
) -> Result<deadreckon_core::PipelineState> {
    match super::reference::resolve_ref(
        paths,
        super::reference::RefQuery {
            reference: Some(reference),
            all_scopes: false,
            verb: "cleanup",
        },
    )? {
        super::reference::ResolvedRef::Job(job) => load_run(paths, job.job.job_id.as_ref())
            .map_err(|source| {
                CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "job {} has no recoverable same-ID run workspace: {source}",
                        job.job.job_id
                    ),
                    &format!("deadreckon show {}", run_prefix(job.job.job_id.as_ref())),
                ))
            }),
        super::reference::ResolvedRef::Run(state)
        | super::reference::ResolvedRef::PlanChild { state, .. } => Ok(*state),
        other => Err(super::reference::refusal_for(
            other.kind(),
            "cleanup",
            &super::reference::resolved_id(&other),
        )),
    }
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
        if super::graph_job::resolve_run_owner(paths, &state)?.is_some() {
            continue;
        }
        let record = match lifecycle_codebase_record(paths, &state) {
            Ok(record) => record,
            Err(error) if paths.job_json(&state.run_id).is_file() => {
                return Err(CliError::Core(error));
            }
            Err(_) => continue,
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
        let was_registered = git_worktree_registered(git_root, worktree)?;
        if worktree.exists() || was_registered {
            if !force
                && worktree.exists()
                && let Some(branch) = record.branch_name.as_deref()
                && local_branch_exists(git_root, branch)?
                && let Err(error) =
                    remove_untracked_runtime_roots(git_root, worktree, branch, &mut removed)
            {
                return Err(cleanup_incomplete_error(
                    state,
                    record,
                    reason,
                    force,
                    &removed,
                    &error.to_string(),
                ));
            }
            let result = if worktree.exists() {
                let mut args = vec!["worktree", "remove"];
                if force {
                    args.push("--force");
                }
                args.push(path_to_str(worktree)?);
                git_status(git_root, &args)
            } else {
                git_status(git_root, &["worktree", "prune", "--expire", "now"])
            };
            if let Err(error) = result {
                return Err(cleanup_incomplete_error(
                    state,
                    record,
                    reason,
                    force,
                    &removed,
                    &error.to_string(),
                ));
            }
            if worktree.exists() || git_worktree_registered(git_root, worktree)? {
                return Err(cleanup_incomplete_error(
                    state,
                    record,
                    reason,
                    force,
                    &removed,
                    "Git returned success but the worktree remains registered or present",
                ));
            }
            removed.push(worktree.display().to_string());
        }
        if !keep_branch
            && let Some(branch) = record.branch_name.as_deref()
            && local_branch_exists(git_root, branch)?
        {
            if let Err(error) = git_status(git_root, &["branch", "-D", branch]) {
                return Err(cleanup_incomplete_error(
                    state,
                    record,
                    reason,
                    force,
                    &removed,
                    &error.to_string(),
                ));
            }
            if local_branch_exists(git_root, branch)? {
                return Err(cleanup_incomplete_error(
                    state,
                    record,
                    reason,
                    force,
                    &removed,
                    "Git returned success but the temporary branch remains",
                ));
            }
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

fn remove_untracked_runtime_roots(
    git_root: &Path,
    worktree: &Path,
    revision: &str,
    removed: &mut Vec<String>,
) -> Result<()> {
    let tracked = git_tree_paths(git_root, revision)?;
    let runtime_roots = workspace_runtime_roots(worktree)?;
    for relative in runtime_roots {
        if tracked
            .iter()
            .any(|path| path == &relative || path.starts_with(&relative))
        {
            continue;
        }
        let absolute = worktree.join(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                fs::remove_dir_all(&absolute)?;
            }
            Ok(_) => fs::remove_file(&absolute)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(CliError::Io(error)),
        }
        removed.push(absolute.display().to_string());
    }
    Ok(())
}

fn workspace_runtime_roots(worktree: &Path) -> Result<Vec<PathBuf>> {
    fn visit(worktree: &Path, directory: &Path, roots: &mut BTreeSet<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(worktree).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "runtime cleanup path {} escaped owned worktree {}",
                    path.display(),
                    worktree.display()
                )))
            })?;
            if entry.file_name() == ".git" {
                continue;
            }
            if let Some(root) = deadreckon_core::runtime_output_root(relative) {
                roots.insert(root);
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                visit(worktree, &path, roots)?;
            }
        }
        Ok(())
    }

    let mut roots = BTreeSet::new();
    visit(worktree, worktree, &mut roots)?;
    Ok(roots.into_iter().collect())
}

fn git_tree_paths(git_root: &Path, revision: &str) -> Result<Vec<PathBuf>> {
    let output = deadreckon_core::git::run_git(
        git_root,
        &["ls-tree", "-r", "-z", "--full-tree", revision, "--"],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            if stderr.is_empty() {
                format!(
                    "could not inspect tracked paths at Git revision {revision} in {}",
                    git_root.display()
                )
            } else {
                stderr
            },
        )));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let tab = entry
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "Git returned a malformed tree entry during runtime cleanup".to_string(),
                    ))
                })?;
            git_path_from_bytes(&entry[tab + 1..])
        })
        .collect()
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn git_path_from_bytes(raw: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(std::ffi::OsString::from_vec(raw.to_vec())))
}

#[cfg(not(unix))]
fn git_path_from_bytes(raw: &[u8]) -> Result<PathBuf> {
    String::from_utf8(raw.to_vec())
        .map(PathBuf::from)
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "Git returned a tracked path that cannot be represented on this platform"
                    .to_string(),
            ))
        })
}

fn git_worktree_registered(git_root: &Path, worktree: &Path) -> Result<bool> {
    let listing = git_stdout(git_root, &["worktree", "list", "--porcelain"])?;
    Ok(listing.lines().any(|line| {
        line.strip_prefix("worktree ")
            .is_some_and(|path| Path::new(path) == worktree)
    }))
}

fn local_branch_exists(git_root: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let output =
        deadreckon_core::git::run_git(git_root, &["show-ref", "--verify", "--quiet", &reference])?;
    Ok(output.status.success())
}

fn cleanup_incomplete_error(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    reason: CleanupReason,
    force: bool,
    removed: &[String],
    detail: &str,
) -> CliError {
    let id = run_prefix(&state.run_id);
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        ("cleanup".to_string(), "incomplete".to_string()),
        ("failure".to_string(), one_line(detail, 240)),
        ("removed entries".to_string(), removed.len().to_string()),
    ];
    for (index, item) in removed.iter().enumerate() {
        evidence.push((format!("removed {}", index + 1), item.clone()));
    }
    if let Some(worktree) = record.worktree_path.as_ref()
        && worktree.exists()
    {
        evidence.push((
            "retained worktree".to_string(),
            worktree.display().to_string(),
        ));
    }
    if let Some(branch) = record.branch_name.as_ref() {
        evidence.push(("retained branch".to_string(), branch.clone()));
    }
    let what = match reason {
        CleanupReason::Applied => "DeadReckon applied the run, but cleanup did not complete.",
        CleanupReason::Abandoned => {
            "DeadReckon could not finish removing the abandoned run resources."
        }
        CleanupReason::Cleaned => {
            "DeadReckon could not finish removing the selected run resources."
        }
    };
    let why = if force {
        "Git refused or failed a cleanup operation even with explicit overwrite authority. DeadReckon retained the remaining resources and did not write a completed-cleanup marker."
    } else {
        "Git refused or failed a cleanup operation. DeadReckon retained the remaining resources, did not write a completed-cleanup marker, and requires explicit overwrite authority before discarding untracked evidence."
    };
    let recommended = if force {
        format!("deadreckon show {id}")
    } else {
        format!("deadreckon cleanup {id} --overwrite")
    };
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            reason.subject(),
            Some(&id),
            ExplanationPanel::new(what, why, evidence),
            vec![("Recommended", recommended)],
            vec![("Secondary", format!("deadreckon show {id}"))],
        )
        .render_plain(!completion_hints_enabled(false)),
    }
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
        VerdictSurface::must_new(
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
    VerdictSurface::must_new(
        VerdictKind::Completed,
        result.reason.subject(),
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
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
        .into_iter()
        .map(|command| ("Secondary", command))
        .collect::<Vec<_>>();
    VerdictSurface::must_new(
        VerdictKind::Noop,
        subject_kind,
        subject,
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary)],
        secondary,
    )
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
        surface: VerdictSurface::must_new(
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
            vec![("Recommended", primary)],
            Vec::<(&str, &str)>::new(),
        )
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
    extend_command_with_launch_plan(args, None, false)
        .await
        .map(|_| ())
}

pub(crate) async fn extend_command_with_launch_plan(
    args: ExtendCommandArgs,
    launch_plan: Option<commands::course::LaunchPlan>,
    quiet: bool,
) -> Result<Option<deadreckon_protocol::JobId>> {
    let ExtendCommandArgs {
        parent_run_id,
        new_goal,
        dest,
        acceptance,
        yes,
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
        narrate,
        no_narrate,
        narrator_model,
    } = args;
    if let Err(message) = crate::narrator::validate_narration_flags(narrate, no_narrate) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
    }
    let new_goal = new_goal.trim().to_string();
    if new_goal.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--goal must be non-empty".to_string(),
        )));
    }

    let paths = DeadreckonPaths::discover();
    let parent = super::reference::resolve_run_like(&paths, Some(&parent_run_id), "extend")?;
    let private_characterization = commands::plan::internal_characterization_requested();
    let parent_receipt_sha256 = if private_characterization {
        super::graph_job::require_current_driver_for_job_owned_run(&paths, &parent, "extend")?;
        None
    } else {
        match materialize_delivery_authority(&paths, &parent)? {
            MaterializeDeliveryAuthority::LegacyUnowned => None,
            MaterializeDeliveryAuthority::Verified(authority) => {
                Some(authority.receipt_sha256.clone())
            }
        }
    };
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
    let context_turns = context_turns(max_context_turns, no_context);
    if !private_characterization {
        return schedule_durable_extension(DurableExtensionRequest {
            parent,
            parent_library,
            new_goal,
            dest,
            acceptance,
            yes,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            no_docs,
            doc_skill,
            narrate,
            no_narrate,
            narrator_model,
            context_turns,
            parent_receipt_sha256,
            launch_plan,
            quiet,
        })
        .await;
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
        .or(Some(36_000.0));
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
            narrate,
            no_narrate,
            narrator_model: narrator_model.clone(),
        })
        .await
        .map(|_| None);
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
    // Live narrator: a spawned reviewer child (NARRATE_CHILD_ENV) narrates
    // file-only; an interactive `dr extend` uses the TTY default. Floor unless a
    // model is pinned; smoke forces the floor.
    let is_narrate_child = std::env::var_os(crate::narrator::NARRATE_CHILD_ENV).is_some();
    let narrator_config = crate::narrator::resolve_narration(
        is_narrate_child,
        io::stdin().is_terminal(),
        narrate,
        no_narrate,
        narrator_model,
    );
    let extend_force_floor = effective_provider.as_deref() == Some("smoke")
        || (is_narrate_child
            && crate::narrator::child_narrator_backend_is_floor(
                narrator_config
                    .as_ref()
                    .and_then(|config| config.model_override.as_deref()),
            ));
    let (narrate_event_sender, narrator_handle) = crate::narrator::build_run_narration(
        paths.home(),
        Some(paths.config_path()),
        &state.run_id,
        &state.run_root,
        extend_force_floor,
        narrator_config.clone(),
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
                event_sender: narrate_event_sender,
                cancellation_token: None,
                narrate: narrator_config,
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
    if let Some(handle) = narrator_handle {
        handle.shutdown().await;
    }
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
    Ok(None)
}

struct DurableExtensionRequest {
    parent: deadreckon_core::PipelineState,
    parent_library: PathBuf,
    new_goal: String,
    dest: Option<PathBuf>,
    acceptance: Option<PathBuf>,
    yes: bool,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: Option<String>,
    no_docs: bool,
    doc_skill: Option<String>,
    narrate: bool,
    no_narrate: bool,
    narrator_model: Option<String>,
    context_turns: Option<u32>,
    parent_receipt_sha256: Option<String>,
    launch_plan: Option<commands::course::LaunchPlan>,
    quiet: bool,
}

async fn schedule_durable_extension(
    request: DurableExtensionRequest,
) -> Result<Option<deadreckon_protocol::JobId>> {
    let DurableExtensionRequest {
        parent,
        parent_library,
        new_goal,
        dest,
        acceptance,
        yes,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        no_docs,
        doc_skill,
        narrate,
        no_narrate,
        narrator_model,
        context_turns,
        parent_receipt_sha256,
        launch_plan,
        quiet,
    } = request;
    if dest.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable extensions choose an isolated Job workspace; --dest is no longer a launch-time mutation",
            "queue the extension without --dest, then use `deadreckon finish <job-id> --export <path>` after verification",
        )));
    }

    let operator_cwd = std::env::current_dir()?;
    let source_cwd = if parent.cwd.is_dir() {
        parent.cwd.clone()
    } else {
        operator_cwd.clone()
    };
    let acceptance = acceptance
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                operator_cwd.join(path)
            }
        })
        .or_else(|| {
            let frozen = parent.run_root.join("acceptance.yaml");
            frozen.is_file().then_some(frozen)
        });
    let continuation = commands::run::DurableContinuationSpec {
        parent_run_id: parent.run_id.clone(),
        parent_scope: parent.scope.clone(),
        parent_state_sha256: deadreckon_core::flight::sha256_file(&parent.state_path())?,
        parent_library_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(
            &parent_library,
        )?
        .tree_hash(),
        parent_receipt_sha256,
        context_turns,
    };

    let run_args = RunCommandArgs {
        goal: new_goal,
        run_id: None,
        durable_source_cwd: Some(source_cwd),
        continuation: Some(continuation),
        tamper_baseline: None,
        fresh: false,
        worktree: false,
        from: Some(parent_library),
        in_place: false,
        base: None,
        branch: None,
        allow_dirty: false,
        init_git: false,
        yes,
        preview: false,
        brief: false,
        no_seams: false,
        plain: false,
        prevent_sleep: Some("off".to_string()),
        quiet,
        max_spend,
        max_wall_seconds,
        sandbox,
        untrusted: false,
        provider,
        model,
        doc_provider: None,
        acceptance,
        skill: parent.skill_name,
        smoke: false,
        i_know_its_a_lot: false,
        no_confirm: false,
        no_hints: false,
        no_docs,
        doc_skill,
        narrate,
        no_narrate,
        narrator_model,
        infer_contract: false,
    };
    if let Some(launch_plan) = launch_plan {
        commands::run::schedule_durable_run_with_launch_plan(run_args, launch_plan).await
    } else {
        commands::run::run_command_with_job_id(run_args).await
    }
}

pub(crate) fn prepare_durable_continuation(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    continuation: &commands::run::DurableContinuationSpec,
) -> Result<()> {
    let parent = deadreckon_core::load_run(paths, &continuation.parent_run_id)?;
    if parent.run_id != continuation.parent_run_id
        || parent.scope != continuation.parent_scope
        || parent.status != RunStatus::Completed
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "durable continuation parent {} no longer matches its frozen completed identity",
            continuation.parent_run_id
        ))));
    }
    let state_sha256 = deadreckon_core::flight::sha256_file(&parent.state_path())?;
    if state_sha256 != continuation.parent_state_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "durable continuation parent {} changed after approval",
            continuation.parent_run_id
        ))));
    }
    let parent_library = paths.library_dir(&parent.scope, &parent.run_id);
    let library_tree_sha256 =
        deadreckon_core::flight::build_deliverable_file_index(&parent_library)?.tree_hash();
    if library_tree_sha256 != continuation.parent_library_tree_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "durable continuation library for {} changed after approval",
            continuation.parent_run_id
        ))));
    }
    match (
        continuation.parent_receipt_sha256.as_deref(),
        materialize_delivery_authority(paths, &parent)?,
    ) {
        (Some(expected), MaterializeDeliveryAuthority::Verified(authority)) => {
            if authority.receipt_sha256 != expected {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "durable continuation receipt for {} changed after approval",
                    continuation.parent_run_id
                ))));
            }
        }
        (None, MaterializeDeliveryAuthority::LegacyUnowned) => {}
        _ => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "durable continuation ownership for {} changed after approval",
                continuation.parent_run_id
            ))));
        }
    }

    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        &extended_parent_marker(&parent, &state.goal, continuation.context_turns),
    )?;
    write_parent_history(state, &parent, continuation.context_turns)?;
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "durable_continuation_bound".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id,
                "parent_scope": parent.scope,
                "parent_goal": parent.goal,
                "parent_completed_at": parent.updated_at,
                "parent_state_sha256": continuation.parent_state_sha256,
                "parent_library_tree_sha256": continuation.parent_library_tree_sha256,
                "parent_receipt_sha256": continuation.parent_receipt_sha256,
                "context_turns_included": continuation.context_turns,
            }),
        },
    )?;
    Ok(())
}

fn incomplete_parent_extend_surface(parent: &deadreckon_core::PipelineState) -> VerdictSurface {
    let id = run_prefix(&parent.run_id);
    let primary = format!("deadreckon resume {id}");
    let secondary = format!("deadreckon show {id}");
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "extend",
        Some(&id),
        ExplanationPanel::new(
            format!(
                "Parent run {id} is {} and cannot be extended yet.",
                parent.status
            ),
            "Extend requires a completed parent with promoted artifacts; an incomplete run may still change if it is resumed.",
            vec![
                ("run".to_string(), id.clone()),
                ("status".to_string(), parent.status.to_string()),
                (
                    "state".to_string(),
                    parent.state_path().display().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
}

fn in_place_parent_extend_surface(
    parent: &deadreckon_core::PipelineState,
    new_goal: &str,
) -> VerdictSurface {
    let id = run_prefix(&parent.run_id);
    let primary = format!("deadreckon run --in-place --i-know-its-a-lot --untrusted {new_goal:?}");
    let secondary = format!("deadreckon show {id}");
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "extend",
        Some("in-place"),
        ExplanationPanel::new(
            format!("Parent run {id} is in-place and cannot be extended."),
            "Extend creates a follow-up from promoted copy or worktree artifacts; in-place work should continue as a new in-place run from the current checkout.",
            vec![
                ("run".to_string(), id),
                ("mode".to_string(), "in-place".to_string()),
                ("new goal".to_string(), new_goal.to_string()),
                (
                    "state".to_string(),
                    parent.state_path().display().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
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
    narrate: bool,
    no_narrate: bool,
    narrator_model: Option<String>,
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
        narrate,
        no_narrate,
        narrator_model,
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
    let is_narrate_child = std::env::var_os(crate::narrator::NARRATE_CHILD_ENV).is_some();
    let narrator_config = crate::narrator::resolve_narration(
        is_narrate_child,
        io::stdin().is_terminal(),
        narrate,
        no_narrate,
        narrator_model,
    );
    let extend_force_floor = effective_provider.as_deref() == Some("smoke")
        || (is_narrate_child
            && crate::narrator::child_narrator_backend_is_floor(
                narrator_config
                    .as_ref()
                    .and_then(|config| config.model_override.as_deref()),
            ));
    let (narrate_event_sender, narrator_handle) = crate::narrator::build_run_narration(
        paths.home(),
        Some(paths.config_path()),
        &state.run_id,
        &state.run_root,
        extend_force_floor,
        narrator_config.clone(),
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
                event_sender: narrate_event_sender,
                cancellation_token: None,
                narrate: narrator_config,
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
    if let Some(handle) = narrator_handle {
        handle.shutdown().await;
    }
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
    copy_deliverable_tree(library_dir, working_dir)?;
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
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        verb,
        Some(&id),
        ExplanationPanel::new(
            format!(
                "{verb} requires a completed run, but run {id} is {}.",
                state.status
            ),
            "This command needs stable completed artifacts; an incomplete run may still change if it is resumed.",
            vec![
                ("run".to_string(), id.clone()),
                ("status".to_string(), state.status.to_string()),
                (
                    "state".to_string(),
                    state.state_path().display().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
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

fn append_materialized_marker_idempotent(library_dir: &Path, dest: &Path) -> Result<()> {
    let path = library_dir.join(".materialized-to");
    let destination = dest.display().to_string();
    let already_recorded = match fs::read_to_string(&path) {
        Ok(raw) => raw.lines().any(|line| {
            line.split_once('\t')
                .is_some_and(|(_, recorded)| recorded == destination)
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => false,
        Err(source) => return Err(CliError::Io(source)),
    };
    if already_recorded {
        return Ok(());
    }
    append_materialized_marker(library_dir, dest)?;
    fs::File::open(&path)?.sync_all()?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
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

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use deadreckon_core::{JobProjection, JobView};
    use deadreckon_protocol::{
        AuthorityAcceptedBy, CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer,
        GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobEvent, JobEventKind,
        JobEventSequence, JobId, JobOutcome, JobPhase, JobPolicy, JobSchemaVersion, JobShape,
        RunId, SandboxBoundaryObservation, SandboxBoundaryObservationIssuer, SemanticDecision,
        SemanticJudgeMode, SemanticJudgment, StopReason,
    };
    use tempfile::TempDir;

    use super::*;

    fn job_view(
        root: &Path,
        outcome: Option<JobOutcome>,
        stop_reason: Option<StopReason>,
    ) -> JobView {
        let job_id = JobId("abababababababababababababababab".to_string());
        JobView {
            job: Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job_id.clone(),
                scope: "test".to_string(),
                goal: "finish safely".to_string(),
                shape: JobShape::Single,
                created_at: Utc::now(),
                source_cwd: root.to_path_buf(),
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
            projection: JobProjection {
                schema_version: JobSchemaVersion::CURRENT,
                job_id,
                phase: if outcome.is_some() {
                    JobPhase::Terminal
                } else {
                    JobPhase::Waiting
                },
                outcome,
                stop_reason,
                last_sequence: 1,
                current_lease_epoch: 1,
                attempt_count: 1,
                child_run_ids: Vec::new(),
                delivery: None,
                updated_at: Some(Utc::now()),
                caveats: Vec::new(),
            },
            attempts: Vec::new(),
            missing_attempts: Vec::new(),
        }
    }

    fn uncontained_receipt(job_id: &JobId) -> CompletionReceipt {
        CompletionReceipt {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId(job_id.as_ref().to_string()),
            issued_at: Utc::now(),
            issuer: CompletionReceiptIssuer::DeadreckonSupervisor,
            proof_kind: CompletionProofKind::TwoKeyCompletion,
            outcome: JobOutcome::Verified,
            stop_reason: StopReason::Verified,
            authority_sha256: "authority".to_string(),
            goal_sha256: "goal".to_string(),
            contract_sha256: "contract".to_string(),
            effective_policy_sha256: "policy".to_string(),
            launch_plan_sha256: "launch".to_string(),
            source_tree_sha256: "source".to_string(),
            source_revision: None,
            result_tree_sha256: "result".to_string(),
            result_revision: None,
            deterministic_marker_sha256: "marker".to_string(),
            semantic_judgment_sha256: "semantic".to_string(),
            sandbox_boundary_observation_sha256: "sandbox-observation".to_string(),
            contained: false,
            sandbox_backend: "none".to_string(),
            signature: "not-trusted".to_string(),
        }
    }

    fn git_repo(root: &Path) -> (String, String) {
        fs::create_dir_all(root).expect("repo");
        git_status(root, &["init"]).expect("git init");
        git_status(
            root,
            &["config", "user.email", "deadreckon@example.invalid"],
        )
        .expect("git email");
        git_status(root, &["config", "user.name", "DeadReckon Test"]).expect("git name");
        fs::write(root.join("signed.txt"), "base\n").expect("base file");
        git_status(root, &["add", "signed.txt"]).expect("add base");
        git_status(root, &["commit", "-m", "approved base"]).expect("commit base");
        let base = git_stdout(root, &["rev-parse", "HEAD"]).expect("base revision");
        let base_branch =
            git_stdout(root, &["symbolic-ref", "--short", "HEAD"]).expect("base branch");
        (base, base_branch)
    }

    fn worktree_record(root: &Path, base: &str, branch: &str) -> CodebaseRecord {
        CodebaseRecord {
            schema_version: deadreckon_core::codebase::CODEBASE_RECORD_VERSION,
            mode: CodebaseMode::Worktree,
            source_path: Some(root.to_path_buf()),
            source_git_root: Some(root.to_path_buf()),
            branch_name: Some(branch.to_string()),
            base_ref: Some(base.to_string()),
            base_sha: Some(base.to_string()),
            parent_branch: None,
            worktree_path: Some(root.to_path_buf()),
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        }
    }

    fn append_test_job_event(
        paths: &DeadreckonPaths,
        job_id: &str,
        sequence: u64,
        kind: JobEventKind,
    ) {
        deadreckon_core::append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                sequence: JobEventSequence::new(sequence).expect("nonzero sequence"),
                event_id: format!("test-{sequence}-{kind:?}"),
                causation_id: "materialize-authority-test".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind,
                detail: json!({}),
            },
        )
        .expect("append Job event");
    }

    fn signed_verified_export_fixture(
        temp: &TempDir,
    ) -> (DeadreckonPaths, deadreckon_core::PipelineState, String) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job_id = "efefefefefefefefefefefefefefefef".to_string();
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "export the exact verified Job result".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("judge".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(2.0),
                max_wall_seconds: Some(60.0),
                run_id: Some(job_id.clone()),
                codebase: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("result.txt"), "verified result\n").expect("result");
        let contract_path = deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root);
        fs::write(
            &contract_path,
            "name: result\nchecks:\n  - file_exists: result.txt\n",
        )
        .expect("contract");
        fs::create_dir_all(paths.job_dir(&job_id)).expect("job dir");
        let launch_path = paths.job_launch_plan(&job_id);
        fs::write(
            &launch_path,
            "{\"schema\":1,\"goal\":\"export the exact verified Job result\"}\n",
        )
        .expect("launch");
        let policy = JobPolicy {
            max_spend_usd: 2.0,
            max_wall_seconds: 60,
            max_attempts: 3,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                "sandbox-exec",
            )),
        };
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.clone()),
            run_id: RunId(job_id.clone()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: deadreckon_core::flight::sha256_text(&state.goal),
            contract_sha256: deadreckon_core::flight::sha256_file(&contract_path)
                .expect("contract digest"),
            effective_policy_sha256: deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&policy).expect("policy json"),
            ),
            launch_plan_sha256: deadreckon_core::flight::sha256_file(&launch_path)
                .expect("launch digest"),
            source_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(
                &state.working_dir,
            )
            .expect("source index")
            .tree_hash(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        fs::write(
            paths.job_authority(&job_id),
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.clone()),
            scope: state.scope.clone(),
            goal: state.goal.clone(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: state.cwd.clone(),
            launch_plan_sha256: authority.launch_plan_sha256.clone(),
            authority_sha256: deadreckon_core::flight::sha256_file(&paths.job_authority(&job_id))
                .expect("authority digest"),
            policy,
        };
        deadreckon_core::write_job(&paths, &job).expect("job");
        append_test_job_event(&paths, &job_id, 1, JobEventKind::Created);
        append_test_job_event(&paths, &job_id, 2, JobEventKind::Verified);
        let key = deadreckon_core::read_gate_key(&paths, &job_id).expect("gate key");
        let marker = deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            job_id.clone(),
            state.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("native marker");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.clone()),
            run_id: RunId(job_id.clone()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the signed result satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: state.goal.clone(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: deadreckon_core::flight::sha256_text("test evidence"),
            spend_usd: 0.0,
        };
        fs::create_dir_all(state.run_root.join("proofs")).expect("proof dir");
        fs::write(
            state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
            serde_json::to_vec_pretty(&judgment).expect("judgment json"),
        )
        .expect("judgment");
        let observation = SandboxBoundaryObservation {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: authority.job_id.clone(),
            run_id: authority.run_id.clone(),
            observed_at: Utc::now(),
            issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
            probe_id: Uuid::new_v4().to_string(),
            attempt: 1,
            outer_launch_id: Uuid::new_v4().to_string(),
            authority_sha256: deadreckon_core::flight::sha256_file(&paths.job_authority(&job_id))
                .expect("authority digest"),
            contract_sha256: authority.contract_sha256.clone(),
            result_tree_sha256: deadreckon_core::sandbox_boundary_result_tree_sha256(&state)
                .expect("result tree"),
            sandbox_requested: authority.sandbox_requested.clone(),
            sandbox_backend: "sandbox-exec".to_string(),
            gate_evaluator_sha256: authority.gate_evaluator_sha256.clone(),
            contained: true,
            gate_key_read_denied: true,
            proof_write_denied: true,
            control_write_denied: true,
            operator_capture_read_denied: true,
            operator_capture_write_denied: true,
            signing_env_scrubbed: true,
            probe_sha256: deadreckon_core::flight::sha256_text("fixed controller probe"),
            signature: String::new(),
        };
        deadreckon_core::seal_sandbox_boundary_observation(
            &paths,
            &state,
            &authority,
            &observation,
        )
        .expect("boundary observation");
        deadreckon_core::seal_completion_receipt(&paths, &state, &authority, &marker, &judgment)
            .expect("signed receipt");
        state.status = RunStatus::Completed;
        save_state(&state).expect("completed state");
        deadreckon_core::promote_completed_run(&paths, &mut state).expect("promotion");
        deadreckon_core::validate_completion_receipt(&paths, &state)
            .expect("receipt remains valid after promotion");
        (paths, state, job_id)
    }

    fn signed_export_authority(
        paths: &DeadreckonPaths,
        state: &deadreckon_core::PipelineState,
        job_id: &str,
    ) -> VerifiedDeliveryAuthority {
        let view = deadreckon_core::JobView::load(paths, job_id).expect("verified Job view");
        VerifiedDeliveryAuthority::from_finished_job(paths, &view, state)
            .expect("signed export authority")
    }

    fn only_export_transaction(paths: &DeadreckonPaths, job_id: &str) -> VerifiedExportTransaction {
        let directory = paths.job_dir(job_id).join("export-transactions");
        let entries = fs::read_dir(&directory)
            .expect("transaction directory")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("transaction entries");
        assert_eq!(entries.len(), 1, "one export transaction");
        serde_json::from_slice(
            &fs::read(entries[0].path()).expect("trusted export transaction journal"),
        )
        .expect("transaction JSON")
    }

    #[test]
    fn runtime_cleanup_removes_only_untracked_disposable_roots() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        git_repo(&repo);
        fs::create_dir_all(repo.join("target")).expect("tracked runtime root");
        fs::write(repo.join("target/tracked.txt"), "operator source\n").expect("tracked file");
        git_status(&repo, &["add", "-f", "target/tracked.txt"]).expect("add tracked file");
        git_status(&repo, &["commit", "-m", "track target source"]).expect("commit tracked file");

        fs::create_dir_all(repo.join("target/debug")).expect("target build output");
        fs::write(repo.join("target/debug/generated"), "build output\n").expect("target output");
        fs::create_dir_all(repo.join("web/node_modules/pkg")).expect("dependency output");
        fs::write(repo.join("web/node_modules/pkg/index.js"), "generated\n")
            .expect("dependency file");
        fs::write(repo.join("operator-note.txt"), "preserve me\n").expect("unknown file");

        let mut removed = Vec::new();
        remove_untracked_runtime_roots(&repo, &repo, "HEAD", &mut removed)
            .expect("remove disposable residue");

        assert!(repo.join("target/tracked.txt").exists());
        assert!(repo.join("target/debug/generated").exists());
        assert!(!repo.join("web/node_modules").exists());
        assert!(repo.join("operator-note.txt").exists());
        assert_eq!(
            removed,
            vec![repo.join("web/node_modules").display().to_string()]
        );
    }

    #[test]
    fn finish_refuses_missing_uncertain_or_uncontained_receipt() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));

        let missing = job_view(
            temp.path(),
            Some(JobOutcome::Verified),
            Some(StopReason::Verified),
        );
        let error = finish_job_state(&paths, &missing).expect_err("missing receipt");
        assert!(error.to_string().contains("no sealed completion receipt"));

        let uncertain = job_view(
            temp.path(),
            Some(JobOutcome::NeedsReview),
            Some(StopReason::SemanticUncertain),
        );
        let error = finish_job_state(&paths, &uncertain).expect_err("uncertain result");
        assert!(error.to_string().contains("no verified receipt"));

        let uncontained = job_view(
            temp.path(),
            Some(JobOutcome::Verified),
            Some(StopReason::Verified),
        );
        let receipt_path = paths.job_receipt(uncontained.job.job_id.as_ref());
        fs::create_dir_all(receipt_path.parent().expect("receipt parent")).expect("job dir");
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&uncontained_receipt(&uncontained.job.job_id))
                .expect("receipt json"),
        )
        .expect("receipt");
        let error = finish_job_state(&paths, &uncontained).expect_err("uncontained receipt");
        assert!(error.to_string().contains("contained execution"));
    }

    #[test]
    fn verified_finish_carries_backing_run_past_job_identity_precedence() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job_id = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let state = create_run(
            &paths,
            RunOptions {
                goal: "deliver the verified result".to_string(),
                cwd: source.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some(job_id.to_string()),
                codebase: None,
            },
        )
        .expect("backing run");
        deadreckon_core::write_job(
            &paths,
            &Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                scope: state.scope.clone(),
                goal: state.goal.clone(),
                shape: JobShape::Single,
                created_at: Utc::now(),
                source_cwd: source,
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
        .expect("job identity");

        let collision =
            resolve_apply_state(&paths, job_id, true, None).expect_err("job outranks backing run");
        assert!(collision.to_string().contains("is a job"), "{collision}");

        let resolved = resolve_apply_state(&paths, job_id, true, Some(state))
            .expect("finish already validated the Job and may retain its backing Run");
        assert_eq!(resolved.run_id, job_id);
    }

    #[test]
    fn export_refuses_a_job_owned_plan_result_before_docs_or_destination_mutation() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let job_id = "dededededededededededededededede";
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: "materialize-authority-test".to_string(),
            goal: "deliver only the sealed result".to_string(),
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
        };
        deadreckon_core::write_job(&paths, &job).expect("Job identity");
        let mut plan = deadreckon_core::plan::Plan::new(
            job.goal.clone(),
            deadreckon_core::plan::PlanMode::FullPlan,
            vec![
                deadreckon_core::plan::PlanTask::new(
                    0,
                    "first",
                    "first task",
                    deadreckon_core::plan::PlanRole::Child,
                    None,
                ),
                deadreckon_core::plan::PlanTask::new(
                    1,
                    "second",
                    "second task",
                    deadreckon_core::plan::PlanRole::Child,
                    None,
                ),
            ],
            deadreckon_core::plan::PlanProviders::default(),
            Some(job.scope.clone()),
            "test",
        )
        .expect("Plan");
        plan.plan_id = job_id.to_string();
        plan.owner_job_id = Some(job_id.to_string());
        plan.parent_cwd = Some(workspace.clone());
        deadreckon_core::plan::save_plan(&paths, &plan).expect("owned Plan");
        let mut result = deadreckon_core::create_owned_run(
            &paths,
            RunOptions {
                goal: job.goal.clone(),
                cwd: workspace,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("dfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdf".to_string()),
                codebase: None,
            },
            deadreckon_core::RunOwnership::plan_result(job_id, job_id),
        )
        .expect("owned result Run");
        result.status = RunStatus::Completed;
        save_state(&result).expect("completed result");
        plan.status = deadreckon_core::plan::PlanStatus::Merged;
        plan.merged_run_id = Some(result.run_id.clone());
        deadreckon_core::plan::save_plan(&paths, &plan).expect("merged Plan");
        let plan_before = fs::read(paths.plan_json(job_id)).expect("Plan bytes before");
        let dest = temp.path().join("forbidden-export");

        let error =
            materialize_command_with_paths(&paths, job_id, Some(dest.clone()), false, false)
                .expect_err("unverified Job result must not be exported");

        assert!(error.to_string().contains("no verified receipt"), "{error}");
        assert!(!dest.exists(), "refused export created its destination");
        assert_eq!(
            fs::read(paths.plan_json(job_id)).expect("Plan bytes after"),
            plan_before
        );
        assert!(
            !plan_docs_dir(&paths, job_id).exists(),
            "refused export generated Plan docs before authority validation"
        );
    }

    #[test]
    fn verified_delivery_authority_exports_the_exact_job_result_and_records_it() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, state, job_id) = signed_verified_export_fixture(&temp);
        let dest = temp.path().join("verified-export");

        materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), false, false)
            .expect("authorized export");

        assert_eq!(
            fs::read_to_string(dest.join("result.txt")).expect("delivered result"),
            "verified result\n"
        );
        let delivered = deadreckon_core::JobView::load(&paths, &job_id).expect("delivered view");
        let canonical_dest = fs::canonicalize(&dest).expect("canonical destination");
        assert_eq!(
            delivered
                .projection
                .delivery
                .as_ref()
                .map(|delivery| delivery.destination.as_path()),
            Some(canonical_dest.as_path())
        );
        assert_eq!(
            super::super::inspection::materialized_marker_count(
                &paths.library_dir(&state.scope, &job_id)
            ),
            1
        );
    }

    #[test]
    fn verified_export_refuses_post_validation_source_mutation_without_touching_destination() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, state, job_id) = signed_verified_export_fixture(&temp);
        let authority = signed_export_authority(&paths, &state, &job_id);
        let dest = temp.path().join("existing-destination");
        fs::create_dir(&dest).expect("existing destination");
        fs::write(dest.join("operator.txt"), "preserve this\n").expect("operator file");
        let verified_dest = absolute_dest(dest.clone()).expect("canonical destination");
        let library = state.working_dir.clone();
        let mut mutate_after_validation = || {
            fs::write(library.join("result.txt"), "mutated after validation\n")
                .expect("post-validation mutation");
        };

        let error = materialize_verified_completed_run(
            &paths,
            &state,
            &state.working_dir,
            &verified_dest,
            true,
            false,
            &authority,
            VerifiedExportFailpoint::None,
            Some(&mut mutate_after_validation),
        )
        .expect_err("post-validation mutation must be refused");

        assert!(error.to_string().contains("result tree"), "{error}");
        assert_eq!(
            fs::read_to_string(dest.join("operator.txt")).expect("preserved destination"),
            "preserve this\n"
        );
        assert!(!dest.join("result.txt").exists());
        let transaction = only_export_transaction(&paths, &job_id);
        assert!(!transaction.stage.exists(), "owned partial stage cleaned");
        assert!(
            !transaction.backup.exists(),
            "prior destination was never moved"
        );
        let view = deadreckon_core::JobView::load(&paths, &job_id).expect("Job view");
        assert!(view.projection.delivery.is_none());
    }

    #[test]
    fn verified_export_crash_after_publication_retries_to_one_marker_and_one_delivery() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, state, job_id) = signed_verified_export_fixture(&temp);
        let authority = signed_export_authority(&paths, &state, &job_id);
        let dest = temp.path().join("published-before-event");
        let verified_dest = absolute_dest(dest.clone()).expect("canonical destination");

        let error = materialize_verified_completed_run(
            &paths,
            &state,
            &state.working_dir,
            &verified_dest,
            false,
            false,
            &authority,
            VerifiedExportFailpoint::AfterPublish,
            None,
        )
        .expect_err("injected publication crash");
        assert!(error.to_string().contains("AfterPublish"), "{error}");
        assert_eq!(
            fs::read_to_string(dest.join("result.txt")).expect("atomically published result"),
            "verified result\n"
        );
        assert!(
            deadreckon_core::JobView::load(&paths, &job_id)
                .expect("pre-retry Job")
                .projection
                .delivery
                .is_none(),
            "publication happened before its factual event"
        );

        materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), false, false)
            .expect("retry records factual publication");
        materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), false, false)
            .expect("duplicate retry converges");

        let library = paths.library_dir(&state.scope, &job_id);
        assert_eq!(
            super::super::inspection::materialized_marker_count(&library),
            1
        );
        let history =
            deadreckon_core::read_job_history(&paths.job_events(&job_id)).expect("Job history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::ResultExported)
                .count(),
            1
        );
        let transaction = only_export_transaction(&paths, &job_id);
        assert_eq!(transaction.phase, VerifiedExportPhase::Completed);
        assert!(!transaction.stage.exists());
        assert!(!transaction.backup.exists());
    }

    #[test]
    fn verified_export_recovers_each_forced_replace_and_event_window() {
        for failpoint in [
            VerifiedExportFailpoint::AfterBackupRename,
            VerifiedExportFailpoint::AfterEvent,
        ] {
            let temp = TempDir::new().expect("tempdir");
            let (paths, state, job_id) = signed_verified_export_fixture(&temp);
            let authority = signed_export_authority(&paths, &state, &job_id);
            let dest = temp.path().join("forced-destination");
            fs::create_dir(&dest).expect("prior destination");
            fs::write(dest.join("prior.txt"), "prior operator tree\n").expect("prior tree");
            let verified_dest = absolute_dest(dest.clone()).expect("canonical destination");

            materialize_verified_completed_run(
                &paths,
                &state,
                &state.working_dir,
                &verified_dest,
                true,
                false,
                &authority,
                failpoint,
                None,
            )
            .expect_err("injected transactional crash");

            let transaction = only_export_transaction(&paths, &job_id);
            assert!(
                transaction.backup.exists(),
                "prior tree is retained through {failpoint:?}"
            );
            materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), false, false)
                .expect("ordinary retry completes the existing transaction");
            assert_eq!(
                fs::read_to_string(dest.join("result.txt")).expect("verified result"),
                "verified result\n"
            );
            assert!(!dest.join("prior.txt").exists());
            let completed = only_export_transaction(&paths, &job_id);
            assert_eq!(completed.phase, VerifiedExportPhase::Completed);
            assert!(!completed.backup.exists());
            let history =
                deadreckon_core::read_job_history(&paths.job_events(&job_id)).expect("Job history");
            assert_eq!(
                history
                    .events()
                    .iter()
                    .filter(|event| event.kind == JobEventKind::ResultExported)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn signed_receipt_refusal_never_mutates_an_explicit_destination() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, state, job_id) = signed_verified_export_fixture(&temp);
        fs::write(
            state.working_dir.join("result.txt"),
            "tampered before export\n",
        )
        .expect("tamper signed result");
        let dest = temp.path().join("must-remain");
        fs::create_dir(&dest).expect("destination");
        fs::write(dest.join("operator.txt"), "untouched\n").expect("operator file");

        let error =
            materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), true, false)
                .expect_err("tampered signed result is refused");

        assert!(error.to_string().contains("result tree"), "{error}");
        assert_eq!(
            fs::read_to_string(dest.join("operator.txt")).expect("operator file"),
            "untouched\n"
        );
        assert!(!dest.join("result.txt").exists());
        assert!(
            !paths.job_dir(&job_id).join("export-transactions").exists(),
            "authority refusal happened before transaction creation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn verified_export_refuses_staging_symlink_substitution_and_preserves_unrelated_tree() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let (paths, state, job_id) = signed_verified_export_fixture(&temp);
        let authority = signed_export_authority(&paths, &state, &job_id);
        let dest = temp.path().join("symlink-substitution-destination");
        let verified_dest = absolute_dest(dest.clone()).expect("canonical destination");

        materialize_verified_completed_run(
            &paths,
            &state,
            &state.working_dir,
            &verified_dest,
            false,
            false,
            &authority,
            VerifiedExportFailpoint::AfterStageSync,
            None,
        )
        .expect_err("injected stage crash");
        let transaction = only_export_transaction(&paths, &job_id);
        fs::remove_dir_all(&transaction.stage).expect("replace owned stage in adversarial test");
        let unrelated = temp.path().join("unrelated");
        fs::create_dir(&unrelated).expect("unrelated tree");
        fs::write(unrelated.join("sentinel.txt"), "preserve\n").expect("sentinel");
        symlink(&unrelated, &transaction.stage).expect("substitute stage symlink");

        let error =
            materialize_command_with_paths(&paths, &job_id, Some(dest.clone()), false, false)
                .expect_err("substituted stage is refused");

        assert!(error.to_string().contains("substituted"), "{error}");
        assert!(!dest.exists());
        assert_eq!(
            fs::read_to_string(unrelated.join("sentinel.txt")).expect("unrelated sentinel"),
            "preserve\n"
        );
        assert!(
            fs::symlink_metadata(&transaction.stage)
                .expect("substituted symlink remains")
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn verified_export_aliases_bind_and_record_the_canonical_destination() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let (paths, _state, job_id) = signed_verified_export_fixture(&temp);
        let real_parent = temp.path().join("real-parent");
        fs::create_dir(&real_parent).expect("real parent");
        let alias_parent = temp.path().join("alias-parent");
        symlink(&real_parent, &alias_parent).expect("destination alias");
        let aliased_dest = alias_parent.join("delivered");
        let canonical_dest = fs::canonicalize(&real_parent)
            .expect("canonical real parent")
            .join("delivered");

        materialize_command_with_paths(&paths, &job_id, Some(aliased_dest), false, false)
            .expect("alias export");

        let view = deadreckon_core::JobView::load(&paths, &job_id).expect("delivered Job");
        assert_eq!(
            view.projection
                .delivery
                .as_ref()
                .map(|delivery| delivery.destination.as_path()),
            Some(canonical_dest.as_path())
        );
        let transaction = only_export_transaction(&paths, &job_id);
        assert_eq!(transaction.destination, canonical_dest);
    }

    #[test]
    fn materialize_public_aliases_parse_to_the_same_export_command() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                for command in ["materialize", "export", "copy-out"] {
                    let parsed =
                        crate::cli::Cli::try_parse_from(["deadreckon", command, "job-123"])
                            .expect("public export alias parses");
                    let Some(crate::cli::Commands::Materialize { run_id, .. }) = parsed.command
                    else {
                        panic!("{command} did not resolve to materialize")
                    };
                    assert_eq!(run_id, "job-123");
                }
            })
            .expect("spawn parser test")
            .join()
            .expect("parser test");
    }

    #[test]
    fn legacy_apply_refuses_private_artifact_even_when_later_deleted() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, _) = git_repo(&repo);
        let private = repo.join(".specstory/history/session.md");
        fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
        fs::write(&private, "provider evidence\n").expect("private evidence");
        git_status(&repo, &["add", "-f", ".specstory/history/session.md"]).expect("add private");
        git_status(&repo, &["commit", "-m", "add private artifact"]).expect("commit private");
        fs::remove_file(&private).expect("remove private");
        git_status(&repo, &["add", "-u", ".specstory/history/session.md"])
            .expect("stage private removal");
        git_status(&repo, &["commit", "-m", "remove private artifact"]).expect("commit removal");
        let branch = git_stdout(&repo, &["symbolic-ref", "--short", "HEAD"]).expect("branch");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "deliver clean history".to_string(),
                cwd: repo.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("legacy-private-history".to_string()),
                codebase: None,
            },
        )
        .expect("state");
        let record = worktree_record(&repo, &base, &branch);

        let error = refuse_non_deliverable_result_history(&state, &record, &repo, &branch)
            .expect_err("legacy result history must be refused");

        assert!(
            error.to_string().contains("non-deliverable paths"),
            "{error}"
        );
    }

    #[test]
    fn applied_identity_uses_committed_revision_not_restored_operator_changes() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, base_branch) = git_repo(&repo);
        git_status(&repo, &["switch", "-c", "result"]).expect("result branch");
        fs::write(repo.join("signed.txt"), "signed result\n").expect("signed result");
        git_status(&repo, &["add", "signed.txt"]).expect("add result");
        git_status(&repo, &["commit", "-m", "signed result"]).expect("commit result");
        let result = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("result revision");
        git_status(&repo, &["switch", &base_branch]).expect("target branch");
        git_status(
            &repo,
            &["merge", "--no-ff", "result", "-m", "deliver result"],
        )
        .expect("merge result");

        let job_id = JobId("efefefefefefefefefefefefefefefef".to_string());
        let mut receipt = uncontained_receipt(&job_id);
        receipt.source_revision = Some(base.clone());
        receipt.result_revision = Some(result);
        let record = worktree_record(&repo, &base, "result");

        // This models an operator edit restored from autostash. It changes the
        // working file, but not the delivered Git revision being recorded.
        fs::write(repo.join("signed.txt"), "operator local edit\n").expect("operator edit");
        verify_applied_receipt_identity(job_id.as_ref(), &receipt, &record, &repo, &base)
            .expect("restored operator edit must not obscure committed delivery identity");

        git_status(&repo, &["add", "signed.txt"]).expect("stage tamper");
        git_status(&repo, &["commit", "-m", "tamper delivered result"]).expect("commit tamper");
        let error =
            verify_applied_receipt_identity(job_id.as_ref(), &receipt, &record, &repo, &base)
                .expect_err("committed delivery mismatch must be refused");
        assert!(
            error
                .to_string()
                .contains("does not match the signed result"),
            "{error}"
        );
    }

    #[test]
    fn applied_identity_refuses_extra_committed_delivery_paths() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, base_branch) = git_repo(&repo);
        git_status(&repo, &["switch", "-c", "result"]).expect("result branch");
        fs::write(repo.join("signed.txt"), "signed result\n").expect("signed result");
        git_status(&repo, &["add", "signed.txt"]).expect("add result");
        git_status(&repo, &["commit", "-m", "signed result"]).expect("commit result");
        let result = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("result revision");
        git_status(&repo, &["switch", &base_branch]).expect("target branch");
        git_status(
            &repo,
            &["merge", "--no-ff", "result", "-m", "deliver result"],
        )
        .expect("merge result");
        fs::write(repo.join("hook-output.txt"), "not in signed result\n").expect("hook output");
        git_status(&repo, &["add", "hook-output.txt"]).expect("add hook output");
        git_status(&repo, &["commit", "-m", "unexpected apply side effect"])
            .expect("commit hook output");

        let job_id = JobId("abababababababababababababababab".to_string());
        let mut receipt = uncontained_receipt(&job_id);
        receipt.source_revision = Some(base.clone());
        receipt.result_revision = Some(result);
        let record = worktree_record(&repo, &base, "result");

        let error =
            verify_applied_receipt_identity(job_id.as_ref(), &receipt, &record, &repo, &base)
                .expect_err("extra committed delivery path must be refused");
        assert!(
            error
                .to_string()
                .contains("outside the signed result: hook-output.txt"),
            "{error}"
        );
    }

    #[test]
    fn applied_identity_refuses_extra_path_added_then_deleted_during_delivery() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, base_branch) = git_repo(&repo);
        git_status(&repo, &["switch", "-c", "result"]).expect("result branch");
        fs::write(repo.join("signed.txt"), "signed result\n").expect("signed result");
        git_status(&repo, &["add", "signed.txt"]).expect("add result");
        git_status(&repo, &["commit", "-m", "signed result"]).expect("commit result");
        let result = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("result revision");
        git_status(&repo, &["switch", &base_branch]).expect("target branch");
        git_status(
            &repo,
            &["merge", "--no-ff", "result", "-m", "deliver result"],
        )
        .expect("merge result");
        fs::write(repo.join("transient-hook-output.txt"), "transient\n").expect("hook output");
        git_status(&repo, &["add", "transient-hook-output.txt"]).expect("add hook output");
        git_status(&repo, &["commit", "-m", "unexpected apply side effect"])
            .expect("commit hook output");
        fs::remove_file(repo.join("transient-hook-output.txt")).expect("remove hook output");
        git_status(&repo, &["add", "-u", "transient-hook-output.txt"]).expect("stage hook removal");
        git_status(&repo, &["commit", "-m", "hide apply side effect"])
            .expect("commit hook removal");

        let job_id = JobId("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd".to_string());
        let mut receipt = uncontained_receipt(&job_id);
        receipt.source_revision = Some(base.clone());
        receipt.result_revision = Some(result);
        let record = worktree_record(&repo, &base, "result");

        let error =
            verify_applied_receipt_identity(job_id.as_ref(), &receipt, &record, &repo, &base)
                .expect_err("transient extra delivery path must be refused");
        assert!(
            error
                .to_string()
                .contains("outside the signed result: transient-hook-output.txt"),
            "{error}"
        );
    }

    #[test]
    fn delivery_history_does_not_reclassify_existing_target_changes_as_side_effects() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, base_branch) = git_repo(&repo);
        git_status(&repo, &["switch", "-c", "result"]).expect("result branch");
        fs::write(repo.join("signed.txt"), "signed result\n").expect("signed result");
        git_status(&repo, &["add", "signed.txt"]).expect("add result");
        git_status(&repo, &["commit", "-m", "signed result"]).expect("commit result");
        let result = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("result revision");
        git_status(&repo, &["switch", &base_branch]).expect("target branch");
        fs::write(repo.join("operator.txt"), "existing target change\n").expect("operator change");
        git_status(&repo, &["add", "operator.txt"]).expect("add operator change");
        git_status(&repo, &["commit", "-m", "operator target change"])
            .expect("commit operator change");
        let delivery_before = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("delivery base");
        git_status(
            &repo,
            &["merge", "--no-ff", "result", "-m", "deliver result"],
        )
        .expect("merge result");

        let job_id = JobId("dededededededededededededededede".to_string());
        let mut receipt = uncontained_receipt(&job_id);
        receipt.source_revision = Some(base.clone());
        receipt.result_revision = Some(result);
        let record = worktree_record(&repo, &base, "result");

        verify_applied_receipt_identity(
            job_id.as_ref(),
            &receipt,
            &record,
            &repo,
            &delivery_before,
        )
        .expect("pre-existing target commits are not delivery side effects");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn applied_identity_preserves_non_utf8_git_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, base_branch) = git_repo(&repo);
        git_status(&repo, &["switch", "-c", "result"]).expect("result branch");
        let raw_path = PathBuf::from(OsString::from_vec(b"signed-\xff.txt".to_vec()));
        fs::write(repo.join(&raw_path), "raw path result\n").expect("raw result");
        git_status(&repo, &["add", "-A"]).expect("add raw result");
        git_status(&repo, &["commit", "-m", "signed raw path"]).expect("commit raw result");
        let result = git_stdout(&repo, &["rev-parse", "HEAD"]).expect("result revision");
        git_status(&repo, &["switch", &base_branch]).expect("target branch");
        git_status(
            &repo,
            &["merge", "--no-ff", "result", "-m", "deliver raw result"],
        )
        .expect("merge raw result");

        let job_id = JobId("efefefefefefefefefefefefefefefef".to_string());
        let mut receipt = uncontained_receipt(&job_id);
        receipt.source_revision = Some(base.clone());
        receipt.result_revision = Some(result);
        let record = worktree_record(&repo, &base, "result");

        verify_applied_receipt_identity(job_id.as_ref(), &receipt, &record, &repo, &base)
            .expect("raw Git path must compare without UTF-8 conversion");
    }

    #[cfg(unix)]
    #[test]
    fn git_tree_path_matching_does_not_require_utf8() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"signed-\xff.txt".to_vec()));
        assert!(git_path_matches(&path, b"signed-\xff.txt").expect("raw path comparison"));
        assert!(!git_path_matches(&path, b"signed-other.txt").expect("different raw path"));
    }

    #[test]
    fn refused_verified_delivery_restores_the_target_revision() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let (base, _) = git_repo(&repo);
        fs::write(repo.join("signed.txt"), "applied but refused\n").expect("applied change");
        git_status(&repo, &["add", "signed.txt"]).expect("add applied change");
        git_status(&repo, &["commit", "-m", "applied result"]).expect("commit applied change");
        assert_ne!(
            git_stdout(&repo, &["rev-parse", "HEAD"]).expect("applied revision"),
            base
        );

        let error = rollback_refused_job_delivery(
            &repo,
            "abababababababababababababababab",
            &base,
            None,
            CliError::Core(DeadreckonError::InvalidInput(
                "post-apply identity mismatch".to_string(),
            )),
        );

        assert!(error.to_string().contains("post-apply identity mismatch"));
        assert_eq!(
            git_stdout(&repo, &["rev-parse", "HEAD"]).expect("restored revision"),
            base
        );
        assert_eq!(
            fs::read_to_string(repo.join("signed.txt")).expect("restored file"),
            "base\n"
        );
    }

    fn durable_continuation_fixture(
        temp: &TempDir,
    ) -> (
        DeadreckonPaths,
        deadreckon_core::PipelineState,
        deadreckon_core::PipelineState,
        commands::run::DurableContinuationSpec,
        String,
    ) {
        let (paths, parent, job_id) = signed_verified_export_fixture(temp);
        let child_source = temp.path().join("continuation-source");
        fs::create_dir_all(&child_source).expect("child source");
        let child = create_run(
            &paths,
            RunOptions {
                goal: "continue the verified result".to_string(),
                cwd: child_source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("test".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: None,
                codebase: None,
            },
        )
        .expect("child run");
        let parent_library = paths.library_dir(&parent.scope, &parent.run_id);
        let continuation = commands::run::DurableContinuationSpec {
            parent_run_id: parent.run_id.clone(),
            parent_scope: parent.scope.clone(),
            parent_state_sha256: deadreckon_core::flight::sha256_file(&parent.state_path())
                .expect("parent state digest"),
            parent_library_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(
                &parent_library,
            )
            .expect("parent library index")
            .tree_hash(),
            parent_receipt_sha256: Some(
                deadreckon_core::flight::sha256_file(&paths.job_receipt(&job_id))
                    .expect("parent receipt digest"),
            ),
            context_turns: Some(2),
        };
        (paths, parent, child, continuation, job_id)
    }

    fn assert_continuation_left_no_child_evidence(state: &deadreckon_core::PipelineState) {
        assert!(
            !state
                .working_dir
                .join(".deadreckon")
                .join("parent.json")
                .exists()
        );
        assert!(!state.run_root.join("history.json").exists());
    }

    #[test]
    fn durable_continuation_binds_verified_parent_receipt_and_tree() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, parent, child, continuation, _) = durable_continuation_fixture(&temp);

        prepare_durable_continuation(&paths, &child, &continuation)
            .expect("unchanged verified parent");

        let marker: ParentMarker = serde_json::from_slice(
            &fs::read(child.working_dir.join(".deadreckon").join("parent.json"))
                .expect("parent marker"),
        )
        .expect("parent marker json");
        assert_eq!(marker.parent_run_id, parent.run_id);
        assert!(child.run_root.join("history.json").is_file());
        let trace = read_jsonl::<TraceRecord>(&child.run_root.join("traces.jsonl"))
            .expect("continuation trace");
        assert!(
            trace
                .iter()
                .any(|record| record.event == "durable_continuation_bound")
        );
    }

    #[test]
    fn durable_continuation_refuses_parent_state_change_before_child_evidence() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, parent, child, continuation, _) = durable_continuation_fixture(&temp);
        let mut changed = parent;
        changed.updated_at = Utc::now() + chrono::Duration::seconds(1);
        save_state(&changed).expect("changed parent state");

        let error = prepare_durable_continuation(&paths, &child, &continuation)
            .expect_err("changed parent state must fail closed");

        assert!(
            error.to_string().contains("changed after approval"),
            "{error}"
        );
        assert_continuation_left_no_child_evidence(&child);
    }

    #[test]
    fn durable_continuation_refuses_parent_library_change_before_child_evidence() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, parent, child, continuation, _) = durable_continuation_fixture(&temp);
        fs::write(
            paths
                .library_dir(&parent.scope, &parent.run_id)
                .join("unapproved.txt"),
            "changed after continuation approval\n",
        )
        .expect("changed parent library");

        let error = prepare_durable_continuation(&paths, &child, &continuation)
            .expect_err("changed parent library must fail closed");

        assert!(
            error.to_string().contains("library")
                && error.to_string().contains("changed after approval"),
            "{error}"
        );
        assert_continuation_left_no_child_evidence(&child);
    }

    #[test]
    fn durable_continuation_refuses_parent_receipt_change_before_child_evidence() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, _, child, continuation, job_id) = durable_continuation_fixture(&temp);
        let receipt_path = paths.job_receipt(&job_id);
        let mut bytes = fs::read(&receipt_path).expect("receipt");
        bytes.push(b'\n');
        fs::write(&receipt_path, bytes).expect("changed receipt bytes");

        let error = prepare_durable_continuation(&paths, &child, &continuation)
            .expect_err("changed parent receipt must fail closed");

        assert!(
            error.to_string().contains("receipt")
                && error.to_string().contains("changed after approval"),
            "{error}"
        );
        assert_continuation_left_no_child_evidence(&child);
    }
}
