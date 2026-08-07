#![allow(clippy::expect_used)]
//! `deadreckon follow <id> --json` — gap G5 step 2, the merged streaming
//! surface over the blessed tailing ledgers (docs/TAILING.md).
//!
//! These tests drive the real binary as an external consumer would: spawn
//! `follow` with a piped stdin (the subprocess-supervision contract), append
//! ledger rows while it streams, and assert the NDJSON contract — merged
//! arrival order, per-record byte offsets that replay via `--from` without
//! duplication or loss, the acceptance-progress restart marker, the final
//! `{"terminal":true,…}` line once the artifact is terminal and the tails
//! are drained, and the typed refusal paths.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use common::{deadreckon, repo_tempdir};
use deadreckon_core::{
    DeadreckonPaths, PipelineState, RunOptions, RunStatus, append_job_event, create_run,
    emit_event, save_state, write_job,
};
use deadreckon_protocol::{
    Job, JobEvent, JobEventKind, JobEventSequence, JobId, JobPolicy, JobSchemaVersion, JobShape,
    RunEventKind, SemanticJudgeMode,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const RECV_TIMEOUT: Duration = Duration::from_secs(20);

fn fixture_run(temp: &TempDir, paths: &DeadreckonPaths, run_id: Option<String>) -> PipelineState {
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    create_run(
        paths,
        RunOptions {
            goal: "follow stream fixture".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id,
            codebase: None,
        },
    )
    .expect("run")
}

/// Byte offset after each newline-terminated line — the exact offsets the
/// follow contract promises per record.
fn line_end_offsets(path: &Path) -> Vec<u64> {
    fs::read(path)
        .expect("ledger bytes")
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .map(|(index, _)| (index + 1) as u64)
        .collect()
}

fn jsonl_rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("ledger text")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("ledger row"))
        .collect()
}

struct FollowSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

fn spawn_follow(paths: &DeadreckonPaths, args: &[&str]) -> FollowSession {
    let mut command = deadreckon(paths);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn follow");
    let stdin = child.stdin.take().expect("stdin pipe");
    let stdout_pipe = child.stdout.take().expect("stdout pipe");
    let stderr_pipe = child.stderr.take().expect("stderr pipe");
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout_pipe).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let stderr = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&stderr);
    thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr_pipe).read_to_string(&mut text);
        sink.lock().expect("stderr sink").push_str(&text);
    });
    FollowSession {
        child,
        stdin: Some(stdin),
        lines,
        stderr,
    }
}

impl FollowSession {
    fn stderr_text(&self) -> String {
        self.stderr.lock().expect("stderr").clone()
    }

