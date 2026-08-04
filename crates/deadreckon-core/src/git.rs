use std::ffi::OsString;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{DeadreckonError, Result};

const COMMIT_FAMILY_VERBS: &[&str] = &[
    "commit",
    "merge",
    "cherry-pick",
    "rebase",
    "tag",
    "am",
    "revert",
];

pub fn run_git(cwd: &Path, args: &[&str]) -> Result<Output> {
    git_command(cwd, args).output().map_err(git_io)
}

pub fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output> {
    let mut command = git_command(cwd, args);
    run_git_command_with_input(cwd, &mut command, input)
}

/// Absolute boundaries for one bounded Git subprocess.
///
/// `work_expires_at` is shared with the enclosing run phase; it must not be
/// recomputed for retries or nested Git calls. `cleanup_budget` starts only
/// after work is interrupted, so useful work can never consume containment
/// time.
pub struct GitCommandDeadline<'a> {
    pub work_expires_at: Instant,
    pub cleanup_budget: Duration,
    pub cancellation_requested: &'a dyn Fn() -> bool,
    pub authority_dir: Option<&'a Path>,
}

impl<'a> GitCommandDeadline<'a> {
    pub fn new(
        work_expires_at: Instant,
        cleanup_budget: Duration,
        cancellation_requested: &'a dyn Fn() -> bool,
    ) -> Self {
        Self {
            work_expires_at,
            cleanup_budget,
            cancellation_requested,
            authority_dir: None,
        }
    }

    pub fn with_authority_dir(mut self, authority_dir: &'a Path) -> Self {
        self.authority_dir = Some(authority_dir);
        self
    }
}

/// The boundary that interrupted a bounded Git subprocess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitCommandBoundary {
    WorkExpired,
    Cancelled,
    SupervisionFailure,
}

/// Recoverable authority for a Git process tree whose cleanup was not proven.
///
/// Callers must persist this value before reporting a terminal state. On Unix,
/// `process_group` owns the whole tree; on platforms without process groups,
/// only the direct `pid` can be represented and cleanup therefore fails closed
/// when the direct child cannot be reaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitProcessAuthority {
    pub pid: u32,
    pub process_group: Option<u32>,
    pub record_path: Option<PathBuf>,
    pub launch_id: Option<String>,
}

/// Result of running Git under an absolute work boundary.
#[derive(Debug)]
pub enum BoundedGitOutcome {
    Completed(Output),
    WorkExpired,
    Cancelled,
    SupervisionFailed {
        detail: String,
    },
    CleanupIncomplete {
        boundary: GitCommandBoundary,
        authority: GitProcessAuthority,
        detail: String,
    },
}

/// Run Git with no stdin while enforcing one work deadline and one cleanup
/// deadline for the entire owned process group.
pub fn run_git_bounded(
    cwd: &Path,
    args: &[&str],
    deadline: GitCommandDeadline<'_>,
) -> Result<BoundedGitOutcome> {
    let mut command = git_command(cwd, args);
    run_git_command_bounded(cwd, &mut command, None, deadline)
}

/// Run Git with stdin while enforcing one work deadline and one cleanup
/// deadline for the entire owned process group.
pub fn run_git_with_input_bounded(
    cwd: &Path,
    args: &[&str],
    input: &[u8],
    deadline: GitCommandDeadline<'_>,
) -> Result<BoundedGitOutcome> {
    let mut command = git_command(cwd, args);
    run_git_command_bounded(cwd, &mut command, Some(input), deadline)
}

/// Run a preconfigured Git command with stdin under absolute work and cleanup
/// boundaries.
pub fn run_git_command_with_input_bounded(
    cwd: &Path,
    command: &mut Command,
    input: &[u8],
    deadline: GitCommandDeadline<'_>,
) -> Result<BoundedGitOutcome> {
    run_git_command_bounded(cwd, command, Some(input), deadline)
}

