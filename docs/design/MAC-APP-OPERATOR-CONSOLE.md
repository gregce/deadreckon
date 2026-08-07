# Mac App Operator Console — Design Exploration & Build Plan

**Subject:** a native macOS app to drive deadreckon — complementary to `attach`, and for many operators a replacement for it.
**Produced:** 2026-08-0a6, on `codex/provider-conformance` @ `1674997` (deadreckon 0.8.2, post-Watchkeeper/Soundings durable job supervision).
**Method:** 14-agent research workflow (run `wf_70841300-151`): six readers over this repo (CLI surface, durable jobs, contracts/gate, attach TUI, providers/flight, philosophy/prior-art), two over `/Users/gdc/getspecstory/specstory-mac`, two web researchers (Conductor + Cursor; Codex/Copilot/Devin/Jules/Amp/Factory/OpenHands/Claude Code), three independent design studios, one gap engineer who verified every design demand against source with `file:line` evidence.

**How to review this doc:**

1. Skim §2 (the principles the designs were held to).
2. Read the three concepts (§3–§5) — ASCII first, annotations tell you where every pixel's data comes from.
3. React to the recommendation (§6): it proposes a composite, not a winner-take-all.
4. Sanity-check §7 (what the Rust binary must grow) and §8 (roadmap + this-week quick wins).
5. §9 is the app shell decision (what we copy from specstory-mac); §10 is what only you can decide.

---

## 0. TL;DR

- **The architecture argument is already over.** All three independently-designed concepts converged on the same chassis: an LSUIElement menubar-first SwiftUI app that is a **pure read-model over `~/.deadreckon`** (the same durable files `attach` tails — there is no daemon and no IPC, and none is needed), with **every mutation dispatched through a vendored, sha256-pinned `deadreckon` binary**, and **fail-closed trust states rendered verbatim with no override affordance**. The real choice is information architecture: what is the home surface.
- **Recommendation (§6):** compose them — **Quarterdeck's decision-ranked queue as the home**, **Chartroom's three-pane workbench as the drill-in**, the **converged Binnacle promote sheet** (all three designed nearly the same one), and **Bridge's menubar popover** for ten-second triage. That composite is exactly `start → attach → kill|promote` shaped.
- **The binary work is modest and additive** (§7): ten gaps, mostly S/M effort, none touching promotion authority. Six are quick wins achievable this week. The single highest-leverage change is a global `--json` result+error envelope on the nine state-changing verbs.
- **The differentiator:** every surveyed product (Conductor, Cursor, Codex, Copilot, Devin, Jules…) ends the agent loop at "here's a diff, trust your eyes." deadreckon ends it with a signed two-key receipt binding ten digests. **This would be the first agent UI where "done" is cryptographically load-bearing** — the app's whole job is to render that trust chain so a promote decision takes ninety seconds.

---

## 1. Ground truth: what the binary gives a Mac app today

Condensed from the code-research passes; every claim was re-verified by the gap pass against source.

### 1.1 The read surface — files are the API

Two state layers under `~/.deadreckon` (relocatable via `DEADRECKON_HOME`):

