#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::DeadreckonPaths;
use serde_json::Value;
use tempfile::TempDir;

const SUPERVISOR_INSTANCE_SCHEMA: u64 = 3;
// v4 carries the typed two-source health truth (service / home_checkpoint /
// verdict / verdict_reason) alongside the existing manager and checkpoint
// evidence.
const SUPERVISOR_STATUS_SCHEMA: u64 = 4;
// Keep the hermetic service fixture aligned with production's bounded
// readiness window. A debug integration-test binary is large, and both
// install and serve hash its exact bytes before readiness can be trusted.
const SUPERVISOR_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// A hermetic per-user service environment for public `start` integration
/// tests.  Both configured and unconfigured fixtures redirect HOME, the
/// systemd user-config root, PATH, and DEADRECKON_HOME into temporary assets;
/// no test can discover, install, or start the developer's real service.
///
/// `configured` installs the real managed unit through the public CLI, then
/// runs the real long-lived supervisor directly.  The fake service manager is
/// observational only: it reports the manually spawned process as
/// loaded/enabled (launchd) or active/enabled (systemd).  This gives `start`
/// the same unit + manager + live-v3-checkpoint evidence it requires in
/// production without asking the host manager to mutate anything.
pub struct SupervisorServiceFixture {
    _assets: TempDir,
    binary: PathBuf,
    deadreckon_home: PathBuf,
    user_home: PathBuf,
    xdg_config_home: PathBuf,
    manager_bin: PathBuf,
    path: String,
    inherited_env: Vec<(String, String)>,
    service: Option<Child>,
}

impl SupervisorServiceFixture {
    /// Create an isolated environment with no installed service.  Use this for
    /// fail-closed and read-only assertions.
    pub fn unconfigured(paths: &DeadreckonPaths) -> Self {
        Self::new(
            paths,
            Path::new(env!("CARGO_BIN_EXE_deadreckon")),
            &[],
            false,
        )
    }

    /// Install a current managed unit and hold a real supervisor process alive
    /// until this guard is dropped.
    pub fn configured(paths: &DeadreckonPaths) -> Self {
        Self::new(
            paths,
            Path::new(env!("CARGO_BIN_EXE_deadreckon")),
            &[],
            true,
        )
    }

    /// Configure a service for another binary target backed by the same main
    /// (the characterization integration suite uses this).  Production
    /// preflight intentionally pins executable identity, so using the public
    /// binary's unit for that target would be a false fixture.
    pub fn configured_for_binary(paths: &DeadreckonPaths, binary: &Path) -> Self {
        Self::new(paths, binary, &[], true)
    }

    /// The same configured fixture with environment inherited by both the
    /// service and commands under test.  Restart-boundary tests use this to
    /// place their boot/failpoint identity on both sides of the preflight.
    pub fn configured_with_env(paths: &DeadreckonPaths, env: &[(&str, &str)]) -> Self {
        Self::new(
            paths,
            Path::new(env!("CARGO_BIN_EXE_deadreckon")),
            env,
            true,
        )
    }

    fn new(paths: &DeadreckonPaths, binary: &Path, env: &[(&str, &str)], configured: bool) -> Self {
        assert!(
            paths.home().is_absolute(),
            "service fixture requires an absolute DEADRECKON_HOME"
        );
        let assets = isolated_tempdir();
        let user_home = assets.path().join("home");
        let xdg_config_home = assets.path().join("xdg-config");
        let fake_bin = assets.path().join("fake-bin");
        fs::create_dir_all(&user_home).expect("isolated service HOME");
        fs::create_dir_all(&xdg_config_home).expect("isolated service XDG_CONFIG_HOME");
        fs::create_dir_all(&fake_bin).expect("fake service-manager bin");
        write_fake_service_manager(&fake_bin);
        let path = format!(
            "{}:{}",
            fake_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let inherited_env = env
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<Vec<_>>();
        let mut fixture = Self {
            _assets: assets,
            binary: fs::canonicalize(binary).expect("canonical isolated supervisor binary"),
            deadreckon_home: paths.home().to_path_buf(),
            user_home,
            xdg_config_home,
            manager_bin: fake_bin,
            path,
            inherited_env,
            service: None,
        };
        if configured {
            fixture.install_and_spawn();
        }
        fixture
    }

    /// Create a public DeadReckon command inside this isolated service
    /// environment.
    pub fn deadreckon(&self) -> Command {
        let mut command = Command::new(&self.binary);
        self.apply(&mut command);
        command
    }

    /// The isolated user home pinned into the managed unit.  Boundary tests
    /// may place fake host credentials under this temp root without changing
    /// the unit identity checked by preflight.
    pub fn user_home(&self) -> &Path {
        &self.user_home
    }

    /// PATH containing the fake manager followed by the test runner's PATH.
    /// Callers may prepend another temp shim while retaining manager isolation.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory containing only the fake service-manager executable.  Tests
    /// for missing provider binaries can use this as PATH while keeping the
    /// supervisor status query functional.
    pub fn manager_bin(&self) -> &Path {
        &self.manager_bin
    }

    /// Exact temp path where the production installer writes its managed unit.
    pub fn managed_unit_path(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            self.user_home
                .join("Library/LaunchAgents/com.deadreckon.supervisor.plist")
        }
        #[cfg(target_os = "linux")]
        {
            self.xdg_config_home
                .join("systemd/user/deadreckon-supervisor.service")
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            self.user_home.join("unsupported-supervisor-service")
        }
    }

