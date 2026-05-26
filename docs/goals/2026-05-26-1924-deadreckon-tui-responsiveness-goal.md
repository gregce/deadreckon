GOAL: Make every DeadReckon ratatui attach surface stay responsive while expensive refresh, filesystem, provider-log, narrative, and child-feed work continues in the background. Today `attach` can feel frozen because blocking provider calls and full-file/full-tree reads share the loop that draws frames and handles `q`/`n`/`v`/`r`. Land tick budgets, cached/incremental collectors, and background narrative refresh jobs. Headline word: **Responsive**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - TUI, events, narrative, and alpha posture.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1924-deadreckon-tui-responsiveness-rider.md` - code-path audit, phase plan, tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1546-deadreckon-narrative-attach-rider.md` - narrative invariants still hold.
- `/Users/gdc/deadreckon/docs/goals/2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md` - plan feed invariants still hold.
- Current code: `crates/deadreckon/src/main.rs`, `crates/deadreckon/src/narrative.rs`, `crates/deadreckon/src/tui_events.rs`, `crates/deadreckon/src/plan_event_bus.rs`.

**Posture.** Stays `alpha`. No `PipelineState`, `Plan`, `Chain`, `RunEvent`, `PlanEvent`, or narrative snapshot schema changes unless a phase proves no file/local alternative works. Prefer in-memory attach models, run-root/plan-root projection files already in use, and mtime/offset caches. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-level renderer rewrites or daemonized attach services go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Diagnosis to fix.**

- Run attach awaits `refresh_run_narrative_with_provider*` inside `attach_tui_with_parent`; manual `r` and automatic narrative refreshes can block input until `claude`/`codex` exits.
- Plan attach does the same through `refresh_plan_narrative_with_provider*`.
- Each run tick reloads `state.json`, full `spend.jsonl`, full `traces.jsonl`, full flight/provider activity, acceptance files, and a working-tree inventory.
- `collect_attach_live` calls `inventory_files` before filtering `node_modules`/`.git`, so large ignored trees are still walked; Chrome profile `.tmp` trees and build output can dominate a tick.
- Narrative rendering can touch projection files during render.
- Chain attach rereads full `chain-events.jsonl` each tick.

**User contract.**

- `q`, Esc, Ctrl-D, `n`, `v`, `j/k`, and `Tab` remain responsive even while a narrative provider refresh or provider-log scan is pending.
- Pressing `r` starts or coalesces a refresh, shows an in-flight notice, and never blocks detach.
- Automatic event/quiet refreshes are background jobs with at most one in flight per attach target.
- Panes may show slightly stale data with age/status labels.
- Completed-run attach, run attach, plan attach, child attach, and chain attach share the same responsiveness posture.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, V1-CANDIDATES, and CHANGELOG.

**Verification.**

- Focused tests: nonblocking narrative refresh, coalescing, cancellation on detach, incremental JSONL readers, inventory pruning, provider-log throttling, render purity, and stale-data labels.
- Smokes: live run narrative `r` remains detachable; plan attach with slow child feed remains detachable; large `.tmp/chrome-profile` tree does not freeze live files; chain attach with large event file remains usable.
- Do not run `make verify`, stress suites, or broad release checks by default unless the human asks.

**Stop when** focused verification passes, manual/automatic refresh work is off the UI loop, expensive collectors are cached/incremental or throttled, AS-BUILT/CHANGELOG document the alpha responsiveness contract, and the work is committed locally.
