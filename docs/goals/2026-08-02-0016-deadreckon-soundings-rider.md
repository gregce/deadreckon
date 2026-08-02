# deadreckon — Soundings Rider (source-true, bounded launch admission)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-08-02-0016-deadreckon-soundings-goal.md`.
It supersedes nothing in Course, Contract or Watchkeeper. Their invariants
still apply. Soundings closes one seam between them: Course preview, Contract
authoring and Watchkeeper admission must agree on the source and must finish
within a visible wall budget.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`$DEADRECKON_HOME` or `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable.** This is launch-admission correctness and latency,
  not a new orchestration product.
- **One source resolution.** Mode/source selection is an in-memory value built
  once and consumed by preview, acceptance, Job authority and dispatch. No
  downstream arm may reinterpret raw flags.
- **Graph `--from` is supported.** Review/full-plan use it to seed the Graph
  parent's controller-owned approved source. Do not replace the late refusal
  with an early refusal; the previewed continuation use case is legitimate.
- **Source is frozen before agents.** A copy source includes deliverable
  tracked, modified and untracked files, is digest-checked, and is never
  modified by planning or child execution.
- **Writing and inspecting are separate.** Done-contract files are written to
  the launch project root; project evidence is read from the resolved source.
  Generated checks use `{working_dir}`, never the original absolute path.
- **No durable schema changes.** Reuse `DurableSource`, `JobAuthority`,
  `launch-plan.json`, `.deadreckon/acceptance.yaml`, `.md`, and helper files.
  New context/policy types are request-scoped. Controller-owned source bytes
  may live under the existing Job directory.
- **Deterministic floor, bounded model ceiling.** Local validation and project
  dossier construction are provider-free. Model calls cannot outlive the
  cumulative done-authoring deadline.
- **One critic and one redraft remain the ceiling.** Preserve Contract's
  anti-loop invariant while making the redraft informed and bounded.
- **No silent weakening.** Timeout, unsupported structured-text posture or a
  weak final contract refuses before Job creation; it never falls through to
  a directory-exists or self-attested gate.
- **No git push, release, live service mutation or live paid dogfood.** Phased
  local commits only; edits stay inside `/Users/gdc/deadreckon`.

## Verified failure chain (do not re-diagnose)

The 2026-08-02 reproduction established all of these facts:

1. `resolve_start_setup` in `commands/start.rs` resolved done criteria before
   source mode (`resolve_start_done_criteria` precedes
   `resolve_start_source_mode`).
2. `materialize_start_done_criteria` used `std::env::current_dir()` for both
   output and inspection. The current directory contained only `.specstory`;
   `--from` named a populated Swift project with `Package.swift`,
   `Sources/Cloudwing`, and `Tests/CloudwingTests`.
3. Preview nevertheless rendered `workspace: copy from <source>`.
4. Initial definition-of-done drafting ran from 03:55:57Z to 04:01:49Z
   (about 352 seconds). The critic took about 14 seconds. Redrafting ran from
   04:02:17Z to 04:10:00Z (about 463 seconds) and performed unnecessary web
   searches for the DeadReckon schema.
5. The redraft was a fresh read-only provider session. It did not receive the
   prior YAML, Markdown or helper files, only a thinned rejection summary.
6. The critic returned `"verdict":"reject"`; `CriticDecision` accepted only
   `pass|redraft`, so `parse_critic_verdict` discarded the provider verdict and
   fell back to a deterministic result with empty missing-clause/check lists.
7. `with_cli_wait_status` displayed elapsed time but imposed no timeout.
   `cli_common::run_cli` awaited `child.wait_with_output()` without a wall
   bound. `ProviderRequest::enforceably_read_only` supplied neither a
   cancellation token nor output schema.
8. The completed contract invented a `FlappyBird` executable, while the real
   package product is `Cloudwing`, and required a screenshot path that had not
   been supplied.
9. Only after the operator accepted the contract did
   `dispatch_advanced_start_job` reject source flags. It then hardcoded Graph
   source to `Worktree { from: None }`, so no Job ID existed despite the
   accepted preview.

This is a causal chain, not nine independent paper cuts: wrong ordering chose
the wrong inspection root; incomplete context encouraged tool discovery and a
wrong contract; fresh redraft plus parser loss repeated work; unbounded CLI
waiting turned that work into minutes; late dispatch validation discarded the
whole interaction.

## In-memory model (not persisted)

Add one resolved launch-source value. Names may adapt to local conventions,
but the separation of responsibilities is normative:

```rust
struct ResolvedStartSource {
    mode: DurableSourceMode,
    requested_from: Option<PathBuf>, // canonical operator input
    inspection_root: PathBuf,        // source truth used by Course + Contract
    contract_write_root: PathBuf,    // launch project receiving .deadreckon/
    allow_dirty: bool,
    provenance: StartSourceProvenance,
}

