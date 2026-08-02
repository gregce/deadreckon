#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_core::glossary::NOUN_DONE_CONTRACT;
use deadreckon_core::lock::lock_path;
use deadreckon_core::{
    CHAIN_EVENTS_JSONL, CoordinatorState, DeadreckonPaths, JobView, PLAN_EVENTS_JSONL, Plan,
    PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind, PlanMode, PlanRole, PlanStatus,
    PlanTaskStatus, RUN_EVENTS_JSONL, RunOptions, RunStatus, append_plan_event,
    append_plan_message, append_trace, create_run, list_runs, load_job_lease, load_plan, load_run,
    pid_is_alive, read_codebase_record, read_job_history, read_plan_events, read_plan_messages,
    save_plan, save_state,
};
use deadreckon_protocol::{JobEventKind, JobId, JobOutcome, TraceRecord};
use serde_json::{Value, json};
use tempfile::TempDir;

mod common;

use common::{
    SupervisorServiceFixture, assert_success, deadreckon as deadreckon_binary, repo_tempdir,
    stderr, stdout,
};

/// This suite characterizes the inner Plan conductor and merge machinery.
/// Public `fork` and `orchestrate` now queue durable Jobs; debug test binaries
/// retain this separate route so the inner executor can still be exercised
/// without turning every conductor assertion into a supervisor integration.
fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon-characterization"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn characterization_service(paths: &DeadreckonPaths) -> SupervisorServiceFixture {
    SupervisorServiceFixture::configured_for_binary(
        paths,
        std::path::Path::new(env!("CARGO_BIN_EXE_deadreckon-characterization")),
    )
}

fn assert_verdict_surface(text: &str, verdict: &str, recommended: &str) {
    assert!(text.contains(verdict), "{text}");
    assert!(text.contains("Explanation\n"), "{text}");
    assert!(text.contains("Evidence\n"), "{text}");
    assert_eq!(text.matches("\nRecommended\n").count(), 1, "{text}");
    assert!(
        text.contains(&format!("Recommended\n{recommended}")),
        "{text}"
    );
    assert!(!text.contains("try:"), "{text}");
}

#[test]
fn plan_writes_plan_json_with_n_tasks() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust in two files",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "3",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.mode, PlanMode::FullPlan);
    assert_eq!(plan.tasks.len(), 3);
    assert_eq!(plan.providers.planner.as_deref(), Some("smoke"));
    assert_eq!(plan.providers.default_child.as_deref(), Some("smoke"));
}

#[test]
fn plan_completed_surface_uses_lifecycle_primary_action_json() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "json verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
            "--json",
        ])
        .output()
        .expect("plan json");

    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["kind"], "plan");
    assert_eq!(value["verdict"]["kind"], "preview");
    assert_eq!(
        value["primary_action"],
        value["verdict"]["recommended_command"]
    );
    assert_eq!(value["primary_action"], value["next_actions"][0]);
    assert_eq!(
        value["primary_action"],
        format!(
            "deadreckon fork {}",
            &value["id"].as_str().expect("id")[..8]
        )
    );
    assert!(
        value["verdict"]["explanation"]
            .as_str()
            .expect("explanation")
            .contains("plan graph")
    );
}

#[test]
fn plan_failed_surface_recommends_why_failed_or_merge_once_json() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "failed json verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Failed;
    save_plan(&paths, &plan).expect("save failed plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--json"])
        .output()
        .expect("show plan json");
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["verdict"]["kind"], "failed");
    assert_eq!(
        value["primary_action"],
        value["verdict"]["recommended_command"]
    );
    assert_eq!(value["primary_action"], value["next_actions"][0]);
    assert_eq!(
        value["primary_action"],
        format!("deadreckon show {} --why-failed", &plan.plan_id[..8])
    );
    assert_eq!(
        value["next_actions"]
            .as_array()
            .expect("next actions")
            .iter()
            .filter(|action| action.as_str() == value["primary_action"].as_str())
            .count(),
        1
    );
}

#[test]
fn finish_failed_plan_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "failed finish verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Failed;
    save_plan(&paths, &plan).expect("save failed plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["finish", &plan.plan_id[..8], "--no-confirm"])
        .output()
        .expect("finish failed plan");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.starts_with("failed plan "), "{err}");
    assert!(err.contains("no completed result to finish"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon show {} --why-failed",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn finish_pending_plan_recommends_fork_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "pending finish verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.status, PlanStatus::Pending);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["finish", &plan.plan_id[..8], "--no-confirm"])
        .output()
        .expect("finish pending plan");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(err.contains("has not started yet"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon fork {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn finish_forked_plan_recommends_attach_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "forked finish verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[0].status = PlanTaskStatus::Completed;
    plan.tasks[1].status = PlanTaskStatus::Pending;
    save_plan(&paths, &plan).expect("save forked plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["finish", &plan.plan_id[..8], "--no-confirm"])
        .output()
        .expect("finish forked plan");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.starts_with("paused plan "), "{err}");
    assert!(err.contains("cannot finish it yet"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon attach {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn plan_writes_plan_created_event_when_saved() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::PlanCreated {
                mode: PlanMode::FullPlan,
                task_count: 2
            }
        )),
        "{events:#?}"
    );
}

#[test]
fn plan_writes_worker_specs_for_each_task() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    for task in &plan.tasks {
        let spec = fs::read_to_string(paths.worker_spec(&plan.plan_id, &task.task_id))
            .expect("worker spec");
        assert!(spec.contains("Root goal: tiny hello rust"), "{spec}");
        assert!(
            spec.contains("Treat this file as the complete brief"),
            "{spec}"
        );
        assert!(
            spec.contains("Do not inspect, tail, or summarize sibling child transcripts"),
            "{spec}"
        );
        assert!(spec.contains("Do not spawn subagents"), "{spec}");
        assert!(spec.contains(&task.task_id), "{spec}");
    }
}

#[test]
fn plan_review_mode_writes_coder_reviewer_plan_without_decomposition() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--mode",
            "review",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.mode, PlanMode::Review);
    assert_eq!(plan.tasks.len(), 2);
    assert_eq!(plan.tasks[0].role, PlanRole::Coder);
    assert_eq!(plan.tasks[0].provider.as_deref(), Some("smoke:coder"));
    assert_eq!(plan.tasks[1].role, PlanRole::Reviewer);
    assert_eq!(plan.tasks[1].provider.as_deref(), Some("smoke:reviewer"));
    assert_eq!(
        plan.tasks[1].depends_on,
        vec![plan.tasks[0].task_id.clone()]
    );
}

#[test]
fn plan_preview_prints_capabilities_and_provider_table() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "deploy websocket app",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let out = stdout(&output);
    let plan = newest_plan(&paths);
    assert_verdict_surface(
        &out,
        &format!("preview plan {}", &plan.plan_id[..8]),
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert!(out.contains("planner=smoke"), "{out}");
    assert!(out.contains("default-child=smoke"), "{out}");
    assert!(out.contains("capabilities :"), "{out}");
    assert!(out.contains("deploy=true"), "{out}");
    assert!(!out.contains("fork:"), "{out}");
}

#[test]
fn fork_completion_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--plain"])
        .output()
        .expect("fork");

    assert_success(&output);
    let out = stdout(&output);
    assert_verdict_surface(
        &out,
        &format!("completed plan {}", &plan.plan_id[..8]),
        &format!("deadreckon merge {}", &plan.plan_id[..8]),
    );
    assert!(!out.contains("attach:"), "{out}");
    assert!(!out.contains("merge:"), "{out}");
    assert!(out.contains("provider roles"), "{out}");
    assert!(out.contains("dependencies"), "{out}");
}

#[test]
fn merge_completion_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--plain"])
        .output()
        .expect("merge");

    assert_success(&output);
    let out = stdout(&output);
    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("result run");
    assert_verdict_surface(
        &out,
        &format!("completed plan {}", &plan.plan_id[..8]),
        &format!("deadreckon finish {}", &plan.plan_id[..8]),
    );
    assert!(out.contains(&merged_run_id[..8]), "{out}");
    assert!(out.contains("artifact library"), "{out}");
    assert!(!out.contains("finish:"), "{out}");
    assert!(!out.contains("apply:"), "{out}");
    assert!(!out.contains("export:"), "{out}");
    assert!(out.contains("provider roles"), "{out}");
    assert!(out.contains("dependencies"), "{out}");
}

#[test]
fn orchestrate_review_preview_shows_coder_reviewer_providers_without_forking() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "tiny hello rust",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("orchestrate preflight"), "{out}");
    assert!(out.contains("mode         : review"), "{out}");
    assert!(out.contains("coder smoke:coder"), "{out}");
    assert!(out.contains("reviewer smoke:reviewer"), "{out}");
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn orchestrate_full_plan_preview_shows_planner_child_providers_without_forking() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke:planner",
            "--provider",
            "smoke:child",
            "--child-provider",
            "1=smoke:reviewer",
            "--n",
            "2",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("orchestrate preflight"), "{out}");
    assert!(out.contains("mode         : full-plan"), "{out}");
    assert!(out.contains("planner smoke:planner"), "{out}");
    assert!(out.contains("default child smoke:child"), "{out}");
    assert!(out.contains("provider=smoke:reviewer"), "{out}");
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn run_and_orchestrate_preview_share_done_criteria_source_label() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::create_dir_all(repo.join(".deadreckon")).expect("deadreckon dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: preview criteria\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    git(&repo, &["add", "-A"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add done criteria"]).expect("commit acceptance");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = deadreckon(&paths)
        .current_dir(&repo)
        .args(["run", "preview done criteria", "--smoke", "--preview"])
        .output()
        .expect("run preview");
    assert_success(&run);
    let run_preview = stderr(&run);

    let orchestrate = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "preview done criteria",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");
    assert_success(&orchestrate);
    let orchestrate_preview = stdout(&orchestrate);

    let shared_label = "project (1 checks) from";
    assert!(run_preview.contains(NOUN_DONE_CONTRACT), "{run_preview}");
    assert!(run_preview.contains(shared_label), "{run_preview}");
    assert!(
        orchestrate_preview.contains(NOUN_DONE_CONTRACT),
        "{orchestrate_preview}"
    );
    assert!(
        orchestrate_preview.contains(shared_label),
        "{orchestrate_preview}"
    );
}

#[test]
fn start_preview_surface_has_one_future_watch_command() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["start", "preview guided start", "--preview", "--plain"])
        .output()
        .expect("start preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("preview start"), "{out}");
    assert!(out.contains("Explanation"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(!out.contains("<after-start>"), "{out}");
    assert_eq!(out.matches("deadreckon attach <id>").count(), 1, "{out}");
    assert_eq!(
        out.matches("deadreckon start \"preview guided start\" --mode run --yes")
            .count(),
        1,
        "{out}"
    );
    for field in ["path", "provider", "done", "workspace"] {
        assert!(out.contains(field), "missing {field}:\n{out}");
    }
    assert!(out.contains("run"), "{out}");
}

#[test]
fn start_preview_uses_provider_goal_shape_suggestion() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let goal = "rebuild billing, notifications, admin, and docs";
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: start ready\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    git(&repo, &["add", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    let provider_root = temp.path().join("provider");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:shape",
        r#"case "$1" in
  *"launch planner"*)
    printf '{"shape":"campaign","n":4,"confidence":0.9,"rationale":"four independent apps"}'
    ;;
  *)
    printf 'unexpected provider prompt\n' >&2
    exit 45
    ;;
esac
"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["start", goal, "--preview", "--plain"])
        .output()
        .expect("start preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("path"), "{out}");
    assert!(out.contains("campaign orchestration"), "{out}");
    assert!(out.contains("suggestion"), "{out}");
    assert!(out.contains("campaign n=4 via provider"), "{out}");
    assert!(
        !plans_contain_campaign_json(&paths),
        "start preview should not launch or create campaign state"
    );
    let scope = deadreckon_core::paths::workspace_scope(&repo).expect("scope");
    let record_path = paths
        .scope_root(&scope)
        .join("preview")
        .join(format!("{}.json", deadreckon_core::paths::task_key(goal)));
    let record: Value =
        serde_json::from_str(&fs::read_to_string(record_path).expect("shape record"))
            .expect("shape json");
    assert_eq!(record["shape"], "campaign");
    assert_eq!(record["n"], 4);
    assert_eq!(record["source"], "provider");
    assert_eq!(record["provider"], "cli:shape");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_with_no_provider_refuses_with_try_line() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .env("PATH", service.manager_bin())
        .args(["start", "build the app", "--plain"])
        .output()
        .expect("start missing provider");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("provider"), "{err}");
    assert_verdict_surface(&err, "blocked start", "deadreckon try");
    assert!(
        err.contains("deadreckon config provider cli:codex"),
        "{err}"
    );
}

#[test]
fn start_adopts_single_detected_subscription_provider_inline() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: start ready\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    git(&repo, &["add", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    let bin = temp.path().join("bin");
    write_fake_path_binary(&bin, "codex", "printf 'codex-cli 9.9.9\\n'\n");
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.display());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("PATH", &path)
        .args([
            "start",
            "build the app",
            "--mode",
            "run",
            "--preview",
            "--plain",
        ])
        .output()
        .expect("start detected provider");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("provider"), "{out}");
    assert!(out.contains("cli:codex (detected)"), "{out}");
    assert!(
        out.contains("deadreckon config provider cli:codex"),
        "{out}"
    );
    assert!(
        !out.contains("Choose provider"),
        "single detected provider should be adopted inline:\n{out}"
    );
    assert!(
        !paths.config_path().exists(),
        "inline detected provider adoption must not write config"
    );
}

#[test]
fn start_with_two_detected_providers_prompts_for_execution_team() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: start ready\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    git(&repo, &["add", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    let bin = temp.path().join("bin");
    write_fake_path_binary(&bin, "codex", "printf 'codex-cli 9.9.9\\n'\n");
    write_fake_path_binary(&bin, "claude", "printf 'claude-code 9.9.9\\n'\n");

    let output = deadreckon_pty(
        &paths,
        &repo,
        &["1", "1"],
        &["start", "build the app", "--mode", "run", "--preview"],
        "provider[[:space:]]*:",
        Some(&bin),
    );

    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("Choose execution team"), "{text}");
    assert!(text.contains("cli:codex"), "{text}");
    assert!(text.contains("cli:claude-code"), "{text}");
    assert!(
        !paths.config_path().exists(),
        "interactive provider choice should not write config"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_missing_done_criteria_suggests_def_done_in_non_tty() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nprovider = \"smoke\"\ndoc_provider = \"smoke\"\n",
    )
    .expect("config");
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args(["start", "build the app", "--plain"])
        .output()
        .expect("start missing done criteria");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains(NOUN_DONE_CONTRACT), "{err}");
    assert_verdict_surface(
        &err,
        "blocked start",
        "deadreckon def-done \"what should count as done\" && deadreckon start \"build the app\"",
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_non_git_non_tty_refuses_with_source_mode_try_lines() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("README.md"), "hello").expect("readme");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &source);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&source)
        .args(["start", "build the app", "--plain"])
        .output()
        .expect("start non git");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("non-interactive without a source mode"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked start",
        "deadreckon start \"build the app\" --from .",
    );
    assert!(
        err.contains("deadreckon start \"build the app\" --fresh"),
        "{err}"
    );
    assert!(err.contains("git init"), "{err}");
}

#[test]
fn start_non_git_tty_can_choose_init_git_copy_or_fresh() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("plain");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("README.md"), "hello").expect("readme");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &source);

    let output = deadreckon_pty(
        &paths,
        &source,
        &["1", "1", "1", "3"],
        &["start", "build the app", "--preview"],
        "workspace.*fresh",
        None,
    );

    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("git init"), "{text}");
    assert!(text.contains("Copy current directory"), "{text}");
    assert!(text.contains("Fresh empty workspace"), "{text}");
    assert!(output_row_contains(&text, "workspace", "fresh"), "{text}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn rescue_never_fires_off_tty_refusal_is_byte_identical() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"anthropic\"\n\n[providers.anthropic]\nkind = \"anthropic\"\n",
    )
    .expect("config");
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args(["start", "rescue off tty", "--mode", "run", "--yes"])
        .output()
        .expect("start");

    assert!(!output.status.success());
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        text.contains("provider anthropic needs credentials or an installed binary"),
        "{text}"
    );
    assert!(text.contains("export ANTHROPIC_API_KEY"), "{text}");
    assert!(
        !text.contains("pick another route"),
        "rescue leaked into a non-TTY flow: {text}"
    );
}

#[test]
fn start_pty_with_unusable_default_offers_provider_picker_then_launches() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"anthropic\"\n\n[providers.anthropic]\nkind = \"anthropic\"\n",
    )
    .expect("config");
    let bin = temp.path().join("bin");
    write_fake_path_binary(&bin, "codex", "printf 'codex-cli 9.9.9\\n'\n");

    let output = deadreckon_pty(
        &paths,
        &repo,
        &["1", "1", "1"],
        &["start", "rescue then preview", "--mode", "run", "--preview"],
        "pick another route",
        Some(&bin),
    );

    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        text.contains("pick another route for this launch"),
        "{text}"
    );
    assert!(text.contains("cli:codex"), "{text}");
}

#[test]
fn full_plan_preview_provider_roles_table_echoes_per_role_models() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "model echo goal",
            "--n",
            "2",
            "--preview",
            "--yes",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--planner-model",
            "planner-mx",
            "--model",
            "child-mx",
            "--child-model",
            "1=override-mx",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("planner-mx"), "{text}");
    assert!(text.contains("child-mx"), "{text}");
    assert!(text.contains("override-mx"), "{text}");
}

#[test]
fn pty_start_picker_choose_full_plan_preview() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon_pty(
        &paths,
        &repo,
        &["1", "1"],
        &[
            "start",
            "parallelize the API frontend docs and tests",
            "--preview",
        ],
        "path[[:space:]]*: full-plan orchestration",
        None,
    );

    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("Choose launch path"), "{text}");
    assert!(text.contains("full-plan orchestration"), "{text}");
}

