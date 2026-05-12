# deadreckon — Audit + Hardening v2 Rider (close §22 thin items)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-2110-deadreckon-audit-harden-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`,
`2026-05-11-1400-deadreckon-robust-rider.md`,
`2026-05-11-1400-deadreckon-usability-rider.md`,
`2026-05-11-1444-deadreckon-orchestrate-rider.md`,
`2026-05-11-1502-deadreckon-codebase-rider.md`,
`2026-05-11-1525-deadreckon-self-documenting-rider.md`) — their invariants,
dependency policy, sandbox defaults, files-not-fields lineage pattern,
error-footer convention, and existing verbs still apply. This rider
adds an audit document, nine concrete closures of §22 thin items, and
a doc/CHANGELOG pass.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
- **No `PipelineState` schema changes.** Hardening state lives in
  files: `~/.deadreckon/sandbox.toml`, `acceptance.yaml` per run,
  `working/.deadreckon/cancel.marker`, etc.
- **Audit-driven, not audit-only.** P1 produces
  `/Users/gdc/deadreckon/docs/AUDIT-2026-05-11.md`; P2–P10 close
  named gaps; P11 revisits the audit with before/after columns.
- **Backwards-compatible.** New configs are optional; absence keeps
  current behavior. No deprecations in this rider.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** If a phase reveals a major architectural
  decision, log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`
  and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Audit document (`docs/AUDIT-2026-05-11.md`)

The audit is a living markdown document with one row per unmet need
from `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md`.

### Frontmatter

```markdown
# deadreckon Audit — 2026-05-11

**Subject:** how the alpha-tier as-built compares to the original 25 unmet needs.
**Audit date:** 2026-05-11
**Auditor:** deadreckon (rider P1)
**Source needs:** `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md`
**As-built reference:** `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
```

### Body table (one row per need)

| # | Need (verbatim title) | Status | Evidence | Recommendation |
|---|---|---|---|---|
| 1 | Live Context And Spend Meter For Coding Agents | Resolved | `crates/deadreckon/src/main.rs:1608-1616` (TUI meters) | Closure phase: P2 (event streaming) |
| 2 | Multi-Agent Worktree Coordination Layer | Partial | `docs/MULTI-RUN.md`; orchestrate-rider authored, not implemented | V1 (defer) |
| ... | ... | ... | ... | ... |

Statuses (closed enum):
- **Resolved** — shipped and load-bearing.
- **Partial** — present but thin per AS-BUILT §22 or recent UX feedback.
- **Unmet** — none of the codebase addresses this need.
- **V1** — explicitly deferred in `docs/V1-CANDIDATES.md` or recorded as out-of-scope in a prior rider.

Every row's `Evidence` cell cites a file:line, a `git log` SHA, or a doc
path under `/Users/gdc/deadreckon/`. Rows whose status is `Resolved`
or `Partial` link to ≥ 1 piece of evidence. Rows whose status is
`Unmet` or `V1` link to the closing rationale (V1-CANDIDATES.md or the
"Out of scope" section of the relevant prior rider).

### Closures map

A bottom section maps each closure phase (P2–P10) to the audit rows it
addresses:

```markdown
## Closures landing in this rider

- **P2 — TUI streaming.** Closes Need #1 (live spend meter latency)
  + Need #7 (terminal-agent UI freshness).
- **P3 — Cross-process kill.** Closes Need #11 (permission boundaries
  for unattended agents) and §22.3.
- **P4 — Mid-tool-call resume.** Closes §22.2.
- **P5 — Sandbox per-tool policy.** Closes Need #11 (permission
  boundaries) + §22.5.
- **P6 — Acceptance YAML.** Closes Need #14 (structural verification)
  + §22.8.
- **P7 — Doctor exhaustive.** Closes Need #15 (hooks/test gates
  discoverable) for the local-host slice + §22.6.
- **P8 — Library query.** Closes Need #25 (workspace inventory + run
  queue) for the inventory slice + §22.10.
- **P9 — Import parity.** Closes Need #6 (cross-tool state sharing)
  + §22.7.
- **P10 — Help/status polish.** Closes Need #15 (discoverability) and
  general UX feedback in `git log --oneline -- README.md HOWTO.md`.
```

