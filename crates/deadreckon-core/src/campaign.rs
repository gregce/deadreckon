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
use crate::plan::{PlanProviders, RootPlannerAccounting, validate_task_count};
use crate::tamper::AcceptanceTamperVerdict;

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
    /// Crash-safe copy of root-planner usage. Campaign events remain the rich
    /// reporting ledger, but recovery can rebuild a missing event from this
    /// immutable creation fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_planner_accounting: Option<RootPlannerAccounting>,
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
            root_planner_accounting: None,
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

/// Environment variables that carry campaign lineage across the spawn boundary.
/// The meta-coordinator sets these on each sub-orchestrator subprocess; the
/// sub-orchestrator reads them at startup (before it has a plan dir) to learn its
/// depth, and writes its result to the sidecar named by [`ENV_SUB_RESULT`].
pub const ENV_DEPTH: &str = "DEADRECKON_CAMPAIGN_DEPTH";
pub const ENV_ROOT: &str = "DEADRECKON_CAMPAIGN_ROOT";
pub const ENV_ANCESTOR_TASK_KEYS: &str = "DEADRECKON_CAMPAIGN_ANCESTOR_TASK_KEYS";
pub const ENV_ANCESTOR_SCOPES: &str = "DEADRECKON_CAMPAIGN_ANCESTOR_SCOPES";
pub const ENV_SUB_RESULT: &str = "DEADRECKON_CAMPAIGN_SUB_RESULT";
pub const ENV_SUB_PLAN_ID: &str = "DEADRECKON_CAMPAIGN_SUB_PLAN_ID";

const SUB_RESULT_FILE: &str = "sub-result.json";

/// What a sub-orchestrator reports back to the meta-coordinator: the plan it ran
/// and the normal run its merge promoted. Written to `<launch-dir>/sub-result.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubResult {
    pub schema_version: u32,
    pub sub_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_run_id: Option<String>,
    pub ok: bool,
}

pub fn sub_result_path(launch_dir: &Path) -> PathBuf {
    launch_dir.join(SUB_RESULT_FILE)
}

pub fn read_sub_result(launch_dir: &Path) -> Result<Option<SubResult>> {
    let path = sub_result_path(launch_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| DeadreckonError::Json { path, source }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

pub fn write_sub_result(launch_dir: &Path, result: &SubResult) -> Result<()> {
    crate::state::atomic_write_json(&sub_result_path(launch_dir), result)
}

/// Build a [`Lineage`] from the campaign env-var values (any may be absent). This
/// is the pure core of reading lineage out of the process environment; absent
/// depth means depth 0 (a top-level invocation).
pub fn parse_lineage(
    depth: Option<&str>,
    campaign_root_id: Option<&str>,
    ancestor_task_keys: Option<&str>,
    ancestor_scopes: Option<&str>,
) -> Lineage {
    let split = |value: Option<&str>| -> Vec<String> {
        value
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    Lineage {
        schema_version: 1,
        depth: depth.and_then(|raw| raw.trim().parse().ok()).unwrap_or(0),
        campaign_root_id: campaign_root_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        ancestor_task_keys: split(ancestor_task_keys),
        ancestor_scopes: split(ancestor_scopes),
    }
}

/// Read the campaign lineage from the current process environment.
pub fn lineage_from_env() -> Lineage {
    parse_lineage(
        std::env::var(ENV_DEPTH).ok().as_deref(),
        std::env::var(ENV_ROOT).ok().as_deref(),
        std::env::var(ENV_ANCESTOR_TASK_KEYS).ok().as_deref(),
        std::env::var(ENV_ANCESTOR_SCOPES).ok().as_deref(),
    )
}

const CAMPAIGN_EVENTS_FILE: &str = "campaign-events.jsonl";

/// Append-only campaign timeline, mirroring `plan-events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampaignEvent {
    pub schema_version: u32,
    pub ts: DateTime<Utc>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub detail: serde_json::Value,
}

pub fn campaign_events_path(plan_dir: &Path) -> PathBuf {
    plan_dir.join(CAMPAIGN_EVENTS_FILE)
}

pub fn append_campaign_event(
    plan_dir: &Path,
    kind: impl Into<String>,
    detail: serde_json::Value,
) -> Result<()> {
    let event = CampaignEvent {
        schema_version: 1,
        ts: Utc::now(),
        kind: kind.into(),
        detail,
    };
    crate::state::append_json_line(&campaign_events_path(plan_dir), &event)
}

pub fn read_campaign_events(plan_dir: &Path) -> Result<Vec<CampaignEvent>> {
    let path = campaign_events_path(plan_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|source| DeadreckonError::Json {
                    path: path.clone(),
                    source,
                })
            })
            .collect(),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

/// Split a campaign's tree-wide spend ceiling evenly across N sub-orchestrators.
/// Computed in whole cents so the shares sum back to the ceiling exactly; any
/// leftover cent goes to the earliest subs (remainder-to-first).
pub fn allocate_budget(tree_budget: f64, n: usize) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let total_cents = (tree_budget * 100.0).round() as i64;
    let base = total_cents / n as i64;
    let remainder = total_cents - base * n as i64;
    (0..n)
        .map(|index| {
            let cents = if (index as i64) < remainder {
                base + 1
            } else {
                base
            };
            cents as f64 / 100.0
        })
        .collect()
}

