# Prime Agent and bb: a direct comparison

A capability comparison of two open-source agentic coding tools as each stands
today. DeadReckon is not part of this comparison; it is a sibling document to
[`PRIME-AGENT-CAPABILITY-MAP.md`](PRIME-AGENT-CAPABILITY-MAP.md), which compares
both against DeadReckon.

## What each one is

**[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)** is Prime
Intellect's terminal coding and research agent, a distribution of the `pi` coding
agent. It owns the model loop and defines the model's entire tool surface, which
is exactly one tool: `ipython`, backed by a long-lived out-of-process IPython
kernel. The conventional file tools were deleted outright. Every edit, search,
shell call, skill invocation and subagent spawn is Python in a single `code`
string against one persistent namespace. Around that sits the Continual Harness:
a versioned store of prompt notes, memories, skill descriptions and subagent
specs that an LLM refinement pass rewrites from the session's own trajectory and
re-injects into the system prompt.

**[bb](https://github.com/get-bb/bb)** is a self-hosted agentic IDE and
orchestration server that contains no agent at all. A Node/Hono server over one
SQLite file dispatches typed RPC commands to per-machine host daemons, which
provision workspaces and spawn coding-agent CLIs the operator already has
installed — Codex, Claude Code, Cursor via ACP, and `pi` — as ordinary child
processes, then normalise their output into a per-thread append-only event log.
Four interchangeable front ends (Electron desktop, web SPA, `bb` CLI, and a
141-route typed HTTP API) are co-equal clients of one contract, and a
VS-Code-shaped plugin system extends every layer of it.

## The comparison is not "which is better"

It is **where each puts the mechanism**.

Prime Agent's answers live *inside* the turn: working state is a Python variable,
delegation is a Python expression, learning is an automatic reflection pass over
the transcript, and unattended safety is a token budget plus a shell command that
must exit 0.

bb's answers live *around* the turn: state is a SQLite row, delegation is an HTTP
call, learning is a record the agent volunteers, and safety is a git worktree plus
a per-machine permission ceiling.

Almost every genuine difference in either direction follows from that one
structural choice. And the layers mostly **stack rather than compete**: bb's
fourth provider is `pi`, driven in-process through Prime Agent's own
`createAgentSession` SDK, so in one real configuration bb is an operator shell
around Prime Agent's engine. The genuinely contested middle is orchestration,
memory, and control of long-running work — where both built their own answer and
the answers differ.

---

## The chart

| Capability | Prime Agent | bb | Ahead |
|---|---|---|---|
| **Execution model** ||||
| What tools the model is given | `ToolName` is the one-member union `"ipython"`. Files, shell, skills and spawns are all Python in one `code` argument. | Defines no model tool surface — the provider CLI brings its own. Adds plugin-registered tools per resolution. | By design |
| Working state between tool calls | One persistent IPython kernel per session. Variables outlive turns, survive compaction, and are dill-snapshotted so they survive process death. | No cross-call state layer; working state is the provider's conversation plus files on disk. | By design |
| Where a unit of work physically runs | The operator's current directory. Children inherit the parent's `_cwd`; the README tells you to use a disposable clone yourself. | An environment binds a workspace to a machine: `git worktree add -B <branch>` under the data dir, or a reused or unmanaged path. | **bb** |
| Preparing that workspace before turn one | None. | `.worktreeinclude` copies gitignored `.env` and certs; `.bb-env-setup.sh` runs pre-turn in its own process group, and a non-zero exit fails provisioning. | **bb** |
| **Agent orchestration** ||||
| Delegating a sub-task and getting the answer back | `await rlm('task')` returns an admission handle, never the answer; the child replies via `agent_message.send(receiver_role="parent")`. | `bb thread spawn --parent-self`; outcomes batch into a rendered `[bb system]` message injected into the parent after a 2s debounce. | Parity |
| Deterministic orchestration of several agents | None. Fan-out is the model writing `rlm()` calls in Python — model-driven, not scripted. | The `workflows` plugin: JavaScript orchestration in a QuickJS WASM sandbox with no fs, shell, net, clock or random, and schema-validated worker output. | **bb** |
| Redirecting an agent that is already running | `streamingBehavior` is hardcoded to `"steer"`; a busy target gets the message queued as a steer rather than dropped. | `expectedTurnId` captured at dispatch; `steerTurn` calls `session.pushInput` on the live query, and a stale id opens a fresh turn. | Parity |
| Bounding agent-to-agent traffic | 16,384-char cap, 20 unfinished actions per target, a sender→target token bucket (capacity 3, refill 1/s), and a global operator pause. | No rate limit or queue cap in core. Bounds are the depth-4 cap and a 2s notification debounce. | **Prime Agent** |
| An agent asking a human and blocking on the answer | No built-in channel. An extension can call `confirm/select/input`, but it is an in-process promise, not durable state. | `pending_interactions` is a durable table: provider approvals and plugin forms both land there, block new sends, and resolve from app, CLI or phone. | **bb** |
| **Memory and learning** ||||
| Shape and scope of the durable store | `harness_state.json`, four kinds, two scopes — session-local by default, global opt-in. Deliberately no project scope. | `memories` in a plugin-owned SQLite file: typed six-kind records scoped global or to one project, plus AGENTS.md at two tiers. | **bb** |
| Who writes durable knowledge, and when | Unattended by default. Auto-refine fires every 25 assistant turns and after each compaction behind a second-LLM evidence gate. | The agent must volunteer `bb memory add --reason …` mid-task. No reflection exists; the memory README defers it. | **Prime Agent** |
| Undoing a persisted lesson | Every applied edit stores deep clones before and after; `/refine rollback <id>` synthesizes inverse edits with no LLM call. | `memory_history` keeps an append-only post-state snapshot and `--expected-version` prevents lost updates, but there is no revert verb. | **Prime Agent** |
| Keeping an auto-injected store from becoming an injection vector | None on harness content. `validateEdit` checks structure — ids, titles, the skill contract — and never inspects the text. | `unsafeMemoryReason` rejects the whole write on invisible or bidi control chars, role-like tags, ignore-previous-instructions patterns, PEM headers, `sk-`/`ghp_` tokens and `*_SECRET=` assignments. | **bb** |
| **Extensibility** ||||
| Skills | Same SKILL.md format and progressive disclosure, but a skill may be a real Python package `uv pip install --editable`'d into the kernel venv and pre-imported as an awaitable. | Same format across six root kinds, four of which are injected. A skill may bundle scripts the agent shells out to, but bb never installs or executes skill code itself. | By design |
| Extension code — reach and trust | TypeScript extensions from `~/.prime/agent/extensions` register tools and slash commands, and can block or mutate a tool call. No install prompt anywhere. | Plugins get 15 backend namespaces, 11 React UI slots, per-plugin SQLite, HTTP/RPC routes, cron and a `bb <name>` CLI — behind one explicit full-trust confirmation. | **bb** (breadth and consent, not containment) |
| External tool servers (MCP) | MCP integrations are Python skills subclassing `McpIntegration`; the protocol runs client-side in the kernel with host-owned OAuth. | Consumes MCP inward only — a `bb-bridge` proxy injected into the agents it launches. User-facing MCP is whatever the vendor CLI supports. | By design |
| **Operator and programmatic surface** ||||
| Human-facing surfaces and device reach | One TUI plus a fleet view. Same machine, same uid, unix socket — no network surface. | Electron desktop, web SPA, CLI and HTTP API as co-equal clients, plus task board, git panel, per-machine daemons and an owner-gated tunnel. | **bb** |
| Programmatic drive surface | `--mode rpc` gives 45 bidirectional JSONL commands including compaction, refinement and fork; plus `--mode json`, `--mode acp`, and an in-process SDK. | One typed 141-route HTTP contract compiled at build time into both the server schema and the SDK client; the CLI wraps the SDK. | Parity |
| Following a running agent | Push. `--mode json` writes one JSON object per event to stdout; RPC streams events on the command channel. | Pull. `/ws` emits content-free invalidations, so a consumer refetches `/events?afterSeq`. No SSE, no `--follow`. | **Prime Agent** |
| Disposing of finished work | Nothing. The work is already in your working directory. | `POST /environments/:id/actions` and matching CLI verbs: commit with an AI-generated message, squash-merge, and PR ready/draft/merge. | **bb** |
| **Trust and isolation** ||||
| Who is authorized to drive the local control plane | Daemon socket is 0600 inside a 0700 uid-scoped directory whose ownership is asserted at startup, plus a per-worker `randomBytes(32)` bearer token. | `/api/v1` has **no authentication middleware at all**, and `serve()` is passed no hostname, so it binds every interface. | **Prime Agent** |
| Confining what the agent may do | Nothing built in. Kernel and worker inherit the parent environment with no uid change or namespace; sandboxing exists only as an example extension. | No OS sandbox of its own either, but it configures the provider's, surfaces provider approvals as resolvable interactions, and enforces a per-machine `maxPermissionMode` ceiling. | **bb** (levers, not containment) |
| **Long-running work** ||||
| Surviving the terminal going away | A detached per-session worker owns the AgentSession; closing the TUI detaches and `attach` reconnects, under a one-writer session lease. | Server and host daemons are always-on services installed as launchd or systemd units; clients were never in the execution path. | Parity |
| Bounding an unattended run | Four budgets under `--autonomous`, plus `--autonomous-gate` shell commands that must exit 0. Checked between turns only, and `normalizeLimit` imposes **no maximum**. | No spend or token cap anywhere. But the `workflows` runtime enforces `maxAgentCalls` (100, hard ceiling 1,000), concurrency, memory, stack and a synchronous deadline. | Contested — see below |
| Recurring and scheduled work | Core: `schedule add` per agent, one operator `/heartbeat`, and `rlm_heartbeat` so the model can schedule itself. | A bundled plugin: `automations` sweeps every 10s and CAS-claims a due row by advancing `next_run_at` before dispatch, rolling back on failure. | Parity |
| **Cost and observability** ||||
| Cost and token accounting | Per-message tokens and dollars from a 1,162-model catalog including cache-write blending; `/context` shows own-vs-total per agent. | Tokens yes, dollars no. Claude Code hands bb `total_cost_usd` and a per-model breakdown every result and bb persists neither. | **Prime Agent** |
| Model and provider access | Owns the provider layer: 9 wire protocols, 31 providers, 1,162 models, OAuth subscription login, per-child model selection. | Owns none of it. Four provider CLIs the user authenticated themselves; reads their credential stores read-only to display quota. | By design |

---

## What Prime Agent does better

1. **Working state is a live namespace, not conversation text.** File contents,
   search results and task handles survive compaction and, via dill snapshots,
   survive process death. bb has no analogue because it has no agent —
   `BB_THREAD_STORAGE` is a scratch directory, not a namespace.
2. **Learning is automatic and reversible.** Auto-refine reads the trajectory
   every 25 turns behind a second-LLM evidence gate, and any refinement can be
   rolled back by id from stored before/after snapshots. bb has no reflection at
   all and no revert verb.
3. **It knows what work cost.** Per-message dollar accounting from a maintained
   catalog, with child spend attributed to the spawning parent turn as an
   own-vs-total tree. bb is handed the number by Claude Code and throws it away.
4. **Its local control plane is genuinely authorized.** A 0600 socket inside a
   uid-asserted 0700 directory plus a per-worker random bearer token, against
   bb's zero authentication on a port bound to every interface.
5. **Agent-to-agent traffic has back-pressure.** Message size cap, per-target
   queue cap, a token bucket, and a global operator pause. bb's messaging reach
   is deliberately wider — any thread to any thread, across projects — which is
   precisely why the missing back-pressure matters more there.

## What bb does better

1. **Work gets its own workspace.** A managed git worktree on its own branch on a
   chosen machine, with `.worktreeinclude` propagating untracked `.env` files and
   a `.bb-env-setup.sh` provisioning hook, torn down with the environment. Prime
   Agent runs N concurrent children in one checkout and manages the collision
   risk with prose rules in `AGENTS.md`.
2. **A blocked agent can ask a human, durably.** A `pending_interactions` row
   blocks further sends, notifies the parent thread, and is answerable from the
   desktop app, the CLI, or a phone. Prime Agent's autonomous run with no gates
   configured keeps going until a budget limit rather than asking.
3. **Deterministic orchestration with real ceilings.** The `workflows` plugin runs
   JavaScript in a QuickJS WASM sandbox with no fs, shell, net, clock or random,
   under limits whose maximums are not operator-overridable, and validates
   structured worker output against a schema with a bounded corrective-retry loop.
   Prime Agent's fan-out is the model writing Python.
4. **Disposition is first-class.** Commit with a generated message, squash-merge,
   and pull-request ready/draft/merge are typed environment actions and CLI verbs.
   Prime Agent has no concept of accepting work because the work is already in
   your directory.
5. **One typed contract drives everything.** 141 routes compiled into both the
   Hono server schema and the SDK client, so desktop, web, CLI, agents and plugins
   structurally cannot diverge.

---

## Where they converged

The most interesting findings are the problems both hit independently and
answered the same way.

1. **Redirecting a running agent.** Both landed on the identical two-mode design —
   steer into the live turn as default, queue behind it as the alternate — and
   both degrade rather than drop. Prime Agent falls back to a queued steer when
   the target is busy; bb falls back to a fresh turn when `expectedTurnId` is
   stale. Same answer, opposite architectures.
2. **Crash recovery.** Both wrote down the same rule — never replay uncertain work
   — and both journal intent durably and reconcile on restart. Prime Agent fsyncs
   a recovery journal and converts still-busy records to `mark_interrupted`; bb
   runs a four-pass reconcile against what the daemon actually reports.
3. **Scheduling.** Both claim a due tick and advance the schedule *before*
   delivering, and both coalesce rather than stack when a dispatch is outstanding.
   Prime Agent under a file lock, bb as a CAS on `next_run_at` with rollback.
   Independently identical.
4. **Letting the agent drive the harness.** Both decided the agent should
   orchestrate the orchestrator, and picked opposite transports. Prime Agent uses
   a typed in-process comm over the kernel's control channel — possible only
   because it owns the loop. bb prepends its own CLI to the agent's PATH, injects
   `BB_THREAD_ID`, appends "you are working inside bb" to every system prompt, and
   ships a `bb-cli` skill. Stringly-typed and slower, but it works on an agent bb
   did not write.
5. **Distinguishing agent speech from user speech, and capping recursion.** Both
   cap agent-tree depth in code rather than trusting the prompt (Prime Agent 1,
   bb 4), and both wrap an inbound agent message so the model cannot mistake it
   for a user turn.
6. **Secret handling.** Both chose 0600 files outside the main data store rather
   than a database column — Prime Agent's `auth.json` with `!shell command`
   indirection so a value can come from a keychain, bb's per-plugin `secrets/`
   directory whose values never reach the frontend.

## Where they diverged on the same problem

1. **Durable state.** Prime Agent chose append-only fsync'd files everywhere —
   session entry trees, subagent registries, the harness store — with lockfiles
   and mtime-based optimistic concurrency. bb chose one SQLite file with a
   declarative lifecycle transition table and a single compare-and-set writer.
   Prime Agent's choice buys in-place fork and branch of a conversation; bb's buys
   transactional consistency across threads, environments and queues.
2. **Which half of the learning loop to build.** Both agree knowledge must outlive
   one unit of work, and both render the store as a capped summaries-only menu
   appended to the system prompt with bodies fetched on demand. Then they split
   completely on authorship: Prime Agent runs an unattended LLM reflection pass by
   default; bb has no reflection and requires the model to volunteer a write
   mid-task. Same container, opposite filling mechanism.
3. **Whether unattended limits should have a ceiling.** This is the sharpest live
   disagreement, and it runs opposite to first impressions. Prime Agent has four
   named budgets and bb has none — but Prime Agent's exist only under
   `--autonomous`, are evaluated only between turns so they never stop an
   in-flight turn, and `normalizeLimit` accepts any positive value with **no
   maximum at all**. They are the backstop on a feature whose purpose is to push
   the agent past its natural stopping point. bb's `workflows` runtime, by
   contrast, has hard maximums an operator cannot raise. Neither has a spend cap.
   Read as "who can actually be stopped", this row is closer to a wash than to a
   rout.

---

## How to read this comparison

**They stack more often than they compete.** bb's fourth provider is `pi`, the
package Prime Agent is a distribution of, driven through Prime Agent's own SDK
inside bb's bridge. In that configuration "which should I pick" is a false
choice. (Version trains differ: bb pins `^0.82.0`; the Prime Agent checkout reads
`0.7.0`.)

**bb having no model tool surface, no compaction, no provider registry and no
cost model is its thesis, not a defect.** What is fair to judge is whether its
orchestration answers match Prime Agent's — and on cost accounting and quality
gates they do not.

**Prime Agent having no GUI, no plugin marketplace and no multi-machine fleet is
mostly not a defect either; it is a terminal program.** What *is* a defect,
because a long-running terminal agent would plainly want it, is the total absence
of workspace isolation and of any agent-to-human question channel.

**Neither product verifies or gates agent output on quality, and neither claims
to.** Prime Agent's autonomous shell gates gate a *run*, not a *result*. bb's
disposition actions commit and merge without inspecting anything.

**Maturity is not symmetric and the table does not encode it.** bb's plugin API
only left experimental status in 0.35.0, and much of bb's own surface is built on
that young contract. Prime Agent's auto-refine defaults appear in no
documentation, only in its CHANGELOG. Read every cell as "in the repo and wired",
not as "stable".

---

## Method, and what the review threw out

Six readers surveyed the bb monorepo; a mapper produced this comparison from
their findings plus a previously-verified Prime Agent survey; then two
adversarial verifiers attacked it — one on bb facts against source, one on the
fairness of the framing. Both repositories were read statically. Nothing was
executed, no server was started, and no agent was run, so every absence claim is
a grep-and-trace result. Those are strongest where structure forbids the thing
outright and weakest where a feature hides under vocabulary nobody searched —
which is exactly what went wrong below.

| Original claim | Why it died |
|---|---|
| "bb has no budget, no gate, no turn limit, no spend cap" — called the table's sharpest asymmetry, and the basis for "its extension system is structurally forbidden from adding one" | **The search was scoped to `apps/` and `packages/`, excluding `plugins/` — where bb's own unattended-execution feature lives.** The `workflows` plugin ships `maxAgentCalls` 100 with a hard ceiling of 1,000, `maxConcurrentAgents`, a memory limit, a stack limit, a synchronous deadline and a 24h wall clock, and runs in a QuickJS sandbox. No spend or token cap exists — that part survives. |
| "Nothing reads a finished thread; `thread.idle` carries `lastAssistantText` and all three subscribers discard it" | **False**, asserted without opening `plugins/workflows/src/server.ts`. The workflows plugin passes that text in as the step's return value and, with an `outputSchema`, validates it and sends up to two corrective turns before failing the call. |
| "bb's exposure is bounded to processes on the loopback interface" | **False, and in the wrong direction.** `serve({port, fetch})` is passed no hostname, there is no bind-host setting anywhere, and bb's own docs name Tailscale ACLs as the access boundary. The no-auth finding is materially worse than the map concluded, not milder. |
| "bb's git diff panel is read-only viewing with no accept, reject, revert or stage action" | **False.** `POST /api/v1/environments/:id/actions` accepts commit, squash_merge and three pull-request verbs, exposed as CLI commands — and `/api/v1` has no auth, so any process that can reach the port can merge an agent's work. |
| "Every leaf CLI command carries `--json`, enforced by a test" | The enforcement test registers 6 of the 16 command groups the CLI actually registers. The property holds today by convention; nothing enforces it, and a new leaf in the other ten would not fail CI. |
| Working state and MCP scored **prime-agent-ahead** | Both are agent-internal axes on which bb deliberately holds no opinion — the same category the note above exempts. Rescored as design differences. |
| "Both arrived at the same extension-trust posture: no manifest, no sandbox" (listed as convergence) | Undermined by the same discovery: bb does sandbox its workflow runtime. |
| "Both concluded a parent must not block on a child" (listed as convergence) | bb ships `bb thread wait`, which blocks. |
| "Both independently converged on SKILL.md and progressive disclosure" (listed as convergence) | Shared inheritance of a public format is not independent convergence. |
| "Opposite answers on durable state" (listed as convergence) | Self-describing as opposite. Moved to the divergence section. |
| Assorted counts | 11 React UI slots, not 10. Six skill root kinds, not four. 45 RPC commands, not "~40". |