enum StartSourceProvenance {
    CurrentCleanWorktree,
    ExplicitCopy,
    ExplicitFresh,
    InitGit,
}
```

`ResolvedStartSource` is built after launch mode is known and before any
provider request or state-changing prompt. It is stored only in the ephemeral
`StartLaunchDecision`. `start_source_flags_present` remains a parser helper,
not a second policy engine.

Acceptance authoring receives explicit roots:

```rust
struct AcceptanceAuthoringContext<'a> {
    write_root: &'a Path,
    inspect_root: &'a Path,
    goal: Option<&'a str>,
}
```

`acceptance_agent_command_in_dir` may be renamed or wrapped, but no call may
silently use one `cwd` for both meanings. Direct `def-done` uses the same path
for both roots. Guided `start --from` does not.

## Launch-admission algorithm (the spec)

Execute in this order:

```text
parse flags
  -> resolve launch shape and deterministic flag compatibility
  -> resolve one ResolvedStartSource
  -> validate/canonicalize/read source; compute bounded dossier
  -> resolve provider/team
  -> resolve or author done contract against inspection_root
  -> compile/lint/critic within one deadline
  -> operator review
  -> render preview from the same decision
  -> final confirmation
  -> freeze contract + source into Job authority
  -> dispatch using the already-resolved DurableSource
```

An implementation may keep provider/team selection before deterministic
dossier construction for UI reasons, but **no provider process, done-contract
write, contract review or final confirmation may occur before source/mode
compatibility has passed**.

Preview and dispatch are projections of the same value. A preview row that
says `copy from X` requires the created Job's `watchkeeper_source` to say copy
from canonical X and its authority source digest to match the frozen bytes.
There is no dispatch-only source-policy branch.

## Source-mode matrix

| Start shape | No source flag in clean Git cwd | `--from DIR` | `--fresh` | Dirty input |
|---|---|---|---|---|
| Single | Existing Worktree behavior | Existing Copy behavior | Existing Fresh behavior | Existing explicit policy |
| Review / full-plan Graph | Worktree from current repo | **Copy into controller-owned approved source, then isolate children from that baseline** | Empty approved source, internally Git-initialized if the Graph conductor requires it | Explicit `--from`/approved snapshot includes untracked deliverables; never create children from an unfrozen dirty tree |
| Campaign / follow-up | Preserve current parent-artifact/shape rules | Refuse before providers unless the shape already has a defined source contract | Refuse before providers unless already supported | Preserve current typed refusal |

For Graph Copy:

1. canonicalize and validate the requested directory without writing it;
2. build the deliverable-file index with existing ignore/exclusion policy;
3. copy to a controller-owned staging directory below the pending Job root;
4. rebuild the index and require the same tree digest;
5. initialize an internal baseline commit only when existing Graph machinery
   needs Git; use DeadReckon's local identity and never touch user config;
6. atomically publish the approved source and bind it into `JobAuthority`;
7. set `Job.source_cwd` to the approved source used by `drive_job_command`;
8. retain original canonical path only as launch provenance (`DurableSource`),
   never as a mutable execution dependency.

If Job creation fails, the pending-directory guard removes staging. Once the
Job is queued, mutation or deletion of the original `--from` directory cannot
change execution or invalidate recovery. This strengthens Watchkeeper's
claim; it does not weaken source-digest validation.

## Bounded project dossier

Replace the file-name-only summary with a deterministic, redacted and capped
dossier built from `inspection_root`. It contains enough truth that a
structured-text model has no reason to browse or shell out:

- canonical project kind and default test/build signals;
- bounded file inventory excluding `.git`, build output, runtime state and
  transcript/history directories;
- manifest excerpts needed to name products and scripts (`Package.swift`
  products/targets, Cargo package/binaries, `package.json` scripts/workspaces,
  `pyproject.toml` project/test configuration, and equivalents already known
  to Polyglot detection);
- test/source entry-point names and existing acceptance helper inventory;
- existing acceptance YAML/Markdown when refining;
- explicit statement that the dossier is complete for authoring and external
  lookup is prohibited.

Apply per-file and total byte caps, stable ordering and redaction before the
prompt. Truncation is labelled. Never include `.env`, credential material,
provider transcripts, `.specstory/history`, or arbitrary source bodies. The
Swift reproduction dossier must expose product `Cloudwing`, target
`Cloudwing`, and `CloudwingTests`.

## Structured, time-bounded provider contract

### Output schemas

Set `ProviderRequest.output_schema` for both calls when supported.

Draft/redraft schema:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["acceptance_yaml", "acceptance_md", "files"],
  "properties": {
    "acceptance_yaml": { "type": "string" },
    "acceptance_md": { "type": "string" },
    "files": {
      "type": "object",
      "additionalProperties": { "type": "string" }
    }
  }
}
```

