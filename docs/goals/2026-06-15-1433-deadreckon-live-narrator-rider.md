# deadreckon — Live Narrator Rider (one rolling story, written live, rendered everywhere)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-15-1433-deadreckon-live-narrator-goal.md`.
It supersedes nothing in prior riders — their invariants still apply, notably:

- `2026-05-26-1546-deadreckon-narrative-attach-rider.md` — the deterministic-first, evidence-cited, "alive without becoming another stream", "useful without a provider call" contract for the Narrative projection. This rider EXTENDS that engine to run live during a run; it does not replace its floor.
- `2026-06-05-0010-deadreckon-attach-tui-uniformity-rider.md` — uniform attach chrome; the live beats render inside the existing Narrative content panel.
- `2026-06-10-1628-deadreckon-stable-readiness-rider.md` — populated model catalogs (`haiku`, `gpt-5.1-codex-mini`, `claude-haiku-4-5`, `gpt-4o-mini` all exist as catalog ids) and `selected_route_info` are now shipped; this rider depends on them.

This rider adds: an in-process narrator sidecar that the run process spawns, a continuity-carrying rolling narrative, a subscription-first cheap-model narrator router, time-gated coalesced cadence, and calm foreground + opt-in headless surfaces.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `<run_root>` under `$DEADRECKON_HOME` (default `~/.deadreckon`).

## Posture (decided — do not redesign)

- **Maturity stays stable track** (0.1.1 shipped; this lands as `0.2.0 — Live Narrator`). Additive only.
- **No `PipelineState` schema changes.** `RunLoopConfig` may gain ONE additive field (`narrate: Option<NarratorConfig>`) defaulting to `None` so all existing constructors (`turn_loop.rs:2005, 2042, 2542, 3135, 3692` pass `event_sender: None`) keep compiling; durable narrator state lives in files under `<run_root>/narrative/`, not in structs.
- **The deterministic projection is the floor.** With no provider credentialed, every surface still narrates via `build_run_projection` output. The non-TTY-no-provider invariant from the narrative-attach rider holds: a plain/headless path never *requires* a provider call.
- **The narrator is a projection.** It only ever appends to `<run_root>/narrative/snapshots.jsonl` and `state.json`; it never writes `flight-events.jsonl`, `plan-events.jsonl`, `events.jsonl`, `traces.jsonl`, `provenance.jsonl`, or `history.json`. Those remain owned by the loop (AS-BUILT §21 append-only telemetry invariant).
- **Never race the loop's spend.** The narrator runs on its own tokio task and must not touch `state.total_spend_usd` / `state.total_wall_seconds`. It owns a separate budget and writes its own `spend.jsonl` rows tagged with a narrator provider label.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Anything beyond P1–P11 → `docs/V1-CANDIDATES.md` (and remove the now-closed "long-lived narrator daemon" framing there — see P11).
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

### `<run_root>/narrative/snapshots.jsonl` (existing file, extended)

Append-only. The narrator appends one record per beat. The latest valid line is the current story (`read_latest_snapshot`, `narrative.rs:1090`). The existing `NarrativeSnapshot` shape is preserved; live beats set new fields:

```jsonc
{
  "schema": 2,                       // bump from the attach-only snapshot schema
  "beat_seq": 7,                     // monotonic per run; 0 = first beat
  "covers_turn": 14,                 // highest turn_id folded into this beat
  "status": "fresh",                 // fresh=model | deterministic=floor | stale | failed
  "source": "live",                  // live=run-process narrator | attach=on-demand refresh
  "headline": "Wiring the narrator task into run.rs",
  "current_work": [ { "text": "...", "evidence": ["turn:14","trace:14:2"], "confidence": "high" } ],
  "architecture_notes": [ /* NarrativeClaim[] */ ],
  "risks": [ /* NarrativeClaim[] */ ],
  "next_likely": [ /* NarrativeClaim[] */ ],
  "rolling_summary": "Through turn 14: established the event bus...",  // carried context, capped
  "provider": { "source": "cli:claude-code", "model": "haiku", "calls": 7, "cost_usd": 0.0, "subscription": true },
  "ts": "2026-06-15T14:33:02Z"
}
```

