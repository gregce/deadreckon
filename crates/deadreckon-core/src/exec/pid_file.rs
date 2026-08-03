use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pid_is_alive;

pub const SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION: u32 = 1;

/// Additive child-process metadata written beside a supervised run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedProcess {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
}

/// Whether a guarded subprocess has crossed its durable release boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisedProcessPhase {
    /// The guard is blocked on a private release pipe. Repository-controlled
    /// code has not started and the process has not left its parent's group.
    Prepared,
    /// The durable identity was revalidated, the fresh process group exists,
    /// and the approved command may execute.
    Running,
}

/// Crash-recoverable identity for a subprocess that may leave its parent's
/// process group.
///
/// PID alone is never enough to signal a process after recovery. The boot and
/// process-start identities prevent a stale sidecar from targeting a reused
/// PID. `launch_id` also makes cleanup compare-and-remove rather than allowing
/// one attempt to delete a later attempt's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedProcessRecord {
    pub schema_version: u32,
    #[serde(flatten)]
    pub process: SupervisedProcess,
    pub launch_id: String,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_launch_id: Option<String>,
    pub release_token_sha256: String,
    pub boot_id: String,
    pub process_start_identity: String,
    pub phase: SupervisedProcessPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisedProcessIdentity {
    Current,
    Exited,
    DifferentBoot,
    Reused,
    Unverifiable,
}

impl SupervisedProcessRecord {
    pub fn prepared(
        process: SupervisedProcess,
        launch_id: String,
        attempt: u32,
        owner_launch_id: Option<String>,
        release_token_sha256: String,
    ) -> io::Result<Self> {
        if process.pid == 0
            || process.pgid.is_some()
            || attempt == 0
            || launch_id.trim().is_empty()
            || release_token_sha256.trim().is_empty()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "guarded process identity is incomplete",
            ));
        }
        let process_start_identity = process_start_identity(process.pid).ok_or_else(|| {
            io::Error::other(format!(
                "could not establish start identity for guarded process {}",
                process.pid
            ))
        })?;
        Ok(Self {
            schema_version: SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION,
            process,
            launch_id,
            attempt,
            owner_launch_id,
            release_token_sha256,
            boot_id: boot_identity(),
            process_start_identity,
            phase: SupervisedProcessPhase::Prepared,
        })
    }

    /// Capture a running, process-group-owned child with a fresh launch
    /// identity suitable for crash recovery and compare-and-remove cleanup.
    ///
    /// Unguarded provider children do not cross a release pipe, but retaining
    /// the existing record schema keeps every child-pid sidecar on the same
    /// fail-closed reconciliation path. The release digest is therefore the
    /// digest of a fresh per-launch nonce and is used only as durable identity
    /// metadata for this kind of record.
    pub fn running(process: SupervisedProcess) -> io::Result<Self> {
        if process.pid == 0 || !running_process_group_is_valid(process) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "running process identity is incomplete",
            ));
        }
        let process_start_identity = process_start_identity(process.pid).ok_or_else(|| {
            io::Error::other(format!(
                "could not establish start identity for running process {}",
                process.pid
            ))
        })?;
        let launch_id = Uuid::new_v4().to_string();
        let nonce = Uuid::new_v4().to_string();
        Ok(Self {
            schema_version: SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION,
            process,
            launch_id,
            attempt: 1,
            owner_launch_id: None,
            release_token_sha256: crate::flight::sha256_text(&nonce),
            boot_id: boot_identity(),
            process_start_identity,
            phase: SupervisedProcessPhase::Running,
        })
    }

    pub fn identity(&self) -> SupervisedProcessIdentity {
        if self.schema_version != SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION
            || self.boot_id.trim().is_empty()
            || self.process_start_identity.trim().is_empty()
        {
            return SupervisedProcessIdentity::Unverifiable;
        }
        if self.boot_id != boot_identity() {
            return SupervisedProcessIdentity::DifferentBoot;
        }
        if !pid_is_alive(self.process.pid) {
            return SupervisedProcessIdentity::Exited;
        }
        match process_start_identity(self.process.pid) {
            Some(observed) if observed == self.process_start_identity => {
                SupervisedProcessIdentity::Current
            }
            Some(_) => SupervisedProcessIdentity::Reused,
            None => SupervisedProcessIdentity::Unverifiable,
        }
    }
}

