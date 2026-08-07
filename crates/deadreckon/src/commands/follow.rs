//! Gap G5 step 2: `deadreckon follow <id> --json [--from <spec>]` — the
//! merged streaming surface over the blessed tailing ledgers.
//!
//! Follow is the machine sibling of `attach`: one NDJSON stream on stdout
//! that merges every blessed JSONL ledger docs/TAILING.md lists for one
//! artifact. Each line is `{"source": <name>, "offset": <byte offset AFTER
//! this record>, "generation": <file-generation token>, "record": <the
//! parsed object verbatim>}`; `offset@generation` is the replay cursor
//! `--from source=offset[@generation][,…]` accepts on reconnect, so a
//! consumer resumes without duplication or loss — the generation token binds
//! the offset to the file it was emitted for, and a cursor whose generation
//! no longer matches fails closed (append-only sources) or restarts
//! (`acceptance-progress`) instead of silently skipping the new file's head.
//! The one file allowed to rewrite, `proofs/acceptance-progress.jsonl`,
//! follows its documented restart rule: on any rewrite anomaly under a
//! retained offset follow emits a
//! `{"source":"acceptance-progress","restart":true}` marker line and then
//! re-emits the file from offset 0. `job-events` rows are additionally held
//! to invariant 5: a `sequence` discontinuity fails the stream closed. The
//! stream ends with one `{"terminal": true, "phase": …}` line once the
//! artifact reached a terminal phase AND the tails are drained, or earlier on
//! SIGINT (the default handler; exit 130, no final line) or stdin close
//! (exit 0). A run whose Executing state has no live runner behind it gets
//! one advisory `{"stalled": true, …}` line (the same staleness rule status
//! uses) so a machine consumer is never left polling a dead run unaware.
//!
//! Resolution rides the shared reference resolver: JOB and RUN refs. A
//! durable Job maps onto its CURRENT attempt run — the same
//! `projection.json` `child_run_ids`-newest-last rule verdict and show use —
//! with the job's `job-events.jsonl` merged in; following across attempt
//! boundaries is out of scope (reconnect after `job-events` shows a new
//! attempt). The tail machinery is `AttachJsonlTail::poll_follow` — the
//! existing torn-tail reader run headlessly, never a reimplementation — and
//! the polling cadence reuses attach's budgeted idle backoff constants.
//! Follow is read-only: it never writes anything, anywhere.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::*;
use super::attach_runtime::{AttachIdleBackoff, attach_tick_budget_from_config};

/// Parsed `deadreckon follow` arguments.
pub(crate) struct FollowArgs {
    pub(crate) run_id: String,
    pub(crate) json: bool,
    pub(crate) from: Option<String>,
}

/// Every source name follow can ever emit, in emission order. `job-events`
/// joins the stream only when the reference named a durable Job.
const FOLLOW_SOURCE_NAMES: [&str; 7] = [
    "job-events",
    "events",
    "spend",
    "traces",
    "flight",
    "acceptance-progress",
    "notify",
];

/// One followed ledger: its blessed source name, the shared tail, whether the
/// tailing contract lets the file restart
/// (`proofs/acceptance-progress.jsonl` only), and the replay bookkeeping the
/// `--from` contract needs.
struct FollowSource {
    name: &'static str,
    restartable: bool,
    /// Salt for the generation token: the owning artifact id (job id for
    /// `job-events`, attempt run id otherwise), so byte-identical first lines
    /// in different attempts still hash to different generations.
    identity: String,
    tail: AttachJsonlTail<Value>,
    /// The generation token of the file the retained offset belongs to:
    /// a short hash of the identity salt plus the file's first complete
    /// line. Emitted on every record line; a change under a retained offset
    /// is a rewrite that evaded the length checks.
    generation: Option<String>,
    /// The generation the `--from` cursor claimed, verified before streaming.
    expected_generation: Option<String>,
    /// The byte offset the `--from` cursor supplied, kept for refusal
    /// wording.
    from_offset: Option<u64>,
    /// True while an unverified nonzero `--from` offset has not survived a
    /// poll yet: a failure then is blamed on the cursor, not the ledger.
    from_pending: bool,
    /// Emit one restart marker before the next poll (set when `--from`
    /// verification finds the restartable file's generation changed).
    pending_restart_marker: bool,
    /// Last `sequence` seen on `job-events`, for the invariant-5 gap check.
    last_sequence: Option<u64>,
}

