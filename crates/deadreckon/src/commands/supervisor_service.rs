//! Per-user service installation for the durable local supervisor.
//!
//! Installation and lifecycle changes are always explicit CLI operations.
//! Rendered units pin the current executable and durable-state home so a
//! machine restart cannot silently select a different DeadReckon binary or
//! state directory.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use chrono::{DateTime, Utc};
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, LockGuard, acquire_lock, boot_identity,
    process_start_identity,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::{CliError, Result};

const LAUNCHD_LABEL: &str = "com.deadreckon.supervisor";
const SYSTEMD_UNIT: &str = "deadreckon-supervisor.service";
const MANAGED_MARKER: &str = "deadreckon-managed-supervisor-v1";
const SERVICE_INSTANCE_SCHEMA: u32 = 2;
const SERVICE_LOCK_SCOPE: &str = "watchkeeper";
const SERVICE_LOCK_TASK: &str = "supervisor-service";
const SERVICE_LOCK_OWNER: &str = "durable-supervisor-service";
const SERVICE_INSTANCE_DIR: &str = "supervisor";
const SERVICE_INSTANCE_FILE: &str = "instance.json";
const SERVICE_STATUS_SCHEMA: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Both variants are rendered and tested on each supported host.
enum ServicePlatform {
    Launchd,
    Systemd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ServiceContext {
    platform: ServicePlatform,
    binary: PathBuf,
    deadreckon_home: PathBuf,
    user_home: PathBuf,
    xdg_config_home: Option<PathBuf>,
    path_env: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallDecision {
    Create,
    Unchanged,
    ReplaceManaged,
    RefuseUnmanaged,
}

/// Installation posture only. `InstalledCurrent` does not assert that the
/// user's service manager has loaded or enabled the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MachineRestartPosture {
    Unsupported,
    NotInstalled,
    UnmanagedUnit,
    StaleManagedUnit,
    InstalledCurrent,
}

/// Admission decision for the ordinary durable `start` surface.
///
/// Installation posture alone is insufficient: a current unit that is stopped
/// (or a systemd unit that is active but disabled) will not recover a Job after
/// a machine restart. `Ready` therefore means the managed unit, manager runtime,
/// and live instance checkpoint agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SupervisorServicePreflight {
    Ready {
        instance: SupervisorServiceInstance,
    },
    SetupRequired {
        reason: String,
        last_instance: Option<SupervisorServiceInstance>,
    },
    Refused {
        reason: String,
    },
}

