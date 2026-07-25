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
    /// Ids of pieces that must finish before this one starts. Carries the
    /// planner's edges through to `PlanTask::depends_on`, which the fork
    /// executor already honors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) depends_on: Vec<String>,
    /// This piece is a project in its own right: its goal is decomposed and
    /// executed as its own graph, reconciled before this piece's dependents
    /// run. What `campaign` reaches for, except a subplan carries its own
    /// apply mode — so a sub-project may be sequential even when its parent
    /// is parallel, which campaign cannot express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subplan: Option<CourseSubplan>,
}

/// A node's own graph. Same shape as the parent's node list, plus its own
/// apply mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CourseSubplan {
    #[serde(default)]
    pub(crate) apply: deadreckon_core::plan::ApplyWhen,
    pub(crate) pieces: Vec<CoursePiece>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CourseContract {
    pub(crate) source: ContractOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) caveat: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) checks: Vec<super::acceptance::CompiledCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) divergence: Option<super::acceptance::ContractDivergence>,
}

impl Default for CourseContract {
    fn default() -> Self {
        Self {
            source: ContractOrigin::None,
            kind: None,
            summary: None,
            caveat: None,
            checks: Vec::new(),
            divergence: None,
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
    /// The goal verbatim — part of the audit record and the keyword rules.
    #[serde(default)]
    pub(crate) goal: String,
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
        goal: goal.to_string(),
        decomposability: analyze_goal_structure(goal),
        contract: contract_signal(cwd),
        workspace: scan_workspace(cwd),
        history: history_signal(paths, cwd, goal),
        budget: budget_signal(ceiling_usd),
    }
}

/// The proven parallel-workstream keyword heuristic (carried over from the
/// pre-Course classifier): an operator who names parallel work gets a plan.
pub(crate) fn goal_names_parallel_workstreams(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    const WORDS: &[&str] = &[
        "parallel",
        "parallelize",
        "workstream",
        "workstreams",
        "separable",
    ];
    const PHRASES: &[&str] = &[
        "multiple independent",
        "many modules",
        "several modules",
        "frontend, docs",
        "api, frontend",
    ];
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| WORDS.contains(&word))
        || PHRASES.iter().any(|phrase| lower.contains(phrase))
}

/// The ladder's decision: shape + n + which rule fired. Campaign is never a
/// ladder outcome — it requires the provider planner or the operator.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LadderDecision {
    pub(crate) shape: CourseShape,
    pub(crate) n: Option<u8>,
    pub(crate) rationale: String,
    pub(crate) rule: &'static str,
}

pub(crate) const PLAN_MAX_PIECES: u8 = 6;

/// Rules in order; first match decides. Deterministic, total, provider-free —
/// this is the floor the planner can only refine, and the exact behavior of
/// the no-provider path.
pub(crate) fn ladder_decision(signals: &SignalBundle) -> LadderDecision {
    // Rule 1 — verified history on this task key: continue, don't restart.
    if signals.history.last_verified_same_task {
        return LadderDecision {
            shape: CourseShape::ChainExtend,
            n: None,
            rationale: format!(
                "prior verified run{} on this task — continuing beats restarting",
                signals
                    .history
                    .last_run_id
                    .as_deref()
                    .map(|id| format!(" {id}"))
                    .unwrap_or_default()
            ),
            rule: "continuation",
        };
    }
    // Rule 2 — money decides: never propose a shape that cannot fit.
    if !signals.budget.plan_feasible {
        return LadderDecision {
            shape: CourseShape::Single,
            n: None,
            rationale: "budget ceiling below the plan floor — single run fits the money"
                .to_string(),
            rule: "budget",
        };
    }
    let d = &signals.decomposability;
    // Rule 2.5 — the operator named parallel workstreams explicitly; honor
    // the ask (the proven keyword heuristic the old classifier carried).
    if goal_names_parallel_workstreams(&signals.goal) {
        let n = pieces_hint(d).clamp(2, 4);
        return LadderDecision {
            shape: CourseShape::Plan,
            n: Some(n),
            rationale: "goal names parallel or separable workstreams".to_string(),
            rule: "parallel-keywords",
        };
    }
    // Rule 3 — strong decomposition + a workspace parallelism map.
    if d.strong && signals.workspace.members >= 2 {
        let members = u8::try_from(signals.workspace.members).unwrap_or(PLAN_MAX_PIECES);
        let n = pieces_hint(d).min(members).clamp(2, PLAN_MAX_PIECES);
        return LadderDecision {
            shape: CourseShape::Plan,
            n: Some(n),
            rationale: format!(
                "{} separable pieces across {} workspace members",
                pieces_hint(d),
                signals.workspace.members
            ),
            rule: "decomposition+workspace",
        };
    }
    // Rule 4 — strong decomposition in a single-package tree.
    if d.strong {
        let n = pieces_hint(d).clamp(2, 4);
        return LadderDecision {
            shape: CourseShape::Plan,
            n: Some(n),
            rationale: format!("{} separable pieces named in the goal", pieces_hint(d)),
            rule: "decomposition",
        };
    }
    // Rules 5/7 — focused goal (or nothing else fired): one supervised run.
    // Rule 6 (campaign) intentionally does not exist here.
    LadderDecision {
        shape: CourseShape::Single,
        n: None,
        rationale: format!("goal reads focused enough for one {NOUN_VERIFIED_RUN}"),
        rule: "default-single",
    }
}

fn pieces_hint(d: &DecompositionHints) -> u8 {
    let hint = d.enumerated_items.max(d.conjunction_clauses);
    u8::try_from(hint).unwrap_or(PLAN_MAX_PIECES)
}

/// Wrap a ladder decision as a plan resolution. Rule-certain about what it
/// saw, conservative about what it means: single/continuation clear the
/// default auto-accept floor, plan sits below it so `--yes` still shows the
/// card unless the operator raised confidence deliberately.
pub(crate) fn ladder_resolution(decision: &LadderDecision) -> CourseResolution {
    CourseResolution {
        source: ResolutionSource::Ladder,
        confidence: match decision.shape {
            CourseShape::Single | CourseShape::ChainExtend => 0.75,
            _ => 0.6,
        },
        rationale: format!("[{}] {}", decision.rule, decision.rationale),
        clamps_applied: Vec::new(),
    }
}

/// The provider planner's raw draft.
///
/// The planner returns a graph, not a shape word. The old vocabulary
/// (`single | plan | campaign`) could not express ordering at all — there was
/// no `depends_on` in the schema — so the one shape deadreckon runs
/// sequentially was unreachable from the planner no matter how it was tuned.
/// `shape`/`n` are still accepted so a model that answers in the old
/// vocabulary is understood rather than discarded.
#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCoursePlanDraft {
    #[serde(default)]
    nodes: Vec<ProviderCourseNodeDraft>,
    /// `at-end` (default) or `per-node`.
    #[serde(default)]
    apply: Option<String>,
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    n: Option<u8>,
    /// Legacy field name for `nodes`.
    #[serde(default)]
    pieces: Vec<ProviderCourseNodeDraft>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    rationale: Option<String>,
}

