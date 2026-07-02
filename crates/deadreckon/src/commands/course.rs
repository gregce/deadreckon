//! Course — the durable launch plan behind `deadreckon start`.
//!
//! The mental model is a query planner: the goal is the query, the done
//! contract is the schema plus assertions, `start` is the planner, and the
//! course card is EXPLAIN. This module owns the decision artifact itself:
//! `launch-plan.json`, a typed, additive-tolerant record of WHAT will run
//! (shape and pieces), WHO runs it (per-role providers), HOW MUCH it may
//! spend (budget ceiling and split), what DONE means (the contract signal),
//! WHY the shape was chosen (resolution source, confidence, rationale, and
//! every clamp applied), and the ESCAPE hatches. The plan is a file, not
//! `PipelineState` fields, so dispatch, attach, verdict, and replay can all
//! read the same decision without a schema migration.
//!
//! P1 lands the schema and load/save; the signal bundle, ladder, planner,
//! card, dispatch wiring, replay, collapse, and reshape land in C-P2..C-P13.
// Consumed progressively through C-P13 (dispatch wiring removes the last
// unused-item allowances); until then the schema is exercised by depth tests.
#![allow(dead_code)]

use super::super::*;

/// Schema stamp for `launch-plan.json`; bump only with a migration story.
pub(crate) const LAUNCH_PLAN_SCHEMA: u8 = 1;
/// File name of the durable launch decision inside a dispatched root.
pub(crate) const LAUNCH_PLAN_FILE: &str = "launch-plan.json";

/// The execution shape the course resolves to. `Plan` maps to full-plan
/// orchestration; `ChainExtend` is the continuation shape used when verified
/// history on the same task key makes a follow-up run the right course.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CourseShape {
    Single,
    Plan,
    Campaign,
    ChainExtend,
}

impl CourseShape {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Plan => "plan",
            Self::Campaign => "campaign",
            Self::ChainExtend => "chain-extend",
        }
    }
}

/// Where the resolved done contract came from. Mirrors the Polyglot
/// provenance vocabulary (`detected`/`inferred`) plus the launch-time cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContractOrigin {
    Detected,
    Operator,
    Inferred,
    Asked,
    None,
}

impl ContractOrigin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Operator => "operator",
            Self::Inferred => "inferred",
            Self::Asked => "asked",
            Self::None => "none",
        }
    }
}

/// Who resolved the shape: the deterministic ladder, the provider planner,
/// an explicit operator override, or a `--plan` replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionSource {
    Provider,
    Ladder,
    Operator,
    Replay,
}

impl ResolutionSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Ladder => "ladder",
            Self::Operator => "operator",
            Self::Replay => "replay",
        }
    }
}

/// One unit of planned work. A single run has exactly one piece; a plan has
/// 2..=6; a campaign carries one piece per sub-goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CoursePiece {
    pub(crate) id: String,
    pub(crate) goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) done_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) budget_usd: Option<f64>,
}

/// Per-role provider routes recorded for the launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CourseProviders {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) planner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) coder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reviewer: Option<String>,
}

/// The money and wall-clock envelope the course must fit inside.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CourseBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ceiling_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) split: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wall_seconds: Option<u64>,
}

/// The done contract as the launch saw it (summary, not the spec itself —
/// the spec stays at `acceptance_spec_path_for_run_root`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CourseContract {
    pub(crate) source: ContractOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) caveat: Option<String>,
}

impl Default for CourseContract {
    fn default() -> Self {
        Self {
            source: ContractOrigin::None,
            kind: None,
            summary: None,
            caveat: None,
        }
    }
}

/// Why this shape: who decided, how confident, and every clamp applied on
/// the way — the plan is self-explaining forever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CourseResolution {
    pub(crate) source: ResolutionSource,
    pub(crate) confidence: f64,
    pub(crate) rationale: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) clamps_applied: Vec<String>,
}

/// The one-command escape hatches shown on the card and recorded for audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CourseEscape {
    pub(crate) kill: String,
    pub(crate) undo: String,
}

impl Default for CourseEscape {
    fn default() -> Self {
        Self {
            kill: "deadreckon kill latest".to_string(),
            undo: "deadreckon undo latest".to_string(),
        }
    }
}

