use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::state::{PipelineState, append_json_line};
use crate::tamper::AcceptanceTamperVerdict;

pub const ACCEPTANCE_MARKER: &str = "turn-acceptance.json";
pub const ACCEPTANCE_PROGRESS_JSONL: &str = "acceptance-progress.jsonl";
pub const ACCEPTANCE_SPEC: &str = "acceptance.yaml";
const GATE_NONCE: &str = "gate/nonce";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,
    pub produced_by: String,
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub check_count: usize,
    #[serde(default)]
    pub checks: Vec<AcceptanceCheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceSpec {
    pub name: Option<String>,
    #[serde(default)]
    pub checks: Vec<AcceptanceCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcceptanceCheck {
    CargoTest {
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    FileExists {
        path: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    ContentMatch {
        path: String,
        pattern: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    BuildSuccess {
        cwd: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    Shell {
        command: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AcceptanceCheckResult {
    pub kind: String,
    pub passed: bool,
    pub must_pass: bool,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceProgressEntry {
    pub checked_at: DateTime<Utc>,
    pub status: String,
    pub index: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AcceptanceCheckResult>,
}

pub fn marker_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn marker_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn acceptance_progress_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_PROGRESS_JSONL)
}

pub fn acceptance_spec_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(ACCEPTANCE_SPEC)
}

pub fn gate_nonce_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(GATE_NONCE)
}

pub fn validate_acceptance_marker(state: &PipelineState) -> Result<AcceptanceMarker> {
    // AS-BUILT §8/§17: completion is accepted only from an external marker
    // written by a binary runner and bound to this run_id.
    let path = marker_path(state);
    let raw = std::fs::read(&path).with_path(&path)?;
    let marker: AcceptanceMarker = serde_json::from_slice(&raw).with_json_path(&path)?;
    if marker.schema_version != 1 {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported acceptance marker schema {}",
            marker.schema_version
        )));
    }
    if marker.run_id != state.run_id {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance marker run_id {} does not match {}",
            marker.run_id, state.run_id
        )));
    }
    if marker.status != "pass" || marker.produced_by != "dr-gate" {
        return Err(DeadreckonError::InvalidInput(
            "acceptance marker was not produced by dr-gate with pass status".to_string(),
        ));
    }
    let expected = marker_signature(&state.run_root, &marker)?;
    if marker.signature != expected {
        return Err(DeadreckonError::InvalidInput(
            "acceptance marker signature is invalid; forged self-attestation refused".to_string(),
        ));
    }
    Ok(marker)
}

pub fn write_acceptance_marker(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    check_count: usize,
) -> Result<AcceptanceMarker> {
    let checks = (0..check_count)
        .map(|idx| AcceptanceCheckResult {
            kind: "legacy".to_string(),
            passed: true,
            must_pass: true,
            detail: format!("legacy check {}", idx + 1),
            command: None,
            cwd: None,
            duration_ms: None,
            stdout: None,
            stderr: None,
        })
        .collect::<Vec<_>>();
    write_acceptance_marker_with_results(run_root, run_id, working_dir, checks)
}

pub fn write_acceptance_marker_with_results(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    checks: Vec<AcceptanceCheckResult>,
) -> Result<AcceptanceMarker> {
    let proofs = run_root.join("proofs");
    std::fs::create_dir_all(&proofs).with_path(&proofs)?;
    let mut marker = AcceptanceMarker {
        schema_version: 1,
        run_id,
        status: "pass".to_string(),
        produced_by: "dr-gate".to_string(),
        checked_at: Utc::now(),
        working_dir,
        signature: String::new(),
        check_count: checks.len(),
        checks,
    };
    marker.signature = marker_signature(run_root, &marker)?;
    std::fs::write(
        proofs.join(ACCEPTANCE_MARKER),
        serde_json::to_vec_pretty(&marker).map_err(|source| DeadreckonError::Json {
            path: proofs.join(ACCEPTANCE_MARKER),
            source,
        })?,
    )
    .with_path(proofs.join(ACCEPTANCE_MARKER))?;
    Ok(marker)
}

