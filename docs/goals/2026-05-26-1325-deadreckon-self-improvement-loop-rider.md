# deadreckon - Self-Improvement Loop Rider (Improving)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-26-1325-deadreckon-self-improvement-loop-goal.md`.
It supersedes nothing in prior riders
(`2026-05-25-2238-deadreckon-provider-flight-recorder-rider.md`,
`2026-05-24-1426-deadreckon-provider-done-setup-rider.md`,
`2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`,
`2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md`) - their
invariants still apply. This rider adds a local evidence index, proposal loop,
self-run mode, and evidence-gated PR opening for DeadReckon improving itself.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** New durable state lives in files under
  `DEADRECKON_HOME/learning/` and candidate run roots.
- **Local-first by default.** No cloud sync, model fine-tuning, background
  telemetry, or sharing of other users' data in this milestone.
- **Self-improvement is proposal-plus-evidence, not blind autopilot.** A
  proposal must cite observed DeadReckon stimulus and a measurable done
  contract before any self-run starts.
- **PR opening is opt-in and evidence-gated.** The product may push/open only
  when the user invokes `--open-pr` or an explicit local policy enables it and
  all criteria pass. Implementation tests must use fake/dry-run adapters.
- **No live `git push` while executing this goal.** Build the capability; do
  not use it against GitHub during the goal implementation unless the human
  separately asks.
- **No V1 invention.** Anything beyond the P1-P11 slices goes to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Research grounding

Treat these as design pressure, not dependencies:

- Reflexion (`https://arxiv.org/abs/2303.11366`) supports storing verbal
  reflections from failed/successful episodes instead of retraining weights.
- Voyager (`https://arxiv.org/abs/2305.16291`) supports an open-ended curriculum
  plus reusable skill/proposal library, but DeadReckon should store proposals
  and evidence packets rather than executable skills first.
- SWE-agent/SWE-bench (`https://arxiv.org/abs/2405.15793`) reinforces that
  coding-agent progress needs reproducible problem statements and tests.
- Darwin Godel Machine (`https://arxiv.org/abs/2505.22954`) is the closest
  shape: archive self-modification attempts and promote only candidates with
  empirical evidence. DeadReckon's alpha version must stay local, auditable, and
  PR-based.

## Current substrate

DeadReckon already has the ingredients:

- Durable run roots under `DEADRECKON_HOME/runstate/` with `state.json`,
  `events.jsonl`, `traces.jsonl`, `spend.jsonl`, snapshots, gate proofs, and
  docs.
- Plan, chain, and flight recorder artifacts that expose multi-agent,
  provider-native, and checkpoint signals without altering `PipelineState`.
- `dr-gate` acceptance and promotion boundaries, plus setup helpers for
  provider and done criteria.
- Hidden `import` and provider ingest paths that can normalize external
  transcripts, but do not yet aggregate cross-run learning.

This goal composes those files into an experience index and self-run wrapper.

## Data model (files, not fields)

All learning files are versioned JSON/JSONL. Unknown future fields must be
ignored by readers. Paths in exported bundles must be redacted unless they are
relative to the project root.

### `DEADRECKON_HOME/learning/episodes/<scope>/<run-id>.json`

```json
{
  "version": 1,
  "run_id": "dr-...",
  "scope": "default",
  "task_key": "deadreckon",
  "project_root_hash": "sha256:...",
  "created_at": "<RFC3339>",
  "completed_at": "<RFC3339|null>",
  "operation_mode": "run|extend|resume|orchestrate|chain|import",
  "provider_routes": [{"role": "primary", "id": "cli:codex"}],
  "sandbox": {"backend": "seatbelt|bubblewrap|none", "mode": "worktree"},
  "goal_digest": "sha256:...",
  "goal_summary": "short redacted summary",
  "outcome": "completed|failed|killed|paused|abandoned",
  "done_criteria": {"kind": "project|generated|default", "weak": false},
  "metrics": {
    "turns": 4,
    "wall_seconds": 812,
    "spend_usd": 0.0,
    "gate_failures": 1,
    "doc_warnings": 0,
    "rewinds": 0
  },
  "artifacts": {
    "state": "runstate/.../state.json",
    "events": "runstate/.../events.jsonl",
    "flight": "runstate/.../flight-events.jsonl"
  },
  "redaction": {"profile": "local-v1", "findings": []}
}
```

