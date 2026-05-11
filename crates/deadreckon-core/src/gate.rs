use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::state::PipelineState;

pub const ACCEPTANCE_MARKER: &str = "turn-acceptance.json";
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceCheckResult {
    pub kind: String,
    pub passed: bool,
    pub must_pass: bool,
    pub detail: String,
}

pub fn marker_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn marker_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_MARKER)
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
        check_count,
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

pub fn evaluate_acceptance(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    if !spec_path.exists() {
        return evaluate_default_acceptance(working_dir);
    }
    let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
    let spec: AcceptanceSpec = serde_yaml::from_str(&raw).map_err(|source| {
        DeadreckonError::InvalidInput(format!("invalid acceptance.yaml: {source}"))
    })?;
    let mut results = Vec::new();
    for check in spec.checks {
        let result = evaluate_check(working_dir, check)?;
        if result.must_pass && !result.passed {
            results.push(result);
            return Err(DeadreckonError::InvalidInput(format!(
                "acceptance check failed: {}",
                results
                    .last()
                    .map(|result| result.detail.as_str())
                    .unwrap_or("unknown")
            )));
        }
        results.push(result);
    }
    Ok(results)
}

fn evaluate_default_acceptance(working_dir: &Path) -> Result<Vec<AcceptanceCheckResult>> {
    if working_dir.join("Cargo.toml").exists() {
        let status = Command::new("cargo")
            .arg("test")
            .current_dir(working_dir)
            .status()
            .map_err(|source| DeadreckonError::Io {
                path: working_dir.join("Cargo.toml"),
                source,
            })?;
        if !status.success() {
            return Err(DeadreckonError::InvalidInput(
                "cargo test failed in working directory".to_string(),
            ));
        }
        return Ok(vec![AcceptanceCheckResult {
            kind: "cargo_test".to_string(),
            passed: true,
            must_pass: true,
            detail: "cargo test passed".to_string(),
        }]);
    }
    if !working_dir.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "working directory does not exist: {}",
            working_dir.display()
        )));
    }
    Ok(vec![AcceptanceCheckResult {
        kind: "working_dir".to_string(),
        passed: true,
        must_pass: true,
        detail: "working directory exists".to_string(),
    }])
}

fn evaluate_check(working_dir: &Path, check: AcceptanceCheck) -> Result<AcceptanceCheckResult> {
    match check {
        AcceptanceCheck::CargoTest { args, must_pass } => {
            let status = Command::new("cargo")
                .arg("test")
                .args(args)
                .current_dir(working_dir)
                .status()
                .map_err(|source| DeadreckonError::Io {
                    path: working_dir.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "cargo_test".to_string(),
                passed: status.success(),
                must_pass,
                detail: format!("cargo test exited with {status}"),
            })
        }
        AcceptanceCheck::FileExists { path, must_pass } => {
            let path = render_template(working_dir, &path);
            Ok(AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: path.exists(),
                must_pass,
                detail: format!("{} exists", path.display()),
            })
        }
        AcceptanceCheck::ContentMatch {
            path,
            pattern,
            must_pass,
        } => {
            let path = render_template(working_dir, &path);
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            Ok(AcceptanceCheckResult {
                kind: "content_match".to_string(),
                passed: body.contains(&pattern),
                must_pass,
                detail: format!("{} contains {:?}", path.display(), pattern),
            })
        }
        AcceptanceCheck::BuildSuccess { cwd, must_pass } => {
            let cwd = render_template(working_dir, &cwd);
            let status = Command::new("cargo")
                .arg("build")
                .current_dir(&cwd)
                .status()
                .map_err(|source| DeadreckonError::Io {
                    path: cwd.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "build_success".to_string(),
                passed: status.success(),
                must_pass,
                detail: format!("cargo build in {} exited with {status}", cwd.display()),
            })
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
    Ok(format!("{:016x}", hasher.finish()))
}

fn default_must_pass() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::{ACCEPTANCE_MARKER, AcceptanceMarker, validate_acceptance_marker};

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
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject forged");
        assert!(err.to_string().contains("signature"));
    }
}