pub fn run_acceptance_gate_and_write_marker(
    run_root: &Path,
    run_id: &str,
    working_dir: &Path,
) -> Result<AcceptanceMarker> {
    let results = evaluate_acceptance_checks_with_progress(run_root, working_dir)?;
    let checks = compiled_acceptance_checks(run_root, working_dir)?;
    let tamper = crate::tamper::evaluate(run_id, run_root, working_dir, &checks)?;
    crate::tamper::write_acceptance_tamper(run_root, &tamper)?;
    if tamper.verdict == AcceptanceTamperVerdict::Refuse {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance refused: {}",
            tamper.refusal_reasons.join("; ")
        )));
    }
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance check failed: {}",
            failed.detail
        )));
    }
    write_acceptance_marker_with_results(
        run_root,
        run_id.to_string(),
        working_dir.to_path_buf(),
        results,
    )
}

pub fn evaluate_acceptance(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    let results = evaluate_acceptance_checks(run_root, working_dir)?;
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance check failed: {}",
            failed.detail
        )));
    }
    Ok(results)
}

pub fn evaluate_acceptance_checks(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    evaluate_acceptance_checks_inner(run_root, working_dir, None)
}

pub fn evaluate_acceptance_checks_with_progress(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    let progress_path = acceptance_progress_path_for_run_root(run_root);
    match std::fs::remove_file(&progress_path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: progress_path,
                source,
            });
        }
    }
    evaluate_acceptance_checks_inner(run_root, working_dir, Some(&progress_path))
}

pub fn compiled_acceptance_checks(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheck>> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    if spec_path.exists() {
        // An operator spec (or a previously-generated one) always wins verbatim.
        let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
        return parse_acceptance_checks(&raw);
    }
    // No operator spec: detect the project kind, compile a real default, and
    // persist it as the auditable generated spec before returning.
    let kind = crate::acceptance_defaults::detect_project_kind(working_dir);
    let checks = crate::acceptance_defaults::default_checks_for(&kind, working_dir);
    write_generated_spec(&spec_path, &kind, &checks)?;
    Ok(checks)
}

/// Serialize a detected/inferred default contract to the run's acceptance spec
/// path with a provenance comment header, so the operator, `verdict`, and tamper
/// all see exactly the contract that ran.
fn write_generated_spec(
    spec_path: &Path,
    kind: &crate::acceptance_defaults::ProjectKind,
    checks: &[AcceptanceCheck],
) -> Result<()> {
    let spec = AcceptanceSpec {
        name: None,
        checks: checks.to_vec(),
    };
    let body = serde_yaml::to_string(&spec).map_err(|source| {
        DeadreckonError::InvalidInput(format!(
            "failed to serialize generated acceptance spec: {source}"
        ))
    })?;
    let header = format!(
        "# generated by deadreckon detect: {}\n",
        crate::acceptance_defaults::kind_label(kind)
    );
    std::fs::write(spec_path, format!("{header}{body}")).with_path(spec_path)
}

pub fn acceptance_checks_from_yaml(raw: &str) -> Result<Vec<AcceptanceCheck>> {
    parse_acceptance_checks(raw)
}

fn evaluate_acceptance_checks_inner(
    run_root: &Path,
    working_dir: &Path,
    progress_path: Option<&Path>,
) -> Result<Vec<AcceptanceCheckResult>> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    if !spec_path.exists() {
        return evaluate_default_acceptance_with_progress(working_dir, progress_path);
    }
    let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
    let checks = parse_acceptance_checks(&raw)?;
    let total = checks.len();
    emit_acceptance_progress(progress_path, "started", 0, total, None)?;
    let mut results = Vec::new();
    for (idx, check) in checks.into_iter().enumerate() {
        let index = idx + 1;
        emit_acceptance_progress(progress_path, "running", index, total, None)?;
        let result = evaluate_check(working_dir, check)?;
        let status = if result.passed { "passed" } else { "failed" };
        emit_acceptance_progress(progress_path, status, index, total, Some(result.clone()))?;
        results.push(result);
    }
    let status = if results
        .iter()
        .any(|result| result.must_pass && !result.passed)
    {
        "failed"
    } else {
        "passed"
    };
    emit_acceptance_progress(progress_path, status, total, total, None)?;
    Ok(results)
}

