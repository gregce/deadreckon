use super::super::*;
use deadreckon_providers::{CliAuthStatus, probe_cli_auth};
use std::path::Path;

pub(crate) async fn doctor_command(json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let source = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let report = build_doctor_report(&paths, source).await?;
    let surface = doctor_verdict_surface(&report);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &surface.add_to_json(doctor_json_payload(&paths, &report, &surface))
            )?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

#[derive(Debug, Clone)]
struct DoctorReport {
    source: PathBuf,
    config_present: bool,
    sandboxes: Vec<Value>,
    seams: Value,
    findings: Vec<DoctorFinding>,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorFinding {
    status: String,
    subject: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
}

impl DoctorFinding {
    fn passed(
        subject: impl Into<String>,
        detail: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            status: "passed".to_string(),
            subject: subject.into(),
            detail: detail.into(),
            action,
        }
    }

    fn warning(
        subject: impl Into<String>,
        detail: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            status: "warning".to_string(),
            subject: subject.into(),
            detail: detail.into(),
            action,
        }
    }

    fn failed(
        subject: impl Into<String>,
        detail: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            status: "failed".to_string(),
            subject: subject.into(),
            detail: detail.into(),
            action,
        }
    }

    fn is_failed(&self) -> bool {
        self.status == "failed"
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DoctorSetupSummary {
    checked_count: usize,
    failed_count: usize,
    warning_count: usize,
}

impl DoctorSetupSummary {
    pub(crate) fn has_blocking_issues(&self) -> bool {
        self.failed_count > 0
    }

    pub(crate) fn evidence_detail(&self) -> String {
        format!(
            "{} setup checks; {} blocking issue(s); {} optional warning(s)",
            self.checked_count, self.failed_count, self.warning_count
        )
    }
}

impl From<&DoctorReport> for DoctorSetupSummary {
    fn from(report: &DoctorReport) -> Self {
        Self {
            checked_count: report.findings.len(),
            failed_count: report
                .findings
                .iter()
                .filter(|finding| finding.is_failed())
                .count(),
            warning_count: report
                .findings
                .iter()
                .filter(|finding| finding.status == "warning")
                .count(),
        }
    }
}

pub(crate) async fn doctor_setup_summary() -> Result<DoctorSetupSummary> {
    let paths = DeadreckonPaths::discover();
    let source = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let report = build_doctor_report(&paths, source).await?;
    Ok(DoctorSetupSummary::from(&report))
}

async fn build_doctor_report(paths: &DeadreckonPaths, source: PathBuf) -> Result<DoctorReport> {
    let mut findings = vec![
        DoctorFinding::passed("source", source.display().to_string(), None),
        DoctorFinding::passed("home", paths.home().display().to_string(), None),
    ];

    let mut sandboxes = Vec::new();
    for backend in deadreckon_sandbox::doctor() {
        let path = backend.path.as_ref().map(|path| path.display().to_string());
        let version = backend.path.as_ref().and_then(|path| command_version(path));
        if backend.available {
            findings.push(DoctorFinding::passed(
                format!("sandbox {}", backend.backend),
                format!(
                    "found{}{}",
                    path.as_ref()
                        .map(|path| format!(" at {path}"))
                        .unwrap_or_default(),
                    version
                        .as_ref()
                        .map(|version| format!(" ({version})"))
                        .unwrap_or_default()
                ),
                Some(format!(
                    "deadreckon run \"goal\" --sandbox {} --preview",
                    backend.backend
                )),
            ));
        } else {
            findings.push(DoctorFinding::warning(
                format!("sandbox {}", backend.backend),
                format!("missing: {}", backend.note),
                None,
            ));
        }
        sandboxes.push(json!({
            "backend": backend.backend,
            "available": backend.available,
            "path": backend.path,
            "note": backend.note,
        }));
    }

    let mut config_root = None;
    let config_present = paths.config_path().exists();
    if config_present {
        match load_config_value(paths) {
            Ok(root) => {
                findings.push(DoctorFinding::passed(
                    "config",
                    format!("{} present and parseable", paths.config_path().display()),
                    Some("deadreckon config provider".to_string()),
                ));
                config_root = Some(root);
            }
            Err(err) => findings.push(DoctorFinding::failed(
                "config",
                format!("{} is not parseable: {err}", paths.config_path().display()),
                Some("deadreckon init".to_string()),
            )),
        }
    } else {
        findings.push(DoctorFinding::failed(
            "config",
            format!("{} missing", paths.config_path().display()),
            Some("deadreckon init".to_string()),
        ));
    }

    if let Some(root) = &config_root {
        findings.extend(collect_doctor_provider_findings(paths, root).await?);
    }

    let defaults = config_defaults(paths).unwrap_or_default();
    if defaults.provider.is_some() || config_present {
        findings.push(DoctorFinding::passed(
            "provider defaults",
            "provider defaults configured",
            Some("deadreckon config provider".to_string()),
        ));
    } else if command_exists("claude") || command_exists("codex") {
        findings.push(DoctorFinding::passed(
            "provider defaults",
            "CLI subscription provider available",
            Some("deadreckon init --no-confirm".to_string()),
        ));
    } else {
        findings.push(DoctorFinding::failed(
            "provider defaults",
            "no provider configured",
            Some("deadreckon init".to_string()),
        ));
    }

    let seams = match doctor_seam_resolution(paths, false) {
        Ok(lines) => {
            findings.push(DoctorFinding::passed(
                "seams",
                format!("resolved: {}", lines.join("; ")),
                Some("deadreckon run \"goal\" --no-seams".to_string()),
            ));
            json!({
                "status": "ok",
                "resolution": lines,
            })
        }
        Err(err) => {
            findings.push(DoctorFinding::failed(
                "seams",
                format!("config invalid: {err}"),
                Some("deadreckon doctor".to_string()),
            ));
            json!({
                "status": "invalid",
                "error": err.to_string(),
            })
        }
    };

    collect_doctor_disk_and_permission_findings(paths, &mut findings);
    collect_doctor_os_finding(&mut findings);
    collect_doctor_sleep_finding(&mut findings);
    collect_doctor_subscription_binary_finding("claude", &mut findings);
    collect_doctor_subscription_binary_finding("codex", &mut findings);

    Ok(DoctorReport {
        source,
        config_present,
        sandboxes,
        seams,
        findings,
    })
}

fn doctor_verdict_surface(report: &DoctorReport) -> VerdictSurface {
    let failed_count = report
        .findings
        .iter()
        .filter(|finding| finding.is_failed())
        .count();
    let warning_count = report
        .findings
        .iter()
        .filter(|finding| finding.status == "warning")
        .count();
    let kind = if failed_count == 0 {
        VerdictKind::Verified
    } else {
        VerdictKind::Blocked
    };
    let primary = doctor_primary_action(report);
    let secondary = doctor_secondary_actions(report, &primary)
        .into_iter()
        .map(|action| ("Secondary", action))
        .collect::<Vec<_>>();
    VerdictSurface::must_new(
        kind,
        "doctor",
        None,
        doctor_explanation(report, failed_count, warning_count),
        vec![("Recommended", primary.as_str())],
        secondary,
    )
}

fn doctor_explanation(
    report: &DoctorReport,
    failed_count: usize,
    warning_count: usize,
) -> ExplanationPanel {
    let what = if failed_count == 0 {
        format!(
            "Doctor checked {} setup areas and found no blocking setup failures.",
            report.findings.len()
        )
    } else {
        format!(
            "Doctor checked {} setup areas and found {failed_count} blocking setup issue(s).",
            report.findings.len()
        )
    };
    let why = if failed_count == 0 {
        "All required checks passed; warnings, if any, are optional capabilities or local environment notes."
            .to_string()
    } else {
        format!(
            "The verdict is blocked because required setup checks failed; {warning_count} optional warning(s) were also recorded."
        )
    };
    let evidence = report
        .findings
        .iter()
        .map(|finding| {
            (
                finding.subject.clone(),
                format!("{} - {}", finding.status, finding.detail),
            )
        })
        .collect::<Vec<_>>();
    ExplanationPanel::new(what, why, evidence)
}

fn doctor_primary_action(report: &DoctorReport) -> String {
    report
        .findings
        .iter()
        .filter(|finding| finding.is_failed())
        .filter_map(|finding| finding.action.as_deref())
        .find(|action| action.starts_with("deadreckon "))
        .map(str::to_string)
        .unwrap_or_else(|| "deadreckon run \"goal\" --preview".to_string())
}

fn doctor_secondary_actions(report: &DoctorReport, primary: &str) -> Vec<String> {
    let mut actions = Vec::new();
    for action in report
        .findings
        .iter()
        .filter_map(|finding| finding.action.as_deref())
    {
        if action != primary && !actions.iter().any(|existing| existing == action) {
            actions.push(action.to_string());
        }
    }
    actions
}

fn doctor_json_payload(
    paths: &DeadreckonPaths,
    report: &DoctorReport,
    surface: &VerdictSurface,
) -> Value {
    let mut next_actions = vec![surface.primary_action.command.clone()];
    next_actions.extend(doctor_secondary_actions(
        report,
        &surface.primary_action.command,
    ));
    json!({
        "kind": "doctor",
        "id": &report.source,
        "status": surface.kind.as_str(),
        "next_actions": next_actions,
        "try_lines": Vec::<String>::new(),
        "paths": {
            "home": paths.home(),
            "config": paths.config_path(),
        },
        "source": &report.source,
        "home": paths.home(),
        "config_path": paths.config_path(),
        "config_present": report.config_present,
        "sandboxes": report.sandboxes,
        "seams": report.seams,
        "findings": report.findings,
    })
}

async fn collect_doctor_provider_findings(
    paths: &DeadreckonPaths,
    root: &toml::Value,
) -> Result<Vec<DoctorFinding>> {
    // A config without a [providers] table is legitimate: built-in registry
    // descriptors cover every default route (e.g. after `config
    // remove-provider` deletes the last override). Check the configured
    // default and fallback routes instead of failing a healthy setup.
    let empty = toml::map::Map::new();
    let owned_table;
    let providers = match root.get("providers").and_then(toml::Value::as_table) {
        Some(table) if !table.is_empty() => table,
        _ => {
            let mut synthesized = toml::map::Map::new();
            // Only the default route must be usable; fallback routes are
            // best-effort by design (the router skips credential-less ones),
            // so a keyless fallback entry must not block a healthy setup.
            let mut route_names = Vec::new();
            if let Some(name) = root.get("default_provider").and_then(toml::Value::as_str) {
                route_names.push(name.to_string());
            }
            if route_names.is_empty() {
                return Ok(vec![DoctorFinding::failed(
                    "providers",
                    "no providers configured",
                    Some("deadreckon init".to_string()),
                )]);
            }
            for name in route_names {
                synthesized.insert(name, toml::Value::Table(empty.clone()));
            }
            owned_table = synthesized;
            &owned_table
        }
    };
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let mut findings = Vec::new();
    for (name, entry) in providers {
        let kind = entry
            .get("kind")
            .and_then(toml::Value::as_str)
            .unwrap_or(name);
        let kind_label = registry
            .get(name)
            .map(|descriptor| descriptor_kind_label(&descriptor.kind).to_string())
            .unwrap_or_else(|| config_provider_kind_label(kind).to_string());
        let subject = format!("provider {name} kind={kind_label}");
        if kind.contains("cli") || name.starts_with("cli:") {
            let binary = entry
                .get("binary")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| {
                    if name.contains("claude") {
                        "claude"
                    } else {
                        "codex"
                    }
                });
            if command_exists(binary) || PathBuf::from(binary).exists() {
                // Presence says "installed", the auth probe says "usable":
                // surface installed-but-logged-out here instead of mid-run.
                let probe_status = registry
                    .get(name)
                    .and_then(|descriptor| descriptor.auth_probe.as_ref())
                    .map(|probe| (probe, probe_cli_auth(binary, probe)));
                match probe_status {
                    Some((_, CliAuthStatus::LoggedIn)) => {
                        findings.push(DoctorFinding::passed(
                            subject,
                            format!("CLI binary {binary} found; logged in"),
                            Some(format!(
                                "deadreckon run \"goal\" --provider {name} --preview"
                            )),
                        ));
                    }
                    Some((probe, CliAuthStatus::NotLoggedIn { detail })) => {
                        findings.push(DoctorFinding::failed(
                            subject,
                            format!("CLI binary {binary} installed but not logged in ({detail})"),
                            probe
                                .login_try_lines
                                .first()
                                .cloned()
                                .or_else(|| Some("deadreckon config provider".to_string())),
                        ));
                    }
                    _ => {
                        findings.push(DoctorFinding::passed(
                            subject,
                            format!("CLI binary {binary} found"),
                            Some(format!(
                                "deadreckon run \"goal\" --provider {name} --preview"
                            )),
                        ));
                    }
                }
            } else {
                findings.push(DoctorFinding::failed(
                    subject,
                    format!("CLI binary {binary} missing"),
                    Some("deadreckon config provider".to_string()),
                ));
            }
        } else if provider_has_key(entry) {
            if std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
                findings.push(collect_doctor_provider_ping(paths, name, &kind_label).await?);
            } else {
                findings.push(DoctorFinding::passed(
                    subject,
                    "credential present; ping skipped",
                    Some("DEADRECKON_DOCTOR_PING=1 deadreckon doctor".to_string()),
                ));
            }
        } else {
            findings.push(DoctorFinding::failed(
                subject,
                "credential missing",
                Some(format!(
                    "deadreckon config set providers.{name}.api_key <KEY>"
                )),
            ));
        }
    }
    Ok(findings)
}