#[test]
fn pty_start_single_detected_provider_prompts_for_model_without_config_write() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: start ready\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    git(&repo, &["add", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    let bin = temp.path().join("bin");
    write_fake_path_binary(&bin, "codex", "printf 'codex-cli 9.9.9\\n'\n");

    let output = deadreckon_pty(
        &paths,
        &repo,
        &["1", "1"],
        &["start", "build the app", "--mode", "run", "--preview"],
        "provider[[:space:]]*: cli:codex",
        Some(&bin),
    );

    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("Choose execution team"), "{text}");
    assert!(text.contains("Codex CLI · provider default"), "{text}");
    assert!(text.contains("cli:codex / provider default"), "{text}");
    assert!(
        !paths.config_path().exists(),
        "interactive execution-team selection should not write config"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_dirty_git_reuses_allow_dirty_guidance() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::write(repo.join("dirty.txt"), "dirty").expect("dirty");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args(["start", "build the app", "--plain"])
        .output()
        .expect("start dirty repo");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("working tree has uncommitted changes"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked start",
        "git stash && deadreckon start \"build the app\"",
    );
    assert!(err.contains("--allow-dirty"), "{err}");
}

#[test]
fn start_run_preview_creates_no_state() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "start",
            "preview run dispatch",
            "--mode",
            "run",
            "--preview",
        ])
        .output()
        .expect("start run preview");

    assert_success(&output);
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[test]
fn start_review_preview_creates_no_plan_state() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "start",
            "preview review dispatch",
            "--mode",
            "review",
            "--preview",
        ])
        .output()
        .expect("start review preview");

    assert_success(&output);
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_dispatches_explicit_run_to_single_job() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "run through guided start",
            "--mode",
            "run",
            "--yes",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("start run dispatch");

    assert_success(&output);
    let root = only_job_root(&paths);
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    let plan = read_launch_plan(&root);
    assert_eq!(job["goal"], "run through guided start", "{job}");
    assert_eq!(job["shape"], "single", "{job}");
    assert_eq!(plan["goal"], "run through guided start", "{plan}");
    assert_eq!(plan["shape"], "single", "{plan}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_dispatches_explicit_review_to_graph_job() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "review through guided start",
            "--mode",
            "review",
            "--yes",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("start review dispatch");

    assert_success(&output);
    let root = only_job_root(&paths);
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    let plan = read_launch_plan(&root);
    assert_eq!(job["goal"], "review through guided start", "{job}");
    assert_eq!(job["shape"], "graph", "{job}");
    assert_eq!(plan["goal"], "review through guided start", "{plan}");
    assert_eq!(
        plan["signals"]["watchkeeper_driver"]["kind"], "review",
        "{plan}"
    );
    assert_eq!(
        plan["signals"]["watchkeeper_driver"]["apply"], "at-end",
        "{plan}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn full_plan_from_preview_currently_disagrees_with_dispatch() {
    assert_full_plan_from_preview_contract_authority_and_driver_agree();
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn full_plan_from_preview_contract_authority_and_driver_agree() {
    assert_full_plan_from_preview_contract_authority_and_driver_agree();
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn flappy_reproduction_returns_graph_job_id_after_accepting_done_contract() {
    assert_full_plan_from_preview_contract_authority_and_driver_agree();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn assert_full_plan_from_preview_contract_authority_and_driver_agree() {
    let temp = repo_tempdir();
    let launch = temp.path().join("empty-launch");
    fs::create_dir_all(&launch).expect("launch directory");
    let source = soundings_swift_fixture(temp.path());
    let source = source.canonicalize().expect("canonical source");
    let source_before = deadreckon_core::flight::build_deliverable_file_index(&source)
        .expect("source before")
        .tree_hash();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &launch);
    let service = characterization_service(&paths);

    let preview = service
        .deadreckon()
        .current_dir(&launch)
        .args([
            "start",
            "continue Cloudwing customization and gameplay review",
            "--mode",
            "full-plan",
            "--from",
            source.to_str().expect("utf8 source"),
            "--preview",
            "--plain",
        ])
        .output()
        .expect("full-plan copy preview");
    assert_success(&preview);
    let preview = stdout(&preview);
    assert!(
        preview.contains(&format!("copy from {}", source.display())),
        "{preview}"
    );

    let launched = service
        .deadreckon()
        .current_dir(&launch)
        .args([
            "start",
            "continue Cloudwing customization and gameplay review",
            "--mode",
            "full-plan",
            "--from",
            source.to_str().expect("utf8 source"),
            "--yes",
            "--plain",
        ])
        .output()
        .expect("full-plan copy launch");
    assert_success(&launched);
    let launched_stdout = stdout(&launched);

    let root = only_job_root(&paths);
    let job_id = root
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("job id");
    assert!(
        launched_stdout.contains(&job_id[..8]),
        "launch must return the created Job ID: {launched_stdout}"
    );
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    let approved = PathBuf::from(job["source_cwd"].as_str().expect("source cwd"));
    assert!(
        approved.starts_with(&root),
        "{approved:?} is not below {root:?}"
    );
    assert_ne!(
        approved, source,
        "Graph execution must not retain the mutable source"
    );
    assert_eq!(
        deadreckon_core::flight::build_deliverable_file_index(&approved)
            .expect("approved source")
            .tree_hash(),
        source_before
    );
    assert_eq!(
        deadreckon_core::flight::build_deliverable_file_index(&source)
            .expect("source after")
            .tree_hash(),
        source_before,
        "launch must leave the operator source byte-identical"
    );
    assert_eq!(
        fs::read_to_string(root.join("acceptance.yaml")).expect("frozen acceptance"),
        fs::read_to_string(launch.join(".deadreckon/acceptance.yaml")).expect("launch acceptance"),
        "the accepted contract must be frozen with the resolved-source Job"
    );

    let authority: Value =
        serde_json::from_slice(&fs::read(root.join("authority.json")).expect("authority"))
            .expect("authority json");
    assert_eq!(
        authority["source_tree_sha256"], source_before,
        "authority must bind the same source tree previewed and copied"
    );

    let plan = read_launch_plan(&root);
    assert_eq!(plan["signals"]["watchkeeper_source"]["mode"], "copy");
    assert_eq!(
        plan["signals"]["watchkeeper_source"]["from"],
        source.display().to_string()
    );
    assert_eq!(
        plan["signals"]["watchkeeper_driver"]["source_init_git"], true,
        "Graph children must initialize Git inside the approved copy"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn single_and_clean_graph_launches_keep_existing_behavior() {
    fn launch_shape(mode: &str, expected_shape: &str) -> (Value, Value) {
        let temp = repo_tempdir();
        let repo = clean_git_repo(&temp);
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        write_start_ready_setup(&paths, &repo);
        let service = characterization_service(&paths);
        let output = service
            .deadreckon()
            .current_dir(&repo)
            .args([
                "start",
                "preserve the established launch path",
                "--mode",
                mode,
                "--yes",
                "--quiet",
                "--plain",
            ])
            .output()
            .expect("launch");
        assert_success(&output);
        let root = only_job_root(&paths);
        let job: Value = serde_json::from_slice(&fs::read(root.join("job.json")).expect("job"))
            .expect("job json");
        let plan = read_launch_plan(&root);
        assert_eq!(job["shape"], expected_shape, "{job}");
        (job, plan)
    }

    let (_single, single_plan) = launch_shape("run", "single");
    assert_eq!(
        single_plan["signals"]["watchkeeper_source"]["mode"],
        "worktree"
    );

    let (_graph, graph_plan) = launch_shape("full-plan", "graph");
    assert_eq!(
        graph_plan["signals"]["watchkeeper_source"]["mode"],
        "worktree"
    );
    assert_eq!(
        graph_plan["signals"]["watchkeeper_driver"]["kind"],
        "full_plan"
    );
}

#[test]
fn soundings_swift_fixture_contains_tracked_modified_and_untracked_inputs() {
    let temp = repo_tempdir();
    let source = soundings_swift_fixture(temp.path());
    let status = Command::new("git")
        .arg("-C")
        .arg(&source)
        .args(["status", "--short"])
        .output()
        .expect("git status");
    assert!(status.status.success());
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status.contains(" M Sources/Cloudwing/GameScene.swift"),
        "{status}"
    );
    assert!(
        status.contains("?? Sources/Cloudwing/Customization.swift"),
        "{status}"
    );

    let index =
        deadreckon_core::flight::build_deliverable_file_index(&source).expect("deliverable index");
    let paths = index
        .files
        .keys()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<BTreeSet<_>>();
    for expected in [
        "Package.swift",
        "Sources/Cloudwing/GameScene.swift",
        "Sources/Cloudwing/Customization.swift",
        "Tests/CloudwingTests/GameMathTests.swift",
    ] {
        assert!(paths.contains(expected), "missing {expected} in {paths:#?}");
    }
}

#[test]
fn unsupported_launch_input_must_not_reach_provider_or_write_contract() {
    let temp = repo_tempdir();
    let launch = temp.path().join("launch");
    fs::create_dir_all(&launch).expect("launch");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("provider");
    let called = temp.path().join("provider-called");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:soundings-counter",
        &format!("printf called > '{}'\nprintf '{{}}'\n", called.display()),
    );
    let missing = temp.path().join("missing-source");

    let output = deadreckon(&paths)
        .current_dir(&launch)
        .args([
            "start",
            "continue Cloudwing",
            "--mode",
            "full-plan",
            "--from",
            missing.to_str().expect("utf8 missing source"),
            "--provider",
            "cli:soundings-counter",
            "--preview",
            "--plain",
        ])
        .output()
        .expect("invalid source preview");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(!called.exists(), "invalid source reached the provider");
    assert!(
        !launch.join(".deadreckon/acceptance.yaml").exists(),
        "invalid source wrote a done contract"
    );
    assert!(
        !paths.jobs_dir().exists()
            || fs::read_dir(paths.jobs_dir())
                .expect("jobs directory")
                .next()
                .is_none(),
        "invalid source wrote a Job"
    );
}

#[test]
fn retry_reuses_valid_generated_contract_without_provider_call() {
    let temp = repo_tempdir();
    let launch = temp.path().join("launch");
    fs::create_dir_all(&launch).expect("launch");
    let source = soundings_swift_fixture(temp.path());
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &launch);
    let called = temp.path().join("provider-called");
    write_fake_cli_subagent_provider(
        &paths,
        &temp.path().join("provider"),
        "cli:retry-counter",
        &format!("printf called > '{}'\nprintf '{{}}'\n", called.display()),
    );

    let output = deadreckon(&paths)
        .current_dir(&launch)
        .args([
            "start",
            "continue Cloudwing",
            "--mode",
            "full-plan",
            "--from",
            source.to_str().expect("utf8 source"),
            "--provider",
            "cli:retry-counter",
            "--preview",
            "--plain",
        ])
        .output()
        .expect("retry preview");

    assert_success(&output);
    assert!(!called.exists(), "valid contract caused a provider call");
    assert!(launch.join(".deadreckon/acceptance.yaml").is_file());
}

#[test]
fn start_preview_json_has_next_actions_and_try_lines_without_ansi() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin).expect("empty bin");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("PATH", &empty_bin)
        .args(["start", "json recovery", "--preview", "--json"])
        .output()
        .expect("start json preview");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(!out.contains("\u{1b}["), "{out}");
    assert!(!out.contains("Choose launch path"), "{out}");
    assert!(!out.contains("Choose provider"), "{out}");
    let value: Value = serde_json::from_str(&out).expect("json");
    assert_eq!(value["kind"], "start");
    assert_eq!(value["will_start"], false);
    assert_eq!(value["try_lines"][0], "deadreckon try");
    assert_eq!(value["next_actions"][0], "deadreckon try");
    assert_eq!(value["primary_action"], "deadreckon try");
    assert_eq!(value["verdict"]["kind"], "blocked");
    assert_eq!(value["verdict"]["recommended_command"], "deadreckon try");
}

#[test]
fn start_plain_preview_strips_ansi() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["start", "plain preview", "--preview", "--plain"])
        .output()
        .expect("start plain preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("preview start"), "{out}");
    assert!(!out.contains("\u{1b}["), "{out}");
}

#[test]
fn start_preview_in_existing_repo_shows_concise_history_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let mut parent = create_test_run(
        &paths,
        &repo,
        "aaaabbbbccccdddd1111222233334444",
        "build original app",
    );
    parent.status = RunStatus::Completed;
    save_state(&parent).expect("save parent");
    let library = paths.library_dir(&parent.scope, &parent.run_id);
    fs::create_dir_all(&library).expect("library");
    fs::write(library.join("README.md"), "completed parent\n").expect("library readme");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["start", "add settings", "--preview", "--plain"])
        .output()
        .expect("start preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("history"), "{out}");
    assert!(out.contains("follow-up available from aaaabbbb"), "{out}");
    assert!(
        out.contains("override: deadreckon start <goal> --mode review"),
        "{out}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_quiet_success_obeys_existing_quiet_policy() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "quiet guided run",
            "--mode",
            "run",
            "--yes",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("start quiet run");

    assert_success(&output);
    assert!(stdout(&output).trim().is_empty(), "{}", stdout(&output));
    assert!(stderr(&output).trim().is_empty(), "{}", stderr(&output));
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_run_success_prints_job_lifecycle_commands() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "footer guided run",
            "--mode",
            "run",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("start run footer");

    assert_success(&output);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    let root = only_job_root(&paths);
    let job_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("job id");
    let job_ref = &job_id[..8];
    assert!(text.contains("completed start"), "{text}");
    assert!(
        text.contains("launched the guided start path as a job"),
        "{text}"
    );
    assert_eq!(text.matches("\nRecommended\n").count(), 1, "{text}");
    assert!(
        text.contains(&format!("deadreckon attach {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon status {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon kill {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon finish {job_ref}")),
        "{text}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_review_success_prints_same_job_lifecycle_commands() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "footer guided review",
            "--mode",
            "review",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("start review footer");

    assert_success(&output);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    let root = only_job_root(&paths);
    let job_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("job id");
    let job_ref = &job_id[..8];
    assert!(text.contains("completed start"), "{text}");
    assert!(
        text.contains("launched the guided start path as a job"),
        "{text}"
    );
    assert_eq!(text.matches("\nRecommended\n").count(), 1, "{text}");
    assert!(
        text.contains(&format!("deadreckon attach {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon status {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon kill {job_ref}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("deadreckon finish {job_ref}")),
        "{text}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_footer_uses_parent_job_id_for_orchestrated_goal() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "plan id footer review",
            "--mode",
            "review",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("start plan id footer");

    assert_success(&output);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    let root = only_job_root(&paths);
    let job_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("job id");
    let job_ref = &job_id[..8];
    assert!(
        text.contains(&format!("deadreckon attach {job_ref}")),
        "{text}"
    );
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    assert_eq!(job["job_id"], job_id, "{job}");
    assert_eq!(job["shape"], "graph", "{job}");
}

#[test]
fn run_preview_uses_same_done_and_workspace_labels() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["run", "preview shared labels", "--smoke", "--preview"])
        .output()
        .expect("run preview");

    assert_success(&output);
    let out = stderr(&output);
    for field in ["path", "done", "workspace", "watch", "stop", "finish"] {
        assert!(out.contains(field), "missing {field}:\n{out}");
    }
}

#[test]
fn orchestrate_preview_uses_same_watch_stop_finish_labels() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "preview shared lifecycle",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");

    assert_success(&output);
    let out = stdout(&output);
    for field in ["path", "done", "workspace", "watch", "stop", "finish"] {
        assert!(out.contains(field), "missing {field}:\n{out}");
    }
    assert!(out.contains("review orchestration"), "{out}");
}

#[test]
fn orchestrate_start_prints_run_like_context() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "tiny hello rust",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--sandbox",
            "none",
            "--yes",
        ])
        .output()
        .expect("orchestrate");

    assert_success(&output);
    let plan = newest_plan(&paths);
    let out = stdout(&output);
    assert!(out.contains("started orchestration"), "{out}");
    assert_verdict_surface(
        &out,
        &format!("completed plan {}", &plan.plan_id[..8]),
        &format!("deadreckon finish {}", &plan.plan_id[..8]),
    );
    assert!(
        out.contains(&format!("deadreckon attach {}", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(!out.contains("attach:"), "{out}");
    assert!(!out.contains("show:"), "{out}");
    assert!(!out.contains("child:"), "{out}");
    assert!(!out.contains("when done:"), "{out}");
    assert!(!out.contains("history:"), "{out}");
    assert!(out.contains("provider"), "{out}");
    assert!(out.contains("workspace"), "{out}");
    // One vocabulary per fact: the launch rows must not duplicate under the
    // legacy labels (providers/source/done next to provider/workspace/contract).
    assert!(!out.contains("\nproviders "), "{out}");
    assert!(!out.contains("\nsource "), "{out}");
    assert!(out.contains("merge repair"), "{out}");
    assert!(out.contains("automatic via"), "{out}");
    assert!(out.contains("plan"), "{out}");
}

#[test]
fn orchestrate_headless_merge_conflict_auto_repairs_with_yes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:orchestrate-repair",
        "case \"$1\" in\n  *\"read-only planning agent\"*) printf '{\"tasks\":[{\"subject\":\"Write first README\",\"goal\":\"Write README from task zero\",\"active_form\":\"Writing first README\",\"depends_on\":[]},{\"subject\":\"Write second README\",\"goal\":\"Write README from task one\",\"active_form\":\"Writing second README\",\"depends_on\":[]}]}' ;;\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"merge both README lanes\",\"actions\":[{\"path\":\"README.md\",\"action\":\"write_synthesized\",\"content\":\"# orchestrated repair\\\\n\",\"preserve\":[\"both README lanes\"]}]}' ;;\n  *\"Task: task-0\"*) printf '# task zero\\n' > README.md; printf 'task zero done\\n' ;;\n  *\"Task: task-1\"*) printf '# task one\\n' > README.md; printf 'task one done\\n' ;;\n  *) printf 'unexpected prompt\\n' >&2; exit 44 ;;\nesac\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "build conflicting README lanes",
            "--planner-provider",
            "cli:orchestrate-repair",
            "--provider",
            "cli:orchestrate-repair",
            "--n",
            "2",
            "--sandbox",
            "none",
            "--yes",
            "--quiet",
        ])
        .output()
        .expect("orchestrate");
    assert_success(&output);

    let plan = newest_plan(&paths);
    assert_eq!(plan.status, PlanStatus::Merged);
    let merged_run_id = plan.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# orchestrated repair\n"
    );
    assert!(
        paths
            .merge_proofs(&plan.plan_id)
            .join("repair-plan.json")
            .is_file()
    );
}

#[test]
fn orchestrate_no_repair_prints_artifact_paths() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:orchestrate-no-repair",
        "case \"$1\" in\n  *\"read-only planning agent\"*) printf '{\"tasks\":[{\"subject\":\"Write first README\",\"goal\":\"Write README from task zero\",\"active_form\":\"Writing first README\",\"depends_on\":[]},{\"subject\":\"Write second README\",\"goal\":\"Write README from task one\",\"active_form\":\"Writing second README\",\"depends_on\":[]}]}' ;;\n  *\"read-only merge repair planner\"*) printf 'repair planner should not be called\\n' >&2; exit 45 ;;\n  *\"Task: task-0\"*) printf '# task zero\\n' > README.md; printf 'task zero done\\n' ;;\n  *\"Task: task-1\"*) printf '# task one\\n' > README.md; printf 'task one done\\n' ;;\n  *) printf 'unexpected prompt\\n' >&2; exit 44 ;;\nesac\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "build conflicting README lanes without repair",
            "--planner-provider",
            "cli:orchestrate-no-repair",
            "--provider",
            "cli:orchestrate-no-repair",
            "--n",
            "2",
            "--sandbox",
            "none",
            "--yes",
            "--no-repair",
            "--quiet",
        ])
        .output()
        .expect("orchestrate");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("automatic repair disabled"), "{err}");
    assert!(err.contains("conflicts.json"), "{err}");
    let plan = newest_plan(&paths);
    assert!(
        paths
            .merge_proofs(&plan.plan_id)
            .join("conflicts.json")
            .is_file()
    );
    assert!(
        !paths
            .merge_proofs(&plan.plan_id)
            .join("repair-plan.json")
            .is_file()
    );
}

#[test]
fn orchestrate_review_works_in_plain_directory_and_copies_done_criteria() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("plain-workspace");
    fs::create_dir_all(workspace.join(".deadreckon")).expect("workspace");
    fs::write(workspace.join("README.md"), "hello").expect("readme");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: plain acceptance\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "orchestrate",
            "review",
            "plain directory work",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--sandbox",
            "none",
            "--yes",
            "--quiet",
        ])
        .output()
        .expect("orchestrate");

    assert_success(&output);
    assert!(!workspace.join(".git").exists());
    let plan = newest_plan(&paths);
    assert_eq!(plan.status, PlanStatus::Merged);
    assert_eq!(
        plan.parent_cwd.as_deref(),
        Some(workspace.canonicalize().expect("canonical").as_path())
    );
    assert!(
        plan.acceptance_path
            .as_ref()
            .is_some_and(|path| { path.ends_with(".deadreckon/acceptance.yaml") })
    );
    let coder_run_id = plan.tasks[0].child_run_id.as_deref().expect("coder run");
    let coder_state = deadreckon_core::load_run(&paths, coder_run_id).expect("coder state");
    let acceptance =
        fs::read_to_string(coder_state.run_root.join("acceptance.yaml")).expect("run acceptance");
    assert!(acceptance.contains("plain acceptance"), "{acceptance}");
}

#[test]
fn orchestrate_acceptance_override_is_passed_to_child_runs() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let spec = temp.path().join("external-acceptance.yaml");
    fs::write(
        &spec,
        "name: external acceptance\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("orchestrate")
        .arg("review")
        .arg("override done criteria")
        .arg("--coder-provider")
        .arg("smoke")
        .arg("--reviewer-provider")
        .arg("smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--acceptance")
        .arg(&spec)
        .arg("--yes")
        .arg("--quiet")
        .output()
        .expect("orchestrate");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.acceptance_path.as_deref(), Some(spec.as_path()));
    let coder_run_id = plan.tasks[0].child_run_id.as_deref().expect("coder run");
    let coder_state = deadreckon_core::load_run(&paths, coder_run_id).expect("coder state");
    let acceptance =
        fs::read_to_string(coder_state.run_root.join("acceptance.yaml")).expect("run acceptance");
    assert!(acceptance.contains("external acceptance"), "{acceptance}");
}

#[test]
fn orchestrate_preview_with_init_git_keeps_plain_source_and_home_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("plain-init");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("README.md"), "hello").expect("readme");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let source_before = filesystem_snapshot(&workspace);
    let home_before = filesystem_snapshot(paths.home());

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "orchestrate",
            "review",
            "initialize before planning",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
            "--init-git",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("orchestrate preflight"), "{out}");
    assert!(out.contains("Implement requested change"), "{out}");
    assert!(out.contains("Review and fix implementation"), "{out}");
    assert_eq!(filesystem_snapshot(&workspace), source_before);
    assert_eq!(filesystem_snapshot(paths.home()), home_before);
    assert!(!workspace.join(".git").exists());
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn orchestrate_headless_without_mode_uses_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["orchestrate", "tiny hello rust"])
        .output()
        .expect("orchestrate");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("non-interactive orchestrate requires an explicit mode"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked orchestrate",
        "deadreckon orchestrate review \"tiny hello rust\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes",
    );
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn orchestrate_headless_without_yes_recommends_forking_saved_plan() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "tiny hello rust",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--quiet",
        ])
        .output()
        .expect("orchestrate review");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    let plan = newest_plan(&paths);
    assert!(
        err.contains("non-interactive orchestrate requires --yes after reviewing preflight"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked orchestrate",
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert_eq!(saved_plan_count(&paths), 1);
}

#[test]
fn plan_rejects_unknown_role_provider_before_writing_plan() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "made-up",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("unknown provider route made-up"), "{err}");
    assert!(err.contains("deadreckon providers list --all"), "{err}");
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn list_includes_orchestration_plans_with_clear_kind() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("list")
        .output()
        .expect("list");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains(&plan.plan_id[..8]), "{out}");
    assert!(out.contains("orchestrate"), "{out}");
    assert!(out.contains("full-plan"), "{out}");
    assert!(out.contains("fork"), "{out}");
    assert!(out.contains("deadreckon attach <id>"), "{out}");
}

