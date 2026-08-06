use super::super::*;
use deadreckon_core::install_receipt::{Channel, detect_channel};
use deadreckon_providers::{
    ProviderCleanup, ProviderPhaseDeadline, ProviderPhaseOutcome, complete_provider_phase,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) async fn doctor_command(json_output: bool, live: bool, repair: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let repair_findings = if repair {
        perform_doctor_repairs(&paths)
    } else {
        Vec::new()
    };
    let source = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let report = build_doctor_report(&paths, source, true, live, repair_findings).await?;
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
    binary_health: DeadreckonBinaryHealth,
    findings: Vec<DoctorFinding>,
}

#[derive(Debug, Clone, Serialize)]
struct DeadreckonBinaryHealth {
    current_path: PathBuf,
    current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_selected: Option<PathBuf>,
    installations: Vec<DeadreckonBinaryInstallation>,
    conflicts: Vec<String>,
    advisories: Vec<String>,
    gate_helper_compatible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate_helper_path: Option<PathBuf>,
    gate_protocol_version: u32,
    bundle_build_id: String,
    repairable_receipt: bool,
    repairable_active_installation: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DeadreckonBinaryInstallation {
    canonical_path: PathBuf,
    locations: Vec<PathBuf>,
    roles: Vec<String>,
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_error: Option<String>,
    update_command: String,
}

#[derive(Debug, Default)]
struct BinaryCandidate {
    locations: BTreeSet<PathBuf>,
    roles: BTreeSet<String>,
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
    let report = build_doctor_report(&paths, source, false, false, Vec::new()).await?;
    Ok(DoctorSetupSummary::from(&report))
}

async fn build_doctor_report(
    paths: &DeadreckonPaths,
    source: PathBuf,
    include_supervisor: bool,
    live_provider_check: bool,
    mut repair_findings: Vec<DoctorFinding>,
) -> Result<DoctorReport> {
    let mut findings = vec![
        DoctorFinding::passed("source", source.display().to_string(), None),
        DoctorFinding::passed("home", paths.home().display().to_string(), None),
    ];
    findings.append(&mut repair_findings);

    let binary_health = inspect_deadreckon_binaries(paths)?;
    findings.push(binary_health_finding(&binary_health));

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
        findings.extend(collect_doctor_provider_findings(paths, root, live_provider_check).await?);
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
    if let Some(finding) = collect_doctor_semantic_reviewer_finding(paths, &defaults)? {
        findings.push(finding);
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
    if include_supervisor {
        // Configuration and provider failures stay ahead of service repair in
        // the one-primary-action surface. A user with no usable setup should
        // finish initialization before binding a durable supervisor to it.
        findings.push(supervisor_service_finding());
    }

    Ok(DoctorReport {
        source,
        config_present,
        sandboxes,
        seams,
        binary_health,
        findings,
    })
}

fn perform_doctor_repairs(paths: &DeadreckonPaths) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    match repair_active_shell_installation(paths) {
        Ok(detail) => findings.push(DoctorFinding::passed(
            "repair active installation",
            detail,
            None,
        )),
        Err(error) => findings.push(DoctorFinding::failed(
            "repair active installation",
            format!("left unchanged: {error}"),
            Some("invoke the intended DeadReckon binary explicitly, then run deadreckon doctor --repair".to_string()),
        )),
    }
    match super::providers::reconcile_receipt_for_current_binary(paths, false) {
        Ok(detail) => findings.push(DoctorFinding::passed(
            "repair install receipt",
            detail,
            None,
        )),
        Err(error) => findings.push(DoctorFinding::failed(
            "repair install receipt",
            format!("left unchanged: {error}"),
            Some("invoke the intended DeadReckon binary explicitly, then run deadreckon doctor --repair".to_string()),
        )),
    }
    match super::supervisor_service::repair_supervisor_service() {
        Ok(detail) => findings.push(DoctorFinding::passed(
            "repair supervisor service",
            detail,
            None,
        )),
        Err(error) => findings.push(DoctorFinding::failed(
            "repair supervisor service",
            format!("left unchanged: {error}"),
            Some("deadreckon supervisor status".to_string()),
        )),
    }
    findings
}

/// Reconcile the executable that a new shell would select with the binary the
/// operator explicitly invoked for `doctor --repair`.
///
/// Only the shell-installer-owned path is mutable here. Package-manager paths
/// remain under npm, Homebrew, or Cargo. On Unix the selected path becomes an
/// atomic symbolic-link alias, so the binary, adjacent helpers, receipt, and
/// supervisor all continue to resolve to one canonical installation.
fn repair_active_shell_installation(paths: &DeadreckonPaths) -> Result<String> {
    let health = inspect_deadreckon_binaries(paths)?;
    let Some(selected) = health.path_selected.as_deref() else {
        return Ok("PATH does not currently select a DeadReckon installation".to_string());
    };
    if same_binary_identity(&health.current_path, selected) {
        return Ok(format!(
            "PATH already resolves to the running binary through {}",
            selected.display()
        ));
    }
    if detect_channel(selected) != Channel::Shell || !is_managed_shell_binary_path(selected) {
        let channel = detect_channel(selected);
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "PATH selects a conflicting {} installation at {}; doctor will not overwrite a package-manager or user-owned executable",
                channel.as_str(),
                selected.display()
            ),
            super::providers::channel_native_update_command(channel),
        )));
    }
    let selected_metadata = fs::symlink_metadata(selected)?;
    if !selected_metadata.file_type().is_file() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "PATH-selected shell entry at {} is not a regular installer-owned binary; refusing to replace it",
                selected.display()
            ),
            "replace the custom alias yourself, then rerun deadreckon doctor",
        )));
    }

    let (selected_version, selected_probe_error) = probe_deadreckon_version(selected);
    if let Some(selected_version) = selected_version.as_deref()
        && version_is_newer(selected_version, &health.current_version)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "PATH selects newer DeadReckon {selected_version} at {}; refusing to replace it with the explicitly invoked {}",
                selected.display(),
                health.current_version
            ),
            &format!("{} doctor --repair", selected.display()),
        )));
    }
    if let Some(error) = selected_probe_error {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "PATH-selected DeadReckon at {} could not be verified before repair: {error}",
                selected.display()
            ),
            "repair or reinstall that shell-managed copy, then rerun deadreckon doctor --repair",
        )));
    }

    let backup = replace_shell_binary_with_alias(
        &health.current_path,
        selected,
        &paths.home().join("install-repair-backups"),
    )?;
    Ok(format!(
        "PATH now resolves {} to the explicitly invoked DeadReckon {} {}; previous binary backed up at {}",
        selected.display(),
        health.current_version,
        health.current_path.display(),
        backup.display()
    ))
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