- `rolling_summary` is the bounded carry-forward (see Continuity). Hard cap: 1200 chars; on overflow the narrator re-summarizes (the model is asked to compress its own prior summary plus the latest beat).
- `evidence` ids are strings of the form `turn:<n>` or `trace:<n>:<idx>`. Validation: every id must resolve against the turn records / traces written so far. NEW for live: `turn:<covers_turn>` and ids from turns in the current window ARE valid evidence (the attach-only engine rejected unknown ids; live relaxes this to "must be a real, already-written turn/trace id", not "must be in the pre-built deterministic snapshot").

### `<run_root>/narrative/state.json` (existing, extended)

```jsonc
{
  "schema": 2,
  "last_beat_seq": 7,
  "last_beat_at": "2026-06-15T14:33:02Z",
  "last_covered_turn": 14,
  "beats_emitted": 8,                 // for the per-run cap
  "narrator_backend": "cli:claude-code",
  "narrator_model": "haiku",
  "narrator_spend_usd": 0.0,
  "budget_exhausted": false
}
```

### `<run_root>/spend.jsonl` (existing, narrator rows added)

The narrator appends `SpendRecord` rows (`artifacts.rs:12`) via `append_spend`, with a distinguishing label so the run's spend math (which reads its own accumulators, not the file) is unaffected and `dr show` can break out narrator cost:

```jsonc
{ "kind": "narrator", "provider": "cli:claude-code", "model": "haiku", "cost_usd": 0.0,
  "input_tokens": 1840, "output_tokens": 210, "subscription": true, "estimated": false,
  "cap_usd": 0.50, "wall_time_seconds": 1.9, "turn": 14 }
```

`kind: "narrator"` distinguishes these from loop rows. If `SpendRecord` has no `kind` field today, add it as an additive serde-default (`#[serde(default)]`) — additive only, no break.

## Narrator backend selection (pseudocode — match this)

Build the narrator's OWN `ProviderRouter` once at run start, following the `complete_run_docs` precedent (`turn_loop.rs:1617`, which constructs its own router rather than sharing the loop's). The run's router stays on the big coding model; the narrator gets a cheap one.

```
fn select_narrator_route(config_path) -> NarratorBackend:
    # preference order, first credentialed wins (mirror router.rs:198 selected_route_info)
    candidates = [
        ("cli:claude-code", "haiku"),            # $0 subscription
        ("cli:codex",       "gpt-5.1-codex-mini"),# $0 subscription
        ("anthropic",       "claude-haiku-4-5"),  # API key, cheap
        ("openai",          "gpt-4o-mini"),       # API key, cheap
    ]
    for (provider, model) in candidates:
        if route_available(provider):            # CLI: installed AND logged-in; HTTP: key present
            return Model(provider, model)
    return DeterministicFloor                     # no provider → template only
```

- `route_available` for CLI providers MUST use the richer `auth_probe.rs` (`probe_cli_auth` — installed AND logged-in), not the router's presence-only `has_credential`, so a logged-out `claude` correctly falls through to the next candidate. For HTTP providers, key-present is sufficient. Honor `DEADRECKON_AUTH_PROBE=0` (probe disabled → treat as available, matching existing behavior).
- Construct via `ProviderRouter::from_config_path_with_model(config_path, Some(provider), Some(model))` (`router.rs:53`) — `override_provider` collapses to one route, `override_model` pins the cheap model.
- Selection runs ONCE per run; the chosen backend is recorded in `state.json`. No per-beat re-selection (avoids churn). If the chosen backend starts failing mid-run, fall back to `DeterministicFloor` for the remainder and set `state.json.budget_exhausted`/a `narrator_degraded` marker — never crash the run.
- A `--narrator-model <id>` / `[defaults] narrator_model` override forces a specific model but keeps the provider preference order.

## Cadence (reuse `provider_refresh_decision`, `narrative.rs:1196`)

```
on DocsCheckpoint(turn N):                 # fires once per completed action, turn_loop.rs:1653
    window.push(turn_record(N))            # read the just-written TurnRecord (docs.rs:62)
    if NOT cadence_ok(): return            # coalesce: keep accumulating into window
    if beats_emitted >= per_run_cap: return
    emit_beat(window); window.clear()

cadence_ok():                              # mirror NarrativeCadence (narrative.rs:177)
    return now - last_beat_at >= min_gap   # default min_gap 30s
        OR turns_since_last_beat >= turn_burst   # default 8 turns force a beat even if fast

quiet_timer (every quiet_interval, default 20s):
    if a turn is in-flight AND now - last_beat_at >= quiet_threshold (default 45s):
        emit_beat(window_or_interim)       # a long single turn still gets a beat
```

