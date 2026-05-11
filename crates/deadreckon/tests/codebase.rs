use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::{
    CodebaseMode, CodebaseRecord, DeadreckonPaths, ModeFlags, ResolvedMode, RunOptions, create_run,
    read_codebase_record, record_for_resolved_mode, resolve_mode, write_codebase_record,
};
use tempfile::TempDir;

#[test]
fn mode_resolution_in_git_repo_defaults_to_worktree() {
    let temp = repo_tempdir();
    git(temp.path(), &["init"]).expect("git init");

    let resolved = resolve_mode(&ModeFlags::default(), temp.path(), false).expect("resolve mode");

    match resolved {
        ResolvedMode::Worktree {
            source_path,
            git_root,
        } => {
            assert_eq!(source_path, temp.path().canonicalize().expect("canonical"));
            assert_eq!(git_root, temp.path().canonicalize().expect("canonical"));
        }
        other => panic!("expected worktree, got {other:?}"),
    }
}

#[test]
fn mode_resolution_outside_git_non_interactive_refuses() {
    let temp = TempDir::new().expect("tempdir");

    let err = resolve_mode(&ModeFlags::default(), temp.path(), false).expect_err("refuse");

    let message = err.to_string();
    assert!(message.contains("non-interactive without a mode flag"));
    assert!(message.contains("try: --fresh or --from . or git init"));
}

#[test]
fn codebase_json_roundtrip_for_each_mode() {
    let temp = repo_tempdir();
    let source = temp.path().join("source");
    let git_root = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    fs::create_dir_all(&source).expect("source");
    fs::create_dir_all(&git_root).expect("git root");
    fs::create_dir_all(&worktree).expect("worktree");

    let records = [
        CodebaseRecord::fresh(),
        record_for_resolved_mode(ResolvedMode::Copy {
            source_path: source.clone(),
        }),
        record_for_resolved_mode(ResolvedMode::InPlace {
            source_path: source.clone(),
        }),
        CodebaseRecord {
            mode: CodebaseMode::Worktree,
            source_path: Some(source.clone()),
            source_git_root: Some(git_root),
            branch_name: Some("dr/example-ab12cd34".to_string()),
            base_ref: Some("main".to_string()),
            base_sha: Some("abc1234".to_string()),
            worktree_path: Some(worktree),
            ..CodebaseRecord::fresh()
        },
    ];

    for record in records {
        let working = temp.path().join(format!("working-{}", record.mode));
        write_codebase_record(&working, &record).expect("write");
        let loaded = read_codebase_record(&working).expect("read");
        assert_eq!(loaded, record);
    }
}

#[test]
fn create_run_writes_fresh_codebase_json_by_default() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_run(
        &paths,
        RunOptions {
            goal: "fresh metadata".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");

    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::Fresh);
    assert!(record.source_path.is_none());
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    assert!(
        output.status.success(),
        "git {:?}\nstdout:{}\nstderr:{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
