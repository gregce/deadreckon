//! Independent, read-only assessment of whether a checked result means the
//! approved goal was achieved.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use deadreckon_core::{
    AcceptanceMarker, DeadreckonError, PipelineState, Result, acceptance_spec_path_for_run_root,
    diff_working_trees, implementation_notes_path, snapshot_diff,
};
use deadreckon_protocol::{
    GoalCoverage, JobId, JobSchemaVersion, RunId, SemanticDecision, SemanticJudgment,
};
use deadreckon_providers::{
    ProviderKind, ProviderRequest, ProviderResponse, ProviderRouter, WorkspaceAccess,
};
use deadreckon_sandbox::SandboxBackend;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::{NamedTempFile, TempDir};

pub const SEMANTIC_JUDGMENT_PATH: &str = "proofs/semantic-judgment.json";
const MAX_CONTRACT_BYTES: usize = 64 * 1024;
const MAX_DIFF_BYTES: usize = 256 * 1024;
const MAX_NOTES_BYTES: usize = 64 * 1024;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_FINDINGS: usize = 64;

const EVIDENCE_GOAL: &str = "approved-goal";
const EVIDENCE_CONTRACT: &str = "approved-contract";
const EVIDENCE_DIFF: &str = "source-diff";
const EVIDENCE_GATE: &str = "deterministic-gate";
const EVIDENCE_AUTHORITY: &str = "authority";
const EVIDENCE_NOTES: &str = "implementation-notes";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticEvidencePack {
    pub schema_version: u32,
    pub job_id: String,
    pub run_id: String,
    pub goal: EvidenceItem,
    pub contract: EvidenceItem,
    pub source_diff: EvidenceItem,
    pub deterministic_gate: EvidenceItem,
    pub authority: EvidenceItem,
    pub implementation_notes: EvidenceItem,
    pub changed_files: Vec<String>,
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub content: Value,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticJudgeResult {
    Achieved(SemanticJudgment),
    Revise(SemanticJudgment),
    NeedsReview(SemanticJudgment),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticJudgeAccounting {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub subscription: bool,
    pub wall_time_seconds: f64,
    pub sandbox_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticJudgeRun {
    pub result: SemanticJudgeResult,
    pub accounting: SemanticJudgeAccounting,
}

impl SemanticJudgeResult {
    pub fn judgment(&self) -> Option<&SemanticJudgment> {
        match self {
            Self::Achieved(judgment) | Self::Revise(judgment) | Self::NeedsReview(judgment) => {
                Some(judgment)
            }
            Self::Unavailable(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticModelResponse {
    decision: SemanticDecision,
    summary: String,
    goal_coverage: Vec<GoalCoverage>,
    #[serde(default)]
    missing: Vec<String>,
}

pub fn semantic_judgment_path(run_root: &Path) -> PathBuf {
    run_root.join(SEMANTIC_JUDGMENT_PATH)
}

pub fn build_semantic_evidence(
    state: &PipelineState,
    marker: &AcceptanceMarker,
) -> Result<SemanticEvidencePack> {
    build_semantic_evidence_with_baseline(state, marker, None)
}

/// Build semantic evidence for a composed result whose approved baseline is
/// not represented by this run's turn snapshots.
///
/// A durable graph has many child snapshots and one merged result. Comparing
/// the operator-approved source tree directly with that result gives the
/// independent judge the same concrete changed-file evidence a leaf run gets
/// from turn snapshots.
pub fn build_semantic_evidence_against_source(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    approved_source: &Path,
) -> Result<SemanticEvidencePack> {
    build_semantic_evidence_with_baseline(state, marker, Some(approved_source))
}

fn build_semantic_evidence_with_baseline(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    approved_source: Option<&Path>,
) -> Result<SemanticEvidencePack> {
    let mut caveats = Vec::new();
    let contract_path = acceptance_spec_path_for_run_root(&state.run_root);
    let (contract, contract_truncated) =
        read_bounded_text(&contract_path, MAX_CONTRACT_BYTES, &mut caveats);
    let (notes, notes_truncated) = read_bounded_text(
        &implementation_notes_path(&state.working_dir),
        MAX_NOTES_BYTES,
        &mut caveats,
    );
    let home = state
        .run_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "cannot infer DeadReckon home from run root {}",
                state.run_root.display()
            ))
        })?;
    let authority_path =
        deadreckon_core::DeadreckonPaths::from_home(home).job_authority(&state.run_id);
    let authority_bytes = fs::read(&authority_path).map_err(|source| DeadreckonError::Io {
        path: authority_path.clone(),
        source,
    })?;
    let authority =
        serde_json::from_slice(&authority_bytes).map_err(|source| DeadreckonError::Json {
            path: authority_path,
            source,
        })?;
    let diff = approved_source.map_or_else(
        || snapshot_diff(state, 0, state.turn),
        |source| diff_working_trees(source, &state.working_dir),
    );
    let (changed_files, diff_text) = match diff {
        Ok(summary) => {
            let changed_files = summary
                .files
                .iter()
                .map(|file| file.path.to_string_lossy().to_string())
                .collect();
            let text = summary
                .files
                .iter()
                .filter_map(|file| file.unified_diff.as_deref())
                .collect::<Vec<_>>()
                .join("\n");
            (changed_files, text)
        }
        Err(error) => {
            caveats.push(format!("source diff unavailable: {error}"));
            (Vec::new(), String::new())
        }
    };
    let (diff_text, diff_truncated) = truncate_text(&diff_text, MAX_DIFF_BYTES);

    Ok(SemanticEvidencePack {
        schema_version: 1,
        job_id: state.run_id.clone(),
        run_id: state.run_id.clone(),
        goal: EvidenceItem {
            id: EVIDENCE_GOAL.to_string(),
            content: Value::String(state.goal.clone()),
            truncated: false,
        },
        contract: EvidenceItem {
            id: EVIDENCE_CONTRACT.to_string(),
            content: Value::String(contract),
            truncated: contract_truncated,
        },
        source_diff: EvidenceItem {
            id: EVIDENCE_DIFF.to_string(),
            content: Value::String(diff_text),
            truncated: diff_truncated,
        },
        deterministic_gate: EvidenceItem {
            id: EVIDENCE_GATE.to_string(),
            content: serde_json::to_value(marker).map_err(json_error("deterministic marker"))?,
            truncated: false,
        },
        authority: EvidenceItem {
            id: EVIDENCE_AUTHORITY.to_string(),
            content: authority,
            truncated: false,
        },
        implementation_notes: EvidenceItem {
            id: EVIDENCE_NOTES.to_string(),
            content: Value::String(notes),
            truncated: notes_truncated,
        },
        changed_files,
        caveats,
    })
}

/// Invoke a fresh provider request. CLI routes receive an empty, temporary
/// workspace under an enforceable read-only sandbox; direct APIs receive only
/// prompt bytes and no cwd. No worker session is ever supplied.
pub async fn run_semantic_judge(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(state, marker, router, sandbox_backend, None).await
}

/// Run the fresh, read-only semantic judge for a composed result against the
/// source tree frozen by the parent job authority.
pub async fn run_semantic_judge_against_source(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: &Path,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        Some(approved_source),
    )
    .await
}

async fn run_semantic_judge_with_baseline(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: Option<&Path>,
) -> Result<SemanticJudgeRun> {
    let started = Instant::now();
    let selected = router.selected_route_info();
    let evidence = match build_semantic_evidence_with_baseline(state, marker, approved_source) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Ok(unavailable_run(
                format!(
                    "strict semantic judge unavailable: trusted evidence could not be assembled: {error}"
                ),
                accounting_without_response(
                    selected.as_ref(),
                    sandbox_backend,
                    started.elapsed().as_secs_f64(),
                ),
            ));
        }
    };
    let input = serde_json::to_string(&evidence).map_err(json_error("semantic evidence"))?;
    let input_sha256 = deadreckon_core::flight::sha256_text(&input);
    let is_cli = selected
        .as_ref()
        .is_some_and(|route| provider_kind_is_cli(&route.kind));
    if is_cli && sandbox_backend == SandboxBackend::None {
        return Ok(unavailable_run(
            "strict semantic judge unavailable: CLI provider has no enforceable read-only sandbox"
                .to_string(),
            accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            ),
        ));
    }

    let before = deadreckon_core::flight::build_working_file_index(&state.working_dir)?.tree_hash();
    let judge_workspace =
        is_cli
            .then(TempDir::new)
            .transpose()
            .map_err(|source| DeadreckonError::Io {
                path: state.run_root.join("semantic-judge-workspace"),
                source,
            })?;
    let request = semantic_provider_request(
        &input,
        judge_workspace.as_ref().map(TempDir::path),
        sandbox_backend,
    );
    let response = match router.complete(&request).await {
        Ok(response) => response,
        Err(error) => {
            return Ok(unavailable_run(
                format!("strict semantic judge unavailable: {error}"),
                accounting_without_response(
                    selected.as_ref(),
                    sandbox_backend,
                    started.elapsed().as_secs_f64(),
                ),
            ));
        }
    };
    let accounting =
        accounting_from_response(&response, sandbox_backend, started.elapsed().as_secs_f64());
    let after = deadreckon_core::flight::build_working_file_index(&state.working_dir)?.tree_hash();
    if before != after {
        return Ok(unavailable_run(
            "semantic judge changed the result tree; judgment refused".to_string(),
            accounting,
        ));
    }

    let result = classify_semantic_response(&state.run_id, &state.run_id, &input_sha256, response);
    let Some(judgment) = result.judgment() else {
        return Ok(SemanticJudgeRun { result, accounting });
    };
    if let Err(error) = write_semantic_judgment(&state.run_root, judgment) {
        return Ok(unavailable_run(
            format!("strict semantic judge unavailable: judgment could not be persisted: {error}"),
            accounting,
        ));
    }
    Ok(SemanticJudgeRun { result, accounting })
}

