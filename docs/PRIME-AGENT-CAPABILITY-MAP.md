# Prime Agent, bb and DeadReckon: a capability map

A concept-by-concept comparison of DeadReckon against two other harnesses as
each stands today, and a judgement about which of their ideas would make
DeadReckon meaningfully more powerful.

- **[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)** — Prime
  Intellect's terminal coding agent. Owns the model loop; its two headline
  abstractions are a Recursive Language Model and a Continual Harness.
- **[bb](https://github.com/get-bb/bb)** — a self-hosted agentic IDE and
  orchestration server. Contains no agent of its own; it drives the provider
  CLIs you already have, from a desktop app, web app, CLI and HTTP API.

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
document is the sibling that compares against real competing implementations
rather than against an article. A standalone Prime Agent to bb comparison, with
no DeadReckon framing at all, is in
[`PRIME-AGENT-VS-BB.md`](PRIME-AGENT-VS-BB.md).

**How this was produced.** Two passes. The first put ten readers over the Prime
Agent and DeadReckon source trees, one mapper over their findings, and three
adversarial verifiers over the result. The second did the same for bb: six
readers, two mappers, three verifiers. In both passes the verifiers overturned a
large fraction of the recommendations — three of five in the first, four of seven
plus three of five revisions in the second — and every overturned claim was
re-checked by hand against the file and line cited.
[What the review threw out](#what-the-review-threw-out) records the reversals,
because the discarded reasoning is itself useful.

**Evidence boundary.** Claims about DeadReckon are from source read at commit
`f6913b2`. Nothing was executed: no server was started and no agent was run, so
every absence claim is a grep-and-trace result. Those are strongest where
structure forbids the thing outright and weakest where a feature can hide under
vocabulary nobody searched — a failure that happened twice in the bb pass and is
recorded below. The one observation about an empty learning store is from this
machine's `~/.deadreckon` only. Nothing here is a benchmark; no product was run
head to head.

---

## The short answer

DeadReckon is ahead of both on nearly everything that touches trust:
verification, receipts, confinement, budget enforcement, who is allowed to
declare work done, and — contrary to first impressions — how often correctness
checks actually run. Several are cases where the other products have an explicit
non-goal where DeadReckon has an implementation. bb in particular owns no
judgement whatsoever: no gate, no receipt, no spend cap, no OS sandbox, and no
authentication on `/api/v1`.

**Prime Agent is ahead on one thing that matters strategically**, and it is the
thing you asked about. Its harness learns from a finished trajectory and that
learning steers subsequent work. DeadReckon's `learn` does the harder half well —
deterministic indexing, redaction, citation-enforced proposals — and then stops.
`crates/deadreckon-runtime/`, the crate that builds every prompt and drives every
turn, contains **zero occurrences of the string `learning`**. Nothing DeadReckon
learns has ever steered a DeadReckon run.

**bb sharpens that finding by splitting the same loop the other way.** bb has no
reflection at all — its memory plugin's README says background reflection was
deliberately deferred — but it built the read-back half completely: a memory
catalog auto-appended to every system prompt on every turn. So of two serious
competitors, one invested in reflection and *both* invested in read-back.
DeadReckon is the only one of the three where nothing durable ever re-enters a
prompt. That reorders the framing rather than reversing it: auto-indexing at Job
terminal is still the right cheap first move, but it should be described as
instrumentation, not as progress toward closing the loop.

**Where bb is genuinely ahead of DeadReckon** it is on operator ergonomics for
long-running work, and DeadReckon should read those rows closely: work gets its
own provisioned workspace before turn one, work can run on a machine other than
the one you launched from, and a blocked agent can ask a human a durable
question. Nearly every place bb is behind is a place it never claimed to be.

---

## The chart

The **Ahead** column names which product leads on that row. It is not a score:
several rows are design choices where "ahead" is the wrong frame, marked `—`.

| # | Capability | Prime Agent | DeadReckon | bb | Ahead |
|---|---|---|---|---|---|
| 1 | **Learning from finished work** | `/refine` reflects on the trajectory, writes typed entries, rebuilds the live system prompt. Loop closes. | `learn index` → `propose` writes cited proposals. Nothing reads them back. | No reflection into a store; deferred in the memory README. The `workflows` plugin does read a finished child's text as a step result. | **PA** |
| 2 | **Cross-run memory** | `memory` entries, session-local or global. No project scope. | Per-project library artifacts. Never re-enter a prompt. | Memory plugin: typed SQLite records whose summary catalog is auto-appended to every system prompt, capped at 3,900 chars. | **bb** |
| 3 | **When correctness checks run** | Operator shell gates at the completion boundary. Unsandboxed. | Full frozen contract after **every turn** on CLI routes, keyless and sandboxed. `done` only on API routes. | Never. No gate exists; output is a direct SQLite read and plugin handlers cannot veto. | **DR** |
| 4 | **Detecting a futile retry** | Worktree snapshot; skip the rerun if byte-identical. Run stays alive. | Terminates the run and **preserves the real gate failure** as the cause. | No gate loop to be futile. `workflows` sends up to 2 corrective turns on invalid structured output. | **DR** |
| 5 | **Context management** | LLM compaction at a token threshold, plus kernel-state notes. | Deterministic compaction, no model call, durable `compaction.jsonl`. Direct-API routes only. | Delegates to the vendor CLI. Renders an occupancy ring; warns at 75%, destructive at 90%. | **DR** |
| 6 | **Who declares the goal met** | The model calls `goal.complete()`. | Deterministic gate + fresh read-only semantic judge. The agent structurally cannot. | The provider does: a completed turn is idle. A delegated worker is told to set its own task status. | **DR** |
| 7 | **Proof that work is done** | Process exit code. Nothing signed. | HMAC-SHA-256 receipt binding ~15 digests, revalidated at finish, undo and promotion. | A status row and an event log. Disposition is `POST /environments/:id/actions` — commit, squash-merge, PR merge — unauthenticated. | **DR** |
| 8 | **Confining execution** | None. Full env, no uid drop, no namespaces — a stated non-goal. | Three backends, control-plane denylist, boundary probe signed into the receipt. | No OS sandbox. Passes vendor sandbox flags through; a per-machine `maxPermissionMode` ceiling. | **DR** |
| 9 | **Budget enforcement** | Four limits, in process memory, only under `--autonomous`, checked between turns. | Frozen into signed `JobPolicy`, hashed, enforced twice — child pauses, supervisor independently terminates. | No spend or token cap anywhere. The `workflows` plugin caps agent calls (100, ceiling 1,000), concurrency, memory, stack and wall clock. | **DR** |
| 10 | **Telling the agent its budget** | Gate feedback carries `attempt N/max`. | Revise feedback is a fixed string. The agent never learns `max_attempts` or `max_turns` exist. | A workflow calls `budget()` for a live snapshot of its own remaining calls. | **bb** |
| 11 | **Surviving terminal and reboot** | Detached worker + supervisor, recovery journal. No OS service. | Plus a launchd/systemd user service and fenced lease; `start` refuses without it. | Server and daemons are always-on; `install.sh` writes a launchd plist or systemd unit with KeepAlive / `Restart=always`. | **DR bb** |
| 12 | **Agent splitting its own work** | `rlm()` spawns a real child; admission is a depth check. | Agent may write only an **inert** proposal. Fan-out is operator-initiated. | Any thread spawns children through the injected CLI, depth cap 4, cycle-checked. | — |
| 13 | **Steering a running agent** | Family messaging at a turn boundary; can wake a saved session. | Durable inbox; one of eleven routes drains it, with expected-turn-id and no-drop retry. | Steers **into the live turn** via `session.pushInput`; a stale turn id opens a new turn rather than dropping. | **bb** |
| 14 | **Read-only observation for the agent** | `agent-observe` skill: list, inspect, read. Explicitly cannot mutate. | Operator-facing only. Nothing for a plan parent. | Agents get read verbs plus a blocking `bb thread wait` — but the same CLI mutates, so nothing is read-only. | **PA** |
| 15 | **Scheduling and recurring work** | `/heartbeat`, model-owned heartbeats, `schedule` with cron. | None. A Job runs to a terminal outcome and stops. | `automations` plugin: cron plus one-shot, 10s sweep, CAS-claim before dispatch with rollback. | **PA bb** |
| 16 | **Provider and model catalog** | 9 wire APIs; 1,162 models across 31 providers, generated, with pricing. | 11 embedded descriptors; `providers.d/*.toml` overrides. Hand-maintained. | No catalog at all. Four provider descriptors; reads each vendor's own account API for plan usage. | **PA** |
| 17 | **Programmatic control surface** | `--mode json`, ~45-command bidirectional RPC, ACP. | `--json` on ~30 verbs. `watch` aliases `attach` but is one-shot, not a stream. | 141 typed HTTP routes compiled into both server and SDK, plus a CLI. Watch is pull-based invalidations. | **bb** |
| 18 | **Cost observability** | Per-agent own-vs-total tree with dollars from the catalog. | `spend.jsonl` per run with running total against the cap. | Claude Code hands bb `total_cost_usd` and a usage breakdown every result; bb persists neither. | **PA** |
| 19 | **Reusable instruction files** | Five-tier discovery, progressive disclosure, collisions reported. | Run skill resolves two tiers, deliberately excluding the worked-on repo. | Six skill root kinds; six ordered instruction blocks, each non-builtin one prefixed with its source and capped at 4,096 chars. | **bb** |
| 20 | **Executable extensions** | Skills are pip-installed Python packages; installing one runs its build backend, no prompt. | Four fixed subprocess seams, conformance-validated, with the gate explicitly not a seam. | Full-trust in-process Node plugins behind one install-time y/N. But `workflows` runs in QuickJS with no fs, shell, net, clock or random. | **DR** (containment) |
| 21 | **Editing the harness's own source** | Explicitly forbidden; base-prompt edits hard-rejected. | `improve self`: isolated worktree, evidence score, eight-reason PR gate. | No self-improve verb. `skill-creator` has an agent write `~/.bb/skills/<name>/SKILL.md`, loaded by the next thread with no approval step. | — |
| 22 | **The actuator surface** | One tool, `ipython`, taking a string of Python. | On CLI routes, no tool surface — the vendor CLI owns its own. | Provider owns tools; bb injects an MCP proxy so plugin-registered tools reach an agent it did not write. | — |
| 23 | **Where the work executes** | The operator's current directory. No provisioning of any kind. | The operator's own machine only. | Any enrolled machine. `bb thread spawn --machine <id>`; each machine carries its own permission ceiling. | **bb** |
| 24 | **Preparing the workspace before turn one** | None. Work happens where the agent was started. | Four codebase modes with base ref pinned, but `seed_dirty_files` never seeds gitignored files, and no setup step exists. | `.worktreeinclude` copies gitignored `.env` and certs; `.bb-env-setup.sh` runs pre-turn, and a non-zero exit fails provisioning. | **bb** |

Rows 23 and 24 are new in this pass. Three other candidate rows were cut — see
[what the review threw out](#what-the-review-threw-out).

---

## The one that matters: `learn`, `/refine`, and bb's memory

You asked specifically how `learn` maps to the feedback-loop concept. With a
third implementation on the table the picture is sharper, because the three
products split the same loop three different ways.

### The loop has two halves, and each product built a different subset

|  | Reflect on finished work | Read the result back into a later prompt |
|---|---|---|
| **Prime Agent** | Yes — auto-refine, on by default | Yes — system prompt rebuilt in place |
| **bb** | **No** — deliberately deferred | **Yes** — catalog appended every turn |
| **DeadReckon** | **Yes** — and with the best discipline of the three | **No** |

```
Prime Agent   trajectory ─▶ /refine ─▶ harness_state.json ─▶ system prompt ─▶ next turn
                                                                  └──── closed ────┘

bb            (no reflection)          memories table ─▶ system prompt ─▶ next turn
                                            ▲                  └──── closed ────┘
                                    agent volunteers a write mid-task

DeadReckon    run evidence ─▶ learn index ─▶ signals ─▶ learn propose ─▶ proposal.json
                                                                             ╳
                                                                      (a human reads it)
```

That is the finding worth acting on: **both competitors built the read-back
half, and DeadReckon is the only one that did not.** One of them decided
reflection could wait; neither decided read-back could.

### DeadReckon is better at the part that is hard to get right

The citation enforcement in `crates/deadreckon-core/src/learning.rs` has no
equivalent in either competitor: a proposal whose stimulus cannot cite a
locally-known `signal_id` matching its `run_id` does not get written. Prime
Agent's refinement is a model asserting that a model's self-report is worth
persisting, checked only by a second model. bb's memory is whatever the agent
volunteered.

But the consequence of the missing half is visible. On this machine there are 22
runs in runstate, **2 indexed episodes**, and no `signals.jsonl`, no
`insights.jsonl`, no `proposals/`, no `candidates/` at all. Because indexing only
happens when the operator remembers to type it, and nothing ever prompts them to,
the loop has never run end to end even once.

### The honest complication

DeadReckon's own architecture map already looked at the same evidence and drew
the opposite conclusion. [`MAP-OF-DEADRECKON.md:201`](MAP-OF-DEADRECKON.md) grades
learning and self-improve as *"**Experimental**; weak connection to the original
highest-priority needs"* and recommends it as a *"**Strong
product-decision/deprecation candidate**"*, and line 220 states it is not part of
the primary product promise.

The empty store is the same fact read two ways. **This is a keep-or-cut decision
before it is a build decision.** What the competitors add is evidence that two
independent teams both considered the read-back half load-bearing enough to ship.

### If you keep it, the cheap first move

Append a `LearnIndexed` step to the Job terminal path in
`crates/deadreckon/src/commands/supervisor.rs`, so `index_learning` runs
automatically when a Job reaches a terminal outcome. Keep it deterministic and
best-effort, so an indexing failure can never change a Job's outcome. Within a
few weeks of ordinary use it produces the usage-and-quality evidence the
deprecation question is waiting on.

### Adoption costs four pieces of trust machinery, not three

The obvious design — materialise an accepted proposal as guidance at
`~/.deadreckon/skills/<name>/SKILL.md`, which `resolve_skill_path` already
prefers — needs:

1. **A denylist entry.** `~/.deadreckon/skills/` is not currently protected;
   `ProtectedPathPolicy::for_paths` covers gate keys, the jobs dir, per-run
   acceptance and proofs, and resolved `.git` metadata. Skills appear nowhere, so
   an agent could rewrite its own steering between turns.
2. **A per-turn digest check, not an admission-time one.** `run_skill_text`
   re-reads the skill from disk on *every* prompt build, so an admission-time
   hash would prove only what the file said before turn one.
3. **The `JobAuthority` and receipt binding**, so a receipt can never be produced
   under guidance nobody approved.
4. **Content sanitation on the write path, plus a per-artifact prompt cap.**
   *This is the piece bb adds.* bb's memory plugin refuses a write outright when
   the text contains invisible or bidirectional control characters, a role-like
   tag such as `<system>`, an ignore-previous-instructions pattern near
   "previous" or "system", a PEM private-key header, an `sk-`/`ghp_` token, or a
   `*_SECRET = value` assignment — precisely because the store is auto-injected
   into a system prompt. DeadReckon's `secret_like`
   (`learning.rs:1333-1342`) already covers the credential half of that list. The
   **prompt-injection half is genuinely absent**, and it is the half that matters
   when the text being stored was written by a model. Without it the denylist
   protects the file while the file's contents steer the agent: the digest binds
   what was said, not whether what was said is an instruction the operator would
   have approved.

Note also that [`V1-CANDIDATES.md:39`](V1-CANDIDATES.md) already defers "make
repository/team rules a signed acceptance input and name their evidence in the
combined receipt" — the same digest-bound-guidance design, reached
independently. This is a deferred idea, not an unconsidered one.

---

## What is worth taking, ranked

### 1. Tell the agent how much budget it has left — *smallest change, immediate value*

Prime Agent's gate feedback carries `attempt N/max`. bb lets a running workflow
call `budget()` for a live snapshot of its own remaining agent calls. DeadReckon's
revise feedback is a fixed string:

```
acceptance failed after turn {turn}: {reason}. Continue by fixing the failing
done criteria; do not declare done until dr-gate passes.
```

The agent is never told that `max_attempts` or `max_turns` exist, let alone how
much of either remains. An agent that knows it is on its last bounded attempt can
consolidate and leave the tree reviewable instead of starting a risky refactor.

One more interpolated field in an existing string at
`crates/deadreckon-runtime/src/turn_loop.rs:5824`. No new input class, no new
signing surface, no change to what "done" means. **This is the only row where
DeadReckon is behind both other products**, and it is the cheapest fix in the
document.

### 2. A declared workspace-preparation step whose transcript is evidence

This is the strongest thing bb has that DeadReckon does not, and it addresses the
most common way an unattended run fails for a reason that has nothing to do with
its goal: no dependencies installed, no `.env` present.

bb's `.worktreeinclude` copies gitignored local files — explicitly `.env` and
certs — into every new worktree, hardened with `lstat`, symlink skipping and
`copyFile(..., COPYFILE_EXCL)`. `.bb-env-setup.sh` then runs inside the worktree
in its own process group under a timeout, SIGKILLed by group, with output
streamed into the thread's provisioning transcript. A non-zero exit, a signal or
a timeout **fails provisioning and the thread never starts**.

DeadReckon's `seed_dirty_files` deliberately never seeds gitignored files, and
none of its four seams prepares a workspace. So the only way to get dependencies
installed today is to make the agent do it inside the run — burning turns against
`max_turns: 12` and putting environment setup inside the window the gate is
judging.

The two halves should be sequenced separately, because they are not equally
ready. An operator-declared include list is a small extension to
`seed_dirty_files` in `crates/deadreckon-core/src/codebase.rs:571`. A setup
command is harder than it looks: its whole purpose is usually installing
dependencies, which means network, inside a sandbox whose posture is to deny it.
That tension is real and unresolved, and `V1-CANDIDATES.md` already touches both
halves — check it before designing.

If it is built, bb's failure semantics are the right ones and are the opposite of
what a first reading suggests: **a failing setup fails admission, never
acceptance.**

### 3. A blocking wait with distinct exit codes, instead of operator polling

bb's CLI maps timeout, unreachable-status and invalid-request to distinct exit
codes, and its built-in skill explicitly tells agents not to shell-sleep.

Add `--until <state>` and `--timeout <dur>` to `deadreckon attach` and its
existing `watch` alias, driven by the `AttachJsonlTail` that already exists: exit
0 on the sealed receipt, a distinct code on timeout, and a distinct code when the
requested state is already unreachable because the run reached a terminal
outcome. Pairs directly with recommendation 7.

Use `--until`, not `--wait`: `library list` and `search` already own that
vocabulary. And note that bb's own implementation is weaker than its design —
`bb thread wait`'s default `--status` path polls at 250ms and never touches the
long-poll endpoint. Take the exit-code contract, not the implementation.

### 4. Generate the model and pricing catalog rather than hand-maintaining it

Prime Agent generates a catalog of 1,162 models with per-token pricing.
DeadReckon hand-maintains catalogs inside eleven TOML descriptors — and this is
not cosmetic, because **DeadReckon meters in dollars**. `max_spend_usd` is frozen
into `JobPolicy` and hashed as `effective_policy_sha256`. Stale pricing silently
degrades the budget guarantee the whole authority chain exists to protect, and
does so invisibly, because the receipt binds the policy digest, not the pricing
that priced it.

bb suggests a cheaper partial answer: it reads each vendor's own account and
usage endpoint rather than modelling cost itself. A `doctor` check that warns
when a configured route's account is on a **subscription plan** — where a
per-token `max_spend_usd` is measuring the wrong thing entirely — would catch a
real class of silent mismeasurement without building a catalog generator.

### 5. Provenance-labelled instruction assembly

bb builds one instruction blob in a fixed documented order and prefixes every
non-builtin block with a sentence naming its source, capping each contributor at
4,096 characters.

DeadReckon's prompt is already sectioned by label and has only two operator-owned
instruction inputs, against bb's six blocks from untrusted plugins — so this is a
**low-impact** cleanliness item, not a safety one. It earns its place only as
groundwork: prefixing each block with the resolved path from `resolve_skill_path`
makes the digest-bound-guidance design of the learning section auditable by eye
rather than only by hash.

### 6. Fix the `improve self` bugs — *but do not generalise it*

Four real defects in `crates/deadreckon/src/commands/learning.rs`:

- `load_self_improve_proposal` mints a fresh `prop-<uuid>` per invocation (`:946`),
  so a later `--pr-dry-run` can never match a candidate an earlier `--yes` created.
- `blocked_auto_pr_reasons` is persisted (`:879`) and read by nothing.
- `is_high_risk_path` (`:1729`) covers gate, sandbox and release paths but **not
  `learning.rs` itself** — the file that owns redaction is not high-risk under its
  own rules.
- The `--yes` reset/restore/stage/commit sequence (`:744-766`) has no end-to-end
  test; the only test using `--yes` asserts a refusal.

The tempting fix — derive the candidate's acceptance contract from the proposal's
own `done_criteria` — is backwards for this product. It means the model that
authored the change also authors the test that certifies it, and
`evaluate_auto_pr` opens a PR at ≥0.85 with no operator in the loop. That is
`goal.complete()` with more steps.

### 7. Streaming `attach --json`

`watch` already exists as a visible alias of `attach` and already takes `--json`,
which prints one pretty JSON projection and exits. The gap is a follow mode that
emits newline-delimited lifecycle rows and terminates on the sealed receipt. The
incremental tailer already exists but is reachable only from the TUI path.

Keep it a file tail, not a socket. bb is the cautionary case here: its
`/api/v1` has **no authentication middleware at all**, and its server passes no
hostname to `serve()`, so it binds every interface. Any host that can route to
the port can create threads, send messages, and commit or squash-merge an agent's
work. DeadReckon's "no production listener at all" is the strongest posture of
the three and should survive this feature.

### 8. Two small borrows

- **`!shell-command` credentials** (Prime Agent). Source keys from 1Password or
  the keychain instead of storing them.
- **Own-vs-total spend rollup** for plan children and campaign subs. The data is
  already on disk in the per-run ledgers plus plan lineage.

---

## What DeadReckon should refuse

**Default-on automatic refinement** (Prime Agent). Runs every 25 assistant turns
subject to a 20-minute cooldown, gated only by a second model, applied to the live
system prompt with no human in the loop. DeadReckon refuses the agent's own
completion claim; it cannot coherently accept the agent's own *steering* claim.
Worth noting that this writes the **session-local** store only, and appears in no
Prime Agent documentation — only in the CHANGELOG.

**Agent-callable refinement** (Prime Agent). `refine.run()` and
`rlm.harness.create_memory(...)` let the model write its own durable steering
state from inside its own kernel. The `reshape` design already establishes the
right answer: the agent may propose, the proposal is recorded inert, and only an
operator command makes it real.

**Unapproved skill authorship** (bb). bb's `skill-creator` has an agent write
`~/.bb/skills/<name>/SKILL.md`, which the next thread loads with no approval step.
That is the same hole as agent-callable refinement, reached from a different
direction, and it is exactly what item 4 of the adoption-cost list guards against.

**A project tier for the run skill.** Doc skills resolve three tiers including the
worked-on repo; the run skill resolves two. That is a blast-radius boundary, not
drift — the run skill is re-read from disk on every prompt build, and in
`CodebaseMode::InPlace` the working directory *is* the source path.

**Skills as installed Python packages** (Prime Agent). Installing one runs its
build backend with no confirmation, inside a harness with no sandbox.

**An authenticated HTTP control plane** (bb). See recommendation 7.

**Recurring agent instructions.** Perpetual agents are the opposite of
DeadReckon's bounded-run identity.

**Trace upload to a vendor endpoint.** `learn export` already does the useful part
better and locally, with deep redaction and hash-verified bundles.

---

## What the review threw out

Recorded because the reversals are more instructive than the survivors. All of
these were confident, cited, and wrong.

### From the Prime Agent pass

| Original recommendation | Why it died |
|---|---|
| "Move the sensors left — checks only fire when the agent claims `done`" | **Backwards.** `turn_loop.rs:1178` sits inside the `is_cli_subagent` branch: on CLI routes, the primary path, the full contract already runs after **every** turn, keyless and sandboxed. Only `:1949` is a `done` boundary, and it is direct-API only. The proposal was to add an *unsigned advisory* tier — strictly weaker than the signed one already running. |
| "Prime Agent runs its gates after every assistant turn" | **False**, and it was the premise for the above. Both products sense at the completion boundary. |
| "Bound the prompt history — no context management at all" | **Already shipped.** `compaction.rs` is deterministic, default-on and context-window-aware with a durable ledger. The recommendation's own risk note was describing the existing implementation. |
| "A long run's prompt grows until the context window ends it" | **Structurally impossible.** `max_turns: 12` at every production call site, and tool output never enters history — only `"tool {id} result: status={code}"`. |
| "No way to notice a futile gate retry" | **Already present and stronger.** `classify_cli_no_deliverable_changes` terminates and preserves the real cause. |
| "Add a `models.toml` override" / "Add `deadreckon watch --json`" | Both already ship (`providers.d/*.toml`; `watch` is an alias of `attach`). |
| "Give the run skill a project tier" | Net-negative for the trust model. |
| "`verdict --watch` as a drift sensor" | Cites a self-criticism about deployed SLOs, not about re-proving a frozen tree. `verdict --all` already exists and `verdict` self-labels as non-authoritative. |

### From the bb pass

| Original claim or recommendation | Why it died |
|---|---|
| "Steer into the live turn with a staleness check and no-drop fallback" — proposed as new work | **Already ships**, with a passing test literally named `stale_turn_precondition_retries_not_drops` (`codex_app_server.rs:2207`, plus `pending_steer_delivers_with_expected_turn_id` at `:2144`). The "open design question" framing would have misled a reader into thinking the no-drop contract is unbuilt. |
| "bb never reads a finished thread — all three `thread.idle` subscribers discard the content" | **False**, and it was asserted without opening `plugins/workflows/src/server.ts`. The workflows plugin passes `lastAssistantText` in as the step's return value, and with an `outputSchema` validates it and sends up to two corrective turns before failing. |
| "bb has no budget, no ceiling of any kind" — called the table's sharpest asymmetry | **False**, and the search that produced it was scoped to `apps/` and `packages/`, excluding `plugins/` where bb's own unattended-execution feature lives. `workflows` ships `maxAgentCalls` 100 (ceiling 1,000), `maxConcurrentAgents`, memory, stack, a synchronous deadline and a 24h wall clock — and runs in a QuickJS sandbox with no fs, shell, net, clock or random. |
| "bb has no OS service install" | **False.** `install-machine.sh`, served from the bb server at `GET /install.sh`, writes a launchd plist with `RunAtLoad`/`KeepAlive` or a systemd user unit with `Restart=always`. |
| "bb's git panel is read-only with no accept action" | **False.** `POST /environments/:id/actions` accepts commit, squash_merge, pull_request_ready/draft/merge, and `/api/v1` has no auth. |
| "bb's exposure is bounded to loopback" | **False, and the wrong direction.** `serve({port, fetch})` passes no hostname, so it binds all interfaces, and bb's own docs name Tailscale ACLs as the boundary. |
| "Two independently designed orchestrators both carve the completion decision out of the extension surface" | **Manufactured convergence.** bb bars plugins from vetoing a *status-row transition* — bookkeeping, not a trust boundary. bb's own summary says it owns no judgement at all. |
| "Automatic child-outcome report-back to the plan parent" | Already built as `PlanEventKind::TaskCompleted { run_id, status }`, and would not close row 14 anyway, which is about an agent-facing capability. |
| "Read the vendor's usage endpoint to detect subscription plans" (as a route-detection mechanism) | The route kind already tells DeadReckon this deterministically and offline. Only the *staleness* half survives, as recommendation 4. |
| Three further proposed rows — human-decision-mid-run, control-plane authorization, concurrency limits | Each refuted itself in its own justification ("deliberate, not a gap", "context only", "already approximates"). The authorization row was also the mirror category error: scoring a headless CLI on HTTP listener authentication. |

Two notes for whoever picks up the futile-retry work: Prime Agent's snapshot
guard only functions because it excludes `target/` and `Cargo.lock` — a naive port
would never see two identical snapshots, because a `cargo test` gate rewrites
`target/` every attempt. And DeadReckon's existing guard fires on "no deliverable
change at all" rather than on a tree hash, so an agent thrashing a file back and
forth still consumes attempts.

---

## The pattern underneath

The first pass concluded: *where Prime Agent reaches for a model, DeadReckon
reaches for a deterministic function plus a durable record.* That holds. bb
sharpens it, because bb reaches for neither.

bb keeps an exemplary durable record — an append-only `events` table with a
unique `(thread_id, sequence)`, from which the conversation UI is a pure
projection rather than stored view state — and then applies no judgement to it
whatsoever: no gate, no scoring, output served as a direct database read, and an
extension system contractually unable to veto.

So the axis is not model-versus-deterministic-function. It is **how much
judgement the harness is willing to own.** Prime Agent delegates it to the model.
bb delegates it to the operator's eyes. DeadReckon owns it deterministically and
signs the result. Put that way, the durable record is table stakes for anything
in this category — bb's is arguably the cleanest of the three — and the actual
differentiator is the refusal to let anyone else make the call.

That is also why the learning gap is worth taking seriously rather than copying.
The reason DeadReckon has not closed its loop is the same reason its loop would
be worth trusting once closed. The last inch is genuinely harder here than it was
for either competitor, because DeadReckon has to bind what it learned into an
authority chain neither of them has.