impl FollowSource {
    fn new(name: &'static str, restartable: bool, identity: &str, path: PathBuf) -> Self {
        Self {
            name,
            restartable,
            identity: identity.to_string(),
            tail: AttachJsonlTail::new(path),
            generation: None,
            expected_generation: None,
            from_offset: None,
            from_pending: false,
            pending_restart_marker: false,
            last_sequence: None,
        }
    }

    fn append_only(name: &'static str, identity: &str, path: PathBuf) -> Self {
        Self::new(name, false, identity, path)
    }

    /// Invariant 5 (docs/TAILING.md): `job-events` `sequence` runs `1..N`
    /// with no gaps. A fresh stream from offset 0 must observe sequence 1
    /// first; a `--from` resume seeds from the first replayed row and
    /// enforces `last + 1` from there. Any discontinuity is corruption.
    fn check_job_events_sequence(&mut self, record: &Value) -> Result<()> {
        if self.name != "job-events" {
            return Ok(());
        }
        let Some(sequence) = record.get("sequence").and_then(Value::as_u64) else {
            return Err(follow_tail_corruption(
                &self.tail.path,
                "a job-events row carries no numeric sequence (invariant 5)",
            ));
        };
        let expected = match self.last_sequence {
            Some(last) => Some(last.saturating_add(1)),
            None if self.from_offset.unwrap_or(0) == 0 => Some(1),
            None => None,
        };
        if let Some(expected) = expected
            && sequence != expected
        {
            return Err(follow_tail_corruption(
                &self.tail.path,
                &format!(
                    "job-events sequence {sequence} arrived where {expected} was required \
                     (invariant 5: sequence continues exactly last+1, no gaps, no reordering)"
                ),
            ));
        }
        self.last_sequence = Some(sequence);
        Ok(())
    }
}