fn evaluate_default_acceptance_with_progress(
    working_dir: &Path,
    progress_path: Option<&Path>,
) -> Result<Vec<AcceptanceCheckResult>> {
    emit_acceptance_progress(progress_path, "started", 0, 1, None)?;
    emit_acceptance_progress(progress_path, "running", 1, 1, None)?;
    let results = evaluate_default_acceptance(working_dir)?;
    if let Some(result) = results.first().cloned() {
        let status = if result.passed { "passed" } else { "failed" };
        emit_acceptance_progress(progress_path, status, 1, 1, Some(result))?;
    }
    let status = if results
        .iter()
        .any(|result| result.must_pass && !result.passed)
    {
        "failed"
    } else {
        "passed"
    };
    emit_acceptance_progress(progress_path, status, 1, 1, None)?;
    Ok(results)
}

fn emit_acceptance_progress(
    progress_path: Option<&Path>,
    status: &str,
    index: usize,
    total: usize,
    result: Option<AcceptanceCheckResult>,
) -> Result<()> {
    let Some(progress_path) = progress_path else {
        return Ok(());
    };
    append_json_line(
        progress_path,
        &AcceptanceProgressEntry {
            checked_at: Utc::now(),
            status: status.to_string(),
            index,
            total,
            result,
        },
    )
}

/// The default checks the no-spec (dr-gate) path evaluates — the same detection
/// and compilation as `compiled_acceptance_checks`, so the standalone binary and
/// the in-process compile agree byte-for-byte instead of diverging into a
/// Rust-only special case.
fn default_acceptance_checks(working_dir: &Path) -> Vec<AcceptanceCheck> {
    let kind = crate::acceptance_defaults::detect_project_kind(working_dir);
    crate::acceptance_defaults::default_checks_for(&kind, working_dir)
}

fn evaluate_default_acceptance(working_dir: &Path) -> Result<Vec<AcceptanceCheckResult>> {
    let mut results = Vec::new();
    for check in default_acceptance_checks(working_dir) {
        results.push(evaluate_check(working_dir, check)?);
    }
    Ok(results)
}

fn evaluate_check(working_dir: &Path, check: AcceptanceCheck) -> Result<AcceptanceCheckResult> {
    match check {
        AcceptanceCheck::CargoTest { args, must_pass } => {
            let started = Instant::now();
            let output = Command::new("cargo")
                .arg("test")
                .args(&args)
                .current_dir(working_dir)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: working_dir.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "cargo_test".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!("cargo test exited with {}", output.status),
                command: Some(format_command("cargo test", &args)),
                cwd: Some(working_dir.to_path_buf()),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
        AcceptanceCheck::FileExists { path, must_pass } => {
            let path = render_template(working_dir, &path);
            let exists = path.exists();
            Ok(AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: exists,
                must_pass,
                detail: if exists {
                    format!("{} exists", path.display())
                } else {
                    format!("{} is missing", path.display())
                },
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            })
        }
        AcceptanceCheck::ContentMatch {
            path,
            pattern,
            must_pass,
        } => {
            let path = render_template(working_dir, &path);
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            let matched = regex::Regex::new(&pattern)
                .map(|regex| regex.is_match(&body))
                .unwrap_or_else(|_| body.contains(&pattern));
            Ok(AcceptanceCheckResult {
                kind: "content_match".to_string(),
                passed: matched,
                must_pass,
                detail: if matched {
                    format!("{} matches {:?}", path.display(), pattern)
                } else {
                    format!("{} does not match {:?}", path.display(), pattern)
                },
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            })
        }
        AcceptanceCheck::BuildSuccess { cwd, must_pass } => {
            let cwd = render_template(working_dir, &cwd);
            let started = Instant::now();
            let output = Command::new("cargo")
                .arg("build")
                .current_dir(&cwd)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: cwd.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "build_success".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!(
                    "cargo build in {} exited with {}",
                    cwd.display(),
                    output.status
                ),
                command: Some("cargo build".to_string()),
                cwd: Some(cwd),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
        AcceptanceCheck::Shell {
            command,
            cwd,
            must_pass,
        } => {
            let cwd = cwd
                .map(|cwd| render_template(working_dir, &cwd))
                .unwrap_or_else(|| working_dir.to_path_buf());
            let started = Instant::now();
            let output = Command::new("sh")
                .arg("-lc")
                .arg(&command)
                .current_dir(&cwd)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: cwd.clone(),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!(
                    "shell {:?} in {} exited with {}",
                    command,
                    cwd.display(),
                    output.status
                ),
                command: Some(command),
                cwd: Some(cwd),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
    }
}

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn clipped_stdout(output: &Output) -> Option<String> {
    clipped_output(&output.stdout)
}

fn clipped_stderr(output: &Output) -> Option<String> {
    clipped_output(&output.stderr)
}

fn clipped_output(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(clip_text(&text, 4096))
    }
}

fn clip_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut clipped = text
        .char_indices()
        .take_while(|(idx, _)| *idx < limit)
        .map(|(_, ch)| ch)
        .collect::<String>();
    clipped.push_str("\n... output truncated ...");
    clipped
}