P11 revisits each cited row with the new evidence; the status column
flips from `Partial` to `Resolved` (or stays `Partial` with the
remaining residue named).

## Sandbox per-tool policy (file shape)

`~/.deadreckon/sandbox.toml` (per-user; per-scope override at
`<source-root>/.deadreckon/sandbox.toml`):

```toml
[tool.bash]
allow = true
deny_paths = ["~/.ssh", "~/.aws", "~/.config/gcloud"]
allow_network = true   # default ON: agents typically need outbound (deps, fetch, API)

[tool.write_file]
allow = true
deny_paths = ["~/.ssh"]

[tool.cli_subagent]
allow = true
allow_network = true   # CLI agents need outbound for their own API
```

**Network is allowed by default for every tool.** Operators who want a
network-deny posture set `allow_network = false` per tool in
`sandbox.toml` (or globally via `[defaults] tool_allow_network = false`).
The choice flows down to `SandboxSpec.allow_network` per tool dispatch,
overriding the underlying sandbox profile's deny-by-default.

Resolution: per-scope > user > built-in defaults. Absence of the file
keeps current behavior — `allow = true` for every tool, network ON,
inside the existing Seatbelt/bwrap profile.

Refusal footer (canonical):

```
deadreckon: tool 'bash' refused by sandbox policy: command would read denied path '/Users/gdc/.ssh/id_rsa'
try: deadreckon run ... --allow-tool bash:read:~/.ssh   (one-shot opt-in)
try: deadreckon config set tool.bash.deny_paths -= ~/.ssh   (persistent)
```

Refusals append a `provenance.jsonl` entry with `event: "tool.refused"`
and the policy hash.

## Acceptance YAML spec (`acceptance.yaml`)

Lives at `<run-root>/acceptance.yaml` (optional; absence keeps current
`cargo test` default for Rust targets).

```yaml
schema_version: 1
required:
  - kind: tests
    run: cargo test --workspace
    timeout_seconds: 600
  - kind: file-exists
    path: README.md
  - kind: content-match
    path: src/main.rs
    pattern: "fn main"
  - kind: build-success
    run: cargo build --release
  - kind: shell
    run: ./scripts/smoke.sh
    expect_exit: 0
optional:
  - kind: tests
    run: cargo bench
```

`dr-gate` runs `required` first; any failure → `status: "fail"`.
`optional` checks contribute to `proofs/turn-acceptance.json` (per-check
result list) but don't fail the run.

Marker shape extension (no `PipelineState` change):

```json
{
  "schema_version": 1,
  "run_id": "...",
  "status": "pass",
  "produced_by": "dr-gate",
  "checked_at": "...",
  "working_dir": "...",
  "signature": "...",
  "check_count": 5,
  "checks": [
    {"kind": "tests", "ok": true, "duration_ms": 12450},
    {"kind": "file-exists", "ok": true},
    {"kind": "content-match", "ok": true},
    {"kind": "build-success", "ok": true},
    {"kind": "shell", "ok": true, "exit": 0}
  ]
}
```

## Cross-process cancel marker

`<run-root>/cancel.marker` is a file the run loop watches:

- `kill <run-id>` writes the marker (timestamp + reason + signaller PID).
- The turn loop polls between turns and inside long HTTP requests via
  a `tokio::select!` arm that races the in-flight `reqwest` against a
  filesystem-watch future (250 ms cadence).
- On marker presence, the loop trips its cancellation token, drains
  child PIDs, sets `RunStatus::Killed`, removes the marker.
- Same-process kill (current behavior) still works: it flips the
  in-memory token directly and writes the marker as a belt-and-suspenders.

## Library query verb

```
deadreckon library list                 # promoted runs in current scope
    [--all]                             # every scope
    [--since <duration>]                # e.g. 7d, 24h
    [--limit <N>]                       # default 20
    [--full]                            # full IDs, exact paths
deadreckon library search <pattern>     # grep promoted run docs
    [--all]
    [--kind narrative|as-built|decisions]   # default: narrative
    [--max-snippets <N>]                # default 3
deadreckon library show <run-id>        # promoted-tree summary; aliases of `show`
```

