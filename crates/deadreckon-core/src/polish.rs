use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use deadreckon_providers::{ProviderRequest, ProviderRouter};
use deadreckon_sandbox::SandboxBackend;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::codebase::{read_codebase_record, write_codebase_record};
use crate::docs::{
    AS_BUILT_DELTA, RUN_AS_BUILT, RUN_DECISIONS, RUN_NARRATIVE, append_docs_warning, as_built_path,
    changed_doc_files, decisions_path, delta_path, docs_dir, missing_files_in_narrative,
    narrative_path, polish_path, publish_docs_for_promotion, rewrite_templated_docs,
};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::paths::SOURCE_ROOT;
use crate::state::PipelineState;

#[derive(Debug, Clone)]
pub struct PolishConfig {
    pub home: PathBuf,
    pub doc_skill: String,
    pub doc_provider: Option<String>,
    pub no_llm: bool,
    pub force: bool,
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
    pub status: String,
    pub inputs_hash: String,
    pub provider: Option<String>,
    pub skill_path: Option<PathBuf>,
    pub skill_source: Option<String>,
    pub completed_at: String,
    pub cost_usd: f64,
    pub retries: u32,
    pub missing_files: Vec<String>,
    pub error: Option<String>,
}

pub async fn polish_run_docs(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &PolishConfig,
) -> Result<()> {
    rewrite_templated_docs(state, "templated only")?;
    if config.no_llm {
        let hash = inputs_hash(state)?;
        write_polish_record(
            state,
            PolishRecord {
                status: "incremental".to_string(),
                inputs_hash: hash,
                provider: config.doc_provider.clone(),
                skill_path: None,
                skill_source: None,
                completed_at: Utc::now().to_rfc3339(),
                cost_usd: 0.0,
                retries: 0,
                missing_files: Vec::new(),
                error: None,
            },
        )?;
        publish_docs_for_promotion(state)?;
        return Ok(());
    }

    let hash = inputs_hash(state)?;
    if !config.force
        && let Some(existing) = read_polish_record(state)?
        && existing.status == "polished"
        && existing.inputs_hash == hash
    {
        publish_docs_for_promotion(state)?;
        return Ok(());
    }

    let resolved = match resolve_skill(&config.doc_skill, state, &config.home) {
        Ok(skill) => skill,
        Err(err) => {
            write_polish_record(
                state,
                PolishRecord {
                    status: "no_skill".to_string(),
                    inputs_hash: hash,
                    provider: config.doc_provider.clone(),
                    skill_path: None,
                    skill_source: None,
                    completed_at: Utc::now().to_rfc3339(),
                    cost_usd: 0.0,
                    retries: 0,
                    missing_files: Vec::new(),
                    error: Some(err.to_string()),
                },
            )?;
            publish_docs_for_promotion(state)?;
            return Ok(());
        }
    };

    let mut retries = 0;
    let mut prompt_suffix = String::new();
    let mut total_cost = 0.0;
    loop {
        let prompt = polish_prompt(state, &resolved.path, &prompt_suffix)?;
        let response = match router
            .complete(&ProviderRequest {
                prompt,
                max_output_tokens: 20_000,
                cwd: Some(state.working_dir.clone()),
                output_path: Some(
                    state
                        .run_root
                        .join("turns")
                        .join("docs-polish")
                        .join("provider.out"),
                ),
                sandbox_backend: Some(SandboxBackend::None),
                pid_file: None,
                cancellation_token: None,
            })
            .await
        {
            Ok(response) => response,
            Err(err) => {
                write_polish_record(
                    state,
                    PolishRecord {
                        status: "provider_error".to_string(),
                        inputs_hash: hash,
                        provider: config.doc_provider.clone(),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(err.to_string()),
                    },
                )?;
                publish_docs_for_promotion(state)?;
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
                        "\nYour previous output omitted these files; revise to include them with citations: {}",
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
                        status: "polished".to_string(),
                        inputs_hash: hash.clone(),
                        provider: Some(response.provider),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: missing,
                        error: None,
                    },
                )?;
                write_hash_to_codebase(state, &hash)?;
                publish_docs_for_promotion(state)?;
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
                        status: "json_parse".to_string(),
                        inputs_hash: hash,
                        provider: Some(response.provider),
                        skill_path: Some(resolved.path.clone()),
                        skill_source: Some(skill_source_label(&resolved.source).to_string()),
                        completed_at: Utc::now().to_rfc3339(),
                        cost_usd: total_cost,
                        retries,
                        missing_files: Vec::new(),
                        error: Some(error_message),
                    },
                )?;
                publish_docs_for_promotion(state)?;
                return Ok(());
            }
        }
    }
}

pub fn resolve_skill(name: &str, state: &PipelineState, home: &Path) -> Result<ResolvedSkill> {
    let mut candidates = Vec::new();
    if let Ok(record) = read_codebase_record(&state.working_dir)
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
    candidates.push((
        PathBuf::from(SOURCE_ROOT)
            .join("skills")
            .join(name)
            .join("SKILL.md"),
        SkillSource::Repo,
    ));
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
    ] {
        if let Ok(bytes) = fs::read(&path) {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(bytes);
        }
    }
    for file in changed_doc_files(state)? {
        hasher.update(file.as_bytes());
    }
    if let Ok(record) = read_codebase_record(&state.working_dir)
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
    let files = changed_doc_files(state)?.join("\n");
    let prompt = substitute_placeholders(
        &skill,
        &[
            ("goal", state.goal.clone()),
            ("run_id", state.run_id.clone()),
            ("status", state.status.to_string()),
            ("provider", state.provider.clone().unwrap_or_default()),
            ("sandbox", state.sandbox.clone()),
            ("trace_jsonl", traces),
            ("incremental_jsonl", incremental),
            ("current_narrative", narrative),
            ("changed_files", files),
        ],
    );
    Ok(format!("{prompt}\n{suffix}"))
}

fn write_polished_docs(state: &PipelineState, docs: &PolishedDocs) -> Result<()> {
    fs::create_dir_all(docs_dir(&state.working_dir)).with_path(docs_dir(&state.working_dir))?;
    fs::write(narrative_path(&state.working_dir), &docs.narrative)
        .with_path(narrative_path(&state.working_dir))?;
    fs::write(as_built_path(&state.working_dir), &docs.as_built)
        .with_path(as_built_path(&state.working_dir))?;
    fs::write(decisions_path(&state.working_dir), &docs.decisions)
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
    let Ok(mut record) = read_codebase_record(&state.working_dir) else {
        return Ok(());
    };
    record.doc_polish_hash = Some(hash.to_string());
    write_codebase_record(&state.working_dir, &record)
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