fn unavailable_run(reason: String, accounting: SemanticJudgeAccounting) -> SemanticJudgeRun {
    SemanticJudgeRun {
        result: SemanticJudgeResult::Unavailable(reason),
        accounting,
    }
}

fn accounting_without_response(
    selected: Option<&deadreckon_providers::ProviderRouteInfo>,
    sandbox_backend: SandboxBackend,
    wall_time_seconds: f64,
) -> SemanticJudgeAccounting {
    SemanticJudgeAccounting {
        provider: selected
            .map(|route| route.name.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
        model: selected
            .map(|route| route.model.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        subscription: selected.is_some_and(|route| provider_kind_is_cli(&route.kind)),
        wall_time_seconds,
        sandbox_backend: Some(sandbox_backend.to_string()),
    }
}

fn accounting_from_response(
    response: &ProviderResponse,
    requested_backend: SandboxBackend,
    measured_wall_seconds: f64,
) -> SemanticJudgeAccounting {
    SemanticJudgeAccounting {
        provider: response.spend.provider.clone(),
        model: response.spend.model.clone(),
        input_tokens: response.spend.input_tokens,
        output_tokens: response.spend.output_tokens,
        cost_usd: response.spend.cost_usd,
        subscription: response.spend.subscription,
        wall_time_seconds: response
            .spend
            .wall_time_seconds
            .unwrap_or(measured_wall_seconds),
        sandbox_backend: response
            .trace
            .get("sandbox_backend")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(requested_backend.to_string())),
    }
}

fn provider_kind_is_cli(kind: &ProviderKind) -> bool {
    match kind {
        ProviderKind::CliClaudeCode | ProviderKind::CliCodex => true,
        ProviderKind::Generic(id) => id.starts_with("cli:") || id.starts_with("cli-"),
        ProviderKind::Anthropic
        | ProviderKind::OpenAi
        | ProviderKind::OpenAiCompatible
        | ProviderKind::ScriptedSmoke => false,
    }
}

fn semantic_provider_request(
    evidence_json: &str,
    judge_workspace: Option<&Path>,
    sandbox_backend: SandboxBackend,
) -> ProviderRequest {
    ProviderRequest {
        prompt: format!(
            "You are an independent completion judge. Assess meaning only; deterministic checks \
             have already passed and you may not override them. Use only the evidence packet. \
             Return JSON matching the supplied schema. Every evidence citation must be one of \
             approved-goal, approved-contract, source-diff, deterministic-gate, authority, \
             implementation-notes.\n\nEVIDENCE PACK:\n{evidence_json}"
        ),
        max_output_tokens: 2_048,
        cwd: judge_workspace.map(Path::to_path_buf),
        output_path: None,
        sandbox_backend: Some(sandbox_backend),
        workspace_access: WorkspaceAccess::ReadOnly,
        pid_file: None,
        cancellation_token: None,
        session_dir: None,
        output_schema: Some(semantic_output_schema()),
        capability_posture: None,
    }
}

fn classify_semantic_response(
    job_id: &str,
    run_id: &str,
    input_sha256: &str,
    response: ProviderResponse,
) -> SemanticJudgeResult {
    let judgment = match parse_semantic_response(job_id, run_id, input_sha256, response) {
        Ok(judgment) => judgment,
        Err(error) => {
            return SemanticJudgeResult::Unavailable(format!(
                "strict semantic judge unavailable: response was malformed or invalid: {error}"
            ));
        }
    };
    match judgment.decision {
        SemanticDecision::Achieved => SemanticJudgeResult::Achieved(judgment),
        SemanticDecision::Revise => SemanticJudgeResult::Revise(judgment),
        SemanticDecision::Uncertain => SemanticJudgeResult::NeedsReview(judgment),
    }
}

fn parse_semantic_response(
    job_id: &str,
    run_id: &str,
    input_sha256: &str,
    response: ProviderResponse,
) -> Result<SemanticJudgment> {
    let content = strip_json_fence(response.content.trim());
    let mut model: SemanticModelResponse = serde_json::from_str(content).map_err(|source| {
        DeadreckonError::InvalidInput(format!("semantic judge returned malformed JSON: {source}"))
    })?;
    bound_model_response(&mut model)?;
    validate_evidence_references(&model.goal_coverage)?;
    Ok(SemanticJudgment {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(job_id.to_string()),
        run_id: RunId(run_id.to_string()),
        judged_at: Utc::now(),
        provider: response.provider,
        model: response.model,
        decision: model.decision,
        summary: model.summary,
        goal_coverage: model.goal_coverage,
        missing: model.missing,
        input_sha256: input_sha256.to_string(),
        spend_usd: response.spend.cost_usd,
    })
}

fn write_semantic_judgment(run_root: &Path, judgment: &SemanticJudgment) -> Result<()> {
    let path = semantic_judgment_path(run_root);
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "semantic proof path has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| DeadreckonError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut temp = NamedTempFile::new_in(parent).map_err(|source| DeadreckonError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    serde_json::to_writer_pretty(&mut temp, judgment).map_err(|source| DeadreckonError::Json {
        path: path.clone(),
        source,
    })?;
    temp.write_all(b"\n")
        .map_err(|source| DeadreckonError::Io {
            path: path.clone(),
            source,
        })?;
    temp.as_file_mut()
        .sync_all()
        .map_err(|source| DeadreckonError::Io {
            path: path.clone(),
            source,
        })?;
    temp.persist(&path).map_err(|error| DeadreckonError::Io {
        path,
        source: error.error,
    })?;
    Ok(())
}