fn is_managed_shell_binary_path(path: &Path) -> bool {
    let Some(user_home) = std::env::home_dir() else {
        return false;
    };
    #[cfg(windows)]
    let expected = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home.join("AppData/Local"))
        .join("deadreckon/bin")
        .join(deadreckon_binary_name());
    #[cfg(not(windows))]
    let expected = user_home
        .join(".local/share/deadreckon/bin")
        .join(deadreckon_binary_name());
    path == expected
}

fn repair_backup_dir(root: &Path) -> PathBuf {
    root.join(format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    ))
}

fn backup_binary(source: &Path, backup_root: &Path) -> Result<PathBuf> {
    let backup_dir = repair_backup_dir(backup_root);
    fs::create_dir_all(&backup_dir)?;
    let backup = backup_dir.join(deadreckon_binary_name());
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_file() {
        fs::copy(source, &backup)?;
        fs::set_permissions(&backup, metadata.permissions())?;
    } else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "PATH-selected DeadReckon is not a regular installer-owned file: {}",
            source.display()
        ))));
    }
    Ok(backup)
}

fn replace_shell_binary_with_alias(
    current: &Path,
    selected: &Path,
    backup_root: &Path,
) -> Result<PathBuf> {
    let current = std::fs::canonicalize(current)?;
    if !current.is_file() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "running DeadReckon binary is unavailable at {}",
            current.display()
        ))));
    }
    let parent = selected.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "PATH-selected DeadReckon has no parent directory: {}",
            selected.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let backup = backup_binary(selected, backup_root)?;
    let temporary = parent.join(format!(
        ".deadreckon-repair-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    create_binary_alias(&current, &temporary)?;
    if let Err(error) = replace_binary_alias(&temporary, selected) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(backup)
}

#[cfg(unix)]
fn create_binary_alias(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(windows)]
fn create_binary_alias(target: &Path, link: &Path) -> Result<()> {
    fs::copy(target, link)?;
    Ok(())
}

#[cfg(unix)]
fn replace_binary_alias(temporary: &Path, selected: &Path) -> Result<()> {
    // POSIX rename replaces the old directory entry atomically. The verified
    // backup remains available if the operator wants to undo the repair.
    fs::rename(temporary, selected)?;
    Ok(())
}

#[cfg(windows)]
fn replace_binary_alias(temporary: &Path, selected: &Path) -> Result<()> {
    let displaced = selected.with_extension("deadreckon-repair-old");
    if displaced.exists() {
        fs::remove_file(&displaced)?;
    }
    fs::rename(selected, &displaced)?;
    match fs::rename(temporary, selected) {
        Ok(()) => {
            fs::remove_file(displaced)?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(displaced, selected);
            Err(error.into())
        }
    }
}

fn inspect_deadreckon_binaries(paths: &DeadreckonPaths) -> Result<DeadreckonBinaryHealth> {
    let current_path = std::env::current_exe()?;
    let current_canonical = canonical_binary_path(&current_path);
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let mut candidates = BTreeMap::<PathBuf, BinaryCandidate>::new();
    add_binary_candidate(&mut candidates, &current_path, "current");

    let mut path_selected = None;
    if let Some(path_env) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_env) {
            let candidate = directory.join(deadreckon_binary_name());
            if candidate.is_file() {
                if path_selected.is_none() {
                    path_selected = Some(candidate.clone());
                    add_binary_candidate(&mut candidates, &candidate, "path-selected");
                }
                add_binary_candidate(&mut candidates, &candidate, "path");
            }
        }
    }

    if let Some(home) = std::env::home_dir() {
        for candidate in [
            home.join(".local/share/deadreckon/bin")
                .join(deadreckon_binary_name()),
            home.join(".local/bin").join(deadreckon_binary_name()),
            home.join(".cargo/bin").join(deadreckon_binary_name()),
        ] {
            add_binary_candidate(&mut candidates, &candidate, "known-install-location");
        }
    }
    #[cfg(unix)]
    for candidate in [
        PathBuf::from("/opt/homebrew/bin").join(deadreckon_binary_name()),
        PathBuf::from("/usr/local/bin").join(deadreckon_binary_name()),
    ] {
        add_binary_candidate(&mut candidates, &candidate, "known-install-location");
    }

    let mut conflicts = Vec::new();
    let mut advisories = Vec::new();
    let (gate_helper_compatible, gate_helper_path) =
        match super::job::inspect_compatible_host_gate() {
            Ok(path) => (true, Some(path)),
            Err(error) => {
                conflicts.push(format!("gate helper is incompatible: {error}"));
                (false, None)
            }
        };
    let mut repairable_receipt = false;
    match read_receipt(paths) {
        Ok(Some(receipt)) => {
            add_binary_candidate(&mut candidates, &receipt.binary_path, "install-receipt");
            let (conflict, repairable) = install_receipt_conflict(
                &receipt.binary_path,
                &receipt.channel_version,
                &current_path,
                &current_version,
            );
            repairable_receipt = repairable;
            if let Some(conflict) = conflict {
                conflicts.push(conflict);
            }
        }
        Ok(None) => {
            repairable_receipt = true;
            conflicts.push(format!(
                "install receipt is missing at {}",
                deadreckon_core::install_receipt::receipt_path(paths).display()
            ));
        }
        Err(error) => conflicts.push(format!("install receipt could not be read: {error}")),
    }

    match super::supervisor_service::supervisor_service_checkpoint_binary(paths) {
        Ok(Some(binary)) => add_binary_candidate(&mut candidates, &binary, "supervisor-checkpoint"),
        Ok(None) => {}
        Err(error) => conflicts.push(format!(
            "supervisor checkpoint binary could not be inspected: {error}"
        )),
    }

    let mut installations = Vec::new();
    for (canonical_path, candidate) in candidates {
        let is_current = canonical_path == current_canonical;
        let active = candidate_has_active_role(&candidate.roles);
        let (version, probe_error) = if is_current {
            (Some(current_version.clone()), None)
        } else {
            probe_deadreckon_version(&canonical_path)
        };
        if let Some(version) = version.as_deref()
            && version != current_version
        {
            let message = format!(
                "{} is version {version}; the running binary is version {current_version}",
                canonical_path.display()
            );
            if active {
                conflicts.push(message);
            } else {
                advisories.push(message);
            }
        }
        let channel = detect_channel(&canonical_path);
        installations.push(DeadreckonBinaryInstallation {
            sha256: deadreckon_core::flight::sha256_file(&canonical_path).ok(),
            canonical_path,
            locations: candidate.locations.into_iter().collect(),
            roles: candidate.roles.into_iter().collect(),
            channel: channel.as_str().to_string(),
            version,
            probe_error,
            update_command: super::providers::channel_native_update_command(channel).to_string(),
        });
    }

    if let Some(selected) = path_selected.as_ref()
        && !same_binary_identity(selected, &current_path)
    {
        conflicts.push(format!(
            "PATH selects {}, but this process is running {}",
            selected.display(),
            current_path.display()
        ));
    }
    conflicts.sort();
    conflicts.dedup();
    advisories.sort();
    advisories.dedup();
    let repairable_active_installation = path_selected.as_ref().is_some_and(|selected| {
        !same_binary_identity(selected, &current_path)
            && detect_channel(selected) == Channel::Shell
            && is_managed_shell_binary_path(selected)
    });

    Ok(DeadreckonBinaryHealth {
        current_path,
        current_version,
        path_selected,
        installations,
        conflicts,
        advisories,
        gate_helper_compatible,
        gate_helper_path,
        gate_protocol_version: deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION,
        bundle_build_id: env!("DEADRECKON_BUNDLE_BUILD_ID").to_string(),
        repairable_receipt,
        repairable_active_installation,
    })
}