/// Run a preconfigured Git command while communicating over all three pipes.
///
/// The input writer must run concurrently with stdout/stderr collection. Git
/// commands such as `check-attr --stdin` emit output while they consume input;
/// writing all input first can fill both pipe directions and deadlock.
pub fn run_git_command_with_input(
    cwd: &Path,
    command: &mut Command,
    input: &[u8],
) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| git_io_at(cwd, source))?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        DeadreckonError::InvalidInput("Git command did not expose its input pipe".to_string())
    })?;

    let (write_result, output_result) = thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        // `wait_with_output` drains stdout and stderr concurrently on supported
        // platforms. Starting it before joining the writer keeps every pipe
        // moving even when input and output both exceed the OS pipe capacity.
        let output = child.wait_with_output();
        (writer.join(), output)
    });
    let write_result = write_result.map_err(|_| {
        DeadreckonError::InvalidInput("Git input writer thread panicked".to_string())
    })?;
    write_result.map_err(|source| git_io_at(cwd, source))?;
    output_result.map_err(|source| git_io_at(cwd, source))
}

fn run_git_command_bounded(
    cwd: &Path,
    command: &mut Command,
    input: Option<&[u8]>,
    deadline: GitCommandDeadline<'_>,
) -> Result<BoundedGitOutcome> {
    if (deadline.cancellation_requested)() {
        return Ok(BoundedGitOutcome::Cancelled);
    }
    if Instant::now() >= deadline.work_expires_at {
        return Ok(BoundedGitOutcome::WorkExpired);
    }

    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_owned_process_group(command);
    let mut child = command.spawn().map_err(|source| git_io_at(cwd, source))?;
    let mut authority = GitProcessAuthority {
        pid: child.id(),
        process_group: owned_process_group(child.id()),
        record_path: None,
        launch_id: None,
    };
    let pipes = GitPipeThreads::spawn(&mut child, input)?;
    if let Some(authority_dir) = deadline.authority_dir {
        if let Err(error) = persist_git_process_authority(authority_dir, &mut authority) {
            return cleanup_bounded_git(
                cwd,
                &mut child,
                pipes,
                None,
                authority,
                GitCommandBoundary::SupervisionFailure,
                deadline.cleanup_budget,
                Some(format!("could not persist Git process authority: {error}")),
            );
        }
    }
    let mut status = None;

    loop {
        let boundary = if (deadline.cancellation_requested)() {
            Some(GitCommandBoundary::Cancelled)
        } else if Instant::now() >= deadline.work_expires_at {
            Some(GitCommandBoundary::WorkExpired)
        } else {
            None
        };
        if let Some(boundary) = boundary {
            return cleanup_bounded_git(
                cwd,
                &mut child,
                pipes,
                status,
                authority,
                boundary,
                deadline.cleanup_budget,
                None,
            );
        }

        if status.is_none() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(error) => {
                    return cleanup_bounded_git(
                        cwd,
                        &mut child,
                        pipes,
                        status,
                        authority,
                        GitCommandBoundary::SupervisionFailure,
                        deadline.cleanup_budget,
                        Some(format!("could not inspect Git child status: {error}")),
                    );
                }
            }
        }
        if let Some(status) = status {
            match process_authority_is_alive(&authority) {
                Ok(false) if pipes.is_finished() => {
                    let output = pipes.completed_output(cwd, status)?;
                    remove_git_process_authority(&authority)?;
                    return Ok(BoundedGitOutcome::Completed(output));
                }
                Ok(_) => {}
                Err(error) => {
                    return cleanup_bounded_git(
                        cwd,
                        &mut child,
                        pipes,
                        Some(status),
                        authority,
                        GitCommandBoundary::SupervisionFailure,
                        deadline.cleanup_budget,
                        Some(format!("could not inspect Git process authority: {error}")),
                    );
                }
            }
        }

        thread::sleep(poll_interval(deadline.work_expires_at));
    }
}

