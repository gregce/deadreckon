# deadreckon — Logbook Rider (one read model, many projections)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-03-1817-deadreckon-logbook-goal.md`.
It supersedes nothing in prior riders (Course `2026-07-01-2010`, Helm
`2026-07-01-2011`, Contract `2026-07-03-1304`, and earlier) — their invariants
still apply. This rider adds one thing: a single owned read model, `RunView`,
that every run-reading surface projects from, plus a per-turn diff primitive, a
static `report` verb, and readers for the artifacts that have none.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`/Users/gdc/.deadreckon` (tests and smoke use
`DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke`).

## The problem this closes (grounding)

A run is a directory of files under
`DeadreckonPaths::run_root(scope, run_id)`. Each read command parses those
files independently, so two failures recur:

- Blind files — no command reads them today. Confirmed by grep over
  `crates/deadreckon/src`:
  - the actual code diff (no `diff` verb, no `show --diff`; the change is only
    reachable by dropping to git on the `dr/...` branch)
  - `sandbox.toml` (referenced in zero CLI source files)
  - `history.json`, the per-turn model exchange (touched only by `resume`)
  - `events.jsonl` (`history grep` accepts only `trace` and `provenance`)
- Fact drift — the diff summarised in `RUN-NARRATIVE.md` is derived by a
  different path than git would use, so the two can disagree. No single piece of
  code owns the join across a run.

Helm (§47) already ships the live consolidated TUI (status spine, event tree,
`w`-for-why, turn timeline). This rider does not touch that surface's behavior.
It extracts the model underneath it, shares it, and reuses it for the static and
single-fact surfaces that Helm never covered.

## Posture (decided — do not redesign)

- **Maturity stays stable (0.5.0).** This is a read-model consolidation, not a
  new capability tier.
- **No `PipelineState` schema changes.** `RunView` is assembled at read time
  from files under `run_root`; it is never persisted and never a state field.
  Lineage/mode metadata stays in its marker files as today.
- **No new acceptance check kinds.** The four kinds (`cargo_test`,
  `file_exists`, `content_match`, `build_success`, `shell`) are unchanged; the
  proof band renders what already exists.
- **Diff excludes build output.** The per-turn snapshot copies the whole working
  tree including `target/`; the diff primitive filters `target/`, `.git/`, and
  the run's own `.deadreckon/` control dir. Source-only.
- **Behavior-preserving rewires are golden-guarded.** `show`, `verdict`, `doc`,
  and Helm attach must produce byte-identical output on existing characterization
  goldens after they are rewired to project from `RunView`. New behavior sits
  behind new flags.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Web report, cross-run stats, and syntax-highlighted diff
  are out of scope (see Out of scope); log any larger decision in
  `docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

`RunView` is a read-model struct, not a file. It lives in
`crates/deadreckon-core/src/run_view.rs` (new module) so both the binary
commands and the attach TUI can build it. It is assembled by
`RunView::load(paths: &DeadreckonPaths, scope: &str, run_id: &str)` and every
field degrades to an explicit `absent`/empty value when its backing file is
missing — loading a partial or crashed run must never panic.

```rust
// crates/deadreckon-core/src/run_view.rs
pub struct RunView {
    pub id: RunId,               // scope + run_id + short prefix
    pub goal: String,            // from state.json
    pub verdict: VerdictBand,    // VERIFIED / REGRESSED / UNVERIFIED + why-1-liner
    pub signature: SignatureFact,// valid | invalid | absent (from gate + tamper)
    pub sandbox: SandboxFact,    // backend name + fallback note (from sandbox.toml)
    pub spend: SpendBand,        // total + per-turn deltas (from spend.jsonl)
    pub wall_secs: Option<u64>,  // from state.json timestamps
    pub provider: String,        // route (from state.json / launch-plan.json)
    pub changed: DiffSummary,    // full-run diff summary (files, +/-)
    pub why: WhyBand,            // narrative + decisions refs (from docs/)
    pub turns: Vec<TurnView>,    // the spine
    pub proof: ProofBand,        // checks + marker + tamper (from proofs/, gate/)
    pub missing: Vec<Artifact>,  // which backing files were absent
}