    fn next_value(&mut self, why: &str) -> Value {
        match self.lines.recv_timeout(RECV_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line).unwrap_or_else(|error| {
                panic!("non-JSON follow line while waiting for {why}: {error}: {line}")
            }),
            Err(_) => panic!(
                "timed out waiting for {why}; stderr:\n{}",
                self.stderr_text()
            ),
        }
    }

    /// The next line from one source, skipping lines other ledgers emitted
    /// in between (per-source order is what the offset contract pins).
    fn next_from_source(&mut self, source: &str, why: &str) -> Value {
        loop {
            let value = self.next_value(why);
            if value["source"] == source {
                return value;
            }
        }
    }

    /// Drain until the final `{"terminal":true,…}` line, returning it plus
    /// everything that preceded it.
    fn drain_to_terminal(&mut self, why: &str) -> (Vec<Value>, Value) {
        let mut before = Vec::new();
        loop {
            let value = self.next_value(why);
            if value["terminal"] == json!(true) {
                return (before, value);
            }
            before.push(value);
        }
    }

    /// Drain raw stdout lines until the stream closes (process exit). Every
    /// line is returned verbatim so callers can assert per-line parseability
    /// — the NDJSON framing contract.
    fn drain_raw_until_exit(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            match self.lines.recv_timeout(RECV_TIMEOUT) {
                Ok(line) => lines.push(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => return lines,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                    "timed out draining follow stdout; stderr:\n{}",
                    self.stderr_text()
                ),
            }
        }
    }

    fn wait_code(&mut self, expected: i32) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll follow child") {
                assert_eq!(
                    status.code(),
                    Some(expected),
                    "follow exited with {status}; stderr:\n{}",
                    self.stderr_text()
                );
                return;
            }
            assert!(
                started.elapsed() < RECV_TIMEOUT,
                "follow did not exit; stderr:\n{}",
                self.stderr_text()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_success(&mut self) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll follow child") {
                assert!(
                    status.success(),
                    "follow exited with {status}; stderr:\n{}",
                    self.stderr_text()
                );
                return;
            }
            assert!(
                started.elapsed() < RECV_TIMEOUT,
                "follow did not exit; stderr:\n{}",
                self.stderr_text()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for FollowSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn follow_streams_appends_in_arrival_order_and_ends_terminal_when_drained() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");
    let events_path = state.run_root.join("events.jsonl");

    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);

    // Backlog: both records, verbatim, each with the byte offset after it.
    let first = session.next_from_source("events", "first backlog record");
    let second = session.next_from_source("events", "second backlog record");
    let rows = jsonl_rows(&events_path);
    let ends = line_end_offsets(&events_path);
    assert_eq!(first["record"], rows[0]);
    assert_eq!(second["record"], rows[1]);
    assert_eq!(first["offset"].as_u64(), Some(ends[0]));
    assert_eq!(second["offset"].as_u64(), Some(ends[1]));

    // A live append is picked up and arrives after the backlog.
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 3 }).expect("turn 3");
    let third = session.next_from_source("events", "live appended record");
    assert_eq!(third["record"], jsonl_rows(&events_path)[2]);
    assert_eq!(
        third["offset"].as_u64(),
        Some(fs::metadata(&events_path).expect("events metadata").len())
    );

    // A different ledger merges into the same stream in arrival order.
    let spend_path = state.run_root.join("spend.jsonl");
    let spend_row = json!({ "kind": "loop", "cost_usd": 0.01 });
    let mut spend_text = fs::read_to_string(&spend_path).unwrap_or_default();
    spend_text.push_str(&format!("{spend_row}\n"));
    fs::write(&spend_path, spend_text).expect("append spend row");
    let spend = session.next_from_source("spend", "merged spend record");
    assert_eq!(spend["record"], spend_row);
    assert_eq!(
        spend["offset"].as_u64(),
        Some(fs::metadata(&spend_path).expect("spend metadata").len())
    );

    // Terminal phase + drained tails => one final terminal line, exit 0.
    let mut terminal_state = state.clone();
    terminal_state.status = RunStatus::Completed;
    save_state(&terminal_state).expect("terminal state");
    let (_, terminal) = session.drain_to_terminal("terminal line");
    assert_eq!(terminal, json!({ "terminal": true, "phase": "completed" }));
    session.wait_success();
}

#[test]
fn follow_from_offsets_resume_without_duplication_or_loss() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");
    let events_path = state.run_root.join("events.jsonl");

    // Session 1: consume two records, keep the emitted offset, disconnect by
    // closing stdin (the subprocess-supervision contract ends the stream).
    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("events", "first record");
    let second = session.next_from_source("events", "second record");
    let resume_offset = second["offset"].as_u64().expect("offset");
    session.close_stdin();
    session.wait_success();

    emit_event(&state, None, RunEventKind::TurnStarted { turn: 3 }).expect("turn 3");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 4 }).expect("turn 4");
    let mut terminal_state = state.clone();
    terminal_state.status = RunStatus::Completed;
    save_state(&terminal_state).expect("terminal state");

    // Session 2: reconnect from the emitted offset. Rows 1–2 must not be
    // re-emitted, rows 3–4 must not be lost.
    let from = format!("events={resume_offset}");
    let mut session = spawn_follow(
        &paths,
        &["follow", &state.run_id, "--json", "--from", &from],
    );
    let (before, terminal) = session.drain_to_terminal("replayed stream");
    session.wait_success();
    assert_eq!(terminal["phase"], "completed");

    let rows = jsonl_rows(&events_path);
    let ends = line_end_offsets(&events_path);
    let replayed = before
        .iter()
        .filter(|line| line["source"] == "events")
        .collect::<Vec<_>>();
    assert_eq!(
        replayed.len(),
        2,
        "resume must emit exactly the rows after the offset: {before:?}"
    );
    assert_eq!(replayed[0]["record"], rows[2]);
    assert_eq!(replayed[1]["record"], rows[3]);
    assert_eq!(replayed[0]["offset"].as_u64(), Some(ends[2]));
    assert_eq!(replayed[1]["offset"].as_u64(), Some(ends[3]));
}

