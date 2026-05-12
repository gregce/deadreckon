# deadreckon — Robustness Rider (alpha hardening)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1400-deadreckon-robust-goal.md`. It
supersedes nothing in the earlier riders
(`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`) — their invariants,
dependency policy, UX commitments, sandbox defaults, and CLI surface still
apply. This rider adds depth contracts and a no-new-features fence around
the hardening work.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **No new features.** Depth, not breadth. If the build agent reaches for a
  new CLI verb, a new provider, a new sandbox backend, a new doc type, or a
  new V1-candidate idea, that's out of scope. Park it in
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue.
- **Drop V0 nomenclature.** The product is just deadreckon. Maturity tier is
  `alpha`. "V0" stays only in dated retrospective notes inside `CHANGELOG.md`.
- **Adversarial verification is mandatory.** Each hardening area must include
  a test that *would have caught the scaffolding-thin behavior the
  retrospective flagged*. A test that only exercises the happy path doesn't
  count.

## Ten hardening areas (one phase each)

### 1. TUI streaming

- Replace polling with a `tokio::sync::broadcast` channel emitted from
  `deadreckon-core::turn_loop` and consumed in the `ratatui` view.
- Live events: turn start, tool call started, tool call result, token usage
  delta, spend delta, error.
- Per-turn timer in the status bar; spend meter updates within 250 ms of the
  underlying event.
- Detach (`Ctrl-D`) cleanly without killing the run, with a status-bar
  reminder visible at all times.

**Depth tests:**
- `tui_streams_tool_call_within_250ms` — fixture run; assert the broadcast
  receiver gets the tool-call event before the polling interval would.
- `tui_detach_does_not_kill_run` — start, detach, verify run continues, then
  reattach and verify state preserved.

### 2. Resume from partial trace

- Tolerate `traces.jsonl` ending mid-tool-call (process killed between
  provider response and sandbox dispatch); reconstruct by replaying complete
  entries and resuming at the half-finished turn.
- Add `deadreckon resume <run-id> --from-turn <N>` to force resumption from
  a specific turn.
- Document the resumption ordering in `docs/RESUME-SEMANTICS.md`.

**Depth tests:**
- `resume_partial_trace_replays_history` — truncate `traces.jsonl` mid-entry;
  resume; verify the next turn starts at the right turn counter with the
  right history.
- `resume_from_turn_override` — `--from-turn 2` with a 5-turn history;
  verify history truncated to turn 2 in subsequent runs.

### 3. Cancellation model

- Hierarchical `tokio_util::sync::CancellationToken`:
  `run_token → turn_token → tool_token → child_token`.
- HTTP requests cancel via reqwest task abort on `tool_token.cancel()`.
- CLI subprocess providers + sandbox children receive `SIGTERM` then
  `SIGKILL` after 2 s.
- `kill --force` skips the SIGTERM grace window.

**Depth tests:**
- `kill_storm_no_leaks` — 10 concurrent `run` invocations, kill each at a
  random turn; assert: every child PID is gone after 5 s, every lock is
  released, no `state.json` remains in `executing` status.
- `kill_during_http_streaming` — long mock-streaming response; kill
  mid-stream; assert reqwest task aborted, no orphan socket.

### 4. Wall-clock spend for CLI providers

- Each CLI turn records `wall_time_seconds` in `spend.jsonl`.
- New flag `--max-wall-seconds <N>` (default from
  `defaults.cli_max_wall_seconds`, default 3600).
- When budget exhausted, pause and prompt resume just like `--max-spend`.

**Depth tests:**
- `cli_wall_clock_budget_enforced` — fixture CLI provider that sleeps 30 s;
  `--max-wall-seconds 10`; assert run pauses, state preserved, resume works.

### 5. Sandbox hardening

- Per-run Seatbelt profile (macOS) and bwrap profile (Linux) generated from
  the run config. Profile policy:
  - Allow filesystem read on workspace + system frameworks; deny elsewhere.
  - Allow network only to configured provider endpoints (or `*` for CLI
    sub-agent providers — they need it).
  - tmpfs over `$HOME` for the child process.
- Profile path: `/Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/sandbox/profile.sb`
  or `.bwrap-args`. Persisted for debugging.
- CLI provider subprocesses honor the same profile (sandbox the sub-agent).

**Depth tests:**
- `sandbox_blocks_ssh_read_macos` (gated on macOS) — prompt the smoke
  provider to emit a tool call that reads `/Users/gdc/.ssh/id_rsa`; assert
  the read fails with EPERM and the agent surfaces the error.
- `sandbox_blocks_outbound_to_evil_host` — tool call attempts `curl
  https://example.com`; assert blocked unless allowlisted.

### 6. Doctor exhaustiveness

`deadreckon doctor` checks (each with `✓` / `✗ + fix`):

- sandbox-exec / bwrap / docker binaries (with version)
- config file present + parseable
- each configured provider:
  - HTTP: cheapest-model ping (e.g., Anthropic `claude-haiku-4-5-20251001`
    or OpenAI `gpt-4o-mini`) with `<= 10 input tokens`; succeed or print the
    error verbatim
  - CLI: `which <binary>` + version
- disk space (warn at < 1 GB free in `/Users/gdc/.deadreckon/`)
- write permissions on runstate dir
- OS/kernel version (sanity check: macOS ≥ 13, Linux ≥ 5.13 for landlock
  hint)