### `DEADRECKON_HOME/learning/signals.jsonl`

Each signal is an extracted, explainable observation:

```json
{
  "version": 1,
  "signal_id": "sig-...",
  "run_id": "dr-...",
  "timestamp": "<RFC3339>",
  "kind": "repeat_failure|slow_path|provider_gap|setup_friction|docs_drift|tui_gap|acceptance_gap|cost_spike",
  "severity": "low|medium|high",
  "confidence": 0.82,
  "summary": "3 recent runs failed because setup asked for missing provider route",
  "evidence_refs": [{"file": "events.jsonl", "line": 17}],
  "privacy": "local-only|shareable-redacted"
}
```

### `DEADRECKON_HOME/learning/proposals/<proposal-id>.json`

```json
{
  "version": 1,
  "proposal_id": "prop-...",
  "created_at": "<RFC3339>",
  "title": "Unify missing-provider recovery hints",
  "stimulus": [{"signal_id": "sig-...", "run_id": "dr-..."}],
  "hypothesis": "A shared recovery footer will reduce repeated setup failures",
  "target": {"repo": "/Users/gdc/deadreckon", "scope": "cli-friendliness"},
  "goal_text": "Implement ...",
  "done_criteria": [
    "focused tests cover missing provider recovery for run and orchestrate",
    "plain/json/quiet behavior stays unchanged"
  ],
  "expected_risk": "low|medium|high",
  "blocked_auto_pr_reasons": []
}
```

### `DEADRECKON_HOME/learning/candidates/<candidate-id>/candidate.json`

```json
{
  "version": 1,
  "candidate_id": "cand-...",
  "proposal_id": "prop-...",
  "branch": "deadreckon/self/cand-...",
  "base_commit": "abc123",
  "head_commit": "def456",
  "run_id": "dr-...",
  "worktree": "/tmp/deadreckon-self/cand-...",
  "diff": {"files": 4, "insertions": 120, "deletions": 42},
  "risk": {"class": "low|medium|high", "reasons": []},
  "status": "running|verified|rejected|pr_opened",
  "evidence_packet": "evidence.json"
}
```

### `DEADRECKON_HOME/learning/evals/<candidate-id>.json`

```json
{
  "version": 1,
  "candidate_id": "cand-...",
  "evaluated_at": "<RFC3339>",
  "accepted_run": true,
  "commands": [
    {"cmd": "cargo test -p deadreckon --test learning", "status": 0}
  ],
  "docs_updated": true,
  "redaction_passed": true,
  "evidence_score": 0.91,
  "auto_pr": {"eligible": true, "reasons": []}
}
```

### `DEADRECKON_HOME/learning/pr-events.jsonl`

Append-only audit rows for PR dry-runs and live attempts:

```json
{
  "version": 1,
  "timestamp": "<RFC3339>",
  "candidate_id": "cand-...",
  "mode": "dry-run|open",
  "status": "prepared|opened|refused|failed",
  "branch": "deadreckon/self/cand-...",
  "pr_url": null,
  "body_path": "pr-body.md",
  "reason": null
}
```

### `DEADRECKON_HOME/learning/policy.toml`

Optional local policy. Do not add global config keys unless the implementation
needs them.

```toml
[learning]
enabled = true
export_redaction = "local-v1"

[learning.self]
require_isolated_worktree = true
allow_sandbox_none = false
verification_profile = "focused"

[learning.pr]
auto_open = false
default_dry_run = true
min_evidence_score = 0.85
require_docs_for_public_surface = true
block_high_risk = true
```