/// The durable launch decision. Additive-tolerant: unknown fields are
/// ignored on load so older binaries never choke on newer sidecar data, and
/// every newer field defaults so older plans stay readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LaunchPlan {
    pub(crate) schema: u8,
    pub(crate) created_at: String,
    pub(crate) goal: String,
    pub(crate) shape: CourseShape,
    pub(crate) pieces: Vec<CoursePiece>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) n: Option<u8>,
    #[serde(default)]
    pub(crate) providers: CourseProviders,
    #[serde(default)]
    pub(crate) budget: CourseBudget,
    #[serde(default)]
    pub(crate) contract: CourseContract,
    /// The SignalBundle the decision saw, embedded verbatim (C-P2/C-P3).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub(crate) signals: serde_json::Value,
    pub(crate) resolution: CourseResolution,
    #[serde(default)]
    pub(crate) escape: CourseEscape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) accepted_by: Option<String>,
    /// Parent run id when this plan is a reshape proposal (C-P12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
}

impl LaunchPlan {
    /// A minimal well-formed plan; call sites layer signals/pieces on top.
    pub(crate) fn new(goal: &str, shape: CourseShape, resolution: CourseResolution) -> Self {
        Self {
            schema: LAUNCH_PLAN_SCHEMA,
            created_at: chrono::Utc::now().to_rfc3339(),
            goal: goal.to_string(),
            shape,
            pieces: Vec::new(),
            n: None,
            providers: CourseProviders::default(),
            budget: CourseBudget::default(),
            contract: CourseContract::default(),
            signals: serde_json::Value::Null,
            resolution,
            escape: CourseEscape::default(),
            accepted_by: None,
            parent: None,
        }
    }
}

/// How decomposable the goal text reads: enumerations, conjunction clauses,
/// and imperative verbs. Pure text analysis — no provider, no filesystem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DecompositionHints {
    pub(crate) enumerated_items: usize,
    pub(crate) conjunction_clauses: usize,
    pub(crate) imperative_verbs: usize,
    pub(crate) goal_words: usize,
    pub(crate) strong: bool,
}

const IMPERATIVE_VERBS: &[&str] = &[
    "add",
    "build",
    "create",
    "document",
    "fix",
    "implement",
    "migrate",
    "refactor",
    "remove",
    "rename",
    "rewrite",
    "ship",
    "test",
    "update",
    "upgrade",
    "wire",
    "write",
];

const CLAUSE_STOPWORDS: &[&str] = &["a", "an", "and", "as", "for", "the", "then", "to", "with"];

/// Analyze the goal text for decomposability. `strong` means the goal names
/// separable pieces explicitly (a numbered/bulleted list) or reads as two or
/// more imperative clauses — the textual half of the plan-shape signal.
pub(crate) fn analyze_goal_structure(goal: &str) -> DecompositionHints {
    let lower = goal.to_ascii_lowercase();
    let goal_words = lower.split_whitespace().count();
    let enumerated_items = count_enumerated_items(&lower);
    let clauses = split_conjunction_clauses(&lower);
    let conjunction_clauses = clauses.len();
    let imperative_verbs = clauses
        .iter()
        .filter(|clause| {
            clause
                .split_whitespace()
                .next()
                .is_some_and(|first| IMPERATIVE_VERBS.contains(&first))
        })
        .count();
    let strong = enumerated_items >= 2 || (conjunction_clauses >= 2 && imperative_verbs >= 2);
    DecompositionHints {
        enumerated_items,
        conjunction_clauses,
        imperative_verbs,
        goal_words,
        strong,
    }
}

/// Count explicit list markers: `1.` / `2)` numbered tokens or leading `-`
/// bullets. The max of the two counts, not the sum — a goal that uses both
/// styles is still one list.
fn count_enumerated_items(lower: &str) -> usize {
    let mut numbered = 0usize;
    for token in lower.split_whitespace() {
        let trimmed = token.trim_end_matches(['.', ')', ':']);
        if !trimmed.is_empty()
            && trimmed.len() <= 2
            && trimmed.chars().all(|c| c.is_ascii_digit())
            && token.len() > trimmed.len()
        {
            numbered += 1;
        }
    }
    let bullets = lower
        .split(['\n', ';'])
        .filter(|line| line.trim_start().starts_with("- "))
        .count();
    numbered.max(bullets)
}