fn candidate_has_active_role(roles: &BTreeSet<String>) -> bool {
    roles.iter().any(|role| {
        matches!(
            role.as_str(),
            "current" | "path-selected" | "install-receipt" | "supervisor-checkpoint"
        )
    })
}

fn same_binary_identity(left: &Path, right: &Path) -> bool {
    if canonical_binary_path(left) == canonical_binary_path(right) {
        return true;
    }
    match (
        deadreckon_core::flight::sha256_file(left),
        deadreckon_core::flight::sha256_file(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Classify receipt drift by executable identity and version, not path
/// spelling. Installer-managed aliases deliberately point at the canonical
/// binary recorded in the receipt.
fn install_receipt_conflict(
    receipt_path: &Path,
    receipt_version: &str,
    current_path: &Path,
    current_version: &str,
) -> (Option<String>, bool) {
    if !same_binary_identity(receipt_path, current_path) {
        return (
            Some(format!(
                "install receipt points at {}, not the running binary {}",
                receipt_path.display(),
                current_path.display()
            )),
            false,
        );
    }
    if receipt_version != current_version {
        return (
            Some(format!(
                "install receipt says {} at {}; the running binary is {} at {}",
                receipt_version,
                receipt_path.display(),
                current_version,
                current_path.display()
            )),
            true,
        );
    }
    (None, false)
}

fn add_binary_candidate(
    candidates: &mut BTreeMap<PathBuf, BinaryCandidate>,
    path: &Path,
    role: &str,
) {
    if !path.is_file() {
        return;
    }
    let canonical = canonical_binary_path(path);
    let candidate = candidates.entry(canonical).or_default();
    candidate.locations.insert(path.to_path_buf());
    candidate.roles.insert(role.to_string());
}

fn canonical_binary_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn deadreckon_binary_name() -> &'static str {
    if cfg!(windows) {
        "deadreckon.exe"
    } else {
        "deadreckon"
    }
}

fn probe_deadreckon_version(path: &Path) -> (Option<String>, Option<String>) {
    let mut child = match Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return (None, Some(error.to_string())),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => match child.wait_with_output() {
                Ok(output) => break output,
                Err(error) => return (None, Some(error.to_string())),
            },
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return (
                    None,
                    Some("version probe exceeded the 2 second limit".to_string()),
                );
            }
            Err(error) => return (None, Some(error.to_string())),
        }
    };
    if !output.status.success() {
        return (
            None,
            Some(format!("version probe exited with {}", output.status)),
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .split_whitespace()
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|first| first.is_ascii_digit())
        })
        .map(|part| part.trim_start_matches('v').to_string());
    match version {
        Some(version) => (Some(version), None),
        None => (
            None,
            Some(format!("unrecognized version output: {}", text.trim())),
        ),
    }
}