/// The generation token for one source file: a short hash of the identity
/// salt plus the file's first complete line. `None` while the file is
/// missing, empty, or its first line is still torn — there is no generation
/// to bind a cursor to yet. Append-only ledgers never change their first
/// line, so a changed token under a retained offset always means a rewrite.
fn source_generation(identity: &str, path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = io::BufReader::new(file);
    let mut first_line = Vec::new();
    reader.read_until(b'\n', &mut first_line).ok()?;
    if first_line.last() != Some(&b'\n') {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    hasher.update([0u8]);
    hasher.update(&first_line);
    let digest = hasher.finalize();
    Some(
        digest
            .iter()
            .take(6)
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// The `{"source":…,"restart":true}` marker line announcing that a
/// restartable source is about to re-emit from offset 0.
fn restart_marker(name: &str) -> Value {
    json!({ "source": name, "restart": true })
}

/// What follow re-reads each poll to decide the stream is over — or stalled.
enum FollowTerminalProbe {
    Run {
        state_path: PathBuf,
    },
    Job {
        job_id: String,
        attempt_state_path: PathBuf,
    },
}

/// One probe result: the artifact is terminal (final line to emit once
/// drained), visibly stalled (advisory line, stream continues), or live.
enum FollowProbeSignal {
    Terminal(Value),
    Stalled(Value),
    Live,
}

impl FollowTerminalProbe {
    /// `Terminal(final line)` once the artifact reached a terminal phase. The
    /// phase is the serialized lifecycle enum (`completed`/`failed`/`killed`
    /// for runs, `terminal` for jobs — jobs additionally carry their typed
    /// `outcome`, since "terminal" alone names no result). A non-terminal
    /// state whose runner is provably dead (`is_stale_executing`, the same
    /// pid-liveness/staleness rule `status` and cleanup use) yields
    /// `Stalled` so a machine consumer is told the terminal line may never
    /// come.
    fn probe(&self, paths: &DeadreckonPaths) -> Result<FollowProbeSignal> {
        match self {
            Self::Run { state_path } => {
                let state = load_state(state_path)?;
                if matches!(
                    state.status,
                    RunStatus::Completed | RunStatus::Failed | RunStatus::Killed
                ) {
                    return Ok(FollowProbeSignal::Terminal(
                        json!({ "terminal": true, "phase": state.status }),
                    ));
                }
                Ok(stalled_signal(&state))
            }
            Self::Job {
                job_id,
                attempt_state_path,
            } => {
                let projection = deadreckon_core::load_job_projection(paths, job_id)?;
                if projection.is_terminal() {
                    return Ok(FollowProbeSignal::Terminal(json!({
                        "terminal": true,
                        "phase": projection.phase,
                        "outcome": projection.outcome,
                    })));
                }
                let state = load_state(attempt_state_path)?;
                Ok(stalled_signal(&state))
            }
        }
    }
}

/// The advisory dead-runner signal: an Executing state with no live
/// supervised pid (or gone stale past the staleness window) will never reach
/// a terminal phase on its own, so the consumer gets one typed line instead
/// of an indefinitely silent poll.
fn stalled_signal(state: &deadreckon_core::PipelineState) -> FollowProbeSignal {
    if is_stale_executing(state) {
        FollowProbeSignal::Stalled(json!({
            "stalled": true,
            "phase": state.status,
            "run_id": state.run_id,
            "detail": "no live runner process backs this executing run (or its state has gone \
                       stale); it may never reach a terminal phase",
        }))
    } else {
        FollowProbeSignal::Live
    }
}

/// Stream one artifact's blessed ledgers as merged NDJSON.
pub(crate) async fn follow_command(args: FollowArgs) -> Result<()> {
    if !args.json {
        return Err(CliError::Core(deadreckon_core::user_error(
            "follow is a machine streaming surface and requires --json",
            &format!("deadreckon follow {} --json", args.run_id),
        )));
    }
    let paths = DeadreckonPaths::discover();
    let resolved = super::reference::resolve_ref(
        &paths,
        super::reference::RefQuery {
            reference: Some(&args.run_id),
            all_scopes: false,
            verb: "follow",
        },
    )?;
    let (probe, state, job_id) = match resolved {
        super::reference::ResolvedRef::Job(view) => {
            // The job's current attempt run, resolved exactly the way
            // verdict and show resolve it, so no two read verbs can disagree
            // about which attempt is "current". Follow stays on this attempt
            // for its whole life; a retry that spawns a new attempt run is
            // visible in job-events, and following it means reconnecting.
            let Some(state) = super::verdict::try_current_attempt_state(&paths, &view)? else {
                let prefix = run_prefix(view.job.job_id.as_ref());
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "job {prefix} has no attempt run yet, and follow streams the current \
                         attempt's ledgers; reconnect once the supervisor starts the first \
                         attempt (jobs/{}/job-events.jsonl records attempt starts)",
                        view.job.job_id.as_ref()
                    ),
                    &format!("deadreckon status {prefix}"),
                )));
            };
            let job_id = view.job.job_id.as_ref().to_string();
            (
                FollowTerminalProbe::Job {
                    job_id: job_id.clone(),
                    attempt_state_path: state.state_path(),
                },
                state,
                Some(job_id),
            )
        }
        super::reference::ResolvedRef::Run(state) => (
            FollowTerminalProbe::Run {
                state_path: state.state_path(),
            },
            *state,
            None,
        ),
        super::reference::ResolvedRef::PlanChild { state, .. } => (
            FollowTerminalProbe::Run {
                state_path: state.state_path(),
            },
            *state,
            None,
        ),
        // The resolver already refuses kinds outside follow's acceptance
        // matrix; this arm only restates that refusal for exhaustiveness.
        other => {
            return Err(super::reference::refusal_for(
                other.kind(),
                "follow",
                &super::reference::resolved_id(&other),
            ));
        }
    };
    let mut sources = follow_sources(&paths, &state, job_id.as_deref());
    if let Some(spec) = args.from.as_deref() {
        apply_from_spec(spec, &mut sources, &args.run_id)?;
        verify_from_cursors(&mut sources, &args.run_id)?;
    }
    stream_follow(&paths, &probe, &mut sources, &args.run_id).await
}

