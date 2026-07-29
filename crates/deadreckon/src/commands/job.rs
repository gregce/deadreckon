//! Durable job creation and detached supervisor launch.
//!
//! `start` resolves and approves the mutable inputs. This module freezes those
//! inputs before the first agent turn, writes the initial append-only control
//! facts, and only then starts a supervisor.

use super::super::*;

use std::fs::OpenOptions;
use std::process::{Command, Stdio};

use chrono::Utc;
use deadreckon_protocol::{
    AuthorityAcceptedBy, Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence, JobId,
    JobPolicy, JobSchemaVersion, JobShape, RunId, SemanticJudgeMode, StopReason,
};

const JOB_ACCEPTANCE_FILE: &str = "acceptance.yaml";
const SUPERVISOR_LAUNCH_STDOUT: &str = "supervisor.out";
const SUPERVISOR_LAUNCH_STDERR: &str = "supervisor.err";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DurableSourceMode {
    Worktree,
    Copy,
    Fresh,
    InitGit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DurableSource {
    pub(crate) mode: DurableSourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<PathBuf>,
    #[serde(default)]
    pub(crate) allow_dirty: bool,
}

pub(crate) struct CreateJob<'a> {
    pub(crate) paths: &'a DeadreckonPaths,
    pub(crate) source_cwd: &'a Path,
    pub(crate) scope: String,
    pub(crate) launch_plan: commands::course::LaunchPlan,
    pub(crate) shape: JobShape,
    pub(crate) driver: Option<commands::graph_job::DriverSpec>,
    pub(crate) contract_source: Option<&'a Path>,
    pub(crate) source: DurableSource,
    pub(crate) max_spend_usd: f64,
    pub(crate) max_wall_seconds: u64,
    pub(crate) max_attempts: u32,
    pub(crate) sandbox_requested: String,
    pub(crate) accepted_by: AuthorityAcceptedBy,
}

