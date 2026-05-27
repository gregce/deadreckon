#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::net::SocketAddr;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::{
    CodebaseMode, DeadreckonPaths, PhaseId, PhaseStatus, PipelineState, RunOptions, RunStatus,
    TraceRecord, acquire_lock, append_trace, create_run, list_runs, load_run,
    promote_completed_run, read_codebase_record, save_state, write_acceptance_marker,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[test]
fn materialize_copies_library_to_dest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize copy");
    let dest = temp.path().join("materialized-copy");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(dest.join("app.txt")).expect("app"),
        "parent app"
    );
    assert!(!dest.join("manifest.json").exists());
    assert_eq!(
        parent_json(&dest)["kind"].as_str().expect("kind"),
        "materialized"
    );
}

#[test]
fn materialize_refuses_existing_nonempty_dest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize refuse");
    let dest = temp.path().join("nonempty");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("keep.txt"), "keep").expect("keep");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("is not empty"));
    assert_eq!(
        fs::read_to_string(dest.join("keep.txt")).expect("keep"),
        "keep"
    );
}

#[test]
fn materialize_force_overwrites() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize force");
    let dest = temp.path().join("force");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("stale.txt"), "stale").expect("stale");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("materialize");

    assert_success(&output);
    assert!(!dest.join("stale.txt").exists());
    assert!(dest.join("app.txt").exists());
}

#[test]
fn materialize_writes_parent_manifest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize parent manifest");
    let dest = temp.path().join("manifest");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .arg("--include-manifest")
        .output()
        .expect("materialize");

    assert_success(&output);
    assert!(dest.join("manifest.json").exists());
    let parent_marker = parent_json(&dest);
    assert_eq!(parent_marker["parent_run_id"], parent.run_id);
    assert_eq!(parent_marker["parent_scope"], parent.scope);
}

#[test]
fn materialize_records_reverse_marker_in_library() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize reverse");
    let dest = temp.path().join("reverse");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert_success(&output);
    let marker = fs::read_to_string(
        paths
            .library_dir(&parent.scope, &parent.run_id)
            .join(".materialized-to"),
    )
    .expect("reverse marker");
    assert!(marker.contains(&dest.display().to_string()));
}

#[test]
fn materialize_refuses_dest_inside_runstate() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize inside runstate");
    let dest = paths.home().join("runstate").join("bad-dest");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("refusing to export back into runstate"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_creates_new_run_with_parent_artifacts() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend parent artifacts");
    fs::create_dir_all(parent.cwd.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        parent.cwd.join(".deadreckon/acceptance.yaml"),
        "name: extend acceptance\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/child.txt\"\n",
    )
    .expect("acceptance yaml");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add child file")
        .output()
        .expect("extend");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("started run "), "{stdout}");
    assert!(stdout.contains("provider: mock"), "{stdout}");
    assert!(stdout.contains("model   : mock-agent"), "{stdout}");
    assert!(stdout.contains("attach  : deadreckon attach "), "{stdout}");
    assert!(stdout.contains("state   : "), "{stdout}");
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    assert_eq!(child.status, RunStatus::Completed);
    assert_eq!(child.scope, parent.scope);
    assert_eq!(child.task_key, parent.task_key);
    assert_eq!(
        fs::read_to_string(child.working_dir.join("app.txt")).expect("parent app"),
        "parent app"
    );
    assert_eq!(
        fs::read_to_string(child.working_dir.join("child.txt")).expect("child"),
        "extended child"
    );
    assert!(
        fs::read_to_string(child.run_root.join("acceptance.yaml"))
            .expect("child acceptance")
            .contains("extend acceptance")
    );
    assert_eq!(
        parent_json(&child.working_dir)["kind"]
            .as_str()
            .expect("kind"),
        "extended"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_pre_populates_history_with_parent_summary() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend history parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add child history")
        .arg("--max-context-turns")
        .arg("2")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let history = fs::read_to_string(child.run_root.join("history.json")).expect("history");
    assert!(history.contains("Previous run summary"));
    assert!(history.contains("extend history parent"));
    assert!(history.contains("Recent activity"));
    assert!(history.contains("parent-tool-1"));
    let traces = fs::read_to_string(child.run_root.join("traces.jsonl")).expect("traces");
    assert!(traces.contains("extended_from_parent"));
}