## Algorithms

### Episode indexing

1. Discover candidate run roots through `DeadReckonPaths::runstate_dir()` and
   scope roots.
2. Load `state.json`; skip live or corrupt runs with a warning signal rather
   than failing the whole index.
3. Read `events.jsonl`, `traces.jsonl`, `spend.jsonl`, `acceptance-progress.jsonl`,
   plan/chain files, and flight files when present.
4. Produce one `episode` file per run and append zero or more `signals`.
5. Redact absolute home paths, credential-like values, provider raw content,
   raw prompts beyond short summaries, and provider-owned log bodies.
6. Use stable content hashes so unchanged runs do not rewrite episode files.

### Signal extraction

Classifiers are deterministic rules in this milestone. Do not call an LLM for
indexing. Examples:

- Repeated provider setup failures across recent runs -> `setup_friction`.
- Gate failures followed by successful retries -> `acceptance_gap`.
- CLI provider subturns without flight events -> `provider_gap`.
- Long wallclock with low file-change count -> `slow_path`.
- User resumes/rewinds/kills similar runs -> `repeat_failure`.

### Proposal generation

`learn propose` may use a provider only after it has gathered deterministic
signals. The prompt must ask for JSON proposals that cite signal ids and
define testable done criteria. Invalid provider output is refused; it is not
silently massaged into a proposal.

### Self-run mode

`improve self` executes a proposal against `/Users/gdc/deadreckon/`:

1. Verify the source worktree is clean unless `--allow-dirty-base` exists in a
   future explicit design. Do not add that flag in this milestone.
2. Create an isolated git worktree on `deadreckon/self/<candidate-id>`.
3. Materialize the proposal into a goal file inside the candidate run root, not
   into `docs/goals/` unless the proposal explicitly targets docs.
4. Launch a normal DeadReckon run in the candidate worktree with focused done
   criteria and no provider preselection beyond the existing provider resolver.
5. Require the outer coordinator to evaluate evidence using the baseline
   binary, not candidate-modified gate logic.
6. Archive the candidate diff, run id, verification results, and PR body.
7. On failure, leave the branch/worktree discoverable and print one rollback
   command.

### Evidence score

Start with a simple deterministic score; do not hide it behind LLM judgment.
Suggested weights:

- 0.20 proposal cites at least one valid signal and run id.
- 0.20 candidate run completed with accepted done criteria.
- 0.20 focused verification commands passed.
- 0.15 changed files match proposal scope and no high-risk paths are touched.
- 0.10 docs/CHANGELOG updated when user-facing or architectural behavior moved.
- 0.10 redaction/secrets scan passed.
- 0.05 rollback command and branch metadata exist.

Default PR threshold is `>= 0.85`.

### Auto-PR gate

Auto-PR eligibility requires all hard criteria:

1. Explicit opt-in: `--open-pr` or `learning.pr.auto_open = true`.
2. Clean base at candidate start and no unrelated changes in source worktree.
3. Isolated worktree mode; no in-place self-run.
4. Sandbox backend is not `none`.
5. Done criteria are not weak default criteria.
6. Candidate run accepted and the outer baseline evaluator recorded it.
7. Focused verification commands passed.
8. Evidence score is at or above policy threshold.
9. Redaction/secrets scan found no blocking finding.
10. High-risk diff policy passes.
11. PR branch points at the evaluated commit hash.

High-risk paths block auto-PR by default:

- `crates/deadreckon-core/src/gate.rs`
- `crates/deadreckon/src/bin/dr-gate.rs`
- `crates/deadreckon-sandbox/`
- provider credential/config handling
- `.github/workflows/`, release, install, or publishing scripts
- acceptance policy weakening
- files under user home outside `/Users/gdc/deadreckon/`

Manual PR preparation may still write a draft body for high-risk changes, but
must refuse live opening and print why.

### PR builder

PR body sections are fixed and depth-tested:

