use super::super::*;

pub(crate) const TRUSTED_SUPERVISOR_JOB_ID_ENV: &str = "DEADRECKON_SUPERVISOR_JOB_ID";
pub(crate) const TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV: &str = "DEADRECKON_SUPERVISOR_LAUNCH_PLAN";
pub(crate) const LEGACY_CHAIN_FOREGROUND_ENV: &str = "DEADRECKON_LEGACY_CHAIN_STEP_FOREGROUND";
const DURABLE_LEAF_SIGNAL: &str = "watchkeeper_leaf";
const UNTRUSTED_FOREGROUND_SIGNAL: &str = "untrusted_foreground";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectRunRoute {
    DurableJob,
    Foreground,
}

/// Direct-run options that affect the worker's result, frozen before the
/// detached supervisor starts. Source, budget, provider and model already live
/// in first-class launch-plan/authority fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DurableContinuationSpec {
    pub(crate) parent_run_id: String,
    pub(crate) parent_scope: String,
    pub(crate) parent_state_sha256: String,
    pub(crate) parent_library_tree_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_receipt_sha256: Option<String>,
    pub(crate) context_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DurableLeafSpec {
    pub(crate) base: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) no_seams: bool,
    pub(crate) doc_provider: Option<String>,
    pub(crate) skill: String,
    pub(crate) no_docs: bool,
    pub(crate) doc_skill: Option<String>,
    pub(crate) narrate: bool,
    pub(crate) no_narrate: bool,
    pub(crate) narrator_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) continuation: Option<DurableContinuationSpec>,
}

pub(crate) fn durable_leaf_spec(
    plan: &commands::course::LaunchPlan,
) -> Result<Option<DurableLeafSpec>> {
    plan.signals
        .get(DURABLE_LEAF_SIGNAL)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(CliError::from)
}

fn trusted_supervisor_run_id(requested: Option<String>) -> Result<Option<String>> {
    let Some(run_id) = requested else {
        return Ok(None);
    };
    let trusted = std::env::var(TRUSTED_SUPERVISOR_JOB_ID_ENV).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "--run-id is reserved for trusted supervisor launches".to_string(),
        ))
    })?;
    if trusted != run_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "trusted supervisor job id does not match --run-id".to_string(),
        )));
    }
    if run_id.len() != 32 || !run_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "trusted supervisor job id must be exactly 32 hexadecimal characters".to_string(),
        )));
    }
    Ok(Some(run_id))
}

/// A project done contract is controller input, not an implementation change.
/// Permit only the exact bounded bundle that `def-done` wrote, without seeding
/// it into the implementation worktree. The later Job freeze binds these bytes
/// into immutable authority; unrelated dirt remains a hard refusal.
pub(crate) fn bounded_project_contract_dirty_paths(
    source_cwd: &Path,
    acceptance_source: &commands::acceptance::AcceptanceSource,
) -> Result<Vec<PathBuf>> {
    let expected_project = source_cwd.join(".deadreckon/acceptance.yaml");
    if canonical_or_original(&acceptance_source.path) != canonical_or_original(&expected_project) {
        return Ok(Vec::new());
    }
    let bundle = commands::job::source_contract_bundle_inventory(&acceptance_source.path)?;
    project_contract_dirty_paths_from_bundle(source_cwd, bundle, None)
}

pub(crate) fn bounded_existing_project_contract_dirty_paths(
    source_cwd: &Path,
    explicit_acceptance: Option<&Path>,
    allow_dirty: bool,
) -> Result<Vec<PathBuf>> {
    if allow_dirty {
        return Ok(Vec::new());
    }
    let source = commands::acceptance::resolve_acceptance_source(source_cwd, explicit_acceptance)?;
    source
        .as_ref()
        .map(|source| bounded_project_contract_dirty_paths(source_cwd, source))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Guided start writes its approved project contract after its first
/// source-mode preflight. Return only the controller-owned paths whose bytes
/// are already bound by the immutable launch plan and match both the project
/// and frozen Job copies.
fn trusted_controller_contract_dirty_paths(
    paths: &DeadreckonPaths,
    run_id: &str,
    source_cwd: &Path,
    acceptance_source: &commands::acceptance::AcceptanceSource,
    launch_plan: &commands::course::LaunchPlan,
) -> Result<Vec<PathBuf>> {
    let expected_frozen = commands::job::job_acceptance_path(paths, run_id);
    if canonical_or_original(&acceptance_source.path) != canonical_or_original(&expected_frozen) {
        return Ok(Vec::new());
    }
    commands::job::validate_frozen_contract_bundle(paths, run_id, launch_plan)?;
    let Some(bundle) = commands::job::contract_bundle_from_plan(launch_plan)? else {
        return Ok(Vec::new());
    };
    project_contract_dirty_paths_from_bundle(source_cwd, bundle, Some(&paths.job_dir(run_id)))
}

fn project_contract_dirty_paths_from_bundle(
    source_cwd: &Path,
    bundle: commands::job::FrozenContractBundle,
    frozen_dir: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let Some(git_root) = deadreckon_core::find_git_root(source_cwd)? else {
        return Ok(Vec::new());
    };
    let project_contract_dir = canonical_or_original(&source_cwd.join(".deadreckon"));
    let mut allowed = Vec::with_capacity(bundle.files.len());
    for file in bundle.files {
        let relative = Path::new(&file.path);
        let valid = !relative.is_absolute()
            && relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            && (relative == Path::new("acceptance.yaml")
                || relative == Path::new("acceptance.md")
                || relative.starts_with("acceptance"));
        if !valid {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "invalid done-contract bundle path: {}",
                file.path
            ))));
        }
        let project_file = project_contract_dir.join(relative);
        let project_metadata = match fs::symlink_metadata(&project_file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if project_metadata.file_type().is_symlink()
            || !project_metadata.file_type().is_file()
            || !commands::job::contract_file_matches_inventory(&project_file, &file)?
        {
            return Ok(Vec::new());
        }
        if let Some(frozen_dir) = frozen_dir {
            let frozen_file = frozen_dir.join(relative);
            let frozen_metadata = fs::symlink_metadata(&frozen_file)?;
            if frozen_metadata.file_type().is_symlink()
                || !frozen_metadata.file_type().is_file()
                || !commands::job::contract_file_matches_inventory(&frozen_file, &file)?
            {
                return Ok(Vec::new());
            }
        }
        let relative_to_repo = project_file.strip_prefix(&git_root).map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "guided contract bundle is outside its git repository".to_string(),
            ))
        })?;
        allowed.push(relative_to_repo.to_path_buf());
    }
    Ok(allowed)
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Direct `deadreckon run`: a trivial operator plan records the decision so
/// every run root carries `launch-plan.json`, however the launch began.
pub(crate) async fn run_command(args: RunCommandArgs) -> Result<()> {
    run_command_with_job_id(args).await.map(|_| ())
}