#[test]
fn extend_refuses_incomplete_parent() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let paths = DeadreckonPaths::from_home(&home);
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    let parent = create_run(
        &paths,
        RunOptions {
            goal: "incomplete parent".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(30.0),
            run_id: None,
            codebase: None,
        },
    )
    .expect("parent");

    let output = deadreckon(&paths)
        .arg("extend")
        .arg(&parent.run_id)
        .arg("should refuse")
        .output()
        .expect("extend");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("use 'deadreckon resume' for incomplete runs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_locks_correctly_against_concurrent_extension() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend locked parent");
    let _lock = acquire_lock(
        &paths,
        &parent.task_key,
        "held-run",
        &parent.scope,
        "test-held",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )
    .expect("lock");

    let output = deadreckon(&paths)
        .arg("extend")
        .arg(&parent.run_id)
        .arg("blocked by lock")
        .output()
        .expect("extend");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("lock held"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_no_context_flag_omits_recent_turns() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend no context parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add without context")
        .arg("--no-context")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let history = fs::read_to_string(child.run_root.join("history.json")).expect("history");
    assert!(history.contains("Previous run summary"));
    assert!(!history.contains("Recent activity"));
    assert!(!history.contains("parent-tool-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn materialize_then_extend_roundtrip() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "roundtrip parent");
    let dest = temp.path().join("roundtrip-materialized");
    let materialize = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");
    assert_success(&materialize);
    assert!(dest.join("app.txt").exists());

    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = extend_command(&paths, &parent, "extend after materialize")
        .output()
        .expect("extend");
    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    assert!(child.working_dir.join("app.txt").exists());
    assert!(child.working_dir.join("child.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_in_worktree_chains_branches() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: worktree acceptance\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance yaml");
    git(&repo, &["add", "-f", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let parent_run = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("worktree parent")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("parent run");
    assert_success(&parent_run);
    let parent = load_run(&paths, &run_id_from_stdout(&parent_run)).expect("parent");
    let parent_record = read_codebase_record(&parent.working_dir).expect("parent codebase");
    let parent_branch = parent_record.branch_name.clone().expect("parent branch");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "worktree child")
        .current_dir(&repo)
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let child_record = read_codebase_record(&child.working_dir).expect("child codebase");
    assert_eq!(child_record.mode, CodebaseMode::Worktree);
    assert_eq!(
        child_record.base_ref.as_deref(),
        Some(parent_branch.as_str())
    );
    assert_eq!(
        child_record.parent_branch.as_deref(),
        Some(parent_branch.as_str())
    );
    assert_ne!(child_record.branch_name.as_ref(), Some(&parent_branch));
    assert!(child_record.worktree_path.expect("child worktree").exists());
    assert!(child.working_dir.join("child.txt").exists());
    assert!(
        fs::read_to_string(child.run_root.join("acceptance.yaml"))
            .expect("child acceptance")
            .contains("worktree acceptance")
    );
    assert_eq!(
        parent_json(&child.working_dir)["kind"]
            .as_str()
            .expect("kind"),
        "extended"
    );
    git(&repo, &["rev-parse", "--verify", &parent_branch]).expect("parent branch");
    git(
        &repo,
        &[
            "rev-parse",
            "--verify",
            child_record.branch_name.as_deref().expect("child branch"),
        ],
    )
    .expect("child branch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_worktree_after_apply_cleanup_recovers_from_library_record() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let parent_run = deadreckon(&paths)
        .current_dir(&repo)
        .arg("run")
        .arg("worktree parent cleanup")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--yes")
        .arg("--no-hints")
        .output()
        .expect("parent run");
    assert_success(&parent_run);
    let parent = load_run(&paths, &run_id_from_stdout(&parent_run)).expect("parent");
    let parent_record = read_codebase_record(&parent.working_dir).expect("parent codebase");
    let parent_branch = parent_record.branch_name.clone().expect("parent branch");
    let original_base = parent_record.base_ref.clone().expect("parent base");

    let apply = deadreckon(&paths)
        .current_dir(&repo)
        .arg("apply")
        .arg(&parent.run_id)
        .arg("--no-confirm")
        .arg("--cleanup")
        .output()
        .expect("apply cleanup");
    assert_success(&apply);
    assert!(!git_ref_exists(&repo, &parent_branch));

    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = extend_command(&paths, &parent, "worktree child after cleanup")
        .current_dir(&repo)
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let child_record = read_codebase_record(&child.working_dir).expect("child codebase");
    assert_eq!(child_record.mode, CodebaseMode::Worktree);
    assert_eq!(
        child_record.base_ref.as_deref(),
        Some(original_base.as_str())
    );
    assert_eq!(
        child_record.parent_branch.as_deref(),
        Some(parent_branch.as_str())
    );
    assert!(child.working_dir.join("child.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_in_copy_unchanged_from_today() {
    let temp = repo_tempdir();
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("app.txt"), "copy parent").expect("app");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let parent_run = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("run")
        .arg("copy parent")
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
        .expect("parent run");
    assert_success(&parent_run);
    let parent = load_run(&paths, &run_id_from_stdout(&parent_run)).expect("parent");
    assert_eq!(
        read_codebase_record(&parent.working_dir)
            .expect("parent codebase")
            .mode,
        CodebaseMode::Copy
    );
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "copy child")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    assert_eq!(
        fs::read_to_string(child.working_dir.join("app.txt")).expect("app"),
        "copy parent"
    );
    assert_eq!(
        fs::read_to_string(child.working_dir.join("child.txt")).expect("child"),
        "extended child"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_in_in_place_refuses_with_run_hint() {
    let temp = repo_tempdir();
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let parent_run = deadreckon(&paths)
        .current_dir(&source)
        .arg("run")
        .arg("in-place parent")
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
        .expect("parent run");
    assert_success(&parent_run);
    let parent = load_run(&paths, &run_id_from_stdout(&parent_run)).expect("parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "in-place child")
        .output()
        .expect("extend");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("extend is not available for in-place runs"));
    assert!(stderr.contains("try: deadreckon run --in-place --i-know-its-a-lot"));
}

#[test]
fn library_list_shows_materialized_status() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "list materialized parent");
    let dest = temp.path().join("listed-materialized");
    let materialize = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");
    assert_success(&materialize);
    assert_eq!(list_runs(&paths, None).expect("runs").len(), 1);

    let output = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["library", "list"])
        .output()
        .expect("library list");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("EXPORTED"));
    assert!(stdout.contains("yes (1)"));
}

#[test]
fn library_list_search_show_reads_promoted_manifests() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "gallery provenance search target");

    let list = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["library", "list"])
        .output()
        .expect("library list");
    assert_success(&list);
    let list_stdout = stdout(&list);
    assert!(list_stdout.contains("RUN"));
    assert!(list_stdout.contains(&parent.run_id[..8]));
    assert!(list_stdout.contains("gallery provenance search target"));

    let search = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["library", "search", "provenance"])
        .output()
        .expect("library search");
    assert_success(&search);
    assert!(stdout(&search).contains(&parent.run_id[..8]));

    let show = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["library", "show", &parent.run_id])
        .output()
        .expect("library show");
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains("library artifact"));
    assert!(show_stdout.contains(&parent.run_id));
    assert!(show_stdout.contains("provenance:"));
    assert!(show_stdout.contains("gallery provenance search target"));
}