1. Summary
2. Stimulus and proposal
3. Evidence packet
4. Verification
5. Risk classification
6. Rollback
7. Files changed

`--pr-dry-run` writes the branch name, title, and body to candidate files and
appends `pr-events.jsonl` without network or push. `--open-pr` may call a small
adapter around `gh` or a GitHub connector only after eligibility passes. Tests
must use a fake adapter.

## Verb signatures

```text
deadreckon learn index
    [--scope <scope>]
    [--all]
    [--since <RFC3339|duration>]
    [--json]

deadreckon learn report
    [--scope <scope>]
    [--limit <n>]
    [--json]

deadreckon learn export <run-id|proposal-id>
    [--output <path>]
    [--redacted]

deadreckon learn import-bundle <path>
    [--preview]
    [--yes]

deadreckon learn propose
    [--from-local]
    [--bundle <path>]
    [--limit <n>]
    [--json]

deadreckon improve self <proposal-id|goal-file>
    [--preview]
    [--yes]
    [--pr-dry-run]
    [--open-pr]
    [--json]
```

Refusal cases:

| Command | Refuse when |
|---|---|
| `learn index` | run root is corrupt and cannot be partially read; continue for other runs |
| `learn export` | redaction would be disabled for a bundle containing provider/home data |
| `learn import-bundle` | bundle schema unknown, hashes fail, or bundle is not redacted |
| `learn propose` | no signals meet confidence threshold |
| `improve self --yes` | base worktree dirty, sandbox none, weak done criteria, or no provider route resolves |
| `improve self --open-pr` | any Auto-PR gate criterion fails |

## Phases (eleven)

Each phase writes the named depth tests first and watches them fail; implements
the smallest slice; runs focused verification; makes a conventional local
commit; and appends a one-line CHANGELOG entry. Do not run full-workspace
verification by default.

### P1 - Learning paths and durable types

- Add learning path helpers under core or CLI-adjacent code without touching
  `PipelineState`.
- Define versioned structs for episodes, signals, proposals, candidates, evals,
  policy, and PR audit rows.
- Readers ignore unknown fields and reject unknown major versions.

Depth tests:
- `learning_paths_stay_under_deadreckon_home`
- `learning_schemas_roundtrip_and_reject_unknown_major_version`
- `episode_writer_is_idempotent_for_unchanged_run`

### P2 - Episode indexer

- Build `learn index` over completed run roots.
- Include runstate, traces, spend, docs warnings, gate outcomes, plan/chain
  context, and flight files when present.
- Skip corrupt runs with a warning signal.

Depth tests:
- `learn_index_writes_episode_from_completed_run`
- `learn_index_skips_live_run_without_failure`
- `learn_index_includes_flight_and_gate_metrics`

### P3 - Redaction and bundle import/export

- Implement redaction profiles before export.
- `learn export` emits redacted bundles with manifests and hashes.
- `learn import-bundle --preview` explains what would be imported.

Depth tests:
- `learn_export_redacts_home_paths_provider_logs_and_secret_like_values`
- `learn_import_bundle_preview_refuses_unredacted_bundle`
- `learn_import_bundle_hash_mismatch_has_try_footer`

### P4 - Signal extraction and reports

- Add deterministic signal rules for repeated failures, setup friction,
  acceptance gaps, provider gaps, slow paths, docs drift, cost spikes, and TUI
  freshness gaps.
- `learn report` renders concise text and JSON parity.

Depth tests:
- `signal_rules_extract_setup_friction_from_repeated_provider_failures`
- `signal_rules_extract_acceptance_gap_from_gate_retry`
- `learn_report_json_matches_text_counts`

### P5 - Proposal generation

- `learn propose` creates proposal JSON tied to signal ids and run ids.
- Provider-backed proposal text is optional and must validate strictly.
- Store proposal done criteria and expected risk.

