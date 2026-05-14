use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const READY_ENV: &str = "DEADRECKON_SLEEP_REEXEC_READY_PATH";
const INHIBITED_ENV: &str = "DEADRECKON_SLEEP_INHIBITED";
const READY_DIR_PREFIX: &str = "deadreckon-sleep-";
const READY_FILE_NAME: &str = "reexec-ready";
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const READY_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepPrefs {
    Auto,
    On,
    Off,
}

#[derive(Debug)]
pub enum SleepPrevention {
    Active { handle: SleepHandle },
    Skipped { reason: SkipReason },
    Reexeced { exit_code: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    NonTty,
    UserDisabled,
    UnavailableBinary,
    AlreadyInhibited,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SleepPreview {
    pub mode: SleepMode,
    pub binary: Option<PathBuf>,
    pub skip_reason: Option<SkipReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SleepMode {
    Caffeinate,
    SystemdInhibit,
    None,
    Unsupported,
}

#[derive(Debug)]
pub struct SleepHandle {
    metadata_path: PathBuf,
    child: Option<Child>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SleepMetadata {
    pub mode: SleepMode,
    pub pid: Option<u32>,
    pub armed_at: DateTime<Utc>,
    pub inhibitor_binary: Option<PathBuf>,
    pub reason: String,
    pub skip_reason: Option<SkipReason>,
}

impl SleepPrefs {
    pub fn parse(value: Option<&str>, config: Option<&str>) -> Result<Self, String> {
        let value = value.or(config).unwrap_or("auto");
        match value {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => Err(format!(
                "invalid prevent_sleep value '{other}' (expected auto, on, or off)"
            )),
        }
    }
}

impl SleepPreview {
    pub fn label(&self) -> String {
        let mode = match self.mode {
            SleepMode::Caffeinate => "caffeinate",
            SleepMode::SystemdInhibit => "systemd-inhibit",
            SleepMode::None => "none",
            SleepMode::Unsupported => "unsupported",
        };
        if let Some(binary) = self.binary.as_ref() {
            format!("{mode} {}", binary.display())
        } else if let Some(reason) = self.skip_reason {
            format!("{mode} ({})", skip_reason_label(reason))
        } else {
            mode.to_string()
        }
    }
}

impl Drop for SleepHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            let _ = deadreckon_core::terminate_pid(pid, false);
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) => {
                        let _ = deadreckon_core::terminate_pid(pid, true);
                        let _ = child.wait();
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        let _ = fs::remove_file(&self.metadata_path);
    }
}

pub fn preview(prefs: SleepPrefs, is_tty: bool) -> SleepPreview {
    preview_with_binary_lookup(prefs, is_tty, find_binary)
}

pub fn preview_with_binary_lookup<F>(
    prefs: SleepPrefs,
    is_tty: bool,
    binary_lookup: F,
) -> SleepPreview
where
    F: Fn(&str) -> Option<PathBuf>,
{
    if prefs == SleepPrefs::Off {
        return SleepPreview {
            mode: SleepMode::None,
            binary: None,
            skip_reason: Some(SkipReason::UserDisabled),
        };
    }
    if prefs == SleepPrefs::Auto && !is_tty {
        return SleepPreview {
            mode: SleepMode::None,
            binary: None,
            skip_reason: Some(SkipReason::NonTty),
        };
    }
    if already_inhibited() {
        return SleepPreview {
            mode: SleepMode::SystemdInhibit,
            binary: binary_lookup("systemd-inhibit"),
            skip_reason: Some(SkipReason::AlreadyInhibited),
        };
    }
    platform_preview_with_lookup(&binary_lookup)
}

pub fn maybe_reexec_for_linux(prefs: SleepPrefs, is_tty: bool) -> io::Result<Option<i32>> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    if prefs == SleepPrefs::Off || (prefs == SleepPrefs::Auto && !is_tty) || already_inhibited() {
        signal_linux_reexec_ready_best_effort();
        return Ok(None);
    }
    let Some(binary) = find_binary("systemd-inhibit") else {
        return Ok(None);
    };
    let ready_dir = create_ready_dir()?;
    let ready_path = ready_dir.join(READY_FILE_NAME);
    let current_exe = std::env::current_exe()?;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut command = Command::new(binary);
    command
        .arg("--what=sleep")
        .arg("--who=deadreckon")
        .arg("--why=deadreckon overnight run")
        .arg("--mode=block")
        .arg(current_exe)
        .args(args)
        .env(INHIBITED_ENV, "1")
        .env(READY_ENV, &ready_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = fs::remove_dir_all(&ready_dir);
            if err.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(err);
        }
    };
    let ready = wait_for_ready_or_exit(&ready_path, &mut child, READY_TIMEOUT)?;
    if !ready {
        let _ = deadreckon_core::terminate_pid(child.id(), true);
        let _ = child.wait();
        let _ = fs::remove_dir_all(&ready_dir);
        return Ok(None);
    }
    let status = child.wait()?;
    let _ = fs::remove_dir_all(&ready_dir);
    Ok(Some(status.code().unwrap_or(1)))
}

pub fn arm(prefs: SleepPrefs, working_dir: &Path) -> io::Result<SleepPrevention> {
    arm_with_tty(prefs, working_dir, std::io::stdin().is_terminal())
}