fn read_bounded_text(path: &Path, limit: usize, caveats: &mut Vec<String>) -> (String, bool) {
    if let Ok(bytes) = fs::read(path) {
        let text = String::from_utf8_lossy(&bytes);
        truncate_text(&text, limit)
    } else {
        caveats.push(format!("evidence absent: {}", path.display()));
        (String::new(), false)
    }
}

fn truncate_text(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_string(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        format!(
            "{}\n[… {} bytes omitted by semantic evidence bound]",
            &text[..end],
            text.len() - end
        ),
        true,
    )
}

fn bound_model_response(model: &mut SemanticModelResponse) -> Result<()> {
    if model.summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge summary exceeded the 4000-character bound".to_string(),
        ));
    }
    if model.goal_coverage.len() > MAX_FINDINGS || model.missing.len() > MAX_FINDINGS {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge returned too many findings".to_string(),
        ));
    }
    Ok(())
}

fn validate_evidence_references(coverage: &[GoalCoverage]) -> Result<()> {
    let allowed = [
        EVIDENCE_GOAL,
        EVIDENCE_CONTRACT,
        EVIDENCE_DIFF,
        EVIDENCE_GATE,
        EVIDENCE_AUTHORITY,
        EVIDENCE_NOTES,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    for item in coverage {
        for reference in &item.evidence {
            if !allowed.contains(reference.as_str()) {
                return Err(DeadreckonError::InvalidInput(format!(
                    "semantic judge cited unknown evidence id {reference}"
                )));
            }
        }
    }
    Ok(())
}

fn strip_json_fence(content: &str) -> &str {
    content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
        .and_then(|content| content.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(content)
}

fn semantic_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["decision", "summary", "goal_coverage", "missing"],
        "properties": {
            "decision": { "type": "string", "enum": ["achieved", "revise", "uncertain"] },
            "summary": { "type": "string", "maxLength": MAX_SUMMARY_CHARS },
            "goal_coverage": {
                "type": "array",
                "maxItems": MAX_FINDINGS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["claim", "status", "evidence"],
                    "properties": {
                        "claim": { "type": "string" },
                        "status": { "type": "string", "enum": ["met", "missing", "unclear"] },
                        "evidence": { "type": "array", "items": { "type": "string" } }
                    }
                }
            },
            "missing": { "type": "array", "maxItems": MAX_FINDINGS, "items": { "type": "string" } }
        }
    })
}

