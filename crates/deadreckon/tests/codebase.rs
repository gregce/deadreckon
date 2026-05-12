use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use deadreckon_core::{
    CodebaseMode, CodebaseRecord, DeadreckonPaths, ModeFlags, ResolvedMode, RunOptions, RunStatus,
    WorktreeOptions, create_run, create_worktree, list_runs, load_run, prepare_worktree_record,
    read_codebase_record, record_for_resolved_mode, resolve_mode, save_state,
    write_codebase_record,
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

#[test]
fn worktree_run_creates_dr_prefixed_branch_in_worktrees_dir() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("worktree smoke")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::Worktree);
    let branch = record.branch_name.expect("branch");
    assert!(branch.starts_with("dr/worktree-smoke-"));
    let worktree = record.worktree_path.expect("worktree");
    assert!(worktree.starts_with(paths.home().join("worktrees")));
    assert!(worktree.exists());
    git(&repo, &["rev-parse", "--verify", &branch]).expect("branch exists");
}

#[test]
fn worktree_run_sets_working_dir_to_worktree_path() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let worktree = temp.path().join("worktree");
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Worktree;
    record.worktree_path = Some(worktree.clone());

    let state = create_run(
        &paths,
        RunOptions {
            goal: "worktree dir".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: Some(record),
        },
    )
    .expect("run");

    assert_eq!(state.working_dir, worktree);
    assert_eq!(
        read_codebase_record(&state.working_dir)
            .expect("codebase")
            .mode,
        CodebaseMode::Worktree
    );
}