/// Split on clause separators and keep clauses that carry a real word.
fn split_conjunction_clauses(lower: &str) -> Vec<String> {
    let normalized = lower
        .replace(", and ", "|")
        .replace(" and ", "|")
        .replace(" then ", "|")
        .replace([';', ','], "|");
    normalized
        .split('|')
        .map(str::trim)
        .filter(|clause| {
            clause
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .filter(|word| word.len() >= 3)
                .any(|word| !CLAUSE_STOPWORDS.contains(&word))
        })
        .map(str::to_string)
        .collect()
}

/// Tree-size bucket, counted with hard caps so the scan is always cheap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TreeBucket {
    #[default]
    Small,
    Medium,
    Large,
}

/// The workspace shape: member count/names (a parallelism map) and size.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceSignal {
    pub(crate) members: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) member_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) tree_bucket: TreeBucket,
}

/// Scan the working dir for workspace structure. Pure over the filesystem,
/// total: unreadable/absent manifests degrade to a single-package signal.
pub(crate) fn scan_workspace(dir: &Path) -> WorkspaceSignal {
    let (kind, member_names) = workspace_members(dir);
    WorkspaceSignal {
        members: member_names.len(),
        member_names: member_names.into_iter().take(8).collect(),
        kind,
        tree_bucket: tree_bucket(dir),
    }
}

fn workspace_members(dir: &Path) -> (Option<String>, Vec<String>) {
    if let Ok(raw) = fs::read_to_string(dir.join("Cargo.toml"))
        && let Ok(value) = toml::from_str::<toml::Value>(&raw)
        && let Some(members) = value
            .get("workspace")
            .and_then(|ws| ws.get("members"))
            .and_then(toml::Value::as_array)
    {
        let names: Vec<String> = members
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::to_string)
            .collect();
        if !names.is_empty() {
            return (Some("cargo".to_string()), names);
        }
    }
    if let Ok(raw) = fs::read_to_string(dir.join("pnpm-workspace.yaml")) {
        let names: Vec<String> = raw
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- "))
            .map(|entry| entry.trim_matches(['"', '\'']).to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        if !names.is_empty() {
            return (Some("pnpm".to_string()), names);
        }
    }
    if let Ok(raw) = fs::read_to_string(dir.join("package.json"))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(entries) = value.get("workspaces").and_then(|w| w.as_array())
    {
        let names: Vec<String> = entries
            .iter()
            .filter_map(|e| e.as_str())
            .map(str::to_string)
            .collect();
        if !names.is_empty() {
            return (Some("npm".to_string()), names);
        }
    }
    if let Ok(raw) = fs::read_to_string(dir.join("go.work")) {
        let names: Vec<String> = raw
            .lines()
            .filter_map(|line| line.trim().strip_prefix("use "))
            .map(|entry| entry.trim_matches(['(', ')']).trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        if !names.is_empty() {
            return (Some("go-work".to_string()), names);
        }
    }
    (None, Vec::new())
}

/// Bounded recursive file count: stops past the medium threshold or depth 6,
/// and skips dot-dirs, `target`, and `node_modules` so the scan stays cheap.
fn tree_bucket(dir: &Path) -> TreeBucket {
    const SMALL: usize = 200;
    const MEDIUM: usize = 2000;
    let mut count = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((path, depth)) = stack.pop() {
        if depth > 6 || count > MEDIUM {
            break;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else {
                count += 1;
                if count > MEDIUM {
                    break;
                }
            }
        }
    }
    if count <= SMALL {
        TreeBucket::Small
    } else if count <= MEDIUM {
        TreeBucket::Medium
    } else {
        TreeBucket::Large
    }
}

/// The detected done contract as a launch signal (summary only; the gate
/// compiles the real spec at run time via the same detector).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContractSignal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) caveat: Option<String>,
    pub(crate) detected: bool,
}

/// Reuses the Polyglot detector so `start` and the gate agree on what "done"
/// will mean before any money is spent.
pub(crate) fn contract_signal(working_dir: &Path) -> ContractSignal {
    let kind = deadreckon_core::acceptance_defaults::detect_project_kind(working_dir);
    let command = deadreckon_core::acceptance_defaults::default_command_for(&kind, working_dir);
    let caveat = deadreckon_core::acceptance_defaults::detection_caveat(&kind);
    let detected = command.is_some() && caveat.is_none();
    ContractSignal {
        kind: Some(deadreckon_core::acceptance_defaults::kind_label(&kind)),
        command,
        caveat,
        detected,
    }
}