#[test]
fn plan_records_explicit_child_provider_overrides() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke:default",
            "--child-provider",
            "1=smoke:reviewer",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(
        plan.providers.default_child.as_deref(),
        Some("smoke:default")
    );
    assert_eq!(
        plan.providers.children.get(&1).map(String::as_str),
        Some("smoke:reviewer")
    );
    assert_eq!(plan.tasks[0].provider.as_deref(), Some("smoke:default"));
    assert_eq!(plan.tasks[1].provider.as_deref(), Some("smoke:reviewer"));
}

#[test]
fn planner_prompt_is_read_only() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let capture = temp.path().join("planner-prompt.txt");
    write_fake_planner_provider(
        &paths,
        temp.path(),
        "cli:planner-capture",
        &capture,
        r#"{"tasks":[{"subject":"Edit README","goal":"Edit README for tiny hello rust","active_form":"Editing README","depends_on":[]},{"subject":"Add source","goal":"Add source for tiny hello rust","active_form":"Adding source","depends_on":["task-0"]}]}"#,
    );
    // The planner is intentionally read-only, so it cannot prove its prompt
    // by writing a host capture file. Make the fixture validate the prompt in
    // process and return the response only when every contract phrase exists.
    let binary = temp.path().join("fake-planner");
    let response = temp.path().join("fake-planner-response.json");
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
set -eu
prompt=$(printf '%s\n' "$@")
for required in \
  'read-only planning agent' \
  'Do not write files' \
  'create temporary files' \
  'install packages' \
  'commit' \
  'Return JSON only' \
  'Dependencies must refer to earlier child ids' \
  'child goals must be implementation or verification slices' \
  'Do not return research-only'
do
  case "$prompt" in
    *"$required"*) ;;
    *) printf 'missing planner contract phrase: %s\n' "$required" >&2; exit 41 ;;
  esac
done
cat '{}'
"#,
            response.display()
        ),
    )
    .expect("asserting fake planner");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("asserting fake planner executable");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "cli:planner-capture",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    assert!(
        !capture.exists(),
        "read-only planner wrote a host prompt capture"
    );
}

#[test]
fn plan_refuses_one_task_response() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let capture = temp.path().join("planner-prompt.txt");
    write_fake_planner_provider(
        &paths,
        temp.path(),
        "cli:planner-one-task",
        &capture,
        r#"{"tasks":[{"subject":"Only task","goal":"Do everything in one task","active_form":"Doing everything","depends_on":[]}]}"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "cli:planner-one-task",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("plan");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("provider returned 1 children; need 2"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked plan",
        "deadreckon plan ... --provider <other>",
    );
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn plan_n_flag_clamped_to_2_through_6() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "1",
            "--quiet",
        ])
        .output()
        .expect("plan n 1");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("plan must have >= 2 children"), "{err}");
    assert_verdict_surface(&err, "blocked plan", "deadreckon run \"<the only child>\"");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "7",
            "--quiet",
        ])
        .output()
        .expect("plan n 7");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("plan capped at 6 children; got 7"), "{err}");
    assert_verdict_surface(
        &err,
        "blocked plan",
        "deadreckon chain plan \"tiny hello rust\" --n 7",
    );
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn plan_refusal_json_adds_verdict_and_primary_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "1",
            "--quiet",
            "--json",
        ])
        .output()
        .expect("plan refusal json");

    assert!(!output.status.success(), "{}", stdout(&output));
    let value: Value = serde_json::from_str(&stderr(&output)).expect("json refusal surface");
    assert_eq!(value["kind"], "plan_refusal");
    assert_eq!(value["error"], "plan must have >= 2 children");
    assert_eq!(value["verdict"]["kind"], "blocked");
    assert_eq!(
        value["primary_action"],
        "deadreckon run \"<the only child>\""
    );
    assert_eq!(
        value["primary_action"],
        value["verdict"]["recommended_command"]
    );
    assert_eq!(value["next_actions"][0], value["primary_action"]);
}

#[test]
fn plan_records_explicit_planner_and_child_providers() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke:planner",
            "--provider",
            "smoke:default",
            "--child-provider",
            "1=smoke:reviewer",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.providers.planner.as_deref(), Some("smoke:planner"));
    assert_eq!(
        plan.providers.default_child.as_deref(),
        Some("smoke:default")
    );
    assert_eq!(
        plan.providers.children.get(&1).map(String::as_str),
        Some("smoke:reviewer")
    );
    assert_eq!(plan.tasks[0].provider.as_deref(), Some("smoke:default"));
    assert_eq!(plan.tasks[1].provider.as_deref(), Some("smoke:reviewer"));
}

#[test]
fn quiet_plain_combined_emits_no_stdout() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "tiny hello rust",
            "--smoke",
            "--plain",
            "--quiet",
            "--sandbox",
            "none",
            "--no-hints",
            "--yes",
        ])
        .output()
        .expect("run");

    assert_success(&output);
    let out = stdout(&output);
    assert_eq!(out, "");
    assert!(!out.contains("\x1b["), "{out:?}");
}

#[test]
fn quiet_emits_no_stdout_on_success() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "tiny hello rust",
            "--smoke",
            "--quiet",
            "--sandbox",
            "none",
            "--no-hints",
            "--yes",
        ])
        .output()
        .expect("run");

    assert_success(&output);
    assert_eq!(stdout(&output), "");
}

#[test]
fn plain_mode_progress_works_without_tty() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("TERM", "dumb")
        .env("DEADRECKON_FORCE_PLAIN", "1")
        .args([
            "run",
            "tiny hello rust",
            "--smoke",
            "--plain",
            "--sandbox",
            "none",
            "--yes",
            "--no-hints",
        ])
        .output()
        .expect("run");

    assert_success(&output);
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(combined.contains("completed"), "{combined}");
    assert!(!combined.contains("\x1b["), "{combined:?}");
}

#[test]
fn review_mode_post_action_hints_name_coder_and_reviewer() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--mode",
            "review",
            "--coder-provider",
            "smoke:coder",
            "--reviewer-provider",
            "smoke:reviewer",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let out = stdout(&output);
    let plan = newest_plan(&paths);
    assert_verdict_surface(
        &out,
        &format!("preview plan {}", &plan.plan_id[..8]),
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert!(out.contains("coder=smoke:coder"), "{out}");
    assert!(out.contains("reviewer=smoke:reviewer"), "{out}");
    assert!(!out.contains("fork:"), "{out}");
}

#[test]
fn plan_hints_name_capabilities_and_ready_tasks() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "deploy websocket app",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("network=Allowlist deploy=true install=false"),
        "{out}"
    );
    assert!(
        output_row_contains(&out, "children", "2 (2 ready / 0 blocked)"),
        "{out}"
    );
    let plan = newest_plan(&paths);
    assert_verdict_surface(
        &out,
        &format!("preview plan {}", &plan.plan_id[..8]),
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert!(!out.contains("fork:"), "{out}");
}

#[test]
fn plan_capability_preview_allows_network_for_multiplayer_live_goals() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "make a fully multiplayer live flight simulator",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("network=Allowlist deploy=false install=false"),
        "{out}"
    );
}

#[test]
fn error_messages_include_recovery_guidance() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let verdict_cases: Vec<(Vec<&str>, &str, &str)> = vec![
        (
            vec!["plan", "", "--quiet"],
            "--goal must be non-empty",
            "deadreckon plan \"your goal\"",
        ),
        (
            vec![
                "plan",
                "tiny hello rust",
                "--planner-provider",
                "smoke",
                "--provider",
                "smoke",
                "--n",
                "1",
                "--quiet",
            ],
            "plan must have >= 2 children",
            "deadreckon run \"<the only child>\"",
        ),
    ];

    for (args, message, recommended) in verdict_cases {
        let output = deadreckon(&paths)
            .current_dir(&repo)
            .args(args)
            .output()
            .expect("command");
        assert!(!output.status.success(), "{}", stdout(&output));
        let err = stderr(&output);
        assert!(err.contains(message), "{err}");
        assert_verdict_surface(&err, "blocked plan", recommended);
    }

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "[", "--regex"])
        .output()
        .expect("history grep");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("History grep could not compile regex pattern"),
        "{err}"
    );
    assert_verdict_surface(
        &err,
        "blocked history grep --regex",
        "deadreckon history grep '[' --regex",
    );
}

#[test]
fn fork_spawns_n_children_with_distinct_scopes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");

    assert_success(&output);
    let plan = load_plan(&paths, &plan.plan_id).expect("plan");
    assert_eq!(plan.status, PlanStatus::Forked);
    assert!(!paths.coordinator_json(&plan.plan_id).is_file());
    assert!(
        plan.tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed)
    );
    let scopes = plan
        .tasks
        .iter()
        .map(|task| task.child_scope.clone().expect("child scope"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(scopes.len(), 2);
    for task in &plan.tasks {
        assert!(paths.child_summary(&plan.plan_id, &task.task_id).is_file());
        let run_id = task.child_run_id.as_ref().expect("run id");
        let state = deadreckon_core::load_run(&paths, run_id).expect("child run");
        assert!(state.working_dir.join(".deadreckon/parent.json").is_file());
    }
    let messages = read_plan_messages(&paths, &plan.plan_id).expect("messages");
    assert!(messages.len() >= 4, "{messages:#?}");
    let summaries = messages
        .iter()
        .map(|message| message.summary.as_str())
        .collect::<Vec<_>>();
    let task_0_started = summaries
        .iter()
        .position(|summary| *summary == "task-0 started")
        .expect("task 0 started");
    let task_1_started = summaries
        .iter()
        .position(|summary| *summary == "task-1 started")
        .expect("task 1 started");
    let first_completed = summaries
        .iter()
        .position(|summary| summary.contains("completed"))
        .expect("completion message");
    assert!(task_0_started < first_completed, "{summaries:#?}");
    assert!(task_1_started < first_completed, "{summaries:#?}");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &plan.plan_id[..8], "--no-hints"])
        .output()
        .expect("attach");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("capabilities:"), "{out}");
    assert!(
        out.lines()
            .any(|line| line.contains("gate") && line.contains("PASSED 1/1")),
        "{out}"
    );
    assert!(out.contains("latest turn"), "{out}");
    assert!(out.contains("tokens"), "{out}");
}

#[test]
fn fork_launches_each_child_with_resolved_provider() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke:default",
            "--child-provider",
            "1=smoke:reviewer",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("plan");
    assert_eq!(plan.tasks[0].provider.as_deref(), Some("smoke:default"));
    assert_eq!(plan.tasks[1].provider.as_deref(), Some("smoke:reviewer"));
    assert!(
        plan.tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed)
    );
}

#[test]
fn fork_writes_progress_messages_jsonl() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let messages = read_plan_messages(&paths, &plan.plan_id).expect("messages");
    assert!(
        messages
            .iter()
            .any(|message| message.kind == PlanMessageKind::Progress
                && message.summary == "task-0 started"),
        "{messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.kind == PlanMessageKind::Progress
                && message.summary == "task-1 completed"),
        "{messages:#?}"
    );
}

#[test]
fn fork_writes_plan_lifecycle_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::PlanStarted)),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskReady { task_id, .. } if task_id == "task-0"
        )),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskStarted { task_id, .. } if task_id == "task-0"
        )),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskRunDiscovered { task_id, run_id: Some(_), .. }
                if task_id == "task-0"
        )),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskCompleted { task_id, status, .. }
                if task_id == "task-0" && status == "completed"
        )),
        "{events:#?}"
    );
}

#[test]
fn fork_writes_plan_started_event_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.event, PlanEventKind::PlanStarted))
            .count(),
        1,
        "{events:#?}"
    );
}

#[test]
fn orchestrate_full_plan_preview_keeps_source_and_state_unchanged_and_starts_no_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("preview-child-provider");
    let child_marker = temp.path().join("preview-child-ran");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "preview-child",
        &format!(
            "printf '%s\\n' child-ran > '{}'\nexit 99",
            child_marker.display()
        ),
    );
    let filesystem_before = filesystem_snapshot(temp.path());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke:planner",
            "--provider",
            "preview-child",
            "--n",
            "2",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("orchestrate preflight"), "{out}");
    assert!(out.contains("Create foundation"), "{out}");
    assert!(out.contains("Add behavior"), "{out}");
    assert!(out.contains("preview-child"), "{out}");
    assert_eq!(filesystem_snapshot(temp.path()), filesystem_before);
    assert!(!child_marker.exists());
    assert_eq!(saved_plan_count(&paths), 0);
}

#[test]
fn fork_emits_task_ready_for_ready_batch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    for task in &plan.tasks {
        assert!(
            events.iter().any(|event| matches!(
                &event.event,
                PlanEventKind::TaskReady { task_id, task_index }
                    if task_id == &task.task_id && *task_index == task.index as usize
            )),
            "{events:#?}"
        );
    }
}

#[test]
fn fork_emits_task_started_before_child_launch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    let started = event_position(&events, |event| {
        matches!(
            event,
            PlanEventKind::TaskStarted { task_id, .. } if task_id == "task-0"
        )
    });
    let discovered = event_position(&events, |event| {
        matches!(
            event,
            PlanEventKind::TaskRunDiscovered { task_id, .. } if task_id == "task-0"
        )
    });

    assert!(started < discovered, "{events:#?}");
}

#[test]
fn blocked_dependency_does_not_emit_task_started() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = review_gate_failure_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(!events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::TaskStarted { task_id, .. } if task_id == "task-1"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::TaskBlocked { task_id, .. } if task_id == "task-1"
    )));
}

#[test]
fn fork_emits_task_run_discovered_with_pid_and_run_id() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskRunDiscovered {
                task_id,
                run_id: Some(_),
                pid: Some(_),
                ..
            } if task_id == "task-0"
        )),
        "{events:#?}"
    );
}

#[test]
fn fork_emits_task_completed_with_run_status() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskCompleted {
                task_id,
                run_id: Some(_),
                status,
                ..
            } if task_id == "task-0" && status == "completed"
        )),
        "{events:#?}"
    );
}

#[test]
fn fork_emits_task_failed_for_red_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = review_gate_failure_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskFailed { task_id, .. } if task_id == "task-0"
        )),
        "{events:#?}"
    );
}

#[test]
fn blocked_pending_task_gets_task_blocked_event() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = review_gate_failure_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskBlocked { task_id, reason, .. }
                if task_id == "task-1" && reason.contains("task-0")
        )),
        "{events:#?}"
    );
}

#[test]
fn failed_plan_emits_plan_failed_event() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = review_gate_failure_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert_eq!(plan.status, PlanStatus::Failed);
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::PlanFailed { .. })),
        "{events:#?}"
    );
}

#[test]
fn fork_writes_coordinator_json_with_child_pids() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:slow-child",
        "sleep 2\nprintf 'changed by slow child\\n' > slow-child.txt\nprintf 'slow child done\\n'\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:slow-child",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let mut child = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .spawn()
        .expect("fork spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut live_pid_seen = false;
    while std::time::Instant::now() < deadline {
        if let Ok(raw) = fs::read_to_string(paths.coordinator_json(&plan.plan_id))
            && let Ok(coordinator) = serde_json::from_str::<CoordinatorState>(&raw)
            && coordinator
                .children
                .iter()
                .filter_map(|child| child.pid)
                .any(pid_is_alive)
        {
            live_pid_seen = true;
            break;
        }
        if let Some(status) = child.try_wait().expect("try wait") {
            panic!("fork exited before live pid snapshot: {status}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let output = child.wait_with_output().expect("fork output");
    assert_success(&output);
    assert!(
        live_pid_seen,
        "coordinator.json never contained a live child pid"
    );
}

#[test]
fn kill_plan_cascade_cleans_all_children_in_under_5s() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:slow-kill-child",
        "sleep 10\nprintf 'changed by slow child\\n' > slow-child.txt\nprintf 'slow child done\\n'\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:slow-kill-child",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);
    let mut fork = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .spawn()
        .expect("fork spawn");

    let pids = wait_for_plan_child_pids(&paths, &plan.plan_id);
    let started = std::time::Instant::now();
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["kill", &plan.plan_id[..8], "--force"])
        .output()
        .expect("kill plan");
    assert_success(&output);
    let kill_stdout = stdout(&output);
    assert!(kill_stdout.starts_with("killed plan "), "{kill_stdout}");
    assert!(kill_stdout.contains("Explanation"), "{kill_stdout}");
    assert_eq!(
        kill_stdout
            .matches(&format!(
                "deadreckon show {} --why-failed",
                &plan.plan_id[..8]
            ))
            .count(),
        1,
        "{kill_stdout}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let _ = fork.wait();

    for pid in pids {
        assert!(!pid_is_alive(pid), "pid {pid} still alive");
    }
    let plan = load_plan(&paths, &plan.plan_id).expect("killed plan");
    assert_eq!(plan.status, PlanStatus::Failed);
    assert!(
        plan.tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Killed),
        "{plan:#?}"
    );
    for task in &plan.tasks {
        let run_id = task.child_run_id.as_deref().expect("child run id");
        let state = deadreckon_core::load_run(&paths, run_id).expect("child state");
        assert_eq!(state.status, RunStatus::Killed);
        assert!(!lock_path(&paths, &state.scope, &state.task_key).exists());
    }
}

#[test]
fn plan_kill_can_use_discovered_run_id_from_event_or_sidecar() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = kill_live_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(plan.tasks.iter().all(|task| task.child_run_id.is_some()));
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::TaskRunDiscovered {
                run_id: Some(_),
                ..
            }
        )),
        "{events:#?}"
    );
}

#[test]
fn kill_plan_emits_task_killed_for_live_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = kill_live_plan(&paths, &repo, temp.path());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::TaskKilled { .. })),
        "{events:#?}"
    );
}

#[test]
fn fork_respects_task_dependencies() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    let first_task_id = plan.tasks[0].task_id.clone();
    plan.tasks[1].depends_on = vec![first_task_id];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("forked plan");
    let messages = read_plan_messages(&paths, &plan.plan_id).expect("messages");
    let summaries = messages
        .iter()
        .map(|message| message.summary.as_str())
        .collect::<Vec<_>>();
    let first_completed = summaries
        .iter()
        .position(|summary| *summary == "task-0 completed")
        .expect("task 0 completed");
    let second_started = summaries
        .iter()
        .position(|summary| *summary == "task-1 started")
        .expect("task 1 started");
    assert!(first_completed < second_started, "{summaries:#?}");

    let second_run_id = plan.tasks[1].child_run_id.as_deref().expect("second run");
    let second_state = deadreckon_core::load_run(&paths, second_run_id).expect("second state");
    assert!(
        second_state.goal.contains("## Dependency summaries"),
        "{}",
        second_state.goal
    );
    assert!(
        second_state
            .goal
            .contains("Child Summary: Create foundation"),
        "{}",
        second_state.goal
    );
    assert!(
        second_state.goal.contains(
            plan.tasks[0]
                .child_run_id
                .as_deref()
                .expect("first child run")
        ),
        "{}",
        second_state.goal
    );
}

#[test]
fn full_plan_dependent_child_starts_from_completed_dependency_artifact() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:source-chain",
        r#"
case "$1" in
  *"Task: task-1"*)
    test -f base.txt || { printf 'missing base from dependency\n' >&2; exit 42; }
    printf 'task 1 saw dependency\n' > child-saw-base.txt
    ;;
  *"Task: task-0"*)
    printf 'from task 0\n' > base.txt
    ;;
  *)
    printf 'unknown task prompt\n' >&2
    exit 43
    ;;