impl ProviderCoursePlanDraft {
    /// The drafted nodes under either field name.
    fn drafted_nodes(&self) -> &[ProviderCourseNodeDraft] {
        if self.nodes.is_empty() {
            &self.pieces
        } else {
            &self.nodes
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCourseNodeDraft {
    #[serde(default)]
    id: Option<String>,
    goal: String,
    #[serde(default)]
    done_hint: Option<String>,
    /// Ids of nodes that must finish first. This is the field the old schema
    /// had no way to express.
    #[serde(default)]
    depends_on: Vec<String>,
    /// Set when this node is a project in its own right and should be run as
    /// its own graph rather than a single run.
    #[serde(default)]
    subplan: Option<Box<ProviderCourseSubplanDraft>>,
}

impl ProviderCourseNodeDraft {
    /// A short label for clamp messages, so the audit trail names the node
    /// that was changed rather than an opaque index.
    fn goal_label(&self) -> String {
        let words = self
            .goal
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        if words.is_empty() {
            self.id.clone().unwrap_or_else(|| "node".to_string())
        } else {
            words
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCourseSubplanDraft {
    #[serde(default)]
    apply: Option<String>,
    #[serde(default)]
    nodes: Vec<ProviderCourseNodeDraft>,
}

/// Below this confidence a provider shape that disagrees with the ladder is
/// downgraded to the ladder result (overridable via config in C-P6).
pub(crate) const SHAPE_CONFIDENCE_FLOOR_DEFAULT: f64 = 0.7;

/// The grounded planner prompt: the provider sees exactly what the ladder
/// saw, so its answer refines measured signals instead of guessing.
pub(crate) fn course_planner_prompt(goal: &str, signals: &SignalBundle) -> String {
    let contract = signals
        .contract
        .command
        .as_deref()
        .unwrap_or("none detected");
    let workspace = if signals.workspace.members >= 2 {
        format!(
            "{} members ({})",
            signals.workspace.members,
            signals.workspace.member_names.join(", ")
        )
    } else {
        "single package".to_string()
    };
    let history = if signals.history.prior_runs > 0 {
        format!(
            "{} prior run(s), last status {}",
            signals.history.prior_runs,
            signals.history.last_status.as_deref().unwrap_or("unknown")
        )
    } else {
        "no prior runs".to_string()
    };
    let budget = signals
        .budget
        .ceiling_usd
        .map(|c| format!("${c:.2} ceiling"))
        .unwrap_or_else(|| "no explicit ceiling".to_string());
    let affordable = affordable_node_count(signals.budget.ceiling_usd);
    format!(
        "You are a read-only launch planner for deadreckon. Do not write files, install packages, or mutate state.\n\n\
         deadreckon executes a GRAPH of supervised agent runs. Your job is to choose its shape.\n\n\
         EXECUTION MODEL\n\
         - Each node is one supervised run in an isolated git worktree, checked by a separate\n\
           gate process the agent cannot forge. A node that misses its checks is retried\n\
           automatically (up to {max_attempts} attempts) before the plan gives up on it.\n\
         - Edges are dependencies. Nodes with no edge between them run in parallel; a node\n\
           listing depends_on waits for those nodes to finish.\n\
         - apply=\"at-end\": every node runs, then one merge composes the result. Best when the\n\
           nodes touch different areas and do not need each other's changes.\n\
         - apply=\"per-node\": each node lands on the branch as it passes and later nodes build\n\
           on it. Required when a node needs an earlier node's changes present in its working\n\
           tree. Costs serialization; buys incremental landing.\n\
         - A node may carry a \"subplan\": its goal is decomposed and executed as its own graph,\n\
           reconciled before its dependents run. Use when one node is itself a multi-part\n\
           project. A subplan has its own apply mode, so it may be sequential even when the\n\
           parent is parallel. Maximum nesting depth: {max_depth}.\n\
         - A node whose job is to review another node should depend on it; a different provider\n\
           will be routed to it so the review starts from fresh assumptions.\n\n\
         WHAT THIS RUN CAN AFFORD (measured — do not second-guess these)\n\
         - budget: {budget}\n\
         - roughly ${min_piece:.2} of work per node at current routes\n\
         - affordable nodes in total: {affordable}\n\
         - flat multi-node plan feasible: {plan_feasible}\n\
         - nested (subplan) work feasible: {campaign_feasible}\n\n\
         MEASURED SIGNALS (ground your answer in these)\n\
         - detected done contract: {contract}\n\
         - workspace: {workspace}\n\
         - history: {history}\n\
         - text analysis: {enumerated} enumerated items, {clauses} clauses, {imperatives} imperative verbs\n\n\
         Return JSON only:\n\
         {{\"nodes\":[{{\"id\":\"n0\",\"goal\":\"self-contained goal\",\"done_hint\":\"what proves it\",\"depends_on\":[],\"subplan\":null}}],\n\
          \"apply\":\"at-end\",\"confidence\":0.8,\"rationale\":\"one short line\"}}\n\n\
         Rules:\n\
         - Prefer the fewest nodes that fit the work. One node is a valid and common answer.\n\
         - depends_on must reference ids of earlier nodes in the array and form a DAG.\n\
         - If the goal names ordered steps (\"then\", \"after\", \"once X is done\"), express that\n\
           with edges and set apply=\"per-node\".\n\
         - A subplan is {{\"apply\":\"...\",\"nodes\":[...]}} using the same node shape.\n\
         - Every node goal must be self-contained: the agent running it never sees this prompt.\n\n\
         Goal: {goal}",
        max_attempts = deadreckon_core::plan::DEFAULT_MAX_ATTEMPTS,
        max_depth = deadreckon_core::plan::MAX_SUBPLAN_DEPTH,
        min_piece = MIN_PIECE_BUDGET_USD,
        affordable = affordable
            .map(|count| count.to_string())
            .unwrap_or_else(|| "no explicit ceiling".to_string()),
        plan_feasible = yes_no(signals.budget.plan_feasible),
        campaign_feasible = yes_no(signals.budget.campaign_feasible),
        enumerated = signals.decomposability.enumerated_items,
        clauses = signals.decomposability.conjunction_clauses,
        imperatives = signals.decomposability.imperative_verbs,
    )
}

/// How many nodes the ceiling pays for at the per-piece floor. The old prompt
/// said "use campaign sparingly" without ever naming a price, then clamped the
/// answer afterward; telling the planner the arithmetic removes the guess.
fn affordable_node_count(ceiling_usd: Option<f64>) -> Option<u32> {
    let ceiling = ceiling_usd?;
    if ceiling <= 0.0 {
        return Some(0);
    }
    Some((ceiling / MIN_PIECE_BUDGET_USD).floor().max(1.0) as u32)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// The planner's resolved output: shape, clamped n, typed pieces, when the
/// results land, and a resolution carrying every clamp applied on the way.
pub(crate) type ResolvedCoursePlan = (
    CourseShape,
    Option<u8>,
    Vec<CoursePiece>,
    deadreckon_core::plan::ApplyWhen,
    CourseResolution,
);

/// A short imperative label for a piece, derived from its goal. Pieces carry a
/// goal but no subject; plan tasks need both, and subjects must be unique
/// (validate_task_graph rejects duplicates), so a too-similar prefix is
/// lengthened rather than truncated blindly.
pub(crate) fn piece_subject(piece: &CoursePiece) -> String {
    const SUBJECT_WORDS: usize = 8;
    let goal = piece.goal.trim();
    let short = goal
        .split_whitespace()
        .take(SUBJECT_WORDS)
        .collect::<Vec<_>>()
        .join(" ");
    if short.is_empty() {
        return piece.id.clone();
    }
    if short.len() < goal.len() {
        format!("{short}…")
    } else {
        short
    }
}

fn normalized_apply(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

/// Read a shape out of the drafted graph.
///
/// The shape is no longer something the planner picks — it is a description of
/// what it drew. One node is a single run; any node carrying its own graph is
/// campaign-shaped; anything else is a plan.
fn shape_of_graph(nodes: &[ProviderCourseNodeDraft]) -> CourseShape {
    if nodes.iter().any(|node| {
        node.subplan
            .as_deref()
            .is_some_and(|subplan| !subplan.nodes.is_empty())
    }) {
        return CourseShape::Campaign;
    }
    if nodes.len() <= 1 {
        return CourseShape::Single;
    }
    CourseShape::Plan
}

/// Convert drafted nodes into course pieces, rewriting the planner's node ids
/// into piece ids so `depends_on` survives the translation. Edges naming an
/// unknown node are dropped and recorded rather than failing the launch — the
/// planner can never make a launch impossible, only shape it.
fn course_pieces_from_nodes(
    nodes: &[ProviderCourseNodeDraft],
    clamps: &mut Vec<String>,
) -> Vec<CoursePiece> {
    course_pieces_at_depth(nodes, clamps, 0)
}

fn course_pieces_at_depth(
    nodes: &[ProviderCourseNodeDraft],
    clamps: &mut Vec<String>,
    depth: u32,
) -> Vec<CoursePiece> {
    let ids: BTreeMap<String, String> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node.id
                .as_deref()
                .map(|id| (id.trim().to_string(), format!("p{}", index + 1)))
        })
        .collect();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let mut depends_on = Vec::new();
            for dependency in &node.depends_on {
                match ids.get(dependency.trim()) {
                    Some(piece_id) if piece_id != &format!("p{}", index + 1) => {
                        depends_on.push(piece_id.clone());
                    }
                    Some(_) => {
                        clamps.push(format!("dropped self-dependency on {}", dependency.trim()))
                    }
                    None => clamps.push(format!(
                        "dropped edge to unknown node {}",
                        dependency.trim()
                    )),
                }
            }
            depends_on.sort();
            depends_on.dedup();
            CoursePiece {
                id: format!("p{}", index + 1),
                goal: node.goal.clone(),
                done_hint: node.done_hint.clone(),
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on,
                subplan: subplan_for_node(node, clamps, depth),
            }
        })
        .collect()
}

/// A node's own graph, if it has a usable one at this depth.
///
/// A subplan of a single node is inlined: a project of one is just a node,
/// the same de-escalation the top level already applies. Nesting past
/// `MAX_SUBPLAN_DEPTH` is flattened and recorded rather than refused — the
/// planner shapes a launch, it cannot make one impossible.
fn subplan_for_node(
    node: &ProviderCourseNodeDraft,
    clamps: &mut Vec<String>,
    depth: u32,
) -> Option<CourseSubplan> {
    let drafted = node.subplan.as_deref()?;
    if drafted.nodes.is_empty() {
        return None;
    }
    if drafted.nodes.len() == 1 {
        clamps.push(format!(
            "subplan on {} inlined: decomposition yielded one node",
            node.goal_label()
        ));
        return None;
    }
    if depth + 1 >= deadreckon_core::plan::MAX_SUBPLAN_DEPTH {
        clamps.push(format!(
            "subplan on {} flattened: nesting cap {} reached",
            node.goal_label(),
            deadreckon_core::plan::MAX_SUBPLAN_DEPTH
        ));
        return None;
    }
    let pieces = course_pieces_at_depth(&drafted.nodes, clamps, depth + 1);
    let apply = match drafted.apply.as_deref().map(normalized_apply) {
        Some(apply) if apply == "per-node" => deadreckon_core::plan::ApplyWhen::PerNode,
        _ => deadreckon_core::plan::ApplyWhen::AtEnd,
    };
    Some(CourseSubplan { apply, pieces })
}

/// Parse + clamp a provider draft against the ladder floor. `None` on any
/// parse miss — the caller falls back to the ladder; the planner can never
/// fail a launch.
pub(crate) fn resolve_provider_course_plan(
    content: &str,
    signals: &SignalBundle,
    ladder: &LadderDecision,
    confidence_floor: f64,
) -> Option<ResolvedCoursePlan> {
    let draft = serde_json::from_str::<ProviderCoursePlanDraft>(content)
        .ok()
        .or_else(|| {
            commands::plan::json_slice(content, '{', '}')
                .and_then(|slice| serde_json::from_str::<ProviderCoursePlanDraft>(slice).ok())
        })?;
    let mut clamps: Vec<String> = Vec::new();
    // The graph is the answer. A shape word is only consulted when the planner
    // returned no nodes at all, so a model still answering in the old
    // vocabulary is understood rather than thrown away.
    let nodes = draft.drafted_nodes();
    let shape = if nodes.is_empty() {
        match draft
            .shape
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "single" | "run" => CourseShape::Single,
            "plan" | "orchestrate" | "orchestration" | "full-plan" | "full_plan" => {
                CourseShape::Plan
            }
            "campaign" => CourseShape::Campaign,
            _ => return None,
        }
    } else {
        shape_of_graph(nodes)
    };
    let confidence = draft.confidence.unwrap_or(0.5).clamp(0.0, 1.0);
    let rationale = draft
        .rationale
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    if rationale.is_empty() {
        return None;
    }
    // Confidence-floor downgrade: an unsure planner defers to the ladder.
    if confidence < confidence_floor && shape != ladder.shape {
        clamps.push(format!(
            "confidence {confidence:.2} below floor {confidence_floor:.2}: shape {} downgraded to ladder {}",
            shape.label(),
            ladder.shape.label()
        ));
        let mut resolution = ladder_resolution(ladder);
        resolution.clamps_applied = clamps;
        return Some((
            ladder.shape,
            ladder.n,
            Vec::new(),
            deadreckon_core::plan::ApplyWhen::AtEnd,
            resolution,
        ));
    }
    // Budget clamp: an infeasible shape downgrades, recorded.
    let (shape, downgraded) = match shape {
        CourseShape::Campaign if !signals.budget.campaign_feasible => (CourseShape::Plan, true),
        CourseShape::Plan if !signals.budget.plan_feasible => (CourseShape::Single, true),
        other => (other, false),
    };
    if downgraded {
        clamps.push("shape downgraded to fit the budget ceiling".to_string());
    }
    // C-P11: de-escalation — a decomposition of exactly one piece is not a
    // plan; it collapses to a single run instead of inflating to n=2 (the
    // old refusal path becomes graceful fallback, recorded in the trail).
    let drafted = draft.drafted_nodes().len();
    let shape = if shape == CourseShape::Plan
        && (draft.n == Some(1) || (draft.n.is_none() && drafted == 1))
    {
        clamps.push("plan collapsed to single: decomposition yielded one piece".to_string());
        CourseShape::Single
    } else {
        shape
    };
    let n = match shape {
        CourseShape::Single | CourseShape::ChainExtend => None,
        CourseShape::Plan | CourseShape::Campaign => {
            let raw = draft
                .n
                .or_else(|| u8::try_from(drafted).ok().filter(|count| *count > 0))
                .unwrap_or(3);
            let clamped = raw.clamp(2, PLAN_MAX_PIECES);
            if clamped != raw {
                clamps.push(format!("n clamped {raw}->{clamped}"));
            }
            Some(clamped)
        }
    };
    let mut pieces = course_pieces_from_nodes(draft.drafted_nodes(), &mut clamps);
    // Per-node apply is executable now, so it is honored rather than lowered.
    // A single node has nothing to sequence, so it stays at-end regardless.
    let requested_per_node = draft
        .apply
        .as_deref()
        .map(normalized_apply)
        .is_some_and(|apply| apply == "per-node");
    let apply = match (requested_per_node, shape) {
        (true, CourseShape::Single) => {
            clamps
                .push("apply per-node ignored: a single node has nothing to sequence".to_string());
            deadreckon_core::plan::ApplyWhen::AtEnd
        }
        (true, _) => deadreckon_core::plan::ApplyWhen::PerNode,
        (false, _) => deadreckon_core::plan::ApplyWhen::AtEnd,
    };

    if let Some(n) = n
        && pieces.len() > usize::from(n)
    {
        clamps.push(format!("pieces truncated {}->{}", pieces.len(), n));
        pieces.truncate(usize::from(n));
    }
    if matches!(shape, CourseShape::Single) && pieces.len() > 1 {
        pieces.truncate(1);
    }
    Some((
        shape,
        n,
        pieces,
        apply,
        CourseResolution {
            source: ResolutionSource::Provider,
            confidence,
            rationale,
            clamps_applied: clamps,
        },
    ))
}

/// `--yes` may auto-accept only under this ceiling (config-overridable via
/// `[defaults] shape_auto_spend_ceiling`).
pub(crate) const SHAPE_AUTO_SPEND_CEILING_DEFAULT: f64 = 20.0;
/// Campaign ALWAYS confirms interactively above this line (config-overridable
/// via `[defaults] campaign_confirm_line`). Guardrail beats every flag.
pub(crate) const CAMPAIGN_CONFIRM_LINE_DEFAULT: f64 = 25.0;

/// What happens at the launch gate: show the card, sail silently, or refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptDecision {
    InteractiveCard,
    AutoAccept,
    RefuseWithTry,
}

/// Everything the accept gate weighs. The asymmetry is deliberate: a wrong
/// single costs a retry; a wrong campaign costs real money.
#[derive(Clone, Copy)]
pub(crate) struct AcceptPolicyInput<'a> {
    pub(crate) resolution: &'a CourseResolution,
    pub(crate) shape: CourseShape,
    pub(crate) ceiling_usd: Option<f64>,
    pub(crate) tty: bool,
    pub(crate) yes: bool,
    pub(crate) confidence_floor: f64,
    pub(crate) auto_spend_ceiling: f64,
    pub(crate) campaign_confirm_line: f64,
}

/// The launch accept matrix (TTY × yes × confidence × ceiling × shape):
/// - campaign above the confirm line ALWAYS confirms (TTY) or refuses
///   (non-TTY) — no flag overrides the guardrail;
/// - `--yes` auto-accepts only when confidence clears the floor AND the
///   ceiling (when set) is under the auto-spend line;
/// - otherwise a TTY shows the card and a non-TTY refuses with a `try:`
///   (a script must opt in explicitly; launches never hang on stdin).
pub(crate) fn accept_policy(input: AcceptPolicyInput<'_>) -> AcceptDecision {
    let campaign_above_line = input.shape == CourseShape::Campaign
        && input
            .ceiling_usd
            .is_none_or(|c| c > input.campaign_confirm_line);
    if campaign_above_line {
        return if input.tty {
            AcceptDecision::InteractiveCard
        } else {
            AcceptDecision::RefuseWithTry
        };
    }
    if input.yes {
        let under_ceiling = input
            .ceiling_usd
            .is_none_or(|c| c <= input.auto_spend_ceiling);
        return if input.resolution.confidence >= input.confidence_floor && under_ceiling {
            AcceptDecision::AutoAccept
        } else if input.tty {
            AcceptDecision::InteractiveCard
        } else {
            AcceptDecision::RefuseWithTry
        };
    }
    if input.tty {
        AcceptDecision::InteractiveCard
    } else {
        AcceptDecision::RefuseWithTry
    }
}

/// What the operator chose at the course card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardOutcome {
    Sail,
    Edit,
    ReviewDone,
    ForceSingle,
    Abort,
}