Critic schema uses the exact fields already represented by `CriticVerdict` and
an enum `pass|redraft`. The parser additionally normalizes `reject` to
`redraft` for provider compatibility; arrays must survive normalization.

### Structured-text-only posture

Done authoring is a completion task, not an open-ended coding session.
Introduce a request-scoped structured-text posture. For Codex CLI, use an
ephemeral, schema-constrained invocation and a probed isolated configuration
that disables web/browser/plugin/code-mode/shell tools for this request while
retaining authentication and the selected model. Do not hardcode feature flags
without capability tests. API adapters send no tools. An adapter that cannot
enforce the posture must report that limitation; it may not silently launch an
interactive tool-using agent for this path.

The router continues to prefer explicit authoring provider, then the existing
`doc_provider`, then the ordinary provider. No new provider registry or model
catalog is introduced. Preview/review names the actual authoring provider and
model separately from planner/implementors.

### Wall budget and cancellation

Add a request-scoped timeout/cancellation mechanism to the provider transport.
Default cumulative done-authoring wall budget:

| Stage | Maximum allocation | Cumulative rule |
|---|---:|---|
| Initial draft | 60 seconds | May use no more than half the configured total |
| Critic | 20 seconds | Skipped when deterministic lint already requires refusal and no useful draft exists |
| Redraft | remaining budget, at most 60 seconds | Never resets the cumulative clock |
| Whole authoring flow | 120 seconds | Includes process startup, capability probe and cleanup |

Expose one additive config key, clamped to `30..=600`:

```toml
[defaults]
done_contract_max_wall_seconds = 120
```

Timeout cancels the token, terminates the provider's complete owned process
group with the existing graceful-then-forceful discipline, removes PID/schema
temporary files, and waits for reaping before returning. `max_output_tokens`
is not a wall timeout and must never be described as one. Cache immutable CLI
capability probes per binary/version for the process lifetime so draft,
critic and redraft do not each spawn `exec --help`.

### Timeout result policy

- Initial draft timeout/invalid schema: write nothing; refuse with elapsed,
  limit, provider and one recovery command.
- Critic timeout: retain the deterministic lint floor. A lint-clean draft may
  be shown for explicit human acceptance with a caveat; a non-interactive
  strict launch refuses.
- Redraft timeout: retain the prior draft only if it is lint-clean and the
  operator explicitly accepts it. A stub-passable or strongly divergent draft
  is never written as approved.
- Cancellation/Ctrl-C: same cleanup, no partial files, status 130 semantics
  where the CLI already uses them.

The spinner renders stage, provider/model and `elapsed / limit`; it is an
observation, not the timeout mechanism.

## Redraft continuity

The one redraft prompt includes:

- original user request and run goal;
- prior `acceptance_yaml` and `acceptance_md`;
- bounded prior helper-file map or digests plus the specific file bodies that
  may be revised, within the same prompt cap;
- deterministic lint findings;
- full normalized critic verdict, including uncovered clauses and weak check
  indices;
- the same source dossier and output schema.

Do not make the redrafter rediscover schema or inspect SpecStory transcripts.
It edits a candidate; it does not start over. After redraft, compile and lint
locally. The existing `critic_floor_verdict` remains the final deterministic
floor; there is no second critic call.

## User-facing contract

The exact reproduced command must either:

1. preview `copy from <canonical source>`, show that done criteria were
   inspected from that source, accept, create a Graph Job using the same
   frozen source, print its Job ID and lifecycle actions; or
2. refuse before provider work with a specific incompatibility and one
   runnable `try:` line.

