# Prime Agent and DeadReckon: a capability map

A concept-by-concept comparison of [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)
and DeadReckon as each stands today, and a judgement about which of Prime Agent's
ideas would make DeadReckon meaningfully more powerful.

The organising question is the one Birgitta Böckeler poses in
[Harness engineering for coding agent users](https://martinfowler.com/articles/harness-engineering.html#FeedforwardAndFeedback):
a harness needs **guides** that steer the agent before it acts and **sensors**
that observe after it acts, and it needs both, because

> Separate, you get either an agent that keeps repeating the same mistakes
> (feedback-only) or an agent that encodes rules but never finds out whether they
> worked (feed-forward-only).

That framing is what makes `learn` the centre of this document. DeadReckon's
existing comparison against the same article is
[`HARNESS-ENGINEERING-COMPARISON.md`](HARNESS-ENGINEERING-COMPARISON.md); this
document is the sibling that compares against a real competing implementation
rather than against an article.

**How this was produced.** Ten readers went over both source trees in parallel,
one mapper aligned their findings, and three adversarial verifiers then attacked
the result: one against Prime Agent's source, one against DeadReckon's, and one
against the judgement itself. The verifiers overturned three of the five original
recommendations, and every overturned claim in this document was re-checked by
hand against the file and line cited. [What the review threw out](#what-the-review-threw-out)
records those reversals, because the discarded reasoning is itself useful.

**Evidence boundary.** Claims about code are from source read at commit `f6913b2`.
The one observation about an empty learning store is from this machine's
`~/.deadreckon` only, and is stated as such. Nothing here is a live benchmark;
neither product was run head to head.

---

## The short answer

DeadReckon is ahead of Prime Agent on nearly everything that touches trust:
verification, receipts, confinement, budget enforcement, who is allowed to
declare work done, and — contrary to first impressions — how often correctness
checks actually run. Those are not close calls, and several are cases where
Prime Agent has an explicit non-goal where DeadReckon has an implementation.

Prime Agent is ahead on exactly one thing that matters strategically, and it is
the thing you asked about. Its harness **learns from a finished trajectory and
that learning steers subsequent work**. DeadReckon's `learn` performs the harder
half of that job well — deterministic indexing, redaction, citation-enforced
proposals — and then stops. `crates/deadreckon-runtime/`, the crate that builds
every prompt and drives every turn, contains **zero occurrences of the string
`learning`**. Nothing DeadReckon learns has ever steered a DeadReckon run.

In Böckeler's terms DeadReckon built a good sensor and wired its output to
nothing. It is the "finds out but never encodes" failure, which is the mirror
image of the one she warns about most often.

---

## The chart

Verdicts: **DR ahead** · **Parity** · **Gap** (worth closing) · **Deliberate**
(DeadReckon refuses this on purpose) · **N/A** (architecturally inapplicable).

| # | Capability | Prime Agent | DeadReckon | Verdict |
|---|---|---|---|---|
| 1 | **Learning from finished work** | `/refine` reflects on the trajectory, writes typed entries (prompt / memory / skill / subagent) to a store, rebuilds the live system prompt. Loop closes. | `learn index` → `learn propose` writes cited proposals to JSON. Nothing reads them back. Loop is open. | **Gap** — the one that matters |
| 2 | **Cross-run memory** | `memory` entries, session-local by default, global tier on request; injected into every prompt as a capped menu. | Per-project library at `~/.deadreckon/library/<scope>/` holds as-built, decisions, narrative, provenance. Never re-enters a prompt. | **Gap** (design-sensitive) |
| 3 | **When correctness checks run** | Operator shell gates run at the **completion boundary**, when the agent would otherwise stop. Unsandboxed. | On CLI routes the **entire frozen contract runs after every turn**, keyless and inside the sandbox. On direct-API routes, only at `done`. | **DR ahead** (primary path) |
| 4 | **Detecting a futile retry** | Git worktree snapshot; if byte-identical to the last failure, skip the rerun and tell the model. Run stays alive. | `classify_cli_no_deliverable_changes` terminates the run and **preserves the real gate failure** as the cause. | **DR ahead** |
| 5 | **Context management** | LLM compaction at a token threshold, plus notes on what survived in the Python kernel. | Deterministic compaction, no model call, legible in-prompt marker, durable `compaction.jsonl` record. **Direct-API routes only.** | **DR ahead**; CLI routes uncovered |
| 6 | **Who declares the goal met** | The model calls `await goal.complete()`. That is the only path to complete. | Deterministic gate + fresh read-only semantic judge. The agent structurally cannot. | **DR ahead** (defining) |
| 7 | **Proof that work is done** | Process exit code, optionally shell gates. Nothing signed. | HMAC-SHA-256 receipt binding ~15 digests, revalidated at finish, undo and promotion. | **DR ahead** (defining) |
| 8 | **Confining execution** | None. Full `process.env`, no uid drop, no namespaces, no seccomp — stated as a non-goal. | Three backends, control-plane denylist, boundary probe proving from inside the sandbox that the key is unreadable. | **DR ahead** (opposite postures) |
| 9 | **Budget enforcement** | Four limits (3 continuations / 12 turns / 80k tokens / 30 min), in process memory. | Frozen into signed `JobPolicy`, hashed, enforced twice — child pauses itself, supervisor independently terminates. | **DR ahead** |
| 10 | **Telling the agent its budget** | Gate feedback includes `attempt N/max`. | Revise feedback is a fixed string. The agent never learns `max_attempts` or `max_turns` exist. | **Gap** — cheapest fix here |
| 11 | **Surviving terminal and reboot** | Detached worker + supervisor, retries, recovery journal. No OS service, so nothing survives a reboot. | Same, plus launchd/systemd user service, fenced renewable lease, generation-numbered checkpoint. `start` refuses without it. | **DR ahead** |
| 12 | **Agent splitting its own work** | `await rlm(...)` spawns a real child, admission is depth-check only. | Agent may only write an **inert** `reshape-proposal.json`. Fan-out is operator-initiated, and the parent verifies itself. | **DR ahead** |
| 13 | **Steering a running agent** | Any family member messages any other at a turn boundary; can wake a saved session into a process to receive it. | Durable steer inbox exists in core. **One of eleven routes drains it** (`cli:codex-server`). Others refuse honestly. | **Gap** (deferred, see below) |
| 14 | **Read-only observation for the agent** | `agent-observe` skill: list, inspect, read messages. Explicitly cannot mutate. | `attach --json`, `--why`, `verdict` — all operator-facing only. Nothing equivalent for a plan parent. | **Gap** (small) |
| 15 | **Scheduling and recurring work** | `/heartbeat`, model-owned `rlm_heartbeat`, `prime-agent schedule` with cron. | None. A Job is admitted, runs to a terminal outcome, stops. | **Deliberate** |
| 16 | **Provider and model catalog** | 9 wire APIs; 1162 models across 31 providers, **generated**, with per-token pricing. `!shell-command` credentials. | 11 embedded descriptors; `providers.d/*.toml` operator overrides ship. Catalogs hand-maintained. | **Parity**; staleness risk (see #16 below) |
| 17 | **Programmatic control surface** | `--mode json`, a ~45-command bidirectional RPC, and ACP for editors. | `--json` on ~30 verbs. `watch` already aliases `attach` and takes `--json`, but it is one-shot, not a stream. | **Gap** (narrow) |
| 18 | **Cost observability** | Per-agent own-vs-total tree; child spend folded into the spawning message. | `spend.jsonl` per run with running total; evidence-cited spine; every claim carries a file path. | **Gap** (small) |
| 19 | **Reusable instruction files** | Five-tier discovery, progressive disclosure, first-wins collisions reported. | Run skill resolves two tiers, deliberately excluding the worked-on repo. Doc skills resolve three. | **Deliberate** (see below) |
| 20 | **Executable extensions** | Skills are pip-installed Python packages; installing one runs its build backend, no prompt, no sandbox. | Four fixed subprocess seams, conformance-validated, with the explicit rule that the gate is not a seam. | **Deliberate** |
| 21 | **Editing the harness's own source** | Explicitly forbidden. `validateEdit` hard-rejects base-prompt edits. | `improve self` does it: isolated worktree, dirty-tree refusal, seven-component evidence score, eight-reason PR gate. | **DR-only** (with real bugs) |
| 22 | **The actuator surface** | One tool, `ipython`, taking a string of Python. Everything is code in a persistent kernel. | On CLI routes, no tool surface at all — the vendor CLI owns its own tools. | **N/A** |

---

## The one that matters: `learn` against the Continual Harness

You asked specifically how `learn` maps to the feedback-loop concept. Here is the
exact comparison, because the difference is narrower and sharper than "one has a
feature the other lacks".

### What each side actually does

|  | Prime Agent `/refine` | DeadReckon `learn` |
|---|---|---|
| Trigger | Operator slash command, **plus auto-refine on by default** | Operator types `learn index`, then `learn propose`. Only ever manual. |
| Input | The session trajectory | Redacted run evidence: episodes, seven rule-derived signal kinds |
| Discipline on the write | Structural validation; base prompt immutable | **Citation-enforced**: refuses any insight whose stimulus does not cite a locally-known `signal_id` matching its `run_id` |
| Output lands in | A typed store the prompt builder reads | A JSON proposal file |
| Effect on later work | System prompt rebuilt in place — **steers the next turn** | **None.** Requires a human to read the proposal and act |
| Was it any good? | Records an `expectedOutcome`; never measures it | Never measured either |

DeadReckon is *better* at the part that is hard to get right. The citation
enforcement in `crates/deadreckon-core/src/learning.rs` is a real discipline that
Prime Agent has no equivalent of: a proposal that cannot point at the signal that
motivated it does not get written. Prime Agent's refinement is a model asserting
that a model's self-report is worth persisting, checked only by a second model.

DeadReckon is missing the last inch. And the consequence is visible: on this
machine there are 22 runs in runstate, **2 indexed episodes**, and no
`signals.jsonl`, no `insights.jsonl`, no `proposals/`, no `candidates/` at all.
Because indexing only happens when the operator remembers to type it, and nothing
ever prompts them to, the loop has never run end to end even once.

### Where the loops close

```
Prime Agent          trajectory ──▶ /refine ──▶ harness_state.json ──▶ system prompt ──▶ next turn
                                                                            └──── closed ────┘

DeadReckon           run evidence ──▶ learn index ──▶ signals ──▶ learn propose ──▶ proposal.json
                                                                                        ╳
                                                                                   (human reads it)
```

### The honest complication

Before treating this as a build task, note that DeadReckon's own architecture map
already looked at the same evidence and drew the opposite conclusion.
[`MAP-OF-DEADRECKON.md:201`](MAP-OF-DEADRECKON.md) grades learning and
self-improve as *"**Experimental**; weak connection to the original
highest-priority needs"* and recommends it as a *"**Strong
product-decision/deprecation candidate** unless usage and proposal quality
justify it"*, and line 220 states it is not part of the primary product promise.

The empty store is the same fact read two ways. It is either missing wiring or a
feature nobody wants. **This is a keep-or-cut decision before it is a build
decision.** What the comparison with Prime Agent adds is evidence that a serious
competitor considers this concept important enough to be one of its two
headline abstractions — which is a reason to test the keep hypothesis cheaply
rather than to commit to the full build.

### If you keep it, the cheap first move

Append a `LearnIndexed` step to the Job terminal path in
`crates/deadreckon/src/commands/supervisor.rs`, so `index_learning` runs
automatically when a Job reaches a terminal outcome. Keep it deterministic and
best-effort, so an indexing failure can never change a Job's outcome.

That is small, it changes no trust surface, and within a few weeks of ordinary
use it produces the usage-and-quality evidence the deprecation question is
waiting on. Everything else should wait for that evidence.

### If the evidence justifies going further, what adoption really costs

The obvious design — materialise an accepted proposal as guidance at
`~/.deadreckon/skills/<name>/SKILL.md`, which `resolve_skill_path` already
prefers — needs **three** pieces of new trust machinery, not one:

1. **A denylist entry.** `~/.deadreckon/skills/` is not currently protected.
   `ProtectedPathPolicy::for_paths` covers gate keys, the jobs dir, per-run
   acceptance and proofs, and resolved `.git` metadata. Skills appear nowhere. An
   agent could rewrite its own steering between turns.
2. **A per-turn digest check, not an admission-time one.** `run_skill_text`
   re-reads the skill from disk on *every* prompt build. Hashing it into
   `JobAuthority` at admission would prove only what the file said before turn
   one. Either freeze a copy under the run root or re-verify per turn.
3. **The `JobAuthority` and receipt binding**, so a receipt can never be produced
   under guidance nobody approved.

Note also that [`V1-CANDIDATES.md:39`](V1-CANDIDATES.md) already defers "make
repository/team rules a signed acceptance input and name their evidence in the
combined receipt" — the same digest-bound-guidance-input design, reached
independently. This is not an unconsidered idea; it is a deferred one.

And the outcome measurement that would make this a real closed loop (re-derive
whether the targeted signal kind recurred) is statistically meaningless at 22
lifetime runs. It belongs in the design, not in the first release.

---

## What is worth taking, ranked

### 1. Tell the agent how much budget it has left — *smallest change, immediate value*

Prime Agent's gate feedback carries `attempt N/max`. DeadReckon's revise feedback
is a fixed string:

```
acceptance failed after turn {turn}: {reason}. Continue by fixing the failing
done criteria; do not declare done until dr-gate passes.
```

The agent is never told that `max_attempts` or `max_turns` exist, let alone how
much of either remains. An agent that knows it is on its last bounded attempt can
consolidate and leave the tree reviewable instead of starting a risky refactor.

This is one more interpolated field in an existing string at
`crates/deadreckon-runtime/src/turn_loop.rs:5824`. No new input class, no new
signing surface, no change to what "done" means. DeadReckon enforces its budgets
perfectly and tells the agent nothing about them.

### 2. Auto-index at Job terminal — *unblocks the keep-or-cut decision*

As above. Small, deterministic, generates the evidence the deprecation question
needs.

### 3. Generate the model and pricing catalog rather than hand-maintaining it

Prime Agent generates a catalog of 1162 models with per-token pricing. DeadReckon
hand-maintains catalogs inside eleven TOML descriptors — and this is not
cosmetic, because **DeadReckon meters in dollars**. `max_spend_usd` is frozen
into `JobPolicy` and hashed as `effective_policy_sha256`. Stale pricing silently
degrades the budget guarantee the whole authority chain exists to protect, and
does so invisibly, because the receipt binds the policy digest, not the pricing
that priced it.

Build-time generation from a maintained upstream, plus a `doctor` check that
flags descriptors whose pricing has not been refreshed. (The runtime-override
version of this idea is already shipped as `providers.d/*.toml`, and an override
is a per-operator patch that does not fix staleness at the source.)

### 4. Extend compaction to CLI routes — *if the context-window question can be answered*

`compaction.rs` is deterministic, default-on, operator-tunable and leaves a
durable record. It is guarded by `is_direct_api_provider_kind`, and a test
(`cli_provider_path_is_never_compacted`) locks the exclusion in.

The exclusion is defensible — on a CLI route the vendor CLI owns its own inner
context. The real obstacle is that the turn loop never calls
`context_window_for_route_with_source` for CLI routes at all, so there is no
number to compare against. Extending compaction needs a decision about whose
context window counts: the vendor CLI's, or the descriptor's. That design
question is the actual work, not the windowing function.

### 5. Fix the `improve self` bugs — *but do not generalise it*

Four real defects in `crates/deadreckon/src/commands/learning.rs`:

- `load_self_improve_proposal` mints a fresh `prop-<uuid>` per invocation (`:946`),
  so a later `--pr-dry-run` can never match a candidate an earlier `--yes` created.
- `blocked_auto_pr_reasons` is persisted (`:879`) and read by nothing — the
  model's own self-reported concerns are silently discarded.
- `is_high_risk_path` (`:1729`) covers gate, sandbox, release and anything
  containing "credential" or "config", but **not `learning.rs` itself** — the file
  that owns redaction is not high-risk under its own rules.
- The `--yes` reset/restore/stage/commit sequence (`:744-766`) has no end-to-end
  test. The only test using `--yes` asserts a refusal.

The tempting fix — derive the candidate's acceptance contract from the proposal's
own `done_criteria` — is backwards for this product. It means the model that
authored the change also authors the test that certifies it, and `evaluate_auto_pr`
opens a PR at ≥0.85 with no operator in the loop. That is `goal.complete()` with
more steps, which is precisely what DeadReckon exists to refuse. If the bar must
widen, widen it to `default_checks_for(detect_project_kind(...))` — derived from
the repository, deterministically, not from the model.

### 6. Streaming `attach --json`

`watch` already exists as a visible alias of `attach` and already takes `--json`,
which prints one pretty JSON projection and exits. The gap is a follow mode that
emits newline-delimited lifecycle rows and terminates on the sealed receipt. The
incremental tailer (`AttachJsonlTail`) already exists but is reachable only from
the TUI path. Smaller than it first appeared, and it adds no mutation path.

### 7. Two small borrows

- **`!shell-command` credentials.** Let operators source keys from 1Password or
  the keychain instead of storing them. Fits the local-first, secret-averse
  posture.
- **Own-vs-total spend rollup** for plan children and campaign subs, so an
  operator can see which child consumed the budget. The data is already on disk
  in the per-run ledgers plus plan lineage; nothing new needs instrumenting.

---

## What DeadReckon should refuse

These are Prime Agent capabilities that would damage DeadReckon if adopted.

**Default-on automatic refinement.** Prime Agent runs refinement every 25
assistant turns (subject to a 20-minute cooldown, root sessions only), gated only
by a second model deciding whether the trajectory contains anything worth
persisting, and applies the result to the live system prompt with no human in the
loop. DeadReckon refuses the agent's own completion claim; it cannot coherently
accept the agent's own *steering* claim. Any adoption must be an explicit
operator verb with a preview.

*Worth noting:* this auto-refine writes the **session-local** store only. It
steers the remainder of that session, not future ones. Prime Agent's own
documentation does not describe it at all — it appears only in the CHANGELOG.

**Agent-callable refinement.** `refine.run()` and `rlm.harness.create_memory(...)`
let the model write its own durable steering state from inside its own kernel,
tagged only `source: "agent"`. That is the same class of move as an agent signing
its own acceptance marker. DeadReckon's `reshape` design already establishes the
right answer for this shape: the agent may propose, the proposal is recorded
inert, and only an operator command makes it real.

**A project tier for the run skill.** This looked like a small consistency fix —
doc skills resolve three tiers including the worked-on repo, the run skill
resolves two. It is not drift, it is a blast-radius boundary. The run skill is
re-read from disk on every prompt build, and in `CodebaseMode::InPlace` the
working directory *is* the source path, so a project tier would let the agent
rewrite its own standing instructions between turns. Same hole as agent-callable
refinement, through a different door.

**Skills as installed Python packages.** Installing one runs its build backend
and pulls its PyPI dependencies with no confirmation, inside a harness with no
sandbox. Where DeadReckon needs custom verification logic it already has the
`shell` acceptance check kind, which runs inside the sandbox under the approved
contract.

**Recurring agent instructions.** Perpetual agents are the opposite of
DeadReckon's bounded-run identity.

**Trace upload to a vendor endpoint.** DeadReckon's `learn export` already does
the useful part better and locally: deep string redaction, per-section SHA-256
hashes, and an `import-bundle` that refuses any bundle whose section hashes do
not verify.

---

## What the review threw out

Recorded because the reversals are more instructive than the survivors, and
because three of them were confident, well-cited, and wrong.

| Original recommendation | Why it died |
|---|---|
| "Move the sensors left — checks only fire when the agent claims `done`" | **Backwards.** `turn_loop.rs:1178` sits inside the `is_cli_subagent` branch: on CLI routes, the primary path, the full contract already runs after **every** turn, keyless and sandboxed. Only `:1949` is a `done` boundary, and it is direct-API only. The proposal was to add an *unsigned advisory* check tier — strictly weaker than the signed one already running. |
| "Prime Agent runs its gates after every assistant turn" | **False**, and it was the premise for the above. `nextAutonomousContinuation` is reached only from the `getContinuationMessages` hook, fired after the inner tool loop drains — the comment in `agent-loop.ts` reads "Agent would stop here." Both products sense at the completion boundary. |
| "Bound the prompt history — no context management at all" | **Already shipped.** `compaction.rs` is 293 lines of deterministic, default-on, context-window-aware compaction with a durable ledger. The recommendation's own risk note ("do NOT use a model call to compact") was describing the existing implementation without recognising it. |
| "A long run's prompt grows until the context window or spend cap ends it" | **Structurally impossible.** The loop is `for _ in 0..max_turns` with `max_turns: 12` at every production call site, and a completed bash turn pushes the literal `"tool {id} result: status={code}"` — tool output never enters history. Worst case is about a dozen short lines. |
| "No way to notice a futile gate retry — port Prime Agent's snapshot guard" | **Already present and stronger.** `classify_cli_no_deliverable_changes` terminates the run and preserves the real gate failure as the cause, where Prime Agent's guard leaves the run alive and burning budget. |
| "Add a `models.toml` operator override" | **Already ships** as `providers.d/*.toml` with merge-over-builtin semantics. |
| "Add `deadreckon watch --json`" | The verb is taken — `watch` is already an alias of `attach` and already takes `--json`. |
| "Give the run skill a project tier" | Net-negative for the trust model. See above. |
| "`verdict --watch` as a continuous drift sensor" | The self-criticism it cited is about deployed SLOs and dependency drift, not about re-proving a frozen tree against itself. `verdict --all` also already exists, and `verdict` self-labels as non-authoritative — scheduling an unattended non-authoritative signal manufactures alarm fatigue in a product whose pitch is that its signals are trustworthy. |
| "Drain the steer inbox at the turn boundary for all routes" (rated high impact) | Downgraded, not deleted. [`V1-CANDIDATES.md:315`](V1-CANDIDATES.md) already defers this pending "a steer-and-acknowledge wire contract with the same no-drop guarantees", and [`MAP-OF-DEADRECKON.md:200`](MAP-OF-DEADRECKON.md) says "validate before broad surface investment." Mechanically it also buys less than claimed: on a CLI route one DeadReckon turn is one entire vendor-CLI invocation, so a turn-boundary steer arrives after the agent has already done whatever it was going to do. |

Two further notes for whoever picks up the futile-retry work: Prime Agent's
snapshot guard only functions because `captureGitWorktreeSnapshot` excludes
`target/`, `Cargo.lock` and similar paths — a naive DeadReckon port would never
see two identical snapshots, because a `cargo test` gate rewrites `target/` on
every attempt. And DeadReckon's existing guard fires on "no deliverable change at
all" rather than on a tree hash, so an agent thrashing a file back and forth still
consumes attempts.

---

## The pattern underneath

Where Prime Agent reaches for a model, DeadReckon reaches for a deterministic
function plus a durable record.

Prime Agent compacts context with an LLM pass; DeadReckon compacts deterministically
and writes a `CompactionRecord`. Prime Agent refines its harness with a model
reviewing a model; DeadReckon requires a proposal to cite a signal ID that
provably exists. Prime Agent's agent declares its own goal met; DeadReckon's
cannot, and the refusal is signed.

That pattern is the product, and it holds on every row of the chart where
DeadReckon is ahead. It is also why the learning gap is worth taking seriously
rather than copying: the reason DeadReckon has not closed its loop is the same
reason its loop would be worth trusting once closed. The last inch is genuinely
harder for DeadReckon than it was for Prime Agent, because DeadReckon has to bind
what it learned into an authority chain that Prime Agent does not have.