#[test]
fn acceptance_progress_rewrite_emits_a_restart_marker_then_reemits_from_zero() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    let proofs = state.run_root.join("proofs");
    fs::create_dir_all(&proofs).expect("proofs dir");
    let progress_path = proofs.join("acceptance-progress.jsonl");
    let streamed = [
        json!({ "checked_at": Utc::now(), "status": "started", "index": 0, "total": 2 }),
        json!({ "checked_at": Utc::now(), "status": "running", "index": 1, "total": 2 }),
    ];
    fs::write(
        &progress_path,
        format!("{}\n{}\n", streamed[0], streamed[1]),
    )
    .expect("streamed advisory rows");

    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("acceptance-progress", "first advisory row");
    session.next_from_source("acceptance-progress", "second advisory row");

    // The sign-time rewrite: truncate-and-write with different, shorter
    // content. The documented reader rule is restart, not corruption.
    let reconstructed =
        json!({ "checked_at": Utc::now(), "status": "passed", "index": 2, "total": 2 });
    fs::write(&progress_path, format!("{reconstructed}\n")).expect("sign-time rewrite");

    let marker = session.next_from_source("acceptance-progress", "restart marker");
    assert_eq!(
        marker,
        json!({ "source": "acceptance-progress", "restart": true }),
        "the rewrite must announce itself before any re-emitted row"
    );
    let reemitted = session.next_from_source("acceptance-progress", "re-emitted row");
    assert_eq!(reemitted["record"], reconstructed);
    assert_eq!(
        reemitted["offset"].as_u64(),
        Some(line_end_offsets(&progress_path)[0]),
        "re-emission restarts the offset sequence from the top of the file"
    );

    session.close_stdin();
    session.wait_success();
}

#[test]
fn single_shape_job_merges_job_events_and_ends_at_job_terminal() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let job_id = "f0110w00aaaabbbbccccddddeeeeffff";
    let state = fixture_run(&temp, &paths, Some(job_id.to_string()));
    assert_eq!(
        state.run_id, job_id,
        "Single-shape attempt run IS the job id"
    );
    write_job(
        &paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: state.scope.clone(),
            goal: "follow a durable job".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: temp.path().join("workspace"),
            launch_plan_sha256: "fixture-launch-plan".to_string(),
            authority_sha256: "fixture-authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 1,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            },
        },
    )
    .expect("write Job");
    let job_event = |sequence: u64, event_id: &str, kind: JobEventKind| JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(job_id.to_string()),
        sequence: JobEventSequence::new(sequence).expect("sequence"),
        event_id: event_id.to_string(),
        causation_id: event_id.to_string(),
        timestamp: Utc::now(),
        lease_epoch: 0,
        kind,
        detail: json!({}),
    };
    append_job_event(&paths, &job_event(1, "created", JobEventKind::Created)).expect("created");
    append_job_event(&paths, &job_event(2, "queued", JobEventKind::Queued)).expect("queued");
    let events_path = paths.job_events(job_id);

    let mut session = spawn_follow(&paths, &["follow", job_id, "--json"]);
    let first = session.next_from_source("job-events", "first job event");
    let second = session.next_from_source("job-events", "second job event");
    let rows = jsonl_rows(&events_path);
    let ends = line_end_offsets(&events_path);
    assert_eq!(first["record"], rows[0]);
    assert_eq!(second["record"], rows[1]);
    assert_eq!(first["offset"].as_u64(), Some(ends[0]));
    assert_eq!(second["offset"].as_u64(), Some(ends[1]));

    append_job_event(
        &paths,
        &job_event(3, "attempt", JobEventKind::AttemptStarted),
    )
    .expect("attempt started");
    let third = session.next_from_source("job-events", "live job event");
    assert_eq!(third["record"], jsonl_rows(&events_path)[2]);

    // A terminal job event both appends a record and flips the projection to
    // its terminal phase: the record must arrive before the terminal line.
    append_job_event(&paths, &job_event(4, "cancelled", JobEventKind::Cancelled))
        .expect("cancelled");
    let (before, terminal) = session.drain_to_terminal("job terminal line");
    session.wait_success();
    assert!(
        before
            .iter()
            .any(|line| line["source"] == "job-events" && line["record"]["sequence"] == json!(4)),
        "the terminal job event must be emitted before the terminal line: {before:?}"
    );
    assert_eq!(
        terminal,
        json!({ "terminal": true, "phase": "terminal", "outcome": "cancelled" })
    );
}