/// Prior work on this task key: the continuation signal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HistorySignal {
    pub(crate) prior_runs: usize,
    pub(crate) last_verified_same_task: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_status: Option<String>,
}

/// Prior runs whose goal derives the same task key in this workspace scope.
/// Total: unreadable scope/state degrades to the empty signal.
pub(crate) fn history_signal(paths: &DeadreckonPaths, cwd: &Path, goal: &str) -> HistorySignal {
    let Ok(scope) = workspace_scope(cwd) else {
        return HistorySignal::default();
    };
    let key = task_key(goal);
    let Ok(mut runs) = deadreckon_core::list_runs(paths, Some(&scope)) else {
        return HistorySignal::default();
    };
    runs.retain(|entry| task_key(&entry.goal) == key);
    runs.sort_by_key(|entry| entry.updated_at);
    let last = runs.last();
    HistorySignal {
        prior_runs: runs.len(),
        last_verified_same_task: last
            .map(|entry| matches!(entry.status, deadreckon_core::RunStatus::Completed))
            .unwrap_or(false),
        last_run_id: last.map(|entry| entry.run_id.clone()),
        last_status: last.map(|entry| format!("{:?}", entry.status).to_ascii_lowercase()),
    }
}

/// Whether the requested/default money envelope fits each shape. A shape the
/// budget cannot fund is never proposed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BudgetSignal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ceiling_usd: Option<f64>,
    pub(crate) plan_feasible: bool,
    pub(crate) campaign_feasible: bool,
}

/// The smallest sensible per-piece spend; a plan of n needs n of these.
pub(crate) const MIN_PIECE_BUDGET_USD: f64 = 1.0;
pub(crate) const MIN_PLAN_PIECES: u8 = 2;
/// A campaign multiplies plan cost by its sub count.
pub(crate) const MIN_CAMPAIGN_SUBS: u8 = 2;

pub(crate) fn plan_feasible_floor(n: u8) -> f64 {
    f64::from(n.max(MIN_PLAN_PIECES)) * MIN_PIECE_BUDGET_USD
}

pub(crate) fn campaign_feasible_floor(n: u8) -> f64 {
    f64::from(n.max(MIN_CAMPAIGN_SUBS)) * plan_feasible_floor(MIN_PLAN_PIECES)
}

pub(crate) fn budget_signal(ceiling_usd: Option<f64>) -> BudgetSignal {
    BudgetSignal {
        ceiling_usd,
        plan_feasible: ceiling_usd.is_none_or(|c| c >= plan_feasible_floor(MIN_PLAN_PIECES)),
        campaign_feasible: ceiling_usd
            .is_none_or(|c| c >= campaign_feasible_floor(MIN_CAMPAIGN_SUBS)),
    }
}

/// Everything the shape decision saw, computed free and embedded verbatim in
/// the launch plan for audit. Total: every field degrades, none error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SignalBundle {
    pub(crate) decomposability: DecompositionHints,
    pub(crate) contract: ContractSignal,
    pub(crate) workspace: WorkspaceSignal,
    pub(crate) history: HistorySignal,
    pub(crate) budget: BudgetSignal,
}

pub(crate) fn collect_signal_bundle(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    ceiling_usd: Option<f64>,
) -> SignalBundle {
    SignalBundle {
        decomposability: analyze_goal_structure(goal),
        contract: contract_signal(cwd),
        workspace: scan_workspace(cwd),
        history: history_signal(paths, cwd, goal),
        budget: budget_signal(ceiling_usd),
    }
}

/// Where the plan lives inside a dispatched root (run root, plan dir,
/// campaign dir, chain dir).
pub(crate) fn launch_plan_path(root: &Path) -> PathBuf {
    root.join(LAUNCH_PLAN_FILE)
}

/// Persist a plan as pretty JSON, creating parents. Matches the sidecar
/// convention used by the goal-shape preview record and the verdict cache.
pub(crate) fn save_launch_plan(path: &Path, plan: &LaunchPlan) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(plan).map_err(|source| DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })?,
    )?;
    Ok(())
}