/// The blessed tails for one artifact, in `FOLLOW_SOURCE_NAMES` order — the
/// source-name list the follow section of docs/TAILING.md documents, which
/// is also the per-poll emission order. `job-events` lives under
/// `jobs/<job_id>/` and exists only for durable Jobs; every other source
/// lives under the attempt run's root.
fn follow_sources(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    job_id: Option<&str>,
) -> Vec<FollowSource> {
    let mut sources = Vec::new();
    if let Some(job_id) = job_id {
        sources.push(FollowSource::append_only(
            "job-events",
            job_id,
            paths.job_events(job_id),
        ));
    }
    let run_root = &state.run_root;
    let run_id = state.run_id.as_str();
    sources.push(FollowSource::append_only(
        "events",
        run_id,
        run_root.join(RUN_EVENTS_JSONL),
    ));
    sources.push(FollowSource::append_only(
        "spend",
        run_id,
        run_root.join("spend.jsonl"),
    ));
    sources.push(FollowSource::append_only(
        "traces",
        run_id,
        run_root.join("traces.jsonl"),
    ));
    sources.push(FollowSource::append_only(
        "flight",
        run_id,
        run_root.join(FLIGHT_EVENTS_JSONL),
    ));
    sources.push(FollowSource::new(
        "acceptance-progress",
        true,
        run_id,
        acceptance_progress_path_for_run_root(run_root),
    ));
    sources.push(FollowSource::append_only(
        "notify",
        run_id,
        deadreckon_core::notify_jsonl_path(run_root),
    ));
    sources
}

/// Apply `--from source=offset[@generation][,…]`: replay cursors exactly as a
/// previous follow emitted them. Unknown or unfollowed sources refuse with
/// the valid names in the try line, so a reconnecting consumer can never
/// silently drop a ledger; malformed offsets, duplicate cursors, and an
/// empty spec refuse rather than guess.
fn apply_from_spec(spec: &str, sources: &mut [FollowSource], reference: &str) -> Result<()> {
    let followed = sources
        .iter()
        .map(|source| source.name)
        .collect::<Vec<_>>()
        .join(", ");
    let mut named: Vec<&'static str> = Vec::new();
    for pair in spec
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let Some((name, cursor)) = pair.split_once('=') else {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("--from entry {pair:?} is not of the form source=offset"),
                &format!("deadreckon follow {reference} --json --from events=0"),
            )));
        };
        let name = name.trim();
        let Some(source) = sources.iter_mut().find(|source| source.name == name) else {
            let message = if FOLLOW_SOURCE_NAMES.contains(&name) {
                format!(
                    "source {name} is not part of this artifact's follow stream \
                     (a run without a durable Job has no job-events ledger); \
                     this stream carries: {followed}"
                )
            } else {
                format!("unknown follow source {name:?}; this stream carries: {followed}")
            };
            return Err(CliError::Core(deadreckon_core::user_error(
                &message,
                &format!("deadreckon follow {reference} --json --from events=0"),
            )));
        };
        if named.contains(&source.name) {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("--from names {name} twice; a reconnect carries one cursor per source"),
                &format!("deadreckon follow {reference} --json --from {name}=0"),
            )));
        }
        named.push(source.name);
        let cursor = cursor.trim();
        let (offset_text, generation) = match cursor.split_once('@') {
            Some((offset, generation)) => (offset.trim(), Some(generation.trim())),
            None => (cursor, None),
        };
        if generation == Some("") {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("--from cursor for {name} has an empty generation token after '@'"),
                &format!("deadreckon follow {reference} --json --from {name}=0"),
            )));
        }
        let offset = offset_text.parse::<u64>().map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                &format!(
                    "--from offset for {name} must be a byte offset previously \
                     emitted by follow (got {offset_text:?})"
                ),
                &format!("deadreckon follow {reference} --json --from {name}=0"),
            ))
        })?;
        source.tail.offset = offset;
        source.from_offset = Some(offset);
        source.from_pending = offset > 0;
        source.expected_generation = generation.map(str::to_string);
    }
    if named.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--from is empty; name at least one source=offset cursor (or drop --from to stream \
             everything from the top)",
            &format!("deadreckon follow {reference} --json --from events=0"),
        )));
    }
    Ok(())
}

