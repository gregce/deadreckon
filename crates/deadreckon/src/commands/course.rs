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
}