struct GitPipeThreads {
    stdin: Option<thread::JoinHandle<std::io::Result<()>>>,
    stdout: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr: thread::JoinHandle<std::io::Result<Vec<u8>>>,
}

impl GitPipeThreads {
    fn spawn(child: &mut Child, input: Option<&[u8]>) -> Result<Self> {
        let stdin = match input {
            Some(input) => {
                let mut pipe = child.stdin.take().ok_or_else(|| {
                    DeadreckonError::InvalidInput(
                        "bounded Git command did not expose its input pipe".to_string(),
                    )
                })?;
                let input = input.to_vec();
                Some(thread::spawn(move || pipe.write_all(&input)))
            }
            None => None,
        };
        let stdout = child.stdout.take().ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "bounded Git command did not expose its stdout pipe".to_string(),
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "bounded Git command did not expose its stderr pipe".to_string(),
            )
        })?;

        Ok(Self {
            stdin,
            stdout: spawn_pipe_reader(stdout),
            stderr: spawn_pipe_reader(stderr),
        })
    }

    fn is_finished(&self) -> bool {
        self.stdin
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
            && self.stdout.is_finished()
            && self.stderr.is_finished()
    }

    fn completed_output(self, cwd: &Path, status: ExitStatus) -> Result<Output> {
        if let Some(stdin) = self.stdin {
            join_git_pipe(cwd, stdin, "input writer")?;
        }
        let stdout = join_git_pipe(cwd, self.stdout, "stdout reader")?;
        let stderr = join_git_pipe(cwd, self.stderr, "stderr reader")?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

fn spawn_pipe_reader<R>(mut pipe: R) -> thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_git_pipe<T>(
    cwd: &Path,
    handle: thread::JoinHandle<std::io::Result<T>>,
    label: &str,
) -> Result<T> {
    handle
        .join()
        .map_err(|_| {
            DeadreckonError::InvalidInput(format!(
                "Git {label} thread panicked at {}",
                cwd.display()
            ))
        })?
        .map_err(|source| git_io_at(cwd, source))
}

fn cleanup_bounded_git(
    cwd: &Path,
    child: &mut Child,
    pipes: GitPipeThreads,
    mut status: Option<ExitStatus>,
    authority: GitProcessAuthority,
    boundary: GitCommandBoundary,
    cleanup_budget: Duration,
    initial_cleanup_error: Option<String>,
) -> Result<BoundedGitOutcome> {
    let mut cleanup_error = initial_cleanup_error;
    let now = Instant::now();
    let cleanup_expires_at = now.checked_add(cleanup_budget).unwrap_or(now);
    let term_until = now
        .checked_add(Duration::from_millis(250))
        .unwrap_or(cleanup_expires_at)
        .min(cleanup_expires_at);
    let mut kill_sent = now >= cleanup_expires_at;

    if let Err(error) = signal_owned_process(child, &authority, kill_sent) {
        cleanup_error = Some(error.to_string());
    }

    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(error) => cleanup_error = Some(error.to_string()),
            }
        }
        match process_authority_is_alive(&authority) {
            Ok(false) if status.is_some() && pipes.is_finished() => {
                // The process tree is gone and every pipe has closed. Join the
                // workers to release their resources; interruption-related I/O
                // errors are not command failures after a policy boundary.
                let _ = pipes.completed_output(cwd, status.expect("checked above"));
                if let Err(error) = remove_git_process_authority(&authority) {
                    return Ok(BoundedGitOutcome::CleanupIncomplete {
                        boundary: GitCommandBoundary::SupervisionFailure,
                        authority,
                        detail: format!(
                            "Git process cleanup completed but its durable authority could not be removed: {error}"
                        ),
                    });
                }
                return Ok(match boundary {
                    GitCommandBoundary::WorkExpired => BoundedGitOutcome::WorkExpired,
                    GitCommandBoundary::Cancelled => BoundedGitOutcome::Cancelled,
                    GitCommandBoundary::SupervisionFailure => {
                        BoundedGitOutcome::SupervisionFailed {
                            detail: cleanup_error.unwrap_or_else(|| {
                                "Git supervision failed after process cleanup was proven"
                                    .to_string()
                            }),
                        }
                    }
                });
            }
            Ok(_) => {}
            Err(error) => cleanup_error = Some(error.to_string()),
        }

        let now = Instant::now();
        if now >= cleanup_expires_at {
            if !kill_sent {
                if let Err(error) = signal_owned_process(child, &authority, true) {
                    cleanup_error = Some(error.to_string());
                }
            }
            return Ok(BoundedGitOutcome::CleanupIncomplete {
                boundary,
                authority,
                detail: cleanup_error.unwrap_or_else(|| {
                    format!(
                        "Git process cleanup was not proven by the absolute cleanup deadline at {}",
                        cwd.display()
                    )
                }),
            });
        }
        if !kill_sent && now >= term_until {
            kill_sent = true;
            if let Err(error) = signal_owned_process(child, &authority, true) {
                cleanup_error = Some(error.to_string());
            }
        }
        thread::sleep(poll_interval(cleanup_expires_at));
    }
}