pub fn write_supervised_process(path: &Path, process: SupervisedProcess) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(&process).map_err(io::Error::other)?;
    encoded.push(b'\n');
    write_atomic_synced(path, &encoded)
}

pub fn write_supervised_process_record(
    path: &Path,
    record: &SupervisedProcessRecord,
) -> io::Result<()> {
    validate_record(record)?;
    let mut encoded = serde_json::to_vec(record).map_err(io::Error::other)?;
    encoded.push(b'\n');
    write_atomic_synced(path, &encoded)
}

pub fn read_supervised_process_record(path: &Path) -> io::Result<SupervisedProcessRecord> {
    let bytes = fs::read(path)?;
    let record: SupervisedProcessRecord = serde_json::from_slice(trim_ascii(&bytes))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    validate_record(&record)?;
    Ok(record)
}

pub fn remove_supervised_process_record_if_matches(
    path: &Path,
    launch_id: &str,
    pid: u32,
) -> io::Result<bool> {
    let record = match read_supervised_process_record(path) {
        Ok(record) => record,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if record.launch_id != launch_id || record.process.pid != pid {
        return Ok(false);
    }
    remove_record_file(path)
}

/// Remove only the exact record a caller previously wrote or validated.
///
/// Cleanup must not re-probe a process after it has been signalled: the PID can
/// disappear between the liveness check and the process-start query, making an
/// already-dead child look unverifiable. Exact durable-record comparison closes
/// the replacement race without depending on live process state.
pub fn remove_supervised_process_record_if_same(
    path: &Path,
    expected: &SupervisedProcessRecord,
) -> io::Result<bool> {
    let record = match read_supervised_process_record(path) {
        Ok(record) => record,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if record != *expected {
        return Ok(false);
    }
    remove_record_file(path)
}

fn remove_record_file(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => {
            sync_parent(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn read_supervised_process(path: &Path) -> io::Result<SupervisedProcess> {
    let bytes = fs::read(path)?;
    let trimmed = trim_ascii(&bytes);
    if trimmed.starts_with(b"{") {
        return serde_json::from_slice(trimmed)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }

    // Provider pid files before Capstan contained only `<pid>\n`.
    let raw = std::str::from_utf8(trimmed)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let pid = raw
        .parse::<u32>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(SupervisedProcess { pid, pgid: None })
}

pub fn boot_identity() -> String {
    if let Some(value) = std::env::var_os("DEADRECKON_BOOT_ID")
        && !value.is_empty()
    {
        return value.to_string_lossy().into_owned();
    }
    #[cfg(target_os = "linux")]
    if let Ok(value) = fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        && output.status.success()
    {
        let value = String::from_utf8_lossy(&output.stdout);
        let value = value.trim();
        if !value.is_empty() {
            return format!("macos:{value}");
        }
    }
    // Unknown is deliberately stable. A random per-process value would look
    // like a reboot and could reclaim a live process identity.
    "unknown-boot".to_string()
}

pub fn process_start_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let command_end = raw.rfind(')')?;
        let fields = raw[command_end + 1..]
            .split_whitespace()
            .collect::<Vec<_>>();
        // `/proc/<pid>/stat` field 22 is process start ticks. The slice starts
        // at field 3 (`state`), so index 19 is the stable same-boot identity.
        return fields.get(19).map(|start| format!("linux:{start}"));
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let start = String::from_utf8_lossy(&output.stdout);
        let start = start.trim();
        return (!start.is_empty()).then(|| format!("macos:{start}"));
    }
    #[cfg(target_os = "windows")]
    {
        let script =
            format!("(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToUniversalTime().Ticks");
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let start = String::from_utf8_lossy(&output.stdout);
        let start = start.trim();
        return (!start.is_empty()).then(|| format!("windows:{start}"));
    }
    #[allow(unreachable_code)]
    None
}

fn validate_record(record: &SupervisedProcessRecord) -> io::Result<()> {
    if record.schema_version != SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported supervised process record schema {}",
                record.schema_version
            ),
        ));
    }
    if record.process.pid == 0
        || record.attempt == 0
        || record.launch_id.trim().is_empty()
        || record.release_token_sha256.trim().is_empty()
        || record.boot_id.trim().is_empty()
        || record.process_start_identity.trim().is_empty()
        || !phase_process_group_is_valid(record)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid supervised process record",
        ));
    }
    Ok(())
}