/// Verify every `--from` cursor that carried a generation token before
/// streaming: a nonzero offset is only meaningful inside the file generation
/// it was emitted for. A mismatch on an append-only source fails closed —
/// the ledger this cursor came from no longer exists, and resuming by byte
/// offset would silently lose the new generation's head. A mismatch on the
/// restartable source follows its documented rule: restart marker, offset 0.
/// Cursors without a generation token stay accepted but unverified (offset 0
/// is always safe; docs/TAILING.md tells reconnecting consumers to carry the
/// token).
fn verify_from_cursors(sources: &mut [FollowSource], reference: &str) -> Result<()> {
    for source in sources.iter_mut() {
        let Some(expected) = source.expected_generation.clone() else {
            continue;
        };
        if source.tail.offset == 0 {
            continue;
        }
        let current = source_generation(&source.identity, &source.tail.path);
        if current.as_deref() == Some(expected.as_str()) {
            source.generation = current;
            source.from_pending = false;
            continue;
        }
        if source.restartable {
            source.tail.offset = 0;
            source.tail.signature = None;
            source.from_pending = false;
            source.pending_restart_marker = true;
            continue;
        }
        let observed = current.map_or_else(
            || "the ledger has no complete first line yet".to_string(),
            |generation| format!("the ledger is now generation {generation}"),
        );
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "the --from cursor {}={}@{expected} names a ledger generation that no longer \
                 matches {} ({observed}); offsets from one generation are not valid in another",
                source.name,
                source.from_offset.unwrap_or(source.tail.offset),
                source.tail.path.display(),
            ),
            &format!(
                "deadreckon follow {reference} --json --from {}=0",
                source.name
            ),
        )));
    }
    Ok(())
}

/// A poll failed while an unverified nonzero `--from` offset had not yet
/// survived a read: the overwhelmingly likely cause is the cursor (stale,
/// mid-record, or from another file generation), not writer corruption, so
/// the refusal blames the cursor and offers the recovery the operator
/// actually needs — reconnect that source from 0.
fn bad_from_cursor(source: &FollowSource, reference: &str, error: &CliError) -> CliError {
    let detail = error
        .to_string()
        .lines()
        .filter(|line| !line.trim_start().starts_with("try:"))
        .collect::<Vec<_>>()
        .join(" ");
    CliError::Core(deadreckon_core::user_error(
        &format!(
            "the --from cursor {}={} does not line up with {}; the cursor is stale, mid-record, \
             or from a different file generation ({detail})",
            source.name,
            source.from_offset.unwrap_or(source.tail.offset),
            source.tail.path.display(),
        ),
        &format!(
            "deadreckon follow {reference} --json --from {}=0",
            source.name
        ),
    ))
}

