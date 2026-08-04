//! Login-state probing for subscription CLI providers.
//!
//! Binary presence says "installed", not "usable": a user with `claude` on
//! PATH but no login sails through setup and hits raw subprocess stderr
//! mid-run. Descriptors may declare an `[auth_probe]` (a local status
//! subcommand such as `claude auth status`); this module runs it with a
//! timeout and classifies the result.
//!
//! Classification is deliberately fail-open: only an explicit logged-out
//! marker (or a logged-in marker that is absent while a logged-out marker
//! matches) yields `NotLoggedIn`. Anything ambiguous — unsupported
//! subcommand on an older CLI, unexpected output shape, timeout — is
//! `Unknown`, and callers treat `Unknown` exactly as they treated binary
//! presence before this probe existed.

use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use deadreckon_core::HeadTailBuffer;

use crate::cli_common::{remove_current_process_record, write_current_process_record};
use crate::registry::AuthProbe;

const DEFAULT_PROBE_TIMEOUT_SECONDS: u64 = 10;
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PROBE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAuthStatus {
    LoggedIn,
    NotLoggedIn { detail: String },
    Unknown { reason: String },
}

/// Run the descriptor's auth probe against `binary` and classify the output.
/// Never errors: every failure mode collapses into `Unknown` so the caller
/// can fall back to presence-only behavior. `DEADRECKON_AUTH_PROBE=0` (or
/// `off`) disables probing entirely — for hermetic test environments that
/// redirect `HOME`, where the real CLI would truthfully report logged-out.
pub fn probe_cli_auth(binary: &str, probe: &AuthProbe) -> CliAuthStatus {
    if matches!(
        std::env::var("DEADRECKON_AUTH_PROBE").as_deref(),
        Ok("0") | Ok("off") | Ok("false")
    ) {
        return CliAuthStatus::Unknown {
            reason: "auth probe disabled by DEADRECKON_AUTH_PROBE".to_string(),
        };
    }
    let timeout = Duration::from_secs(
        probe
            .timeout_seconds
            .unwrap_or(DEFAULT_PROBE_TIMEOUT_SECONDS),
    );
    let mut command = Command::new(binary);
    command
        .args(&probe.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_auth_probe_process_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return CliAuthStatus::Unknown {
                reason: format!("could not run {binary}: {err}"),
            };
        }
    };
    let pid = child.id();
    // Start draining immediately. A CLI can fill either 64 KiB pipe before it
    // exits, so polling the child before reading both streams can deadlock even
    // when the probe itself is otherwise healthy. The drainers retain only a
    // bounded head/tail view and discard the rest.
    let Some(stdout) = child.stdout.take() else {
        let cleanup_proven = cleanup_unsupervised_auth_probe(&mut child, pid);
        return CliAuthStatus::Unknown {
            reason: format!(
                "auth probe stdout was unavailable{}",
                (!cleanup_proven)
                    .then_some("; process-tree cleanup could not be proven")
                    .unwrap_or_default()
            ),
        };
    };
    let Some(stderr) = child.stderr.take() else {
        let cleanup_proven = cleanup_unsupervised_auth_probe(&mut child, pid);
        return CliAuthStatus::Unknown {
            reason: format!(
                "auth probe stderr was unavailable{}",
                (!cleanup_proven)
                    .then_some("; process-tree cleanup could not be proven")
                    .unwrap_or_default()
            ),
        };
    };
    let stdout_drain = spawn_bounded_output_drain(stdout);
    let stderr_drain = spawn_bounded_output_drain(stderr);
    let authority = auth_probe_authority_path();
    let record = match write_current_process_record(&authority, pid) {
        Ok(record) => record,
        Err(error) => {
            let cleanup_deadline = Instant::now() + PROBE_CLEANUP_TIMEOUT;
            terminate_auth_probe_group(pid, true);
            let _ = child.kill();
            let direct_reaped = reap_auth_probe_child(&mut child, cleanup_deadline);
            let group_reconciled = reconcile_auth_probe_group(pid);
            let stdout_drained = receive_bounded_output(stdout_drain, cleanup_deadline).is_some();
            let stderr_drained = receive_bounded_output(stderr_drain, cleanup_deadline).is_some();
            let cleanup_proven =
                direct_reaped && group_reconciled && stdout_drained && stderr_drained;
            let authority_removed =
                cleanup_proven && remove_partial_auth_probe_authority(&authority);
            return CliAuthStatus::Unknown {
                reason: format!(
                    "could not supervise auth probe: {error}{}",
                    (!authority_removed)
                        .then(|| format!(
                            "; process authority may remain at {}",
                            authority.display()
                        ))
                        .unwrap_or_default()
                ),
            };
        }
    };
    let started = Instant::now();
    let completion = loop {
        match child.try_wait() {
            Ok(Some(_)) => break AuthProbeCompletion::Completed,
            Ok(None) => {
                if started.elapsed() > timeout {
                    break AuthProbeCompletion::TimedOut;
                }
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Err(err) => {
                break AuthProbeCompletion::Failed(err.to_string());
            }
        }
    };

    let cleanup_deadline = Instant::now() + PROBE_CLEANUP_TIMEOUT;
    terminate_auth_probe_group(pid, !matches!(&completion, AuthProbeCompletion::TimedOut));
    let _ = child.kill();
    let direct_reaped = matches!(&completion, AuthProbeCompletion::Completed)
        || reap_auth_probe_child(&mut child, cleanup_deadline);
    let group_reconciled = reconcile_auth_probe_group(pid);
    let stdout = receive_bounded_output(stdout_drain, cleanup_deadline);
    let stderr = receive_bounded_output(stderr_drain, cleanup_deadline);
    let drains_completed = stdout.is_some() && stderr.is_some();
    let cleanup_proven = direct_reaped
        && group_reconciled
        && drains_completed
        && remove_current_process_record(&authority, &record).is_ok();
    let cleanup = (!cleanup_proven)
        .then(|| format!("; process authority remains at {}", authority.display()));

    match completion {
        AuthProbeCompletion::TimedOut => CliAuthStatus::Unknown {
            reason: format!(
                "auth probe timed out after {}s{}",
                timeout.as_secs(),
                cleanup.unwrap_or_default()
            ),
        },
        AuthProbeCompletion::Failed(error) => CliAuthStatus::Unknown {
            reason: format!("auth probe failed: {error}{}", cleanup.unwrap_or_default()),
        },
        AuthProbeCompletion::Completed if !cleanup_proven => CliAuthStatus::Unknown {
            reason: format!(
                "auth probe returned without proving process-tree cleanup{}",
                cleanup.unwrap_or_default()
            ),
        },
        AuthProbeCompletion::Completed => classify_probe_output(
            probe,
            &format!(
                "{}{}",
                stdout.unwrap_or_default(),
                stderr.unwrap_or_default()
            ),
        ),
    }
}