    /// Apply the isolated service environment to a command created elsewhere.
    pub fn apply(&self, command: &mut Command) {
        command
            .env("DEADRECKON_HOME", &self.deadreckon_home)
            .env("HOME", &self.user_home)
            .env("XDG_CONFIG_HOME", &self.xdg_config_home)
            .env("PATH", &self.path);
        for (key, value) in &self.inherited_env {
            command.env(key, value);
        }
    }

    fn install_and_spawn(&mut self) {
        let install = self
            .deadreckon()
            .args(["supervisor", "install"])
            .output()
            .expect("install isolated supervisor service");
        assert_success_with_labels(&install);

        let child = self
            .deadreckon()
            .args(["supervisor", "serve"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated supervisor service");
        let expected_pid = child.id();
        self.service = Some(child);

        let checkpoint = self.deadreckon_home.join("supervisor/instance.json");
        let readiness_started = Instant::now();
        let deadline = readiness_started + SUPERVISOR_READINESS_TIMEOUT;
        let mut last_checkpoint = None;
        loop {
            if let Some(service) = self.service.as_mut()
                && let Some(status) = service.try_wait().expect("poll isolated supervisor")
            {
                panic!("isolated supervisor exited before its checkpoint: {status}")
            }
            if let Ok(bytes) = fs::read(&checkpoint)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            {
                let ready = value["schema_version"].as_u64() == Some(SUPERVISOR_INSTANCE_SCHEMA)
                    && value["pid"].as_u64() == Some(u64::from(expected_pid));
                last_checkpoint = Some(value);
                if ready {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "isolated supervisor pid {expected_pid} did not publish a live v{SUPERVISOR_INSTANCE_SCHEMA} checkpoint at {} within {}s (elapsed: {:.2?}); last observed checkpoint: {}",
                checkpoint.display(),
                SUPERVISOR_READINESS_TIMEOUT.as_secs(),
                readiness_started.elapsed(),
                last_checkpoint
                    .as_ref()
                    .map(Value::to_string)
                    .unwrap_or_else(|| "absent or invalid".to_string())
            );
            thread::sleep(Duration::from_millis(20));
        }

        let status = self
            .deadreckon()
            .args(["supervisor", "status", "--json"])
            .output()
            .expect("query isolated supervisor status");
        assert_success_with_labels(&status);
        let report: Value = serde_json::from_slice(&status.stdout).expect("supervisor status JSON");
        assert_eq!(
            report["schema_version"], SUPERVISOR_STATUS_SCHEMA,
            "{report}"
        );
        assert_eq!(report["installed"], "current", "{report}");
        assert_eq!(report["checkpoint"]["pid"], expected_pid, "{report}");
        assert_eq!(report["service"], "running", "{report}");
        assert_eq!(report["home_checkpoint"], "present", "{report}");
        assert_eq!(report["verdict"], "healthy", "{report}");
        #[cfg(target_os = "macos")]
        {
            assert_eq!(report["manager"], "launchd", "{report}");
            assert_eq!(report["loaded"], true, "{report}");
            assert_eq!(report["enabled"], "enabled", "{report}");
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(report["manager"], "systemd", "{report}");
            assert_eq!(report["active"], "active", "{report}");
            assert_eq!(report["enabled"], "enabled", "{report}");
        }

        // The real service has now published the exact live checkpoint that
        // admission validates.  Freeze its scanner so the ordinary detached
        // per-Job supervisor remains the sole executor in tests that
        // deliberately crash/reclaim leases.  A stopped process retains the
        // same live PID and process-start identity (and a real launchd/systemd
        // manager likewise continues to regard it as loaded/active), while
        // Drop's SIGKILL still reaps it deterministically.
        #[cfg(unix)]
        {
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(expected_pid).expect("supervisor PID")),
                nix::sys::signal::Signal::SIGSTOP,
            )
            .expect("freeze isolated supervisor scanner");
            assert!(
                self.service
                    .as_mut()
                    .expect("isolated supervisor child")
                    .try_wait()
                    .expect("poll frozen supervisor")
                    .is_none(),
                "frozen supervisor unexpectedly exited"
            );
        }
    }
}

impl Drop for SupervisorServiceFixture {
    fn drop(&mut self) {
        if let Some(mut service) = self.service.take() {
            let _ = service.kill();
            let _ = service.wait();
        }
    }
}

fn write_fake_service_manager(bin: &Path) {
    #[cfg(target_os = "macos")]
    let (name, source) = (
        "launchctl",
        r#"#!/bin/sh
set -eu
case "${1:-}" in
  print-disabled)
    printf '%s\n' 'disabled services = {' '  "com.deadreckon.supervisor" => false;' '}'
    ;;
  print|disable|enable|bootout|bootstrap)
    ;;
  *)
    printf 'unexpected launchctl fixture command: %s\n' "$*" >&2
    exit 64
    ;;
