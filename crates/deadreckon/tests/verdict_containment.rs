#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use deadreckon_core::{DeadreckonPaths, RunOptions, acceptance_spec_path_for_run_root, create_run};
use tempfile::TempDir;

#[test]
fn verdict_without_a_sandbox_fails_closed_before_a_hostile_check_runs() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let empty_path = temp.path().join("empty-path");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&empty_path).expect("empty PATH");
    fs::create_dir_all(paths.jobs_dir()).expect("jobs");
    let state = fixture_run(&paths, &workspace, unavailable_native_backend());
    let workspace_sentinel = workspace.join("hostile-workspace-write");
    let control_sentinel = paths.jobs_dir().join("hostile-control-write");
    write_hostile_contract(
        &state,
        &format!(
            "printf workspace > {}; printf control > {}",
            shell_quote(&workspace_sentinel),
            shell_quote(&control_sentinel),
        ),
    );

    let output = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env("PATH", &empty_path)
        .args(["verdict", &state.run_id, "--plain"])
        .output()
        .expect("verdict");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("verdict requires an available sandbox backend")
            && stderr.contains("no repository-controlled check was run"),
        "{stderr}"
    );
    assert!(!workspace_sentinel.exists(), "hostile workspace write ran");
    assert!(!control_sentinel.exists(), "hostile control write ran");
    assert!(
        !state.run_root.join("proofs").exists(),
        "failed-closed verdict wrote a sidecar"
    );
}

#[test]
fn verdict_without_an_approved_contract_refuses_without_materializing_one() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    let state = fixture_run(&paths, &workspace, "none");
    let contract = acceptance_spec_path_for_run_root(&state.run_root);
    assert!(!contract.exists(), "fixture unexpectedly has a contract");

    let output = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args(["verdict", &state.run_id, "--plain"])
        .output()
        .expect("verdict");

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("verdict requires an approved acceptance contract"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !contract.exists(),
        "verdict synthesized an unapproved acceptance contract"
    );
    assert!(
        !state.run_root.join("proofs").exists(),
        "refused verdict wrote a sidecar"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn verdict_discards_workspace_writes_and_denies_control_path_writes() {
    if !sandbox_exec_available() {
        eprintln!("skipping: sandbox-exec is unavailable on this macOS host");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(paths.jobs_dir()).expect("jobs");
    let state = fixture_run(&paths, &workspace, "sandbox-exec");
    let proofs = state.run_root.join("proofs");
    fs::create_dir_all(&proofs).expect("proofs");
    let workspace_sentinel = workspace.join("hostile-workspace-write");
    let job_sentinel = paths.jobs_dir().join("hostile-job-write");
    let proof_sentinel = proofs.join("forged-proof");
    let host_home = temp.path().join("host-home");
    let host_secret = host_home.join(".aws/credentials");
    fs::create_dir_all(host_secret.parent().expect("secret parent")).expect("secret parent");
    fs::write(&host_secret, "VERDICT_HOST_SECRET_MUST_NOT_LEAK\n").expect("host secret");
    write_hostile_contract(
        &state,
        &format!(
            concat!(
                "if printf workspace > {}; then exit 41; fi; ",
                "if printf job > {}; then exit 42; fi; ",
                "if printf proof > {}; then exit 43; fi; ",
                "test -z \"${{VERDICT_AMBIENT_SECRET+present}}\"; ",
                "if cat {} >/dev/null 2>&1; then exit 44; fi; ",
                "printf disposable > \"$PWD/disposable-write\"; ",
                "test -f \"$PWD/disposable-write\""
            ),
            shell_quote(&workspace_sentinel),
            shell_quote(&job_sentinel),
            shell_quote(&proof_sentinel),
            shell_quote(&host_secret),
        ),
    );

    let output = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env("HOME", &host_home)
        .env(
            "VERDICT_AMBIENT_SECRET",
            "VERDICT_AMBIENT_SECRET_MUST_NOT_LEAK",
        )
        .args(["verdict", &state.run_id, "--plain"])
        .output()
        .expect("verdict");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !workspace_sentinel.exists(),
        "original workspace was mutated"
    );
    assert!(!job_sentinel.exists(), "Job control path was mutated");
    assert!(!proof_sentinel.exists(), "proof path was forged");
    assert!(
        !workspace.join("disposable-write").exists(),
        "disposable evaluator write escaped into the original workspace"
    );
    let captured = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !captured.contains("VERDICT_HOST_SECRET_MUST_NOT_LEAK")
            && !captured.contains("VERDICT_AMBIENT_SECRET_MUST_NOT_LEAK"),
        "verdict captured a host secret: {captured}"
    );
    assert!(
        fs::read_dir(&proofs)
            .expect("proofs")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("verdict-")),
        "trusted verdict sidecar was not written after evaluation"
    );
}

fn fixture_run(
    paths: &DeadreckonPaths,
    workspace: &Path,
    sandbox: &str,
) -> deadreckon_core::PipelineState {
    create_run(
        paths,
        RunOptions {
            goal: "contain a hostile verdict contract".to_string(),
            cwd: workspace.to_path_buf(),
            sandbox: sandbox.to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("create run")
}

fn write_hostile_contract(state: &deadreckon_core::PipelineState, command: &str) {
    let spec = serde_yaml::to_string(&deadreckon_core::AcceptanceSpec {
        name: Some("hostile verdict boundary".to_string()),
        checks: vec![deadreckon_core::AcceptanceCheck::Shell {
            command: command.to_string(),
            cwd: Some("{working_dir}".to_string()),
            must_pass: true,
        }],
    })
    .expect("acceptance YAML");
    fs::write(acceptance_spec_path_for_run_root(&state.run_root), spec)
        .expect("acceptance contract");
}

fn public_deadreckon() -> Command {
    let deadreckon = Path::new(env!("CARGO_BIN_EXE_deadreckon"));
    let gate = Path::new(env!("CARGO_BIN_EXE_dr-gate"));
    assert_eq!(
        deadreckon.parent(),
        gate.parent(),
        "verdict must use its real sibling dr-gate"
    );
    Command::new(deadreckon)
}

#[cfg(target_os = "macos")]
fn sandbox_exec_available() -> bool {
    Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)\n(allow default)", "--", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn unavailable_native_backend() -> &'static str {
    "bwrap"
}

#[cfg(target_os = "linux")]
fn unavailable_native_backend() -> &'static str {
    "sandbox-exec"
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unavailable_native_backend() -> &'static str {
    "bwrap"
}
