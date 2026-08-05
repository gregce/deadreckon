use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::codebase::{
    CodebaseMode, CodebaseRecord, write_codebase_record, write_trusted_codebase_record,
};
use crate::docs::ensure_docs_started;
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::gate::create_gate_key;
use crate::paths::{DeadreckonPaths, source_root, task_key, workspace_scope};
use deadreckon_protocol::SpendRecord;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Pending,
    Planned,
    Executing,
    Completed,
    Failed,
    Killed,
}

/// Typed provider disposition retained with a failed Run so an outer durable
/// supervisor can decide whether another process attempt is legitimate
/// without reinterpreting human-readable error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureDisposition {
    Retryable,
    Fatal,
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::glossary::run_status_label(*self))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PhaseId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseStatus {
    Pending,
    Planned,
    Executing,
    Completed,
    Failed,
}

impl fmt::Display for PhaseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::glossary::phase_status_label(*self))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseState {
    pub id: PhaseId,
    pub name: String,
    pub status: PhaseStatus,
    pub plan_path: Option<PathBuf>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub version: u32,
    pub goal: String,
    pub task_key: String,
    pub run_id: String,
    pub scope: String,
    pub status: RunStatus,
    pub current_phase_id: PhaseId,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub cwd: PathBuf,
    pub run_root: PathBuf,
    pub working_dir: PathBuf,
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub sandbox: String,
    pub provider: Option<String>,
    /// Durable Job authority that owns this Run as one Plan task attempt.
    ///
    /// Ordinary and historical Runs omit this field. Trusted Plan children
    /// receive it in the first state write so public Run lifecycle commands
    /// cannot mistake them for independently mutable compatibility Runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<RunOwnership>,
    pub max_spend_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_seconds: Option<f64>,
    pub total_spend_usd: f64,
    #[serde(default)]
    pub total_wall_seconds: f64,
    pub turn: u32,
    pub pause_reason: Option<String>,
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_failure: Option<ProviderFailureDisposition>,
    #[serde(default)]
    pub child_pids: Vec<u32>,
    pub killed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_library_dir: Option<PathBuf>,
    pub phases: Vec<PhaseState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOwnership {
    pub schema_version: u32,
    pub job_id: String,
    #[serde(flatten)]
    pub artifact: RunOwnershipArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunOwnershipArtifact {
    PlanTask {
        plan_id: String,
        task_id: String,
        task_index: u32,
        task_attempt: u32,
    },
    PlanResult {
        plan_id: String,
    },
    CampaignResult {
        campaign_id: String,
    },
    MergeRepair {
        root_artifact_id: String,
        repair_id: String,
        repair_round: u32,
        run_id: String,
        proof_dir: PathBuf,
        repair_request_sha256: String,
        repair_plan_sha256: String,
    },
    ParentResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRepairOwnership {
    pub root_artifact_id: String,
    pub repair_id: String,
    pub repair_round: u32,
    pub run_id: String,
    pub proof_dir: PathBuf,
    pub repair_request_sha256: String,
    pub repair_plan_sha256: String,
}

impl RunOwnership {
    pub fn plan_task(
        job_id: impl Into<String>,
        plan_id: impl Into<String>,
        task_id: impl Into<String>,
        task_index: u32,
        task_attempt: u32,
    ) -> Self {
        Self {
            schema_version: 1,
            job_id: job_id.into(),
            artifact: RunOwnershipArtifact::PlanTask {
                plan_id: plan_id.into(),
                task_id: task_id.into(),
                task_index,
                task_attempt,
            },
        }
    }

    pub fn plan_result(job_id: impl Into<String>, plan_id: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            job_id: job_id.into(),
            artifact: RunOwnershipArtifact::PlanResult {
                plan_id: plan_id.into(),
            },
        }
    }

    pub fn campaign_result(job_id: impl Into<String>, campaign_id: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            job_id: job_id.into(),
            artifact: RunOwnershipArtifact::CampaignResult {
                campaign_id: campaign_id.into(),
            },
        }
    }

    pub fn merge_repair(job_id: impl Into<String>, repair: MergeRepairOwnership) -> Self {
        Self {
            schema_version: 1,
            job_id: job_id.into(),
            artifact: RunOwnershipArtifact::MergeRepair {
                root_artifact_id: repair.root_artifact_id,
                repair_id: repair.repair_id,
                repair_round: repair.repair_round,
                run_id: repair.run_id,
                proof_dir: repair.proof_dir,
                repair_request_sha256: repair.repair_request_sha256,
                repair_plan_sha256: repair.repair_plan_sha256,
            },
        }
    }

    pub fn parent_result(job_id: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            job_id: job_id.into(),
            artifact: RunOwnershipArtifact::ParentResult,
        }
    }

