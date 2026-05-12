use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::DeadreckonPaths;
use crate::state::{append_json_line, atomic_write_json};

pub const CHAIN_JSON: &str = "chain.json";
pub const CHAIN_EVENTS_JSONL: &str = "chain-events.jsonl";
pub const CONDUCTOR_JSON: &str = "conductor.json";
pub const CHAIN_STEP_JSON: &str = ".deadreckon/chain-step.json";
pub const CHAIN_LOCK_PREFIX: &str = "chain--";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Killed,
    Undone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Applied,
    Undone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchPolicy {
    Stack,
    Base,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    Auto,
    Preview,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyStrategy {
    Squash,
    Merge,
    CherryPick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    Stop,
    Skip,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainStep {
    pub index: u32,
    pub goal: String,
    pub status: ChainStepStatus,
    pub run_id: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub applied_sha: Option<String>,
    pub fail_reason: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub spend_usd: f64,
}

impl ChainStep {
    pub fn new(index: u32, goal: impl Into<String>) -> Self {
        Self {
            index,
            goal: goal.into(),
            status: ChainStepStatus::Pending,
            run_id: None,
            applied_at: None,
            applied_sha: None,
            fail_reason: None,
            max_spend_usd: None,
            spend_usd: 0.0,
        }
    }

    pub fn transition_to(&mut self, status: ChainStepStatus) -> Result<()> {
        if !step_transition_allowed(self.status, status) {
            return Err(DeadreckonError::InvalidInput(format!(
                "invalid chain step transition {:?} -> {:?}",
                self.status, status
            )));
        }
        self.status = status;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    pub schema_version: u32,
    pub chain_id: String,
    pub root_goal: String,
    pub steps: Vec<ChainStep>,
    pub branch_policy: BranchPolicy,
    pub apply_mode: ApplyMode,
    pub apply_strategy: ApplyStrategy,
    pub apply_allowlist: Vec<String>,
    pub on_fail: OnFail,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_consecutive_failures: u32,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub total_spend_usd: f64,
    pub total_wall_seconds: f64,
    pub scope: String,
    pub base_branch: String,
    pub base_sha: String,
    pub cwd: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub sandbox: String,
    pub status: ChainStatus,
    pub paused_reason: Option<String>,
    pub failure_reason: Option<String>,
    pub conductor_pid: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub deadreckon_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainNewOptions {
    pub root_goal: String,
    pub goals: Vec<String>,
    pub scope: String,
    pub base_branch: String,
    pub base_sha: String,
    pub cwd: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub sandbox: String,
    pub branch_policy: BranchPolicy,
    pub apply_mode: ApplyMode,
    pub apply_strategy: ApplyStrategy,
    pub apply_allowlist: Vec<String>,
    pub on_fail: OnFail,
    pub circuit_breaker_threshold: u32,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub deadreckon_version: String,
}

impl Chain {
    pub fn new(options: ChainNewOptions) -> Result<Self> {
        validate_goal_count(options.goals.len())?;
        let now = Utc::now();
        Ok(Self {
            schema_version: 1,
            chain_id: Uuid::new_v4().simple().to_string(),
            root_goal: options.root_goal,
            steps: options
                .goals
                .into_iter()
                .enumerate()
                .map(|(index, goal)| ChainStep::new(index as u32, goal))
                .collect(),
            branch_policy: options.branch_policy,
            apply_mode: options.apply_mode,
            apply_strategy: options.apply_strategy,
            apply_allowlist: options.apply_allowlist,
            on_fail: options.on_fail,
            circuit_breaker_threshold: options.circuit_breaker_threshold,
            circuit_breaker_consecutive_failures: 0,
            max_spend_usd: options.max_spend_usd,
            max_wall_seconds: options.max_wall_seconds,
            total_spend_usd: 0.0,
            total_wall_seconds: 0.0,
            scope: options.scope,
            base_branch: options.base_branch,
            base_sha: options.base_sha,
            cwd: options.cwd,
            provider: options.provider,
            model: options.model,
            sandbox: options.sandbox,
            status: ChainStatus::Pending,
            paused_reason: None,
            failure_reason: None,
            conductor_pid: None,
            created_at: now,
            started_at: None,
            completed_at: None,
            deadreckon_version: options.deadreckon_version,
        })
    }

    pub fn task_key(&self) -> String {
        chain_task_key(&self.chain_id)
    }

    pub fn pending_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Pending)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainStepMarker {
    pub schema_version: u32,
    pub kind: String,
    pub chain_id: String,
    pub step_index: u32,
    pub chain_root_goal: String,
    pub step_goal: String,
    pub prior_applied_sha: Option<String>,
    pub created_at: DateTime<Utc>,
    pub deadreckon_version: String,
}

impl ChainStepMarker {
    pub fn new(chain: &Chain, step: &ChainStep, prior_applied_sha: Option<String>) -> Self {
        Self {
            schema_version: 1,
            kind: "chain_step".to_string(),
            chain_id: chain.chain_id.clone(),
            step_index: step.index,
            chain_root_goal: chain.root_goal.clone(),
            step_goal: step.goal.clone(),
            prior_applied_sha,
            created_at: Utc::now(),
            deadreckon_version: chain.deadreckon_version.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConductorState {
    pub schema_version: u32,
    pub chain_id: String,
    pub conductor_pid: u32,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub live_step: Option<u32>,
    #[serde(default)]
    pub live_run_id: Option<String>,
    #[serde(default)]
    pub live_child_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainEvent {
    pub timestamp: DateTime<Utc>,
    pub chain_id: String,
    pub event: ChainEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u32>,
    #[serde(default)]
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainEventKind {
    ChainCreated,
    ChainStepStarted,
    ChainRunCompleted,
    ChainApplyStarted,
    ChainApplied,
    ChainApplyRefused,
    ChainStepFailed,
    ChainPaused,
    ChainResumed,
    ChainKilled,
    ChainCompleted,
    ChainUndoStarted,
    ChainUndoneStep,
    ChainHookInvoked,
    ChainStepExtended,
    ChainStepRedone,
}

pub fn validate_goal_count(count: usize) -> Result<()> {
    match count {
        0 | 1 => Err(DeadreckonError::InvalidInput(
            "chain must have >= 2 steps\ntry: deadreckon run \"<the only step>\"".to_string(),
        )),
        2..=12 => Ok(()),
        _ => Err(DeadreckonError::InvalidInput(format!(
            "chain capped at 12 steps; got {count}\ntry: split the input into multiple chains"
        ))),
    }
}

pub fn chain_task_key(chain_id: &str) -> String {
    format!("{CHAIN_LOCK_PREFIX}{chain_id}")
}

pub fn chain_json_path(paths: &DeadreckonPaths, chain_id: &str) -> PathBuf {
    paths.chain_json(chain_id)
}

pub fn save_chain(paths: &DeadreckonPaths, chain: &Chain) -> Result<()> {
    atomic_write_json(&chain_json_path(paths, &chain.chain_id), chain)
}

pub fn load_chain(paths: &DeadreckonPaths, chain_id: &str) -> Result<Chain> {
    let path = chain_json_path(paths, chain_id);
    let raw = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&raw).with_json_path(&path)
}

pub fn append_chain_event(
    paths: &DeadreckonPaths,
    chain_id: &str,
    event: ChainEventKind,
    step_index: Option<u32>,
    detail: serde_json::Value,
) -> Result<()> {
    append_json_line(
        &paths.chain_events(chain_id),
        &ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain_id.to_string(),
            event,
            step_index,
            detail,
        },
    )
}

pub fn write_chain_step_marker(working_dir: &Path, marker: &ChainStepMarker) -> Result<PathBuf> {
    let path = working_dir.join(CHAIN_STEP_JSON);
    atomic_write_json(&path, marker)?;
    Ok(path)
}

pub fn read_chain_step_marker(working_dir: &Path) -> Result<Option<ChainStepMarker>> {
    let path = working_dir.join(CHAIN_STEP_JSON);
    match fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw).map(Some).with_json_path(&path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

fn step_transition_allowed(from: ChainStepStatus, to: ChainStepStatus) -> bool {
    use ChainStepStatus::{Applied, Completed, Failed, Pending, Running, Skipped, Undone};
    matches!(
        (from, to),
        (Pending, Running)
            | (Pending, Skipped)
            | (Pending, Failed)
            | (Running, Completed)
            | (Running, Failed)
            | (Running, Skipped)
            | (Completed, Applied)
            | (Completed, Failed)
            | (Completed, Pending)
            | (Failed, Pending)
            | (Applied, Pending)
            | (Applied, Undone)
            | (Skipped, Pending)
            | (Undone, Pending)
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        ApplyMode, ApplyStrategy, BranchPolicy, CHAIN_EVENTS_JSONL, CHAIN_JSON, CHAIN_LOCK_PREFIX,
        Chain, ChainEventKind, ChainNewOptions, ChainStepStatus, OnFail, append_chain_event,
        chain_task_key, load_chain, save_chain,
    };
    use crate::lock::lock_path;
    use crate::paths::DeadreckonPaths;

    fn sample_chain(temp: &TempDir) -> Chain {
        Chain::new(ChainNewOptions {
            root_goal: "manual: 2 steps".to_string(),
            goals: vec!["one".to_string(), "two".to_string()],
            scope: "scope-a".to_string(),
            base_branch: "main".to_string(),
            base_sha: "abc123".to_string(),
            cwd: temp.path().join("repo"),
            provider: Some("mock".to_string()),
            model: Some("test-model".to_string()),
            sandbox: "none".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Auto,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 2,
            max_spend_usd: Some(10.0),
            max_wall_seconds: Some(3600.0),
            deadreckon_version: "0.1.0".to_string(),
        })
        .expect("chain")
    }

    #[test]
    fn chain_json_serializes_roundtrip() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);

        save_chain(&paths, &chain).expect("save");
        let loaded = load_chain(&paths, &chain.chain_id).expect("load");

        assert_eq!(loaded, chain);
        assert!(paths.chain_json(&chain.chain_id).ends_with(CHAIN_JSON));
    }

    #[test]
    fn chain_step_status_transitions_pending_running_completed() {
        let temp = TempDir::new().expect("tempdir");
        let mut chain = sample_chain(&temp);
        let step = &mut chain.steps[0];

        step.transition_to(ChainStepStatus::Running)
            .expect("running");
        step.transition_to(ChainStepStatus::Completed)
            .expect("completed");
        step.transition_to(ChainStepStatus::Applied)
            .expect("applied");
        let err = step
            .transition_to(ChainStepStatus::Skipped)
            .expect_err("cannot skip applied");
        assert!(err.to_string().contains("invalid chain step transition"));
    }

    #[test]
    fn chain_paths_match_locks_pattern() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);

        assert_eq!(
            paths.chain_dir(&chain.chain_id),
            paths.chains_dir().join(&chain.chain_id)
        );
        assert_eq!(
            paths.chain_json(&chain.chain_id),
            paths.chain_dir(&chain.chain_id).join(CHAIN_JSON)
        );
        assert_eq!(
            paths.chain_events(&chain.chain_id),
            paths.chain_dir(&chain.chain_id).join(CHAIN_EVENTS_JSONL)
        );
        assert_eq!(
            lock_path(&paths, &chain.scope, &chain.task_key()),
            paths
                .locks_dir()
                .join(format!("{}--{}.lock", chain.scope, chain.task_key()))
        );
    }

    #[test]
    fn chain_lock_task_key_prefix_chain_double_dash() {
        let task_key = chain_task_key("abc123");

        assert_eq!(task_key, "chain--abc123");
        assert!(task_key.starts_with(CHAIN_LOCK_PREFIX));
    }

    #[test]
    fn append_chain_event_writes_jsonl() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);
        save_chain(&paths, &chain).expect("save");

        append_chain_event(
            &paths,
            &chain.chain_id,
            ChainEventKind::ChainCreated,
            None,
            serde_json::json!({ "steps": chain.steps.len() }),
        )
        .expect("event");

        let raw = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
        assert!(raw.contains(r#""event":"chain_created""#));
        assert!(raw.contains(r#""steps":2"#));
    }
}
