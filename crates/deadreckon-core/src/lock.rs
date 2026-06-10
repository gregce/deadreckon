use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::DeadreckonPaths;

pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockState {
    /// Scoped owner key. Normal runs use a task key; chain conductors use the
    /// reserved `chain--<chain-id>` prefix while keeping the same lock format.
    pub task_key: String,
    pub run_id: String,
    pub scope: String,
    pub phase: String,
    pub pid: u32,
    pub acquired_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockStatus {
    pub held: bool,
    pub stale: bool,
    pub alive: bool,
    pub state: Option<LockState>,
}

#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
    file: File,
    state: LockState,
    released: bool,
}

impl LockGuard {
    pub fn state(&self) -> &LockState {
        &self.state
    }

    pub fn heartbeat(&mut self, phase: impl Into<String>) -> Result<()> {
        self.state.phase = phase.into();
        self.state.updated_at = Utc::now();
        write_lock_state_to_file(&mut self.file, &self.path, &self.state)
    }

    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<()> {
        if self.released {
            return Ok(());
        }
        self.file.unlock().with_path(&self.path)?;
        match fs::remove_file(&self.path) {
            Ok(()) => {
                self.released = true;
                Ok(())
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                self.released = true;
                Ok(())
            }
            Err(source) => Err(DeadreckonError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn lock_path(paths: &DeadreckonPaths, scope: &str, task_key: &str) -> PathBuf {
    paths.locks_dir().join(format!("{scope}--{task_key}.lock"))
}

pub fn acquire_lock(
    paths: &DeadreckonPaths,
    task_key: &str,
    run_id: &str,
    scope: &str,
    phase: &str,
    // Kept for status displays via lock_status; never authorizes reclaim.
    _stale_after: Duration,
) -> Result<LockGuard> {
    // REPORT.md: Multi-Agent Worktree Coordination Layer starts with scoped
    // locks so parallel agents do not claim the same task at once.
    fs::create_dir_all(paths.locks_dir()).with_path(paths.locks_dir())?;
    let path = lock_path(paths, scope, task_key);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_path(&path)?;

    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
            if let Some(existing) = read_lock_state(&path)? {
                return Err(DeadreckonError::LockHeld {
                    task_key: task_key.to_string(),
                    heartbeat_age_seconds: heartbeat_age_seconds(&existing),
                    run_id: existing.run_id,
                    phase: existing.phase,
                });
            }
            return Err(DeadreckonError::Io { path, source });
        }
        Err(source) => return Err(DeadreckonError::Io { path, source }),
    }

    let existing = read_lock_state_from_file(&mut file, &path)?;
    if let Some(existing) = existing {
        let alive = pid_is_alive(existing.pid);
        let same_owner = existing.run_id == run_id;
        // An alive holder is never usurped, however stale its heartbeat —
        // a wedged-but-running owner still owns its working tree. Reclaim
        // requires a dead pid; `deadreckon kill <run-id> --force` is the
        // operator path past a wedged holder.
        if !same_owner && alive {
            file.unlock().with_path(&path)?;
            return Err(DeadreckonError::LockHeld {
                task_key: task_key.to_string(),
                heartbeat_age_seconds: heartbeat_age_seconds(&existing),
                run_id: existing.run_id,
                phase: existing.phase,
            });
        }
    }

    // AS-BUILT §6: process-held locks are paired with a durable heartbeat so
    // a successor can distinguish live work from crash residue.
    let now = Utc::now();
    let state = LockState {
        task_key: task_key.to_string(),
        run_id: run_id.to_string(),
        scope: scope.to_string(),
        phase: phase.to_string(),
        pid: std::process::id(),
        acquired_at: now,
        updated_at: now,
    };
    write_lock_state_to_file(&mut file, &path, &state)?;
    Ok(LockGuard {
        path,
        file,
        state,
        released: false,
    })
}

pub fn lock_status(
    paths: &DeadreckonPaths,
    scope: &str,
    task_key: &str,
    stale_after: Duration,
) -> Result<LockStatus> {
    let path = lock_path(paths, scope, task_key);
    let Some(state) = read_lock_state(&path)? else {
        return Ok(LockStatus {
            held: false,
            stale: false,
            alive: false,
            state: None,
        });
    };
    let stale = lock_is_stale(&state, stale_after);
    let alive = pid_is_alive(state.pid);
    Ok(LockStatus {
        held: true,
        stale,
        alive,
        state: Some(state),
    })
}

pub fn release_lock_file(paths: &DeadreckonPaths, scope: &str, task_key: &str) -> Result<()> {
    let path = lock_path(paths, scope, task_key);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

/// Advisory staleness for status displays. A stale heartbeat never
/// authorizes reclaiming the lock — only a dead holder pid does.
pub fn lock_is_stale(state: &LockState, stale_after: Duration) -> bool {
    let age = Utc::now()
        .signed_duration_since(state.updated_at)
        .to_std()
        .unwrap_or(Duration::ZERO);
    age > stale_after || !pid_is_alive(state.pid)
}

pub fn heartbeat_age_seconds(state: &LockState) -> u64 {
    Utc::now()
        .signed_duration_since(state.updated_at)
        .to_std()
        .map(|age| age.as_secs())
        .unwrap_or(0)
}

pub fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    pid_is_alive_platform(pid)
}

#[cfg(unix)]
pub fn terminate_pid(pid: u32, force: bool) -> Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let signal = if force {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    match kill(Pid::from_raw(pid as i32), Some(signal)) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(DeadreckonError::InvalidInput(format!(
            "failed to signal pid {pid}: {err}"
        ))),
    }
}

#[cfg(not(unix))]
pub fn terminate_pid(_pid: u32, _force: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn pid_is_alive_platform(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(Errno::ESRCH) => false,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn pid_is_alive_platform(pid: u32) -> bool {
    pid == std::process::id()
}

fn read_lock_state(path: &Path) -> Result<Option<LockState>> {
    match fs::read(path) {
        Ok(data) if data.is_empty() => Ok(None),
        Ok(data) => serde_json::from_slice(&data).map(Some).with_json_path(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_lock_state_from_file(file: &mut File, path: &Path) -> Result<Option<LockState>> {
    file.seek(SeekFrom::Start(0)).with_path(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).with_path(path)?;
    if data.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&data).map(Some).with_json_path(path)
}

fn write_lock_state_to_file(file: &mut File, path: &Path, state: &LockState) -> Result<()> {
    file.set_len(0).with_path(path)?;
    file.seek(SeekFrom::Start(0)).with_path(path)?;
    serde_json::to_writer_pretty(&mut *file, state).with_json_path(path)?;
    file.write_all(b"\n").with_path(path)?;
    file.sync_all().with_path(path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, Utc};
    use tempfile::TempDir;

    use super::{LockState, acquire_lock, lock_path, lock_status};
    use crate::DeadreckonError;
    use crate::paths::DeadreckonPaths;

    #[test]
    fn held_file_lock_blocks_same_scope_second_owner() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let guard = acquire_lock(
            &paths,
            "task",
            "run-a",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect("first lock");

        let err = acquire_lock(
            &paths,
            "task",
            "run-b",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect_err("second lock must fail");
        assert!(matches!(err, DeadreckonError::LockHeld { .. }));
        drop(guard);
    }

    #[test]
    fn concurrent_runs_no_state_bleed() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let guard_a = acquire_lock(
            &paths,
            "task",
            "run-a",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect("scope a lock");
        let guard_b = acquire_lock(
            &paths,
            "task",
            "run-b",
            "scope-b",
            "execute",
            Duration::from_secs(60),
        )
        .expect("scope b lock");
        assert_ne!(guard_a.state().scope, guard_b.state().scope);
    }

    #[test]
    fn same_scope_second_run_refused_with_hint() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let _guard = acquire_lock(
            &paths,
            "task",
            "run-a",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect("first lock");
        let err = acquire_lock(
            &paths,
            "task",
            "run-b",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect_err("same scope refused");
        let message = err.to_string();
        assert!(message.contains("lock held"));
        assert!(message.contains("run-a"));
    }

    #[test]
    fn stale_lock_reclaims_with_dead_pid() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        fs::create_dir_all(paths.locks_dir()).expect("locks dir");
        let path = lock_path(&paths, "new-scope", "task");
        let stale = LockState {
            task_key: "task".to_string(),
            run_id: "old-run".to_string(),
            scope: "old-scope".to_string(),
            phase: "execute".to_string(),
            pid: 999_999,
            acquired_at: Utc::now() - ChronoDuration::minutes(60),
            updated_at: Utc::now() - ChronoDuration::minutes(60),
        };
        fs::write(&path, serde_json::to_vec_pretty(&stale).expect("json")).expect("write stale");

        let guard = acquire_lock(
            &paths,
            "task",
            "new-run",
            "new-scope",
            "resume",
            Duration::from_secs(60),
        )
        .expect("reclaim");
        assert_eq!(guard.state().run_id, "new-run");
    }

    #[test]
    fn alive_holder_with_ancient_heartbeat_is_not_reclaimed() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        fs::create_dir_all(paths.locks_dir()).expect("locks dir");
        let path = lock_path(&paths, "scope-x", "task");
        let busy = LockState {
            task_key: "task".to_string(),
            run_id: "busy-run".to_string(),
            scope: "scope-x".to_string(),
            phase: "execute".to_string(),
            pid: std::process::id(),
            acquired_at: Utc::now() - ChronoDuration::minutes(90),
            updated_at: Utc::now() - ChronoDuration::minutes(90),
        };
        fs::write(&path, serde_json::to_vec_pretty(&busy).expect("json")).expect("write busy");

        let err = acquire_lock(
            &paths,
            "task",
            "usurper-run",
            "scope-x",
            "execute",
            Duration::from_secs(60),
        )
        .expect_err("alive holder must never be usurped, however old the heartbeat");
        assert!(matches!(err, DeadreckonError::LockHeld { .. }));
    }

    #[test]
    fn dead_holder_is_reclaimed_regardless_of_heartbeat_age() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        fs::create_dir_all(paths.locks_dir()).expect("locks dir");
        let path = lock_path(&paths, "scope-y", "task");
        let dead = LockState {
            task_key: "task".to_string(),
            run_id: "dead-run".to_string(),
            scope: "scope-y".to_string(),
            phase: "execute".to_string(),
            pid: 999_999,
            acquired_at: Utc::now(),
            updated_at: Utc::now(),
        };
        fs::write(&path, serde_json::to_vec_pretty(&dead).expect("json")).expect("write dead");

        let guard = acquire_lock(
            &paths,
            "task",
            "successor-run",
            "scope-y",
            "execute",
            Duration::from_secs(3600),
        )
        .expect("dead holder reclaims even with a fresh heartbeat");
        assert_eq!(guard.state().run_id, "successor-run");
    }

    #[test]
    fn lockheld_refusal_names_heartbeat_age_and_force_kill_try_line() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        fs::create_dir_all(paths.locks_dir()).expect("locks dir");
        let path = lock_path(&paths, "scope-z", "task");
        let busy = LockState {
            task_key: "task".to_string(),
            run_id: "busy-run".to_string(),
            scope: "scope-z".to_string(),
            phase: "verify".to_string(),
            pid: std::process::id(),
            acquired_at: Utc::now() - ChronoDuration::minutes(2),
            updated_at: Utc::now() - ChronoDuration::minutes(2),
        };
        fs::write(&path, serde_json::to_vec_pretty(&busy).expect("json")).expect("write busy");

        let err = acquire_lock(
            &paths,
            "task",
            "usurper-run",
            "scope-z",
            "execute",
            Duration::from_secs(60),
        )
        .expect_err("refused");
        let message = err.to_string();
        assert!(message.contains("busy-run"), "{message}");
        assert!(message.contains("heartbeat"), "{message}");
        assert!(
            message.contains("s ago") || message.contains("seconds"),
            "{message}"
        );
    }

    #[test]
    fn heartbeat_updates_phase_and_status_probe() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let mut guard = acquire_lock(
            &paths,
            "task",
            "run-a",
            "scope-a",
            "execute",
            Duration::from_secs(60),
        )
        .expect("lock");
        guard.heartbeat("verify").expect("heartbeat");

        let status =
            lock_status(&paths, "scope-a", "task", Duration::from_secs(60)).expect("status");
        let state = status.state.expect("state");
        assert!(status.held);
        assert!(status.alive);
        assert_eq!(state.phase, "verify");
    }
}