pub struct TurnView {
    pub n: u32,
    pub did: String,             // one line from traces.jsonl for this turn
    pub diff: DiffSummary,       // snapshot(n-1) -> snapshot(n), target/ excluded
    pub spend_delta: Money,      // from spend.jsonl
    pub check: Option<CheckOutcome>, // gate check outcome if this turn hit the gate
    pub exchange_ref: Option<ExchangeRef>, // pointer into history.json for this turn
    pub sandbox_events: Vec<SandboxEvent>, // from events.jsonl scoped to the turn
}

pub enum Artifact {            // the addressable run files, for --raw and `missing`
    State, History, Spend, Traces, Events, Provenance,
    Acceptance, Sandbox, LaunchPlan, Seams,
    Proofs, GateNonce,          // GateNonce is addressable but NEVER dumped (see below)
    Snapshot(u32),
    Doc(DocKind),
}
```

`RunView` derives `Serialize` so every `--json` projection is the same struct.
The struct is the single source of truth; no command re-parses raw files after
this rider.

## Per-turn diff primitive

Add to `crates/deadreckon-core/src/artifacts.rs`, beside `snapshot_working` /
`restore_snapshot`:

```rust
pub fn diff_snapshots(a: &Path, b: &Path) -> Result<DiffSummary>;
pub fn snapshot_diff(state: &PipelineState, from: u32, to: u32) -> Result<DiffSummary>;
```

Rules:

- Walk both snapshot trees, skipping `target/`, `.git/`, and `.deadreckon/`.
- A `DiffSummary` is a list of `FileDelta { path, added, removed, status }`
  where `status ∈ {added, removed, modified}`, plus roll-up `files/+/-` counts.
- Full-run diff is `snapshot_diff(state, 0, last_turn)`. Per-turn diff is
  `snapshot_diff(state, n-1, n)`.
- Text diff is computed with the Tier-1 `similar` crate (see Dependencies). Full
  hunk text is available on demand; the summary carries counts only so `RunView`
  stays cheap to assemble.
- Binary or unreadable files report `status` with `added/removed = 0`, never
  error the whole diff.

## Verb signatures

```
report <run>                       # NEW: one self-contained artifact for a run
    [--html]                       # single inlined HTML file (default: markdown)
    [--dest <path>]                # write here (default: <run>/report.md|html, printed)
    [--open]                       # open the written file (TTY only)
    [--json]                       # emit the RunView struct instead of rendering

show <run>                         # rewired: now a projection of RunView
    [--diff]                       # full-run source diff (target/ excluded)
    [--turn <N>]                   # one turn: did, diff, exchange, sandbox events
    [--raw <artifact>]             # dump a named run file verbatim
    [--why-failed]                 # unchanged; now sourced from RunView.proof
    [--json]                       # RunView (or the selected band) as JSON

verdict <run>                      # rewired: projects RunView.verdict + .proof
doc <run>                          # rewired: projects RunView.why
history grep <pattern>
    [--kind trace|provenance|events]   # events kind ADDED