fn binary_health_finding(health: &DeadreckonBinaryHealth) -> DoctorFinding {
    let mut detail = if health.conflicts.is_empty() {
        format!(
            "{} installation(s) inspected; running {} from {}",
            health.installations.len(),
            health.current_version,
            health.current_path.display()
        )
    } else {
        format!(
            "{} installation(s) inspected; {} conflict(s): {}",
            health.installations.len(),
            health.conflicts.len(),
            health.conflicts.join("; ")
        )
    };
    if !health.advisories.is_empty() {
        detail.push_str(&format!(
            "; {} shadowed installation advisory(s): {}",
            health.advisories.len(),
            health.advisories.join("; ")
        ));
    }
    if !health.gate_helper_compatible {
        DoctorFinding::failed(
            "deadreckon binary bundle",
            detail,
            Some(
                super::providers::channel_native_update_command(detect_channel(
                    &health.current_path,
                ))
                .to_string(),
            ),
        )
    } else if health.conflicts.is_empty() {
        DoctorFinding::passed("deadreckon binaries", detail, None)
    } else {
        DoctorFinding::warning(
            "deadreckon binaries",
            detail,
            (health.repairable_receipt || health.repairable_active_installation)
                .then(|| "deadreckon doctor --repair".to_string()),
        )
    }
}