Uses existing `~/.deadreckon/library/<scope>/<run-id>/manifest.json`
+ `docs/RUN-NARRATIVE.md` from the self-documenting rider; no new index
service.

Refusal cases:
| Condition | Error | `try:` |
|---|---|---|
| No promoted runs in scope | `no promoted runs in current project scope` | `deadreckon library list --all` |
| Pattern matches nothing | `no matches for '<pattern>'` | `deadreckon library list` |
| `show` ID ambiguous | `id prefix '<x>' matches N runs` | `deadreckon library list` |

## Doctor exhaustive

`deadreckon doctor` adds these checks (each with a `try:` footer):

- **OS/kernel sanity.** `sw_vers` (mac) / `uname -a` (linux); refuse
  unsupported majors with a `try: upgrade or use --insecure-os`.
- **Write-perm checks.** `~/.deadreckon/`, current scope's worktree
  parent, `/tmp` writable.
- **CLI provider versions.** `claude --version`, `codex --version` if
  binaries are present; report version + minimum-known-good.
- **Provider ping.** Behind `DEADRECKON_DOCTOR_PING=1`. Sends a 1-token
  request to the cheapest configured model; reports latency + cost
  estimate. Default off so routine doctor runs spend $0.
- **Library disk usage.** `du -sh ~/.deadreckon/library/<scope>` per
  scope; warn at >5 GB with `try: deadreckon cleanup --completed --older-than 30d`.

## Help discoverability + status polish

- `deadreckon --help` groups verbs by lifecycle stage:
  - **Setup**: `init`, `config`, `doctor`.
  - **Run**: `run`, `attach`, `kill`, `resume`, `extend`.
  - **Inspect**: `status`, `next`, `list`, `show`, `doc`, `library`.
  - **Land**: `apply`, `materialize`, `export`.
  - **Tidy**: `undo`, `abandon`, `discard`, `cleanup`, `prune`.
- `deadreckon status` adds a final block:
  ```
  Library: 12 promoted runs in current scope, 47 across all scopes
  Disk:    218 MB local runstate, 1.4 GB library
  Tip:     deadreckon cleanup --completed --older-than 30d  (frees ~880 MB)
  ```
- Post-action hints (existing) gain a `--no-hints` flag (already
  implemented) and `DEADRECKON_HINTS=0` env override; depth-tested.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry.

### P1 — Audit document

- Read REPORT.md and AS-BUILT.md; produce `docs/AUDIT-2026-05-11.md`
  per the schema above.
- The closures-map section is forward-looking (lists which phases will
  flip which rows); P11 revisits.

Depth tests (in `crates/deadreckon/tests/audit_harden.rs`):
- `audit_doc_lists_all_25_needs_with_status`
- `audit_doc_status_values_are_closed_enum`
- `audit_doc_evidence_paths_resolve_under_repo`
- `audit_doc_closures_map_lists_p2_through_p10`

### P2 — TUI event streaming

- New module `crates/deadreckon/src/tui_events.rs` that subscribes to
  `RunEventBus::subscribe()` for in-process attach and falls back to
  `events.jsonl` tailing for cross-process attach.
- Detect cross-process by checking whether the `attach` process is the
  same PID as the lock holder; same-PID → broadcast, different PID →
  file-tail at 100 ms cadence (down from 500 ms).
- Latency budget: same-process events surface in ≤ 50 ms;
  cross-process in ≤ 200 ms.

Depth tests:
- `same_process_attach_uses_broadcast_channel`
- `cross_process_attach_replays_events_jsonl`
- `kill_mid_turn_surfaces_run_completed_event_within_200ms`
- `attach_falls_back_to_file_replay_when_broadcast_unavailable`

### P3 — Cross-process cancel marker

- `kill <run-id>` writes `<run-root>/cancel.marker` before signalling
  child PIDs.
- Turn loop adds a `tokio::select!` arm racing each `reqwest` future
  against `watch_marker(cancel_marker_path, 250ms)`.
- Marker removal happens on graceful exit OR on the next process boot
  that finds it stale (>5 min, no live owner PID).

