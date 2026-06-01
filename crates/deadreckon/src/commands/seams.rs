use super::super::*;

use deadreckon_providers::ModelCatalogOverride;
use deadreckon_runtime::{SeamCommandConfig, SeamOutcome, dispatch_seam};
use deadreckon_sandbox::resolve_backend;

pub(crate) async fn seams_command(command: SeamsCommand) -> Result<()> {
    match command {
        SeamsCommand::Validate {
            kind,
            config,
            fixture,
            sandbox,
            json,
        } => {
            let kind = seam_kind_from_cli(kind);
            let sandbox_backend: SandboxBackend = sandbox.parse()?;
            let report =
                validate_seam_report(kind, &config, fixture.as_deref(), sandbox_backend).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_validation_report(&report);
            }
            if report.exit_success {
                Ok(())
            } else {
                Err(CliError::Core(deadreckon_core::user_error(
                    "seam validation failed",
                    &report
                        .try_lines
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "deadreckon doctor".to_string()),
                )))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SeamValidationReport {
    kind: String,
    command: String,
    fixture: String,
    sandbox: String,
    outcome: String,
    fail_policy: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(rename = "try")]
    try_lines: Vec<String>,
    #[serde(skip)]
    exit_success: bool,
}

async fn validate_seam_report(
    kind: SeamKind,
    config_path: &Path,
    fixture_path: Option<&Path>,
    sandbox_backend: SandboxBackend,
) -> Result<SeamValidationReport> {
    let fixture = fixture_path
        .map(PathBuf::from)
        .unwrap_or_else(|| default_fixture_path(kind, config_path));
    let fail_policy = seam_fail_policy_label(kind).to_string();
    let sandbox = resolve_backend(sandbox_backend)?.0.to_string();

    if !config_path.exists() {
        return Ok(SeamValidationReport {
            kind: cli_kind_label(kind).to_string(),
            command: "-".to_string(),
            fixture: display_path(&fixture),
            sandbox,
            outcome: "failed".to_string(),
            fail_policy,
            source: "missing-config".to_string(),
            decision: None,
            detail: Some(format!("config not found: {}", config_path.display())),
            try_lines: vec![
                format!("create {}", config_path.display()),
                "deadreckon seams validate policy --config examples/seams/config.toml --sandbox none"
                    .to_string(),
            ],
            exit_success: false,
        });
    }

    let seams = read_seams_config(config_path, false)?;
    let Some(command) = seams.command_for(kind).cloned() else {
        return Ok(SeamValidationReport {
            kind: cli_kind_label(kind).to_string(),
            command: "-".to_string(),
            fixture: display_path(&fixture),
            sandbox,
            outcome: "failed".to_string(),
            fail_policy,
            source: "builtin".to_string(),
            decision: None,
            detail: Some(format!(
                "no [seams.{}] external worker is configured",
                kind.config_key()
            )),
            try_lines: vec![
                format!(
                    "add [seams.{}] command/timeout_ms to {}",
                    kind.config_key(),
                    config_path.display()
                ),
                "deadreckon doctor".to_string(),
            ],
            exit_success: false,
        });
    };

    let request = read_fixture(&fixture)?;
    let run_root = validation_run_root();
    fs::create_dir_all(run_root.join("gate"))?;
    fs::create_dir_all(run_root.join("proofs"))?;
    fs::create_dir_all(run_root.join("sandbox"))?;
    let ctx = SeamRunCtx {
        run_root: run_root.clone(),
        working_dir: std::env::current_dir()?,
        sandbox_backend,
    };
    let single = SeamsConfig::with_command(kind, command.clone())?;
    let outcome = dispatch_seam(kind, &request, &single, &ctx).await;
    let _ = fs::remove_dir_all(run_root);

    Ok(classify_validation_outcome(
        kind, &command, &fixture, sandbox, outcome,
    ))
}

fn classify_validation_outcome(
    kind: SeamKind,
    command: &SeamCommandConfig,
    fixture: &Path,
    sandbox: String,
    outcome: SeamOutcome,
) -> SeamValidationReport {
    let command_name = seam_command_basename(&command.command);
    let fail_policy = seam_fail_policy_label(kind).to_string();
    let base = |outcome: &str,
                decision: Option<String>,
                detail: Option<String>,
                try_lines: Vec<String>,
                exit_success| {
        SeamValidationReport {
            kind: cli_kind_label(kind).to_string(),
            command: command_name.clone(),
            fixture: display_path(fixture),
            sandbox: sandbox.clone(),
            outcome: outcome.to_string(),
            fail_policy: fail_policy.clone(),
            source: "external".to_string(),
            decision,
            detail,
            try_lines,
            exit_success,
        }
    };

    match outcome {
        SeamOutcome::Ok(_) if kind == SeamKind::Policy => {
            base("passed", Some("allow".to_string()), None, Vec::new(), true)
        }
        SeamOutcome::Ok(value) if kind == SeamKind::Catalog => {
            match ModelCatalogOverride::from_value(value) {
                Ok(_) => base("passed", None, None, Vec::new(), true),
                Err(err) => base(
                    "failed",
                    None,
                    Some(format!("catalog response failed open: {err}")),
                    vec![
                        r#"return {"models":[{"id":"local-model","context_window":4000}]}"#
                            .to_string(),
                    ],
                    false,
                ),
            }
        }
        SeamOutcome::Ok(_) => base("passed", None, None, Vec::new(), true),
        SeamOutcome::Deny(reason)
            if kind == SeamKind::Policy && !reason.contains("failed closed") =>
        {
            base(
                "passed",
                Some("deny".to_string()),
                Some(reason),
                Vec::new(),
                true,
            )
        }
        SeamOutcome::Deny(reason) => base(
            "failed",
            None,
            Some(reason),
            vec![
                r#"return {"decision":"allow"} or {"decision":"deny","reason":"..."}"#.to_string(),
                "rerun the launch with --no-seams to force built-ins".to_string(),
            ],
            false,
        ),
        SeamOutcome::Fallback => base(
            "failed",
            None,
            Some("catalog worker failed open; runtime would use the built-in catalog".to_string()),
            vec![r#"return {"models":[{"id":"local-model","context_window":4000}]}"#.to_string()],
            false,
        ),
        SeamOutcome::Skipped(reason) => base(
            "non_fatal",
            None,
            Some(format!(
                "observer worker failed safe; runtime would continue: {reason}"
            )),
            vec!["inspect the worker command, timeout, and stderr".to_string()],
            true,
        ),
        SeamOutcome::Unconfigured => base(
            "failed",
            None,
            Some("no external worker configured".to_string()),
            vec![format!("add [seams.{}] to config", kind.config_key())],
            false,
        ),
    }
}

fn print_validation_report(report: &SeamValidationReport) {
    println!("{}", ui_heading("seam validation"));
    let status = if report.exit_success {
        ui_ok("✓")
    } else {
        ui_warn("✗")
    };
    println!(
        "{} {} {} command={} fixture={} sandbox={} fail={}",
        status,
        report.kind,
        report.outcome,
        report.command,
        report.fixture,
        report.sandbox,
        report.fail_policy
    );
    if let Some(decision) = &report.decision {
        println!("    decision {decision}");
    }
    if let Some(detail) = &report.detail {
        println!("    detail {detail}");
    }
    for line in &report.try_lines {
        println!("    {} {line}", ui_command("try:"));
    }
}

fn seam_kind_from_cli(kind: CliSeamKind) -> SeamKind {
    match kind {
        CliSeamKind::Policy => SeamKind::Policy,
        CliSeamKind::Catalog => SeamKind::Catalog,
        CliSeamKind::Hooks => SeamKind::Hooks,
        CliSeamKind::EventSink => SeamKind::EventSink,
    }
}

fn cli_kind_label(kind: SeamKind) -> &'static str {
    match kind {
        SeamKind::Policy => "policy",
        SeamKind::Catalog => "catalog",
        SeamKind::Hooks => "hooks",
        SeamKind::EventSink => "event-sink",
    }
}

fn seam_fail_policy_label(kind: SeamKind) -> &'static str {
    match kind {
        SeamKind::Policy => "closed",
        SeamKind::Catalog => "open",
        SeamKind::Hooks | SeamKind::EventSink => "safe",
    }
}