- subscription detection: `claude --version`, `codex --version`

**Depth test:**
- `doctor_fails_actionably` — break each prerequisite in turn (remove
  config, revoke key, fill disk to 100 MB free in a temp HOME); assert
  doctor prints the specific fix command for each.

### 7. Import normalization

- `deadreckon import claude-code` parses every JSONL in
  `/Users/gdc/.claude/projects/` and writes equivalent `traces.jsonl` +
  `provenance.jsonl` under a synthesized `runs/imported-<hash>/` directory.
- Same for `~/.codex/sessions/` (JSONL) and `~/.cursor/chats/` (SQLite).
- Round-trip: import → render via `deadreckon show <imported-id>` →
  comparable shape to the source.

**Depth tests:**
- `import_claude_code_roundtrip` — fixture JSONL → import → show; assert
  ≥ 1 tool call, ≥ 1 file edit, matching turn count.
- `import_codex_roundtrip`, `import_cursor_roundtrip` — same pattern.

### 8. Acceptance spec for dr-gate

YAML schema at `/Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/acceptance.yaml`:

```yaml
name: optional human-readable
checks:
  - kind: cargo_test
    args: ["--workspace"]
    must_pass: true
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: content_match
    path: "{working_dir}/notes.md"
    pattern: "dead reckoning"
  - kind: build_success
    cwd: "{working_dir}"
```

- `dr-gate --run <id>` reads the spec, runs each check, writes
  `proofs/turn-acceptance.json` only if all `must_pass` checks pass.
- The deadreckon binary refuses to mark a run `completed` without a valid
  marker.

**Depth tests:**
- `acceptance_yaml_parsed_and_evaluated` — fixture spec; dr-gate produces
  the marker only when checks pass.
- `self_attest_attempt_fails` — agent writes a forged marker via a tool
  call; deadreckon refuses the run-id signature.

### 9. Multi-run coordination

- Document the lock-ordering contract in `docs/MULTI-RUN.md`: scope-level
  lock first, then run-level lock; never the reverse.
- Test 5 concurrent runs in 5 distinct scopes for 10 minutes (mock provider,
  deterministic 100-turn task each); assert: each run completes, no
  cross-scope `provenance.jsonl` contamination, every lock released.
- Test 2 concurrent runs in the **same** scope: second one blocks or fails
  with a clear error pointing at the first.

**Depth tests:**
- `concurrent_runs_no_state_bleed`
- `same_scope_second_run_refused_with_hint`

### 10. Promotion / library workflow

- After `dr-gate` writes the acceptance marker, deadreckon atomically swaps
  `working/<run-id>` → `library/<run-id>/` (rename), writes
  `library/<run-id>/manifest.json` with run identity + provenance hash, and
  releases the lock.
- Crash between rename and manifest-write is recoverable: restart detects
  the orphan, completes the manifest if data is intact, or rolls back.

**Depth tests:**
- `promotion_atomic_under_crash` — `kill -9` between the rename and the
  manifest-write; restart; assert the promotion completes or rolls back
  cleanly, never half-done.

## V0 nomenclature cleanup (concrete edits)

- `/Users/gdc/deadreckon/README.md` — replace "V0 scaffolding" framing with
  "deadreckon (alpha)".
- `/Users/gdc/deadreckon/DESIGN.md` — keep "V0 decisions" only in a section
  titled "Initial build decisions (locked)".
- `/Users/gdc/deadreckon/CHANGELOG.md` — keep "V0" in retrospective entries
  only; new entries use semver-style (`0.1.0`, `0.2.0`).
- Source comments — replace `// V0:` markers with `// alpha:` or remove if
  trivial.
- `Cargo.toml` — set `version = "0.1.0"` on all crates; add
  `categories = ["development-tools"]`, `keywords = ["agent","cli","coding"]`.
- Allowed contexts for the literal string `V0`: the two retrospective
  documents in `docs/` (`GAP-ANALYSIS.md`, `CHANGELOG.md` historical entries).

## Stress & adversarial harness

`/Users/gdc/deadreckon/tests/harness/`:

- `stress_5_concurrent_10min.rs` — gated by `DEADRECKON_STRESS=1`; spawns 5
  scopes, runs each for 10 min against the mock provider with a 100-turn
  fixture; asserts no leaks.
- `adversarial_sandbox_escape.rs` — gated by OS; fixture prompt that emits
  tool calls reading restricted paths; asserts blocked.
- `adversarial_self_attest.rs` — fixture prompt that emits a tool call
  writing a forged acceptance marker; asserts refused.

## Dependencies (per Tier 1 / 2 / 3 policy from earlier riders)

Tier 1 (utility, free):
- `serde_yaml` — acceptance spec parsing.
- `tokio-stream` — broadcast helpers for TUI streaming.

Tier 2 (architectural, log to `DEPENDENCIES.md`):
- None expected. If the agent reaches for one, justify per the original
  rider's policy.

Tier 3 (blocked):
- Same blocks as earlier riders.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the depth test for that area passing and a CHANGELOG
  entry naming the SHA.
- If a hardening item genuinely requires a V1 architectural decision, stop
  and log it in `V1-CANDIDATES.md`; do not silently expand scope.
- After the final phase, append a "Robustness milestone (alpha)" section to
  `CHANGELOG.md` summarizing what hardened, what surprised, what's still
  thin.