esac
printf 'done\n'
"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "source chaining example",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:source-chain",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[1].depends_on = vec![plan.tasks[0].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("forked plan");
    assert_eq!(plan.tasks[1].status, PlanTaskStatus::Completed);
    let second_run_id = plan.tasks[1].child_run_id.as_deref().expect("second run");
    let second_state = deadreckon_core::load_run(&paths, second_run_id).expect("second state");
    let second_library = paths.library_dir(&second_state.scope, &second_state.run_id);
    assert!(second_library.join("base.txt").is_file());
    assert!(second_library.join("child-saw-base.txt").is_file());

    let codebase = read_codebase_record(&second_state.working_dir).expect("codebase");
    let source_path = codebase.source_path.as_ref().expect("source path");
    let expected_plan_dir = paths
        .plan_dir(&plan.plan_id)
        .canonicalize()
        .unwrap_or_else(|_| paths.plan_dir(&plan.plan_id));
    assert!(
        source_path.starts_with(&expected_plan_dir),
        "{}",
        source_path.display()
    );
    assert!(
        source_path.ends_with("launch/task-1/source"),
        "{}",
        source_path.display()
    );
    assert!(source_path.join("base.txt").is_file());
}

#[test]
fn full_plan_multi_dependency_child_uses_composed_dependency_artifacts() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:multi-source-chain",
        r#"
case "$1" in
  *"Task: task-2"*)
    test -f a.txt || { printf 'missing a.txt\n' >&2; exit 42; }
    test -f b.txt || { printf 'missing b.txt\n' >&2; exit 43; }
    printf 'saw both\n' > saw-a-and-b.txt
    ;;
  *"Task: task-0"*)
    printf 'a\n' > a.txt
    ;;
  *"Task: task-1"*)
    printf 'b\n' > b.txt
    ;;
  *)
    printf 'unknown task prompt\n' >&2
    exit 44
    ;;
esac
printf 'done\n'
"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "multi source chaining example",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:multi-source-chain",
            "--n",
            "3",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[2].depends_on = vec![plan.tasks[0].task_id.clone(), plan.tasks[1].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("forked plan");
    assert_eq!(plan.tasks[2].status, PlanTaskStatus::Completed);
    let third_run_id = plan.tasks[2].child_run_id.as_deref().expect("third run");
    let third_state = deadreckon_core::load_run(&paths, third_run_id).expect("third state");
    let third_library = paths.library_dir(&third_state.scope, &third_state.run_id);
    assert!(third_library.join("a.txt").is_file());
    assert!(third_library.join("b.txt").is_file());
    assert!(third_library.join("saw-a-and-b.txt").is_file());
}

#[test]
fn full_plan_multi_dependency_conflict_fails_before_launching_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:conflicting-source-chain",
        r#"
	case "$1" in
	  *"read-only merge repair planner"*)
	    printf 'repair planner should not be called\n' >&2
	    exit 45
	    ;;
	  *"Task: task-0"*)
	    printf 'from task 0\n' > shared.txt
	    ;;
  *"Task: task-1"*)
    printf 'from task 1\n' > shared.txt
    ;;
  *"Task: task-2"*)
    printf 'task 2 should not launch\n' > should-not-exist.txt
    ;;
  *)
    printf 'unknown task prompt\n' >&2
    exit 44
    ;;
esac
printf 'done\n'
"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "conflicting source chaining example",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:conflicting-source-chain",
            "--n",
            "3",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[2].depends_on = vec![plan.tasks[0].task_id.clone(), plan.tasks[1].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "fork",
            &plan.plan_id[..8],
            "--sandbox",
            "none",
            "--no-repair",
            "--quiet",
        ])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("forked plan");
    assert_eq!(plan.status, PlanStatus::Failed);
    assert_eq!(plan.tasks[2].status, PlanTaskStatus::Failed);
    assert!(plan.tasks[2].child_run_id.is_none());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::TaskFailed { task_id, reason, .. }
            if task_id == "task-2" && reason.contains("dependency source conflict")
    )));
}

#[test]
fn full_plan_multi_dependency_conflict_repairs_before_launching_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:repairing-source-chain",
        r#"
case "$1" in
  *"read-only merge repair planner"*)
    printf '{"decision":"synthesize","rationale":"combine sibling dependency outputs","actions":[{"path":"shared.txt","action":"write_synthesized","content":"merged dependency\\n","preserve":["task-0 shared output","task-1 shared output"]}]}'
    ;;
  *"Task: task-2"*)
    test "$(cat shared.txt)" = "merged dependency" || {
      printf 'unexpected shared content: %s\n' "$(cat shared.txt)" >&2
      exit 46
    }
    printf 'task 2 saw repaired dependency\n' > saw-repaired.txt
    ;;
  *"Task: task-0"*)
    printf 'from task 0\n' > shared.txt
    ;;
  *"Task: task-1"*)
    printf 'from task 1\n' > shared.txt
    ;;
  *)
    printf 'unknown task prompt\n' >&2
    exit 44
    ;;
esac
printf 'done\n'
"#,
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "conflicting source chaining example",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:repairing-source-chain",
            "--n",
            "3",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[2].depends_on = vec![plan.tasks[0].task_id.clone(), plan.tasks[1].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "fork",
            &plan.plan_id[..8],
            "--sandbox",
            "none",
            "--repair-provider",
            "cli:repairing-source-chain",
            "--quiet",
        ])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("forked plan");
    assert_eq!(plan.status, PlanStatus::Forked);
    assert_eq!(plan.tasks[2].status, PlanTaskStatus::Completed);
    let task_2_run = plan.tasks[2].child_run_id.as_deref().expect("task 2 run");
    let task_2_state = deadreckon_core::load_run(&paths, task_2_run).expect("task 2 state");
    let task_2_library = paths.library_dir(&task_2_state.scope, &task_2_state.run_id);
    assert_eq!(
        fs::read_to_string(task_2_library.join("shared.txt")).expect("shared"),
        "merged dependency\n"
    );
    assert!(
        task_2_library.join("saw-repaired.txt").is_file(),
        "{}",
        task_2_library.display()
    );
    assert!(
        paths
            .plan_dir(&plan.plan_id)
            .join("launch")
            .join(&plan.tasks[2].task_id)
            .join("merge-proofs")
            .join("repair-plan.json")
            .is_file()
    );
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::MergeRepaired { strategy, .. } if strategy == "dependency-synthesize"
    )));
}

#[test]
fn fork_passes_worker_spec_to_child_prompt() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let first = &plan.tasks[0];
    let run_id = first.child_run_id.as_deref().expect("first run");
    let state = deadreckon_core::load_run(&paths, run_id).expect("child run");
    let spec_path = paths.worker_spec(&plan.plan_id, &first.task_id);
    let spec = fs::read_to_string(&spec_path).expect("worker spec");

    assert!(state.goal.contains("Worker spec path:"), "{}", state.goal);
    assert!(
        state.goal.contains(&spec_path.display().to_string()),
        "{}",
        state.goal
    );
    assert!(
        state.goal.contains("Root goal: tiny hello rust"),
        "{}",
        state.goal
    );
    assert!(
        state.goal.contains("Do not spawn subagents"),
        "{}",
        state.goal
    );
    assert!(
        state.goal.contains(spec.trim()),
        "child prompt should include exact worker spec\n{}",
        state.goal
    );
}

#[test]
fn fork_refuses_when_plan_already_forked() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("no-op plan "), "{err}");
    assert!(err.contains("running"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_fails_on_conflict_then_prefer_child_promotes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let second = &plan.tasks[1];
    let second_run = second.child_run_id.as_ref().expect("second run");
    let second_state = deadreckon_core::load_run(&paths, second_run).expect("second state");
    let second_library = paths.library_dir(&second_state.scope, &second_state.run_id);
    fs::write(second_library.join("README.md"), "# preferred child\n").expect("conflict write");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("automatic repair disabled"),
        "{}",
        stderr(&output)
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--strategy",
            "prefer-child",
            "--prefer-child",
            "1",
            "--quiet",
        ])
        .output()
        .expect("merge prefer");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    assert_eq!(merged.status, PlanStatus::Merged);
    let merged_run_id = merged.merged_run_id.as_ref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert!(library.join("deadreckon-plan-manifest.json").is_file());
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# preferred child\n"
    );
}

#[test]
fn merge_composes_disjoint_children() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    for task in &plan.tasks {
        let run_id = task.child_run_id.as_deref().expect("run id");
        let state = deadreckon_core::load_run(&paths, run_id).expect("child run");
        let library = paths.library_dir(&state.scope, &state.run_id);
        fs::write(
            library.join(format!("child-{}.txt", task.index)),
            format!("from {}", task.task_id),
        )
        .expect("child marker");
    }

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let expected_merged = merged_run_id.to_string();
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("child-0.txt")).expect("child 0"),
        "from task-0"
    );
    assert_eq!(
        fs::read_to_string(library.join("child-1.txt")).expect("child 1"),
        "from task-1"
    );
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::MergeStarted)),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.event,
            PlanEventKind::MergeCompleted { merged_run_id } if merged_run_id == &expected_merged
        )),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::PlanCompleted)),
        "{events:#?}"
    );
}

#[test]
fn materialize_accepts_completed_plan_id() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let dest = temp.path().join("materialized-plan");
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("materialize")
        .arg(&plan.plan_id[..8])
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize plan");
    assert_success(&output);

    let out = stdout(&output);
    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("result run");
    assert!(out.contains("plan result:"), "{out}");
    assert!(out.contains(&plan.plan_id[..8]), "{out}");
    assert!(out.contains(&merged_run_id[..8]), "{out}");
    assert!(dest.join("README.md").is_file());
    assert!(dest.join("deadreckon-plan-manifest.json").is_file());
}

#[test]
fn apply_accepts_completed_plan_id_and_commits_to_source_branch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    write_plan_child_marker(
        &paths,
        &plan,
        0,
        "plan-apply-marker.txt",
        "from plan apply\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["apply", &plan.plan_id[..8], "--no-confirm", "--cleanup"])
        .output()
        .expect("apply plan");
    assert_success(&output);

    let out = stdout(&output);
    assert!(out.contains("plan result:"), "{out}");
    assert!(out.contains(&plan.plan_id[..8]), "{out}");
    assert!(repo.join("plan-apply-marker.txt").is_file());
    assert!(!repo.join("deadreckon-plan-manifest.json").exists());
    let subject = git_output(&repo, &["log", "-1", "--pretty=%s"]);
    assert!(subject.contains("deadreckon plan"), "{subject}");
    assert_eq!(git_output(&repo, &["status", "--short"]), "");
}

#[test]
fn apply_plan_id_supports_autostash_in_dirty_source_repo() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    write_plan_child_marker(
        &paths,
        &plan,
        0,
        "plan-autostash-marker.txt",
        "from plan autostash\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    fs::write(repo.join("local-note.txt"), "keep my local note\n").expect("local note");
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "apply",
            &plan.plan_id[..8],
            "--autostash",
            "--no-confirm",
            "--cleanup",
        ])
        .output()
        .expect("apply plan");
    assert_success(&output);

    assert!(repo.join("plan-autostash-marker.txt").is_file());
    assert_eq!(
        fs::read_to_string(repo.join("local-note.txt")).expect("local note"),
        "keep my local note\n"
    );
    let status = git_output(&repo, &["status", "--short"]);
    assert!(status.contains("?? local-note.txt"), "{status}");
}

#[test]
fn finish_accepts_completed_plan_id_and_applies_in_git_repo() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    write_plan_child_marker(
        &paths,
        &plan,
        0,
        "plan-finish-marker.txt",
        "from plan finish\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["finish", &plan.plan_id[..8], "--no-confirm", "--cleanup"])
        .output()
        .expect("finish plan");
    assert_success(&output);

    let out = stdout(&output);
    assert!(out.contains("plan result:"), "{out}");
    assert!(out.contains("completed apply "), "{out}");
    assert!(out.contains("Explanation\n"), "{out}");
    assert!(out.contains("Evidence\n"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(out.contains("Recommended\ndeadreckon show "), "{out}");
    assert!(!out.contains("try:"), "{out}");
    assert!(!out.contains("finish:"), "{out}");
    assert!(!out.contains("apply:"), "{out}");
    assert!(repo.join("plan-finish-marker.txt").is_file());
    let subject = git_output(&repo, &["log", "-1", "--pretty=%s"]);
    assert!(subject.contains("deadreckon plan"), "{subject}");
}

#[test]
fn completed_plan_list_action_points_to_finish() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .arg("list")
        .output()
        .expect("list");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains(&plan.plan_id[..8]), "{out}");
    assert!(out.contains("orchestrate"), "{out}");
    assert!(out.contains("finish"), "{out}");
}

#[test]
fn plan_child_selector_drills_to_child_run() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let child_run_id = plan.tasks[0].child_run_id.as_deref().expect("child run");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "attach",
            &format!("{}:task-0", &plan.plan_id[..8]),
            "--plain",
            "--no-hints",
        ])
        .output()
        .expect("attach child");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(&format!("plan {} / task-0", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(out.contains(&child_run_id[..8]), "{out}");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &format!("{}:0", &plan.plan_id[..8]), "--plain"])
        .output()
        .expect("show child");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(&format!("plan {} / task-0", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(out.contains(&child_run_id[..8]), "{out}");
}

#[test]
fn merge_emits_started_and_completed_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::MergeStarted)),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::MergeCompleted { .. })),
        "{events:#?}"
    );
}

#[test]
fn merged_plan_emits_plan_completed_after_merge_run_id() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    let merge_completed = event_position(
        &events,
        |event| matches!(event, PlanEventKind::MergeCompleted { merged_run_id } if !merged_run_id.is_empty()),
    );
    let plan_completed = event_position(&events, |event| {
        matches!(event, PlanEventKind::PlanCompleted)
    });

    assert!(merge_completed < plan_completed, "{events:#?}");
}

#[test]
fn merge_fails_on_conflict_default() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("failed plan "), "{err}");
    assert!(err.contains("merge conflict at README.md"), "{err}");
    assert!(err.contains("automatic repair disabled"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::MergeConflict { .. })),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::PlanFailed { .. })),
        "{events:#?}"
    );
}

#[test]
fn merge_conflict_path_characterization() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let conflicts_path = paths.merge_proofs(&plan.plan_id).join("conflicts.json");
    let err = stderr(&output);
    assert!(err.starts_with("failed plan "), "{err}");
    assert!(err.contains("merge conflict at README.md"), "{err}");
    assert!(err.contains("automatic repair disabled"), "{err}");
    assert!(
        err.contains(&format!("conflicts: {}", conflicts_path.display())),
        "{err}"
    );
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");

    let bundle: Value =
        serde_json::from_str(&fs::read_to_string(conflicts_path).expect("conflicts"))
            .expect("conflicts json");
    assert_eq!(bundle["schema_version"], 2);
    assert_eq!(bundle["strategy"], "dag-aware");
    assert_eq!(bundle["conflicts"][0]["path"], "README.md");
    assert_eq!(bundle["conflicts"][0]["children"][0]["task_id"], "task-0");
    assert_eq!(bundle["conflicts"][0]["children"][1]["task_id"], "task-1");
}

#[test]
fn merge_conflict_emits_conflict_event_before_refusal() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    let conflict = event_position(&events, |event| {
        matches!(event, PlanEventKind::MergeConflict { .. })
    });
    let failed = event_position(&events, |event| {
        matches!(event, PlanEventKind::PlanFailed { .. })
    });

    assert!(conflict < failed, "{events:#?}");
}

#[test]
fn merge_conflict_bundle_records_all_child_versions() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let bundle: Value = serde_json::from_str(
        &fs::read_to_string(paths.merge_proofs(&plan.plan_id).join("conflicts.json"))
            .expect("conflicts"),
    )
    .expect("conflicts json");
    assert_eq!(bundle["schema_version"], 2);
    assert_eq!(bundle["plan_id"], plan.plan_id);
    assert_eq!(bundle["strategy"], "dag-aware");
    let conflict = &bundle["conflicts"][0];
    assert_eq!(conflict["path"], "README.md");
    assert_eq!(conflict["children"].as_array().expect("children").len(), 2);
    assert_eq!(conflict["children"][0]["task_id"], "task-0");
    assert_eq!(conflict["children"][1]["task_id"], "task-1");
    assert!(
        conflict["children"][0]["artifact_path"]
            .as_str()
            .expect("artifact path")
            .ends_with("README.md"),
        "{conflict:#}"
    );
}

#[test]
fn merge_repair_request_includes_task_graph_and_summaries() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let request: Value = serde_json::from_str(
        &fs::read_to_string(
            paths
                .merge_proofs(&plan.plan_id)
                .join("repair-request.json"),
        )
        .expect("request"),
    )
    .expect("request json");
    assert_eq!(request["schema_version"], 1);
    assert_eq!(request["plan_id"], plan.plan_id);
    assert_eq!(request["task_graph"][0]["task_id"], "task-0");
    assert_eq!(request["task_graph"][1]["task_id"], "task-1");
    assert!(
        request["summary_paths"]["task-0"]
            .as_str()
            .expect("summary")
            .ends_with("summaries/task-0.md"),
        "{request:#}"
    );
    assert_eq!(request["conflicts"][0]["path"], "README.md");
}

#[test]
fn merge_allows_descendant_child_to_override_ancestor_file() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_dependency_readme_conflict(&paths, &repo, false);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# descendant child\n"
    );
    assert!(
        !paths
            .merge_proofs(&plan.plan_id)
            .join("conflicts.json")
            .is_file()
    );
}

#[test]
fn merge_keeps_descendant_when_ancestor_seen_later() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_dependency_readme_conflict(&paths, &repo, true);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# descendant child\n"
    );
}

#[test]
fn merge_parallel_children_still_conflict() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(stderr(&output).contains("merge conflict at README.md"));
}

#[test]
fn merge_prefer_child_requires_child_index_verdict() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--strategy",
            "prefer-child",
            "--quiet",
        ])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(
        err.contains("prefer-child needs --prefer-child <idx>"),
        "{err}"
    );
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {} --strategy prefer-child --prefer-child 1",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_unknown_strategy_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--strategy",
            "surprise",
            "--quiet",
        ])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(
        err.contains("unknown plan merge strategy surprise"),
        "{err}"
    );
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {} --strategy dag-aware",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_unknown_child_index_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--strategy",
            "prefer-child",
            "--prefer-child",
            "99",
            "--quiet",
        ])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(err.contains("unknown child index 99"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon show {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_unknown_repair_mode_uses_one_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-mode",
            "wild",
            "--quiet",
        ])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(err.contains("unknown repair mode wild"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon merge {} --repair-mode auto",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_repair_prefer_child_records_rationale_and_promotes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-prefer",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"prefer_child\",\"rationale\":\"task-1 contains the intended README\",\"actions\":[{\"path\":\"README.md\",\"action\":\"prefer_child\",\"chosen_task_id\":\"task-1\",\"preserve\":[\"README intent\"]}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let mut plan = plan_with_readme_conflict(&paths, &repo);
    let first_run_id = plan.tasks[0].child_run_id.as_deref().expect("first run");
    let first_state = deadreckon_core::load_run(&paths, first_run_id).expect("first state");
    let first_library = paths.library_dir(&first_state.scope, &first_state.run_id);
    let first_before = fs::read_to_string(first_library.join("README.md")).expect("first readme");
    plan.providers.planner = Some("cli:repair-prefer".to_string());
    save_plan(&paths, &plan).expect("save repair provider");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge repair");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    assert_eq!(merged.status, PlanStatus::Merged);
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# preferred child\n"
    );
    assert_eq!(
        fs::read_to_string(first_library.join("README.md")).expect("first readme after"),
        first_before
    );
    let repair_plan: Value = serde_json::from_str(
        &fs::read_to_string(paths.merge_proofs(&plan.plan_id).join("repair-plan.json"))
            .expect("repair plan"),
    )
    .expect("repair plan json");
    assert_eq!(repair_plan["decision"], "prefer_child");
    assert_eq!(
        repair_plan["rationale"],
        "task-1 contains the intended README"
    );
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::MergeRepairPlanned {
            provider: Some(provider),
            ..
        } if provider == "cli:repair-prefer"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::MergeRepaired { strategy, .. } if strategy == "prefer_child"
    )));
}

#[test]
fn merge_repair_synthesizes_only_conflict_paths() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-synthesize",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"combine README requirements\",\"actions\":[{\"path\":\"README.md\",\"action\":\"write_synthesized\",\"content\":\"# synthesized repair\\\\n\",\"preserve\":[\"both child summaries\"]}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-synthesize",
            "--repair-mode",
            "synthesize",
            "--quiet",
        ])
        .output()
        .expect("merge repair");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# synthesized repair\n"
    );
}

#[test]
fn merge_repair_synthesize_rejects_path_traversal() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-bad-path",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"bad path\",\"actions\":[{\"path\":\"../evil.txt\",\"action\":\"write_synthesized\",\"content\":\"bad\"}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-bad-path",
            "--repair-mode",
            "synthesize",
            "--quiet",
        ])
        .output()
        .expect("merge repair");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("unsafe repair path"),
        "{}",
        stderr(&output)
    );
    assert!(
        !paths
            .merge_working(&plan.plan_id)
            .join("../evil.txt")
            .exists()
    );
}

