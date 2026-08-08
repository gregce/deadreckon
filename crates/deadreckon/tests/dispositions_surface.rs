#![allow(clippy::expect_used)]

//! Dispositions + doctor hygiene — the DISPOSITIONS machine surfaces
//! (FULL-DRIVE gaps A11 undo, A12 rewind refusals, A14 cleanup, B4 doctor).
//!
//! `undo --json` and `cleanup --json` are G1 envelopes over the same
//! `VerdictSurface` facts the prose renders (the promote-success
//! `next_actions` advertise `deadreckon undo --run <id>`, so that exact
//! spelling must produce the envelope); `rewind` refusals — previously prose
//! even under `--json` — are typed `{kind:"error"}` envelopes; and
//! `doctor --repair --json` (previously a clap conflict) reports findings
//! plus one `{attempted, result}` row per bounded repair.

use std::fs;
use std::process::{Command, Output};

use deadreckon_core::{
    CodebaseMode, CodebaseRecord, DeadreckonPaths, PipelineState, RunOptions, create_run,
    save_state, snapshot_working,
};
use serde_json::Value;
use tempfile::TempDir;

mod common;

use common::{SupervisorServiceFixture, assert_success, deadreckon, repo_tempdir, stderr, stdout};

fn parse_stdout(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "expected one JSON object on stdout ({error})\nstdout:\n{}\nstderr:\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

fn assert_error_envelope(output: &Output, verb: &str) -> Value {
    assert!(!output.status.success(), "{}", stdout(output));
    let value = parse_stdout(output);
    assert_eq!(value["kind"], "error", "{value}");
    assert_eq!(value["verb"], verb, "{value}");
    assert_eq!(
        value["code"],
        i64::from(output.status.code().expect("exit code")),
        "{value}"
    );
    assert!(
        !value["message"].as_str().unwrap_or_default().is_empty(),
        "{value}"
    );
    value
}

fn snapshot_run(paths: &DeadreckonPaths, temp: &TempDir, goal: &str) -> PipelineState {
    let cwd = temp.path().join(format!("workspace-{goal}"));
    fs::create_dir_all(&cwd).expect("workspace");
    let mut state = create_run(
        paths,
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
    fs::write(state.working_dir.join("app.txt"), "before").expect("before");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(state.working_dir.join("app.txt"), "current").expect("current");
    state.turn = 1;
    save_state(&state).expect("save");
    state
}

#[test]
fn undo_run_snapshot_json_is_the_g1_envelope_and_prose_stays_pinned() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = snapshot_run(&paths, &temp, "undo-envelope");

    // The exact advertised spelling from the promote-success next_actions:
    // `deadreckon undo --run <id>`, plus `--json`.
    let undo = deadreckon(&paths)
        .args(["undo", "--run", &state.run_id, "--turn", "0", "--json"])
        .output()
        .expect("undo --json");
    assert_success(&undo);
    let value = parse_stdout(&undo);
    assert_eq!(value["kind"], "undo", "{value}");
    assert_eq!(value["id"], state.run_id.as_str(), "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    assert_eq!(value["undo_kind"], "run-snapshot", "{value}");
    assert_eq!(value["restored_turn"], 0, "{value}");
    assert_eq!(
        value["workspace"],
        state.working_dir.display().to_string().as_str(),
        "{value}"
    );
    assert!(
        value["snapshot"]
            .as_str()
            .unwrap_or_default()
            .ends_with("turn-0"),
        "{value}"
    );
    assert_eq!(
        value["next_actions"][0],
        format!("deadreckon show {}", &state.run_id[..8]).as_str(),
        "{value}"
    );
    assert_eq!(value["try_lines"], serde_json::json!([]), "{value}");
    assert_eq!(value["verdict"]["kind"], "completed", "{value}");
    assert_eq!(
        fs::read_to_string(state.working_dir.join("app.txt")).expect("app"),
        "before"
    );

    // Prose without --json is byte-compatible with the pre-envelope surface.
    fs::write(state.working_dir.join("app.txt"), "current").expect("current again");
    let prose = deadreckon(&paths)
        .args(["undo", "--run", &state.run_id, "--turn", "0"])
        .output()
        .expect("undo prose");
    assert_success(&prose);
    let prose_stdout = stdout(&prose);
    assert!(
        prose_stdout.starts_with("completed undo "),
        "{prose_stdout}"
    );
    assert!(
        !prose_stdout.trim_start().starts_with('{'),
        "{prose_stdout}"
    );
}

#[test]
fn undo_refusal_is_a_typed_error_envelope() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .args(["undo", "--run", "deadbeef", "--json"])
        .output()
        .expect("undo refusal");

    assert_error_envelope(&output, "undo");
}

#[test]
fn rewind_refusals_are_typed_error_envelopes_and_prose_refusals_stay_prose() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = snapshot_run(&paths, &temp, "rewind-refusal");

    // No target selector at all: the argument-shape refusal.
    let no_target = deadreckon(&paths)
        .args(["rewind", &state.run_id, "--json"])
        .output()
        .expect("rewind without target");
    let value = assert_error_envelope(&no_target, "rewind");
    assert!(
        value["message"]
            .as_str()
            .unwrap_or_default()
            .contains("choose exactly one"),
        "{value}"
    );

    // A run without provider checkpoints: the not-found refusal.
    let no_checkpoints = deadreckon(&paths)
        .args(["rewind", &state.run_id, "--to-turn", "1", "--json"])
        .output()
        .expect("rewind without checkpoints");
    let value = assert_error_envelope(&no_checkpoints, "rewind");
    assert!(
        value["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no provider checkpoints"),
        "{value}"
    );

    // Without --json the refusal never becomes JSON: stdout stays empty and
    // the prose lands on stderr exactly as before.
    let prose = deadreckon(&paths)
        .args(["rewind", &state.run_id, "--to-turn", "1"])
        .output()
        .expect("rewind prose refusal");
    assert!(!prose.status.success());
    assert!(stdout(&prose).is_empty(), "{}", stdout(&prose));
    assert!(
        stderr(&prose).contains("no provider checkpoints"),
        "{}",
        stderr(&prose)
    );
}

