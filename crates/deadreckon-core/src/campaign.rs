//! Campaign orchestration: one meta-coordinator spawns N independent
//! sub-orchestrators (depth-capped at 2), composes their merged results into one
//! promoted run, and rolls every leaf's tamper-evident gate verdict up to the top.
//!
//! This module owns the file-backed campaign state, the depth/cycle guard, and the
//! lineage record that rides the spawn boundary. Child work stays normal
//! `deadreckon` subprocesses (AS-BUILT §30.1): campaigns add no fields to `Plan`,
//! `PlanTask`, or `PipelineState`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DeadreckonError, Result};
use crate::plan::{PlanProviders, validate_task_count};

/// The hard cap on orchestration nesting. A campaign sits at depth 0; the
/// orchestrators it launches sit at depth 1; a depth-1 process is forbidden from
/// launching a campaign (its sub-orchestrators would reach depth 2). Not
/// configurable — the cap is what keeps the blast radius and the trust surface
/// bounded.
pub const CAMPAIGN_MAX_DEPTH: u32 = 2;

const LINEAGE_FILE: &str = "lineage.json";

/// Durable nesting lineage for a plan or campaign. Written into the plan/campaign
/// dir at creation; the matching env vars (`DEADRECKON_CAMPAIGN_*`) are the
/// transport that lets a freshly spawned subprocess know its depth before it has a
/// plan dir. Absent lineage means depth 0 (a top-level invocation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    pub schema_version: u32,
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_root_id: Option<String>,
    #[serde(default)]
    pub ancestor_task_keys: Vec<String>,
    #[serde(default)]
    pub ancestor_scopes: Vec<String>,
}

impl Default for Lineage {
    fn default() -> Self {
        Self {
            schema_version: 1,
            depth: 0,
            campaign_root_id: None,
            ancestor_task_keys: Vec::new(),
            ancestor_scopes: Vec::new(),
        }
    }
}

pub fn lineage_path_for_plan_dir(plan_dir: &Path) -> PathBuf {
    plan_dir.join(LINEAGE_FILE)
}

/// Read the lineage record for a plan/campaign dir. A missing file is not an
/// error: it means depth 0.
pub fn read_lineage(plan_dir: &Path) -> Result<Lineage> {
    let path = lineage_path_for_plan_dir(plan_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|source| DeadreckonError::Json { path, source })
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Lineage::default()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

pub fn write_lineage(plan_dir: &Path, lineage: &Lineage) -> Result<()> {
    crate::state::atomic_write_json(&lineage_path_for_plan_dir(plan_dir), lineage)
}

/// Guard a requested campaign against the depth cap and ancestor cycles.
///
/// `current_depth` is the depth of the process requesting the campaign (0 for a
/// top-level invocation, 1 for a sub-orchestrator). The sub-orchestrators a
/// campaign launches sit at `current_depth + 1`; if that would reach
/// [`CAMPAIGN_MAX_DEPTH`] the campaign is refused. A requested sub-goal whose
/// `task_key` or resolved scope matches an ancestor's is refused as a cycle.
pub fn guard(
    current_depth: u32,
    ancestor_task_keys: &[String],
    ancestor_scopes: &[String],
    requested_sub_task_keys: &[String],
    requested_sub_scopes: &[String],
) -> Result<()> {
    if current_depth + 1 >= CAMPAIGN_MAX_DEPTH {
        return Err(DeadreckonError::InvalidInput(format!(
            "campaign refused: depth cap {CAMPAIGN_MAX_DEPTH} reached\n\
             try: run `orchestrate full-plan` (not campaign) inside a sub-orchestrator"
        )));
    }
    for key in requested_sub_task_keys {
        if ancestor_task_keys.iter().any(|ancestor| ancestor == key) {
            return Err(DeadreckonError::InvalidInput(format!(
                "campaign refused: sub-goal task_key '{key}' cycles to an ancestor\n\
                 try: reword the sub-goal so its task differs from an ancestor"
            )));
        }
    }
    for scope in requested_sub_scopes {
        if ancestor_scopes.iter().any(|ancestor| ancestor == scope) {
            return Err(DeadreckonError::InvalidInput(format!(
                "campaign refused: sub-goal scope '{scope}' cycles to an ancestor\n\
                 try: launch the campaign from a different checkout"
            )));
        }
    }
    Ok(())
}

const CAMPAIGN_FILE: &str = "campaign.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignStatus {
    Pending,
    Forked,
    Merged,
    Failed,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubGoalStatus {
    Pending,
    Running,
    Merged,
    Failed,
    Killed,
}

/// One independent workstream of a campaign. Each sub-goal becomes a full
/// `orchestrate full-plan` sub-orchestrator whose merged result is a normal run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubGoal {
    pub sub_id: String,
    pub goal: String,
    pub task_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub status: SubGoalStatus,
}