- Defaults: `min_gap = 30s`, `turn_burst = 8`, `quiet_threshold = 45s`, `per_run_cap = 200` beats. All configurable under `[defaults]` (see Config additions). These mirror the attach cadence constants; reuse the type.
- Coalescing: when turns arrive faster than `min_gap`, multiple `TurnRecord`s fold into one beat (the window). The beat's `covers_turn` is the max turn in the window; `current_work` cites the most significant turn(s).
- On `RunCompleted`/`RunPromoted`/`Error`/final cap: flush a final beat unconditionally (ignore `min_gap`) so the last state is always narrated.

## Continuity / windowing (the missing piece — match this)

Each beat's prompt is built from THREE inputs, not a from-scratch projection:

1. `previous_narrative` — the latest snapshot's headline + claim sections (`read_latest_snapshot`, `narrative.rs:1090`).
2. `new_turns` — ONLY the `TurnRecord`s in the current window (their `response_summary`, `tool_kind`, `FileChange` add/del + `largest_hunk_excerpt`, `outcome`, `commit_sha`; cap `response_full` slice to ~2 KiB/turn). NOT the whole `traces.jsonl`/`incremental.jsonl` (that is the O(turns²) trap the post-hoc path falls into at `polish.rs:1119`).
3. `rolling_summary` — the carried compressed history from the prior snapshot.

The system prompt instructs the model to **AMEND and EXTEND** the previous narrative using the new turns — append a beat to the story, revise `headline`/`current_work`/`next_likely`, keep prior architecture/risk claims unless contradicted — and to return the updated sections PLUS a refreshed `rolling_summary` (≤1200 chars). It is NOT the attach-only "projector that may only relabel" prompt (`narrative.rs:1181`). `apply_provider_response` (`narrative.rs:1224`) is extended with a merge path: validate evidence ids against real turns/traces, then write a NEW snapshot line (append, never overwrite) with `beat_seq = prev + 1`.

This rolling-amend approach keeps per-beat input bounded to (window turns + summary + prior sections) regardless of run length: cost is O(turns), not O(turns²). Over a 100+ turn run the `rolling_summary` re-summarization keeps carried context flat.

The narrator's prompt template lives as a tunable skill/template, not a hardcoded const (judgment-in-markdown invariant). Reuse the `skills/run-narrator` family voice; add `skills/live-narrator/SKILL.md` (the per-beat amend prompt) so the narration voice is tunable without a rebuild.

**Post-hoc seeding (decision, Q2).** The two narrators converge: at run end, `complete_run_docs` (`turn_loop.rs:1617`) seeds the post-hoc `RUN-NARRATIVE.md` from the FULL accumulated live narration — the entire ordered beat history in `<run_root>/narrative/snapshots.jsonl`, not just the latest snapshot — fed as `current_narrative` (the input the post-hoc prompt already accepts, `polish.rs:1138`). The final doc is a polish/consolidation of the live story the run already told, not a from-scratch pass over the raw trace. The live narrator writes the draft turn-by-turn; the post-hoc pass refines it.

## Surface rendering (match these idioms)

All surfaces render the latest snapshot through `narrative_plain_lines` (`narrative.rs:1006`). The DIFFERENCE is the frame:

- **Foreground TTY (`dr run` / `orchestrate` / `campaign`, default ON):** a CALM bounded block of at most `narrate_lines` (default 4) lines — headline + top 1–3 `current_work` claims — redrawn in place as beats advance. NOT a scrolling log. It replaces/augments the existing CLI wait-status indicator. For orchestrate/campaign, show one block per active child (or an aggregate headline line per child, bounded), reusing the uniform attach chrome.
- **Piped / no-TTY (opt-in `--narrate`):** append-only. Each beat prints `[turn N] <headline> — <current_work[0]>` (and indented claims) to **STDERR**, never stdout. stdout stays clean for `| jq` consumers. Turn-stamped, one block per beat, monotonic.
- **Silent-pipe fix:** today `plain` is resolved at `run.rs:50` from `--plain || defaults.plain || NO_COLOR` but NOT from `!stdin().is_terminal()`. Add: when stdout is not a TTY and `--narrate` is set, the narrator drives progress; independently, a piped run without `--narrate` should still emit the deterministic per-turn `--plain` telemetry line rather than nothing (close the silent-run gap noted in the investigation).
- **Attach (interactive + headless):** unchanged entry points; the Narrative view now reads the live-written snapshots (`source: "live"`). Interactive attach MAY still issue an on-demand refresh ('r'); those write `source: "attach"` snapshots. Headless `attach --plain/--json` renders the latest live snapshot and, honoring the existing depth test `non_tty_narrative_attach_does_not_call_provider_without_explicit_refresh`, does NOT itself call a provider — it renders what the run already wrote.