It must never show `will start`, generate/write/review a contract and then
reject a source flag at dispatch. A generated valid contract remains reusable
after unrelated later failures, keyed only by its files and normal project
discovery—not by hidden session state.

Preview adds bounded factual rows when authoring is needed:

```text
done inspect : /canonical/source
done writer  : /launch/project/.deadreckon
done provider: cli:codex / <model> (structured text)
done limit   : 120s total
workspace    : approved copy from /canonical/source
```

Keep compact card/plain layout conventions; JSON carries the same roots,
source mode and budget without terminal-only wording.

## Phases (eleven)

Every phase starts by writing the named depth tests and watching them fail.
Then implement, run focused tests, format/lint, make a conventional local
commit and add one CHANGELOG line naming the SHA. Run `make verify` at P3, P7,
P10 and P11 at minimum.

### P1 — Characterize the reproduced failure and ordering

- Build a hermetic fixture with an empty launch directory and a separate
  dirty/untracked Swift `--from` tree whose product is `Cloudwing`.
- Use scripted providers that count calls and can hang at named stages.
- Pin current preview/dispatch contradiction and done-before-source ordering
  before changing behavior.

Depth tests:

- `full_plan_from_preview_currently_disagrees_with_dispatch`
- `done_authoring_currently_inspects_launch_cwd_not_from_source`
- `unsupported_launch_input_must_not_reach_provider_or_write_contract`
- `soundings_swift_fixture_contains_tracked_modified_and_untracked_inputs`

### P2 — One resolved source before authoring

- Add `ResolvedStartSource` (or the equivalent exact responsibilities) to the
  ephemeral launch decision.
- Move deterministic source/mode resolution and validation ahead of done
  authoring and final confirmation.
- Make preview consume only the resolved value.

Depth tests:

- `start_resolves_source_once_before_done_authoring`
- `preview_and_dispatch_share_the_same_resolved_source`
- `invalid_source_refuses_before_provider_confirmation_or_write`
- `json_plain_and_card_project_identical_source_truth`

### P3 — Graph `--from` approved-source snapshot

- Thread the resolved `DurableSource` through
  `dispatch_advanced_start_job`; remove its unconditional source-flag refusal
  for review/full-plan.
- Freeze Copy inputs into controller-owned approved source, digest before and
  after, and give the Graph driver that path.
- Ensure dirty/untracked deliverables enter the baseline without modifying the
  original tree.

Depth tests:

- `full_plan_from_creates_graph_job_with_copy_source`
- `graph_copy_freezes_untracked_deliverables_before_queue`
- `graph_copy_never_modifies_the_operator_source`
- `graph_execution_survives_original_source_mutation_or_removal`
- `graph_authority_and_preview_bind_the_same_source_digest`

### P4 — Separate acceptance inspection and write roots

- Add `AcceptanceAuthoringContext` and remove ambiguous dual-purpose `cwd`
  from guided materialization.
- Detect existing project contract at the writer root, inspect source truth at
  the inspection root, and freeze the accepted writer artifact into the Job.
- Keep direct `def-done` behavior by passing one root twice.

Depth tests:

- `guided_from_writes_contract_to_launch_project_and_inspects_source`
- `direct_def_done_uses_project_as_both_roots`
- `generated_checks_never_embed_original_absolute_source_path`
- `accepted_contract_is_frozen_with_the_resolved_source_job`

### P5 — Bounded source dossier

- Implement the deterministic capped dossier and manifest extractors.
- Exclude histories, secrets, build output and runtime state.
- Pin stable ordering, caps, truncation labels and redaction.

Depth tests:

- `swift_dossier_names_cloudwing_product_target_and_tests`
- `dossier_excludes_specstory_history_env_and_build_output`
- `dossier_caps_each_file_and_total_bytes_deterministically`
- `dossier_truncation_is_explicit_not_silent`
- `dossier_is_identical_for_equivalent_directory_orderings`

### P6 — Schema-constrained structured-text authoring

- Attach exact draft and critic output schemas.
- Add/enforce the request-scoped structured-text-only posture for Codex CLI
  and already tool-free API routes; fail closed for an adapter that cannot
  honor it.
- Use ephemeral calls and the existing doc-provider preference; report the
  authoring route separately.

Depth tests:

- `acceptance_draft_request_carries_exact_output_schema`
- `critic_request_carries_pass_redraft_output_schema`
- `codex_done_authoring_is_ephemeral_and_disables_tool_surfaces`
- `structured_text_posture_never_silently_degrades_to_tools`
- `done_authoring_prefers_configured_doc_provider`