```

Refusal cases (each exercised by a named depth test):

| Verb / flag | Refusal | `try:` |
|---|---|---|
| `report <id>` unknown id | ambiguous/missing run | `deadreckon list` |
| `report` on a live run | run still in progress | `deadreckon attach <id>` |
| `show --turn <N>` out of range | no snapshot for turn N | `deadreckon show <id>` |
| `show --raw <artifact>` unknown name | not an addressable artifact | `deadreckon show <id> --json` |
| `show --raw gate-nonce` | secret is never dumpable | `deadreckon verdict <id>` |
| `show --diff` no snapshots | snapshots pruned/absent | `deadreckon show <id>` |
| `history grep --kind events` no file | run predates event ledger | `deadreckon show <id>` |

`--raw gate/nonce` must refuse by design: dumping the gate secret would let a
caller forge the marker. This refusal is a security invariant, depth-tested.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
green on `make verify`; conventional-commit; one-line CHANGELOG entry naming the
SHA.

### P1 — RunView read model (the join)

- New `crates/deadreckon-core/src/run_view.rs` with `RunView`, the band structs,
  and `RunView::load`. Assemble every band from files under `run_root`. No
  command consumes it yet.
- `missing` records every absent backing file; no absent file panics.

Depth tests (in `crates/deadreckon-core/src/run_view.rs` tests):
- `run_view_load_assembles_all_bands_from_fixture_run`
- `run_view_load_records_absent_files_in_missing_not_panic`
- `run_view_serializes_stable_json_shape`

### P2 — Per-turn diff primitive

- `diff_snapshots` / `snapshot_diff` in `artifacts.rs`; `target/`, `.git/`,
  `.deadreckon/` excluded; `similar` for text deltas.

Depth tests (in `artifacts.rs` tests):
- `snapshot_diff_reports_source_file_added_between_turns`
- `snapshot_diff_excludes_target_build_output`
- `snapshot_diff_handles_binary_and_missing_without_error`

### P3 — Fold the blind files into RunView

- `SandboxFact` from `sandbox.toml`; `TurnView.exchange_ref` from `history.json`;
  `TurnView.sandbox_events` from `events.jsonl` scoped by turn.

Depth tests:
- `run_view_sandbox_fact_names_backend_from_sandbox_toml`
- `run_view_turn_carries_model_exchange_ref_from_history`
- `run_view_turn_carries_sandbox_events_from_events_jsonl`

### P4 — `show` projects RunView + `--diff`

- Rewire `show` in `crates/deadreckon/src/commands/inspection.rs` to render from
  `RunView`; existing output byte-identical on goldens. Add `--diff` (full run).

Depth tests:
- `show_default_output_matches_characterization_golden`
- `show_diff_prints_full_run_source_diff`
- `show_diff_excludes_target_from_output`

### P5 — `show --turn`, `--raw`, `--json`

- `--turn <N>` renders one `TurnView` (did, diff, exchange, sandbox events);
  `--raw <artifact>` dumps a named file; `gate/nonce` refuses; `--json` emits the
  RunView band.

Depth tests:
- `show_turn_renders_diff_exchange_and_sandbox_events`
- `show_raw_dumps_named_artifact_verbatim`
- `show_raw_gate_nonce_refuses_with_verdict_try`
- `show_turn_out_of_range_refuses_with_show_try`

### P6 — `verdict` projects RunView.proof

- Rewire `verdict` (`crates/deadreckon/src/commands/verdict.rs`) to read the
  proof/signature/tamper facts from `RunView`; no drift with `show`.

Depth tests:
- `verdict_derives_signature_and_tamper_from_run_view`
- `verdict_and_show_report_identical_signature_fact`
- `verdict_default_output_matches_characterization_golden`

### P7 — `doc` projects RunView.why

- Rewire `doc` (`crates/deadreckon/src/commands/doc.rs`) to source the narrative
  and decisions refs from `RunView.why`; parity with the report why band.

Depth tests:
- `doc_why_band_sources_narrative_and_decisions_from_run_view`
- `doc_default_output_matches_characterization_golden`

### P8 — `report` command (static artifact)

- New `report` verb: render the whole `RunView` as five bands (verdict, changed,
  why, turns, proof). Markdown default; `--html` is one self-contained file with
  all CSS inlined and no external references. `--dest`, `--open`, `--json`.

Depth tests (in a new `crates/deadreckon/src/commands/report.rs` + tests):
- `report_markdown_contains_all_five_bands`
- `report_html_is_self_contained_no_external_refs`
- `report_json_emits_run_view_struct`
- `report_on_live_run_refuses_with_attach_try`

### P9 — Helm attach reads RunView

- Lift the shared read logic out of `crates/deadreckon/src/narrative.rs` so the
  attach timeline, `w`-evidence, and status spine derive from `RunView` (or a
  live superset of it) rather than tailing raw JSONL independently. Behavior
  preserved; Helm goldens unchanged.

Depth tests:
- `attach_timeline_turn_count_equals_run_view_turns`
- `attach_why_evidence_equals_run_view_proof`
- `attach_characterization_goldens_unchanged`

### P10 — Friendliness + surface parity

- One primary action footer on `report` and new `show` bands (VerdictSurface);
  refuse-with-`try:` for every row in the refusal table; lifecycle hints; add
  `events` to `history grep --kind`; `--json`/`--plain`/`--quiet` parity on all
  new surfaces.

Depth tests:
- `report_footer_names_one_primary_action`
- `history_grep_events_kind_matches_event_ledger`
- `show_new_flags_honor_plain_and_json`

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert into `docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## 49. Logbook: one read model, many projections

  49.1 RunView — the owned join across a run's files
  49.2 Per-turn diff over snapshots
  49.3 show --diff / --turn / --raw and the blind-file readers
  49.4 report — the static run artifact
  49.5 Helm attach as a RunView projection
  ```
- Update any "what's shipped vs thin" section: add RunView, the per-turn diff,
  `report`, and the blind-file readers to shipped; note this closes UNMET-NEEDS
  C3 (static report) and the read-side of the introspection gaps, and does NOT
  close C2 (cross-run stats) or F2 (CLI token telemetry).
- Append to `CHANGELOG.md`:
  ```
  ## Logbook (stable) — 2026-07-03

  - RunView read model; show/verdict/doc rewired as projections
  - per-turn diff primitive; show --diff / --turn / --raw
  - report verb (markdown + self-contained --html)
  - history grep --kind events; attach reads the shared model
  ```

## Integration matrix

| Surface | Reads | Projection of RunView | New behavior |
|---|---|---|---|
| `show` | inspection.rs | yes (P4) | `--diff`, `--turn`, `--raw`, `--json` |
| `verdict` | verdict.rs | yes (P6) | none (parity only) |
| `doc` | doc.rs | yes (P7) | none (parity only) |
| `report` | report.rs (new) | yes (P8) | whole model, static |
| `attach` (Helm) | narrative.rs | yes (P9) | none (dedupe only) |
| `history grep` | inspection.rs | no (ledger search) | `--kind events` |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `run <id> not found` | `deadreckon list` |
| `run <id> still running` | `deadreckon attach <id>` |
| `no snapshot for turn N` | `deadreckon show <id>` |
| `unknown artifact <name>` | `deadreckon show <id> --json` |
| `gate secret is not dumpable` | `deadreckon verdict <id>` |
| `no event ledger for this run` | `deadreckon show <id>` |

## Config additions

None required. `report` picks markdown/HTML by flag; no new `[defaults]` knob in
this slice. A default report format is a V1 candidate.

## Out of scope (explicitly not in this milestone)

- A web-served or live-refreshing report (static file only).
- Cross-run aggregate stats — spend-per-change, turns-to-done (UNMET-NEEDS C2).
- CLI-provider token telemetry / context meter (UNMET-NEEDS F2).
- Syntax highlighting or side-by-side rendering in the diff (unified text only).
- Any new acceptance check kind or `PipelineState` field.
- MCP exposure of `report`/`RunView` (rides a later interop slice).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free): `similar` for text diffs (widely used, no transitive
weight of note). Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected
— the HTML report inlines its own CSS rather than pulling a templating or CSS
crate. Tier 3 (blocked): same blocks as prior riders (no network, no telemetry,
no framework migration).

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** `RunView` is assembled at read time and
  never persisted.
- **One depth test before each phase implementation.** A phase whose tests were
  never red is suspect.
- **RunView is the only reader.** After P6, no command re-parses `spend.jsonl`,
  `provenance.jsonl`, `traces.jsonl`, `proofs/`, `sandbox.toml`, or `history.json`
  outside `RunView::load`. Grep for direct reads in a test if practical.
- **The gate secret is never rendered.** `--raw gate/nonce` refuses; depth-tested
  as a security invariant.
- **Diff is source-only.** `target/`, `.git/`, `.deadreckon/` are always excluded.
- **Behavior-preserving means golden-identical.** Rewired commands change no
  existing output byte; new output is flag-gated.
- **No silent expansion.** Anything beyond P1–P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the
  SHA.
- After P11, optionally capture a short asciinema cast of `deadreckon report
  latest` and `show --diff` under `/Users/gdc/deadreckon/` demo assets. Skip if
  the terminal capture adds no clarity.
- If a phase reveals a V1-architecture decision (for example, a report that must
  stream for very large runs), stop and log it in `V1-CANDIDATES.md`; do not
  expand scope.
