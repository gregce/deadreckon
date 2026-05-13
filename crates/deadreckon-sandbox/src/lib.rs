#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

mod backend;
mod commands;
mod doctor;
mod policy;
mod process;
mod spec;

pub use backend::{Result, SandboxBackend, SandboxError, resolve_backend};
pub use commands::{SandboxCommand, build_command};
pub use doctor::{BackendAvailability, doctor};
pub use policy::ToolSandboxPolicy;
pub use process::{SandboxRunOutput, run};
pub use spec::SandboxSpec;

#[cfg(test)]
pub(crate) use commands::sandbox_exec_profile;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[cfg(target_os = "macos")]
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::{SandboxBackend, SandboxError, SandboxSpec, ToolSandboxPolicy, build_command, run};

    fn shell_spec() -> SandboxSpec {
        SandboxSpec {
            backend: SandboxBackend::None,
            cwd: std::env::current_dir().expect("cwd"),
            program: OsString::from("sh"),
            args: vec![OsString::from("-c"), OsString::from("printf ok")],
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            network_allowlist: Vec::new(),
        }
    }

    #[tokio::test]
    async fn none_backend_runs_command_with_warning() {
        let output = run(shell_spec()).await.expect("run");
        assert_eq!(output.status_code, Some(0));
        assert_eq!(output.stdout, "ok");
        assert!(output.pid.is_some());
        assert!(output.warning.expect("warning").contains("unsafe"));
    }

    #[tokio::test]
    async fn subprocess_cancel_escalates_sigterm_to_sigkill() {
        let token = CancellationToken::new();
        let mut spec = shell_spec();
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("trap '' TERM; while true; do sleep 1; done"),
        ];
        spec.cancellation_token = Some(token.clone());
        let started = Instant::now();
        let handle = tokio::spawn(async move { run(spec).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        let err = handle.await.expect("join").expect_err("cancelled");

        assert!(matches!(err, SandboxError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "cancel did not escalate promptly"
        );
    }

    #[test]
    fn docker_backend_builds_network_none_when_requested() {
        let mut spec = shell_spec();
        spec.backend = SandboxBackend::Docker;
        spec.allow_network = false;
        let command = build_command(&spec).unwrap_or_else(|_| {
            let mut fallback = spec.clone();
            fallback.backend = SandboxBackend::None;
            build_command(&fallback).expect("fallback")
        });
        if command.backend == SandboxBackend::Docker {
            assert!(command.args.iter().any(|arg| arg == "--network"));
            assert!(command.args.iter().any(|arg| arg == "none"));
        }
    }

    #[test]
    fn sandbox_exec_profile_blocks_home_ssh_by_default() {
        let mut spec = shell_spec();
        spec.backend = SandboxBackend::SandboxExec;
        let command = build_command(&spec).unwrap_or_else(|_| {
            let mut fallback = spec.clone();
            fallback.backend = SandboxBackend::None;
            build_command(&fallback).expect("fallback")
        });
        if command.backend == SandboxBackend::SandboxExec {
            let profile = command.args[1].to_string_lossy();
            assert!(profile.contains("(allow default)"));
            assert!(profile.contains(".ssh"));
            assert!(profile.contains("(deny file-read*"));
        }
    }

    #[test]
    fn sandbox_escape_prompt_cannot_read_home_ssh() {
        let spec = shell_spec();
        let profile = super::sandbox_exec_profile(&spec).expect("profile");
        assert!(profile.contains(".ssh"));
        assert!(profile.contains("(deny file-read*"));
        assert!(profile.contains("(deny file-write*"));
    }

    #[test]
    fn network_allowlist_blocks_unknown_host() {
        let mut spec = shell_spec();
        spec.allow_network = true;
        spec.network_allowlist = vec!["api.openai.com".to_string()];
        let profile = super::sandbox_exec_profile(&spec).expect("profile");
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn per_tool_policy_scopes_bash_to_working_dir_without_network() {
        let work = PathBuf::from("/tmp/deadreckon-policy-work");
        let policy = ToolSandboxPolicy::bash(&work);
        assert!(!policy.allow_network);
        assert_eq!(policy.read_allowlist, vec![work.clone()]);
        assert_eq!(policy.write_allowlist, vec![work]);
        assert!(policy.network_allowlist.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandbox_blocks_ssh_read_macos() {
        if which::which("sandbox-exec").is_err() {
            return;
        }
        let home_secret = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".ssh/id_rsa"));
        let Some(home_secret) = home_secret.filter(|path| path.exists()) else {
            let mut spec = shell_spec();
            spec.backend = SandboxBackend::SandboxExec;
            let command = build_command(&spec).expect("profile");
            let profile = command.args[1].to_string_lossy();
            assert!(profile.contains(".ssh"));
            assert!(profile.contains("(deny file-read*"));
            return;
        };
        let temp = TempDir::new().expect("tempdir");
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let output = run(SandboxSpec {
            backend: SandboxBackend::SandboxExec,
            cwd: work,
            program: OsString::from("sh"),
            args: vec![
                OsString::from("-c"),
                OsString::from(format!("cat {}", home_secret.display())),
            ],
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            network_allowlist: Vec::new(),
        })
        .await
        .expect("sandbox run");
        assert_ne!(output.status_code, Some(0));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandbox_blocks_outbound_to_evil_host() {
        if which::which("sandbox-exec").is_err() || which::which("curl").is_err() {
            return;
        }
        let temp = TempDir::new().expect("tempdir");
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let output = run(SandboxSpec {
            backend: SandboxBackend::SandboxExec,
            cwd: work,
            program: OsString::from("curl"),
            args: vec![
                OsString::from("--max-time"),
                OsString::from("2"),
                OsString::from("https://example.com"),
            ],
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            network_allowlist: Vec::new(),
        })
        .await
        .expect("sandbox run");
        assert_ne!(output.status_code, Some(0));
    }
}