/// File-backed campaign state at `~/.deadreckon/plans/<campaign-id>/campaign.json`.
/// Adds no fields to `Plan`/`PlanTask`/`PipelineState`; sub-orchestrators produce
/// ordinary plans and ordinary merged runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Campaign {
    pub schema_version: u32,
    pub campaign_id: String,
    pub root_goal: String,
    pub n: u32,
    pub depth: u32,
    pub providers: PlanProviders,
    pub sub_goals: Vec<SubGoal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_wall_seconds: Option<f64>,
    pub status: CampaignStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<DateTime<Utc>>,
    pub deadreckon_version: String,
}

impl Campaign {
    pub fn new(
        root_goal: impl Into<String>,
        sub_goals: Vec<SubGoal>,
        providers: PlanProviders,
        depth: u32,
        tree_budget_usd: Option<f64>,
        tree_wall_seconds: Option<f64>,
        deadreckon_version: impl Into<String>,
    ) -> Result<Self> {
        validate_task_count(sub_goals.len())?;
        Ok(Self {
            schema_version: 1,
            campaign_id: Uuid::new_v4().simple().to_string(),
            root_goal: root_goal.into(),
            n: sub_goals.len() as u32,
            depth,
            providers,
            sub_goals,
            tree_budget_usd,
            tree_wall_seconds,
            status: CampaignStatus::Pending,
            merged_run_id: None,
            created_at: Utc::now(),
            forked_at: None,
            merged_at: None,
            deadreckon_version: deadreckon_version.into(),
        })
    }

    pub fn sub_by_id(&self, sub_id: &str) -> Option<&SubGoal> {
        self.sub_goals.iter().find(|sub| sub.sub_id == sub_id)
    }

    pub fn sub_by_id_mut(&mut self, sub_id: &str) -> Option<&mut SubGoal> {
        self.sub_goals.iter_mut().find(|sub| sub.sub_id == sub_id)
    }

    pub fn sub_task_keys(&self) -> Vec<String> {
        self.sub_goals
            .iter()
            .map(|sub| sub.task_key.clone())
            .collect()
    }
}

/// Turn planner output (N sub-goal strings) into validated [`SubGoal`]s. Refuses a
/// count that does not match the request, an empty sub-goal, or a duplicate
/// (whitespace- and case-normalized).
pub fn build_sub_goals(goals: Vec<String>, requested_n: usize) -> Result<Vec<SubGoal>> {
    if goals.len() != requested_n {
        return Err(DeadreckonError::InvalidInput(format!(
            "campaign planner returned {} sub-goals; expected exactly {requested_n}",
            goals.len()
        )));
    }
    validate_task_count(goals.len())?;
    let mut seen = BTreeSet::new();
    let mut subs = Vec::with_capacity(goals.len());
    for (index, goal) in goals.into_iter().enumerate() {
        let trimmed = goal.trim();
        if trimmed.is_empty() {
            return Err(DeadreckonError::InvalidInput(
                "campaign sub-goal must not be empty".to_string(),
            ));
        }
        let normalized = trimmed
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if !seen.insert(normalized) {
            return Err(DeadreckonError::InvalidInput(format!(
                "duplicate campaign sub-goal: {trimmed}"
            )));
        }
        subs.push(SubGoal {
            sub_id: format!("sub-{index}"),
            goal: trimmed.to_string(),
            task_key: crate::paths::task_key(trimmed),
            sub_plan_id: None,
            result_run_id: None,
            scope: None,
            status: SubGoalStatus::Pending,
        });
    }
    Ok(subs)
}

pub fn campaign_path_for_plan_dir(plan_dir: &Path) -> PathBuf {
    plan_dir.join(CAMPAIGN_FILE)
}

pub fn read_campaign(plan_dir: &Path) -> Result<Campaign> {
    let path = campaign_path_for_plan_dir(plan_dir);
    let bytes = std::fs::read(&path).map_err(|source| DeadreckonError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| DeadreckonError::Json { path, source })
}