pub(crate) fn create_job(mut request: CreateJob<'_>) -> Result<Job> {
    let expected_shape = match request.launch_plan.shape {
        commands::course::CourseShape::Single => JobShape::Single,
        commands::course::CourseShape::Plan => JobShape::Graph,
        commands::course::CourseShape::Campaign => JobShape::LegacyCampaign,
        commands::course::CourseShape::ChainExtend => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "legacy chain jobs remain process-bound; guided ordered work must use a durable graph plan until chain hooks, apply, undo, and child adoption share the job event history".to_string(),
            )));
        }
    };
    if request.shape != expected_shape {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable job shape does not match its launch plan".to_string(),
        )));
    }
    if let Some(driver) = request.driver.as_ref() {
        commands::graph_job::embed_driver_spec(&mut request.launch_plan, driver)?;
    } else if request.shape != JobShape::Single {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced durable jobs require an immutable driver specification".to_string(),
        )));
    }
    let job_id = JobId(Uuid::new_v4().simple().to_string());
    let job_dir = request.paths.job_dir(job_id.as_ref());
    fs::create_dir_all(&job_dir)?;

    let mut signals = request
        .launch_plan
        .signals
        .as_object()
        .cloned()
        .unwrap_or_default();
    signals.insert(
        "watchkeeper_source".to_string(),
        serde_json::to_value(&request.source)?,
    );
    request.launch_plan.signals = serde_json::Value::Object(signals);

    let launch_path = request.paths.job_launch_plan(job_id.as_ref());
    commands::course::save_launch_plan(&launch_path, &request.launch_plan)?;
    sync_file(&launch_path)?;
    let launch_plan_sha256 = deadreckon_core::flight::sha256_file(&launch_path)?;

    let contract_path = job_dir.join(JOB_ACCEPTANCE_FILE);
    freeze_contract(request.contract_source, request.source_cwd, &contract_path)?;
    let contract_sha256 = deadreckon_core::flight::sha256_file(&contract_path)?;

    let policy = JobPolicy {
        max_spend_usd: request.max_spend_usd,
        max_wall_seconds: request.max_wall_seconds,
        max_attempts: request.max_attempts.max(1),
        deadline: None,
        semantic_judge: SemanticJudgeMode::Required,
    };
    let effective_policy_sha256 = deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&policy).map_err(|source| DeadreckonError::Json {
            path: job_dir.join("policy"),
            source,
        })?,
    );
    let source_tree_sha256 =
        deadreckon_core::flight::build_working_file_index(request.source_cwd)?.tree_hash();
    let authority = JobAuthority {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        run_id: RunId(job_id.as_ref().to_string()),
        approved_at: Utc::now(),
        accepted_by: request.accepted_by,
        goal_sha256: deadreckon_core::flight::sha256_text(&request.launch_plan.goal),
        contract_sha256,
        effective_policy_sha256,
        launch_plan_sha256: launch_plan_sha256.clone(),
        source_tree_sha256,
        source_revision: git_revision(request.source_cwd),
        sandbox_requested: request.sandbox_requested,
        semantic_judge_mode: SemanticJudgeMode::Required,
    };
    let authority_path = request.paths.job_authority(job_id.as_ref());
    write_json_synced(&authority_path, &authority)?;
    let authority_sha256 = deadreckon_core::flight::sha256_file(&authority_path)?;

    let job = Job {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        scope: request.scope,
        goal: request.launch_plan.goal,
        shape: request.shape,
        created_at: Utc::now(),
        source_cwd: request.source_cwd.to_path_buf(),
        launch_plan_sha256,
        authority_sha256,
        policy,
    };
    deadreckon_core::write_job(request.paths, &job)?;
    for (sequence, kind, detail) in [
        (
            1,
            JobEventKind::Created,
            json!({ "shape": request.shape, "root_id": job_id.as_ref() }),
        ),
        (
            2,
            JobEventKind::ContractApproved,
            json!({
                "contract_sha256": authority.contract_sha256,
                "accepted_by": authority.accepted_by,
            }),
        ),
        (
            3,
            JobEventKind::Queued,
            json!({ "reason": "approved inputs frozen before first agent turn" }),
        ),
    ] {
        let sequence = JobEventSequence::new(sequence).ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "initial job event sequence must be non-zero".to_string(),
            ))
        })?;
        deadreckon_core::append_job_event(
            request.paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job_id.clone(),
                sequence,
                event_id: Uuid::new_v4().to_string(),
                causation_id: format!("start:{}", job_id.as_ref()),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind,
                detail,
            },
        )?;
    }
    Ok(job)
}

pub(crate) fn launch_detached_supervisor(paths: &DeadreckonPaths, job_id: &JobId) -> Result<()> {
    let executable = std::env::current_exe()?;
    let job_dir = paths.job_dir(job_id.as_ref());
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_LAUNCH_STDOUT))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_LAUNCH_STDERR))?;
    let mut command = Command::new(executable);
    command
        .arg("supervisor")
        .arg("serve")
        .arg("--once")
        .arg(job_id.as_ref())
        .env("DEADRECKON_HOME", paths.home())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let (child, _terminator) = deadreckon_core::spawn_grouped(command)?;
    drop(child);
    Ok(())
}

pub(crate) fn job_acceptance_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(JOB_ACCEPTANCE_FILE)
}

pub(crate) fn list_jobs(
    paths: &DeadreckonPaths,
    scope: Option<&str>,
) -> Result<Vec<deadreckon_core::JobView>> {
    let entries = match fs::read_dir(paths.jobs_dir()) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(source.into()),
    };
    let mut jobs = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(ToString::to_string))
        .filter_map(|id| deadreckon_core::JobView::load(paths, &id).ok())
        .filter(|view| scope.is_none_or(|scope| view.job.scope == scope))
        .collect::<Vec<_>>();
    jobs.sort_by_key(|view| view.projection.updated_at.unwrap_or(view.job.created_at));
    Ok(jobs)
}

pub(crate) fn job_status_label(view: &deadreckon_core::JobView) -> String {
    view.projection
        .outcome
        .map(serialized_label)
        .unwrap_or_else(|| serialized_label(view.projection.phase))
}

