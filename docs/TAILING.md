# Tailing Contract

External observers may watch deadreckon's durable JSONL ledgers directly — no
daemon, no socket, no CLI subprocess. This document is the supported contract
for that: which files are blessed for tailing, what every blessed file
guarantees, and how a correct reader consumes them. Files not listed here carry
no tailing guarantees, even if they happen to be JSONL today.

Consumers that prefer one merged stream over per-file tails can instead let
the binary run this contract for them: `deadreckon follow <id> --json` is the
blessed streaming reader over these same files (see
["The blessed streaming reader"](#the-blessed-streaming-reader-deadreckon-follow)
below). Direct file tailing remains fully supported either way.

The conformance test for this contract is
`crates/deadreckon/tests/tailing_contract.rs`; the follow stream's contract is
pinned by `crates/deadreckon/tests/follow_stream.rs`.

## Blessed files

Per Job, under `$DEADRECKON_HOME/jobs/<job_id>/`:

| file | one line is | checked schema |
|---|---|---|
| `job-events.jsonl` | one Job lifecycle fact | [`job-event.schema.json`](schemas/job-event.schema.json) |

Per run, under `$DEADRECKON_HOME/runstate/<scope>/runs/<run_id>/`:

| file | one line is | checked schema |
|---|---|---|
| `events.jsonl` | one run event | [`run-event.schema.json`](schemas/run-event.schema.json) |
| `spend.jsonl` | one spend record | [`spend-record.schema.json`](schemas/spend-record.schema.json) |
| `traces.jsonl` | one trace record | [`trace-record.schema.json`](schemas/trace-record.schema.json) |
| `flight-events.jsonl` | one provider flight event | [`flight-event.schema.json`](schemas/flight-event.schema.json) |
| `notify.jsonl` | one notify line (attention signal or delivery attempt) | [`notify-event.schema.json`](schemas/notify-event.schema.json) |
| `proofs/acceptance-progress.jsonl` | one advisory gate progress row | none (see below) |

Notes on individual files:

- `spend.jsonl` is a shared ledger: the run loop writes `kind == "loop"` rows
  and the live narrator writes `kind == "narrator"` rows to the same file.
  Only `loop` rows are the run's provider spend; do not sum across kinds.
- `notify.jsonl` carries two row shapes, both described by the checked
  schema and distinguished by the presence of a `kind` field. Rows with
  `kind: "operator_attention"` are typed operator-attention signals
  `{schema_version, kind, reason, job_id?, run_id?, scope?, at, summary,
  next_actions}` where `reason` is one of `verified_awaiting_promote |
  paused_at_cap | blocked | failed | cancelled | waiting_input`; each is
  appended once by the process that owns the transition, and a desktop
  companion may turn one row into one user notification. Rows without `kind`
  are delivery attempts `{ts, transition, channel, ok, detail?}` where
  `transition` is one of `accepted | paused | failed`; they record delivery
  *attempts*, including failures (`ok: false` with a `detail`). The file is
  append-only like every blessed file, but each append is best-effort: a
  transition can happen without its row landing, so treat this file as
  display-only observability — never as an authoritative delivery log, and
  never as evidence of the state it describes (`status --json` and the
  signed markers stay the source of truth). The converse also holds: a row
  can outlive the state it describes. In particular
  `verified_awaiting_promote` is appended when a receipt is newly sealed,
  and a bounded sealing attempt that is rolled back afterwards leaves the
  row behind; the successful reseal then appends a second row. Readers must
  dedupe (e.g. on `{reason, job_id}`) and re-check `status --json` before
  acting on any row.
- `proofs/acceptance-progress.jsonl` rows are
  `{checked_at, status, index, total, result?}`. They are display data only —
  never evidence. The signed acceptance marker is the only trustworthy record
  of what the gate concluded.

## Invariants

Every blessed file promises, unless the exception below says otherwise:

1. **Append-only.** Writers only append. Files are never rotated, renamed,
   truncated, or rewritten in place. Byte offsets you have already read stay
   valid forever.
2. **One JSON object per newline-terminated line.** Every line that ends in
   `\n` is one complete JSON object. Where a checked schema exists in
   [`docs/schemas/`](schemas/README.md), the object conforms to it.
3. **An unterminated final line is a torn append.** It is either an in-flight
   append that will complete shortly or residue of a crash mid-append. A
   reader must ignore it and retry from the same position; it must never parse
   a partial line, and must never treat one as corruption.
4. **Appends are fsynced.** Each writer syncs the file after every append, so
   a line that has fully appeared is durable.
5. **`job-events.jsonl` is strictly sequenced.** The `sequence` field runs
   `1..N` with no gaps, no reordering, and no rewrites: the writer refuses any
   out-of-order append, and it refuses to extend a history whose final row is
   torn. A gap, or two rows with the same `event_id` but different bytes, is
   corruption — render "unknown", never a guessed state. The writer also
   fsyncs each Job event before checkpointing `projection.json`, so the event
   ledger is never behind the projection it authorizes.

Damage to a *completed* line (invalid JSON before the final line) is
corruption, not a torn append. deadreckon's own readers fail closed on it;
external readers should surface it the same way rather than skip the row.

## Exception: `proofs/acceptance-progress.jsonl`

This file deliberately breaks invariant 1, twice:

- **It restarts on each gate attempt.** The trusted controller removes the
  file (together with the stale acceptance marker) at every gate-attempt
  entry, fail closed (`clear_stale_gate_attempt_evidence`), so evidence and
  display rows are always scoped to the current attempt; the evaluator
  additionally best-effort deletes it before streaming. Stale rows from an
  earlier attempt never mix with live ones.
- **It is rewritten once at sign time.** Trusted signing reconstructs the
  whole file from validated results and overwrites the streamed advisory rows
  (a plain truncate-and-write, not an atomic rename, and not fsynced). This is
  intentional: streamed rows come from the sandboxed evaluator and must never
  be mistaken for evidence, so the trusted controller replaces them. The
  rewrite happens after the evaluation's structural, contract, and tamper
  validation but before the acceptance decision: a valid-but-failing
  evaluation still gets the reconstructed file (with `failed` rows) even
  though the signer then refuses to write a marker, while an evaluation that
  fails validation is refused before any write and leaves the file untouched.

**Contained (strict) gate evaluations stream nothing.** The sandbox mounts
the run root read-only and denies every write under `proofs/` on all
backends, so a contained evaluator's advisory rows are silently dropped and
the file first appears whole at sign time. Only uncontained/non-strict
evaluations and manual `dr-gate evaluate` runs produce live rows; a tailer of
a strict gate must expect the file to be entirely absent until signing — and
to stay absent for that attempt if the signer refuses the evaluation before
reconstruction (see the rewrite bullet above).

Reader rule for this file only: treat ANY anomaly — the file shrinking below
your offset, disappearing, or a retained-offset read producing a line that
fails to parse — as a restart, not corruption. The sign-time rewrite reuses
the same inode and may leave the file the same length or longer than your
offset, landing your next read mid-line inside new content; reset the offset
to 0, discard retained rows, and re-read from the top. Within one attempt,
rows append normally and the other invariants hold. The other blessed files
keep the strict corruption rule below.

## Recommended reader algorithm

Keep a byte offset per file, initially 0, and on every poll:

1. `stat` the file. Missing file: nothing yet, keep offset 0 and retry later.
   Length below your offset: the file restarted (only possible for
   `acceptance-progress.jsonl`) — reset the offset to 0 and discard retained
   rows.
2. Open, seek to the offset, read to end of file, and advance the offset by
   the bytes read.
3. Prepend any bytes retained from the previous poll, then split at the *last*
   newline. Everything before it is complete lines; everything after it is a
   torn append — retain those bytes for the next poll without parsing them.
4. Parse each complete line as one JSON object. A parse failure here is
   corruption: report it and stop trusting the file, do not skip the line.
   Exception: for `acceptance-progress.jsonl` a parse failure means the
   sign-time rewrite landed under your offset — reset to 0 and re-read
   instead (see the exception section above).
5. For `job-events.jsonl`, additionally verify that `sequence` continues
   exactly `last + 1` from the previous row. Any gap is corruption.

This is the same algorithm deadreckon's own attach surfaces use
(`TuiEventFeed::file_tail` in `crates/deadreckon/src/tui_events.rs`,
`AttachJsonlTail` in `crates/deadreckon/src/main.rs`), so an external reader
built this way can never diverge from what the CLI itself displays.

Polling with a short interval is the supported mechanism. File-watch APIs
(FSEvents, inotify) are a fine wake-up hint, but the read path above must
still run as written — watchers can coalesce or drop events.

## The blessed streaming reader: `deadreckon follow`

`deadreckon follow <id> --json [--from <spec>]` runs the exact reader
algorithm above headlessly over every blessed file for one artifact and emits
the result as one merged NDJSON stream on stdout. It is the supported way to
consume these ledgers without implementing seven tails; everything above
stays the contract for readers that tail files directly.

- **References.** `<id>` resolves like every other read verb: durable JOB and
  RUN refs (plus plan-child refs; plans, chains, and campaigns redirect to
  `attach`). A Job follows its **current attempt run** — `projection.json`
  `child_run_ids`, newest last, the same rule `verdict` and `show` use — with
  the job's `job-events.jsonl` merged in. Following across attempt boundaries
  is out of scope: the stream stays on the attempt that was current at start
  (a retry is still visible as `job-events` rows), and following the new
  attempt means reconnecting.
- **Line shape.** Each line is one record:
  `{"source": <name>, "offset": <byte offset AFTER this record>,
  "generation": <file-generation token>, "record": <the parsed row
  verbatim>}` where `source` is one of
  `job-events | events | spend | traces | flight | acceptance-progress |
  notify` (`flight` is `flight-events.jsonl`; `job-events` appears only when
  the reference named a durable Job). `generation` is a short opaque token
  identifying the exact file the offset was read from (it changes when the
  file is replaced — a new attempt, or the acceptance-progress rewrite).
  Lines are merged in arrival order at poll granularity: appends to
  different files that land within one poll window are emitted in the fixed
  source order listed above, not true cross-file append order; per-source
  ordering is exact.
- **Replay.** `--from source=offset[@generation][,…]` resumes each named
  source from a previously emitted cursor, so a reconnect neither duplicates
  nor loses rows. Carry the `generation` token from the last line you
  consumed: follow verifies it before streaming, and a nonzero offset whose
  generation no longer matches the file refuses (append-only sources — the
  ledger the cursor came from no longer exists, e.g. a new attempt) or emits
  the restart marker (`acceptance-progress`) instead of silently skipping
  the new file's head. A bare `source=offset` is accepted but unverified —
  safe only for offset 0. Only cursors follow itself emitted are valid;
  sources not named in `--from` restart from 0. Unknown or unfollowed
  sources, malformed offsets, duplicate cursors, and an empty spec refuse
  with `try_lines`. A stale or mid-record nonzero offset that fails its
  first read is refused as an invalid cursor (reconnect that source from 0)
  rather than reported as ledger corruption.
- **Restart marker.** `proofs/acceptance-progress.jsonl` keeps its documented
  exception: on any rewrite anomaly under a retained offset follow emits one
  `{"source":"acceptance-progress","restart":true}` marker line, resets that
  source's offset to 0, and re-emits the file from the top — discard rows you
  retained for that source when you see the marker. One carve-out: a parse
  failure with no retained offset (offset 0) is corruption and fails closed
  even for this file — restarting would loop forever over the same bad
  bytes. Every other source keeps the strict corruption rule: follow fails
  closed (nonzero exit; with the armed `--json`, a `{"kind":"error",…}`
  envelope, serialized compactly so it is itself one valid NDJSON line even
  mid-stream) rather than skip or re-emit a row. `job-events` rows are
  additionally held to invariant 5: a `sequence` discontinuity fails the
  stream closed.
- **End of stream.** Once the artifact reaches a terminal phase (run:
  `completed`/`failed`/`killed`; job: projection phase `terminal`, plus the
  job's typed `outcome`) AND a poll drains nothing new, follow emits one
  final `{"terminal": true, "phase": …}` line and exits 0. Ctrl-C ends the
  stream with no final line (exit 130). When stdin is a pipe, closing it ends
  the stream quietly (exit 0): spawn follow with a piped stdin you hold open
  and drop it to disconnect — a supervising app that dies can never leak
  followers. Corollary: stdin from `/dev/null` is already closed, so follow
  emits one full drain of the backlog and exits — a snapshot, not a hang.
- **Dead-runner signal.** A run stuck at an executing phase with no live
  runner process behind it (the same pid-liveness/staleness rule `status`
  uses) gets one advisory `{"stalled": true, "phase": …, "run_id": …,
  "detail": …}` line. The stream stays open — cleanup or a resume can still
  move the state — but the consumer is told the terminal line may never
  arrive on its own and can apply its own timeout.
- **Read-only, poll-driven.** Follow never writes anything, anywhere. Its
  cadence is attach's budgeted idle backoff (16 ms doubling to 250 ms while
  idle, reset on activity).

## What this contract does not grant

Tailing is read-only observation. Rows never confer authority: acceptance is
proven only by the signed marker and validated receipt, spend rows are
provider evidence rather than a billing source of truth, and streamed gate
progress is cosmetic. Writing to any of these files from outside deadreckon
voids every guarantee above and is treated as corruption by its readers.
