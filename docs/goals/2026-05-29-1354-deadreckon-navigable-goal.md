GOAL: Make `deadreckon attach <campaign-id>` a first-class, live TUI. Today it is a dead-end plain-text dump (you must retype `attach <sub-plan-id>`); plan/run attach are ratatui TUIs with a live feed and Enter-drill-in. A campaign is a 3-level tree (campaign -> N sub-plans -> child runs) the current 2-level event model can't show. Land a campaign TUI **navigated by drill-in** — a pane listing the sub-plans live, descending into each sub-plan's plan TUI, reusing the plan/run TUIs unchanged. Headline word: **Navigable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §18 attach, §36 campaign, §38 binary module layout (post-decompose).
- `/Users/gdc/deadreckon/docs/goals/2026-05-29-1354-deadreckon-navigable-rider.md` — full contract.
- Post-decompose homes (`crates/deadreckon/src/`): `commands/attach.rs` (attach loop, `attach_plan_tui`, `suspend_tui`/`resume_tui`), `commands/attach_runtime.rs` (`AttachSurface`/`handle_key`/tick), `commands/campaign.rs` (`resolve_campaign`, `campaign_attach_summary`), `tui/render.rs`, `tui/attach_state.rs` (`AttachParentPlan`), `plan_event_bus.rs`, `ui_card.rs`.
- Prior riders (`2026-05-28-1841` campaign, `2026-05-29-1600` decompose, `2026-05-26-1546` narrative-attach) — invariants hold.

**Posture.** Production-release track; presentation + READ-ONLY event tailing only. New code lands in the decomposed modules (`commands/campaign.rs`, `commands/attach.rs`, `tui/render.rs` — §38), tests lifted, never inline in `main.rs`; the `commands/`/`tui/` facade and `pub(crate)` discipline hold. No core-mechanism change — campaign attach **reuses** `attach_plan_tui`, never modifies it; gate/sandbox/promotion/providers/campaign engine/plan+run TUIs untouched. Files-not-fields: no schema changes (the campaign/plan event + rollup files already exist). Reuse ratatui/`ui_card`/`JsonlTail`. No `git push`. Big bets → `docs/V1-CANDIDATES.md`.

**Architecture (decided — drill-in, not flattened).** Campaign TUI = header (goal, status, roll-up, tree budget, live spend) + selectable sub-plan cards + feed + footer; `Enter` drills into the sub-plan's existing `attach_plan_tui` (`suspend_tui`/`resume_tui`), `b` returns — navigated, so no flattened 3-level feed. A one-tier `CampaignEventFeed` tails `campaign-events.jsonl` + each sub-plan's `plan-events.jsonl` (`JsonlTail<T>`, read-side). Breadcrumb gains a campaign tier; off-TTY/`--json` reuse `campaign_attach_summary`.

**Key risk (first-class test).** `campaign -> plan -> run` is TWO nested `suspend_tui`/`resume_tui` levels (plan -> run is one today). P6 tests two-deep drill-and-back returns to the campaign cleanly, converting suspend/resume to a counter/stack if it is a flag today.

**Testable seams (no real terminal).** `render_campaign_attach_text` in `tui/render.rs` + `handle_key` over `AttachSurface::Campaign` (`commands/attach_runtime.rs`). Tests assert render content and key transitions, not pixels.

**Phases.** Eleven (P1-P11) in the rider, depth-tests-first; the live VIEW (P1-P5) lands before DRILL-IN (P6) so a fragile nesting fix can't block it. Each: test -> implement -> `make verify` green -> commit -> CHANGELOG. P11 updates AS-BUILT §18/§36.9 and logs deferrals.

**Verification.**

- `make verify` green at every commit (the decompose net is the gate); CLI characterization snapshots stay green, the off-TTY attach golden updated deliberately.
- Smoke: `render_campaign_attach_text` for a 2-sub campaign shows both sub rows + roll-up + budget.
- Smoke: off-TTY `attach <campaign-id>` prints the plain summary; drilling `campaign -> sub -> child` and backing out twice returns to the campaign.
- No edits outside the repo; no `git push`; no schema changes.

**Stop when** campaign attach is a live TUI with sub-plan cards + feed, two-deep drill-in and back works, the breadcrumb shows the campaign tier, off-TTY/JSON fall back, AS-BUILT and CHANGELOG record it, deferrals are in V1-CANDIDATES, committed locally.