#[cfg(unix)]
fn configure_owned_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_owned_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn owned_process_group(pid: u32) -> Option<u32> {
    Some(pid)
}

#[cfg(not(unix))]
fn owned_process_group(_pid: u32) -> Option<u32> {
    None
}

#[cfg(unix)]
fn process_authority_is_alive(authority: &GitProcessAuthority) -> std::io::Result<bool> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let process_group = authority.process_group.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix Git authority has no process group",
        )
    })?;
    let process_group = i32::try_from(process_group).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Git process group {process_group} does not fit in i32"),
        )
    })?;
    match kill(Pid::from_raw(-process_group), None) {
        Ok(()) | Err(Errno::EPERM) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(std::io::Error::other(error)),
    }
}

#[cfg(not(unix))]
fn process_authority_is_alive(_authority: &GitProcessAuthority) -> std::io::Result<bool> {
    // The direct child is checked through `try_wait`; this platform does not
    // expose process-group authority through std.
    Ok(false)
}

#[cfg(unix)]
fn signal_owned_process(
    _child: &mut Child,
    authority: &GitProcessAuthority,
    kill_immediately: bool,
) -> std::io::Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let process_group = authority.process_group.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix Git authority has no process group",
        )
    })?;
    let process_group = i32::try_from(process_group).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Git process group {process_group} does not fit in i32"),
        )
    })?;
    let signal = if kill_immediately {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    match kill(Pid::from_raw(-process_group), Some(signal)) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(std::io::Error::other(error)),
    }
}

#[cfg(not(unix))]
fn signal_owned_process(
    child: &mut Child,
    _authority: &GitProcessAuthority,
    _kill_immediately: bool,
) -> std::io::Result<()> {
    child.kill()
}

fn persist_git_process_authority(
    authority_dir: &Path,
    authority: &mut GitProcessAuthority,
) -> Result<()> {
    std::fs::create_dir_all(authority_dir).map_err(|source| git_io_at(authority_dir, source))?;
    let record = crate::SupervisedProcessRecord::running(crate::SupervisedProcess {
        pid: authority.pid,
        pgid: authority.process_group,
    })
    .map_err(|source| git_io_at(authority_dir, source))?;
    let path = authority_dir.join(format!("git-{}.json", record.launch_id));
    crate::write_supervised_process_record(&path, &record)
        .map_err(|source| git_io_at(&path, source))?;
    authority.launch_id = Some(record.launch_id);
    authority.record_path = Some(path);
    Ok(())
}