fn supervisor_service_finding() -> DoctorFinding {
    match super::supervisor_service::supervisor_service_preflight() {
        Ok(preflight) if preflight.is_ready() => DoctorFinding::passed(
            "supervisor service",
            "managed unit, service-manager state, and live checkpoint agree",
            Some("deadreckon supervisor status".to_string()),
        ),
        Ok(preflight) if preflight.setup_is_supported() => {
            let reason = preflight.reason().unwrap_or("setup is required");
            if reason.contains("is not installed") {
                DoctorFinding::warning(
                    "supervisor service",
                    format!("{reason}; durable start is unavailable until it is installed"),
                    Some("deadreckon doctor --repair".to_string()),
                )
            } else {
                DoctorFinding::failed(
                    "supervisor service",
                    reason,
                    Some("deadreckon doctor --repair".to_string()),
                )
            }
        }
        Ok(preflight) => DoctorFinding::failed(
            "supervisor service",
            preflight.reason().unwrap_or("readiness was refused"),
            Some("deadreckon supervisor status".to_string()),
        ),
        Err(error) => DoctorFinding::failed(
            "supervisor service",
            format!("readiness could not be inspected: {error}"),
            Some("deadreckon doctor --repair".to_string()),
        ),
    }
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
        "binary_health": report.binary_health,
        "findings": report.findings,
    })
}

async fn collect_doctor_provider_findings(
    paths: &DeadreckonPaths,
    root: &toml::Value,
    live: bool,
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
    for name in providers.keys() {
        let descriptor = registry.get(name);
        let kind_label = descriptor
            .map(|descriptor| descriptor_kind_label(&descriptor.kind).to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let subject = format!("provider {name} kind={kind_label}");
        let selection = setup::select_provider_setup(
            &paths.config_path(),
            &registry,
            setup::ProviderSetupRequest {
                role: setup::SetupProviderRoleRef::ConfigDefault,
                explicit_provider: Some(name),
                explicit_model: None,
                config_default_provider: None,
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: true,
            },
        );
        let selection = match selection {
            Ok(selection) => selection,
            Err(refusal) => {
                findings.push(DoctorFinding::failed(
                    subject,
                    refusal.message,
                    Some(refusal.try_line),
                ));
                continue;
            }
        };

        if live || std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
            findings.push(collect_doctor_provider_ping(paths, name, &kind_label).await?);
            continue;
        }

        if descriptor.is_some_and(|descriptor| descriptor.kind == DescriptorKind::Cli) {
            let binary = selection.binary.as_deref().unwrap_or("unknown");
            findings.push(DoctorFinding::passed(
                subject,
                format!(
                    "CLI binary {binary} found; static launch preflight passed; live model turn not tested"
                ),
                Some("deadreckon doctor --live".to_string()),
            ));
        } else {
            findings.push(DoctorFinding::passed(
                subject,
                "credential present; static setup passed; live model turn not tested",
                Some("deadreckon doctor --live".to_string()),
            ));
        }
    }
    Ok(findings)
}