#[test]
fn cleanup_json_envelopes_cover_noop_targeted_and_aggregate_results() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    // No candidates: an honest no-op envelope, not a refusal.
    let noop = deadreckon(&paths)
        .args(["cleanup", "--json"])
        .output()
        .expect("cleanup --json without candidates");
    assert_success(&noop);
    let value = parse_stdout(&noop);
    assert_eq!(value["kind"], "cleanup", "{value}");
    assert_eq!(value["id"], "no-candidates", "{value}");
    assert_eq!(value["status"], "no-op", "{value}");
    assert_eq!(value["runs"], serde_json::json!([]), "{value}");
    assert_eq!(value["completed_filter"], false, "{value}");
    assert_eq!(value["all_scopes"], false, "{value}");

    let repo = clean_git_repo(&temp);
    let first = abandoned_worktree_run(&paths, &repo, "dr-cleanup-one", "cleanup one");
    let second = abandoned_worktree_run(&paths, &repo, "dr-cleanup-two", "cleanup two");
    let third = abandoned_worktree_run(&paths, &repo, "dr-cleanup-three", "cleanup three");

    // Non-interactive cleanup with candidates still requires --no-confirm;
    // under --json that refusal is the typed error envelope.
    let refused = deadreckon(&paths)
        .current_dir(&repo)
        .args(["cleanup", "--json"])
        .output()
        .expect("cleanup refusal");
    let value = assert_error_envelope(&refused, "cleanup");
    assert!(
        value["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--no-confirm"),
        "{value}"
    );

    // Targeted single-run cleanup: the per-run envelope. The codebase record
    // written into the worktree makes it non-empty, so cleanup needs the same
    // explicit force that abandon's `--anyway` carries.
    let targeted = deadreckon(&paths)
        .current_dir(&repo)
        .args(["cleanup", &first.state.run_id, "--overwrite", "--json"])
        .output()
        .expect("targeted cleanup --json");
    assert_success(&targeted);
    let value = parse_stdout(&targeted);
    assert_eq!(value["kind"], "cleanup", "{value}");
    assert_eq!(value["id"], first.state.run_id.as_str(), "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    assert!(
        !value["removed"].as_array().expect("removed").is_empty(),
        "{value}"
    );
    assert_eq!(value["worktree_found"], true, "{value}");
    assert!(!first.worktree.exists(), "worktree must be removed");

    // Aggregate cleanup over the remaining candidates: the multi-run envelope.
    let aggregate = deadreckon(&paths)
        .current_dir(&repo)
        .args(["cleanup", "--no-confirm", "--overwrite", "--json"])
        .output()
        .expect("aggregate cleanup --json");
    assert_success(&aggregate);
    let value = parse_stdout(&aggregate);
    assert_eq!(value["kind"], "cleanup", "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    let runs = value["runs"].as_array().expect("runs");
    assert!(runs.len() >= 2, "{value}");
    for run in [&second, &third] {
        assert!(
            runs.iter()
                .any(|row| row["run_id"] == run.state.run_id.as_str()),
            "{value}"
        );
    }
    assert!(
        value["removed_total"].as_u64().expect("removed_total") >= 2,
        "{value}"
    );
    assert!(!second.worktree.exists(), "worktree must be removed");
    assert!(!third.worktree.exists(), "worktree must be removed");
}