#[test]
fn dirty_repo_refused_with_stash_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::write(repo.join("dirty.txt"), "dirty").expect("dirty");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("dirty run")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("working tree has uncommitted changes"));
    assert!(stderr.contains("try: git stash && deadreckon run"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn no_commits_refused_with_initial_commit_hint() {
    let temp = repo_tempdir();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("no commits")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("git repo has no commits"));
    assert!(stderr.contains("try: git commit -m initial"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn detached_head_refused_with_switch_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    git(&repo, &["checkout", "--detach"]).expect("detach");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("detached")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("HEAD is detached"));
    assert!(stderr.contains("try: git switch -c <branch>"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn mid_merge_refused_with_abort_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::write(repo.join(".git/MERGE_HEAD"), "deadbeef\n").expect("merge head");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("mid merge")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("git is in the middle of a merge"));
    assert!(stderr.contains("try: git merge --abort"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn mid_rebase_refused_with_abort_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::create_dir_all(repo.join(".git/rebase-merge")).expect("rebase dir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("mid rebase")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("git is in the middle of a rebase"));
    assert!(stderr.contains("try: git rebase --abort"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn allow_dirty_seeds_uncommitted_into_worktree() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::write(repo.join("keep.txt"), "clean").expect("keep");
    git(&repo, &["add", "keep.txt"]).expect("add keep");
    git(&repo, &["commit", "-m", "add keep"]).expect("commit keep");
    fs::write(repo.join("keep.txt"), "dirty keep").expect("dirty keep");
    fs::write(repo.join("untracked.txt"), "dirty untracked").expect("untracked");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("allow dirty")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--allow-dirty")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.expect("worktree");
    assert!(record.dirty_files_seeded);
    assert_eq!(
        fs::read_to_string(worktree.join("keep.txt")).expect("keep"),
        "dirty keep"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("untracked.txt")).expect("untracked"),
        "dirty untracked"
    );
}

#[test]
fn branch_collision_refused_with_branch_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("branch collision")
        .arg("--smoke")
        .arg("--branch")
        .arg("main")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("branch main already exists"));
    assert!(stderr.contains("try: pass --branch <other-name>"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn worktree_path_collision_appends_suffix() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    let options = WorktreeOptions {
        run_id: run_id.clone(),
        task_key: "collision".to_string(),
        source_path: repo.clone(),
        base_ref: None,
        branch_name: Some("dr/collision-a".to_string()),
        allow_dirty: false,
    };
    let first = prepare_worktree_record(&paths, options.clone()).expect("first");
    fs::create_dir_all(first.worktree_path.as_ref().expect("worktree")).expect("worktree");
    fs::write(
        first
            .worktree_path
            .as_ref()
            .expect("worktree")
            .join("occupied"),
        "occupied",
    )
    .expect("occupied");

    let second = prepare_worktree_record(&paths, options).expect("second");

    assert!(
        second
            .worktree_path
            .expect("worktree")
            .file_name()
            .expect("name")
            .to_string_lossy()
            .ends_with("-2")
    );
}

#[test]
fn preview_flag_exits_zero_without_state_change() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("preview run")
        .arg("--smoke")
        .arg("--preview")
        .output()
        .expect("preview");

    assert_success(&output);
    let stderr = stderr(&output);
    assert!(stderr.contains("deadreckon: ready to run"));
    assert!(stderr.contains("  goal:     preview run"));
    assert!(stderr.contains("  mode:     worktree"));
    assert!(stderr.contains("  on success: deadreckon apply "));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn preview_block_contains_required_fields_in_order() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("ordered preview")
        .arg("--smoke")
        .arg("--preview")
        .output()
        .expect("preview");

    assert_success(&output);
    let stderr = stderr(&output);
    let fields = [
        "deadreckon: ready to run",
        "  goal:",
        "  source:",
        "  mode:",
        "    branch:",
        "    base:",
        "    worktree:",
        "  provider:",
        "  model:",
        "  sandbox:",
        "  caps:",
        "  on success:",
        "  on fail:",
    ];
    let mut cursor = 0;
    for field in fields {
        let offset = stderr[cursor..]
            .find(field)
            .unwrap_or_else(|| panic!("missing {field} in {stderr}"));
        cursor += offset + field.len();
    }
}

#[test]
fn brief_mode_is_one_line() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("brief preview")
        .arg("--smoke")
        .arg("--preview")
        .arg("--brief")
        .output()
        .expect("preview");

    assert_success(&output);
    let stderr = stderr(&output);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("mode=worktree branch=dr/brief-preview-"));
    assert!(stderr.contains(" provider=smoke model=local-scripted-smoke"));
    assert!(stderr.contains(" cap=$10/1h"));
}

#[test]
fn run_preview_shows_provider_and_model_override() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        r#"
default_provider = "openai"
fallback = ["openai"]

[defaults]
provider = "openai"
sandbox = "none"
max_spend = 10

[providers.openai]
kind = "open-ai"
api_key = "test"
model = "configured-model"
"#,
    )
    .expect("config");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("model preview")
        .arg("--preview")
        .arg("--model")
        .arg("override-model")
        .output()
        .expect("preview");

    assert_success(&output);
    let stderr = stderr(&output);
    assert!(stderr.contains("  provider: openai"), "{stderr}");
    assert!(stderr.contains("  model:    override-model"), "{stderr}");
}

#[test]
fn config_provider_and_model_are_user_friendly_shortcuts() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let provider = deadreckon(&paths)
        .arg("config")
        .arg("provider")
        .arg("cli:codex")
        .output()
        .expect("provider");
    assert_success(&provider);

    let model = deadreckon(&paths)
        .arg("config")
        .arg("model")
        .arg("gpt-5.1-codex")
        .output()
        .expect("model");
    assert_success(&model);

    let raw = fs::read_to_string(paths.config_path()).expect("config");
    assert!(raw.contains("provider = \"cli:codex\""), "{raw}");
    assert!(raw.contains("\"cli:codex\""), "{raw}");
    assert!(raw.contains("model = \"gpt-5.1-codex\""), "{raw}");

    let show = deadreckon(&paths)
        .arg("config")
        .arg("model")
        .output()
        .expect("show model");
    assert_success(&show);
    let stdout = stdout(&show);
    assert!(stdout.contains("cli:codex"), "{stdout}");
    assert!(stdout.contains("gpt-5.1-codex"), "{stdout}");
    assert!(stdout.contains("deadreckon run \"goal\""), "{stdout}");
}

#[test]
fn yes_flag_skips_confirm_prompt() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("yes run")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("run");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("started run "));
    assert!(stdout.contains("completed run "));
    assert!(!stderr(&output).contains("continue?"));
}

