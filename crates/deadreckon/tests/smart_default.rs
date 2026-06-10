#![allow(clippy::expect_used)]

//! Bare `deadreckon` reads the room: first run on a machine welcomes and
//! points at guided setup, a configured machine in a runless directory gets
//! oriented, and a directory with runs gets its status. All three routes are
//! pipe-clean (no banner art off-TTY) and exit 0.

use std::fs;

use deadreckon_core::{DeadreckonPaths, RunOptions, create_run};
use tempfile::TempDir;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stdout};

#[test]
fn bare_invocation_without_config_prints_first_run_welcome() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .output()
        .expect("bare deadreckon");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Welcome"), "{out}");
    assert!(out.contains("deadreckon init"), "{out}");
    assert!(out.contains("deadreckon try"), "{out}");
    assert!(out.contains("deadreckon help"), "{out}");
    // Piped output stays byte-clean: no banner art, no ANSI.
    assert!(
        !out.contains("____"),
        "banner art leaked into a pipe: {out}"
    );
    assert!(!out.contains('\u{1b}'), "ANSI leaked into a pipe: {out}");
}

#[test]
fn bare_invocation_with_config_but_no_runs_orients_the_directory() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "[providers.smoke]\nkind = \"scripted-smoke\"\n",
    )
    .expect("config");
    let workdir = temp.path().join("fresh-project");
    fs::create_dir_all(&workdir).expect("workdir");

    let output = deadreckon(&paths)
        .current_dir(&workdir)
        .output()
        .expect("bare deadreckon");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("No deadreckon runs in this directory yet."),
        "{out}"
    );
    assert!(out.contains("deadreckon start"), "{out}");
    assert!(out.contains("deadreckon list --all"), "{out}");
    assert!(out.contains("deadreckon doctor"), "{out}");
    assert!(!out.contains('\u{1b}'), "ANSI leaked into a pipe: {out}");
}

#[test]
fn bare_invocation_with_runs_in_scope_shows_status() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "[providers.smoke]\nkind = \"scripted-smoke\"\n",
    )
    .expect("config");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let state = create_run(
        &paths,
        RunOptions {
            goal: "smart default status".to_string(),
            cwd: cwd.clone(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(10.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");

    let output = deadreckon(&paths)
        .current_dir(&cwd)
        .output()
        .expect("bare deadreckon");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("deadreckon status"), "{out}");
    assert!(out.contains(&state.run_id[..8]), "{out}");
}

#[test]
fn bare_invocation_first_run_welcome_lists_detected_agent_clis() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    // A stub agent CLI on PATH must show up in the welcome's detection list.
    let bin = temp.path().join("fake-bin");
    fs::create_dir_all(&bin).expect("bin");
    let stub = bin.join("opencode");
    fs::write(&stub, "#!/bin/sh\nexit 0\n").expect("stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .expect("bare deadreckon");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("OpenCode (opencode)"), "{out}");
}
