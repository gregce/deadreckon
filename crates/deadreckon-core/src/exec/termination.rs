use std::process::{Child, Command};
use std::time::Duration;

/// Result of a bounded TERM-then-KILL termination attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationOutcome {
    ExitedInGrace,
    Killed,
    AlreadyDead,
    Failed(String),
}

pub trait ChildTerminator: Send + Sync {
    fn terminate(&self, grace: Duration) -> TerminationOutcome;
}

#[derive(Debug, Clone, Copy)]
pub struct RawPidTerminator {
    pid: u32,
}

impl RawPidTerminator {
    pub const fn new(pid: u32) -> Self {
        Self { pid }
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub struct ProcessGroupTerminator {
    pgid: i32,
}

#[cfg(unix)]
impl ProcessGroupTerminator {
    pub const fn new(pgid: i32) -> Self {
        Self { pgid }
    }

    pub const fn pgid(&self) -> i32 {
        self.pgid
    }
}

#[cfg(unix)]
pub fn spawn_grouped(mut command: Command) -> std::io::Result<(Child, Box<dyn ChildTerminator>)> {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
    let child = command.spawn()?;
    let terminator = ProcessGroupTerminator::new(child.id() as i32);
    Ok((child, Box::new(terminator)))
}

#[cfg(not(unix))]
pub fn spawn_grouped(mut command: Command) -> std::io::Result<(Child, Box<dyn ChildTerminator>)> {
    let child = command.spawn()?;
    let terminator = RawPidTerminator::new(child.id());
    Ok((child, Box::new(terminator)))
}

#[cfg(unix)]
impl ChildTerminator for ProcessGroupTerminator {
    fn terminate(&self, grace: Duration) -> TerminationOutcome {
        if self.pgid <= 0 {
            return TerminationOutcome::Failed(format!("invalid process group id {}", self.pgid));
        }
        terminate_unix(-self.pgid, grace, "process group")
    }
}

#[cfg(unix)]
impl ChildTerminator for RawPidTerminator {
    fn terminate(&self, grace: Duration) -> TerminationOutcome {
        let Ok(pid) = i32::try_from(self.pid) else {
            return TerminationOutcome::Failed(format!("invalid pid {}", self.pid));
        };
        if pid == 0 {
            return TerminationOutcome::Failed("invalid pid 0".to_string());
        }
        terminate_unix(pid, grace, "pid")
    }
}

#[cfg(not(unix))]
impl ChildTerminator for RawPidTerminator {
    fn terminate(&self, _grace: Duration) -> TerminationOutcome {
        TerminationOutcome::Failed(format!(
            "raw pid termination is unavailable on this platform for pid {}",
            self.pid
        ))
    }
}

#[cfg(unix)]
fn terminate_unix(target: i32, grace: Duration, label: &str) -> TerminationOutcome {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let target = Pid::from_raw(target);
    match kill(target, None) {
        Ok(()) | Err(Errno::EPERM) => {}
        Err(Errno::ESRCH) => return TerminationOutcome::AlreadyDead,
        Err(error) => {
            return TerminationOutcome::Failed(format!(
                "failed to inspect {label} {}: {error}",
                target.as_raw().unsigned_abs()
            ));
        }
    }

    match kill(target, Some(Signal::SIGTERM)) {
        Ok(()) => {}
        Err(Errno::ESRCH) => return TerminationOutcome::AlreadyDead,
        Err(error) => {
            return TerminationOutcome::Failed(format!(
                "failed to send SIGTERM to {label} {}: {error}",
                target.as_raw().unsigned_abs()
            ));
        }
    }

    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        match kill(target, None) {
            Err(Errno::ESRCH) => return TerminationOutcome::ExitedInGrace,
            // Some Unix kernels report EPERM for a group whose remaining
            // members are only unreaped zombies. We already sent TERM to this
            // same target successfully, so there is no signalable process
            // left to escalate.
            Err(Errno::EPERM) => return TerminationOutcome::ExitedInGrace,
            Ok(()) => {
                std::thread::sleep(poll_interval(deadline));
            }
            Err(error) => {
                return TerminationOutcome::Failed(format!(
                    "failed to inspect {label} {} after SIGTERM: {error}",
                    target.as_raw().unsigned_abs()
                ));
            }
        }
    }

    match kill(target, Some(Signal::SIGKILL)) {
        Ok(()) => TerminationOutcome::Killed,
        Err(Errno::ESRCH) | Err(Errno::EPERM) => TerminationOutcome::ExitedInGrace,
        Err(error) => TerminationOutcome::Failed(format!(
            "failed to send SIGKILL to {label} {}: {error}",
            target.as_raw().unsigned_abs()
        )),
    }
}

#[cfg(unix)]
fn poll_interval(deadline: std::time::Instant) -> Duration {
    deadline
        .saturating_duration_since(std::time::Instant::now())
        .min(Duration::from_millis(10))
}

#[cfg(all(test, unix))]
mod tests {
    use super::{TerminationOutcome, spawn_grouped};
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::{Pid, getpgid};
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn spawned_child_is_its_own_process_group() {
        let mut command = Command::new("sleep");
        command.arg("60");
        let (mut child, terminator) = spawn_grouped(command).expect("spawn grouped child");

        assert_eq!(
            getpgid(Some(Pid::from_raw(child.id() as i32))).expect("read process group"),
            Pid::from_raw(child.id() as i32)
        );

        let _ = terminator.terminate(Duration::ZERO);
        let _ = child.wait();
    }