#[test]
fn non_tty_without_yes_refuses() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("needs yes")
        .arg("--smoke")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("non-interactive without --yes"));
    assert!(stderr.contains("try: --yes (skip confirm) or run interactively"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn non_git_interactive_offers_three_choices_with_init_default() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon_pty(
        &paths,
        &source,
        "3\n",
        &["run", "interactive choices", "--smoke", "--yes"],
    );

    assert_success(&output);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("this is not a git repo. options:"));
    assert!(text.contains("[1] git init for me"));
    assert!(text.contains("[2] copy mode"));
    assert!(text.contains("[3] cancel"));
    assert!(text.contains("choose [1]:"));
    assert!(!source.join(".git").exists());
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn non_git_choice_init_runs_git_init_then_worktree() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("app.txt"), "app").expect("app");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon_pty(
        &paths,
        &source,
        "\n",
        &[
            "run",
            "init default",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--no-hints",
        ],
    );

    assert_success(&output);
    assert!(source.join(".git").exists());
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::Worktree);
    assert_eq!(
        record.source_git_root.expect("git root"),
        source.canonicalize().expect("canonical")
    );
}

#[test]
fn non_git_choice_copy_resolves_to_copy_mode() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("app.txt"), "app").expect("app");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon_pty(
        &paths,
        &source,
        "2\n",
        &[
            "run",
            "copy choice",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--no-hints",
        ],
    );

    assert_success(&output);
    assert!(!source.join(".git").exists());
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::Copy);
    assert_eq!(
        record.source_path.expect("source"),
        source.canonicalize().expect("canonical")
    );
}

#[test]
fn non_git_choice_cancel_exits_zero_no_changes() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon_pty(
        &paths,
        &source,
        "3\n",
        &["run", "cancel choice", "--smoke", "--yes"],
    );

    assert_success(&output);
    assert!(stdout(&output).contains("cancelled"));
    assert!(!source.join(".git").exists());
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn non_git_non_interactive_refuses_with_try_line() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("plain non tty")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("non-interactive without a mode flag"));
    assert!(stderr.contains("try: --fresh or --from . or git init"));
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn copy_mode_respects_gitignore() {
    let temp = repo_tempdir();
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join(".gitignore"), "ignored.txt\n").expect("ignore");
    fs::write(source.join("kept.txt"), "keep").expect("kept");
    fs::write(source.join("ignored.txt"), "ignore").expect("ignored");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("run")
        .arg("copy mode")
        .arg("--from")
        .arg(&source)
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::Copy);
    assert!(state.working_dir.join("kept.txt").exists());
    assert!(!state.working_dir.join("ignored.txt").exists());
}

#[test]
fn copy_mode_succeeds_in_non_git_dir() {
    let temp = repo_tempdir();
    let source = temp.path().join("plain-source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("hello.txt"), "hello").expect("hello");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("run")
        .arg("copy non git")
        .arg("--from")
        .arg(&source)
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert!(state.working_dir.join("hello.txt").exists());
}

#[test]
fn copy_mode_materialize_writes_dest_unchanged_from_today() {
    let temp = repo_tempdir();
    let source = temp.path().join("copy-source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("hello.txt"), "hello").expect("hello");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("run")
        .arg("copy materialize")
        .arg("--from")
        .arg(&source)
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let dest = temp.path().join("materialized");
    let materialize = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("materialize")
        .arg(&run.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert_success(&materialize);
    assert_eq!(
        fs::read_to_string(dest.join("hello.txt")).expect("hello"),
        "hello"
    );
    assert!(dest.join(".deadreckon/parent.json").exists());
}

#[test]
fn in_place_requires_double_confirm_or_i_know_flag() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place refused")
        .arg("--in-place")
        .arg("--smoke")
        .arg("--yes")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("--in-place requires --i-know-its-a-lot"));
    assert!(stderr.contains("try: add --i-know-its-a-lot or run in a TTY"));
}

#[test]
fn in_place_run_edits_source_path_directly() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place smoke")
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    assert!(source.join("Cargo.toml").exists());
    assert!(source.join("README.md").exists());
    assert!(source.join(".deadreckon/codebase.json").exists());
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    assert_eq!(record.mode, CodebaseMode::InPlace);
}