pub(crate) fn print_job_status(view: &deadreckon_core::JobView, json_output: bool) -> Result<()> {
    print_job_status_with_open_action(view, json_output, "attach")
}

pub(crate) fn print_job_status_after_attach(
    view: &deadreckon_core::JobView,
    json_output: bool,
) -> Result<()> {
    print_job_status_with_open_action(view, json_output, "status")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobDeliveryKind {
    Applied,
    Exported,
}

/// Record the successful operator delivery transition after it happens.
///
/// The event is idempotent for the same kind, destination and resulting
/// revision. It intentionally does not authorize delivery: `finish` validates
/// the sealed receipt first and calls this only after apply/export succeeds.
pub(crate) fn record_job_delivery(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: JobDeliveryKind,
    destination: &Path,
    resulting_revision: Option<&str>,
) -> Result<()> {
    let event_kind = match kind {
        JobDeliveryKind::Applied => JobEventKind::ResultApplied,
        JobDeliveryKind::Exported => JobEventKind::ResultExported,
    };
    let destination = destination.to_path_buf();
    let detail = json!({
        "destination": destination,
        "resulting_revision": resulting_revision,
    });
    let event_fingerprint = deadreckon_core::flight::sha256_text(&format!(
        "{}:{}",
        serialized_label(event_kind),
        serde_json::to_string(&detail)?
    ));
    let event_id = format!("finish-delivery:{event_fingerprint}");
    let mut last_error = None;
    // A duplicate finish may race after both callers successfully delivered
    // the same result. The append API deliberately rejects stale sequences, so
    // reload and retry a small bounded number of times. If the peer appended
    // the same factual event, this call is already complete.
    for _ in 0..4 {
        let view = deadreckon_core::JobView::load(paths, job_id)?;
        if view.projection.outcome != Some(deadreckon_protocol::JobOutcome::Verified) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot record delivery for unverified job {job_id}"
            ))));
        }
        let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
        if history
            .events()
            .iter()
            .any(|event| event.kind == event_kind && event.detail == detail)
        {
            return Ok(());
        }
        let sequence =
            JobEventSequence::new(view.projection.last_sequence + 1).ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "job {job_id} event sequence exhausted"
                )))
            })?;
        match deadreckon_core::append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: view.job.job_id,
                sequence,
                event_id: event_id.clone(),
                causation_id: format!("finish:{job_id}"),
                timestamp: Utc::now(),
                lease_epoch: view.projection.current_lease_epoch,
                kind: event_kind,
                detail: detail.clone(),
            },
        ) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(CliError::Core(last_error.unwrap_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "could not record delivery for job {job_id} after bounded retries"
        ))
    })))
}

fn print_job_status_with_open_action(
    view: &deadreckon_core::JobView,
    json_output: bool,
    open_action: &str,
) -> Result<()> {
    let id = view.job.job_id.as_ref();
    let status = job_status_label(view);
    let next_action = if view.projection.is_terminal() {
        match view.projection.outcome {
            Some(deadreckon_protocol::JobOutcome::Verified) => {
                format!("deadreckon finish {}", run_prefix(id))
            }
            _ => format!("deadreckon {open_action} {}", run_prefix(id)),
        }
    } else {
        format!("deadreckon {open_action} {}", run_prefix(id))
    };
    let (process_durability, machine_restart_durability) =
        super::supervisor_service::guided_durability_labels();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "job_status",
                "id": id,
                "status": status,
                "next_actions": [&next_action],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "job": view.job.source_cwd,
                },
                "durability": {
                    "process": process_durability,
                    "machine_restart": machine_restart_durability,
                },
                "job": view,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("job"));
    print_kv_block(&[
        ("id", id),
        ("status", &status),
        ("goal", &view.job.goal),
        ("scope", &view.job.scope),
        ("attempts", &view.projection.attempt_count.to_string()),
        (
            "stop reason",
            &view
                .projection
                .stop_reason
                .map(serialized_label)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("process durability", process_durability),
        ("machine restart", &machine_restart_durability),
    ]);
    println!();
    println!("  {} {}", ui_muted("next:"), ui_command(next_action));
    Ok(())
}

