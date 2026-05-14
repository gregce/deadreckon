use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::codebase::{CodebaseMode, CodebaseRecord, write_codebase_record};
use crate::docs::ensure_docs_started;
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::{DeadreckonPaths, SOURCE_ROOT, task_key, workspace_scope};

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Pending,
    Planned,
    Executing,
    Completed,
    Failed,
    Killed,
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
    pub max_spend_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_seconds: Option<f64>,
    pub total_spend_usd: f64,
    #[serde(default)]
    pub total_wall_seconds: f64,
    pub turn: u32,
    pub pause_reason: Option<String>,
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub child_pids: Vec<u32>,
    pub killed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_library_dir: Option<PathBuf>,
    pub phases: Vec<PhaseState>,
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

pub fn create_run(paths: &DeadreckonPaths, options: RunOptions) -> Result<PipelineState> {
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
    fs::write(gate_dir.join("nonce"), Uuid::new_v4().simple().to_string())
        .with_path(gate_dir.join("nonce"))?;

    // AS-BUILT §3: the binary owns deterministic paths while the skill stays a
    // markdown subprocess boundary under the source tree.
    let skill_path = PathBuf::from(SOURCE_ROOT)
        .join("skills")
        .join(&options.skill_name)
        .join("SKILL.md");
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
        max_spend_usd: options.max_spend_usd,
        max_wall_seconds: options.max_wall_seconds,
        total_spend_usd: 0.0,
        total_wall_seconds: 0.0,
        turn: 0,
        pause_reason: None,
        failure_reason: None,
        child_pids: Vec::new(),
        killed_at: None,
        promoted_library_dir: None,
        phases: default_phases(now),
    };
    state.set_phase_status(PhaseId(0), PhaseStatus::Planned)?;
    save_state(&state)?;
    write_codebase_record(&state.working_dir, &codebase)?;
    ensure_docs_started(&state)?;
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
    for entry in WalkDir::new(&root)
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
    runs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
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
    let mut file = File::options()
        .create(true)
        .append(true)
        .open(path)
        .with_path(path)?;
    serde_json::to_writer(&mut file, value).with_json_path(path)?;
    file.write_all(b"\n").with_path(path)?;
    file.sync_all().with_path(path)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{
        PhaseId, PhaseStatus, RunOptions, RunStatus, create_run, load_current_pointer, load_run,
        save_state,
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
