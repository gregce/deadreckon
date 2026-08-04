use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_providers::{
    ProviderCleanup, ProviderKind, ProviderPhaseDeadline, ProviderPhaseOutcome, ProviderRequest,
    ProviderResponse, ProviderRouter, complete_provider_phase,
};
use deadreckon_sandbox::SandboxBackend;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::IoContext;
use deadreckon_core::codebase::{
    read_run_codebase_record, write_codebase_record, write_trusted_codebase_record,
};
use deadreckon_core::docs::{
    AS_BUILT_DELTA, RUN_AS_BUILT, RUN_DECISIONS, RUN_NARRATIVE, append_docs_warning, as_built_path,
    changed_doc_files, decisions_path, delta_path, diff_samples_markdown, docs_dir,
    implementation_notes_path, is_documentable_path, missing_files_in_narrative, narrative_path,
    polish_path, publish_docs_for_promotion, publish_docs_for_promotion_uncommitted,
    read_turn_records, rewrite_templated_docs, source_layout, tool_stdio_markdown,
};
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_core::paths::source_root;
use deadreckon_core::polish_subcalls::{
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, PolishDiffCoverage, PolishSubcallRecord,
};
use deadreckon_core::state::PipelineState;

const DOC_POLISH_CUMULATIVE_WALL_SECONDS: u64 = 30 * 60;
const DOC_POLISH_CLI_CALL_WALL_SECONDS: u64 = 5 * 60;
const DOC_POLISH_HTTP_CALL_WALL_SECONDS: u64 = 2 * 60;
const DOC_POLISH_CLEANUP_GRACE_SECONDS: u64 = 30;

#[derive(Debug, Clone)]
pub struct PolishConfig {
    pub home: PathBuf,
    pub doc_skill: String,
    pub doc_provider: Option<String>,
    pub doc_provider_source: Option<String>,
    pub doc_subskills: Vec<String>,
    pub token_budget: u32,
    pub budget_cap_usd: Option<f64>,
    pub sandbox_backend: SandboxBackend,
    /// Standalone `doc --polish` keeps the historical immediate commit. A
    /// durable run sets this false and commits through the trusted turn
    /// sanitizer after all provider-backed documentation work has finished.
    pub commit_docs: bool,
    pub no_llm: bool,
    pub force: bool,
    /// Optional outer wall budget remaining when polish begins. `None` uses
    /// the bounded 30-minute polish default; a durable run passes its smaller
    /// remaining Job budget here.
    pub max_wall_seconds: Option<f64>,
    /// Durable runs pass the exact outer Job boundary. This disables the
    /// standalone polish ceilings and prevents documentation subcalls from
    /// rebuilding relative timeouts from a rounded remaining duration.
    pub phase_deadline: Option<ProviderPhaseDeadline>,
    /// Durable controller cancellation. Every provider attempt gets its own
    /// child token so a phase timeout can stop just that process tree while an
    /// outer cancellation can stop the complete polish pass.
    pub cancellation_token: Option<CancellationToken>,
}

struct PolishWallBudget {
    work_expires_at: tokio::time::Instant,
    cleanup_budget: Duration,
    standalone_call_ceilings: bool,
}

impl PolishWallBudget {
    fn new(config: &PolishConfig) -> Self {
        if let Some(deadline) = config.phase_deadline {
            return Self {
                work_expires_at: deadline.work_expires_at,
                cleanup_budget: deadline.cleanup_budget,
                standalone_call_ceilings: false,
            };
        }
        let requested = config
            .max_wall_seconds
            .filter(|seconds| seconds.is_finite())
            .map(|seconds| seconds.max(0.0))
            .unwrap_or(DOC_POLISH_CUMULATIVE_WALL_SECONDS as f64);
        let cumulative =
            Duration::from_secs_f64(requested.min(DOC_POLISH_CUMULATIVE_WALL_SECONDS as f64));
        Self {
            work_expires_at: tokio::time::Instant::now() + cumulative,
            cleanup_budget: Duration::from_secs(DOC_POLISH_CLEANUP_GRACE_SECONDS),
            standalone_call_ceilings: true,
        }
    }

    fn allocate(&self, router: &ProviderRouter) -> Option<ProviderPhaseDeadline> {
        let now = tokio::time::Instant::now();
        if now >= self.work_expires_at {
            return None;
        }
        let work_expires_at = if self.standalone_call_ceilings {
            self.work_expires_at.min(now + polish_call_ceiling(router))
        } else {
            self.work_expires_at
        };
        Some(ProviderPhaseDeadline::new(
            work_expires_at,
            self.cleanup_budget,
        ))
    }
}

enum PolishProviderCompletion {
    Finished(deadreckon_providers::Result<Box<ProviderResponse>>),
    WallExpired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub path: PathBuf,
    pub source: SkillSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Project,
    User,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolishedDocs {
    pub narrative: String,
    pub as_built: String,
    pub decisions: String,
    #[serde(default)]
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishRecord {
    #[serde(default = "default_polish_schema_version")]
    pub schema_version: u32,
    pub status: String,
    pub inputs_hash: String,
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_provider_source: Option<String>,
    pub skill_path: Option<PathBuf>,
    pub skill_source: Option<String>,
    pub completed_at: String,
    pub cost_usd: f64,
    pub retries: u32,
    pub missing_files: Vec<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub subcalls: Vec<PolishSubcallRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_coverage: Option<PolishDiffCoverage>,
}

fn default_polish_schema_version() -> u32 {
    1
}

pub async fn polish_run_docs(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
) -> Result<()> {
    let mut wall_budget = PolishWallBudget::new(config);
    rewrite_templated_docs(state, &doc_writer_label(config))?;
    if config.no_llm {
        let hash = inputs_hash(state)?;
        write_polish_record(
            state,
            PolishRecord {
                schema_version: 2,
                status: "incremental".to_string(),
                inputs_hash: hash,
                provider: config.doc_provider.clone(),
                doc_provider_source: config.doc_provider_source.clone(),
                skill_path: None,
                skill_source: None,
                completed_at: Utc::now().to_rfc3339(),
                cost_usd: 0.0,
                retries: 0,
                missing_files: Vec::new(),
                error: None,
                subcalls: Vec::new(),
                merged_at: None,
                diff_coverage: None,
            },
        )?;
        publish_docs(state, config)?;
        return Ok(());
    }

    let hash = inputs_hash(state)?;
    if !config.force
        && let Some(existing) = read_polish_record(state)?
        && existing.status == "polished"
        && existing.inputs_hash == hash
    {
        publish_docs(state, config)?;
        return Ok(());
    }

    let requested_subskills = doc_subskills(config);
    if should_use_split_polish(config) {
        match resolve_doc_subskills(&requested_subskills, state, &config.home) {
            Ok(resolved) => {
                return polish_run_docs_split(
                    state,
                    router,
                    config,
                    &hash,
                    resolved,
                    &mut wall_budget,
                )
                .await;
            }
            Err(err) if !config.doc_subskills.is_empty() => {
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 2,
                        status: "no_skill".to_string(),
                        inputs_hash: hash,
                        provider: config.doc_provider.clone(),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: None,
                        skill_source: None,
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: 0.0,
                        retries: 0,
                        missing_files: Vec::new(),
                        error: Some(err.to_string()),
                        subcalls: Vec::new(),
                        merged_at: None,
                        diff_coverage: None,
                    },
                )?;
                publish_docs(state, config)?;
                return Ok(());
            }
            Err(_) => {
                // Fall through to the legacy single-skill path for older custom installs.
            }
        }
    }