/// The poll-emit loop. Polling with attach's budgeted idle backoff is the
/// supported cadence (docs/TAILING.md); every cycle polls each tail once in
/// the fixed source order and emits what arrived, so "merged in arrival
/// order" holds at poll granularity (within one cycle, sources drain in the
/// fixed listed order; per-source order is exact).
async fn stream_follow(
    paths: &DeadreckonPaths,
    probe: &FollowTerminalProbe,
    sources: &mut [FollowSource],
    reference: &str,
) -> Result<()> {
    let mut idle = AttachIdleBackoff::from_budget(attach_tick_budget_from_config(paths));
    let stdin_closed = watch_stdin_close();
    let mut out = io::stdout().lock();
    let mut terminal_line: Option<Value> = None;
    let mut stalled_emitted = false;
    loop {
        let mut activity = false;
        'sources: for source in sources.iter_mut() {
            if source.pending_restart_marker {
                source.pending_restart_marker = false;
                activity = true;
                if !write_ndjson_line(&mut out, &restart_marker(source.name))? {
                    return Ok(());
                }
            }
            let poll = match source.tail.poll_follow(source.restartable) {
                Ok(poll) => poll,
                Err(error) if source.from_pending => {
                    return Err(bad_from_cursor(source, reference, &error));
                }
                Err(error) => return Err(error),
            };
            source.from_pending = false;
            if poll.restarted {
                activity = true;
                source.generation = None;
                if !write_ndjson_line(&mut out, &restart_marker(source.name))? {
                    return Ok(());
                }
            }
            if !poll.records.is_empty() {
                // Bind the retained offset to the file generation it was
                // read from. A changed first line means a rewrite whose
                // length and line boundaries evaded the offset checks — the
                // polled rows belong to a new file whose head was never
                // emitted.
                let current = source_generation(&source.identity, &source.tail.path);
                match (&source.generation, current) {
                    (Some(previous), Some(current)) if *previous != current => {
                        if source.restartable {
                            source.tail.offset = 0;
                            source.tail.signature = None;
                            source.generation = None;
                            activity = true;
                            if !write_ndjson_line(&mut out, &restart_marker(source.name))? {
                                return Ok(());
                            }
                            continue 'sources;
                        }
                        return Err(follow_tail_corruption(
                            &source.tail.path,
                            "the ledger's first line changed under a retained offset \
                             (a rewrite evaded the length checks)",
                        ));
                    }
                    (None, current) => source.generation = current,
                    _ => {}
                }
            }
            for (offset, record) in poll.records {
                source.check_job_events_sequence(&record)?;
                activity = true;
                if !write_ndjson_line(
                    &mut out,
                    &json!({
                        "source": source.name,
                        "offset": offset,
                        "generation": source.generation,
                        "record": record,
                    }),
                )? {
                    return Ok(());
                }
            }
        }
        if activity {
            let _ = out.flush();
        }
        if let Some(line) = &terminal_line {
            // Terminal was observed on an earlier cycle; the stream ends on
            // the first subsequent cycle that drains nothing new, so every
            // append fsynced before the terminal transition is emitted first.
            if !activity {
                let _ = write_ndjson_line(&mut out, line)?;
                let _ = out.flush();
                return Ok(());
            }
        } else {
            match probe.probe(paths)? {
                FollowProbeSignal::Terminal(line) => terminal_line = Some(line),
                FollowProbeSignal::Stalled(line) => {
                    // Advisory, at most once per session: the stream stays
                    // open (cleanup or a resume can still move the state),
                    // but the consumer knows the terminal line may never
                    // arrive on its own.
                    if !stalled_emitted {
                        stalled_emitted = true;
                        if !write_ndjson_line(&mut out, &line)? {
                            return Ok(());
                        }
                        let _ = out.flush();
                    }
                }
                FollowProbeSignal::Live => {}
            }
        }
        if stdin_closed
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            // The consumer owns the stream through the pipe; closing it ends
            // the follow quietly, without a terminal line.
            return Ok(());
        }
        if activity {
            idle.reset();
        } else {
            idle.record_idle();
        }
        tokio::time::sleep(idle.current_delay()).await;
    }
}

/// Write one compact NDJSON line. `Ok(false)` means the consumer closed the
/// pipe and the stream should end quietly (exit 0), the same way a pager
/// quitting ends a stream; every other write failure propagates.
fn write_ndjson_line(out: &mut impl Write, value: &Value) -> Result<bool> {
    let line = serde_json::to_string(value)?;
    match writeln!(out, "{line}") {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Subprocess-supervision contract: when stdin is a pipe, the spawning
/// process owns the stream's lifetime and closing the pipe ends the follow —
/// so an app that dies can never leak followers. A TTY stdin never signals
/// EOF; Ctrl-C (the default SIGINT disposition, exit 130) ends the stream
/// there. Note the corollary: with stdin from /dev/null the pipe is already
/// closed, so follow emits one full drain of the backlog and exits — a
/// snapshot, not a hang.
fn watch_stdin_close() -> Option<Arc<AtomicBool>> {
    if io::stdin().is_terminal() {
        return None;
    }
    let closed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&closed);
    let spawned = std::thread::Builder::new()
        .name("follow-stdin-close".to_string())
        .spawn(move || {
            let mut sink = [0u8; 4096];
            let mut stdin = io::stdin();
            loop {
                match stdin.read(&mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            flag.store(true, Ordering::SeqCst);
        });
    spawned.is_ok().then_some(closed)
}