#[test]
fn in_place_undo_reverts_via_runstate_snapshot() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("keep.txt"), "before").expect("keep");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place undo")
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    assert!(source.join("Cargo.toml").exists());
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");

    let undo = deadreckon(&paths)
        .arg("undo")
        .arg("--run")
        .arg(&run.run_id)
        .arg("--turn")
        .arg("0")
        .output()
        .expect("undo");

    assert_success(&undo);
    assert!(!source.join("Cargo.toml").exists());
    assert_eq!(
        fs::read_to_string(source.join("keep.txt")).expect("keep"),
        "before"
    );
    assert!(source.join(".deadreckon/codebase.json").exists());
}

#[test]
fn materialize_in_place_refuses_with_undo_hint() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place materialize")
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");
    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");

    let materialize = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("materialize")
        .arg(&run.run_id)
        .arg("--dest")
        .arg(temp.path().join("dest"))
        .output()
        .expect("materialize");

    assert!(!materialize.status.success());
    let stderr = stderr(&materialize);
    assert!(stderr.contains("materialize is not needed; run edited the source in-place"));
    assert!(stderr.contains("try: deadreckon undo"));
}

#[test]
fn in_place_refuses_apply_with_try_undo_hint() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place apply")
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("run");
    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");

    let apply = deadreckon(&paths)
        .arg("apply")
        .arg(&run.run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert!(!apply.status.success());
    let stderr = stderr(&apply);
    assert!(stderr.contains("apply requires worktree mode; run was in-place"));
    assert!(stderr.contains("try: deadreckon undo to revert if needed"));
}

#[test]
fn apply_squash_creates_commit_on_user_branch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert_success(&apply);
    assert!(repo.join("Cargo.toml").exists());
    let log = git_stdout(&repo, &["log", "-1", "--oneline"]);
    assert!(log.contains("worktree smoke"));
}

#[test]
fn apply_squash_is_idempotent_after_changes_already_landed() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let first_apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("first apply");
    assert_success(&first_apply);
    let commits_after_first = git_stdout(&repo, &["rev-list", "--count", "HEAD"]);

    let second_apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("second apply");

    assert_success(&second_apply);
    let stdout = stdout(&second_apply);
    assert!(stdout.contains("already applied"), "{stdout}");
    assert_eq!(
        git_stdout(&repo, &["rev-list", "--count", "HEAD"]),
        commits_after_first
    );
}

#[test]
fn apply_refuses_on_dirty_user_tree() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    fs::write(repo.join("local-dirty.txt"), "dirty").expect("dirty");

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert!(!apply.status.success());
    let stderr = stderr(&apply);
    assert!(stderr.contains("your working tree has uncommitted changes"));
    assert!(stderr.contains(&format!(
        "try: deadreckon apply {run_id} --autostash --no-confirm"
    )));
}

#[test]
fn apply_autostash_restores_untracked_user_files() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    fs::write(repo.join(".cursorindexingignore"), "target\n").expect("cursor ignore");

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .arg("--autostash")
        .output()
        .expect("apply");

    assert_success(&apply);
    assert_eq!(
        fs::read_to_string(repo.join(".cursorindexingignore")).expect("cursor ignore"),
        "target\n"
    );
    assert!(git_stdout(&repo, &["status", "--short"]).contains("?? .cursorindexingignore"));
    assert!(!git_stdout(&repo, &["stash", "list"]).contains("deadreckon apply"));
}

#[test]
fn apply_cleanup_removes_worktree_and_branch_after_success() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .arg("--cleanup")
        .output()
        .expect("apply");

    assert_success(&apply);
    assert!(!worktree.exists());
    assert!(!git_ref_exists(&repo, &branch));
    assert!(state.run_root.join("abandoned.json").exists());
}