pub fn arm_with_tty(
    prefs: SleepPrefs,
    working_dir: &Path,
    is_tty: bool,
) -> io::Result<SleepPrevention> {
    if prefs == SleepPrefs::Off {
        return Ok(SleepPrevention::Skipped {
            reason: SkipReason::UserDisabled,
        });
    }
    if prefs == SleepPrefs::Auto && !is_tty {
        return Ok(SleepPrevention::Skipped {
            reason: SkipReason::NonTty,
        });
    }

    if cfg!(target_os = "macos") {
        arm_macos(working_dir)
    } else if cfg!(target_os = "linux") {
        if already_inhibited() {
            signal_linux_reexec_ready_best_effort();
            let metadata_path = write_metadata(
                working_dir,
                &SleepMetadata {
                    mode: SleepMode::SystemdInhibit,
                    pid: Some(std::process::id()),
                    armed_at: Utc::now(),
                    inhibitor_binary: find_binary("systemd-inhibit"),
                    reason: "deadreckon overnight run".to_string(),
                    skip_reason: None,
                },
            )?;
            return Ok(SleepPrevention::Active {
                handle: SleepHandle {
                    metadata_path,
                    child: None,
                },
            });
        }
        Ok(SleepPrevention::Skipped {
            reason: SkipReason::UnavailableBinary,
        })
    } else {
        Ok(SleepPrevention::Skipped {
            reason: SkipReason::Unsupported,
        })
    }
}

pub fn metadata_path(working_dir: &Path) -> PathBuf {
    working_dir
        .join(".deadreckon")
        .join("sleep-prevention.json")
}

pub fn is_trusted_linux_ready_path(path: &Path) -> bool {
    let Ok(path) = path
        .canonicalize()
        .or_else(|_| Ok::<PathBuf, io::Error>(path.to_path_buf()))
    else {
        return false;
    };
    if path.file_name().and_then(|name| name.to_str()) != Some(READY_FILE_NAME) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(dirname) = parent.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !dirname.starts_with(READY_DIR_PREFIX) {
        return false;
    }
    let Some(grandparent) = parent.parent() else {
        return false;
    };
    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    grandparent == tmp
}

pub fn skip_reason_label(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::NonTty => "non-tty",
        SkipReason::UserDisabled => "user-disabled",
        SkipReason::UnavailableBinary => "unavailable",
        SkipReason::AlreadyInhibited => "already-inhibited",
        SkipReason::Unsupported => "unsupported",
    }
}

fn arm_macos(working_dir: &Path) -> io::Result<SleepPrevention> {
    let Some(binary) = find_binary("caffeinate").or_else(|| {
        let fallback = PathBuf::from("/usr/bin/caffeinate");
        fallback.is_file().then_some(fallback)
    }) else {
        return Ok(SleepPrevention::Skipped {
            reason: SkipReason::UnavailableBinary,
        });
    };
    let mut command = Command::new(&binary);
    command
        .arg("-di")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let child = command.spawn()?;
    let metadata_path = write_metadata(
        working_dir,
        &SleepMetadata {
            mode: SleepMode::Caffeinate,
            pid: Some(child.id()),
            armed_at: Utc::now(),
            inhibitor_binary: Some(binary),
            reason: "deadreckon overnight run".to_string(),
            skip_reason: None,
        },
    )?;
    Ok(SleepPrevention::Active {
        handle: SleepHandle {
            metadata_path,
            child: Some(child),
        },
    })
}

fn write_metadata(working_dir: &Path, metadata: &SleepMetadata) -> io::Result<PathBuf> {
    let path = metadata_path(working_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&path)?;
    serde_json::to_writer_pretty(&mut file, metadata).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    Ok(path)
}

fn platform_preview_with_lookup<F>(binary_lookup: &F) -> SleepPreview
where
    F: Fn(&str) -> Option<PathBuf>,
{
    if cfg!(target_os = "macos") {
        match binary_lookup("caffeinate").or_else(|| binary_lookup("/usr/bin/caffeinate")) {
            Some(binary) => SleepPreview {
                mode: SleepMode::Caffeinate,
                binary: Some(binary),
                skip_reason: None,
            },
            None => SleepPreview {
                mode: SleepMode::Unsupported,
                binary: None,
                skip_reason: Some(SkipReason::UnavailableBinary),
            },
        }
    } else if cfg!(target_os = "linux") {
        match binary_lookup("systemd-inhibit") {
            Some(binary) => SleepPreview {
                mode: SleepMode::SystemdInhibit,
                binary: Some(binary),
                skip_reason: None,
            },
            None => SleepPreview {
                mode: SleepMode::Unsupported,
                binary: None,
                skip_reason: Some(SkipReason::UnavailableBinary),
            },
        }
    } else {
        SleepPreview {
            mode: SleepMode::Unsupported,
            binary: None,
            skip_reason: Some(SkipReason::Unsupported),
        }
    }
}

fn already_inhibited() -> bool {
    std::env::var_os(INHIBITED_ENV).is_some()
}

fn signal_linux_reexec_ready_best_effort() {
    let Some(path) = std::env::var_os(READY_ENV).map(PathBuf::from) else {
        return;
    };
    if !is_trusted_linux_ready_path(&path) {
        return;
    }
    let _ = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .and_then(|mut file| file.write_all(b"ready\n"));
}

fn wait_for_ready_or_exit(path: &Path, child: &mut Child, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            return Ok(true);
        }
        if child.try_wait()?.is_some() {
            return Ok(path.exists());
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(READY_POLL);
    }
}

fn create_ready_dir() -> io::Result<PathBuf> {
    let mut attempts = 0u8;
    loop {
        let candidate = std::env::temp_dir().join(format!(
            "{READY_DIR_PREFIX}{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempts < 8 => {
                attempts += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

fn find_binary(binary: &str) -> Option<PathBuf> {
    let path = Path::new(binary);
    if path.components().count() > 1 && path.is_file() {
        return Some(path.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

#[allow(dead_code)]
fn os_args_without_binary() -> Vec<OsString> {
    std::env::args_os().skip(1).collect()
}