/// Whether the aggregate spend across already-launched leaf runs has reached the
/// campaign's tree ceiling. An absent ceiling is never exhausted.
pub fn tree_budget_exhausted(tree_budget: Option<f64>, spent_usd: f64) -> bool {
    tree_budget.is_some_and(|cap| spent_usd >= cap)
}

/// The warning to surface when a campaign runs with no tree budget: each
/// sub-orchestrator falls back to the default per-run cap, so the whole tree is
/// unbounded. Returns `None` when a ceiling is set.
pub fn unbounded_budget_warning(tree_budget: Option<f64>) -> Option<String> {
    if tree_budget.is_none() {
        Some(
            "campaign tree budget is unbounded: each sub-orchestrator inherits the default \
             per-run cap. pass --max-spend to bound the whole tree"
                .to_string(),
        )
    } else {
        None
    }
}

const ROLLUP_FILE: &str = "campaign-rollup.json";

/// The aggregate trust verdict of a campaign, worst-of across every leaf run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupVerdict {
    Clean,
    Caveat,
    Refused,
}

/// One leaf run's contribution to the roll-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafVerdict {
    pub sub_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_run_id: Option<String>,
    /// "signed" | "refused" | "missing" — whether the leaf produced a valid gate.
    pub gate: String,
    pub tamper_verdict: AcceptanceTamperVerdict,
    #[serde(default)]
    pub caveats: Vec<String>,
}