esac
"#,
    );
    #[cfg(target_os = "linux")]
    let (name, source) = (
        "systemctl",
        r#"#!/bin/sh
set -eu
case "$*" in
  "--user is-active deadreckon-supervisor.service")
    printf '%s\n' active
    ;;
  "--user is-enabled deadreckon-supervisor.service")
    printf '%s\n' enabled
    ;;
  "--user daemon-reload"|"--user enable deadreckon-supervisor.service"|"--user restart deadreckon-supervisor.service"|"--user disable deadreckon-supervisor.service"|"--user disable --now deadreckon-supervisor.service"|"--user stop deadreckon-supervisor.service")
    ;;
  *)
    printf 'unexpected systemctl fixture command: %s\n' "$*" >&2
    exit 64
    ;;
esac
"#,
    );
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let (name, source) = ("unsupported-service-manager", "#!/bin/sh\nexit 64\n");

    let manager = bin.join(name);
    fs::write(&manager, source).expect("fake service manager");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&manager, fs::Permissions::from_mode(0o755))
            .expect("chmod fake service manager");
    }

    // launchd domains are resolved through `id -u`.  Snapshot the real uid
    // once, then make the hermetic PATH answer without another nested exec.
    // This avoids dyld/shell startup races when several integration tests
    // install isolated units concurrently.
    #[cfg(target_os = "macos")]
    {
        let uid_output = Command::new("/usr/bin/id")
            .arg("-u")
            .output()
            .expect("read test user id");
        assert!(uid_output.status.success(), "read test user id");
        let uid = String::from_utf8(uid_output.stdout)
            .expect("test user id UTF-8")
            .trim()
            .to_string();
        assert!(
            !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid test user id {uid:?}"
        );
        let id = bin.join("id");
        fs::write(&id, format!("#!/bin/sh\nprintf '%s\\n' '{uid}'\n"))
            .expect("fake-bin id snapshot");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&id, fs::Permissions::from_mode(0o755))
            .expect("chmod fake-bin id snapshot");
    }
}

/// Write an executable stub named `binary` into a temp `fake-bin/` dir and
/// return a PATH with it prepended, so provider-presence checks resolve
/// without a real install on the host (CI runners have no agent CLIs).
pub fn prepend_fake_cli_to_path(temp: &TempDir, binary: &str) -> String {
    let bin = temp.path().join("fake-bin");
    fs::create_dir_all(&bin).expect("fake bin dir");
    let path = bin.join(binary);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fake cli");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

pub fn repo_tempdir() -> TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

pub fn workspace_tempdir() -> TempDir {
    let root = workspace_root().join(".test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

pub fn isolated_tempdir() -> TempDir {
    TempDir::new().expect("tempdir")
}

pub fn repo_tempdir_with_empty_bin() -> TempDir {
    let temp = repo_tempdir();
    fs::create_dir_all(temp.path().join("empty-bin")).expect("empty bin");
    temp
}

pub fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

pub fn deadreckon_home(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", home);
    command
}

pub fn deadreckon_home_no_color(home: &Path) -> Command {
    let mut command = deadreckon_home(home);
    command.env("NO_COLOR", "1");
    command
}

pub fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

pub fn assert_success_with_labels(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !dir.join("Cargo.toml").is_file() || !dir.join("crates").is_dir() {
        assert!(dir.pop(), "workspace root not found");
    }
    dir
}