#[test]
fn follow_refusals_are_typed_and_carry_try_lines() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);

    // --json is required: prose refusal (the envelope contract begins when
    // --json arms the machine rule), exit 1, nothing on stdout.
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id])
        .output()
        .expect("follow without --json");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        common::stdout(&output).trim().is_empty(),
        "no envelope without --json"
    );
    let text = common::stderr(&output);
    assert!(text.contains("requires --json"), "{text}");
    assert!(text.contains("try:"), "{text}");

    // Unknown --from source: machine refusal envelope with try_lines.
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json", "--from", "bogus=0"])
        .output()
        .expect("follow with unknown source");
    assert_eq!(output.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error");
    assert_eq!(envelope["verb"], "follow");
    assert_eq!(envelope["code"], 1);
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("bogus"),
        "{envelope}"
    );
    assert!(
        !envelope["try_lines"]
            .as_array()
            .expect("try_lines")
            .is_empty(),
        "{envelope}"
    );

    // job-events is a real source but not part of a plain run's stream.
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json", "--from", "job-events=0"])
        .output()
        .expect("follow with unfollowed source");
    assert_eq!(output.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error");
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("job-events"),
        "{envelope}"
    );

    // A malformed offset refuses rather than guessing.
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json", "--from", "events=later"])
        .output()
        .expect("follow with malformed offset");
    assert_eq!(output.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error");
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("byte offset"),
        "{envelope}"
    );

    // An unresolvable reference is a typed machine refusal too.
    let output = deadreckon(&paths)
        .args(["follow", "00000000", "--json"])
        .output()
        .expect("follow with unknown reference");
    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error");
    assert_eq!(envelope["verb"], "follow");
}

/// Recursive `(path, len)` snapshot for read-only assertions.
fn fs_snapshot(root: &Path) -> Vec<(std::path::PathBuf, u64)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(metadata) = fs::metadata(&path) {
                entries.push((path, metadata.len()));
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn mid_stream_corruption_envelope_is_itself_one_ndjson_line() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");
    let events_path = state.run_root.join("events.jsonl");

    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("events", "first record");
    session.next_from_source("events", "second record");

    // Truncate the append-only ledger under the retained offset: the strict
    // corruption rule fails the stream closed. The refusal envelope arrives
    // on the same fd as the records, so it must itself be one parseable
    // NDJSON line — a line-oriented decoder never sees a broken frame.
    let bytes = fs::read(&events_path).expect("events bytes");
    let first_end = line_end_offsets(&events_path)[0] as usize;
    fs::write(&events_path, &bytes[..first_end]).expect("truncate events");

    let lines = session.drain_raw_until_exit();
    session.wait_code(1);
    assert!(!lines.is_empty(), "expected a refusal envelope line");
    let parsed = lines
        .iter()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("non-parseable stdout line: {error}: {line}"))
        })
        .collect::<Vec<_>>();
    let envelope = parsed.last().expect("envelope line");
    assert_eq!(envelope["kind"], "error", "{envelope}");
    assert_eq!(envelope["verb"], "follow", "{envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("shrank"),
        "{envelope}"
    );
}

