GOAL: Adopt the workspace-level discipline that holds codex-rs together — strict clippy lints, registry-style `lib.rs`, tuned release profile, formatted imports, library-only print refusal, internal crates routed through `[workspace.dependencies]`, and an `is_retryable()`/`is_fatal()` taxonomy on each crate's existing error enum — without changing a single byte of runtime behavior. Today every `Cargo.toml` lints independently, four `lib.rs` files mix re-exports with no rule, library crates can `println!`, the release profile is stock, and internal crate paths are duplicated. This goal lands the seven moves listed below as a pure scaffolding refactor. Headline word: **Hygiene**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §22.
- `/Users/gdc/deadreckon/docs/goals/2026-05-12-2039-deadreckon-hygiene-rider.md` — lint table, profile values, `lib.rs` target shape, depth tests.
- `/Users/gdc/codex/codex-rs/Cargo.toml` — model `[workspace.lints]`, `[workspace.dependencies]`, `[profile.*]`.
- `/Users/gdc/codex/codex-rs/clippy.toml` — exemption shape.
- `/Users/gdc/codex/codex-rs/protocol/src/{lib,error}.rs` — registry-`lib.rs` and `is_retryable()` exemplars.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. **Pure scaffolding refactor: zero functional-behavior change.** No `PipelineState` schema changes. No CLI verb additions. No observability changes. No new crates. Every binary dispatch, every state transition, every artifact byte stays identical. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Major architectural decisions → `docs/V1-CANDIDATES.md`.

**The seven hygiene moves (specs in rider).**

- **`[workspace.lints]` + `clippy.toml`** — clippy/rustc deny set; `[lints] workspace = true` on every crate.
- **`rustfmt.toml`** — `edition = "2024"`, `imports_granularity = "Item"`. Apply once; standalone commit.
- **`[profile.release]`** — `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`. Plus `[profile.dev] debug = "limited"`.
- **`[workspace.dependencies]` for internal crates** — declare four internal crates once at root; downstream uses `{ workspace = true }`.
- **`lib.rs` as registry** — each library crate's `lib.rs` becomes `//!` doc + `mod` declarations + a curated `pub use` block. Same public surface; no logic in `lib.rs`.
- **Library print refusal** — `#![deny(clippy::print_stdout, clippy::print_stderr)]` at the root of every library crate. The `deadreckon` binary is exempt.
- **Error taxonomy** — add `is_retryable(&self) -> bool` and `is_fatal(&self) -> bool` to each crate's existing `Error` enum. No callers wired; defines vocabulary for a future watchdog rider.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green (`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`) → conventional-commit → CHANGELOG. P11 adds §29 "Workspace Hygiene" to AS-BUILT and amends §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Public-surface golden: `tests/public_surface.rs` lists every `pub use` symbol per library crate (committed at P1); zero drift through P11.
- Behavior smoke: `make smoke` (keyless deterministic run) produces a `RUN-NARRATIVE.md` whose sha256 matches the pre-rider baseline at `tests/.smoke-baseline`.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`.

**Stop when** verification passes, AS-BUILT §29 added, CHANGELOG has a "Workspace hygiene (alpha)" section, committed locally.