fn remove_git_process_authority(authority: &GitProcessAuthority) -> Result<()> {
    let (Some(path), Some(launch_id)) = (
        authority.record_path.as_deref(),
        authority.launch_id.as_deref(),
    ) else {
        return Ok(());
    };
    if crate::remove_supervised_process_record_if_matches(path, launch_id, authority.pid)
        .map_err(|source| git_io_at(path, source))?
    {
        return Ok(());
    }
    match std::fs::symlink_metadata(path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(DeadreckonError::InvalidInput(format!(
            "Git process authority changed before compare-and-remove at {}",
            path.display()
        ))),
        Err(source) => Err(git_io_at(path, source)),
    }
}

fn poll_interval(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_millis(10))
}

pub fn git_command(cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    for arg in hardened_git_prefix(args) {
        command.arg(arg);
    }
    command.arg("-C").arg(cwd).args(args);
    command
}

pub fn hardened_git_prefix(args: &[&str]) -> Vec<&'static str> {
    let Some(verb) = first_git_verb(args) else {
        return Vec::new();
    };
    if COMMIT_FAMILY_VERBS.contains(&verb) {
        vec![
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "gpg.format=",
        ]
    } else {
        Vec::new()
    }
}

pub fn hardened_git_argv(cwd: &Path, args: &[&str]) -> Vec<OsString> {
    let mut argv = vec![OsString::from("git")];
    argv.extend(hardened_git_prefix(args).into_iter().map(OsString::from));
    argv.push(OsString::from("-C"));
    argv.push(cwd.as_os_str().to_os_string());
    argv.extend(args.iter().map(OsString::from));
    argv
}

fn first_git_verb<'a>(args: &'a [&str]) -> Option<&'a str> {
    args.iter().copied().find(|arg| !arg.starts_with('-'))
}

fn git_io(source: std::io::Error) -> DeadreckonError {
    DeadreckonError::Io {
        path: PathBuf::from("git"),
        source,
    }
}