## Flag / config signatures

```
dr run <goal>
    [--narrate]                 # headless: append-only beats to stderr (opt-in; no-op extra on a TTY where foreground is already on)
    [--no-narrate]              # disable narration entirely (foreground + headless)
    [--narrator-model <id>]     # pin narrator model, keep provider preference order
dr orchestrate ... [--narrate] [--no-narrate] [--narrator-model <id>]
dr campaign    ... [--narrate] [--no-narrate] [--narrator-model <id>]
```

Refusal / degradation cases:

| Case | Behavior |
|---|---|
| No provider credentialed | Narrate via deterministic floor; no refusal, no exit code change. |
| `--narrate` + `--no-narrate` both passed | Refuse: `try: pass only one of --narrate / --no-narrate`. |
| `--narrator-model X` where X not in any catalog | Refuse with `try: deadreckon models` to list valid ids. |
| Narrator backend fails mid-run | Degrade to floor; record `narrator_degraded`; run continues unaffected. |
| Budget cap reached | Stop emitting model beats; floor continues; `state.json.budget_exhausted = true`. |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; implement; green on `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + focused `cargo test`; conventional-commit; one-line CHANGELOG entry.

### P1 — Narrator state files + schema v2 (plumbing, no behavior)

- Bump `snapshots.jsonl`/`state.json` to schema 2 with the new fields above; additive serde-defaults so existing attach snapshots still parse. Add `SpendRecord.kind` (`#[serde(default)]`).
- Add `NarratorConfig` and the additive `RunLoopConfig.narrate: Option<NarratorConfig>` field defaulting to `None`.

Depth tests (in `crates/deadreckon/src/narrative.rs` tests + `crates/deadreckon-core/src/docs.rs` tests):
- `narrative_snapshot_schema2_roundtrips_and_reads_legacy_schema1`
- `run_loop_config_narrate_defaults_none_keeps_existing_constructors`
- `spend_record_kind_defaults_to_loop_when_absent`

### P2 — Narrator backend selection

- Implement `select_narrator_route` with subscription-first order, `probe_cli_auth` gating for CLI, key-present for HTTP, deterministic floor fallback. Honor `--narrator-model` and `DEADRECKON_AUTH_PROBE`.

Depth tests (in `crates/deadreckon-providers` or a new `narrator_backend_tests.rs`):
- `narrator_prefers_claude_code_haiku_when_logged_in`
- `narrator_falls_through_logged_out_cli_to_next_candidate`
- `narrator_uses_anthropic_haiku_when_no_cli_but_api_key`
- `narrator_returns_deterministic_floor_when_nothing_available`
- `narrator_model_override_keeps_provider_order`

### P3 — RunEventBus wiring into the run process

- In `run.rs`, construct a `RunEventBus` (`events.rs:66`), set `RunLoopConfig.event_sender = Some(bus.sender())` (replacing `None` at `run.rs:387`), and thread a `CancellationToken` (replacing `None` at `run.rs:388`) so the narrator shuts down with the run. Mirror the test precedent that already passes `event_sender: Some(bus.sender())` (`turn_loop.rs:2907, 2975, 3032`).

Depth tests:
- `run_command_wires_event_bus_when_narration_enabled`
- `narrator_task_stops_on_run_cancellation`

### P4 — Continuity prompt + amend-merge

- Add `skills/live-narrator/SKILL.md` (the amend/extend per-beat prompt). Extend `build_provider_prompt` (`narrative.rs:1156`) with a `live` mode taking `previous_narrative` + `new_turns` + `rolling_summary`. Extend `apply_provider_response` (`narrative.rs:1224`) with a merge path that appends a new `beat_seq` snapshot and relaxes evidence validation to "real already-written turn/trace id".

