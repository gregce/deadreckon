#![allow(clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::{
    DeadreckonPaths, Plan, PlanMessage, PlanMessageKind, PlanMode, PlanRole, PlanStatus,
    PlanTaskStatus, RunOptions, RunStatus, TraceRecord, append_plan_message, append_trace,
    create_run, load_plan, read_plan_messages, save_plan, save_state,
};
use serde_json::{Value, json};
use tempfile::TempDir;

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
    assert_eq!(plan.mode, PlanMode::Split);
    assert_eq!(plan.tasks.len(), 3);
    assert_eq!(plan.providers.planner.as_deref(), Some("smoke"));
    assert_eq!(plan.providers.default_child.as_deref(), Some("smoke"));
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
    assert!(
        out.contains("providers: planner=smoke default-child=smoke"),
        "{out}"
    );
    assert!(out.contains("capabilities:"), "{out}");
    assert!(out.contains("deploy=true"), "{out}");
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
    let prompt = fs::read_to_string(&capture).expect("planner prompt");
    assert!(prompt.contains("read-only planning agent"), "{prompt}");
    assert!(prompt.contains("Do not write files"), "{prompt}");
    assert!(prompt.contains("create temporary files"), "{prompt}");
    assert!(prompt.contains("install packages"), "{prompt}");
    assert!(prompt.contains("commit"), "{prompt}");
    assert!(prompt.contains("Return JSON only"), "{prompt}");
    assert!(
        prompt.contains("Dependencies must refer to earlier task ids"),
        "{prompt}"
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
    assert!(err.contains("provider returned 1 tasks; need 2"), "{err}");
    assert!(
        err.contains("try: deadreckon plan ... --provider <other>"),
        "{err}"
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
    assert!(err.contains("plan must have >= 2 tasks"), "{err}");
    assert!(
        err.contains("try: deadreckon run \"<the only task>\""),
        "{err}"
    );

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
    assert!(err.contains("plan capped at 6 tasks; got 7"), "{err}");
    assert!(err.contains("try: split the goal into a chain"), "{err}");
    assert_eq!(saved_plan_count(&paths), 0);
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
fn run_accepts_plain_quiet_headless_smoke_flags() {
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
        ])
        .output()
        .expect("run");

    assert_success(&output);
    let out = stdout(&output);
    let lines = out.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{out}");
    assert!(lines[0].contains("completed"), "{out}");
    assert!(!out.contains("\x1b["), "{out:?}");
}

#[test]
fn run_quiet_emits_no_stdout_on_success() {
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
        ])
        .output()
        .expect("run");

    assert_success(&output);
    assert_eq!(stdout(&output), "");
}

#[test]
fn fork_spawns_children_with_distinct_scopes_and_messages() {
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
    assert!(out.contains("capabilities network="), "{out}");
    assert!(out.contains("gate passed by dr-gate"), "{out}");
    assert!(out.contains("latest turn"), "{out}");
    assert!(out.contains("tokens"), "{out}");
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
    assert!(err.contains("plan"), "{err}");
    assert!(err.contains("Forked"), "{err}");
    assert!(err.contains("try: deadreckon merge <plan-id>"), "{err}");
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
        .args(["merge", &plan.plan_id[..8], "--quiet"])
        .output()
        .expect("merge");
    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("conflict at README.md"),
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
}

#[test]
fn merge_manifest_records_child_provider_roles_and_task_graph() {
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
    assert!(err.contains("child 0 still running"), "{err}");
    assert!(
        err.contains("try: wait, or run deadreckon kill <plan-id>"),
        "{err}"
    );
}

#[test]
fn orchestrate_review_mode_runs_fork_and_merge() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "orchestrate",
            "tiny hello rust",
            "--mode",
            "review",
            "--coder-provider",
            "smoke",
            "--reviewer-provider",
            "smoke",
            "--sandbox",
            "none",
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
    assert!(out.contains("plan"), "{out}");
    assert!(out.contains("task-0"), "{out}");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &plan.plan_id[..8]])
        .output()
        .expect("show");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains(&plan.plan_id), "{out}");
    assert!(out.contains("\"root_goal\""), "{out}");
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
    assert!(out.contains("failure summary"), "{out}");
    assert!(out.contains("child 0 task-0 status failed"), "{out}");
    assert!(
        out.contains("deadreckon show abc12345 --why-failed"),
        "{out}"
    );
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
    assert_eq!(stdout(&output).trim(), "no failures detected");
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
    assert!(out.contains("run ddddcccc status failed"), "{out}");
    assert!(out.contains("reason acceptance failed"), "{out}");
    assert!(out.contains("turn 3 tool.failed"), "{out}");
    assert!(out.contains("boom from failing test"), "{out}");
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
}

#[test]
fn history_grep_substring_finds_pattern_across_runs() {
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
fn history_grep_plan_scope_excludes_unrelated_runs() {
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
    assert!(err.contains("invalid regex"), "{err}");
    assert!(err.contains("try: re-quote"), "{err}");
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

fn write_fake_planner_provider(
    paths: &DeadreckonPaths,
    root: &std::path::Path,
    id: &str,
    capture: &std::path::Path,
    response_json: &str,
) {
    fs::create_dir_all(paths.home()).expect("home");
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

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
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
    assert!(
        output.status.success(),
        "git {:?}\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
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
