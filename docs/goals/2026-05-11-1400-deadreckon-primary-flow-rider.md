# deadreckon — Primary-Flow Rider (V0 completion)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1400-deadreckon-primary-flow-goal.md`.
It supersedes nothing in the original V0 rider
(`/Users/gdc/deadreckon/docs/goals/2026-05-10-1400-deadreckon-build-rider.md`) — that
rider's invariants, dependency policy, UX commitments, file-system layout,
and engineering style **still apply**. This rider adds the contracts needed
to make the primary agentic flow real.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Why we're here (the audit)

The V0 build report claimed completion, but a closer look found the primary
flow was a smoke fallback:

- `deadreckon run` invoked a hardcoded `coding_turn_script()` that wrote a
  fixed Rust project; no LLM was called.
- `ProviderRouter::complete` existed but was unused by the run loop.
- "Long-running unattended coding task" was not real: no iterative
  model→tool→result loop, no bounded retries beyond snapshots.
- `kill` could not terminate live work (synchronous run, no child PID
  supervision).
- `resume` reset state instead of continuing.
- `demo.cast` was hand-authored, not recorded.
- Anti-self-attestation gate was unimplemented.
- Cross-tool import only listed inventory.

This rider locks in what "real" requires and how to prove it **without API
keys** (mock provider) plus optional **subscription-driven live tests** via
the `claude` and `codex` CLIs that exist on this machine.

## Provider model (decided — extends V0 rider)

A `Provider` implementation may be backed by either an HTTP endpoint or a
subprocess CLI. The router does not care; the run loop calls `complete()`.

### HTTP-backed providers (already adapted; now must be wired)

- `anthropic` — Anthropic Messages API.
- `openai` — OpenAI Chat Completions / Responses API.
- `openai-compatible` — generic OpenAI-shaped endpoints (OpenRouter,
  llama.cpp, vLLM, LiteLLM proxy).

### CLI sub-agent providers (new in this rider)

These let users with **Claude Max** or **ChatGPT Pro** subscriptions drive
deadreckon end-to-end without raw API keys. Subscription-BYOK is the
deadreckon BYOK posture extended.

- **`cli:claude-code`** — invokes:

  ```zsh
  claude --dangerously-skip-permissions -p "<prompt>" \
    > /Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/turns/turn-<N>/claude.out 2>&1
  ```

  `--dangerously-skip-permissions` is required because no human is in the
  loop. The Codex/Claude binary runs **inside the deadreckon sandbox profile**
  (sandbox-exec / bwrap), so even though Claude's own permission model is
  bypassed, deadreckon's outer sandbox still scopes the process.

- **`cli:codex`** — invokes:

  ```zsh
  codex exec "<prompt>" \
    > /Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/turns/turn-<N>/codex.out 2>&1
  ```

  Verify the exact non-interactive verb against `codex --help` at build
  time; document the chosen verb in `crates/deadreckon-providers/src/cli_codex.rs`
  with a code comment citing the help output.

### Configuration

`/Users/gdc/.deadreckon/config.toml`:

```toml
[providers.cli-claude-code]
binary = "claude"  # resolved via `which` if unset
extra_args = []

[providers.cli-codex]
binary = "codex"
extra_args = []

[defaults]
provider = "cli:claude-code"  # if no other credentials
fallback_chain = ["cli:claude-code", "cli:codex", "anthropic", "openai"]
```

`deadreckon init` detects `claude` and `codex` in `$PATH` and offers them as
provider options before asking for API keys. If a CLI provider is selected
and reachable, no API key is required for V0.

### Spend accounting

- HTTP providers: usage tokens × per-token price → USD entry in `spend.jsonl`.
- CLI sub-agent providers: subscription is flat-rate from the user's POV.
  `spend.jsonl` records `{ provider: "cli:claude-code", subscription: true,
  usd: 0.0, wall_time_seconds: <N> }`. The `--max-spend` cap is wall-time-
  based for CLI providers when subscription is true (default cap maps to a
  reasonable wall-clock budget; configurable in `defaults.cli_max_wall_seconds`).

### What CLI sub-agents return

The CLI agent does its own internal tool calls. From deadreckon's POV:

- One `complete()` call = one subprocess invocation = one **turn**.
- Output: captured stdout / stderr (saved to disk).
- File effects: detected by snapshotting `working/` before and after.
- Trace entry: `{ kind: "cli_subagent", binary, args, stdout_path,
  duration_ms, exit_code, pid }`.