fn format_command(base: &str, args: &[String]) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        format!("{base} {}", args.join(" "))
    }
}

fn parse_acceptance_checks(raw: &str) -> Result<Vec<AcceptanceCheck>> {
    let root: YamlValue = serde_yaml::from_str(raw).map_err(|source| {
        DeadreckonError::InvalidInput(format!("invalid acceptance.yaml: {source}"))
    })?;
    let mut checks = Vec::new();
    for item in yaml_seq(yaml_get(&root, "checks")) {
        checks.push(parse_check_value(item, None)?);
    }
    for item in yaml_seq(yaml_get(&root, "required")) {
        checks.push(parse_check_value(item, Some(true))?);
    }
    for item in yaml_seq(yaml_get(&root, "optional")) {
        checks.push(parse_check_value(item, Some(false))?);
    }
    for item in yaml_seq(yaml_get(&root, "tests")) {
        checks.push(parse_shell_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "file-exists")) {
        checks.push(parse_file_exists_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "content-match")) {
        checks.push(parse_content_match_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "build-success")) {
        checks.push(parse_build_success_check(item, true));
    }
    Ok(checks)
}

fn parse_check_value(value: &YamlValue, force_must_pass: Option<bool>) -> Result<AcceptanceCheck> {
    if let Ok(mut check) = serde_yaml::from_value::<AcceptanceCheck>(value.clone()) {
        if let Some(must_pass) = force_must_pass {
            check.set_must_pass(must_pass);
        }
        return Ok(check);
    }
    if let Some(command) = yaml_string(value) {
        return Ok(AcceptanceCheck::Shell {
            command,
            cwd: None,
            must_pass: force_must_pass.unwrap_or(true),
        });
    }
    let Some((kind, body)) = single_key_mapping(value) else {
        return Err(DeadreckonError::InvalidInput(format!(
            "invalid acceptance check: {:?}",
            value
        )));
    };
    let must_pass = force_must_pass.unwrap_or(true);
    match kind.as_str() {
        "file-exists" | "file_exists" => parse_file_exists_check(body, must_pass),
        "content-match" | "content_match" => parse_content_match_check(body, must_pass),
        "build-success" | "build_success" => Ok(parse_build_success_check(body, must_pass)),
        "shell" | "test" => parse_shell_check(body, must_pass),
        "cargo-test" | "cargo_test" => Ok(AcceptanceCheck::CargoTest {
            args: yaml_string(body).map(|arg| vec![arg]).unwrap_or_default(),
            must_pass,
        }),
        other => Err(DeadreckonError::InvalidInput(format!(
            "unknown acceptance check kind {other}"
        ))),
    }
}

fn parse_file_exists_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let path = yaml_string(value)
        .or_else(|| yaml_get(value, "path").and_then(yaml_string))
        .ok_or_else(|| DeadreckonError::InvalidInput("file-exists requires path".to_string()))?;
    Ok(AcceptanceCheck::FileExists { path, must_pass })
}

