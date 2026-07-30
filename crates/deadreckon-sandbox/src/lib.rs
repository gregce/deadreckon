#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Sandbox backend selection, policy, and subprocess execution.

mod backend;
mod commands;
mod doctor;
mod policy;
mod process;
mod spec;

pub use backend::{Result, SandboxBackend, SandboxError, resolve_backend};
pub use commands::{SandboxCommand, build_command};
pub use doctor::{BackendAvailability, doctor};
pub use policy::{ProtectedPathPolicy, ToolSandboxPolicy};
pub use process::{SandboxRunOutput, run};
pub use spec::{GuardedLaunchSpec, SandboxSpec, WorkspaceAccess};

#[cfg(test)]
pub(crate) use commands::sandbox_exec_profile;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    #[cfg(target_os = "macos")]
    use std::io::{Read, Write};
    #[cfg(target_os = "macos")]
    use std::net::TcpListener;
    use std::path::PathBuf;
    #[cfg(target_os = "macos")]
    use std::thread;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    #[cfg(target_os = "macos")]
    use super::ProtectedPathPolicy;
    #[cfg(target_os = "macos")]
    use deadreckon_core::DeadreckonPaths;

    use super::{
        SandboxBackend, SandboxError, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess,
        build_command, run,
    };

    fn shell_spec() -> SandboxSpec {
        SandboxSpec {
            backend: SandboxBackend::None,
            cwd: std::env::current_dir().expect("cwd"),
            program: OsString::from("sh"),
            args: vec![OsString::from("-c"), OsString::from("printf ok")],
            stdin: None,
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: WorkspaceAccess::ReadWrite,
            cleanup_process_group: false,
            guarded_launch: None,
        }
    }

    #[cfg(target_os = "macos")]
    fn local_http_probe() -> (String, thread::JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local HTTP probe");
        listener
            .set_nonblocking(true)
            .expect("nonblocking local HTTP probe");
        let address = listener.local_addr().expect("local HTTP address");
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .expect("HTTP read timeout");
                        let mut request = [0_u8; 1024];
                        let _ = stream.read(&mut request);
                        stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                            )
                            .expect("HTTP response");
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("local HTTP accept failed: {error}"),
                }
            }
            false
        });
        (format!("http://{address}/probe"), handle)
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

    #[tokio::test]
    async fn sandbox_boundary_scrubs_inherited_gate_signing_inputs() {
        let mut spec = shell_spec();
        spec.env.insert(
            deadreckon_core::GATE_KEY_ENV.to_string(),
            "must-not-cross".to_string(),
        );
        spec.env.insert(
            deadreckon_core::GATE_CONTAINED_ENV.to_string(),
            "true".to_string(),
        );
        spec.env.insert(
            deadreckon_core::GATE_SANDBOX_BACKEND_ENV.to_string(),
            "sandbox-exec".to_string(),
        );
        spec.args = vec![
            OsString::from("-c"),
            OsString::from(
                "test -z \"$DEADRECKON_GATE_KEY\" && \
                 test -z \"$DEADRECKON_GATE_CONTAINED\" && \
                 test -z \"$DEADRECKON_GATE_SANDBOX_BACKEND\"",
            ),
        ];

        let output = run(spec).await.expect("run");
        assert_eq!(output.status_code, Some(0), "{output:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn completed_command_cleans_background_process_group_before_returning() {
        let temp = TempDir::new().expect("tempdir");
        let delayed = temp.path().join("delayed");
        let mut spec = shell_spec();
        spec.cleanup_process_group = true;
        spec.env.insert(
            "DEADRECKON_DELAYED_SENTINEL".to_string(),
            delayed.to_string_lossy().into_owned(),
        );
        spec.args = vec![
            OsString::from("-c"),
            OsString::from("(sleep 0.5; : > \"$DEADRECKON_DELAYED_SENTINEL\") & printf done"),
        ];

        let output = run(spec).await.expect("run");
        assert_eq!(output.status_code, Some(0), "{output:?}");
        assert_eq!(output.stdout, "done");
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            !delayed.exists(),
            "background sandbox descendant survived the direct child"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_group_is_persisted_for_cross_process_cancellation() {
        let temp = TempDir::new().expect("tempdir");
        let pid_file = temp.path().join("child-pids/gate.pid");
        let mut spec = shell_spec();
        spec.cleanup_process_group = true;
        spec.pid_file = Some(pid_file.clone());
        spec.args = vec![OsString::from("-c"), OsString::from("sleep 0.5")];

        let handle = tokio::spawn(async move { run(spec).await });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let process =
            deadreckon_core::read_supervised_process(&pid_file).expect("supervised process");
        assert_eq!(process.pgid, Some(process.pid));
        handle.await.expect("join").expect("run");
        assert!(!pid_file.exists());
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
            stdin: None,
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: WorkspaceAccess::ReadWrite,
            cleanup_process_group: false,
            guarded_launch: None,
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
        let (baseline_url, baseline_server) = local_http_probe();
        let baseline = std::process::Command::new("curl")
            .args(["--fail", "--silent", "--max-time", "2", &baseline_url])
            .output()
            .expect("unsandboxed local curl");
        assert!(baseline.status.success(), "{baseline:?}");
        assert_eq!(baseline.stdout, b"ok");
        assert!(baseline_server.join().expect("baseline server"));

        let (blocked_url, blocked_server) = local_http_probe();
        let temp = TempDir::new().expect("tempdir");
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let output = run(SandboxSpec {
            backend: SandboxBackend::SandboxExec,
            cwd: work,
            program: OsString::from("curl"),
            args: vec![
                OsString::from("--fail"),
                OsString::from("--silent"),
                OsString::from("--max-time"),
                OsString::from("2"),
                OsString::from(blocked_url),
            ],
            stdin: None,
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: WorkspaceAccess::ReadWrite,
            cleanup_process_group: false,
            guarded_launch: None,
        })
        .await
        .expect("sandbox run");
        assert!(
            !output.stderr.contains("sandbox_apply"),
            "Seatbelt profile did not apply: {}",
            output.stderr
        );
        assert_ne!(output.status_code, Some(0));
        assert!(
            !blocked_server.join().expect("blocked server"),
            "contained curl reached the local HTTP server"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn read_only_workspace_blocks_write_macos() {
        if which::which("sandbox-exec").is_err() {
            return;
        }
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("work");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let forbidden = workspace.join("forbidden");
        let mut spec = shell_spec();
        spec.backend = SandboxBackend::SandboxExec;
        spec.cwd = workspace;
        spec.args = vec![
            OsString::from("-c"),
            OsString::from(format!("touch {}", forbidden.display())),
        ];
        spec.workspace_access = WorkspaceAccess::ReadOnly;

        let output = run(spec).await.expect("sandbox run");

        assert_ne!(output.status_code, Some(0));
        assert!(!forbidden.exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn hostile_agent_cannot_find_keys_or_forge_marker_macos() {
        if which::which("sandbox-exec").is_err() {
            return;
        }
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("work");
        let paths = DeadreckonPaths::from_home(workspace.join(".deadreckon"));
        let run_root = paths.run_root("project", "run-1");
        let key_store = paths.home().join("gate-keys");
        let marker = run_root.join("proofs/turn-acceptance.json");
        std::fs::create_dir_all(&key_store).expect("key store");
        std::fs::create_dir_all(marker.parent().expect("proof parent")).expect("proofs");
        std::fs::write(key_store.join("run-1.key"), "super-secret-signing-material").expect("key");
        std::fs::write(&marker, "original-marker").expect("marker");
        let boundary = ProtectedPathPolicy::for_paths(&paths);

        let mut spec = shell_spec();
        spec.backend = SandboxBackend::SandboxExec;
        spec.cwd = workspace.clone();
        spec.read_allowlist = vec![workspace.clone()];
        spec.write_allowlist = vec![workspace.clone()];
        spec.read_denylist = boundary.read_denylist;
        spec.write_denylist = boundary.write_denylist;
        spec.args = vec![
            OsString::from("-c"),
            OsString::from(format!(
                "find '{}' -name '*.key' -exec cat {{}} \\; 2>/dev/null || true; \
                 if printf forged > '{}'; then exit 41; fi; \
                 test \"$(cat '{}')\" = original-marker",
                workspace.display(),
                marker.display(),
                marker.display()
            )),
        ];

        let output = run(spec).await.expect("sandbox run");

        assert_eq!(output.status_code, Some(0), "{}", output.stderr);
        assert!(!output.stdout.contains("super-secret-signing-material"));
        assert_eq!(
            std::fs::read_to_string(marker).expect("marker"),
            "original-marker"
        );
    }
}
