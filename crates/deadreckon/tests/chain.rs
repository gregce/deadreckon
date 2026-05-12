use std::fs;

use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, ChainStepStatus,
    DeadreckonPaths, OnFail, RunEvent, RunEventKind, RunOptions, chain_task_key, create_run,
    promote_completed_run,
};
use tempfile::TempDir;

fn sample_chain(temp: &TempDir) -> Chain {
    Chain::new(ChainNewOptions {
        root_goal: "manual: 2 steps".to_string(),
        goals: vec!["first".to_string(), "second".to_string()],
        scope: "test-scope".to_string(),
        base_branch: "main".to_string(),
        base_sha: "abcdef0".to_string(),
        cwd: temp.path().join("repo"),
        provider: Some("mock".to_string()),
        model: Some("model-a".to_string()),
        sandbox: "none".to_string(),
        branch_policy: BranchPolicy::Stack,
        apply_mode: ApplyMode::Auto,
        apply_strategy: ApplyStrategy::Squash,
        apply_allowlist: Vec::new(),
        on_fail: OnFail::Stop,
        circuit_breaker_threshold: 2,
        max_spend_usd: Some(5.0),
        max_wall_seconds: Some(600.0),
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain")
}

#[test]
fn chain_json_serializes_roundtrip() {
    let temp = TempDir::new().expect("tempdir");
    let chain = sample_chain(&temp);
    let json = serde_json::to_string_pretty(&chain).expect("json");
    let decoded = serde_json::from_str::<Chain>(&json).expect("decoded");

    assert_eq!(decoded, chain);
}

#[test]
fn chain_step_status_transitions_pending_running_completed() {
    let temp = TempDir::new().expect("tempdir");
    let mut chain = sample_chain(&temp);
    let step = &mut chain.steps[0];

    step.transition_to(ChainStepStatus::Running)
        .expect("running");
    step.transition_to(ChainStepStatus::Completed)
        .expect("completed");

    assert_eq!(step.status, ChainStepStatus::Completed);
}

#[test]
fn chain_paths_match_locks_pattern() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let chain = sample_chain(&temp);

    assert_eq!(
        paths.chain_json(&chain.chain_id),
        paths.chains_dir().join(&chain.chain_id).join("chain.json")
    );
    assert_eq!(
        paths.chain_events(&chain.chain_id),
        paths
            .chains_dir()
            .join(&chain.chain_id)
            .join("chain-events.jsonl")
    );
    assert_eq!(
        paths.conductor_json(&chain.chain_id),
        paths
            .chains_dir()
            .join(&chain.chain_id)
            .join("conductor.json")
    );
}

#[test]
fn chain_lock_task_key_prefix_chain_double_dash() {
    assert_eq!(chain_task_key("chain-a"), "chain--chain-a");
}

#[test]
fn run_promoted_event_emitted_after_atomic_swap() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "promoted event".to_string(),
            cwd: std::env::current_dir().expect("cwd"),
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
    fs::write(state.working_dir.join("README.md"), "ok").expect("write");
    deadreckon_core::write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )
    .expect("marker");

    let library_dir = promote_completed_run(&paths, &mut state).expect("promote");
    let events = read_run_events(&state);

    assert!(library_dir.join("manifest.json").exists());
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::RunPromoted { library_dir: emitted } if emitted == &library_dir
        )
    }));
}

#[test]
fn run_promoted_event_includes_library_dir() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "promoted event library dir".to_string(),
            cwd: std::env::current_dir().expect("cwd"),
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
    fs::write(state.working_dir.join("README.md"), "ok").expect("write");
    deadreckon_core::write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )
    .expect("marker");

    let library_dir = promote_completed_run(&paths, &mut state).expect("promote");
    let raw = fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");

    assert!(raw.contains(r#""kind":"run_promoted""#));
    assert!(raw.contains(&library_dir.display().to_string()));
}

fn read_run_events(state: &deadreckon_core::PipelineState) -> Vec<RunEvent> {
    fs::read_to_string(state.run_root.join("events.jsonl"))
        .expect("events")
        .lines()
        .map(|line| serde_json::from_str::<RunEvent>(line).expect("event"))
        .collect()
}