#[test]
fn apply_cleanup_after_already_applied_removes_worktree_and_branch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");

    let first_apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("first apply");
    assert_success(&first_apply);

    let cleanup_apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .arg("--cleanup")
        .output()
        .expect("cleanup apply");

    assert_success(&cleanup_apply);
    let stdout = stdout(&cleanup_apply);
    assert!(stdout.contains("already applied"), "{stdout}");
    assert!(!worktree.exists());
    assert!(!git_ref_exists(&repo, &branch));
    assert!(state.run_root.join("abandoned.json").exists());
}

#[test]
fn apply_merge_no_ff_creates_merge_commit() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--strategy")
        .arg("merge")
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert_success(&apply);
    assert_eq!(
        git_stdout(&repo, &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count(),
        3
    );
}

#[test]
fn apply_cherry_pick_preserves_turn_commits() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--strategy")
        .arg("cherry-pick")
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert_success(&apply);
    let log = git_stdout(&repo, &["log", "-1", "--pretty=%s"]);
    assert!(log.starts_with("turn "), "{log}");
}

#[test]
fn apply_conflict_leaves_markers_and_prints_resolve_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::write(repo.join("conflict.txt"), "base\n").expect("conflict");
    git(&repo, &["add", "conflict.txt"]).expect("add conflict");
    git(&repo, &["commit", "-m", "add conflict base"]).expect("commit conflict");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");

    fs::write(worktree.join("conflict.txt"), "branch\n").expect("branch conflict");
    git(&worktree, &["add", "conflict.txt"]).expect("branch add");
    git(&worktree, &["commit", "-m", "branch conflict"]).expect("branch commit");
    fs::write(repo.join("conflict.txt"), "main\n").expect("main conflict");
    git(&repo, &["add", "conflict.txt"]).expect("main add");
    git(&repo, &["commit", "-m", "main conflict"]).expect("main commit");

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert!(!apply.status.success());
    let stderr = stderr(&apply);
    assert!(stderr.contains("merge produced conflicts"));
    assert!(stderr.contains(&format!(
        "try: resolve, then git commit && deadreckon abandon {run_id}"
    )));
    assert!(
        fs::read_to_string(repo.join("conflict.txt"))
            .expect("conflict markers")
            .contains("<<<<<<<")
    );
}

#[test]
fn apply_refuses_non_worktree_with_mode_specific_hint() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("run")
        .arg("fresh apply")
        .arg("--fresh")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("run");
    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");

    let apply = deadreckon(&paths)
        .arg("apply")
        .arg(&run.run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert!(!apply.status.success());
    let stderr = stderr(&apply);
    assert!(stderr.contains("apply requires worktree mode; run was fresh"));
    assert!(stderr.contains(&format!(
        "try: deadreckon materialize {} --dest <path>",
        run.run_id
    )));
}

#[test]
fn apply_refuses_uncompleted_run() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_run(
        &paths,
        RunOptions {
            goal: "planned only".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");

    let apply = deadreckon(&paths)
        .arg("apply")
        .arg(&state.run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert!(!apply.status.success());
    let stderr = stderr(&apply);
    assert!(stderr.contains(&format!("run {} is planned", state.run_id)));
    assert!(stderr.contains(&format!("try: deadreckon resume {}", state.run_id)));
}

#[test]
fn post_apply_hint_includes_git_log_one_stat() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&run_id)
        .arg("--no-confirm")
        .output()
        .expect("apply");

    assert_success(&apply);
    let stdout = stdout(&apply);
    assert!(stdout.contains(&format!("applied {run_id} onto")));
    assert!(stdout.contains("commit "));
    assert!(stdout.contains("Cargo.toml"));
    assert!(stdout.contains(&format!("next: deadreckon discard {}", &run_id[..8])));
}

