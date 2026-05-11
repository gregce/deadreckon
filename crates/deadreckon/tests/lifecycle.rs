use std::fs;
use std::process::Command;

use deadreckon_core::{
    DeadreckonPaths, PhaseId, PhaseStatus, PipelineState, RunOptions, RunStatus, create_run,
    load_run, promote_completed_run, save_state, write_acceptance_marker,
};
use serde_json::Value;
use tempfile::TempDir;

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
    assert!(stderr(&output).contains("refusing to materialize back into runstate"));
}

fn completed_parent(temp: &TempDir, goal: &str) -> (DeadreckonPaths, PipelineState) {
    let home = temp.path().join("home");
    let paths = DeadreckonPaths::from_home(&home);
    let cwd = temp.path().join("workspace");
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
        },
    )
    .expect("run");
    fs::write(state.working_dir.join("app.txt"), "parent app").expect("app");
    fs::write(state.working_dir.join("notes.md"), "parent notes").expect("notes");
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

fn parent_json(dest: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(dest.join(".deadreckon/parent.json")).expect("parent marker"))
        .expect("parent json")
}

fn repo_tempdir() -> TempDir {
    let root = std::path::Path::new("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
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