impl SupervisorServicePreflight {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub(crate) fn setup_is_supported(&self) -> bool {
        matches!(self, Self::SetupRequired { .. })
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Ready { .. } => None,
            Self::SetupRequired { reason, .. } | Self::Refused { reason } => Some(reason),
        }
    }

    /// Return the last validated checkpoint identity, including for a stopped
    /// service. Callers that explicitly restart the service can snapshot this
    /// value and require a fresh successor before launching a Job.
    pub(crate) fn instance(&self) -> Option<&SupervisorServiceInstance> {
        match self {
            Self::Ready { instance } => Some(instance),
            Self::SetupRequired { last_instance, .. } => last_instance.as_ref(),
            Self::Refused { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn ready_for_test() -> Self {
        let pid = std::process::id();
        Self::Ready {
            instance: SupervisorServiceInstance {
                generation: 1,
                instance_id: "supervisor-test-instance".to_string(),
                boot_id: "supervisor-test-boot".to_string(),
                pid,
                process_start_identity: process_start_identity(pid)
                    .unwrap_or_else(|| "supervisor-test-process-start".to_string()),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SupervisorServiceInstance {
    generation: u64,
    instance_id: String,
    boot_id: String,
    pid: u32,
    process_start_identity: String,
}

impl SupervisorServiceInstance {
    /// A service start is observed only when both the durable generation and
    /// the process identity move forward. Generation alone is writable state;
    /// PID alone is reusable after exit.
    pub(crate) fn is_fresh_successor_of(&self, prior: &Self) -> bool {
        self.generation > prior.generation
            && self.instance_id != prior.instance_id
            && (self.pid, self.process_start_identity.as_str())
                != (prior.pid, prior.process_start_identity.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceInstanceCheckpoint {
    schema_version: u32,
    generation: u64,
    instance_id: String,
    boot_id: String,
    pid: u32,
    #[serde(default)]
    process_start_identity: String,
    started_at: DateTime<Utc>,
    binary: PathBuf,
    deadreckon_home: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ServiceInstanceCheckpointHeader {
    schema_version: u32,
    generation: u64,
}

impl From<&ServiceInstanceCheckpoint> for SupervisorServiceInstance {
    fn from(checkpoint: &ServiceInstanceCheckpoint) -> Self {
        Self {
            generation: checkpoint.generation,
            instance_id: checkpoint.instance_id.clone(),
            boot_id: checkpoint.boot_id.clone(),
            pid: checkpoint.pid,
            process_start_identity: checkpoint.process_start_identity.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ServiceManager {
    Launchd,
    Systemd,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ServiceInstallationState {
    Unsupported,
    NotInstalled,
    Stale,
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BootIdentitySource {
    TestOverride,
    LinuxProcfs,
    MacosSysctl,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BootIdentityObservation {
    current_boot_id: String,
    source: BootIdentitySource,
    test_override: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ServiceManagerRuntime {
    loaded: Option<bool>,
    enabled: Option<String>,
    active: Option<String>,
}

impl ServiceManagerRuntime {
    fn is_running(&self) -> bool {
        self.loaded == Some(true) || self.active.as_deref() == Some("active")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SupervisorServiceStatusReport {
    schema_version: u32,
    manager: ServiceManager,
    installed: ServiceInstallationState,
    loaded: Option<bool>,
    enabled: Option<String>,
    active: Option<String>,
    checkpoint: Option<ServiceInstanceCheckpoint>,
    current_boot_id: String,
    boot_identity_source: BootIdentitySource,
    /// Live evidence consumers must refuse reports produced with the test-only
    /// `DEADRECKON_BOOT_ID` override.
    test_override: bool,
}

/// Fail-closed admission rule for any consumer turning status JSON into a
/// live machine-reboot claim. Status itself still renders these fields so an
/// operator can diagnose the environment without accidentally accepting it.
#[allow(dead_code)] // The live-trial consumer is intentionally wired separately.
pub(crate) fn validate_supervisor_service_live_evidence(
    report: &SupervisorServiceStatusReport,
) -> Result<()> {
    if report.schema_version != SERVICE_STATUS_SCHEMA {
        return Err(invalid_input(format!(
            "unsupported supervisor status schema {}; expected {}",
            report.schema_version, SERVICE_STATUS_SCHEMA
        )));
    }
    if report.test_override || report.boot_identity_source == BootIdentitySource::TestOverride {
        return Err(invalid_input(
            "supervisor status used the test-only boot identity override and cannot prove a live reboot",
        ));
    }
    if report.boot_identity_source == BootIdentitySource::Unknown {
        return Err(invalid_input(
            "supervisor status has no authoritative boot identity source and cannot prove a live reboot",
        ));
    }
    validate_service_value("current supervisor boot id", &report.current_boot_id)?;
    if report.installed != ServiceInstallationState::Current {
        return Err(invalid_input(
            "supervisor status does not describe a current managed service unit",
        ));
    }
    let manager_ready = match report.manager {
        ServiceManager::Launchd => {
            report.loaded == Some(true) && report.enabled.as_deref() == Some("enabled")
        }
        ServiceManager::Systemd => {
            report.active.as_deref() == Some("active")
                && report.enabled.as_deref() == Some("enabled")
        }
        ServiceManager::Unsupported => false,
    };
    if !manager_ready {
        return Err(invalid_input(
            "supervisor status does not prove an active, restart-enabled service",
        ));
    }
    let checkpoint = report
        .checkpoint
        .as_ref()
        .ok_or_else(|| invalid_input("supervisor status has no live instance checkpoint"))?;
    validate_service_instance_checkpoint(checkpoint)?;
    if checkpoint.boot_id != report.current_boot_id {
        return Err(invalid_input(format!(
            "supervisor checkpoint boot {} does not match current boot {}",
            checkpoint.boot_id, report.current_boot_id
        )));
    }
    match process_start_identity(checkpoint.pid) {
        Some(observed) if observed == checkpoint.process_start_identity => Ok(()),
        Some(_) => Err(invalid_input(
            "supervisor checkpoint process-start identity does not match its live PID",
        )),
        None => Err(invalid_input(
            "supervisor checkpoint PID is not a live observable process",
        )),
    }
}

/// A process-held singleton paired with a durable, replaceable checkpoint.
///
/// The lock owner is intentionally stable across process and machine restarts:
/// the OS file lock is the singleton authority. This avoids a stale PID reused
/// after reboot preventing the service from coming back. The checkpoint keeps
/// the changing instance/boot identity for operator evidence.
#[derive(Debug)]
pub(crate) struct SupervisorServiceGuard {
    _lock: LockGuard,
    #[cfg_attr(not(test), allow(dead_code))]
    checkpoint: ServiceInstanceCheckpoint,
}

pub(crate) fn acquire_supervisor_service_guard(
    paths: &DeadreckonPaths,
    instance_id: &str,
    boot_id: &str,
    binary: &Path,
) -> Result<SupervisorServiceGuard> {
    validate_service_value("supervisor instance id", instance_id)?;
    validate_service_value("supervisor boot id", boot_id)?;
    let lock = acquire_lock(
        paths,
        SERVICE_LOCK_TASK,
        SERVICE_LOCK_OWNER,
        SERVICE_LOCK_SCOPE,
        "serving",
        Duration::from_secs(30),
    )?;
    let checkpoint_path = service_instance_checkpoint_path(paths);
    let generation = match read_service_instance_header(&checkpoint_path)? {
        Some(prior) => prior.generation.checked_add(1).ok_or_else(|| {
            invalid_input("supervisor checkpoint generation cannot advance beyond u64::MAX")
        })?,
        None => 1,
    };
    let pid = std::process::id();
    let process_start_identity = process_start_identity(pid).ok_or_else(|| {
        invalid_input(format!(
            "could not establish process-start identity for supervisor pid {pid}"
        ))
    })?;
    let checkpoint = ServiceInstanceCheckpoint {
        schema_version: SERVICE_INSTANCE_SCHEMA,
        generation,
        instance_id: instance_id.to_string(),
        boot_id: boot_id.to_string(),
        pid,
        process_start_identity,
        started_at: Utc::now(),
        binary: binary.to_path_buf(),
        deadreckon_home: paths.home().to_path_buf(),
    };
    fs::create_dir_all(
        checkpoint_path
            .parent()
            .ok_or_else(|| invalid_input("supervisor checkpoint path has no parent"))?,
    )?;
    let bytes = serde_json::to_vec_pretty(&checkpoint).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: checkpoint_path.clone(),
            source,
        })
    })?;
    write_unit_atomically(&checkpoint_path, &bytes)?;
    Ok(SupervisorServiceGuard {
        _lock: lock,
        checkpoint,
    })
}

pub(crate) fn supervisor_machine_restart_posture() -> Result<MachineRestartPosture> {
    let Ok(platform) = service_platform_for_os(std::env::consts::OS) else {
        return Ok(MachineRestartPosture::Unsupported);
    };
    let context = ServiceContext::discover_for_platform(platform)?;
    machine_restart_posture(&context)
}

/// Prove that the per-user service can recover an ordinary durable Job after a
/// machine restart. This is deliberately stronger than the label-only posture
/// used by preview and status output.
pub(crate) fn supervisor_service_preflight() -> Result<SupervisorServicePreflight> {
    let Ok(platform) = service_platform_for_os(std::env::consts::OS) else {
        return Ok(SupervisorServicePreflight::Refused {
            reason: "machine-restart supervision is unsupported on this platform (macOS launchd and Linux systemd are supported)".to_string(),
        });
    };
    let context = ServiceContext::discover_for_platform(platform)?;
    let posture = machine_restart_posture(&context)?;
    if posture == MachineRestartPosture::UnmanagedUnit {
        return Ok(SupervisorServicePreflight::Refused {
            reason: format!(
                "an unmanaged service unit occupies {}",
                context.unit_path().display()
            ),
        });
    }
    let paths = DeadreckonPaths::from_home(context.deadreckon_home.clone());
    let checkpoint_path = service_instance_checkpoint_path(&paths);
    if read_service_instance_header(&checkpoint_path)?
        .is_some_and(|header| header.schema_version == 1)
    {
        return Ok(SupervisorServicePreflight::SetupRequired {
            reason: "the supervisor has a legacy checkpoint without live process identity; restart it to publish a v2 checkpoint".to_string(),
            last_instance: None,
        });
    }
    let checkpoint = read_service_instance_checkpoint(&checkpoint_path)?;
    let last_instance = checkpoint.as_ref().map(SupervisorServiceInstance::from);
    match posture {
        MachineRestartPosture::Unsupported => Ok(SupervisorServicePreflight::Refused {
            reason: "machine-restart supervision is unsupported on this platform".to_string(),
        }),
        MachineRestartPosture::UnmanagedUnit => unreachable!("handled before checkpoint reads"),
        MachineRestartPosture::NotInstalled => {
            Ok(SupervisorServicePreflight::SetupRequired {
                reason: "the DeadReckon supervisor service is not installed".to_string(),
                last_instance,
            })
        }
        MachineRestartPosture::StaleManagedUnit => {
            Ok(SupervisorServicePreflight::SetupRequired {
                reason: "the installed DeadReckon supervisor service points at a different binary or state directory".to_string(),
                last_instance,
            })
        }
        MachineRestartPosture::InstalledCurrent => {
            let runtime = query_service_manager_runtime(&context)?;
            if runtime.is_running() && checkpoint.is_none() {
                return Ok(SupervisorServicePreflight::SetupRequired {
                    reason: "the service manager reports the supervisor running, but its live instance checkpoint is absent; restart it to publish one".to_string(),
                    last_instance: None,
                });
            }
            let report = match build_service_status_report(
                &context,
                posture,
                runtime,
                checkpoint,
                current_boot_identity(),
            ) {
                Ok(report) => report,
                Err(error) => {
                    return Ok(SupervisorServicePreflight::SetupRequired {
                        reason: format!(
                            "the current supervisor service has no valid live instance identity: {error}"
                        ),
                        last_instance,
                    });
                }
            };
            Ok(service_preflight_from_status(&report))
        }
    }
}

/// Return the binary recorded by the last supervisor checkpoint. This is
/// diagnostic evidence only: readiness still comes from the stronger service
/// preflight, which also checks the managed unit and live process identity.
pub(crate) fn supervisor_service_checkpoint_binary(
    paths: &DeadreckonPaths,
) -> Result<Option<PathBuf>> {
    Ok(
        read_service_instance_checkpoint(&service_instance_checkpoint_path(paths))?
            .map(|checkpoint| checkpoint.binary),
    )
}

/// Reconcile a DeadReckon-managed service to the binary executing this command
/// and wait until the service manager plus live checkpoint prove readiness.
/// Unmanaged units remain a hard boundary and are never overwritten.
pub(crate) fn repair_supervisor_service() -> Result<String> {
    let before = supervisor_service_preflight()?;
    if before.is_ready() {
        return Ok("managed supervisor already matches this binary and is ready".to_string());
    }
    if !before.setup_is_supported() {
        return Err(invalid_input(before.reason().unwrap_or(
            "the supervisor service cannot be repaired on this platform",
        )));
    }

    supervisor_service_install_command()?;
    supervisor_service_start_command()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match supervisor_service_preflight() {
            Ok(ready) if ready.is_ready() => {
                return Ok(
                    "managed supervisor now matches this binary and is restart-ready".to_string(),
                );
            }
            Ok(not_ready) if std::time::Instant::now() >= deadline => {
                return Err(invalid_input(format!(
                    "supervisor repair did not become ready: {}",
                    not_ready.reason().unwrap_or("readiness was not proven")
                )));
            }
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            _ => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

fn service_preflight_from_status(
    report: &SupervisorServiceStatusReport,
) -> SupervisorServicePreflight {
    if report.installed != ServiceInstallationState::Current {
        return SupervisorServicePreflight::SetupRequired {
            reason: "the DeadReckon supervisor service is not current".to_string(),
            last_instance: report
                .checkpoint
                .as_ref()
                .map(SupervisorServiceInstance::from),
        };
    }
    let manager_ready = match report.manager {
        ServiceManager::Launchd => {
            report.loaded == Some(true) && report.enabled.as_deref() == Some("enabled")
        }
        ServiceManager::Systemd => {
            report.enabled.as_deref() == Some("enabled")
                && report.active.as_deref() == Some("active")
        }
        ServiceManager::Unsupported => false,
    };
    if manager_ready && let Some(checkpoint) = report.checkpoint.as_ref() {
        SupervisorServicePreflight::Ready {
            instance: SupervisorServiceInstance::from(checkpoint),
        }
    } else {
        let reason = match report.manager {
            ServiceManager::Launchd => {
                "the DeadReckon launchd supervisor is not both loaded and enabled with a live checkpoint"
            }
            ServiceManager::Systemd => {
                "the DeadReckon systemd supervisor is not both active and enabled with a live checkpoint"
            }
            ServiceManager::Unsupported => {
                "machine-restart supervision is unsupported on this platform"
            }
        };
        SupervisorServicePreflight::SetupRequired {
            reason: reason.to_string(),
            last_instance: report
                .checkpoint
                .as_ref()
                .map(SupervisorServiceInstance::from),
        }
    }
}

/// Operator-facing durability claims for ordinary Job commands.
///
/// A detached one-shot supervisor is always launched for a successful guided
/// start. A unit file only proves machine-restart configuration; it does not
/// prove that launchd/systemd currently has the service loaded or active.
pub(crate) fn guided_durability_labels() -> (&'static str, String) {
    let process = "detached supervisor continues after the launching shell exits";
    let machine = match supervisor_machine_restart_posture() {
        Ok(posture) => machine_restart_posture_label(posture).to_string(),
        Err(error) => format!(
            "unknown (service posture could not be read: {error}); check with `deadreckon supervisor status`"
        ),
    };
    (process, machine)
}

fn machine_restart_posture_label(posture: MachineRestartPosture) -> &'static str {
    match posture {
        MachineRestartPosture::InstalledCurrent => {
            "configured by the current service unit; runtime state is reported by `deadreckon supervisor status`"
        }
        MachineRestartPosture::NotInstalled => {
            "not configured; run `deadreckon setup --supervisor`"
        }
        MachineRestartPosture::StaleManagedUnit => {
            "not configured for this binary; update with `deadreckon setup --supervisor`"
        }
        MachineRestartPosture::UnmanagedUnit => {
            "not configured by DeadReckon; an unmanaged unit occupies the service path"
        }
        MachineRestartPosture::Unsupported => {
            "unsupported on this platform (macOS launchd and Linux systemd are supported)"
        }
    }
}

pub(crate) fn supervisor_requires_service_singleton(
    once: bool,
    requested_job_id: Option<&str>,
) -> bool {
    !(once && requested_job_id.is_some())
}

pub(crate) fn supervisor_service_install_command() -> Result<()> {
    let context = ServiceContext::discover()?;
    let unit_path = context.unit_path();
    let rendered = render_service(&context)?;
    fs::create_dir_all(context.log_dir())?;
    fs::create_dir_all(unit_path.parent().ok_or_else(|| {
        invalid_input(format!(
            "supervisor service path has no parent: {}",
            unit_path.display()
        ))
    })?)?;

    let outcome = install_rendered_unit(&context, &rendered)?;
    match context.platform {
        ServicePlatform::Launchd if outcome == InstallDecision::Create => {
            let target = format!("{}/{LAUNCHD_LABEL}", launchd_domain()?);
            run_required(
                "launchctl",
                ["disable", target.as_str()],
                "leave the newly installed supervisor stopped until explicit start",
            )?;
        }
        ServicePlatform::Systemd => {
            run_required(
                "systemctl",
                ["--user", "daemon-reload"],
                "reload the per-user systemd manager",
            )?;
        }
        ServicePlatform::Launchd => {}
    }

    let action = match outcome {
        InstallDecision::Create => "installed",
        InstallDecision::Unchanged => "already installed",
        InstallDecision::ReplaceManaged => "updated",
        InstallDecision::RefuseUnmanaged => {
            return Err(unmanaged_conflict_error(&unit_path));
        }
    };
    println!(
        "supervisor service {action}: {}\nnext: deadreckon supervisor start",
        unit_path.display()
    );
    Ok(())
}

pub(crate) fn supervisor_service_start_command() -> Result<()> {
    let context = ServiceContext::discover()?;
    require_current_managed_unit(&context)?;
    match context.platform {
        ServicePlatform::Launchd => launchd_start(&context)?,
        ServicePlatform::Systemd => {
            run_required(
                "systemctl",
                ["--user", "daemon-reload"],
                "reload the per-user systemd manager",
            )?;
            run_required(
                "systemctl",
                ["--user", "enable", SYSTEMD_UNIT],
                "enable the DeadReckon supervisor",
            )?;
            run_required(
                "systemctl",
                ["--user", "restart", SYSTEMD_UNIT],
                "start the current DeadReckon supervisor unit",
            )?;
        }
    }
    println!(
        "supervisor service started\nstate: {}\nbinary: {}",
        context.deadreckon_home.display(),
        context.binary.display()
    );
    Ok(())
}

pub(crate) fn supervisor_service_status_command(json: bool) -> Result<()> {
    let posture = supervisor_machine_restart_posture()?;
    if posture == MachineRestartPosture::Unsupported {
        if json {
            let report = unsupported_service_status_report(current_boot_identity());
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        println!(
            "supervisor service: unsupported\nmachine-restart durability is available only on macOS launchd and Linux systemd"
        );
        return Ok(());
    }
    let context = ServiceContext::discover()?;
    let unit_path = context.unit_path();
    let installed = match posture {
        MachineRestartPosture::NotInstalled => {
            if json {
                let report = build_service_status_report(
                    &context,
                    posture,
                    ServiceManagerRuntime::default(),
                    read_service_instance_checkpoint(&service_instance_checkpoint_path(
                        &DeadreckonPaths::from_home(context.deadreckon_home.clone()),
                    ))?,
                    current_boot_identity(),
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            println!("supervisor service: not installed\nnext: deadreckon supervisor install");
            return Ok(());
        }
        MachineRestartPosture::UnmanagedUnit => {
            return Err(unmanaged_conflict_error(&unit_path));
        }
        MachineRestartPosture::StaleManagedUnit => "stale (run `deadreckon supervisor install`)",
        MachineRestartPosture::InstalledCurrent => "current",
        MachineRestartPosture::Unsupported => unreachable!("handled above"),
    };

    let runtime = query_service_manager_runtime(&context)?;
    if json {
        let paths = DeadreckonPaths::from_home(context.deadreckon_home.clone());
        let report = build_service_status_report(
            &context,
            posture,
            runtime,
            read_service_instance_checkpoint(&service_instance_checkpoint_path(&paths))?,
            current_boot_identity(),
        )?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{}",
        render_human_service_status(&context, installed, &runtime)
    );
    Ok(())
}

fn query_service_manager_runtime(context: &ServiceContext) -> Result<ServiceManagerRuntime> {
    match context.platform {
        ServicePlatform::Launchd => {
            let domain = launchd_domain()?;
            let target = format!("{domain}/{LAUNCHD_LABEL}");
            let loaded = run_observational(
                "launchctl",
                ["print", target.as_str()],
                "read the per-user launchd service state",
            )?;
            let enabled = run_observational(
                "launchctl",
                ["print-disabled", domain.as_str()],
                "read the per-user launchd enablement state",
            )?;
            launchd_manager_runtime(&loaded, &enabled)
        }
        ServicePlatform::Systemd => {
            let active = run_observational(
                "systemctl",
                ["--user", "is-active", SYSTEMD_UNIT],
                "read the per-user systemd service state",
            )?;
            let enabled = run_observational(
                "systemctl",
                ["--user", "is-enabled", SYSTEMD_UNIT],
                "read the per-user systemd enablement state",
            )?;
            systemd_manager_runtime(&active, &enabled)
        }
    }
}

fn launchd_manager_runtime(loaded: &Output, disabled: &Output) -> Result<ServiceManagerRuntime> {
    refuse_unavailable_manager("read the per-user launchd service state", loaded)?;
    refuse_unavailable_manager("read the per-user launchd enablement state", disabled)?;
    if !loaded.status.success() && !manager_reports_absent(loaded) {
        return Err(command_failure(
            "read the per-user launchd service state",
            loaded,
        ));
    }
    if !disabled.status.success() {
        return Err(command_failure(
            "read the per-user launchd enablement state",
            disabled,
        ));
    }
    Ok(ServiceManagerRuntime {
        loaded: Some(loaded.status.success()),
        enabled: Some(parse_launchd_enabled_state(&String::from_utf8_lossy(
            &disabled.stdout,
        ))?),
        active: None,
    })
}

/// `launchctl print-disabled <domain>` has emitted both boolean and textual
/// override values across macOS releases. `true`/`disabled` mean disabled;
/// `false`/`enabled` mean enabled. Absence is not accepted as enabled because
/// the ordinary `start` promise needs affirmative persistence evidence, not
/// launchd's implicit default.
fn parse_launchd_enabled_state(output: &str) -> Result<String> {
    for line in output.lines() {
        let Some((raw_label, raw_disabled)) = line.split_once("=>") else {
            continue;
        };
        let label = raw_label.trim().trim_matches('"');
        if label != LAUNCHD_LABEL {
            continue;
        }
        let disabled = raw_disabled.trim().trim_end_matches([';', ',']).trim();
        return match disabled {
            "false" | "enabled" => Ok("enabled".to_string()),
            "true" | "disabled" => Ok("disabled".to_string()),
            _ => Err(invalid_input(format!(
                "launchd returned an invalid enablement value for {LAUNCHD_LABEL}: {disabled:?}"
            ))),
        };
    }
    Err(invalid_input(format!(
        "launchd did not report an explicit enablement override for {LAUNCHD_LABEL}"
    )))
}

fn systemd_manager_runtime(active: &Output, enabled: &Output) -> Result<ServiceManagerRuntime> {
    refuse_unavailable_manager("read the per-user systemd service state", active)?;
    refuse_unavailable_manager("read the per-user systemd enablement state", enabled)?;
    Ok(ServiceManagerRuntime {
        loaded: None,
        enabled: Some(output_label(enabled, "disabled")),
        active: Some(output_label(active, "inactive")),
    })
}

fn render_human_service_status(
    context: &ServiceContext,
    installed: &str,
    runtime: &ServiceManagerRuntime,
) -> String {
    let manager_state = match context.platform {
        ServicePlatform::Launchd => {
            let loaded = if runtime.loaded == Some(true) {
                "loaded"
            } else {
                "stopped"
            };
            format!(
                "{loaded}, {}",
                runtime.enabled.as_deref().unwrap_or("enablement unknown")
            )
        }
        ServicePlatform::Systemd => format!(
            "{}, {}",
            runtime.active.as_deref().unwrap_or("inactive"),
            runtime.enabled.as_deref().unwrap_or("disabled")
        ),
    };
    format!(
        "supervisor service: {manager_state}\nunit: {installed}\npath: {}\nstate: {}\nbinary: {}",
        context.unit_path().display(),
        context.deadreckon_home.display(),
        context.binary.display()
    )
}

fn build_service_status_report(
    context: &ServiceContext,
    posture: MachineRestartPosture,
    runtime: ServiceManagerRuntime,
    checkpoint: Option<ServiceInstanceCheckpoint>,
    boot: BootIdentityObservation,
) -> Result<SupervisorServiceStatusReport> {
    validate_manager_runtime(context.platform, posture, &runtime)?;
    if let Some(checkpoint) = checkpoint.as_ref() {
        validate_checkpoint_identity(context, &runtime, checkpoint, &boot.current_boot_id)?;
    } else if runtime.is_running() {
        return Err(invalid_input(
            "service manager reports the supervisor running, but its instance checkpoint is absent",
        ));
    }
    Ok(SupervisorServiceStatusReport {
        schema_version: SERVICE_STATUS_SCHEMA,
        manager: match context.platform {
            ServicePlatform::Launchd => ServiceManager::Launchd,
            ServicePlatform::Systemd => ServiceManager::Systemd,
        },
        installed: service_installation_state(posture),
        loaded: runtime.loaded,
        enabled: runtime.enabled,
        active: runtime.active,
        checkpoint,
        current_boot_id: boot.current_boot_id,
        boot_identity_source: boot.source,
        test_override: boot.test_override,
    })
}

fn validate_manager_runtime(
    platform: ServicePlatform,
    posture: MachineRestartPosture,
    runtime: &ServiceManagerRuntime,
) -> Result<()> {
    if posture == MachineRestartPosture::NotInstalled {
        if runtime != &ServiceManagerRuntime::default() {
            return Err(invalid_input(
                "an uninstalled supervisor cannot carry service-manager runtime state",
            ));
        }
        return Ok(());
    }
    match platform {
        ServicePlatform::Launchd
            if runtime.loaded.is_none()
                || runtime.enabled.is_none()
                || runtime.active.is_some() =>
        {
            Err(invalid_input(
                "launchd status must contain loaded and enabled states",
            ))
        }
        ServicePlatform::Systemd
            if runtime.loaded.is_some()
                || runtime.enabled.is_none()
                || runtime.active.is_none() =>
        {
            Err(invalid_input(
                "systemd status must contain enabled and active states",
            ))
        }
        ServicePlatform::Systemd => {
            validate_service_value(
                "systemd enabled state",
                runtime.enabled.as_deref().unwrap_or_default(),
            )?;
            validate_service_value(
                "systemd active state",
                runtime.active.as_deref().unwrap_or_default(),
            )
        }
        ServicePlatform::Launchd => validate_service_value(
            "launchd enabled state",
            runtime.enabled.as_deref().unwrap_or_default(),
        ),
    }
}

fn unsupported_service_status_report(
    boot: BootIdentityObservation,
) -> SupervisorServiceStatusReport {
    SupervisorServiceStatusReport {
        schema_version: SERVICE_STATUS_SCHEMA,
        manager: ServiceManager::Unsupported,
        installed: ServiceInstallationState::Unsupported,
        loaded: None,
        enabled: None,
        active: None,
        checkpoint: None,
        current_boot_id: boot.current_boot_id,
        boot_identity_source: boot.source,
        test_override: boot.test_override,
    }
}

fn service_installation_state(posture: MachineRestartPosture) -> ServiceInstallationState {
    match posture {
        MachineRestartPosture::Unsupported => ServiceInstallationState::Unsupported,
        MachineRestartPosture::NotInstalled => ServiceInstallationState::NotInstalled,
        MachineRestartPosture::StaleManagedUnit => ServiceInstallationState::Stale,
        MachineRestartPosture::InstalledCurrent => ServiceInstallationState::Current,
        MachineRestartPosture::UnmanagedUnit => {
            unreachable!("unmanaged units are refused before status serialization")
        }
    }
}

fn current_boot_identity() -> BootIdentityObservation {
    let test_override =
        std::env::var_os("DEADRECKON_BOOT_ID").is_some_and(|value| !value.is_empty());
    let current_boot_id = boot_identity();
    let source =
        classify_boot_identity_source(std::env::consts::OS, &current_boot_id, test_override);
    BootIdentityObservation {
        current_boot_id,
        source,
        test_override,
    }
}

fn classify_boot_identity_source(
    target_os: &str,
    current_boot_id: &str,
    test_override: bool,
) -> BootIdentitySource {
    if test_override {
        return BootIdentitySource::TestOverride;
    }
    match target_os {
        "linux" if current_boot_id != "unknown-boot" => BootIdentitySource::LinuxProcfs,
        "macos" if current_boot_id.starts_with("macos:") => BootIdentitySource::MacosSysctl,
        _ => BootIdentitySource::Unknown,
    }
}

fn validate_checkpoint_identity(
    context: &ServiceContext,
    runtime: &ServiceManagerRuntime,
    checkpoint: &ServiceInstanceCheckpoint,
    current_boot_id: &str,
) -> Result<()> {
    if checkpoint.binary != context.binary {
        return Err(invalid_input(format!(
            "supervisor checkpoint binary {} does not match the current service binary {}",
            checkpoint.binary.display(),
            context.binary.display()
        )));
    }
    if checkpoint.deadreckon_home != context.deadreckon_home {
        return Err(invalid_input(format!(
            "supervisor checkpoint state {} does not match the current DEADRECKON_HOME {}",
            checkpoint.deadreckon_home.display(),
            context.deadreckon_home.display()
        )));
    }
    if runtime.is_running() && checkpoint.boot_id != current_boot_id {
        return Err(invalid_input(format!(
            "service manager reports the supervisor running, but checkpoint boot {} does not match current boot {}",
            checkpoint.boot_id, current_boot_id
        )));
    }
    if runtime.is_running() {
        match process_start_identity(checkpoint.pid) {
            Some(observed) if observed == checkpoint.process_start_identity => {}
            Some(_) => {
                return Err(invalid_input(format!(
                    "service manager reports the supervisor running, but checkpoint pid {} has a different process-start identity",
                    checkpoint.pid
                )));
            }
            None => {
                return Err(invalid_input(format!(
                    "service manager reports the supervisor running, but checkpoint pid {} is not live",
                    checkpoint.pid
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn supervisor_service_stop_command() -> Result<()> {
    let context = ServiceContext::discover()?;
    if !require_any_managed_unit(&context)? {
        println!("supervisor service already stopped (not installed)");
        return Ok(());
    }
    match context.platform {
        ServicePlatform::Launchd => {
            let domain = launchd_domain()?;
            let target = format!("{domain}/{LAUNCHD_LABEL}");
            run_required(
                "launchctl",
                ["disable", target.as_str()],
                "disable the DeadReckon supervisor",
            )?;
            if command_succeeds("launchctl", ["print", target.as_str()])? {
                run_required(
                    "launchctl",
                    ["bootout", target.as_str()],
                    "unload the DeadReckon supervisor",
                )?;
            }
        }
        ServicePlatform::Systemd => {
            let output = run_observational(
                "systemctl",
                ["--user", "disable", "--now", SYSTEMD_UNIT],
                "disable and stop the DeadReckon supervisor",
            )?;
            if !output.status.success() && !manager_reports_absent(&output) {
                return Err(command_failure(
                    "disable and stop the DeadReckon supervisor",
                    &output,
                ));
            }
        }
    }
    println!(
        "supervisor service stopped\nunit retained: {}\nrestart: deadreckon supervisor start",
        context.unit_path().display()
    );
    Ok(())
}

impl ServiceContext {
    fn discover() -> Result<Self> {
        let platform = detected_platform()?;
        Self::discover_for_platform(platform)
    }

    fn discover_for_platform(platform: ServicePlatform) -> Result<Self> {
        let binary = absolute_path(std::env::current_exe()?)?;
        let paths = DeadreckonPaths::discover();
        let deadreckon_home = absolute_path(paths.home().to_path_buf())?;
        let user_home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                invalid_input(
                    "HOME is required to install a per-user supervisor service".to_string(),
                )
            })
            .and_then(absolute_path)?;
        let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let path_env = std::env::var("PATH")
            .map_err(|_| invalid_input("PATH must be valid UTF-8 for service installation"))?;
        validate_service_value("PATH", &path_env)?;
        Ok(Self {
            platform,
            binary,
            deadreckon_home,
            user_home,
            xdg_config_home,
            path_env,
        })
    }

    fn unit_path(&self) -> PathBuf {
        match self.platform {
            ServicePlatform::Launchd => self
                .user_home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist")),
            ServicePlatform::Systemd => self
                .xdg_config_home
                .clone()
                .unwrap_or_else(|| self.user_home.join(".config"))
                .join("systemd")
                .join("user")
                .join(SYSTEMD_UNIT),
        }
    }

    fn log_dir(&self) -> PathBuf {
        self.deadreckon_home.join("logs")
    }
}

fn detected_platform() -> Result<ServicePlatform> {
    service_platform_for_os(std::env::consts::OS)
}

fn service_platform_for_os(target_os: &str) -> Result<ServicePlatform> {
    match target_os {
        "macos" => Ok(ServicePlatform::Launchd),
        "linux" => Ok(ServicePlatform::Systemd),
        _ => Err(invalid_input(
            "per-user supervisor services are supported on macOS launchd and Linux systemd only"
                .to_string(),
        )),
    }
}

fn machine_restart_posture(context: &ServiceContext) -> Result<MachineRestartPosture> {
    let unit_path = context.unit_path();
    match read_service_unit(&unit_path)? {
        Some(existing) if !is_managed_unit(context.platform, &existing) => {
            Ok(MachineRestartPosture::UnmanagedUnit)
        }
        Some(existing) if managed_unit_matches_context(context, &existing)? => {
            Ok(MachineRestartPosture::InstalledCurrent)
        }
        Some(_) => Ok(MachineRestartPosture::StaleManagedUnit),
        None => Ok(MachineRestartPosture::NotInstalled),
    }
}

fn service_instance_checkpoint_path(paths: &DeadreckonPaths) -> PathBuf {
    paths
        .home()
        .join(SERVICE_INSTANCE_DIR)
        .join(SERVICE_INSTANCE_FILE)
}

fn read_regular_non_symlink_file(path: &Path, description: &str) -> Result<Option<Vec<u8>>> {
    let initial_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(source.into()),
    };
    if initial_metadata.file_type().is_symlink() || !initial_metadata.file_type().is_file() {
        return Err(invalid_input(format!(
            "{description} must be a regular non-symlink file: {}",
            path.display()
        )));
    }
    let mut file = fs::File::open(path)?;
    let opened_metadata = file.metadata()?;
    let current_metadata = fs::symlink_metadata(path)?;
    if !opened_metadata.file_type().is_file()
        || current_metadata.file_type().is_symlink()
        || !current_metadata.file_type().is_file()
        || !same_file_identity(&opened_metadata, &current_metadata)
    {
        return Err(invalid_input(format!(
            "{description} changed identity while being opened: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    // Per-user services are unsupported off Unix, but keep parsing builds
    // portable without making a false live-service claim there.
    false
}

fn read_service_unit(path: &Path) -> Result<Option<String>> {
    let Some(bytes) = read_regular_non_symlink_file(path, "supervisor service unit")? else {
        return Ok(None);
    };
    String::from_utf8(bytes).map(Some).map_err(|_| {
        invalid_input(format!(
            "supervisor service unit must be valid UTF-8: {}",
            path.display()
        ))
    })
}

fn read_service_instance_checkpoint(path: &Path) -> Result<Option<ServiceInstanceCheckpoint>> {
    let Some(bytes) = read_regular_non_symlink_file(path, "supervisor checkpoint")? else {
        return Ok(None);
    };
    let checkpoint: ServiceInstanceCheckpoint =
        serde_json::from_slice(&bytes).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.to_path_buf(),
                source,
            })
        })?;
    validate_service_instance_checkpoint(&checkpoint)?;
    Ok(Some(checkpoint))
}

/// Read only the monotonic generation while replacing an older checkpoint.
/// Schema v1 never proves a live process because it lacks process-start
/// identity, but accepting its positive generation here lets a newly started
/// v2 service replace it without resetting durable ordering.
fn read_service_instance_header(path: &Path) -> Result<Option<ServiceInstanceCheckpointHeader>> {
    let Some(bytes) = read_regular_non_symlink_file(path, "supervisor checkpoint")? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| invalid_input("supervisor checkpoint schema_version must be an integer"))?;
    if schema != 1 && schema != u64::from(SERVICE_INSTANCE_SCHEMA) {
        return Err(invalid_input(format!(
            "unsupported supervisor checkpoint schema {schema}; replacement accepts schema 1 or {}",
            SERVICE_INSTANCE_SCHEMA
        )));
    }
    let generation = value
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .filter(|generation| *generation > 0)
        .ok_or_else(|| invalid_input("supervisor checkpoint generation must be positive"))?;
    Ok(Some(ServiceInstanceCheckpointHeader {
        schema_version: u32::try_from(schema)
            .map_err(|_| invalid_input("supervisor checkpoint schema_version is out of range"))?,
        generation,
    }))
}

fn validate_service_instance_checkpoint(checkpoint: &ServiceInstanceCheckpoint) -> Result<()> {
    if checkpoint.schema_version != SERVICE_INSTANCE_SCHEMA {
        return Err(invalid_input(format!(
            "unsupported supervisor checkpoint schema {}; expected {}",
            checkpoint.schema_version, SERVICE_INSTANCE_SCHEMA
        )));
    }
    if checkpoint.generation == 0 {
        return Err(invalid_input(
            "supervisor checkpoint generation must be positive",
        ));
    }
    validate_service_value("supervisor checkpoint instance id", &checkpoint.instance_id)?;
    validate_service_value("supervisor checkpoint boot id", &checkpoint.boot_id)?;
    if checkpoint.pid == 0 {
        return Err(invalid_input("supervisor checkpoint pid must be positive"));
    }
    validate_service_value(
        "supervisor checkpoint process-start identity",
        &checkpoint.process_start_identity,
    )?;
    for (name, path) in [
        ("supervisor checkpoint binary", &checkpoint.binary),
        (
            "supervisor checkpoint DEADRECKON_HOME",
            &checkpoint.deadreckon_home,
        ),
    ] {
        if !path.is_absolute() {
            return Err(invalid_input(format!(
                "{name} must be absolute: {}",
                path.display()
            )));
        }
        validate_service_value(name, path_to_utf8(name, path)?)?;
    }
    Ok(())
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let text = path_to_utf8("service path", &path)?;
    validate_service_value("service path", text)?;
    Ok(path)
}

fn render_service(context: &ServiceContext) -> Result<String> {
    let binary = path_to_utf8("DeadReckon binary", &context.binary)?;
    let home = path_to_utf8("DEADRECKON_HOME", &context.deadreckon_home)?;
    let stdout = context.log_dir().join("supervisor-service.out");
    let stderr = context.log_dir().join("supervisor-service.err");
    let stdout = path_to_utf8("supervisor stdout log", &stdout)?;
    let stderr = path_to_utf8("supervisor stderr log", &stderr)?;
    for (name, value) in [
        ("DeadReckon binary", binary),
        ("DEADRECKON_HOME", home),
        ("PATH", context.path_env.as_str()),
        ("supervisor stdout log", stdout),
        ("supervisor stderr log", stderr),
    ] {
        validate_service_value(name, value)?;
    }

    Ok(match context.platform {
        ServicePlatform::Launchd => format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- {MANAGED_MARKER} -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>supervisor</string>
    <string>serve</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>DEADRECKON_HOME</key>
    <string>{home}</string>
    <key>PATH</key>
    <string>{path_env}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>
"#,
            binary = xml_escape(binary),
            home = xml_escape(home),
            path_env = xml_escape(&context.path_env),
            stdout = xml_escape(stdout),
            stderr = xml_escape(stderr),
        ),
        ServicePlatform::Systemd => format!(
            r#"# {MANAGED_MARKER}
[Unit]
Description=DeadReckon durable local job supervisor

[Service]
Type=simple
ExecStart={} supervisor serve
Environment={}
Environment={}
Restart=always
RestartSec=5
KillMode=control-group
TimeoutStopSec=20
StandardOutput={}
StandardError={}

[Install]
WantedBy=default.target
"#,
            systemd_quote(binary),
            systemd_quote(&format!("DEADRECKON_HOME={home}")),
            systemd_quote(&format!("PATH={}", context.path_env)),
            systemd_quote(&format!("append:{stdout}")),
            systemd_quote(&format!("append:{stderr}")),
        ),
    })
}

fn install_rendered_unit(context: &ServiceContext, rendered: &str) -> Result<InstallDecision> {
    let unit_path = context.unit_path();
    let lock_path = unit_path.with_extension("deadreckon-install.lock");
    let _lock = InstallLock::acquire(lock_path)?;
    let existing = read_service_unit(&unit_path)?;
    let decision = decide_install(context.platform, existing.as_deref(), rendered);
    match decision {
        InstallDecision::Create | InstallDecision::ReplaceManaged => {
            write_unit_atomically(&unit_path, rendered.as_bytes())?;
        }
        InstallDecision::Unchanged => {}
        InstallDecision::RefuseUnmanaged => return Err(unmanaged_conflict_error(&unit_path)),
    }
    Ok(decision)
}

fn decide_install(
    platform: ServicePlatform,
    existing: Option<&str>,
    expected: &str,
) -> InstallDecision {
    match existing {
        None => InstallDecision::Create,
        Some(existing) if existing == expected => InstallDecision::Unchanged,
        Some(existing) if is_managed_unit(platform, existing) => InstallDecision::ReplaceManaged,
        Some(_) => InstallDecision::RefuseUnmanaged,
    }
}

fn is_managed_unit(platform: ServicePlatform, existing: &str) -> bool {
    match platform {
        ServicePlatform::Launchd => existing.starts_with(&format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- {MANAGED_MARKER} -->\n"
        )),
        ServicePlatform::Systemd => existing.starts_with(&format!("# {MANAGED_MARKER}\n")),
    }
}

/// A service's PATH is installation-time environment, not executable
/// identity. Reconstruct the expected managed unit with its persisted PATH and
/// then compare every other byte. This accepts caller PATH drift while keeping
/// the binary, DEADRECKON_HOME, arguments, logs, and restart policy pinned.
fn managed_unit_matches_context(context: &ServiceContext, existing: &str) -> Result<bool> {
    if !is_managed_unit(context.platform, existing) {
        return Ok(false);
    }
    let Some(installed_path) = installed_unit_path_env(context, existing)? else {
        return Ok(false);
    };
    let mut installed_context = context.clone();
    installed_context.path_env = installed_path;
    Ok(render_service(&installed_context)? == existing)
}

fn installed_unit_path_env(context: &ServiceContext, existing: &str) -> Result<Option<String>> {
    const PLACEHOLDER: &str = "deadreckon-path-identity-placeholder-93d6f53f";
    let mut placeholder_context = context.clone();
    placeholder_context.path_env = PLACEHOLDER.to_string();
    let expected = render_service(&placeholder_context)?;
    let Some((prefix, suffix)) = expected.split_once(PLACEHOLDER) else {
        return Err(invalid_input(
            "rendered supervisor service omitted its PATH identity placeholder",
        ));
    };
    let Some(encoded) = existing
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return Ok(None);
    };
    let decoded = match context.platform {
        ServicePlatform::Launchd => decode_generated_xml_value(encoded),
        ServicePlatform::Systemd => decode_generated_systemd_value(encoded),
    };
    let Some(decoded) = decoded else {
        return Ok(None);
    };
    if validate_service_value("installed supervisor PATH", &decoded).is_err() {
        return Ok(None);
    }
    Ok(Some(decoded))
}

fn decode_generated_xml_value(encoded: &str) -> Option<String> {
    let decoded = encoded
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    (xml_escape(&decoded) == encoded).then_some(decoded)
}

fn decode_generated_systemd_value(encoded: &str) -> Option<String> {
    let mut decoded = String::with_capacity(encoded.len());
    let mut characters = encoded.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some(next @ ('\\' | '"')) => decoded.push(next),
                _ => return None,
            },
            '%' => match characters.next() {
                Some('%') => decoded.push('%'),
                _ => return None,
            },
            _ => decoded.push(character),
        }
    }
    (systemd_quote(&decoded) == format!("\"{encoded}\"")).then_some(decoded)
}

fn require_current_managed_unit(context: &ServiceContext) -> Result<()> {
    let unit_path = context.unit_path();
    let existing = read_service_unit(&unit_path)?.ok_or_else(|| {
        invalid_input(format!(
            "supervisor service is not installed at {}; run `deadreckon supervisor install`",
            unit_path.display()
        ))
    })?;
    if !is_managed_unit(context.platform, &existing) {
        return Err(unmanaged_conflict_error(&unit_path));
    }
    if !managed_unit_matches_context(context, &existing)? {
        return Err(invalid_input(format!(
            "installed supervisor service is stale; run `deadreckon supervisor install` to pin {} and {}",
            context.binary.display(),
            context.deadreckon_home.display()
        )));
    }
    Ok(())
}

fn require_any_managed_unit(context: &ServiceContext) -> Result<bool> {
    let unit_path = context.unit_path();
    match read_service_unit(&unit_path)? {
        Some(existing) if is_managed_unit(context.platform, &existing) => Ok(true),
        Some(_) => Err(unmanaged_conflict_error(&unit_path)),
        None => Ok(false),
    }
}

fn launchd_start(context: &ServiceContext) -> Result<()> {
    let domain = launchd_domain()?;
    let target = format!("{domain}/{LAUNCHD_LABEL}");
    run_required(
        "launchctl",
        ["enable", target.as_str()],
        "enable the DeadReckon supervisor",
    )?;
    if command_succeeds("launchctl", ["print", target.as_str()])? {
        run_required(
            "launchctl",
            ["bootout", target.as_str()],
            "reload the current DeadReckon supervisor unit",
        )?;
    }
    let unit_path = context.unit_path();
    let unit = path_to_utf8("launchd unit", &unit_path)?;
    run_required(
        "launchctl",
        ["bootstrap", domain.as_str(), unit],
        "load and start the DeadReckon supervisor",
    )
}

fn launchd_domain() -> Result<String> {
    let output = run_observational("id", ["-u"], "resolve the current user id")?;
    if !output.status.success() {
        return Err(command_failure("resolve the current user id", &output));
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() || !uid.chars().all(|character| character.is_ascii_digit()) {
        return Err(invalid_input(format!(
            "`id -u` returned an invalid user id: {uid:?}"
        )));
    }
    Ok(format!("gui/{uid}"))
}

fn command_succeeds<I, S>(program: &str, args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(Command::new(program).args(args).output()?.status.success())
}

fn run_required<I, S>(program: &str, args: I, action: &str) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|source| {
            invalid_input(format!(
                "could not {action}: {source}; this command requires the per-user service manager"
            ))
        })?;
    if !output.status.success() {
        return Err(command_failure(action, &output));
    }
    Ok(())
}

fn run_observational<I, S>(program: &str, args: I, action: &str) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program).args(args).output().map_err(|source| {
        invalid_input(format!(
            "could not {action}: {source}; this command requires the per-user service manager"
        ))
    })
}

fn output_label(output: &Output, fallback: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let label = stdout.trim();
    if label.is_empty() {
        fallback.to_string()
    } else {
        label.to_string()
    }
}

fn manager_reports_absent(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    stderr.contains("not loaded")
        || stderr.contains("not found")
        || stderr.contains("could not find")
        || stderr.contains("does not exist")
}

fn refuse_unavailable_manager(action: &str, output: &Output) -> Result<()> {
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("failed to connect")
        || stderr.contains("cannot connect")
        || stderr.contains("no medium found")
    {
        return Err(command_failure(action, output));
    }
    Ok(())
}

fn command_failure(action: &str, output: &Output) -> CliError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else if !stdout.trim().is_empty() {
        stdout.trim()
    } else {
        "service manager returned no diagnostic"
    };
    invalid_input(format!(
        "could not {action} (exit {}): {detail}",
        output.status
    ))
}

fn write_unit_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid_input(format!("invalid service unit path: {}", path.display())))?;
    let temp_path = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let _cleanup = TempPathCleanup(temp_path.clone());
    let mut temp = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)?;
    temp.write_all(contents)?;
    temp.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o644))?;
    }
    fs::rename(&temp_path, path)?;
    Ok(())
}

struct TempPathCleanup(PathBuf);

impl Drop for TempPathCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct InstallLock {
    path: PathBuf,
}

impl InstallLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={} started_by=deadreckon-supervisor-install",
                    std::process::id()
                )?;
                Ok(Self { path })
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(invalid_input(format!(
                    "another supervisor service installation holds {}; retry after it finishes",
                    path.display()
                )))
            }
            Err(source) => Err(source.into()),
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unmanaged_conflict_error(path: &Path) -> CliError {
    invalid_input(format!(
        "refusing to overwrite or control unmanaged service unit {}; move it aside or remove it after inspection, then retry",
        path.display()
    ))
}

fn invalid_input(message: impl Into<String>) -> CliError {
    CliError::Core(DeadreckonError::InvalidInput(message.into()))
}

fn path_to_utf8<'a>(name: &str, path: &'a Path) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| invalid_input(format!("{name} must be valid UTF-8: {}", path.display())))
}

fn validate_service_value(name: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(invalid_input(format!("{name} cannot be empty")));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid_input(format!(
            "{name} cannot contain control characters"
        )));
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use deadreckon_core::{JobView, append_job_event, write_job};
    use deadreckon_protocol::{
        Job, JobEvent, JobEventKind, JobEventSequence, JobId, JobPolicy, JobSchemaVersion,
        JobShape, SemanticJudgeMode,
    };
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn context(platform: ServicePlatform) -> ServiceContext {
        ServiceContext {
            platform,
            binary: PathBuf::from("/opt/Dead Reckon/bin/deadreckon"),
            deadreckon_home: PathBuf::from("/Users/test/Dead Reckon State"),
            user_home: PathBuf::from("/Users/test"),
            xdg_config_home: None,
            path_env: "/opt/Agent CLIs/bin:/usr/bin:/bin".to_string(),
        }
    }

    fn checkpoint(
        context: &ServiceContext,
        generation: u64,
        instance_id: &str,
        boot_id: &str,
    ) -> ServiceInstanceCheckpoint {
        let pid = std::process::id();
        ServiceInstanceCheckpoint {
            schema_version: SERVICE_INSTANCE_SCHEMA,
            generation,
            instance_id: instance_id.to_string(),
            boot_id: boot_id.to_string(),
            pid,
            process_start_identity: process_start_identity(pid)
                .expect("test process must expose process-start identity"),
            started_at: Utc::now(),
            binary: context.binary.clone(),
            deadreckon_home: context.deadreckon_home.clone(),
        }
    }

    fn boot(current_boot_id: &str) -> BootIdentityObservation {
        BootIdentityObservation {
            current_boot_id: current_boot_id.to_string(),
            source: BootIdentitySource::LinuxProcfs,
            test_override: false,
        }
    }

    #[cfg(unix)]
    fn command_output(stdout: &str, stderr: &str, success: bool) -> Output {
        use std::os::unix::process::ExitStatusExt;

        Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn macos_launch_agent_is_restart_capable_and_path_safe() {
        let rendered =
            render_service(&context(ServicePlatform::Launchd)).expect("render launchd service");
        assert!(rendered.starts_with("<?xml version=\"1.0\""));
        assert!(rendered.contains(MANAGED_MARKER));
        assert!(rendered.contains("<string>/opt/Dead Reckon/bin/deadreckon</string>"));
        assert!(rendered.contains("<string>/Users/test/Dead Reckon State</string>"));
        assert!(rendered.contains("<string>supervisor</string>"));
        assert!(rendered.contains("<string>serve</string>"));
        assert!(rendered.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(is_managed_unit(ServicePlatform::Launchd, &rendered));
    }

    #[test]
    fn supervisor_service_lifecycle_is_discoverable_from_cli_help() {
        let handle = std::thread::Builder::new()
            .name("supervisor-service-clap-tree".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(assert_supervisor_service_lifecycle_is_discoverable_from_cli_help)
            .expect("spawn clap command tree test");
        if let Err(payload) = handle.join() {
            std::panic::resume_unwind(payload);
        }
    }

    fn assert_supervisor_service_lifecycle_is_discoverable_from_cli_help() {
        let mut command = crate::cli::Cli::command();
        let supervisor = command
            .find_subcommand_mut("supervisor")
            .expect("supervisor subcommand");
        assert!(!supervisor.is_hide_set());
        let help = supervisor.render_long_help().to_string();
        for operation in ["install", "start", "status", "stop"] {
            assert!(
                help.contains(operation),
                "missing {operation} from:\n{help}"
            );
        }
        let status = supervisor
            .find_subcommand_mut("status")
            .expect("supervisor status subcommand");
        assert!(status.render_long_help().to_string().contains("--json"));
    }

    #[test]
    fn launchd_status_reports_loaded_state_and_preserves_human_output() {
        let context = context(ServicePlatform::Launchd);
        let runtime = ServiceManagerRuntime {
            loaded: Some(true),
            enabled: Some("enabled".to_string()),
            active: None,
        };
        let report = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            runtime.clone(),
            Some(checkpoint(&context, 3, "launchd-instance", "boot-a")),
            boot("boot-a"),
        )
        .expect("launchd status report");

        assert_eq!(report.manager, ServiceManager::Launchd);
        assert_eq!(report.installed, ServiceInstallationState::Current);
        assert_eq!(report.loaded, Some(true));
        assert_eq!(report.enabled.as_deref(), Some("enabled"));
        assert_eq!(report.active, None);
        assert_eq!(
            render_human_service_status(&context, "current", &runtime),
            "supervisor service: loaded, enabled\nunit: current\npath: /Users/test/Library/LaunchAgents/com.deadreckon.supervisor.plist\nstate: /Users/test/Dead Reckon State\nbinary: /opt/Dead Reckon/bin/deadreckon"
        );
        let error = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            runtime,
            None,
            boot("boot-a"),
        )
        .expect_err("a running manager requires a checkpoint");
        assert!(error.to_string().contains("checkpoint is absent"));
    }

    #[cfg(unix)]
    #[test]
    fn systemd_status_reports_active_and_enabled_manager_labels() {
        let context = context(ServicePlatform::Systemd);
        let active = command_output("active\n", "", true);
        let enabled = command_output("enabled\n", "", true);
        let runtime = systemd_manager_runtime(&active, &enabled).expect("systemd state");
        let report = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            runtime.clone(),
            Some(checkpoint(&context, 4, "systemd-instance", "boot-a")),
            boot("boot-a"),
        )
        .expect("systemd status report");

        assert_eq!(report.manager, ServiceManager::Systemd);
        assert_eq!(report.loaded, None);
        assert_eq!(report.active.as_deref(), Some("active"));
        assert_eq!(report.enabled.as_deref(), Some("enabled"));
        assert_eq!(
            render_human_service_status(&context, "current", &runtime),
            format!(
                "supervisor service: active, enabled\nunit: current\npath: {}\nstate: /Users/test/Dead Reckon State\nbinary: /opt/Dead Reckon/bin/deadreckon",
                context.unit_path().display()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn launchd_runtime_requires_explicit_loaded_and_enabled_evidence() {
        let loaded = command_output("service = { pid = 42; }\n", "", true);
        let stopped = command_output(
            "",
            "Could not find service \"com.deadreckon.supervisor\" in domain for user\n",
            false,
        );
        let enabled = command_output(
            "disabled services = {\n  \"com.deadreckon.supervisor\" => false;\n}\n",
            "",
            true,
        );
        let disabled = command_output(
            "disabled services = {\n  \"com.deadreckon.supervisor\" => true;\n}\n",
            "",
            true,
        );

        assert_eq!(
            launchd_manager_runtime(&loaded, &enabled).expect("loaded and enabled"),
            ServiceManagerRuntime {
                loaded: Some(true),
                enabled: Some("enabled".to_string()),
                active: None,
            }
        );
        assert_eq!(
            launchd_manager_runtime(&stopped, &disabled).expect("stopped and disabled"),
            ServiceManagerRuntime {
                loaded: Some(false),
                enabled: Some("disabled".to_string()),
                active: None,
            }
        );
        let textual_enabled = command_output(
            "disabled services = {\n  \"com.deadreckon.supervisor\" => enabled\n}\n",
            "",
            true,
        );
        let textual_disabled = command_output(
            "disabled services = {\n  \"com.deadreckon.supervisor\" => disabled\n}\n",
            "",
            true,
        );
        assert_eq!(
            launchd_manager_runtime(&loaded, &textual_enabled)
                .expect("textual launchd enabled state"),
            ServiceManagerRuntime {
                loaded: Some(true),
                enabled: Some("enabled".to_string()),
                active: None,
            }
        );
        assert_eq!(
            launchd_manager_runtime(&stopped, &textual_disabled)
                .expect("textual launchd disabled state"),
            ServiceManagerRuntime {
                loaded: Some(false),
                enabled: Some("disabled".to_string()),
                active: None,
            }
        );
        let absent_override = command_output("disabled services = {\n}\n", "", true);
        assert!(
            launchd_manager_runtime(&loaded, &absent_override)
                .expect_err("implicit enablement cannot prove restart readiness")
                .to_string()
                .contains("explicit enablement override")
        );
        let malformed = command_output("\"com.deadreckon.supervisor\" => maybe;\n", "", true);
        assert!(
            launchd_manager_runtime(&loaded, &malformed)
                .expect_err("malformed launchd enablement")
                .to_string()
                .contains("invalid enablement value")
        );
    }

    #[test]
    fn ordinary_start_preflight_requires_a_loaded_launchd_service() {
        let context = context(ServicePlatform::Launchd);
        let ready = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: Some(true),
                enabled: Some("enabled".to_string()),
                active: None,
            },
            Some(checkpoint(&context, 1, "launchd-ready", "boot-a")),
            boot("boot-a"),
        )
        .expect("ready launchd status");
        assert!(service_preflight_from_status(&ready).is_ready());

        let stopped = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: Some(false),
                enabled: Some("enabled".to_string()),
                active: None,
            },
            Some(checkpoint(&context, 1, "launchd-stopped", "boot-a")),
            boot("boot-a"),
        )
        .expect("stopped launchd status");
        let decision = service_preflight_from_status(&stopped);
        assert!(!decision.is_ready());
        assert!(decision.setup_is_supported());
        assert!(
            decision
                .reason()
                .unwrap_or_default()
                .contains("not both loaded")
        );

        let disabled = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: Some(true),
                enabled: Some("disabled".to_string()),
                active: None,
            },
            Some(checkpoint(&context, 1, "launchd-disabled", "boot-a")),
            boot("boot-a"),
        )
        .expect("disabled launchd status");
        assert!(!service_preflight_from_status(&disabled).is_ready());
    }

    #[test]
    fn ordinary_start_preflight_requires_active_and_enabled_systemd() {
        let context = context(ServicePlatform::Systemd);
        let ready = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: None,
                enabled: Some("enabled".to_string()),
                active: Some("active".to_string()),
            },
            Some(checkpoint(&context, 1, "systemd-ready", "boot-a")),
            boot("boot-a"),
        )
        .expect("ready systemd status");
        assert!(service_preflight_from_status(&ready).is_ready());

        for (enabled, active) in [("disabled", "active"), ("enabled", "inactive")] {
            let report = build_service_status_report(
                &context,
                MachineRestartPosture::InstalledCurrent,
                ServiceManagerRuntime {
                    loaded: None,
                    enabled: Some(enabled.to_string()),
                    active: Some(active.to_string()),
                },
                Some(checkpoint(&context, 1, "systemd-not-ready", "boot-a")),
                boot("boot-a"),
            )
            .expect("non-ready systemd status");
            let decision = service_preflight_from_status(&report);
            assert!(!decision.is_ready(), "{active}, {enabled}");
            assert!(decision.setup_is_supported(), "{active}, {enabled}");
        }
    }

    #[test]
    fn stopped_service_status_preserves_a_checkpoint_from_the_prior_boot() {
        let context = context(ServicePlatform::Launchd);
        let report = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: Some(false),
                enabled: Some("enabled".to_string()),
                active: None,
            },
            Some(checkpoint(&context, 8, "prior-instance", "boot-before")),
            boot("boot-after"),
        )
        .expect("boot-change evidence");

        let checkpoint = report.checkpoint.expect("checkpoint");
        assert_eq!(checkpoint.boot_id, "boot-before");
        assert_eq!(report.current_boot_id, "boot-after");

        let error = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: Some(true),
                enabled: Some("enabled".to_string()),
                active: None,
            },
            Some(checkpoint),
            boot("boot-after"),
        )
        .expect_err("a running manager cannot use a prior-boot checkpoint");
        assert!(error.to_string().contains("does not match current boot"));
    }

    #[test]
    fn running_service_status_rejects_dead_or_reused_checkpoint_pid() {
        let context = context(ServicePlatform::Systemd);
        let runtime = ServiceManagerRuntime {
            loaded: None,
            enabled: Some("enabled".to_string()),
            active: Some("active".to_string()),
        };
        let mut reused = checkpoint(&context, 2, "reused", "boot-a");
        reused.process_start_identity = "different-process-start".to_string();
        let error = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            runtime.clone(),
            Some(reused),
            boot("boot-a"),
        )
        .expect_err("PID reuse must fail closed");
        assert!(
            error
                .to_string()
                .contains("different process-start identity")
        );

        let mut dead = checkpoint(&context, 3, "dead", "boot-a");
        dead.pid = u32::MAX;
        let error = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            runtime,
            Some(dead),
            boot("boot-a"),
        )
        .expect_err("dead checkpoint PID must fail closed");
        assert!(error.to_string().contains("is not live"));
    }

    #[test]
    fn fresh_service_successor_requires_generation_instance_and_process_change() {
        let context = context(ServicePlatform::Systemd);
        let prior_checkpoint = checkpoint(&context, 7, "prior", "boot-a");
        let prior = SupervisorServiceInstance::from(&prior_checkpoint);
        let mut next_checkpoint = prior_checkpoint.clone();
        next_checkpoint.generation = 8;
        next_checkpoint.instance_id = "next".to_string();
        next_checkpoint.pid = next_checkpoint.pid.saturating_add(1);
        let next = SupervisorServiceInstance::from(&next_checkpoint);
        assert!(next.is_fresh_successor_of(&prior));

        let mut same_generation = next.clone();
        same_generation.generation = prior.generation;
        assert!(!same_generation.is_fresh_successor_of(&prior));
        let mut same_instance = next.clone();
        same_instance.instance_id = prior.instance_id.clone();
        assert!(!same_instance.is_fresh_successor_of(&prior));
        let mut same_process = next;
        same_process.pid = prior.pid;
        same_process.process_start_identity = prior.process_start_identity.clone();
        assert!(!same_process.is_fresh_successor_of(&prior));
    }

    #[test]
    fn service_status_json_is_closed_and_marks_test_boot_overrides() {
        let context = context(ServicePlatform::Systemd);
        let report = build_service_status_report(
            &context,
            MachineRestartPosture::InstalledCurrent,
            ServiceManagerRuntime {
                loaded: None,
                enabled: Some("enabled".to_string()),
                active: Some("active".to_string()),
            },
            Some(checkpoint(&context, 5, "instance-json", "override-boot")),
            BootIdentityObservation {
                current_boot_id: "override-boot".to_string(),
                source: BootIdentitySource::TestOverride,
                test_override: true,
            },
        )
        .expect("closed status report");
        let mut value = serde_json::to_value(&report).expect("status JSON");
        let keys = value
            .as_object()
            .expect("status object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "active",
                "boot_identity_source",
                "checkpoint",
                "current_boot_id",
                "enabled",
                "installed",
                "loaded",
                "manager",
                "schema_version",
                "test_override",
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(value["boot_identity_source"], "test_override");
        assert_eq!(value["test_override"], true);
        assert_eq!(value["schema_version"], SERVICE_STATUS_SCHEMA);
        assert!(
            validate_supervisor_service_live_evidence(&report)
                .expect_err("test override cannot become live evidence")
                .to_string()
                .contains("cannot prove a live reboot")
        );
        let mut unknown_source = report.clone();
        unknown_source.boot_identity_source = BootIdentitySource::Unknown;
        unknown_source.test_override = false;
        assert!(
            validate_supervisor_service_live_evidence(&unknown_source)
                .expect_err("unknown boot identity cannot become live evidence")
                .to_string()
                .contains("no authoritative boot identity source")
        );
        let mut live_source = report.clone();
        live_source.boot_identity_source = BootIdentitySource::LinuxProcfs;
        live_source.test_override = false;
        validate_supervisor_service_live_evidence(&live_source)
            .expect("authoritative boot identity");
        let checkpoint_keys = value["checkpoint"]
            .as_object()
            .expect("checkpoint object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            checkpoint_keys,
            [
                "binary",
                "boot_id",
                "deadreckon_home",
                "generation",
                "instance_id",
                "pid",
                "process_start_identity",
                "schema_version",
                "started_at",
            ]
            .into_iter()
            .collect()
        );
        value
            .as_object_mut()
            .expect("status object")
            .insert("untrusted_note".to_string(), json!("forged"));
        assert!(
            serde_json::from_value::<SupervisorServiceStatusReport>(value).is_err(),
            "unknown status fields must be rejected"
        );
    }

    #[test]
    fn malformed_service_checkpoint_is_rejected() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("instance.json");
        fs::write(&path, b"{not-json\n").expect("malformed checkpoint");
        assert!(
            read_service_instance_checkpoint(&path)
                .expect_err("malformed checkpoint must fail")
                .to_string()
                .contains("JSON")
        );

        let context = context(ServicePlatform::Systemd);
        let mut invalid = checkpoint(&context, 0, "instance", "boot-a");
        invalid.pid = 0;
        fs::write(
            &path,
            serde_json::to_vec(&invalid).expect("invalid checkpoint JSON"),
        )
        .expect("invalid checkpoint");
        let error = read_service_instance_checkpoint(&path)
            .expect_err("invalid checkpoint identity must fail");
        assert!(error.to_string().contains("generation must be positive"));

        let legacy = json!({
            "schema_version": 1,
            "generation": 9,
            "instance_id": "legacy-instance",
            "boot_id": "boot-a",
            "pid": std::process::id(),
            "started_at": Utc::now(),
            "binary": context.binary.clone(),
            "deadreckon_home": context.deadreckon_home.clone(),
        });
        fs::write(
            &path,
            serde_json::to_vec(&legacy).expect("legacy checkpoint JSON"),
        )
        .expect("legacy checkpoint");
        let error = read_service_instance_checkpoint(&path)
            .expect_err("legacy checkpoint without process identity must fail closed");
        assert!(
            error
                .to_string()
                .contains("unsupported supervisor checkpoint schema 1")
        );
        assert_eq!(
            read_service_instance_header(&path).expect("legacy generation for replacement"),
            Some(ServiceInstanceCheckpointHeader {
                schema_version: 1,
                generation: 9,
            }),
            "a new service may preserve ordering while replacing legacy evidence"
        );

        let mut missing_identity = checkpoint(&context, 10, "missing-identity", "boot-a");
        missing_identity.process_start_identity.clear();
        fs::write(
            &path,
            serde_json::to_vec(&missing_identity).expect("missing identity JSON"),
        )
        .expect("missing identity checkpoint");
        assert!(
            read_service_instance_checkpoint(&path)
                .expect_err("missing process identity must fail closed")
                .to_string()
                .contains("process-start identity cannot be empty")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_service_checkpoint_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let target = temp.path().join("real-instance.json");
        let link = temp.path().join("instance.json");
        let context = context(ServicePlatform::Launchd);
        fs::write(
            &target,
            serde_json::to_vec(&checkpoint(&context, 1, "instance", "boot-a"))
                .expect("checkpoint JSON"),
        )
        .expect("checkpoint");
        symlink(&target, &link).expect("checkpoint symlink");

        let error =
            read_service_instance_checkpoint(&link).expect_err("symlink checkpoint must fail");
        assert!(error.to_string().contains("regular non-symlink"));
    }

    #[test]
    fn linux_user_unit_is_restart_capable_and_path_safe() {
        let rendered =
            render_service(&context(ServicePlatform::Systemd)).expect("render systemd service");
        assert!(rendered.starts_with(&format!("# {MANAGED_MARKER}\n")));
        assert!(
            rendered.contains("ExecStart=\"/opt/Dead Reckon/bin/deadreckon\" supervisor serve")
        );
        assert!(rendered.contains("Environment=\"DEADRECKON_HOME=/Users/test/Dead Reckon State\""));
        assert!(rendered.contains(
            "StandardOutput=\"append:/Users/test/Dead Reckon State/logs/supervisor-service.out\""
        ));
        assert!(rendered.contains("Restart=always"));
        assert!(rendered.contains("KillMode=control-group"));
        assert!(is_managed_unit(ServicePlatform::Systemd, &rendered));
    }

    #[test]
    fn install_decision_is_idempotent_and_managed_updates_are_explicit() {
        let expected =
            render_service(&context(ServicePlatform::Systemd)).expect("render systemd service");
        assert_eq!(
            decide_install(ServicePlatform::Systemd, None, &expected),
            InstallDecision::Create
        );
        assert_eq!(
            decide_install(ServicePlatform::Systemd, Some(&expected), &expected),
            InstallDecision::Unchanged
        );
        let stale = format!("# {MANAGED_MARKER}\nold managed content\n");
        assert_eq!(
            decide_install(ServicePlatform::Systemd, Some(&stale), &expected),
            InstallDecision::ReplaceManaged
        );
    }

    #[test]
    fn unmanaged_units_are_never_selected_for_replacement() {
        let expected =
            render_service(&context(ServicePlatform::Launchd)).expect("render launchd service");
        let foreign = "<?xml version=\"1.0\"?><plist><dict/></plist>";
        assert_eq!(
            decide_install(ServicePlatform::Launchd, Some(foreign), &expected),
            InstallDecision::RefuseUnmanaged
        );
    }

    #[test]
    fn render_escapes_service_syntax_metacharacters() {
        let mut launchd = context(ServicePlatform::Launchd);
        launchd.path_env = "/opt/a&b/<cli>:/bin".to_string();
        let plist = render_service(&launchd).expect("escaped plist");
        assert!(plist.contains("/opt/a&amp;b/&lt;cli&gt;:/bin"));

        let mut systemd = context(ServicePlatform::Systemd);
        systemd.path_env = "/opt/100%/a\"b:/bin".to_string();
        let unit = render_service(&systemd).expect("escaped unit");
        assert!(unit.contains("PATH=/opt/100%%/a\\\"b:/bin"));
    }

    #[test]
    fn render_rejects_control_character_injection() {
        let mut malicious = context(ServicePlatform::Systemd);
        malicious.path_env = "/usr/bin\nExecStart=/tmp/foreign".to_string();
        let error = render_service(&malicious).expect_err("newline must be rejected");
        assert!(error.to_string().contains("control characters"));
    }

    #[test]
    fn unsupported_platform_never_claims_machine_restart_durability() {
        for target_os in ["windows", "freebsd", "unknown"] {
            let error =
                service_platform_for_os(target_os).expect_err("unsupported service platform");
            assert!(
                error
                    .to_string()
                    .contains("supported on macOS launchd and Linux systemd only"),
                "{error}"
            );
        }
        assert_eq!(
            service_platform_for_os("macos").expect("macOS"),
            ServicePlatform::Launchd
        );
        assert_eq!(
            service_platform_for_os("linux").expect("Linux"),
            ServicePlatform::Systemd
        );
    }

    #[test]
    fn unit_posture_never_claims_runtime_service_state() {
        let configured = machine_restart_posture_label(MachineRestartPosture::InstalledCurrent);
        assert!(configured.contains("configured by the current service unit"));
        assert!(configured.contains("runtime state is reported"));
        assert!(!configured.contains("loaded"));
        assert!(!configured.contains("active"));

        for posture in [
            MachineRestartPosture::NotInstalled,
            MachineRestartPosture::StaleManagedUnit,
            MachineRestartPosture::UnmanagedUnit,
            MachineRestartPosture::Unsupported,
        ] {
            let label = machine_restart_posture_label(posture);
            assert!(
                label.contains("not configured") || label.contains("unsupported"),
                "{posture:?}: {label}"
            );
        }
    }

    #[test]
    fn lazy_per_job_supervisors_coexist_with_the_installed_service() {
        assert!(!supervisor_requires_service_singleton(true, Some("job-1")));
        assert!(supervisor_requires_service_singleton(true, None));
        assert!(supervisor_requires_service_singleton(false, Some("job-1")));
        assert!(supervisor_requires_service_singleton(false, None));
    }

    #[test]
    fn uninstalled_service_posture_never_claims_machine_restart_durability() {
        let temp = TempDir::new().expect("tempdir");
        let context = ServiceContext {
            platform: ServicePlatform::Systemd,
            binary: temp.path().join("deadreckon"),
            deadreckon_home: temp.path().join("state"),
            user_home: temp.path().join("user"),
            xdg_config_home: None,
            path_env: "/usr/bin:/bin".to_string(),
        };
        assert_eq!(
            machine_restart_posture(&context).expect("posture"),
            MachineRestartPosture::NotInstalled
        );
    }

    #[test]
    fn service_posture_distinguishes_unmanaged_stale_and_current_units() {
        let temp = TempDir::new().expect("tempdir");
        let context = ServiceContext {
            platform: ServicePlatform::Systemd,
            binary: temp.path().join("deadreckon"),
            deadreckon_home: temp.path().join("state"),
            user_home: temp.path().join("user"),
            xdg_config_home: None,
            path_env: "/usr/bin:/bin".to_string(),
        };
        let unit_path = context.unit_path();
        fs::create_dir_all(unit_path.parent().expect("unit parent")).expect("unit dir");

        fs::write(&unit_path, "[Unit]\nDescription=someone else\n").expect("foreign unit");
        assert_eq!(
            machine_restart_posture(&context).expect("unmanaged posture"),
            MachineRestartPosture::UnmanagedUnit
        );

        fs::write(&unit_path, format!("# {MANAGED_MARKER}\nold settings\n"))
            .expect("stale managed unit");
        assert_eq!(
            machine_restart_posture(&context).expect("stale posture"),
            MachineRestartPosture::StaleManagedUnit
        );

        fs::write(&unit_path, render_service(&context).expect("current unit"))
            .expect("current managed unit");
        assert_eq!(
            machine_restart_posture(&context).expect("current posture"),
            MachineRestartPosture::InstalledCurrent
        );
        let mut path_drift = context.clone();
        path_drift.path_env = "/caller/path/changed:/bin".to_string();
        assert_eq!(
            machine_restart_posture(&path_drift).expect("PATH-independent current posture"),
            MachineRestartPosture::InstalledCurrent
        );
    }

    #[test]
    fn current_unit_identity_ignores_caller_path_but_binds_execution_contract() {
        for platform in [ServicePlatform::Launchd, ServicePlatform::Systemd] {
            let mut installed_context = context(platform);
            installed_context.path_env = match platform {
                ServicePlatform::Launchd => "/opt/a&b/<cli>:/bin".to_string(),
                ServicePlatform::Systemd => "/opt/100%/a\"b\\cli:/bin".to_string(),
            };
            let installed = render_service(&installed_context).expect("installed service");
            let mut caller_context = installed_context.clone();
            caller_context.path_env = "/different/caller/path:/bin".to_string();
            assert!(
                managed_unit_matches_context(&caller_context, &installed)
                    .expect("PATH-insensitive service identity"),
                "{platform:?}"
            );

            let mut wrong_binary = caller_context.clone();
            wrong_binary.binary = PathBuf::from("/opt/other/deadreckon");
            assert!(
                !managed_unit_matches_context(&wrong_binary, &installed).expect("binary mismatch"),
                "{platform:?}"
            );
            let mut wrong_home = caller_context.clone();
            wrong_home.deadreckon_home = PathBuf::from("/tmp/other-deadreckon-home");
            assert!(
                !managed_unit_matches_context(&wrong_home, &installed).expect("home mismatch"),
                "{platform:?}"
            );
            let altered_arguments = match platform {
                ServicePlatform::Launchd => installed.replace(
                    "<string>supervisor</string>",
                    "<string>foreign-command</string>",
                ),
                ServicePlatform::Systemd => {
                    installed.replace(" supervisor serve", " foreign-command serve")
                }
            };
            assert!(
                !managed_unit_matches_context(&caller_context, &altered_arguments)
                    .expect("argument mismatch"),
                "{platform:?}"
            );
            let altered_restart = match platform {
                ServicePlatform::Launchd => {
                    installed.replace("<key>KeepAlive</key>", "<key>AbandonProcessGroup</key>")
                }
                ServicePlatform::Systemd => installed.replace("Restart=always", "Restart=no"),
            };
            assert!(
                !managed_unit_matches_context(&caller_context, &altered_restart)
                    .expect("restart mismatch"),
                "{platform:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn service_unit_paths_reject_symlinks_and_non_regular_files() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let context = ServiceContext {
            platform: ServicePlatform::Systemd,
            binary: temp.path().join("deadreckon"),
            deadreckon_home: temp.path().join("state"),
            user_home: temp.path().join("user"),
            xdg_config_home: None,
            path_env: "/usr/bin:/bin".to_string(),
        };
        let unit_path = context.unit_path();
        fs::create_dir_all(unit_path.parent().expect("unit parent")).expect("unit parent");
        let target = temp.path().join("real.service");
        let rendered = render_service(&context).expect("rendered service");
        fs::write(&target, &rendered).expect("real unit");
        symlink(&target, &unit_path).expect("unit symlink");

        for error in [
            machine_restart_posture(&context).expect_err("posture must reject symlink"),
            require_current_managed_unit(&context).expect_err("start must reject symlink"),
            install_rendered_unit(&context, &rendered).expect_err("install must reject symlink"),
            require_any_managed_unit(&context).expect_err("stop must reject symlink"),
        ] {
            assert!(error.to_string().contains("regular non-symlink"), "{error}");
        }

        fs::remove_file(&unit_path).expect("remove symlink");
        fs::create_dir(&unit_path).expect("directory at unit path");
        assert!(
            machine_restart_posture(&context)
                .expect_err("directory must be rejected")
                .to_string()
                .contains("regular non-symlink")
        );
    }

    #[test]
    fn service_singleton_is_nonblocking_and_replacement_gets_a_new_checkpoint() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("state"));
        let binary = temp.path().join("bin").join("deadreckon");
        let first = acquire_supervisor_service_guard(&paths, "service-a", "boot-a", &binary)
            .expect("first service");
        assert_eq!(first.checkpoint.generation, 1);
        assert_eq!(first.checkpoint.instance_id, "service-a");

        let error = acquire_supervisor_service_guard(&paths, "service-b", "boot-a", &binary)
            .expect_err("live singleton must refuse immediately");
        assert!(matches!(
            error,
            CliError::Core(DeadreckonError::LockHeld { .. })
        ));

        // Dropping the process-held file lock simulates the old service no
        // longer running. Its durable checkpoint remains for the successor.
        drop(first);
        let replacement = acquire_supervisor_service_guard(&paths, "service-b", "boot-b", &binary)
            .expect("replacement service");
        assert_eq!(replacement.checkpoint.generation, 2);
        assert_eq!(replacement.checkpoint.instance_id, "service-b");
        assert_eq!(replacement.checkpoint.boot_id, "boot-b");
        assert_eq!(replacement.checkpoint.deadreckon_home, paths.home());
        assert_eq!(
            read_service_instance_checkpoint(&service_instance_checkpoint_path(&paths))
                .expect("checkpoint read")
                .expect("checkpoint"),
            replacement.checkpoint
        );
    }

    #[test]
    fn service_restart_resumes_same_job_workspace_budget_and_attempt() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("state"));
        let workspace = temp.path().join("isolated-workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(workspace.join("progress.txt"), "durable work\n").expect("workspace progress");
        let job_id = JobId("service-restart-job".to_string());
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: "service-restart-scope".to_string(),
            goal: "survive a service replacement".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: workspace.clone(),
            launch_plan_sha256: "sha256:launch".to_string(),
            authority_sha256: "sha256:authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 2.75,
                max_wall_seconds: 180,
                max_attempts: 4,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            },
        };
        write_job(&paths, &job).expect("job");
        for (index, (kind, lease_epoch, detail)) in [
            (JobEventKind::Created, 0, json!({})),
            (JobEventKind::Queued, 0, json!({})),
            (
                JobEventKind::LeaseAcquired,
                1,
                json!({ "owner_id": "service-a" }),
            ),
            (
                JobEventKind::AttemptStarted,
                1,
                json!({
                    "attempt": 1,
                    "max_spend_usd": 2.75,
                    "max_wall_seconds": 180,
                }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            append_job_event(
                &paths,
                &JobEvent {
                    schema_version: JobSchemaVersion::CURRENT,
                    job_id: job_id.clone(),
                    sequence: JobEventSequence::new(index as u64 + 1).expect("sequence"),
                    event_id: format!("restart-{index}"),
                    causation_id: format!("restart-{index}"),
                    timestamp: Utc::now(),
                    lease_epoch,
                    kind,
                    detail,
                },
            )
            .expect("job event");
        }
        let history_before = fs::read(paths.job_events(job_id.as_ref())).expect("history bytes");
        let first = acquire_supervisor_service_guard(
            &paths,
            "service-a",
            "boot-a",
            Path::new("/opt/deadreckon-a"),
        )
        .expect("first service");
        let view_before = JobView::load(&paths, job_id.as_ref()).expect("job before restart");
        drop(first);

        let replacement = acquire_supervisor_service_guard(
            &paths,
            "service-b",
            "boot-b",
            Path::new("/opt/deadreckon-b"),
        )
        .expect("replacement service");
        let view_after = JobView::load(&paths, job_id.as_ref()).expect("job after restart");

        assert_eq!(replacement.checkpoint.generation, 2);
        assert_eq!(view_after.job, view_before.job);
        assert_eq!(view_after.job.job_id, job_id);
        assert_eq!(view_after.job.source_cwd, workspace);
        assert_eq!(view_after.job.policy.max_spend_usd, 2.75);
        assert_eq!(view_after.job.policy.max_wall_seconds, 180);
        assert_eq!(view_after.job.policy.max_attempts, 4);
        assert_eq!(view_after.projection.attempt_count, 1);
        assert_eq!(view_after.projection.last_sequence, 4);
        assert_eq!(
            fs::read_to_string(view_after.job.source_cwd.join("progress.txt"))
                .expect("workspace survives"),
            "durable work\n"
        );
        assert_eq!(
            fs::read(paths.job_events(job_id.as_ref())).expect("history after restart"),
            history_before,
            "service replacement must not rewrite lifecycle truth"
        );
    }
}