    fn is_complete(&self) -> bool {
        if self.schema_version != 1 || self.job_id.trim().is_empty() {
            return false;
        }
        match &self.artifact {
            RunOwnershipArtifact::PlanTask {
                plan_id,
                task_id,
                task_attempt,
                ..
            } => !plan_id.trim().is_empty() && !task_id.trim().is_empty() && *task_attempt > 0,
            RunOwnershipArtifact::PlanResult { plan_id } => !plan_id.trim().is_empty(),
            RunOwnershipArtifact::CampaignResult { campaign_id } => !campaign_id.trim().is_empty(),
            RunOwnershipArtifact::MergeRepair {
                root_artifact_id,
                repair_id,
                repair_round,
                run_id,
                proof_dir,
                repair_request_sha256,
                repair_plan_sha256,
            } => {
                !root_artifact_id.trim().is_empty()
                    && !repair_id.trim().is_empty()
                    && *repair_round > 0
                    && !run_id.trim().is_empty()
                    && proof_dir.is_absolute()
                    && repair_request_sha256.starts_with("sha256:")
                    && repair_plan_sha256.starts_with("sha256:")
            }
            RunOwnershipArtifact::ParentResult => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentRunPointer {
    pub goal: String,
    pub task_key: String,
    pub run_id: String,
    pub scope: String,
    pub cwd: PathBuf,
    pub working_dir: PathBuf,
    pub state_path: PathBuf,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub goal: String,
    pub cwd: PathBuf,
    pub sandbox: String,
    pub provider: Option<String>,
    pub skill_name: String,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub run_id: Option<String>,
    pub codebase: Option<CodebaseRecord>,
}

#[derive(Debug, Clone)]
pub struct RunListEntry {
    pub run_id: String,
    pub scope: String,
    pub goal: String,
    pub status: RunStatus,
    pub updated_at: DateTime<Utc>,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpendSummary {
    pub total_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_seconds: f64,
    pub turns: usize,
    pub subscription_turns: usize,
    pub any_subscription_turn: bool,
    pub any_estimated_turn: bool,
}

impl PipelineState {
    pub fn state_path(&self) -> PathBuf {
        self.run_root.join("state.json")
    }

    pub fn set_phase_status(&mut self, id: PhaseId, status: PhaseStatus) -> Result<()> {
        let now = Utc::now();
        let phase = self
            .phases
            .iter_mut()
            .find(|phase| phase.id == id)
            .ok_or_else(|| DeadreckonError::NotFound(format!("phase {}", id.0)))?;
        phase.status = status;
        phase.updated_at = now;
        self.current_phase_id = id;
        self.updated_at = now;
        self.status = match status {
            PhaseStatus::Executing => RunStatus::Executing,
            PhaseStatus::Failed => RunStatus::Failed,
            PhaseStatus::Completed if id == PhaseId(60) => RunStatus::Completed,
            PhaseStatus::Planned => RunStatus::Planned,
            PhaseStatus::Pending | PhaseStatus::Completed => self.status,
        };
        Ok(())
    }

    pub fn active_phase(&self) -> Option<&PhaseState> {
        self.phases
            .iter()
            .find(|phase| phase.id == self.current_phase_id)
    }
}

fn resolve_skill_path(paths: &DeadreckonPaths, skill_name: &str) -> PathBuf {
    let relative = Path::new("skills").join(skill_name).join("SKILL.md");
    let user = paths.home().join(&relative);
    if user.exists() {
        return user;
    }
    if let Some(root) = source_root() {
        let repo = root.join(&relative);
        if repo.exists() {
            return repo;
        }
    }
    user
}

pub fn create_run(paths: &DeadreckonPaths, options: RunOptions) -> Result<PipelineState> {
    create_run_with_optional_ownership(paths, options, None)
}

pub fn create_owned_run(
    paths: &DeadreckonPaths,
    options: RunOptions,
    ownership: RunOwnership,
) -> Result<PipelineState> {
    if !ownership.is_complete() {
        return Err(DeadreckonError::InvalidInput(
            "owned Run requires complete durable Job artifact authority".to_string(),
        ));
    }
    create_run_with_optional_ownership(paths, options, Some(ownership))
}

fn create_run_with_optional_ownership(
    paths: &DeadreckonPaths,
    options: RunOptions,
    ownership: Option<RunOwnership>,
) -> Result<PipelineState> {
    let scope = workspace_scope(&options.cwd)?;
    let task_key = task_key(&options.goal);
    let run_id = options
        .run_id
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let run_root = paths.run_root(&scope, &run_id);
    let codebase = options.codebase.unwrap_or_else(CodebaseRecord::fresh);
    let working_dir = working_dir_for_codebase(&run_root, &codebase);
    fs::create_dir_all(&working_dir).with_path(&working_dir)?;
    fs::create_dir_all(run_root.join("snapshots")).with_path(run_root.join("snapshots"))?;
    let gate_dir = run_root.join("gate");
    fs::create_dir_all(&gate_dir).with_path(&gate_dir)?;
    create_gate_key(paths, &run_id)?;

    // AS-BUILT §3: the binary owns deterministic paths while the skill stays a
    // markdown subprocess boundary. User skills (<home>/skills) win over the
    // source checkout; a missing file falls back to the built-in contract at read
    // time, so installed binaries without either tier still run.
    let skill_path = resolve_skill_path(paths, &options.skill_name);
    let now = Utc::now();
    let mut state = PipelineState {
        version: STATE_VERSION,
        goal: options.goal,
        task_key,
        run_id,
        scope,
        status: RunStatus::Pending,
        current_phase_id: PhaseId(0),
        started_at: now,
        updated_at: now,
        cwd: options.cwd,
        run_root,
        working_dir,
        skill_name: options.skill_name,
        skill_path,
        sandbox: options.sandbox,
        provider: options.provider,
        ownership,
        max_spend_usd: options.max_spend_usd,
        max_wall_seconds: options.max_wall_seconds,
        total_spend_usd: 0.0,
        total_wall_seconds: 0.0,
        turn: 0,
        pause_reason: None,
        failure_reason: None,
        provider_failure: None,
        child_pids: Vec::new(),
        killed_at: None,
        promoted_library_dir: None,
        phases: default_phases(now),
    };
    state.set_phase_status(PhaseId(0), PhaseStatus::Planned)?;
    save_state(&state)?;
    write_trusted_codebase_record(&state.run_root, &codebase)?;
    write_codebase_record(&state.working_dir, &codebase)?;
    ensure_docs_started(&state)?;
    // Freeze Git hydration, ignore rules, and generated-output hints while the
    // controller still owns the workspace. Post-provider capture paths must
    // consume this record and must never reconstruct it from agent-visible
    // Git state. Copy mode freezes the source because the working copy is
    // materialized immediately after run creation.
    let capture_source = if codebase.mode == CodebaseMode::Copy {
        codebase
            .source_path
            .as_deref()
            .unwrap_or(&state.working_dir)
    } else {
        &state.working_dir
    };
    let capture_policy = crate::workspace_capture::freeze_workspace_capture_policy(capture_source)?;
    crate::workspace_capture::write_workspace_capture_policy(&state.run_root, &capture_policy)?;
    write_current_pointer(paths, &state)?;
    Ok(state)
}

fn working_dir_for_codebase(run_root: &Path, codebase: &CodebaseRecord) -> PathBuf {
    match codebase.mode {
        CodebaseMode::Worktree => codebase
            .worktree_path
            .clone()
            .unwrap_or_else(|| run_root.join("working")),
        CodebaseMode::InPlace => codebase
            .source_path
            .clone()
            .unwrap_or_else(|| run_root.join("working")),
        CodebaseMode::Copy | CodebaseMode::Fresh => run_root.join("working"),
    }
}

pub fn default_phases(now: DateTime<Utc>) -> Vec<PhaseState> {
    // AS-BUILT §4: gap-numbered phases leave room for future gates without
    // rewriting durable state files.
    [
        (0, "init"),
        (10, "plan"),
        (20, "provider"),
        (30, "sandbox"),
        (40, "execute"),
        (50, "verify"),
        (60, "complete"),
    ]
    .into_iter()
    .map(|(id, name)| PhaseState {
        id: PhaseId(id),
        name: name.to_string(),
        status: PhaseStatus::Pending,
        plan_path: None,
        updated_at: now,
    })
    .collect()
}

pub fn save_state(state: &PipelineState) -> Result<()> {
    atomic_write_json(&state.state_path(), state)
}

pub fn load_state(path: &Path) -> Result<PipelineState> {
    let data = fs::read(path).with_path(path)?;
    serde_json::from_slice(&data).with_json_path(path)
}

pub fn spend_summary(state: &PipelineState) -> Result<SpendSummary> {
    let path = state.run_root.join("spend.jsonl");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SpendSummary {
                total_usd: state.total_spend_usd,
                wall_seconds: state.total_wall_seconds,
                ..SpendSummary::default()
            });
        }
        Err(source) => {
            return Err(DeadreckonError::Io { path, source });
        }
    };
    let mut summary = SpendSummary::default();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let record: SpendRecord =
            serde_json::from_str(line).map_err(|source| DeadreckonError::Json {
                path: path.clone(),
                source,
            })?;
        // Only the run loop's own turns count toward run spend. The live
        // narrator writes kind="narrator" rows to the same ledger; those must
        // not inflate provider tokens/turns or overwrite the running total.
        if record.kind != "loop" {
            continue;
        }
        summary.total_usd = record.total_cost_usd;
        summary.input_tokens = summary.input_tokens.saturating_add(record.input_tokens);
        summary.output_tokens = summary.output_tokens.saturating_add(record.output_tokens);
        summary.wall_seconds += record.wall_time_seconds.unwrap_or(0.0);
        summary.turns = summary.turns.saturating_add(1);
        if record.subscription {
            summary.subscription_turns = summary.subscription_turns.saturating_add(1);
        }
        summary.any_subscription_turn |= record.subscription;
        summary.any_estimated_turn |= record.estimated;
    }
    // Lifecycle wall time is controller-measured across every run phase.
    // Spend rows remain provider evidence and therefore cannot be summed into
    // an authoritative active-run duration.
    summary.wall_seconds = state.total_wall_seconds;
    if summary.total_usd == 0.0 {
        summary.total_usd = state.total_spend_usd;
    }
    Ok(summary)
}