| Layer | Path | What a GUI reads |
|---|---|---|
| **Durable Job** | `jobs/<id>/` | `job.json` (immutable goal/identity) · `job-events.jsonl` (lifecycle **truth**: strict sequence 1..N, fsync'd, torn-tail tolerated) · `projection.json` (rebuildable checkpoint: phase `queued\|running\|verifying_checks\|verifying_meaning\|waiting\|terminal`, outcome, **typed stop_reason**) · `lease.json` (fenced ownership: owner/epoch/boot_id, heartbeat 2s, TTL 60s) · `launch-plan.json` · `authority.json` (sha256 freeze of goal/contract/policy/source) · frozen `acceptance.yaml` · `receipt.json` (signed two-key completion) · `supervisor.out/err` · `control.lock`/`operation.lock` |
| **Run** | `runstate/<scope>/runs/<id>/` | `state.json` (`PipelineState`: RunStatus, phases 0–60) · `events.jsonl` · `spend.jsonl` · `traces.jsonl` · `flight-events.jsonl` + `checkpoints/` · `provenance.jsonl` · `steer-inbox.jsonl` · `notify.jsonl` · `narrative/{state.json,snapshots.jsonl,architecture-graph.json}` · `proofs/turn-acceptance.json` + `acceptance-progress.jsonl` + `acceptance-tamper.json` |

**The decisive fact:** `attach` itself is a pure read-model over these files. `TuiEventFeed::file_tail` tails `events.jsonl`; the in-process broadcast bus is `#[cfg(test)]`-only. A GUI can render **everything the TUI renders** by tailing the same files with FSEvents — no new plumbing required for observation. Schemas for the load-bearing records are already checked in under `docs/schemas/` (job, job-event, job-lease, job-authority, run-event, spend, trace, flight, completion-receipt, semantic-judgment, sandbox-boundary-observation, plus `projections/run-view.schema.json` = `report --json`).

### 1.2 The write surface — verbs, and where they fall short

- **Operator happy path today:** `deadreckon start "goal" --yes` → `attach latest` → `status latest` → `kill latest` **or** `finish latest [--autostash --cleanup | --dest DIR]`.
- `start` queues a durable Job and detaches via `supervisor serve --once <job>`; a launchd/systemd singleton supervisor service scans `jobs/`, recovers after crash/reboot (boot_id change), max 4 concurrent recoveries; orphans fail closed to `Blocked/LostContainment`.
- `--json` exists on **inspection** surfaces (`status`, `list`, `show`, `report`, `verdict`, `doctor`, `detect`, `providers`, `models`, `plan`, `try`, `start` preview, `attach --view narrative`) emitting a consistent envelope `{kind,id,status,next_actions,try_lines,paths}`.
- `--json` does **not** exist on state-changers (`finish`, `kill`, `steer`, `apply`, `extend`, `merge`, `fork`, `materialize`, `abandon`) — text-first; exit codes (0/1/2/130) are the only machine signal. This is gap **G1**.
- `steer` is narrow by design: provider `cli:codex-server` AND RunStatus `Executing` only; it appends to `steer-inbox.jsonl`.
- `kill` mechanics: append sticky `CancelRequested` + `cancel.marker`, SIGTERM process groups, 2s grace, SIGKILL; supervisor writes terminal `Cancelled` only after proven cleanup.

**Surfaces the design teams initially missed (they exist today — fold into all plans):**
- `start --plan <launch-plan.json>` replays a saved plan and never prompts ("the plan is the decision").
- `start --json` (without `--yes`) emits a read-only launch **preview** envelope with `will_start:false`.
- `show <id> --diff --json` emits a full-run DiffSummary; `show --turn N --json` gives per-turn diffs.
- `materialize` / `apply` / `abandon` / `extend` exist as plain verbs (attach's `m/e/a/x` keys dispatch to the same functions) — only JSON envelopes are missing.
- Public `resume` is **retired**: torn-job recovery belongs to the supervisor. Any "resume picker" UI idea is misaligned — render supervisor status + lease freshness instead.

### 1.3 The trust model — the constraint every screen inherits

- The agent cannot mark its own gate. Keyless `dr-gate evaluate` runs **inside the sandbox** and refuses signing env vars; trusted childless `dr-gate sign` holds the per-run HMAC key (`0o600`, `~/.deadreckon/gate-keys/`, provably unreadable from the agent sandbox) and writes the v2 marker.
- Completion is **two keys**: the deterministic marker AND a read-only semantic judgment (`achieved|revise|uncertain`), sealed into `receipt.json` binding ~10 digests (goal, contract, policy, launch plan, source tree, result tree, marker, judgment, boundary observation, authority).
- The contract's `capabilities.network: deny|loopback|full` (default deny) is compiled into immutable `JobGatePolicy` (`e5d7f1d`); lint/admission/runtime fail closed if checks need more than declared.
- Promotion revalidates **before and after** the atomic rename into the library; tamper/drift fails closed **with no operator override**.

**Consequences for the app:** the app never invokes `dr-gate`, never signs, never renders an override control, never reads or displays gate keys, and re-validates via `verdict --json` before enabling any Promote button. Preview (`finish --dry-run`, §7 G4) must be report-only — real finish re-validates and re-stages from scratch.

### 1.4 What attach can't do — the GUI's headroom

1000-event scrollback cap · fixed pane heights · one artifact per session (no fleet view) · TUI suspension for nested output · no notifications · no search. `V1-CANDIDATES.md` explicitly defers the attach daemon, web mirror, and cross-machine attach — and `MAP-OF-DEADRECKON.md`'s standing guidance is **"expose, don't duplicate, the control plane."** The durable-jobs work (Watchkeeper, Soundings) is what changed since the desktop idea was last shelved: file truth + launchd supervision now make an external observer viable with zero new authority.

### 1.5 The philosophy the app must speak

From `WHAT-CODING-AGENTS-LEAVE-TO-THE-OPERATOR.md` and `PRODUCT.md`: *the agent produces a candidate; only an operator-owned controller may accept it.* The operator owns definition-of-done, limits, acceptance, promotion, undo. UI reports **file-backed reality, never model intent** — "evidence before elegance," no decorative dashboards. The six-clause friendliness contract carries into every sheet: auto-detect · preview-before-mutate · refuse-with-try · one-command rollback · one verdict + ONE primary action · lifecycle hint. Vocabulary is load-bearing (glossary.rs user words; nautical milestones: Course=launch planning, Helm=attach, Rudder=steer, Binnacle=unforgeable seal, Watchkeeper=durable supervision, Soundings=admission).

---

## 2. Design principles

### 2.1 From Conductor's IA (principles adopted; layout not copied)

- **P1** Sidebar is a work tree, not a menu: Projects → their active workspaces (units of agent work) nested beneath, each stateful.
- **P2** Unit-of-work has visible provenance: the system narrates what it did as a timeline of receipts ("Branched … from origin/main", "copied 611 files").
- **P3** Three-pane anatomy: left = what exists · center = drive/observe · right = evidence.
- **P4** Evidence tabs make acceptance first-class: Changes and Checks sit at the same level as the file tree, not behind a modal.
- **P5** Execution surfaces (terminal/run) live in a drawer adjacent to evidence, not a separate window.
- **P6** The input bar is the control point (model/effort at the point of intent).
- **P7** Progressive onboarding via empty-state CTAs, not wizards.
- **P8** Quiet status chips everywhere — counts and states, not prose.
- **P9** Tabs for parallel threads within a unit of work.

### 2.2 From the 2025–26 agent-control-plane landscape

- **L1** Convergent IA everywhere: sidebar/inbox of parallel sessions + per-session detail + diff-first review + PR handoff + quiet-by-default notifications (interrupt only on completion, question, approval).
- **L2** Chat-transcript-as-primary-surface is the known failure mode: transcripts hide state and verification. Artifact-first wins.
- **L3** **Review, not generation, is the bottleneck** (PR review time +441% in Faros' data) — spend the pixels on evidence and decisions.
- **L4** Trust surfaces that work: test results, screenshots, session logs linked from commits (Copilot's commit-to-session traceability, Devin's scrubbable timeline, Conductor's Checks tab that blocks merge while blockers are open).
- **L5** Fleet triage is ranked attention, not a table (Codex projects, Copilot mission control, Devin filters).
- **L6** Steering mid-run is queued messages, honestly labeled, not a fake live dialogue.

### 2.3 Emergence rules for deadreckon (where we deliberately diverge from Conductor)

The operator does not chat with the agent as the primary act. The center of gravity is: **(a)** glanceable fleet awareness, **(b)** deep single-job inspection (narrative, turns, spend, flight rewind), **(c)** contract-and-checks as THE promote decision surface, **(d)** safe verbs with honest confirmation semantics, **(e)** trust through receipts. Where Conductor's center pane is a conversation you drive, ours is a **living surface you read** with one narrow control point (the Rudder).

### 2.4 Common mechanics all three concepts share (settled — review once)

1. **Shell:** LSUIElement menubar-first SwiftUI app; lazily-built main window flips activation policy `.accessory↔.regular` (specstory-mac's Granola pattern). Menubar icon: template when idle, colored when jobs run, badged on needs-decision/stale-lease/supervisor-down.
2. **Read path:** FSEvents over `~/.deadreckon` + poll fallback; tail semantics copied from the harness (torn final line ignored and retried; `job-events.jsonl` strict seq 1..N verified continuously and shown as an integrity chip).
3. **Write path:** every mutation shells out to the vendored, sha256-manifest-pinned `deadreckon` binary; the app never writes harness files itself (single-writer discipline). Sheets display the literal CLI line they will run.
4. **Trust rendering:** "VERIFIED" only after fresh `verdict --json` at render time; fail-closed refusals rendered verbatim with try-lines; no override control exists anywhere; provider-refreshed narrative overlay is labeled unverified and **never** appears on a decision surface.
5. **Kill/steer honesty:** kill confirmations state the real mechanics (CancelRequested → SIGTERM group → 2s grace → SIGKILL → supervisor-proven Cancelled) and resolve only on the file-backed terminal event; steer input is truth-gated with the named reason when disabled.
6. **Escape hatch:** "Open in Terminal: `deadreckon attach <id>`" everywhere (apple-events entitlement) — the TUI remains first-class.

---

## 3. Concept A — **The Bridge** (fleet mission control)

> *Ten seconds to know which job needs you — and signed proof in hand before you promote.*

**Thesis:** the fleet is the product. The operator's scarce resource is attention; the home surface is a board of every durable Job split by the only taxonomy a returning operator cares about — needs-decision / underway / moored — with a ranked Decision Queue badged into the menubar. The core loop is *returning-because-it-notified-me*.

### A1 · Home — the fleet board

```
┌──────────────────────────────────────────────────────────────────────────────────────────────┐
│ ⚓ BRIDGE   deadreckon 0.8.2 · supervisor ● running (launchd) · doctor OK    6 jobs  3▶  2⚑  │
├──────────────────────────────────────────────────────────────────────────────────────────────┤
│ DECISION QUEUE (2)                                                       [ranked: needs you] │
│ ┌──────────────────────────────────────────────────────────────────────────────────────────┐ │
│ │ ⚑ a3f9e2  "migrate spend ledger to v2 schema"          VERIFIED · receipt signed         │ │
│ │    5/5 checks ✓ · judge: achieved · net: deny · $4.12/$20.00 · ready 2h14m               │ │
│ │    [ Review & Promote ]  [ Diff result tree ]  [ Abandon ]                               │ │
│ │ ⚠ c81d07  "port TUI spine to plan surface"             PausedAtCap  $25.00/$25.00        │ │
│ │    phase: waiting · lease ● 2s · turn 31 · cli:claude-code                               │ │
│ │    [ Extend cap & resume ]  [ Kill ]  [ Inspect ]                                        │ │
│ └──────────────────────────────────────────────────────────────────────────────────────────┘ │
│ UNDERWAY (3)                                   provider          spend        lease   phase  │
│ ▶ 7be410 "fix provider fallback on 429"        cli:claude-code   $2.31/$10    ● 2s    verify │
│     sandbox-exec · turn 14 · "re-running cargo test; 2 failures left"           _checks      │
│ ▶ e52c9b "chain 2/5: release-trust hardening"  cli:codex-server  $11.80/$40   ● 1s    running│
│     steerable ⌨ · rudder inbox empty · child run r-0644                                      │
│ ▶ f10a33 "campaign: docs sweep"                anthropic         $0.94/$15    ○ 71s ⚠ running│
│     lease stale >60s — watchkeeper recovery queued (1 of max 4)                              │
│ MOORED (1)                                                                                   │
│ ✕ 90ab1e "spike: bwrap on linux runner"  failed · stop_reason: SandboxDenied · $0.42  [why]  │
├──────────────────────────────────────────────────────────────────────────────────────────────┤
│ [+ Start job ⌘N]        filter: all scopes ▾ · sort: attention ▾            Logbook ⌘L  ⌘K   │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** title-bar pills from `--version`, `supervisor status --json`, cached `doctor --json`; counts from scanning `jobs/*/projection.json`. Decision Queue rows: "VERIFIED" is re-confirmed by `verdict <id> --json` before the Promote button enables; `PausedAtCap` from the typed stop_reason + last SpendRecord (`cost_usd` vs `cap_usd`). Underway rows: goal from immutable `job.json`; phase from `projection.json`; lease dot from `lease.json` heartbeat age (○ >60s renders the watchkeeper-recovery warning); provider+sandbox from `launch-plan.json`; the quoted "doing" line is the deterministic NarrativeProjection headline. Buttons dispatch real verbs (`finish`/`kill`/`extend`); `[why]` opens `report <id> --json` (run-view projection schema).

### A2 · Menubar popover — ten-second triage (app may be "closed")

```
┌─ ⚓ 3▶ 2⚑ ─────────────────────────────────┐
│ NEEDS DECISION                             │
│ ⚑ a3f9e2  migrate spend ledger…            │
│     VERIFIED · ready 2h14m    [Promote…]   │
│ ⚠ c81d07  port TUI spine…                  │
│     PausedAtCap $25/$25       [Inspect]    │
│ ────────────────────────────────────────── │
│ UNDERWAY                                   │
│ ▶ 7be410  fix provider fallback   ● 2s     │
│     verifying_checks · $2.31/$10           │
│ ▶ e52c9b  chain 2/5               ● 1s     │
│ ▶ f10a33  docs sweep         ○ 71s stale ⚠ │
│ ────────────────────────────────────────── │
│ supervisor ● running · spend today $19.47  │
│ Open Bridge ⌘O    Start Job ⌘N    Quit ⌘Q  │
└────────────────────────────────────────────┘
```

**Note the deliberate restraint:** `[Promote…]` does **not** promote from the popover — it opens the full window's acceptance sheet, because a destructive verb deserves the whole evidence surface. Notifications that summon this popover come from tailing `notify.jsonl` + phase transitions in `job-events.jsonl` (completion, waiting, PausedAtCap, lease-stale).

### A3 · Job drill-in (Helm view)

```
┌ ◀ Fleet   7be410 "fix provider fallback on 429"          running · verifying_checks · ● 2s   ┐
│ SPINE  alive ✓2s │ doing "re-run cargo test" │ on-track likely │ wrong 2 tests red │ next t15 │
├──────────────────────────────────────────────────┬───────────────────────────────────────────┤
│ NARRATIVE ▾   activity · docs · why · timeline   │ ACCEPTANCE          net: deny     3/5 ✓   │
│ ──────────────────────────────────────────────── │ ✓ build_success  cargo build       8.2s   │
│ 14:02 t14  Router falls through on retryable     │ ✓ file_exists    src/router.rs     0.0s   │
│   429s; added exponential backoff before the     │ ✓ content_match  probe_provider…   0.1s   │
│   fallback chain advances to cli:codex.          │ ✗ cargo_test     router::fallback 41.3s   │
│ 13:58 t13  Reproduced 429 loop in drydock test   │     "2 failed: retry_429, skip_no_cred"   │
│   harness; wrote failing case first.             │ ○ shell          ./scripts/lint.sh  —     │
│ 13:51 t12  Read registry/mod.rs descriptor       │ ─────────────────────────────────────────│
│   chain; confirmed built-in fallback order.      │ SPEND   $2.31 / $10.00 cap   wall 38m     │
│ ──────────────────────────────────────────────── │ ▂▃▃▅▆ loop $2.10 · narrator $0.21         │
│ RUDDER  (steer: unavailable — cli:claude-code)   │ FLIGHT  ⏪ c-07 c-08 [c-09]               │
│ ┆ steer works on cli:codex-server runs only    ┆ │ 41 events · 12 files touched              │
│ ──────────────────────────────────────────────── │ [Preview rewind to c-08]                  │
│ PROCESSES  cargo(2) rustc(1) · pgid 71203        │ LIVE FILES  src/providers/router.rs ●     │
├──────────────────────────────────────────────────┴───────────────────────────────────────────┤
│ [ Kill… ]   [ Open in Terminal: attach 7be410 ]   [ Report ⌘R ]        events 1..N ✓ contig  │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** the SPINE is attach's 5-question status spine (`tui/spine.rs` semantics) recomputed from the same files — parity with the TUI's read-model, not a new invention. Center pane mirrors attach's run-surface tabs; narrative from `narrative/{state.json,snapshots.jsonl}`; activity is a torn-tail-tolerant tail of `events.jsonl` — but with no 1000-event cap, plus search and resizable panes. ACCEPTANCE panel from GateEvaluation results / `acceptance-progress.jsonl` / `proofs/turn-acceptance.json`; "net: deny" is the contract capability compiled into JobGatePolicy. FLIGHT from `flight-manifest.json` + `checkpoints/`; rewind is hash-guarded preview-first. RUDDER is honestly disabled with the reason. `events 1..N ✓ contig` is the app continuously verifying strict sequence — a visible integrity claim.

### A4 · Promote sheet (Binnacle)

```
┌ Promote a3f9e2  "migrate spend ledger to v2 schema"                        VERIFIED by dr-gate┐
│ TWO-KEY COMPLETION                                                                           │
│  KEY 1 ✓ deterministic marker   proofs/turn-acceptance.json · HMAC-SHA-256 v2 · signature ok  │
│         containment ✓ probe-boundary: agent sandbox cannot read gate key (0o600, controller) │
│  KEY 2 ✓ semantic judgment      achieved — "ledger v2 written, migrations pass, docs updated"│
│ ──────────────────────────────────────────────────────────────────────────────────────────── │
│ CONTRACT  acceptance.yaml · frozen sha256 9c41…e7 = authority.json ✓ · net authority: deny   │
│  ✓ cargo_test     spend_v2::roundtrip          must_pass   12.4s                             │
│  ✓ build_success  cargo build --release        must_pass    9.1s                             │
│  ✓ file_exists    docs/schemas/spend-record.schema.json     0.0s                             │
│  ✓ content_match  "total_cost_usd" in schema                0.1s                             │
│  ✓ shell          ./scripts/validate-ledger.sh  must_pass   3.8s   [stdout ▸]                │
│  tamper facts ✓ acceptance-tamper.json clean · progress ledger reconstructed ✓               │
│ ──────────────────────────────────────────────────────────────────────────────────────────── │
│ RECEIPT  receipt.json binds 10 digests: goal ✓ contract ✓ policy ✓ launch-plan ✓ source ✓    │
│          result-tree ✓ marker ✓ judgment ✓ boundary-obs ✓ capture-policy ✓                   │
│ RESULT   142 files staged (capture-policy filtered) · +2,381 −447    [ Diff before promote ] │
│ ──────────────────────────────────────────────────────────────────────────────────────────── │
│ Destination  ( ) apply to worktree  --autostash --cleanup                                    │
│              (•) export to  ~/src/deadreckon-ledger-v2   --dest                              │
│ After: one-command rollback available — deadreckon undo                                      │
│                                        [ Cancel ]        [ Promote (finish a3f9e2) ⏎ ]      │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

The sheet re-runs `verdict --json` at open (never trusts its own cache), renders both keys, the frozen contract sha matched live against `authority.json`, per-check evidence with clipped stdout, tamper facts, the receipt's bound digests, and the staged diff (needs `finish --dry-run --json`, G4). There is deliberately no override control anywhere.

### Bridge — flows in brief

- **Start:** ⌘N Course sheet (form, not wizard) → providers/models from `providers list --json`/`models --json` → contract step (5 check kinds + network capability, draft-then-approve) → cap (> $50 requires typing the amount — the GUI-honest `--i-know-its-a-lot`) → launch-plan preview → `start --plan <file> --yes --json`; new row appears the moment `job.json` lands via FSEvents.
- **Observe:** drill-in = same files attach reads, ~2s cadence matched to the heartbeat.
- **Steer:** enabled only by the harness predicate; queued chip until ack visible in `events.jsonl`.
- **Kill:** confirmation states real semantics; row shows amber "cancel requested" driven by `job-events.jsonl`, never optimistic.
- **Promote:** A4 above; refusals verbatim with try-lines.

**Demands on the binary:** G1, G2, G3 (its 2-second fleet poll is ~6 files × N jobs without the rollup), G4, G5, G6. Completed-run actions ride G1 (verbs already exist).

**Sharpest risks:** Decision-Queue derivation drifting from `verdict` truth (mitigated by re-validation before enabling Promote, fully fixed by G3); heartbeat false alarms across Mac sleep/wake (debounce against boot_id/epoch + supervisor recovery signals — if operators learn to ignore amber, the attention product is dead); missed-notification catch-up on launch must be correct or *return-because-it-notified-me* silently fails; scope gravity toward chat (the Rudder's honest narrowness must be defended).

---

## 4. Concept B — **The Chartroom** (job workbench)

> *Stand over the chart table while the ship sails itself — one job, all its evidence, at arm's reach.*

**Thesis:** the operator's attention belongs to one job at a time, with every piece of evidence within arm's reach. Conductor's three-pane anatomy (P3) and evidence-as-first-class-tabs (P4), but the center pane is not a conversation you drive — it is a living surface you read: narrative projection on top, phase/turn reality beneath, one narrow control point (Rudder) at the bottom (P6), honest that steering is a queued suggestion to a supervised process.

### B1 · Home — fleet sidebar + selected job + evidence rail

```
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│ ⚓ CHARTROOM    fleet: 2 running · 1 waiting · 1 queued · 2 done   [+ Lay Course] [Doctor ✓]    │
├────────────────────┬─────────────────────────────────────────────┬─────────────────────────────┤
│ FLEET              │ webhook-retries-9f3e21         ● running    │ EVIDENCE                    │
│ ▾ itavero/billing  │ "Add durable retry queue to webhook         │ [Contract]│Chg 7│Docs│Flt   │
│  ● webhook-retries │  dispatcher; all delivery tests green"      │─────────────────────────────│
│    verifying_checks│ phase verifying_checks · lease ♥ 1.4s ago   │ acceptance.yaml  (frozen)   │
│  ◐ rate-limit-tests│ spend $4.83 / $25.00 cap · wall 2h 41m      │ contract_sha256 e0c1…94a    │
│    running · ph 20 │─────────────────────────────────────────────│ network: deny  ✓ held       │
│  ◌ invoice-pdf     │ NARRATIVE  snapshot #31 · refreshed 12s ago │─────────────────────────────│
│    queued          │ Dispatcher now enqueues failed deliveries   │ ✓ cargo_test        4.2s    │
│ ▾ specstory-mac    │ into a backed-off retry table; a worker     │   webhook::retry_drain      │
│  ⚑ menubar-badge   │ drains it on a jittered schedule. Currently │ ✓ build_success     38s     │
│    waiting: decide │ re-running the failing integration test     │   cargo build --release     │
│  ✓ granola-window  │ after fixing a race in drain_batch().       │ ✓ content_match     0.1s    │
│    completed       │─────────────────────────────────────────────│   CHANGELOG.md ~ "retry"    │
│  ✗ sparkle-feed    │ SPINE alive ✓ · doing: rerun int test ·     │ ◌ shell (must_pass) 12s…    │
│    failed          │ on-track ✓ · wrong: — · next: gate evaluate │   ./scripts/smoke.sh        │
│                    │─────────────────────────────────────────────│─────────────────────────────│
│ HARBOR             │ TIMELINE 0──10──20──30──40▓──50──60         │ TWO KEYS                    │
│ providers 4/5 ok   │ planned → executing → verifying_checks      │ ⚿ marker: not yet signed    │
│ supervisor ● svc   │─────────────────────────────────────────────│ ⚖ judge: pending            │
│ home 412 MB        │ ACTIVITY (live tail)                        │─────────────────────────────│
│                    │ 14:02:11 tool  cargo test webhook::retry    │ ▸ Drawer: Terminal · Events │
│                    │ 14:02:04 edit  src/dispatch/drain.rs        │                             │
│                    │ 14:01:58 steer ack "prefer sqlx not diesel" │ [ Kill ]  [ Promote… ]      │
│                    │─────────────────────────────────────────────│  promote disabled:          │
│                    │ ⟩ RUDDER  codex-server · Executing · ok     │  waiting on 2 keys          │
│                    │ │ cap exponential backoff at 5 minutes▌     │                             │
├────────────────────┴─────────────────────────────────────────────┴─────────────────────────────┤
│ latest: webhook-retries-9f3e21 · verifying_checks · next_action: dr-gate evaluate → sign       │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** FLEET rows from `jobs/<id>/job.json` grouped by source project (authority.json source digest → repo path), glyphs from `projection.json` phase + stop_reason translated through **glossary.rs user words only** (never raw enum names). Selected-job header: lease ♥ age from `lease.json`; spend from `spend.jsonl`. NARRATIVE with honest staleness. TWO KEYS chip = marker + judgment presence; **Promote stays disabled until both, with the missing key named**. HARBOR: `providers list --json` probe results + `supervisor status --json`. Bottom bar: the JSON envelope's `next_actions`, kept to one verdict + one primary action (friendliness contract).

### B2 · Deep-dive — turns, spend, flight recorder, drawer

```
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│ ⚓ CHARTROOM   ◂ fleet    webhook-retries-9f3e21 · run r-20260806-0412 · cli:codex-server       │
├────────────────────┬─────────────────────────────────────────────┬─────────────────────────────┤
│ FLEET              │ TURNS (traces.jsonl · 42 turns)             │ Contract│Chg 7│Docs│[Flt]   │
│ ▾ itavero/billing  │ ▾ turn 38  14:01:12  in 3.1k out 1.8k tok   │─────────────────────────────│
│  ● webhook-retries │    plan: fix race in drain_batch, rerun     │ FLIGHT RECORDER             │
│    verifying_checks│    tool bash: cargo test webhook::retry     │ flight-events.jsonl seq 214 │
│  ◐ rate-limit-tests│      exit 101 · 2 failed                    │                             │
│    running · ph 20 │    tool edit: src/dispatch/drain.rs +14 -3  │ ckpt-041  14:01  anchor     │
│  ◌ invoice-pdf     │ ▾ turn 39  14:02:04                         │  edit drain.rs · 3 files    │
│    queued          │    tool edit: src/dispatch/drain.rs +2 -2   │ ckpt-042  14:02  delta      │
│ ▾ specstory-mac    │    tool bash: cargo test webhook::retry     │  test rerun · usage 4.9k    │
│  ⚑ menubar-badge   │      running… 22s                           │►ckpt-043  14:02  delta      │
│    waiting: decide │ ▸ turn 37  13:58:40  (collapsed)            │  HEAD · live                │
│  ✓ granola-window  │ ▸ turn 36  13:55:02                         │                             │
│  ✗ sparkle-feed    │─────────────────────────────────────────────│ [Preview rewind to 041]     │
│                    │ SPEND (spend.jsonl)                         │  hash-guarded · preview     │
│ HARBOR             │ loop $4.61 · narrator $0.22 · cap $25.00    │  before apply, undoable     │
│ providers 4/5 ok   │ ▂▃▃▅▆█ last 6 turns · burn $1.71/h          │─────────────────────────────│
│ supervisor ● svc   │─────────────────────────────────────────────│ PROVENANCE                  │
│                    │ ⟩ RUDDER  codex-server · Executing · ok     │ source tree b41c…e2 frozen  │
│                    │ │ ▌                                       │ │ authority.json ✓ matches    │
├────────────────────┴─────────────────────────────────────────────┴─────────────────────────────┤
│ DRAWER  [Terminal]│ Raw events │ Job events                                          ▼ close   │
│ $ tail supervisor.out                                                                          │
│ [supervisor] lease renewed epoch 7 · heartbeat ok · child pid 48112 alive                      │
│ [gate] evaluate deferred: run still Executing (checks run in sandbox at verify)                │
│ 14:02:26 event seq 1287 ToolResult cargo test … 2 passed 0 failed  ← events.jsonl              │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ r-20260806-0412 · Executing · steerable ✓ · rewind ready: 3 checkpoints                        │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** TURNS grouped from `traces.jsonl` with tool calls interleaved from `events.jsonl` by seq — collapsible like Claude's view modes, **but sourced from ledgers, not a chat buffer** (L2), with unbounded scrollback. FLIGHT tab from `flight-manifest.json`/`flight-events.jsonl`/`checkpoints/`; rewind is `rewind --preview --json` first. PROVENANCE cross-checks source-tree digest against `authority.json` — mismatch renders fail-closed red. DRAWER (P5): `supervisor.out/err`, raw `events.jsonl`, `job-events.jsonl` with a torn-tail badge.

### B3 · The Binnacle (promote sheet)

```
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│ ⚓ PROMOTE — webhook-retries-9f3e21                    "verified by dr-gate" · VERIFIED         │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ TWO-KEY COMPLETION (Binnacle)                                                                  │
│  ⚿ Key 1  Deterministic marker   proofs/turn-acceptance.json   HMAC-SHA-256 v2  ✓ VALID        │
│     signed by dr-gate sign (childless, key 0o600 gate-keys/, agent sandbox cannot read)        │
│     tamper facts: acceptance-tamper.json ✓ clean · containment ✓ probe-boundary held           │
│  ⚖ Key 2  Semantic judgment      read-only judge → achieved                                    │
│     "Retry queue delivers all failed webhooks within backoff caps; tests demonstrate it."      │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ CHECKS (from GateEvaluation, run inside sandbox-exec · network: deny)                          │
│  ✓ cargo_test     webhook::retry_drain          must_pass   4.2s   14 passed 0 failed          │
│  ✓ build_success  cargo build --release         must_pass  38.0s   exit 0                      │
│  ✓ content_match  CHANGELOG.md =~ "retry"       must_pass   0.1s   1 match                     │
│  ✓ shell          ./scripts/smoke.sh            must_pass  41.7s   exit 0  ▸ stdout (clipped)  │
│  ✓ file_exists    migrations/0007_retry.sql     advisory    0.0s   present                     │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ RECEIPT  receipt.json — binds this exact result (10 digests)                                   │
│  goal 1f9a…  contract e0c1…  policy 77b2…  launch-plan 03dd…  source-tree b41c…                │
│  result-tree 5a6e…  marker 9c02…  judgment 4e11…  boundary-obs a807…  authority c3f0…          │
│  lease epoch 7 · owner watchkeeper svc · signed 2026-08-06 14:09:12Z                           │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ CANDIDATE  capture-policy filtered · 7 files staged                        [View Changes ▸]    │
│  src/dispatch/drain.rs +89 -12 · src/dispatch/retry.rs +214 (new) · tests/… (+3 more)          │
│                                                                                                │
│  Destination   (•) Apply to working tree  ~/itavero/billing   --autostash --cleanup            │
│                ( ) Export to directory    --dest ~/reviews/webhook-retries                     │
│                                                                                                │
│  After promote: revalidates before AND after atomic rename · rollback = `deadreckon undo`      │
│                                                                                                │
│                                   [ Cancel ]        [ Promote — finish 9f3e21 ✓ ]              │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ if any digest drifts this sheet fails closed — no operator override exists, by design          │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

The judge's reason is quoted verbatim, never paraphrased. Check rows show the *how* (sandbox, network authority actually granted), not just the *what*. Digest chips reveal what they hash on click.

### B4 · Lay Course sheet (guided start without TTY pickers)

```
┌────────────────────────────────────────────────────────────────────────────────────────────────┐
│ ⚓ LAY COURSE — new durable Job in itavero/billing                                              │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ GOAL                                                                                           │
│ │ Add a durable retry queue to the webhook dispatcher; failed deliveries must retry with     │ │
│ │ exponential backoff and all delivery tests must pass.▌                                     │ │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ROUTE (Pennant)                                  LIMITS                                        │
│  provider  [cli:codex-server ▾]  ● ok, steerable  spend cap  [$25.00]  (>$50 needs typed ack)  │
│  model     [gpt-5.3-codex    ▾]  subscription     wall clock [8h ▾]   attempts [3]             │
│  fallback  cli:claude-code → anthropic            deadline   [tomorrow 09:00 ▾]                │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ DONE CONTRACT (Soundings)          [Detect from repo]  [Draft with LLM — you approve]          │
│  acceptance.yaml                                                                               │
│   cargo_test:     webhook::retry_drain            must_pass                                    │
│   build_success:  cargo build --release           must_pass                                    │
│   shell:          ./scripts/smoke.sh              must_pass                                    │
│   capabilities:   network: deny        ⚠ smoke.sh curls localhost → suggest: loopback          │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ LAUNCH PLAN PREVIEW (nothing has run yet)                                                      │
│  will freeze goal+contract+policy → authority.json (sha256) · queue Job jobs/9f3e21/           │
│  detach via `supervisor serve --once 9f3e21` · sandbox sandbox-exec · source tree frozen       │
│                                                                                                │
│                       [ Cancel ]   [ Edit yaml ]   [ ⚓ Start — queues Job, detaches ]          │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ equivalent: deadreckon start "Add a durable retry queue…" --provider cli:codex-server \        │
│   --cap 25 --contract acceptance.yaml --yes --json                                             │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Notable: admission lint (declared network authority vs what checks need, the `e5d7f1d` fail-closed rule) surfaces **before** freeze with a one-click fix; an LLM-drafted contract is marked DRAFT and cannot freeze without explicit approval; every sheet shows the equivalent CLI line — the app stays an honest skin over the binary.

### Chartroom — flows in brief

- **Observe:** selecting a job **is** attach — no mode switch; the fleet sidebar stays live (fixing one-artifact-per-session); notifications quiet-by-default on stop_reason transitions and two-key completion.
- **Steer:** truth-gated bar; queued → ack lifecycle rendered from the files; unacked-after-N-turns gets a quiet amber chip, not a modal.
- **Kill:** dispatch then *watch* `job-events.jsonl` for terminal Cancelled; spend meter freezes at final cost.
- **Promote:** disabled-until-two-keys with the missing key named; streams finish's narration (validated → staged → revalidated → renamed → revalidated) into the job timeline.

**Demands on the binary:** G2, G1, G4, G10 (its `Chg 7` tab), G6, G5 (its top structural want: the merged follow stream — otherwise Swift reimplements six JSONL tailers per job). Its "resume picker" ask is **withdrawn** per §1.2 (public resume retired; render supervisor status + lease freshness instead — G3 covers it).

**Sharpest risks:** reimplementing ledger semantics in Swift (torn-tail/strict-seq drift misrenders truth — the worst failure for an evidence-first UI; mitigation: G5, and treat any parse anomaly as "unknown", never a guessed state); fusing two state layers into one glyph (derive fleet rows **only** from projection.json + glossary words); the Rudder reading as broken when most routes aren't steerable (collapse to one quiet line); binary drift (manifest-pinned vendored CLI + `doctor --json` schema-version handshake at startup).

---

## 5. Concept C — **The Quarterdeck** (acceptance gate)

> *The acceptance gate is the app: every job queues for your judgment, and nothing promotes past you but signed, file-backed evidence.*

**Thesis:** every agent-supervision UI on the market is organized around the conversation or the fleet. Quarterdeck is organized around the **decision**. The harness already builds the evidence chain for exactly that moment — frozen contract, contained keyless evaluation, unforgeable marker, read-only judge, signed receipt. The CLI makes you assemble that picture from `status`/`verdict`/`report`/`attach`; Quarterdeck renders it as one screen with one decision bar. The queue's sort key is pure file-backed fact: receipt validity, projection phase, last gate attempt's pass count, lease heartbeat. Nothing in the queue is model prose.

### C1 · Home — the Gate Queue

```
┌─ QUARTERDECK ── Gate Queue ─────────────────── 7 jobs · deadreckon 0.8.2 ── supervisor ● ──┐
│ [⌘N Start voyage]  [⌘F Filter]          watchkeeper: serving (launchd singleton, pid 812)  │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ ▼ AT THE GATE — awaiting your decision (2)                                                 │
│  ┌──────────────────────────────────────────────────────────────────────────────────────┐  │
│  │ ● harden-provider-fallback   job-9f2c31   cli:claude-code/sonnet    6h14m    $9.12   │  │
│  │   VERIFIED · 5/5 checks · judge: achieved · receipt signed 07:42  [Review at Gate ⏎] │  │
│  ├──────────────────────────────────────────────────────────────────────────────────────┤  │
│  │ ● jsonl-torn-tail-fuzzing    job-4be089   cli:codex/o4-mini        2h03m    $3.87   │  │
│  │   checks 3/3 · judge: UNCERTAIN — read judgment before promoting  [Review at Gate ⏎] │  │
│  └──────────────────────────────────────────────────────────────────────────────────────┘  │
│ ▼ APPROACHING THE GATE (1)                                                                 │
│   ◐ narrative-refresh-cap      job-77d1a0   verifying_checks · attempt 3 · cargo_test …    │
│     2/5 checks green last attempt · $14.20 of $25.00 cap · lease hb 1.4s ago               │
│ ▼ UNDERWAY (3)                                                                             │
│   ◔ campaign: docs-overhaul    job-b33f10   running · phase 40 · $22.10/$50 · 12 children  │
│   ◔ steer-backpressure         job-c9d422   running · Executing · steerable (codex-server) │
│   ◑ flaky-drydock-quarantine   job-e01f55   waiting · PausedAtCap $25.00 — raise or kill   │
│ ▼ WRECKED / BLOCKED (1)                                                                    │
│   ✗ sandbox-probe-refactor     job-a208dd   terminal · Blocked/LostContainment (orphaned)  │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│ menubar mirror: "2 at the gate · 3 underway"                                               │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** a job ranks AT THE GATE when `projection.json` shows terminal+succeeded AND `receipt.json` validates; judge verdict from the semantic-judgment record (`achieved|revise|uncertain` — UNCERTAIN is ranked lower and painted amber, never hidden). APPROACHING's "2/5 green last attempt" needs the last GateEvaluation summary in `projection.json` (**G8** — today it exists only on dr-gate stdout). WRECKED = typed stop_reason `Blocked/LostContainment` (fail-closed orphan).

### C2 · Gate Review — the promote surface

```
┌─ Gate Review — harden-provider-fallback (job-9f2c31) ─────────── terminal · succeeded ─────┐
│ GOAL  "Make ProviderRouter fall back past credential-less routes without retry storms"     │
│ CONTRACT acceptance.yaml · sha256 e3b0c4…934c ✓ matches authority.json · frozen 01:28      │
│ NETWORK AUTHORITY: deny — boundary observation: 0 egress attempts ✓                        │
├─ CHECKS — dr-gate evaluate · attempt 4 · contained (sandbox-exec) ─────────────────────────┤
│ ✓ cargo_test      must_pass   212 passed / 0 failed                 41.3s   [▸ evidence]   │
│ ✓ build_success   must_pass   cargo build --release · exit 0        88.1s   [▸ evidence]   │
│ ▾ content_match   must_pass   "fallback" present in router.rs        0.1s                  │
│     cmd rg -n "fallback" crates/deadreckon/src/registry/router.rs      cwd /work/src       │
│     stdout │ 214: for route in self.fallback_chain() {          (clipped · 12 lines)       │
│ ✓ shell           must_pass   ./scripts/conformance.sh · exit 0     12.9s   [▸ evidence]   │
│ ✓ file_exists     must_pass   docs/PROVIDER-FALLBACK.md              0.0s                  │
├─ TWO-KEY COMPLETION ───────────────────────────────────────────────────────────────────────┤
│ KEY 1 ✓ deterministic marker · proofs/turn-acceptance.json · HMAC-SHA-256 v2 · sig ok      │
│         contained: true · tamper facts: none · gate key held by controller only            │
│ KEY 2 ✓ semantic judgment: ACHIEVED — "Fallback chain now skips credential-less…"          │
│ RECEIPT ✓ receipt.json valid — 10 digests bound                    [▸ digest table]        │
│   goal ✓ contract ✓ policy ✓ launch-plan ✓ source-tree ✓ result-tree ✓ marker ✓ judge ✓ …  │
├─ RESULT ───────────────────────────────────────────────────────────────────────────────────┤
│ Δ 7 files · +312 −64  [▸ diff]      spend $9.12 / $25.00 · wall 6h14m · 4 attempts         │
├─ DECISION ─────────────────────────────────────────────────────────────────────────────────┤
│ [⏎ PROMOTE → finish --dest ~/src/deadreckon]  [S Send back + note]  [K Kill — discard]     │
│  publish + cleanup are irreversible · applied files are undoable via `deadreckon undo`     │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Data provenance:** the app **recomputes** the contract sha over the frozen `acceptance.yaml` and matches it against `authority.json` — it never trusts a label. Network band pairs declared authority with the observed egress from the boundary observation bound in the receipt. `[▸ digest table]` needs per-digest audit facts (**G7**). The three-button decision bar is the whole app in one line: promote / send back with a recorded note (**G9**) / kill.

### C3 · Approaching the Gate — running-job inspection

```
┌─ Approaching the Gate — narrative-refresh-cap (job-77d1a0) ──── verifying_checks ──────────┐
│ lease hb 1.4s ago · owner supervisor@mbp epoch 7 · attempt 3 · spend $14.20 / $25.00       │
├─ SPINE ────────────────────────────────────────────────────────────────────────────────────┤
│ alive? yes · doing? re-running cargo_test after fix · on-track? 2 tests still red          │
│ wrong? tokio time mocking flaky · next: gate attempt 4 after loop turn completes           │
├─ NARRATIVE (projection from run files · overlay unverified) ─────┬─ CONTRACT RAIL ─────────┤
│ 09:02 isolated 45s cadence race in narrative/refresh.rs          │ ✓ file_exists           │
│ 09:14 patched cap check; 2 tests red (tokio clock)               │ ✓ content_match         │
│ 09:31 gate attempt 3: cargo_test FAILED 2/214 → loop resumed     │ ✗ cargo_test  2 red     │
│ 09:40 rewriting quiet-cadence test with paused clock             │ ◌ build_success  queued │
│                                                                  │ ◌ shell          queued │
├─ TIMELINE events.jsonl seq 1..1841 ──────────────────────────────┴─────────────────────────┤
│ ▂▃▅▇▅▃▂▁▂▅▇ phases 0→40 · last event 4s ago    [▸ traces] [▸ spend] [▸ flight rewind]      │
├─ ACTIONS ──────────────────────────────────────────────────────────────────────────────────┤
│ [T Steer — unavailable: provider cli:claude-code, not Executing]      [K Kill…]            │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

**The hard rule this screen encodes:** the provider-refreshed narrative overlay is labeled *unverified* here and is **never shown on the Gate Review surface** — model prose must not launder into the trust surface. The CONTRACT RAIL (frozen checks × last gate attempt results) needs **G8**.

### C4 · Promote sheet — preview-before-mutate with honest irreversibility

```
        ┌─ Promote harden-provider-fallback ── preview before mutate ──────────────┐
        │ $ deadreckon finish job-9f2c31 --dest ~/src/deadreckon                   │
        ├──────────────────────────────────────────────────────────────────────────┤
        │ 1 revalidate receipt (strict) ........... ✓ signature + 10 digests       │
        │ 2 stage candidate (capture-policy) ...... 7 files · 312 KB  [▸ list]     │
        │ 3 revalidate staged tree ................ runs at promote time           │
        │ 4 atomic rename into library ............ IRREVERSIBLE                   │
        │ 5 apply to ~/src/deadreckon ............. undoable (`deadreckon undo`)   │
        │ 6 working-dir cleanup ................... skipped (--cleanup not set)    │
        │                                                                          │
        │ any tamper or drift fails closed — there is no operator override         │
        ├──────────────────────────────────────────────────────────────────────────┤
        │        [ Cancel esc ]                  [ Promote ⏎ — runs finish ]       │
        └──────────────────────────────────────────────────────────────────────────┘
```

Six pipeline steps with per-step irreversibility labels; the literal CLI line on top; the sheet promises nothing the binary won't enforce.

### Quarterdeck — flows in brief

- **Start:** "the gate is declared at the start gun" — the contract step is mandatory and central, because the home screen is useless for a job without a contract. Admission lint before freeze.
- **Observe:** the queue's whole point is telling you when **no** decision is needed yet (the day-in-the-life leaves a mid-loop job alone).
- **Send back:** for terminal jobs, `extend` with the operator's note recorded in provenance (G9) — *not promoted, not killed, sent back with a receipt of why*. The next agentic turn can read it.
- **Promote:** read-only validation happens before the sheet ever opens; anything amiss renders the decision bar with Promote disabled and the failing fact named.

**Demands on the binary:** G2, G1, G4, G7 (digest table), G8 (queue ranking + contract rail), G9 (send-back), G3 (queue scan without opening five files per job).

**Sharpest risks:** the send-back button outruns the binary on most providers (mitigation: G9 is small; grey-out with exact reason meanwhile); ranking heuristics must stay on durable facts only — any forward-looking "ready in ~18m" estimate is visually quarantined; **narrative laundering** (one careless layout choice putting model prose on Gate Review breaks the whole trust posture); the app runs as the controller user, so `gate-keys/` is *readable* by it — the app must never read, display, or export gate keys, and no file-browser feature may casually open that directory.

---

## 6. Comparison & recommendation

### 6.1 What actually differs

| | **Bridge** | **Chartroom** | **Quarterdeck** |
|---|---|---|---|
| Home surface | Fleet board + Decision Queue | Selected job's workbench (fleet in sidebar) | Decision-ranked Gate Queue |
| Hero moment | Returning on a notification | Standing over one running job | The promote decision |
| Operator verb it optimizes | *triage* | *attach/inspect* | *promote/finalize* |
| Fleet at a glance | ★★★ | ★★ (sidebar only) | ★★★ (ranked) |
| Single-job inspection depth | ★★ | ★★★ (turns, flight, drawer) | ★★ (evidence-focused) |
| Contract/checks prominence | panel in drill-in | evidence-rail tab | the entire app |
| Menubar presence | ★★★ (popover is a product) | ★ | ★★ (mirror counts) |
| Failure mode if pushed alone | shallow inspection | weak triage on return | thin mid-run observability |

All three share the chassis (§2.4), the promote sheet (converged to near-identical Binnacle designs), truth-gated steer, honest kill, and the escape hatch to `attach`.

### 6.2 Recommendation: one app, composed

**"Bridge shell, Quarterdeck home, Chartroom depth."**

1. **Shell & presence — Bridge:** LSUIElement menubar app with the popover and the notification catch-up discipline. For durable overnight jobs, the return moment is the product; the popover is triage-only (no destructive verbs).
2. **Home — Quarterdeck's queue:** AT THE GATE / APPROACHING / UNDERWAY / WRECKED is Bridge's board made *honest* — ranking on pure file-backed facts (receipt validity, phase, last-attempt pass count, heartbeat) rather than an app-side heuristic. Adopt Bridge's column discipline (provider/spend/lease/phase) inside the rows.
3. **Drill-in — Chartroom's three-pane workbench:** fleet sidebar stays live; center = narrative/spine/timeline/activity/turns; right = evidence rail (**Contract & Checks / Changes / Docs / Flight**); bottom drawer (Terminal / raw events / job events). This is where the stated priority — *operator inspection and understanding of running jobs, done contracts and their checks* — lives.
4. **Promote — the converged Binnacle sheet** with Quarterdeck's decision bar (Promote / **Send back + note** / Kill) and per-digest table (G7), preview-before-mutate (G4), no override.
5. **v1 scope:** run + durable-Job surfaces first. Plan/chain/campaign render as rows with child counts and open in `attach` (escape hatch) until a voyage-tree surface earns its way in (v1.x).

Why not pure Chartroom as home: with detached overnight jobs, you arrive to *decide*, and inspection is one click deep. Why not pure Bridge: its Decision Queue is the same surface as Quarterdeck's, minus the honesty of receipt-fact ranking. Why not pure Quarterdeck: mid-run inspection (your priority) needs Chartroom's depth anyway.

**Naming:** the app deserves a milestone name in the nautical registry. *Bridge* (the station the operator commands from) is the natural candidate for the app itself; screens keep their existing feature names (Course sheet, Helm view, Rudder, Binnacle, Gate Queue, Logbook, Harbor). Open question §10.

---

## 7. What the Rust binary must grow (verified gap register)

Every "today" claim below was verified against source by the gap pass. Guiding rules: **one shared machine surface** that CLI, TUI, and app all ride (no app-private side doors); **promotion authority stays exactly where it is** — the app never gains a path around `dr-gate`.

| ID | Gap | Effort | Powers |
|---|---|---|---|
| **G1** | JSON result **and error** envelopes on the 9 state-changing verbs | M | every mutation in every design |
| **G2** | Non-interactive `start` completion (small residue; most exists) | S | all launch sheets |
| **G3** | Fleet rollup: enrich `list --json` rows | M | fleet board / queue / menubar in one read |
| **G4** | `finish --dry-run --json` (validate + stage to scratch + manifest, discard) | M | preview-before-mutate on promote sheets |
| **G5** | Tailing contract doc + conformance test (S), then `follow <id> --json` merged NDJSON stream (L) | S+L | live panes without reimplementing six tailers in Swift |
| **G6** | Steerability as data: `steerable{steerable,reason}` in status/show/run-view | S | honest Rudder enable/disable |
| **G7** | Per-digest receipt audit: `verdict <id> --receipt --json` | M | Gate Review digest table; "which digest broke" errors |
| **G8** | Live gate progress on disk + `last_gate_attempt` stamp in projection.json | S | live check bands; queue ranking |
| **G9** | `extend --note --json` (typed send-back with provenance record) | S | Gate Review's middle button |
| **G10** | `show --diff --json --patch [--file]` per-file unified diffs | M | Changes tab; diff-before-promote |

### G1 — JSON envelopes on state-changers (M)

**Today:** `finish`/`apply`/`kill`/`steer`/`extend`/`fork`/`merge`/`materialize`/`abandon` have no `--json` (cli.rs:1517–1850 area); outcomes print prose; errors always render prose to stderr (`print_error`, main.rs:746–755); exit codes 0/1/2/130 are the only machine signal. The envelope machinery already exists on inspection surfaces (`VerdictSurface::add_to_json` → `{kind,id,status,next_actions,try_lines,paths}`).
**Proposal:** add `--json` to the nine verbs emitting verb-specific outcome objects (`kill: {signal,escalated,terminal_phase}`, `steer: {queued_at,inbox_seq}`, …), and — the single highest-leverage change — one global rule in `main()`: when the parsed subcommand carried `--json`, `print_error` emits `{"kind":"error","code","verb","message","try_lines"}` on stdout before exiting with the same code. That converts **every existing fail-closed refusal** into a machine refusal envelope in one move. Route all nine through `VerdictSurface` so CLI text and app JSON come from one surface object.
**As built (M1), for decoder authors:** all nine verbs take `--json`; dispatch arms a process-global flag (`machine_json`, same pattern as `ui::set_plain_output`) so prose without `--json` stays byte-identical. **Error envelope:** one pretty-printed `{"kind":"error","code":<exit code>,"verb","message","try_lines":[…]}` object on **stdout**; the historical prose still goes to stderr and the exit code is unchanged (0/1/2/130 semantics preserved; a Ctrl-C abort still exits 130 with no envelope). Every `try: …` line `user_error` packs into a message is lifted out of `message` into `try_lines`. **Success envelope:** `{kind:<verb>,id,status,next_actions,try_lines,…verb facts…,primary_action,verdict}` — `status` is the `VerdictSurface` kind (`completed`/`killed`/`no-op`/…), `verdict`/`primary_action` come from the same surface object that renders the prose. Verb facts: `kill` → `{signal:"SIGTERM"|"SIGKILL",escalated,terminal_phase_observed}`; `steer` → `{queued_at,inbox_seq,source,delivery}`; `finish`/`materialize` export → `{destination:{kind:"export"|"in-place"|"git-branch",…},staged_file_count,receipt_validated}`; `apply` adds `{strategy,cleaned,already_applied}`; honesty note on `receipt_validated`: it means "this outcome was Job-backed and passed through the verified-delivery authority path" — it is DERIVED (job.json presence keyed by run id on export; a resolved Job on finish-in-place; a verified Job on apply), not a fresh receipt/signature re-validation at print time, and it reads `false` for a Job whose job id differs from the run id; treat `verdict --receipt` as the only receipt re-validation surface; `abandon` → `{removed,kept_branch,worktree_found}`; `merge` → `{result_run_id,artifact_library,tasks_completed,tasks_total}`. Verbs that queue a durable Job (`extend`, `fork`, and `kill` of a Job-owned target) emit the Job-status payload fields (`verified_proof`, `paths`, `work_clock`, `job`) with `kind` set to the verb, plus `{queued:true,parent_run_id}` for extend and the kill facts for cancel. One caveat: `kill --json` of a **campaign** cascades into sub-plan kills, so stdout carries one envelope per killed sub-plan followed by the campaign envelope — parse as concatenated objects, not a single document. One carve-out: **argument-parse failures** (clap usage errors, e.g. an extra positional) exit 2 with prose only and no envelope — the envelope contract begins after a successful parse, when the parsed subcommand's `--json` arms the machine rule.

### G2 — Non-interactive start (S — most already exists)

**Today (better than assumed):** `start --plan <file>` replays without prompting; `start --json` emits a read-only preview with `will_start:false`; prompts are already gated off for json/plain/quiet/yes/non-TTY; `--yes` fails closed on a missing contract with try_lines. Real residue: (a) the >$50 acknowledgment is `run`-only — `start` hardcodes `i_know_its_a_lot:false` (start.rs:2814), so a GUI cannot launch a >$50 job; (b) `--yes` vs `--no-confirm` is genuinely split across verbs with no documented contract; (c) refusals are prose (fixed by G1).
**Proposal:** add `--i-know-its-a-lot` to Start; document the confirmation contract (`--yes` = approve the launch preview; `--no-confirm` = skip destructive follow-up confirmations) adding `--yes` as an alias where only `--no-confirm` exists; declare **`start --json` (preview) → inspect → `start --plan <file> --yes --json` (execute)** as the supported GUI launch protocol, with the preview envelope embedding the exact replayable launch-plan payload.
**As built (M1), for GUI authors:** shipped as proposed, with the details a decoder needs. `--i-know-its-a-lot` exists on `start` (and its `--plan` replay) and is enforced at the launch boundary — after read-only previews return, before any service install or Job creation — with `run` parity: a >$50 ceiling in a script refuses (`--yes` never acknowledges spend; a TTY prompts). The confirmation contract lives once in HOWTO.md ("Confirmation flags and the machine launch protocol"); `finish`/`apply`/`cleanup`/`undo` now accept `--yes` as a visible alias for `--no-confirm` (nothing renamed). The supported GUI launch protocol is **`start --json` (preview) → write the envelope's `launch_plan` field to disk → `start --plan <file> --yes --json` (execute)**: a launchable preview embeds `launch_plan`, the exact `launch-plan.json` payload `--plan` accepts (`accepted_by` reads `"preview"` until replay stamps `"replay"`; blocked previews omit the field), and the replay execute leg emits the same `{"kind":"launch",…}` envelope as a direct `start … --yes --json`. `start --json` is armed for the G1 global rule, so every start refusal is also a `{"kind":"error",…}` envelope on stdout with the exit code preserved.

### G3 — Fleet rollup in `list --json` (M)

**Today:** rows carry only job_id/scope/goal/status/updated_at/attempts/outcome/stop_reason (inspection.rs:86–99), though `list_command` already loads full JobViews. A poller opens ~6 files per job per tick.
**Proposal:** enrich each row: `projection{phase,outcome,stop_reason,attempt_count,caveats}`, `lease{owner_id,epoch,heartbeat_age_seconds,fresh}`, `spend{total_cost_usd,cap_usd,subscription,wall_time_seconds}`, `provider`, `sandbox`, `receipt{present,verified,error}`, `gate{attempt,n_passed,n_total}` (rides G8). Keep the verb; no new `fleet` command.
**As built (M0), for decoder authors:** `receipt.verified` is NOT a boolean — it is the shared proof-status enum `"valid"|"invalid"|"not-applicable"` (the same classifier `status --json` uses, `job_proof_status`), so model it as a string. `lease` additionally carries `expires_at` (RFC 3339) beyond the proposed fields. `gate` is present only when a signature-verified acceptance marker exists for the run: failed gate attempts carry **no counts** (the only durable per-check evidence is the signed marker; raw progress rows are untrusted display data — see G8), so rank failed attempts on phase/stop_reason, not counts.

**As built (APP-2 follow-up), glossary user-word labels:** `job_phase_label`/`job_outcome_label`/`stop_reason_label` now live in glossary.rs (deadreckon-core, closing the CONTRACTS.md gap GlossaryText.swift tracks — the app mirrors these tables and any drift is an app bug). Machine surfaces carry them as three **additive** fields next to the serialized enums, `null` when the underlying value is absent (matching the row's null convention): (a) `list --json` **job rows** get top-level `phase_label` (always a word), `outcome_label`, `stop_reason_label` — the serialized `projection{…}` block is unchanged; (b) `list --json` **legacy run rows** get the same three fields, derived through the sanctioned legacy tables (`legacy_run_status_phase` + `legacy_outcome_and_stop_reason`, the exact mapping `legacy_run_job_view` uses), so a legacy row can only ever claim `failed` / `legacy (unknown reason)`, never a guessed cause; (c) `status --json` has no hand-built projection block (the projection rides serialized inside `job`), so the same three labels sit top-level next to `status` — and because that payload doubles as the G1 Job-backed verb envelope, `extend`/`fork`/Job-`kill` envelopes carry them too. Labels are display words only: `status` stays the serialized `job_status_label` (including `verified_proof_invalid`, which has no glossary label — it is a proof classification, not a lifecycle word), and decoders must keep keying logic on the serialized enums. A label can never contradict `status`: when the sealed receipt fails validation (`status:"verified_proof_invalid"`), `outcome_label` and `stop_reason_label` are **withheld (null)** even though the serialized `outcome`/`stop_reason` still read `verified` — the projection is immutable lifecycle fact, but the display word "verified" is reserved for a proof that validates now (shared helper `job_glossary_labels`, job.rs).

### G4 — `finish --dry-run --json` (M)

**Today:** no preview form; but the pieces are already factored: strict receipt validation is a pure read (`validate_completion_receipt`, completion.rs:301–422), staging is separated from publish (`prepare_candidate`/`validate_strict_candidate`/`publish_candidate`, promotion.rs), diffstat exists (`DiffSummary`).
**Proposal:** acquire the operation lock, validate, stage into a scratch dir, walk it for `{path,bytes,sha256}` + diffstat, delete the scratch, emit `{"kind":"finish_plan",…,"irreversible_steps":["publish","cleanup"]}`. **Trust note:** report-only — real finish re-validates and re-stages from scratch; the app gains preview without any promotion shortcut.
**As built (M2), for decoder authors:** shipped as `deadreckon finish <id> --dry-run [--json]` with the contract shape `{"kind":"finish_plan","id","status":"deliverable"|"blocked","receipt":{"validated","error"},"mode","destination","staged":[{"path","bytes","sha256"}],"diffstat":{"files_changed","added","removed"},"result_tree_sha256","irreversible_steps":["publish","cleanup"],"next_actions"}` (core: `stage_promotion_preview`, promotion.rs; CLI: `build_finish_plan`, lifecycle.rs). Deviations and decoder-load-bearing details:
- **Refusal split (the "cleanest split" decision):** `status:"blocked"` (exit 0, plan on stdout) is reserved for **completion-proof, staging, and delivery-readiness failures the real finish would refuse** — receipt tamper/digest mismatch/uncontained receipt, capture-policy or scratch-staging refusals, a non-empty export destination without `--overwrite` (same message as the real export; `--overwrite` lifts it here too), and the apply-mode pre-flight blockers below — with `receipt.error` carrying the exact fail-closed message. Everything that means "there is no plan to report" stays a **hard refusal** with the G1 `{"kind":"error",…}` envelope and the historical exit code: unknown/ambiguous id, wrong ref kind, run not Completed (still-executing / failed / killed), Job not Verified, driver-fence refusals, a **held operation lock** (byte-identical to the real verbs' locked refusal), missing library for an export-mode run, a destination inside the DeadReckon home, and a worktree run whose codebase record has no recoverable source root (`missing source_git_root` — a `"kind":"apply"` destination never carries a null `path`). So a blocked plan means "this exact target refuses delivery and here is why", never "the preview could not run".
- `receipt.validated` reports the completion-proof validation, keyed exactly the way promotion keys it: a Job target or any run with a Job control **directory** (promotion's `is_strict_job`) must present a valid two-key receipt, and a failure blocks the plan. An **unowned legacy run mirrors the real finish, which delivers through `LegacyUnowned` without validating any proof**: its signed acceptance marker is validated and reported as *evidence* (`validated` + `error`) but never blocks — so a legacy plan can honestly read `validated:false` with `status:"deliverable"`. A scratch-staging failure after a valid proof reads `validated:true` with `status:"blocked"` and the staging error in `receipt.error`.
- `destination.kind` gains a third value **`"in-place"`** (path = the checkout; branch/strategy null) beyond the proposed `"apply"|"materialize"` — in-place runs finish as a no-op and the contract had no honest bucket for them. For `"apply"`, `branch` is the `--into` value when given, else the target checkout's current branch when readable, else null; `strategy` echoes `--git-strategy` (default `squash`). For `"materialize"`, `path` is the normalized `--dest` (default: cwd/`<short-id>`) and the preview enforces the real export's library-present and not-inside-home refusals up front.
- **Apply-mode pre-flight (worktree runs):** before an apply-mode plan may claim "deliverable", the preview runs the read-only slice of the real apply's refusals: `refuse_non_deliverable_result_history` for every worktree target, and for Job targets additionally `refuse_in_progress_git_operation`, `GitDeliveryTarget::inspect`, the `--into`-vs-attached-branch identity check (same message), and the signed-delivery-intent mismatch refusal (an existing applied receipt or matching intent reconciles idempotently in the real path, so neither blocks). The deeper reconciliation validations (signed revision topology, applied-receipt binding) still run only in the real finish.
- `staged` lists files **only for `"materialize"` plans** — the export is the only finish route that publishes a staged file set. It is the literal promotion candidate (Promotable projection) minus `manifest.json` unless `--include-manifest` was passed (the default real export removes it), and includes `.deadreckon/**`. Apply-mode and in-place plans report `staged: []` — an apply delivers git commits and an in-place finish delivers nothing, so a file list would be the wrong set to render. `result_tree_sha256` is the **deliverable** projection of the staged candidate minus `manifest.json`/`.materialized-to` — exactly the digest a completion receipt binds; on strict Jobs it equals the sealed receipt's `result_tree_sha256`, proved by running the real candidate-bound receipt validation (`validate_strict_candidate`) against the scratch. For apply-mode Job plans it is the receipt-bound digest directly; null when blocked, for legacy apply plans, and for in-place plans.
- `diffstat` baselines against the frozen turn-0 snapshot (the same source `show --diff` rides) and is **`null` when there is no baseline or the mode stages nothing** — so zeros always mean "genuinely nothing changed", never "nothing to diff against".
- `irreversible_steps` is `[]` for in-place plans (the real in-place finish stages nothing, publishes nothing, and is a pure guidance no-op — the preview stages nothing for it either) and `["publish","cleanup"]` otherwise.
- `next_actions[0]` (the recommended command) **reproduces the previewed delivery exactly**: export plans carry `--dest <reported path>` (the preview and real export default differently, so the path is always explicit) plus `--overwrite`/`--include-manifest` when those shaped the plan; apply plans carry `--git-strategy <s>` and `--into <branch>` when given.
- **Lock discipline:** the preview *probes* the per-Job operation lock — same refusal on contention, and when the lock file exists and is free the probe holds it for the preview's duration — but if the lock file does not exist it acquires nothing and creates nothing. Absence proves only that no holder existed **at probe time**; a real finish may create-and-lock mid-preview, which is safe because the preview is read-only (worst case: a torn report, never unsafe reuse). Holders create-then-flock and never unlink; creating the file here would both break byte-identity and, for a legacy run, silently flip it onto the strict promotion path via the Job-dir probe. The verbs that actually acquire this lock are `finish`/`apply` and `undo`; the refusal prose keeps the historical operation wording.
- Scratch lives in the system temp dir — never `~/.deadreckon`, the library parent, the run root, or the working tree, and `stage_promotion_preview` now **enforces** that containment as a hard refusal (canonicalized, so a `TMPDIR` redirected inside the home cannot make a preview stage into the store or leave crash residue there). It is deleted before the plan is printed. Trust properties are pinned by tests in lifecycle.rs: home tree-hash byte-identity (deliverable, blocked, in-place, and apply plans), real-finish fail-closed after a post-preview receipt tamper (nothing reused), the locked-refusal envelope, and the scratch-containment refusal.
- Without `--json` the same plan renders as the human `VerdictSurface` card (`preview finish plan <id>` / `blocked finish plan <id>`); with `--json` the plan object additionally carries the shared `primary_action` + `verdict` surface projection (additive, same one-surface-object rule as G1). Plan-scoped dry-runs resolve the plan's result run and mirror the real routing (apply into the parent checkout when it exists and no `--dest` was given, else materialize) **without** materializing plan docs — the real finish writes those into the library; the preview must not. (Plan-scoped apply routes skip the worktree pre-flight above: the real plan apply resolves its own apply state, and the preview does not model that resolution.)

### G5 — Follow stream / tailing contract (S then L)

**Today:** no follow surface anywhere; `attach --json` is a snapshot; the broadcast bus is in-process-only (GAP-ANALYSIS.md concurs); file tailing semantics are implemented and tested (torn-tail, strict seq, fsync — job.rs:347–548) but undocumented as a contract; V1-CANDIDATES defers the attach daemon.
**Proposal:** **Step 1 (this week):** `docs/TAILING.md` promising per-file guarantees (append-only, schema-conformant lines, unterminated final line = torn append to ignore, strict seq on job-events, no rotation) + a conformance test — this legitimizes every file-tailing design. **Step 2 (M3):** `deadreckon follow <id> --json [--from …]` emitting merged NDJSON `{"source","offset","record"}` with replay-from-offset, reusing TuiEventFeed/AttachJsonlTail headlessly. This becomes the shared machine surface `attach` itself can later re-host on. A Unix-socket control plane stays out of scope until follow proves insufficient.
**As built (M3), for decoder authors:** `deadreckon follow <id> --json [--from <spec>]` shipped as proposed (CLI: `commands/follow.rs`; tail core: `AttachJsonlTail::poll_follow` — the attach torn-tail reader run headlessly, one algorithm, no reimplementation). Decoder-load-bearing details: (a) **refs** — JOB and RUN refs through the shared resolver (plan/chain/campaign refs redirect to `attach`); a Job maps onto its **current attempt run** (`projection.json` `child_run_ids`, newest last — the same rule verdict/G7 and show/G10 use) with the job's `job-events.jsonl` merged in; **following across attempt boundaries is out of scope** — the stream stays on the attempt that was current at start, a retry is visible as `job-events` rows, and following the new attempt means reconnecting. (b) **line shape** — NDJSON `{"source","offset","generation","record"}`: `source` ∈ `job-events|events|spend|traces|flight|acceptance-progress|notify` (`job-events` only on JOB refs; `flight` is `flight-events.jsonl`), `offset` is the byte offset AFTER the record, `generation` is a short opaque token naming the exact file the offset was read from (`offset@generation` is the replay cursor), `record` is the parsed row verbatim; lines merge in arrival order **at poll granularity** — appends to different files inside one poll window drain in the fixed source order above; per-source ordering is exact. (c) **replay** — `--from source=offset[@generation][,…]` resumes named sources without duplication or loss; carry the generation token: a nonzero offset whose generation no longer matches the file refuses (append-only — e.g. a cursor from a previous attempt) or restarts (`acceptance-progress`) instead of silently skipping the new file's head, while a bare offset stays accepted but unverified (safe only at 0); unnamed sources restart from 0; unknown/unfollowed sources, malformed offsets, duplicate cursors, and an empty spec refuse with `try_lines`; a stale/mid-record nonzero cursor that fails its first read is refused as an invalid cursor, not reported as ledger corruption. (d) **acceptance-progress restart** — on any rewrite anomaly under a retained offset follow emits one `{"source":"acceptance-progress","restart":true}` marker line, then re-emits from offset 0 (carve-out: a parse failure at offset 0 is corruption and fails closed — restarting would loop over the same bad bytes); every other source treats the same anomaly as corruption and fails closed rather than skip or re-emit a row, and `job-events` additionally enforces invariant 5 (a `sequence` discontinuity fails the stream closed). (e) **end** — terminal phase AND drained tails ⇒ one final `{"terminal":true,"phase":…}` line (jobs additionally carry their typed `outcome`, since phase `terminal` alone names no result) and exit 0; SIGINT ends the stream with no final line (exit 130); a piped stdin closing ends it quietly (exit 0) — the subprocess-supervision contract, so a dead supervisor can never leak followers (corollary: stdin from `/dev/null` yields one backlog drain then exit — a snapshot); an executing run with no live runner behind it (the `status` staleness rule) gets one advisory `{"stalled":true,…}` line while the stream stays open, so a consumer can apply its own timeout instead of polling a dead run unaware. (f) `--json` is required (machine-only surface; the refusal without it is prose because the G1 envelope contract begins when `--json` arms it); armed refusals ride the G1 `{"kind":"error",…}` envelope, serialized compactly so a mid-stream refusal is itself one valid NDJSON line; polling reuses attach's budgeted idle backoff constants; follow is strictly read-only. Contract pinned by `tests/follow_stream.rs` and documented in docs/TAILING.md ("the blessed streaming reader"). The app can adopt follow post-vendoring — one subprocess stream per visible job replaces its per-job Swift tailers exactly as the M3 roadmap line plans; until then direct file tailing remains fully supported.

### G6 — Steerability as data (S)

**Today:** the predicate lives only inside steer's guard (steer.rs:53–75: `cli:codex-server` + `Executing` + driver fence); no `steerable` field exists anywhere.
**Proposal:** extract `steer_eligibility(&PipelineState) -> {steerable, reason}` into deadreckon-core, call it from the steer path *and* embed `steerable{}` in `status --json`, `show --json`, and RunView — CLI and app can never disagree, and the apps update for free when steer widens.
**As built (M0), legacy caveat:** the JSON surfaces derive `driver_fenced` from the `PipelineState.ownership` stamp; the steer verb additionally runs the full plan-lineage fence. Job-owned runs created before ownership stamping can therefore read `steerable:true` yet still receive the typed fence refusal from the verb. The app must treat a steer refusal after `steerable:true` as authoritative and downgrade the control. Modern job-created runs are stamped at first state write and cannot diverge.
**As built (M1), steer widening:** steer now works for ANY supported provider while a run is Executing, with per-route delivery mechanics every surface states honestly. `cli:codex-server` keeps its exact mid-turn path (`turn/steer` against the active turn). Every other provider is queued-for-next-turn: the run loop drains pending `steer-inbox.jsonl` entries at the top of each turn and injects them into that turn's prompt frame as a clearly-labeled **advisory** operator-guidance block — it cannot alter the frozen goal/contract/policy in `authority.json` and never reaches gate evaluation inputs. Consumption reuses the same file-backed bookkeeping as the codex driver (entry marked `delivered` with loop turn id `turn-N`), so the two paths cannot double-deliver, and each consumed note appends one typed `steer_delivered` event `{turn,source,queued_at,preview}` to `events.jsonl` for queued→ack rendering. `steer_eligibility` accordingly refuses only on `driver_fenced` / `not_executing`; `provider_not_steerable` is never produced anymore but stays in the enum so JSON decoders survive. The steer envelope's `delivery` fact reads `"active or next provider turn"` on codex-server and `"next turn boundary"` elsewhere.

### G7 — Per-digest receipt audit (M)

**Today:** `validate_completion_receipt` is a fail-first monolith (first mismatch → Err) exercised only by finish; `verdict --json` exposes only marker presence/validity.
**Proposal:** refactor into a fact-collecting `audit_completion_receipt(...) -> ReceiptAudit{facts:[{name,pass,detail}]}` where the strict path becomes `audit(...).into_result()` (byte-identical promotion semantics); surface as `verdict <id> --receipt --json`. Inspection only — promotion still runs the strict fail-closed path.
**As built (APP-3 follow-up), JOB refs + the driver-fence carve-out:** `verdict` now accepts durable **Job** references (the registered gap: a Single-shape job's id resolves to the Job kind, so `verdict <job>` used to be a typed refusal and the fence blocked the child run). A Job ref maps onto the job's **current attempt run** — `projection.json` `child_run_ids`, newest last, falling back to the job's own run id when no child is linked (a Single-shape job's attempt run IS the job id; same rule CONTRACTS.md documents for the Chartroom). For JOB refs the DEFAULT is the **read-only receipt audit** — checks are NEVER re-run, because re-executing checks against a job-owned run root is exactly what the driver fence prevents. The exact carve-out (`verdict_job_receipt_audit`, verdict.rs): the Job path skips `require_current_driver_for_job_owned_run` and is sound only because it performs reads and nothing else — marker signature validation (a pure read), `audit_completion_receipt` (reads exactly what the strict validator reads), and the verdict sidecar is **skipped**, not written best-effort, so the driver-owned run root stays byte-identical (`paths.cache` reads `null` in the envelope to say so). Every mutating/re-run path keeps the fence: run refs are unchanged, and `verdict <job> --rerun-checks` (the explicit opt-in) maps to the same attempt run and then takes the fenced re-run path, so a public invocation still gets the typed "belongs to durable Job" refusal. Two fences cover the two ownership shapes: stamped/plan-owned attempt runs hit `require_current_driver_for_job_owned_run`, and a **Single-shape** job's attempt run (run_id == job_id, no stamped ownership for the run fence to see) hits the artifact-level `require_current_driver_for_job_artifact` — the same guard steer uses — with the identical refusal (pinned for both shapes in tests/verdict_job_inspection.rs). **Decoder notes:** the Job envelope is the run envelope plus `job_id`, `mode:"receipt_audit"`, `checks_rerun:false`; `checks` is always `[]`; `receipt_audit.facts` ride whether or not `--receipt` was passed; `status` reuses the verdict enum with receipt-audit-pass standing in for the re-run — `"verified"` here means *valid signed marker AND every receipt audit fact passes now* (recorded proofs re-validated at read time, not checks re-executed).

### G8 — Live gate progress on disk (S)

**Today:** the plumbing exists but is unwired — `emit_acceptance_progress` + `evaluate_acceptance_checks_with_progress` exist with only a test caller; dr-gate evaluate prints to stdout only; `acceptance-progress.jsonl` is written *reconstructed at sign time*; attach already tails that exact path, so today its acceptance band lights up only after signing.
**Proposal:** have `dr-gate evaluate` emit per-check started/passed rows through the existing writer (advisory display data only — sign's reconstruction still overwrites with trusted rows, so anti-self-attestation is untouched); stamp `last_gate_attempt{attempt,n_passed,n_total,finished_at}` into projection.json. Also improves the current TUI.
**As built (M0), two trust-preserving deviations:** (a) contained/strict gate evaluations stream no live rows — the sandbox write-denies `proofs/` on every backend, so the live acceptance band applies only to uncontained/non-strict gates and manual `dr-gate evaluate`; the file appears whole at sign time (see docs/TAILING.md). (b) `last_gate_attempt` counts are sourced exclusively from the signature-verified acceptance marker: failed attempts (no signed marker) stamp no counts into the ledger, because raw progress-file bytes cannot prove controller authorship and must never reach job-events.jsonl.

### G9 — Send-back as a typed action (S)

**Today (smaller than the designs assumed):** `extend <parent> "goal" --yes` is already a durable, non-interactive continuation verb defaulting to the parent's frozen contract. Missing: JSON envelope + a durable operator-rationale record. (Public `resume` is retired — supervisor owns torn-job recovery; no resume UI.)
**Proposal:** `extend --note "..." --json`: the note appends a typed `{kind:"operator_sendback", note, parent_job_id, new_job_id}` ProvenanceRecord; envelope reports the queued continuation.

**As built (M1), for decoder authors:** the envelope `kind` stays `"extend"` per the G1 armed-verb convention — there is no dedicated `extend_result` kind. The G9 facts ride as top-level fields on that envelope: `parent_id`, `parent_run_id`, `contract` (`"inherited"` when the parent's frozen acceptance carries over, `"replaced"` when `--acceptance` was explicit), and `note_recorded: bool` — the note text is **not** echoed back (the caller already knows what it sent), plus the shared Job-status payload fields (`verified_proof`, `paths`, `work_clock`, `job`) and G1's `{queued:true}`. The provenance row appended to the PARENT run's `provenance.jsonl` is `{kind:"operator_sendback", note, parent_job_id, new_job_id, at}` — one additive `at` timestamp beyond the proposed four fields, and `parent_job_id` carries the parent **run** id (extend resolves parents by run identity; for Job-owned parents the two coincide). Refusals: an empty `--note` is a typed refusal, and `--note` on a path that queues no durable Job (the internal characterization path) refuses rather than silently dropping the note.

### G10 — Changes surface (M)

**Today (partially exists):** `show --diff --json` emits the full-run DiffSummary; per-turn diffs exist. Missing: exportable per-file **unified** diffs (hunks) of frozen source tree vs candidate tree.
**Proposal:** `show <id> --diff --json --patch [--file PATH]` adding `patches:[{path,status,unified,truncated}]` with a byte budget per file. Read-only; no new verb.
**As built (APP-3 follow-up), JOB refs:** the registered gap — a Single-shape job's attempt run IS the job id, so the resolver handed `show <job> --diff` the Job kind and the diff flags were silently dropped into the `job_status` envelope. `show`'s Job branch now delegates diff-shaped requests (`--diff [--patch --file]`, `--turn`, `--file`) to the run surfaces against the job's **current attempt run** — the same resolution `verdict <job>` uses (`projection.json` `child_run_ids`, newest last, falling back to the job's own run id), so multi-attempt jobs answer for their current attempt and the two verbs can never disagree about which attempt is "current". Read-only, no fence involvement. Plain `show <job>` (and `--flight`/`--raw` without those flags) keeps its exact historical routing to the job-status envelope, pinned byte-identical against `status <job>` (tests/show_job_diff.rs).

### Notify events — typed operator-attention rows (operator decision 6, M1)

**As built, for the app's notification layer:** run-local `notify.jsonl` now carries typed rows `{schema_version, kind: "operator_attention", reason, job_id?, run_id?, scope?, at, summary, next_actions}` alongside the pre-existing delivery-attempt rows (unchanged; the two shapes are distinguished by the presence of `kind`, and both are described by the checked `docs/schemas/notify-event.schema.json`). `reason` is one of `verified_awaiting_promote | paused_at_cap | blocked | failed | cancelled | waiting_input`. Each row is appended exactly once by the process that owns the transition: the receipt-sealing path itself appends `verified_awaiting_promote` when a two-key completion receipt is *newly* sealed (recovery reseals append nothing), the run-loop process appends `paused_at_cap` when it stops at a spend/wall cap — regardless of whether `[notify]` delivery is enabled — and the supervisor appends `blocked`/`failed`/`cancelled` at terminal classification plus `waiting_input` when it classifies NeedsReview (the durable waiting-for-operator state; `Verified` and `BudgetExhausted` classifications emit nothing here because the sealing path and the run loop already announced them). `summary` is one glossary-worded line; `next_actions` are real runnable CLI commands (`deadreckon finish <id>`, `deadreckon resume <id>`, …). Trust note: rows are display-only observability — appends are best-effort, the binary never reads them back, and they confer no authority; the app must treat `status --json` and the signed markers as truth and use these rows only to post notifications (tail per docs/TAILING.md; dedupe on the app side with stable notification IDs).

---

## 8. Roadmap

### M0 — Read-only companion (no new authority; app mutates nothing)

G6 · G3 · G8 · G5-step-1 (tailing contract) · G10 · G7.
**Exit:** the app renders fleet queue, job drill-in, live acceptance band, diff tab, and the gate-review digest table by polling `list --json` + `status --json` and tailing blessed JSONL — zero pty, zero mutations. *This milestone is shippable as a product on its own: a truthful fleet monitor.*

### M1 — Safe control verbs (typed mutations; promotion still CLI-only)

G1 (global error envelope + result envelopes) · G9 · G2.
**Exit:** launch (`start --json` preview → `start --plan --yes --json`), steer, kill, extend, and completed-run dispositions all from the app, reading only envelopes.

### M2 — Acceptance console (the trust-sensitive slice)

G4 · `finish --json` on the real path (S once G1's machinery exists).
**Trust rule held throughout:** the app never invokes dr-gate, never signs, never overrides a failed digest; dry-run output is a report, not a staged handoff.
**Exit:** the full Binnacle — preview-before-mutate, digest table, send-back — with every fail-closed path surfaced as a typed refusal.

### M3 — Streaming (replaces polling, unblocks scale)

G5-step-2 (`follow`). App drops per-job file tailers for one subprocess stream per visible job; the poll tick survives on `list --json` alone.

### Quick wins (this week, by leverage per line)

1. `steerable{}` in status/show JSON — extract the predicate already sitting in steer.rs.
2. Wire `dr-gate evaluate` to the already-written-and-tested progress emitter — live acceptance bands for every surface **including the current TUI**.
3. `extend --json --note` — an envelope plus one provenance append.
4. `--i-know-its-a-lot` on Start (today hardcoded false → >$50 launches unreachable non-interactively).
5. Enrich `list --json` with projection.phase, provider, receipt presence (JobViews already loaded).
6. Write `docs/TAILING.md` — the guarantees are already implemented and tested; documenting them costs a page and legitimizes every file-tailing design.

---

## 9. App shell & stack (what we take from specstory-mac)

specstory-mac is a shipped SwiftUI menubar-first macOS 14 app wrapping a vendored Go CLI — the closest existing proof of the exact shape we need. Copy / adapt / avoid:

**Copy outright:**
- **Menubar-first shell:** `LSUIElement=true`, `MenuBarExtra` + lazily-built `NSWindow` via `NSHostingController`, activation policy flipping `.accessory↔.regular` (Dock icon only while the window is open). Template icon idle / colored live / badge on error, driven by a small state enum.
- **Vendored-binary pattern:** `scripts/vendor-cli.sh` builds/pins the CLI into `Resources/bin` (gitignored) with a **committed `manifest.json`** (version/commit/sha256); a `BinaryLocator` verifies sha256 at launch; `DEADRECKON_BIN` env override for dev. Our advantage: `cargo dist` artifacts + the Developer ID signing assets already at the repo root.
- **Process discipline:** one `Process` per invocation; stdout parsed line-wise into an `AsyncStream` (NDJSON); SIGTERM-then-SIGKILL with patience. Bounded tail fleet (`WatchSupervisor` pattern: max ~8 active tails, LRU eviction, FSEvents tripwire spins tails on demand) — background rows poll `projection.json` at ~5s; the focused job tails at heartbeat cadence.
- **CONTRACTS.md discipline (the standout):** before building, write literal Swift public-API signatures per module with embedded behavioral invariants. Ours must cover: tailer semantics (torn-tail, strict-seq verification), read-model derivation rules (fleet rows from projection.json + glossary words **only**), the verb dispatcher, and the trust rules (§2.4.4, gate-key hygiene).
- **Kit/App split:** `DeadreckonKit` SwiftPM package holding models/services/read-models with all tests; the app target is views + shell.
- **UX patterns:** design tokens with `dynamicColor(light:dark:)`; provider badge iconography; LivePill; session cards with hover-revealed actions; pinned "Happening now" with expandable cockpit; ⌘K overlay; launch-at-login via SMAppService; notification auth requested lazily with remembered denial and **stable notification IDs** (+ our launch-time catch-up scan).

**Posture:** no App Sandbox (reads `~/.deadreckon`, spawns the CLI); sole entitlement apple-events (Open in Terminal → `deadreckon attach <id>`). Signing/notarization per the existing release runbook; Sparkle-style updates deferred exactly as specstory-mac defers them.

**Avoid (their scars):** the `@MainActor` god-model that grew unbounded (per-domain observable models with contracts instead); debug env-var UI hooks in the app delegate; doc drift between ARCHITECTURE.md intent and shipped behavior; hardcoded strings.

**Startup handshake:** `deadreckon --version` + `doctor --json` + schema-version check; refuse to operate on a `DEADRECKON_HOME` written by a newer binary than the vendored one.

---

## 10. Open questions (operator's call)

1. **App name & milestone:** *Bridge* as the app (goal+rider would follow the registry convention)? Screens keep feature names (Gate Queue, Helm view, Rudder, Binnacle, Logbook, Harbor).
2. **Repo home:** in-repo (`apps/mac/` alongside `crates/`) vs a sibling repo. In-repo keeps the vendored-binary manifest and `docs/schemas/` in lockstep; sibling keeps Xcode/XcodeGen churn out of the Rust workspace. Lean: in-repo until the schema-version handshake is proven, then revisit.
3. **v1 depth for plan/chain/campaign:** rows + child counts + attach escape hatch (recommended), or a voyage-tree surface from day one?
4. **Popover mutation policy:** current stance is triage-only (no kill/promote from the popover). Confirm.
5. **Steer trajectory:** does steer stay `cli:codex-server`-only near-term? G6 makes the app future-proof either way, but it shapes how prominent the Rudder should be in v1.
6. **Notification transport:** file-tail of `notify.jsonl` + phase transitions (v1), vs the supervisor posting user notifications directly (later, would need binary work not currently registered as a gap).
7. **Distribution timing:** dev-signed builds for personal use first (install-app.sh style), or straight to notarized releases riding the existing release-trust pipeline?
8. **Ranking without lying:** any appetite for forward-looking readiness estimates in the queue (visually quarantined), or durable facts only (recommended)?

---

*Full per-agent research reports (with complete file:line citations) are preserved in the workflow journal: `~/.claude/projects/-Users-gdc-deadreckon/2fc67141-b30f-433d-a892-906b618434f0/subagents/workflows/wf_70841300-151/journal.jsonl`. The workflow script is reusable for follow-up passes (e.g., re-running the design studio against a revised brief).*