#[test]
fn library_defaults_to_current_scope_unless_all() {
    let temp = TempDir::new().expect("tempdir");
    let (paths, first) = completed_parent_at(&temp, "scope one artifact", "workspace-one");
    let (_, second) = completed_parent_at(&temp, "scope two artifact", "workspace-two");

    let scoped = deadreckon(&paths)
        .current_dir(&first.cwd)
        .args(["library", "list"])
        .output()
        .expect("library list scoped");
    assert_success(&scoped);
    let scoped_stdout = stdout(&scoped);
    assert!(scoped_stdout.contains(&first.run_id[..8]));
    assert!(!scoped_stdout.contains(&second.run_id[..8]));

    let all = deadreckon(&paths)
        .current_dir(&first.cwd)
        .args(["library", "list", "--all"])
        .output()
        .expect("library list all");
    assert_success(&all);
    let all_stdout = stdout(&all);
    assert!(all_stdout.contains(&first.run_id[..8]));
    assert!(all_stdout.contains(&second.run_id[..8]));
}

#[test]
fn library_list_filters_goal_and_dates() {
    let temp = TempDir::new().expect("tempdir");
    let (paths, first) = completed_parent_at(&temp, "old gallery artifact", "workspace-one");
    let (_, second) = completed_parent_at(&temp, "new gallery artifact", "workspace-two");
    let (_, third) = completed_parent_at(&temp, "future gallery artifact", "workspace-three");
    rewrite_manifest_promoted_at(&first, "2026-05-10T00:00:00Z");
    rewrite_manifest_promoted_at(&second, "2026-05-11T23:30:00Z");
    rewrite_manifest_promoted_at(&third, "2026-05-12T00:00:00Z");

    let goal_and_since = deadreckon(&paths)
        .args([
            "library",
            "list",
            "--all",
            "--goal",
            "new gallery",
            "--since",
            "2026-05-11",
        ])
        .output()
        .expect("library list filtered");
    assert_success(&goal_and_since);
    let goal_since_stdout = stdout(&goal_and_since);
    assert!(goal_since_stdout.contains(&second.run_id[..8]));
    assert!(!goal_since_stdout.contains(&first.run_id[..8]));

    let until = deadreckon(&paths)
        .args(["library", "list", "--all", "--until", "2026-05-11"])
        .output()
        .expect("library list until");
    assert_success(&until);
    let until_stdout = stdout(&until);
    assert!(until_stdout.contains(&first.run_id[..8]));
    assert!(until_stdout.contains(&second.run_id[..8]));
    assert!(!until_stdout.contains(&third.run_id[..8]));
}

