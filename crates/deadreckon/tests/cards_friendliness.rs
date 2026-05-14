#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::DeadreckonPaths;
use tempfile::TempDir;

#[test]
fn plain_flag_strips_color_and_box_drawing_globally() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "plain preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
            "--plain",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(err.contains("+"), "{err}");
}

#[test]
fn no_color_env_implies_plain_for_cards() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("NO_COLOR", "1")
        .args([
            "run",
            "no color preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(err.contains("+"), "{err}");
}

#[test]
fn width_below_forty_uses_single_column_ascii_fallback() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("COLUMNS", "32")
        .args([
            "run",
            "narrow preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("deadreckon run preview"), "{err}");
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(!err.contains("+"), "{err}");
}

#[test]
fn every_refusal_footer_ends_with_try_line() {
    let temp = TempDir::new().expect("tempdir");
    let repo = temp.path().join("not-git");
    fs::create_dir_all(&repo).expect("repo");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["run", "refuse", "--smoke", "--max-spend", "1", "--yes"])
        .output()
        .expect("run");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    let last = err
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rfind(|line| !line.trim_start().starts_with("hint:"))
        .expect("last stderr line");
    assert!(last.contains("try:"), "{err}");
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from("/Users/gdc/deadreckon/.test-tmp");
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
