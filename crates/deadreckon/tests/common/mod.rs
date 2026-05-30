#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use deadreckon_core::DeadreckonPaths;
use tempfile::TempDir;

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