fn collect_doctor_semantic_reviewer_finding(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
) -> Result<Option<DoctorFinding>> {
    let Some(worker) = defaults.provider.as_deref() else {
        return Ok(None);
    };
    let Ok(worker_route) = setup::route_info_for_provider(
        &paths.config_path(),
        Some(worker),
        defaults.model.as_deref(),
    ) else {
        return Ok(None);
    };
    let reviewer = if let Some(reviewer) = defaults.reviewer_provider.as_deref() {
        reviewer
    } else if worker_route
        .as_ref()
        .is_some_and(|route| route.kind.has_schema_only_adapter())
    {
        worker
    } else {
        return Ok(Some(DoctorFinding::failed(
            "semantic reviewer",
            format!(
                "default worker {worker} has no schema-only adapter and defaults.reviewer_provider is not configured"
            ),
            Some("deadreckon config set defaults.reviewer_provider cli:codex".to_string()),
        )));
    };
    let reviewer_model = if defaults.reviewer_provider.as_deref() == Some(reviewer) {
        defaults.reviewer_model.as_deref()
    } else {
        defaults.model.as_deref()
    };
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let selection = setup::select_provider_setup(
        &paths.config_path(),
        &registry,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::Reviewer,
            explicit_provider: Some(reviewer),
            explicit_model: reviewer_model,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: false,
            require_usable_route: true,
        },
    );
    if let Err(refusal) = selection {
        return Ok(Some(DoctorFinding::failed(
            "semantic reviewer",
            refusal.message,
            Some(refusal.try_line),
        )));
    }
    let route = match setup::route_info_for_provider(
        &paths.config_path(),
        Some(reviewer),
        reviewer_model,
    ) {
        Ok(Some(route)) => route,
        Ok(None) => {
            return Ok(Some(DoctorFinding::failed(
                "semantic reviewer",
                format!("semantic reviewer route {reviewer} did not resolve"),
                Some("deadreckon config set defaults.reviewer_provider cli:codex".to_string()),
            )));
        }
        Err(err) => {
            return Ok(Some(DoctorFinding::failed(
                "semantic reviewer",
                format!("semantic reviewer route {reviewer} is invalid: {err}"),
                Some("deadreckon config set defaults.reviewer_provider cli:codex".to_string()),
            )));
        }
    };
    if !route.kind.has_schema_only_adapter() {
        return Ok(Some(DoctorFinding::failed(
            "semantic reviewer",
            format!("provider route {reviewer} cannot perform schema-only semantic review"),
            Some("deadreckon config set defaults.reviewer_provider cli:codex".to_string()),
        )));
    }
    Ok(Some(DoctorFinding::passed(
        "semantic reviewer",
        format!("schema-only route {reviewer} configured for worker {worker}"),
        Some(format!(
            "deadreckon run \"goal\" --provider {worker} --reviewer-provider {reviewer} --preview"
        )),
    )))
}