/// Build the course card: WHAT / SHAPE / pieces / WHO / COST / DONE / WHY /
/// ESCAPE, always all present. Layout is spec — pinned by a golden test, so
/// whitespace changes are contract changes.
pub(crate) fn course_card(plan: &LaunchPlan) -> Card {
    let mut rows: Vec<(String, String)> = Vec::new();
    rows.push(("goal".to_string(), plan.goal.clone()));
    let shape_row = match (plan.shape, plan.n) {
        (CourseShape::Plan, Some(n)) => format!(
            "plan - {n} pieces in parallel - confidence {:.2}",
            plan.resolution.confidence
        ),
        (CourseShape::Campaign, Some(n)) => format!(
            "campaign - {n} sub-goals - confidence {:.2}",
            plan.resolution.confidence
        ),
        (CourseShape::ChainExtend, _) => format!(
            "follow-up run (continues verified history) - confidence {:.2}",
            plan.resolution.confidence
        ),
        _ => format!(
            "single {NOUN_VERIFIED_RUN} - confidence {:.2}",
            plan.resolution.confidence
        ),
    };
    rows.push(("shape".to_string(), shape_row));
    if plan.pieces.len() > 1 {
        for (idx, piece) in plan.pieces.iter().enumerate() {
            rows.push((format!("piece {}", idx + 1), piece.goal.clone()));
        }
    }
    let who = [
        plan.providers
            .planner
            .as_deref()
            .map(|p| format!("planner {p}")),
        plan.providers
            .coder
            .as_deref()
            .map(|p| format!("coder {p}")),
        plan.providers
            .reviewer
            .as_deref()
            .map(|p| format!("reviewer {p}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" - ");
    rows.push((
        "who".to_string(),
        if who.is_empty() {
            "resolved at launch".to_string()
        } else {
            who
        },
    ));
    let cost = match plan.budget.ceiling_usd {
        Some(ceiling) if !plan.budget.split.is_empty() => format!(
            "ceiling ${ceiling:.2} - split {}",
            plan.budget
                .split
                .iter()
                .map(|part| format!("{part:.0}"))
                .collect::<Vec<_>>()
                .join(" / ")
        ),
        Some(ceiling) => format!("ceiling ${ceiling:.2}"),
        None => "no explicit ceiling (defaults apply)".to_string(),
    };
    rows.push(("cost".to_string(), cost));
    let done = match (&plan.contract.summary, &plan.contract.caveat) {
        (Some(summary), None) => format!("{summary} [{}]", plan.contract.source.label()),
        (Some(summary), Some(caveat)) => format!("{summary} - caveat: {caveat}"),
        (None, Some(caveat)) => format!("none - {caveat}"),
        (None, None) => format!("[{}]", plan.contract.source.label()),
    };
    rows.push(("done".to_string(), done));
    for check in &plan.contract.checks {
        rows.push((format!("done {}", check.index), check.summary.clone()));
    }
    if let Some(divergence) = plan.contract.divergence.as_ref()
        && !divergence.clean()
    {
        let flag = if divergence.strong() {
            "strong divergence"
        } else {
            "review suggested"
        };
        rows.push(("done drift".to_string(), flag.to_string()));
    }
    rows.push(("why".to_string(), plan.resolution.rationale.clone()));
    rows.push((
        "escape".to_string(),
        format!("{} - {}", plan.escape.kill, plan.escape.undo),
    ));
    Card {
        title: TitleLine {
            glyph: TitleGlyph::Preview,
            label: "course - plot, preview, sail".to_string(),
        },
        subtitle: None,
        sections: vec![Section::KeyValue { rows }],
        primary_action: Some(HintLine {
            label: "sail".to_string(),
            command: "Enter (or --yes)".to_string(),
        }),
        hints: vec![
            HintLine {
                label: "done".to_string(),
                command: "d".to_string(),
            },
            HintLine {
                label: "edit".to_string(),
                command: "e".to_string(),
            },
            HintLine {
                label: "single".to_string(),
                command: "s".to_string(),
            },
            HintLine {
                label: "abort".to_string(),
                command: "q".to_string(),
            },
        ],
    }
}

/// Render the card for a stream (plain strips borders per CardOptions).
pub(crate) fn render_course_card(plan: &LaunchPlan, plain: bool) -> String {
    render_card(
        &course_card(plan),
        &CardOptions {
            color: !plain,
            plain,
            terminal_columns: Some(80),
            no_color_env: plain,
        },
    )
}

/// Drive the card interaction through the existing prompter seam so tests
/// script it: sail / done / edit / force-single / abort. Forcing single records the
/// operator as the resolution source (their override is part of the audit).
pub(crate) fn prompt_course_card(
    plan: &mut LaunchPlan,
    prompter: &mut dyn super::start::StartPrompter,
) -> Result<CardOutcome> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Sail this course?".to_string(),
        help: Some(
            "Enter sails - d reviews done - e edits - s forces a single run - q aborts".to_string(),
        ),
        choices: vec![
            prompt::SelectChoice::new("sail", "Sail (launch as planned)"),
            prompt::SelectChoice::new("done", "Review done contract"),
            prompt::SelectChoice::new("edit", "Edit shape, count, or budget"),
            prompt::SelectChoice::new("single", "Force a single run"),
            prompt::SelectChoice::new("abort", "Abort"),
        ],
        default_index: 0,
    })?;
    Ok(match choice.id.as_str() {
        "edit" => CardOutcome::Edit,
        "done" => CardOutcome::ReviewDone,
        "single" => {
            plan.shape = CourseShape::Single;
            plan.n = None;
            plan.pieces.truncate(1);
            plan.resolution.source = ResolutionSource::Operator;
            plan.resolution
                .clamps_applied
                .push("operator forced single at the card".to_string());
            CardOutcome::ForceSingle
        }
        "abort" => CardOutcome::Abort,
        _ => CardOutcome::Sail,
    })
}