Depth tests:
- `live_prompt_includes_previous_narrative_and_only_window_turns`
- `apply_live_response_appends_beat_does_not_overwrite`
- `live_beat_rejects_evidence_id_for_nonexistent_turn`
- `live_beat_accepts_new_turn_evidence_id`

### P5 — Windowing + rolling summary bound

- Implement the window accumulator and the `rolling_summary` re-summarization at the 1200-char cap. Prove input size stays bounded across a long synthetic run.

Depth tests:
- `narrator_window_feeds_only_new_turns_not_full_trace`
- `rolling_summary_stays_under_cap_over_120_turns`
- `narrator_input_token_estimate_is_o_turns_not_o_turns_squared`

### P6 — Cadence + coalescing + quiet timer + deterministic live ticker

- Implement `cadence_ok`, burst-force, per-run cap, and the quiet timer. Reuse/extend `NarrativeCadence` (`narrative.rs:177`) and `provider_refresh_decision` (`narrative.rs:1196`).
- **Liveness vs beats are split (decision, Q1).** MODEL beats fire only at `DocsCheckpoint` (turn end), time-gated/coalesced. BETWEEN beats, the foreground line is a DETERMINISTIC live ticker built straight from events — `turn N · <current tool> (<elapsed>)` from `ToolCallStarted`/`TokenUsageDelta` — costing $0 and no model call, so a long turn never looks frozen. The quiet timer escalates to a MODEL beat only when a turn exceeds `narrate_quiet_seconds` AND has new signal; otherwise the ticker carries liveness. Because the ticker hides per-beat latency, model-beat render time is invisible to the operator (this is why respawn-per-beat is acceptable — see Q4).

Depth tests:
- `narrator_coalesces_fast_turns_into_one_beat`
- `narrator_forces_beat_after_turn_burst`
- `narrator_quiet_timer_escalates_long_turn_to_model_beat`
- `deterministic_ticker_updates_between_beats_with_no_model_call`
- `narrator_respects_per_run_beat_cap`

### P7 — Narrator spend accounting + budget cap

- Narrator writes `kind: "narrator"` `SpendRecord` rows via `append_spend`; enforces its own `budget_cap_usd` (precedent `RunLoopDocsConfig.budget_cap_usd`, `turn_loop.rs:59`); degrades to floor at cap. Proves the loop's `state.total_spend_usd` is untouched by narrator calls.

Depth tests:
- `narrator_spend_rows_tagged_and_separate_from_loop_totals`
- `narrator_degrades_to_floor_at_budget_cap`
- `narrator_subscription_backend_records_zero_cost`

### P8 — Foreground calm block (TTY)

- Render the bounded `narrate_lines` block, redrawn in place, default ON for run/orchestrate/campaign on a TTY. Reuse `narrative_plain_lines`. Byte-bounded; never exceeds `narrate_lines`.

Depth tests:
- `foreground_block_is_bounded_to_narrate_lines`
- `foreground_block_updates_in_place_not_appends`
- `foreground_on_by_default_off_with_no_narrate`

### P9 — Headless `--narrate` append + silent-pipe fix

- Append-only turn-stamped beats to STDERR under `--narrate`; stdout stays clean. Close the silent-piped-run gap (per-turn deterministic telemetry even without `--narrate`). Add TTY-awareness alongside `run.rs:50`.

Depth tests:
- `narrate_headless_writes_beats_to_stderr_not_stdout`
- `narrate_headless_beats_are_append_only_and_turn_stamped`
- `piped_run_is_not_silent_between_start_and_exit`

### P10 — Attach convergence + post-hoc seeding + cross-cutting friendliness

- Attach Narrative view renders live `source: "live"` snapshots; interactive 'r' still works (`source: "attach"`); headless attach renders-without-provider (existing depth test stays green).
- **Post-hoc seeding (Q2):** `complete_run_docs` (`turn_loop.rs:1617`) reads the full ordered beat history from `<run_root>/narrative/snapshots.jsonl` and passes it as `current_narrative` into the post-hoc polish, so `RUN-NARRATIVE.md` consolidates the accumulated live story rather than re-deriving from the raw trace.
- Flag refusals (`--narrate`+`--no-narrate`, bad `--narrator-model`) with `try:` footers; lifecycle hints ("narration written to <run>/narrative/; attach to replay").