/// Dispatch a run and return the durable Job identity when this invocation
/// queued one. Foreground compatibility and trusted-child paths return
/// `None`.
pub(crate) async fn run_command_with_job_id(
    args: RunCommandArgs,
) -> Result<Option<deadreckon_protocol::JobId>> {
    let explicitly_uncontained = args.sandbox.as_deref() == Some("none");
    let delegated_job_child = commands::graph_job::delegated_plan_child_authorized();
    let private_characterization = commands::plan::internal_characterization_requested();
    let private_legacy_chain_foreground =
        private_characterization && std::env::var_os(LEGACY_CHAIN_FOREGROUND_ENV).is_some();
    let route = direct_run_route(
        args.preview,
        args.in_place,
        explicitly_uncontained,
        args.untrusted,
        args.run_id.is_some(),
        delegated_job_child,
        private_characterization,
        private_legacy_chain_foreground,
    )?;
    if (args.durable_source_cwd.is_some() || args.continuation.is_some())
        && route != DirectRunRoute::DurableJob
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "internal continuation inputs are valid only when scheduling a durable Job".to_string(),
        )));
    }
    if let Some(run_id) = args.run_id.as_deref() {
        let paths = DeadreckonPaths::discover();
        let _authority = commands::supervisor::require_guarded_driver_launch(&paths, run_id)?;
    }
    let mut plan = if args.run_id.is_some() {
        let path = std::env::var_os(TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "trusted supervisor launch is missing its immutable launch-plan path"
                        .to_string(),
                ))
            })?;
        commands::course::load_launch_plan(&path)?
    } else {
        commands::course::trivial_operator_plan(
            &args.goal,
            commands::course::CourseShape::Single,
            "run",
        )
    };
    if args.untrusted {
        mark_untrusted_foreground(&mut plan);
    }
    match route {
        DirectRunRoute::DurableJob => schedule_direct_run(args, &mut plan, None).await,
        DirectRunRoute::Foreground => run_command_with_launch_plan(args, plan).await.map(|_| None),
    }
}

/// Queue a durable Single Job using an already approved launch plan.
///
/// Guided continuation uses this rather than rebuilding a second planning
/// decision after the operator has approved `start`.
pub(crate) async fn schedule_durable_run_with_launch_plan(
    args: RunCommandArgs,
    mut launch_plan: commands::course::LaunchPlan,
) -> Result<Option<deadreckon_protocol::JobId>> {
    let explicitly_uncontained = args.sandbox.as_deref() == Some("none");
    let route = direct_run_route(
        args.preview,
        args.in_place,
        explicitly_uncontained,
        args.untrusted,
        args.run_id.is_some(),
        commands::graph_job::delegated_plan_child_authorized(),
        commands::plan::internal_characterization_requested(),
        false,
    )?;
    if route != DirectRunRoute::DurableJob {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "an approved guided launch plan can only schedule a durable Job".to_string(),
        )));
    }
    if args.durable_source_cwd.is_none() || args.continuation.is_none() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guided continuation is missing its frozen durable parent inputs".to_string(),
        )));
    }
    let accepted_by = approved_launch_authority(&launch_plan)?;
    schedule_direct_run(args, &mut launch_plan, Some(accepted_by)).await
}

fn approved_launch_authority(
    launch_plan: &commands::course::LaunchPlan,
) -> Result<deadreckon_protocol::AuthorityAcceptedBy> {
    match launch_plan.accepted_by.as_deref() {
        Some("operator") => Ok(deadreckon_protocol::AuthorityAcceptedBy::Operator),
        Some("yes-flag-guardrail") => {
            Ok(deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail)
        }
        Some(other) => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "approved guided launch plan has unsupported acceptance provenance {other}"
        )))),
        None => Err(CliError::Core(DeadreckonError::InvalidInput(
            "approved guided launch plan is missing acceptance provenance".to_string(),
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn direct_run_route(
    preview: bool,
    in_place: bool,
    explicitly_uncontained: bool,
    untrusted: bool,
    trusted_supervisor_child: bool,
    delegated_job_child: bool,
    private_characterization: bool,
    private_legacy_chain_foreground: bool,
) -> Result<DirectRunRoute> {
    if untrusted && (trusted_supervisor_child || delegated_job_child) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--untrusted cannot be used by a trusted supervisor or delegated Job child",
            "remove --untrusted; the owning Job controls containment and receipt authority",
        )));
    }
    if untrusted && !in_place && !explicitly_uncontained {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--untrusted is valid only with --in-place or explicit --sandbox none",
            "remove --untrusted to create a durable isolated Job",
        )));
    }
    if preview {
        return Ok(DirectRunRoute::Foreground);
    }
    if private_characterization
        && (in_place || explicitly_uncontained || private_legacy_chain_foreground)
    {
        return Ok(DirectRunRoute::Foreground);
    }
    if (in_place || explicitly_uncontained) && !untrusted {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--in-place and --sandbox none are untrusted foreground compatibility modes",
            "add --untrusted only if you accept that this Run cannot issue a trusted Job receipt",
        )));
    }
    if untrusted || trusted_supervisor_child || delegated_job_child {
        return Ok(DirectRunRoute::Foreground);
    }
    Ok(DirectRunRoute::DurableJob)
}

fn mark_untrusted_foreground(plan: &mut commands::course::LaunchPlan) {
    let mut signals = plan.signals.as_object().cloned().unwrap_or_default();
    signals.insert(
        UNTRUSTED_FOREGROUND_SIGNAL.to_string(),
        json!({
            "capability": "--untrusted",
            "execution": "foreground",
            "durable_job": false,
            "trusted_job_receipt": false,
            "reason": "in-place or uncontained foreground compatibility execution"
        }),
    );
    plan.signals = serde_json::Value::Object(signals);
}

