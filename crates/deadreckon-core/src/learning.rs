use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::events::{RUN_EVENTS_JSONL, RunEvent, RunEventKind};
use crate::flight::{FLIGHT_EVENTS_JSONL, FLIGHT_MANIFEST_JSON, read_rewind_events};
use crate::gate::{
    ACCEPTANCE_PROGRESS_JSONL, AcceptanceProgressEntry, acceptance_spec_path_for_run_root,
};
use crate::paths::{DeadreckonPaths, SOURCE_ROOT};
use crate::state::{
    PipelineState, RunStatus, append_json_line, atomic_write_json, load_state, spend_summary,
};

pub const LEARNING_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningProviderRoute {
    pub role: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningSandbox {
    pub backend: String,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningDoneCriteria {
    pub kind: String,
    pub weak: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningEpisodeMetrics {
    pub turns: u32,
    pub wall_seconds: f64,
    pub spend_usd: f64,
    pub gate_failures: u32,
    pub doc_warnings: u32,
    pub rewinds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningArtifacts {
    pub state: String,
    pub events: String,
    pub flight: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionReport {
    pub profile: String,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningEpisode {
    pub version: u32,
    pub run_id: String,
    pub scope: String,
    pub task_key: String,
    pub project_root_hash: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub operation_mode: String,
    pub provider_routes: Vec<LearningProviderRoute>,
    pub sandbox: LearningSandbox,
    pub goal_digest: String,
    pub goal_summary: String,
    pub outcome: String,
    pub done_criteria: LearningDoneCriteria,
    pub metrics: LearningEpisodeMetrics,
    pub artifacts: LearningArtifacts,
    pub redaction: RedactionReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningSignal {
    pub version: u32,
    pub signal_id: String,
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub severity: String,
    pub confidence: f64,
    pub summary: String,
    pub evidence_refs: Vec<LearningEvidenceRef>,
    pub privacy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningEvidenceRef {
    pub file: String,
    pub line: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningInsight {
    pub version: u32,
    pub insight_id: String,
    pub created_at: DateTime<Utc>,
    pub provider: LearningInsightProvider,
    pub stimulus: Vec<LearningStimulus>,
    pub summary: String,
    pub user_need: String,
    pub hypothesis: String,
    pub confidence: String,
    pub evidence_coverage: LearningEvidenceCoverage,
    #[serde(default)]
    pub rejected_claims: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningInsightProvider {
    pub route: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningStimulus {
    pub signal_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningEvidenceCoverage {
    pub signals: usize,
    pub runs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningProposal {
    pub version: u32,
    pub proposal_id: String,
    pub created_at: DateTime<Utc>,
    pub title: String,
    #[serde(default)]
    pub insights: Vec<String>,
    pub stimulus: Vec<LearningStimulus>,
    pub hypothesis: String,
    pub target: LearningProposalTarget,
    pub goal_text: String,
    pub done_criteria: Vec<String>,
    pub expected_risk: String,
    #[serde(default)]
    pub blocked_auto_pr_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningProposalTarget {
    pub repo: String,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningCandidate {
    pub version: u32,
    pub candidate_id: String,
    pub proposal_id: String,
    pub branch: String,
    pub base_commit: String,
    pub head_commit: String,
    pub run_id: String,
    pub worktree: PathBuf,
    pub diff: LearningCandidateDiff,
    pub risk: LearningRisk,
    pub status: String,
    pub evidence_packet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningCandidateDiff {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
    #[serde(default)]
    pub changed_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningRisk {
    pub class: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningEval {
    pub version: u32,
    pub candidate_id: String,
    pub evaluated_at: DateTime<Utc>,
    pub accepted_run: bool,
    pub commands: Vec<LearningEvalCommand>,
    pub docs_updated: bool,
    pub redaction_passed: bool,
    pub evidence_score: f64,
    pub auto_pr: LearningAutoPrStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningEvalCommand {
    pub cmd: String,
    pub status: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningAutoPrStatus {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningPrEvent {
    pub version: u32,
    pub timestamp: DateTime<Utc>,
    pub candidate_id: String,
    pub mode: String,
    pub status: String,
    pub branch: String,
    pub pr_url: Option<String>,
    pub body_path: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LearningPolicy {
    #[serde(default)]
    pub learning: LearningPolicyRoot,
    #[serde(default)]
    pub self_run: LearningSelfPolicy,
    #[serde(default)]
    pub pr: LearningPrPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningPolicyRoot {
    pub enabled: bool,
    pub export_redaction: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningSelfPolicy {
    pub require_isolated_worktree: bool,
    pub allow_sandbox_none: bool,
    pub verification_profile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningPrPolicy {
    pub auto_open: bool,
    pub default_dry_run: bool,
    pub min_evidence_score: f64,
    pub require_docs_for_public_surface: bool,
    pub block_high_risk: bool,
}

impl Default for LearningPolicyRoot {
    fn default() -> Self {
        Self {
            enabled: true,
            export_redaction: "local-v1".to_string(),
        }
    }
}

impl Default for LearningSelfPolicy {
    fn default() -> Self {
        Self {
            require_isolated_worktree: true,
            allow_sandbox_none: false,
            verification_profile: "focused".to_string(),
        }
    }
}

impl Default for LearningPrPolicy {
    fn default() -> Self {
        Self {
            auto_open: false,
            default_dry_run: true,
            min_evidence_score: 0.85,
            require_docs_for_public_surface: true,
            block_high_risk: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LearningIndexOptions {
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningIndexSummary {
    pub indexed: usize,
    pub signals_written: usize,
    pub skipped_live: usize,
    pub skipped_corrupt: usize,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningReport {
    pub episodes: usize,
    pub signals: usize,
    pub insights: usize,
    pub proposals: usize,
    pub signals_by_kind: BTreeMap<String, usize>,
    pub top_signals: Vec<LearningSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalGenerationReport {
    pub insights_written: usize,
    pub proposals_written: usize,
    pub proposals: Vec<LearningProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoPrDecision {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDryRun {
    pub title: String,
    pub body: String,
    pub body_path: PathBuf,
    pub branch: String,
    pub decision: AutoPrDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningBundle {
    pub version: u32,
    pub bundle_id: String,
    pub generated_at: DateTime<Utc>,
    pub source_kind: String,
    pub source_id: String,
    pub redacted: bool,
    pub redaction: RedactionReport,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub episodes: Vec<Value>,
    #[serde(default)]
    pub signals: Vec<Value>,
    #[serde(default)]
    pub insights: Vec<Value>,
    #[serde(default)]
    pub proposals: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningBundleExportReport {
    pub bundle_id: String,
    pub output: PathBuf,
    pub episodes: usize,
    pub signals: usize,
    pub insights: usize,
    pub proposals: usize,
    pub redaction: RedactionReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningBundleImportReport {
    pub bundle_id: String,
    pub preview: bool,
    pub applied: bool,
    pub episodes: usize,
    pub signals: usize,
    pub insights: usize,
    pub proposals: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReflectionEnvelope {
    #[serde(default)]
    insights: Vec<ReflectionInsight>,
    #[serde(default)]
    proposals: Vec<ReflectionProposal>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReflectionInsight {
    stimulus: Vec<LearningStimulus>,
    summary: String,
    user_need: String,
    hypothesis: String,
    confidence: String,
    #[serde(default)]
    rejected_claims: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReflectionProposal {
    title: String,
    #[serde(default)]
    insights: Vec<String>,
    stimulus: Vec<LearningStimulus>,
    hypothesis: String,
    target: LearningProposalTarget,
    goal_text: String,
    done_criteria: Vec<String>,
    expected_risk: String,
    #[serde(default)]
    blocked_auto_pr_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteStatus {
    Wrote,
    Unchanged,
}

pub fn index_learning(
    paths: &DeadreckonPaths,
    options: &LearningIndexOptions,
) -> Result<LearningIndexSummary> {
    let mut summary = LearningIndexSummary::default();
    let mut episodes = Vec::new();
    for state_path in discover_state_paths(paths) {
        let Ok(state) = load_state(&state_path) else {
            summary.skipped_corrupt += 1;
            continue;
        };
        if options
            .scope
            .as_ref()
            .is_some_and(|scope| scope != &state.scope)
        {
            continue;
        }
        if !is_terminal_status(state.status) {
            summary.skipped_live += 1;
            continue;
        }
        let episode = episode_from_state(paths, &state)?;
        if write_episode(paths, &episode)? == WriteStatus::Wrote {
            summary.indexed += 1;
        }
        summary.scopes.push(episode.scope.clone());
        episodes.push((episode, state));
    }
    summary.scopes.sort();
    summary.scopes.dedup();

    let existing_signals = read_signals(paths).unwrap_or_default();
    let mut seen_signal_ids = existing_signals
        .iter()
        .map(|signal| signal.signal_id.clone())
        .collect::<BTreeSet<_>>();
    let signals = extract_signals(&episodes);
    for signal in signals {
        if seen_signal_ids.insert(signal.signal_id.clone()) {
            append_json_line(&paths.learning_signals_path(), &signal)?;
            summary.signals_written += 1;
        }
    }

    Ok(summary)
}

pub fn read_episode(path: &Path) -> Result<LearningEpisode> {
    let raw = fs::read(path).with_path(path)?;
    ensure_schema_version(path, &raw)?;
    serde_json::from_slice(&raw).with_json_path(path)
}

pub fn read_signals(paths: &DeadreckonPaths) -> Result<Vec<LearningSignal>> {
    read_jsonl(&paths.learning_signals_path())
}

pub fn read_insights(paths: &DeadreckonPaths) -> Result<Vec<LearningInsight>> {
    read_jsonl(&paths.learning_insights_path())
}

pub fn read_proposal(paths: &DeadreckonPaths, proposal_id: &str) -> Result<LearningProposal> {
    let path = paths.learning_proposal_path(proposal_id);
    read_versioned_json(&path)
}

pub fn write_candidate(paths: &DeadreckonPaths, candidate: &LearningCandidate) -> Result<()> {
    atomic_write_json(
        &paths.learning_candidate_path(&candidate.candidate_id),
        candidate,
    )
}

pub fn write_eval(paths: &DeadreckonPaths, eval: &LearningEval) -> Result<()> {
    atomic_write_json(&paths.learning_eval_path(&eval.candidate_id), eval)
}

pub fn record_pr_event(paths: &DeadreckonPaths, event: &LearningPrEvent) -> Result<()> {
    append_json_line(&paths.learning_pr_events_path(), event)
}

pub fn learning_report(paths: &DeadreckonPaths, scope: Option<&str>) -> Result<LearningReport> {
    let mut episodes = 0usize;
    if paths.learning_dir().join("episodes").exists() {
        for entry in WalkDir::new(paths.learning_dir().join("episodes"))
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let episode = read_episode(entry.path())?;
            if scope.is_none_or(|scope| scope == episode.scope) {
                episodes += 1;
            }
        }
    }

    let signals = read_signals(paths).unwrap_or_default();
    let mut filtered_signals = Vec::new();
    for signal in signals {
        if let Some(scope) = scope
            && !signal_belongs_to_scope(paths, &signal, scope)
        {
            continue;
        }
        filtered_signals.push(signal);
    }
    let mut signals_by_kind = BTreeMap::new();
    for signal in &filtered_signals {
        *signals_by_kind.entry(signal.kind.clone()).or_insert(0) += 1;
    }
    filtered_signals.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| right.confidence.total_cmp(&left.confidence))
    });
    let insights = read_insights(paths).unwrap_or_default().len();
    let proposals = count_proposals(paths)?;

    Ok(LearningReport {
        episodes,
        signals: filtered_signals.len(),
        insights,
        proposals,
        signals_by_kind,
        top_signals: filtered_signals.into_iter().take(10).collect(),
    })
}

pub fn build_reflection_prompt(
    paths: &DeadreckonPaths,
    scope: Option<&str>,
    limit: usize,
) -> Result<String> {
    let report = learning_report(paths, scope)?;
    if report.top_signals.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "no proposal-worthy signals; try: deadreckon learn index --all".to_string(),
        ));
    }
    let payload = json!({
        "task": "Reflect on DeadReckon learning signals and produce strict JSON insights plus proposals.",
        "schema": {
            "insights": [{
                "stimulus": [{"signal_id": "sig-...", "run_id": "dr-..."}],
                "summary": "user-facing pattern",
                "user_need": "need addressed",
                "hypothesis": "testable improvement hypothesis",
                "confidence": "low|medium|high",
                "rejected_claims": []
            }],
            "proposals": [{
                "title": "short title",
                "insights": ["insight summary or generated local id reference"],
                "stimulus": [{"signal_id": "sig-...", "run_id": "dr-..."}],
                "hypothesis": "testable hypothesis",
                "target": {"repo": "/Users/gdc/deadreckon", "scope": "cli-friendliness"},
                "goal_text": "implementation goal",
                "done_criteria": ["focused tests pass"],
                "expected_risk": "low|medium|high",
                "blocked_auto_pr_reasons": []
            }]
        },
        "rules": [
            "Use only cited signal_id/run_id evidence.",
            "Do not include raw provider logs, secrets, home paths, or credentials.",
            "Every proposal must include testable done criteria."
        ],
        "signals": report.top_signals.into_iter().take(limit).collect::<Vec<_>>()
    });
    serde_json::to_string_pretty(&payload).with_json_path(paths.learning_dir().join("prompt.json"))
}

pub fn build_reflection_prompt_from_bundle(
    paths: &DeadreckonPaths,
    bundle: &LearningBundle,
    limit: usize,
) -> Result<String> {
    verify_learning_bundle(bundle)?;
    let mut signals = bundle
        .signals
        .iter()
        .cloned()
        .map(serde_json::from_value::<LearningSignal>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| DeadreckonError::Json {
            path: paths.learning_dir().join("bundle-signals.json"),
            source,
        })?;
    if signals.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "no proposal-worthy signals; try: deadreckon learn index --all".to_string(),
        ));
    }
    signals.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| right.confidence.total_cmp(&left.confidence))
    });
    let payload = json!({
        "task": "Reflect on DeadReckon learning bundle signals and produce strict JSON insights plus proposals.",
        "evidence_source": {
            "kind": "redacted_bundle",
            "bundle_id": bundle.bundle_id,
            "source_kind": bundle.source_kind,
            "source_id": bundle.source_id
        },
        "schema": {
            "insights": [{
                "stimulus": [{"signal_id": "sig-...", "run_id": "dr-..."}],
                "summary": "user-facing pattern",
                "user_need": "need addressed",
                "hypothesis": "testable improvement hypothesis",
                "confidence": "low|medium|high",
                "rejected_claims": []
            }],
            "proposals": [{
                "title": "short title",
                "insights": ["insight summary or generated local id reference"],
                "stimulus": [{"signal_id": "sig-...", "run_id": "dr-..."}],
                "hypothesis": "testable hypothesis",
                "target": {"repo": "<project-root>", "scope": "cli-friendliness"},
                "goal_text": "implementation goal",
                "done_criteria": ["focused tests pass"],
                "expected_risk": "low|medium|high",
                "blocked_auto_pr_reasons": ["imported-only evidence cannot live-open a PR without local corroboration"]
            }]
        },
        "rules": [
            "Use only cited signal_id/run_id evidence from this bundle.",
            "Do not include raw provider logs, secrets, home paths, or credentials.",
            "Every proposal must include testable done criteria.",
            "Imported-only evidence may propose work but cannot justify live PR opening by itself."
        ],
        "signals": signals.into_iter().take(limit).collect::<Vec<_>>()
    });
    serde_json::to_string_pretty(&payload).with_json_path(paths.learning_dir().join("prompt.json"))
}

pub fn export_learning_bundle(
    paths: &DeadreckonPaths,
    source_id: &str,
    output: &Path,
) -> Result<LearningBundleExportReport> {
    let (source_kind, episodes, signals, insights, proposals) =
        bundle_records_for_source(paths, source_id)?;
    let bundle_id = deterministic_id("bundle", &format!("{source_kind}:{source_id}"));
    let mut redaction = RedactionReport {
        profile: "local-v1".to_string(),
        findings: Vec::new(),
    };
    let mut bundle = LearningBundle {
        version: LEARNING_SCHEMA_VERSION,
        bundle_id: bundle_id.clone(),
        generated_at: Utc::now(),
        source_kind,
        source_id: source_id.to_string(),
        redacted: true,
        redaction: redaction.clone(),
        hashes: BTreeMap::new(),
        episodes: redact_bundle_values(episodes, paths, &mut redaction.findings),
        signals: redact_bundle_values(signals, paths, &mut redaction.findings),
        insights: redact_bundle_values(insights, paths, &mut redaction.findings),
        proposals: redact_bundle_values(proposals, paths, &mut redaction.findings),
    };
    redaction.findings.sort();
    redaction.findings.dedup();
    bundle.redaction = redaction.clone();
    bundle.hashes = bundle_hashes(&bundle)?;
    write_if_changed(
        output,
        &serde_json::to_vec_pretty(&bundle).with_json_path(output)?,
    )?;
    Ok(LearningBundleExportReport {
        bundle_id,
        output: output.to_path_buf(),
        episodes: bundle.episodes.len(),
        signals: bundle.signals.len(),
        insights: bundle.insights.len(),
        proposals: bundle.proposals.len(),
        redaction,
    })
}

pub fn read_learning_bundle(path: &Path) -> Result<LearningBundle> {
    let raw = fs::read(path).with_path(path)?;
    ensure_schema_version(path, &raw)?;
    let bundle: LearningBundle = serde_json::from_slice(&raw).with_json_path(path)?;
    verify_learning_bundle(&bundle)?;
    Ok(bundle)
}

pub fn import_learning_bundle(
    paths: &DeadreckonPaths,
    path: &Path,
    apply: bool,
) -> Result<LearningBundleImportReport> {
    let bundle = read_learning_bundle(path)?;
    let report = LearningBundleImportReport {
        bundle_id: bundle.bundle_id.clone(),
        preview: !apply,
        applied: apply,
        episodes: bundle.episodes.len(),
        signals: bundle.signals.len(),
        insights: bundle.insights.len(),
        proposals: bundle.proposals.len(),
    };
    if !apply {
        return Ok(report);
    }

    for episode in bundle_records::<LearningEpisode>(&bundle.episodes, path)? {
        atomic_write_json(
            &paths.learning_episode_path(&episode.scope, &episode.run_id),
            &episode,
        )?;
    }

    let mut existing_signals = read_signals(paths)
        .unwrap_or_default()
        .into_iter()
        .map(|signal| signal.signal_id)
        .collect::<BTreeSet<_>>();
    for signal in bundle_records::<LearningSignal>(&bundle.signals, path)? {
        if existing_signals.insert(signal.signal_id.clone()) {
            append_json_line(&paths.learning_signals_path(), &signal)?;
        }
    }

    let mut existing_insights = read_insights(paths)
        .unwrap_or_default()
        .into_iter()
        .map(|insight| insight.insight_id)
        .collect::<BTreeSet<_>>();
    for insight in bundle_records::<LearningInsight>(&bundle.insights, path)? {
        if existing_insights.insert(insight.insight_id.clone()) {
            append_json_line(&paths.learning_insights_path(), &insight)?;
        }
    }

    for proposal in bundle_records::<LearningProposal>(&bundle.proposals, path)? {
        atomic_write_json(
            &paths.learning_proposal_path(&proposal.proposal_id),
            &proposal,
        )?;
    }

    Ok(report)
}

pub fn persist_reflection(
    paths: &DeadreckonPaths,
    provider: &LearningInsightProvider,
    raw_json: &str,
    limit: usize,
) -> Result<ProposalGenerationReport> {
    let available_signals = read_signals(paths)?
        .into_iter()
        .map(|signal| (signal.signal_id, signal.run_id))
        .collect::<BTreeMap<_, _>>();
    let envelope: ReflectionEnvelope =
        serde_json::from_str(raw_json).map_err(|source| DeadreckonError::Json {
            path: paths.learning_dir().join("reflection-response.json"),
            source,
        })?;
    if envelope.insights.is_empty() || envelope.proposals.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "reflection JSON must include non-empty insights and proposals".to_string(),
        ));
    }
    let now = Utc::now();
    let mut insights_written = Vec::new();
    for insight in envelope.insights.into_iter().take(limit) {
        validate_stimulus(&available_signals, &insight.stimulus)?;
        let insight_id = deterministic_id(
            "ins",
            &json!({
                "summary": insight.summary,
                "stimulus": insight.stimulus
            })
            .to_string(),
        );
        let coverage = evidence_coverage(&insight.stimulus);
        let persisted = LearningInsight {
            version: LEARNING_SCHEMA_VERSION,
            insight_id: insight_id.clone(),
            created_at: now,
            provider: provider.clone(),
            stimulus: insight.stimulus,
            summary: insight.summary,
            user_need: insight.user_need,
            hypothesis: insight.hypothesis,
            confidence: insight.confidence,
            evidence_coverage: coverage,
            rejected_claims: insight.rejected_claims,
        };
        append_json_line(&paths.learning_insights_path(), &persisted)?;
        insights_written.push(persisted);
    }

    let insight_ids = insights_written
        .iter()
        .map(|insight| insight.insight_id.clone())
        .collect::<BTreeSet<_>>();
    let mut proposals = Vec::new();
    for proposal in envelope.proposals.into_iter().take(limit) {
        validate_stimulus(&available_signals, &proposal.stimulus)?;
        if proposal.done_criteria.is_empty() {
            return Err(DeadreckonError::InvalidInput(
                "proposal must include testable done criteria".to_string(),
            ));
        }
        let proposal_id = deterministic_id(
            "prop",
            &json!({
                "title": proposal.title,
                "goal": proposal.goal_text,
                "stimulus": proposal.stimulus
            })
            .to_string(),
        );
        let proposal_insights = if proposal.insights.is_empty() {
            insight_ids.iter().cloned().collect()
        } else {
            proposal.insights
        };
        let persisted = LearningProposal {
            version: LEARNING_SCHEMA_VERSION,
            proposal_id: proposal_id.clone(),
            created_at: now,
            title: proposal.title,
            insights: proposal_insights,
            stimulus: proposal.stimulus,
            hypothesis: proposal.hypothesis,
            target: proposal.target,
            goal_text: proposal.goal_text,
            done_criteria: proposal.done_criteria,
            expected_risk: proposal.expected_risk,
            blocked_auto_pr_reasons: proposal.blocked_auto_pr_reasons,
        };
        atomic_write_json(&paths.learning_proposal_path(&proposal_id), &persisted)?;
        proposals.push(persisted);
    }

    Ok(ProposalGenerationReport {
        insights_written: insights_written.len(),
        proposals_written: proposals.len(),
        proposals,
    })
}

pub fn classify_candidate_risk(changed_files: &[String]) -> LearningRisk {
    let mut reasons = Vec::new();
    for file in changed_files {
        if is_high_risk_path(file) {
            reasons.push(file.clone());
        }
    }
    let class = if reasons.is_empty() { "low" } else { "high" }.to_string();
    LearningRisk { class, reasons }
}

pub fn evidence_score(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    eval: &LearningEval,
) -> f64 {
    let mut score = 0.0;
    if !proposal.stimulus.is_empty() {
        score += 0.20;
    }
    if eval.accepted_run {
        score += 0.20;
    }
    if eval.commands.iter().all(|command| command.status == 0) && !eval.commands.is_empty() {
        score += 0.20;
    }
    if candidate.risk.class != "high" && candidate.diff.files > 0 {
        score += 0.15;
    }
    if eval.docs_updated {
        score += 0.10;
    }
    if eval.redaction_passed {
        score += 0.10;
    }
    if !candidate.evidence_packet.is_empty() && !candidate.branch.is_empty() {
        score += 0.05;
    }
    score
}

pub fn evaluate_auto_pr(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    eval: &LearningEval,
    policy: &LearningPolicy,
    explicit_open_pr: bool,
) -> AutoPrDecision {
    let mut reasons = Vec::new();
    if !explicit_open_pr && !policy.pr.auto_open {
        reasons.push("PR opening requires --open-pr or learning.pr.auto_open".to_string());
    }
    if candidate.worktree.as_os_str().is_empty() && policy.self_run.require_isolated_worktree {
        reasons.push("candidate must use an isolated worktree".to_string());
    }
    if proposal.done_criteria.is_empty() {
        reasons.push("proposal has weak or missing done criteria".to_string());
    }
    if !eval.accepted_run {
        reasons.push("candidate run was not accepted".to_string());
    }
    if eval.commands.is_empty() || eval.commands.iter().any(|command| command.status != 0) {
        reasons.push("focused verification did not pass".to_string());
    }
    if !eval.redaction_passed {
        reasons.push("redaction or secrets scan failed".to_string());
    }
    if eval.evidence_score < policy.pr.min_evidence_score {
        reasons.push(format!(
            "evidence score {:.2} is below {:.2}",
            eval.evidence_score, policy.pr.min_evidence_score
        ));
    }
    if policy.pr.block_high_risk && candidate.risk.class == "high" {
        reasons.push(format!(
            "high-risk diff blocked: {}",
            candidate.risk.reasons.join(", ")
        ));
    }
    if candidate.base_commit == candidate.head_commit {
        reasons.push("candidate head matches base commit".to_string());
    }

    AutoPrDecision {
        eligible: reasons.is_empty(),
        reasons,
    }
}

pub fn prepare_pr_dry_run(
    paths: &DeadreckonPaths,
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    eval: &LearningEval,
    policy: &LearningPolicy,
    explicit_open_pr: bool,
) -> Result<PrDryRun> {
    let decision = evaluate_auto_pr(proposal, candidate, eval, policy, explicit_open_pr);
    let title = format!("Self-improve: {}", proposal.title);
    let body = build_pr_body(proposal, candidate, eval, &decision);
    let body_path = paths
        .learning_candidate_dir(&candidate.candidate_id)
        .join("pr-body.md");
    write_if_changed(&body_path, body.as_bytes())?;
    append_json_line(
        &paths.learning_pr_events_path(),
        &LearningPrEvent {
            version: LEARNING_SCHEMA_VERSION,
            timestamp: Utc::now(),
            candidate_id: candidate.candidate_id.clone(),
            mode: "dry-run".to_string(),
            status: if decision.eligible {
                "prepared".to_string()
            } else {
                "refused".to_string()
            },
            branch: candidate.branch.clone(),
            pr_url: None,
            body_path: path_relative_to(&body_path, paths.home()),
            reason: (!decision.eligible).then(|| decision.reasons.join("; ")),
        },
    )?;
    Ok(PrDryRun {
        title,
        body,
        body_path,
        branch: candidate.branch.clone(),
        decision,
    })
}

pub fn load_learning_policy(paths: &DeadreckonPaths) -> Result<LearningPolicy> {
    let path = paths.learning_policy_path();
    if !path.exists() {
        return Ok(LearningPolicy::default());
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    #[derive(Deserialize)]
    struct PolicyFile {
        #[serde(default)]
        learning: LearningPolicyRoot,
        #[serde(default, rename = "self")]
        self_run: LearningSelfPolicy,
        #[serde(default)]
        pr: LearningPrPolicy,
    }
    let policy: PolicyFile =
        toml::from_str(&raw).map_err(|err| DeadreckonError::InvalidInput(err.to_string()))?;
    Ok(LearningPolicy {
        learning: policy.learning,
        self_run: policy.self_run,
        pr: policy.pr,
    })
}

type BundleRecordValues = (String, Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>);

fn bundle_records_for_source(
    paths: &DeadreckonPaths,
    source_id: &str,
) -> Result<BundleRecordValues> {
    let episodes = read_all_episodes(paths)?;
    let signals = read_signals(paths).unwrap_or_default();
    let insights = read_insights(paths).unwrap_or_default();
    let proposals = read_all_proposals(paths)?;

    if let Some(proposal) = proposals
        .iter()
        .find(|proposal| proposal.proposal_id == source_id)
        .cloned()
    {
        let signal_ids = proposal
            .stimulus
            .iter()
            .map(|item| item.signal_id.as_str())
            .collect::<BTreeSet<_>>();
        let run_ids = proposal
            .stimulus
            .iter()
            .map(|item| item.run_id.as_str())
            .collect::<BTreeSet<_>>();
        let insight_ids = proposal
            .insights
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        return Ok((
            "proposal".to_string(),
            to_values(
                episodes
                    .into_iter()
                    .filter(|episode| run_ids.contains(episode.run_id.as_str()))
                    .collect::<Vec<_>>(),
            )?,
            to_values(
                signals
                    .into_iter()
                    .filter(|signal| signal_ids.contains(signal.signal_id.as_str()))
                    .collect::<Vec<_>>(),
            )?,
            to_values(
                insights
                    .into_iter()
                    .filter(|insight| {
                        insight_ids.contains(insight.insight_id.as_str())
                            || insight
                                .stimulus
                                .iter()
                                .any(|item| signal_ids.contains(item.signal_id.as_str()))
                    })
                    .collect::<Vec<_>>(),
            )?,
            to_values(vec![proposal])?,
        ));
    }

    let matching_episodes = episodes
        .into_iter()
        .filter(|episode| episode.run_id == source_id)
        .collect::<Vec<_>>();
    if matching_episodes.is_empty() {
        return Err(DeadreckonError::InvalidInput(format!(
            "no indexed run or proposal {source_id}; try: deadreckon learn index --all"
        )));
    }
    let run_ids = matching_episodes
        .iter()
        .map(|episode| episode.run_id.as_str())
        .collect::<BTreeSet<_>>();
    let matching_signals = signals
        .into_iter()
        .filter(|signal| run_ids.contains(signal.run_id.as_str()))
        .collect::<Vec<_>>();
    let signal_ids = matching_signals
        .iter()
        .map(|signal| signal.signal_id.clone())
        .collect::<BTreeSet<_>>();
    Ok((
        "run".to_string(),
        to_values(matching_episodes)?,
        to_values(matching_signals)?,
        to_values(
            insights
                .into_iter()
                .filter(|insight| {
                    insight
                        .stimulus
                        .iter()
                        .any(|item| signal_ids.contains(&item.signal_id))
                })
                .collect::<Vec<_>>(),
        )?,
        to_values(
            proposals
                .into_iter()
                .filter(|proposal| {
                    proposal
                        .stimulus
                        .iter()
                        .any(|item| signal_ids.contains(&item.signal_id))
                })
                .collect::<Vec<_>>(),
        )?,
    ))
}

fn read_all_episodes(paths: &DeadreckonPaths) -> Result<Vec<LearningEpisode>> {
    let root = paths.learning_dir().join("episodes");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut episodes = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            episodes.push(read_episode(entry.path())?);
        }
    }
    Ok(episodes)
}

fn read_all_proposals(paths: &DeadreckonPaths) -> Result<Vec<LearningProposal>> {
    let dir = paths.learning_proposals_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut proposals = Vec::new();
    for entry in fs::read_dir(&dir).with_path(&dir)? {
        let entry = entry.with_path(&dir)?;
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            proposals.push(read_versioned_json(&entry.path())?);
        }
    }
    Ok(proposals)
}

fn to_values<T>(items: Vec<T>) -> Result<Vec<Value>>
where
    T: Serialize,
{
    items
        .into_iter()
        .map(|item| {
            serde_json::to_value(item).map_err(|source| DeadreckonError::Json {
                path: PathBuf::from("<learning-bundle>"),
                source,
            })
        })
        .collect()
}

fn bundle_records<T>(values: &[Value], path: &Path) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    values
        .iter()
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|source| DeadreckonError::Json {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect()
}

fn verify_learning_bundle(bundle: &LearningBundle) -> Result<()> {
    if bundle.version != LEARNING_SCHEMA_VERSION {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported learning schema version {}",
            bundle.version
        )));
    }
    if !bundle.redacted {
        return Err(DeadreckonError::InvalidInput(
            "learning bundle must be redacted; try: deadreckon learn export <id> --redacted"
                .to_string(),
        ));
    }
    let actual = bundle_hashes(bundle)?;
    for (section, expected) in &bundle.hashes {
        if actual.get(section) != Some(expected) {
            return Err(DeadreckonError::InvalidInput(format!(
                "bundle hash mismatch for {section}; try: deadreckon learn export <id> --redacted"
            )));
        }
    }
    if actual.len() != bundle.hashes.len() {
        return Err(DeadreckonError::InvalidInput(
            "bundle hash manifest is incomplete; try: deadreckon learn export <id> --redacted"
                .to_string(),
        ));
    }
    Ok(())
}

fn bundle_hashes(bundle: &LearningBundle) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    hashes.insert("episodes".to_string(), hash_json_values(&bundle.episodes)?);
    hashes.insert("signals".to_string(), hash_json_values(&bundle.signals)?);
    hashes.insert("insights".to_string(), hash_json_values(&bundle.insights)?);
    hashes.insert(
        "proposals".to_string(),
        hash_json_values(&bundle.proposals)?,
    );
    Ok(hashes)
}

fn hash_json_values(values: &[Value]) -> Result<String> {
    let raw = serde_json::to_vec(values).map_err(|source| DeadreckonError::Json {
        path: PathBuf::from("<learning-bundle-hash>"),
        source,
    })?;
    Ok(format!("sha256:{}", sha256_hex(&raw)))
}

fn redact_bundle_values(
    values: Vec<Value>,
    paths: &DeadreckonPaths,
    findings: &mut Vec<String>,
) -> Vec<Value> {
    values
        .into_iter()
        .map(|value| redact_bundle_value(value, paths, findings))
        .collect()
}

fn redact_bundle_value(value: Value, paths: &DeadreckonPaths, findings: &mut Vec<String>) -> Value {
    match value {
        Value::String(raw) => Value::String(redact_learning_string(&raw, paths, findings)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact_bundle_value(item, paths, findings))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_bundle_value(value, paths, findings)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_learning_string(
    value: &str,
    paths: &DeadreckonPaths,
    findings: &mut Vec<String>,
) -> String {
    if secret_like(value) {
        findings.push("secret-like value redacted".to_string());
        return "<redacted-secret>".to_string();
    }
    let mut redacted = value.to_string();
    let home = paths.home().to_string_lossy();
    if !home.is_empty() && redacted.contains(home.as_ref()) {
        findings.push("deadreckon home path redacted".to_string());
        redacted = redacted.replace(home.as_ref(), "<deadreckon-home>");
    }
    if redacted.contains(SOURCE_ROOT) {
        findings.push("project root path redacted".to_string());
        redacted = redacted.replace(SOURCE_ROOT, "<project-root>");
    }
    if let Ok(user_home) = std::env::var("HOME")
        && !user_home.is_empty()
        && redacted.contains(&user_home)
    {
        findings.push("user home path redacted".to_string());
        redacted = redacted.replace(&user_home, "<home>");
    }
    redacted
}

fn secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("ghp_")
        || lower.contains("github_pat_")
        || lower.contains("sk-")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("begin openssh private key")
        || lower.contains("begin private key")
}

fn discover_state_paths(paths: &DeadreckonPaths) -> Vec<PathBuf> {
    let root = paths.runstate_dir();
    if !root.exists() {
        return Vec::new();
    }
    let mut state_paths = Vec::new();
    for entry in WalkDir::new(&root)
        .max_depth(4)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file() && entry.file_name() == "state.json" {
            state_paths.push(entry.path().to_path_buf());
        }
    }
    state_paths.sort();
    state_paths
}

fn is_terminal_status(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Killed
    )
}

fn episode_from_state(paths: &DeadreckonPaths, state: &PipelineState) -> Result<LearningEpisode> {
    let spend = spend_summary(state)?;
    let gate_failures = count_gate_failures(state);
    let doc_warnings = count_doc_warnings(state);
    let rewinds = read_rewind_events(state).unwrap_or_default().len() as u32;
    let flight = state
        .run_root
        .join(FLIGHT_EVENTS_JSONL)
        .exists()
        .then(|| path_relative_to(&state.run_root.join(FLIGHT_EVENTS_JSONL), paths.home()));
    let provider_routes = state
        .provider
        .as_ref()
        .map(|provider| {
            vec![LearningProviderRoute {
                role: "primary".to_string(),
                id: provider.clone(),
            }]
        })
        .unwrap_or_default();
    let done_criteria = if acceptance_spec_path_for_run_root(&state.run_root).exists() {
        LearningDoneCriteria {
            kind: "project".to_string(),
            weak: false,
        }
    } else {
        LearningDoneCriteria {
            kind: "default".to_string(),
            weak: true,
        }
    };
    Ok(LearningEpisode {
        version: LEARNING_SCHEMA_VERSION,
        run_id: state.run_id.clone(),
        scope: state.scope.clone(),
        task_key: state.task_key.clone(),
        project_root_hash: sha256_hex(state.cwd.to_string_lossy().as_bytes()),
        created_at: state.started_at,
        completed_at: is_terminal_status(state.status).then_some(state.updated_at),
        operation_mode: operation_mode(state),
        provider_routes,
        sandbox: LearningSandbox {
            backend: state.sandbox.clone(),
            mode: codebase_mode_for_state(state),
        },
        goal_digest: sha256_hex(state.goal.as_bytes()),
        goal_summary: redact_text(&summarize(&state.goal), paths, state),
        outcome: run_status_outcome(state.status).to_string(),
        done_criteria,
        metrics: LearningEpisodeMetrics {
            turns: state.turn,
            wall_seconds: spend.wall_seconds,
            spend_usd: spend.total_usd,
            gate_failures,
            doc_warnings,
            rewinds,
        },
        artifacts: LearningArtifacts {
            state: path_relative_to(&state.state_path(), paths.home()),
            events: path_relative_to(&state.run_root.join(RUN_EVENTS_JSONL), paths.home()),
            flight,
        },
        redaction: RedactionReport {
            profile: "local-v1".to_string(),
            findings: Vec::new(),
        },
    })
}

fn write_episode(paths: &DeadreckonPaths, episode: &LearningEpisode) -> Result<WriteStatus> {
    let path = paths.learning_episode_path(&episode.scope, &episode.run_id);
    let data = serde_json::to_vec_pretty(episode).with_json_path(&path)?;
    write_if_changed(&path, &data)
}

fn write_if_changed(path: &Path, data: &[u8]) -> Result<WriteStatus> {
    let with_newline = if data.ends_with(b"\n") {
        data.to_vec()
    } else {
        let mut value = data.to_vec();
        value.push(b'\n');
        value
    };
    if let Ok(existing) = fs::read(path)
        && existing == with_newline
    {
        return Ok(WriteStatus::Unchanged);
    }
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    temp.write_all(&with_newline).with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    temp.persist(path).map_err(|err| DeadreckonError::Io {
        path: path.to_path_buf(),
        source: err.error,
    })?;
    Ok(WriteStatus::Wrote)
}

fn extract_signals(episodes: &[(LearningEpisode, PipelineState)]) -> Vec<LearningSignal> {
    let mut signals = Vec::new();
    let mut failed_task_counts = BTreeMap::<String, usize>::new();
    for (episode, _state) in episodes {
        if matches!(episode.outcome.as_str(), "failed" | "killed") {
            *failed_task_counts
                .entry(episode.task_key.clone())
                .or_insert(0) += 1;
        }
    }

    for (episode, state) in episodes {
        if state
            .failure_reason
            .as_deref()
            .is_some_and(provider_setup_failure)
        {
            signals.push(signal(
                episode,
                "setup_friction",
                "high",
                0.90,
                "provider setup or route failure blocked the run",
                RUN_EVENTS_JSONL,
            ));
        }
        if episode.metrics.gate_failures > 0 {
            signals.push(signal(
                episode,
                "acceptance_gap",
                "medium",
                0.86,
                "done criteria failed before the run recovered or stopped",
                ACCEPTANCE_PROGRESS_JSONL,
            ));
        }
        if episode
            .provider_routes
            .iter()
            .any(|route| route.id.starts_with("cli:"))
            && episode.artifacts.flight.is_none()
        {
            signals.push(signal(
                episode,
                "provider_gap",
                "medium",
                0.78,
                "CLI provider run has no provider-native flight events",
                FLIGHT_MANIFEST_JSON,
            ));
        }
        if episode.metrics.wall_seconds >= 900.0 && episode.metrics.turns <= 1 {
            signals.push(signal(
                episode,
                "slow_path",
                "medium",
                0.72,
                "long wallclock with little visible turn progress",
                "spend.jsonl",
            ));
        }
        if episode.metrics.doc_warnings > 0 {
            signals.push(signal(
                episode,
                "docs_drift",
                "medium",
                0.80,
                "run documentation produced warnings or stale-note feedback",
                RUN_EVENTS_JSONL,
            ));
        }
        if episode.metrics.spend_usd >= 5.0 {
            signals.push(signal(
                episode,
                "cost_spike",
                "low",
                0.70,
                "run spend exceeded the local cost spike threshold",
                "spend.jsonl",
            ));
        }
        if failed_task_counts
            .get(&episode.task_key)
            .is_some_and(|count| *count > 1)
        {
            signals.push(signal(
                episode,
                "repeat_failure",
                "high",
                0.84,
                "similar task failed or was killed more than once",
                "state.json",
            ));
        }
    }
    signals
}

fn signal(
    episode: &LearningEpisode,
    kind: &str,
    severity: &str,
    confidence: f64,
    summary: &str,
    file: &str,
) -> LearningSignal {
    let signal_id = deterministic_id("sig", &format!("{}:{}:{}", episode.run_id, kind, summary));
    LearningSignal {
        version: LEARNING_SCHEMA_VERSION,
        signal_id,
        run_id: episode.run_id.clone(),
        timestamp: Utc::now(),
        kind: kind.to_string(),
        severity: severity.to_string(),
        confidence,
        summary: summary.to_string(),
        evidence_refs: vec![LearningEvidenceRef {
            file: file.to_string(),
            line: 1,
        }],
        privacy: "shareable-redacted".to_string(),
    }
}

fn validate_stimulus(
    available_signals: &BTreeMap<String, String>,
    stimulus: &[LearningStimulus],
) -> Result<()> {
    if stimulus.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "insight/proposal must cite at least one signal".to_string(),
        ));
    }
    for item in stimulus {
        match available_signals.get(&item.signal_id) {
            Some(run_id) if run_id == &item.run_id => {}
            Some(_) => {
                return Err(DeadreckonError::InvalidInput(format!(
                    "signal {} does not belong to run {}",
                    item.signal_id, item.run_id
                )));
            }
            None => {
                return Err(DeadreckonError::InvalidInput(format!(
                    "unknown signal {}",
                    item.signal_id
                )));
            }
        }
    }
    Ok(())
}

fn evidence_coverage(stimulus: &[LearningStimulus]) -> LearningEvidenceCoverage {
    let signals = stimulus
        .iter()
        .map(|item| item.signal_id.as_str())
        .collect::<BTreeSet<_>>();
    let runs = stimulus
        .iter()
        .map(|item| item.run_id.as_str())
        .collect::<BTreeSet<_>>();
    LearningEvidenceCoverage {
        signals: signals.len(),
        runs: runs.len(),
    }
}

fn build_pr_body(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    eval: &LearningEval,
    decision: &AutoPrDecision,
) -> String {
    let rollback_worktree = if candidate.worktree.is_absolute() {
        "<candidate-worktree>".to_string()
    } else {
        candidate.worktree.display().to_string()
    };
    format!(
        "\
## Summary

{}

## Stimulus and Proposal

- proposal: {}
- hypothesis: {}
- signals: {}

## Evidence Packet

- candidate: {}
- run: {}
- evidence score: {:.2}
- evidence packet: {}

## Verification

{}

## Risk Classification

- class: {}
- reasons: {}
- auto-pr eligible: {}

## Rollback

`git branch -D {} && git worktree remove {}`

## Files Changed

{}
",
        proposal.title,
        proposal.proposal_id,
        proposal.hypothesis,
        proposal
            .stimulus
            .iter()
            .map(|item| format!("{}:{}", item.signal_id, item.run_id))
            .collect::<Vec<_>>()
            .join(", "),
        candidate.candidate_id,
        candidate.run_id,
        eval.evidence_score,
        candidate.evidence_packet,
        eval.commands
            .iter()
            .map(|command| format!("- `{}` -> {}", command.cmd, command.status))
            .collect::<Vec<_>>()
            .join("\n"),
        candidate.risk.class,
        if candidate.risk.reasons.is_empty() {
            "none".to_string()
        } else {
            candidate.risk.reasons.join(", ")
        },
        decision.eligible,
        candidate.branch,
        rollback_worktree,
        if candidate.diff.changed_files.is_empty() {
            "- none recorded".to_string()
        } else {
            candidate
                .diff
                .changed_files
                .iter()
                .map(|file| format!("- {file}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    )
}

fn is_high_risk_path(file: &str) -> bool {
    file == "crates/deadreckon-core/src/gate.rs"
        || file == "crates/deadreckon/src/bin/dr-gate.rs"
        || file.starts_with("crates/deadreckon-sandbox/")
        || file.starts_with(".github/workflows/")
        || file.starts_with("release/")
        || file.contains("credential")
        || file.contains("config")
        || file.contains("acceptance")
}

fn provider_setup_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("provider")
        || lower.contains("credential")
        || lower.contains("no route")
        || lower.contains("unknown provider")
        || lower.contains("config")
}

fn signal_belongs_to_scope(paths: &DeadreckonPaths, signal: &LearningSignal, scope: &str) -> bool {
    let episode_path = paths.learning_episode_path(scope, &signal.run_id);
    episode_path.exists()
}

fn count_proposals(paths: &DeadreckonPaths) -> Result<usize> {
    let dir = paths.learning_proposals_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(&dir).with_path(&dir)? {
        let entry = entry.with_path(&dir)?;
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            count += 1;
        }
    }
    Ok(count)
}

fn count_gate_failures(state: &PipelineState) -> u32 {
    let path = state
        .run_root
        .join("proofs")
        .join(ACCEPTANCE_PROGRESS_JSONL);
    read_jsonl::<AcceptanceProgressEntry>(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry.status == "failed"
                || entry
                    .result
                    .as_ref()
                    .is_some_and(|result| result.must_pass && !result.passed)
        })
        .count() as u32
}

fn count_doc_warnings(state: &PipelineState) -> u32 {
    read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL))
        .unwrap_or_default()
        .into_iter()
        .filter(|event| match &event.event {
            RunEventKind::DocsCheckpoint { status, .. } => {
                matches!(status.as_str(), "warning" | "failed" | "stale")
            }
            RunEventKind::Error { message, .. } => message.to_ascii_lowercase().contains("docs"),
            _ => false,
        })
        .count() as u32
}

fn operation_mode(state: &PipelineState) -> String {
    if state.run_root.join("import.json").exists() {
        "import"
    } else if state.run_root.join("plan-child.json").exists() {
        "orchestrate"
    } else {
        "run"
    }
    .to_string()
}

fn codebase_mode_for_state(state: &PipelineState) -> String {
    if state.working_dir == state.cwd {
        "in-place".to_string()
    } else if state.working_dir.ends_with("working") {
        "copy-or-fresh".to_string()
    } else {
        "worktree".to_string()
    }
}

fn run_status_outcome(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Killed => "killed",
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => "paused",
    }
}

fn summarize(value: &str) -> String {
    let mut summary = value.trim().replace('\n', " ");
    const MAX: usize = 180;
    if summary.len() > MAX {
        summary.truncate(MAX);
        summary.push_str("...");
    }
    summary
}

fn redact_text(value: &str, paths: &DeadreckonPaths, state: &PipelineState) -> String {
    let redacted = value
        .replace(paths.home().to_string_lossy().as_ref(), "<deadreckon-home>")
        .replace(state.cwd.to_string_lossy().as_ref(), "<project-root>")
        .replace(
            state.working_dir.to_string_lossy().as_ref(),
            "<working-dir>",
        );
    if secret_like(&redacted) {
        "<redacted-secret>".to_string()
    } else {
        redacted
    }
}

fn path_relative_to(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn deterministic_id(prefix: &str, input: &str) -> String {
    let digest = sha256_hex(input.as_bytes());
    format!("{prefix}-{}", &digest[..16])
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn read_jsonl<T>(path: &Path) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut records = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        records.push(
            serde_json::from_str(line).map_err(|source| DeadreckonError::Json {
                path: path.to_path_buf(),
                source,
            })?,
        );
    }
    Ok(records)
}

fn read_versioned_json<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned,
{
    let raw = fs::read(path).with_path(path)?;
    ensure_schema_version(path, &raw)?;
    serde_json::from_slice(&raw).with_json_path(path)
}

fn ensure_schema_version(path: &Path, raw: &[u8]) -> Result<()> {
    let value: Value = serde_json::from_slice(raw).with_json_path(path)?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version != u64::from(LEARNING_SCHEMA_VERSION) {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported learning schema version {version}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::flight::{FlightEvent, FlightEventKind, append_flight_event};
    use crate::gate::AcceptanceProgressEntry;
    use crate::state::{RunOptions, create_run, save_state};

    fn temp_paths() -> (TempDir, DeadreckonPaths) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        (temp, paths)
    }

    fn completed_state(paths: &DeadreckonPaths, goal: &str) -> PipelineState {
        let cwd = std::env::current_dir().expect("cwd");
        let mut state = create_run(
            paths,
            RunOptions {
                goal: goal.to_string(),
                cwd,
                sandbox: "seatbelt".to_string(),
                provider: Some("cli:codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        state.status = RunStatus::Completed;
        state.turn = 2;
        state.updated_at = Utc::now();
        save_state(&state).expect("save");
        state
    }

    fn proposal_fixture(done_criteria: Vec<&str>) -> LearningProposal {
        LearningProposal {
            version: 1,
            proposal_id: "prop-1".to_string(),
            created_at: Utc::now(),
            title: "Improve".to_string(),
            insights: vec!["ins-1".to_string()],
            stimulus: vec![LearningStimulus {
                signal_id: "sig-1".to_string(),
                run_id: "run-1".to_string(),
            }],
            hypothesis: "helps".to_string(),
            target: LearningProposalTarget {
                repo: "/Users/gdc/deadreckon".to_string(),
                scope: "cli".to_string(),
            },
            goal_text: "goal".to_string(),
            done_criteria: done_criteria.into_iter().map(str::to_string).collect(),
            expected_risk: "low".to_string(),
            blocked_auto_pr_reasons: Vec::new(),
        }
    }

    fn candidate_fixture(paths: &DeadreckonPaths) -> LearningCandidate {
        LearningCandidate {
            version: 1,
            candidate_id: "cand-1".to_string(),
            proposal_id: "prop-1".to_string(),
            branch: "deadreckon/self/cand-1".to_string(),
            base_commit: "base".to_string(),
            head_commit: "head".to_string(),
            run_id: "run-2".to_string(),
            worktree: paths.learning_candidate_dir("cand-1").join("worktree"),
            diff: LearningCandidateDiff {
                files: 1,
                insertions: 10,
                deletions: 0,
                changed_files: vec!["crates/deadreckon/src/main.rs".to_string()],
            },
            risk: LearningRisk {
                class: "low".to_string(),
                reasons: Vec::new(),
            },
            status: "verified".to_string(),
            evidence_packet: "evidence.json".to_string(),
        }
    }

    fn eval_fixture(candidate_id: &str) -> LearningEval {
        LearningEval {
            version: 1,
            candidate_id: candidate_id.to_string(),
            evaluated_at: Utc::now(),
            accepted_run: true,
            commands: vec![LearningEvalCommand {
                cmd: "cargo test -p deadreckon-core learning --lib".to_string(),
                status: 0,
            }],
            docs_updated: true,
            redaction_passed: true,
            evidence_score: 1.0,
            auto_pr: LearningAutoPrStatus {
                eligible: false,
                reasons: Vec::new(),
            },
        }
    }

    #[test]
    fn learning_schemas_roundtrip_and_reject_unknown_major_version() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "schema");
        let episode = episode_from_state(&paths, &state).expect("episode");
        assert_eq!(
            write_episode(&paths, &episode).expect("write"),
            WriteStatus::Wrote
        );

        let path = paths.learning_episode_path(&episode.scope, &episode.run_id);
        let loaded = read_episode(&path).expect("read");
        assert_eq!(loaded.version, LEARNING_SCHEMA_VERSION);

        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        value["version"] = json!(2);
        fs::write(&path, serde_json::to_vec_pretty(&value).expect("json")).expect("write");
        let err = read_episode(&path).expect_err("reject");
        assert!(
            err.to_string()
                .contains("unsupported learning schema version")
        );
    }

    #[test]
    fn episode_writer_is_idempotent_for_unchanged_run() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "idempotent");
        let episode = episode_from_state(&paths, &state).expect("episode");

        assert_eq!(
            write_episode(&paths, &episode).expect("first"),
            WriteStatus::Wrote
        );
        assert_eq!(
            write_episode(&paths, &episode).expect("second"),
            WriteStatus::Unchanged
        );
    }

    #[test]
    fn learn_index_writes_episode_from_completed_run() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "index completed");

        let summary = index_learning(&paths, &LearningIndexOptions::default()).expect("index");

        assert_eq!(summary.indexed, 1);
        assert!(
            paths
                .learning_episode_path(&state.scope, &state.run_id)
                .exists()
        );
    }

    #[test]
    fn learn_index_skips_live_run_without_failure() {
        let (_temp, paths) = temp_paths();
        let cwd = std::env::current_dir().expect("cwd");
        create_run(
            &paths,
            RunOptions {
                goal: "live".to_string(),
                cwd,
                sandbox: "seatbelt".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");

        let summary = index_learning(&paths, &LearningIndexOptions::default()).expect("index");

        assert_eq!(summary.indexed, 0);
        assert_eq!(summary.skipped_live, 1);
    }

    #[test]
    fn learn_index_includes_flight_and_gate_metrics() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "flight gate");
        append_flight_event(
            &state,
            &FlightEvent {
                version: 1,
                seq: 1,
                run_id: state.run_id.clone(),
                flight_session_id: "flight-turn-1-attempt-1".to_string(),
                deadreckon_turn: 1,
                attempt: 1,
                provider: "cli:codex".to_string(),
                schema: "codex-cli".to_string(),
                timestamp: Some(Utc::now()),
                source_path: None,
                source_line: None,
                source_event: "test".to_string(),
                raw_hash: "sha256:test".to_string(),
                kind: FlightEventKind::Tool,
                role: None,
                summary: "tool edit".to_string(),
                tool_name: Some("apply_patch".to_string()),
                tool_category: Some("write".to_string()),
                files: Vec::new(),
                usage: None,
                checkpoint_id: None,
            },
        )
        .expect("flight");
        append_json_line(
            &state
                .run_root
                .join("proofs")
                .join(ACCEPTANCE_PROGRESS_JSONL),
            &AcceptanceProgressEntry {
                checked_at: Utc::now(),
                status: "failed".to_string(),
                index: 1,
                total: 1,
                result: None,
            },
        )
        .expect("acceptance");

        index_learning(&paths, &LearningIndexOptions::default()).expect("index");
        let episode = read_episode(&paths.learning_episode_path(&state.scope, &state.run_id))
            .expect("episode");

        assert!(episode.artifacts.flight.is_some());
        assert_eq!(episode.metrics.gate_failures, 1);
    }

    #[test]
    fn signal_rules_extract_setup_friction_from_repeated_provider_failures() {
        let (_temp, paths) = temp_paths();
        let mut state = completed_state(&paths, "provider missing");
        state.status = RunStatus::Failed;
        state.failure_reason = Some("provider route cli:missing has no credential".to_string());
        save_state(&state).expect("save");

        index_learning(&paths, &LearningIndexOptions::default()).expect("index");
        let signals = read_signals(&paths).expect("signals");

        assert!(signals.iter().any(|signal| signal.kind == "setup_friction"));
    }

    #[test]
    fn signal_rules_extract_acceptance_gap_from_gate_retry() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "acceptance gap");
        append_json_line(
            &state
                .run_root
                .join("proofs")
                .join(ACCEPTANCE_PROGRESS_JSONL),
            &AcceptanceProgressEntry {
                checked_at: Utc::now(),
                status: "failed".to_string(),
                index: 1,
                total: 1,
                result: None,
            },
        )
        .expect("acceptance");

        index_learning(&paths, &LearningIndexOptions::default()).expect("index");
        let signals = read_signals(&paths).expect("signals");

        assert!(signals.iter().any(|signal| signal.kind == "acceptance_gap"));
    }

    #[test]
    fn learn_propose_requires_provider_reflection_before_writing_proposal() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "propose");
        let episode = episode_from_state(&paths, &state).expect("episode");
        append_json_line(
            &paths.learning_signals_path(),
            &signal(
                &episode,
                "setup_friction",
                "high",
                0.9,
                "provider setup issue",
                RUN_EVENTS_JSONL,
            ),
        )
        .expect("signal");

        let err = persist_reflection(
            &paths,
            &LearningInsightProvider {
                route: "smoke".to_string(),
                model: "test".to_string(),
            },
            r#"{"insights":[],"proposals":[]}"#,
            1,
        )
        .expect_err("reject");

        assert!(err.to_string().contains("non-empty insights and proposals"));
    }

    #[test]
    fn learn_propose_refuses_when_no_signal_meets_threshold() {
        let (_temp, paths) = temp_paths();

        let err = build_reflection_prompt(&paths, None, 3).expect_err("reject");

        assert!(err.to_string().contains("no proposal-worthy signals"));
        assert!(
            err.to_string()
                .contains("try: deadreckon learn index --all")
        );
    }

    #[test]
    fn learn_propose_requires_insight_signal_citations_and_done_criteria() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "proposal citations");
        let episode = episode_from_state(&paths, &state).expect("episode");
        let sig = signal(
            &episode,
            "setup_friction",
            "high",
            0.9,
            "provider setup issue",
            RUN_EVENTS_JSONL,
        );
        append_json_line(&paths.learning_signals_path(), &sig).expect("signal");
        let raw = serde_json::to_string(&json!({
            "insights": [{
                "stimulus": [{"signal_id": sig.signal_id, "run_id": state.run_id}],
                "summary": "setup hurts",
                "user_need": "recover faster",
                "hypothesis": "better footer helps",
                "confidence": "medium"
            }],
            "proposals": [{
                "title": "Improve setup footer",
                "stimulus": [{"signal_id": sig.signal_id, "run_id": state.run_id}],
                "hypothesis": "better footer helps",
                "target": {"repo": "/Users/gdc/deadreckon", "scope": "cli"},
                "goal_text": "Add a clearer setup footer.",
                "done_criteria": ["focused test covers footer"],
                "expected_risk": "low"
            }]
        }))
        .expect("json");

        let report = persist_reflection(
            &paths,
            &LearningInsightProvider {
                route: "smoke".to_string(),
                model: "test".to_string(),
            },
            &raw,
            1,
        )
        .expect("persist");

        assert_eq!(report.insights_written, 1);
        assert_eq!(report.proposals_written, 1);
        assert!(
            paths
                .learning_proposal_path(&report.proposals[0].proposal_id)
                .exists()
        );
    }

    #[test]
    fn learn_propose_invalid_reflection_json_does_not_write_insight_or_proposal() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "invalid reflection");
        let episode = episode_from_state(&paths, &state).expect("episode");
        let sig = signal(
            &episode,
            "setup_friction",
            "high",
            0.9,
            "provider setup issue",
            RUN_EVENTS_JSONL,
        );
        append_json_line(&paths.learning_signals_path(), &sig).expect("signal");

        let err = persist_reflection(
            &paths,
            &LearningInsightProvider {
                route: "smoke".to_string(),
                model: "test".to_string(),
            },
            "{not-json",
            1,
        )
        .expect_err("reject");

        assert!(matches!(err, DeadreckonError::Json { .. }));
        assert!(!paths.learning_insights_path().exists());
        assert!(!paths.learning_proposals_dir().exists());
    }

    #[test]
    fn learn_export_redacts_home_paths_provider_logs_and_secret_like_values() {
        let (_temp, paths) = temp_paths();
        let secret = "sk-test-secret";
        let state = completed_state(
            &paths,
            &format!("inspect {} and token {secret}", paths.home().display()),
        );
        index_learning(&paths, &LearningIndexOptions::default()).expect("index");

        let output = paths.learning_bundle_path("redacted-test");
        let report = export_learning_bundle(&paths, &state.run_id, &output).expect("export");
        let raw = fs::read_to_string(&output).expect("bundle");

        assert_eq!(report.episodes, 1);
        assert!(!raw.contains(paths.home().to_string_lossy().as_ref()));
        assert!(!raw.contains(secret));
        assert!(raw.contains("<redacted-secret>"));
        let bundle = read_learning_bundle(&output).expect("read bundle");
        assert!(bundle.redacted);
        assert!(bundle.hashes.contains_key("episodes"));
    }

    #[test]
    fn learn_import_bundle_preview_refuses_unredacted_bundle() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "bundle preview");
        index_learning(&paths, &LearningIndexOptions::default()).expect("index");
        let output = paths.learning_bundle_path("unredacted-test");
        export_learning_bundle(&paths, &state.run_id, &output).expect("export");
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&output).expect("bundle")).expect("json");
        value["redacted"] = json!(false);
        fs::write(&output, serde_json::to_vec_pretty(&value).expect("json")).expect("write");

        let err = import_learning_bundle(&paths, &output, false).expect_err("reject");

        assert!(err.to_string().contains("learning bundle must be redacted"));
        assert!(err.to_string().contains("try: deadreckon learn export"));
    }

    #[test]
    fn learn_import_bundle_hash_mismatch_has_try_footer() {
        let (_temp, paths) = temp_paths();
        let state = completed_state(&paths, "bundle hash");
        index_learning(&paths, &LearningIndexOptions::default()).expect("index");
        let output = paths.learning_bundle_path("hash-test");
        export_learning_bundle(&paths, &state.run_id, &output).expect("export");
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&output).expect("bundle")).expect("json");
        value["signals"][0]["summary"] = json!("tampered");
        fs::write(&output, serde_json::to_vec_pretty(&value).expect("json")).expect("write");

        let err = import_learning_bundle(&paths, &output, false).expect_err("reject");

        assert!(err.to_string().contains("bundle hash mismatch"));
        assert!(err.to_string().contains("try: deadreckon learn export"));
    }

    #[test]
    fn candidate_archive_records_base_head_diff_and_run_id() {
        let (_temp, paths) = temp_paths();
        let candidate = candidate_fixture(&paths);

        write_candidate(&paths, &candidate).expect("candidate");
        let raw = fs::read(paths.learning_candidate_path(&candidate.candidate_id)).expect("read");
        let loaded: LearningCandidate = serde_json::from_slice(&raw).expect("json");

        assert_eq!(loaded.base_commit, "base");
        assert_eq!(loaded.head_commit, "head");
        assert_eq!(loaded.run_id, "run-2");
        assert_eq!(loaded.diff.files, 1);
        assert_eq!(loaded.diff.changed_files, candidate.diff.changed_files);
    }

    #[test]
    fn evaluation_policy_blocks_weak_done_criteria() {
        let (_temp, paths) = temp_paths();
        let proposal = proposal_fixture(Vec::new());
        let candidate = candidate_fixture(&paths);
        let eval = eval_fixture(&candidate.candidate_id);

        let decision = evaluate_auto_pr(
            &proposal,
            &candidate,
            &eval,
            &LearningPolicy::default(),
            true,
        );

        assert!(!decision.eligible);
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.contains("weak or missing done criteria"))
        );
    }

    #[test]
    fn pr_gate_passes_only_with_complete_evidence_packet() {
        let (_temp, paths) = temp_paths();
        let proposal = proposal_fixture(vec!["focused tests pass"]);
        let candidate = candidate_fixture(&paths);
        let mut eval = eval_fixture(&candidate.candidate_id);

        let pass = evaluate_auto_pr(
            &proposal,
            &candidate,
            &eval,
            &LearningPolicy::default(),
            true,
        );
        assert!(pass.eligible, "{:?}", pass.reasons);

        eval.redaction_passed = false;
        let fail = evaluate_auto_pr(
            &proposal,
            &candidate,
            &eval,
            &LearningPolicy::default(),
            true,
        );
        assert!(!fail.eligible);
        assert!(
            fail.reasons
                .iter()
                .any(|reason| reason.contains("redaction or secrets scan failed"))
        );
    }

    #[test]
    fn evidence_score_requires_signal_run_verification_and_rollback() {
        let proposal = LearningProposal {
            version: 1,
            proposal_id: "prop-1".to_string(),
            created_at: Utc::now(),
            title: "Improve".to_string(),
            insights: vec!["ins-1".to_string()],
            stimulus: vec![LearningStimulus {
                signal_id: "sig-1".to_string(),
                run_id: "run-1".to_string(),
            }],
            hypothesis: "helps".to_string(),
            target: LearningProposalTarget {
                repo: "/Users/gdc/deadreckon".to_string(),
                scope: "cli".to_string(),
            },
            goal_text: "goal".to_string(),
            done_criteria: vec!["tests".to_string()],
            expected_risk: "low".to_string(),
            blocked_auto_pr_reasons: Vec::new(),
        };
        let candidate = LearningCandidate {
            version: 1,
            candidate_id: "cand-1".to_string(),
            proposal_id: "prop-1".to_string(),
            branch: "deadreckon/self/cand-1".to_string(),
            base_commit: "base".to_string(),
            head_commit: "head".to_string(),
            run_id: "run-2".to_string(),
            worktree: PathBuf::from("/tmp/cand"),
            diff: LearningCandidateDiff {
                files: 1,
                insertions: 10,
                deletions: 0,
                changed_files: vec!["crates/deadreckon/src/main.rs".to_string()],
            },
            risk: LearningRisk {
                class: "low".to_string(),
                reasons: Vec::new(),
            },
            status: "verified".to_string(),
            evidence_packet: "evidence.json".to_string(),
        };
        let eval = LearningEval {
            version: 1,
            candidate_id: "cand-1".to_string(),
            evaluated_at: Utc::now(),
            accepted_run: true,
            commands: vec![LearningEvalCommand {
                cmd: "cargo test".to_string(),
                status: 0,
            }],
            docs_updated: true,
            redaction_passed: true,
            evidence_score: 0.0,
            auto_pr: LearningAutoPrStatus {
                eligible: false,
                reasons: Vec::new(),
            },
        };

        assert!((evidence_score(&proposal, &candidate, &eval) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pr_gate_blocks_high_risk_gate_sandbox_credential_and_release_paths() {
        let risk = classify_candidate_risk(&[
            "crates/deadreckon-core/src/gate.rs".to_string(),
            "release/homebrew/patch-formula.mjs".to_string(),
        ]);

        assert_eq!(risk.class, "high");
        assert_eq!(risk.reasons.len(), 2);
    }

    #[test]
    fn pr_dry_run_writes_body_without_home_path() {
        let (_temp, paths) = temp_paths();
        let proposal = LearningProposal {
            version: 1,
            proposal_id: "prop-1".to_string(),
            created_at: Utc::now(),
            title: "Improve".to_string(),
            insights: vec!["ins-1".to_string()],
            stimulus: vec![LearningStimulus {
                signal_id: "sig-1".to_string(),
                run_id: "run-1".to_string(),
            }],
            hypothesis: "helps".to_string(),
            target: LearningProposalTarget {
                repo: "/Users/gdc/deadreckon".to_string(),
                scope: "cli".to_string(),
            },
            goal_text: "goal".to_string(),
            done_criteria: vec!["tests".to_string()],
            expected_risk: "low".to_string(),
            blocked_auto_pr_reasons: Vec::new(),
        };
        let candidate = LearningCandidate {
            version: 1,
            candidate_id: "cand-1".to_string(),
            proposal_id: "prop-1".to_string(),
            branch: "deadreckon/self/cand-1".to_string(),
            base_commit: "base".to_string(),
            head_commit: "head".to_string(),
            run_id: "run-2".to_string(),
            worktree: paths.learning_candidate_dir("cand-1").join("worktree"),
            diff: LearningCandidateDiff {
                files: 1,
                insertions: 1,
                deletions: 0,
                changed_files: vec!["crates/deadreckon/src/main.rs".to_string()],
            },
            risk: LearningRisk {
                class: "low".to_string(),
                reasons: Vec::new(),
            },
            status: "verified".to_string(),
            evidence_packet: "evidence.json".to_string(),
        };
        let eval = LearningEval {
            version: 1,
            candidate_id: "cand-1".to_string(),
            evaluated_at: Utc::now(),
            accepted_run: true,
            commands: vec![LearningEvalCommand {
                cmd: "cargo test -p deadreckon-core learning --lib".to_string(),
                status: 0,
            }],
            docs_updated: true,
            redaction_passed: true,
            evidence_score: 1.0,
            auto_pr: LearningAutoPrStatus {
                eligible: true,
                reasons: Vec::new(),
            },
        };

        let dry_run = prepare_pr_dry_run(
            &paths,
            &proposal,
            &candidate,
            &eval,
            &LearningPolicy::default(),
            true,
        )
        .expect("dry-run");

        assert!(dry_run.body.contains("## Summary"));
        assert!(dry_run.body.contains("## Rollback"));
        assert!(
            !dry_run
                .body
                .contains(paths.home().to_string_lossy().as_ref())
        );
        assert!(dry_run.body_path.exists());
    }
}