Depth tests:
- `attach_renders_live_written_beats_without_provider_call`
- `posthoc_run_narrative_seeds_from_full_live_beat_history`
- `narrate_conflicting_flags_refuse_with_try_line`
- `bad_narrator_model_refuses_with_models_hint`

### P11 — Architecture doc + V1-CANDIDATES + CHANGELOG (doc only; no depth test)

- Insert into `docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## 44. Live Narrator (one rolling story, written live, rendered everywhere)

  44.1 Sidecar architecture: run process spawns the narrator task; RunEventBus → DocsCheckpoint → TurnRecord
  44.2 Continuity: prior narrative + windowed new turns + rolling summary; amend-not-overwrite; O(turns) cost
  44.3 Subscription-first backend selection (auth_probe + selected_route_info); deterministic floor
  44.4 Cadence: time-gated + coalesced + quiet timer + per-run cap
  44.5 Surfaces: calm foreground block, opt-in headless --narrate to stderr, attach convergence
  44.6 Spend isolation: narrator budget + tagged spend.jsonl rows
  ```
- Update §22 "What's Built vs Scaffolding-Thin": add live narration to the shipped side; note it closes the prior thin "narration only at attach time / silent piped run" gap.
- Edit `docs/V1-CANDIDATES.md` lines 64–65: REMOVE the "long-lived narrator daemon" / "live" framing now shipped; keep only genuinely-deferred items (shareable/cloud observer views, graph layout engines, team annotations, learned summary preferences, historical narrative analytics, a shared cross-surface broadcaster daemon).
- Append to `CHANGELOG.md`:
  ```
  ## 0.2.0 — Live Narrator

  - A run now narrates itself in plain English as it works: ...
  ```

## Integration matrix

| Surface | Foreground default | Headless | Provider used | Renders |
|---|---|---|---|---|
| `dr run` (TTY) | calm block ON | n/a | narrator router (cheap) | latest live snapshot |
| `dr run` (piped) | n/a | `--narrate` → stderr append | narrator router | per-beat blocks |
| `dr orchestrate` | calm block ON | `--narrate` | narrator router per child | per-child headlines |
| `dr campaign` | calm block ON | `--narrate` | narrator router per child | per-child headlines |
| `dr attach` (interactive) | Narrative view | — | live snapshots + 'r' on-demand | rolling story |
| `dr attach --plain/--json` | — | renders latest | NONE (no provider) | latest live snapshot |
| No provider anywhere | floor block | floor append | none (deterministic) | templated projection |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `--narrate and --no-narrate are mutually exclusive` | `try: pass only one of --narrate / --no-narrate` |
| `unknown narrator model '<X>'` | `try: deadreckon models` |
| `narrator backend unavailable mid-run` | `try: deadreckon attach <id> --view narrative` (floor still narrates) |
| `narrator budget exhausted` | `try: --narrator-model <cheaper> or raise [defaults] narrator_budget_usd` |

(Each parameterized over a depth test so every error case is exercised.)

## Config additions

```toml
[defaults]
narrate = true                 # foreground narration on a TTY for run/orchestrate/campaign
narrator_model = ""            # "" = use subscription-first preference order
narrator_budget_usd = 0.50     # per-run narrator spend cap (subscription backends record 0)
narrate_lines = 4              # max lines in the calm foreground block
narrate_min_gap_seconds = 30   # cadence floor between beats
narrate_turn_burst = 8         # force a beat after N turns even if under the gap
narrate_quiet_seconds = 45     # a long single turn gets a beat after this
narrate_max_beats = 200        # per-run beat cap
```

## Out of scope (explicitly → V1-CANDIDATES)