### P7 — Cumulative timeout, cancellation and tree reaping

- Add the clamped config/default and request-scoped deadline.
- Enforce the cumulative stage allocations around capability probe and provider
  execution, not around the spinner.
- Reap root and descendants; cache capability results by binary/version.

Depth tests:

- `never_returning_done_draft_stops_within_cumulative_budget`
- `never_returning_redraft_uses_remaining_budget_not_a_fresh_clock`
- `done_timeout_reaps_provider_and_grandchild_processes`
- `done_timeout_removes_schema_pid_and_partial_output_files`
- `codex_capabilities_are_probed_once_per_binary_version`

### P8 — Critic compatibility and redraft continuity

- Normalize `reject` to `redraft` without losing arrays.
- Include the full prior draft/files, dossier, lint and verdict in the redraft.
- Compile/lint once after redraft; retain the no-loop ceiling.

Depth tests:

- `critic_reject_alias_preserves_missing_clauses_and_weak_indices`
- `redraft_prompt_contains_prior_yaml_markdown_and_helpers`
- `redraft_prompt_contains_full_critic_and_same_source_dossier`
- `critic_and_redraft_still_run_at_most_once_each`
- `redraft_never_searches_transcripts_for_its_predecessor`

### P9 — Honest timeout/refusal/reuse surfaces

- Implement the result policy and canonical `try:` footers.
- Render provider/model, stage and cumulative elapsed/limit.
- Ensure a valid written contract is discovered on retry and deterministic
  late checks cannot force recompilation.

Depth tests:

- `initial_draft_timeout_writes_nothing_and_prints_one_try`
- `critic_timeout_allows_only_explicit_acceptance_of_lint_clean_draft`
- `redraft_timeout_never_approves_stub_passable_prior_draft`
- `retry_reuses_valid_generated_contract_without_provider_call`
- `wait_surface_shows_stage_provider_and_cumulative_limit`

### P10 — End-to-end command and latency proof

- Run the exact reproduction against hermetic scripted providers and the
  Swift fixture through preview, review, Job creation and Graph driver source
  preparation.
- Add deterministic latency artifacts for immediate, critic-redraft and hung
  provider cases. Do not call a live provider in CI.
- Re-run Single/Graph source matrices and Watchkeeper authority/receipt tests.

Depth tests:

- `flappy_reproduction_returns_graph_job_id_after_accepting_done_contract`
- `flappy_contract_uses_cloudwing_and_resolved_source_facts`
- `full_plan_from_preview_contract_authority_and_driver_agree`
- `done_authoring_latency_matrix_enforces_120_second_default`
- `single_and_clean_graph_launches_keep_existing_behavior`

### P11 — AS-BUILT §59, MAP, CHANGELOG and operator handoff

- Add `## 59. Soundings: Source-True, Bounded Launch Admission` to
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`, documenting the
  resolved-source pipeline, approved Graph copy, authoring dossier, structured
  provider posture, deadline/fallback policy and compatibility boundaries.
- Update §17.1, §46, §48 and §58 where their old ordering/source claims are
  contradicted; do not leave the current claim that launch resolution precedes
  provider work if tests do not enforce it.
- Update `/Users/gdc/deadreckon/docs/MAP-OF-DEADRECKON.md` Guided Start,
  Contract and durable Graph rows with the observed boundary and proof.
- Add `## Soundings (stable)` to CHANGELOG with phase commit span and measured
  before/after authoring latency.
- Write a plain-language operator acceptance checklist covering the exact
  Flappy continuation command, a hanging provider and retry reuse.

## Integration matrix

| Surface | Source used for preview | Source used for dossier | Source bound by authority | Provider deadline |
|---|---|---|---|---|
| Single, clean current repo | resolved current worktree | same | same | 120s cumulative when authoring |
| Single `--from` | canonical copy source | copy source | approved copy | same |
| Review/full-plan, clean current repo | resolved worktree | same | same | same |
| Review/full-plan `--from` | canonical source + approved-copy label | original before freeze; frozen copy revalidated | controller-owned approved copy | same |
| Direct `def-done` | project | project | not applicable until launch | same |
| Existing contract | resolved source still shown | no provider dossier needed | frozen accepted contract + source | zero provider calls |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| Source missing/unreadable before authoring | `deadreckon start "<goal>" --mode full-plan --from <existing-dir>` |
| Unsupported source flag for selected shape | command omitting the exact unsupported flag, printed before provider work |
| Draft exceeded cumulative budget | `deadreckon def-done "<concise behavioral outcome>"` |
| Adapter cannot enforce structured-text authoring | `deadreckon config provider <compatible-doc-provider>` |
| Critic/redraft timed out with weak candidate | `deadreckon def-done "builds, runs, persists settings, and passes behavioral tests"` |
| Generated contract refers to absent evidence | `deadreckon def-done check` after placing or removing the reviewed evidence requirement |
| Graph approved-source freeze digest mismatch | rerun the same `start` after source writes stop; never suggest `--allow-dirty` as a digest bypass |