Depth tests:
- `kill_writes_cancel_marker_before_signalling`
- `another_process_run_loop_observes_marker_and_exits_within_500ms`
- `marker_cleared_after_graceful_kill`
- `stale_marker_reclaimed_on_next_run`

### P4 — Mid-tool-call resume grace

- `load_or_reconstruct_history` (in `turn_loop.rs`) detects a tool-call
  start without a matching tool-call result (the truncation case).
- Resume re-emits the start-side trace as `event: "tool.replayed"` and
  re-runs the tool dispatch from the same `tool_call_id`.
- The acceptance gate is unaffected — it sees the replayed result.

Depth tests:
- `truncated_traces_jsonl_resumes_at_partial_tool_boundary`
- `replayed_tool_call_keeps_original_tool_call_id`
- `complete_traces_unaffected_by_replay_logic`

### P5 — Sandbox per-tool policy

- Module `crates/deadreckon-sandbox/src/policy.rs`: load + merge
  per-scope and user `sandbox.toml`; expose `Policy::evaluate(tool,
  request) -> Allow | Deny { reason, try_hint }`.
- `turn_loop.rs` calls `policy.evaluate` before every tool dispatch.
- Refusal writes `provenance.jsonl` `event: "tool.refused"` line.

Depth tests:
- `policy_default_allows_all_when_no_sandbox_toml`
- `policy_default_allows_network_for_bash_and_cli_subagent`
- `policy_per_scope_overrides_user_overrides_default`
- `policy_denies_bash_reading_denied_path`
- `policy_denies_network_when_explicitly_set_false`
- `policy_denial_includes_try_hint_in_error_footer`
- `policy_refusal_lands_in_provenance_jsonl`

### P6 — Acceptance YAML spec

- Module `crates/deadreckon/src/bin/dr_gate_spec.rs`: parse
  `acceptance.yaml`; run each check; aggregate per-check results into
  the marker.
- Marker schema bumps `check_count` and adds the `checks: [...]`
  array. Validators tolerate marker-without-checks (back-compat).
- `deadreckon show <id>` prints the per-check breakdown when present.

Depth tests:
- `acceptance_yaml_runs_required_checks_in_order`
- `acceptance_yaml_optional_check_failure_does_not_fail_run`
- `acceptance_yaml_per_check_results_appear_in_marker`
- `acceptance_yaml_absence_falls_back_to_cargo_test_default`
- `acceptance_yaml_shell_check_records_exit_code`

### P7 — Doctor exhaustive

- Extend `doctor_command` (`main.rs:484`) with the OS/perm/version
  blocks. New helpers in a `doctor/` submodule.
- Provider-ping path opt-in via `DEADRECKON_DOCTOR_PING=1`; uses each
  provider's cheapest model + a 1-token completion.
- Each new line ends with a `try:` hint.

Depth tests:
- `doctor_reports_os_kernel_with_actionable_hint`
- `doctor_reports_write_perm_failures_with_chmod_hint`
- `doctor_probes_claude_codex_versions_when_present`
- `doctor_skips_provider_ping_unless_env_set`
- `doctor_reports_library_disk_usage_with_cleanup_hint`

### P8 — Library query verb

- New verb `deadreckon library {list, search, show}` per the rider.
- Backed by walking `~/.deadreckon/library/<scope>/<run-id>/`,
  reading `manifest.json` and `docs/RUN-NARRATIVE.md`.
- Search uses `regex` (already approved); snippet extraction from the
  narrative around the match (3 lines before/after).

Depth tests:
- `library_list_defaults_to_current_scope`
- `library_list_all_includes_other_scopes`
- `library_search_returns_match_with_snippet_and_run_id`
- `library_show_resolves_unique_prefix`
- `library_show_refuses_ambiguous_prefix_with_try_hint`

### P9 — Import round-trip parity

- For each importer (claude-code / codex / cursor), ship a golden
  fixture under `crates/deadreckon/tests/fixtures/import/`.
- After import, render `show <imported-id>` and snapshot to a golden
  expected output file; assert byte-exact.
- Round-trip means: fixture → import → `show` rendered output matches
  the committed golden.