async fn collect_doctor_provider_ping(
    paths: &DeadreckonPaths,
    name: &str,
    kind_label: &str,
) -> Result<DoctorFinding> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(name))?;
    let request = ProviderRequest {
        prompt: "Reply with OK only.".to_string(),
        max_output_tokens: 8,
        cwd: None,
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    let subject = format!("provider {name} kind={kind_label}");
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        router.complete(&request),
    )
    .await
    {
        Ok(Ok(response)) => Ok(DoctorFinding::passed(
            subject,
            format!("ping ok model {}", response.model),
            Some(format!(
                "deadreckon run \"goal\" --provider {name} --preview"
            )),
        )),
        Ok(Err(err)) => Ok(DoctorFinding::failed(
            subject,
            format!("ping failed: {err}"),
            Some("deadreckon config provider".to_string()),
        )),
        Err(_) => Ok(DoctorFinding::failed(
            subject,
            "ping timed out",
            Some("deadreckon config provider".to_string()),
        )),
    }
}

fn collect_doctor_disk_and_permission_findings(
    paths: &DeadreckonPaths,
    findings: &mut Vec<DoctorFinding>,
) {
    if let Err(err) = fs::create_dir_all(paths.runstate_dir()) {
        findings.push(DoctorFinding::failed(
            "runstate dir",
            format!("{} not writable: {err}", paths.runstate_dir().display()),
            None,
        ));
    } else {
        let probe = paths.runstate_dir().join(".doctor-write-test");
        match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
            Ok(()) => findings.push(DoctorFinding::passed(
                "runstate dir",
                format!("{} writable", paths.runstate_dir().display()),
                Some("deadreckon run \"goal\" --preview".to_string()),
            )),
            Err(err) => findings.push(DoctorFinding::failed(
                "runstate dir",
                format!("{} not writable: {err}", paths.runstate_dir().display()),
                None,
            )),
        }
    }

    match free_kb(paths.home()) {
        Some(kb) if kb < 1_048_576 => findings.push(DoctorFinding::failed(
            "disk space",
            format!("{} MB free in {}", kb / 1024, paths.home().display()),
            None,
        )),
        Some(kb) => findings.push(DoctorFinding::passed(
            "disk space",
            format!("{} MB free in {}", kb / 1024, paths.home().display()),
            Some("deadreckon status".to_string()),
        )),
        None => findings.push(DoctorFinding::warning(
            "disk space",
            format!("check unavailable for {}", paths.home().display()),
            None,
        )),
    }
}