    polish_run_docs_legacy(state, router, config, &hash, &mut wall_budget).await
}

fn publish_docs(state: &PipelineState, config: &PolishConfig) -> Result<()> {
    if config.commit_docs {
        publish_docs_for_promotion(state)
    } else {
        publish_docs_for_promotion_uncommitted(state)
    }
}

async fn polish_run_docs_legacy(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
    hash: &str,
    wall_budget: &mut PolishWallBudget,
) -> Result<()> {
    let resolved = match resolve_skill(&config.doc_skill, state, &config.home) {
        Ok(skill) => skill,
        Err(err) => {
            write_polish_record(
                state,
                PolishRecord {
                    schema_version: 1,
                    status: "no_skill".to_string(),
                    inputs_hash: hash.to_string(),
                    provider: config.doc_provider.clone(),
                    doc_provider_source: config.doc_provider_source.clone(),
                    skill_path: None,
                    skill_source: None,
                    completed_at: Utc::now().to_rfc3339(),
                    cost_usd: 0.0,
                    retries: 0,
                    missing_files: Vec::new(),
                    error: Some(err.to_string()),
                    subcalls: Vec::new(),
                    merged_at: None,
                    diff_coverage: None,
                },
            )?;
            publish_docs(state, config)?;
            return Ok(());
        }
    };

    let provider_workspace = tempfile::tempdir().map_err(|source| DeadreckonError::Io {
        path: state.run_root.join("docs-polish-workspace"),
        source,
    })?;
    let mut retries = 0;
    let mut prompt_suffix = String::new();
    let mut total_cost = 0.0;
    loop {
        let prompt = polish_prompt(state, &resolved.path, &prompt_suffix)?;
        let response = match run_polish_provider_call(
            state,
            router,
            config,
            wall_budget,
            "legacy",
            polish_provider_request(
                prompt,
                20_000,
                provider_workspace.path(),
                state
                    .run_root
                    .join("turns")
                    .join("docs-polish")
                    .join("provider.out"),
                config.sandbox_backend,
            ),
        )
        .await?
        {
            PolishProviderCompletion::Finished(Ok(response)) => response,
            PolishProviderCompletion::Finished(Err(err)) => {
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 1,
                        status: "provider_error".to_string(),
                        inputs_hash: hash.to_string(),
                        provider: config.doc_provider.clone(),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(err.to_string()),
                        subcalls: Vec::new(),
                        merged_at: None,
                        diff_coverage: None,
                    },
                )?;
                publish_docs(state, config)?;
                return Ok(());
            }
            PolishProviderCompletion::WallExpired => {
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 1,
                        status: "wall_timeout".to_string(),
                        inputs_hash: hash.to_string(),
                        provider: config.doc_provider.clone(),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(
                            "documentation provider exceeded the bounded polish wall budget"
                                .to_string(),
                        ),
                        subcalls: Vec::new(),
                        merged_at: None,
                        diff_coverage: None,
                    },
                )?;
                publish_docs(state, config)?;
                return Ok(());
            }
            PolishProviderCompletion::Cancelled => {
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 1,
                        status: "cancelled".to_string(),
                        inputs_hash: hash.to_string(),
                        provider: config.doc_provider.clone(),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(
                            "documentation polish cancelled by the durable run controller"
                                .to_string(),
                        ),
                        subcalls: Vec::new(),
                        merged_at: None,
                        diff_coverage: None,
                    },
                )?;
                publish_docs(state, config)?;
                return Ok(());
            }
        };
        total_cost += response.spend.cost_usd;
        match serde_json::from_str::<PolishedDocs>(&response.content) {
            Ok(docs) => {
                write_polished_docs(state, &docs)?;
                let missing = missing_files_in_narrative(state)?;
                if !missing.is_empty() && retries < 2 {
                    retries += 1;
                    prompt_suffix = format!(
                        "\nYour previous output omitted these documentable files; revise to include them with citations: {}",
                        missing.join(", ")
                    );
                    continue;
                }
                if !missing.is_empty() {
                    append_docs_warning(
                        state,
                        &format!(
                            "polished narrative still omitted files after retries: {}",
                            missing.join(", ")
                        ),
                    )?;
                }
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 1,
                        status: "polished".to_string(),
                        inputs_hash: hash.to_string(),
                        provider: Some(response.provider),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: missing,
                        error: None,
                        subcalls: Vec::new(),
                        merged_at: Some(Utc::now().to_rfc3339()),
                        diff_coverage: Some(PolishDiffCoverage {
                            changed_files: changed_doc_files(state)?.len(),
                            missing_files: missing_files_in_narrative(state)?,
                            retries,
                        }),
                    },
                )?;
                write_hash_to_codebase(state, hash)?;
                publish_docs(state, config)?;
                return Ok(());
            }
            Err(err) => {
                let error_message = err.to_string();
                if retries == 0 {
                    retries += 1;
                    prompt_suffix =
                        "\nYour last reply was not valid JSON. Reproduce the JSON exactly."
                            .to_string();
                    continue;
                }
                write_polish_record(
                    state,
                    PolishRecord {
                        schema_version: 1,
                        status: "json_parse".to_string(),
                        inputs_hash: hash.to_string(),
                        provider: Some(response.provider),
                        doc_provider_source: config.doc_provider_source.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(error_message),
                        subcalls: Vec::new(),
                        merged_at: None,
                        diff_coverage: None,
                    },
                )?;
                publish_docs(state, config)?;
                return Ok(());
            }
        }
    }
}

type ResolvedDocSubskill = (String, ResolvedSkill);

struct SubcallResult {
    name: String,
    output: Option<Value>,
    record: PolishSubcallRecord,
}