Depth tests:
- `import_claude_code_fixture_round_trips_to_golden`
- `import_codex_fixture_round_trips_to_golden`
- `import_cursor_fixture_round_trips_to_golden`
- `import_renders_provenance_lines_for_each_file_change`
- `import_normalizes_timestamps_to_rfc3339`

### P10 — Help / status polish

- Group `deadreckon --help` verbs by lifecycle stage using clap's
  `command(next_help_heading = "...")`.
- Add the library/disk block to `deadreckon status`.
- Honor `DEADRECKON_HINTS=0` in addition to existing `--no-hints`.

Depth tests:
- `help_groups_verbs_by_lifecycle_stage`
- `status_includes_library_count_and_disk_usage`
- `status_tip_line_appears_when_disk_over_threshold`
- `hints_env_var_disables_post_action_hints`

### P11 — AS-BUILT update + audit revisit + CHANGELOG (doc only)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## 26. Hardening v2

  26.1 Audit document and closures map
  26.2 TUI event streaming (broadcast + cross-process replay)
  26.3 Cross-process cancel marker
  26.4 Mid-tool-call resume grace
  26.5 Sandbox per-tool policy
  26.6 Acceptance YAML spec
  26.7 Doctor exhaustive checks
  26.8 Library query verb
  26.9 Import round-trip parity
  26.10 Help discoverability and status polish
  ```

- Update §22 ("What's Built vs Scaffolding-Thin"):
  - Move from "Scaffolding-thin" to "Built and reliable":
    items 1, 2, 3, 5, 6, 7, 8, 10 of the §22 thin list (those addressed
    by P2–P10).
  - Leave items 4 (wall-clock budget richness) and 9 (multi-run
    scheduler/queue) on the thin list — out of scope here.
- Revisit `docs/AUDIT-2026-05-11.md`: add a "P11 revisit" column
  listing the new evidence file:line for each closed row, and flip
  the status to `Resolved` where applicable.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Hardening v2 (alpha) — 2026-05-11

  - Added docs/AUDIT-2026-05-11.md mapping the original 25 unmet needs to current evidence.
  - TUI attach now subscribes to RunEventBus for same-process and tails events.jsonl at 100ms for cross-process; kill-mid-turn surfaces in <200ms.
  - Cross-process kill writes a cancel marker the run loop honors inside long HTTP requests.
  - Resume tolerates traces.jsonl truncated mid-tool-call by replaying the partial boundary.
  - Sandbox grew a per-tool policy file (~/.deadreckon/sandbox.toml; per-scope override) with refusal hints in provenance.
  - dr-gate reads acceptance.yaml (tests / file-exists / content-match / build-success / shell) and records per-check results in the marker.
  - doctor adds OS/kernel sanity, write-perm checks, claude/codex version probes, opt-in provider-ping, and library-disk usage with cleanup hints.
  - New verb: deadreckon library {list, search, show}.
  - Import (claude-code / codex / cursor) covered by round-trip golden tests.
  - --help grouped by lifecycle stage; status reports library counts + disk usage.
  ```

## Integration matrix