async fn collect_doctor_provider_ping(
    paths: &DeadreckonPaths,
    name: &str,
    kind_label: &str,
) -> Result<DoctorFinding> {
    const LIVE_TOKEN: &str = "DEADRECKON_PROVIDER_OK";
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(name))?;
    let mut request = ProviderRequest::enforceably_read_only(
        format!("Reply exactly {LIVE_TOKEN} and nothing else."),
        32,
        std::env::current_dir()?,
    );
    request.pid_file = Some(std::env::temp_dir().join(format!(
        "deadreckon-doctor-provider-{}.pid",
        uuid::Uuid::new_v4().simple()
    )));
    let subject = format!("provider {name} kind={kind_label}");
    let deadline = ProviderPhaseDeadline::from_now(
        std::time::Duration::from_secs(20),
        std::time::Duration::from_secs(10),
    );
    match complete_provider_phase(&router, &mut request, deadline).await {
        ProviderPhaseOutcome::Completed(Ok(response)) if response.content.contains(LIVE_TOKEN) => {
            Ok(DoctorFinding::passed(
                subject,
                format!("live worker ping ok model {}", response.model),
                Some(format!(
                    "deadreckon run \"goal\" --provider {name} --preview"
                )),
            ))
        }
        ProviderPhaseOutcome::Completed(Ok(response)) => Ok(DoctorFinding::failed(
            subject,
            format!(
                "live worker ping returned without the required token (model {})",
                response.model
            ),
            Some("deadreckon config provider".to_string()),
        )),
        ProviderPhaseOutcome::Completed(Err(err)) => Ok(DoctorFinding::failed(
            subject,
            format!("ping failed: {err}"),
            Some("deadreckon config provider".to_string()),
        )),
        ProviderPhaseOutcome::WorkExpired { cleanup } => Ok(DoctorFinding::failed(
            subject,
            provider_ping_stop_detail("timed out", &cleanup),
            Some("deadreckon config provider".to_string()),
        )),
        ProviderPhaseOutcome::Cancelled { cleanup } => Ok(DoctorFinding::failed(
            subject,
            provider_ping_stop_detail("was cancelled", &cleanup),
            Some("deadreckon config provider".to_string()),
        )),
    }
}