fn collect_doctor_os_finding(findings: &mut Vec<DoctorFinding>) {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        findings.push(DoctorFinding::passed(
            "os",
            format!("macOS {version}"),
            None,
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        findings.push(DoctorFinding::passed(
            "os",
            format!("Linux kernel {version}"),
            None,
        ));
    }
}

fn collect_doctor_sleep_finding(findings: &mut Vec<DoctorFinding>) {
    let preview = sleep::preview(SleepPrefs::On, true);
    match preview.mode {
        sleep::SleepMode::Caffeinate | sleep::SleepMode::SystemdInhibit => {
            findings.push(DoctorFinding::passed(
                "sleep prevention",
                preview.label(),
                Some("deadreckon run \"goal\" --prevent-sleep auto".to_string()),
            ));
        }
        sleep::SleepMode::None => findings.push(DoctorFinding::warning(
            "sleep prevention",
            "disabled",
            Some("deadreckon run \"goal\" --prevent-sleep on".to_string()),
        )),
        sleep::SleepMode::Unsupported => findings.push(DoctorFinding::warning(
            "sleep prevention",
            "unsupported",
            Some("deadreckon run \"goal\" --prevent-sleep off".to_string()),
        )),
    }
}

fn collect_doctor_subscription_binary_finding(binary: &str, findings: &mut Vec<DoctorFinding>) {
    if command_exists(binary) {
        let provider = if binary == "claude" {
            "cli:claude-code"
        } else {
            "cli:codex"
        };
        findings.push(DoctorFinding::passed(
            format!("subscription binary {binary}"),
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string()),
            Some(format!("deadreckon config provider {provider}")),
        ));
    } else {
        findings.push(DoctorFinding::warning(
            format!("subscription binary {binary}"),
            "missing",
            Some("deadreckon config provider".to_string()),
        ));
    }
}