fn phase_process_group_is_valid(record: &SupervisedProcessRecord) -> bool {
    match record.phase {
        SupervisedProcessPhase::Prepared => record.process.pgid.is_none(),
        SupervisedProcessPhase::Running => {
            #[cfg(unix)]
            {
                record.process.pgid == Some(record.process.pid)
            }
            #[cfg(not(unix))]
            {
                record.process.pgid.is_none()
            }
        }
    }
}

fn running_process_group_is_valid(process: SupervisedProcess) -> bool {
    #[cfg(unix)]
    {
        process.pgid == Some(process.pid)
    }
    #[cfg(not(unix))]
    {
        process.pgid.is_none()
    }
}

fn write_atomic_synced(path: &Path, encoded: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("supervised process path has no parent: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("supervised-process");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(encoded)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{
        SupervisedProcess, SupervisedProcessIdentity, SupervisedProcessPhase,
        SupervisedProcessRecord, read_supervised_process, read_supervised_process_record,
        remove_supervised_process_record_if_matches, remove_supervised_process_record_if_same,
        write_supervised_process, write_supervised_process_record,
    };

    #[test]
    fn pid_file_gains_additive_pgid_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("child.pid");
        write_supervised_process(
            &path,
            SupervisedProcess {
                pid: 41,
                pgid: Some(41),
            },
        )
        .expect("write metadata");

        let raw = std::fs::read_to_string(&path).expect("read metadata");
        assert!(raw.contains("\"pid\":41"));
        assert!(raw.contains("\"pgid\":41"));
        assert_eq!(
            read_supervised_process(&path).expect("parse metadata"),
            SupervisedProcess {
                pid: 41,
                pgid: Some(41),
            }
        );
    }

    #[test]
    fn absent_pgid_key_reads_as_legacy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let json_path = temp.path().join("legacy-json.pid");
        std::fs::write(&json_path, "{\"pid\":42}\n").expect("write legacy json");
        assert_eq!(
            read_supervised_process(&json_path).expect("parse legacy json"),
            SupervisedProcess {
                pid: 42,
                pgid: None,
            }
        );

        let text_path = temp.path().join("legacy-text.pid");
        std::fs::write(&text_path, "43\n").expect("write legacy text");
        assert_eq!(
            read_supervised_process(&text_path).expect("parse legacy text"),
            SupervisedProcess {
                pid: 43,
                pgid: None,
            }
        );
    }

    #[test]
    fn guarded_record_is_atomic_identity_bound_and_compare_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("gate-launch.json");
        let record = SupervisedProcessRecord::prepared(
            SupervisedProcess {
                pid: std::process::id(),
                pgid: None,
            },
            "gate-launch-1".to_string(),
            2,
            Some("job-launch-1".to_string()),
            "sha256:release".to_string(),
        )
        .expect("current process identity");

        write_supervised_process_record(&path, &record).expect("write record");
        let parsed = read_supervised_process_record(&path).expect("read record");
        assert_eq!(parsed.phase, SupervisedProcessPhase::Prepared);
        assert_eq!(parsed.identity(), SupervisedProcessIdentity::Current);
        assert!(
            !remove_supervised_process_record_if_matches(
                &path,
                "different-launch",
                std::process::id()
            )
            .expect("mismatched cleanup")
        );
        assert!(path.exists());
        assert!(
            remove_supervised_process_record_if_matches(&path, "gate-launch-1", std::process::id())
                .expect("matched cleanup")
        );
        assert!(!path.exists());
    }

    #[test]
    fn running_record_captures_current_identity_and_compare_removes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("provider.json");
        let pid = std::process::id();
        let record = SupervisedProcessRecord::running(SupervisedProcess {
            pid,
            pgid: if cfg!(unix) { Some(pid) } else { None },
        })
        .expect("running record");

        write_supervised_process_record(&path, &record).expect("write record");
        let parsed = read_supervised_process_record(&path).expect("read record");
        assert_eq!(parsed, record);
        assert_eq!(parsed.phase, SupervisedProcessPhase::Running);
        assert_eq!(parsed.identity(), SupervisedProcessIdentity::Current);
        assert!(
            !remove_supervised_process_record_if_matches(&path, "another-launch", pid)
                .expect("mismatched cleanup")
        );
        assert!(path.exists());
        assert!(
            remove_supervised_process_record_if_matches(&path, &record.launch_id, pid)
                .expect("matched cleanup")
        );
        assert!(!path.exists());
    }

    #[test]
    fn replaced_pid_identity_is_refused_and_preserved_by_exact_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("provider.json");
        let pid = std::process::id();
        let record = SupervisedProcessRecord::running(SupervisedProcess {
            pid,
            pgid: if cfg!(unix) { Some(pid) } else { None },
        })
        .expect("running record");
        let mut replacement = record.clone();
        replacement.process_start_identity = "different-process-start".to_string();
        write_supervised_process_record(&path, &replacement).expect("write replacement record");

        assert_eq!(replacement.identity(), SupervisedProcessIdentity::Reused);
        assert!(
            !remove_supervised_process_record_if_same(&path, &record)
                .expect("replacement cleanup must fail closed")
        );
        assert!(path.exists(), "replacement identity evidence must remain");
    }

    #[test]
    fn record_reader_rejects_unknown_schema_and_invalid_group() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("gate-launch.json");
        std::fs::write(
            &path,
            format!(
                "{{\"schema_version\":99,\"pid\":{},\"pgid\":{},\"launch_id\":\"launch\",\"attempt\":1,\"release_token_sha256\":\"digest\",\"boot_id\":\"boot\",\"process_start_identity\":\"start\",\"phase\":\"running\"}}\n",
                std::process::id(),
                std::process::id() + 1
            ),
        )
        .expect("fixture");

        let error = read_supervised_process_record(&path).expect_err("invalid record");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn prepared_records_require_no_process_group() {
        let pid = std::process::id();
        let error = SupervisedProcessRecord::prepared(
            SupervisedProcess {
                pid,
                pgid: Some(pid),
            },
            "launch-prepared".to_string(),
            1,
            None,
            "digest".to_string(),
        )
        .expect_err("prepared records must not have a process group");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("supervised.json");
        let mut record = SupervisedProcessRecord::prepared(
            SupervisedProcess { pid, pgid: None },
            "launch-prepared".to_string(),
            1,
            None,
            "digest".to_string(),
        )
        .expect("prepared record");
        record.process.pgid = Some(pid);
        std::fs::write(&path, serde_json::to_vec(&record).expect("serialize"))
            .expect("write fixture");

        let error = read_supervised_process_record(&path).expect_err("invalid prepared record");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn running_records_require_platform_process_group_posture() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("supervised.json");
        let pid = std::process::id();
        let mut record = SupervisedProcessRecord::prepared(
            SupervisedProcess { pid, pgid: None },
            "launch-running".to_string(),
            1,
            None,
            "digest".to_string(),
        )
        .expect("prepared record");
        record.phase = SupervisedProcessPhase::Running;

        #[cfg(unix)]
        {
            std::fs::write(&path, serde_json::to_vec(&record).expect("serialize"))
                .expect("write fixture");
            let error =
                read_supervised_process_record(&path).expect_err("running record without pgid");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

            record.process.pgid = Some(pid);
        }

        #[cfg(not(unix))]
        {
            record.process.pgid = Some(pid);
            std::fs::write(&path, serde_json::to_vec(&record).expect("serialize"))
                .expect("write fixture");
            let error =
                read_supervised_process_record(&path).expect_err("running record with pgid");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

            record.process.pgid = None;
        }

        write_supervised_process_record(&path, &record).expect("valid running record");
        assert_eq!(
            read_supervised_process_record(&path).expect("read valid running record"),
            record
        );
    }
}