#[test]
fn merge_repair_mode_refuses_unsupported_decision() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-wrong-mode",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"needs synthesis\",\"actions\":[{\"path\":\"README.md\",\"action\":\"write_synthesized\",\"content\":\"# synthesized\\\\n\"}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-wrong-mode",
            "--repair-mode",
            "prefer",
            "--quiet",
        ])
        .output()
        .expect("merge repair");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("not allowed by --repair-mode prefer"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn merge_repair_rejects_unknown_conflict_path() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-unknown-path",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"wrong path\",\"actions\":[{\"path\":\"OTHER.md\",\"action\":\"write_synthesized\",\"content\":\"# other\\\\n\"}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-unknown-path",
            "--quiet",
        ])
        .output()
        .expect("merge repair");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("non-conflict path OTHER.md"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn merge_repair_rejects_unknown_task_id() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-unknown-task",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"prefer_child\",\"rationale\":\"bad task\",\"actions\":[{\"path\":\"README.md\",\"action\":\"prefer_child\",\"chosen_task_id\":\"task-9\"}]}' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-unknown-task",
            "--quiet",
        ])
        .output()
        .expect("merge repair");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("unknown task id task-9"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn merge_repair_rejects_malformed_planner_response() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-malformed",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf 'not json' ;;\n  *) printf 'unexpected repair provider invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-malformed",
            "--quiet",
        ])
        .output()
        .expect("merge repair");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("valid repair JSON"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn merge_repair_child_success_retries_merge_and_promotes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let provider_root = temp.path().join("providers");
    write_fake_cli_subagent_provider(
        &paths,
        &provider_root,
        "cli:repair-child",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"spawn_repair_child\",\"rationale\":\"repair child should integrate README\",\"actions\":[{\"path\":\"README.md\",\"action\":\"repair_child\",\"preserve\":[\"both README meanings\"]}],\"repair_goal\":\"Write the repaired README.\"}' ;;\n  *) printf '# repaired by child\\n' > README.md; printf 'repair child completed\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-child",
            "--repair-mode",
            "child",
            "--quiet",
        ])
        .output()
        .expect("merge repair child");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    assert_eq!(merged.status, PlanStatus::Merged);
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# repaired by child\n"
    );
    let repair_run: Value = serde_json::from_str(
        &fs::read_to_string(paths.merge_proofs(&plan.plan_id).join("repair-run.json"))
            .expect("repair run"),
    )
    .expect("repair run json");
    assert_eq!(repair_run["status"], "completed");
    assert!(repair_run["run_id"].as_str().is_some());
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::MergeRepairRunDiscovered { run_id, .. } if !run_id.is_empty()
    )));
}

#[test]
fn merge_conflict_bundle_is_backward_tolerant() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut plan = plan_with_readme_conflict(&paths, &repo);
    plan.status = PlanStatus::Failed;
    save_plan(&paths, &plan).expect("save failed plan");
    fs::create_dir_all(paths.merge_proofs(&plan.plan_id)).expect("proofs");
    fs::write(
        paths.merge_proofs(&plan.plan_id).join("conflicts.json"),
        r#"[{"path":"README.md","first_child":0,"second_child":1,"chosen_child":null}]"#,
    )
    .expect("old conflicts");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("conflicts.json"), "{out}");
}

#[test]
fn merge_conflict_bundle_skips_generated_artifacts() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    for task in &plan.tasks {
        let run_id = task.child_run_id.as_ref().expect("run id");
        let state = deadreckon_core::load_run(&paths, run_id).expect("state");
        let library = paths.library_dir(&state.scope, &state.run_id);
        fs::create_dir_all(library.join("target")).expect("target");
        fs::write(
            library.join("target/generated.txt"),
            format!("generated by {}", task.task_id),
        )
        .expect("generated");
    }

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");

    assert_success(&output);
    assert!(
        !paths
            .merge_proofs(&plan.plan_id)
            .join("conflicts.json")
            .is_file()
    );
}

#[test]
fn merge_conflict_starts_repair_by_default() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-default",
        r#"{"decision":"prefer_child","rationale":"default repair","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-1"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let mut plan = plan_with_readme_conflict(&paths, &repo);
    plan.providers.planner = Some("cli:repair-default".to_string());
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::MergeRepairStarted { .. }))
    );
}

#[test]
fn merge_no_repair_prints_conflict_without_planner() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("automatic repair disabled"), "{err}");
    assert!(err.contains("conflicts.json"), "{err}");
    assert!(
        !paths
            .merge_proofs(&plan.plan_id)
            .join("repair-plan.json")
            .is_file()
    );
}

#[test]
fn merge_missing_repair_provider_recommends_provider_list_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut plan = plan_with_readme_conflict(&paths, &repo);
    plan.providers.planner = None;
    plan.providers.default_child = None;
    save_plan(&paths, &plan).expect("save plan without repair provider");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("failed plan "), "{err}");
    assert!(
        err.contains("merge repair needs a configured provider"),
        "{err}"
    );
    assert!(err.contains("conflicts remain"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ndeadreckon providers list --all"),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_auto_repair_resolves_provider_from_plan_then_config() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-config-default",
        r#"{"decision":"prefer_child","rationale":"config default repair","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-1"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let mut plan = plan_with_readme_conflict(&paths, &repo);
    plan.providers.planner = None;
    plan.providers.default_child = None;
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");
    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::MergeRepairPlanned {
            provider: Some(provider),
            ..
        } if provider == "cli:repair-config-default"
    )));
}

#[test]
fn merge_repair_request_includes_conflicting_artifact_paths() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let request: Value = serde_json::from_str(
        &fs::read_to_string(
            paths
                .merge_proofs(&plan.plan_id)
                .join("repair-request.json"),
        )
        .expect("request"),
    )
    .expect("request json");
    assert!(
        request["conflicts"][0]["children"][0]["artifact_path"]
            .as_str()
            .expect("artifact path")
            .ends_with("README.md")
    );
}

#[test]
fn merge_repair_request_never_points_outside_plan_or_library_roots() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--no-repair", "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let request: Value = serde_json::from_str(
        &fs::read_to_string(
            paths
                .merge_proofs(&plan.plan_id)
                .join("repair-request.json"),
        )
        .expect("request"),
    )
    .expect("request json");
    let plan_root = paths.plan_dir(&plan.plan_id).display().to_string();
    let home_root = paths.home().display().to_string();
    for value in request["worker_specs"]
        .as_object()
        .expect("worker specs")
        .values()
    {
        assert!(value.as_str().expect("spec").starts_with(&plan_root));
    }
    for child in request["conflicts"][0]["children"]
        .as_array()
        .expect("children")
    {
        assert!(
            child["artifact_path"]
                .as_str()
                .expect("artifact")
                .starts_with(&home_root)
        );
    }
}

#[test]
fn merge_repair_planner_json_roundtrips() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:repair-json-slice",
        "case \"$1\" in\n  *\"read-only merge repair planner\"*) printf 'prefix {\"decision\":\"prefer_child\",\"rationale\":\"json slice\",\"actions\":[{\"path\":\"README.md\",\"action\":\"prefer_child\",\"chosen_task_id\":\"task-1\"}]} suffix' ;;\n  *) printf 'unexpected child invocation\\n' ;;\nesac\n",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-json-slice",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);
}

#[test]
fn merge_repair_prefer_child_does_not_mutate_child_libraries() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-no-mutate",
        r#"{"decision":"prefer_child","rationale":"prefer without mutation","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-1"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);
    let first_run = plan.tasks[0].child_run_id.as_deref().expect("first run");
    let first_state = deadreckon_core::load_run(&paths, first_run).expect("first state");
    let first_library = paths.library_dir(&first_state.scope, &first_state.run_id);
    let before = fs::read_to_string(first_library.join("README.md")).expect("before");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-no-mutate",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);

    assert_eq!(
        fs::read_to_string(first_library.join("README.md")).expect("after"),
        before
    );
}

#[test]
fn show_why_failed_reports_prefer_child_repair_when_it_fails() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-bad-prefer-show",
        r#"{"decision":"prefer_child","rationale":"bad prefer","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-9"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-bad-prefer-show",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("repair request"), "{out}");
    assert!(out.contains("conflicts.json"), "{out}");
}

#[test]
fn merge_repair_synthesize_then_retries_gate_and_promotes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-synth-promote",
        r##"{"decision":"synthesize","rationale":"synthesize and promote","actions":[{"path":"README.md","action":"write_synthesized","content":"# synth promoted\n"}]}"##,
        "printf 'unexpected child invocation\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-synth-promote",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);
    assert_eq!(
        load_plan(&paths, &plan.plan_id).expect("plan").status,
        PlanStatus::Merged
    );
}

#[test]
fn merge_repair_child_runs_from_merge_working() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-child-source",
        r#"{"decision":"spawn_repair_child","rationale":"child sees merge working","actions":[{"path":"README.md","action":"repair_child"}],"repair_goal":"Repair README from merge working."}"#,
        "test -f README.md || { printf 'missing merge working README\\n' >&2; exit 45; }\nprintf '# child saw merge working\\n' > README.md\nprintf 'child repaired\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-child-source",
            "--repair-mode",
            "child",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);
}

#[test]
fn merge_repair_child_records_run_id_and_scope() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-child-record",
        r#"{"decision":"spawn_repair_child","rationale":"record child","actions":[{"path":"README.md","action":"repair_child"}],"repair_goal":"Repair README."}"#,
        "printf '# repaired for record\\n' > README.md\nprintf 'child repaired\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-child-record",
            "--repair-mode",
            "child",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);
    let repair_run: Value = serde_json::from_str(
        &fs::read_to_string(paths.merge_proofs(&plan.plan_id).join("repair-run.json"))
            .expect("repair run"),
    )
    .expect("repair run json");
    assert!(repair_run["run_id"].as_str().is_some());
    assert!(repair_run["scope"].as_str().is_some());
}

#[test]
fn merge_repair_child_failure_preserves_conflict_bundle() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-child-fails",
        r#"{"decision":"spawn_repair_child","rationale":"child fails","actions":[{"path":"README.md","action":"repair_child"}],"repair_goal":"Fail while repairing README."}"#,
        "printf 'repair child failure\\n' >&2\nexit 47",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-child-fails",
            "--repair-mode",
            "child",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        paths
            .merge_proofs(&plan.plan_id)
            .join("conflicts.json")
            .is_file()
    );
}

#[test]
fn attach_plain_plan_shows_merge_repair_status() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-attach",
        r#"{"decision":"prefer_child","rationale":"attach should show this","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-1"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-attach",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &plan.plan_id[..8], "--plain"])
        .output()
        .expect("attach");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("merge repair"), "{out}");
    assert!(out.contains("prefer_child"), "{out}");
}

#[test]
fn show_why_failed_plan_names_repair_run_and_conflict_paths() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-child-fail-show",
        r#"{"decision":"spawn_repair_child","rationale":"show failed child","actions":[{"path":"README.md","action":"repair_child"}],"repair_goal":"Fail while repairing README."}"#,
        "printf 'repair child failure\\n' >&2\nexit 47",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-child-fail-show",
            "--repair-mode",
            "child",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("repair-run.json"), "{out}");
    assert!(out.contains("conflicts.json"), "{out}");
}

#[test]
fn history_grep_plan_finds_merge_repair_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_merge_repair_provider(
        &paths,
        temp.path(),
        "cli:repair-history",
        r#"{"decision":"prefer_child","rationale":"history grep repair","actions":[{"path":"README.md","action":"prefer_child","chosen_task_id":"task-1"}]}"#,
        "printf 'unexpected child invocation\\n'",
    );
    let plan = plan_with_readme_conflict(&paths, &repo);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--repair-provider",
            "cli:repair-history",
            "--quiet",
        ])
        .output()
        .expect("merge");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "history",
            "grep",
            "merge repaired",
            "--plan",
            &plan.plan_id[..8],
        ])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("merge repaired"), "{out}");
}

#[test]
fn orchestrate_interactive_merge_conflict_auto_repairs() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:orchestrate-repair-visible",
        "case \"$1\" in\n  *\"read-only planning agent\"*) printf '{\"tasks\":[{\"subject\":\"Write first README\",\"goal\":\"Write README from task zero\",\"active_form\":\"Writing first README\",\"depends_on\":[]},{\"subject\":\"Write second README\",\"goal\":\"Write README from task one\",\"active_form\":\"Writing second README\",\"depends_on\":[]}]}' ;;\n  *\"read-only merge repair planner\"*) printf '{\"decision\":\"synthesize\",\"rationale\":\"merge both README lanes\",\"actions\":[{\"path\":\"README.md\",\"action\":\"write_synthesized\",\"content\":\"# visible orchestrated repair\\\\n\"}]}' ;;\n  *\"Task: task-0\"*) printf '# task zero\\n' > README.md; printf 'task zero done\\n' ;;\n  *\"Task: task-1\"*) printf '# task one\\n' > README.md; printf 'task one done\\n' ;;\n  *) printf 'unexpected prompt\\n' >&2; exit 44 ;;\nesac\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "full-plan",
            "build conflicting README lanes visibly",
            "--planner-provider",
            "cli:orchestrate-repair-visible",
            "--provider",
            "cli:orchestrate-repair-visible",
            "--n",
            "2",
            "--sandbox",
            "none",
            "--yes",
        ])
        .output()
        .expect("orchestrate");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("merge repair"), "{out}");
    assert_eq!(newest_plan(&paths).status, PlanStatus::Merged);
}

#[test]
fn merge_prefer_child_resolves_conflict() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_with_readme_conflict(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "merge",
            &plan.plan_id[..8],
            "--strategy",
            "prefer-child",
            "--prefer-child",
            "1",
            "--quiet",
        ])
        .output()
        .expect("merge prefer");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert_eq!(
        fs::read_to_string(library.join("README.md")).expect("read merged"),
        "# preferred child\n"
    );
    assert!(
        paths
            .merge_proofs(&plan.plan_id)
            .join("conflicts.json")
            .is_file()
    );
}

#[test]
fn merge_promotes_to_new_library_entry() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    assert!(library.is_dir(), "{}", library.display());
    assert!(library.join("manifest.json").is_file());
    assert!(library.join("deadreckon-plan-manifest.json").is_file());
}

#[test]
fn merge_manifest_records_child_provider_roles() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_smoke_role_aliases(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke:planner",
            "--provider",
            "smoke:default",
            "--child-provider",
            "1=smoke:reviewer",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[1].depends_on = vec![plan.tasks[0].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(library.join("deadreckon-plan-manifest.json")).expect("manifest"),
    )
    .expect("manifest json");

    assert_eq!(manifest["providers"]["planner"], "smoke:planner");
    assert_eq!(manifest["providers"]["default_child"], "smoke:default");
    assert_eq!(manifest["providers"]["children"]["1"], "smoke:reviewer");
    assert_eq!(manifest["tasks"][0]["provider"], "smoke:default");
    assert_eq!(manifest["tasks"][1]["provider"], "smoke:reviewer");
    assert_eq!(manifest["task_graph"][1]["depends_on"][0], "task-0");
    assert_eq!(manifest["summary_paths"]["task-0"], "summaries/task-0.md");
    assert_eq!(manifest["summary_paths"]["task-1"], "summaries/task-1.md");
    assert!(
        manifest["coordinator_messages"]["total"]
            .as_u64()
            .expect("total messages")
            >= 4,
        "{manifest:#}"
    );
    assert!(
        manifest["coordinator_messages"]["by_type"]["progress"]
            .as_u64()
            .expect("progress messages")
            >= 4,
        "{manifest:#}"
    );
}

#[test]
fn merge_manifest_records_task_graph_and_summaries() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.tasks[1].depends_on = vec![plan.tasks[0].task_id.clone()];
    save_plan(&paths, &plan).expect("save dependency");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert_success(&output);

    let merged = load_plan(&paths, &plan.plan_id).expect("merged plan");
    let merged_run_id = merged.merged_run_id.as_deref().expect("merged run");
    let merged_state = deadreckon_core::load_run(&paths, merged_run_id).expect("merged state");
    let library = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(library.join("deadreckon-plan-manifest.json")).expect("manifest"),
    )
    .expect("manifest json");

    assert_eq!(manifest["task_graph"][1]["depends_on"][0], "task-0");
    assert_eq!(manifest["summary_paths"]["task-0"], "summaries/task-0.md");
    assert_eq!(manifest["summary_paths"]["task-1"], "summaries/task-1.md");
    assert!(
        manifest["coordinator_messages"]["total"]
            .as_u64()
            .expect("total messages")
            >= 4,
        "{manifest:#}"
    );
}

#[test]
fn merge_refuses_running_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[0].status = PlanTaskStatus::Running;
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("paused plan "), "{err}");
    assert!(err.contains("child 0 is running"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon attach {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(err.contains("Secondary\n"), "{err}");
    assert!(
        err.contains(&format!("deadreckon kill {}", &plan.plan_id[..8])),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_pending_plan_recommends_fork_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "pending merge verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.status, PlanStatus::Pending);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge pending plan");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("blocked plan "), "{err}");
    assert!(err.contains("is pending"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon fork {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn merge_already_merged_plan_recommends_finish_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "merged merge verdict plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Merged;
    save_plan(&paths, &plan).expect("save merged plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge merged plan");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.starts_with("no-op plan "), "{err}");
    assert!(err.contains("is merged"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "Recommended\ndeadreckon finish {}",
            &plan.plan_id[..8]
        )),
        "{err}"
    );
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn review_mode_runs_coder_then_reviewer_extend() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "tiny hello rust",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--sandbox",
            "none",
            "--yes",
            "--quiet",
        ])
        .output()
        .expect("orchestrate");

    assert_success(&output);
    let plan = newest_plan(&paths);
    assert_eq!(plan.mode, PlanMode::Review);
    assert_eq!(plan.status, PlanStatus::Merged);
    assert!(plan.merged_run_id.is_some());
    let coder_run_id = plan.tasks[0].child_run_id.as_deref().expect("coder run");
    let reviewer_run_id = plan.tasks[1].child_run_id.as_deref().expect("reviewer run");
    let reviewer_state = deadreckon_core::load_run(&paths, reviewer_run_id).expect("reviewer run");
    let reviewer_traces =
        fs::read_to_string(reviewer_state.run_root.join("traces.jsonl")).expect("review traces");
    assert!(
        reviewer_traces.contains("extended_from_parent"),
        "{reviewer_traces}"
    );
    assert!(reviewer_traces.contains(coder_run_id), "{reviewer_traces}");
}

#[test]
fn review_mode_emits_reviewer_extend_run_discovered() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "review",
            "tiny hello rust",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--sandbox",
            "none",
            "--yes",
            "--quiet",
        ])
        .output()
        .expect("orchestrate");
    assert_success(&output);
    let plan = newest_plan(&paths);
    let reviewer_run_id = plan.tasks[1].child_run_id.as_deref().expect("reviewer run");
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::TaskRunDiscovered {
            task_id,
            run_id: Some(run_id),
            ..
        } if task_id == "task-1" && run_id == reviewer_run_id
    )));
}

#[test]
fn review_mode_stops_before_reviewer_when_coder_fails_gate() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: impossible\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/never-created.txt\"\n",
    )
    .expect("acceptance");
    write_fake_cli_subagent_provider(
        &paths,
        temp.path(),
        "cli:review-fixture",
        "printf 'generated once\\n' > coder-output.txt\nprintf 'coder wrote files\\n'\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--mode",
            "review",
            "--coder-provider",
            "cli:review-fixture",
            "--reviewer-provider",
            "cli:review-fixture",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);

    let plan = load_plan(&paths, &plan.plan_id).expect("plan");
    let coder_run_id = plan.tasks[0].child_run_id.as_deref().expect("coder run");
    assert_eq!(plan.tasks[0].status, PlanTaskStatus::Failed);
    assert_eq!(plan.tasks[1].status, PlanTaskStatus::Failed);
    assert!(plan.tasks[1].child_run_id.is_none());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("why failed");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(&format!(
            "Recommended\ndeadreckon show {} --why-failed",
            &coder_run_id[..8]
        )),
        "{out}"
    );
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn attach_and_show_accept_plan_ids() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &plan.plan_id[..8], "--no-hints"])
        .output()
        .expect("attach");
    assert_success(&output);
    let out = stdout(&output);
    assert_verdict_surface(
        &out,
        &format!("preview plan {}", &plan.plan_id[..8]),
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert!(out.contains("plan"), "{out}");
    assert!(out.contains("task-0"), "{out}");
    assert!(!out.contains("\nSecondary\n"), "{out}");
    assert!(!out.contains("attach:"), "{out}");
    assert!(!out.contains("show:"), "{out}");
    assert!(!out.contains("finish:"), "{out}");
    assert!(!out.contains("apply:"), "{out}");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8]])
        .output()
        .expect("show");
    assert_success(&output);
    let out = stdout(&output);
    assert_verdict_surface(
        &out,
        &format!("preview plan {}", &plan.plan_id[..8]),
        &format!("deadreckon fork {}", &plan.plan_id[..8]),
    );
    assert!(out.contains(&plan.plan_id), "{out}");
    assert!(out.contains("\"root_goal\""), "{out}");
    assert!(!out.contains("attach:"), "{out}");
    assert!(!out.contains("show:"), "{out}");
    assert!(!out.contains("finish:"), "{out}");
    assert!(!out.contains("apply:"), "{out}");
}