#[test]
fn stale_executing_run_emits_one_advisory_stalled_line() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");

    // An Executing state with no supervised pid whose last update is past
    // the staleness window: the runner is dead and no terminal transition
    // will ever arrive on its own. Follow must say so instead of polling
    // silently forever.
    let mut stale_state = state.clone();
    stale_state.status = RunStatus::Executing;
    stale_state.updated_at = Utc::now() - chrono::Duration::minutes(6);
    save_state(&stale_state).expect("stale executing state");

    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("events", "backlog record");
    let stalled = loop {
        let value = session.next_value("stalled advisory line");
        if value["stalled"] == json!(true) {
            break value;
        }
    };
    assert_eq!(stalled["phase"], "executing", "{stalled}");
    assert_eq!(stalled["run_id"], json!(state.run_id), "{stalled}");
    assert!(
        stalled["detail"].as_str().expect("detail").contains("live"),
        "{stalled}"
    );

    // Advisory, not terminal: the stream stays open until the consumer
    // disconnects.
    session.close_stdin();
    session.wait_success();
}

#[test]
fn job_events_sequence_gap_fails_the_stream_closed() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let job_id = "5e9dea9aaaaabbbbccccddddeeeeffff";
    let state = fixture_run(&temp, &paths, Some(job_id.to_string()));
    assert_eq!(state.run_id, job_id);
    write_job(
        &paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: state.scope.clone(),
            goal: "sequence gap fixture".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: temp.path().join("workspace"),
            launch_plan_sha256: "fixture-launch-plan".to_string(),
            authority_sha256: "fixture-authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 1,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            },
        },
    )
    .expect("write Job");
    let job_event = |sequence: u64, event_id: &str, kind: JobEventKind| JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(job_id.to_string()),
        sequence: JobEventSequence::new(sequence).expect("sequence"),
        event_id: event_id.to_string(),
        causation_id: event_id.to_string(),
        timestamp: Utc::now(),
        lease_epoch: 0,
        kind,
        detail: json!({}),
    };
    append_job_event(&paths, &job_event(1, "created", JobEventKind::Created)).expect("created");
    append_job_event(&paths, &job_event(2, "queued", JobEventKind::Queued)).expect("queued");
    let events_path = paths.job_events(job_id);

    let mut session = spawn_follow(&paths, &["follow", job_id, "--json"]);
    session.next_from_source("job-events", "sequence 1");
    session.next_from_source("job-events", "sequence 2");

    // The writer refuses gaps, so simulate corruption directly: append a row
    // whose sequence skips 3. Invariant 5 makes any discontinuity
    // corruption, and TAILING.md promises follow runs that exact check.
    let mut text = fs::read_to_string(&events_path).expect("job-events text");
    text.push_str("{\"sequence\":4,\"event_id\":\"gap\"}\n");
    fs::write(&events_path, text).expect("gapped append");

    let lines = session.drain_raw_until_exit();
    session.wait_code(1);
    let envelope: Value =
        serde_json::from_str(lines.last().expect("envelope line")).expect("parseable envelope");
    assert_eq!(envelope["kind"], "error", "{envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("sequence"),
        "{envelope}"
    );
}

#[test]
fn mid_record_from_cursor_refuses_as_an_invalid_cursor() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");
    let events_path = state.run_root.join("events.jsonl");

    // A nonzero offset follow never emitted, landing mid-record: the failure
    // is blamed on the cursor (with a reconnect-from-0 recovery), not on the
    // ledger — the writer did nothing wrong.
    let mid_record = line_end_offsets(&events_path)[0] - 3;
    let from = format!("events={mid_record}");
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json", "--from", &from])
        .output()
        .expect("follow with mid-record cursor");
    assert_eq!(output.status.code(), Some(1));
    for line in common::stdout(&output).lines() {
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("non-parseable stdout line: {error}: {line}"));
        assert_eq!(value["kind"], "error", "{value}");
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("--from cursor"),
            "the refusal must blame the cursor, not the ledger: {value}"
        );
        assert!(
            !value["try_lines"].as_array().expect("try_lines").is_empty(),
            "{value}"
        );
    }
}