#[test]
fn library_search_greps_promoted_run_docs() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "ordinary searchable parent");
    let docs_dir = parent.working_dir.join(".deadreckon/docs");
    fs::create_dir_all(&docs_dir).expect("docs dir");
    fs::write(
        docs_dir.join("RUN-NARRATIVE.md"),
        "This promoted artifact mentions calibrate-hyperdrive in docs only.",
    )
    .expect("doc");

    let search = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["library", "search", "calibrate-hyperdrive"])
        .output()
        .expect("library search docs");

    assert_success(&search);
    assert!(stdout(&search).contains(&parent.run_id[..8]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_reveals_parent_lineage() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "show lineage parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = extend_command(&paths, &parent, "lineage child")
        .output()
        .expect("extend");
    assert_success(&output);
    let child_id = extended_run_id(&output);

    let show = deadreckon(&paths)
        .arg("show")
        .arg(&child_id)
        .output()
        .expect("show");

    assert_success(&show);
    assert!(stdout(&show).contains(&format!("Extended from {}", parent.run_id)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_completion_prints_lifecycle_hints_and_no_hints_suppresses() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = deadreckon(&paths)
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("hinted run")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .output()
        .expect("run");
    assert_success(&output);
    let run_stdout = stdout(&output);
    assert!(run_stdout.contains("started run "));
    assert!(run_stdout.contains("attach  : deadreckon attach "));
    assert!(run_stdout.contains("export:"));
    assert!(run_stdout.contains("extend:"));
    let run_id = run_id_from_stdout(&output);
    assert!(run_stdout.contains(&format!("attach  : deadreckon attach {}", &run_id[..8])));
    let attach = deadreckon(&paths)
        .arg("attach")
        .arg(&run_id)
        .output()
        .expect("attach");
    assert_success(&attach);
    let attach_stdout = stdout(&attach);
    assert!(attach_stdout.contains("completed run"), "{attach_stdout}");
    assert!(attach_stdout.contains("export:"));
    assert!(attach_stdout.contains("extend:"));

    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = deadreckon(&paths)
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("quiet hinted run")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-hints")
        .arg("--no-docs")
        .output()
        .expect("run no hints");
    assert_success(&output);
    assert!(!stdout(&output).contains("export:"));
    assert!(!stdout(&output).contains("extend:"));
    let run_id = run_id_from_stdout(&output);
    let attach = deadreckon(&paths)
        .arg("attach")
        .arg(&run_id)
        .arg("--no-hints")
        .output()
        .expect("attach no hints");
    assert_success(&attach);
    assert!(!stdout(&attach).contains("export:"));
    assert!(!stdout(&attach).contains("extend:"));

    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = deadreckon(&paths)
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("env quiet hinted run")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HINTS", "0")
        .output()
        .expect("run env no hints");
    assert_success(&output);
    assert!(!stdout(&output).contains("export:"));
    assert!(!stdout(&output).contains("extend:"));
    let run_id = run_id_from_stdout(&output);
    let attach = deadreckon(&paths)
        .arg("attach")
        .arg(&run_id)
        .env("DEADRECKON_HINTS", "0")
        .output()
        .expect("attach env no hints");
    assert_success(&attach);
    assert!(!stdout(&attach).contains("export:"));
    assert!(!stdout(&attach).contains("extend:"));
}

#[test]
fn help_lists_lifecycle_verbs() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths).arg("--help").output().expect("help");
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Production flow:"));
    assert!(stdout.contains("def-done"));
    assert!(stdout.contains("start"));
    assert!(stdout.contains("attach latest"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("finish latest"));
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("kill"));
    assert!(stdout.contains("resume"));
    assert!(stdout.contains("cleanup"));
    assert!(stdout.contains("help-all"));
    assert!(!stdout.contains("orchestrate    "));
    assert!(!stdout.contains("completion    "));
    assert!(!stdout.contains("acceptance"));
    assert!(stdout.contains(
        "Run, chain, and plan ids accept unique prefixes where that command accepts the kind"
    ));
}

#[test]
fn help_groups_verbs_by_lifecycle_stage() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths).arg("--help").output().expect("help");
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Start, watch, keep:"));
    assert!(stdout.contains("Setup and health:"));
    assert!(stdout.contains("Control:"));
    assert!(stdout.contains("Find more:"));
    assert!(!stdout.contains("Inspect And Import"));
}