async fn polish_run_docs_split(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
    hash: &str,
    resolved: Vec<ResolvedDocSubskill>,
    wall_budget: &mut PolishWallBudget,
) -> Result<()> {
    let mut outputs = BTreeMap::new();
    let mut subcalls = Vec::new();
    let mut total_cost = 0.0;
    let mut first_failed_skill = None;

    for (name, skill) in &resolved {
        let result =
            run_polish_subcall(state, router, config, wall_budget, name, skill, "").await?;
        total_cost += result.record.cost_usd;
        if result.record.status != "ok" && first_failed_skill.is_none() {
            first_failed_skill = Some(name.clone());
        }
        if let Some(output) = result.output {
            outputs.insert(result.name.clone(), output);
        }
        subcalls.push(result.record);
        if config
            .budget_cap_usd
            .is_some_and(|cap| total_cost > cap && first_failed_skill.is_none())
        {
            first_failed_skill = Some("budget_cap".to_string());
            break;
        }
    }

    let mut docs = merge_split_docs(state, &outputs)?;
    write_polished_docs(state, &docs)?;

    let mut coverage_retries = 0;
    let mut missing = missing_files_in_narrative(state)?;
    while !missing.is_empty() && coverage_retries < 2 {
        let Some((_, phases_skill)) = resolved
            .iter()
            .find(|(name, _)| name == "narrator-phases")
            .cloned()
        else {
            break;
        };
        coverage_retries += 1;
        let suffix = format!(
            "\nYour previous phase output omitted these documentable changed files; revise only the phases JSON so each one is cited by path: {}",
            missing.join(", ")
        );
        let result = run_polish_subcall(
            state,
            router,
            config,
            wall_budget,
            "narrator-phases",
            &phases_skill,
            &suffix,
        )
        .await?;
        if result.record.status != "ok" && first_failed_skill.is_none() {
            first_failed_skill = Some("narrator-phases".to_string());
        }
        if let Some(output) = result.output {
            outputs.insert(result.name.clone(), output);
        }
        subcalls.push(result.record);
        docs = merge_split_docs(state, &outputs)?;
        write_polished_docs(state, &docs)?;
        missing = missing_files_in_narrative(state)?;
    }

    if !missing.is_empty() {
        append_docs_warning(
            state,
            &format!(
                "polished narrative still omitted files after retries: {}",
                missing.join(", ")
            ),
        )?;
    }

    let status = first_failed_skill
        .as_deref()
        .map(|name| format!("failed_subcall:{name}"))
        .unwrap_or_else(|| "polished".to_string());
    let provider = subcalls
        .iter()
        .find_map(|subcall| subcall.provider.clone())
        .or_else(|| config.doc_provider.clone());
    let coverage = PolishDiffCoverage {
        changed_files: changed_doc_files(state)?.len(),
        missing_files: missing.clone(),
        retries: coverage_retries,
    };
    write_polish_record(
        state,
        PolishRecord {
            schema_version: 2,
            status: status.clone(),
            inputs_hash: hash.to_string(),
            provider,
            doc_provider_source: config.doc_provider_source.clone(),
            skill_path: None,
            skill_source: None,
            completed_at: Utc::now().to_rfc3339(),
            cost_usd: subcalls.iter().map(|subcall| subcall.cost_usd).sum(),
            retries: coverage_retries,
            missing_files: missing,
            error: first_failed_skill
                .as_ref()
                .map(|name| format!("subcall {name} did not complete cleanly")),
            subcalls,
            merged_at: Some(Utc::now().to_rfc3339()),
            diff_coverage: Some(coverage),
        },
    )?;
    if status == "polished" {
        write_hash_to_codebase(state, hash)?;
    }
    publish_docs(state, config)?;
    Ok(())
}

async fn run_polish_subcall(
    state: &PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
    wall_budget: &mut PolishWallBudget,
    name: &str,
    skill: &ResolvedSkill,
    suffix: &str,
) -> Result<SubcallResult> {
    let provider_workspace = match tempfile::tempdir() {
        Ok(workspace) => workspace,
        Err(error) => {
            return Ok(failed_subcall(
                name,
                skill,
                "workspace_error",
                0,
                None,
                error.to_string(),
            ));
        }
    };
    let mut retries = 0;
    let mut prompt_suffix = suffix.to_string();
    loop {
        let started = Instant::now();
        let prompt = match polish_prompt(state, &skill.path, &prompt_suffix) {
            Ok(prompt) => prompt,
            Err(err) => {
                return Ok(failed_subcall(
                    name,
                    skill,
                    "prompt_error",
                    retries,
                    Some(started.elapsed().as_millis()),
                    err.to_string(),
                ));
            }
        };
        let response = run_polish_provider_call(
            state,
            router,
            config,
            wall_budget,
            name,
            polish_provider_request(
                prompt,
                polish_token_budget(config),
                provider_workspace.path(),
                state
                    .run_root
                    .join("turns")
                    .join("docs-polish")
                    .join(name)
                    .join("provider.out"),
                config.sandbox_backend,
            ),
        )
        .await?;
        let duration_ms = Some(started.elapsed().as_millis());
        let response = match response {
            PolishProviderCompletion::Finished(Ok(response)) => response,
            PolishProviderCompletion::Finished(Err(err)) => {
                return Ok(failed_subcall(
                    name,
                    skill,
                    "provider_error",
                    retries,
                    duration_ms,
                    err.to_string(),
                ));
            }
            PolishProviderCompletion::WallExpired => {
                return Ok(failed_subcall(
                    name,
                    skill,
                    "wall_timeout",
                    retries,
                    duration_ms,
                    "documentation provider exceeded the bounded polish wall budget".to_string(),
                ));
            }
            PolishProviderCompletion::Cancelled => {
                return Ok(failed_subcall(
                    name,
                    skill,
                    "cancelled",
                    retries,
                    duration_ms,
                    "documentation polish cancelled by the durable run controller".to_string(),
                ));
            }
        };
        match serde_json::from_str::<Value>(&response.content) {
            Ok(value) => {
                return Ok(SubcallResult {
                    name: name.to_string(),
                    output: Some(value),
                    record: PolishSubcallRecord {
                        skill: name.to_string(),
                        status: "ok".to_string(),
                        provider: Some(response.provider),
                        skill_path: Some(skill.path.clone()),
                        skill_source: Some(skill_source_label(&skill.source).to_string()),
                        tokens_in: response.usage.input_tokens,
                        tokens_out: response.usage.output_tokens,
                        cost_usd: response.spend.cost_usd,
                        retries,
                        duration_ms,
                        error: None,
                    },
                });
            }
            Err(err) if retries == 0 => {
                retries += 1;
                prompt_suffix = format!(
                    "{}\nYour last reply for `{name}` was not valid JSON: {err}. Reproduce only the requested JSON object.",
                    suffix
                );
            }
            Err(err) => {
                return Ok(SubcallResult {
                    name: name.to_string(),
                    output: None,
                    record: PolishSubcallRecord {
                        skill: name.to_string(),
                        status: "json_parse".to_string(),
                        provider: Some(response.provider),
                        skill_path: Some(skill.path.clone()),
                        skill_source: Some(skill_source_label(&skill.source).to_string()),
                        tokens_in: response.usage.input_tokens,
                        tokens_out: response.usage.output_tokens,
                        cost_usd: response.spend.cost_usd,
                        retries,
                        duration_ms,
                        error: Some(err.to_string()),
                    },
                });
            }
        }
    }
}

