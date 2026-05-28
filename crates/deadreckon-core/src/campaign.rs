//! Campaign orchestration: one meta-coordinator spawns N independent
//! sub-orchestrators (depth-capped at 2), composes their merged results into one
//! promoted run, and rolls every leaf's tamper-evident gate verdict up to the top.
//!
//! This module owns the file-backed campaign state, the depth/cycle guard, and the
//! lineage record that rides the spawn boundary. Child work stays normal
//! `deadreckon` subprocesses (AS-BUILT §30.1): campaigns add no fields to `Plan`,
//! `PlanTask`, or `PipelineState`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, Result};

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
}