Depth tests:
- `learn_propose_refuses_when_no_signal_meets_threshold`
- `learn_propose_requires_signal_citations_and_done_criteria`
- `learn_propose_invalid_provider_json_does_not_write_proposal`

### P6 - Candidate archive and evaluation policy

- Add candidate directories, diff metadata, eval records, and evidence packet
  assembly.
- Implement deterministic evidence scoring.
- Keep verification profile focused and explicit.

Depth tests:
- `candidate_archive_records_base_head_diff_and_run_id`
- `evidence_score_requires_signal_run_verification_and_rollback`
- `evaluation_policy_blocks_weak_done_criteria`

### P7 - Self-run preview and launch

- `improve self --preview` prints the proposal, mode, worktree, done criteria,
  verification profile, and PR posture without side effects.
- `improve self --yes` creates an isolated worktree and launches a normal
  DeadReckon run against the candidate goal.
- Do not preselect providers; use the existing resolver.

Depth tests:
- `improve_self_preview_has_no_worktree_or_run_side_effect`
- `improve_self_launch_uses_isolated_worktree_and_existing_provider_resolver`
- `improve_self_refuses_dirty_base_and_sandbox_none`

### P8 - Evidence gate and PR dry-run

- Implement the Auto-PR gate exactly as specified above.
- `--pr-dry-run` writes title/body/branch metadata and `pr-events.jsonl`.
- High-risk diffs produce a draft body but refuse live opening.

Depth tests:
- `pr_gate_passes_only_with_complete_evidence_packet`
- `pr_dry_run_writes_body_without_network_or_push`
- `pr_gate_blocks_high_risk_gate_sandbox_credential_and_release_paths`

### P9 - Live PR adapter behind evidence gate

- Add a small adapter interface for opening PRs.
- Production adapter may use `gh` or GitHub connector availability; tests use
  a fake adapter.
- The adapter is called only after the gate says eligible and the candidate
  commit hash matches the evaluated hash.

Depth tests:
- `open_pr_adapter_not_called_when_evidence_gate_refuses`
- `open_pr_adapter_receives_fixed_body_sections_and_evaluated_head`
- `open_pr_records_pr_events_row_with_url`

### P10 - CLI friendliness, JSON/plain/quiet, and help

- Add help text for `learn` and `improve self`.
- JSON output must have parity for report, proposal, preview, and PR dry-run.
- Plain/quiet behavior follows the existing matrix: quiet suppresses chatter,
  never data or errors.
- Every refusal prints one concrete `try:` footer.

Depth tests:
- `learn_and_improve_help_use_provider_route_and_done_criteria_vocabulary`
- `learn_report_plain_json_and_quiet_have_expected_parity`
- `improve_self_refusals_emit_canonical_try_footers`

### P11 - Architecture docs, CHANGELOG, and V1 deferrals

- Add an AS-BUILT section:
  ```text
  ## NN. Local Self-Improvement Loop
  NN.1 Experience index
  NN.2 Signals and proposals
  NN.3 Self-run candidates
  NN.4 Evidence-gated PR opening
  NN.5 Privacy, redaction, and non-goals
  ```
- Update `docs/V1-CANDIDATES.md` with deferred cloud learning, cross-machine
  sharing, provider-routing learning, semantic replay, model training, richer
  eval suites, and policy hardening.
- Add a CHANGELOG section for "Self-Improvement Loop (alpha)".
- Document that live PR opening is opt-in, while dry-run is the default test
  path.

Depth tests:
- `docs_as_built_mentions_learning_files_evidence_gate_and_pr_limits`
- `v1_candidates_record_out_of_scope_learning_items`
- `changelog_has_self_improvement_loop_alpha_entry`

## Integration matrix