/// Load and validate a plan. A missing file, unparseable JSON, or an unknown
/// schema stamp is a refusal with a `try:` footer — never a guess.
pub(crate) fn load_launch_plan(path: &Path) -> Result<LaunchPlan> {
    let raw = fs::read_to_string(path).map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("launch plan not found at {}", path.display()),
            "deadreckon start \"<goal>\"",
        ))
    })?;
    let plan: LaunchPlan = serde_json::from_str(&raw).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("launch plan at {} is not valid: {err}", path.display()),
            "deadreckon start \"<goal>\"",
        ))
    })?;
    if plan.schema != LAUNCH_PLAN_SCHEMA {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "launch plan at {} has unsupported schema {} (this build reads schema {LAUNCH_PLAN_SCHEMA})",
                path.display(),
                plan.schema
            ),
            "deadreckon start \"<goal>\"",
        )));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> LaunchPlan {
        let mut plan = LaunchPlan::new(
            "add rate limiting to the API",
            CourseShape::Plan,
            CourseResolution {
                source: ResolutionSource::Provider,
                confidence: 0.82,
                rationale: "three independently testable pieces".to_string(),
                clamps_applied: vec!["n clamped 8->6".to_string()],
            },
        );
        plan.n = Some(3);
        plan.pieces = vec![
            CoursePiece {
                id: "p1".to_string(),
                goal: "token-bucket limiter core".to_string(),
                done_hint: Some("unit tests for refill + burst".to_string()),
                role: Some("coder".to_string()),
                provider: Some("cli:claude-code".to_string()),
                model: None,
                budget_usd: Some(5.0),
            },
            CoursePiece {
                id: "p2".to_string(),
                goal: "config surface in limits.toml".to_string(),
                done_hint: None,
                role: Some("coder".to_string()),
                provider: None,
                model: None,
                budget_usd: Some(3.0),
            },
        ];
        plan.providers = CourseProviders {
            planner: Some("cli:claude-code".to_string()),
            coder: Some("cli:claude-code".to_string()),
            reviewer: Some("cli:codex".to_string()),
        };
        plan.budget = CourseBudget {
            ceiling_usd: Some(12.0),
            split: vec![5.0, 3.0, 4.0],
            wall_seconds: None,
        };
        plan.contract = CourseContract {
            source: ContractOrigin::Detected,
            kind: Some("Node(pnpm)".to_string()),
            summary: Some("pnpm test".to_string()),
            caveat: None,
        };
        plan
    }

    #[test]
    fn launch_plan_roundtrips_serde() {
        let plan = sample_plan();
        let json = serde_json::to_string_pretty(&plan).expect("serialize");
        let back: LaunchPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plan, back);
        assert!(json.contains("\"shape\": \"plan\""));
        assert!(json.contains("\"source\": \"detected\""));
    }

    #[test]
    fn launch_plan_unknown_fields_tolerated_schema_checked() {
        let mut value = serde_json::to_value(sample_plan()).expect("to_value");
        value["future_field"] = serde_json::json!({"anything": true});
        value["resolution"]["future_hint"] = serde_json::json!("ignored");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = launch_plan_path(dir.path());
        fs::write(&path, serde_json::to_vec_pretty(&value).expect("bytes")).expect("write");
        let plan = load_launch_plan(&path).expect("unknown fields must be tolerated");
        assert_eq!(plan.schema, LAUNCH_PLAN_SCHEMA);
        assert_eq!(plan.shape, CourseShape::Plan);
        assert_eq!(plan.pieces.len(), 2);
    }

    #[test]
    fn invalid_plan_schema_refuses_with_try() {
        let mut plan = sample_plan();
        plan.schema = 99;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = launch_plan_path(dir.path());
        save_launch_plan(&path, &plan).expect("save");
        let err = load_launch_plan(&path).expect_err("schema 99 must refuse");
        let message = err.to_string();
        assert!(message.contains("unsupported schema 99"), "{message}");
        assert!(
            message.contains("try:"),
            "refusal must carry a try footer: {message}"
        );

        let garbled = dir.path().join("garbled.json");
        fs::write(&garbled, b"not json").expect("write");
        let err = load_launch_plan(&garbled).expect_err("garbage must refuse");
        assert!(err.to_string().contains("try:"), "{err}");

        let missing = dir.path().join("missing.json");
        let err = load_launch_plan(&missing).expect_err("missing must refuse");
        assert!(err.to_string().contains("try:"), "{err}");
    }

    #[test]
    fn save_creates_parents_and_roundtrips_through_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("runstate/scope/task/run");
        let path = launch_plan_path(&nested);
        let plan = sample_plan();
        save_launch_plan(&path, &plan).expect("save creates parents");
        let back = load_launch_plan(&path).expect("load");
        assert_eq!(plan, back);
    }

    // ---- C-P2: decomposability + workspace signals ----

    #[test]
    fn enumerated_goal_yields_strong_decomposability() {
        let numbered = analyze_goal_structure(
            "1. add a token-bucket limiter 2. add the config surface 3. wire it into main",
        );
        assert!(numbered.enumerated_items >= 2, "{numbered:?}");
        assert!(numbered.strong, "{numbered:?}");

        let clauses = analyze_goal_structure(
            "add rate limiting to the api and write the config docs then wire the ci gate",
        );
        assert!(clauses.conjunction_clauses >= 2, "{clauses:?}");
        assert!(clauses.imperative_verbs >= 2, "{clauses:?}");
        assert!(clauses.strong, "{clauses:?}");
    }

    #[test]
    fn single_sentence_goal_is_weak() {
        let hints = analyze_goal_structure("fix the typo in the readme header");
        assert!(!hints.strong, "{hints:?}");
        assert_eq!(hints.enumerated_items, 0, "{hints:?}");
    }

    #[test]
    fn cargo_workspace_members_counted() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\", \"crates/c\"]\n",
        )
        .expect("write");
        let signal = scan_workspace(dir.path());
        assert_eq!(signal.members, 3, "{signal:?}");
        assert_eq!(signal.kind.as_deref(), Some("cargo"), "{signal:?}");
        assert_eq!(signal.member_names[0], "crates/a");
        assert_eq!(signal.tree_bucket, TreeBucket::Small);

        let empty = tempfile::tempdir().expect("tempdir");
        let none = scan_workspace(empty.path());
        assert_eq!(none.members, 0, "{none:?}");
        assert!(none.kind.is_none(), "{none:?}");
    }

    // ---- C-P3: contract + history + budget signals ----

    #[test]
    fn contract_signal_reuses_polyglot_detection() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("go.mod"), "module example.com/m\n").expect("write");
        let signal = contract_signal(dir.path());
        assert_eq!(
            signal.command.as_deref(),
            Some("go test ./..."),
            "{signal:?}"
        );
        assert!(signal.detected, "{signal:?}");
        assert!(signal.caveat.is_none(), "{signal:?}");

        let empty = tempfile::tempdir().expect("tempdir");
        let unknown = contract_signal(empty.path());
        assert!(!unknown.detected, "{unknown:?}");
        assert!(unknown.caveat.is_some(), "{unknown:?}");
    }

    #[test]
    fn prior_verified_run_sets_continuation_signal() {
        use deadreckon_core::paths::DeadreckonPaths;
        use deadreckon_core::state::{RunOptions, create_run, save_state};
        use deadreckon_core::{PhaseId, PhaseStatus};

        let temp = tempfile::tempdir().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let goal = "add rate limiting to the api";
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: goal.to_string(),
                cwd: repo.clone(),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("course-hist-0001".to_string()),
                codebase: None,
            },
        )
        .expect("create_run");
        state
            .set_phase_status(PhaseId(60), PhaseStatus::Completed)
            .expect("complete");
        save_state(&state).expect("save");

        let signal = history_signal(&paths, &repo, goal);
        assert_eq!(signal.prior_runs, 1, "{signal:?}");
        assert!(signal.last_verified_same_task, "{signal:?}");
        assert_eq!(signal.last_run_id.as_deref(), Some("course-hist-0001"));

        let other = history_signal(&paths, &repo, "a completely different goal");
        assert_eq!(other.prior_runs, 0, "{other:?}");
        assert!(!other.last_verified_same_task, "{other:?}");
    }

    #[test]
    fn budget_below_plan_floor_marks_plan_infeasible() {
        let tiny = budget_signal(Some(1.0));
        assert!(!tiny.plan_feasible, "{tiny:?}");
        assert!(!tiny.campaign_feasible, "{tiny:?}");

        let plan_only = budget_signal(Some(3.0));
        assert!(plan_only.plan_feasible, "{plan_only:?}");
        assert!(!plan_only.campaign_feasible, "{plan_only:?}");

        let open = budget_signal(None);
        assert!(open.plan_feasible, "{open:?}");
        assert!(open.campaign_feasible, "{open:?}");
    }
}