fn failed_subcall(
    name: &str,
    skill: &ResolvedSkill,
    status: &str,
    retries: u32,
    duration_ms: Option<u128>,
    error: String,
) -> SubcallResult {
    SubcallResult {
        name: name.to_string(),
        output: None,
        record: PolishSubcallRecord {
            skill: name.to_string(),
            status: status.to_string(),
            provider: None,
            skill_path: Some(skill.path.clone()),
            skill_source: Some(skill_source_label(&skill.source).to_string()),
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            retries,
            duration_ms,
            error: Some(error),
        },
    }
}

async fn run_polish_provider_call(
    state: &PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
    wall_budget: &PolishWallBudget,
    call_label: &str,
    mut request: ProviderRequest,
) -> Result<PolishProviderCompletion> {
    if config
        .cancellation_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Ok(PolishProviderCompletion::Cancelled);
    }
    let Some(phase_deadline) = wall_budget.allocate(router) else {
        return Ok(PolishProviderCompletion::WallExpired);
    };
    let process_dir = state.run_root.join("child-pids");
    fs::create_dir_all(&process_dir).with_path(&process_dir)?;
    let safe_label = call_label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let pid_file = process_dir.join(format!("docs-polish-{safe_label}-{}.pid", Uuid::new_v4()));
    request.pid_file = Some(pid_file.clone());
    request.cancellation_token = config.cancellation_token.clone();
    match complete_provider_phase(router, &mut request, phase_deadline).await {
        ProviderPhaseOutcome::Completed(result) => {
            Ok(PolishProviderCompletion::Finished(result.map(Box::new)))
        }
        ProviderPhaseOutcome::WorkExpired { cleanup } => {
            require_polish_provider_cleanup(&pid_file, &cleanup)?;
            Ok(PolishProviderCompletion::WallExpired)
        }
        ProviderPhaseOutcome::Cancelled { cleanup } => {
            require_polish_provider_cleanup(&pid_file, &cleanup)?;
            Ok(PolishProviderCompletion::Cancelled)
        }
    }
}

fn require_polish_provider_cleanup(pid_file: &Path, cleanup: &ProviderCleanup) -> Result<()> {
    match cleanup {
        ProviderCleanup::Proven | ProviderCleanup::NotApplicable => Ok(()),
        ProviderCleanup::RetainedAuthority { path, detail } => {
            Err(DeadreckonError::InvalidInput(format!(
                "LOST_CONTAINMENT: documentation provider retained process authority at {} (expected {}): {detail}",
                path.display(),
                pid_file.display()
            )))
        }
    }
}

fn polish_call_ceiling(router: &ProviderRouter) -> Duration {
    let selected = router.selected_route_info();
    polish_call_ceiling_for_kind(selected.as_ref().map(|route| &route.kind))
}

fn polish_call_ceiling_for_kind(kind: Option<&ProviderKind>) -> Duration {
    let cli = kind.is_some_and(|kind| {
        matches!(kind, ProviderKind::CliClaudeCode | ProviderKind::CliCodex)
            || matches!(kind, ProviderKind::Generic(id) if id.starts_with("cli:") || id.starts_with("cli-"))
    });
    Duration::from_secs(if cli {
        DOC_POLISH_CLI_CALL_WALL_SECONDS
    } else {
        DOC_POLISH_HTTP_CALL_WALL_SECONDS
    })
}

fn polish_provider_request(
    prompt: String,
    max_output_tokens: u32,
    isolated_workspace: &Path,
    output_path: PathBuf,
    sandbox_backend: SandboxBackend,
) -> ProviderRequest {
    ProviderRequest {
        prompt,
        max_output_tokens,
        cwd: Some(isolated_workspace.to_path_buf()),
        output_path: Some(output_path),
        sandbox_backend: (sandbox_backend != SandboxBackend::None).then_some(sandbox_backend),
        workspace_access: deadreckon_sandbox::WorkspaceAccess::ReadOnly,
        pid_file: None,
        cancellation_token: None,
        session_dir: None,
        output_schema: None,
        capability_posture: None,
    }
}

fn resolve_doc_subskills(
    names: &[String],
    state: &PipelineState,
    home: &Path,
) -> Result<Vec<ResolvedDocSubskill>> {
    names
        .iter()
        .map(|name| resolve_skill(name, state, home).map(|skill| (name.clone(), skill)))
        .collect()
}

fn doc_subskills(config: &PolishConfig) -> Vec<String> {
    if config.doc_subskills.is_empty() {
        DEFAULT_DOC_SUBSKILLS
            .iter()
            .map(|skill| (*skill).to_string())
            .collect()
    } else {
        config.doc_subskills.clone()
    }
}

fn should_use_split_polish(config: &PolishConfig) -> bool {
    !config.doc_subskills.is_empty()
}

fn polish_token_budget(config: &PolishConfig) -> u32 {
    if config.token_budget == 0 {
        DEFAULT_DOC_POLISH_TOKEN_BUDGET
    } else {
        config.token_budget
    }
}

fn doc_writer_label(config: &PolishConfig) -> String {
    if config.no_llm {
        return "templated only".to_string();
    }
    let provider = config
        .doc_provider
        .as_deref()
        .unwrap_or("unconfigured doc provider");
    if should_use_split_polish(config) {
        format!("{} via {}", provider, doc_subskills(config).join(", "))
    } else {
        provider.to_string()
    }
}

fn merge_split_docs(
    state: &PipelineState,
    outputs: &BTreeMap<String, Value>,
) -> Result<PolishedDocs> {
    let fallback = templated_docs_json(state);
    Ok(PolishedDocs {
        narrative: render_split_narrative(
            state,
            outputs.get("narrator-overview"),
            outputs.get("narrator-phases"),
            &fallback.narrative,
        ),
        as_built: render_split_as_built(
            state,
            outputs.get("narrator-as-built"),
            &fallback.as_built,
        )?,
        decisions: render_split_decisions(outputs.get("narrator-decisions"), &fallback.decisions),
        delta: fallback.delta,
    })
}