#[test]
fn from_generation_mismatch_on_append_only_source_refuses() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");
    let events_path = state.run_root.join("events.jsonl");

    // Session 1: capture a genuine cursor — offset plus generation token.
    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("events", "first record");
    let second = session.next_from_source("events", "second record");
    let offset = second["offset"].as_u64().expect("offset");
    let generation = second["generation"]
        .as_str()
        .expect("generation")
        .to_string();
    session.close_stdin();
    session.wait_success();

    // Replace the ledger with a longer file whose line boundary coincides
    // with the old cursor — the rewrite that evades every length check.
    // The generation token still catches it: the cursor names a file that no
    // longer exists, and resuming by byte offset would silently skip the new
    // file's head.
    let head = "x".repeat(offset as usize - 11);
    fs::write(
        &events_path,
        format!("{{\"pad\":\"{head}\"}}\n{{\"tail\":1}}\n"),
    )
    .expect("boundary-coincident rewrite");
    assert_eq!(
        line_end_offsets(&events_path)[0],
        offset,
        "fixture must land a line boundary exactly on the old cursor"
    );

    let from = format!("events={offset}@{generation}");
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json", "--from", &from])
        .output()
        .expect("follow with stale generation");
    assert_eq!(output.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error", "{envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("generation"),
        "{envelope}"
    );
}

#[test]
fn boundary_coincident_longer_rewrite_restarts_acceptance_progress() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    let proofs = state.run_root.join("proofs");
    fs::create_dir_all(&proofs).expect("proofs dir");
    let progress_path = proofs.join("acceptance-progress.jsonl");
    let streamed = [
        json!({ "checked_at": Utc::now(), "status": "started", "index": 0, "total": 2 }),
        json!({ "checked_at": Utc::now(), "status": "running", "index": 1, "total": 2 }),
    ];
    fs::write(
        &progress_path,
        format!("{}\n{}\n", streamed[0], streamed[1]),
    )
    .expect("streamed advisory rows");

    // Session 1: capture the cursor after the second advisory row.
    let mut session = spawn_follow(&paths, &["follow", &state.run_id, "--json"]);
    session.next_from_source("acceptance-progress", "first advisory row");
    let second = session.next_from_source("acceptance-progress", "second advisory row");
    let offset = second["offset"].as_u64().expect("offset");
    let generation = second["generation"]
        .as_str()
        .expect("generation")
        .to_string();
    session.close_stdin();
    session.wait_success();

    // Sign-time-style rewrite to LONGER content whose first line boundary
    // coincides exactly with the old cursor: length checks cannot see it,
    // and without the generation token the new file's first row would be
    // silently lost. The documented rule for this file is restart, so the
    // reconnect must announce the restart marker and re-emit from 0.
    let head = "y".repeat(offset as usize - 14);
    let rewritten_first = format!("{{\"reborn\":\"{head}\"}}");
    let rewritten_second = json!({ "status": "passed", "index": 2, "total": 2 });
    fs::write(
        &progress_path,
        format!("{rewritten_first}\n{rewritten_second}\n"),
    )
    .expect("boundary-coincident rewrite");
    assert_eq!(
        line_end_offsets(&progress_path)[0],
        offset,
        "fixture must land a line boundary exactly on the old cursor"
    );

    let from = format!("acceptance-progress={offset}@{generation}");
    let mut session = spawn_follow(
        &paths,
        &["follow", &state.run_id, "--json", "--from", &from],
    );
    let marker = session.next_from_source("acceptance-progress", "restart marker");
    assert_eq!(
        marker,
        json!({ "source": "acceptance-progress", "restart": true }),
        "a generation mismatch on the restartable file must restart, not skip"
    );
    let first = session.next_from_source("acceptance-progress", "re-emitted first row");
    assert_eq!(
        first["record"],
        serde_json::from_str::<Value>(&rewritten_first).expect("first row"),
        "the new file's head must be re-emitted, never silently lost"
    );
    let second = session.next_from_source("acceptance-progress", "re-emitted second row");
    assert_eq!(second["record"], rewritten_second);
    session.close_stdin();
    session.wait_success();
}