    #[test]
    fn group_terminate_kills_child_tree_no_orphans() {
        let temp = tempfile::tempdir().expect("tempdir");
        let child_pid_path = temp.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 60 & child=$!; printf '%s\\n' \"$child\" > \"$CHILD_PID_FILE\"; wait")
            .env("CHILD_PID_FILE", &child_pid_path);
        let (mut group_leader, terminator) =
            spawn_grouped(command).expect("spawn grouped child tree");
        wait_for(|| child_pid_path.exists(), Duration::from_secs(3));
        let child_pid = std::fs::read_to_string(&child_pid_path)
            .expect("read child pid")
            .trim()
            .parse::<i32>()
            .expect("parse child pid");

        let outcome = terminator.terminate(Duration::from_millis(250));
        assert!(
            matches!(
                &outcome,
                TerminationOutcome::ExitedInGrace | TerminationOutcome::Killed
            ),
            "unexpected termination outcome: {outcome:?}"
        );
        let _ = group_leader.wait();
        wait_for(|| !pid_is_alive(child_pid), Duration::from_secs(3));
        assert!(
            !pid_is_alive(child_pid),
            "child process {child_pid} survived"
        );
    }

    #[test]
    fn term_then_kill_escalation_honors_grace() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ready_path = temp.path().join("ready");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("trap '' TERM; : > \"$READY_FILE\"; while :; do sleep 1; done")
            .env("READY_FILE", &ready_path);
        let (mut child, terminator) = spawn_grouped(command).expect("spawn TERM-resistant child");
        wait_for(|| ready_path.exists(), Duration::from_secs(3));

        let grace = Duration::from_millis(150);
        let started = Instant::now();
        let outcome = terminator.terminate(grace);
        assert_eq!(outcome, TerminationOutcome::Killed);
        assert!(
            started.elapsed() >= grace,
            "SIGKILL was sent before the grace period elapsed"
        );
        let _ = child.wait();
    }

    #[test]
    fn already_dead_child_reports_cleanly() {
        let command = Command::new("true");
        let (mut child, terminator) = spawn_grouped(command).expect("spawn short child");
        child.wait().expect("reap child");

        assert_eq!(
            terminator.terminate(Duration::from_millis(10)),
            TerminationOutcome::AlreadyDead
        );
    }

    fn wait_for(mut condition: impl FnMut() -> bool, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !condition() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn pid_is_alive(pid: i32) -> bool {
        match kill(Pid::from_raw(pid), None) {
            Ok(()) | Err(Errno::EPERM) => true,
            Err(Errno::ESRCH) => false,
            Err(_) => false,
        }
    }
}