pub fn write_current_pointer(paths: &DeadreckonPaths, state: &PipelineState) -> Result<()> {
    let pointer = CurrentRunPointer {
        goal: state.goal.clone(),
        task_key: state.task_key.clone(),
        run_id: state.run_id.clone(),
        scope: state.scope.clone(),
        cwd: state.cwd.clone(),
        working_dir: state.working_dir.clone(),
        state_path: state.state_path(),
        updated_at: Utc::now(),
    };
    atomic_write_json(
        &paths.current_pointer_path(&state.scope, &state.task_key),
        &pointer,
    )
}

pub fn load_current_pointer(
    paths: &DeadreckonPaths,
    scope: &str,
    task_key: &str,
) -> Result<CurrentRunPointer> {
    let path = paths.current_pointer_path(scope, task_key);
    let data = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&data).with_json_path(path)
}

pub fn find_run_state_path(paths: &DeadreckonPaths, run_id: &str) -> Result<PathBuf> {
    let run_id = run_id.trim();
    if run_id.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "run id prefix cannot be empty".to_string(),
        ));
    }
    let root = paths.runstate_dir();
    if !root.exists() {
        return Err(DeadreckonError::NotFound(format!("run {run_id}")));
    }
    let mut prefix_matches = Vec::new();
    // Bound the walk to the runstate layout depth (run dir -> state.json), matching
    // list_runs. Without this, resolving one run descends into sibling runs' worktree
    // and artifact subtrees (thousands of files) on every attach tick.
    for entry in WalkDir::new(&root)
        .max_depth(4)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() || entry.file_name() != "state.json" {
            continue;
        }
        let candidate = entry.path();
        let Some(candidate_run_id) = candidate
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        if candidate_run_id == run_id {
            return Ok(candidate.to_path_buf());
        }
        if candidate_run_id.starts_with(run_id) {
            prefix_matches.push((candidate_run_id.to_string(), candidate.to_path_buf()));
        }
    }
    match prefix_matches.len() {
        1 => Ok(prefix_matches.remove(0).1),
        0 => Err(DeadreckonError::NotFound(format!("run {run_id}"))),
        _ => {
            let matches = prefix_matches
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
                .join(", ");
            Err(DeadreckonError::InvalidInput(format!(
                "ambiguous run id prefix {run_id}; matches {matches}"
            )))
        }
    }
}