- Provenance: every file changed between snapshot N and N+1 attributes to
  the turn's `tool_call_id`.

This is the AS-BUILT §10 sub-agent forking pattern at the provider boundary.

## Mock provider (for keyless deterministic tests)

`crates/deadreckon-providers/tests/mock_server.rs`:

- Implements the OpenAI Chat Completions API endpoints used by the
  `openai-compatible` adapter.
- Returns scripted responses from a fixture file
  (`tests/fixtures/mock-script-<name>.json`).
- Records every request to a per-test journal so the test can assert
  `requests.len() >= 3`, inspect bodies, etc.
- Streaming optional; non-streaming responses sufficient for V0 tests.

Required fixtures:

- `mock-script-three-turn.json` — three-turn task: model emits `bash`
  tool call, deadreckon executes, feeds result, model emits `edit_file`
  tool call, deadreckon writes file, model says done.
- `mock-script-kill.json` — long-running response (deliberately slow) used
  to test mid-turn kill.
- `mock-script-error.json` — provider returns 5xx; test fallback chain.

## Turn loop contract (`deadreckon-core`)

Pseudocode for the replaced `coding_turn_script()`:

```rust
async fn run_turn_loop(run: &mut Run, ctx: &Ctx) -> Result<RunOutcome> {
    let mut history = run.load_history()?;
    let provider = ctx.router.pick(&run.provider_choice)?;
    loop {
        if run.spend_exceeds_cap() { return Ok(RunOutcome::PausedAtCap); }
        if ctx.cancel.is_cancelled() { return Ok(RunOutcome::Killed); }

        run.snapshot_pre_turn()?;
        let resp = provider.complete(&history, ctx.cancel.clone()).await?;
        run.record_trace(&resp)?;
        run.record_spend(&resp)?;

        match resp.action() {
            Action::ToolCall(call) => {
                let result = ctx.sandbox.dispatch(&call, ctx.cancel.clone()).await?;
                run.snapshot_post_turn(&call)?;
                run.record_provenance(&call, &result)?;
                history.push_tool_call_and_result(call, result);
            }
            Action::CliSubagent(invocation) => {
                let stdout = ctx.sandbox.spawn_cli(&invocation, ctx.cancel.clone()).await?;
                run.snapshot_post_turn_from_dir_diff()?;
                run.record_provenance_from_diff(&invocation)?;
                history.push_subagent_turn(invocation, stdout);
            }
            Action::Done => { run.write_acceptance_proof()?; return Ok(RunOutcome::Done); }
        }
    }
}
```

The provider implementation decides whether to return `Action::ToolCall` or
`Action::CliSubagent` based on its kind.

## Supervision (kill, resume)

- `state.json` adds `child_pids: Vec<u32>` (sandbox child + sub-agent subprocess).
- `kill <run-id>`:
  - Sets a cancellation token (Tokio `CancellationToken`).
  - Aborts any in-flight reqwest task.
  - For each `child_pid`: `nix::sys::signal::kill(pid, SIGTERM)`; sleep 2 s; if alive, `SIGKILL`.
  - Persists `state.status = "killed"` + `killed_at`.
- `resume <run-id>`:
  - Refuses to resume a run whose `status == "completed"`.
  - Loads `history` from `traces.jsonl`.
  - Recomputes spend from `spend.jsonl`.
  - Calls `run_turn_loop` with the loaded history.
  - Turn counter continues from the last `turn-<N>/` snapshot.

## Anti-self-attestation gate

- The agent's response (LLM text or CLI stdout) **cannot** be the source of
  truth for "task done."
