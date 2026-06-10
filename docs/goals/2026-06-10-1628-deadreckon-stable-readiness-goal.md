GOAL: Close the five gaps between rc.11 and a trustworthy stable v0.1.0 — first-class model selection at every launch surface, never-dead-end launch prompts, a rehearsed stable release lane, one real-provider proof per supported CLI, and the two remaining walk-away durability holes. Today the model catalogs are empty ("provider default"), orchestrate/campaign cannot set models per role, an unusable default provider hard-refuses on a TTY instead of offering the picker that exists one module away, the stable lane (Homebrew, npm, Authenticode) has never executed, no CI evidence covers a real claude/codex turn, a torn `history.json` bricks resume, and a stale-heartbeat lock can be usurped from an alive holder. Land this slice named Stable Readiness.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-10-1628-deadreckon-stable-readiness-rider.md` — phases, schemas, depth tests, exact citations.
- `/Users/gdc/deadreckon/crates/deadreckon-providers/registry` + `descriptors/*.toml` — ModelEntry, catalogs.
- `/Users/gdc/deadreckon/crates/deadreckon/src/{setup.rs,prompt.rs,commands/start.rs}` — refusal sites, picker engine.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs` (history reconstruction), `/Users/gdc/deadreckon/crates/deadreckon-core/src/lock.rs` (reclaim).
- `/Users/gdc/deadreckon/docs/{AS-BUILT-ARCHITECTURE.md,RELEASE.md,V1-CANDIDATES.md}`. Prior riders' invariants hold; the Uniform Surface prompt-crate block is superseded by the shipped inquire engine (AS-BUILT §42).

**Posture.** Production-release track. No `PipelineState` schema changes; additive serde-default fields on `plan.json`/descriptor TOMLs only. New verbs: `models` only. No `git push`, no tags — release/operator actions are listed, not executed. Edits inside `/Users/gdc/deadreckon`. Major decisions → V1-CANDIDATES.

**Model selection as a contract.**

- Catalogs populated for all six CLI descriptors + http routes; one `recommended` entry each.
- `deadreckon models [provider] [--all] [--json]` lists them, marking configured default.
- Flag parity: orchestrate `--planner-model/--coder-model/--reviewer-model/--child-model IDX=MODEL`; campaign `--planner-model/--child-model`; run/start/chain already have `--model`.
- TTY picker step after provider in `start`/`init`: catalog entries with context/cost hints, default preselected — Enter keeps today's behavior.
- Every preview/verdict surface that names a provider names the resolved model.

**Friendliness as a verifiable contract.** Auto-detect, don't ask; preflight + preview; refuse with `try:` only when no TTY rescue is possible; unusable provider on a TTY → probe-before-ask picker (start, doc-provider, def-done); rollback one command; lifecycle hints everywhere.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo fmt --check` + `cargo clippy --workspace` + focused `cargo test` green → conventional commit → CHANGELOG line. P11 adds AS-BUILT §43.

**Verification.**

- Every rider depth test present and passing; `cargo test --workspace --locked` green (54 binaries).
- `deadreckon models cli:claude-code` lists a populated catalog; `orchestrate full-plan --preview --planner-model X` echoes X in the provider-roles table.
- With a credential-less default on a PTY, `start` reaches the provider picker instead of exiting nonzero.
- A `history.json` truncated mid-byte resumes via trace reconstruction; a lock whose holder pid is alive is never reclaimed.
- No `git push`. No state-schema changes.

**Stop when** verification passes, AS-BUILT/V1-CANDIDATES/CHANGELOG updated, the stable-lane operator checklist exists in RELEASE.md, and all phases are committed locally.
