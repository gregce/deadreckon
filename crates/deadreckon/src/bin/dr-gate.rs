use std::path::{Path, PathBuf};

use deadreckon_core::gate::{
    AcceptanceContainment, GATE_CONTAINED_ENV, GATE_KEY_ENV, GATE_SANDBOX_BACKEND_ENV,
    compiled_acceptance_checks, decode_gate_key, evaluate_acceptance_checks_with_progress,
    write_native_acceptance_marker_with_results_and_key,
};
use deadreckon_core::tamper::{self, AcceptanceTamperVerdict};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut run_id = None;
    let mut run_root = None;
    let mut working_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => run_id = args.next(),
            "--run-root" => run_root = args.next().map(PathBuf::from),
            "--working-dir" => working_dir = args.next().map(PathBuf::from),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let run_id = run_id.ok_or("--run is required")?;
    let working_dir = working_dir.ok_or("--working-dir is required")?;
    let run_root = match run_root {
        Some(run_root) => run_root,
        None => infer_run_root(&working_dir)?,
    };
    let gate_environment =
        GateEnvironment::from_lookup(|name| std::env::var(name).ok()).map_err(|message| {
            format!("dr-gate cannot sign without trusted supervisor inputs: {message}")
        })?;
    let results = evaluate_acceptance_checks_with_progress(&run_root, &working_dir)?;
    let checks = compiled_acceptance_checks(&run_root, &working_dir)?;
    let tamper = tamper::evaluate(&run_id, &run_root, &working_dir, &checks)?;
    tamper::write_acceptance_tamper(&run_root, &tamper)?;
    if tamper.verdict == AcceptanceTamperVerdict::Refuse {
        eprintln!("acceptance refused");
        for reason in &tamper.refusal_reasons {
            eprintln!("refuse: {reason}");
        }
        return Err(format!("acceptance refused: {}", tamper.refusal_reasons.join("; ")).into());
    }
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        eprintln!("acceptance failed");
        for result in &results {
            let mark = if result.passed { "PASS" } else { "FAIL" };
            let required = if result.must_pass {
                "required"
            } else {
                "optional"
            };
            eprintln!("{mark} {required} {}: {}", result.kind, result.detail);
            if !result.passed {
                if let Some(stderr) = result.stderr.as_deref() {
                    eprintln!("stderr: {}", one_line(stderr, 500));
                }
                if let Some(stdout) = result.stdout.as_deref() {
                    eprintln!("stdout: {}", one_line(stdout, 500));
                }
            }
        }
        return Err(format!("required check failed: {}", failed.detail).into());
    }
    write_native_acceptance_marker_with_results_and_key(
        &run_root,
        run_id,
        working_dir,
        results,
        &gate_environment.key,
        gate_environment.containment,
    )?;
    Ok(())
}

struct GateEnvironment {
    key: Vec<u8>,
    containment: AcceptanceContainment,
}

impl GateEnvironment {
    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, String> {
        let encoded_key = lookup(GATE_KEY_ENV)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{GATE_KEY_ENV} is required"))?;
        let key = decode_gate_key(&encoded_key).map_err(|err| err.to_string())?;
        let backend = lookup(GATE_SANDBOX_BACKEND_ENV)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{GATE_SANDBOX_BACKEND_ENV} is required"))?;
        let contained = match lookup(GATE_CONTAINED_ENV).as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(value) => {
                return Err(format!(
                    "{GATE_CONTAINED_ENV} must be true or false, got {value}"
                ));
            }
            None => return Err(format!("{GATE_CONTAINED_ENV} is required")),
        };
        let containment = if contained {
            AcceptanceContainment::contained(backend)
        } else {
            AcceptanceContainment::uncontained(backend)
        };
        Ok(Self { key, containment })
    }
}

fn one_line(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() <= limit {
        compact
    } else {
        let clipped = compact.chars().take(limit).collect::<String>();
        format!("{clipped}...")
    }
}

fn infer_run_root(working_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if working_dir.file_name().and_then(|name| name.to_str()) != Some("working") {
        return Err("working directory must be <run-root>/working".into());
    }
    working_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "working directory has no parent".into())
}

#[cfg(test)]
mod tests {
    use deadreckon_core::gate::{
        AcceptanceProofKind, GATE_CONTAINED_ENV, GATE_KEY_ENV, GATE_SANDBOX_BACKEND_ENV,
        write_native_acceptance_marker_with_results_and_key,
    };
    use tempfile::TempDir;

    use super::GateEnvironment;

    #[test]
    fn dr_gate_signs_from_the_env_key_without_reading_the_key_path() {
        let temp = TempDir::new().expect("tempdir");
        let environment = GateEnvironment::from_lookup(|name| match name {
            GATE_KEY_ENV => Some("07".repeat(32)),
            GATE_CONTAINED_ENV => Some("true".to_string()),
            GATE_SANDBOX_BACKEND_ENV => Some("seatbelt".to_string()),
            _ => None,
        })
        .expect("environment");
        let marker = write_native_acceptance_marker_with_results_and_key(
            temp.path(),
            "env-signed".to_string(),
            temp.path().join("working"),
            Vec::new(),
            &environment.key,
            environment.containment,
        )
        .expect("marker");

        assert_eq!(marker.proof_kind, AcceptanceProofKind::NativeGate);
        assert!(marker.contained);
        assert_eq!(marker.sandbox_backend, "seatbelt");
        assert!(!temp.path().join("gate-keys").exists());
    }

    #[test]
    fn dr_gate_without_the_env_key_fails_loudly() {
        let err = GateEnvironment::from_lookup(|name| match name {
            GATE_CONTAINED_ENV => Some("false".to_string()),
            GATE_SANDBOX_BACKEND_ENV => Some("none".to_string()),
            _ => None,
        })
        .err()
        .expect("missing key refused");

        assert!(err.contains(GATE_KEY_ENV), "{err}");
        assert!(err.contains("required"), "{err}");
    }
}