- When the loop sees `Action::Done` (or the CLI sub-agent exits cleanly), it
  calls an external Rust test runner:

  ```rust
  let result = std::process::Command::new(env!("CARGO_BIN_EXE_dr-gate"))
      .arg("--run").arg(&run.id)
      .arg("--working-dir").arg(&run.working_dir)
      .output().await?;
  ```

  The `dr-gate` test runner is a separate binary in the workspace. It runs
  whatever acceptance check applies (cargo test, custom script from the
  goal's `acceptance` field, etc.), and writes
  `/Users/gdc/.deadreckon/runstate/<scope>/runs/<id>/proofs/turn-acceptance.json`
  if and only if its own checks pass.

- The deadreckon binary validates the marker against `run_id` and refuses
  any acceptance marker not produced by `dr-gate`.

## Verification matrix

Run all three tiers; tier C is optional (gated by presence of `claude` or `codex`):

### Tier A — keyless deterministic (must pass)

```zsh
cd /Users/gdc/deadreckon && cargo test --workspace
```

The `tests/agentic-loop/` integration suite:

- `mock_provider_records_three_turns` — uses `mock-script-three-turn.json`; asserts ≥ 3 LLM requests, ≥ 3 trace entries, ≥ 3 spend entries, ≥ 2 provenance entries.
- `kill_mid_turn_cancels_inflight_http` — uses `mock-script-kill.json`; kills mid-stream; asserts `state.status == "killed"` and HTTP client task aborted.
- `resume_continues_conversation` — kills after turn 2; resumes; asserts turn 3 sees prior history.
- `provenance_tool_call_ids_match_traces` — for every `provenance.jsonl` entry, `tool_call_id` grep-matches a `traces.jsonl` entry.
- `acceptance_marker_only_writeable_by_dr_gate` — tries to write the marker as the agent path; asserts the binary rejects it.

### Tier B — code-shape checks (must pass)

```zsh
grep -r "coding_turn_script\|hardcoded_smoke\|fn smoke_turn" \
  /Users/gdc/deadreckon/crates/ && echo FAIL_SMOKE_PATH_STILL_DEFAULT
head -1 /Users/gdc/deadreckon/demo.cast | grep -q '"version"' \
  || echo FAIL_DEMO_NOT_RECORDED
grep -q "cli:claude-code\|cli_claude_code" /Users/gdc/deadreckon/crates/deadreckon-providers/src \
  -r || echo FAIL_NO_CLI_CLAUDE_PROVIDER
grep -q "cli:codex\|cli_codex" /Users/gdc/deadreckon/crates/deadreckon-providers/src \
  -r || echo FAIL_NO_CLI_CODEX_PROVIDER
```

### Tier C — live sub-agent (optional, gated)

```zsh
if command -v claude >/dev/null; then
  /Users/gdc/deadreckon/target/release/deadreckon doctor
  /Users/gdc/deadreckon/target/release/deadreckon run \
    "make a 5-line file at notes.md describing dead reckoning" \
    --provider cli:claude-code --max-spend 5
  # Inspect: turns/turn-1/claude.out exists, snapshots show notes.md, provenance ties it to the CLI turn.
fi
```

## CLI surface changes

- `deadreckon run --provider cli:claude-code` and `--provider cli:codex` are valid V0 values.
- `deadreckon init` detects `claude` and `codex` in `$PATH` and offers them as default providers.
- `deadreckon doctor` lists each CLI sub-agent provider with `✓ found at <path>` or `✗ not installed (try `npm i -g @anthropic-ai/claude-code` etc.)`.

## Engineering invariants additions (do not violate)

- **No hardcoded turn scripts in the default path.** `deadreckon run` without `--smoke` must invoke a real provider.
- **Provider trait is the single boundary.** No bypassing `ProviderRouter` from the run loop.
- **Cancellation is uniform.** Both HTTP and CLI providers respect a Tokio `CancellationToken`.
- **CLI sub-agent providers honor sandbox.** The `claude` / `codex` subprocess runs inside the deadreckon sandbox profile; the `--dangerously-skip-permissions` flag only disables the sub-agent's own gate, not deadreckon's outer one.
- **Anti-self-attestation.** Acceptance marker is writeable only by `dr-gate`, validated by binary against `run_id`.
- **Subscription-BYOK is a first-class provider posture.** `cli:claude-code` and `cli:codex` are not lesser providers; they're how most users will run deadreckon V0.

## Dependencies added (Tier 1 / 2 per V0 dep policy)

Tier 1 (utility, free):
- `axum` (dev-dep for mock provider).
- `tower-http` (dev-dep).
- `tokio-util` (CancellationToken).

Tier 2 (architectural, log to `DEPENDENCIES.md`):
- `wiremock` is acceptable instead of hand-rolling the mock; document the choice.

## Process invariants

- Phased local commits only. No `git push`.
- Update `/Users/gdc/deadreckon/CHANGELOG.md` per phase with exact SHAs and files touched.
- If a verification gate genuinely cannot be met (e.g., `codex` doesn't exist on this machine), document in `GAP-ANALYSIS.md` and proceed; do not silently skip.
- After completion, append a "Primary-flow V0 retrospective" section to `CHANGELOG.md` listing what's real, what's deferred, and what tier-C live tests verified.