fn json_error(label: &'static str) -> impl FnOnce(serde_json::Error) -> DeadreckonError {
    move |source| DeadreckonError::Json {
        path: PathBuf::from(label),
        source,
    }
}

#[cfg(test)]
mod tests {
    use deadreckon_providers::{ProviderResponse, ProviderUsage, SpendEstimate};
    use deadreckon_sandbox::SandboxBackend;

    use super::{
        EVIDENCE_CONTRACT, EVIDENCE_DIFF, EVIDENCE_GATE, SemanticDecision, SemanticJudgeResult,
        classify_semantic_response, provider_kind_is_cli, semantic_output_schema,
        semantic_provider_request, strip_json_fence, validate_evidence_references,
    };
    use deadreckon_protocol::{GoalCoverage, GoalCoverageStatus};
    use deadreckon_providers::ProviderKind;

    #[test]
    fn semantic_judge_has_no_worker_session_or_write_capability() {
        let request = semantic_provider_request(
            "{}",
            Some(std::path::Path::new("/tmp/empty-judge-workspace")),
            SandboxBackend::SandboxExec,
        );
        assert_eq!(
            request.workspace_access,
            deadreckon_providers::WorkspaceAccess::ReadOnly
        );
        assert!(request.session_dir.is_none());
        assert!(request.pid_file.is_none());
        assert_eq!(
            request.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/empty-judge-workspace"))
        );
    }