fn serialized_label<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn cancel_job(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    force: bool,
) -> Result<()> {
    if view.projection.is_terminal() {
        return print_job_status(view, false);
    }
    let sequence = JobEventSequence::new(view.projection.last_sequence + 1).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job {} event sequence exhausted",
            view.job.job_id
        )))
    })?;
    deadreckon_core::append_job_event(
        paths,
        &JobEvent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: view.job.job_id.clone(),
            sequence,
            event_id: Uuid::new_v4().to_string(),
            causation_id: format!("operator-cancel:{}", view.job.job_id),
            timestamp: Utc::now(),
            lease_epoch: view.projection.current_lease_epoch,
            kind: JobEventKind::CancelRequested,
            detail: json!({ "stop_reason": StopReason::CancelRequested }),
        },
    )?;
    if let Ok(state) = load_run(paths, view.job.job_id.as_ref()) {
        write_cancel_marker(&state, "operator cancelled durable job")?;
    }
    let metadata_path = paths
        .job_dir(view.job.job_id.as_ref())
        .join("supervised-child.json");
    if let Ok(process) = deadreckon_core::read_supervised_process(&metadata_path) {
        use deadreckon_core::ChildTerminator as _;
        let grace = if force {
            Duration::ZERO
        } else {
            Duration::from_secs(2)
        };
        #[cfg(unix)]
        let outcome = process
            .pgid
            .and_then(|pgid| i32::try_from(pgid).ok())
            .map_or_else(
                || deadreckon_core::RawPidTerminator::new(process.pid).terminate(grace),
                |pgid| deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace),
            );
        #[cfg(not(unix))]
        let outcome = deadreckon_core::RawPidTerminator::new(process.pid).terminate(grace);
        if let deadreckon_core::TerminationOutcome::Failed(reason) = outcome {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "job {} cancellation was recorded but its child could not be stopped: {reason}",
                view.job.job_id
            ))));
        }
    }
    let updated = deadreckon_core::JobView::load(paths, view.job.job_id.as_ref())?;
    print_job_status(&updated, false)
}

fn freeze_contract(source: Option<&Path>, source_cwd: &Path, target: &Path) -> Result<()> {
    if let Some(source) = source {
        fs::copy(source, target)?;
        sync_file(target)?;
        return Ok(());
    }
    let kind = deadreckon_core::acceptance_defaults::detect_project_kind(source_cwd);
    let checks = deadreckon_core::acceptance_defaults::default_checks_for(&kind, source_cwd)
        .into_iter()
        .map(portable_default_check)
        .collect::<Vec<_>>();
    #[derive(Serialize)]
    struct FrozenSpec {
        name: String,
        checks: Vec<deadreckon_core::AcceptanceCheck>,
    }
    let body = serde_yaml::to_string(&FrozenSpec {
        name: format!(
            "deadreckon detected {}",
            deadreckon_core::acceptance_defaults::kind_label(&kind)
        ),
        checks,
    })
    .map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "failed to freeze detected done contract: {source}"
        )))
    })?;
    fs::write(target, body)?;
    sync_file(target)?;
    Ok(())
}

fn portable_default_check(
    check: deadreckon_core::AcceptanceCheck,
) -> deadreckon_core::AcceptanceCheck {
    match check {
        deadreckon_core::AcceptanceCheck::Shell {
            command, must_pass, ..
        } => deadreckon_core::AcceptanceCheck::Shell {
            command,
            cwd: Some("{working_dir}".to_string()),
            must_pass,
        },
        other => other,
    }
}