fn parse_content_match_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let path = yaml_get(value, "path")
        .and_then(yaml_string)
        .ok_or_else(|| DeadreckonError::InvalidInput("content-match requires path".to_string()))?;
    let pattern = yaml_get(value, "pattern")
        .and_then(yaml_string)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput("content-match requires pattern".to_string())
        })?;
    Ok(AcceptanceCheck::ContentMatch {
        path,
        pattern,
        must_pass,
    })
}

fn parse_build_success_check(value: &YamlValue, must_pass: bool) -> AcceptanceCheck {
    let cwd = yaml_string(value)
        .or_else(|| yaml_get(value, "cwd").and_then(yaml_string))
        .unwrap_or_else(|| "{working_dir}".to_string());
    AcceptanceCheck::BuildSuccess { cwd, must_pass }
}

fn parse_shell_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let command = yaml_string(value)
        .or_else(|| yaml_get(value, "command").and_then(yaml_string))
        .ok_or_else(|| DeadreckonError::InvalidInput("shell check requires command".to_string()))?;
    let cwd = yaml_get(value, "cwd").and_then(yaml_string);
    Ok(AcceptanceCheck::Shell {
        command,
        cwd,
        must_pass,
    })
}

fn yaml_get<'a>(value: &'a YamlValue, key: &str) -> Option<&'a YamlValue> {
    value.as_mapping()?.get(YamlValue::String(key.to_string()))
}