fn auth_probe_authority_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "deadreckon-auth-probe-{}.pid",
        uuid::Uuid::new_v4().simple()
    ))
}

enum AuthProbeCompletion {
    Completed,
    TimedOut,
    Failed(String),
}

fn spawn_bounded_output_drain<R>(mut reader: R) -> Receiver<String>
where
    R: std::io::Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut output = HeadTailBuffer::new(PROBE_OUTPUT_LIMIT_BYTES);
        let mut chunk = [0_u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => output.push(&chunk[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = sender.send(output.render(None));
    });
    receiver
}

fn receive_bounded_output(receiver: Receiver<String>, deadline: Instant) -> Option<String> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
}

fn cleanup_unsupervised_auth_probe(child: &mut Child, pid: u32) -> bool {
    let cleanup_deadline = Instant::now() + PROBE_CLEANUP_TIMEOUT;
    terminate_auth_probe_group(pid, true);
    let _ = child.kill();
    reap_auth_probe_child(child, cleanup_deadline) && reconcile_auth_probe_group(pid)
}

fn remove_partial_auth_probe_authority(authority: &std::path::Path) -> bool {
    match std::fs::remove_file(authority) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn reap_auth_probe_child(child: &mut Child, deadline: Instant) -> bool {
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(
                    PROBE_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

#[cfg(unix)]
fn configure_auth_probe_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_auth_probe_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_auth_probe_group(pid: u32, force: bool) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if let Ok(pid) = i32::try_from(pid) {
        let signal = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = kill(Pid::from_raw(-pid), Some(signal));
        if !force {
            std::thread::sleep(Duration::from_millis(250));
            let _ = kill(Pid::from_raw(-pid), Some(Signal::SIGKILL));
        }
    }
}

#[cfg(not(unix))]
fn terminate_auth_probe_group(_pid: u32, _force: bool) {}

fn reconcile_auth_probe_group(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use deadreckon_core::ChildTerminator as _;

        let Ok(pgid) = i32::try_from(pid) else {
            return false;
        };
        return !matches!(
            deadreckon_core::ProcessGroupTerminator::new(pgid)
                .terminate(Duration::from_millis(250)),
            deadreckon_core::TerminationOutcome::Failed(_)
        );
    }
    #[cfg(not(unix))]
    {
        !deadreckon_core::pid_is_alive(pid)
    }
}

fn classify_probe_output(probe: &AuthProbe, combined: &str) -> CliAuthStatus {
    let condensed = condense(combined);
    // Logged-out first: logged-out markers often embed the logged-in phrase
    // ("Not logged in" contains "logged in").
    if let Some(out_marker) = probe.logged_out_substring.as_deref()
        && condensed.contains(&condense(out_marker))
    {
        return CliAuthStatus::NotLoggedIn {
            detail: first_line(combined),
        };
    }
    if let Some(in_marker) = probe.logged_in_substring.as_deref()
        && condensed.contains(&condense(in_marker))
    {
        return CliAuthStatus::LoggedIn;
    }
    CliAuthStatus::Unknown {
        reason: "auth probe output did not match a known login state".to_string(),
    }
}

/// Strip all whitespace so marker matching survives JSON pretty-printing and
/// line-wrapping differences between CLI versions.
fn condense(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> AuthProbe {
        AuthProbe {
            args: vec!["auth".to_string(), "status".to_string()],
            logged_in_substring: Some("\"loggedIn\": true".to_string()),
            logged_out_substring: Some("\"loggedIn\": false".to_string()),
            login_try_lines: vec!["claude login".to_string()],
            timeout_seconds: Some(2),
        }
    }

    #[test]
    fn logged_in_marker_matches_across_whitespace_variants() {
        for raw in [
            "{\n  \"loggedIn\": true,\n  \"authMethod\": \"claude.ai\"\n}",
            "{\"loggedIn\":true}",
        ] {
            assert_eq!(
                classify_probe_output(&probe(), raw),
                CliAuthStatus::LoggedIn
            );
        }
    }

    #[test]
    fn logged_out_marker_wins_over_embedded_logged_in_phrase() {
        let codex_probe = AuthProbe {
            args: vec!["login".to_string(), "status".to_string()],
            logged_in_substring: Some("Logged in".to_string()),
            logged_out_substring: Some("Not logged in".to_string()),
            login_try_lines: vec!["codex login".to_string()],
            timeout_seconds: Some(2),
        };
        assert!(matches!(
            classify_probe_output(&codex_probe, "Not logged in"),
            CliAuthStatus::NotLoggedIn { .. }
        ));
        assert_eq!(
            classify_probe_output(&codex_probe, "Logged in using ChatGPT"),
            CliAuthStatus::LoggedIn
        );
    }

    #[test]
    fn unmatched_output_is_unknown_not_logged_out() {
        assert!(matches!(
            classify_probe_output(&probe(), "fake provider says hi"),
            CliAuthStatus::Unknown { .. }
        ));
    }

    #[test]
    fn missing_binary_is_unknown() {
        let status = probe_cli_auth("/nonexistent/deadreckon-test-binary", &probe());
        assert!(matches!(status, CliAuthStatus::Unknown { .. }));
    }

    #[test]
    fn real_subprocess_logged_out_classifies() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let script = temp.path().join("fake-cli");
        std::fs::write(&script, "#!/bin/sh\necho '{\"loggedIn\": false}'\nexit 1\n")
            .expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let status = probe_cli_auth(&script.display().to_string(), &probe());
        assert!(matches!(status, CliAuthStatus::NotLoggedIn { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn output_flooding_auth_probe_reaps_term_resistant_descendant() {
        use std::os::unix::fs::PermissionsExt as _;

        struct DescendantGuard(std::path::PathBuf);
        impl Drop for DescendantGuard {
            fn drop(&mut self) {
                let Ok(raw) = std::fs::read_to_string(&self.0) else {
                    return;
                };
                let Ok(pid) = raw.trim().parse::<u32>() else {
                    return;
                };
                if deadreckon_core::pid_is_alive(pid) {
                    let _ = deadreckon_core::terminate_pid(pid, true);
                }
            }
        }

        let temp = tempfile::TempDir::new().expect("tempdir");
        let script = temp.path().join("hanging-auth-cli");
        let descendant_path = temp.path().join("descendant.pid");
        let _guard = DescendantGuard(descendant_path.clone());
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n(trap '' TERM; sleep 30) &\nchild=$!\nprintf '%s\\n' \"$child\" > '{}'\ntrap '' TERM\nwhile :; do\n  printf 'stdout-flood-0123456789abcdef0123456789abcdef\\n'\n  printf 'stderr-flood-0123456789abcdef0123456789abcdef\\n' >&2\ndone\n",
                descendant_path.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .expect("permissions");
        let mut hanging_probe = probe();
        hanging_probe.timeout_seconds = Some(1);

        let started = Instant::now();
        let status = probe_cli_auth(&script.display().to_string(), &hanging_probe);

        let CliAuthStatus::Unknown { reason } = status else {
            panic!("hanging probe must be unknown")
        };
        assert!(reason.contains("timed out"), "{reason}");
        assert!(!reason.contains("authority remains"), "{reason}");
        assert!(
            started.elapsed() < Duration::from_secs(7),
            "output flood made the bounded auth probe hang"
        );
        let descendant = std::fs::read_to_string(&descendant_path)
            .expect("descendant pid")
            .trim()
            .parse::<u32>()
            .expect("numeric descendant pid");
        assert!(!deadreckon_core::pid_is_alive(descendant));
    }
}