fn render_split_narrative(
    state: &PipelineState,
    overview: Option<&Value>,
    phases: Option<&Value>,
    fallback: &str,
) -> String {
    if let Some(narrative) = overview.and_then(|value| string_field(value, "narrative")) {
        return narrative;
    }
    let mut out = frontmatter_prefix(fallback);
    if let Some(reading_order) = overview.and_then(|value| string_field(value, "reading_order"))
        && !reading_order.trim().is_empty()
    {
        out.push_str("## Reading order\n\n");
        out.push_str(reading_order.trim());
        out.push_str("\n\n");
    }
    out.push_str("## Goal\n\n");
    out.push_str(&state.goal);
    out.push_str("\n\n## Why now\n\n");
    out.push_str(
        &overview
            .and_then(|value| string_field(value, "why_now"))
            .unwrap_or_else(|| fallback_section(fallback, "## Why now", "## High-level approach")),
    );
    out.push_str("\n\n## High-level approach\n\n");
    out.push_str(
        &overview
            .and_then(|value| string_field(value, "high_level_approach"))
            .unwrap_or_else(|| {
                fallback_section(
                    fallback,
                    "## High-level approach",
                    "## What shipped in this run",
                )
            }),
    );
    out.push_str("\n\n## What shipped in this run\n\n");
    out.push_str(&phases.and_then(render_phases_value).unwrap_or_else(|| {
        fallback_section(fallback, "## What shipped in this run", "## Open threads")
    }));
    out.push_str("\n\n## Open threads\n\n");
    let threads = overview
        .and_then(|value| string_array_field(value, "open_threads"))
        .unwrap_or_default();
    if threads.is_empty() {
        out.push_str("- No open threads recorded by deadreckon.\n");
    } else {
        for thread in threads {
            out.push_str(&format!("- {thread}\n"));
        }
    }
    out.push_str("\n## Cross-references\n\n");
    let refs = overview
        .and_then(|value| string_array_field(value, "cross_references"))
        .unwrap_or_default();
    if refs.is_empty() {
        out.push_str(&fallback_section(fallback, "## Cross-references", ""));
    } else {
        for item in refs {
            out.push_str(&format!("- {item}\n"));
        }
    }
    out.trim_end().to_string() + "\n"
}

fn render_phases_value(value: &Value) -> Option<String> {
    if let Some(markdown) = string_field(value, "phases_markdown") {
        return Some(markdown);
    }
    let phases = value.get("phases")?.as_array()?;
    if phases.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (idx, phase) in phases.iter().enumerate() {
        let title = string_field(phase, "title").unwrap_or_else(|| format!("Phase {}", idx + 1));
        let commit = string_field(phase, "commit")
            .or_else(|| string_field(phase, "commit_sha"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "### Phase {} - {} (commit `{}`)\n\n",
            idx + 1,
            title.trim(),
            commit.trim()
        ));
        let prose = string_field(phase, "paragraph")
            .or_else(|| string_field(phase, "prose"))
            .or_else(|| string_field(phase, "summary"))
            .unwrap_or_else(|| "No phase prose returned by doc provider.".to_string());
        out.push_str(prose.trim());
        out.push_str("\n\n| File | Change | Largest hunk |\n| --- | ---: | --- |\n");
        let file_changes = phase
            .get("file_changes")
            .or_else(|| phase.get("files"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if file_changes.is_empty() {
            out.push_str("| - | +0 / -0 | no file changes returned |\n\n");
        } else {
            for file in file_changes {
                if let Some(text) = file.as_str() {
                    if !looks_documentable_phase_line(text) {
                        continue;
                    }
                    out.push_str(&format!(
                        "| {} | - | evidence returned as prose |\n",
                        text.replace('|', "\\|")
                    ));
                    continue;
                }
                let path = string_field(&file, "path").unwrap_or_else(|| file.to_string());
                if !is_documentable_path(path.trim_matches('"')) {
                    continue;
                }
                let adds = u64_field(&file, "adds");
                let dels = u64_field(&file, "dels");
                let hunk = string_field(&file, "largest_hunk_excerpt")
                    .or_else(|| string_field(&file, "diff_quote"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "| `{}` | +{} / -{} | {} |\n",
                    path.trim_matches('"'),
                    adds,
                    dels,
                    inline_hunk(&hunk)
                ));
            }
            out.push('\n');
        }
        let citations = string_array_field(phase, "citations").unwrap_or_default();
        for citation in citations {
            out.push_str(&format!("- Trace: {citation}\n"));
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    Some(out)
}

fn looks_documentable_phase_line(line: &str) -> bool {
    let candidate = line
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches('`')
        .split(['`', ':', ' '])
        .next()
        .unwrap_or_default();
    candidate.is_empty() || is_documentable_path(candidate)
}

fn render_split_as_built(
    state: &PipelineState,
    as_built: Option<&Value>,
    fallback: &str,
) -> Result<String> {
    if let Some(markdown) = as_built.and_then(|value| string_field(value, "as_built")) {
        return Ok(markdown);
    }
    let Some(value) = as_built else {
        return Ok(fallback.to_string());
    };
    let mut out = frontmatter_prefix(fallback);
    out.push_str("## System overview\n\n");
    out.push_str(
        value
            .get("system_overview")
            .and_then(Value::as_str)
            .unwrap_or("The run produced or changed the components listed below."),
    );
    out.push_str("\n\n## Source layout\n\n");
    let records = read_turn_records(&state.working_dir)?;
    let layout = value
        .get("source_layout")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| source_layout(&records, &state.working_dir));
    out.push_str(&layout);
    if !layout.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n## Components\n\n");
    if let Some(components) = value.get("components").and_then(Value::as_array) {
        out.push_str("| Layer | Responsibilities | Key entrypoints |\n| --- | --- | --- |\n");
        for component in components {
            let layer = string_field(component, "layer").unwrap_or_else(|| "Component".to_string());
            let responsibilities =
                string_field(component, "responsibilities").unwrap_or_else(|| "-".to_string());
            let entrypoints =
                string_field(component, "key_entrypoints").unwrap_or_else(|| "-".to_string());
            if layer != "Project files" {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    layer, responsibilities, entrypoints
                ));
            }
        }
    } else if let Some(components) = value.get("components").and_then(Value::as_str) {
        out.push_str(components);
        if !components.ends_with('\n') {
            out.push('\n');
        }
    } else {
        out.push_str(&fallback_section(
            fallback,
            "## Components",
            "## File layout",
        ));
    }
    out.push_str("\n## Load-bearing paths\n\n");
    out.push_str(
        value
            .get("load_bearing_paths")
            .and_then(Value::as_str)
            .unwrap_or("See the component table above for the files that carry runtime behavior."),
    );
    out.push_str("\n\n## Seams\n\n");
    out.push_str(
        value
            .get("seams")
            .and_then(Value::as_str)
            .unwrap_or("External and internal seams are listed in the trace-backed source layout."),
    );
    out.push('\n');
    Ok(out)
}

fn render_split_decisions(decisions: Option<&Value>, fallback: &str) -> String {
    if let Some(markdown) = decisions.and_then(|value| string_field(value, "decisions_markdown")) {
        return markdown;
    }
    let frontmatter = frontmatter_prefix(fallback);
    let Some(value) = decisions else {
        return fallback.to_string();
    };
    let items = value
        .get("decisions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = frontmatter;
    for (heading, key, next) in [
        ("Design decisions", "design_decisions", "## Deviations"),
        ("Deviations", "deviations", "## Tradeoffs"),
        ("Tradeoffs", "tradeoffs", "## Open questions"),
        (
            "Open questions",
            "open_questions",
            "## Multi-alternative decision details",
        ),
    ] {
        out.push_str(&format!("## {heading}\n\n"));
        out.push_str(
            string_field(value, key)
                .filter(|field| !field.trim().is_empty())
                .unwrap_or_else(|| fallback_section(fallback, &format!("## {heading}"), next))
                .trim(),
        );
        out.push_str("\n\n");
    }
    out.push_str("## Multi-alternative decision details\n\n");
    if items.is_empty() {
        out.push_str("No multi-alternative decisions detected in this run.\n");
        return out;
    }
    for (idx, item) in items.iter().enumerate() {
        out.push_str(&format!("### Decision {}\n\n", idx + 1));
        if let Some(title) = string_field(item, "title")
            && !title.trim().is_empty()
        {
            out.push_str(&format!("- **Title:** {title}\n"));
        }
        if let Some(turn) = item.get("turn").and_then(Value::as_u64) {
            out.push_str(&format!("- **Turn:** {turn}\n"));
        }
        out.push_str(&format!(
            "- **Context:** {}\n",
            string_field(item, "context").unwrap_or_else(|| "-".to_string())
        ));
        let considered = string_array_field(item, "considered")
            .or_else(|| string_array_field(item, "options"))
            .unwrap_or_default()
            .join("; ");
        out.push_str(&format!("- **Options considered:** {}\n", considered));
        out.push_str(&format!(
            "- **Chosen:** {}\n",
            string_field(item, "chosen").unwrap_or_else(|| "-".to_string())
        ));
        out.push_str(&format!(
            "- **Rationale:** {}\n\n",
            string_field(item, "why")
                .or_else(|| string_field(item, "rationale"))
                .unwrap_or_else(|| "-".to_string())
        ));
        if let Some(files) = string_array_field(item, "files_affected")
            && !files.is_empty()
        {
            out.push_str(&format!("- **Files affected:** {}\n", files.join("; ")));
        }
        if let Some(citations) = string_array_field(item, "citations")
            && !citations.is_empty()
        {
            out.push_str(&format!("- **Citations:** {}\n", citations.join("; ")));
        }
        out.push('\n');
    }
    out
}

fn frontmatter_prefix(markdown: &str) -> String {
    markdown
        .find("## ")
        .map(|idx| markdown[..idx].to_string())
        .filter(|prefix| !prefix.trim().is_empty())
        .unwrap_or_default()
}

fn fallback_section(markdown: &str, start: &str, end: &str) -> String {
    let Some(start_idx) = markdown.find(start) else {
        return String::new();
    };
    let body_start = start_idx + start.len();
    let body_end = if end.is_empty() {
        markdown.len()
    } else {
        markdown[body_start..]
            .find(end)
            .map(|idx| body_start + idx)
            .unwrap_or(markdown.len())
    };
    markdown[body_start..body_end].trim().to_string()
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn string_array_field(value: &Value, field: &str) -> Option<Vec<String>> {
    value.get(field).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .or_else(|| string_field(item, "text"))
            })
            .filter(|item| !item.trim().is_empty())
            .collect()
    })
}