#[test]
fn from_into_truncated_acceptance_progress_emits_the_restart_marker() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    let proofs = state.run_root.join("proofs");
    fs::create_dir_all(&proofs).expect("proofs dir");
    let progress_path = proofs.join("acceptance-progress.jsonl");
    let streamed = [
        json!({ "checked_at": Utc::now(), "status": "started", "index": 0, "total": 2 }),
        json!({ "checked_at": Utc::now(), "status": "running", "index": 1, "total": 2 }),
    ];
    fs::write(
        &progress_path,
        format!("{}\n{}\n", streamed[0], streamed[1]),
    )
    .expect("streamed advisory rows");
    let old_cursor = line_end_offsets(&progress_path)[1];

    // The file was truncated between sessions (a new gate attempt): the old
    // cursor now points beyond EOF. For this one file that is a restart, not
    // corruption — marker first, then re-emission from the top.
    let reconstructed =
        json!({ "checked_at": Utc::now(), "status": "failed", "index": 2, "total": 2 });
    fs::write(&progress_path, format!("{reconstructed}\n")).expect("truncating rewrite");

    let from = format!("acceptance-progress={old_cursor}");
    let mut session = spawn_follow(
        &paths,
        &["follow", &state.run_id, "--json", "--from", &from],
    );
    let marker = session.next_from_source("acceptance-progress", "restart marker");
    assert_eq!(
        marker,
        json!({ "source": "acceptance-progress", "restart": true })
    );
    let row = session.next_from_source("acceptance-progress", "re-emitted row");
    assert_eq!(row["record"], reconstructed);
    session.close_stdin();
    session.wait_success();
}

#[test]
fn dev_null_stdin_snapshots_the_backlog_read_only_and_exits_without_terminal_line() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = fixture_run(&temp, &paths, None);
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn 1");
    emit_event(&state, None, RunEventKind::TurnStarted { turn: 2 }).expect("turn 2");

    // Documented corollary: stdin from /dev/null is already closed, so
    // follow emits one full drain of the backlog and exits 0 — a snapshot,
    // not a hang, and no terminal line because the run is not terminal.
    let before = fs_snapshot(paths.home());
    let output = deadreckon(&paths)
        .args(["follow", &state.run_id, "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("snapshot follow");
    assert_eq!(output.status.code(), Some(0));
    let lines = common::stdout(&output)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parseable line"))
        .collect::<Vec<_>>();
    let events = lines
        .iter()
        .filter(|line| line["source"] == "events")
        .count();
    assert_eq!(events, 2, "the full backlog must be drained: {lines:?}");
    assert!(
        lines.iter().all(|line| line["terminal"] != json!(true)),
        "a non-terminal run must not get a terminal line: {lines:?}"
    );

    // Follow is strictly read-only: nothing under the deadreckon home may
    // have been created, removed, or resized.
    assert_eq!(before, fs_snapshot(paths.home()), "follow must not write");
}

#[test]
fn queued_job_without_attempt_run_refuses_with_follow_wording() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let job_id = "0efab1e0aaaabbbbccccddddeeeeffff";
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    write_job(
        &paths,
        &Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.to_string()),
            scope: deadreckon_core::paths::workspace_scope(&workspace).expect("scope"),
            goal: "queued job with no attempt yet".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: temp.path().join("workspace"),
            launch_plan_sha256: "fixture-launch-plan".to_string(),
            authority_sha256: "fixture-authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 1,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            },
        },
    )
    .expect("write Job");
    let job_event = |sequence: u64, event_id: &str, kind: JobEventKind| JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(job_id.to_string()),
        sequence: JobEventSequence::new(sequence).expect("sequence"),
        event_id: event_id.to_string(),
        causation_id: event_id.to_string(),
        timestamp: Utc::now(),
        lease_epoch: 0,
        kind,
        detail: json!({}),
    };
    append_job_event(&paths, &job_event(1, "created", JobEventKind::Created)).expect("created");
    append_job_event(&paths, &job_event(2, "queued", JobEventKind::Queued)).expect("queued");

    // No attempt run exists yet: the refusal must speak follow's language
    // (streaming, reconnect) rather than verdict's ("to verify").
    let output = deadreckon(&paths)
        .args(["follow", job_id, "--json"])
        .output()
        .expect("follow queued job");
    assert_eq!(output.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope on stdout");
    assert_eq!(envelope["kind"], "error", "{envelope}");
    assert_eq!(envelope["verb"], "follow", "{envelope}");
    let message = envelope["message"].as_str().expect("message");
    assert!(message.contains("no attempt run yet"), "{envelope}");
    assert!(!message.contains("verify"), "{envelope}");
    assert!(
        !envelope["try_lines"]
            .as_array()
            .expect("try_lines")
            .is_empty(),
        "{envelope}"
    );
}