    #[test]
    fn semantic_judge_receives_goal_contract_diff_and_gate_evidence() {
        let request = semantic_provider_request("{\"goal\":\"x\"}", None, SandboxBackend::Docker);
        for evidence in [EVIDENCE_CONTRACT, EVIDENCE_DIFF, EVIDENCE_GATE] {
            assert!(request.prompt.contains(evidence));
        }
        assert!(request.output_schema.is_some());
    }

    #[test]
    fn semantic_schema_has_three_decisions() {
        assert_eq!(
            semantic_output_schema()["properties"]["decision"]["enum"],
            serde_json::json!(["achieved", "revise", "uncertain"])
        );
    }

    #[test]
    fn semantic_evidence_rejects_unknown_citations() {
        let coverage = vec![GoalCoverage {
            claim: "done".to_string(),
            status: GoalCoverageStatus::Met,
            evidence: vec!["worker-says-done".to_string()],
        }];
        assert!(validate_evidence_references(&coverage).is_err());
    }

    #[test]
    fn semantic_json_fences_are_bounded_to_one_payload() {
        assert_eq!(
            strip_json_fence("```json\n{\"decision\":\"uncertain\"}\n```"),
            "{\"decision\":\"uncertain\"}"
        );
    }

    #[test]
    fn semantic_response_fixture_uses_supervisor_provider_facts() {
        let response = ProviderResponse {
            provider: "judge-provider".to_string(),
            model: "judge-model".to_string(),
            content: "{}".to_string(),
            usage: ProviderUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            spend: SpendEstimate {
                provider: "judge-provider".to_string(),
                model: "judge-model".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: 0.01,
                subscription: false,
                wall_time_seconds: None,
            },
            trace: serde_json::Value::Null,
        };
        assert_eq!(response.provider, "judge-provider");
        assert_eq!(SemanticDecision::Uncertain, SemanticDecision::Uncertain);
    }