fn u64_field(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn inline_hunk(hunk: &str) -> String {
    let trimmed = hunk.trim();
    if trimmed.is_empty() {
        "no hunk returned".to_string()
    } else {
        format!("`{}`", trimmed.replace('\n', " / ").replace('|', "\\|"))
    }
}

pub fn resolve_skill(name: &str, state: &PipelineState, home: &Path) -> Result<ResolvedSkill> {
    let mut candidates = Vec::new();
    if let Ok(record) = read_run_codebase_record(&state.run_root, &state.working_dir)
        && let Some(source) = record.source_path.as_ref()
    {
        candidates.push((
            source.join("skills").join(name).join("SKILL.md"),
            SkillSource::Project,
        ));
    }
    candidates.push((
        home.join("skills").join(name).join("SKILL.md"),
        SkillSource::User,
    ));
    if let Some(root) = source_root() {
        candidates.push((
            root.join("skills").join(name).join("SKILL.md"),
            SkillSource::Repo,
        ));
    }
    for (path, source) in candidates {
        if path.exists() {
            return Ok(ResolvedSkill { path, source });
        }
    }
    Err(DeadreckonError::NotFound(format!(
        "doc skill {name} in project, user, or repo skill paths"
    )))
}

pub fn substitute_placeholders(template: &str, values: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in values {
        out = out.replace(&format!("{{{{ {} }}}}", key), value);
        out = out.replace(&format!("{{{{{}}}}}", key), value);
    }
    out
}

pub fn inputs_hash(state: &PipelineState) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(state.goal.as_bytes());
    for path in [
        state.run_root.join("traces.jsonl"),
        state.run_root.join("provenance.jsonl"),
        state.run_root.join("spend.jsonl"),
        docs_dir(&state.working_dir).join("_incremental.jsonl"),
        implementation_notes_path(&state.working_dir),
    ] {
        if let Ok(bytes) = fs::read(&path) {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(bytes);
        }
    }
    for file in changed_doc_files(state)? {
        hasher.update(file.as_bytes());
    }
    if let Ok(record) = read_run_codebase_record(&state.run_root, &state.working_dir)
        && let Some(source) = record.source_path.as_ref()
    {
        for name in ["AS-BUILT-ARCHITECTURE.md", "AS-BUILT.md"] {
            let path = source.join(name);
            if let Ok(bytes) = fs::read(path) {
                hasher.update(bytes);
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn polish_prompt(state: &PipelineState, skill_path: &Path, suffix: &str) -> Result<String> {
    let skill = fs::read_to_string(skill_path).with_path(skill_path)?;
    let traces = fs::read_to_string(state.run_root.join("traces.jsonl")).unwrap_or_default();
    let incremental = fs::read_to_string(docs_dir(&state.working_dir).join("_incremental.jsonl"))
        .unwrap_or_default();
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).unwrap_or_default();
    let implementation_notes =
        fs::read_to_string(implementation_notes_path(&state.working_dir)).unwrap_or_default();
    let files = changed_doc_files(state)?.join("\n");
    let records = read_turn_records(&state.working_dir)?;
    let prompt = substitute_placeholders(
        &skill,
        &[
            ("goal", state.goal.clone()),
            ("run_id", state.run_id.clone()),
            ("status", state.status.to_string()),
            ("provider", state.provider.clone().unwrap_or_default()),
            ("sandbox", state.sandbox.clone()),
            ("run_summary", run_summary(state, &records)?),
            ("trace_jsonl", traces),
            ("incremental_jsonl", incremental),
            // Q2 convergence: seed from the full accumulated live narration when
            // present, falling back to the templated draft.
            (
                "current_narrative",
                live_narrative_digest(&state.run_root).unwrap_or(narrative),
            ),
            ("implementation_notes", implementation_notes),
            ("changed_files", files),
            ("diff_samples", diff_samples_markdown(&records)),
            ("tool_stdout", tool_stdio_markdown(&records)),
            ("source_layout", source_layout(&records, &state.working_dir)),
            ("parent_narrative", parent_narrative(state)),
        ],
    );
    Ok(format!("{prompt}\n{suffix}"))
}

fn run_summary(
    state: &PipelineState,
    records: &[deadreckon_core::docs::TurnRecord],
) -> Result<String> {
    Ok(format!(
        "run_id: {}\nstatus: {}\ngoal: {}\nprovider: {}\nsandbox: {}\nturns: {}\ndocumentable_changed_files: {}",
        state.run_id,
        state.status,
        state.goal,
        state
            .provider
            .clone()
            .unwrap_or_else(|| "unconfigured".to_string()),
        state.sandbox,
        records.len(),
        changed_doc_files(state)?.join(", ")
    ))
}

fn parent_narrative(state: &PipelineState) -> String {
    let marker_path = state.working_dir.join(".deadreckon/parent.json");
    let Ok(raw) = fs::read_to_string(marker_path) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return String::new();
    };
    let Some(parent_run_id) = value.get("parent_run_id").and_then(Value::as_str) else {
        return String::new();
    };
    let parent_scope = value
        .get("parent_scope")
        .and_then(Value::as_str)
        .unwrap_or(&state.scope);
    let Some(home) = state
        .run_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return String::new();
    };
    let path = home
        .join("library")
        .join(parent_scope)
        .join(parent_run_id)
        .join("docs")
        .join(RUN_NARRATIVE);
    fs::read_to_string(path).unwrap_or_default()
}

/// Minimal view of a live narration beat, just enough to seed the post-hoc
/// `RUN-NARRATIVE.md` from the accumulated story (Live Narrator rider, Q2).
#[derive(Deserialize)]
struct LiveBeatRow {
    #[serde(default)]
    headline: String,
    #[serde(default)]
    current_work: Vec<LiveClaimRow>,
    #[serde(default)]
    live: Option<LiveBeatMetaRow>,
}

#[derive(Deserialize)]
struct LiveClaimRow {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct LiveBeatMetaRow {
    #[serde(default)]
    covers_turn: u32,
    #[serde(default)]
    rolling_summary: Option<String>,
}

/// Render the full accumulated live narration (every beat in
/// `<run_root>/narrative/snapshots.jsonl`) into a markdown digest. The post-hoc
/// polish consolidates this rather than re-deriving from the raw trace, so the
/// final narrative refines the story the run already told. `None` when no live
/// beats were written (then the templated draft is used).
fn live_narrative_digest(run_root: &Path) -> Option<String> {
    let raw = fs::read_to_string(run_root.join("narrative").join("snapshots.jsonl")).ok()?;
    let beats: Vec<LiveBeatRow> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<LiveBeatRow>(line).ok())
        .filter(|row| row.live.is_some())
        .collect();
    if beats.is_empty() {
        return None;
    }
    let mut out = String::from("## Live narration (accumulated beats)\n\n");
    if let Some(summary) = beats
        .last()
        .and_then(|beat| beat.live.as_ref())
        .and_then(|meta| meta.rolling_summary.as_deref())
        .filter(|summary| !summary.trim().is_empty())
    {
        out.push_str(&format!("Rolling summary: {summary}\n\n"));
    }
    for beat in &beats {
        let covers = beat.live.as_ref().map(|meta| meta.covers_turn).unwrap_or(0);
        out.push_str(&format!("- [turn {covers}] {}\n", beat.headline));
        for claim in &beat.current_work {
            out.push_str(&format!("  - {}\n", claim.text));
        }
    }
    Some(out)
}

fn write_polished_docs(state: &PipelineState, docs: &PolishedDocs) -> Result<()> {
    fs::create_dir_all(docs_dir(&state.working_dir)).with_path(docs_dir(&state.working_dir))?;
    fs::write(narrative_path(&state.working_dir), &docs.narrative)
        .with_path(narrative_path(&state.working_dir))?;
    fs::write(as_built_path(&state.working_dir), &docs.as_built)
        .with_path(as_built_path(&state.working_dir))?;
    let decisions = converged_decisions_markdown(state, &docs.decisions);
    fs::write(decisions_path(&state.working_dir), decisions)
        .with_path(decisions_path(&state.working_dir))?;
    if docs.delta.trim().is_empty() {
        let path = delta_path(&state.working_dir);
        if path.exists() {
            fs::remove_file(&path).with_path(&path)?;
        }
    } else {
        fs::write(delta_path(&state.working_dir), &docs.delta)
            .with_path(delta_path(&state.working_dir))?;
    }
    Ok(())
}

fn converged_decisions_markdown(state: &PipelineState, polished: &str) -> String {
    if decisions_have_interpretation_sections(polished) {
        return polished.to_string();
    }
    let fallback = fs::read_to_string(decisions_path(&state.working_dir)).unwrap_or_default();
    if fallback.trim().is_empty() {
        return polished.to_string();
    }
    if polished.trim().is_empty()
        || polished.contains("No multi-alternative decisions detected in this run.")
    {
        return fallback;
    }
    format!("{fallback}\n\n## Provider-polished decision notes\n\n{polished}")
}

fn decisions_have_interpretation_sections(markdown: &str) -> bool {
    [
        "## Design decisions",
        "## Deviations",
        "## Tradeoffs",
        "## Open questions",
        "## Multi-alternative decision details",
    ]
    .iter()
    .all(|heading| markdown.contains(heading))
}

// SAFETY: Call sites construct one-shot records at the boundary where they are persisted.
#[allow(clippy::needless_pass_by_value)]
fn write_polish_record(state: &PipelineState, record: PolishRecord) -> Result<()> {
    let path = polish_path(&state.working_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(&record).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )
    .with_path(path)
}

pub fn read_polish_record(state: &PipelineState) -> Result<Option<PolishRecord>> {
    let path = polish_path(&state.working_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|source| DeadreckonError::Json { path, source })
}

fn write_hash_to_codebase(state: &PipelineState, hash: &str) -> Result<()> {
    let Ok(mut record) = read_run_codebase_record(&state.run_root, &state.working_dir) else {
        return Ok(());
    };
    record.doc_polish_hash = Some(hash.to_string());
    write_codebase_record(&state.working_dir, &record)?;
    write_trusted_codebase_record(&state.run_root, &record)
}

fn skill_source_label(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::Project => "project",
        SkillSource::User => "user",
        SkillSource::Repo => "repo",
    }
}