#[test]
fn plan_plain_summary_lists_child_attach_and_show_commands() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let plan = plan_and_fork_smoke(&paths, &repo);
    let child = plan.tasks[0].child_run_id.as_deref().expect("child run");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &plan.plan_id[..8], "--plain"])
        .output()
        .expect("attach");
    assert_success(&output);
    let out = stdout(&output);

    assert!(
        out.contains(&format!("deadreckon attach {}", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(
        out.contains(&format!("deadreckon attach {}:task-0", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(
        out.contains(&format!("deadreckon show {}:task-0", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(out.contains(&format!("run id {}", &child[..8])), "{out}");
}

#[test]
fn show_why_failed_plan_names_blocking_child() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[0].status = PlanTaskStatus::Failed;
    plan.tasks[0].child_run_id = Some("abc1234567890def".to_string());
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show why failed");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("failed plan "), "{out}");
    assert!(out.contains("Explanation\n"), "{out}");
    assert!(out.contains("Evidence\n"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(out.contains("child 0 task-0 status failed"), "{out}");
    assert!(
        out.contains("Recommended\ndeadreckon show abc12345 --why-failed"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn show_why_failed_completed_says_no_failures() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_test_run(
        &paths,
        &repo,
        "ccccddddccccdddd1111222233334444",
        "completed why failed",
    );
    state.status = RunStatus::Completed;
    save_state(&state).expect("save state");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &state.run_id[..8], "--why-failed"])
        .output()
        .expect("show why failed");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("no-op run ccccdddd"), "{out}");
    assert!(out.contains("Explanation\n"), "{out}");
    assert!(out.contains("Evidence\n"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\ndeadreckon show ccccdddd"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn show_why_failed_failed_emits_rca() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_test_run(
        &paths,
        &repo,
        "ddddccccddddcccc1111222233334444",
        "failed why failed",
    );
    state.status = RunStatus::Failed;
    state.failure_reason = Some("acceptance failed".to_string());
    save_state(&state).expect("save state");
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 3,
            event: "tool.failed".to_string(),
            latency_ms: Some(42),
            detail: json!({
                "tool": "shell",
                "exit_code": 2,
                "stderr": "boom from failing test",
            }),
        },
    )
    .expect("trace");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &state.run_id[..8], "--why-failed"])
        .output()
        .expect("show why failed");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.starts_with("failed run ddddcccc"), "{out}");
    assert!(out.contains("Explanation\n"), "{out}");
    assert!(out.contains("Evidence\n"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\ndeadreckon show ddddcccc --why-failed"),
        "{out}"
    );
    assert!(out.contains("status: failed"), "{out}");
    assert!(out.contains("reason: acceptance failed"), "{out}");
    assert!(out.contains("turn 3 tool.failed"), "{out}");
    assert!(out.contains("boom from failing test"), "{out}");
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn show_why_failed_plan_includes_blocker_message() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[1].status = PlanTaskStatus::Failed;
    save_plan(&paths, &plan).expect("save plan");
    append_plan_message(
        &paths,
        &plan.plan_id,
        &PlanMessage::new(
            "coordinator",
            "task-1",
            PlanMessageKind::Blocker,
            "task-1 blocked by failed dependency",
            json!({ "missing_dependencies": ["task-0"] }),
        )
        .expect("message"),
    )
    .expect("append message");
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "task-1 blocked by failed dependency".to_string(),
        },
    )
    .expect("append plan event");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show why failed");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("child 1 task-1 status failed"), "{out}");
    assert!(
        out.contains("blocker coordinator -> task-1: task-1 blocked by failed dependency"),
        "{out}"
    );
    assert!(
        out.contains("latest plan event") && out.contains("task-1 blocked"),
        "{out}"
    );
}

#[test]
fn show_why_failed_plan_cites_latest_plan_event() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[1].status = PlanTaskStatus::Failed;
    save_plan(&paths, &plan).expect("save plan");
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "latest-event-why-failed".to_string(),
        },
    )
    .expect("append plan event");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8], "--why-failed"])
        .output()
        .expect("show why failed");
    assert_success(&output);
    let out = stdout(&output);

    assert!(out.contains("latest plan event"), "{out}");
    assert!(out.contains("latest-event-why-failed"), "{out}");
}

#[test]
fn history_grep_substring_finds_pattern_across_library() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    seed_trace_run(
        &paths,
        &repo,
        "aaaabbbbccccdddd1111222233334444",
        "first needle-from-history",
    );
    seed_trace_run(
        &paths,
        &repo,
        "bbbbccccddddaaaa1111222233335555",
        "second needle-from-history",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "needle-from-history"])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("aaaabbbb"), "{out}");
    assert!(out.contains("bbbbcccc"), "{out}");
}

#[test]
fn history_grep_no_matches_has_one_primary_recovery_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    seed_trace_run(
        &paths,
        &repo,
        "aaaabbbbccccdddd1111222233334444",
        "trace without the searched phrase",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "missing-history-needle"])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);

    assert!(
        out.contains("no-op history grep missing-history-needle"),
        "{out}"
    );
    assert!(out.contains("Explanation\n"), "{out}");
    assert!(out.contains("Evidence\n"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\ndeadreckon history grep missing-history-needle --all"),
        "{out}"
    );
    assert!(out.contains("Secondary\n"), "{out}");
    assert!(out.contains("deadreckon show <run-id>"), "{out}");
    assert!(!out.contains("hint:"), "{out}");
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn history_grep_invalid_since_has_one_recovery_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "needle", "--since", "soon"])
        .output()
        .expect("history grep");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("blocked history grep --since"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ndeadreckon history grep needle --since 7d"),
        "{err}"
    );
    assert!(err.contains("value: soon"), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn history_grep_scope_and_all_match_list_semantics() {
    let temp = repo_tempdir();
    let repo_a = clean_git_repo(&temp);
    let repo_b = temp.path().join("repo-b");
    fs::create_dir_all(&repo_b).expect("repo b");
    git(&repo_b, &["init", "--initial-branch=main"]).expect("git init b");
    fs::write(repo_b.join("README.md"), "hello b").expect("readme b");
    git(&repo_b, &["add", "-A"]).expect("add b");
    git(&repo_b, &["commit", "-m", "initial b"]).expect("commit b");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    seed_trace_run(
        &paths,
        &repo_a,
        "aaaa1111ccccdddd1111222233334444",
        "scope-history-needle repo a",
    );
    seed_trace_run(
        &paths,
        &repo_b,
        "bbbb2222eeeeffff5555666677778888",
        "scope-history-needle repo b",
    );
    let scope_b = deadreckon_core::paths::workspace_scope(&repo_b).expect("scope b");

    let scoped_default = deadreckon(&paths)
        .current_dir(&repo_a)
        .args(["history", "grep", "scope-history-needle"])
        .output()
        .expect("history grep scoped");
    assert_success(&scoped_default);
    let out = stdout(&scoped_default);
    assert!(out.contains("aaaa1111"), "{out}");
    assert!(!out.contains("bbbb2222"), "{out}");

    let explicit_scope = deadreckon(&paths)
        .current_dir(&repo_a)
        .args([
            "history",
            "grep",
            "scope-history-needle",
            "--scope",
            &scope_b,
        ])
        .output()
        .expect("history grep scope");
    assert_success(&explicit_scope);
    let out = stdout(&explicit_scope);
    assert!(!out.contains("aaaa1111"), "{out}");
    assert!(out.contains("bbbb2222"), "{out}");

    let all_scopes = deadreckon(&paths)
        .current_dir(&repo_a)
        .args(["history", "grep", "scope-history-needle", "--all"])
        .output()
        .expect("history grep all");
    assert_success(&all_scopes);
    let out = stdout(&all_scopes);
    assert!(out.contains("aaaa1111"), "{out}");
    assert!(out.contains("bbbb2222"), "{out}");
}

#[test]
fn history_grep_plan_scope_excludes_others() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let child = "11112222333344445555666677778888";
    let unrelated = "99998888777766665555444433332222";
    seed_trace_run(&paths, &repo, child, "shared-plan-needle child");
    seed_trace_run(&paths, &repo, unrelated, "shared-plan-needle unrelated");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.status = PlanStatus::Forked;
    plan.tasks[0].child_run_id = Some(child.to_string());
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "history",
            "grep",
            "shared-plan-needle",
            "--plan",
            &plan.plan_id[..8],
        ])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("11112222"), "{out}");
    assert!(!out.contains("99998888"), "{out}");
}

#[test]
fn history_grep_plan_scope_includes_plan_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "event-only-plan-needle".to_string(),
        },
    )
    .expect("append plan event");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "history",
            "grep",
            "event-only-plan-needle",
            "--plan",
            &plan.plan_id[..8],
        ])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("plan-events"), "{out}");
    assert!(out.contains("event-only-plan-needle"), "{out}");
}

#[test]
fn history_grep_plan_searches_plan_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(&paths);
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::TaskBlocked {
            task_id: "task-1".to_string(),
            task_index: 1,
            reason: "exact-plan-event-needle".to_string(),
        },
    )
    .expect("append plan event");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "history",
            "grep",
            "exact-plan-event-needle",
            "--plan",
            &plan.plan_id[..8],
        ])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);

    assert!(out.contains("plan-events"), "{out}");
    assert!(out.contains("exact-plan-event-needle"), "{out}");
}

#[test]
fn history_grep_regex_invalid_pattern_errors() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "[", "--regex"])
        .output()
        .expect("history grep");
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("blocked history grep --regex"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ndeadreckon history grep '[' --regex"),
        "{err}"
    );
    assert!(err.contains("pattern: ["), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn history_grep_invalid_limit_has_one_recovery_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "needle", "--limit", "0"])
        .output()
        .expect("history grep");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("blocked history grep --limit"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ndeadreckon history grep needle --limit 20"),
        "{err}"
    );
    assert!(err.contains("value: 0"), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn history_grep_limit_respected() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_test_run(
        &paths,
        &repo,
        "abcdabcdabcdabcd1111222233334444",
        "history limit",
    );
    for index in 0..20 {
        append_trace(
            &state,
            &TraceRecord {
                timestamp: Utc::now(),
                run_id: state.run_id.clone(),
                turn: index + 1,
                event: "limit-check".to_string(),
                latency_ms: None,
                detail: json!({ "message": format!("limit-needle-{index}") }),
            },
        )
        .expect("trace");
    }

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["history", "grep", "limit-needle", "--limit", "5"])
        .output()
        .expect("history grep");
    assert_success(&output);
    let out = stdout(&output);
    assert_eq!(out.matches("limit-needle").count(), 5, "{out}");
    assert!(out.contains("... (15 more)"), "{out}");
}

#[test]
fn run_extend_resume_chain_event_paths_unchanged() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_test_run(
        &paths,
        &repo,
        "eeeeffffeeeeffff1111222233334444",
        "event path check",
    );

    assert_eq!(RUN_EVENTS_JSONL, "events.jsonl");
    assert_eq!(CHAIN_EVENTS_JSONL, "chain-events.jsonl");
    assert_eq!(PLAN_EVENTS_JSONL, "plan-events.jsonl");
    assert!(
        state
            .run_root
            .join(RUN_EVENTS_JSONL)
            .ends_with("events.jsonl")
    );
    assert!(
        paths
            .chain_dir("chain-unchanged")
            .join(CHAIN_EVENTS_JSONL)
            .ends_with("chain-events.jsonl")
    );
    assert!(
        paths
            .plan_events("plan-unchanged")
            .ends_with("plan-events.jsonl")
    );
}

