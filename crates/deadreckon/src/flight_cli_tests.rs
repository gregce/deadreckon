use super::*;
use deadreckon_core::flight::{
    CheckpointBase, CheckpointBaseKind, CheckpointCaptureRequest, CheckpointTrigger,
    FlightManifest, FlightSession, FlightUsage, append_flight_event, capture_delta_checkpoint,
    write_flight_manifest,
};
use tempfile::TempDir;

fn checkpoint_fixture() -> (TempDir, deadreckon_core::PipelineState) {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_run(
        &paths,
        RunOptions {
            goal: "flight rewind".to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("cli:test".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    deadreckon_core::snapshot_working(&state, 0).expect("snapshot");
    (temp, state)
}

fn write_manifest(state: &deadreckon_core::PipelineState, status: FlightSessionStatus) {
    let mut manifest = FlightManifest::new(state.run_id.clone());
    manifest.sessions.push(FlightSession {
        flight_session_id: "flight-turn-1-attempt-1".to_string(),
        provider: "cli:test".to_string(),
        schema: "test".to_string(),
        deadreckon_turn: 1,
        attempt: 1,
        status,
        started_at: Utc::now(),
        completed_at: Some(Utc::now()),
        source_paths: Vec::new(),
    });
    write_flight_manifest(state, &manifest).expect("manifest");
}

fn capture_fixture_checkpoint(state: &deadreckon_core::PipelineState) {
    let before = build_working_file_index(&state.working_dir).expect("before");
    let source = state.working_dir.join("src/lib.rs");
    std::fs::create_dir_all(source.parent().expect("parent")).expect("src");
    std::fs::write(&source, "pub fn value() -> u8 { 1 }\n").expect("source");
    let after = build_working_file_index(&state.working_dir).expect("after");
    capture_delta_checkpoint(
        state,
        &before,
        &after,
        CheckpointCaptureRequest {
            checkpoint_id: "cp-000001".to_string(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider_event_seq: Some(3),
            trigger: CheckpointTrigger::ProviderExit,
            base: CheckpointBase {
                kind: CheckpointBaseKind::TurnSnapshot,
                id: "turn-0".to_string(),
            },
            full_anchor: false,
        },
    )
    .expect("checkpoint");
}

#[test]
fn rewind_target_resolves_provider_event_checkpoint() {
    let (_temp, state) = checkpoint_fixture();
    write_manifest(&state, FlightSessionStatus::Completed);
    capture_fixture_checkpoint(&state);
    append_flight_event(
        &state,
        &FlightEvent {
            version: 1,
            seq: 3,
            run_id: state.run_id.clone(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider: "cli:test".to_string(),
            schema: "test".to_string(),
            timestamp: Some(Utc::now()),
            source_path: None,
            source_line: None,
            source_event: "{}".to_string(),
            raw_hash: "sha256:test".to_string(),
            kind: FlightEventKind::Tool,
            role: None,
            summary: "tool".to_string(),
            tool_name: Some("write_file".to_string()),
            tool_category: None,
            files: vec![PathBuf::from("src/lib.rs")],
            usage: None,
            checkpoint_id: Some("cp-000001".to_string()),
        },
    )
    .expect("event");
    let resolved = resolve_rewind_target(
        &state,
        &RewindCliOptions {
            to_turn: None,
            to_provider_event: Some(3),
            to_checkpoint: None,
            preview: true,
            apply: false,
            json: false,
        },
    )
    .expect("target");
    assert_eq!(resolved.checkpoint_id, "cp-000001");
    assert_eq!(resolved.target.kind, RewindTargetKind::ProviderEvent);
}

#[test]
fn rewind_apply_hash_guard_refuses_unrelated_file_edits() {
    let (_temp, mut state) = checkpoint_fixture();
    write_manifest(&state, FlightSessionStatus::Completed);
    capture_fixture_checkpoint(&state);
    state.turn = 1;
    deadreckon_core::snapshot_working(&state, 1).expect("snapshot");
    let target_dir = state.run_root.join("rewind-preview/cp-000001-test");
    materialize_checkpoint(&state, "cp-000001", &target_dir).expect("materialize");
    std::fs::write(state.working_dir.join("src/lib.rs"), "user edit\n").expect("edit");
    let result = hash_guard_rewind_apply(&state, &target_dir, &[PathBuf::from("src/lib.rs")]);
    assert!(result.is_err());
    assert!(result.expect_err("refusal").contains("unrelated edits"));
}

#[test]
fn attach_provider_activity_uses_flight_events() {
    let (_temp, state) = checkpoint_fixture();
    append_flight_event(
        &state,
        &FlightEvent {
            version: 1,
            seq: 1,
            run_id: state.run_id.clone(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider: "cli:test".to_string(),
            schema: "test".to_string(),
            timestamp: Some(Utc::now()),
            source_path: None,
            source_line: None,
            source_event: "{}".to_string(),
            raw_hash: "sha256:test".to_string(),
            kind: FlightEventKind::Tool,
            role: None,
            summary: "edited src/lib.rs".to_string(),
            tool_name: Some("write_file".to_string()),
            tool_category: None,
            files: vec![PathBuf::from("src/lib.rs")],
            usage: Some(FlightUsage {
                input_tokens: 10,
                output_tokens: 5,
                context_window: Some(100),
            }),
            checkpoint_id: Some("cp-000001".to_string()),
        },
    )
    .expect("event");
    let activity = collect_provider_activity(&state);
    assert!(activity.lines.join("\n").contains("flight #000001"));
    assert_eq!(activity.context_tokens, Some(15));
    assert_eq!(activity.context_window, Some(100));
}