pub fn templated_docs_json(state: &PipelineState) -> PolishedDocs {
    PolishedDocs {
        narrative: fs::read_to_string(narrative_path(&state.working_dir)).unwrap_or_default(),
        as_built: fs::read_to_string(as_built_path(&state.working_dir)).unwrap_or_default(),
        decisions: fs::read_to_string(decisions_path(&state.working_dir)).unwrap_or_default(),
        delta: fs::read_to_string(delta_path(&state.working_dir)).unwrap_or_default(),
    }
}

pub fn default_polished_json_for_tests(state: &PipelineState) -> String {
    let docs = templated_docs_json(state);
    serde_json::to_string(&json!({
        "narrative": if docs.narrative.is_empty() { format!("# Run {}\n\n{}", state.run_id, state.goal) } else { docs.narrative },
        "as_built": if docs.as_built.is_empty() { format!("# AS-BUILT {}\n", state.run_id) } else { docs.as_built },
        "decisions": if docs.decisions.is_empty() { "No multi-alternative decisions detected in this run.\n".to_string() } else { docs.decisions },
        "delta": docs.delta,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

#[allow(dead_code)]
fn public_file_name(file: &str, state: &PipelineState) -> String {
    if file == AS_BUILT_DELTA {
        format!(
            "AS-BUILT-DELTA-{}.md",
            state.run_id.chars().take(8).collect::<String>()
        )
    } else {
        file.to_string()
    }
}

#[allow(dead_code)]
fn required_doc_names() -> [&'static str; 3] {
    [RUN_NARRATIVE, RUN_AS_BUILT, RUN_DECISIONS]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use deadreckon_providers::{ProviderKind, ProviderPhaseDeadline, ProviderRouter};
    use deadreckon_sandbox::{SandboxBackend, WorkspaceAccess};

    use super::{
        DOC_POLISH_CLI_CALL_WALL_SECONDS, DOC_POLISH_HTTP_CALL_WALL_SECONDS, PolishConfig,
        PolishWallBudget, live_narrative_digest, polish_call_ceiling_for_kind,
        polish_provider_request,
    };
    use tempfile::TempDir;

    #[test]
    fn documentation_provider_is_read_only_in_an_isolated_workspace() {
        let source = TempDir::new().expect("source");
        let isolated = TempDir::new().expect("isolated");
        let request = polish_provider_request(
            "{}".to_string(),
            1_000,
            isolated.path(),
            source.path().join("provider.out"),
            SandboxBackend::SandboxExec,
        );

        assert_eq!(request.workspace_access, WorkspaceAccess::ReadOnly);
        assert_eq!(request.sandbox_backend, Some(SandboxBackend::SandboxExec));
        assert_eq!(request.cwd.as_deref(), Some(isolated.path()));
        assert_ne!(request.cwd.as_deref(), Some(source.path()));
        assert!(request.session_dir.is_none());
        assert!(request.pid_file.is_none());
    }

    #[test]
    fn docs_polish_allocates_longer_calls_to_subscription_clis() {
        assert_eq!(
            polish_call_ceiling_for_kind(Some(&ProviderKind::CliCodex)).as_secs(),
            DOC_POLISH_CLI_CALL_WALL_SECONDS
        );
        assert_eq!(
            polish_call_ceiling_for_kind(Some(&ProviderKind::Generic("cli:pi".to_string())))
                .as_secs(),
            DOC_POLISH_CLI_CALL_WALL_SECONDS
        );
        assert_eq!(
            polish_call_ceiling_for_kind(Some(&ProviderKind::OpenAi)).as_secs(),
            DOC_POLISH_HTTP_CALL_WALL_SECONDS
        );
    }

    #[test]
    fn durable_docs_reuse_the_outer_absolute_deadline_without_call_ceiling() {
        let temp = TempDir::new().expect("tempdir");
        let work_expires_at = tokio::time::Instant::now() + Duration::from_secs(10 * 60);
        let config = PolishConfig {
            home: temp.path().to_path_buf(),
            doc_skill: "run-narrator".to_string(),
            doc_provider: Some("smoke".to_string()),
            doc_provider_source: Some("test".to_string()),
            doc_subskills: Vec::new(),
            token_budget: 0,
            budget_cap_usd: None,
            sandbox_backend: SandboxBackend::None,
            commit_docs: false,
            no_llm: false,
            force: false,
            max_wall_seconds: Some(60.0),
            phase_deadline: Some(ProviderPhaseDeadline::new(
                work_expires_at,
                Duration::from_secs(17),
            )),
            cancellation_token: None,
        };
        let allocation = PolishWallBudget::new(&config)
            .allocate(&ProviderRouter::smoke())
            .expect("durable allocation");

        assert_eq!(allocation.work_expires_at, work_expires_at);
        assert_eq!(allocation.cleanup_budget, Duration::from_secs(17));
    }

    #[test]
    fn posthoc_run_narrative_seeds_from_full_live_beat_history() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().to_path_buf();
        let narrative = run_root.join("narrative");
        std::fs::create_dir_all(&narrative).expect("mkdir");
        // Two accumulated live beats plus a non-live (legacy) row that must be
        // ignored by the seeding digest.
        let lines = [
            r#"{"headline":"Started run","current_work":[]}"#,
            r#"{"headline":"Wired the bus","current_work":[{"text":"added narrator task"}],"live":{"covers_turn":3,"rolling_summary":"Through turn 3: plumbing."}}"#,
            r#"{"headline":"Continuity landed","current_work":[{"text":"amend-merge beats"}],"live":{"covers_turn":7,"rolling_summary":"Through turn 7: continuity + windowing."}}"#,
        ];
        std::fs::write(narrative.join("snapshots.jsonl"), lines.join("\n")).expect("write");

        let digest = live_narrative_digest(&run_root).expect("digest from live beats");
        assert!(digest.contains("Wired the bus"), "includes earlier beat");
        assert!(digest.contains("Continuity landed"), "includes later beat");
        assert!(digest.contains("[turn 7]"), "carries beat turn stamps");
        assert!(
            digest.contains("Through turn 7"),
            "carries the latest rolling summary"
        );
        assert!(
            !digest.contains("Started run"),
            "non-live legacy rows are not part of the live story"
        );
    }

    #[test]
    fn live_narrative_digest_absent_without_beats() {
        let temp = TempDir::new().expect("tempdir");
        assert!(live_narrative_digest(temp.path()).is_none());
    }
}