async fn schedule_direct_run(
    mut args: RunCommandArgs,
    launch_plan: &mut commands::course::LaunchPlan,
    accepted_by_override: Option<deadreckon_protocol::AuthorityAcceptedBy>,
) -> Result<Option<deadreckon_protocol::JobId>> {
    if args.infer_contract {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--infer-contract requires an interactive foreground review before work starts",
            "deadreckon def-done \"what should count as done\", then rerun `deadreckon run`",
        )));
    }
    if args.smoke && args.provider.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --provider".to_string(),
        )));
    }
    if args.smoke && args.model.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --model".to_string(),
        )));
    }
    if let Err(message) = crate::narrator::validate_narration_flags(args.narrate, args.no_narrate) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
    }
    if args
        .prevent_sleep
        .as_deref()
        .is_some_and(|value| value != "off")
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "the durable supervisor owns process lifetime; direct jobs cannot freeze a child --prevent-sleep mode",
            "use `--prevent-sleep off`; install the supervisor user service when the job must outlive login sessions",
        )));
    }
    let explicitly_approved = direct_run_approval_policy(
        args.yes,
        args.no_confirm,
        args.quiet,
        prompt::is_tty(),
        &args.goal,
        args.no_hints,
    )?;
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    if let Some(model) = args.narrator_model.as_deref()
        && let Ok(registry) =
            deadreckon_providers::registry::ProviderRegistry::with_overrides(paths.home())
        && !crate::narrator::narrator_model_known(&registry, model)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            crate::narrator::narrator_model_refusal(model),
        )));
    }
    let cwd = args
        .durable_source_cwd
        .clone()
        .unwrap_or(std::env::current_dir()?);
    if args.init_git {
        init_git_repo(&cwd)?;
    }
    let explicit_mode =
        args.fresh || args.worktree || args.from.is_some() || args.in_place || args.init_git;
    if !explicit_mode && deadreckon_core::find_git_root(&cwd)?.is_none() {
        if !io::stdin().is_terminal() {
            return Err(run_codebase_refusal_error(
                deadreckon_core::user_error(
                    "non-interactive without a mode flag",
                    "--fresh or --from . or git init",
                ),
                &args.goal,
                args.no_hints,
            ));
        }
        match prompt_non_git_mode()? {
            NonGitChoice::Init => {
                init_git_repo(&cwd)?;
                args.worktree = true;
            }
            NonGitChoice::Copy => {
                args.from = Some(cwd.clone());
            }
            NonGitChoice::Cancel => {
                print!(
                    "{}",
                    cancelled_run_surface().render_plain(!completion_hints_enabled(args.no_hints))
                );
                return Ok(None);
            }
        }
    }
    let source = direct_run_source(&cwd, &args)?;
    let authority_source_cwd = source.from.clone().unwrap_or_else(|| cwd.clone());
    let durable_job_cwd = if args.durable_source_cwd.is_some() {
        cwd.clone()
    } else {
        authority_source_cwd.clone()
    };
    let trusted_contract_dirty_paths = bounded_existing_project_contract_dirty_paths(
        &authority_source_cwd,
        args.acceptance.as_deref(),
        args.allow_dirty,
    )?;
    if matches!(source.mode, commands::job::DurableSourceMode::Worktree) {
        prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: Uuid::new_v4().simple().to_string(),
                task_key: deadreckon_core::paths::task_key(&args.goal),
                source_path: authority_source_cwd.clone(),
                base_ref: args.base.clone(),
                branch_name: args.branch.clone(),
                allow_dirty: args.allow_dirty,
                allowed_dirty_paths: trusted_contract_dirty_paths,
            },
        )
        .map_err(|error| run_codebase_refusal_error(error, &args.goal, args.no_hints))?;
    }
    let contract = commands::acceptance::ensure_acceptance_before_start(
        &authority_source_cwd,
        args.acceptance.as_deref(),
        &args.goal,
        args.provider.clone(),
        args.model.clone(),
        explicitly_approved,
        "run",
    )
    .await?;
    let max_spend_usd = args.max_spend.or(defaults.max_spend).unwrap_or(10.0);
    let max_wall_seconds = commands::job::checked_job_wall_seconds(
        args.max_wall_seconds
            .or(defaults.cli_max_wall_seconds)
            .unwrap_or(36_000.0),
    )?;
    let sandbox_requested = args
        .sandbox
        .clone()
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    if sandbox_requested == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable direct runs require a containment backend; sandbox `none` cannot produce a trusted receipt",
            "omit `--sandbox none` or choose auto, sandbox-exec, bwrap, or docker",
        )));
    }
    let provider = if args.smoke {
        Some("smoke".to_string())
    } else {
        args.provider.clone()
    };
    launch_plan.providers.coder.clone_from(&provider);
    launch_plan.pieces = vec![commands::course::CoursePiece {
        id: "run".to_string(),
        goal: args.goal.clone(),
        done_hint: None,
        role: Some("coder".to_string()),
        provider,
        model: args.model.clone(),
        budget_usd: Some(max_spend_usd),
        depends_on: Vec::new(),
        subplan: None,
    }];
    launch_plan.budget.ceiling_usd = Some(max_spend_usd);
    launch_plan.budget.wall_seconds = Some(max_wall_seconds);
    launch_plan.budget.deadline = args.deadline;
    let leaf = DurableLeafSpec {
        base: args.base.clone(),
        branch: args.branch.clone(),
        no_seams: args.no_seams,
        doc_provider: args.doc_provider.clone(),
        skill: args.skill.clone(),
        no_docs: args.no_docs,
        doc_skill: args.doc_skill.clone(),
        narrate: args.narrate,
        no_narrate: args.no_narrate,
        narrator_model: args.narrator_model.clone(),
        continuation: args.continuation.clone(),
    };
    let mut signals = launch_plan.signals.as_object().cloned().unwrap_or_default();
    signals.insert(DURABLE_LEAF_SIGNAL.to_string(), serde_json::to_value(leaf)?);
    launch_plan.signals = serde_json::Value::Object(signals);

    let accepted_by = if let Some(accepted_by) = accepted_by_override {
        accepted_by
    } else if explicitly_approved {
        deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
    } else {
        if !prompt::confirm("queue this durable job?", true)? {
            eprintln!("cancelled before job creation");
            return Ok(None);
        }
        deadreckon_protocol::AuthorityAcceptedBy::Operator
    };
    let job = persist_direct_run_job(
        &paths,
        &durable_job_cwd,
        launch_plan.clone(),
        contract.as_ref().map(|source| source.path.as_path()),
        source,
        max_spend_usd,
        max_wall_seconds,
        args.deadline,
        sandbox_requested,
        accepted_by,
    )?;
    commands::job::launch_detached_supervisor(&paths, &job.job_id)?;
    if !args.quiet {
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref())?;
        commands::job::print_job_status(&view, false)?;
    }
    Ok(Some(job.job_id))
}