#[test]
fn abandon_removes_worktree_and_branch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");

    let abandon = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .output()
        .expect("abandon");

    assert_success(&abandon);
    assert!(!worktree.exists());
    assert!(
        Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "--verify", &branch])
            .output()
            .expect("branch check")
            .status
            .code()
            .is_some_and(|code| code != 0)
    );
    assert!(state.run_root.join("abandoned.json").exists());
}

#[test]
fn abandon_keep_branch_keeps_branch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");

    let abandon = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .arg("--keep-branch")
        .output()
        .expect("abandon");

    assert_success(&abandon);
    assert!(!worktree.exists());
    git(&repo, &["rev-parse", "--verify", &branch]).expect("branch kept");
    assert!(state.run_root.join("abandoned.json").exists());
}

#[test]
fn abandon_force_terminates_running_run_then_cleans() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
    let record = prepare_worktree_record(
        &paths,
        WorktreeOptions {
            run_id: run_id.clone(),
            task_key: "force-abandon".to_string(),
            source_path: repo.clone(),
            base_ref: None,
            branch_name: Some("dr/force-abandon-bbbbbbbb".to_string()),
            allow_dirty: false,
        },
    )
    .expect("record");
    create_worktree(&record).expect("worktree");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "force abandon".to_string(),
            cwd: repo.clone(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: Some(run_id.clone()),
            codebase: Some(record),
        },
    )
    .expect("run");
    let mut child = Command::new("sleep").arg("60").spawn().expect("sleep");
    state.status = RunStatus::Executing;
    state.child_pids = vec![child.id()];
    save_state(&state).expect("save");

    let abandon = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .arg("--force")
        .output()
        .expect("abandon");

    assert_success(&abandon);
    let _ = child.wait();
    assert!(!deadreckon_core::pid_is_alive(state.child_pids[0]));
    assert!(!worktree.exists());
    assert!(!git_ref_exists(&repo, &branch));
    let reloaded = load_run(&paths, &run_id).expect("state");
    assert_eq!(reloaded.status, RunStatus::Killed);
    assert!(reloaded.run_root.join("abandoned.json").exists());
}

#[test]
fn abandon_idempotent_when_already_abandoned() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let first = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .output()
        .expect("abandon first");
    assert_success(&first);
    let second = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .output()
        .expect("abandon second");

    assert_success(&second);
    assert!(stdout(&second).contains(&format!("abandoned {run_id}")));
}

#[test]
fn abandon_writes_abandoned_json_for_list_visibility() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let abandon = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .output()
        .expect("abandon");
    assert_success(&abandon);
    let state = load_run(&paths, &run_id).expect("state");
    assert!(state.run_root.join("abandoned.json").exists());

    let list = deadreckon(&paths)
        .current_dir(&repo)
        .arg("list")
        .output()
        .expect("list");
    assert_success(&list);
    assert!(stdout(&list).contains("abandoned"));
}

#[test]
fn post_abandon_hint_lists_removed_paths() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);
    let state = load_run(&paths, &run_id).expect("state");
    let record = read_codebase_record(&state.working_dir).expect("codebase");
    let worktree = record.worktree_path.clone().expect("worktree");
    let branch = record.branch_name.clone().expect("branch");

    let abandon = deadreckon(&paths)
        .current_dir(&repo)
        .arg("abandon")
        .arg(&run_id)
        .output()
        .expect("abandon");

    assert_success(&abandon);
    let stdout = stdout(&abandon);
    assert!(stdout.contains(&format!("abandoned {run_id}")));
    assert!(stdout.contains(&format!("removed: {}", worktree.display())));
    assert!(stdout.contains(&format!("removed: branch {branch}")));
}

#[test]
fn abandon_in_place_refuses_with_undo_hint() {
    let temp = repo_tempdir();
    let source = temp.path().join("in-place-source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place abandon")
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("run");
    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");

    let abandon = deadreckon(&paths)
        .arg("abandon")
        .arg(&run.run_id)
        .output()
        .expect("abandon");

    assert!(!abandon.status.success());
    let stderr = stderr(&abandon);
    assert!(stderr.contains("cannot abandon in-place edits"));
    assert!(stderr.contains("try: deadreckon undo"));
}