fn yaml_seq(value: Option<&YamlValue>) -> Vec<&YamlValue> {
    match value {
        Some(YamlValue::Sequence(values)) => values.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}

fn yaml_string(value: &YamlValue) -> Option<String> {
    value.as_str().map(ToString::to_string)
}

fn single_key_mapping(value: &YamlValue) -> Option<(String, &YamlValue)> {
    let mapping = value.as_mapping()?;
    if mapping.len() != 1 {
        return None;
    }
    let (key, value) = mapping.iter().next()?;
    Some((key.as_str()?.to_string(), value))
}

impl AcceptanceCheck {
    fn set_must_pass(&mut self, value: bool) {
        match self {
            AcceptanceCheck::CargoTest { must_pass, .. }
            | AcceptanceCheck::FileExists { must_pass, .. }
            | AcceptanceCheck::ContentMatch { must_pass, .. }
            | AcceptanceCheck::BuildSuccess { must_pass, .. }
            | AcceptanceCheck::Shell { must_pass, .. } => *must_pass = value,
        }
    }
}

fn render_template(working_dir: &Path, value: &str) -> PathBuf {
    PathBuf::from(value.replace("{working_dir}", &working_dir.to_string_lossy()))
}

fn marker_signature(run_root: &Path, marker: &AcceptanceMarker) -> Result<String> {
    let nonce_path = gate_nonce_path_for_run_root(run_root);
    let nonce = std::fs::read_to_string(&nonce_path).with_path(&nonce_path)?;
    let mut hasher = DefaultHasher::new();
    nonce.trim().hash(&mut hasher);
    marker.schema_version.hash(&mut hasher);
    marker.run_id.hash(&mut hasher);
    marker.status.hash(&mut hasher);
    marker.produced_by.hash(&mut hasher);
    marker.checked_at.to_rfc3339().hash(&mut hasher);
    marker.working_dir.hash(&mut hasher);
    marker.check_count.hash(&mut hasher);
    for check in &marker.checks {
        check.hash(&mut hasher);
    }
    let tamper_path = crate::tamper::acceptance_tamper_path_for_run_root(run_root);
    match std::fs::read(&tamper_path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => "".hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: tamper_path,
                source,
            });
        }
    }
    // A campaign result run carries its gate-verdict roll-up; binding it here means
    // the roll-up cannot be edited after signing to launder a refused leaf into a
    // clean pass. Absent (the normal, non-campaign case) hashes empty.
    let rollup_path = crate::campaign::rollup_path_at_run_root(run_root);
    match std::fs::read(&rollup_path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => "".hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: rollup_path,
                source,
            });
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn default_must_pass() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::artifacts::{ProvenanceRecord, append_provenance, snapshot_working};
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};
    use crate::tamper::{AcceptanceTamperVerdict, read_acceptance_tamper_for_run_root};

    use super::{
        ACCEPTANCE_MARKER, AcceptanceCheckResult, AcceptanceMarker, validate_acceptance_marker,
    };

    // ---- P5: detection wired into compiled_acceptance_checks + spec persisted ----

    #[test]
    fn compiled_checks_persist_generated_spec_for_node() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");

        let checks = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "npm test"
        ));

        let spec_path = super::acceptance_spec_path_for_run_root(&run_root);
        let written = std::fs::read_to_string(&spec_path).expect("generated spec written");
        assert!(written.contains("# generated by deadreckon detect: node"));
        assert!(written.contains("npm test"));
    }

    #[test]
    fn operator_spec_overrides_detection() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        // Node tree, but operator already wrote a different spec.
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");
        let spec_path = super::acceptance_spec_path_for_run_root(&run_root);
        let operator =
            "# operator\nchecks:\n- kind: shell\n  command: ./my-checks.sh\n  must_pass: true\n";
        std::fs::write(&spec_path, operator).expect("operator spec");

        let checks = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "./my-checks.sh"
        ));
        // The operator spec is not overwritten by detection.
        assert_eq!(std::fs::read_to_string(&spec_path).expect("spec"), operator);
    }

    #[test]
    fn generated_spec_roundtrips_through_parse_acceptance_checks() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(working.join("go.mod"), "module example.com/x\n").expect("go.mod");

        let compiled = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        // Re-reading the persisted spec yields the same checks.
        let reparsed = super::compiled_acceptance_checks(&run_root, &working).expect("reparse");
        assert_eq!(compiled, reparsed);
        assert!(matches!(
            reparsed.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "go test ./..."
        ));
    }

    // ---- P6: dr-gate default eval routes through default_checks_for ----

    #[test]
    fn dr_gate_default_eval_matches_compiled_checks_for_python() {
        let temp = TempDir::new().expect("tempdir");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(working.join("pyproject.toml"), "[project]\nname = \"x\"\n")
            .expect("pyproject");
        std::fs::write(
            working.join("test_app.py"),
            "def test_ok():\n    assert True\n",
        )
        .expect("test file");

        let dr_gate = super::default_acceptance_checks(&working);
        let in_process = crate::acceptance_defaults::default_checks_for(
            &crate::acceptance_defaults::ProjectKind::Python,
            &working,
        );
        assert_eq!(dr_gate, in_process);
        assert!(matches!(
            dr_gate.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "python -m pytest -q"
        ));
    }

    #[test]
    fn dr_gate_default_eval_node_runs_test_not_fileexists() {
        let temp = TempDir::new().expect("tempdir");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");

        let checks = super::default_acceptance_checks(&working);
        // The dr-gate default path attempts the real test command, never the
        // hollow FileExists "working directory exists".
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "npm test"
        ));
        assert!(
            !checks
                .iter()
                .any(|c| matches!(c, super::AcceptanceCheck::FileExists { .. }))
        );
    }

    #[test]
    fn rejects_agent_written_marker_with_wrong_run_id() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate".to_string(),
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
        let proofs = state.run_root.join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let marker = AcceptanceMarker {
            schema_version: 1,
            run_id: "wrong-run".to_string(),
            status: "pass".to_string(),
            produced_by: "agent".to_string(),
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            signature: "forged".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject");
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn acceptance_yaml_parsed_and_evaluated() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec".to_string(),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
name: fixture
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: content_match
    path: "{working_dir}/notes.md"
    pattern: "dead reckoning"
"#,
        )
        .expect("spec");
        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.passed));
    }

    #[test]
    fn acceptance_yaml_required_optional_and_shell_evaluated() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-v2".to_string(),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
required:
  - file-exists: "{working_dir}/notes.md"
  - content-match:
      path: "{working_dir}/notes.md"
      pattern: "dead reckoning"
  - shell:
      command: "test -f notes.md"