fn plan_and_fork_smoke(paths: &DeadreckonPaths, repo: &std::path::Path) -> Plan {
    let output = deadreckon(paths)
        .current_dir(repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(paths);
    let output = deadreckon(paths)
        .current_dir(repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);
    load_plan(paths, &plan.plan_id).expect("forked plan")
}

fn write_plan_child_marker(
    paths: &DeadreckonPaths,
    plan: &Plan,
    child_index: usize,
    name: &str,
    body: &str,
) {
    let task = plan.tasks.get(child_index).expect("child task");
    let run_id = task.child_run_id.as_deref().expect("child run id");
    let child = load_run(paths, run_id).expect("child run");
    let library = paths.library_dir(&child.scope, &child.run_id);
    fs::write(library.join(name), body).expect("child marker");
}

fn review_gate_failure_plan(
    paths: &DeadreckonPaths,
    repo: &std::path::Path,
    provider_root: &std::path::Path,
) -> Plan {
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: impossible\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/never-created.txt\"\n",
    )
    .expect("acceptance");
    write_fake_cli_subagent_provider(
        paths,
        provider_root,
        "cli:review-fixture",
        "printf 'generated once\\n' > coder-output.txt\nprintf 'coder wrote files\\n'\n",
    );

    let output = deadreckon(paths)
        .current_dir(repo)
        .args([
            "plan",
            "tiny hello rust",
            "--mode",
            "review",
            "--coder-provider",
            "cli:review-fixture",
            "--reviewer-provider",
            "cli:review-fixture",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(paths);
    let output = deadreckon(paths)
        .current_dir(repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);
    load_plan(paths, &plan.plan_id).expect("failed plan")
}

fn kill_live_plan(
    paths: &DeadreckonPaths,
    repo: &std::path::Path,
    provider_root: &std::path::Path,
) -> Plan {
    write_fake_cli_subagent_provider(
        paths,
        provider_root,
        "cli:slow-kill-child",
        "sleep 10\nprintf 'changed by slow child\\n' > slow-child.txt\nprintf 'slow child done\\n'\n",
    );

    let output = deadreckon(paths)
        .current_dir(repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "cli:slow-kill-child",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let plan = newest_plan(paths);
    let mut fork = deadreckon(paths)
        .current_dir(repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .spawn()
        .expect("fork spawn");

    let _pids = wait_for_plan_child_pids(paths, &plan.plan_id);
    let output = deadreckon(paths)
        .current_dir(repo)
        .args(["kill", &plan.plan_id[..8], "--force"])
        .output()
        .expect("kill plan");
    assert_success(&output);
    let _ = fork.wait();
    load_plan(paths, &plan.plan_id).expect("killed plan")
}

fn event_position(events: &[PlanEvent], predicate: impl Fn(&PlanEventKind) -> bool) -> usize {
    events
        .iter()
        .position(|event| predicate(&event.event))
        .unwrap_or_else(|| panic!("missing event in {events:#?}"))
}

fn plan_with_readme_conflict(paths: &DeadreckonPaths, repo: &std::path::Path) -> Plan {
    let plan = plan_and_fork_smoke(paths, repo);
    let second = &plan.tasks[1];
    let second_run = second.child_run_id.as_ref().expect("second run");
    let second_state = deadreckon_core::load_run(paths, second_run).expect("second state");
    let second_library = paths.library_dir(&second_state.scope, &second_state.run_id);
    fs::write(second_library.join("README.md"), "# preferred child\n").expect("conflict write");
    plan
}

fn plan_with_dependency_readme_conflict(
    paths: &DeadreckonPaths,
    repo: &std::path::Path,
    reverse_task_order_after_fork: bool,
) -> Plan {
    let output = deadreckon(paths)
        .current_dir(repo)
        .args([
            "plan",
            "tiny hello rust",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(paths);
    plan.tasks[1].depends_on = vec![plan.tasks[0].task_id.clone()];
    save_plan(paths, &plan).expect("save dependency");

    let output = deadreckon(paths)
        .current_dir(repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
        .output()
        .expect("fork");
    assert_success(&output);
    let mut plan = load_plan(paths, &plan.plan_id).expect("forked plan");
    for task in &plan.tasks {
        let run_id = task.child_run_id.as_ref().expect("run id");
        let state = deadreckon_core::load_run(paths, run_id).expect("state");
        let library = paths.library_dir(&state.scope, &state.run_id);
        let content = if task.task_id == "task-0" {
            "# ancestor child\n"
        } else {
            "# descendant child\n"
        };
        fs::write(library.join("README.md"), content).expect("write readme");
    }
    if reverse_task_order_after_fork {
        plan.tasks.swap(0, 1);
        save_plan(paths, &plan).expect("save reversed plan");
    }
    plan
}

fn wait_for_plan_child_pids(paths: &DeadreckonPaths, plan_id: &str) -> Vec<u32> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Ok(raw) = fs::read_to_string(paths.coordinator_json(plan_id))
            && let Ok(coordinator) = serde_json::from_str::<CoordinatorState>(&raw)
        {
            let pids = coordinator
                .children
                .iter()
                .filter_map(|child| child.pid)
                .filter(|pid| pid_is_alive(*pid))
                .collect::<Vec<_>>();
            if !pids.is_empty() && plan_child_runs_exist(paths, plan_id) {
                return pids;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("no live child pids recorded for plan {plan_id}");
}

fn plan_child_runs_exist(paths: &DeadreckonPaths, plan_id: &str) -> bool {
    let Ok(plan) = load_plan(paths, plan_id) else {
        return false;
    };
    plan.tasks.iter().all(|task| {
        let launch_dir = paths.plan_dir(plan_id).join("launch").join(&task.task_id);
        if let Ok(run_id) = fs::read_to_string(launch_dir.join("run-id"))
            && deadreckon_core::load_run(paths, run_id.trim()).is_ok()
        {
            return true;
        }
        let Ok(scope) = deadreckon_core::paths::workspace_scope(&launch_dir) else {
            return false;
        };
        list_runs(paths, Some(scope.as_str()))
            .map(|runs| !runs.is_empty())
            .unwrap_or(false)
    })
}

fn write_fake_planner_provider(
    paths: &DeadreckonPaths,
    root: &std::path::Path,
    id: &str,
    capture: &std::path::Path,
    response_json: &str,
) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::create_dir_all(root).expect("provider root");
    let providers_dir = paths.home().join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers dir");
    let binary = root.join("fake-planner");
    let response = root.join("fake-planner-response.json");
    fs::write(&response, response_json).expect("response");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\n{{\n  for arg in \"$@\"; do\n    printf '%s\\n' \"$arg\"\n  done\n}} > '{}'\ncat '{}'\n",
            capture.display(),
            response.display()
        ),
    )
    .expect("fake planner");
    let mut perms = fs::metadata(&binary).expect("fake metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&binary, perms).expect("fake chmod");
    let descriptor = format!(
        r#"
id = "{id}"
display_name = "Planner Fixture"
kind = "cli"
default_binary = "{binary}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{{prompt}}"]
"#,
        binary = binary.display()
    );
    fs::write(providers_dir.join("planner-fixture.toml"), descriptor).expect("descriptor");
    fs::write(
        paths.config_path(),
        format!(
            r#"
default_provider = "{id}"
fallback = ["{id}"]

[providers."{id}"]
binary = "{binary}"
"#,
            binary = binary.display()
        ),
    )
    .expect("config");
}

fn write_fake_cli_subagent_provider(
    paths: &DeadreckonPaths,
    root: &std::path::Path,
    id: &str,
    script_body: &str,
) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::create_dir_all(root).expect("provider root");
    let providers_dir = paths.home().join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers dir");
    let slug = id.replace(':', "-");
    let binary = root.join(format!("{slug}.sh"));
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
write_deadreckon_notes() {{
  stamp=$(date +%s%N 2>/dev/null || date +%s)
  cat > implementation-notes.html <<HTML
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>Fixture updated implementation notes at $stamp.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>Orchestration fixtures keep notes current for child and repair runs.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
HTML
}}
{script_body}
case "$1" in
  *"read-only planning agent"*|*"read-only merge repair planner"*|*"read-only launch planner"*) ;;
  *) write_deadreckon_notes ;;
esac
"#
        ),
    )
    .expect("fake cli");
    let mut perms = fs::metadata(&binary).expect("fake metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&binary, perms).expect("fake chmod");
    let descriptor = format!(
        r#"
id = "{id}"
display_name = "Fake CLI Subagent"
kind = "cli"
default_binary = "{binary}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{{prompt}}"]
"#,
        binary = binary.display()
    );
    fs::write(providers_dir.join(format!("{slug}.toml")), descriptor).expect("descriptor");
    fs::write(
        paths.config_path(),
        format!(
            r#"
default_provider = "{id}"
fallback = ["{id}"]

[providers."{id}"]
binary = "{binary}"
"#,
            binary = binary.display()
        ),
    )
    .expect("config");
}

fn write_fake_path_binary(root: &std::path::Path, name: &str, body: &str) {
    fs::create_dir_all(root).expect("fake binary root");
    let binary = root.join(name);
    fs::write(&binary, format!("#!/bin/sh\n{body}")).expect("fake binary");
    let mut perms = fs::metadata(&binary)
        .expect("fake binary metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&binary, perms).expect("fake binary chmod");
}

// ---- C-P9: dispatch reads the plan ----

fn read_launch_plan(root: &std::path::Path) -> serde_json::Value {
    let raw = fs::read_to_string(root.join("launch-plan.json"))
        .unwrap_or_else(|err| panic!("launch-plan.json missing in {}: {err}", root.display()));
    serde_json::from_str(&raw).expect("launch plan parses")
}

fn newest_run_root(paths: &DeadreckonPaths) -> std::path::PathBuf {
    let mut runs = deadreckon_core::list_runs(paths, None).expect("list runs");
    runs.sort_by_key(|entry| entry.updated_at);
    let last = runs.last().expect("at least one run");
    last.state_path.parent().expect("run root").to_path_buf()
}

fn job_roots(paths: &DeadreckonPaths) -> Vec<std::path::PathBuf> {
    let mut jobs = fs::read_dir(paths.jobs_dir())
        .expect("jobs directory")
        .map(|entry| entry.expect("job entry").path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    jobs.sort();
    jobs
}

fn only_job_root(paths: &DeadreckonPaths) -> std::path::PathBuf {
    let jobs = job_roots(paths);
    assert_eq!(jobs.len(), 1, "expected exactly one durable Job");
    jobs[0].clone()
}

fn job_ids(paths: &DeadreckonPaths) -> BTreeSet<String> {
    job_roots(paths)
        .into_iter()
        .map(|root| {
            root.file_name()
                .and_then(|name| name.to_str())
                .expect("utf8 job id")
                .to_string()
        })
        .collect()
}

fn new_replayed_graph_job_root(
    paths: &DeadreckonPaths,
    before: &BTreeSet<String>,
) -> std::path::PathBuf {
    let new = job_roots(paths)
        .into_iter()
        .filter(|root| {
            let is_new = root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !before.contains(name));
            if !is_new {
                return false;
            }
            let job: Value = fs::read(root.join("job.json"))
                .ok()
                .and_then(|raw| serde_json::from_slice(&raw).ok())
                .unwrap_or(Value::Null);
            if job["shape"] != "graph" {
                return false;
            }
            let launch = read_launch_plan(root);
            launch["resolution"]["source"] == "replay"
        })
        .collect::<Vec<_>>();
    assert_eq!(new.len(), 1, "expected exactly one new replayed Graph Job");
    new[0].clone()
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn accepted_plan_lands_in_job_root_before_work() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args(["start", "plan lands in run root", "--yes", "--plain"])
        .output()
        .expect("start run");

    assert_success(&output);
    let plan = read_launch_plan(&only_job_root(&paths));
    assert_eq!(plan["schema"], 1, "{plan}");
    assert_eq!(plan["shape"], "single", "{plan}");
    assert_eq!(plan["goal"], "plan lands in run root", "{plan}");
    assert_eq!(plan["accepted_by"], "yes-flag-guardrail", "{plan}");
    assert!(plan["resolution"]["rationale"].as_str().is_some(), "{plan}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn plan_shape_dispatches_graph_job_with_planned_n() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "planned shape goal",
            "--mode",
            "full-plan",
            "--children",
            "3",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("start full-plan");

    assert_success(&output);
    let root = only_job_root(&paths);
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    let plan = read_launch_plan(&root);
    assert_eq!(job["shape"], "graph", "{job}");
    assert_eq!(plan["shape"], "plan", "{plan}");
    assert_eq!(plan["n"], 3, "{plan}");
    assert_eq!(
        plan["signals"]["watchkeeper_driver"]["child_count"], 3,
        "{plan}"
    );
    assert_eq!(
        plan["signals"]["watchkeeper_driver"]["kind"], "full_plan",
        "{plan}"
    );
}

#[test]
fn direct_run_writes_trivial_operator_plan() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "direct verb plan record",
            "--smoke",
            "--sandbox",
            "none",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("direct run");

    assert_success(&output);
    let plan = read_launch_plan(&newest_run_root(&paths));
    assert_eq!(plan["shape"], "single", "{plan}");
    assert_eq!(plan["accepted_by"], "operator", "{plan}");
    assert_eq!(plan["resolution"]["source"], "operator", "{plan}");
    assert!(
        plan["resolution"]["rationale"]
            .as_str()
            .unwrap_or_default()
            .contains("direct run verb"),
        "{plan}"
    );
}

// ---- C-P12: reshape proposals ----

fn smoke_run_with_proposal(paths: &DeadreckonPaths, repo: &std::path::Path) -> String {
    let output = deadreckon(paths)
        .current_dir(repo)
        .args([
            "run",
            "parent run for reshape",
            "--smoke",
            "--sandbox",
            "none",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("smoke parent run");
    assert_success(&output);
    let root = newest_run_root(paths);
    let run_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run id")
        .to_string();
    fs::write(
        root.join("reshape-proposal.json"),
        serde_json::json!({
            "schema": 1,
            "created_at": "2026-07-02T00:00:00Z",
            "goal": "parent run for reshape",
            "shape": "plan",
            "n": 2,
            "pieces": [
                { "id": "p1", "goal": "piece a" },
                { "id": "p2", "goal": "piece b" }
            ],
            "resolution": {
                "source": "provider",
                "confidence": 0.6,
                "rationale": "worker proposed decomposition at turn 2"
            },
            "parent": run_id
        })
        .to_string(),
    )
    .expect("write proposal");
    run_id
}

fn plan_dir_count(paths: &DeadreckonPaths) -> usize {
    fs::read_dir(paths.plans_dir())
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[test]
fn reshape_verb_previews_proposal_card() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let run_id = smoke_run_with_proposal(&paths, &repo);

    // Non-TTY without --yes: the card previews, then acceptance refuses.
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["reshape", &run_id, "--plain"])
        .output()
        .expect("reshape preview");

    assert!(!output.status.success(), "{}", stdout(&output));
    let out = stdout(&output);
    assert!(out.contains("course - plot, preview, sail"), "{out}");
    assert!(out.contains("piece a"), "{out}");
    let err = stderr(&output);
    assert!(err.contains("explicit acceptance"), "{err}");
    assert!(
        err.contains(&format!("deadreckon reshape {run_id} --yes")),
        "{err}"
    );
}

#[test]
fn proposal_never_executes_without_accept() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let run_id = smoke_run_with_proposal(&paths, &repo);

    let before = plan_dir_count(&paths);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["reshape", &run_id, "--plain"])
        .output()
        .expect("reshape refusal");
    assert!(!output.status.success());
    assert_eq!(
        plan_dir_count(&paths),
        before,
        "a refused proposal must dispatch nothing"
    );

    // A run without a proposal refuses with the status hint.
    let bare = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "run without proposal",
            "--smoke",
            "--sandbox",
            "none",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("bare run");
    assert_success(&bare);
    let bare_root = newest_run_root(&paths);
    let bare_id = bare_root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("id")
        .to_string();
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["reshape", &bare_id, "--plain", "--yes"])
        .output()
        .expect("no proposal");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("has no reshape proposal"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn accepted_reshape_dispatches_plan_with_parent_lineage() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let run_id = smoke_run_with_proposal(&paths, &repo);

    let output = deadreckon_binary(&paths)
        .current_dir(&repo)
        .args(["reshape", &run_id, "--yes", "--plain", "--quiet"])
        .output()
        .expect("reshape accept");
    assert_success(&output);

    let root = only_job_root(&paths);
    let job: Value =
        serde_json::from_slice(&fs::read(root.join("job.json")).expect("job")).expect("job json");
    assert_eq!(job["goal"], "parent run for reshape", "{job}");
    assert_eq!(job["shape"], "graph", "{job}");
    let record = read_launch_plan(&root);
    assert_eq!(record["parent"], run_id.as_str(), "{record}");
    assert_eq!(record["accepted_by"], "operator", "{record}");
    assert_eq!(record["shape"], "plan", "{record}");
    assert_eq!(record["n"], 2, "{record}");
    assert_eq!(
        record["pieces"].as_array().map(Vec::len),
        Some(2),
        "{record}"
    );
}

// ---- C-P10: replay + launch JSON parity ----

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_plan_replays_identical_shape_and_pieces() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = SupervisorServiceFixture::configured(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "replayable goal",
            "--mode",
            "full-plan",
            "--children",
            "3",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("first launch");
    assert_success(&output);
    let first = only_job_root(&paths);
    let first_id = first
        .file_name()
        .and_then(|name| name.to_str())
        .expect("first job id")
        .to_string();
    let first_record = read_launch_plan(&first);
    let saved = temp.path().join("saved-launch-plan.json");
    fs::copy(first.join("launch-plan.json"), &saved).expect("copy plan");
    let before = job_ids(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "--plan",
            saved.to_str().expect("utf8"),
            "--yes",
            "--plain",
        ])
        .output()
        .expect("replay");
    assert_success(&output);

    let replayed = new_replayed_graph_job_root(&paths, &before);
    let replayed_id = replayed
        .file_name()
        .and_then(|name| name.to_str())
        .expect("replayed job id");
    assert_ne!(replayed_id, first_id, "a new Job launched");
    let job: Value = serde_json::from_slice(&fs::read(replayed.join("job.json")).expect("job"))
        .expect("job json");
    let record = read_launch_plan(&replayed);
    assert_eq!(job["shape"], "graph", "{job}");
    assert_eq!(record["shape"], "plan", "{record}");
    assert_eq!(record["n"], 3, "{record}");
    assert_eq!(record["pieces"], first_record["pieces"], "{record}");
    assert_eq!(record["goal"], "replayable goal", "{record}");
    assert_eq!(record["resolution"]["source"], "replay", "{record}");
    assert_eq!(record["accepted_by"], "replay", "{record}");
    assert_eq!(
        record["signals"]["watchkeeper_driver"]["child_count"], 3,
        "{record}"
    );
}

#[test]
fn public_start_without_supervisor_service_refuses_before_job_creation() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = SupervisorServiceFixture::unconfigured(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "refuse an unattended launch without restart recovery",
            "--mode",
            "run",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("unconfigured public start");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(
        err.contains("durable start requires a restart-capable supervisor service"),
        "{err}"
    );
    assert!(err.contains("deadreckon setup --supervisor"), "{err}");
    assert!(
        !paths.jobs_dir().exists(),
        "service refusal allocated durable Job state"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn configured_supervisor_fixture_allows_public_start_launch() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = SupervisorServiceFixture::configured(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "launch with proven machine restart supervision",
            "--mode",
            "run",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("configured public start");

    assert_success(&output);
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    assert!(paths.job_dir(job_id).is_dir(), "{envelope}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_returns_job_id_before_worker_finishes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let barrier = SmokeCargoBarrier::install(&repo);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    write_bounded_start_config(&paths);
    let service = SupervisorServiceFixture::configured(&paths);

    let mut launch = service.deadreckon();
    launch
        .current_dir(&repo)
        .args([
            "start",
            "return durable identity while smoke work remains live",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let launch = wait_with_output_before(
        launch.spawn().expect("spawn public start"),
        // The smoke worker remains blocked until this test releases it, so a
        // longer process-start allowance does not weaken the detach proof.
        // Keep it below the worker's 60-second wall cap: a coupled `start`
        // would still time out here instead of returning after that cap.
        Duration::from_secs(30),
        "public start did not detach from its worker",
    );

    assert_success(&launch);
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    assert!(paths.job_dir(job_id).is_dir(), "{envelope}");

    let state = barrier.wait_until_started(&paths, job_id);
    assert!(
        !barrier.finished(&state),
        "start returned only after the worker finished"
    );
    let live = JobView::load(&paths, job_id).expect("live Job view");
    assert!(
        !live.projection.is_terminal(),
        "start returned a terminal Job instead of detached live work: {live:?}"
    );

    barrier.release(&state);
    let terminal = wait_for_terminal_job(&paths, job_id, Duration::from_secs(60));
    assert!(barrier.finished(&state), "smoke worker did not finish");
    assert_eq!(barrier.invocation_count(&state), 1);
    assert_ne!(terminal.projection.outcome, Some(JobOutcome::Cancelled));
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn attach_disconnect_does_not_cancel_execution() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let barrier = SmokeCargoBarrier::install(&repo);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    write_bounded_start_config(&paths);
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "keep smoke execution alive after its observer disconnects",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert_success(&launch);
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    let state = barrier.wait_until_started(&paths, job_id);

    let attach = service
        .deadreckon()
        .current_dir(&repo)
        .args(["attach", job_id, "--plain"])
        .output()
        .expect("plain attach snapshot");
    assert_success(&attach);
    assert!(
        !barrier.finished(&state),
        "attach remained coupled to the worker until it finished"
    );
    let live = JobView::load(&paths, job_id).expect("Job after attach disconnect");
    assert!(
        !live.projection.is_terminal(),
        "disconnecting attach made the live Job terminal: {live:?}"
    );
    assert_no_cancellation(&paths, job_id);

    barrier.release(&state);
    // The approved worker wall cap is 60 seconds; allow a second cap-length
    // window for the detached supervisor to persist its terminal projection
    // when the host is under executable-scanning or parallel-test pressure.
    let terminal = wait_for_terminal_job(&paths, job_id, Duration::from_secs(120));
    assert!(barrier.finished(&state), "smoke worker did not finish");
    assert_eq!(barrier.invocation_count(&state), 1);
    assert_ne!(terminal.projection.outcome, Some(JobOutcome::Cancelled));
    assert_no_cancellation(&paths, job_id);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn two_supervisor_processes_racing_execute_smoke_worker_once() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let barrier = SmokeCargoBarrier::install(&repo);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    write_bounded_start_config(&paths);
    let service = SupervisorServiceFixture::configured_with_env(
        &paths,
        &[("DEADRECKON_BOOT_ID", "watchkeeper-race-before")],
    );

    // Leave a real queued Job behind a crashed lazy supervisor. No worker has
    // been released yet, so the two explicit processes below are the only
    // contenders capable of producing the durable smoke side effect.
    let launch = service
        .deadreckon()
        .current_dir(&repo)
        .env("DEADRECKON_TEST_SUPERVISOR_FAILPOINTS", "1")
        .env(
            "DEADRECKON_TEST_SUPERVISOR_FAILPOINT",
            "after_launch_prepared",
        )
        .args([
            "start",
            "execute one smoke worker under two racing supervisors",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert_success(&launch);
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    wait_for_job_event(&paths, job_id, JobEventKind::ChildLaunchPrepared);
    wait_for_failed_job_supervisor(&paths, job_id);

    let spawn_contender = || {
        let mut command = service.deadreckon();
        command
            .current_dir(&repo)
            .env("DEADRECKON_BOOT_ID", "watchkeeper-race-after")
            .args(["supervisor", "serve", "--once", job_id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().expect("spawn racing supervisor")
    };
    let mut first = spawn_contender();
    let mut second = spawn_contender();

    let state = barrier.wait_until_started(&paths, job_id);
    wait_for_one_process_to_lose(&mut first, &mut second, Duration::from_secs(10));
    assert_eq!(
        barrier.invocation_count(&state),
        1,
        "both supervisors released a smoke worker"
    );

    barrier.release(&state);
    let first = wait_with_output_before(
        first,
        Duration::from_secs(60),
        "first racing supervisor did not exit",
    );
    let second = wait_with_output_before(
        second,
        Duration::from_secs(60),
        "second racing supervisor did not exit",
    );
    let outputs = [&first, &second];
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1,
        "exactly one supervisor must own the live lease\nfirst: {}{}\nsecond: {}{}",
        stdout(&first),
        stderr(&first),
        stdout(&second),
        stderr(&second),
    );
    assert!(
        outputs
            .iter()
            .any(|output| stderr(output).contains("live lease held by owner")),
        "the losing process did not report the fenced lease\nfirst: {}\nsecond: {}",
        stderr(&first),
        stderr(&second),
    );

    let terminal = wait_for_terminal_job(&paths, job_id, Duration::from_secs(60));
    assert!(terminal.projection.is_terminal(), "{terminal:?}");
    assert_eq!(barrier.invocation_count(&state), 1);
    let history = read_job_history(&paths.job_events(job_id)).expect("Job history");
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::AttemptStarted)
            .count(),
        1,
        "the lease race consumed more than one logical attempt"
    );
}

struct SmokeCargoBarrier;

impl SmokeCargoBarrier {
    const FINISHED: &'static str = ".deadreckon-test-smoke-finished";
    const INVOCATIONS: &'static str = ".deadreckon-test-smoke-invocations";
    const RELEASE: &'static str = ".deadreckon-test-smoke-release";
    const STARTED: &'static str = ".deadreckon-test-smoke-started";

    fn install(repo: &std::path::Path) -> Self {
        let cargo_dir = repo.join(".cargo");
        fs::create_dir_all(&cargo_dir).expect("smoke Cargo config directory");
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\nrustc-wrapper = \"./deadreckon-test-rustc-wrapper\"\n",
        )
        .expect("smoke Cargo wrapper config");
        let wrapper = repo.join("deadreckon-test-rustc-wrapper");
        fs::write(
            &wrapper,
            format!(
                r#"#!/bin/sh
set -eu
claim=".deadreckon-test-smoke-cargo-$PPID"
if mkdir "$claim" 2>/dev/null; then
  printf '%s\n' "$PPID" >> {invocations}
  : > {started}
  while [ ! -f {release} ]; do
    sleep 0.02
  done
  : > {finished}
fi
exec "$@"
"#,
                invocations = Self::INVOCATIONS,
                started = Self::STARTED,
                release = Self::RELEASE,
                finished = Self::FINISHED,
            ),
        )
        .expect("smoke Cargo wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
            .expect("smoke Cargo wrapper executable");
        git(
            repo,
            &["add", ".cargo/config.toml", "deadreckon-test-rustc-wrapper"],
        )
        .expect("stage smoke Cargo barrier");
        git(repo, &["commit", "-m", "add smoke execution barrier"])
            .expect("commit smoke Cargo barrier");
        Self
    }

    fn wait_until_started(
        &self,
        paths: &DeadreckonPaths,
        job_id: &str,
    ) -> deadreckon_core::PipelineState {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(state) = load_run(paths, job_id)
                && state.working_dir.join(Self::STARTED).is_file()
            {
                return state;
            }
            if Instant::now() >= deadline {
                let view = JobView::load(paths, job_id);
                let state = load_run(paths, job_id);
                let events = fs::read_to_string(paths.job_events(job_id));
                panic!(
                    "smoke Job {job_id} never reached its durable worker barrier\nview: {view:#?}\nstate: {state:#?}\nevents: {events:#?}"
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn release(&self, state: &deadreckon_core::PipelineState) {
        fs::write(state.working_dir.join(Self::RELEASE), "release\n")
            .expect("release smoke Cargo barrier");
    }

    fn finished(&self, state: &deadreckon_core::PipelineState) -> bool {
        state.working_dir.join(Self::FINISHED).is_file()
    }

    fn invocation_count(&self, state: &deadreckon_core::PipelineState) -> usize {
        fs::read_to_string(state.working_dir.join(Self::INVOCATIONS))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

fn write_bounded_start_config(paths: &DeadreckonPaths) {
    fs::write(
        paths.config_path(),
        concat!(
            "default_provider = \"smoke\"\n",
            "\n[defaults]\n",
            "cli_max_wall_seconds = 60\n",
        ),
    )
    .expect("bounded start config");
}

fn wait_with_output_before(
    mut child: Child,
    timeout: Duration,
    timeout_message: &str,
) -> std::process::Output {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll child process").is_some() {
            return child.wait_with_output().expect("collect child output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("collect timed-out output");
            panic!(
                "{timeout_message}\nstdout:\n{}\nstderr:\n{}",
                stdout(&output),
                stderr(&output)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_terminal_job(paths: &DeadreckonPaths, job_id: &str, timeout: Duration) -> JobView {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            return view;
        }
        assert!(
            Instant::now() < deadline,
            "Job {job_id} did not become terminal"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_job_event(paths: &DeadreckonPaths, job_id: &str, kind: JobEventKind) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(history) = read_job_history(&paths.job_events(job_id))
            && history.events().iter().any(|event| event.kind == kind)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Job {job_id} never persisted {kind:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_failed_job_supervisor(paths: &DeadreckonPaths, job_id: &str) {
    let lease = load_job_lease(paths, &JobId(job_id.to_string())).expect("failed supervisor lease");
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_is_alive(lease.pid) {
        assert!(
            Instant::now() < deadline,
            "failpoint supervisor {} did not exit",
            lease.pid
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_one_process_to_lose(first: &mut Child, second: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let first_exited = first.try_wait().expect("poll first supervisor").is_some();
        let second_exited = second.try_wait().expect("poll second supervisor").is_some();
        if first_exited || second_exited {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "neither racing supervisor lost the live lease"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_no_cancellation(paths: &DeadreckonPaths, job_id: &str) {
    let history = read_job_history(&paths.job_events(job_id)).expect("Job history");
    assert!(
        history.events().iter().all(|event| !matches!(
            event.kind,
            JobEventKind::CancelRequested | JobEventKind::Cancelled
        )),
        "attach disconnect persisted a cancellation: {:?}",
        history.events()
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn setup_supervisor_alias_installs_current_unit_without_host_mutation() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);

    let setup = service
        .deadreckon()
        .args(["setup", "--supervisor"])
        .output()
        .expect("setup isolated supervisor service");
    assert_success(&setup);
    assert!(
        service.managed_unit_path().is_file(),
        "setup did not install the managed unit at {}",
        service.managed_unit_path().display()
    );

    let reinstall = service
        .deadreckon()
        .args(["supervisor", "install"])
        .output()
        .expect("recheck isolated managed unit");
    assert_success(&reinstall);
    assert!(
        stdout(&reinstall).contains("already installed"),
        "the setup alias did not install the current binary/unit identity:\n{}",
        stdout(&reinstall)
    );
}

#[test]
fn replay_inspection_is_read_only_without_supervisor_service() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let saved = temp.path().join("read-only-launch-plan.json");
    write_minimal_replay_plan(&saved, "read-only replay");
    let service = SupervisorServiceFixture::unconfigured(&paths);

    for args in [
        vec![
            "start",
            "--plan",
            saved.to_str().expect("UTF-8 plan path"),
            "--preview",
            "--plain",
        ],
        vec![
            "start",
            "--plan",
            saved.to_str().expect("UTF-8 plan path"),
            "--json",
        ],
    ] {
        let output = service
            .deadreckon()
            .current_dir(&repo)
            .args(args)
            .output()
            .expect("read-only replay");
        assert_success(&output);
        assert!(
            !paths.jobs_dir().exists(),
            "read-only replay allocated durable Job state"
        );
    }
}

#[test]
fn replay_json_yes_without_supervisor_service_refuses_before_job_creation() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let saved = temp.path().join("unconfigured-launch-plan.json");
    write_minimal_replay_plan(&saved, "unconfigured replay launch");
    let service = SupervisorServiceFixture::unconfigured(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args([
            "start",
            "--plan",
            saved.to_str().expect("UTF-8 plan path"),
            "--yes",
            "--json",
        ])
        .output()
        .expect("unconfigured replay launch");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("restart-capable supervisor service"),
        "{}",
        stderr(&output)
    );
    assert!(
        !paths.jobs_dir().exists(),
        "replay service refusal allocated durable Job state"
    );
}

fn write_minimal_replay_plan(path: &std::path::Path, goal: &str) {
    fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "created_at": "2026-07-31T00:00:00Z",
            "goal": goal,
            "shape": "single",
            "pieces": [],
            "providers": { "coder": "smoke" },
            "budget": { "ceiling_usd": 1.0 },
            "resolution": {
                "source": "operator",
                "confidence": 1.0,
                "rationale": "integration-test replay plan"
            },
            "accepted_by": "operator"
        }))
        .expect("serialize replay plan"),
    )
    .expect("write replay plan");
}

#[test]
fn replay_with_smaller_budget_refuses_naming_numbers() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);

    let saved = temp.path().join("expensive-plan.json");
    fs::write(
        &saved,
        serde_json::json!({
            "schema": 1,
            "created_at": "2026-07-01T00:00:00Z",
            "goal": "expensive goal",
            "shape": "single",
            "pieces": [],
            "budget": { "ceiling_usd": 12.0 },
            "resolution": {
                "source": "operator",
                "confidence": 1.0,
                "rationale": "test plan"
            }
        })
        .to_string(),
    )
    .expect("write plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "start",
            "--plan",
            saved.to_str().expect("utf8"),
            "--max-spend",
            "5",
            "--yes",
            "--plain",
        ])
        .output()
        .expect("replay refusal");

    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("$12.00"), "{err}");
    assert!(err.contains("$5.00"), "{err}");
    assert!(err.contains("try:"), "{err}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_json_emits_launch_envelope_no_card() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let service = characterization_service(&paths);

    let output = service
        .deadreckon()
        .current_dir(&repo)
        .args(["start", "json launch goal", "--yes", "--json"])
        .output()
        .expect("json launch");

    assert_success(&output);
    let out = stdout(&output);
    let envelope: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|err| panic!("stdout must be exactly one JSON envelope: {err}\n{out}"));
    assert_eq!(envelope["kind"], "launch", "{envelope}");
    assert_eq!(envelope["plan"]["shape"], "single", "{envelope}");
    assert!(
        envelope["dispatched"]["ids"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty()),
        "{envelope}"
    );
    assert!(
        envelope["next_actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()),
        "{envelope}"
    );
    assert!(!out.contains("+---"), "no card borders in json mode: {out}");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn concurrent_same_goal_start_json_returns_each_exact_job_id() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_start_ready_setup(&paths, &repo);
    let goal = "concurrent exact job identity";
    let service = characterization_service(&paths);

    let launch = || {
        let mut command = service.deadreckon();
        command
            .current_dir(&repo)
            .env("DEADRECKON_TEST_START_ENVELOPE_DELAY_MS", "500")
            .args(["start", goal, "--mode", "run", "--fresh", "--yes", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().expect("spawn concurrent start")
    };
    let first = launch();
    let second = launch();
    let outputs = [
        first.wait_with_output().expect("first start"),
        second.wait_with_output().expect("second start"),
    ];

    let mut returned = BTreeSet::new();
    for output in outputs {
        assert_success(&output);
        let envelope: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("start output must be JSON: {error}"));
        let ids = envelope["dispatched"]["ids"]
            .as_array()
            .expect("dispatched ids");
        assert_eq!(ids.len(), 1, "{envelope}");
        returned.insert(ids[0].as_str().expect("job id").to_string());
    }

    assert_eq!(returned.len(), 2, "each launch must return its own Job ID");
    assert_eq!(
        returned,
        job_ids(&paths),
        "returned identities must be the exact persisted Jobs"
    );
    for job_id in returned {
        let job = deadreckon_core::load_job(&paths, &job_id).expect("returned Job");
        assert_eq!(job.job_id.as_ref(), job_id.as_str());
        assert_eq!(job.goal, goal);
    }
}

fn write_start_ready_setup(paths: &DeadreckonPaths, root: &std::path::Path) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), "default_provider = \"smoke\"\n").expect("config");
    fs::create_dir_all(root.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        root.join(".deadreckon/acceptance.yaml"),
        concat!(
            "name: start ready plan root shape footer review json launch replayable quiet guided\n",
            "checks:\n",
            "  - kind: shell\n",
            "    command: \"python3 -c 'from pathlib import Path; assert Path(\\\"README.md\\\").is_file()'\"\n",
            "    cwd: \"{working_dir}\"\n",
        ),
    )
    .expect("acceptance");
    if root.join(".git").is_dir() {
        git(root, &["add", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
        git(root, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
    }
}

fn deadreckon_pty(
    paths: &DeadreckonPaths,
    cwd: &std::path::Path,
    answers: &[&str],
    args: &[&str],
    _final_pattern: &str,
    path_prefix: Option<&std::path::Path>,
) -> std::process::Output {
    let command = std::iter::once(env!("CARGO_BIN_EXE_deadreckon").to_string())
        .chain(args.iter().map(|arg| arg.to_string()))
        .map(|part| tcl_brace_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    let mut interactions = String::new();
    for answer in answers {
        interactions.push_str("expect -re {choose \\[[0-9]+\\]:}\n");
        // Line mode reads whole lines: every answer needs Enter.
        interactions.push_str(&format!("send -- \"{}\\r\"\n", tcl_string_escape(answer)));
    }
    let path_setup = path_prefix
        .map(|path| {
            format!(
                "set env(PATH) \"{}:/usr/bin:/bin:/usr/sbin:/sbin\"\n",
                tcl_string_escape(&path.display().to_string())
            )
        })
        .unwrap_or_default();
    let script = format!(
        "set timeout 30\ncd {}\nset env(DEADRECKON_HOME) {}\nset env(DEADRECKON_PROMPT_LINE_MODE) 1\n{}spawn {}\n{}catch {{ expect eof }}\nexit 0\n",
        tcl_brace_quote(&cwd.display().to_string()),
        tcl_brace_quote(&paths.home().display().to_string()),
        path_setup,
        command,
        interactions
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

fn write_fake_merge_repair_provider(
    paths: &DeadreckonPaths,
    root: &std::path::Path,
    id: &str,
    repair_json: &str,
    child_script: &str,
) {
    let script = format!(
        r#"case "$1" in
  *"read-only merge repair planner"*)
    cat <<'JSON'
{repair_json}
JSON
    ;;
  *)
{child_script}
    ;;
esac
"#
    );
    write_fake_cli_subagent_provider(paths, root, id, &script);
}

/// Retry and per-node apply, together — the composition the per-feature E2Es
/// missed. A node fails its gate once (a flag file outside the repo makes the
/// check fail exactly once), the plan retries it from a fresh branch off the
/// tip rather than from the failed attempt's worktree, and both nodes land as
/// commits in dependency order.
#[test]
fn a_per_node_plan_retries_a_failed_node_and_still_lands_everything() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    // The gate runs every turn, so a naive fail-once flag would heal inside
    // attempt 1 (turn-loop healing working as designed). Keying on the
    // working directory fails the whole first RUN instead: the first check
    // records its tree, every check in that same tree keeps failing, and the
    // retry's fresh worktree is a different tree and passes.
    let flag = temp.path().join("first-attempt.tree");
    fs::create_dir_all(repo.join(".deadreckon")).expect("dot dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        format!(
            "name: fail-first-run\nchecks:\n  - kind: shell\n    command: \"test -f {flag} || pwd > {flag}; test \\\"$(cat {flag})\\\" != \\\"$(pwd)\\\"\"\n",
            flag = flag.display()
        ),
    )
    .expect("acceptance");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "acceptance"]).expect("commit");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "touch base twice",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);
    plan.apply = deadreckon_core::plan::ApplyWhen::PerNode;
    plan.on_fail = deadreckon_core::plan::OnFail::Stop;
    plan.tasks[1].depends_on = vec![plan.tasks[0].task_id.clone()];
    save_plan(&paths, &plan).expect("save per-node plan");

    let before = git_stdout_count(&repo);
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--plain"])
        .output()
        .expect("fork");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("retry 2/"), "the failed node retried: {out}");
    assert!(
        out.matches("landed on the isolated candidate").count() == 2,
        "both nodes landed: {out}"
    );
    let after = git_stdout_count(&repo);
    assert_eq!(after, before + 2, "two commits stacked on the branch");
    let forked = load_plan(&paths, &plan.plan_id).expect("plan");
    assert!(
        forked
            .tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed),
        "{forked:?}"
    );
    // The trail records failed attempts; the run that counted is
    // child_run_id. One recorded failure, and the surviving run is a
    // different run than the one that failed.
    assert_eq!(
        forked.tasks[0].attempts.len(),
        1,
        "{:?}",
        forked.tasks[0].attempts
    );
    let failed_run = forked.tasks[0].attempts[0]
        .run_id
        .as_deref()
        .expect("failed attempt has a run");
    let surviving_run = forked.tasks[0].child_run_id.as_deref().expect("result run");
    assert_ne!(failed_run, surviving_run, "the retry was a fresh run");
    assert!(
        forked.tasks[0].attempts[0]
            .finished_at
            .is_some_and(|finished| finished > forked.tasks[0].attempts[0].started_at),
        "the attempt records a real duration: {:?}",
        forked.tasks[0].attempts[0]
    );
}

fn git_stdout_count(repo: &std::path::Path) -> usize {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("rev-list");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("count")
}

/// A crashed fork used to be unrecoverable: the plan sat Forked forever and
/// fork refused re-entry. With the conductor pid recorded and attempts
/// durable, a dead conductor's plan resumes — the orphaned child's failure is
/// adopted as a recorded attempt and the retry machinery takes it from there.
#[test]
fn fork_resumes_after_a_lost_conductor_and_finishes_the_plan() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny resumable work",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);

    // Simulate the crash: the plan is mid-fork on disk, its conductor pid
    // belongs to a process that no longer exists, and task-0 was running a
    // child whose run failed its gate just before the lights went out.
    let mut dead = Command::new("true").spawn().expect("spawn");
    let dead_pid = dead.id();
    dead.wait().expect("wait");
    let mut orphan = create_test_run(
        &paths,
        &repo,
        "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
        "orphaned child work",
    );
    orphan.status = RunStatus::Failed;
    orphan.failure_reason = Some("acceptance failed after turn 2: check failed".to_string());
    save_state(&orphan).expect("orphan state");
    plan.status = PlanStatus::Forked;
    plan.conductor_pid = Some(dead_pid);
    plan.tasks[0].status = PlanTaskStatus::Running;
    plan.tasks[0].child_run_id = Some(orphan.run_id.clone());
    save_plan(&paths, &plan).expect("crashed plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--plain"])
        .output()
        .expect("fork resume");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("resumed plan"), "{out}");
    let finished = load_plan(&paths, &plan.plan_id).expect("plan");
    assert!(
        finished
            .tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed),
        "{finished:?}"
    );
    assert_eq!(
        finished.tasks[0].attempts.len(),
        1,
        "the orphan's failure is on the trail: {:?}",
        finished.tasks[0].attempts
    );
    assert_eq!(
        finished.tasks[0].attempts[0].run_id.as_deref(),
        Some(orphan.run_id.as_str()),
    );
    assert_ne!(
        finished.tasks[0].child_run_id.as_deref(),
        Some(orphan.run_id.as_str()),
        "the retry was a fresh run"
    );
    assert_eq!(finished.conductor_pid, None, "a finished fork holds no pid");
}

/// Two supervisors over one plan is the failure mode resume must not create:
/// a live conductor refuses the second fork outright.
#[test]
fn fork_refuses_to_resume_under_a_live_conductor() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "tiny guarded work",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--n",
            "2",
            "--quiet",
        ])
        .output()
        .expect("plan");
    assert_success(&output);
    let mut plan = newest_plan(&paths);

    let mut live = Command::new("sleep").arg("60").spawn().expect("sleep");
    plan.status = PlanStatus::Forked;
    plan.conductor_pid = Some(live.id());
    plan.tasks[0].status = PlanTaskStatus::Running;
    save_plan(&paths, &plan).expect("supervised plan");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--plain"])
        .output()
        .expect("fork refusal");

    live.kill().ok();
    live.wait().ok();
    assert!(!output.status.success(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("already being supervised"), "{err}");
    assert!(err.contains("deadreckon attach"), "{err}");
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "--initial-branch=main"]).expect("git init");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
}

