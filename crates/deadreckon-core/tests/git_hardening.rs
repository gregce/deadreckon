#![allow(clippy::expect_used)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use deadreckon_core::git::{git_command, hardened_git_argv, hardened_git_prefix, run_git};
use tempfile::TempDir;

#[test]
fn git_run_exports_git_terminal_prompt_zero_in_env() {
    let command = git_command(Path::new("/tmp/repo"), &["status"]);
    let has_prompt_zero = command.get_envs().any(|(key, value)| {
        (
            key.to_string_lossy().to_string(),
            value.map(|value| value.to_string_lossy().to_string()),
        ) == ("GIT_TERMINAL_PROMPT".to_string(), Some("0".to_string()))
    });

    assert!(has_prompt_zero);
}

#[test]
fn git_commit_args_include_commit_gpgsign_false() {
    let prefix = hardened_git_prefix(&["commit", "-m", "msg"]);
    assert!(prefix.contains(&"commit.gpgsign=false"));
    assert!(prefix.contains(&"tag.gpgsign=false"));
    assert!(prefix.contains(&"gpg.format="));

    let argv = hardened_git_argv(Path::new("/tmp/repo"), &["commit", "-m", "msg"]);
    let rendered = argv
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        rendered.starts_with("git -c commit.gpgsign=false"),
        "{rendered}"
    );
}

#[test]
fn git_status_args_do_not_include_commit_gpgsign_false() {
    let prefix = hardened_git_prefix(&["status", "--porcelain"]);
    assert!(prefix.is_empty(), "{prefix:?}");
}

#[test]
fn git_invocation_grep_finds_no_raw_command_new_outside_helper() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_roots = [
        manifest.join("src"),
        manifest.join("../deadreckon/src"),
        manifest.join("../deadreckon-runtime/src"),
    ];
    let mut offenders = Vec::new();
    for root in source_roots {
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if path.ends_with("git.rs") {
                continue;
            }
            let text = std::fs::read_to_string(path).expect("source");
            if text.contains("Command::new(\"git\")")
                || text.contains("std::process::Command::new(\"git\")")
            {
                offenders.push(path.display().to_string());
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

#[cfg(unix)]
#[test]
fn worktree_turn_commit_succeeds_under_fake_gpg_that_would_hang() {
    let temp = signed_repo();

    fs::write(temp.path().join("repo/file.txt"), "change").expect("change");
    run_git(temp.path().join("repo").as_path(), &["add", "-A"]).expect("add");
    let output = run_git(
        temp.path().join("repo").as_path(),
        &["commit", "-m", "turn commit"],
    )
    .expect("commit");

    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !temp.path().join("gpg-invoked").exists(),
        "fake gpg should not be invoked"
    );
}

#[cfg(unix)]
#[test]
fn apply_commit_succeeds_under_global_signing_config() {
    let temp = signed_repo();

    fs::write(temp.path().join("repo/apply.txt"), "apply").expect("apply");
    run_git(temp.path().join("repo").as_path(), &["add", "-A"]).expect("add");
    let output = run_git(
        temp.path().join("repo").as_path(),
        &["commit", "-m", "apply commit"],
    )
    .expect("commit");

    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !temp.path().join("gpg-invoked").exists(),
        "fake gpg should not be invoked"
    );
}

#[cfg(unix)]
fn signed_repo() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    run_git(&repo, &["init"]).expect("init");
    run_git(
        &repo,
        &["config", "user.email", "deadreckon@example.invalid"],
    )
    .expect("email");
    run_git(&repo, &["config", "user.name", "deadreckon"]).expect("name");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    run_git(&repo, &["add", "-A"]).expect("add");
    let initial = run_git(&repo, &["commit", "-m", "initial"]).expect("initial");
    assert!(
        initial.status.success(),
        "{}{}",
        String::from_utf8_lossy(&initial.stdout),
        String::from_utf8_lossy(&initial.stderr)
    );
    let fake_gpg = temp.path().join("fake-gpg");
    fs::write(
        &fake_gpg,
        format!(
            "#!/bin/sh\nprintf invoked > {}\nexit 1\n",
            shell_quote(&temp.path().join("gpg-invoked").display().to_string())
        ),
    )
    .expect("fake gpg");
    let mut permissions = fs::metadata(&fake_gpg).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gpg, permissions).expect("chmod");
    run_git(&repo, &["config", "commit.gpgsign", "true"]).expect("gpgsign");
    run_git(&repo, &["config", "tag.gpgsign", "true"]).expect("tag gpgsign");
    run_git(
        &repo,
        &["config", "gpg.program", &fake_gpg.display().to_string()],
    )
    .expect("gpg program");
    temp
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