pub(crate) fn write_json_synced<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job artifact path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn git_revision(cwd: &Path) -> Option<String> {
    let output = deadreckon_core::git::run_git(cwd, &["rev-parse", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

#[cfg(test)]
mod tests {
    use deadreckon_protocol::{AuthorityAcceptedBy, JobEventKind, SemanticJudgeMode};
    use tempfile::TempDir;

    use super::*;

    fn request<'a>(
        paths: &'a DeadreckonPaths,
        source: &'a Path,
        contract: Option<&'a Path>,
    ) -> CreateJob<'a> {
        let mut plan = commands::course::trivial_operator_plan(
            "finish the durable fixture",
            commands::course::CourseShape::Single,
            "test",
        );
        plan.accepted_by = Some("operator".to_string());
        CreateJob {
            paths,
            source_cwd: source,
            scope: "fixture-scope".to_string(),
            launch_plan: plan,
            shape: JobShape::Single,
            driver: None,
            contract_source: contract,
            source: DurableSource {
                mode: DurableSourceMode::Copy,
                from: Some(source.to_path_buf()),
                allow_dirty: false,
            },
            max_spend_usd: 4.0,
            max_wall_seconds: 90,
            max_attempts: 2,
            sandbox_requested: "auto".to_string(),
            accepted_by: AuthorityAcceptedBy::Operator,
        }
    }

    #[test]
    fn approved_inputs_exist_before_job_is_queued() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable").expect("source file");
        let contract = source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: durable fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("contract");

        let job = create_job(request(&paths, &source, Some(&contract))).expect("create job");
        let authority_path = paths.job_authority(job.job_id.as_ref());
        let authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority bytes"))
                .expect("authority");
        assert_eq!(
            authority.contract_sha256,
            deadreckon_core::flight::sha256_file(&job_acceptance_path(&paths, job.job_id.as_ref()))
                .expect("contract digest")
        );
        assert_eq!(authority.semantic_judge_mode, SemanticJudgeMode::Required);
        assert_eq!(
            job.authority_sha256,
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest")
        );
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![
                JobEventKind::Created,
                JobEventKind::ContractApproved,
                JobEventKind::Queued,
            ]
        );
        assert!(history.events().iter().all(|event| event.lease_epoch == 0));
    }

    #[test]
    fn detected_contract_is_frozen_without_source_absolute_path() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("package.json"),
            r#"{"scripts":{"test":"node test.js"}}"#,
        )
        .expect("package");

        let job = create_job(request(&paths, &source, None)).expect("create job");
        let frozen =
            fs::read_to_string(job_acceptance_path(&paths, job.job_id.as_ref())).expect("frozen");
        assert!(frozen.contains("{working_dir}"));
        assert!(!frozen.contains(&source.display().to_string()));
    }

    #[test]
    fn durable_graph_job_freezes_shape_and_normalizes_parent_delivery_to_at_end() {
        use commands::course::{CoursePiece, CourseSubplan};
        use deadreckon_core::plan::ApplyWhen;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "graph").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::Plan;
        request.launch_plan.n = Some(3);
        request.launch_plan.pieces = vec![
            CoursePiece {
                id: "foundation".to_string(),
                goal: "build foundation".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: Vec::new(),
                subplan: None,
            },
            CoursePiece {
                id: "parallel".to_string(),
                goal: "build independent slice".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: Vec::new(),
                subplan: None,
            },
            CoursePiece {
                id: "nested".to_string(),
                goal: "integrate nested slice".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: vec!["foundation".to_string()],
                subplan: Some(CourseSubplan {
                    apply: ApplyWhen::PerNode,
                    pieces: vec![CoursePiece {
                        id: "nested-check".to_string(),
                        goal: "verify nested result".to_string(),
                        done_hint: None,
                        role: None,
                        provider: None,
                        model: None,
                        budget_usd: None,
                        depends_on: Vec::new(),
                        subplan: None,
                    }],
                }),
            },
        ];
        request.shape = JobShape::Graph;
        request.driver = Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::FullPlan,
            child_count: Some(3),
            apply: ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            model: None,
            source_init_git: false,
        });

        let job = create_job(request).expect("create graph job");
        assert_eq!(job.shape, JobShape::Graph);
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("frozen launch");
        assert_eq!(frozen.pieces.len(), 3);
        assert!(frozen.pieces[0].depends_on.is_empty());
        assert!(frozen.pieces[1].depends_on.is_empty());
        assert_eq!(frozen.pieces[2].depends_on, ["foundation"]);
        assert_eq!(
            frozen.pieces[2]
                .subplan
                .as_ref()
                .expect("nested graph")
                .apply,
            ApplyWhen::PerNode
        );
        let frozen_driver = commands::graph_job::driver_spec(&frozen).expect("driver");
        assert_eq!(
            frozen_driver.kind,
            commands::graph_job::DriverKind::FullPlan
        );
        assert_eq!(frozen_driver.apply, ApplyWhen::AtEnd);
    }

    #[test]
    fn public_job_labels_use_snake_case() {
        assert_eq!(
            serialized_label(deadreckon_protocol::JobOutcome::NeedsReview),
            "needs_review"
        );
        assert_eq!(
            serialized_label(deadreckon_protocol::StopReason::SemanticUnavailable),
            "semantic_unavailable"
        );
    }

    #[test]
    fn status_distinguishes_phase_from_stop_reason() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let mut view =
            deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        view.projection.phase = deadreckon_protocol::JobPhase::Waiting;
        view.projection.stop_reason = Some(deadreckon_protocol::StopReason::LostContainment);

        assert_eq!(job_status_label(&view), "waiting");
        assert_eq!(
            serialized_label(view.projection.stop_reason.expect("stop reason")),
            "lost_containment"
        );
    }

    #[test]
    fn campaign_start_also_enters_the_durable_job_queue() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "campaign").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::Campaign;
        request.shape = JobShape::LegacyCampaign;
        request.driver = Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::Campaign,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            model: None,
            source_init_git: false,
        });

        let job = create_job(request).expect("create campaign job");
        assert_eq!(job.shape, JobShape::LegacyCampaign);
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.projection.phase, deadreckon_protocol::JobPhase::Queued);
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("launch");
        assert_eq!(
            commands::graph_job::driver_spec(&frozen)
                .expect("driver")
                .kind,
            commands::graph_job::DriverKind::Campaign
        );
    }

    #[test]
    fn legacy_chain_is_rejected_before_durable_job_state_is_written() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "legacy chain").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::ChainExtend;
        request.shape = JobShape::LegacyChain;

        let error = create_job(request).expect_err("legacy chain must stay outside job scheduler");

        assert!(
            error
                .to_string()
                .contains("legacy chain jobs remain process-bound"),
            "{error}"
        );
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs dir")
                    .next()
                    .is_none(),
            "refusal must happen before a partial job is persisted"
        );
    }

    #[test]
    fn successful_job_delivery_is_recorded_once_with_factual_after_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        deadreckon_core::append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(4).expect("sequence"),
                event_id: "verified".to_string(),
                causation_id: "test-verification".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::Verified,
                detail: json!({ "stop_reason": StopReason::Verified }),
            },
        )
        .expect("verified");
        let destination = temp.path().join("delivered");

        record_job_delivery(
            &paths,
            job.job_id.as_ref(),
            JobDeliveryKind::Exported,
            &destination,
            Some("result-revision"),
        )
        .expect("first delivery");
        record_job_delivery(
            &paths,
            job.job_id.as_ref(),
            JobDeliveryKind::Exported,
            &destination,
            Some("result-revision"),
        )
        .expect("idempotent delivery");

        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        let delivered = history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::ResultExported)
            .collect::<Vec<_>>();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].detail["destination"], json!(destination));
        assert_eq!(delivered[0].detail["resulting_revision"], "result-revision");
    }

    #[test]
    fn concurrent_duplicate_finish_delivery_is_one_idempotent_fact() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        deadreckon_core::append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(4).expect("sequence"),
                event_id: "verified-concurrent".to_string(),
                causation_id: "test-verification".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::Verified,
                detail: json!({ "stop_reason": StopReason::Verified }),
            },
        )
        .expect("verified");
        let destination = temp.path().join("delivered");
        let barrier = std::sync::Barrier::new(4);

        std::thread::scope(|scope| {
            let handles = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        record_job_delivery(
                            &paths,
                            job.job_id.as_ref(),
                            JobDeliveryKind::Applied,
                            &destination,
                            Some("revision-after-apply"),
                        )
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                handle.join().expect("delivery thread").expect("delivery");
            }
        });

        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::ResultApplied)
                .count(),
            1
        );
    }
}