pub fn load_run(paths: &DeadreckonPaths, run_id: &str) -> Result<PipelineState> {
    let path = find_run_state_path(paths, run_id)?;
    load_state(&path)
}

pub fn list_runs(paths: &DeadreckonPaths, scope_filter: Option<&str>) -> Result<Vec<RunListEntry>> {
    let root = paths.runstate_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in WalkDir::new(&root)
        .max_depth(4)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() || entry.file_name() != "state.json" {
            continue;
        }
        let state = load_state(entry.path())?;
        if scope_filter.is_some_and(|scope| scope != state.scope) {
            continue;
        }
        runs.push(RunListEntry {
            run_id: state.run_id,
            scope: state.scope,
            goal: state.goal,
            status: state.status,
            updated_at: state.updated_at,
            state_path: entry.path().to_path_buf(),
        });
    }
    runs.sort_by_key(|run| std::cmp::Reverse(run.updated_at));
    Ok(runs)
}

pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    persist_temp(temp, path)
}

fn persist_temp(temp: NamedTempFile, path: &Path) -> Result<()> {
    match temp.persist(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: err.error,
        }),
    }
}

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut line = Vec::new();
    serde_json::to_writer(&mut line, value).with_json_path(path)?;
    line.push(b'\n');
    let mut file = File::options()
        .create(true)
        .append(true)
        .open(path)
        .with_path(path)?;
    file.write_all(&line).with_path(path)?;
    file.sync_all().with_path(path)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use deadreckon_protocol::SpendRecord;
    use tempfile::TempDir;

    use super::{
        PhaseId, PhaseStatus, RunOptions, RunStatus, append_json_line, create_run,
        load_current_pointer, load_run, save_state, spend_summary,
    };
    use crate::DeadreckonError;
    use crate::paths::{DeadreckonPaths, task_key};

    #[test]
    fn create_run_writes_state_and_current_pointer() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let cwd = std::env::current_dir().expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "tiny hello rust".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("create run");

        assert!(state.state_path().exists());
        assert!(state.run_root.join(crate::TRUSTED_CODEBASE_RECORD).exists());
        let capture_policy = crate::read_workspace_capture_policy(&state.run_root)
            .expect("controller-frozen capture policy");
        assert!(capture_policy.frozen_git_hydration.is_some());
        assert_eq!(
            crate::read_run_codebase_record(&state.run_root, &state.working_dir)
                .expect("trusted codebase record"),
            crate::read_codebase_record(&state.working_dir).expect("workspace codebase record")
        );
        assert_eq!(state.status, RunStatus::Planned);
        assert_eq!(state.phases[0].id, PhaseId(0));
        assert_eq!(state.phases[1].id, PhaseId(10));

        let pointer = load_current_pointer(&paths, &state.scope, &task_key("tiny hello rust"))
            .expect("current pointer");
        assert_eq!(pointer.run_id, state.run_id);
        assert_eq!(pointer.state_path, state.state_path());
    }

    #[test]
    fn resume_after_process_death_reloads_executing_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let cwd = std::env::current_dir().expect("cwd");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "crash resume".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("openai".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("create run");
        state
            .set_phase_status(PhaseId(40), PhaseStatus::Executing)
            .expect("set phase");
        save_state(&state).expect("save state");

        let reloaded = load_run(&paths, &state.run_id).expect("load run");
        assert_eq!(reloaded.run_id, state.run_id);
        assert_eq!(reloaded.status, RunStatus::Executing);
        assert_eq!(reloaded.current_phase_id, PhaseId(40));
        assert_eq!(
            reloaded.active_phase().map(|phase| phase.status),
            Some(PhaseStatus::Executing)
        );
    }

    #[test]
    fn spend_summary_uses_authoritative_run_wall_clock() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "authoritative wall clock".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: None,
                codebase: None,
            },
        )
        .expect("create run");
        state.total_wall_seconds = 12.0;
        append_json_line(
            &state.run_root.join("spend.jsonl"),
            &SpendRecord {
                timestamp: Utc::now(),
                turn: 1,
                provider: "test".to_string(),
                model: "test".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: 0.0,
                total_cost_usd: 0.0,
                cap_usd: Some(1.0),
                subscription: false,
                estimated: false,
                wall_time_seconds: Some(2.0),
                wall_time_cap_seconds: Some(60.0),
                kind: "loop".to_string(),
            },
        )
        .expect("spend row");

        let summary = spend_summary(&state).expect("summary");

        assert_eq!(summary.wall_seconds, 12.0);
        assert_eq!(summary.turns, 1);
    }

    #[test]
    fn load_run_accepts_unique_prefix() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let cwd = std::env::current_dir().expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "prefix load".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: Some("abcdef1234567890abcdef1234567890".to_string()),
                codebase: None,
            },
        )
        .expect("create run");

        let reloaded = load_run(&paths, "abcdef12").expect("load prefix");
        assert_eq!(reloaded.run_id, state.run_id);
    }

    #[test]
    fn load_run_rejects_ambiguous_prefix() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let cwd = std::env::current_dir().expect("cwd");
        for (id, goal) in [
            ("abc11111111111111111111111111111", "prefix one"),
            ("abc22222222222222222222222222222", "prefix two"),
        ] {
            create_run(
                &paths,
                RunOptions {
                    goal: goal.to_string(),
                    cwd: cwd.clone(),
                    sandbox: "none".to_string(),
                    provider: None,
                    skill_name: "default-coding".to_string(),
                    max_spend_usd: Some(1.0),
                    max_wall_seconds: None,
                    run_id: Some(id.to_string()),
                    codebase: None,
                },
            )
            .expect("create run");
        }

        let err = load_run(&paths, "abc").expect_err("ambiguous");
        assert!(matches!(err, DeadreckonError::InvalidInput(_)));
        assert!(err.to_string().contains("ambiguous run id prefix abc"));
    }
}