Each error is parameterized by a depth test. Error output has one primary
recovery, at most two secondary actions, and no command that loops back to the
same late failure.

## Config addition

```toml
[defaults]
done_contract_max_wall_seconds = 120 # clamped 30..=600; cumulative draft+critic+redraft
```

Existing `doc_provider` remains the authoring-route knob. Do not add
`done_provider`, per-stage timeout keys or a provider-specific web flag to the
public config in this slice.

## Out of scope (explicitly not in this milestone)

- More than one critic or one redraft.
- New acceptance check kinds, semantic embeddings or browser drivers.
- Model-generated project scaffolding outside `.deadreckon/acceptance/`.
- Campaign `--from` semantics, follow-up source replacement, cross-machine
  source upload or remote source URLs.
- General provider-agent tool policy redesign beyond the request-scoped
  structured-text posture needed here.
- Persisted authoring sessions, authoring transcript migration or cloud cache.
- Automatic screenshot capture/visual truth classification; screenshots stay
  explicit reviewed evidence plus deterministic image checks.
- Live-provider performance claims in CI. Operator dogfood may record them
  separately after hermetic correctness passes.

## Dependencies

Tier 1 (existing, free): Tokio cancellation/time, existing process-group
termination, `serde_json`, provider output schema, Polyglot project detection,
deliverable-tree indexing/copy, Watchkeeper pending Job directories and
authority digests.

Tier 2: none expected. If process-tree reaping cannot reuse current Capstan /
Watchkeeper primitives, stop and record the architectural gap in
`docs/V1-CANDIDATES.md` and `DEPENDENCIES.md` before adding a crate.

Tier 3 (blocked): workflow engines, background authoring daemons, remote
source stores, a bundled browser, or a second provider SDK solely for done
criteria.

## Engineering invariants (do not violate)

- Source compatibility is decided before any provider, write, review or final
  confirmation.
- Preview, dossier, authority and dispatch consume one source resolution.
- A Graph `--from` run executes from controller-owned approved bytes, not a
  mutable external directory.
- Copy includes untracked deliverables and excludes Git/runtime/build noise by
  the same canonical policy used for authority hashing.
- Acceptance writer root and inspection root are explicit at every call.
- No generated acceptance command embeds the operator's original absolute
  source path.
- Deterministic dossier first; model cannot be asked to discover the project.
- Structured-text calls carry schemas and cannot silently gain tool access.
- One cumulative authoring deadline; redraft never receives a fresh budget.
- Cancellation reaps descendants before returning.
- Timeout never writes or approves a weak/partial contract.
- `reject` and `redraft` mean the same critic decision; feedback arrays survive.
- One critic, one redraft, no loops.
- Existing durable authority, gate and receipt validation remain the final
  execution truth.
- No live provider/service mutation, push or release by the implementation
  agent.

## Process invariants

- Phased local commits only; never stage unrelated `.specstory`, map or source
  changes already present in the worktree.
- A phase whose named depth tests were never observed red is incomplete.
- Focused tests after each edit; `make verify` at the named milestones and at
  closure.
- Provider tests use scripted binaries with deterministic clocks/processes;
  no network or paid model is required.
- Source tests compare deliverable indexes/digests, not mtimes.
- Performance evidence records stage and cumulative elapsed values from an
  injected clock where possible; avoid flaky real-time upper bounds except a
  generous process-reaping integration test.
- Each phase ends with one CHANGELOG line naming its local SHA. P11 replaces
  incremental notes with a coherent stable section without erasing evidence.
- At each buildable milestone, hand the operator a plain-language acceptance
  script before claiming completion.
- Anything beyond P1–P11 is logged in `docs/V1-CANDIDATES.md`; do not silently
  expand Soundings into a general provider or orchestration rewrite.
