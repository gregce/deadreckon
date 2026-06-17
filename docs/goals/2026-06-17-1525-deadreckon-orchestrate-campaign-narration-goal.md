GOAL: Extend the shipped Live Narrator (§44) so every orchestrated and campaign child narrates itself, a parent aggregate line shows the fan-out at a glance, and `dr attach` on a campaign gets a Narrative view at parity with plan attach. Today narration is wired only in `dr run`; orchestrate/campaign children get zero live beats because they are SUBPROCESSES — `run` children hit `resolve_narrator_config`'s off-TTY None, and `extend` children (reviewers) re-enter `lifecycle.rs` with `event_sender: None`. The engine, plan attach folding, and post-hoc seeding are reusable as-is; this slice routes narration into the children and surfaces it at the parent. Land this slice named Orchestrated Narration.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-17-1525-deadreckon-orchestrate-campaign-narration-rider.md` — phases, depth tests, citations.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §44 — the shipped Live Narrator.
- `/Users/gdc/deadreckon/crates/deadreckon/src/narrator.rs` + `commands/run.rs` (399-465) — engine + template.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/{lifecycle.rs,plan.rs,orchestrate.rs,campaign.rs}`, `src/narrative.rs`, `crates/deadreckon-core/src/state.rs`.
- Prior `2026-06-15-1433-…-live-narrator-rider.md` — invariants hold.

**Posture.** Stable track. Additive only; no `PipelineState`/`RunLoopConfig` schema breakage; per-child state stays file-backed under `<run_root>/narrative/`; deterministic floor stays the no-provider floor. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**Children narrate file-only, surfaced at the parent.** Each child writes Live beats to its own `snapshots.jsonl`; a new `resolve_narrator_config_for_child` returns `foreground=false, headless_append=false` so beats hit the file but never child stdout (parent scrapes run-id) or stderr (failure capture). Default child backend is the deterministic floor ($0, no probe) unless `--narrator-model` is passed; the parent resolves once and threads the model + `DEADRECKON_AUTH_PROBE=0` to avoid an N-child probe storm. Per-child isolated budget.

**Two-pronged wiring.** Extract `run.rs` narrator-build into a shared `build_run_narration` helper (one shutdown contract). Wire both `lifecycle.rs` extend sites (~1575, ~1833). Add `--narrate/--no-narrate/--narrator-model` to extend/orchestrate/campaign; `run_plan_child` + `build_sub_orchestrator_command` append `--narrate` to child argv.

**Reliability + a latent fix.** `spend_summary` (state.rs 316) skips `kind != "loop"` rows (narrator rows inflate totals today). Cap the plan agent_table to `narrate_lines` children + "+N more" (Q5); stop an attach-time Deterministic projection masking a prior Live beat.

**Option D (included).** D1: under `--narrate`, the parent tails each active child's snapshots.jsonl (`plan_event_bus::JsonlTail`) and prints one capped line per child to STDERR. D2: a new `build_campaign_projection` + campaign attach Narrative view (none today), at plan-attach parity.

**Phases.** Eleven in the rider. Each: depth test first → implement → fmt+clippy+test green → conventional-commit → CHANGELOG line. P11 adds AS-BUILT §45, corrects the §44 overclaim.

**Verification.**

- Every rider depth test present and passing; `cargo test --workspace --locked` green.
- `dr orchestrate full-plan --narrate`: each child writes Live beats to its `snapshots.jsonl`; plan attach shows one capped line per child; the parent prints a capped stderr aggregate; child stdout stays clean (run-id still scraped).
- `dr campaign … --narrate` narrates end-to-end; `dr attach <campaign>` has a Narrative view. `spend_summary` excludes `kind:"narrator"` rows. `cargo fmt --check` + `git diff --check` clean. No `git push`/schema breakage.

**Stop when** verification passes, AS-BUILT §45 (+ §44 correction) / V1-CANDIDATES / a `0.3.0 — Orchestrated Narration` CHANGELOG section are updated, and all phases committed locally.