| Surface | Experience index | Self-run | Auto-PR |
|---|---|---|---|
| `run` | source episodes | target execution primitive | evidence source |
| `extend`/`resume` | lineage signals | not special-cased | evidence source |
| `orchestrate`/`plan` | plan and child summaries | future multi-candidate mode | evidence source only |
| `chain` | step-level friction and wallclock | future candidate chains | evidence source only |
| `import` | imported bundles feed signals | no live replay in this goal | no PR from imported-only evidence |
| flight recorder | provider subturn and checkpoint signals | candidate debug evidence | high-value PR body links |
| TUI | no new TUI in alpha | attach existing run | future learning dashboard |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| No indexable runs | `try: deadreckon run <goal> --yes` |
| Bundle is not redacted | `try: deadreckon learn export <id> --redacted` |
| No proposal-worthy signals | `try: deadreckon learn index --all` |
| Dirty base worktree | `try: git status --short` |
| Sandbox backend is none | `try: deadreckon config sandbox auto` |
| Weak done criteria | `try: deadreckon def-done --goal <file>` |
| PR gate failed | `try: deadreckon improve self <proposal> --pr-dry-run` |
| High-risk diff blocked | `try: open the generated PR draft manually after review` |
| Missing GitHub adapter | `try: gh auth status` |

## Config additions

Prefer `DEADRECKON_HOME/learning/policy.toml` over global config schema changes.
Do not add durable config keys unless a phase proves the local policy file is
insufficient. Defaults:

- Learning index is user-invoked, not background.
- Bundle export is redacted by default.
- Self-run requires isolated worktree.
- PR mode defaults to dry-run.
- Live auto-open requires explicit opt-in plus evidence gate success.

## Out of scope (explicitly not in this milestone)

- Training or fine-tuning models from DeadReckon runs.
- Cloud telemetry, background sync, or shared remote learning stores.
- Automatically applying self-improvement changes to `main`.
- Live PR opening in tests or during goal execution.
- Imported-only external bundles opening PRs without local corroborating runs.
- Multi-candidate evolutionary search beyond one proposal at a time.
- Provider-routing learning that automatically changes defaults.
- Semantic replay of provider sessions or AST-aware rewind.
- A learning TUI dashboard.
- Cryptographically tamper-proof audit logs.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):

- Existing Rust crates already in the workspace for JSON, time, hashing, temp
  dirs, and command execution.
- `gh` as an optional production PR adapter if already available to the user.

Tier 2 (architectural, log to `DEPENDENCIES.md` if added):

- A secrets scanning crate or git diff parser if existing workspace utilities
  are insufficient.
- A GitHub API client crate if `gh`/connector abstraction proves inadequate.

Tier 3 (blocked):

- Hosted telemetry SDKs.
- Services that upload run artifacts by default.
- Any dependency that requires storing provider credentials outside the existing
  provider config model.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.**
- **Files-not-fields.** Learning state lives under `DEADRECKON_HOME/learning/`
  and candidate run roots.
- **Depth tests first.** Every P1-P11 phase has named tests above; if a phase's
  tests were never red, call that out in the commit message.
- **Do not trust candidate-modified gate logic.** The outer baseline evaluator
  decides evidence eligibility.
- **Redaction before sharing.** No raw provider logs, credentials, home paths,
  or secret-like strings in export bundles or PR bodies.
- **Auto-PR is never a fallback.** If evidence is incomplete, refuse with a
  footer; do not silently downgrade to live PR.
- **Spec-pinned PR body.** The seven PR body sections are depth-tested.
- **No silent expansion.** Anything beyond P1-P11 goes into
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No live `git push` while executing this goal.
- Each phase ends with focused tests passing and a CHANGELOG entry naming the
  change.
- Use focused verification: targeted cargo tests for touched crates, CLI
  snapshots/goldens for touched commands, `cargo fmt --check`, and clippy for
  touched crates when relevant.
- Avoid `make verify`, release builds, stress tests, broad smoke suites, and
  full-workspace tests by default; run them only if a phase touches broad
  surfaces and the commit explains why.
- After P11, optionally capture a dry-run transcript under docs if it clarifies
  the user flow. Do not capture secrets or provider logs.