fn git_io_at(path: &Path, source: std::io::Error) -> DeadreckonError {
    DeadreckonError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::{
        BoundedGitOutcome, GitCommandBoundary, GitCommandDeadline, run_git,
        run_git_command_bounded, run_git_with_input, run_git_with_input_bounded,
    };

    #[test]
    fn git_with_input_captures_stdout_for_callers_that_parse_it() {
        let temp = TempDir::new().expect("tempdir");
        let output = run_git_with_input(temp.path(), &["hash-object", "--stdin"], b"fixture\n")
            .expect("hash stdin");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ee8c1ee49b4799bbd170233915a897c19e3b55e1"
        );
    }

    #[test]
    fn git_with_input_drains_large_bidirectional_output_without_deadlock() {
        let temp = TempDir::new().expect("tempdir");
        let init = run_git(temp.path(), &["init", "-q"]).expect("git init");
        assert!(init.status.success());

        let mut input = Vec::new();
        for index in 0..5_000_u32 {
            writeln!(input, "src/{index:05}-long-check-attribute-path.rs").expect("path input");
        }
        // `check-attr --stdin` expects newline-delimited input unless `-z` is
        // supplied. Its three-field output is comfortably larger than a 64 KiB
        // pipe, as is the input itself.
        assert!(input.len() > 64 * 1024);
        let output = run_git_with_input(temp.path(), &["check-attr", "--stdin", "filter"], &input)
            .expect("large check-attr communication");

        assert!(output.status.success());
        assert!(output.stdout.len() > 64 * 1024);
        assert_eq!(
            output.stdout.iter().filter(|byte| **byte == b'\n').count(),
            5_000
        );
    }

    #[test]
    fn bounded_git_drains_large_bidirectional_output() {
        let temp = TempDir::new().expect("tempdir");
        let init = run_git(temp.path(), &["init", "-q"]).expect("git init");
        assert!(init.status.success());

        let mut input = Vec::new();
        for index in 0..5_000_u32 {
            writeln!(input, "src/{index:05}-long-check-attribute-path.rs").expect("path input");
        }
        let now = Instant::now();
        let never_cancel = || false;
        let authority_dir = temp.path().join("authorities");
        let outcome = run_git_with_input_bounded(
            temp.path(),
            &["check-attr", "--stdin", "filter"],
            &input,
            GitCommandDeadline::new(
                now + Duration::from_secs(5),
                Duration::from_secs(3),
                &never_cancel,
            )
            .with_authority_dir(&authority_dir),
        )
        .expect("bounded check-attr communication");
        let BoundedGitOutcome::Completed(output) = outcome else {
            panic!("expected completed Git command, got {outcome:?}");
        };

        assert!(output.status.success());
        assert!(output.stdout.len() > 64 * 1024);
        assert_eq!(
            output.stdout.iter().filter(|byte| **byte == b'\n').count(),
            5_000
        );
        assert!(
            std::fs::read_dir(&authority_dir)
                .expect("authority directory")
                .next()
                .is_none(),
            "completed Git must compare-and-remove its durable authority"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_git_work_expiry_reaps_the_owned_process_group() {
        let temp = TempDir::new().expect("tempdir");
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 60 & wait");
        let now = Instant::now();
        let never_cancel = || false;
        let outcome = run_git_command_bounded(
            temp.path(),
            &mut command,
            None,
            GitCommandDeadline::new(
                now + Duration::from_millis(50),
                Duration::from_secs(3),
                &never_cancel,
            ),
        )
        .expect("bounded process group");

        assert!(matches!(outcome, BoundedGitOutcome::WorkExpired));
        assert!(now.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_git_cancellation_reaps_the_owned_process_group() {
        let temp = TempDir::new().expect("tempdir");
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 60 & wait");
        let cancelled = Arc::new(AtomicBool::new(false));
        let controller = Arc::clone(&cancelled);
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            controller.store(true, Ordering::Release);
        });
        let cancellation_requested = || cancelled.load(Ordering::Acquire);
        let now = Instant::now();
        let outcome = run_git_command_bounded(
            temp.path(),
            &mut command,
            None,
            GitCommandDeadline::new(
                now + Duration::from_secs(5),
                Duration::from_secs(3),
                &cancellation_requested,
            ),
        )
        .expect("cancelled process group");
        cancel_thread.join().expect("cancellation controller");

        assert!(matches!(outcome, BoundedGitOutcome::Cancelled));
        assert!(now.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn expired_cleanup_retains_process_authority_for_reconciliation() {
        let temp = TempDir::new().expect("tempdir");
        let mut command = Command::new("sh");
        command.arg("-c").arg("trap '' TERM; sleep 60 & wait");
        let now = Instant::now();
        let work_expires_at = now + Duration::from_millis(50);
        let never_cancel = || false;
        let authority_dir = temp.path().join("authorities");
        let outcome = run_git_command_bounded(
            temp.path(),
            &mut command,
            None,
            GitCommandDeadline::new(work_expires_at, Duration::ZERO, &never_cancel)
                .with_authority_dir(&authority_dir),
        )
        .expect("expired cleanup boundary");

        let BoundedGitOutcome::CleanupIncomplete {
            boundary,
            authority,
            detail,
        } = outcome
        else {
            panic!("expected retained process authority, got {outcome:?}");
        };
        assert_eq!(boundary, GitCommandBoundary::WorkExpired);
        assert_eq!(authority.process_group, Some(authority.pid));
        let record_path = authority
            .record_path
            .as_deref()
            .expect("durable process authority path");
        let record = crate::read_supervised_process_record(record_path)
            .expect("durable process authority record");
        assert_eq!(record.process.pid, authority.pid);
        assert_eq!(
            Some(record.launch_id.as_str()),
            authority.launch_id.as_deref()
        );
        assert!(detail.contains("cleanup was not proven"));
    }
}