#[test]
fn every_top_level_help_shows_lifecycle_usage() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    for command in [
        "init",
        "config",
        "completion",
        "help-all",
        "def-done",
        "acceptance",
        "run",
        "orchestrate",
        "plan",
        "fork",
        "merge",
        "chain",
        "doctor",
        "detect",
        "providers",
        "list",
        "library",
        "finish",
        "materialize",
        "apply",
        "abandon",
        "cleanup",
        "extend",
        "doc",
        "attach",
        "kill",
        "resume",
        "undo",
        "show",
        "status",
        "import",
    ] {
        let output = deadreckon(&paths)
            .arg(command)
            .arg("--help")
            .output()
            .expect("command help");
        assert_success(&output);
        let stdout = stdout(&output);
        assert!(
            stdout.contains("Lifecycle:"),
            "{command} help did not include lifecycle guidance:\n{stdout}"
        );
    }
}

#[test]
fn completion_scripts_cover_commands_flags_and_advanced_verbs() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .args(["completion", "zsh"])
        .output()
        .expect("completion");
    assert_success(&output);
    let zsh = stdout(&output);
    assert!(zsh.contains("#compdef deadreckon"), "{zsh}");
    assert!(zsh.contains("run:"), "{zsh}");
    assert!(zsh.contains("def-done:"), "{zsh}");
    assert!(zsh.contains("--provider"), "{zsh}");
    assert!(zsh.contains("completion:"), "{zsh}");
    assert!(
        zsh.contains("acceptance:") && zsh.contains("materialize:"),
        "{zsh}"
    );

    let alias = deadreckon(&paths)
        .args(["completions", "fish"])
        .output()
        .expect("completion alias");
    assert_success(&alias);
    assert!(stdout(&alias).contains("complete -c deadreckon"));
}

#[test]
fn completion_install_detects_zsh_writes_script_and_managed_rc_block() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .env("HOME", temp.path())
        .env("SHELL", "/bin/zsh")
        .args(["completion", "install"])
        .output()
        .expect("completion install");
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("installed"), "{stdout}");
    assert!(stdout.contains("_deadreckon"), "{stdout}");

    let script = fs::read_to_string(temp.path().join(".zsh/completions/_deadreckon"))
        .expect("completion script");
    assert!(script.contains("#compdef deadreckon"), "{script}");
    assert!(script.contains("run:"), "{script}");
    assert!(script.contains("--provider"), "{script}");

    let zshrc = fs::read_to_string(temp.path().join(".zshrc")).expect("zshrc");
    assert!(zshrc.contains("# >>> deadreckon completion >>>"), "{zshrc}");
    assert!(zshrc.contains(".zsh/completions"), "{zshrc}");
}

#[test]
fn init_installs_shell_completion_by_default() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .env("HOME", temp.path())
        .env("SHELL", "/bin/zsh")
        .args([
            "init",
            "--provider",
            "cli:codex",
            "--sandbox",
            "none",
            "--no-confirm",
        ])
        .output()
        .expect("init");
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("completion"), "{stdout}");
    assert!(temp.path().join(".zsh/completions/_deadreckon").exists());
    assert!(temp.path().join(".zshrc").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn done_plain_english_uses_configured_provider() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("done-app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("README.md"), "done app").expect("readme");
    let response = json!({
        "acceptance_yaml": "name: done\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        "acceptance_md": "# Done Criteria\n\nREADME must exist."
    })
    .to_string();
    let server = MockServer::start(vec![FixtureResponse {
        content: response,
        prompt_tokens: 20,
        completion_tokens: 20,
    }])
    .await;
    write_config(paths.home(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["def-done", "README exists", "--provider", "mock"])
        .output()
        .expect("def-done");

    assert_success(&output);
    assert!(stdout(&output).contains("done criteria configured"));
    assert!(workspace.join(".deadreckon/acceptance.yaml").exists());
}

#[test]
fn done_check_and_show_are_user_facing() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("done-check-app");
    fs::create_dir_all(workspace.join(".deadreckon")).expect("workspace");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: check\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n",
    )
    .expect("yaml");

    let check = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["def-done", "check"])
        .output()
        .expect("def-done check");
    assert_success(&check);
    assert!(stdout(&check).contains("done criteria passed"));

    let show = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["def-done", "show"])
        .output()
        .expect("def-done show");
    assert_success(&show);
    assert!(stdout(&show).contains("done criteria"));
}