impl LeafVerdict {
    /// The leaf's effective trust verdict: a leaf with no valid signed gate counts
    /// as refused regardless of its tamper verdict.
    pub fn effective(&self) -> RollupVerdict {
        if self.gate != "signed" {
            return RollupVerdict::Refused;
        }
        match self.tamper_verdict {
            AcceptanceTamperVerdict::Clean => RollupVerdict::Clean,
            AcceptanceTamperVerdict::Caveat => RollupVerdict::Caveat,
            AcceptanceTamperVerdict::Refuse => RollupVerdict::Refused,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampaignRollup {
    pub schema_version: u32,
    pub campaign_id: String,
    pub evaluated_at: DateTime<Utc>,
    pub leaves: Vec<LeafVerdict>,
    pub rollup_verdict: RollupVerdict,
    pub refused_subs: Vec<String>,
    pub caveat_subs: Vec<String>,
}

/// Worst-of across leaf verdicts: any refused wins, else any caveat, else clean.
/// An empty leaf set is refused (a campaign with nothing merged is not clean).
pub fn worst_of(verdicts: &[RollupVerdict]) -> RollupVerdict {
    if verdicts.is_empty() {
        return RollupVerdict::Refused;
    }
    if verdicts.contains(&RollupVerdict::Refused) {
        RollupVerdict::Refused
    } else if verdicts.contains(&RollupVerdict::Caveat) {
        RollupVerdict::Caveat
    } else {
        RollupVerdict::Clean
    }
}

/// A campaign may reach clean completion only if no leaf was refused. This is the
/// no-laundering invariant: nesting cannot turn a refused leaf into a clean result.
pub fn rollup_permits_completion(verdict: RollupVerdict) -> bool {
    verdict != RollupVerdict::Refused
}

/// Build the roll-up from a campaign and a per-leaf lookup that yields each result
/// run's gate state, tamper verdict, and caveats. A sub with no merged result run
/// is recorded as a missing (refused) leaf.
pub fn build_rollup<F>(campaign: &Campaign, mut leaf_lookup: F) -> CampaignRollup
where
    F: FnMut(&str) -> (String, AcceptanceTamperVerdict, Vec<String>),
{
    let mut leaves = Vec::new();
    for sub in &campaign.sub_goals {
        let leaf = match sub.result_run_id.as_deref() {
            Some(run_id) => {
                let (gate, tamper_verdict, caveats) = leaf_lookup(run_id);
                LeafVerdict {
                    sub_id: sub.sub_id.clone(),
                    result_run_id: Some(run_id.to_string()),
                    gate,
                    tamper_verdict,
                    caveats,
                }
            }
            None => LeafVerdict {
                sub_id: sub.sub_id.clone(),
                result_run_id: None,
                gate: "missing".to_string(),
                tamper_verdict: AcceptanceTamperVerdict::Refuse,
                caveats: Vec::new(),
            },
        };
        leaves.push(leaf);
    }
    let effective: Vec<RollupVerdict> = leaves.iter().map(LeafVerdict::effective).collect();
    let refused_subs = leaves
        .iter()
        .filter(|leaf| leaf.effective() == RollupVerdict::Refused)
        .map(|leaf| leaf.sub_id.clone())
        .collect();
    let caveat_subs = leaves
        .iter()
        .filter(|leaf| leaf.effective() == RollupVerdict::Caveat)
        .map(|leaf| leaf.sub_id.clone())
        .collect();
    CampaignRollup {
        schema_version: 1,
        campaign_id: campaign.campaign_id.clone(),
        evaluated_at: Utc::now(),
        leaves,
        rollup_verdict: worst_of(&effective),
        refused_subs,
        caveat_subs,
    }
}

/// A campaign reaches clean completion only when every sub merged *and* the
/// roll-up permits completion (no refused leaf). This is where the no-laundering
/// invariant meets the all-subs-done requirement.
pub fn campaign_can_complete(campaign: &Campaign, rollup: &CampaignRollup) -> bool {
    campaign
        .sub_goals
        .iter()
        .all(|sub| sub.status == SubGoalStatus::Merged)
        && rollup_permits_completion(rollup.rollup_verdict)
}

pub fn rollup_path_for_plan_dir(plan_dir: &Path) -> PathBuf {
    plan_dir.join(ROLLUP_FILE)
}

/// Where a campaign result run carries its bound roll-up (hashed into the run's
/// acceptance-marker signature by the gate).
pub fn rollup_path_at_run_root(run_root: &Path) -> PathBuf {
    run_root.join(ROLLUP_FILE)
}

pub fn write_campaign_rollup(plan_dir: &Path, rollup: &CampaignRollup) -> Result<()> {
    crate::state::atomic_write_json(&rollup_path_for_plan_dir(plan_dir), rollup)
}

/// Write the roll-up into a result run's root so the gate binds it into the
/// acceptance-marker signature (the no-laundering guarantee).
pub fn write_campaign_rollup_at_run_root(run_root: &Path, rollup: &CampaignRollup) -> Result<()> {
    crate::state::atomic_write_json(&rollup_path_at_run_root(run_root), rollup)
}

pub fn read_campaign_rollup(plan_dir: &Path) -> Result<CampaignRollup> {
    let path = rollup_path_for_plan_dir(plan_dir);
    let bytes = std::fs::read(&path).map_err(|source| DeadreckonError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| DeadreckonError::Json { path, source })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn lookup_clean(_run_id: &str) -> (String, AcceptanceTamperVerdict, Vec<String>) {
        (
            "signed".to_string(),
            AcceptanceTamperVerdict::Clean,
            Vec::new(),
        )
    }

    fn campaign_with_results() -> Campaign {
        let mut campaign = Campaign::new(
            "root",
            build_sub_goals(vec!["sub a".to_string(), "sub b".to_string()], 2).expect("subs"),
            PlanProviders::default(),
            0,
            Some(10.0),
            None,
            "0.1.0",
        )
        .expect("campaign");
        campaign.sub_goals[0].result_run_id = Some("run-0".to_string());
        campaign.sub_goals[1].result_run_id = Some("run-1".to_string());
        campaign
    }

    #[test]
    fn all_clean_leaves_yield_clean_rollup() {
        let campaign = campaign_with_results();
        let rollup = build_rollup(&campaign, lookup_clean);
        assert_eq!(rollup.rollup_verdict, RollupVerdict::Clean);
        assert!(rollup_permits_completion(rollup.rollup_verdict));
        assert!(rollup.refused_subs.is_empty());
        assert!(rollup.caveat_subs.is_empty());
    }

    #[test]
    fn caveat_leaf_surfaces_caveat_but_campaign_completes() {
        let campaign = campaign_with_results();
        let rollup = build_rollup(&campaign, |run_id| {
            if run_id == "run-1" {
                (
                    "signed".to_string(),
                    AcceptanceTamperVerdict::Caveat,
                    vec!["agent modified tests/auth_test.rs".to_string()],
                )
            } else {
                lookup_clean(run_id)
            }
        });
        assert_eq!(rollup.rollup_verdict, RollupVerdict::Caveat);
        assert!(rollup_permits_completion(rollup.rollup_verdict));
        assert_eq!(rollup.caveat_subs, vec!["sub-1"]);
        assert!(rollup.refused_subs.is_empty());
    }

    #[test]
    fn refused_leaf_makes_campaign_fail_and_blocks_clean_completion() {
        let campaign = campaign_with_results();
        let rollup = build_rollup(&campaign, |run_id| {
            if run_id == "run-0" {
                (
                    "signed".to_string(),
                    AcceptanceTamperVerdict::Refuse,
                    Vec::new(),
                )
            } else {
                lookup_clean(run_id)
            }
        });
        assert_eq!(rollup.rollup_verdict, RollupVerdict::Refused);
        assert!(!rollup_permits_completion(rollup.rollup_verdict));
        assert_eq!(rollup.refused_subs, vec!["sub-0"]);

        // A sub that never merged (no result run) is also a refused leaf.
        let mut missing = campaign_with_results();
        missing.sub_goals[1].result_run_id = None;
        let rollup2 = build_rollup(&missing, lookup_clean);
        assert_eq!(rollup2.rollup_verdict, RollupVerdict::Refused);
        assert_eq!(rollup2.refused_subs, vec!["sub-1"]);
    }

    #[test]
    fn campaign_completes_only_when_all_subs_merged_and_no_refusal() {
        let mut campaign = campaign_with_results();
        for sub in &mut campaign.sub_goals {
            sub.status = SubGoalStatus::Merged;
        }
        let clean = build_rollup(&campaign, lookup_clean);
        assert!(campaign_can_complete(&campaign, &clean));

        // A sub that has not merged blocks completion even with a clean roll-up.
        let mut pending = campaign.clone();
        pending.sub_goals[1].status = SubGoalStatus::Pending;
        assert!(!campaign_can_complete(&pending, &clean));
    }

    #[test]
    fn campaign_with_refused_sub_never_reaches_completed() {
        let mut campaign = campaign_with_results();
        for sub in &mut campaign.sub_goals {
            sub.status = SubGoalStatus::Merged;
        }
        // Even with all subs merged, a refused roll-up blocks completion.
        let refused = build_rollup(&campaign, |run_id| {
            if run_id == "run-0" {
                (
                    "signed".to_string(),
                    AcceptanceTamperVerdict::Refuse,
                    Vec::new(),
                )
            } else {
                lookup_clean(run_id)
            }
        });
        assert_eq!(refused.rollup_verdict, RollupVerdict::Refused);
        assert!(!campaign_can_complete(&campaign, &refused));
    }

    #[test]
    fn edited_rollup_file_fails_result_marker_signature() {
        use crate::gate::{validate_acceptance_marker, write_acceptance_marker};
        use crate::paths::DeadreckonPaths;
        use crate::state::{RunOptions, create_run};

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "campaign-result".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");

        // A campaign result run carries its roll-up in the run root; the gate binds
        // it into the marker signature. Start from a refused roll-up so the later
        // edit is a real byte change.
        let refused_campaign = campaign_with_results();
        let rollup = build_rollup(&refused_campaign, |run_id| {
            if run_id == "run-0" {
                (
                    "signed".to_string(),
                    AcceptanceTamperVerdict::Refuse,
                    Vec::new(),
                )
            } else {
                lookup_clean(run_id)
            }
        });
        assert_eq!(rollup.rollup_verdict, RollupVerdict::Refused);
        crate::state::atomic_write_json(&rollup_path_at_run_root(&state.run_root), &rollup)
            .expect("write rollup into run root");
        write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("sign marker");
        validate_acceptance_marker(&state).expect("valid before edit");

        // Editing the bound roll-up after signing (to launder the refusal into a
        // clean pass) invalidates the marker.
        let mut tampered = rollup;
        tampered.rollup_verdict = RollupVerdict::Clean;
        tampered.refused_subs.clear();
        crate::state::atomic_write_json(&rollup_path_at_run_root(&state.run_root), &tampered)
            .expect("edit rollup");
        let err = validate_acceptance_marker(&state).expect_err("signature must reject edit");
        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn tree_budget_splits_evenly_with_remainder_to_first() {
        assert_eq!(allocate_budget(10.0, 2), vec![5.0, 5.0]);
        let three = allocate_budget(10.0, 3);
        assert_eq!(three.len(), 3);
        assert!((three.iter().sum::<f64>() - 10.0).abs() < 1e-9);
        assert!(three[0] >= three[2]); // leftover cent lands on the earliest sub
        assert_eq!(allocate_budget(9.0, 3), vec![3.0, 3.0, 3.0]);
        assert!(allocate_budget(10.0, 0).is_empty());
    }

    #[test]
    fn null_tree_budget_logs_unbounded_warning() {
        assert!(unbounded_budget_warning(None).is_some());
        assert!(unbounded_budget_warning(Some(10.0)).is_none());
        assert!(!tree_budget_exhausted(None, 1_000_000.0));
        assert!(tree_budget_exhausted(Some(10.0), 10.0));
        assert!(!tree_budget_exhausted(Some(10.0), 9.99));
    }

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
    fn parse_lineage_reads_strings_and_defaults_depth_zero() {
        let lineage = parse_lineage(
            Some("1"),
            Some("camp-root"),
            Some("root-key, sub-0-key"),
            Some("repo-abc"),
        );
        assert_eq!(lineage.depth, 1);
        assert_eq!(lineage.campaign_root_id.as_deref(), Some("camp-root"));
        assert_eq!(lineage.ancestor_task_keys, vec!["root-key", "sub-0-key"]);
        assert_eq!(lineage.ancestor_scopes, vec!["repo-abc"]);

        let absent = parse_lineage(None, None, None, None);
        assert_eq!(absent.depth, 0);
        assert!(absent.campaign_root_id.is_none());
        assert!(absent.ancestor_task_keys.is_empty());
    }

    #[test]
    fn sub_result_round_trips_and_absent_is_none() {
        let temp = TempDir::new().expect("tempdir");
        let launch_dir = temp.path().join("launch").join("sub-0");
        assert!(read_sub_result(&launch_dir).expect("absent").is_none());
        let result = SubResult {
            schema_version: 1,
            sub_id: "sub-0".to_string(),
            plan_id: Some("plan-xyz".to_string()),
            result_run_id: Some("run-abc".to_string()),
            ok: true,
        };
        write_sub_result(&launch_dir, &result).expect("write");
        assert_eq!(read_sub_result(&launch_dir).expect("read"), Some(result));
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