fn default_fixture_path(kind: SeamKind, config_path: &Path) -> PathBuf {
    let name = match kind {
        SeamKind::Policy => "policy-allow.json",
        SeamKind::Catalog => "catalog-request.json",
        SeamKind::Hooks => "hook-event.json",
        SeamKind::EventSink => "event-sink-event.json",
    };
    let config_relative = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("fixtures")
        .join(name);
    if config_relative.exists() {
        config_relative
    } else {
        PathBuf::from("examples/seams/fixtures").join(name)
    }
}

fn read_fixture(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn validation_run_root() -> PathBuf {
    std::env::temp_dir().join(format!("deadreckon-seams-validate-{}", Uuid::new_v4()))
}

fn seam_command_basename(command: &[String]) -> String {
    command
        .first()
        .and_then(|argv0| Path::new(argv0).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("-")
        .to_string()
}

fn display_path(path: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return path.display().to_string();
    };
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_worker(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("worker");
        path
    }

    fn toml_string(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    fn write_config(temp: &TempDir, kind: SeamKind, worker: &Path) -> PathBuf {
        let config = temp.path().join("config.toml");
        fs::write(
            &config,
            format!(
                "[seams.{}]\ncommand = [\"sh\", \"{}\"]\ntimeout_ms = 1000\n",
                kind.config_key(),
                toml_string(worker)
            ),
        )
        .expect("config");
        config
    }

    fn write_fixture(temp: &TempDir, name: &str, raw: &str) -> PathBuf {
        let path = temp.path().join(name);
        fs::write(&path, raw).expect("fixture");
        path
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root")
    }

    #[test]
    fn docs_seams_examples_reference_existing_files() {
        let repo = repo_root();
        let docs = fs::read_to_string(repo.join("docs/SEAMS.md")).expect("docs");
        for relative in [
            "examples/seams/config.toml",
            "examples/seams/fixtures/policy-allow.json",
            "examples/seams/fixtures/policy-deny.json",
            "examples/seams/fixtures/catalog-request.json",
            "examples/seams/fixtures/hook-event.json",
            "examples/seams/fixtures/event-sink-event.json",
            "examples/seams/workers/policy-allow.sh",
            "examples/seams/workers/policy-deny.sh",
            "examples/seams/workers/catalog-minimal.sh",
            "examples/seams/workers/hooks-jsonl.sh",
            "examples/seams/workers/event-sink-jsonl.sh",
        ] {
            assert!(docs.contains(relative), "docs should mention {relative}");
            assert!(repo.join(relative).exists(), "{relative} should exist");
        }
    }

    #[test]
    fn seam_cli_kind_maps_to_runtime_kind() {
        assert_eq!(seam_kind_from_cli(CliSeamKind::Policy), SeamKind::Policy);
        assert_eq!(seam_kind_from_cli(CliSeamKind::Catalog), SeamKind::Catalog);
        assert_eq!(seam_kind_from_cli(CliSeamKind::Hooks), SeamKind::Hooks);
        assert_eq!(
            seam_kind_from_cli(CliSeamKind::EventSink),
            SeamKind::EventSink
        );
    }

    #[tokio::test]
    async fn missing_config_reports_try_lines() {
        let temp = TempDir::new().expect("temp");
        let report = validate_seam_report(
            SeamKind::Policy,
            &temp.path().join("missing.toml"),
            None,
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(!report.exit_success);
        assert_eq!(report.outcome, "failed");
        assert!(report.detail.expect("detail").contains("config not found"));
        assert!(!report.try_lines.is_empty());
    }

    #[tokio::test]
    async fn validate_policy_allow_worker_passes() {
        let temp = TempDir::new().expect("temp");
        let worker = write_worker(
            temp.path(),
            "allow.sh",
            "cat >/dev/null\nprintf '%s\\n' '{\"decision\":\"allow\"}'\n",
        );
        let config = write_config(&temp, SeamKind::Policy, &worker);
        let fixture = write_fixture(&temp, "policy.json", r#"{"function_id":"bash"}"#);

        let report = validate_seam_report(
            SeamKind::Policy,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(report.exit_success);
        assert_eq!(report.outcome, "passed");
        assert_eq!(report.decision.as_deref(), Some("allow"));
        assert_eq!(report.fail_policy, "closed");
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["kind"], "policy");
        assert_eq!(json["outcome"], "passed");
        assert_eq!(json["fail_policy"], "closed");
    }

    #[tokio::test]
    async fn validate_policy_deny_worker_reports_valid_denial() {
        let temp = TempDir::new().expect("temp");
        let worker = write_worker(
            temp.path(),
            "deny.sh",
            "cat >/dev/null\nprintf '%s\\n' '{\"decision\":\"deny\",\"reason\":\"blocked by test\"}'\n",
        );
        let config = write_config(&temp, SeamKind::Policy, &worker);
        let fixture = write_fixture(&temp, "policy.json", r#"{"function_id":"bash"}"#);

        let report = validate_seam_report(
            SeamKind::Policy,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(report.exit_success);
        assert_eq!(report.outcome, "passed");
        assert_eq!(report.decision.as_deref(), Some("deny"));
        assert!(report.detail.expect("detail").contains("blocked by test"));
    }

    #[tokio::test]
    async fn validate_policy_malformed_response_reports_fail_closed() {
        let temp = TempDir::new().expect("temp");
        let worker = write_worker(
            temp.path(),
            "bad-policy.sh",
            "cat >/dev/null\nprintf '%s\\n' '{\"decision\":\"maybe\"}'\n",
        );
        let config = write_config(&temp, SeamKind::Policy, &worker);
        let fixture = write_fixture(&temp, "policy.json", r#"{"function_id":"bash"}"#);

        let report = validate_seam_report(
            SeamKind::Policy,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(!report.exit_success);
        assert_eq!(report.outcome, "failed");
        assert_eq!(report.fail_policy, "closed");
        assert!(report.detail.expect("detail").contains("failed closed"));
    }

    #[tokio::test]
    async fn validate_catalog_malformed_reports_fail_open() {
        let temp = TempDir::new().expect("temp");
        let worker = write_worker(
            temp.path(),
            "bad-catalog.sh",
            "cat >/dev/null\nprintf '%s\\n' '{\"models\":[{\"context_window\":1}]}'\n",
        );
        let config = write_config(&temp, SeamKind::Catalog, &worker);
        let fixture = write_fixture(&temp, "catalog.json", "{}");

        let report = validate_seam_report(
            SeamKind::Catalog,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(!report.exit_success);
        assert_eq!(report.outcome, "failed");
        assert_eq!(report.fail_policy, "open");
        assert!(report.detail.expect("detail").contains("failed open"));
    }

    #[tokio::test]
    async fn validate_observer_nonzero_is_visible_but_nonfatal() {
        let temp = TempDir::new().expect("temp");
        let worker = write_worker(temp.path(), "bad-hook.sh", "cat >/dev/null\nexit 42\n");
        let config = write_config(&temp, SeamKind::Hooks, &worker);
        let fixture = write_fixture(&temp, "hook.json", r#"{"kind":"tool_call_started"}"#);

        let report = validate_seam_report(
            SeamKind::Hooks,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(report.exit_success);
        assert_eq!(report.outcome, "non_fatal");
        assert_eq!(report.fail_policy, "safe");
        assert!(report.detail.expect("detail").contains("failed safe"));
    }

    #[tokio::test]
    async fn validation_does_not_expose_gate_or_proof_paths_in_env() {
        let temp = TempDir::new().expect("temp");
        let capture = temp.path().join("env.txt");
        let worker = write_worker(
            temp.path(),
            "env-hook.sh",
            &format!(
                "env > '{}'\ncat >/dev/null\nprintf '%s\\n' '{{\"ok\":true}}'\n",
                capture.display()
            ),
        );
        let config = write_config(&temp, SeamKind::Hooks, &worker);
        let fixture = write_fixture(&temp, "hook.json", r#"{"kind":"tool_call_started"}"#);

        let report = validate_seam_report(
            SeamKind::Hooks,
            &config,
            Some(&fixture),
            SandboxBackend::None,
        )
        .await
        .expect("report");

        assert!(report.exit_success);
        let env = fs::read_to_string(capture).expect("env capture");
        assert!(!env.contains("DEADRECKON"));
        assert!(!env.contains("/gate"));
        assert!(!env.contains("/proofs"));
    }
}