#[test]
fn done_is_not_kept_as_a_compatibility_alias() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .arg("done")
        .arg("--help")
        .output()
        .expect("old done command");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unrecognized subcommand"));
}

#[test]
fn chain_help_lists_real_subcommands() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths)
        .args(["chain", "help"])
        .output()
        .expect("chain help");

    assert_success(&output);
    let chain_stdout = stdout(&output);
    assert!(chain_stdout.contains("deadreckon chain plan"));
    assert!(chain_stdout.contains("deadreckon chain redo latest --step 2"));
    assert!(chain_stdout.contains("deadreckon chain hooks list"));

    let redo = deadreckon(&paths)
        .args(["chain", "help", "redo"])
        .output()
        .expect("chain help redo");
    assert_success(&redo);
    assert!(stdout(&redo).contains("deadreckon chain undo/redo"));
}

#[test]
fn acceptance_init_writes_project_spec() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("node-app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(
        workspace.join("package.json"),
        r#"{"scripts":{"build":"node -e \"process.exit(0)\""}}"#,
    )
    .expect("package");

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["acceptance", "init", "--preset", "node"])
        .output()
        .expect("acceptance init");

    assert_success(&output);
    let yaml = fs::read_to_string(workspace.join(".deadreckon/acceptance.yaml")).expect("yaml");
    assert!(yaml.contains("npm run build --if-present"));
    assert!(workspace.join(".deadreckon/acceptance.md").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_draft_uses_configured_provider() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("draft-app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("README.md"), "draft app").expect("readme");
    let response = json!({
        "acceptance_yaml": "name: drafted\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        "acceptance_md": "# Acceptance\n\nREADME must exist."
    })
    .to_string();
    let server = MockServer::start(vec![FixtureResponse {
        content: response,
        prompt_tokens: 20,
        completion_tokens: 20,
    }])
    .await;
    write_config(paths.home(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "acceptance",
            "draft",
            "require README",
            "--provider",
            "mock",
        ])
        .output()
        .expect("acceptance draft");

    assert_success(&output);
    let yaml = fs::read_to_string(workspace.join(".deadreckon/acceptance.yaml")).expect("yaml");
    assert!(yaml.contains("README.md"));
    assert!(stdout(&output).contains("agent draft via mock / mock-agent"));
}

#[test]
fn acceptance_check_dry_runs_project_spec() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("checked-app");
    fs::create_dir_all(workspace.join(".deadreckon")).expect("workspace");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: check\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n",
    )
    .expect("yaml");

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["acceptance", "check"])
        .output()
        .expect("acceptance check");

    assert_success(&output);
    assert!(stdout(&output).contains("done criteria passed"));
}

#[test]
fn acceptance_add_browser_pack_writes_helper_and_yaml() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("browser-app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("index.html"), "<html><body>ok</body></html>").expect("html");

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["acceptance", "add", "browser"])
        .output()
        .expect("acceptance add");

    assert_success(&output);
    let yaml = fs::read_to_string(workspace.join(".deadreckon/acceptance.yaml")).expect("yaml");
    assert!(yaml.contains("browser-smoke.mjs"));
    assert!(
        workspace
            .join(".deadreckon/acceptance/browser-smoke.mjs")
            .exists()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_add_plain_english_uses_provider_files() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("english-app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("README.md"), "gallery").expect("readme");
    let response = json!({
        "acceptance_yaml": "name: english\nchecks:\n  - kind: shell\n    command: \"node .deadreckon/acceptance/gallery-check.mjs\"\n    cwd: \"{working_dir}\"\n",
        "acceptance_md": "# Acceptance\n\nUsers can add and browse artwork.",
        "files": {
            ".deadreckon/acceptance/gallery-check.mjs": "console.log('gallery ok')"
        }
    })
    .to_string();
    let server = MockServer::start(vec![FixtureResponse {
        content: response,
        prompt_tokens: 20,
        completion_tokens: 20,
    }])
    .await;
    write_config(paths.home(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "acceptance",
            "add",
            "users can add artwork and browse the gallery",
            "--provider",
            "mock",
        ])
        .output()
        .expect("acceptance add english");

    assert_success(&output);
    assert!(
        fs::read_to_string(workspace.join(".deadreckon/acceptance.md"))
            .expect("md")
            .contains("Users can add")
    );
    assert!(
        workspace
            .join(".deadreckon/acceptance/gallery-check.mjs")
            .exists()
    );
}

#[test]
fn acceptance_check_reports_shell_failure_output() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("failed-acceptance");
    fs::create_dir_all(workspace.join(".deadreckon")).expect("workspace");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: failing\nchecks:\n  - kind: shell\n    command: \"echo helpful failure >&2; exit 9\"\n    cwd: \"{working_dir}\"\n",
    )
    .expect("yaml");

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args(["acceptance", "check"])
        .output()
        .expect("acceptance check");

    assert!(!output.status.success());
    assert!(stdout(&output).contains("done criteria failed"));
    assert!(stdout(&output).contains("helpful failure"));
}