#[test]
fn cleanup_refusal_for_an_unknown_run_is_a_typed_error_envelope() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .args(["cleanup", "deadbeef", "--json"])
        .output()
        .expect("cleanup unknown run");

    assert_error_envelope(&output, "cleanup");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn doctor_repair_json_reports_findings_and_per_repair_results() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);

    // A fresh home has a repairable finding: the install receipt is missing.
    let before = service
        .deadreckon()
        .args(["doctor", "--json"])
        .output()
        .expect("doctor --json");
    assert_success(&before);
    let value = parse_stdout(&before);
    assert!(value.get("repairs").is_none(), "{value}");
    assert!(
        value["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("install receipt is missing")),
        "{value}"
    );

    // The previously clap-refused combination: findings plus one
    // {attempted, result} row per bounded repair.
    let repaired = service
        .deadreckon()
        .args(["doctor", "--repair", "--json"])
        .output()
        .expect("doctor --repair --json");
    assert_success(&repaired);
    let value = parse_stdout(&repaired);
    let repairs = value["repairs"].as_array().expect("repairs");
    assert_eq!(repairs.len(), 3, "{value}");
    for repair in repairs {
        assert!(
            repair["attempted"]
                .as_str()
                .unwrap_or_default()
                .starts_with("repair "),
            "{value}"
        );
        assert!(
            matches!(
                repair["result"].as_str().unwrap_or_default(),
                "passed" | "warning" | "failed"
            ),
            "{value}"
        );
    }
    assert!(
        repairs
            .iter()
            .any(|repair| repair["attempted"] == "repair install receipt"
                && repair["result"] == "passed"),
        "{value}"
    );
    // The repair rows also land in the findings list the way prose reports
    // them, so decoders see one consistent story.
    assert!(
        value["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["subject"] == "repair install receipt"),
        "{value}"
    );
}

struct WorktreeRun {
    state: PipelineState,
    worktree: std::path::PathBuf,
}

fn abandoned_worktree_run(
    paths: &DeadreckonPaths,
    repo: &std::path::Path,
    branch: &str,
    goal: &str,
) -> WorktreeRun {
    let worktree = repo
        .parent()
        .expect("repo parent")
        .join(format!("{branch}-worktree"));
    git(
        repo,
        &["worktree", "add", "-b", branch, path_str(&worktree)],
    );
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Worktree;
    record.source_git_root = Some(repo.to_path_buf());
    record.worktree_path = Some(worktree.clone());
    record.branch_name = Some(branch.to_string());
    let state = create_run(
        paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: repo.to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(30.0),
            run_id: None,
            codebase: Some(record),
        },
    )
    .expect("run");
    fs::write(state.run_root.join("abandoned.json"), "{}").expect("abandoned marker");
    WorktreeRun { state, worktree }
}

fn clean_git_repo(temp: &TempDir) -> std::path::PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "--initial-branch=main"]);
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "initial"]);
    repo
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
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
}

fn path_str(path: &std::path::Path) -> &str {
    path.to_str().expect("utf8 path")
}
