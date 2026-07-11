GOAL: Harden the machinery that hauls child processes — capture output like a ledger, kill like you mean it. deadreckon's whole job is running other programs (provider CLIs via `run_cli`, acceptance checks via `std::process::Command` in `gate.rs`), yet its capture and kill primitives are naive: child stdout is buffered whole (a 100k-line build log lands verbatim in ledgers, provider context, and attach panes), truncation is scattered magic constants, and kill is per-PID — a gate check like `npm test` that spawns a process tree can leave orphans, which the kill/resume release proof depends on not happening. codex-rs solved exactly this: `HeadTailBuffer` keeps 50% head + 50% tail with an explicit omitted-bytes marker, truncation is a typed `TruncationPolicy` not inline numbers, kill semantics live behind a `ChildTerminator` trait (PTY vs raw-PID impls), and children run in process groups so the whole tree dies together. This slice lands those four primitives in a `deadreckon-exec` utility layer and rewires provider capture, gate checks, and the kill path through them. Land this slice named Capstan.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1122-deadreckon-capstan-rider.md` — buffer spec, policy type, terminator trait, process-group rules, eleven phases, depth tests.
- `crates/deadreckon-providers/src/cli_common.rs` — `run_cli`/`run_cli_with_options` (capture + pid_file + cancellation today).
- `crates/deadreckon-core/src/gate.rs` — acceptance checks via `Command::new` (:430/:499/:532); the kill verb path and supervised-pid handling in `crates/deadreckon/src/main.rs`.
- `/Users/gdc/codex/codex-rs/core/src/unified_exec/head_tail_buffer.rs`, `utils/output-truncation/`, `utils/pty/` (`ChildTerminator`, `process_group.rs`).
- `docs/AS-BUILT-ARCHITECTURE.md` §35 (gate), §43 (rescue/durability). Prior riders hold; Capstan takes §53.

**Posture.** Stable track. Behavior-preserving by default: full child output still lands on disk exactly as today (the `output_path` file is the complete record); head+tail truncation applies to what flows onward — ledger rows, provider `content`, attach panes — with the omission marker naming the on-disk full copy. `TruncationPolicy` is a type with named constructors, not config; no new config keys. Process-group placement (`setsid`/`process_group(0)`) applies to children deadreckon spawns; kill escalates SIGTERM → grace → SIGKILL on the group. Unix-first: Windows keeps current per-PID behavior behind the same trait (parity is V1). No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The four primitives.**

- `HeadTailBuffer` — capped, keeps head+tail halves, tracks `omitted_bytes`, renders `[… N bytes omitted; full output: <path>]`.
- `TruncationPolicy` — `ledger()`, `provider_content()`, `pane()` named policies; every truncation site names its policy.
- `ChildTerminator` — trait with `ProcessGroupTerminator` (unix) and `RawPidTerminator`; the kill verb and cancellation tokens speak the trait.
- Process groups — spawn helpers place children in their own group; the release-proof invariant "kill leaves no orphans" becomes depth-tested fact.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §53.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit; characterization goldens unchanged (full-output files byte-identical).
- A gate check spawning a child tree dies entirely on kill (orphan-scan test).
- A 1MB-stdout fixture yields a bounded ledger row with the omission marker while the on-disk file holds all bytes.

**Stop when** verification passes, AS-BUILT §53 + V1-CANDIDATES + a `Capstan (stable)` CHANGELOG section are updated, committed locally.