/// Build the durable plan from a resolved start decision (C-P9). Everything
/// dispatch is about to do must be readable from this artifact alone.
pub(crate) fn launch_plan_from_decision(
    decision: &super::start::StartLaunchDecision,
    ceiling_usd: Option<f64>,
    accepted_by: &str,
) -> LaunchPlan {
    use super::start::StartSelectedMode;
    let shape = match decision.selected_mode {
        StartSelectedMode::Run => CourseShape::Single,
        StartSelectedMode::Extend => CourseShape::ChainExtend,
        StartSelectedMode::Review | StartSelectedMode::FullPlan => CourseShape::Plan,
        StartSelectedMode::Campaign => CourseShape::Campaign,
    };
    let resolution = match decision.goal_shape.as_ref() {
        Some(recommendation) => CourseResolution {
            source: match recommendation.source {
                super::start::GoalShapeSource::Provider => ResolutionSource::Provider,
                super::start::GoalShapeSource::Fallback => ResolutionSource::Ladder,
            },
            confidence: match recommendation.source {
                super::start::GoalShapeSource::Provider => 0.8,
                super::start::GoalShapeSource::Fallback => 0.75,
            },
            rationale: recommendation.rationale.clone(),
            clamps_applied: Vec::new(),
        },
        None => CourseResolution {
            source: ResolutionSource::Operator,
            confidence: 1.0,
            rationale: decision.reason.clone(),
            clamps_applied: Vec::new(),
        },
    };
    let mut plan = LaunchPlan::new(&decision.goal, shape, resolution);
    plan.n = decision.child_count;
    plan.providers = CourseProviders {
        planner: decision.planner_provider_route.clone(),
        coder: decision
            .coder_provider_route
            .clone()
            .or_else(|| decision.provider_route.clone()),
        reviewer: decision.reviewer_provider_route.clone(),
    };
    plan.budget = CourseBudget {
        ceiling_usd,
        split: Vec::new(),
        wall_seconds: None,
    };
    plan.contract = CourseContract {
        source: match decision.done_criteria_source {
            super::start::StartDoneCriteriaSource::Detected => ContractOrigin::Detected,
            super::start::StartDoneCriteriaSource::Asked => ContractOrigin::Asked,
            super::start::StartDoneCriteriaSource::Project
            | super::start::StartDoneCriteriaSource::Generated
            | super::start::StartDoneCriteriaSource::Manual => ContractOrigin::Operator,
            super::start::StartDoneCriteriaSource::DefaultGate
            | super::start::StartDoneCriteriaSource::Missing => ContractOrigin::None,
        },
        kind: None,
        summary: Some(decision.done_criteria_label.clone()),
        caveat: None,
        checks: decision
            .done_contract
            .as_ref()
            .map(|contract| contract.checks.clone())
            .unwrap_or_default(),
        divergence: decision.done_divergence.clone(),
    };
    plan.accepted_by = Some(accepted_by.to_string());
    plan
}

