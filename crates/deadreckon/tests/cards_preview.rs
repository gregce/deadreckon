#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::{DeadreckonPaths, list_runs};
use tempfile::TempDir;

#[cfg(target_os = "macos")]
#[test]
fn preview_card_shows_sleep_mode_row_for_caffeinate() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "caffeinate preview",
            "--smoke",
            "--preview",
            "--prevent-sleep",
            "on",
            "--max-spend",
            "1",
            "--plain",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("sleep         caffeinate"), "{err}");
}

#[test]
fn preview_card_shows_sleep_skip_reason_when_non_tty() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "non tty preview",
            "--smoke",
            "--preview",
            "--prevent-sleep",
            "auto",
            "--max-spend",
            "1",
            "--plain",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("sleep         none (non-tty)"), "{err}");
}

#[test]
fn preview_card_exits_zero_when_preview_flag_set() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "preview only",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    assert!(stderr(&output).contains("deadreckon run preview"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn preview_card_shows_confirmation_row_when_max_spend_above_fifty() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "expensive preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "60",
            "--no-confirm",
            "--plain",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("confirmation"), "{err}");
    assert!(err.contains("high spend acknowledged"), "{err}");
}

#[test]
fn preview_card_aesthetic_matches_exit_card_fixture() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env_remove("NO_COLOR")
        .args([
            "run",
            "border preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("╭"), "{err}");
    assert!(err.contains("│"), "{err}");
    assert!(err.contains("╰"), "{err}");
    assert!(!err.contains("+"), "{err}");
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    git(
        &repo,
        &["config", "user.email", "deadreckon@example.invalid"],
    )
    .expect("email");
    git(&repo, &["config", "user.name", "deadreckon"]).expect("name");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    assert!(
        output.status.success(),
        "git {:?}\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