fn provider_ping_stop_detail(reason: &str, cleanup: &ProviderCleanup) -> String {
    match cleanup {
        ProviderCleanup::Proven => format!("ping {reason}; provider cleanup proven"),
        ProviderCleanup::NotApplicable => format!("ping {reason}"),
        ProviderCleanup::RetainedAuthority { path, detail } => format!(
            "ping {reason}; provider cleanup was not proven ({detail}); process authority remains at {}",
            path.display()
        ),
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

    #[test]
    fn doctor_requires_a_schema_capable_reviewer_for_a_generic_default_worker() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let missing = ConfigDefaults {
            provider: Some("cli:copilot".to_string()),
            ..ConfigDefaults::default()
        };

        let finding = collect_doctor_semantic_reviewer_finding(&paths, &missing)
            .expect("finding")
            .expect("configured worker finding");
        assert_eq!(finding.status, "failed");
        assert!(finding.detail.contains("defaults.reviewer_provider"));

        let configured = ConfigDefaults {
            provider: Some("cli:copilot".to_string()),
            reviewer_provider: Some("smoke".to_string()),
            ..ConfigDefaults::default()
        };
        let finding = collect_doctor_semantic_reviewer_finding(&paths, &configured)
            .expect("finding")
            .expect("configured reviewer finding");
        assert_eq!(finding.status, "passed");
        assert!(finding.detail.contains("schema-only route smoke"));
    }

    #[tokio::test]
    async fn doctor_missing_config_surface_has_one_primary_action() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("repo");
        std::fs::create_dir_all(&source).expect("source");

        let report = build_doctor_report(&paths, source, false, false, Vec::new())
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

        let report = build_doctor_report(&paths, source, false, false, Vec::new())
            .await
            .expect("doctor report");
        let surface = doctor_verdict_surface(&report);
        let value = surface.add_to_json(doctor_json_payload(&paths, &report, &surface));

        assert!(value["findings"].as_array().expect("findings").len() > 3);
        assert!(value["binary_health"]["installations"].is_array());
        assert_eq!(value["primary_action"], "deadreckon init");
        assert_eq!(value["verdict"]["kind"], "blocked");
        assert_eq!(
            value["verdict"]["recommended_command"],
            value["primary_action"]
        );
        assert_eq!(value["next_actions"][0], value["primary_action"]);
    }

    #[test]
    fn only_authoritative_installation_roles_create_version_conflicts() {
        let shadowed = BTreeSet::from(["path".to_string(), "known-install-location".to_string()]);
        let selected = BTreeSet::from(["path".to_string(), "path-selected".to_string()]);
        let receipt = BTreeSet::from(["install-receipt".to_string()]);

        assert!(!candidate_has_active_role(&shadowed));
        assert!(candidate_has_active_role(&selected));
        assert!(candidate_has_active_role(&receipt));
    }

    #[test]
    fn install_repair_never_downgrades_a_newer_selected_version() {
        assert!(version_is_newer("0.8.0", "0.7.0"));
        assert!(!version_is_newer("0.7.0", "0.7.0"));
        assert!(!version_is_newer("0.6.9", "0.7.0"));
    }

    #[cfg(unix)]
    #[test]
    fn install_receipt_accepts_a_same_version_binary_alias() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().expect("temp");
        let receipt_target = temp.path().join("target/deadreckon");
        let running_alias = temp.path().join("bin/deadreckon");
        fs::create_dir_all(receipt_target.parent().expect("target parent")).expect("target parent");
        fs::create_dir_all(running_alias.parent().expect("alias parent")).expect("alias parent");
        fs::write(&receipt_target, b"same binary").expect("receipt target");
        symlink(&receipt_target, &running_alias).expect("running alias");

        let (conflict, repairable) =
            install_receipt_conflict(&receipt_target, "0.8.1", &running_alias, "0.8.1");
        assert_eq!(conflict, None);
        assert!(!repairable);
    }

    #[cfg(unix)]
    #[test]
    fn install_receipt_still_flags_stale_versions_and_different_binaries() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().expect("temp");
        let receipt_target = temp.path().join("target/deadreckon");
        let running_alias = temp.path().join("bin/deadreckon");
        let different = temp.path().join("other/deadreckon");
        for path in [&receipt_target, &running_alias, &different] {
            fs::create_dir_all(path.parent().expect("binary parent")).expect("binary parent");
        }
        fs::write(&receipt_target, b"receipt binary").expect("receipt target");
        fs::write(&different, b"different binary").expect("different binary");
        symlink(&receipt_target, &running_alias).expect("running alias");

        let (stale_conflict, stale_repairable) =
            install_receipt_conflict(&receipt_target, "0.8.0", &running_alias, "0.8.1");
        assert!(
            stale_conflict
                .as_deref()
                .is_some_and(|conflict| conflict.contains("install receipt says 0.8.0"))
        );
        assert!(stale_repairable);

        let (different_conflict, different_repairable) =
            install_receipt_conflict(&different, "0.8.1", &running_alias, "0.8.1");
        assert!(
            different_conflict
                .as_deref()
                .is_some_and(|conflict| conflict.contains("not the running binary"))
        );
        assert!(!different_repairable);
    }

    #[cfg(unix)]
    #[test]
    fn shell_install_repair_backs_up_and_atomically_aliases_running_binary() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::TempDir::new().expect("temp");
        let current = temp.path().join("source/deadreckon");
        let selected = temp.path().join("shell/bin/deadreckon");
        fs::create_dir_all(current.parent().expect("current parent")).expect("current parent");
        fs::create_dir_all(selected.parent().expect("selected parent")).expect("selected parent");
        fs::write(&current, b"new binary").expect("current");
        fs::write(&selected, b"old binary").expect("selected");
        fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).expect("current mode");
        fs::set_permissions(&selected, fs::Permissions::from_mode(0o755)).expect("selected mode");

        let backup = replace_shell_binary_with_alias(
            &current,
            &selected,
            &temp.path().join("repair-backups"),
        )
        .expect("repair");

        assert_eq!(fs::read(&backup).expect("backup bytes"), b"old binary");
        assert!(
            fs::symlink_metadata(&selected)
                .expect("selected metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::canonicalize(&selected).expect("selected canonical"),
            fs::canonicalize(&current).expect("current canonical")
        );
        assert!(same_binary_identity(&current, &selected));
    }

    #[cfg(unix)]
    #[test]
    fn shell_install_repair_refuses_an_existing_custom_alias() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().expect("temp");
        let current = temp.path().join("current-deadreckon");
        let custom = temp.path().join("custom-deadreckon");
        let selected = temp.path().join("bin/deadreckon");
        fs::create_dir_all(selected.parent().expect("selected parent")).expect("selected parent");
        fs::write(&current, b"current").expect("current");
        fs::write(&custom, b"custom").expect("custom");
        symlink(&custom, &selected).expect("custom alias");

        let error = replace_shell_binary_with_alias(
            &current,
            &selected,
            &temp.path().join("repair-backups"),
        )
        .expect_err("custom alias must be refused");

        assert!(
            error
                .to_string()
                .contains("not a regular installer-owned file")
        );
        assert_eq!(fs::read_link(&selected).expect("alias retained"), custom);
    }
}