fn soundings_swift_fixture(root: &std::path::Path) -> PathBuf {
    let source = root.join("cloudwing-source");
    fs::create_dir_all(source.join("Sources/Cloudwing")).expect("Swift sources");
    fs::create_dir_all(source.join("Tests/CloudwingTests")).expect("Swift tests");
    git(&source, &["init", "--initial-branch=main"]).expect("git init");
    fs::write(
        source.join("Package.swift"),
        r#"// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Cloudwing",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "Cloudwing", targets: ["Cloudwing"])],
    targets: [
        .executableTarget(name: "Cloudwing"),
        .testTarget(name: "CloudwingTests", dependencies: ["Cloudwing"]),
    ]
)
"#,
    )
    .expect("Package.swift");
    fs::write(source.join("README.md"), "# Cloudwing\n").expect("README");
    fs::write(
        source.join("Sources/Cloudwing/GameScene.swift"),
        "struct GameScene { let gravity = 9.8 }\n",
    )
    .expect("GameScene");
    fs::write(
        source.join("Tests/CloudwingTests/GameMathTests.swift"),
        "import Testing\n@Test func scoreStartsAtZero() { #expect(0 == 0) }\n",
    )
    .expect("GameMathTests");
    git(&source, &["add", "-A"]).expect("git add");
    git(&source, &["commit", "-m", "Cloudwing foundation"]).expect("git commit");

    fs::write(
        source.join("Sources/Cloudwing/GameScene.swift"),
        "struct GameScene { let gravity = 9.8; let boost = 1.25 }\n",
    )
    .expect("modified GameScene");
    fs::write(
        source.join("Sources/Cloudwing/Customization.swift"),
        "struct Customization { var trail = \"aurora\" }\n",
    )
    .expect("untracked customization");
    source
}

fn seed_trace_run(paths: &DeadreckonPaths, repo: &std::path::Path, run_id: &str, message: &str) {
    let state = create_test_run(paths, repo, run_id, message);
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "test-trace".to_string(),
            latency_ms: Some(1),
            detail: json!({ "message": message }),
        },
    )
    .expect("trace");
}

fn create_test_run(
    paths: &DeadreckonPaths,
    repo: &std::path::Path,
    run_id: &str,
    goal: &str,
) -> deadreckon_core::PipelineState {
    create_run(
        paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: repo.to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "deadreckon".to_string(),
            max_spend_usd: Some(10.0),
            max_wall_seconds: None,
            run_id: Some(run_id.to_string()),
            codebase: None,
        },
    )
    .expect("run")
}

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    // Commits need an identity; CI runners have no global git config.
    if args.first() == Some(&"init") && output.status.success() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["config", "user.email", "deadreckon@example.invalid"])
            .output();
        let _ = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["config", "user.name", "deadreckon"])
            .output();
    }
    assert!(
        output.status.success(),
        "git {:?}\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn git_output(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git output");
    assert!(
        output.status.success(),
        "git {:?}\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn filesystem_snapshot(root: &std::path::Path) -> Vec<(PathBuf, &'static str, u32, Vec<u8>)> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        snapshot: &mut Vec<(PathBuf, &'static str, u32, Vec<u8>)>,
    ) {
        let metadata = fs::symlink_metadata(path)
            .unwrap_or_else(|source| panic!("metadata {}: {source}", path.display()));
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path under root")
            .to_path_buf();
        let relative = if relative.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            relative
        };
        let mode = metadata.permissions().mode();
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(path)
                .unwrap_or_else(|source| panic!("read link {}: {source}", path.display()));
            snapshot.push((
                relative,
                "symlink",
                mode,
                target.to_string_lossy().as_bytes().to_vec(),
            ));
        } else if metadata.is_dir() {
            snapshot.push((relative, "directory", mode, Vec::new()));
            let mut children = fs::read_dir(path)
                .unwrap_or_else(|source| panic!("read dir {}: {source}", path.display()))
                .map(|entry| entry.expect("snapshot entry").path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                visit(root, &child, snapshot);
            }
        } else if metadata.is_file() {
            snapshot.push((
                relative,
                "file",
                mode,
                fs::read(path)
                    .unwrap_or_else(|source| panic!("read file {}: {source}", path.display())),
            ));
        } else {
            snapshot.push((relative, "other", mode, Vec::new()));
        }
    }

    if !root.exists() {
        return Vec::new();
    }
    let mut snapshot = Vec::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn newest_plan(paths: &DeadreckonPaths) -> Plan {
    let mut plans = fs::read_dir(paths.plans_dir())
        .expect("plans dir")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("plan.json"))
        .filter(|path| path.exists())
        .map(|path| {
            let plan_id = path
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .expect("plan id")
                .to_string();
            load_plan(paths, &plan_id).expect("plan")
        })
        .collect::<Vec<_>>();
    plans.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    plans.into_iter().next().expect("plan")
}

fn saved_plan_count(paths: &DeadreckonPaths) -> usize {
    match fs::read_dir(paths.plans_dir()) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().join("plan.json").exists())
            .count(),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => 0,
        Err(source) => panic!("plans dir: {source}"),
    }
}

fn plans_contain_campaign_json(paths: &DeadreckonPaths) -> bool {
    fs::read_dir(paths.plans_dir())
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .any(|entry| entry.path().join("campaign.json").is_file())
}

fn write_smoke_role_aliases(paths: &DeadreckonPaths) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        r#"
[providers."smoke:planner"]
kind = "smoke"

[providers."smoke:default"]
kind = "smoke"

[providers."smoke:reviewer"]
kind = "smoke"
"#,
    )
    .expect("config");
}

fn output_row_contains(output: &str, label: &str, value: &str) -> bool {
    output
        .lines()
        .any(|line| line.contains(label) && line.contains(value))
}
