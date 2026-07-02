GOAL: Make attach the best mission-control TUI in the agent-supervision category — instant comprehension, snappy controls, one pane for the whole voyage. Today four ratatui surfaces answer different questions differently: campaign forces drill-in navigation, chain has no narrative view, the loop polls input at 250ms, render.rs is a 2,905-line monolith, and in-frame prompts suspend the alternate screen. This slice lands: a uniform STATUS SPINE (five questions every surface answers above the fold: alive? doing what? on track? anything wrong? what next?), a flattened campaign→plan→run EVENT TREE with zoom-free comprehension, an event-driven async loop (keystroke-to-frame in single-digit ms, proven by the tick instrumentation), a k9s-style `:` command mode + contextual keys, in-frame input/modals, a `w`-for-why evidence panel, a scrubable turn TIMELINE, and a restrained effects layer behind a motion policy. Stay on ratatui — upgrade the architecture, not the framework. Land this slice named Helm.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-01-2011-deadreckon-helm-rider.md` — spine contract, tree schema, loop design, key/command map, sixteen phases, depth tests.
- `crates/deadreckon/src/commands/attach.rs` — tick loop, `AttachTickTiming`, 250ms poll.
- `crates/deadreckon/src/tui/{render.rs,attach_state.rs,navigation.rs}` — `render_attach`, `NavigableSurface`, the monolith to decompose.
- `crates/deadreckon/src/narrative.rs` + `crates/deadreckon-core/src/events.rs` — projections, `PlanEventBus`, JSONL tails.
- `docs/AS-BUILT-ARCHITECTURE.md` §18/§25/§27/§32/§36; `docs/V1-CANDIDATES.md` (flattened tree, in-frame input deferrals). Prior riders hold.

**Posture.** Stable track (0.4.0). Stay on ratatui 0.29 + crossterm 0.29 — no framework migration (iocraft/r3bl/rooibos rejected; ratzilla web mirror is V1). The non-blocking render contract is sacred: no provider calls inline, complete-JSONL-row tailing only, q/Esc always instant. New widget crates (tree, textarea, effects) are Tier 2 — logged in DEPENDENCIES.md, pinned to ratatui 0.29. Decomposition is behavior-preserving, guarded by characterization goldens. No `PipelineState` schema changes; read models only. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**Comprehension is a contract.** The spine is a code table like the friendliness contract: five questions × four surfaces, every cell depth-tested — uniformity enforced, not aspired to. The tree makes zoom optional: status, gate progress, spend visible per node; Enter zooms but is never required to know state. `w` on any failed/paused node opens cited evidence (pause reason, gate check, tamper verdict, provider error). The timeline scrubs turn checkpoints with per-turn story + diff counts.

**Snappy is measured.** The async loop selects over crossterm's EventStream + ledger wakeups; `AttachTickTiming` gains an input-to-frame stage and a budget test pins it. Command mode (`:kill`, `:why`, `:verdict`) maps to existing verbs only.

**Pizzazz with a policy.** tachyonfx effects fire ONLY on meaning: gate pass, verdict, node state change. `[ui] motion = full|reduced|off`; `reduced` is the non-TTY/replay default; every effect skippable. Calm is the brand.

**Phases.** Sixteen (P1–P16) in the rider. Each: depth test first → implement → `make verify` green → commit → CHANGELOG line. P16 adds AS-BUILT §47.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit; characterization goldens unchanged where behavior is preserved.
- `attach <campaign-id>` shows the full tree + spine, zero drill-ins; chain attach gains the narrative view.
- Input-to-frame stays within the pinned budget under a replayed event storm; `w` cites real artifacts; `motion = off` disables every effect.

**Stop when** verification passes, AS-BUILT + V1-CANDIDATES + a `Helm (stable)` CHANGELOG section are updated, committed locally.