| Closure | Touches | Files added | Files changed |
|---|---|---|---|
| TUI streaming (P2) | `attach` | `tui_events.rs` | `main.rs` (attach handler) |
| Cross-process cancel (P3) | `kill`, run loop | none | `main.rs`, `turn_loop.rs` |
| Mid-tool-call resume (P4) | `resume`, run loop | none | `turn_loop.rs` |
| Per-tool policy (P5) | sandbox, run loop | `sandbox/policy.rs` | `turn_loop.rs`, `main.rs` |
| Acceptance YAML (P6) | `dr-gate`, `show` | `dr_gate_spec.rs` | `gate.rs`, `main.rs` |
| Doctor exhaustive (P7) | `doctor` | `main/doctor/` | `main.rs` |
| Library query (P8) | new verb | `main/library.rs` | `main.rs` (clap) |
| Import parity (P9) | tests | `tests/fixtures/import/` | none source |
| Help / status (P10) | clap, `status` | none | `main.rs` |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `tool 'bash' refused by sandbox policy: denied path '<p>'` | `deadreckon run ... --allow-tool bash:read:<p>` |
| `tool 'bash' refused by sandbox policy: network denied` (only when operator opts in to deny) | `deadreckon config set tool.bash.allow_network = true` |
| `acceptance.yaml: required check 'tests' failed (exit 101)` | `cargo test --workspace` (locally) then `deadreckon resume` |
| `acceptance.yaml: malformed at line N: <msg>` | `deadreckon doctor` (which now lints acceptance.yaml) |
| `library: no promoted runs in current scope` | `deadreckon library list --all` |
| `library: pattern '<x>' matched nothing` | `deadreckon library list` |
| `library: id prefix '<x>' matches N runs` | `deadreckon library list` |
| `cancel marker present but lock holder PID is dead; reclaiming` | (informational; no action) |
| `doctor: ~/.deadreckon not writable` | `chmod -R u+w ~/.deadreckon` |
| `doctor: claude version <X> below known-good <Y>` | `npm i -g @anthropic-ai/claude-code` (or platform equivalent) |

(Each pair is parameterized over a depth test; see P5/P6/P7/P8.)

## Config additions (`config.toml`)

```toml
[defaults]
# Existing keys unchanged.
hints = true                          # mirrors --no-hints; honors DEADRECKON_HINTS env
library_disk_warn_gb = 5              # status / doctor threshold
library_disk_warn_age_days = 30       # cleanup hint

[doctor]
provider_ping = false                 # opt-in; or set DEADRECKON_DOCTOR_PING=1
```

`sandbox.toml` lives at its own path (see "Sandbox per-tool policy"
above), not in `config.toml`, because per-scope override is wanted.

## Out of scope (explicitly not in this milestone)

- **Wall-clock budget richness** (§22.4). Cap exists; richer mapping
  is V1.
- **Multi-run scheduler / queue** (§22.9). Lock+scope is enough for
  alpha; queueing is V1 (Need #25 partial; full queue stays V1).
- **Sub-agent forking verb** (already in V1-CANDIDATES.md).
- **Hooks system** / **MCP client** / **embeddings search** /
  **cost-aware routing** / **cloud sync** / **voice capture** — V1.
- **Cross-run doc rollups** ("what shipped in last 7 days across all
  runs") — V1; library `search` covers the inspect slice.
- **Auto-PR open** on `apply` — out of scope; `apply` remains local.
- **Permission UI** in TUI — sandbox policy is file-based in alpha;
  interactive permission dialogs are V1.
- **Queue management TUI panel** — out of scope.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):
- `notify` — filesystem watch for `cancel.marker`. Justification: low
  churn, no native deps on mac/linux. Alternative is polling at 250 ms,
  which is acceptable too — fall back to polling if `notify` is too
  large; depth test doesn't pin the mechanism.
- `serde_yaml` — already a transitive dep via `cargo`-adjacent crates;
  used for `acceptance.yaml` parsing.
- `regex` — already approved (orchestrate-rider, self-doc-rider).

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same blocks as prior riders (no `bollard`, no
`asciinema` runtime dep, no Lima).

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Audit findings, sandbox
  policy, acceptance results, library query data all live in files.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **Backwards-compatible.** Absence of `sandbox.toml` /
  `acceptance.yaml` keeps current behavior. No migrations.
- **Audit drives closures.** P2–P10 each cite the audit row(s) they
  close in the closure phase's commit message.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `V1-CANDIDATES.md`.
- **Spec-pinning invariants.** The `acceptance.yaml` schema, the
  `sandbox.toml` schema, the marker `checks` array shape, and the
  refusal-footer format are depth-tested; changing whitespace or
  ordering changes the spec.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, run a smoke flow end-to-end (`run` against a toy
  fixture in worktree mode with a `sandbox.toml` and an
  `acceptance.yaml`); capture an asciinema cast at
  `/Users/gdc/deadreckon/demo-hardening-v2.cast` if the change is
  user-visible. Skip the cast if the user-visible surface didn't
  shift in a demo-able way.
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
- The audit document is updated in P1 and revisited in P11; do not
  silently revise the audit between those two phases.
