#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::{DeadreckonPaths, list_runs};
use tempfile::TempDir;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stderr};

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
    assert!(card_row_contains(&err, "sleep", "caffeinate"), "{err}");
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
    assert!(card_row_contains(&err, "sleep", "none (non-tty)"), "{err}");
}

fn card_row_contains(card: &str, label: &str, value: &str) -> bool {
    card.lines()
        .any(|line| line.contains(label) && line.contains(value))
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

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "--initial-branch=main"]).expect("git init");
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