#[test]
fn run_copies_project_acceptance_spec_into_run_root() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("run-acceptance");
    fs::create_dir_all(workspace.join(".deadreckon/acceptance")).expect("workspace");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: run acceptance\nchecks:\n  - kind: shell\n    command: \"sh .deadreckon/acceptance/check.sh\"\n    cwd: \"{working_dir}\"\n",
    )
    .expect("yaml");
    fs::write(
        workspace.join(".deadreckon/acceptance/check.sh"),
        "test -d .\n",
    )
    .expect("helper");

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "run",
            "create hello.txt containing exactly hello",
            "--fresh",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--no-hints",
            "--no-docs",
        ])
        .output()
        .expect("run");

    assert_success(&output);
    let run_id = run_id_from_stdout(&output);
    let state = load_run(&paths, &run_id).expect("state");
    assert!(state.run_root.join("acceptance.yaml").exists());
    assert!(
        state
            .working_dir
            .join(".deadreckon/acceptance/check.sh")
            .exists()
    );
    let marker =
        fs::read_to_string(state.run_root.join("proofs/turn-acceptance.json")).expect("marker");
    assert!(marker.contains("\"kind\": \"shell\""));
}

#[test]
fn finish_exports_completed_fresh_run() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "finish export parent");
    let dest = temp.path().join("finished-export");

    let output = deadreckon(&paths)
        .arg("finish")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("finish");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("finish:"));
    assert!(stdout.contains("exported run"));
    assert_eq!(
        fs::read_to_string(dest.join("app.txt")).expect("app"),
        "parent app"
    );
}

#[test]
fn status_includes_library_count_and_disk_usage() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "status library disk");

    let status = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["status", &parent.run_id])
        .output()
        .expect("status");

    assert_success(&status);
    let stdout = stdout(&status);
    assert!(stdout.contains("run health"));
    assert!(stdout.contains("library"));
    assert!(stdout.contains("scope artifacts: 1"));
    assert!(stdout.contains("disk"));
    assert!(stdout.contains("MB free"));
}

#[test]
fn status_tip_line_appears_when_disk_over_threshold() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "status disk tip");

    let status = deadreckon(&paths)
        .current_dir(&parent.cwd)
        .args(["status", &parent.run_id])
        .output()
        .expect("status");

    assert_success(&status);
    let stdout = stdout(&status);
    assert!(stdout.contains("tip:"), "{stdout}");
    assert!(
        stdout.contains("deadreckon cleanup --completed"),
        "{stdout}"
    );
}

fn completed_parent(temp: &TempDir, goal: &str) -> (DeadreckonPaths, PipelineState) {
    completed_parent_at(temp, goal, "workspace")
}

fn completed_parent_at(
    temp: &TempDir,
    goal: &str,
    workspace_name: &str,
) -> (DeadreckonPaths, PipelineState) {
    let home = temp.path().join("home");
    let paths = DeadreckonPaths::from_home(&home);
    let cwd = temp.path().join(workspace_name);
    fs::create_dir_all(&cwd).expect("workspace");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(30.0),
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    fs::write(state.working_dir.join("app.txt"), "parent app").expect("app");
    fs::write(state.working_dir.join("notes.md"), "parent notes").expect("notes");
    append_trace(
        &state,
        &TraceRecord {
            timestamp: chrono::Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "tool.write_file".to_string(),
            latency_ms: None,
            detail: json!({"tool_call_id": "parent-tool-1", "path": "app.txt"}),
        },
    )
    .expect("trace");
    state.turn = 2;
    state
        .set_phase_status(PhaseId(60), PhaseStatus::Completed)
        .expect("complete");
    save_state(&state).expect("save");
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        state.turn as usize,
    )
    .expect("acceptance marker");
    promote_completed_run(&paths, &mut state).expect("promote");
    let state = load_run(&paths, &state.run_id).expect("reload");
    assert_eq!(state.status, RunStatus::Completed);
    (paths, state)
}