#[test]
fn materialize_in_worktree_refuses_with_apply_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let materialize = deadreckon(&paths)
        .current_dir(&repo)
        .arg("materialize")
        .arg(&run_id)
        .arg("--dest")
        .arg(temp.path().join("dest"))
        .output()
        .expect("materialize");

    assert!(!materialize.status.success());
    let stderr = stderr(&materialize);
    assert!(stderr.contains("materialize is for copy/fresh runs; run was worktree"));
    assert!(stderr.contains(&format!("try: deadreckon apply {run_id}")));
}

#[test]
fn list_shows_mode_column() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    run_worktree_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("list")
        .output()
        .expect("list");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("MODE"));
    assert!(stdout.contains("worktree"));
}

#[test]
fn list_default_is_compact_and_full_keeps_full_values() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let goal = "make this exceptionally long paint application goal readable in list output without wrapping every row across the terminal";
    let state = create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            run_id: None,
            codebase: Some(CodebaseRecord::fresh()),
        },
    )
    .expect("run");

    let compact = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("list")
        .output()
        .expect("list");
    assert_success(&compact);
    let compact_stdout = stdout(&compact);
    assert!(compact_stdout.contains("AGE"));
    assert!(compact_stdout.contains(&state.run_id[..8]));
    assert!(!compact_stdout.contains(&state.run_id));
    assert!(compact_stdout.contains("..."));
    assert!(
        compact_stdout
            .lines()
            .all(|line| line.chars().count() <= 180),
        "{compact_stdout}"
    );

    let full = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["list", "--full"])
        .output()
        .expect("list full");
    assert_success(&full);
    let full_stdout = stdout(&full);
    assert!(full_stdout.contains(&state.run_id));
    assert!(full_stdout.contains(goal));
}

#[test]
fn list_defaults_to_current_scope_and_all_shows_other_scopes() {
    let temp = repo_tempdir();
    let repo_a = clean_git_repo_in(&temp, "repo-a");
    let repo_b = clean_git_repo_in(&temp, "repo-b");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_a = run_worktree_smoke(&paths, &repo_a);
    let run_b = run_worktree_smoke(&paths, &repo_b);

    let scoped = deadreckon(&paths)
        .current_dir(&repo_a)
        .arg("list")
        .output()
        .expect("list");
    assert_success(&scoped);
    let scoped_stdout = stdout(&scoped);
    assert!(scoped_stdout.contains(&run_a[..8]), "{scoped_stdout}");
    assert!(!scoped_stdout.contains(&run_b[..8]), "{scoped_stdout}");

    let all = deadreckon(&paths)
        .current_dir(&repo_a)
        .args(["list", "--all"])
        .output()
        .expect("list all");
    assert_success(&all);
    let all_stdout = stdout(&all);
    assert!(all_stdout.contains(&run_a[..8]), "{all_stdout}");
    assert!(all_stdout.contains(&run_b[..8]), "{all_stdout}");
}

#[test]
fn latest_alias_resolves_to_current_scope_for_show_and_status() {
    let temp = repo_tempdir();
    let repo_a = clean_git_repo_in(&temp, "repo-a");
    let repo_b = clean_git_repo_in(&temp, "repo-b");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_a = run_worktree_smoke(&paths, &repo_a);
    let run_b = run_worktree_smoke(&paths, &repo_b);

    let show = deadreckon(&paths)
        .current_dir(&repo_a)
        .args(["show", "latest"])
        .output()
        .expect("show latest");
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains(&run_a), "{show_stdout}");
    assert!(!show_stdout.contains(&run_b), "{show_stdout}");

    let status = deadreckon(&paths)
        .current_dir(&repo_a)
        .arg("status")
        .output()
        .expect("status");
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("deadreckon status"));
    assert!(status_stdout.contains(&run_a[..8]), "{status_stdout}");
    assert!(status_stdout.contains("next actions:"));
}