/// A trivial operator plan for a direct verb launch (`deadreckon run` etc.)
/// so every dispatched root carries the decision record, however it began.
pub(crate) fn trivial_operator_plan(goal: &str, shape: CourseShape, verb: &str) -> LaunchPlan {
    let mut plan = LaunchPlan::new(
        goal,
        shape,
        CourseResolution {
            source: ResolutionSource::Operator,
            confidence: 1.0,
            rationale: format!("direct {verb} verb — operator chose the shape"),
            clamps_applied: Vec::new(),
        },
    );
    plan.accepted_by = Some("operator".to_string());
    plan
}

/// Persist the plan into a dispatched root, best-effort: a read-only or
/// exotic filesystem must not fail a launch that already succeeded.
pub(crate) fn save_launch_plan_best_effort(root: &Path, plan: &LaunchPlan) {
    let _ = save_launch_plan(&launch_plan_path(root), plan);
}

/// `deadreckon reshape <id>` (C-P12): preview a run's inert reshape
/// proposal on the course card and, on acceptance, dispatch it as a
/// full-plan orchestration with the parent run recorded in the plan.
/// The proposal NEVER executes without this explicit accept.
pub(crate) async fn reshape_command(args: ReshapeArgs) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = super::reference::resolve_run_like(&paths, Some(&args.run_id), "verdict")?;
    if matches!(state.status, deadreckon_core::RunStatus::Executing) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("run {} is still running", state.run_id),
            &format!("deadreckon attach {}", state.run_id),
        )));
    }
    let proposal_path = state.run_root.join("reshape-proposal.json");
    if !proposal_path.exists() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("run {} has no reshape proposal", state.run_id),
            &format!("deadreckon status {}", state.run_id),
        )));
    }
    let mut plan = load_launch_plan(&proposal_path)?;
    plan.parent.get_or_insert_with(|| state.run_id.clone());

    if !args.quiet && !args.json {
        print!("{}", render_course_card(&plan, args.plain));
    }
    if args.json && !args.yes {
        // Read-only preview envelope; accepting from JSON requires --yes.
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "reshape-preview",
                "run_id": state.run_id,
                "plan": &plan,
                "next_actions": [format!("deadreckon reshape {} --yes", state.run_id)],
            }))?
        );
        return Ok(());
    }
    if !args.yes {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "a reshape proposal needs explicit acceptance",
                &format!("deadreckon reshape {} --yes", state.run_id),
            )));
        }
        let mut prompter = super::start::TerminalStartPrompter;
        match prompt_course_card(&mut plan, &mut prompter)? {
            CardOutcome::Sail => {}
            CardOutcome::ForceSingle
            | CardOutcome::Edit
            | CardOutcome::ReviewDone
            | CardOutcome::Abort => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "reshape not accepted",
                    &format!("deadreckon reshape {} --yes", state.run_id),
                )));
            }
        }
    }
    plan.accepted_by = Some("operator".to_string());
    let n = plan
        .n
        .unwrap_or_else(|| u8::try_from(plan.pieces.len()).unwrap_or(2))
        .clamp(2, PLAN_MAX_PIECES);

    let before: std::collections::BTreeSet<String> =
        super::inspection::list_plan_entries(&paths, None)?
            .into_iter()
            .map(|entry| entry.plan_id)
            .collect();
    let goal = plan.goal.clone();
    super::orchestrate::orchestrate_command(super::orchestrate::OrchestrateRunArgs {
        // A reshape already carries the accepted decomposition; hand it to
        // plan creation instead of re-planning the same goal.
        seed_pieces: plan.pieces.clone(),
        plan: crate::cli::PlanCommandArgs {
            goal: goal.clone(),
            n,
            mode: crate::cli::CliPlanMode::FullPlan,
            // LaunchPlan predates apply modes; a reshape lands once at the end.
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            max_spend: plan.budget.ceiling_usd,
            max_wall_seconds: None,
            sandbox: None,
            planner_provider: plan.providers.planner.clone(),
            provider: plan.providers.coder.clone(),
            child_provider: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            model: None,
            child_model: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            init_git: false,
            acceptance: None,
            skip_acceptance_prompt: true,
            no_hints: args.quiet,
            quiet: args.quiet,
            json: false,
            plain: args.plain,
        },
        preview: false,
        yes: true,
        no_repair: false,
        completion_surface: !args.quiet,
        narrate: false,
        narrator_model: None,
    })
    .await?;
    let dispatched = super::inspection::list_plan_entries(&paths, None)?
        .into_iter()
        .filter(|entry| entry.goal == goal && !before.contains(&entry.plan_id))
        .map(|entry| entry.plan_id)
        .next_back();
    if let Some(plan_id) = dispatched.as_ref() {
        save_launch_plan_best_effort(&paths.plan_dir(plan_id), &plan);
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "reshape",
                "run_id": state.run_id,
                "dispatched": { "plan_id": dispatched },
                "plan": &plan,
            }))?
        );
    }
    Ok(())
}