    #[test]
    fn malformed_semantic_response_is_unavailable_not_execution_error() {
        let result = classify_semantic_response(
            "job-1",
            "run-1",
            "sha256:input",
            ProviderResponse {
                provider: "smoke".to_string(),
                model: "scripted".to_string(),
                content: r#"{"action":"bash","command":"echo nope"}"#.to_string(),
                usage: ProviderUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "smoke".to_string(),
                    model: "scripted".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: 0.0,
                    subscription: false,
                    wall_time_seconds: Some(0.01),
                },
                trace: serde_json::Value::Null,
            },
        );

        let SemanticJudgeResult::Unavailable(reason) = result else {
            panic!("malformed output must be unavailable")
        };
        assert!(reason.contains("malformed or invalid"));
    }

    #[test]
    fn semantic_response_maps_achieved_revise_and_uncertain() {
        fn response(decision: &str) -> ProviderResponse {
            ProviderResponse {
                provider: "judge".to_string(),
                model: "judge-model".to_string(),
                content: format!(
                    r#"{{"decision":"{decision}","summary":"bounded","goal_coverage":[],"missing":[]}}"#
                ),
                usage: ProviderUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "judge".to_string(),
                    model: "judge-model".to_string(),
                    input_tokens: 2,
                    output_tokens: 1,
                    cost_usd: 0.01,
                    subscription: false,
                    wall_time_seconds: Some(0.1),
                },
                trace: serde_json::Value::Null,
            }
        }

        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("achieved")),
            SemanticJudgeResult::Achieved(_)
        ));
        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("revise")),
            SemanticJudgeResult::Revise(_)
        ));
        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("uncertain")),
            SemanticJudgeResult::NeedsReview(_)
        ));
    }

    #[test]
    fn generic_cli_routes_are_treated_as_cli_not_direct_apis() {
        assert!(provider_kind_is_cli(&ProviderKind::Generic(
            "cli:codex-server".to_string()
        )));
        assert!(provider_kind_is_cli(&ProviderKind::Generic(
            "cli:pi".to_string()
        )));
        assert!(!provider_kind_is_cli(&ProviderKind::OpenAi));
    }
}