fn rewrite_manifest_promoted_at(state: &PipelineState, promoted_at: &str) {
    let path = state.working_dir.join("manifest.json");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("manifest json");
    value["promoted_at"] = Value::String(promoted_at.to_string());
    fs::write(
        &path,
        serde_json::to_vec_pretty(&value).expect("manifest bytes"),
    )
    .expect("write manifest");
}

fn parent_json(dest: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(dest.join(".deadreckon/parent.json")).expect("parent marker"))
        .expect("parent json")
}

fn repo_tempdir() -> TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn clean_git_repo(temp: &TempDir) -> std::path::PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
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

fn git_ref_exists(cwd: &std::path::Path, reference: &str) -> bool {
    Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn extend_command(paths: &DeadreckonPaths, parent: &PipelineState, goal: &str) -> Command {
    let mut command = deadreckon(paths);
    command
        .arg("extend")
        .arg(&parent.run_id)
        .arg(goal)
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs");
    command
}

fn write_config(home: &std::path::Path, base_url: &str) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["mock"]

[providers.mock]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
"#
        ),
    )
    .expect("config");
}

fn extend_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"write_file\",\"tool_call_id\":\"extend-write\",\"path\":\"child.txt\",\"content\":\"extended child\"}",
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": implementation_notes_write_action(),
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"done\",\"summary\":\"extended complete\"}",
            "prompt_tokens": 160,
            "completion_tokens": 40
        }
    ]))
    .expect("script")
}

fn implementation_notes_write_action() -> String {
    json!({
        "action": "write_file",
        "tool_call_id": "extend-notes",
        "path": "implementation-notes.html",
        "content": r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>Implementation Notes</title></head>
<body>
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2>
<ul><li>Extended runs write child artifacts and keep the parent artifacts intact.</li></ul></section>
<section id="deviations"><h2>Deviations</h2>
<ul><li>None.</li></ul></section>
<section id="tradeoffs"><h2>Tradeoffs</h2>
<ul><li>The fixture uses a separate notes write so tests exercise the freshness gate.</li></ul></section>
<section id="open-questions"><h2>Open questions</h2>
<ul><li>None.</li></ul></section>
</body>
</html>
"#
    })
    .to_string()
}

fn extended_run_id(output: &std::process::Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("completed extended run "))
        .expect("extended run id")
        .to_string()
}

fn run_id_from_stdout(output: &std::process::Output) -> String {
    let stdout = stdout(output);
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("completed run ")
                .map(str::to_string)
                .or_else(|| {
                    line.strip_prefix("started run ")
                        .and_then(|rest| rest.rsplit_once('('))
                        .map(|(_, id)| id.trim_end_matches(')').to_string())
                })
        })
        .expect("run id")
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

#[derive(Clone)]
struct MockState {
    fixtures: Arc<Mutex<Vec<FixtureResponse>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureResponse {
    content: String,
    prompt_tokens: u64,
    completion_tokens: u64,
}

struct MockServer {
    addr: SocketAddr,
}

impl MockServer {
    async fn start(fixtures: Vec<FixtureResponse>) -> Self {
        let state = MockState {
            fixtures: Arc::new(Mutex::new(fixtures)),
        };
        let app = Router::new()
            .route("/chat/completions", post(chat_completions))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

async fn chat_completions(
    State(state): State<MockState>,
    Json(_request): Json<Value>,
) -> impl IntoResponse {
    let fixture = {
        let mut fixtures = state.fixtures.lock().expect("fixtures");
        if fixtures.is_empty() {
            None
        } else {
            Some(fixtures.remove(0))
        }
    };
    let Some(fixture) = fixture else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": {"message": "no fixture response left"}})),
        );
    };
    (
        StatusCode::OK,
        Json(json!({
            "id": "mock",
            "object": "chat.completion",
            "model": "mock-agent",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": fixture.content},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": fixture.prompt_tokens,
                "completion_tokens": fixture.completion_tokens,
                "total_tokens": fixture.prompt_tokens + fixture.completion_tokens
            }
        })),
    )
}
