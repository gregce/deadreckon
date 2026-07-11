# deadreckon — Capstan Rider (haul child processes with real machinery)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-11-1122-deadreckon-capstan-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: **`HeadTailBuffer`** bounded output capture, a typed
**`TruncationPolicy`**, the **`ChildTerminator`** trait with process-group
kill, and the **rewire** of provider capture (`cli_common.rs`), acceptance
checks (`gate.rs`), and the kill/cancellation paths through them.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Codex reference `/Users/gdc/codex/codex-rs` (pattern grounding only —
`core/src/unified_exec/head_tail_buffer.rs`, `utils/pty/`).

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Capstan` CHANGELOG section).
- **The on-disk full record is sacred.** `output_path` files and any full-output artifacts keep every byte, exactly as today. Truncation applies only to derived flows: ledger rows, `ProviderResponse.content` handed to narrators/parsers, attach panes. The omission marker always names the full-copy path.
- **Policies are types, not config.** `TruncationPolicy::{ledger, provider_content, pane}` named constructors with fixed budgets in code; no new config keys in this slice (operator tuning is V1).
- **Unix-first process groups.** New spawn helpers set `process_group(0)` (std `CommandExt`) on unix; kill escalates SIGTERM(group) → grace (default 3s) → SIGKILL(group). Windows compiles and keeps today's per-PID kill behind `RawPidTerminator`; group parity is V1.
- **Module placement:** a new `exec` module inside `deadreckon-core` (`crates/deadreckon-core/src/exec/`), NOT a new crate — providers and core both reach it; a standalone crate is V1 (and Keel's dependency-law question, not Capstan's).
- **No behavior change to sandbox backends** — the terminator wraps whatever pid the sandbox layer reports; sandbox-internal process management is out of scope.
- **No `PipelineState` schema changes. No `git push`. No V1 invention. Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (in-memory primitives; no new files)

### HeadTailBuffer

```rust
pub struct HeadTailBuffer {
    limit: usize,            // total byte budget
    head: Vec<u8>,           // first limit/2 bytes
    tail: VecDeque<u8>,      // last limit/2 bytes (ring)
    omitted_bytes: u64,
}
impl HeadTailBuffer {
    pub fn new(limit: usize) -> Self;
    pub fn push(&mut self, chunk: &[u8]);
    pub fn omitted_bytes(&self) -> u64;
    pub fn render(&self, full_copy: Option<&Path>) -> String;
    // render: head + "\n[… {omitted} bytes omitted{; full output: path}]\n" + tail
    // UTF-8 boundary-safe at both cut points (never split a code point).
}
```

### TruncationPolicy

```rust
#[derive(Clone, Copy, Debug)]
pub struct TruncationPolicy { pub limit_bytes: usize, pub label: &'static str }
impl TruncationPolicy {
    pub const fn ledger() -> Self          { Self { limit_bytes: 16 * 1024,  label: "ledger" } }
    pub const fn provider_content() -> Self{ Self { limit_bytes: 256 * 1024, label: "provider-content" } }
    pub const fn pane() -> Self            { Self { limit_bytes: 64 * 1024,  label: "pane" } }
    pub fn buffer(self) -> HeadTailBuffer;
}
```

Budgets above are the spec; changing one is a spec change (depth-tested
values). Every truncation site constructs via a named policy — a grep for
raw `HeadTailBuffer::new(` outside the policy module is a hygiene test.

### ChildTerminator

```rust
pub trait ChildTerminator: Send + Sync {
    fn terminate(&self, grace: Duration) -> TerminationOutcome; // TERM→wait→KILL
}
pub struct ProcessGroupTerminator { pgid: i32 }   // unix: signals -pgid
pub struct RawPidTerminator { pid: u32 }          // portable fallback
pub enum TerminationOutcome { ExitedInGrace, Killed, AlreadyDead, Failed(String) }
```

Spawn helper: `spawn_grouped(Command) -> (Child, Box<dyn ChildTerminator>)` —
on unix sets `process_group(0)` pre-exec and returns the group terminator;
elsewhere returns `RawPidTerminator`. The pid_file written for supervision
records the pgid alongside the pid (additive JSON key in the existing pid
file format; absent key ⇒ old behavior).

## Rewire map

| Site | Today | After |
|---|---|---|
| `cli_common.rs::run_cli*` capture | whole stdout in memory → `content` | stream into full-copy file (unchanged) + `provider_content()` buffer → `content` |
| provider trace/ledger rows | full stdout echoes | `ledger()` render with full-copy path |
| `gate.rs` check execution (:430/:499/:532) | `Command::new(…).output()` | `spawn_grouped` + streamed capture; check detail in proofs uses `ledger()` render |
| kill verb / cancellation token | per-PID signal | `ChildTerminator::terminate(grace)` via recorded pgid |
| attach activity pane feed | raw lines | `pane()` render (marker visible in TUI) |

## Phases (eleven)

Each phase: named depth test(s) first (red) → implement → `make verify` green
→ conventional-commit → CHANGELOG line naming the SHA.

### P1 — HeadTailBuffer
Depth tests (`crates/deadreckon-core/src/exec/`):
- `head_tail_keeps_both_ends_and_counts_omitted`
- `render_marker_names_full_copy_path`
- `utf8_boundaries_never_split`
- `under_limit_output_is_untouched`

### P2 — TruncationPolicy
Depth tests:
- `named_policies_pin_budgets`   (asserts the exact byte values)
- `no_raw_buffer_construction_outside_policy_module`  (grep hygiene)

### P3 — spawn_grouped + pid-file pgid
Depth tests:
- `spawned_child_is_its_own_process_group`
- `pid_file_gains_additive_pgid_key`
- `absent_pgid_key_reads_as_legacy`

### P4 — ChildTerminator impls
Depth tests:
- `group_terminate_kills_child_tree_no_orphans`  (spawn `sh -c 'sleep 60 & sleep 60'`, terminate, scan)
- `term_then_kill_escalation_honors_grace`
- `already_dead_child_reports_cleanly`

### P5 — Provider capture rewire
- `run_cli*` streams to the full-copy file and a `provider_content()` buffer; `CliOutput.stdout` becomes the rendered form; full file path unchanged.

Depth tests:
- `megabyte_stdout_yields_bounded_content_with_marker`
- `full_copy_file_holds_every_byte`
- `provider_traces_use_ledger_policy`

### P6 — Gate check rewire
- gate.rs checks run via `spawn_grouped`; proof detail excerpts use `ledger()`; check timeout (existing) uses the terminator.

Depth tests:
- `gate_check_child_tree_dies_on_timeout`
- `check_proof_excerpt_is_bounded_with_marker`

### P7 — Kill path rewire
- The kill verb and run cancellation terminate via pgid when recorded; kill/resume release-proof invariant becomes a test.

Depth tests:
- `kill_verb_terminates_process_group`
- `resume_after_group_kill_completes`   (fixture-level kill/resume)

### P8 — Attach pane policy
Depth tests:
- `pane_render_bounded_and_marker_visible_in_activity`

### P9 — Semaphore/flight interplay
- JSONL event parsing (Semaphore's `--json` stream) reads the FULL stream, never the truncated render — line-oriented parse happens during streaming, before truncation.

Depth tests:
- `jsonl_event_parse_sees_all_lines_despite_truncated_content`

### P10 — Friendliness
- `show --raw output` names the full-copy artifact; omission markers are stable wording (golden-safe); help/docs mention the full-output location.

Depth tests:
- `omission_marker_wording_is_pinned`
- `show_raw_serves_full_output_artifact`

### P11 — Architecture doc + CHANGELOG (doc only)
- Insert `## 53. Capstan: Child-Process Machinery` into AS-BUILT (buffer, policy table, terminator, group-kill escalation, full-record doctrine); update §35/§43 cross-references.
- CHANGELOG:
  ```
  ## Capstan (stable) — haul child processes with real machinery — <date>
  - bounded head+tail output capture with omitted-bytes markers (full record
    stays on disk), typed truncation policies at every site, and
    process-group kill with TERM→KILL escalation behind a ChildTerminator
    trait — gate checks and provider children die as trees, no orphans.
  ```
- V1-CANDIDATES: Windows job-object parity, operator-tunable budgets, PTY capture (interactive children), standalone exec crate.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| terminate failed (permission/zombie) | `try: deadreckon cleanup --stale` |
| full-copy file missing at render | marker degrades to byte count only (no path); no refusal |

## Out of scope (explicitly → V1-CANDIDATES)

- PTY-backed interactive exec (codex `unified_exec` full pattern).
- Windows process-group (job object) parity.
- Config-tunable truncation budgets.
- Applying policies to codex/claude child INTERNAL output (their own ledgers).
- Approval/sandbox-retry orchestration (Rudder territory).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: std (`CommandExt::process_group`), existing nix usage if already in
tree — check first; if signal delivery needs it, `nix` (signal + unistd
features only) is Tier 2, logged in DEPENDENCIES.md with a pin. Tier 3
(blocked): portable-pty/conpty (V1 with PTY work), tokio-process wrappers.

## Engineering invariants (do not violate)

- **Full record on disk, always** — truncation is presentation, never storage.
- **Every truncation names its policy**; budgets are depth-tested constants.
- **No orphans**: the group-kill test is the release-proof invariant made executable; it runs in CI, not just preflight.
- **UTF-8 safety at cut points** — pinned by test.
- **Marker wording is spec** (goldens depend on it).
- **One depth test before each phase.**

## Process invariants

- Phased local commits only. No `git push`.
- Each phase: depth tests green + CHANGELOG SHA line.
- P4/P6/P7 orphan-scan tests must be load-tolerant (poll with deadline, no fixed sleeps — the PTY-flake lesson).
- V1 discoveries logged, not implemented.