- A long-lived, cross-surface narrator DAEMON shared by run/plan/chain (this rider is per-run, in-process).
- **Persistent / streaming CLI session (decision Q4 — deferred, path documented).** This rider spawns a fresh subprocess per beat; the time-gated cadence (≥`narrate_min_gap_seconds`) plus the deterministic ticker make cold-start negligible and invisible. The upgrade path, for when higher-frequency (sub-gap, near-per-turn) narration is wanted, is a long-lived CLI session: Claude Code's streaming headless mode (one persistent `claude` process fed messages over stdin via `--input-format stream-json --output-format stream-json` — verify exact flags/protocol at implementation), with crash-restart supervision and backpressure. This is a new provider LIFECYCLE (the current `Provider::complete` is one-shot spawn-per-call), so it earns its own slice. Direct-API backends already need no subprocess. Log it in `V1-CANDIDATES.md` as "persistent narrator session" with this rationale.
- Unifying the live and post-hoc narrators into literally ONE document/pass. Q2 is decided: they stay distinct passes but converge — the post-hoc `RUN-NARRATIVE.md` SEEDS from the full accumulated live narrative (see Continuity + P10). A single merged generator remains out of scope.
- Learned per-user summary preferences, narrative analytics, shareable/cloud observer views, graph layout engines, team annotations.
- Intra-turn (sub-action) narration for direct-API runs lacking flight-recorder detail.

## Dependencies (Tier 1 / 2 / 3 policy)

- **Tier 1 (utility, free):** none new expected — `tokio` (broadcast already used), `serde`, `which` (auth probe) are present.
- **Tier 2 (architectural, log to `DEPENDENCIES.md`):** none expected. If a terminal in-place redraw needs more than the existing UI layer offers, log before adding.
- **Tier 3 (blocked):** same blocks as prior riders (no new network clients, no telemetry uploaders).

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes**; `RunLoopConfig` gains at most the one additive `narrate` field; all other state is files under `<run_root>/narrative/`.
- **One depth test before each phase implementation.** A phase whose tests were never red is suspect (`grep -c '^\s*fn ' <test files>` enforces presence per the named lists above).
- **Narrator never mutates loop-owned telemetry** and never touches `state.total_spend_usd`/`state.total_wall_seconds`.
- **Deterministic floor always available**; no surface hard-requires a provider call.
- **Append, never overwrite** snapshots — continuity depends on the beat history; the byte format of `snapshots.jsonl` records and the foreground block are depth-tested specs (changing whitespace changes the spec).
- **No silent expansion.** Anything beyond P1–P11 → `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its named depth tests passing and a CHANGELOG line naming the SHA.
- Conventional commits, scoped: `feat(narrator):` / `fix(narrator):` / `feat(run):` per phase; `docs(goals):` for this pair.
- After P11, optionally capture an asciinema cast under `/Users/gdc/deadreckon/` showing a live-narrated run (the change is user-visible — a cast is worth it for a "glorious" feature).
- If a phase reveals a V1-architecture decision (e.g., a shared daemon is the only clean way to do orchestrate fan-out), stop and log it in `V1-CANDIDATES.md`; do not silently expand scope.

## Decisions (resolved with the owner — implement as written)

1. **Long turns: liveness vs beats are split.** Model beats fire only at `DocsCheckpoint`; a DETERMINISTIC, $0 event-driven ticker (`turn N · <tool> (<elapsed>)`) carries liveness between beats so a long turn never looks frozen. The quiet timer escalates to a model beat only on a genuinely long turn with new signal. See P6.
2. **Live ↔ post-hoc: converge by seeding.** `RUN-NARRATIVE.md` seeds from the FULL accumulated live beat history (entire `snapshots.jsonl`) as `current_narrative`; the post-hoc pass consolidates the live story rather than re-deriving from the raw trace. Two passes, one story. See Continuity + P10. A single merged generator stays out of scope.
3. **Compaction: keep all beats; bound only the carry.** `snapshots.jsonl` stays a whole append-only audit trail; only `rolling_summary` is bounded (1200-char cap with re-summarization, P5).
4. **Cold-start: respawn-per-beat now; persistent session is the documented upgrade.** Time-gated cadence + the deterministic ticker make per-beat cold-start negligible and invisible, so a fresh subprocess per beat is acceptable. The persistent CLI streaming session (Claude Code `--input-format stream-json`, a new provider lifecycle) is logged in `V1-CANDIDATES.md` for when higher-frequency narration is wanted. See Out of scope.
5. **Orchestrate/campaign density: one line per active child**, capped at `narrate_lines`, with a "+N more" overflow.
6. **Milestone: `0.2.0 — Live Narrator`** (next minor on 0.1.1).
