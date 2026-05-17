#![allow(clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::lock::lock_path;
use deadreckon_core::{
    CHAIN_EVENTS_JSONL, CoordinatorState, DeadreckonPaths, PLAN_EVENTS_JSONL, Plan, PlanEvent,
    PlanEventKind, PlanMessage, PlanMessageKind, PlanMode, PlanRole, PlanStatus, PlanTaskStatus,
    RUN_EVENTS_JSONL, RunOptions, RunStatus, TraceRecord, append_plan_event, append_plan_message,
    append_trace, create_run, list_runs, load_plan, load_run, pid_is_alive, read_codebase_record,
    read_plan_events, read_plan_messages, save_plan, save_state,
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
    assert_eq!(plan.mode, PlanMode::FullPlan);
    assert_eq!(plan.tasks.len(), 3);
    assert_eq!(plan.providers.planner.as_deref(), Some("smoke"));
    assert_eq!(plan.providers.default_child.as_deref(), Some("smoke"));
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
    assert!(out.contains("planner=smoke"), "{out}");
    assert!(out.contains("default-child=smoke"), "{out}");
    assert!(out.contains("capabilities:"), "{out}");
    assert!(out.contains("deploy=true"), "{out}");
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
    assert!(out.contains("mode        : review"), "{out}");
    assert!(out.contains("coder smoke:coder"), "{out}");
    assert!(out.contains("reviewer smoke:reviewer"), "{out}");
    let plan = newest_plan(&paths);
    assert_eq!(plan.mode, PlanMode::Review);
    assert_eq!(plan.status, PlanStatus::Pending);
    assert!(plan.tasks.iter().all(|task| task.child_run_id.is_none()));
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
    assert!(out.contains("mode        : full-plan"), "{out}");
    assert!(out.contains("planner smoke:planner"), "{out}");
    assert!(out.contains("default child smoke:child"), "{out}");
    assert!(out.contains("provider=smoke:reviewer"), "{out}");
    let plan = newest_plan(&paths);
    assert_eq!(plan.mode, PlanMode::FullPlan);
    assert_eq!(plan.status, PlanStatus::Pending);
    assert!(plan.tasks.iter().all(|task| task.child_run_id.is_none()));
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
    assert!(out.contains(&plan.plan_id[..8]), "{out}");
    assert!(out.contains("attach:"), "{out}");
    assert!(
        out.contains(&format!("deadreckon attach {}", &plan.plan_id[..8])),
        "{out}"
    );
    assert!(out.contains("providers"), "{out}");
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
fn orchestrate_init_git_initializes_plain_directory_before_preview() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("plain-init");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("README.md"), "hello").expect("readme");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .args([
            "orchestrate",
            "review",
            "initialize before planning",
            "--init-git",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");

    assert_success(&output);
    assert!(workspace.join(".git").is_dir());
    let plan = newest_plan(&paths);
    assert_eq!(plan.status, PlanStatus::Pending);
    assert_eq!(
        plan.parent_cwd.as_deref(),
        Some(workspace.canonicalize().expect("canonical").as_path())
    );
}

#[test]
fn orchestrate_headless_without_mode_refuses_with_try_line() {
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
    assert!(err.contains("deadreckon orchestrate review"), "{err}");
    assert_eq!(saved_plan_count(&paths), 0);
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
        prompt.contains("Dependencies must refer to earlier child ids"),
        "{prompt}"
    );
    assert!(
        prompt.contains("child goals must be implementation or verification slices"),
        "{prompt}"
    );
    assert!(prompt.contains("Do not return research-only"), "{prompt}");
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
    assert!(err.contains("plan must have >= 2 children"), "{err}");
    assert!(
        err.contains("try: deadreckon run \"<the only child>\""),
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
    assert!(err.contains("plan capped at 6 children; got 7"), "{err}");
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
    assert!(out.contains("coder=smoke:coder"), "{out}");
    assert!(out.contains("reviewer=smoke:reviewer"), "{out}");
    assert!(out.contains("fork:"), "{out}");
    assert!(out.contains("deadreckon fork"), "{out}");
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
        out.contains("children    : 2 (2 ready / 0 blocked)"),
        "{out}"
    );
    assert!(out.contains("fork:"), "{out}");
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
fn error_messages_end_with_try_footer() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cases: Vec<(Vec<&str>, &str, &str)> = vec![
        (
            vec!["plan", "", "--quiet"],
            "--goal must be non-empty",
            "try: deadreckon plan \"your goal\"",
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
            "try: deadreckon run \"<the only child>\"",
        ),
        (
            vec!["history", "grep", "[", "--regex"],
            "invalid regex",
            "try: re-quote or escape",
        ),
    ];

    for (args, message, hint) in cases {
        let output = deadreckon(&paths)
            .current_dir(&repo)
            .args(args)
            .output()
            .expect("command");
        assert!(!output.status.success(), "{}", stdout(&output));
        let err = stderr(&output);
        assert!(err.contains(message), "{err}");
        assert!(err.contains(hint), "{err}");
    }
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
    assert!(out.contains("gate passed by dr-gate"), "{out}");
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
fn orchestrate_preview_writes_plan_created_without_fork_events() {
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
            "--n",
            "2",
            "--preview",
        ])
        .output()
        .expect("orchestrate preview");
    assert_success(&output);
    let plan = newest_plan(&paths);
    let events = read_plan_events(&paths, &plan.plan_id).expect("events");

    assert!(events.iter().any(|event| matches!(
        &event.event,
        PlanEventKind::PlanCreated {
            mode: PlanMode::FullPlan,
            task_count: 2
        }
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::PlanStarted))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(&event.event, PlanEventKind::TaskStarted { .. }))
    );
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
    assert!(
        source_path.starts_with(paths.plan_dir(&plan.plan_id)),
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
        .args(["fork", &plan.plan_id[..8], "--sandbox", "none", "--quiet"])
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
    assert!(err.contains("running"), "{err}");
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
    assert!(
        out.contains(&format!("deadreckon apply {}", &plan.plan_id[..8])),
        "{out}"
    );
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
    assert!(err.contains("merge conflict at README.md"), "{err}");
    assert!(err.contains("automatic repair disabled"), "{err}");
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
    assert!(err.contains("child 0 is running"), "{err}");
    assert!(
        err.contains("try: wait, or run deadreckon kill <plan-id>"),
        "{err}"
    );
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
            "try: deadreckon show {} --why-failed",
            &coder_run_id[..8]
        )),
        "{out}"
    );
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
    assert!(out.contains("run ddddcccc failure summary"), "{out}");
    assert!(out.contains("status: failed"), "{out}");
    assert!(out.contains("reason: acceptance failed"), "{out}");
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
    fs::write(&binary, format!("#!/bin/sh\n{script_body}")).expect("fake cli");
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