/// Parsed `deadreckon reshape` arguments.
pub(crate) struct ReshapeArgs {
    pub(crate) run_id: String,
    pub(crate) yes: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
    pub(crate) quiet: bool,
}

/// Start-then-watch (C-P13): whether a successful launch should drop
/// straight into attach. Opt-in via `[defaults] start_attach = true`, and
/// only for an interactive human — machine modes and quiet sessions never
/// auto-attach, and preview launches have nothing to watch.
pub(crate) fn should_auto_attach(
    config_enabled: bool,
    tty: bool,
    json: bool,
    quiet: bool,
    preview: bool,
) -> bool {
    config_enabled && tty && !json && !quiet && !preview
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
                depends_on: Vec::new(),
                subplan: None,
            },
            CoursePiece {
                id: "p2".to_string(),
                goal: "config surface in limits.toml".to_string(),
                done_hint: None,
                role: Some("coder".to_string()),
                provider: None,
                model: None,
                budget_usd: Some(3.0),
                depends_on: vec!["p1".to_string()],
                subplan: None,
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
            checks: Vec::new(),
            divergence: None,
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

    // ---- C-P4: the deterministic ladder ----

    fn bundle_with(
        decomposability: DecompositionHints,
        members: usize,
        history: HistorySignal,
        ceiling: Option<f64>,
    ) -> SignalBundle {
        SignalBundle {
            goal: String::new(),
            decomposability,
            contract: ContractSignal::default(),
            workspace: WorkspaceSignal {
                members,
                member_names: Vec::new(),
                kind: None,
                tree_bucket: TreeBucket::Small,
            },
            history,
            budget: budget_signal(ceiling),
        }
    }

    #[test]
    fn ladder_prefers_continuation_on_verified_history() {
        let decision = ladder_decision(&bundle_with(
            analyze_goal_structure("1. build a 2. build b 3. build c"),
            4,
            HistorySignal {
                prior_runs: 2,
                last_verified_same_task: true,
                last_run_id: Some("run-42".to_string()),
                last_status: Some("completed".to_string()),
            },
            None,
        ));
        assert_eq!(decision.shape, CourseShape::ChainExtend, "{decision:?}");
        assert_eq!(decision.rule, "continuation");
        assert!(decision.rationale.contains("run-42"), "{decision:?}");
    }

    #[test]
    fn small_budget_forces_single() {
        let decision = ladder_decision(&bundle_with(
            analyze_goal_structure("1. build a 2. build b 3. build c"),
            4,
            HistorySignal::default(),
            Some(1.0),
        ));
        assert_eq!(decision.shape, CourseShape::Single, "{decision:?}");
        assert_eq!(decision.rule, "budget");
    }

    #[test]
    fn enumerated_goal_plus_workspace_yields_plan_n_clamped() {
        let hints = analyze_goal_structure(
            "1. api 2. config 3. docs 4. tests 5. ci 6. release 7. site 8. bench",
        );
        let decision = ladder_decision(&bundle_with(hints, 3, HistorySignal::default(), None));
        assert_eq!(decision.shape, CourseShape::Plan, "{decision:?}");
        // Clamped by workspace members (3), not the raw enumeration (8).
        assert_eq!(decision.n, Some(3), "{decision:?}");
        assert_eq!(decision.rule, "decomposition+workspace");

        let single_pkg = ladder_decision(&bundle_with(
            analyze_goal_structure("add limiter and write docs then wire ci"),
            0,
            HistorySignal::default(),
            None,
        ));
        assert_eq!(single_pkg.shape, CourseShape::Plan, "{single_pkg:?}");
        assert!(single_pkg.n.unwrap() <= 4, "{single_pkg:?}");
        assert_eq!(single_pkg.rule, "decomposition");
    }

    // ---- C-P7: the course card ----

    struct ScriptedCardPrompter {
        choice: &'static str,
    }

    impl super::super::start::StartPrompter for ScriptedCardPrompter {
        fn select_one(&mut self, _prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice> {
            Ok(prompt::SelectChoice::new(self.choice, self.choice))
        }
        fn confirm(&mut self, _question: &str, default_yes: bool) -> Result<bool> {
            Ok(default_yes)
        }
        fn input(&mut self, _message: &str, _default: Option<&str>) -> Result<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn course_card_golden_snapshot_pins_layout() {
        // The card layout is spec: whitespace changes are contract changes.
        let expected = "\
+------------------------------------------------------------------------------+
| > course - plot, preview, sail                                               |
|   goal          add rate limiting to the API                                 |
|   shape         plan - 3 pieces in parallel - confidence 0.82                |
|   piece 1       token-bucket limiter core                                    |
|   piece 2       config surface in limits.toml                                |
|   who           planner cli:claude-code - coder cli:claude-code - reviewe... |
|   cost          ceiling $12.00 - split 5 / 3 / 4                             |
|   done          pnpm test [detected]                                         |
|   why           three independently testable pieces                          |
|   escape        deadreckon kill latest - deadreckon undo latest              |
|                                                                              |
|   sail          Enter (or --yes)                                             |
|   done          d                                                            |
|   edit          e                                                            |
|   single        s                                                            |
|   abort         q                                                            |
+------------------------------------------------------------------------------+
";
        let rendered = render_course_card(&sample_plan(), true);
        assert_eq!(rendered, expected, "--- actual ---\n{rendered}");
    }

    #[test]
    fn card_always_names_done_contract_and_escape() {
        // Whatever the plan carries, done and escape rows always render.
        let full = render_course_card(&sample_plan(), true);
        assert!(full.contains("done"), "{full}");
        assert!(full.contains("pnpm test [detected]"), "{full}");
        assert!(full.contains("deadreckon kill latest"), "{full}");

        let bare = LaunchPlan::new(
            "x",
            CourseShape::Single,
            CourseResolution {
                source: ResolutionSource::Ladder,
                confidence: 0.75,
                rationale: "r".to_string(),
                clamps_applied: Vec::new(),
            },
        );
        let rendered = render_course_card(&bare, true);
        assert!(rendered.contains("[none]"), "{rendered}");
        assert!(rendered.contains("deadreckon undo latest"), "{rendered}");
        assert!(rendered.contains("escape"), "{rendered}");
    }

    #[test]
    fn s_key_forces_single_and_records_operator_source() {
        let mut plan = sample_plan();
        assert_eq!(plan.shape, CourseShape::Plan);
        let outcome = prompt_course_card(&mut plan, &mut ScriptedCardPrompter { choice: "single" })
            .expect("outcome");
        assert_eq!(outcome, CardOutcome::ForceSingle);
        assert_eq!(plan.shape, CourseShape::Single);
        assert_eq!(plan.n, None);
        assert!(plan.pieces.len() <= 1, "{:?}", plan.pieces);
        assert_eq!(plan.resolution.source, ResolutionSource::Operator);
        assert!(
            plan.resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("operator forced single")),
            "{:?}",
            plan.resolution
        );

        let mut sail_plan = sample_plan();
        let outcome =
            prompt_course_card(&mut sail_plan, &mut ScriptedCardPrompter { choice: "sail" })
                .expect("outcome");
        assert_eq!(outcome, CardOutcome::Sail);
        assert_eq!(
            sail_plan.shape,
            CourseShape::Plan,
            "sail leaves the plan untouched"
        );
    }

    #[test]
    fn course_card_done_lists_compiled_checks() {
        let mut plan = sample_plan();
        let contract = super::super::acceptance::compile_contract(
            r#"
name: behavior
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/check.mjs"
    cwd: "{working_dir}"
"#,
            Some("# Done\n"),
        )
        .expect("contract");
        plan.contract.checks = contract.checks;

        let rendered = render_course_card(&plan, true);

        assert!(rendered.contains("done 1"), "{rendered}");
        assert!(rendered.contains("runs shell: npm run build"), "{rendered}");
    }

    #[test]
    fn course_card_flags_goal_divergence() {
        let mut plan = sample_plan();
        let contract = super::super::acceptance::compile_contract(
            r#"
name: weak
checks:
  - kind: content_match
    path: "{working_dir}/src/main.js"
    pattern: "offline"
"#,
            Some("# Done\n"),
        )
        .expect("contract");
        plan.contract.divergence = Some(super::super::acceptance::reconcile(
            "build a realtime dashboard",
            &contract,
        ));
        plan.contract.checks = contract.checks;

        let rendered = render_course_card(&plan, true);

        assert!(rendered.contains("done drift"), "{rendered}");
    }

    #[test]
    fn course_card_d_key_opens_review_loop() {
        let mut plan = sample_plan();
        let outcome = prompt_course_card(&mut plan, &mut ScriptedCardPrompter { choice: "done" })
            .expect("outcome");

        assert_eq!(outcome, CardOutcome::ReviewDone);
    }

    // ---- C-P6: guardrails + accept policy ----

    fn policy(
        shape: CourseShape,
        confidence: f64,
        ceiling: Option<f64>,
        tty: bool,
        yes: bool,
    ) -> AcceptDecision {
        let resolution = CourseResolution {
            source: ResolutionSource::Provider,
            confidence,
            rationale: "test".to_string(),
            clamps_applied: Vec::new(),
        };
        accept_policy(AcceptPolicyInput {
            resolution: &resolution,
            shape,
            ceiling_usd: ceiling,
            tty,
            yes,
            confidence_floor: SHAPE_CONFIDENCE_FLOOR_DEFAULT,
            auto_spend_ceiling: SHAPE_AUTO_SPEND_CEILING_DEFAULT,
            campaign_confirm_line: CAMPAIGN_CONFIRM_LINE_DEFAULT,
        })
    }

    #[test]
    fn yes_flag_autoaccepts_only_above_confidence_and_under_ceiling() {
        // Confident + under the auto-spend ceiling → silent sail.
        assert_eq!(
            policy(CourseShape::Plan, 0.9, Some(10.0), true, true),
            AcceptDecision::AutoAccept
        );
        // Confident but over the auto-spend ceiling → card, not silence.
        assert_eq!(
            policy(CourseShape::Plan, 0.9, Some(100.0), true, true),
            AcceptDecision::InteractiveCard
        );
        // Under the ceiling but unsure → card, not silence.
        assert_eq!(
            policy(CourseShape::Plan, 0.4, Some(10.0), true, true),
            AcceptDecision::InteractiveCard
        );
        // No explicit ceiling: confidence alone decides for non-campaign.
        assert_eq!(
            policy(CourseShape::Single, 0.9, None, true, true),
            AcceptDecision::AutoAccept
        );
    }

    #[test]
    fn campaign_above_line_always_confirms_or_refuses() {
        // Above the line: --yes and confidence are irrelevant.
        assert_eq!(
            policy(CourseShape::Campaign, 0.99, Some(100.0), true, true),
            AcceptDecision::InteractiveCard
        );
        assert_eq!(
            policy(CourseShape::Campaign, 0.99, Some(100.0), false, true),
            AcceptDecision::RefuseWithTry
        );
        // No ceiling means unbounded — that is above the line by definition.
        assert_eq!(
            policy(CourseShape::Campaign, 0.99, None, true, true),
            AcceptDecision::InteractiveCard
        );
        // Under the line campaign behaves like any other shape.
        assert_eq!(
            policy(CourseShape::Campaign, 0.9, Some(10.0), true, true),
            AcceptDecision::AutoAccept
        );
    }

    #[test]
    fn non_tty_without_yes_refuses_with_try() {
        assert_eq!(
            policy(CourseShape::Single, 0.9, Some(5.0), false, false),
            AcceptDecision::RefuseWithTry
        );
        assert_eq!(
            policy(CourseShape::Plan, 0.9, None, false, false),
            AcceptDecision::RefuseWithTry
        );
        // A TTY without --yes always gets the card.
        assert_eq!(
            policy(CourseShape::Single, 0.9, Some(5.0), true, false),
            AcceptDecision::InteractiveCard
        );
    }

    // ---- C-P13: start-then-watch ----

    #[test]
    fn start_attach_config_drops_into_attach_on_tty() {
        assert!(should_auto_attach(true, true, false, false, false));
        // Off by default: the config knob is the only way in.
        assert!(!should_auto_attach(false, true, false, false, false));
        // Nothing to watch after a preview.
        assert!(!should_auto_attach(true, true, false, false, true));
        // No terminal, no TUI.
        assert!(!should_auto_attach(true, false, false, false, false));
    }

    #[test]
    fn json_and_quiet_never_auto_attach() {
        assert!(!should_auto_attach(true, true, true, false, false));
        assert!(!should_auto_attach(true, true, false, true, false));
        assert!(!should_auto_attach(true, false, true, true, false));
    }

    // ---- C-P11: de-escalation plan collapse ----

    #[test]
    fn single_task_decomposition_collapses_to_run() {
        let signals = bundle_with(
            analyze_goal_structure("do the thing"),
            0,
            HistorySignal::default(),
            None,
        );
        let ladder = ladder_decision(&signals);
        // n=1 explicitly, and pieces-of-one with n unset, both collapse.
        for draft in [
            r#"{"shape":"plan","n":1,"confidence":0.9,"rationale":"one piece really"}"#,
            r#"{"shape":"plan","confidence":0.9,"rationale":"one piece really",
                "pieces":[{"goal":"the only piece"}]}"#,
        ] {
            let (shape, n, _pieces, _apply, resolution) = resolve_provider_course_plan(
                draft,
                &signals,
                &ladder,
                SHAPE_CONFIDENCE_FLOOR_DEFAULT,
            )
            .expect("resolved");
            assert_eq!(shape, CourseShape::Single, "{resolution:?}");
            assert_eq!(n, None, "{resolution:?}");
        }
    }

    #[test]
    fn collapse_recorded_as_event_and_rationale() {
        let signals = bundle_with(
            analyze_goal_structure("do the thing"),
            0,
            HistorySignal::default(),
            None,
        );
        let ladder = ladder_decision(&signals);
        let (_shape, _n, _pieces, _apply, resolution) = resolve_provider_course_plan(
            r#"{"shape":"plan","n":1,"confidence":0.9,"rationale":"one piece really"}"#,
            &signals,
            &ladder,
            SHAPE_CONFIDENCE_FLOOR_DEFAULT,
        )
        .expect("resolved");
        // The clamp trail is the durable record — it rides launch-plan.json
        // into the dispatched root, so the collapse is auditable forever.
        assert!(
            resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("collapsed to single")),
            "{resolution:?}"
        );
        assert_eq!(resolution.rationale, "one piece really");
    }

    // ---- C-P5: the provider planner ----

    #[test]
    fn planner_prompt_includes_contract_and_workspace_signals() {
        let mut signals = bundle_with(
            analyze_goal_structure("add limiter and write docs then wire ci"),
            3,
            HistorySignal::default(),
            Some(12.0),
        );
        signals.contract = ContractSignal {
            kind: Some("go".to_string()),
            command: Some("go test ./...".to_string()),
            caveat: None,
            detected: true,
        };
        signals.workspace.member_names = vec![
            "crates/a".to_string(),
            "crates/b".to_string(),
            "crates/c".to_string(),
        ];
        let prompt = course_planner_prompt("add limiter and write docs then wire ci", &signals);
        assert!(prompt.contains("go test ./..."), "{prompt}");
        assert!(prompt.contains("3 members"), "{prompt}");
        assert!(prompt.contains("crates/a"), "{prompt}");
        assert!(prompt.contains("$12.00 ceiling"), "{prompt}");
        assert!(prompt.contains("Goal: add limiter"), "{prompt}");
    }

    #[test]
    fn low_confidence_draft_downgrades_to_ladder_shape() {
        let signals = bundle_with(
            analyze_goal_structure("fix the readme typo"),
            0,
            HistorySignal::default(),
            None,
        );
        let ladder = ladder_decision(&signals);
        assert_eq!(ladder.shape, CourseShape::Single);
        let (shape, n, pieces, _apply, resolution) = resolve_provider_course_plan(
            r#"{"shape":"campaign","n":4,"confidence":0.3,"rationale":"maybe several things"}"#,
            &signals,
            &ladder,
            SHAPE_CONFIDENCE_FLOOR_DEFAULT,
        )
        .expect("resolved");
        assert_eq!(shape, CourseShape::Single, "{resolution:?}");
        assert_eq!(n, None);
        assert!(pieces.is_empty());
        assert_eq!(resolution.source, ResolutionSource::Ladder);
        assert!(
            resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("downgraded to ladder")),
            "{resolution:?}"
        );
    }

    #[test]
    fn oversized_n_clamped_and_recorded_in_clamps_applied() {
        let signals = bundle_with(
            analyze_goal_structure("1. a 2. b 3. c"),
            0,
            HistorySignal::default(),
            None,
        );
        let ladder = ladder_decision(&signals);
        let draft = r#"{"shape":"plan","n":9,"confidence":0.9,"rationale":"nine pieces",
            "pieces":[{"goal":"a"},{"goal":"b"},{"goal":"c"},{"goal":"d"},{"goal":"e"},
                      {"goal":"f"},{"goal":"g"},{"goal":"h"},{"goal":"i"}]}"#;
        let (shape, n, pieces, _apply, resolution) =
            resolve_provider_course_plan(draft, &signals, &ladder, SHAPE_CONFIDENCE_FLOOR_DEFAULT)
                .expect("resolved");
        assert_eq!(shape, CourseShape::Plan);
        assert_eq!(n, Some(PLAN_MAX_PIECES));
        assert_eq!(pieces.len(), usize::from(PLAN_MAX_PIECES));
        assert!(
            resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("n clamped 9->6")),
            "{resolution:?}"
        );
        assert!(
            resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("pieces truncated 9->6")),
            "{resolution:?}"
        );
    }

    #[test]
    fn planner_failure_falls_back_to_ladder_source() {
        let signals = bundle_with(
            analyze_goal_structure("fix the readme typo"),
            0,
            HistorySignal::default(),
            None,
        );
        let ladder = ladder_decision(&signals);
        // Garbage, empty rationale, and unknown shapes all yield None — the
        // caller then uses the ladder, so a planner can never fail a launch.
        for content in [
            "no json here at all",
            r#"{"shape":"plan","n":3,"confidence":0.9,"rationale":""}"#,
            r#"{"shape":"surprise","n":3,"confidence":0.9,"rationale":"x"}"#,
        ] {
            assert!(
                resolve_provider_course_plan(
                    content,
                    &signals,
                    &ladder,
                    SHAPE_CONFIDENCE_FLOOR_DEFAULT
                )
                .is_none(),
                "{content}"
            );
        }
        // Budget clamp: an infeasible campaign downgrades with a record.
        let tight = bundle_with(
            analyze_goal_structure("1. a 2. b"),
            0,
            HistorySignal::default(),
            Some(3.0),
        );
        let tight_ladder = ladder_decision(&tight);
        let (shape, _n, _pieces, _apply, resolution) = resolve_provider_course_plan(
            r#"{"shape":"campaign","n":3,"confidence":0.95,"rationale":"three projects"}"#,
            &tight,
            &tight_ladder,
            SHAPE_CONFIDENCE_FLOOR_DEFAULT,
        )
        .expect("resolved");
        assert_eq!(shape, CourseShape::Plan, "{resolution:?}");
        assert!(
            resolution
                .clamps_applied
                .iter()
                .any(|clamp| clamp.contains("fit the budget")),
            "{resolution:?}"
        );
    }

    #[test]
    fn ladder_never_selects_campaign() {
        // Sweep a grid of synthetic bundles — many clauses, many members,
        // open budgets — and assert campaign is structurally unreachable.
        let goals = [
            "fix the readme typo",
            "add limiter and write docs then wire ci",
            "1. api 2. config 3. docs 4. tests 5. ci 6. release",
            "rebuild billing, notifications, admin, search, and export",
        ];
        for goal in goals {
            for members in [0usize, 2, 6, 12] {
                for ceiling in [None, Some(1.0), Some(50.0), Some(500.0)] {
                    let decision = ladder_decision(&bundle_with(
                        analyze_goal_structure(goal),
                        members,
                        HistorySignal::default(),
                        ceiling,
                    ));
                    assert_ne!(
                        decision.shape,
                        CourseShape::Campaign,
                        "campaign must never be ladder-chosen: {goal} {members} {ceiling:?}"
                    );
                }
            }
        }
    }
}
