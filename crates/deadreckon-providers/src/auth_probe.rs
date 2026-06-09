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

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::registry::AuthProbe;

const DEFAULT_PROBE_TIMEOUT_SECONDS: u64 = 10;
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAuthStatus {
    LoggedIn,
    NotLoggedIn { detail: String },
    Unknown { reason: String },
}

/// Run the descriptor's auth probe against `binary` and classify the output.
/// Never errors: every failure mode collapses into `Unknown` so the caller
/// can fall back to presence-only behavior.
pub fn probe_cli_auth(binary: &str, probe: &AuthProbe) -> CliAuthStatus {
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
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return CliAuthStatus::Unknown {
                reason: format!("could not run {binary}: {err}"),
            };
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CliAuthStatus::Unknown {
                        reason: format!(
                            "auth probe timed out after {}s",
                            timeout.as_secs()
                        ),
                    };
                }
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Err(err) => {
                return CliAuthStatus::Unknown {
                    reason: format!("auth probe failed: {err}"),
                };
            }
        }
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => {
            return CliAuthStatus::Unknown {
                reason: format!("auth probe failed: {err}"),
            };
        }
    };
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    classify_probe_output(probe, &combined)
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
            assert_eq!(classify_probe_output(&probe(), raw), CliAuthStatus::LoggedIn);
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
}