fn doctor_seam_resolution(paths: &DeadreckonPaths, no_seams: bool) -> Result<Vec<String>> {
    let seams = read_seams_config(&paths.config_path(), no_seams)?;
    Ok(SeamKind::all()
        .into_iter()
        .map(|kind| {
            let fail = seam_fail_policy_label(kind);
            match seams.command_for(kind) {
                Some(command) => format!(
                    "{}: external command={} timeout_ms={} fail={fail}",
                    kind.config_key(),
                    seam_command_basename(&command.command),
                    command.timeout_ms
                ),
                None => format!("{}: builtin fail={fail}", kind.config_key()),
            }
        })
        .collect())
}

fn seam_command_basename(command: &[String]) -> String {
    command
        .first()
        .and_then(|argv0| Path::new(argv0).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("-")
        .to_string()
}

fn config_provider_kind_label(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("cli") {
        "cli"
    } else if kind.contains("compatible") || kind.contains("local") {
        "local-http"
    } else if kind.contains("smoke") || kind.contains("script") {
        "scripted"
    } else if kind.contains("anthropic") || kind.contains("open-ai") || kind.contains("openai") {
        "http"
    } else {
        "custom"
    }
}

fn provider_has_key(entry: &toml::Value) -> bool {
    entry
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || entry
            .get("api_key_env")
            .and_then(toml::Value::as_str)
            .and_then(std::env::var_os)
            .is_some()
}

fn command_version(path: &std::path::Path) -> Option<String> {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            text.lines().next().unwrap_or_default().trim().to_string()
        })
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_lists_seam_resolution() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        std::fs::create_dir_all(paths.home()).expect("home");
        std::fs::write(
            paths.config_path(),
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"allow\"}'"]
timeout_ms = 1234
"#,
        )
        .expect("config");

        let lines = doctor_seam_resolution(&paths, false).expect("seams");

        assert!(lines.iter().any(|line| {
            line.contains("policy: external")
                && line.contains("command=sh")
                && line.contains("timeout_ms=1234")
                && line.contains("fail=closed")
        }));
        assert!(
            lines
                .iter()
                .any(|line| line == "catalog: builtin fail=open")
        );
        assert!(lines.iter().any(|line| line == "hooks: builtin fail=safe"));
        assert!(
            lines
                .iter()
                .any(|line| line == "event_sink: builtin fail=safe")
        );
    }

    #[tokio::test]
    async fn doctor_missing_config_surface_has_one_primary_action() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("repo");
        std::fs::create_dir_all(&source).expect("source");

        let report = build_doctor_report(&paths, source)
            .await
            .expect("doctor report");
        let rendered = doctor_verdict_surface(&report).render_plain(false);

        assert!(rendered.starts_with("blocked doctor"), "{rendered}");
        assert!(rendered.contains("Explanation\n"), "{rendered}");
        assert!(rendered.contains("Evidence\n"), "{rendered}");
        assert!(rendered.contains("config.toml missing"), "{rendered}");
        assert!(rendered.contains("runstate dir"), "{rendered}");
        assert!(rendered.contains("disk space"), "{rendered}");
        assert_eq!(rendered.matches("\nRecommended\n").count(), 1, "{rendered}");
        assert!(
            rendered.contains("Recommended\ndeadreckon init"),
            "{rendered}"
        );
        assert!(!rendered.contains("try:"), "{rendered}");
        assert!(!rendered.contains("fix:"), "{rendered}");
    }

    #[tokio::test]
    async fn doctor_json_adds_verdict_and_primary_action() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("repo");
        std::fs::create_dir_all(&source).expect("source");

        let report = build_doctor_report(&paths, source)
            .await
            .expect("doctor report");
        let surface = doctor_verdict_surface(&report);
        let value = surface.add_to_json(doctor_json_payload(&paths, &report, &surface));

        assert!(value["findings"].as_array().expect("findings").len() > 3);
        assert_eq!(value["primary_action"], "deadreckon init");
        assert_eq!(value["verdict"]["kind"], "blocked");
        assert_eq!(
            value["verdict"]["recommended_command"],
            value["primary_action"]
        );
        assert_eq!(value["next_actions"][0], value["primary_action"]);
    }
}