#[test]
fn cleanup_completed_defaults_to_current_scope() {
    let temp = repo_tempdir();
    let repo_a = clean_git_repo_in(&temp, "repo-a");
    let repo_b = clean_git_repo_in(&temp, "repo-b");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_a = run_worktree_smoke(&paths, &repo_a);
    let run_b = run_worktree_smoke(&paths, &repo_b);
    let state_a = load_run(&paths, &run_a).expect("state a");
    let state_b = load_run(&paths, &run_b).expect("state b");
    let worktree_a = read_codebase_record(&state_a.working_dir)
        .expect("codebase a")
        .worktree_path
        .expect("worktree a");
    let worktree_b = read_codebase_record(&state_b.working_dir)
        .expect("codebase b")
        .worktree_path
        .expect("worktree b");

    let cleanup = deadreckon(&paths)
        .current_dir(&repo_a)
        .args(["cleanup", "--completed", "--no-confirm"])
        .output()
        .expect("cleanup");
    assert_success(&cleanup);
    assert!(!worktree_a.exists());
    assert!(worktree_b.exists());
}

#[test]
fn show_reveals_codebase_lineage() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run_id = run_worktree_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .arg("show")
        .arg(&run_id)
        .output()
        .expect("show");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Mode worktree"));
    assert!(stdout.contains("Branch dr/worktree-smoke-"));
    assert!(stdout.contains("Worktree "));
}

#[test]
fn post_run_hint_lists_apply_and_abandon_lines() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("hinted run")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");

    assert_success(&output);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let short = &run.run_id[..8];
    let stdout = stdout(&output);
    assert!(stdout.contains("next actions:"));
    assert!(stdout.contains(&format!("apply:   deadreckon apply {short}")));
    assert!(stdout.contains(&format!(
        "cleanup: deadreckon apply {short} --autostash --cleanup"
    )));
    assert!(stdout.contains(&format!("discard: deadreckon discard {short}")));
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn run_worktree_smoke(paths: &DeadreckonPaths, repo: &std::path::Path) -> String {
    let output = deadreckon(paths)
        .current_dir(repo)
        .arg("run")
        .arg("worktree smoke")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .output()
        .expect("run");
    assert_success(&output);
    list_runs(paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run")
        .run_id
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    clean_git_repo_in(temp, "repo")
}

fn clean_git_repo_in(temp: &TempDir, name: &str) -> PathBuf {
    let repo = temp.path().join(name);
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
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

fn deadreckon_pty(
    paths: &DeadreckonPaths,
    cwd: &std::path::Path,
    input: &str,
    args: &[&str],
) -> std::process::Output {
    let command = std::iter::once(env!("CARGO_BIN_EXE_deadreckon").to_string())
        .chain(args.iter().map(|arg| arg.to_string()))
        .map(|part| tcl_brace_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    let answer = input.trim_end_matches('\n').to_string() + "\r";
    let script = format!(
        "set timeout 30\ncd {}\nset env(DEADRECKON_HOME) {}\nspawn {}\nexpect \"choose \\[1\\]:\"\nsend -- \"{}\"\nexpect {{\n  \"completed run\" {{ exit 0 }}\n  \"cancelled\" {{ exit 0 }}\n  eof {{ exit 125 }}\n  timeout {{ exit 124 }}\n}}\n",
        tcl_brace_quote(&cwd.display().to_string()),
        tcl_brace_quote(&paths.home().display().to_string()),
        command,
        tcl_string_escape(&answer)
    );
    Command::new("expect")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("expect")
}

fn tcl_brace_quote(value: &str) -> String {
    format!("{{{}}}", value.replace('\\', "\\\\").replace('}', "\\}"))
}

fn tcl_string_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
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

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if args.first() == Some(&"init") && output.status.success() {
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["config", "user.email", "deadreckon@example.invalid"])
            .output();
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["config", "user.name", "deadreckon"])
            .output();
    }
    assert!(
        output.status.success(),
        "git {:?}\nstdout:{}\nstderr:{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn git_stdout(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {:?}\nstdout:{}\nstderr:{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_ref_exists(cwd: &std::path::Path, name: &str) -> bool {
    Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--verify", name])
        .output()
        .expect("git")
        .status
        .success()
}