fn direct_run_approval_policy(
    yes: bool,
    no_confirm: bool,
    _quiet: bool,
    is_tty: bool,
    goal: &str,
    no_hints: bool,
) -> Result<bool> {
    if yes || no_confirm {
        return Ok(true);
    }
    if !is_tty {
        return Err(durable_run_confirmation_refusal_error(goal, no_hints));
    }
    Ok(false)
}

fn direct_run_source(cwd: &Path, args: &RunCommandArgs) -> Result<commands::job::DurableSource> {
    let (mode, from) = if args.fresh {
        (commands::job::DurableSourceMode::Fresh, None)
    } else if let Some(from) = args.from.as_ref() {
        (
            commands::job::DurableSourceMode::Copy,
            Some(resolve_direct_source_path(cwd, from)),
        )
    } else if args.init_git || args.worktree || deadreckon_core::find_git_root(cwd)?.is_some() {
        (commands::job::DurableSourceMode::Worktree, None)
    } else {
        (
            commands::job::DurableSourceMode::Copy,
            Some(cwd.to_path_buf()),
        )
    };
    Ok(commands::job::DurableSource {
        mode,
        from,
        allow_dirty: args.allow_dirty,
    })
}

fn resolve_direct_source_path(cwd: &Path, requested: &Path) -> PathBuf {
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        cwd.join(requested)
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_direct_run_job(
    paths: &DeadreckonPaths,
    cwd: &Path,
    launch_plan: commands::course::LaunchPlan,
    contract_source: Option<&Path>,
    source: commands::job::DurableSource,
    max_spend_usd: f64,
    max_wall_seconds: u64,
    deadline: Option<DateTime<Utc>>,
    sandbox_requested: String,
    accepted_by: deadreckon_protocol::AuthorityAcceptedBy,
) -> Result<deadreckon_protocol::Job> {
    commands::job::create_job(commands::job::CreateJob {
        paths,
        source_cwd: cwd,
        scope: workspace_scope(cwd)?,
        launch_plan,
        shape: deadreckon_protocol::JobShape::Single,
        driver: None,
        contract_source,
        source,
        max_spend_usd,
        max_wall_seconds,
        max_attempts: 3,
        deadline,
        sandbox_requested,
        accepted_by,
    })
}

/// Launch a run carrying an accepted launch plan (C-P9): the plan is saved
/// into the run root right after the run is created, so attach, verdict, and
/// replay can read the decision from the artifact alone.
pub(crate) async fn run_command_with_launch_plan(
    args: RunCommandArgs,
    launch_plan: commands::course::LaunchPlan,
) -> Result<()> {
    let RunCommandArgs {
        goal,
        run_id: requested_run_id,
        durable_source_cwd: _,
        continuation: _,
        tamper_baseline,
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
        deadline,
        sandbox,
        untrusted,
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
        infer_contract,
    } = args;
    let requested_run_id = trusted_supervisor_run_id(requested_run_id)?;
    if untrusted {
        eprintln!(
            "UNTRUSTED FOREGROUND RUN: no durable Job is created and this Run cannot issue a trusted Job receipt."
        );
    }
    if let Err(message) = crate::narrator::validate_narration_flags(narrate, no_narrate) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
    }
    let auto_confirm =
        foreground_run_auto_confirm(yes, no_confirm, quiet, requested_run_id.is_some());
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
    let trusted_contract_dirty_paths =
        match (requested_run_id.as_deref(), acceptance_source.as_ref()) {
            (Some(run_id), Some(source)) if !allow_dirty => {
                trusted_controller_contract_dirty_paths(&paths, run_id, &cwd, source, &launch_plan)?
            }
            (None, Some(source)) if !allow_dirty => {
                bounded_project_contract_dirty_paths(&cwd, source)?
            }
            _ => Vec::new(),
        };
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
    let run_id = requested_run_id
        .or_else(commands::graph_job::delegated_plan_child_run_id)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
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
                allowed_dirty_paths: trusted_contract_dirty_paths,
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
        deadline: deadline.as_ref(),
        acceptance: &acceptance_preview,
        sleep: &sleep_preview,
        seams: &seams_label,
        brief,
        plain,
        run_id: &run_id,
    });
    // Detected-but-unrunnable trees (a JS/Python project we can't resolve a test
    // command for) refuse with a `try:` footer rather than silently running a
    // hollow gate — unless the operator gave --acceptance or opted into
    // --infer-contract.
    if !preview
        && acceptance.is_none()
        && !infer_contract
        && let Some((reason, try_hint)) = commands::detect::unrunnable_refusal(&cwd, "run")
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &reason, &try_hint,
        )));
    }
    let contract_source = if acceptance.is_some() {
        deadreckon_core::acceptance_defaults::ContractSource::Operator
    } else {
        deadreckon_core::acceptance_defaults::ContractSource::Detected
    };
    if preview {
        eprintln!("{preview_text}");
        if !brief {
            eprintln!("{}", commands::detect::detect_report(&cwd));
        }
        return Ok(());
    }
    if !quiet {
        eprintln!("{preview_text}");
        if !brief {
            eprintln!(
                "{}",
                commands::detect::contract_preview_line(&cwd, contract_source)
            );
            if let Some(caveat) = commands::detect::contract_caveat(&cwd) {
                eprintln!("caveat: {caveat}");
            }
        }
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
    let run_options = RunOptions {
        goal,
        cwd,
        sandbox: backend.to_string(),
        provider: effective_provider.clone(),
        skill_name: skill,
        max_spend_usd: effective_max_spend,
        max_wall_seconds: effective_max_wall_seconds,
        run_id: Some(run_id),
        codebase: Some(codebase.clone()),
    };
    let mut state = if let Some(ownership) = commands::graph_job::delegated_plan_child_ownership() {
        deadreckon_core::create_owned_run(&paths, run_options, ownership)?
    } else {
        create_run(&paths, run_options)?
    };
    commands::graph_job::link_delegated_owned_run(&state)?;
    if untrusted {
        commands::course::save_launch_plan(
            &commands::course::launch_plan_path(&state.run_root),
            &launch_plan,
        )?;
    } else {
        commands::course::save_launch_plan_best_effort(&state.run_root, &launch_plan);
    }
    if let Some(baseline) = tamper_baseline.as_deref() {
        deadreckon_core::tamper::write_tamper_baseline(&state.run_root, baseline)
            .map_err(CliError::Core)?;
    }
    if let Some(source_path) = codebase
        .source_path
        .as_ref()
        .filter(|_| codebase.mode == CodebaseMode::Copy)
    {
        let capture_policy = freeze_workspace_capture_policy(source_path)?;
        let hydration =
            copy_source_to_working_with_policy(source_path, &state.working_dir, &capture_policy)?;
        write_workspace_capture_policy(&state.run_root, &capture_policy)?;
        write_capture_manifest(
            &state.run_root.join(SOURCE_HYDRATION_MANIFEST_JSON),
            &hydration,
        )?;
        deadreckon_core::write_codebase_record(&state.working_dir, &codebase)?;
    }
    commands::acceptance::copy_acceptance_into_run(&state, &acceptance_source)?;
    if let Some(continuation) = durable_leaf_spec(&launch_plan)?.and_then(|leaf| leaf.continuation)
    {
        commands::lifecycle::prepare_durable_continuation(&paths, &state, &continuation)?;
    }
    // Freeze ignore, tracked-file, ecosystem-output, and capture-budget policy
    // before the first implementation provider can mutate the workspace. Copy
    // mode already persisted the source repository's policy above.
    deadreckon_core::ensure_workspace_capture_policy(&state)?;
    maybe_infer_contract(
        &paths,
        &state,
        infer_contract,
        auto_confirm,
        quiet,
        provider.as_deref(),
    )
    .await?;
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
    // Live narrator. A spawned orchestrate/campaign child (NARRATE_CHILD_ENV set)
    // narrates FILE-ONLY and defaults to the deterministic floor unless a model
    // was pinned; an interactive `dr run` keeps the TTY-default foreground block.
    // Off-TTY with no --narrate, resolve returns None and the run is wired as
    // before. A smoke run forces the floor so tests stay hermetic.
    let is_narrate_child = std::env::var_os(crate::narrator::NARRATE_CHILD_ENV).is_some();
    let narrator_config = crate::narrator::resolve_narration(
        is_narrate_child,
        io::stdin().is_terminal(),
        narrate,
        no_narrate,
        narrator_model,
    );
    let force_floor = smoke
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
        force_floor,
        narrator_config.clone(),
    );
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