optional:
  - shell: "exit 7"
tests:
  - "test -f notes.md"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");

        assert_eq!(results.len(), 5);
        assert!(
            results
                .iter()
                .filter(|result| result.must_pass)
                .all(|result| result.passed)
        );
        assert!(
            results
                .iter()
                .any(|result| !result.must_pass && !result.passed)
        );
    }

    #[test]
    fn acceptance_required_failure_blocks_optional_success() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-fail".to_string(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
required:
  - file-exists: "{working_dir}/missing.txt"
optional:
  - shell: "exit 0"
"#,
        )
        .expect("spec");

        let err = super::evaluate_acceptance(&state.run_root, &state.working_dir)
            .expect_err("required failure");

        assert!(err.to_string().contains("acceptance check failed"));
    }

    #[test]
    fn acceptance_checks_collect_failure_evidence_without_short_circuiting() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-fail-evidence".to_string(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: shell
    command: "echo first-failed >&2; exit 4"
  - kind: shell
    command: "echo second-ran"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance_checks(&state.run_root, &state.working_dir).expect("checks");

        assert_eq!(results.len(), 2);
        assert!(!results[0].passed);
        assert!(
            results[0]
                .stderr
                .as_deref()
                .is_some_and(|stderr| stderr.contains("first-failed"))
        );
        assert!(results[1].passed);
        assert!(
            results[1]
                .stdout
                .as_deref()
                .is_some_and(|stdout| stdout.contains("second-ran"))
        );
    }

    #[test]
    fn acceptance_progress_jsonl_records_running_and_result_rows() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-progress".to_string(),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: shell
    command: "test -f notes.md"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance_checks_with_progress(&state.run_root, &state.working_dir)
                .expect("checks");
        let raw = std::fs::read_to_string(super::acceptance_progress_path_for_run_root(
            &state.run_root,
        ))
        .expect("progress");
        let progress = raw
            .lines()
            .map(|line| serde_json::from_str::<super::AcceptanceProgressEntry>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(results.len(), 2);
        assert!(
            progress
                .iter()
                .any(|entry| entry.status == "running" && entry.index == 1 && entry.total == 2),
            "{progress:?}"
        );
        assert_eq!(
            progress
                .iter()
                .filter(|entry| entry.result.as_ref().is_some_and(|result| result.passed))
                .count(),
            2
        );
        assert_eq!(
            progress.last().map(|entry| entry.status.as_str()),
            Some("passed")
        );
    }

    #[test]
    fn content_match_accepts_regex_patterns() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "regex".to_string(),
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
        std::fs::write(state.working_dir.join("app.txt"), "version 12").expect("app");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: content_match
    path: "{working_dir}/app.txt"
    pattern: 'version \d+'
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");

        assert!(results[0].passed);
    }

    #[test]
    fn marker_signature_includes_check_results() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "marker-checks".to_string(),
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
        let mut marker = super::write_acceptance_marker_with_results(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            vec![AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: true,
                must_pass: true,
                detail: "original".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
        )
        .expect("marker");
        marker.checks[0].detail = "tampered".to_string();
        std::fs::write(
            super::marker_path(&state),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("tamper");

        let err = validate_acceptance_marker(&state).expect_err("tamper rejected");

        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn gate_refuse_writes_tamper_file_and_no_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate refuse tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: shell\n    command: \"cargo test || true\"\n",
        )
        .expect("spec");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("refuse");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert!(err.to_string().contains("acceptance refused"));
        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Refuse);
        assert!(
            !super::marker_path(&state).exists(),
            "refused gate must not write marker"
        );
    }

    #[test]
    fn gate_caveat_writes_signed_marker_and_caveat_record() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate caveat tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "before\n").expect("readme");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(state.working_dir.join("README.md"), "after\n").expect("edit readme");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.working_dir.join("README.md")],
            },
        )
        .expect("provenance");

        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("caveat signs");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Caveat);
        assert!(
            tamper
                .caveats
                .iter()
                .any(|caveat| caveat.contains("README.md")),
            "{tamper:?}"
        );
        validate_acceptance_marker(&state).expect("signed caveat validates");
    }

    #[test]
    fn forged_tamper_file_fails_marker_signature_validation() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "forged tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "before\n").expect("readme");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(state.working_dir.join("README.md"), "after\n").expect("edit readme");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.working_dir.join("README.md")],
            },
        )
        .expect("provenance");
        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("caveat signs");
        let tamper_path = crate::tamper::acceptance_tamper_path_for_run_root(&state.run_root);
        std::fs::write(
            tamper_path,
            r#"{"schema_version":1,"run_id":"forged","verdict":"clean"}"#,
        )
        .expect("forge");

        let err = validate_acceptance_marker(&state).expect_err("tamper rejected");

        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn clean_run_signs_and_validates_unchanged() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "clean gate".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");

        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("clean signs");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Clean);
        validate_acceptance_marker(&state).expect("clean marker validates");
    }

    #[test]
    fn gate_signature_unchanged_with_all_seams_active() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "seam sidecars do not bind gate".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(
            state.run_root.join("seams.json"),
            r#"{"schema_version":1,"no_seams":false,"kinds":{"policy":{"source":"external"},"catalog":{"source":"external"},"hooks":{"source":"external"},"event_sink":{"source":"external"}}}"#,
        )
        .expect("seams");
        std::fs::write(
            state.run_root.join("compaction.jsonl"),
            "{\"schema_version\":1,\"turn\":2,\"context_window\":200000}\n",
        )
        .expect("compaction");

        let marker = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("clean signs with seam sidecars");
        let signature = marker.signature;
        validate_acceptance_marker(&state).expect("marker validates before sidecar edit");

        std::fs::write(state.run_root.join("seams.json"), "{\"tampered\":true}\n")
            .expect("edit seams");
        std::fs::write(
            state.run_root.join("compaction.jsonl"),
            "{\"schema_version\":1,\"turn\":99}\n",
        )
        .expect("edit compaction");
        let validated =
            validate_acceptance_marker(&state).expect("sidecars are not signature inputs");
        let expected = super::marker_signature(&state.run_root, &validated).expect("signature");

        assert_eq!(validated.signature, signature);
        assert_eq!(validated.signature, expected);
    }

    #[test]
    fn self_attest_attempt_fails() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "forged".to_string(),
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
        let proofs = state.run_root.join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let marker = AcceptanceMarker {
            schema_version: 1,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            signature: "agent-forged".to_string(),
            check_count: 1,
            checks: Vec::new(),
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject forged");
        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn deleting_a_covered_test_file_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "deleted test hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("Cargo.toml"),
            "[package]\nname = \"gate_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n",
        )
        .expect("cargo");
        std::fs::write(
            state.working_dir.join("src/lib.rs"),
            "pub fn ok() -> bool { true }\n",
        )
        .expect("lib");
        std::fs::write(
            state.working_dir.join("tests/auth_test.rs"),
            "#[test]\nfn expired_token_is_rejected() { assert!(!gate_fixture::ok()); }\n",
        )
        .expect("test");
        snapshot_working(&state, 0).expect("snapshot");

        std::fs::remove_file(state.working_dir.join("tests/auth_test.rs")).expect("delete test");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("deleted covered test refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "deleted covered test must not produce a signed marker"
        );
    }

    #[test]
    fn editing_acceptance_yaml_during_run_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "edited acceptance hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: edited\nchecks:\n  - kind: shell\n    command: \"true\"\n    cwd: \"{working_dir}\"\n",
        )
        .expect("acceptance");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.run_root.join("acceptance.yaml")],
            },
        )
        .expect("provenance");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("edited acceptance refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "edited acceptance.yaml must not produce a signed marker"
        );
    }

    #[test]
    fn suppression_pattern_in_shell_check_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "suppressed shell hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: suppressed\nchecks:\n  - kind: shell\n    command: \"cargo test || true\"\n    cwd: \"{working_dir}\"\n",
        )
        .expect("acceptance");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("suppression refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "suppression-pattern check must not produce a signed marker"
        );
    }
}