pub fn write_campaign(plan_dir: &Path, campaign: &Campaign) -> Result<()> {
    crate::state::atomic_write_json(&campaign_path_for_plan_dir(plan_dir), campaign)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn campaign_at_depth_one_is_refused() {
        // A depth-1 sub-orchestrator launching a campaign would create depth-2
        // work, which the hard cap forbids.
        let err = guard(
            1,
            &[],
            &[],
            &["sub-a".to_string()],
            &["scope-sub".to_string()],
        )
        .expect_err("depth cap must refuse");
        assert!(err.to_string().contains("depth cap"));
        // Depth 0 (a normal top-level campaign) is allowed.
        guard(
            0,
            &[],
            &[],
            &["sub-a".to_string()],
            &["scope-sub".to_string()],
        )
        .expect("top-level campaign allowed");
    }

    #[test]
    fn subgoal_cycling_to_ancestor_task_key_is_refused() {
        let ancestors = vec!["root-key".to_string(), "sub-0-key".to_string()];
        let err = guard(
            0,
            &ancestors,
            &[],
            &["fresh-key".to_string(), "sub-0-key".to_string()],
            &[],
        )
        .expect_err("ancestor task_key cycle must refuse");
        assert!(err.to_string().contains("cycles to an ancestor"));
        assert!(err.to_string().contains("sub-0-key"));
    }

    #[test]
    fn subgoal_cycling_to_ancestor_scope_is_refused() {
        let scopes = vec!["repo-abc123".to_string()];
        let err = guard(
            0,
            &[],
            &scopes,
            &["fresh-key".to_string()],
            &["repo-abc123".to_string()],
        )
        .expect_err("ancestor scope cycle must refuse");
        assert!(err.to_string().contains("cycles to an ancestor"));
        assert!(err.to_string().contains("repo-abc123"));
    }

    #[test]
    fn lineage_round_trips_and_defaults_depth_zero_when_absent() {
        let temp = TempDir::new().expect("tempdir");
        let plan_dir = temp.path().join("plans").join("campaign-1");

        // Absent lineage => depth 0.
        let absent = read_lineage(&plan_dir).expect("absent read");
        assert_eq!(absent.depth, 0);
        assert!(absent.ancestor_task_keys.is_empty());

        let written = Lineage {
            schema_version: 1,
            depth: 1,
            campaign_root_id: Some("campaign-root".to_string()),
            ancestor_task_keys: vec!["root-key".to_string()],
            ancestor_scopes: vec!["repo-abc123".to_string()],
        };
        write_lineage(&plan_dir, &written).expect("write lineage");
        let round_tripped = read_lineage(&plan_dir).expect("read lineage");
        assert_eq!(round_tripped, written);
    }

    #[test]
    fn campaign_plan_rejects_n_outside_2_6() {
        let one =
            build_sub_goals(vec!["only sub-goal".to_string()], 1).expect_err("n=1 must be refused");
        assert!(one.to_string().contains(">= 2"));
        let seven = build_sub_goals((0..7).map(|i| format!("sub-goal {i}")).collect(), 7)
            .expect_err("n=7 must be refused");
        assert!(seven.to_string().contains("capped at 6"));
    }

    #[test]
    fn campaign_plan_rejects_planner_count_mismatch() {
        let err = build_sub_goals(vec!["sub-goal a".to_string(), "sub-goal b".to_string()], 3)
            .expect_err("count mismatch must be refused");
        assert!(err.to_string().contains("expected exactly 3"));
    }

    #[test]
    fn campaign_plan_rejects_duplicate_subgoals() {
        let err = build_sub_goals(
            vec!["Build the API".to_string(), "build   the  api".to_string()],
            2,
        )
        .expect_err("duplicate sub-goals must be refused");
        assert!(err.to_string().contains("duplicate campaign sub-goal"));
    }

    #[test]
    fn campaign_round_trips_through_disk() {
        let temp = TempDir::new().expect("tempdir");
        let plan_dir = temp.path().join("plans").join("campaign-7");
        let subs = build_sub_goals(
            vec![
                "rebuild billing".to_string(),
                "rebuild notifications".to_string(),
            ],
            2,
        )
        .expect("subs");
        let campaign = Campaign::new(
            "rebuild billing and notifications",
            subs,
            PlanProviders::default(),
            0,
            Some(15.0),
            None,
            "0.1.0",
        )
        .expect("campaign");
        write_campaign(&plan_dir, &campaign).expect("write");
        let read = read_campaign(&plan_dir).expect("read");
        assert_eq!(read, campaign);
        assert_eq!(read.n, 2);
        assert_eq!(read.sub_goals[0].sub_id, "sub-0");
        assert!(!read.sub_goals[0].task_key.is_empty());
    }
}