fn foreground_run_auto_confirm(
    yes: bool,
    no_confirm: bool,
    _quiet: bool,
    trusted_supervisor_child: bool,
) -> bool {
    yes || no_confirm || trusted_supervisor_child
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

fn durable_run_confirmation_refusal_error(goal: &str, no_hints: bool) -> CliError {
    let primary = format!("deadreckon run {} --yes", run_goal_argument(goal));
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "run",
            None,
            ExplanationPanel::new(
                "non-interactive without --yes",
                "A durable direct run needs explicit approval before its immutable Job is queued. DeadReckon refused before creating Job or run state because this shell cannot answer the confirmation prompt.",
                [
                    ("command".to_string(), "run".to_string()),
                    ("goal".to_string(), goal.to_string()),
                    ("Job".to_string(), "not created".to_string()),
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
            format!("deadreckon run {quoted_goal} --in-place --i-know-its-a-lot --untrusted --yes"),
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
        RunEventKind::RunCompleted {
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

/// `--infer-contract` preflight: for an unknown project tree with no operator
/// `acceptance.yaml`, a cheap model proposes a test command the operator must
/// approve before it arms the gate. A no-op unless opted in, Unknown, and on an
/// interactive surface — a model proposal never defines "done" unattended.
async fn maybe_infer_contract(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    infer_flag: bool,
    yes: bool,
    quiet: bool,
    provider_arg: Option<&str>,
) -> Result<()> {
    use crate::commands::infer_contract::{
        InferenceOutcome, arm_inferred_contract, infer_contract_eligible, propose_contract,
        resolve_inferred_contract,
    };
    if !infer_flag {
        return Ok(());
    }
    let spec_path = deadreckon_core::gate::acceptance_spec_path_for_run_root(&state.run_root);
    if spec_path.exists() {
        return Ok(());
    }
    let kind = deadreckon_core::acceptance_defaults::detect_project_kind(&state.working_dir);
    let kind_unknown = matches!(
        kind,
        deadreckon_core::acceptance_defaults::ProjectKind::Unknown
    );
    let is_tty = io::stdin().is_terminal();
    let eligible = infer_contract_eligible(infer_flag, kind_unknown, yes, quiet, false, is_tty);
    if !eligible {
        return Ok(());
    }
    let defaults = config_defaults(paths)?;
    let Some(provider) = commands::start::goal_shape_provider_route(paths, &defaults, provider_arg)
    else {
        return Ok(());
    };
    let config_path = paths.config_path();
    let proposal = propose_contract(&config_path, &provider, &state.working_dir).await;
    let outcome = resolve_inferred_contract(
        eligible,
        || proposal,
        |proposal| {
            eprintln!("\nInferred done-contract (no acceptance.yaml, unknown project tree):");
            eprintln!("  command:    {}", proposal.command);
            if !proposal.test_globs.is_empty() {
                eprintln!("  test files: {}", proposal.test_globs.join(", "));
            }
            eprintln!(
                "  rationale:  {} (confidence {:.0}%)",
                proposal.rationale,
                proposal.confidence * 100.0
            );
            crate::prompt::confirm("Approve this contract to gate the run?", false).unwrap_or(false)
        },
    );
    match outcome {
        InferenceOutcome::Approved(proposal) => {
            arm_inferred_contract(
                &spec_path,
                &state.working_dir,
                &proposal,
                &provider,
                chrono::Utc::now(),
            )
            .map_err(|source| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "failed to write inferred contract: {source}"
                )))
            })?;
            if !quiet {
                eprintln!(
                    "Inferred contract armed; the gate will run: {}",
                    proposal.command
                );
            }
        }
        InferenceOutcome::NoProvider if !quiet => {
            eprintln!("No usable inference; continuing with a no-test-contract caveat.");
        }
        _ => {}
    }
    Ok(())
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

#[cfg(test)]
mod durable_direct_tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let output = deadreckon_core::git::run_git(repo, args).expect("git command");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn trusted_contract_fixture(
        temp: &TempDir,
    ) -> (
        DeadreckonPaths,
        PathBuf,
        String,
        commands::acceptance::AcceptanceSource,
        commands::course::LaunchPlan,
    ) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        fs::write(repo.join("app.py"), "print('unfinished')\n").expect("app");
        git(&repo, &["add", "app.py"]);
        git(&repo, &["commit", "-m", "fixture"]);

        let contract =
            "name: guided\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/app.py'\n";
        let project_contract = repo.join(".deadreckon/acceptance.yaml");
        fs::create_dir_all(project_contract.parent().expect("contract parent"))
            .expect("contract directory");
        fs::write(&project_contract, contract).expect("project contract");
        fs::write(repo.join(".deadreckon/acceptance.md"), "# Done\n").expect("notes");

        let run_id = "0123456789abcdef0123456789abcdef".to_string();
        let frozen = commands::job::job_acceptance_path(&paths, &run_id);
        fs::create_dir_all(frozen.parent().expect("frozen parent")).expect("job directory");
        fs::write(&frozen, contract).expect("frozen contract");
        fs::write(
            frozen
                .parent()
                .expect("frozen parent")
                .join("acceptance.md"),
            "# Done\n",
        )
        .expect("frozen notes");
        let source = commands::acceptance::resolve_acceptance_source(&repo, Some(&frozen))
            .expect("acceptance resolution")
            .expect("acceptance source");
        let mut plan = commands::course::trivial_operator_plan(
            "guided fixture",
            commands::course::CourseShape::Single,
            "test",
        );
        set_contract_bundle_signal(
            &mut plan,
            frozen.parent().expect("frozen parent"),
            &["acceptance.yaml", "acceptance.md"],
        );
        (paths, repo, run_id, source, plan)
    }

    fn set_contract_bundle_signal(
        plan: &mut commands::course::LaunchPlan,
        frozen_dir: &Path,
        files: &[&str],
    ) {
        let mut files = files
            .iter()
            .map(|relative| {
                let path = frozen_dir.join(relative);
                commands::job::FrozenContractFile {
                    path: (*relative).to_string(),
                    sha256: deadreckon_core::flight::sha256_file(&path).expect("bundle digest"),
                    bytes: fs::metadata(path).expect("bundle metadata").len(),
                    executable: false,
                }
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        plan.signals[commands::job::DURABLE_CONTRACT_BUNDLE_SIGNAL] =
            serde_json::to_value(commands::job::FrozenContractBundle {
                schema_version: 1,
                files,
            })
            .expect("bundle signal");
    }

    #[test]
    fn project_contract_bundle_is_the_only_dirty_input_allowed_before_job_freeze() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, _, _) = trusted_contract_fixture(&temp);
        let helper_dir = repo.join(".deadreckon/acceptance");
        fs::create_dir_all(&helper_dir).expect("helper directory");
        fs::write(helper_dir.join("check output ☃.sh"), "#!/bin/sh\nexit 0\n")
            .expect("helper with a Git-quoted name");
        let source = commands::acceptance::resolve_acceptance_source(&repo, None)
            .expect("acceptance resolution")
            .expect("project contract");
        let allowed = bounded_project_contract_dirty_paths(&repo, &source)
            .expect("bounded project contract paths");
        assert_eq!(
            allowed,
            [
                PathBuf::from(".deadreckon/acceptance.md"),
                PathBuf::from(".deadreckon/acceptance.yaml"),
                PathBuf::from(".deadreckon/acceptance/check output ☃.sh"),
            ]
        );

        prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: run_id.clone(),
                task_key: "pre-freeze-contract".to_string(),
                source_path: repo.clone(),
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed.clone(),
            },
        )
        .expect("exact project contract remains admissible");

        fs::write(repo.join("unrelated.txt"), "operator work\n").expect("unrelated dirt");
        let error = prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id,
                task_key: "pre-freeze-contract".to_string(),
                source_path: repo,
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect_err("unrelated dirt remains rejected");
        assert!(error.to_string().contains("unrelated.txt"), "{error}");
    }

    #[test]
    fn external_contract_does_not_excuse_project_contract_dirt() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, _, _) = trusted_contract_fixture(&temp);
        let external = temp.path().join("external.yaml");
        fs::write(
            &external,
            "name: external\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/app.py'\n",
        )
        .expect("external contract");

        let allowed =
            bounded_existing_project_contract_dirty_paths(&repo, Some(external.as_path()), false)
                .expect("external contract resolution");
        assert!(allowed.is_empty());
        let error = prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id,
                task_key: "external-contract".to_string(),
                source_path: repo,
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect_err("an explicit external contract cannot bless project dirt");
        assert!(error.to_string().contains(".deadreckon"), "{error}");
    }

    #[test]
    fn nested_project_contract_maps_to_its_git_relative_paths() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        let project = repo.join("apps/native bird");
        fs::create_dir_all(&project).expect("nested project");
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        fs::write(project.join("app.py"), "print('unfinished')\n").expect("app");
        git(&repo, &["add", "apps/native bird/app.py"]);
        git(&repo, &["commit", "-m", "fixture"]);
        fs::create_dir_all(project.join(".deadreckon/acceptance")).expect("contract helper root");
        fs::write(
            project.join(".deadreckon/acceptance.yaml"),
            "name: nested\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/app.py'\n",
        )
        .expect("contract");
        fs::write(
            project.join(".deadreckon/acceptance/check output.sh"),
            "#!/bin/sh\nexit 0\n",
        )
        .expect("helper");

        let source = commands::acceptance::resolve_acceptance_source(&project, None)
            .expect("acceptance resolution")
            .expect("project contract");
        let allowed = bounded_project_contract_dirty_paths(&project, &source)
            .expect("bounded project contract paths");
        assert_eq!(
            allowed,
            [
                PathBuf::from("apps/native bird/.deadreckon/acceptance.yaml"),
                PathBuf::from("apps/native bird/.deadreckon/acceptance/check output.sh"),
            ]
        );
        prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: "fedcba9876543210fedcba9876543210".to_string(),
                task_key: "nested-contract".to_string(),
                source_path: project,
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect("nested exact bundle remains admissible");
    }

    #[test]
    fn trusted_guided_contract_files_do_not_dirty_the_child_worktree() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, source, plan) = trusted_contract_fixture(&temp);
        let allowed =
            trusted_controller_contract_dirty_paths(&paths, &run_id, &repo, &source, &plan)
                .expect("trusted contract transition");
        assert_eq!(allowed.len(), 2);

        let record = prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id,
                task_key: "guided-contract".to_string(),
                source_path: repo.clone(),
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect("controller-owned contract remains admissible");
        assert!(!record.dirty_files_seeded);
        create_worktree(&record).expect("isolated worktree");
        let worktree = record.worktree_path.expect("worktree path");
        assert!(worktree.join("app.py").is_file());
        assert!(!worktree.join(".deadreckon/acceptance.yaml").exists());
    }

    #[test]
    fn trusted_guided_contract_rejects_unrelated_dirt_at_worktree_admission() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, source, plan) = trusted_contract_fixture(&temp);
        let allowed =
            trusted_controller_contract_dirty_paths(&paths, &run_id, &repo, &source, &plan)
                .expect("trusted contract transition");

        fs::write(repo.join("unrelated.txt"), "operator change\n").expect("unrelated dirty file");
        let error = prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id,
                task_key: "guided-contract".to_string(),
                source_path: repo,
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect_err("unrelated dirtiness remains rejected");
        assert!(error.to_string().contains("unrelated.txt"), "{error}");
    }

    #[test]
    fn trusted_guided_contract_hydrates_and_executes_frozen_helpers() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, _, mut plan) = trusted_contract_fixture(&temp);
        let helper = repo.join(".deadreckon/acceptance/check.sh");
        fs::create_dir_all(helper.parent().expect("helper parent")).expect("helper directory");
        fs::write(&helper, "#!/bin/sh\nprintf helper-ok\n").expect("helper");
        let frozen_helper = paths.job_dir(&run_id).join("acceptance/check.sh");
        fs::create_dir_all(frozen_helper.parent().expect("frozen helper parent"))
            .expect("frozen helper directory");
        fs::copy(&helper, &frozen_helper).expect("freeze helper");
        set_contract_bundle_signal(
            &mut plan,
            &paths.job_dir(&run_id),
            &["acceptance.yaml", "acceptance.md", "acceptance/check.sh"],
        );
        let source = commands::acceptance::resolve_acceptance_source(
            &repo,
            Some(&commands::job::job_acceptance_path(&paths, &run_id)),
        )
        .expect("acceptance resolution")
        .expect("acceptance source");
        let allowed =
            trusted_controller_contract_dirty_paths(&paths, &run_id, &repo, &source, &plan)
                .expect("trusted helper transition");
        assert!(
            allowed
                .iter()
                .any(|path| path.ends_with(".deadreckon/acceptance/check.sh"))
        );

        let record = prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: run_id.clone(),
                task_key: "guided-helper".to_string(),
                source_path: repo.clone(),
                base_ref: None,
                branch_name: None,
                allow_dirty: false,
                allowed_dirty_paths: allowed,
            },
        )
        .expect("helper-backed contract remains admissible");
        create_worktree(&record).expect("isolated worktree");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "guided fixture".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some(run_id),
                codebase: Some(record),
            },
        )
        .expect("run state");
        commands::acceptance::copy_acceptance_into_run(&state, &Some(source))
            .expect("hydrate frozen bundle");
        let output = Command::new("sh")
            .arg(state.working_dir.join(".deadreckon/acceptance/check.sh"))
            .output()
            .expect("execute helper");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "helper-ok");
    }

    #[test]
    fn trusted_guided_contract_requires_the_frozen_yaml_identity() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, repo, run_id, source, plan) = trusted_contract_fixture(&temp);
        fs::write(
            repo.join(".deadreckon/acceptance.yaml"),
            "name: tampered\nchecks: []\n",
        )
        .expect("tampered project contract");

        assert!(
            trusted_controller_contract_dirty_paths(&paths, &run_id, &repo, &source, &plan)
                .expect("mismatched contract is rejected")
                .is_empty()
        );
    }

    #[test]
    fn guided_continuation_preserves_approved_authority_provenance() {
        let mut plan = commands::course::trivial_operator_plan(
            "continue the approved result",
            commands::course::CourseShape::ChainExtend,
            "guided start",
        );

        assert_eq!(
            approved_launch_authority(&plan).expect("interactive approval"),
            deadreckon_protocol::AuthorityAcceptedBy::Operator
        );

        plan.accepted_by = Some("yes-flag-guardrail".to_string());
        assert_eq!(
            approved_launch_authority(&plan).expect("yes guardrail approval"),
            deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
        );

        plan.accepted_by = Some("replay".to_string());
        assert!(
            approved_launch_authority(&plan)
                .expect_err("unsupported approval provenance")
                .to_string()
                .contains("unsupported acceptance provenance")
        );
    }

    #[test]
    fn direct_run_persists_one_bounded_job_with_the_same_run_identity() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable direct run").expect("source file");
        let contract = source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: direct fixture\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/README.md'\n    must_pass: true\n",
        )
        .expect("contract");
        let mut plan = commands::course::trivial_operator_plan(
            "finish the direct task",
            commands::course::CourseShape::Single,
            "run",
        );
        let leaf = DurableLeafSpec {
            base: Some("main".to_string()),
            branch: Some("deadreckon/direct".to_string()),
            no_seams: true,
            doc_provider: Some("doc-provider".to_string()),
            skill: "coder".to_string(),
            no_docs: true,
            doc_skill: Some("documenter".to_string()),
            narrate: false,
            no_narrate: true,
            narrator_model: None,
            continuation: None,
        };
        plan.signals = json!({ DURABLE_LEAF_SIGNAL: leaf });
        let deadline = Utc::now() + chrono::TimeDelta::hours(2);

        let job = persist_direct_run_job(
            &paths,
            &source,
            plan,
            Some(&contract),
            commands::job::DurableSource {
                mode: commands::job::DurableSourceMode::Copy,
                from: Some(source.clone()),
                allow_dirty: false,
            },
            7.5,
            240,
            Some(deadline),
            "auto".to_string(),
            deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail,
        )
        .expect("persist direct job");
        let authority: deadreckon_protocol::JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("launch plan");

        assert_eq!(job.shape, deadreckon_protocol::JobShape::Single);
        assert_eq!(job.job_id.as_ref(), authority.run_id.as_ref());
        assert_eq!(job.policy.max_attempts, 3);
        assert_eq!(job.policy.max_spend_usd, 7.5);
        assert_eq!(job.policy.max_wall_seconds, 240);
        assert_eq!(job.policy.deadline, Some(deadline));
        assert_eq!(frozen.budget.deadline, Some(deadline));
        assert_eq!(
            authority.accepted_by,
            deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
        );
        assert_eq!(
            durable_leaf_spec(&frozen)
                .expect("leaf spec")
                .expect("leaf"),
            DurableLeafSpec {
                base: Some("main".to_string()),
                branch: Some("deadreckon/direct".to_string()),
                no_seams: true,
                doc_provider: Some("doc-provider".to_string()),
                skill: "coder".to_string(),
                no_docs: true,
                doc_skill: Some("documenter".to_string()),
                narrate: false,
                no_narrate: true,
                narrator_model: None,
                continuation: None,
            }
        );
    }

    #[test]
    fn relative_copy_source_is_resolved_against_the_operator_checkout() {
        let cwd = Path::new("/operator/project");
        assert_eq!(
            resolve_direct_source_path(cwd, Path::new("../source")),
            PathBuf::from("/operator/project/../source")
        );
        assert_eq!(
            resolve_direct_source_path(cwd, Path::new("/absolute/source")),
            PathBuf::from("/absolute/source")
        );
    }

    #[test]
    fn quiet_non_tty_run_is_not_operator_approval() {
        let error = direct_run_approval_policy(false, false, true, false, "quiet job", false)
            .expect_err("quiet cannot approve a job");
        assert!(error.to_string().contains("needs explicit approval"));
        assert!(
            direct_run_approval_policy(true, false, true, false, "quiet job", false)
                .expect("yes approves")
        );
    }

    #[test]
    fn quiet_does_not_approve_foreground_execution_but_trusted_child_does() {
        assert!(!foreground_run_auto_confirm(false, false, true, false));
        assert!(foreground_run_auto_confirm(false, false, true, true));
    }

    #[test]
    fn ordinary_non_preview_run_routes_to_a_durable_job() {
        assert_eq!(
            direct_run_route(false, false, false, false, false, false, false, false)
                .expect("ordinary route"),
            DirectRunRoute::DurableJob
        );
    }

    #[test]
    fn untrusted_capability_is_narrow_and_never_valid_for_job_children() {
        assert_eq!(
            direct_run_route(false, true, false, true, false, false, false, false)
                .expect("explicit in-place capability"),
            DirectRunRoute::Foreground
        );
        assert_eq!(
            direct_run_route(false, false, true, true, false, false, false, false)
                .expect("explicit uncontained capability"),
            DirectRunRoute::Foreground
        );
        let irrelevant = direct_run_route(false, false, false, true, false, false, false, false)
            .expect_err("capability without unsafe mode");
        assert!(irrelevant.to_string().contains("valid only"));
        for (supervisor, delegated) in [(true, false), (false, true)] {
            let error = direct_run_route(
                false, true, false, true, supervisor, delegated, false, false,
            )
            .expect_err("Job children cannot become untrusted");
            assert!(
                error
                    .to_string()
                    .contains("trusted supervisor or delegated Job child")
            );
        }
    }

    #[test]
    fn unsafe_public_runs_refuse_but_read_only_preview_and_private_harness_survive() {
        for (in_place, uncontained) in [(true, false), (false, true), (true, true)] {
            let error = direct_run_route(
                false,
                in_place,
                uncontained,
                false,
                false,
                false,
                false,
                false,
            )
            .expect_err("unsafe public execution needs the capability");
            assert!(error.to_string().contains("untrusted foreground"));
            assert_eq!(
                direct_run_route(
                    true,
                    in_place,
                    uncontained,
                    false,
                    false,
                    false,
                    false,
                    false,
                )
                .expect("preview is read-only"),
                DirectRunRoute::Foreground
            );
            assert_eq!(
                direct_run_route(
                    false,
                    in_place,
                    uncontained,
                    false,
                    false,
                    false,
                    true,
                    false,
                )
                .expect("private characterization route"),
                DirectRunRoute::Foreground
            );
        }
        assert_eq!(
            direct_run_route(false, false, false, false, false, false, true, true)
                .expect("private legacy chain step"),
            DirectRunRoute::Foreground
        );
        assert_eq!(
            direct_run_route(false, false, false, false, false, false, false, true)
                .expect("public binary ignores forged legacy signal"),
            DirectRunRoute::DurableJob
        );
    }

    #[test]
    fn untrusted_plan_signal_denies_durable_job_and_receipt_claims() {
        let mut plan = commands::course::trivial_operator_plan(
            "unsafe compatibility work",
            commands::course::CourseShape::Single,
            "run",
        );
        mark_untrusted_foreground(&mut plan);

        assert_eq!(
            plan.signals[UNTRUSTED_FOREGROUND_SIGNAL]["execution"],
            "foreground"
        );
        assert_eq!(
            plan.signals[UNTRUSTED_FOREGROUND_SIGNAL]["durable_job"],
            false
        );
        assert_eq!(
            plan.signals[UNTRUSTED_FOREGROUND_SIGNAL]["trusted_job_receipt"],
            false
        );
    }
}
