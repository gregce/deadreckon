# AS-BUILT-ARCHITECTURE.md

**Subject:** deadreckon — a long-running, BYOK, sandboxed agentic CLI harness in Rust
**Frame:** Reference specification for the **production-release** as-built reality at `/Users/gdc/deadreckon/`. Modeled on `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` (the Printing Press).
**Last updated:** 2026-05-31 (Composable seams, direct-API compaction, Effortless friendliness contract, tamper-evident gate, production-release posture, consolidated plan-result docs, guided first use, local self-improvement loop, provider flight recorder, checkpoint rewind, implementation decision ledger, orchestration live UX, plan event bus feed, coherence closure)
**Maturity:** production-release posture. Workspace version `0.1.0` pending release tagging. Focused build/test/fmt checks are green for the current slice; broad release/stress verification remains an explicit operator choice.

This document captures the system as built today — what's wired, what's load-bearing, where the seams are. It is both a record of the present and a reference an engineer could use to mentally reconstruct deadreckon from first principles.

---

## Table of Contents

1. [System Overview & Mental Model](#1-system-overview--mental-model)
2. [Component Map](#2-component-map)
3. [The Two-Layer Architecture](#3-the-two-layer-architecture)
4. [State Machine & Persistence](#4-state-machine--persistence)
5. [File-System Layout](#5-file-system-layout)
6. [Run Lifecycle & Phase Machine](#6-run-lifecycle--phase-machine)
7. [Locks, Heartbeats, Crash Recovery](#7-locks-heartbeats-crash-recovery)
8. [Atomic Promotion & the Library](#8-atomic-promotion--the-library)
9. [The Turn Loop](#9-the-turn-loop)
10. [Provider Model](#10-provider-model)
11. [Sandbox Model](#11-sandbox-model)
12. [Cancellation & Supervision](#12-cancellation--supervision)
13. [Acceptance Gate & Anti-Self-Attestation](#13-acceptance-gate--anti-self-attestation)
14. [Telemetry: Spend, Traces, Provenance, Events](#14-telemetry-spend-traces-provenance-events)
15. [Resume Semantics](#15-resume-semantics)
16. [Cross-Tool Import](#16-cross-tool-import)
17. [CLI Surface](#17-cli-surface)
18. [TUI (`attach`)](#18-tui-attach)
19. [Configuration & BYOK](#19-configuration--byok)
20. [Testing Strategy](#20-testing-strategy)
21. [Key Design Decisions](#21-key-design-decisions)
22. [What's Built vs Scaffolding-Thin](#22-whats-built-vs-scaffolding-thin)
23. [Glossary](#23-glossary)
24. [Codebase Modes](#24-codebase-modes)
25. [Self-Documenting Runs](#25-self-documenting-runs)
26. [Coherence Pass And Production Command Model](#26-coherence-pass-and-production-command-model)
27. [Overnight UX](#27-overnight-ux)
28. [Chains & Autonomous Goal Chaining](#28-chains--autonomous-goal-chaining)
29. [Workspace Hygiene](#29-workspace-hygiene)
30. [Plans & Multi-Agent Orchestration](#30-plans--multi-agent-orchestration)
31. [Distribution & Self-Update](#31-distribution--self-update)
32. [Plan Observability](#32-plan-observability)
33. [Provider Flight Recorder & Rewind](#33-provider-flight-recorder--rewind)
34. [Local Self-Improvement Loop](#34-local-self-improvement-loop)
35. [Tamper-Evident Gate](#35-tamper-evident-gate)
36. [Campaign Orchestration (one task, N orchestrators)](#36-campaign-orchestration-one-task-n-orchestrators)
37. [Effortless: the friendliness contract](#37-effortless-the-friendliness-contract)
38. [Binary Module Layout (post-decompose)](#38-binary-module-layout-post-decompose)
39. [Composable Seams (swap a worker, keep the gate)](#39-composable-seams-swap-a-worker-keep-the-gate)

---

## 1. System Overview & Mental Model

deadreckon is a Rust 2024 CLI harness whose default flow is **unattended long-running coding tasks**: `deadreckon run <goal>` creates durable run state, picks a BYOK provider route, executes turns inside a platform-native sandbox, writes spend/provenance/trace records after every turn, and exits only when the LLM declares done, a budget caps the run, or the operator kills it. The CLI is the user-facing layer; `deadreckon-runtime` owns orchestration across providers, sandboxes, docs, and promotion; `deadreckon-core` owns deterministic primitives and durable schemas.

```
┌───────────────────────────────────────────────────────────────────────────┐
│ USER                                                                      │
│ deadreckon run "make a hello-world Rust binary" --max-spend 5             │
└────────────────────────────────┬──────────────────────────────────────────┘
                                 │
┌────────────────────────────────▼──────────────────────────────────────────┐
│ CLI LAYER (crates/deadreckon)                                             │
│   cli.rs       clap parser definitions                                    │
│   main.rs      entrypoint, dispatcher, shared root helpers                 │
│   commands/    private command-family modules                             │
│   tui/         private attach render/state layer                           │
└────────────────────────────────┬──────────────────────────────────────────┘
                                 │ create_run → acquire_lock → runtime loop
                                 ▼
┌───────────────────────────────────────────────────────────────────────────┐
│ RUNTIME ORCHESTRATION (crates/deadreckon-runtime)                         │
│                                                                           │
│  turn_loop.rs  provider turns, sandboxed tool dispatch, cancellation,      │
│                acceptance gate invocation, promotion orchestration         │
│  polish.rs     optional end-of-run doc polish via a doc-provider           │
└──┬──────────────────────────┬──────────────────────────┬────────────────┘
   │ durable primitives       │ ProviderRouter::complete  │ run_sandbox
   ▼                          ▼                          ▼
┌──────────────────────────┐  ┌──────────────────────┐  ┌──────────────────────┐
│ CORE LIBRARY             │  │ PROVIDERS            │  │ SANDBOX              │
│ state, paths, locks      │  │ HTTP: anthropic,     │  │ sandbox-exec, bwrap, │
│ artifacts, docs, events  │  │ openai, compatible   │  │ docker, none, auto   │
│ gate, promotion, chains  │  │ CLI: claude, codex   │  │ policy, PID/cancel   │
│ adapter-free schemas     │  │ TEST: smoke          │  │ doctor checks        │
└────────────┬─────────────┘  └──────────┬───────────┘  └──────────┬───────────┘
             │                           │                         │
             └───────────────────────────┴─────────────┬───────────┘
                                                       ▼
┌───────────────────────────────────────────────────────────────────────────┐
│ RUNTIME STATE (~/.deadreckon/)                                            │
│ runstate/<scope>/runs/<run-id>, library/<scope>/<run-id>, locks, config   │
└───────────────────────────────────────────────────────────────────────────┘
```

Why this shape works:

- **The CLI is thin.** It parses args, sets up state, hands off to `deadreckon-runtime`, and prints summaries. Durable schemas and atomic file operations live in `deadreckon-core`.
- **State is on disk before every meaningful change.** `state.json` is atomic-written via temp+rename after every phase transition, snapshot, spend record, and tool call.
- **The agent cannot mark its own gate.** `dr-gate` is a separate binary that signs an acceptance marker against a nonce only it can read; the deadreckon binary refuses to mark a run `Completed` without that signed marker.
- **Sandboxes are platform-native.** macOS uses Seatbelt; Linux uses Bubblewrap; Docker is opt-in. No daemon, no `bollard`, no Lima.
- **BYOK extends to subscriptions.** Subscription-bearing users drive deadreckon by routing turns through their local `claude` or `codex` CLIs; no API key required.

---

## 2. Component Map

### 2.1 Workspace shape

`/Users/gdc/deadreckon/Cargo.toml:1-41`:

- Edition `2024`, resolver `3`, workspace version `0.1.0`.
- Five workspace members:
  - `crates/deadreckon-core` — library for durable state, artifacts, gates, docs, locks, chains, plans, codebase modes, the provider flight recorder, and learning primitives.
  - `crates/deadreckon-runtime` — library for provider/sandbox orchestration, the turn loop, doc polish, and the provider flight recorder.
  - `crates/deadreckon-providers` — library for provider config, adapters, and fallback routing.
  - `crates/deadreckon-sandbox` — library for platform-native sandbox backends and per-tool policy.
  - `crates/deadreckon` — binary (`deadreckon`) + binary (`dr-gate` at `src/bin/dr-gate.rs`).

### 2.2 Crate-by-crate

**`deadreckon-core` (`crates/deadreckon-core/src/lib.rs`).** Re-exports the public surface of the harness primitives. Modules:

| Module | Purpose |
|---|---|
| `artifacts.rs` | `copy_tree`, `snapshot_working`, `restore_snapshot`, `append_{spend,trace,provenance}`, `inventory_files` |
| `cancel.rs` | `CancelMarker`, `write_cancel_marker`, `cancel_marker_present`, run-root cancel checks |
| `chain.rs` | autonomous goal-chain records, `ChainStatus`, `ChainStep`, `ConductorState`, chain event bus |
| `codebase.rs` | `CodebaseMode`, `CodebaseRecord`, worktree/copy/fresh/in-place materialization, `create_worktree` |
| `docs.rs` | run-doc templates (`RUN_NARRATIVE`, `RUN_AS_BUILT`, `RUN_DECISIONS`), frontmatter, inventory, delta JSONL, polish records |
| `error.rs` | `DeadreckonError`, `Result<T>`, `is_retryable()`, `is_fatal()` |
| `events.rs` | `RunEvent`, `RunEventBus`, `RunEventKind`, `tokio::sync::broadcast` channel, JSONL emit |
| `flight.rs` | `FlightManifest`, `FlightSession`, checkpoint policy, provider-log capture, rewind events (`flight-manifest.json`, `flight-events.jsonl`, `rewind-events.jsonl`, `checkpoints/`) |
| `gate.rs` | `AcceptanceMarker`, `AcceptanceSpec`, `AcceptanceCheck`, signature validation, anti-self-attestation, progress JSONL |
| `git.rs` | `run_git`, `git_command`, `hardened_git_prefix`/`hardened_git_argv` — gpg-sign disabling for commit-family verbs |
| `glossary.rs` | user-facing display vocabulary for all status enums (`RunStatus`, `PhaseStatus`, `ChainStatus`, `PlanStatus`, …) |
| `install_receipt.rs` | `Receipt`, `Channel`, install receipt at `~/.deadreckon/install-receipt.json`, channel detection |
| `learning.rs` | learning episodes, signals, insights, proposals, candidates, evals, PR events; files under `~/.deadreckon/learning/` |
| `lock.rs` | `LockState`, `LockGuard`, `acquire_lock`, `heartbeat`, `pid_is_alive` via `nix::kill`, stale detection |
| `paths.rs` | `DeadreckonPaths`, `workspace_scope`, `task_key`, all runtime path resolution including `plans_*`, `chains_*`, `learning_*` |
| `plan.rs` | `Plan`, `PlanTask`, `PlanStatus`, `CoordinatorState`, worker specs, summaries, plan event bus |
| `polish_subcalls.rs` | `PolishSubcallRecord`, `DocProviderSelection`, `DocProviderSource`, `DEFAULT_DOC_SUBSKILLS` (4 narrator skills) |
| `promotion.rs` | `PromotionManifest`, `promote_completed_run`, manifest writing, atomic staging→library rename, crash recovery |
| `state.rs` | `PipelineState`, `RunStatus`, `PhaseId`, `PhaseState`, `create_run`, `load_run`, `atomic_write_json`, `append_json_line` |
| `update_cache.rs` | `Cache`, `update-check.json` at `~/.deadreckon/`, 24-hour TTL staleness check |

**`deadreckon-runtime` (`crates/deadreckon-runtime/src/lib.rs`).** The orchestration layer that depends on core, providers, and sandbox.

| Module | Purpose |
|---|---|
| `turn_loop.rs` | `RunLoopConfig`, `RunLoopOutcome`, `run_turn_loop`, model action parsing, tool dispatch, cancellation, acceptance, and promotion |
| `polish.rs` | `polish_run_docs`, `PolishConfig`, skill resolution, polish input hashing, and nonfatal doc-provider polish |
| `flight.rs` | `ProviderFlightRecorder` — spawns a per-CLI-turn sidecar that polls provider logs + the working tree and writes flight events/checkpoints |
| `error.rs` | runtime `Error` / `Result<T>` |

**`deadreckon-providers` (`crates/deadreckon-providers/src/lib.rs`).** A facade that re-exports the `Provider` trait, config types, HTTP adapter, smoke adapter, and router while keeping implementation modules private.

| Module / file | Adapter |
|---|---|
| `http.rs` | `ProviderAdapter` (HTTP, used by `anthropic` / `openai` / `openai-compatible`) |
| `smoke.rs` | `ScriptedSmokeProvider` (`smoke`, dev-only, deterministic) |
| `router.rs` | `ProviderRouter` — config-driven fallback chain |
| `config.rs` | TOML config loading and built-in provider defaults |
| `types.rs` | public provider request/response/config traits and structs |
| `error.rs` | `ProviderError`, `Result<T>` |
| `cli_claude_code.rs` | `CliClaudeCodeProvider` — shells `claude --dangerously-skip-permissions -p` |
| `cli_codex.rs` | `CliCodexProvider` — shells `codex --ask-for-approval never exec --skip-git-repo-check --sandbox <mode>` |
| `cli_common.rs` | shared subprocess + allowlist machinery |
| `cli_generic.rs` | `GenericCliProvider` — descriptor-template CLI adapter for registered `cli:*` providers |
| `registry/mod.rs` | `ProviderRegistry`, `ProviderDescriptor`, compiled-in TOML descriptors + `providers.d` overrides, probe logic |
| `taxonomy.rs` | provider categorization + normalized tool-label taxonomy |

**`deadreckon-sandbox` (`crates/deadreckon-sandbox/src/lib.rs`).** A facade over sandbox backend resolution, command construction, policy, doctor checks, and subprocess supervision.

| Module | Purpose |
|---|---|
| `backend.rs` | `SandboxBackend`, `SandboxError`, backend resolution |
| `spec.rs` | `SandboxSpec` |
| `commands.rs` | per-backend command/profile construction |
| `policy.rs` | `ToolSandboxPolicy` |
| `doctor.rs` | backend availability checks |
| `process.rs` | `run(SandboxSpec) -> SandboxRunOutput`, PID files, cancellation, SIGTERM/SIGKILL escalation |

**`deadreckon` (binary crate, `crates/deadreckon/src/`).** Clap parser definitions (`cli.rs`), a root entrypoint/dispatcher and shared helpers (`main.rs`), private command-family modules (`commands/`), private attach render/state modules (`tui/`), and `dr-gate` as a standalone acceptance-marker writer (`bin/dr-gate.rs`). Supporting modules: `narrative.rs` (deterministic + provider-backed narrative projection), `plan_event_bus.rs` (`PlanEventBus`/`PlanEventFeed`), `tui_events.rs` (`TuiEventFeed`), `ui.rs` + `ui_card.rs` + `cards/` (CLI/TUI rendering vocabulary and cards), `setup.rs` (provider/done-contract resolution), `prompt.rs` (confirmation prompts), and `sleep.rs` (sleep-prevention).

### 2.3 Top-level documentation

- `README.md` — quickstart.
- `DESIGN.md` — intent + reference patterns (AS-BUILT §3–9, Claude Code mining notes).
- `CHANGELOG.md` — version history.
- `DEPENDENCIES.md` — Tier 1/2/3 rationale per dependency policy.
- `HOWTO.md` — usage guide.
- `docs/AS-BUILT-ARCHITECTURE.md` — this file.
- `docs/AUDIT-2026-05-11.md` — 2026-05-11 audit findings.
- `docs/DEVELOPMENT-README.md` — preserved developer-oriented README notes.
- `docs/GAP-ANALYSIS.md` — outstanding gaps vs. requirements.
- `docs/MULTI-RUN.md` — multi-run sequencing semantics.
- `docs/RELEASE.md` — release runbook.
- `docs/RESUME-SEMANTICS.md` — resume behavior.
- `docs/V1-CANDIDATES.md` — deferred features.
- `docs/design/` — supplemental design docs (`PROVIDER-CLI-INGEST.md`, `USER-FACING-MATRIX.md`).
- `docs/goals/` — 30 dated goal+rider pairs (60 files).

### 2.4 Skills

`/Users/gdc/deadreckon/skills/default-coding/SKILL.md` is the coding-agent skill loaded at runtime. The skill is opaque to the binary; the run records `skill_name` + `skill_path` in `PipelineState` and includes the skill in the prompt frame. Six skills ship today: `default-coding` (the executor), the four doc-polish narrator subskills used by `polish_subcalls.rs` (`narrator-overview`, `narrator-phases`, `narrator-as-built`, `narrator-decisions`), and `run-narrator` (the legacy single-call doc-polish skill). New skills can be added under `skills/<name>/SKILL.md` and selected with `deadreckon run --skill <name>`.

### 2.5 External tools used at runtime

| Tool | Caller | Purpose |
|---|---|---|
| `sandbox-exec` | `deadreckon-sandbox/src/commands.rs` | macOS Seatbelt profile execution |
| `bwrap` | `deadreckon-sandbox/src/commands.rs` | Linux Bubblewrap container |
| `docker` | `deadreckon-sandbox/src/commands.rs` | Opt-in fallback |
| `claude` | `deadreckon-providers/src/cli_claude_code.rs:25` | CLI sub-agent provider |
| `codex` | `deadreckon-providers/src/cli_codex.rs:26` | CLI sub-agent provider |
| `cargo` | `deadreckon-core/src/gate.rs:182,219,260` | Acceptance check for Rust targets |
| `sw_vers` / `uname` / `df` / `ps` | `deadreckon/src/main.rs:645,655,698,1362` | Doctor and PID liveness |

Notably **not** shelled out: `asciinema`, `bollard`. (Demo cast is committed; Docker uses the `docker` CLI directly.)

---

## 3. The Two-Layer Architecture

deadreckon mirrors the Printing Press split (AS-BUILT §3):

| Layer | Owns | Lives in | Language |
|---|---|---|---|
| **Skill** | Agent-facing prose, judgment, prompt frame | `skills/<name>/SKILL.md` (6 skills: `default-coding`, four narrator subskills, `run-narrator`) | Markdown |
| **Binary** | State, locks, sandboxes, providers, gates | `crates/deadreckon*` | Rust |

The skill is invoked indirectly: it sits at the path recorded in `state.skill_path` and is read into the prompt frame by `build_prompt` in `crates/deadreckon-runtime/src/turn_loop.rs`. The binary never reaches into skill internals. New skills can be added under `skills/<name>/SKILL.md` and selected with `deadreckon run --skill <name>`.

This split lets each side do what it's good at:

- The Rust binary enforces invariants — locks, atomic file ops, signed acceptance markers, sandboxed subprocesses.
- The Markdown skill makes judgment calls — what to ask the LLM for, what tool sequence to prefer, when to declare done.

---

## 4. State Machine & Persistence

### 4.1 `PipelineState`

`crates/deadreckon-core/src/state.rs:66-98`:

```rust
pub struct PipelineState {
    pub version: u32,                       // STATE_VERSION = 1 (line 18)
    pub goal: String,
    pub task_key: String,
    pub run_id: String,
    pub scope: String,
    pub status: RunStatus,
    pub current_phase_id: PhaseId,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub cwd: PathBuf,
    pub run_root: PathBuf,
    pub working_dir: PathBuf,
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub sandbox: String,
    pub provider: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub total_spend_usd: f64,
    pub total_wall_seconds: f64,
    pub turn: u32,
    pub pause_reason: Option<String>,
    pub failure_reason: Option<String>,
    pub child_pids: Vec<u32>,
    pub killed_at: Option<DateTime<Utc>>,
    pub promoted_library_dir: Option<PathBuf>,
    pub phases: Vec<PhaseState>,
}
```

### 4.2 Status and phase enums

`state.rs:22-29`:

```rust
pub enum RunStatus {
    Pending, Planned, Executing, Completed, Failed, Killed,
}
```

`state.rs:38-63`:

```rust
pub struct PhaseId(pub u16);

pub struct PhaseState {
    pub id: PhaseId,
    pub name: String,
    pub status: PhaseStatus,    // Pending | Planned | Executing | Completed | Failed
    pub plan_path: Option<PathBuf>,
    pub updated_at: DateTime<Utc>,
}
```

### 4.3 Phase numbering (gap-numbered)

`state.rs:252-273` initializes seven phases for every new run:

| `PhaseId` | name | role |
|---|---|---|
| 0 | `init` | skill load, run-id mint, working-dir create |
| 10 | `plan` | reserved; currently a no-op |
| 20 | `provider` | router built from config, fallback chain resolved |
| 30 | `sandbox` | sandbox backend selected, profile prepared |
| 40 | `execute` | the turn loop runs here; sets `RunStatus::Executing` |
| 50 | `verify` | post-loop verification (currently runs `dr-gate`) |
| 60 | `complete` | acceptance marker validated, promotion atomic-swaps, `RunStatus::Completed` |

The gap-numbering (0, 10, 20 …) leaves room for future phases (e.g., 15, 25, 55) without re-writing on-disk state.

### 4.4 Status transitions

`state.rs:150-169` is the only function that mutates `PipelineState.status`:

```rust
pub fn set_phase_status(&mut self, id: PhaseId, status: PhaseStatus) -> Result<()> {
    let now = Utc::now();
    let phase = self.phases.iter_mut().find(|phase| phase.id == id)
        .ok_or_else(|| DeadreckonError::NotFound(format!("phase {}", id.0)))?;
    phase.status = status;
    phase.updated_at = now;
    self.current_phase_id = id;
    self.updated_at = now;
    self.status = match status {
        PhaseStatus::Executing => RunStatus::Executing,
        PhaseStatus::Failed => RunStatus::Failed,
        PhaseStatus::Completed if id == PhaseId(60) => RunStatus::Completed,
        PhaseStatus::Planned => RunStatus::Planned,
        PhaseStatus::Pending | PhaseStatus::Completed => self.status,
    };
    Ok(())
}
```

Key rule: `RunStatus::Completed` is reachable **only** through `Phase(60).Completed`, which the loop refuses to set without `validate_acceptance_marker()` passing (§13).

### 4.5 Atomic persistence

`state.rs:435-445`:

```rust
pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(...)?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;   // fsync before rename
    persist_temp(temp, path)
}
```

Every state-changing function ends with `save_state(state)?`. Crashes never leave a half-written `state.json`.

JSONL files (`spend.jsonl`, `traces.jsonl`, `provenance.jsonl`, `events.jsonl`) use `append_json_line` (`state.rs:457-470`): open in append mode, write line + newline, `sync_all`.

### 4.6 Schema versioning

`STATE_VERSION = 1` (`state.rs:18`) gates future migrations. `load_state` rejects unknown versions; migrations would land here.

---

## 5. File-System Layout

### 5.1 Source tree (`/Users/gdc/deadreckon/`)

```
/Users/gdc/deadreckon/
├── Cargo.toml                    # workspace
├── Cargo.lock
├── README.md / DESIGN.md / CHANGELOG.md / DEPENDENCIES.md / HOWTO.md / RELEASE.md
├── Makefile / dist-workspace.toml / clippy.toml / rustfmt.toml
├── crates/
│   ├── deadreckon-core/          # durable state, locks, gates, docs, artifacts, flight, learning, plans, chains
│   ├── deadreckon-runtime/       # turn loop, sandbox dispatch, doc polish, flight recorder
│   ├── deadreckon-providers/     # provider trait + adapters + registry/descriptors
│   ├── deadreckon-sandbox/       # platform-native sandboxes
│   └── deadreckon/               # CLI binary + dr-gate binary + TUI + tests
├── skills/
│   ├── default-coding/SKILL.md
│   ├── narrator-overview/SKILL.md
│   ├── narrator-phases/SKILL.md
│   ├── narrator-as-built/SKILL.md
│   ├── narrator-decisions/SKILL.md
│   └── run-narrator/SKILL.md
├── npm/                          # npm wrapper + 5 platform packages
├── release/                      # Homebrew formula patch script
├── tests/                        # workspace-level guards (hygiene_config, public_surface, smoke_invariant)
├── docs/
│   ├── AS-BUILT-ARCHITECTURE.md  # this file
│   ├── AUDIT-2026-05-11.md
│   ├── DEVELOPMENT-README.md
│   ├── GAP-ANALYSIS.md
│   ├── MULTI-RUN.md
│   ├── RELEASE.md
│   ├── RESUME-SEMANTICS.md
│   ├── V1-CANDIDATES.md
│   ├── assets/
│   ├── design/                   # PROVIDER-CLI-INGEST.md, USER-FACING-MATRIX.md
│   └── goals/                    # 30 dated goal+rider pairs (60 files)
├── demo.cast / demo-codebase.cast / demo-self-documenting.cast
└── target/
```

### 5.2 Runtime tree (`/Users/gdc/.deadreckon/`)

```
/Users/gdc/.deadreckon/
├── config.toml                   # BYOK keys, defaults, fallback chain
├── runstate/
│   └── <scope>/                  # scope = "<repo-basename>-<fnv1a32-hash>"
│       ├── current/
│       │   └── <task_key>.json   # CurrentRunPointer for the active run per task
│       └── runs/
│           └── <run_id>/         # uuid simple-form
│               ├── state.json
│               ├── working/      # before promotion; library/... after
│               ├── snapshots/
│               │   └── turn-<N>/  # per-turn full working-tree copy
│               ├── turns/
│               │   └── turn-<N>/  # provider stdout, prompt.md
│               ├── proofs/
│               │   ├── turn-acceptance.json       # AcceptanceMarker
│               │   └── acceptance-progress.jsonl  # streaming AcceptanceProgressEntry
│               ├── gate/
│               │   └── nonce      # uuid; only dr-gate can read for signing
│               ├── child-pids/
│               │   └── *.pid      # per-subprocess PID files
│               ├── sandbox/       # per-run Seatbelt profile (if mac) or bwrap args
│               ├── traces.jsonl
│               ├── provenance.jsonl
│               ├── spend.jsonl
│               ├── events.jsonl
│               ├── history.json          # narrative tool-call summaries
│               ├── acceptance.yaml       # optional spec for dr-gate
│               ├── flight-manifest.json  # provider flight recorder manifest
│               ├── flight-events.jsonl   # normalized provider-native rows
│               ├── rewind-events.jsonl   # rewind preview/apply/refusal log
│               ├── checkpoints/
│               │   └── <checkpoint-id>/  # delta checkpoint (files/ + manifest.json)
│               └── narrative/            # attach narrative projection (state.json, snapshots.jsonl, architecture-graph.json)
├── locks/
│   └── <scope>--<task_key>.lock   # one lock per task
├── library/
│   └── <scope>/
│       └── <run_id>/
│           ├── manifest.json      # PromotionManifest
│           └── ...                # promoted working tree
├── chains/
│   └── <chain-id>/                # chain.json, chain-events.jsonl, conductor.json
├── plans/
│   └── <plan-id>/                 # plan.json, coordinator.json, messages.jsonl, plan-events.jsonl, worker-specs/, summaries/, docs/PLAN-*.md, merge-proofs/
├── worktrees/
│   └── <scope>-<run-id-prefix>/   # git worktree for worktree-mode runs
├── learning/                      # episodes/, signals.jsonl, insights.jsonl, proposals/, candidates/, evals/, bundles/, pr-events.jsonl, policy.toml
├── install-receipt.json           # install channel receipt (npm|brew|shell|cargo|source)
└── update-check.json              # 24-hour TTL update-check cache
```

The split between `runstate/` (mutable working state) and `library/` (durable promoted artifacts) is deliberate: `runstate/` is per-scope and ephemeral; `library/` is global and intended to outlive cleanup. The `state.working_dir` field starts pointing at `runstate/.../working/` and is rewritten to `library/<scope>/<run_id>/` after promotion.

### 5.3 Path derivation

`crates/deadreckon-core/src/paths.rs:14-195` exposes (key methods):

```rust
impl DeadreckonPaths {
    pub fn home(&self) -> &Path                              // ~/.deadreckon (or DEADRECKON_HOME)
    pub fn config_path(&self) -> PathBuf                     // home/config.toml
    pub fn runstate_dir(&self) -> PathBuf                    // home/runstate
    pub fn scope_root(&self, scope: &str) -> PathBuf
    pub fn current_pointer_path(&self, scope, task_key) -> PathBuf
    pub fn run_root(&self, scope, run_id) -> PathBuf
    pub fn locks_dir(&self) -> PathBuf                       // home/locks
    pub fn library_dir(&self, scope, run_id) -> PathBuf
    // … plus plan_dir/plan_events, chain_dir, learning_*, install-receipt, and update-check helpers
}
```

Scope (`paths.rs:197-216`) derives from `DEADRECKON_SCOPE_ROOT` env var, or the nearest `.git` root, or `cwd`. The literal scope string is `"<sanitized-basename>-<fnv1a32-hex>"` of the canonical path — unique per worktree, stable per checkout.

Task key (`paths.rs:218-229`) is `"<slug-of-goal>-<fnv1a32-hex-of-goal>"` (slug capped at 48 chars). Two runs with the same goal share a task key (and a lock).

---

## 6. Run Lifecycle & Phase Machine

Sequence for a typical `deadreckon run` invocation:

```
main.rs run_command()
  ↓
  paths = DeadreckonPaths::discover()
  ↓
  state = create_run(paths, RunOptions{...})       # state.rs:178-231
  │   ├── mint run_id (uuid simple form)
  │   ├── derive scope (paths.rs:67) and task_key (paths.rs:88)
  │   ├── create run_root/working/snapshots/proofs/gate/turns
  │   ├── write gate/nonce (uuid)
  │   ├── initialize 7 phases (state.rs:233-254)
  │   ├── write CurrentRunPointer
  │   └── save_state()
  ↓
  lock = acquire_lock(paths, task_key, run_id, scope, "run", 30min)
  │   └── lock.rs:89-158
  ↓
  state.child_pids = vec![cli_pid]; save_state()
  ↓
  set_phase_status(20, Completed); save_state()   # provider built
  set_phase_status(30, Completed); save_state()   # sandbox resolved
  ↓
  outcome = run_turn_loop(state, router, RunLoopConfig{...})
  ↓
  state.child_pids.clear(); save_state()
  lock.release()
  ↓
  match outcome { Done | PausedAtCap | Killed | Failed }
  print_run_locations(state)
```

`Completed` is only reached if the turn loop emits `RunLoopOutcome::Done`, which itself requires:

1. The loop saw `Action::Done` (or a CLI sub-agent finished with file changes), **and**
2. `acceptance_gate_passed_or_record_failure(state, ...)` returned `true` after invoking `dr-gate` and validating the signed marker (`turn_loop.rs:1442`). Gate failures are non-terminal: the helper logs them and returns `false`, letting the loop continue until the gate passes or the turn budget is exhausted (§13.6), **and**
3. `promote_if_ready(state)` swapped `working/` → `library/<scope>/<run_id>/`, **and**
4. `set_phase_status(PhaseId(60), Completed)` ran (which is the only path to `RunStatus::Completed`).

If gate failures accumulate during a run, `state.failure_reason` accumulates a chained record (e.g. `"acceptance failed after turn 3: ...; acceptance failed after turn 5: ...; max turn budget exhausted"`).

---

## 7. Locks, Heartbeats, Crash Recovery

### 7.1 `LockState`

`crates/deadreckon-core/src/lock.rs:15-24`:

```rust
pub struct LockState {
    pub task_key: String,
    pub run_id: String,
    pub scope: String,
    pub phase: String,
    pub pid: u32,
    pub acquired_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Lock file lives at `~/.deadreckon/locks/<scope>--<task_key>.lock` (`lock.rs:86`).

### 7.2 Acquisition

`lock.rs:89-158`:

1. Create the `.lock` file.
2. `fs2::FileExt::try_lock_exclusive` for OS-level advisory lock.
3. If `EWOULDBLOCK`, read the existing `LockState` from disk and return `LockHeld` immediately — an `fs2` advisory lock that won't acquire is held by a *live* process.
4. If the OS lock **is** acquired, read any existing `LockState` from the file (a crashed holder's advisory lock is auto-released on death, so its file may linger). Stale detection: `acquired_at` age > `DEFAULT_STALE_AFTER` (30 min, `lock.rs:13`) **or** `pid_is_alive(pid)` returns false. PID liveness is `nix::sys::signal::kill(pid, 0)` (`lock.rs:237-248`): `ESRCH` → dead, `EPERM` → alive. A different, live, non-stale holder causes the OS lock to be released and `LockHeld` returned.
5. Write the new `LockState` to disk.
6. Return a `LockGuard` that releases on drop.

### 7.3 Heartbeat

`lock.rs:47-51`:

```rust
pub fn heartbeat(&mut self, phase: impl Into<String>) -> Result<()> {
    self.state.phase = phase.into();
    self.state.updated_at = Utc::now();
    write_lock_state_to_file(&mut self.file, &self.path, &self.state)
}
```

Updates `updated_at` to keep stale-detection at bay. Callers in the turn loop tag the current sub-phase (e.g., `"executing-turn-3"`).

### 7.4 Release

`lock.rs:53-76`. Drops the file lock, deletes the lock file, gracefully handles `NotFound`.

### 7.5 Crash recovery

If the process holding the lock dies, the lock file persists but `pid_is_alive(pid)` will return false. The next caller sees the stale state and reclaims it. The new caller writes its own PID; previous run state survives intact in `runstate/<scope>/runs/<dead_run_id>/`.

---

## 8. Atomic Promotion & the Library

### 8.1 `PromotionManifest`

`crates/deadreckon-core/src/promotion.rs:17-25`:

```rust
pub struct PromotionManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub scope: String,
    pub goal: String,
    pub promoted_at: DateTime<Utc>,
    pub source_working_dir: PathBuf,
    pub provenance_hash: String,   // hash of provenance.jsonl bytes
}
```

### 8.2 Promotion flow

`promotion.rs:27-76`:

1. **Guard.** `validate_acceptance_marker(state)?` — refuses if marker missing / wrong run_id / unsigned.
2. **Recovery.** `recover_promotion()` — idempotent if a previous attempt half-completed (§8.4).
3. **Idempotency check.** If `library/<scope>/<run_id>/manifest.json` already exists, update state and return — no work to do.
4. **Staging.** Create `library/<scope>/.{run_id}.promoting/` (parent dir created if needed).
5. **Move/Copy.** If `working_dir` is the run's own `working/` dir, `fs::rename(working_dir, staging)` — atomic on same filesystem. Otherwise (worktree/in-place modes, where `working_dir` lives elsewhere) `copy_tree(working_dir, staging)`.
6. **Manifest.** Write `manifest.json` inside staging.
7. **Final rename.** `fs::rename(staging, library/<scope>/<run_id>/)` — atomic.
8. **State update.** `state.working_dir = library_dir`; `state.promoted_library_dir = Some(library_dir)`; `save_state()`.

### 8.3 Where promotion happens

In `crates/deadreckon-runtime/src/turn_loop.rs`, **before** `set_phase_status(PhaseId(60), Completed)`. If promotion fails, the run never reaches `Completed`. The `working/` directory is the source of truth until promotion; after promotion, the library copy is canonical and `working/` is gone.

### 8.4 Crash recovery between rename steps

`promotion.rs:78-97` handles the half-completed states:

- If `staging` exists and final dir doesn't: complete the rename.
- If both exist: the final rename happened but didn't atomically remove staging; clean up staging.
- If final dir exists but its `manifest.json` is missing: write the manifest.

This makes promotion crash-safe across a `kill -9` between the two renames.

---

## 9. The Turn Loop

The load-bearing function lives in `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs`.

### 9.1 `RunLoopConfig`

`crates/deadreckon-runtime/src/turn_loop.rs` defines:

```rust
pub struct RunLoopConfig {
    pub provider: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub sandbox_backend: SandboxBackend,
    pub max_turns: u32,             // hard cap, currently 12
    pub from_turn: Option<u32>,     // resume override
    pub event_sender: Option<broadcast::Sender<RunEvent>>,
    pub cancellation_token: Option<CancellationToken>,
    pub docs: RunLoopDocsConfig,    // doc-polish settings (see below)
}

// RunLoopDocsConfig is resolved before the loop and carries:
//   home, config_path, doc_provider (+ doc_provider_source), doc_subskills,
//   token_budget, budget_cap_usd, doc_skill, no_docs.
```

### 9.2 Top-level signature

The top-level signature is:

```rust
pub async fn run_turn_loop(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: RunLoopConfig,
) -> Result<RunLoopOutcome>
```

### 9.3 Loop body (paraphrased; see `crates/deadreckon-runtime/src/turn_loop.rs`)

```text
load_or_reconstruct_history()      # load history.json, else replay traces.jsonl
set Phase(40)=Executing
save_state(); save_history()

for turn in (state.turn+1)..=max_turns:
    if cancelled or status=Killed:
        return Killed
    emit TurnStarted event
    snapshot_working(turn-1)        # capture pre-turn state
    prompt = build_prompt(state, history) [or build_cli_subagent_prompt]
    request = ProviderRequest{
        prompt, max_output_tokens=2048, cwd, output_path, sandbox_backend,
        pid_file=run_root/child-pids/provider-turn-N.pid,
        cancellation_token=turn_token,
    }
    if provider is cli:*: flight = ProviderFlightRecorder::start().spawn()  # sidecar polls provider logs + tree
    response = router.complete(&request).await
    if provider is cli:*: flight.finish(status)   # writes flight-events.jsonl + checkpoints
    append_trace(llm.complete, latency, response.trace)
    state.total_spend_usd += response.spend.cost_usd
    state.total_wall_seconds += response.spend.wall_time_seconds
    append_spend(...)
    if max_spend exceeded: return PausedAtCap
    if subscription and max_wall_seconds exceeded: return PausedAtCap

    if response is cli_subagent:
        changed = changed_files_since_snapshot(turn-1)
        if changed.is_empty: return Failed
        snapshot_working(turn)
        append_trace(tool.cli_subagent, files=changed)
        append_provenance_for_files(turn, tool_call_id, model, changed)
        commit_worktree_turn(turn); append turn-doc checkpoint
        if !implementation_notes_ready_or_request_followup(): continue   # nudge, do not fail
        complete_run_docs(state, router, config)                         # deterministic + optional polish
        if !acceptance_gate_passed_or_record_failure(): continue         # dr-gate; non-terminal (§13.6)
        promote_if_ready(state)           # working → library
        set Phase(60)=Completed
        return Done

    parse Action from response.content (serde tag="action"):
        Bash { tool_call_id, command } =>
            run_sandbox(SandboxSpec{
                program=sh, args=[-lc, command], cwd=working_dir,
                pid_file, cancellation_token=turn_token.child_token(),
                read_allowlist=[working_dir],
                allow_network=false,
            }).await
            append_trace(tool.bash, stdout, stderr, status)
            snapshot_working(turn); append_provenance(turn, changed_files)
            history.push("tool {id} result: status={...}")
        WriteFile { tool_call_id, path, content } =>
            safe_working_path(path); write_file
            append_trace(tool.write_file); snapshot_working(turn)
            append_provenance(turn, [path])
            history.push("tool {id} result: wrote file")
        Done { summary } =>
            append turn-doc checkpoint
            if !implementation_notes_ready_or_request_followup(): continue
            complete_run_docs(state, router, config)
            if !acceptance_gate_passed_or_record_failure(): continue      # non-terminal (§13.6)
            promote_if_ready(state)
            set Phase(60)=Completed
            return Done

    state.turn = turn; save_history(); save_state()

# loop exited via max_turns
state.failure_reason = Some("max turn budget exhausted")
set Phase(40)=Failed; save_state()
return Failed
```

### 9.4 Action enum

Inline in `crates/deadreckon-runtime/src/turn_loop.rs` (paraphrased):

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Action {
    Bash { tool_call_id: String, command: String },
    WriteFile { tool_call_id: String, path: PathBuf, content: String },
    Done { summary: Option<String> },
}
```

Providers return JSON; the loop parses one action per turn. The CLI sub-agent path is detected by `response.trace["kind"] == "cli_subagent"` before action parsing — those providers do their own tool calls inside the subprocess and return a narrative, not an action JSON.

### 9.5 No smoke fallback in the default path

`grep -r "coding_turn_script\|hardcoded_smoke" /Users/gdc/deadreckon/crates/` returns empty. The deterministic-script path lives entirely inside `ScriptedSmokeProvider` (`crates/deadreckon-providers/src/smoke.rs`), reachable only via `deadreckon run --smoke` (which selects the `smoke` provider, not via a bypass of the run loop).

### 9.6 Error handling

The loop does **not** retry on errors:

- Provider error → propagates and the run fails.
- Tool-call non-zero exit → result fed back to history, next turn's prompt sees the failure; the model decides whether to retry.
- Acceptance failure → run fails; no auto-retry.

The bound is `max_turns` (currently 12), not an error budget.

---

## 10. Provider Model

### 10.1 The `Provider` trait

`crates/deadreckon-providers/src/types.rs`:

```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
    fn has_credential(&self) -> bool;
    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate;
    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a>;
}
```

`ProviderRequest` (`types.rs:89`) carries: `prompt`, `max_output_tokens`, optional `cwd`, optional `output_path`, optional `sandbox_backend`, optional `pid_file`, optional `cancellation_token`.

`ProviderResponse` (`types.rs:100`) carries: `provider`, `model`, `content`, `usage`, `spend`, `trace` (JSON value).

### 10.2 Kinds

`types.rs:18`:

```rust
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    CliClaudeCode,
    CliCodex,
    ScriptedSmoke,
    Generic(String),
}
```

### 10.3 HTTP adapters

A single `ProviderAdapter` (`http.rs:12-211`) handles all three HTTP kinds via shared `reqwest::Client` (`lib.rs` is a ~309-line facade of `pub use` re-exports + tests, no logic):

| Kind | Endpoint | Auth header | Default model |
|---|---|---|---|
| Anthropic | `{base_url}/v1/messages` | `x-api-key: <key>` + `anthropic-version: 2023-06-01` | `claude-sonnet-4-5` |
| OpenAI | `{base_url}/chat/completions` | `Authorization: Bearer <key>` | configurable |
| OpenAI-compatible | `{base_url}/chat/completions` | `Authorization: Bearer <key>` | configurable |

Pricing defaults come from the registry descriptor model catalogs (e.g. `descriptors/anthropic.toml`, `descriptors/openai.toml`) and are fed into each `ProviderEntry` by `config.rs`; `http.rs` falls back to $0/$0 when a model carries no catalog price. Anthropic `claude-sonnet-4-5`: $3/$15 per million in/out; OpenAI: $1.25/$10 per million; OpenAI-compatible: user-configured.

Response parsing: `parse_anthropic_response` (`http.rs:260-277`) extracts `content[0].text` + `usage.{input_tokens, output_tokens}`; `parse_openai_response` (`http.rs:241-258`) extracts `choices[0].message.content` + `usage.{prompt_tokens, completion_tokens}`. The `Action` tag-typed enum is parsed in the **turn loop**, not the provider; providers return text.

Cancellation: `tokio::select!` on `token.cancelled()` vs `client.post().send()` (`http.rs:119-134`).

### 10.4 CLI sub-agent adapters

**`cli:claude-code` (`crates/deadreckon-providers/src/cli_claude_code.rs:1-142`).** Invocation:

```zsh
claude [--model <model>] --dangerously-skip-permissions -p "<prompt>"
```

The `--dangerously-skip-permissions` flag is non-negotiable (no human is in the loop). The subprocess runs **inside** the deadreckon sandbox profile, so Claude's bypass only disables its own permission gate; deadreckon's outer Seatbelt/bwrap still scopes the process. Stdout is captured to `request.output_path` (the turn's `claude.out`).

**`cli:codex` (`crates/deadreckon-providers/src/cli_codex.rs:1-164`).** Invocation:

```zsh
codex --ask-for-approval never exec --skip-git-repo-check --sandbox <mode> -- "<prompt>"
```

`<mode>` (`cli_codex.rs:121-131`) is `workspace-write` when the outer sandbox is `None`/`SandboxBackend::None` (safer, codex limits itself to cwd), and `danger-full-access` when an outer sandbox is active (the outer sandbox is doing the isolating; codex needs full filesystem access inside).

The trailing `--` delimiter is non-negotiable: doc-polish prompts often begin with YAML frontmatter (`---`), which `clap`-based Codex CLIs otherwise interpret as an option-like argument. Adding `--` forces the prompt to be parsed as the positional value.

**Descriptor-backed CLI providers.** The provider registry now owns compiled-in TOML descriptors plus `providers.d` overrides. Generic CLI descriptors (`ProviderKind::Generic(id)` where the descriptor kind is `cli`) are launched by `GenericCliProvider`, which renders `exec_template.args_template` with `{prompt}`, `{sandbox}`, and `{cwd}` placeholders and applies the descriptor `model_arg` near the prompt without splitting prompt-value flags like `-p <prompt>`. `cli:gemini`, `cli:opencode`, `cli:copilot`, and `cli:pi` are built-in generic CLIs; `cli:claude-code` and `cli:codex` remain concrete adapters for compatibility with their established launch quirks. Copilot launches as `copilot -p <prompt> --output-format json --stream off --no-color --allow-all`; Pi launches as `pi --mode json --print <prompt>` so its default saved sessions remain available to the TUI.

**Shared subprocess machinery (`cli_common.rs:22-120`).** Builds a `SandboxSpec` with explicit allowlists (`cli_common.rs:154-166`):

- Write allowlist: descriptor `sandbox_writes` for registered CLIs, with concrete compatibility fallbacks for codex and claude.
- Read allowlist: binary location + `~/.bun`, `~/.local`, `~/.npm-global`, `~/.opencode`.
- `allow_network: true` (CLI agents need outbound for their own API calls).

**Spend (subscription).** Per `cli_claude_code.rs:100-110` and the equivalent in `cli_codex.rs`:

```rust
SpendEstimate {
    cost_usd: 0.0,
    subscription: true,
    wall_time_seconds: Some(elapsed),
}
```

The wall-clock is what `--max-wall-seconds` caps (§9.3).

### 10.5 Scripted smoke provider

`crates/deadreckon-providers/src/smoke.rs`. In-memory `VecDeque<String>` initialized with three responses:

1. `{"action": "bash", "tool_call_id": "smoke-bash-1", "command": "..."}`
2. `{"action": "write_file", "tool_call_id": "smoke-write-2", "path": "README.md", "content": "..."}`
3. `{"action": "done", "summary": "tiny Rust project created"}`

Zero cost, no subscription. Reachable only via `--smoke` flag. The trace records `{"kind": "scripted_smoke", "remaining_steps": N}`.

### 10.6 `ProviderRouter` and fallback chain

`crates/deadreckon-providers/src/router.rs`. Reads config (TOML), loads the provider registry with `providers.d` overrides, resolves a route list (`default_provider` leads if set, then `fallback` entries deduped, then the built-in chain `cli:claude-code` → `cli:codex` → `anthropic` → `openai` → `openai-compatible` only when neither is configured; see `configured_route_names`), and constructs a `Box<dyn Provider>` per route. Concrete providers handle Anthropic/OpenAI/OpenAI-compatible/smoke/Codex/Claude; descriptor-backed generic CLI providers handle any registered CLI descriptor that does not need a concrete adapter. On `complete()`:

```rust
for route in &self.routes {
    if !route.has_credential() { failures.push(...); continue; }
    match route.complete(request).await {
        Ok(resp) => return Ok(resp),
        Err(e) => failures.push(format!("{}: {e}", route.name())),
    }
}
Err(ProviderError::NoRoute(failures.join("; ")))
```

First credentialed route to succeed wins. All errors aggregate into the failure message.

---

## 11. Sandbox Model

`crates/deadreckon-sandbox/src/lib.rs` is a public facade over focused modules that abstract four backends behind one entry point.

### 11.1 `SandboxBackend`

`crates/deadreckon-sandbox/src/backend.rs`:

```rust
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    Auto,
    SandboxExec,
    Bwrap,
    Docker,
    None,
}
```

### 11.2 Auto resolution

`backend.rs:93-128` (`resolve_backend`). Platform-conditional:

- macOS: probes `sandbox-exec` via `which`. Falls back to `None` with a warning if unavailable.
- Linux: probes `bwrap`. Falls back to `None` with a warning.
- Other: `None` with platform-unavailable warning.

The fallback to `None` is loud — the warning ends up in `SandboxRunOutput.warning` and is surfaced in the trace.

### 11.3 `run(SandboxSpec) -> SandboxRunOutput`

`process.rs:22-80` (`run`) is the single dispatch entry point. It:

1. Calls `build_command(spec)` (`commands.rs:18`) to construct the per-backend invocation.
2. Spawns the child via `tokio::process::Command`, capturing stdout + stderr piped.
3. Persists the child PID to `spec.pid_file` (`process.rs:36-41`) for `kill` supervision.
4. Reads stdout/stderr in parallel async tasks (`read_pipe`, `process.rs:82-92`).
5. Runs the cancellation `tokio::select!` (`process.rs:42-62`).
6. Returns `SandboxRunOutput { stdout, stderr, status_code, pid, backend, warning }`.

### 11.4 macOS Seatbelt profile

`commands.rs:149-199` generates a per-run profile string:

```
(version 1)
(allow default)
{ssh_deny}                    ; deny file-read* / file-write* (subpath "$HOME/.ssh")
{network}                     ; (deny network*) unless allow_network=true
(allow file-read*
{read_rules})                 ; cwd + system framework subpaths + spec.read_allowlist
(allow file-write*
    (subpath "<cwd>")
    (subpath "/private/tmp")
    (subpath "/tmp")
{write_rules})                ; spec.write_allowlist
```

Optionally writes the profile to `spec.profile_dir` for debugging (`commands.rs:195-199`). Otherwise inline via `sandbox-exec -p '<profile>' -- <program> <args...>`.

### 11.5 Linux Bubblewrap

`commands.rs:62-116`. Constructs `bwrap` args with:

- `--die-with-parent --unshare-pid --unshare-ipc --unshare-uts`
- `--tmpfs <cwd>/.deadreckon-home` and `--setenv HOME <cwd>/.deadreckon-home` (ephemeral tmpfs `$HOME`)
- `--ro-bind <path> <path>` for each entry in `system_read_allowlist(cwd, spec.read_allowlist)`
- `--bind <path> <path>` for each entry in `spec.write_allowlist`
- `--bind <cwd> <cwd>`, `--proc /proc`, `--dev /dev`, `--chdir <cwd>`
- `--unshare-net` unless `allow_network=true`

### 11.6 Docker

`commands.rs:118-147`. Constructs `docker run --rm -v <cwd>:<cwd> -w <cwd> [--network none] [-e KEY=VAL]... rust:1 <program> <args...>`. Hardcoded base image is `rust:1`. Only the cwd is mounted.

### 11.7 None

`commands.rs:26-39`. No isolation. Always returns a warning: `"sandbox backend none is unsafe; use only for explicit local verification"`. The warning lands in the trace.

### 11.8 SIGTERM/SIGKILL escalation

`process.rs:42-58`:

```rust
if let Some(token) = spec.cancellation_token.as_ref() {
    tokio::select! {
        _ = token.cancelled() => {
            if let Some(pid) = pid {
                signal_pid(pid, false);                      // SIGTERM
                sleep(Duration::from_secs(2)).await;         // 2s grace
                if child.try_wait()?.is_none() {
                    signal_pid(pid, true);                   // SIGKILL
                }
            }
            let _ = child.wait().await;
            if let Some(pid_file) = spec.pid_file.as_ref() {
                let _ = tokio::fs::remove_file(pid_file).await;
            }
            return Err(SandboxError::Cancelled);
        }
        status = child.wait() => status
    }
}
```

`signal_pid` is defined at `process.rs:95-108` (uses `nix::sys::signal::kill`).

---

## 12. Cancellation & Supervision

### 12.1 Cancellation tokens

The turn loop owns `run_token = CancellationToken::new()`; per turn it creates `turn_token = run_token.child_token()`; per tool call it can further create `tool_token = turn_token.child_token()`. The token threads through:

- HTTP providers: `tokio::select!` on `token.cancelled()` vs the `reqwest` future.
- CLI providers: passed to `cli_common::run_cli` → `SandboxSpec.cancellation_token` → the sandbox layer.
- Tool dispatch (`Action::Bash`): passed to `run_sandbox(SandboxSpec { cancellation_token: Some(tool_token), ... })`.

### 12.2 Child PID supervision

Multiple sources contribute PIDs to track:

- `state.child_pids: Vec<u32>` — populated by the CLI itself (`main.rs:333`).
- Per-subprocess PID files at `run_root/child-pids/*.pid` — written by the sandbox layer (`lib.rs:129`) and read by `supervised_pids()` (`main.rs:923`).

`kill_command` reads both, releases the lock, sets `state.status = Killed`, and signals every PID. Without `--escalate`, it tries the graceful signal, waits 1.5s, then SIGKILLs anything still alive.

### 12.3 What `kill` cannot do

`kill` cannot reach directly into another process's in-memory cancellation token, so it bridges across processes with a **durable cancel marker**: `kill` writes `cancel.marker` under the run root before signaling child PIDs, and the running turn loop's `CancelMarkerGuard` polls for it (~50 ms) and trips the run-level `CancellationToken` while a provider call is in flight. Killing the children additionally makes the provider/sandbox error out, which the loop also propagates as a failure. The marker path means a cross-process `kill` deterministically stops the loop and records killed status through events, rather than relying solely on child-process death.

---

## 13. Acceptance Gate & Anti-Self-Attestation

### 13.1 The principle

The agent (LLM) **cannot** be trusted to declare a run done. `Completed` is reachable only via an acceptance marker signed by an external binary that the agent does not have keys to.

### 13.2 `AcceptanceMarker`

`crates/deadreckon-core/src/gate.rs:20-33`:

```rust
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,           // "pass" | "fail"
    pub produced_by: String,      // must be "dr-gate"
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
    pub signature: String,        // hash over fields + nonce + checks + tamper file bytes
    pub check_count: usize,
    #[serde(default)]
    pub checks: Vec<AcceptanceCheckResult>,  // per-check evidence, also covered by the signature
}
```

Marker location: `<run_root>/proofs/turn-acceptance.json`. The marker remains schema 1; tamper evidence is an external proof file, not a marker field.

### 13.3 `dr-gate` binary

`crates/deadreckon/src/bin/dr-gate.rs` (73 lines). Standalone binary that:

1. Reads `--run <id>` and `--working-dir <path>`.
2. Loads `acceptance.yaml` (if present) from the run root.
3. Runs each check (cargo test, file-exists, content-match, build-success, shell).
4. Evaluates `tamper.rs` against the compiled checks, provenance, and the first snapshot diff, then writes `proofs/acceptance-tamper.json` for clean, caveat, and refuse verdicts.
5. If the tamper verdict is `refuse`, prints the refusal reasons and exits nonzero without writing a marker.
6. If all required checks pass and the tamper verdict is `clean` or `caveat`, writes a signed `AcceptanceMarker` to `proofs/turn-acceptance.json`.

The marker's `signature` is computed from `gate/nonce` (a UUID written at run-init time by the deadreckon binary at `state.rs:188`), marker fields, check results, and the raw bytes of `proofs/acceptance-tamper.json` (empty bytes if absent for backward tolerance). Only `dr-gate` reads the nonce; the agent never has access. Editing the tamper proof after signing invalidates marker validation.

### 13.4 Validation

`gate.rs:124-154`:

```rust
pub fn validate_acceptance_marker(state: &PipelineState) -> Result<AcceptanceMarker> {
    let path = marker_path(state);
    let raw = std::fs::read(&path).with_path(&path)?;
    let marker: AcceptanceMarker = serde_json::from_slice(&raw).with_json_path(&path)?;
    if marker.schema_version != 1 { return Err(...); }
    if marker.run_id != state.run_id {
        return Err("acceptance marker run_id ... does not match");
    }
    if marker.status != "pass" || marker.produced_by != "dr-gate" {
        return Err("acceptance marker was not produced by dr-gate with pass status");
    }
    let expected = marker_signature(&state.run_root, &marker)?;
    if marker.signature != expected {
        return Err("acceptance marker signature is invalid; forged self-attestation refused");
    }
    Ok(marker)
}
```

A forged marker by the agent fails the signature check (`gate.rs:149`).

### 13.5 Streaming progress

While checks are running, `evaluate_acceptance_checks_with_progress` (`gate.rs:233-249`) appends one `AcceptanceProgressEntry` per state transition to `proofs/acceptance-progress.jsonl`:

```rust
pub struct AcceptanceProgressEntry {
    pub checked_at: DateTime<Utc>,
    pub status: String,        // "started" | "running" | "passed" | "failed"
    pub index: usize,
    pub total: usize,
    pub result: Option<AcceptanceCheckResult>,
}
```

The progress file is truncated at the start of each evaluation so resumed/extended runs do not mix with prior attempts. The attach TUI tails this file (§18) so operators can see acceptance advance from "running 2/5" → "passed 2/5" while `dr-gate` works. The signed `AcceptanceMarker` is still the load-bearing artifact; progress is observational telemetry only.

### 13.7 Tamper evidence

The gate is tamper-evident, not tamper-proof. `crates/deadreckon-core/src/tamper.rs` builds a touched-file set from `provenance.jsonl` plus the earliest `snapshots/turn-*` inventory, maps compiled checks to covered paths, lints shell/cargo command strings for suppression patterns, and classifies the run as `clean`, `caveat`, or `refuse`. The durable proof is `proofs/acceptance-tamper.json`; it is signed indirectly because the marker signature hashes the proof bytes. See §35 for the full policy and limits.

### 13.6 Where the gate is invoked

When the turn loop emits `Action::Done` — either through a CLI sub-agent finishing or a JSON-action provider returning `Done` — it routes through `acceptance_gate_passed_or_record_failure` (`crates/deadreckon-runtime/src/turn_loop.rs:1442`). Both call sites (`turn_loop.rs:405` for CLI sub-agent Done, `turn_loop.rs:672` for JSON Done) use this helper.

The helper composes `run_acceptance_gate` (invokes `dr-gate` as a subprocess) and `validate_acceptance_marker` (signature + run_id check):

- **If the gate passes:** the helper returns `true` and the loop continues into `promote_if_ready`.
- **If the gate fails:** the helper logs `acceptance.failed` to `traces.jsonl`, appends an explicit corrective hint to the run history (`"acceptance failed after turn N: <reason>. Continue by fixing the failing done criteria; do not declare done until dr-gate passes."`), emits a `RunEventKind::Error` event, records the reason in `state.failure_reason`, and **returns `false` — the run does not terminate**.

The agent sees the failure inside the next turn doc and can revise the working tree and re-declare `Done`. Only when the turn budget is exhausted does the run fail; at that point the accumulated reasons in `state.failure_reason` become the final `failure_reason` text (`turn_loop.rs:693-695`).

---

## 14. Telemetry: Spend, Traces, Provenance, Events

Five append-only JSONL files capture every run's history. Four (`spend.jsonl`, `traces.jsonl`, `provenance.jsonl`, `events.jsonl`) live under `<run_root>/` directly; the fifth (`proofs/acceptance-progress.jsonl`, see §13.5) is gate-scoped and truncated per evaluation. The gate also writes `proofs/acceptance-tamper.json`, a single proof object bound into the signed marker (§35.5). JSONL files are written via `append_json_line` (`state.rs:375-388`), which opens in append mode and `sync_all`s after each line.

### 14.1 `spend.jsonl`

Schema:

```json
{
  "timestamp": "<RFC3339>",
  "run_id": "...",
  "turn": 3,
  "provider": "cli:claude-code",
  "model": "claude-sonnet-4-5",
  "input_tokens": 0,
  "output_tokens": 0,
  "cost_usd": 0.0,
  "subscription": true,
  "wall_time_seconds": 42.1
}
```

One line per LLM call. HTTP providers fill in token counts and USD; CLI providers fill in `subscription: true` + `wall_time_seconds`. User-facing spend render is honest by route: subscription-only runs show `not metered (subscription) · wall <s>s · <n> turns` instead of a fake `~$0.000000`, while mixed routes show the metered total plus a `+ subscription turns` note.

### 14.2 `traces.jsonl`

Schema:

```json
{
  "timestamp": "<RFC3339>",
  "run_id": "...",
  "turn": 3,
  "event": "llm.complete" | "tool.bash" | "tool.write_file" | "tool.cli_subagent" | "import.claude-code" | ...,
  "latency_ms": 1234,
  "detail": { "tool_call_id": "...", "provider": "...", "trace": {...} }
}
```

Every LLM call, every tool dispatch, every notable event. The detail block carries free-form JSON per event kind.

### 14.3 `provenance.jsonl`

Schema (per file changed):

```json
{
  "timestamp": "<RFC3339>",
  "run_id": "...",
  "turn": 3,
  "tool_call_id": "...",
  "model": "claude-sonnet-4-5",
  "session_id": "...",
  "file": "src/main.rs"
}
```

One line per file changed by a tool call. Lets `show <run-id>` answer "which prompt produced this file?"

### 14.4 `events.jsonl`

Structured `RunEvent` log: `TurnStarted { turn }`, `ToolCallStarted { tool_call_id, kind }`, `ToolCallResult { tool_call_id, status, latency_ms }`, `RunCompleted { outcome }`. Also published on a `tokio::sync::broadcast` channel (`events.rs`): `emit_event` writes the JSONL line and sends on the channel together. In production, `attach` always reads live events by tailing `events.jsonl` (`TuiEventFeed::file_tail`) — even for same-process runs; the broadcast-receiver path (`TuiEventFeed::from_broadcast`) is `#[cfg(test)]` only. The channel is wired and reserved for a future same-process subscriber (see §18.3).

---

## 15. Resume Semantics

`crates/deadreckon/src/main.rs:18484` is the `resume_command` handler.

### 15.1 The Completed guard

`main.rs:18493`:

```rust
if state.status == RunStatus::Completed {
    println!("run {} is already completed", state.run_id);
    return Ok(());
}
```

Completed runs cannot be resumed. They can be re-promoted (idempotent) but not re-entered into the loop. The only forward path for a completed run today is to start a new run.

### 15.2 The replay process

For non-Completed runs (`Failed`, `Killed`, `Paused`):

1. Acquire the task lock.
2. Clear `failure_reason`, `pause_reason`, `killed_at`.
3. Set `status = Planned`.
4. Optionally extend `max_wall_seconds` from the resume flag.
5. Set `child_pids = vec![cli_pid]`.
6. `save_state()`.
7. Call `run_turn_loop` with `RunLoopConfig.from_turn = <CLI flag>`.
8. The loop calls `load_or_reconstruct_history(state, from_turn)` in `crates/deadreckon-runtime/src/turn_loop.rs`, which loads `history.json` if present, else reconstructs from `traces.jsonl`. With `from_turn = N`, history is truncated to N entries and `state.turn = N`.

History reconstruction tolerates a `traces.jsonl` that ends mid-tool-call by ignoring incomplete entries — the next turn re-runs that turn.

See `/Users/gdc/deadreckon/docs/RESUME-SEMANTICS.md` for the contract.

---

## 16. Cross-Tool Import

`deadreckon import <source>` reads from external coding-agent histories and synthesizes a completed deadreckon run from one concrete imported session, or from an explicit `--all` root import. It is read-only with respect to provider-owned transcript roots and writes only deadreckon run-state files.

Today's sources:

| Source | Discovery | Format |
|---|---|---|
| `codex` / `cli:codex` | `cli:codex` descriptor `[ingest]`, including `CODEX_SESSIONS_DIR`, `session_meta.payload.cwd`, `*.jsonl`, and `codex-cli` schema | JSONL |
| `claude-code` / `cli:claude-code` | `cli:claude-code` descriptor `[ingest]`, including `CLAUDE_PROJECTS_DIR` and Claude project-directory cwd mapping | JSONL |
| `gemini` / `cli:gemini` | `cli:gemini` descriptor `[ingest]`, including `GEMINI_DIR`, JSON/JSONL storage, and `gemini` schema | JSON or JSONL |
| `opencode` / `cli:opencode` | `cli:opencode` descriptor `[ingest]`, file-mode `storage/session`, `storage/message`, and `storage/part` | JSON |
| `copilot` / `cli:copilot` | `cli:copilot` descriptor `[ingest]`, including `COPILOT_DIR`, `session-state`, nested `events.jsonl`, and `data.context.cwd` | JSONL |
| `pi` / `cli:pi` | `cli:pi` descriptor `[ingest]`, including `PI_CODING_AGENT_SESSION_DIR`, top-level `cwd`, and Pi session-header validation | JSONL |
| `cursor` | `~/.cursor/chats/` (`DEADRECKON_IMPORT_CURSOR_ROOT`) | SQLite via `sqlite3 -json` |

The command surface is `deadreckon import <source> [--preview|--list|--session <id-or-path>|--cwd <path>|--all|--since <duration>|--replace|--json]`. Default import filters candidates to the launch cwd or `--cwd` where the descriptor supports cwd matching. Empty, stale, and ambiguous candidate sets refuse without creating a run and print concrete `try:` lines. `--preview` and `--list` never create run directories. `--all` is the explicit whole-root mode.

The handler creates an `imported-<hash>` run id from the concrete source-session identity, parses selected transcript files, appends normalized source-neutral entries to `traces.jsonl`, writes file provenance to `provenance.jsonl`, marks the run `Completed` (skipping the gate), and writes `import.json` under the run root. The manifest records `source`, `source_alias`, `schema`, `storage`, `cwd`, `mode`, `session_id`, `session_paths`, `content_hash`, source time bounds, row/event/provenance counts, `raw_rows_stored = false`, and the `deadreckon import ... --replace` command needed to reimport. If an existing imported run has a different content hash, import refuses unless `--replace` is explicit.

Trace `detail` rows use a stable import schema: `import_version`, canonical `source`, `schema`, optional `session_id`, `source_path`, optional `source_line`, `source_event`, optional `role`, `summary`, optional tool fields, `files`, optional token `usage`, and `raw_hash`. Raw provider payloads are not copied into every trace row by default; source path/line plus hashes preserve auditability. Parser coverage includes Codex agent messages/function calls/token counts, Claude content/tool blocks, Gemini JSON/JSONL messages/tool calls, OpenCode file-mode sessions/messages/parts, Copilot assistant/reasoning/tool/usage rows, Pi session/message/tool/result rows, and Cursor SQLite rows.

`deadreckon import` is the *user-driven* ingest. The *TUI-driven* ingest path that lets `attach` read live provider transcripts uses the same descriptor `[ingest]` roots, env overrides, storage kinds, file globs, freshness windows, cwd matching, and schema keys for CLI providers; see §18.3 and `docs/design/PROVIDER-CLI-INGEST.md`. OpenCode SQLite, provider transcript mutation/undo, full replay UI, and cross-run import analytics remain V1 candidates in `docs/V1-CANDIDATES.md`.

---

## 17. CLI Surface

The `Commands` enum in `crates/deadreckon/src/cli.rs` defines the CLI surface; `main_inner` in `crates/deadreckon/src/main.rs` dispatches to private command-family modules under `crates/deadreckon/src/commands/` and to root helpers for the remaining config, try, status/show, and control surfaces. Verbs and roles (line numbers are intentionally omitted — `cli.rs` is the source of truth and grows over time):

Default top-level help presents the production model: `start`, `attach`,
`status`, `list`, `finish`, `doctor`, `init`, `def-done`, `kill`, `resume`,
`cleanup`, `help-all`, and `<command> --help`. Power-user launchers and
building-block verbs remain callable, documented by `help-all`, exposed through
their own `--help`, and available to completions, but they are no longer the
first screen.

| Verb | Role |
|---|---|
| `init` | Interactive setup of `~/.deadreckon/config.toml` |
| `config get/set` | Non-interactive TOML edits |
| `start` | Guided production front door for choosing and launching a run, follow-up, or orchestration path |
| `run` | Advanced direct one-run launcher; create + enter turn loop |
| `doctor` | Actionable preflight (OS, sandbox, providers, config, disk, runtime) |
| `status` / `next` | Current project's latest run, locations, and next action |
| `list` | Project-scoped run inventory by default; `--all` for global history, `--full` for exact values |
| `apply` | Apply a completed worktree run (or merged plan result) to the user's current branch |
| `finish` | Choose the right completion action (apply for git worktrees, export for non-git) for a run or merged plan |
| `abandon` / `discard` | Remove a run's worktree branch/path or mark no-op modes abandoned |
| `materialize` / `export` | Copy a completed fresh/copy artifact (or merged plan result) to a normal directory |
| `cleanup` / `prune` | Clean abandoned, stale, or selected completed worktree runs |
| `attach` | TUI on a live or completed run, chain, or plan |
| `kill` | Lock release + child PID termination (run, chain, or plan) |
| `resume` | Re-enter the loop on a non-Completed run |
| `extend` | Re-open a completed run with a follow-up goal that inherits history |
| `undo` | Restore snapshot to a target turn |
| `rewind` | Preview or apply a provider flight checkpoint rewind (see §33) |
| `show` | Pretty-print full state + provenance + traces; `--why-failed` for runs and plans; `--flight` / `--file` for the flight recorder |
| `import` | Read-only descriptor-backed transcript import from CLI providers plus Cursor SQLite |
| `chain` | Create, plan, run, attach, pause/resume/kill, undo, extend, and redo serial autonomous chains |
| `orchestrate` | Advanced one-command wrappers for `review` and `full-plan` multi-agent runs |
| `plan` | Advanced building block that writes an orchestration plan (worker specs + `plan.json`) without starting child runs |
| `fork` | Advanced building block that spawns ready child runs for a plan and supervises them through completion |
| `merge` | Advanced building block that composes completed child library artifacts into a new promoted run (with semantic merge repair) |
| `def-done` | Write, add, show, or check the project's English done contract |
| `acceptance` | Hidden compatibility surface for creating, explaining, or checking the done contract |
| `doc` | Print run narrative, as-built, implementation decision ledger, or delta; with optional polish pass |
| `history` | Search durable run traces and provenance (regex/scope/plan filters) |
| `library` | Query promoted run artifacts by goal/date/scope |
| `detect` | Probe registered providers and return availability and credential status |
| `providers` | List provider routes, models, kind tokens, and the active selection |
| `update` | Check for or route self-updates via npm, Homebrew, shell, Cargo, or source channel |
| `completion` / `completions` | Generate or install shell tab-completion scripts (bash, zsh, fish, elvish, powershell) |
| `learn` | Index local run evidence and propose deadreckon improvements (see §34) |
| `improve` | Run evidence-backed deadreckon self-improvement candidates (see §34) |
| `help-all` / `commands` | Show every command, including advanced ones hidden from short help |

### 17.1 Guided first use

`deadreckon start "<goal>"` is the guided production command. It is intentionally a thin CLI-layer decision helper, not a new runtime state machine: each invocation builds an ephemeral launch decision, prints the selected path and reason, and either previews or dispatches to the existing `run`, `extend`, and `orchestrate` handlers. In an interactive TTY, `start` uses normal terminal selection prompts for launch mode, provider route, done contract, source mode, and final confirmation when flags have not already made those choices explicit. No `PipelineState` schema changes were introduced for this path, and previews remain state-free.

The launch decision resolves provider setup, done contract, source mode, history, and run-vs-orchestrate/campaign mode before any provider work begins. Provider resolution uses configured defaults first and only probes installed subscription CLIs when no default route is configured; TTY users can select a detected route ephemerally for that launch without writing config, while non-TTY or scripted users still get concrete `try:` lines for `init`, `detect`, `config provider`, or `providers list --all`. Done-contract resolution uses the same `def-done` and `.deadreckon/acceptance.yaml` contract as direct runs: existing project criteria trigger a TTY keep/view/check/update/cancel prompt so users can inspect or change the contract before launch, missing criteria can be generated or defaulted, and non-TTY callers get deterministic recovery lines. Source-mode resolution follows the existing run safety posture: git worktree by default in repositories, TTY selection for init-git/copy/fresh in non-git directories, and explicit stash or `--allow-dirty` choices for dirty worktrees.

History-aware `start` scans the current project scope for the newest completed, promoted, non-in-place run. When one exists, the TTY launch picker adds a "Follow up" choice that dispatches through `extend`; preview and JSON output also include exact commands for `deadreckon extend <run-id> "<goal>"`, `deadreckon start "<goal>" --mode review --yes`, and `deadreckon start "<goal>" --mode full-plan --yes`. This keeps scripted `start` deterministic while making it obvious how to continue prior work or launch a new orchestration pass.

Auto mode is advisory. When a usable provider exists, `start` makes one bounded read-only classifier call through the existing provider router to recommend a single verified run, review/full-plan orchestration, or campaign with a count and rationale. The validated recommendation is preview-scoped and state-free; no personal preference is persisted. Smoke/no-provider paths use deterministic fallback heuristics. In a TTY, the recommendation appears first in the picker and the user can override it with an explicit selection or flag.

`start --preview`, `run --preview`, and `orchestrate --preview` share launch-preview rows: path, provider, done contract, workspace, watch, stop, and finish, with optional base/history rows when a follow-up is selected or available. Orchestrated `start` previews also show role reuse when one selected provider route is used for coder/reviewer or planner/child roles. Successful guided launches add a `start lifecycle` footer with exact `attach`, `status`, `kill`, and `finish` commands for the created run or plan. Existing `run`, `extend`, `orchestrate`, and `campaign` remain the canonical direct commands for users who already know the path they want.

Prompt eligibility is deliberately narrow: `--json`, `--plain`, `--quiet`, `--yes`, and non-TTY execution never start the picker and never block on stdin. Those paths preserve deterministic JSON/recovery output and scriptable launch behavior. `--preview` may ask TTY users for selections, but it remains state-free; provider config is not written by a provider selection, and done-contract files are only generated for an actual launch after final confirmation.

The CLI defaults are honest: `--sandbox` defaults to `auto`, `--max-spend` defaults to `$10` (with a confirmation gate above `$50`), `--provider` defaults to the highest-credentialed entry per the fallback chain, `--skill` defaults to `default-coding`.

`run` now starts codebase-aware by default. In a git repo it previews and then creates a `git worktree` on a `dr/...` branch; `--fresh` preserves the old empty-working-dir behavior, `--from <path>` uses copy mode, and `--in-place --i-know-its-a-lot` edits the source tree directly. Completed worktree runs hint `apply` / `discard`; copy and fresh runs hint `export` / `extend`. Run-id arguments accept unique prefixes and `latest` / `last` resolves to the latest run in the current project scope.

`completion install` is driven from the real clap command tree, so subcommand aliases, flags, and value-hint completions stay in sync with `deadreckon --help`. The handler detects the active shell via `$SHELL`, writes the script to a per-shell default path (e.g. `~/.zsh/completions/_deadreckon`, `~/.local/share/bash-completion/completions/deadreckon`), and for zsh adds a managed `# deadreckon completion` block to `~/.zshrc` unless `--no-rc` is passed. `init` invokes `try_install_completion_after_init` so first-time setup ships completions opt-out (`init --no-completion`). The per-shell stdout variants (`completion bash|zsh|fish|elvish|powershell`) print the script for users who manage their own shell config.

`run` startup details (`print_run_started`) are now also emitted at the top of `extend` and `resume` so extended/resumed runs surface their selected provider route and doc-provider source the same way fresh runs do. Interactive terminals receive a `deadreckoning_course` ASCII progress strip and a polled `cli_wait_status` line while a long turn is in flight; the status is cleared as soon as the loop reports back. `kill` against a loaded run now also persists `RunStatus::Killed` + `killed_at` + `failure_reason = "killed by user"` before returning so downstream tooling sees a consistent terminal state.

---

## 18. TUI (`attach`)

`attach_command` lives in `crates/deadreckon/src/commands/attach.rs`. The terminal loops delegate to the private `tui/` render/state facade for run, plan, chain, and campaign frames; provider refresh and narrative projection stay outside the render path. Historical `main.rs` line numbers for attach are obsolete after the Decompose pass.

### 18.1 Behavior

- On a TTY: `attach_tui()` enables raw mode, alternate screen, and renders a `ratatui` UI.
- Off-TTY: prints a plain-text summary + locations.

`attach` dispatches by id kind: a run id opens the run TUI documented below, a chain id opens the chain attach view (`Chains`, §28), a plan id opens the plan attach TUI (`Plans`, §30.3 / §32.3), and a campaign id opens the campaign attach TUI (`Campaign Orchestration`, §36.9). These TUIs draw from the same palette (`ui::TUI_PALETTE`, §26.7) and the same key conventions (`q`/`Esc`/`Ctrl-D` detach; `d` toggles docs view in the run TUI; `Enter` drills into a child run from plan attach, or into a selected sub-plan from campaign attach). Campaign drill-in is navigated rather than flattened: campaign attach suspends its frame, opens the existing plan attach loop for the selected sub-plan, and the plan loop can then suspend again into the existing run attach loop.

`attach <id> --view narrative` adds a calmer operator projection for runs, plans, and plan child refs. The default remains `activity`, so raw tool/provider lines still open first unless the user requests the narrative view. In narrative mode, `n` toggles back to raw activity, `v` cycles `architecture -> agents -> files -> evidence -> none`, and `r` requests a provider-backed refresh when a configured route is available. While the TTY narrative view is open, meaningful run and plan events also request a provider refresh: errors, completions, tool milestones, docs checkpoints, acceptance running/pass/fail transitions, child-run discovery, task terminal states, and merge-repair milestones. Long-running quiet periods request a refresh after the narrative quiet window when the run or plan is still running. Provider refreshes are background jobs: manual/event/quiet refreshes coalesce while one is active, `q`/`Esc`/`Ctrl-D` detach remains immediate, and child drill-in from plan attach cancels or suspends the in-flight narrator before opening the child run. Provider refreshes are bounded: the prompt is built from redacted evidence windows, the provider must return strict cited JSON, graph labels may only target deterministic graph ids, and failures persist a stale deterministic projection instead of breaking attach.

### 18.2 Layout

`attach_panel_layout` (`main.rs:24166`):

```rust
let vertical = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(5),    // top band: header + spend (if metered) + context + acceptance
        Constraint::Min(10),      // tool calls + provider activity + live files
        Constraint::Length(4),    // processes/status
        Constraint::Length(2),    // keybindings footer (multi-line during long ops)
    ])
    .split(area);
```

The top band is split horizontally into three panels for subscription providers (header 66 / context 17 / acceptance 17) and four for metered API providers (header 55 / spend 15 / context 15 / acceptance 15):

- **Header** (short run id, status, phase, provider, sandbox, turn timer, truncated goal, working/artifact path; degrades to an identity strip when live status is unavailable).
- **Spend meter** only for metered API providers; CLI subscription providers omit cost and emphasize context/wall time.
- **Context meter**: compact token/window summary with green/yellow/red thresholds.
- **Acceptance meter**: derived from `AcceptanceLive` (`collect_acceptance_live` in `main.rs:24615`). When `proofs/acceptance-progress.jsonl` exists, the panel tails it and surfaces `running 2/5`, `passed`, or `failed` with the offending check; once `turn-acceptance.json` is signed, it pivots to a marker view (`acceptance_live_from_marker`). Color thresholds are owned by `acceptance_color`.
- **Center, left**: wide streaming list of tool calls + provider activity + recent events. Acceptance lines from `acceptance_activity_lines` are interleaved so the operator sees the same progress in the activity stream and the meter.
- **Narrative view**: `--view narrative` or `n` swaps the center-left activity pane for prose sections under the `Narrated` operator heading: freshness/coverage, headline, current work, architecture notes, risks, next likely action, and citations. Wide terminals split that pane with a right-side visual map; narrow terminals collapse to prose first. Run narratives cite `proofs/acceptance-progress.jsonl` or `proofs/turn-acceptance.json` when acceptance evidence exists, so failed done criteria point at the durable proof artifact. Plain/off-TTY narrative attach prints the same projection with citations and ASCII map lines when `--visual` is not `none`; `--json --view narrative` emits the structured state, snapshot, and graph objects. Non-TTY narrative attach stays deterministic and does not call a provider unless a future explicit refresh surface opts in. Chain narrative attach currently returns an unsupported response with `try:` lines for run and plan narrative attach.
- **Completed docs view**: pressing `d` toggles the center-left panel from provider activity or narrative view to `RUN-NARRATIVE.md` rendered through `pulldown-cmark` into ratatui `Line`/`Span`s. Headings, bullets, inline code, fenced code blocks, links, task markers, math, and horizontal rules receive terminal styles and remain scrollable. The docs view remains a separate completed-run artifact rather than being merged into the live narrative projection.
- **Center, right**: narrower live files list with count/bytes in the panel title.
- **Bottom**: supervised PIDs + their `ps` lines (alive/dead annotation).
- **Footer**: action-first completed footer (`[d] Docs` / `[d] Activity`, `[a] Apply`, `[b] Abandon`, `[s] Show`) or scroll/detach help while running. The footer's second line carries `deadreckoning_status_line` while long operations are in flight.

Campaign attach has its own campaign-shaped frame rather than reusing the run panel grid. The TTY view shows a campaign header with goal, status, roll-up, aggregate spend, tree budget, and campaign breadcrumb; selectable sub-plan cards with sub id, status, plan/result prefixes, spend, and goal; a campaign feed; and a footer with select/drill/back/refresh/detach controls. `Enter` requires a selected sub with a `sub_plan_id`; otherwise it keeps the campaign frame open. Off-TTY and `--plain` still print the read-only campaign summary with an explicit `deadreckon attach <sub-plan-id>` hint, and `--json` emits the structured campaign attach object instead of entering ratatui.

### 18.3 Data source and responsiveness contract

Each attach surface runs a budgeted tick loop. The render path is pure: it must not call providers, recurse through provider roots, append narrative snapshots, or reread unbounded JSONL files. Slow or potentially blocking work is either moved into an attach-owned cache/tailer or into a background refresh job whose completion is polled between frames.

Run attach uses `TuiEventFeed` for run events and `AttachJsonlTail` for `spend.jsonl`, `traces.jsonl`, and `flight-events.jsonl`, so redraws parse only appended complete rows after the first load and ignore partial trailing JSONL until it is complete. Live-file collection uses an attach-specific inventory walker that prunes heavy cache/profile directories before descent and caps displayed rows without losing total counts. Provider activity prefers current flight rows; descriptor-backed provider-log fallback scans are throttled by freshness, matched path, root mtime, and file mtime so a live attach does not recursively scan provider homes every frame.

`collect_provider_activity` resolves provider ingest through descriptor `[ingest]` metadata: candidate roots, env overrides, cwd matching, storage kind, file glob, freshness window, and schema key. `deadreckon import` reuses the same descriptor metadata for provider transcript discovery and adds import-only session selection, manifest writing, and normalized trace/provenance event creation. `cli:codex` reads `~/.codex/sessions/**.jsonl` and matches `session_meta.payload.cwd`; `cli:claude-code` reads `~/.claude/projects/<cwd-slug>/*.jsonl` using Claude Code's path-to-project mapping and matches top-level `cwd`; `cli:gemini` reads Gemini JSON/JSONL file logs; `cli:opencode` reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON; `cli:copilot` reads `~/.copilot/session-state/*.jsonl` plus nested `events.jsonl` and matches `data.context.cwd`; `cli:pi` reads `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`, validates the first nonblank row is a Pi `session`, and matches the header `cwd`. Schema-specific adapters only decode rows into common activity lines (`agent`, `thinking`, `tool`, `result`, `todo`, `tokens`) and normalize tool labels through `deadreckon_providers::taxonomy`.

Production run attaches — same-process and cross-process alike — read run events by tailing `events.jsonl` via `TuiEventFeed::file_tail`; `TuiEventFeed::from_broadcast` is `#[cfg(test)]` only. (The loop's `emit_event` writes the file and sends on the `RunEventBus` channel together, so the file tail stays current; the broadcast path is reserved for a future same-process attach.) Plan attach consumes `PlanEventBus` / `PlanEventFeed`, which owns `plan-events.jsonl` replay/tailing, emits plan snapshots, tolerates malformed or partial plan-event rows, and multiplexes discovered child and repair run `events.jsonl` streams into the plan activity pane. Chain attach keeps its own `AttachJsonlTail<ChainEvent>` for `chain-events.jsonl`, preserves the existing drill/redo/extend/pause/kill controls, ignores partial last lines until complete, and shows an activity-read hint when chain event catch-up falls behind the tick budget. Campaign attach uses `CampaignEventFeed`: it tails `campaign-events.jsonl` with `JsonlTail`, rediscovering sub-plans from `campaign.json`, and tails each discovered sub-plan's `plan-events.jsonl` with the same read-side tailer. It emits campaign snapshots, campaign events, sub-plan plan events, and warnings, but it does not flatten child run streams into a three-level tree; operators navigate to the existing plan/run attach loops for that detail. The production feeds remain durable-file backed for cross-process attach, with broadcast-capable APIs available for same-process streams.

### 18.4 Narrative projection files

Narrative attach writes projection files, not source-of-truth state. Run projections live under `<run-root>/narrative/`; plan projections live under `<plan-dir>/narrative/`.

- `state.json` records the latest snapshot id/status, coverage, cadence, provider source, provider call count, cost/wall-clock accounting, and last refresh error.
- `snapshots.jsonl` is append-only. Each row contains cited `current_work`, `architecture_notes`, `risks`, `next_likely`, citations, and plan-only agent/coordination sections. Malformed rows are skipped when reading the latest snapshot.
- `architecture-graph.json` is a deterministic graph over run files, provider ids, checkpoints, plan tasks, dependencies, child runs, and citation ids. The TUI renders architecture, agent, file, and evidence views from this graph using ASCII-compatible labels and color-independent badges.

Plan projections prefer each child run's latest narrative snapshot when one exists. The plan agent table cites that child snapshot and uses its headline as the child summary, then falls back to child run state when no child narrative exists. Plan graphs also roll up file nodes from child narrative graphs so the plan-level files visual can show cross-agent touched file evidence without copying child logs into `plan-events.jsonl`.

Provider-backed narration is an overlay on these projections. Manual `r` refresh, TTY narrative-view event refreshes, and TTY narrative-view quiet-threshold refreshes start an asynchronous off-loop job (`tokio::spawn`); each attach tick only polls the job handle non-blockingly, so the render loop never stalls on a provider call. Provider refresh jobs build a redacted prompt from the deterministic projection, send only bounded evidence summaries to the selected provider route, validate all returned claims against known evidence ids, reject invented graph ids, and persist a new fresh snapshot only after validation. The default narrator route is `cli:claude-code` with model override `sonnet`, which the Claude Code adapter launches as `claude --model sonnet --dangerously-skip-permissions -p <prompt>`; `--narrative-provider` overrides the route and leaves model selection to that provider/config entry, while `--no-narrative-provider` keeps attach deterministic-only. Event refreshes can bypass the ordinary freshness interval for meaningful deltas, while quiet-threshold refreshes still obey the freshness interval; budget, provider availability, JSON validation, citation validation, graph validation, and redaction guards always apply. If the provider is missing, over budget, behind cadence, returns malformed JSON, cites unknown evidence, emits secret-like text, or fails, attach keeps running and persists a stale deterministic snapshot with the error in `state.json`.

The run and plan narrative panes cache projection objects by coverage and feed signature. Redraws reuse the cached projection, stale provider snapshots survive ordinary frame churn, and render helpers fall back to deterministic projection builders instead of `ensure_*` persistence calls. This is the nonblocking attach contract: provider-backed narrative work may improve the next frame after it finishes, but it never owns the current frame.

---

## 19. Configuration & BYOK

### 19.1 `config.toml`

Lives at `/Users/gdc/.deadreckon/config.toml` (overridable via `DEADRECKON_HOME`). Schema (`crates/deadreckon-providers/src/types.rs`):

```rust
pub struct ProviderConfigFile {
    pub default_provider: Option<String>,
    pub fallback: Option<Vec<String>>,
    pub providers: BTreeMap<String, ProviderEntry>,
}

pub struct ProviderEntry {
    pub kind: Option<ProviderKind>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub input_cost_per_million: Option<f64>,
    pub output_cost_per_million: Option<f64>,
    pub binary: Option<String>,
    pub extra_args: Vec<String>,
}
```

Example:

```toml
default_provider = "cli:codex"
fallback = ["cli:codex", "cli:claude-code", "anthropic", "openai"]

[defaults]
max_spend = 10
provider = "cli:codex"
sandbox = "auto"

[providers."cli:codex"]
binary = "codex"
extra_args = []
kind = "cli-codex"
model = "gpt-5.1-codex" # optional; omitted means provider default

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
model = "claude-sonnet-4-5"
input_cost_per_million = 3.0
output_cost_per_million = 15.0
```

Operator affordances: `deadreckon run --preview "goal"` renders the selected
provider route, source, and model before state is created; `run --model <model>`
and `extend --model <model>` override one run; `deadreckon config provider
<route>` and `deadreckon config model <model> --provider <route>` persist the
defaults. `init`, `config provider`, primary run/extend, orchestration roles,
resume/doc polish, and provider-selection display all route through
`crates/deadreckon/src/setup.rs`, which reports provider role, route, model,
source (`flag`, `config`, `auto_subscription`, `run_provider`,
`built_in_default`, or `none`), credential state, warnings, and `try:` lines
without adding durable state. CLI providers pass explicit model overrides
through to the underlying tool (`codex exec --model ...`, `claude --model ...`)
and otherwise display `provider default`.

Route resolution (`configured_route_names` in `crates/deadreckon-providers/src/router.rs`) now puts `default_provider` at the head of the chain, then appends `fallback` entries that aren't already present, then falls back to the built-in chain (`cli:claude-code` → `cli:codex` → `anthropic` → `openai` → `openai-compatible`) only if neither is configured. `read_config` (`config.rs`) backfills `default_provider` from a top-level `[defaults] provider` key when it's omitted, so the same TOML stanza drives both `init`-style defaults and the router. `--provider` on the CLI still short-circuits the whole chain.

### 19.2 BYOK posture

Three credential paths:

1. **HTTP key.** Set `api_key` directly or `api_key_env = "FOO"` to read at runtime.
2. **CLI subscription.** Run with `--provider cli:claude-code` or `cli:codex`. The binary's presence in `$PATH` is the credential. No key required.
3. **OpenAI-compatible.** Plug an OpenRouter or `llama.cpp` endpoint into `base_url` + `api_key`.

`deadreckon init` walks the user through option (1) or (2): it queries the provider registry for any subscription CLI provider whose binary is available (`setup::auto_subscription_cli_provider`) and offers the first match — `cli:claude-code`, `cli:codex`, `cli:gemini`, `cli:opencode`, `cli:copilot`, or `cli:pi`, depending on what's installed — before asking for keys. The chosen route is immediately validated through the shared setup resolver, and the generated config preserves a `cli:*` route in the fallback chain rather than overwriting with the historic claude/codex default.

---

## 20. Testing Strategy

### 20.1 Test locations

Tests span four `tests/` directories plus inline module tests (~34 integration test files):

- **`deadreckon-core/tests/`** — `git_hardening.rs`, `install_receipt.rs`, `spend_summary.rs`, `update_cache.rs`.
- **`deadreckon-providers/tests/`** — `cli_providers.rs` (CLI provider routing, fake `claude`/`codex` binaries, output capture, spend), `mock_server.rs` (axum-based OpenAI-compatible mock for HTTP provider tests), `registry.rs` (provider registry + descriptors). Inline module tests cover config parsing, spend math, credential checks, and smoke determinism.
- **`deadreckon/tests/`** — `agentic_loop.rs` (~2170 lines; end-to-end run/kill/resume/list/attach/import/doctor + acceptance-gate cycling), plus `attach_inline_card.rs`, `audit_harden.rs`, `cards_{exit_summary,friendliness,preview,status}.rs`, `chain.rs`, `codebase.rs`, `coherence.rs`, `detect.rs`, `doc_depth.rs`, `hygiene_config.rs`, `learning_cli.rs`, `lifecycle.rs`, `narrative_attach.rs`, `npm_wrapper.rs`, `orchestrate.rs`, `providers_list.rs`, `public_surface.rs`, `release_plan.rs`, `self_documenting.rs`, `sleep_{linux,macos}.rs`, `smoke_invariant.rs`, `ui_card.rs`, `update_cli.rs`.
- **Top-level `tests/`** — workspace-wide guards: `hygiene_config.rs`, `public_surface.rs`, `smoke_invariant.rs`.

### 20.2 Notable integration tests

| Test (in `agentic_loop.rs` unless noted) | What it proves |
|---|---|
| `mock_provider_records_three_turns_and_artifacts_match` | mock-driven run produces 3 turns, ≥ 5 trace lines, 3 spend lines, signed acceptance marker, working files |
| `kill_run_across_processes_terminates_in_5s` | cross-process `kill` interrupts within 5 s and sets `Killed` |
| `kill_during_http_streaming_aborts_request` | `kill` aborts an in-flight HTTP provider stream |
| `resume_preserves_history_file` | resume preserves `history.json` tool_call_ids |
| `cli_subagent_without_file_changes_fails_run` | CLI provider with no file effects → `Failed` |
| `acceptance_failure_restarts_cli_subagent_until_gate_passes` | gate failure loops the agent until the gate passes |
| `acceptance_failure_exhaustion_persists_failed_state` | gate exhaustion leaves a persisted `Failed` state |
| `init_config_and_default_spend_work` | `init` writes config; `config get/set` works; `--smoke` respects defaults |
| `high_spend_requires_confirmation_flag_in_scripts` | `--max-spend 51` without `--i-know-its-a-lot` fails with a hint |
| `cli_wall_clock_budget_enforced` | subscription run pauses at `--max-wall-seconds` cap |
| `kill_storm_no_leaks` | concurrent kills release all PIDs + locks |
| `doctor_fails_actionably` | `doctor` output contains specific fix commands |
| `import_*` focused cases | descriptor-backed import normalizes CLI/Cursor histories, writes manifests, refuses ambiguous/changed imports, round-trips to goldens, and preserves preview/list no-write behavior |
| stress run (gated by `DEADRECKON_STRESS=1`, via `make stress`) | concurrent scoped runs complete cleanly |

### 20.3 Verification gates

Every commit is expected to leave the workspace green on:

```zsh
cargo build --release
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## 21. Key Design Decisions

1. **No new `PipelineState` fields without a strong reason.** Parent lineage, materialization status, and other run-level metadata live in files inside the working tree (e.g., `.deadreckon/parent.json`), not on the state struct. This keeps `state.json` migration-safe.

2. **Two-layer split is non-negotiable.** Skills (Markdown) own judgment; the binary owns invariants. A skill cannot bypass the gate; a gate cannot read prompts.

3. **Anti-self-attestation via a nonce only `dr-gate` reads.** The agent can produce any output it wants; it cannot produce a valid `AcceptanceMarker.signature` because it never sees `gate/nonce`.

4. **Atomic promotion before completion.** `Completed` is set only after the library swap succeeds. If promotion fails, the run fails. This means a `Completed` status always implies a durable artifact in `library/`.

5. **Platform-native sandboxing.** No Docker daemon dependency. Seatbelt on macOS, bwrap on Linux. Docker is opt-in. The "running on host machine feels like a bad idea" pain point is solved without forcing Docker.

6. **BYOK extends to subscriptions.** `cli:claude-code` and `cli:codex` are first-class providers, not afterthoughts. Most users will run deadreckon via subscription, not API keys.

7. **Phase numbering with gaps (0, 10, 20, …).** Future phases can be inserted (e.g., a phase 15 for plan refinement) without rewriting durable state.

8. **Append-only telemetry.** `spend.jsonl`, `traces.jsonl`, `provenance.jsonl`, `events.jsonl` are immutable audit trails. The TUI and `show` command read them; nothing rewrites them.

9. **The smoke provider is a real provider.** `--smoke` selects `ScriptedSmokeProvider`, which goes through the same `ProviderRouter::complete` path. There is no separate "smoke turn loop" — the loop is always the loop.

10. **PID-aware locks with stale reclaim.** A crashed deadreckon process can be detected (its lock holder PID is dead via `kill(pid, 0)`); the next run reclaims the lock. No external lock daemon.

---

## 22. What's Built vs Scaffolding-Thin

The codebase is more complete than a typical first pass, and the 2026-05-11 hardening pass replaced earlier thin seams with depth-tested implementations where those seams were in scope. Honest accounting per `docs/CHANGELOG.md`, `docs/GAP-ANALYSIS.md`, and `docs/AUDIT-2026-05-11.md`:

### Built and reliable

- Workspace, crates, build, lint, fmt, test discipline.
- Workspace lint discipline (deny-tier clippy + rustc), tuned release profile, registry-shaped library `lib.rs`, library print refusal, and error retryable/fatal taxonomy as vocabulary for future watchdog work.
- Binary module layout: the former 40.6k-line `crates/deadreckon/src/main.rs` has been split into private `commands/` and `tui/` modules behind `main_inner` dispatch. `cli.rs`, the `Command` enum, all verbs, all user-facing output, and the public library surface remain unchanged by that split.
- `PipelineState` shape, phase machine, atomic state writes, schema version.
- PID-aware locks + heartbeats + stale reclaim.
- Atomic working→library promotion with crash recovery.
- Sandbox dispatch for sandbox-exec / bwrap / docker / none + auto resolution.
- Composable governance seams: `[seams]` can swap policy, model-catalog,
  hook-fanout, and event-sink workers through one sandboxed JSON-over-stdio
  primitive; unconfigured kinds keep built-in behavior and `--no-seams` forces
  built-ins per run/start. This adds worker-swapping capability without weakening
  §35: the acceptance gate is deliberately not a seam.
- HTTP providers (Anthropic / OpenAI / OpenAI-compatible) with token-based spend.
- CLI providers (`cli:claude-code`, `cli:codex`) with wall-clock subscription spend.
- Descriptor-backed CLI providers with generic `exec_template` launch, registry-driven detection/init/listing, descriptor sandbox writes, and built-in `cli:gemini`, `cli:opencode`, `cli:copilot`, and `cli:pi` providers.
- Direct-API history compaction: HTTP/API provider prompts are deterministically
  elided against catalog `context_window` thresholds, keep the goal/done spec
  intact, append `compaction.jsonl`, and leave CLI-provider paths untouched.
- Smoke provider (deterministic) for keyless tests.
- Turn loop with action parsing (Bash / WriteFile / Done) and CLI sub-agent path.
- Overnight UX: `run --prevent-sleep <auto|on|off>` previews sleep posture and arms macOS `caffeinate` or Linux `systemd-inhibit` around the run loop, with run-local `working/.deadreckon/sleep-prevention.json`; run previews, run exit summaries, and completed attach footers use the shared `ui_card` renderer with `--plain` and `NO_COLOR` support, while read-only inspection commands stay in quieter table/report layouts.
- Unattended git hardening: production git invocations route through `deadreckon-core::git`, export `GIT_TERMINAL_PROMPT=0`, and disable commit/tag GPG signing for commit-family verbs so global signing cannot hang on pinentry.
- Honest spend summaries: `spend.jsonl` replay renders subscription-only CLI routes as not metered with wall time and turns, and renders mixed routes as metered total plus `+ subscription turns` rather than pretending a subscription turn cost `$0.000000`.
- Distribution and self-update: install receipts and update-check caches live under `~/.deadreckon/`; `deadreckon update --check` and `deadreckon update` honor npm, Homebrew, shell, cargo, and source channels; shell installs preview the target/archive/checksum/backup path, require confirmation or `--yes`, keep the latest three backups, and print a post-update `deadreckon doctor` hint.
- Release packaging: `cargo-dist` configuration covers five OS/architecture targets, shell and PowerShell installers, Linux glibc 2.28 metadata, lane-aware fail-closed macOS signing/notarization for official RC/stable tags, Authenticode signing for stable Windows artifacts, `SHA256SUMS`, `release-manifest.json`, `release.spdx.json`, GitHub artifact attestations, npm provenance, a no-network npm wrapper with five platform packages, and Homebrew tap publishing through `gdc/homebrew-tap`.
- Codebase-default running: worktree mode, copy mode, in-place mode, fresh-mode preservation, preflight + preview UX, and `codebase.json` files-not-fields metadata.
- `apply` and `abandon` for worktree rollback/apply lifecycle.
- `materialize`, `extend`, `undo`, `list`, and `show` integration with codebase mode metadata, including worktree extension branches chained from parent `dr/...` branches.
- UX consolidation: project-scoped `list`, `latest` run aliases, `status`/`next`, `cleanup`/`prune`, `export`/`discard` aliases, and TTY-aware formatted output.
- Self-documenting run artifacts in stoa shape: `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, optional `AS-BUILT-DELTA.md`, per-turn `_incremental.jsonl`, explicit `docs_checkpoint` run events, and `polish.json` schema v2.
- `deadreckon doc`, `list` DOCS status, doc-aware `apply` commit bodies, extend-parent narrative updates, diff coverage retry, the legacy repo/user/project `run-narrator` skill mechanism, and default split polish skills (`narrator-overview`, `narrator-phases`, `narrator-as-built`, `narrator-decisions`).
- Acceptance gate with signed marker; anti-self-attestation and tamper-evident hollow-pass detection are enforced without adding `PipelineState`, `Plan`, marker, or check-result fields.
- `init`, `config get/set`, `run`, `doctor`, `status`/`next`, `list`, `attach`, `kill`, `resume`, `undo`, `rewind`, `show`, `import`, `cleanup`/`prune`, `completion`, `learn`, and `improve` verbs.
- Shell tab-completion via `completion install` / `completion {bash,zsh,fish,elvish,powershell}` driven from the live clap command tree; `init` opt-out installs completions and (for zsh) appends a managed `.zshrc` block.
- `ratatui` attach TUI with spend/context/acceptance telemetry, provider activity, in-TUI Markdown docs rendering, live files, process panel, scrollable panels, campaign sub-plan cards, and completion action footer. Run, plan, chain, and campaign attach now share an explicit responsiveness contract: render paths are provider-free and write-free, JSONL streams are tailed or cached, provider narrative refreshes run in cancellable/coalesced background jobs, stale narrative snapshots survive redraw, and long operations surface a `deadreckoning` ASCII status line in CLI and footer alike.
- Descriptor-driven provider activity ingest for Codex, Claude Code, Gemini JSON/JSONL, OpenCode file-mode logs, GitHub Copilot CLI session-state JSONL, and Pi session JSONL, normalized into `agent` / `thinking` / `tool` / `result` / `todo` / `tokens` rows without rewriting provider-owned logs.
- Descriptor import hardening: `deadreckon import` accepts legacy aliases and provider descriptor ids, discovers CLI transcripts through descriptor `[ingest]`, selects concrete sessions by cwd or `--session`, writes `import.json`, refuses ambiguous/changed imports with `try:` lines, and normalizes trace/provenance rows for Codex, Claude Code, Gemini, OpenCode file-mode, GitHub Copilot CLI, Pi, and Cursor SQLite.
- Streaming acceptance progress: `proofs/acceptance-progress.jsonl` reports per-check `started`/`running`/`passed`/`failed` transitions while `dr-gate` is mid-evaluation; the attach TUI tails it alongside the signed marker.
- Extended runs carry the parent's `acceptance.yaml` into the child run and emit the same `print_run_started` startup details (provider route, doc-provider source) as fresh runs; resume does the same.
- `--max-spend` cap with pause-at-cap; `--max-wall-seconds` for subscription providers.
- Event-backed TUI attach: production run attaches (same- and cross-process) tail `events.jsonl` incrementally via `TuiEventFeed::file_tail` — `TuiEventFeed::from_broadcast` is `#[cfg(test)]` only. Plan attach uses `PlanEventBus` for durable replay/tail plus child/repair event multiplexing, chain attach tails `chain-events.jsonl` incrementally with partial-line tolerance, and campaign attach tails `campaign-events.jsonl` plus each discovered sub-plan's `plan-events.jsonl`.
- Cross-process cancellation: `kill` writes a durable cancel marker before signaling; the run loop observes it while provider calls are in flight and reports killed status through events.
- Partial-trace resume: resume reconstructs only completed tool boundaries and `resume --from-turn` truncates traces, spend records, and future snapshots together.
- Durable per-run `sandbox.toml` plus per-tool sandbox policy: bash/write-file paths get specific filesystem and network permissions; refusals include `try:` and are recorded in traces and provenance.
- YAML done-contract files (`acceptance.yaml`): `dr-gate` evaluates required/optional tests, file existence, content matches, shell commands, and build checks, writes `acceptance-tamper.json`, refuses suppression-pattern/spec edits, caveats check-covered test/target edits, then signs check-level proof results and the tamper proof bytes.
- Exhaustive local doctor: OS, sandbox binaries, provider binaries, config, runstate permissions, disk, and opt-in provider pings all produce actionable `try:` hints.
- Promoted library query surface: `deadreckon library list|search|show` reads library manifests and reverse materialization markers, filters by goal/date, and searches promoted run docs.
- Import parity hardening: descriptor-backed CLI imports and Cursor SQLite imports preserve source metadata, deterministic session run IDs, stable row ordering, manifests, content hashes, and provenance paths; committed goldens and fixtures cover normalized `show` output plus provider-specific discovery.
- CLI usability polish: root help includes command groups, `status` includes run health/library/disk blocks, and `DEADRECKON_HINTS=0` suppresses post-completion prompts.
- Effortless friendliness contract: `docs/FRIENDLINESS-AUDIT.md` codifies six clauses for every canonical top-level verb (auto-detect don't ask, preview before mutation, refusal `try:`, rollback, one primary action, lifecycle hints), and `friendliness_contract.rs` plus focused tests keep that checklist executable.
- Keyless first-ten-seconds path: `deadreckon try` runs the normal turn loop with the deterministic smoke provider, signs the real `dr-gate` marker, and prints the proof/story/lineage block plus one next command without requiring credentials.
- Guided production front door: `deadreckon start` reuses the existing run/orchestrate/campaign mechanisms, self-bootstraps a single detected subscription CLI provider inline, keeps preview/JSON/plain paths state-free, and refuses with specific recovery commands when provider, done contract, or source mode is incomplete.
- One-verdict lifecycle surfaces: completed exit cards now lead with `VERIFIED`; exit cards, `status`, and `finish` expose one primary action while demoting secondary actions to quieter rows.
- Opt-in lifecycle notifications: `[notify]` supports native, command, and webhook channels for accepted, paused-at-cap, and failed outcomes; records append to run-local `notify.jsonl`; command notification failures include a recovery `try:` detail.
- Goal-shape recommendations: `start` performs one bounded provider-backed classifier call through existing provider routing (with deterministic fallback) to suggest single run, orchestration, or campaign + count; campaign `--n` is optional and editable before launch.
- Vocabulary and error-footers: user-facing copy treats a passed run as a "verified run" (verified by `dr-gate`) and groups `def-done` / `acceptance.yaml` wording under the "done contract"; P10 coverage asserts the shared glossary and a parameterized refusal table with final `try:` footers.
- Autonomous sequential chains: `chain "..."`, `chain plan`/`expand`, `chain run`, `chain attach`, `chain status/show/list`, `chain pause/resume/kill`, `chain undo`, `chain extend`, and `chain redo`; chains use `latest`/`last` aliases, `chain.json`, `chain-events.jsonl`, a conductor lock, chain hooks, aggregate spend caps, green-policy auto-apply, and a multi-step ratatui timeline with single-run chain context.
- Plan observability: orchestration plans now write `plan-events.jsonl`; `attach <plan-id>` renders plan events, drills into child run attach, and returns to the plan context; plain attach, `history grep --plan`, and `show --why-failed <plan-id>` include plan event evidence.
- Campaign observability: `attach <campaign-id>` opens a live campaign ratatui TUI on TTYs with sub-plan cards, roll-up/budget header, campaign/sub-plan feed rows, and `Enter` drill-in to the existing plan attach TUI; plan child drill-in then reaches the normal run attach TUI, and backing out twice returns to the campaign frame. Off-TTY/`--plain` remains the read-only summary, and `--json` emits a structured campaign attach object.
- Consolidated plan-result docs: completed plans write provider-backed or deterministic `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, `PLAN-CHILDREN.md`, and `PLAN-DOCS-MANIFEST.json`; merged libraries, apply worktrees, and exports carry those docs, and synthetic plan-result apply runs expose wrapper `RUN-*` docs that point at the consolidated plan story.
- Semantic merge repair: `merge <plan-id>` now defaults to DAG-aware composition, lets descendant tasks supersede ancestor file edits, writes conflict/repair sidecars under `merge-proofs/`, and automatically invokes a repair provider for true parallel conflicts unless `--no-repair` is set.
- Provider flight recorder & rewind: CLI-backed provider turns record `flight-manifest.json`, `flight-events.jsonl`, and delta `checkpoints/`; `show --flight` / `--file` inspects them and `rewind --to-turn|--to-provider-event|--to-checkpoint` previews or applies a hash-guarded checkpoint restore.
- Local self-improvement loop: `learn index|report|propose|export|import-bundle` builds a redacted local experience index and provider-backed proposals; `improve self … --preview|--yes|--pr-dry-run|--open-pr` runs evidence-gated self-run candidates with PR opening held behind an explicit evidence gate.
- Mock HTTP server for tests; CLI provider tests with fake binaries; integration coverage for stress, import round-trips, lifecycle, codebase modes, docs, sandbox policy, and gate proof.

The hygiene rider is purely structural; it does not close prior thin items, but it raises the floor for every future rider.

### Hardening v2 closures

The previously named thin areas now have code paths and depth tests:

1. **TUI streaming and responsiveness.** `tui_events.rs` covers broadcast attach, JSONL replay, partial-line handling, and kill visibility. The May 2026 responsiveness slice adds attach tick budgets, background/coalesced narrative refresh, attach-owned JSONL tailers, provider-log scan throttling, narrative projection caches, chain event tailing, and focused slow-narrator/large-worktree/max-chain smokes.
2. **Resume from partial trace.** `turn_loop` tests cover mid-tool-call truncation and `--from-turn` cleanup.
3. **Cancellation model.** `kill` writes a cancel marker before signals; tests cover cross-process marker semantics, HTTP aborts, and kill storms.
4. **Wall-clock spend for CLI providers.** CLI providers accumulate wall time and caps; richer subscription-to-budget policy remains a future routing concern.
5. **Sandbox profiles.** `sandbox.toml` drives per-tool policy; policy blocks disallowed filesystem/network access and records refusals.
6. **Doctor.** Local setup checks are actionable and exhaustive for the production CLI; provider network pings are opt-in.
7. **Import normalization.** JSONL/JSON/SQLite imports now carry source path/line metadata, raw hashes, content-hash manifests, deterministic imported run IDs, and normalized trace/provenance details, with golden-file `show` round trips and descriptor-provider fixtures.
8. **Acceptance gate.** `acceptance.yaml` supports structured checks and signed per-check results.
9. **Multi-run coordination.** Scope-qualified locks, stale reclaim, same-scope refusal tests, and sequential chain coordination are in place; parallel/DAG scheduling remains out of scope.
10. **Promotion / library workflow.** Promotion is atomic and `library list|search|show` makes artifacts discoverable by scope, goal, date, and promoted-doc content.
11. **Composable seams and API compaction.** Policy, catalog, hook, and event-sink workers are swappable through `[seams]` with fixed fail policies, while direct-API history is bounded by deterministic context-window compaction. The gate remains non-swappable and is protected by adversarial seam sandbox tests.

### Not yet built (V1+ candidates per `docs/goals/2026-05-11-1400-deadreckon-usability-rider.md` and the V1 list in the robust rider)

- Sub-agent forking as a user-facing CLI verb.
- Human-in-the-loop approval seam or long-lived worker bus.
- MCP client surface.
- Cost-aware provider routing.
- Cloud sync of histories.
- Voice / meeting capture.
- Real-time multi-cursor TUI presence.

The codebase-mode rider adds capability; it does not close the robust-rider thin items above.

The Effortless pass is presentation and advisory orchestration only. It did not add durable fields to `PipelineState`, `Plan`, `Campaign`, or provider schemas, and it did not change the gate, sandbox, promotion, provider, plan merge, or campaign core mechanisms. Bigger product bets such as palettes, localization, card templates, notifier daemons, and richer classification stay in `docs/V1-CANDIDATES.md`.

---

## 23. Glossary

- **Run** — a single invocation of `deadreckon run <goal>`. Has a `run_id` (UUID), a `task_key` (slug + hash of goal), and a `scope` (slug + hash of repo path). Materialized in `~/.deadreckon/runstate/<scope>/runs/<run_id>/`.
- **Scope** — `"<basename>-<fnv1a32-hex>"` of the canonical run-root path (git root or env var). Identifies a worktree.
- **Task key** — `"<slug-of-goal>-<fnv1a32-hex>"`. Multiple runs of the same goal share a task key and a lock.
- **Phase** — one of the seven gap-numbered stages of a run (`init`, `plan`, `provider`, `sandbox`, `execute`, `verify`, `complete`).
- **Provider** — an implementation of the `Provider` trait that takes a prompt and returns text + spend. Can be HTTP (`anthropic`/`openai`/`openai-compatible`), CLI subprocess (`cli:claude-code`/`cli:codex`), or scripted (`smoke`).
- **Action** — the JSON tag-typed enum the LLM emits per turn: `Bash`, `WriteFile`, `Done`. Parsed in the turn loop.
- **Sandbox** — process-isolation backend selected at run start (`Auto` → platform-native by default; `SandboxExec` / `Bwrap` / `Docker` / `None`).
- **Snapshot** — a full copy of `working/` taken before each turn. Lives at `snapshots/turn-<N>/`. Restored by `undo`.
- **Promotion** — the atomic swap of `working/` into `library/<scope>/<run_id>/`. Only runs that pass the gate get promoted.
- **Acceptance marker** — a signed JSON file (`proofs/turn-acceptance.json`) written by `dr-gate`. Its signature is tied to a per-run nonce only `dr-gate` reads.
- **Spend** — a record of LLM cost per turn. USD for HTTP providers; wall-clock seconds + `subscription: true` for CLI providers.
- **Provenance** — per-file attribution: which `tool_call_id` produced which file in which turn under which model.
- **Trace** — every LLM call and every tool dispatch, with latency + structured detail.
- **CLI sub-agent** — a `cli:*` provider whose `complete()` invocation is one whole turn (the sub-agent does its own tool calls inside). Detected by `response.trace["kind"] == "cli_subagent"`.
- **dr-gate** — the standalone binary at `crates/deadreckon/src/bin/dr-gate.rs` that owns acceptance verification. The agent cannot impersonate it.
- **BYOK** — Bring Your Own Key. In deadreckon this extends to subscriptions: a Claude Max or ChatGPT Pro user can drive deadreckon via `cli:*` providers without an API key.

---

## 24. Codebase Modes

### 24.1 Why Codebase-Aware Running Is The Default

The default `run` flow now gives the agent the user's project instead of an empty directory. This directly addresses the "agent never sees my repo" failure mode while preserving isolation: in git repos, deadreckon edits a new worktree and branch, not the user's checkout.

### 24.2 Mode Resolution

`run` resolves modes before state is written: `--fresh` keeps the old empty directory, `--in-place` requires explicit danger acknowledgement, `--worktree` forces git worktree mode, `--from <path>` forces copy mode, and a clean git repo defaults to worktree mode. Non-git interactive runs offer init/copy/cancel; non-interactive runs require an explicit mode or `--yes`.

### 24.3 Worktree Mode

Worktree mode creates branch `dr/<task-slug>-<run-id-prefix>` at `~/.deadreckon/worktrees/<scope>-<run-id-prefix>`. The branch is based on `--base` or the current branch. The source checkout is not touched until `deadreckon apply`.

### 24.4 Copy Mode

Copy mode seeds `runstate/<scope>/runs/<id>/working` from `--from <path>` using the `ignore` crate so `.gitignore`, `.ignore`, global gitignore, `.git`, `target`, and `node_modules` are not copied.

### 24.5 In-Place Mode

In-place mode sets `working_dir` to the source path and writes `.deadreckon/codebase.json` there. It requires `--in-place --i-know-its-a-lot` in non-interactive use. The user tree is edited directly; `undo` is the rollback tool.

### 24.6 Fresh Mode

Fresh mode is the previous empty-working-directory behavior behind `--fresh`. It records `mode: "fresh"` in `working/.deadreckon/codebase.json` and keeps existing smoke tests honest.

### 24.7 `codebase.json`

Mode metadata lives in `working/.deadreckon/codebase.json`, not `PipelineState`. Fields include `mode`, `source_path`, `source_git_root`, `branch_name`, `base_ref`, `base_sha`, `worktree_path`, dirty seeding flags, timestamp, and deadreckon version.

### 24.8 Worktree Preflight

Worktree preflight refuses non-git sources, repos with no commits, detached HEAD, mid-merge, mid-rebase, dirty trees unless `--allow-dirty`, branch collisions, and occupied worktree paths. Errors include `try:` lines.

### 24.9 Preview Block

Before file changes, `run` prints a single preview block with goal, source/git state, mode, branch/base/worktree when relevant, provider, sandbox, caps, and next verbs. `--preview` exits after printing; `--yes` skips confirmation.

### 24.10 `apply` / `abandon`

`apply` supports `squash` (default), `merge`, and `cherry-pick`. It refuses non-worktree runs and dirty user checkouts unless `--autostash` is set, then prints `git log -1 --stat`. `--cleanup` removes the temporary worktree/branch after successful apply. `abandon` / `discard` removes the worktree and branch when safe, supports `--keep-branch`, and writes `abandoned.json`.

### 24.10.1 `status`, `list`, and `cleanup`

`list` defaults to the current project scope so old runs from unrelated repos do
not dominate the common path. `list --all` scans every scope; `show <run-id>`
prints exact IDs, locations, docs, traces, and the next recommended action.
`status` (alias `next`) prints the latest current-project run, its
artifact/worktree locations, and the next recommended action. Running
`deadreckon` with no subcommand dispatches to `status`.

`cleanup` (alias `prune`) removes temporary run worktrees and branches for
abandoned, stale, or completed worktree runs, with opt-in `--completed`,
`--stale`, `--all-scopes`, `--escalate`, and `--overwrite` selectors. It does
not delete plan state, promoted library artifacts, or directories exported with
`deadreckon export`. The older `--all` and `--force` spellings remain hidden
compatibility aliases.

### 24.11 Integration With Existing Verbs

`materialize` refuses worktree runs with an `apply` hint and refuses in-place runs
with an `undo` hint. `list` shows `MODE`. `show` prints mode, branch, worktree,
and source lines. `undo` restores the original source path for in-place runs.
`extend` chains worktree children from the parent `dr/...` branch and records
`parent_branch` in the child's `codebase.json`; copy/fresh extension keeps the
library-seeding path, and in-place parents refuse with a `run --in-place` hint.
Extend now also carries the parent's done-criteria file into the child run via
`copy_existing_acceptance_into_run` (looking at `cwd` then `working_dir`),
matching the behavior of a fresh `run` so an extended turn is held to the same
gate as the original.
Other lifecycle commands continue to use files-not-fields metadata and snapshot
semantics.

### 24.12 Not Yet Built

V1 candidates remain in `docs/V1-CANDIDATES.md`: richer apply targets, remote-aware refresh, multi-repo orchestration, and conflict-resolution assistance. This section does not remove any robust-rider thin items.

---

## 25. Self-Documenting Runs

### 25.1 The Three Artifacts

Every run starts `working/.deadreckon/docs/`, seeds root-level `implementation-notes.html`, and writes three human-readable Markdown files:

- `RUN-NARRATIVE.md` for the chronological implementation story.
- `RUN-AS-BUILT.md` for the subsystem shape changed by the run.
- `RUN-DECISIONS.md` as the canonical implementation decision ledger: design decisions, deviations, tradeoffs, open questions, and evidence-filtered multi-alternative decision details.

When the worktree has a nearby `AS-BUILT-ARCHITECTURE.md` or `AS-BUILT.md` and the diff is broad enough, deadreckon also emits `AS-BUILT-DELTA.md` as a proposed amendment.

### 25.2 Frontmatter

The docs use stoa-style bold frontmatter: Date, Last updated, Status, Run ID, Goal, optional Parent run, Commit span or working-directory mode, Owner, Provider, Sandbox, Spend, and Doc-writer. Fresh runs omit commit span; copy and in-place runs identify their working path.

### 25.3 Per-Turn Templating

After every successful tool/provider turn, `crates/deadreckon-runtime/src/turn_loop.rs` calls the turn-end documentation checkpoint. The deterministic record lands in `_incremental.jsonl`, rewrites the Markdown drafts, projects current `implementation-notes.html` sections into `RUN-DECISIONS.md`, and emits a `docs_checkpoint` run event before the loop advances. This happens for both CLI sub-agent turns that complete in one provider process and JSON-action providers that may take many Bash/WriteFile/Done turns.

Each turn record carries the full provider response capped at 50 KB, a short response summary, per-file add/delete counts, largest diff-hunk excerpts, binary markers, optional stdout/stderr samples, trace citation, snapshot reference, and worktree commit SHA when available.

### 25.4 End-of-Run Polish Pass

Before acceptance/promotion, `polish_run_docs` first writes deterministic docs, then optionally runs provider-backed polish unless `--no-docs` is set. The default path resolves four repo/user/project skills in order: `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions`. Each subcall receives the same run evidence plus a focused prompt, uses a 16K output-token budget by default, retries once on malformed JSON, and contributes to the merged docs.

The legacy `run-narrator` single-call path remains available for custom installs that do not opt into split `doc_subskills`. Provider, JSON, or skill failures are nonfatal; templated docs remain and `polish.json` records `failed_subcall:<name>` when a split subcall failed. `--smoke` now implies `effective_no_docs` unless `--doc-provider` is explicitly passed, so deterministic smoke runs no longer attempt to call a live doc provider. The `deadreckon status` report reads `polish.json` when `docs_status_for_state` reports `Failed` and surfaces a `polish failed (<reason>); fallback docs are still available` line so the templated docs are visibly distinct from a successful polish.

### 25.5 Phase And Decision Detection

`docs.rs` coalesces turns into 3-8 phases by file overlap and tool-kind continuity. Decision candidates are detected with case-insensitive marker regexes and a minimum response length so incidental short phrases do not become multi-alternative decision details. Implementation interpretations do not need to satisfy that regex: they are read from `implementation-notes.html` and rendered into the four ledger sections of `RUN-DECISIONS.md`.

### 25.5.1 Implementation Notes Freshness

The run prompt frames the task as "Implement the SPEC", where the spec is the stored goal plus any copied `acceptance.md`, `acceptance.yaml`, or orchestration worker spec. `skills/default-coding/SKILL.md` tells the executor to maintain root `implementation-notes.html` with Design decisions, Deviations, Tradeoffs, and Open questions. Before a JSON-action provider's `done` action or a CLI sub-agent completion can advance to docs polish, acceptance, and promotion, the runtime checks that the file exists, contains all four sections, and was updated on or after the latest documentable source/config/test/doc turn. Stale notes do not immediately fail the run; the loop records a docs warning/error event, appends a history instruction asking the provider to update the notes, saves state, and continues. When the notes are current, deterministic docs are rewritten so `deadreckon doc <run-id> --kind decisions` is the primary inspection path.

### 25.6 Diff Coverage And Retry

After polish, deadreckon verifies every changed file appears in `RUN-NARRATIVE.md` by relative path or basename. Missing files trigger up to two targeted `narrator-phases` retries with an explicit omission list; other subskills are not re-run for phase coverage misses. Remaining omissions are logged as `docs.warning` traces and do not fail the run.

### 25.7 AS-BUILT-DELTA

The delta is generated for worktree runs whose source has an AS-BUILT file at the root or beside touched files and whose diff touches at least three files or adds public/exported API. Public docs are copied to `working/docs/`; the branch gets a `turn docs: deadreckon run docs for <id>` commit so `apply` carries docs forward.

### 25.8 Apply Commit Body

When `deadreckon apply` builds the default squash or merge message, it reads `RUN-NARRATIVE.md` and `RUN-DECISIONS.md` to include an executive summary, phase list, decision count, open-thread count, and a `docs/RUN-NARRATIVE.md` trace pointer. `--message` still overrides the generated body.

### 25.9 `deadreckon doc`

`deadreckon doc <run-id>` prints the narrative by default. `--kind as-built|decisions|delta` selects another run artifact, `--export <path>` writes it to disk, and `--overwrite` overwrites exports or a prior polish result. `--kind decisions` prints the converged implementation decision ledger, not only regex-detected choice points. For orchestration plans, `deadreckon doc <plan-id>` and `deadreckon doc <plan-result-wrapper-run-id>` resolve to consolidated `PLAN-*` docs; `--kind children` prints the plan child index. `--polish` prints a preview for run docs and refreshes provider-backed plan docs for plans, with deterministic fallback if provider output is unavailable or invalid. `--no-confirm` skips the run-doc prompt for scripts. `--doc-provider <route>` overrides the automatic documentation provider route and `--max-spend <usd>` limits the polish pass. The older `--force` and `--budget-cap` spellings remain hidden compatibility aliases.

### 25.10 Cost And Idempotency

`polish.json` stores a SHA-256 inputs hash over goal, traces, provenance, spend, incremental records, changed files, and source AS-BUILT content. Split-subcall paths write schema v2, recording `doc_provider_source`, `subcalls[]` with skill/status/provider/tokens/cost/duration/retries, `merged_at`, and `diff_coverage`; the legacy single-call path writes schema v1. A matching polished hash skips duplicate provider calls unless forced. CLI subscription providers report wall time rather than USD cost, but the doc-provider resolver still records whether the route came from a flag, config, auto-detected subscription CLI, run provider fallback, or no provider.

### 25.11 Skill Split Into Four Subskills

The default polish path resolves `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions` separately so each prompt owns one documentation surface. The legacy `run-narrator` skill remains as the single-call compatibility path.

### 25.12 Per-Turn Capture Richness

Turn docs preserve the provider response up to 50 KB, stdout/stderr up to 10 KB each, per-file add/delete counts, binary markers, and the largest textual hunk excerpt. These fields are stored in `_incremental.jsonl` rather than `PipelineState`.

### 25.13 Doc-Provider Auto-Resolution

Doc polish chooses `--doc-provider`, then `[defaults].doc_provider`, then in-PATH subscription CLIs from the provider registry, then the run provider. The same `setup.rs` resolver backs run, extend, resume, and `doc --polish`, so previews and start banners use the same provider-source labels (`flag`, `config`, `auto_subscription`, `run_provider`, `none`). If none resolve, the command fails with an actionable `try:` hint instead of silently leaving `Doc-writer: templated only`.

### 25.14 Component Inference And Topology

The deterministic as-built seed maps changed paths into concrete layers such as Rust crates, frontend components/routes, tests, documentation, manifests, migrations, and CI. Unmapped files are omitted instead of grouped under `Project files`; topology ASCII is emitted only when at least three top-level directories changed.

### 25.15 Polish Preview And Max Spend

`deadreckon doc <id> --polish` estimates the maximum output-token cost before calling the provider. Paid API routes are refused when the estimate exceeds `--max-spend` or `[defaults].doc_polish_budget_cap_usd`; subscription CLI routes estimate as `$0.00 (subscription)`.

---

## 26. Coherence Pass And Production Command Model

> **Status (2026-05-28):** The May 2026 coherence pass, closure pass, production command model, and Effortless pass are release-complete for the current CLI contract. Glossary labels, style helpers, `print_kv_block`, flag-truth, prompt builder, attach/kill parity, shared TUI palette, provider-route wording, provider/done-contract setup, JSON parity, orchestration commands, plan attach, polymorphic lifecycle ids, default-help command audience, history-aware `start`, verified-run wording, and done-contract review prompts now share the same user-facing model. The closure briefs are at `docs/goals/2026-05-13-1900-deadreckon-coherence-goal.md`, `docs/goals/2026-05-17-1403-deadreckon-coherence-closure-goal.md`, `docs/goals/2026-05-24-1426-deadreckon-provider-done-setup-goal.md`, and `docs/goals/2026-05-27-1152-deadreckon-production-command-model-goal.md`; the closure matrix is `docs/design/USER-FACING-MATRIX.md`, with larger follow-ups explicitly deferred to `docs/V1-CANDIDATES.md`. The pass is intentionally schema-preserving: no `RunStatus`/`ChainStatus`/`PlanTaskStatus` variant names changed, only display strings changed via `glossary.rs` and runtime setup helpers.

### 26.1 Glossary

`crates/deadreckon-core/src/glossary.rs` is the display vocabulary source for statuses and primary nouns. Stored enum variants keep their historical schema names, including `RunStatus::Executing`, but user-facing run and phase text now renders `running`. Chains, chain steps, plan status, and plan-child status use the same helper family, so `attach <plan-id> --plain`, `merge <plan-id>`, and `status <run-id>` agree on `running`.

### 26.2 Style Helpers

`crates/deadreckon/src/ui.rs` owns ANSI rendering through `Tone`, `Stream`, `render`, `writeln`, `hint`, line replacement, and ANSI stripping helpers. The small CLI facade (`ui_heading`, `ui_muted`, `ui_id`, `ui_command`, `ui_ok`, `ui_warn`, `ui_note`, `ui_status`, and `ui_error`) also lives there, so command modules import style intent instead of defining their own wrappers. Raw ANSI escapes are confined to `ui.rs`, and status labels route through `ui::status_tone` before choosing a tone. `failed`/`killed`, `paused`, `warning`, and `note` are separate style intents even when two intents currently share the same terminal color. The cyan `deadreckoning` banner, blue `* ^ . -` course strip, magenta IDs, spend gauge gradient, and chain glyph family remain product affordances.

Custom top-level help and `help-all` command discovery now render from `COMMAND_HELP_CATALOG` in `crates/deadreckon/src/main.rs`. The default help first screen presents the production model (`start`, `attach`, `status`, `list`, `finish`, setup, and control) rather than every callable verb. The top-level clap after-help no longer carries a duplicate command table; unit tests verify catalog row uniqueness, command audience classification, and that every catalog entry points at a real clap subcommand or explicit pseudo-row such as `<command> --help`. `help-all` states the discovery policy: advanced commands remain callable and documented there, while compatibility aliases stay inline on their canonical row.

### 26.3 Key/Value Layout

`print_kv_block` is a binary-private formatting helper backed by `ui::writeln` and `Tone::Plain`. Run start banners, run summaries, run locations, status cards, plan creation, and plan summaries now use lowercase keys, padded colons, and six-decimal spend where applicable.

### 26.4 Flag Truth

The CLI names intent before force. Kill paths use `--escalate`; destination overwrites use `--overwrite`; override paths use `--anyway`; chain and cleanup cross-scope commands use `--all-scopes`; status uses `--global`; run branch naming uses `--branch-name`; apply/finish target branches use `--into`, and apply output says work landed `into` the target branch; doc polish uses `--max-spend`; apply and finish git behavior use `--git-strategy`. Strategy words are scoped: `merge --strategy` is plan composition, `apply`/`finish --git-strategy` is git apply behavior, chain `--apply-mode` controls chain policy, and chain `--apply-strategy` controls the per-step git operation. The old spellings stay as hidden compatibility aliases for one release window. Chain branch policy displays `linear-merge` while accepting the old `merge` value. Cross-project help says "all project scopes" on run, chain, history, cleanup, and library surfaces; provider `--all` remains provider inventory rather than project scope.

Every visible `--plain` flag uses the same help definition: "Plain output without TUI, spinner, or ANSI affordances." Individual commands still implement their own plain-mode effect, such as `attach --plain` choosing the text summary instead of ratatui.

Output and scripting flags have a visible policy in `deadreckon help-all`. `--yes` confirms preflight previews for start/update-style commands. `--no-confirm` skips destructive or follow-up confirmations after the target is known. `--quiet` suppresses success chatter and post-action hints, never requested data or errors. `--plain` disables TUI, spinner, and ANSI affordances without implying quiet. `--json` is reserved for inspection/list surfaces and wins over styling and hints. `--no-hints` suppresses optional next-step hints; `DEADRECKON_HINTS=0|false|off|no` disables them globally for the process.

`deadreckon help-all` also carries the spend-cap glossary. A run cap is `run --max-spend` for one run, a per-child cap is `orchestrate`/`fork --max-spend` for each child run, an aggregate chain cap is `chain --max-spend`, and a doc polish cap is `doc --max-spend`.

### 26.5 Prompts

`crates/deadreckon/src/prompt.rs` owns `prompt::open` and `prompt::confirm`. Every `Y/n` and `y/N` confirmation now renders with the same `? question [Y/n]: ` shape. The high-spend prompt says `continue with --max-spend $N? [y/N]:`, and doc polish now treats Enter as the displayed yes default.

The user-facing skip model is split by timing. `--yes` belongs to preflight preview acceptance on commands such as `run`, `orchestrate`, `chain`, and shell-channel `update`; `--no-confirm` belongs to direct destructive or follow-up actions such as `finish`, `apply`, `cleanup`, chain recovery commands, and doc polish.

### 26.6 Attach And Kill Parity

`attach <id>` prints `attaching to run|chain|plan <prefix>` to stderr before opening a TUI. `kill <id>` accepts run, chain, and plan ids and routes through one banner shape; plan kill keeps the plan-only process count.

### 26.7 TUI Palette And Parity

`ui::TUI_PALETTE` names the shared TUI color slots for focused borders, acceptance states, run states, and spend thresholds. The chain attach poll cadence matches the run TUI at 200 ms (`event::poll` timeout); the plan TUI uses 250 ms. Applied chain steps render `◉`, so applied and running no longer share `●`. The spend gauge keeps the green, yellow, red, and cap-paused magenta thresholds; above 60 percent, the title exposes the budget percentage so the label remains readable at narrow widths.

### 26.8 Provider And Failure Vocabulary

Provider displays use the provider/route/model/kind vocabulary consistently. Human provider lists and detection rows use the same `kind=cli|http|local-http|scripted` tokens, and the configured route is marked with `*` in the selection and registry views. `setup.rs` owns runtime provider setup rows for config/default-provider, primary run, doc polish, planner, child, coder, reviewer, and repair roles; it validates unknown routes before writes, reports source/credential/install state, and leaves built-in fallback routing unforced for normal run defaults. `show --why-failed` and `chain show --why-failed` now route through one failure-summary renderer with shared `status:`, `reason:`, `evidence:`, and `try:` sections.

`deadreckon help-all` includes the provider-role glossary. `--provider` is the primary run provider route and the default child route in full-plan orchestration. `--planner-provider` writes the full-plan child graph. `--child-provider IDX=PROVIDER` overrides a specific child. `--coder-provider` performs the review-mode implementation pass. `--reviewer-provider` independently reviews or fixes the coder result. `--doc-provider` handles documentation polish, resolving through explicit flag, config, subscription CLI, then run provider. `--repair-provider` handles merge repair planning and repair-child runs. Normal user surfaces say provider route/model/kind; descriptor remains registry documentation vocabulary.

Done-contract setup also resolves through `setup.rs`. Explicit `--acceptance <path>`, project `.deadreckon/acceptance.yaml`, generated criteria from `def-done`/pre-run drafting, and default `dr-gate` behavior all produce one `DoneCriteriaSelection`. User-facing previews and orchestration preflights say `done contract`; technical files, gate proofs, and hidden compatibility commands may still say `acceptance.yaml` or `gate`.

Plan merge/result output keeps the plan id as the primary object. The synthesized run id is labeled as a secondary result run, and the promoted path is labeled as the artifact library so users can still inspect implementation details without mistaking them for the main lifecycle id.

### 26.9 JSON Parity

Inspection surfaces that already read durable state now expose `--json`: `list`, `chain list`, `providers list`, `library list`, `status`, `show`, `detect`, and `doctor`. `plan --json` is also available for the write-plan-only preview surface, returning the saved plan id, status, paths, and next action without human hints. Representative JSON responses are top-level objects with `kind`, `id`, `status`, `next_actions`, `try_lines`, `paths`, and the existing named payload (`run`, `plan`, `runs`, `providers`, `chains`, and so on) for compatibility. JSON mode disables ANSI and optional hints. State-changing start/merge/fork/update actions remain text-first in the current release.

### 26.10 Deferred V1 Work

Mass renaming stored enum variants, themable palettes, localization hooks, a full output-layout facade, generic lifecycle renderer, command-matrix golden snapshots, and a template engine for status cards stay in `docs/V1-CANDIDATES.md`. Provider and done-contract setup unification has landed as the production runtime layer, so the remaining V1 work is deeper output-layout/golden coverage and richer interactive setup polish rather than another resolver. The orchestration live-UX slice has landed shared role/dependency/repair summaries and the `PlanEventBus` feed; remaining orchestration work is now the broader interactive setup/output-layout polish, not the basic live attach freshness gap.

---

## 27. Overnight UX

### 27.1 Card Vocabulary

`crates/deadreckon/src/ui_card.rs` is the shared CLI card renderer for run previews, run exit summaries, and completed attach footers. It has ANSI-aware visible-width helpers, deterministic layout, ASCII fallback under `--plain` / `NO_COLOR`, and a narrow-terminal fallback below 40 columns. `crates/deadreckon/src/cards/exit_summary.rs` builds the shared run outcome card used after run/resume/kill paths. Read-only inspection surfaces such as `list`, `show`, and `status` keep quieter table/report output so they do not duplicate the same metadata inside cards.

### 27.2 Sleep Prevention

`deadreckon run --prevent-sleep <auto|on|off>` defaults through `[defaults].prevent_sleep`. `auto` arms only for interactive runs; `on` forces a platform attempt; `off` skips. macOS launches `caffeinate -di` for the run-loop lifetime. Linux re-execs under `systemd-inhibit` with a trusted tmpdir ready-file handshake before run state is created, then writes run-local metadata from the inhibited child. Windows reports unsupported and remains a V1 candidate.

Sleep state is file-based, not a `PipelineState` field: `working/.deadreckon/sleep-prevention.json` records `mode`, `pid`, `armed_at`, `inhibitor_binary`, `reason`, and `skip_reason`. The RAII handle removes the file and reaps the inhibitor on drop.

### 27.3 Unattended-Git Hardening

`crates/deadreckon-core/src/git.rs` is the production git boundary. It exports `GIT_TERMINAL_PROMPT=0` for every git child and inserts `-c commit.gpgsign=false -c tag.gpgsign=false -c gpg.format=` for commit-family verbs (`commit`, `merge`, `cherry-pick`, `rebase`, `tag`, `am`, `revert`). A grep-style depth test rejects raw production `Command::new("git")` outside that helper.

### 27.4 Honest Spend Display

`deadreckon_core::spend_summary` replays `spend.jsonl` and reports total USD, token totals, wall seconds, and sticky `any_subscription_turn` / `any_estimated_turn` flags. Exit cards render `~$N.NNNNNN` when either flag is true so subscription and estimated dollar displays do not imply more precision than the data has.

---

## 28. Chains & Autonomous Goal Chaining

### 28.1 Mental Model

A chain is an ordered list of step goals plus branch, apply, budget, and stop policy. The conductor is a CLI process entered by `deadreckon chain ...` or `deadreckon chain run <id>`. It acquires a chain lock, spawns each step as a normal `deadreckon run --worktree`, waits for that run to complete and pass acceptance, applies it to the source branch when policy allows, then bases the next step on the updated head.

Chain state is separate from `PipelineState`: no run schema fields were added. Files live under `~/.deadreckon/chains/<chain-id>/`.

### 28.2 `chain.json`

`crates/deadreckon-core/src/chain.rs` defines `Chain`, `ChainStep`, and `ConductorState`. The top-level chain records `chain_id`, `root_goal`, ordered `steps`, `branch_policy`, `apply_mode`, `apply_strategy`, `apply_allowlist`, `on_fail`, circuit-breaker counters, aggregate spend/wall caps, scope, base branch/SHA, cwd, provider/model/sandbox, status, pause/failure reason, conductor pid, timestamps, and deadreckon version.

Each `ChainStep` records index, goal, status, run id, applied timestamp/SHA, failure reason, step cap, and actual spend. `ConductorState` is the live-process pointer in `conductor.json`: conductor pid, live step, live run id, and live child pid.

### 28.3 Create And Run Shape

The common path mirrors `run`:

```bash
deadreckon chain "step one" "step two" "step three" --yes
deadreckon chain plan "build a chess app" --n 6 --yes
```

`chain expand` is an alias for `chain plan`. `--from-file` and `--from-stdin` accept newline-separated steps. `--draft` writes `chain.json` without starting the conductor. Bare `deadreckon chain` prints scoped status; `deadreckon chain run` resumes `latest`; `latest` and `last` are accepted anywhere a chain id is expected.

### 28.4 Branch Policy

`stack` bases step N+1 on the SHA applied by step N. `base` bases every step on the original chain base SHA. `linear-merge` follows stack semantics but forces `apply --git-strategy merge`, producing merge commits instead of squash commits. The old `merge` branch-policy value remains accepted as a hidden compatibility alias.

### 28.5 Apply-Mode Green Policy

`apply_mode=auto` advances only after the inner run completes, the acceptance marker validates, the target tree is clean, file changes match the allowlist when configured, and `on-promote` hooks accept the change. Failure pauses the chain and writes an actionable pause reason. `preview` and `manual` pause after the inner run so the user can inspect/apply explicitly.

### 28.6 Conductor Lifecycle And Locks

The conductor holds a `chain--<id>` lock while active. Inner runs keep their normal task locks. The conductor writes `conductor.json` before work starts and updates it while a child run is live. `chain kill` reads that file, kills the inner run through the normal cancel-marker path when a live run id exists, signals the child process, signals the conductor, waits briefly, then escalates.

### 28.7 Hook Contract

Hooks resolve in project, user, then repo order:

```text
.deadreckon/hooks/chain/<hook>
~/.deadreckon/hooks/chain/<hook>
/Users/gdc/deadreckon/hooks/chain/<hook>
```

Supported hooks are `pre-step`, `post-step`, `on-promote`, and `on-chain-end`. Payloads are JSON on stdin plus `DEADRECKON_CHAIN_ID`, `DEADRECKON_HOME`, and `DEADRECKON_STEP_INDEX`. Exit `0` proceeds. Exit `1` pauses or skips where defined. Exit `2` refuses/fails the transition. Every invocation appends `chain_hook_invoked`.

### 28.8 Events And Promotion

`chain-events.jsonl` is the chain audit log: created, step started, run completed, apply started, applied/refused, step failed, paused/resumed/killed/completed, undo, hooks, extend, and redo. `promotion.rs` also emits `RunPromoted { library_dir }`, so a chain can attach provenance to the promoted inner run artifact. Chain attach reads this file through an attach-local JSONL tail cache after the first load; redraws parse appended complete rows only, tolerate a partial final line, and keep the previous activity rows visible if a read is delayed.

### 28.9 Lifecycle Verbs

`chain pause`, `resume`, `kill`, `undo`, `extend`, and `redo` compose with the existing run lifecycle. Undo reverts applied SHAs in reverse order. Extend inserts or appends a step and can reopen a completed chain when inserting. Redo chooses a specified step, the first failed step, or the latest applied step; applied-step redo requires `--reapply`, which reverts before requeueing.

### 28.10 TUI Surfaces

`chain attach <id>` opens a ratatui step timeline on TTYs and falls back to a plain snapshot off-TTY. The timeline shows policy, spend, step dots/statuses/run prefixes, recent chain activity, and controls for drill/show, redo, extend, pause, kill, detach, and scrolling. Activity reads are incremental and the activity panel title can show a catch-up/partial-line/read-delayed hint when event reading falls behind. Single-run `attach` reads `.deadreckon/chain-step.json`, renders a chain context banner, and exposes `[c] Chain` to drill out to `chain attach`.

### 28.11 Spend And Budgeting

`--max-spend` is aggregate. Each pending step receives `(remaining cap)/(remaining pending steps)` as its inner run cap. The conductor reads the completed run's state and adds actual spend/wall time back into `chain.json`. `resume --max-spend-add` increases the aggregate ceiling; `--reset-breaker` clears the consecutive failure counter.

### 28.12 Not Yet Built

Out of scope for the current release: mid-chain provider replanning, parallel/DAG steps inside one chain, cross-machine handoff, cloud sync, and a richer conflict-resolution UI. Those remain V1 candidates.

---

## 29. Workspace Hygiene

### 29.1 Centralized Lints

The root `Cargo.toml` owns `[workspace.lints]` for Rust and clippy policy. Every crate inherits it with `[lints] workspace = true`, so deny-tier rules such as `unsafe_code`, `unused_must_use`, `unwrap_used`, `expect_used`, `await_holding_lock`, `redundant_clone`, `needless_borrow`, and the `manual_*` family are enforced from one place. `clippy.toml` keeps test ergonomics explicit with test-only unwrap/expect/dbg exemptions and a `large-error-threshold = 256`.

### 29.2 Formatted Imports

`rustfmt.toml` pins edition `2024`, import reordering, `imports_granularity = "Item"`, and `group_imports = "StdExternalCrate"`. Stable rustfmt currently warns that the last two knobs are nightly-only, but `cargo fmt --check` still exits cleanly and the guard tests ensure the config remains intentional.

### 29.3 Tuned Profiles

`[profile.release]` uses fat LTO, one codegen unit, symbol stripping, and `split-debuginfo = "off"` while keeping the pre-existing panic behavior explicit with `panic = "unwind"`. `[profile.dev] debug = "limited"` keeps local compiles lighter without changing runtime behavior.

### 29.4 Internal Crate Routing

The four internal libraries (`deadreckon-core`, `deadreckon-runtime`, `deadreckon-providers`, `deadreckon-sandbox`) are declared in root `[workspace.dependencies]`. Member crates depend on them through `{ workspace = true }`, which keeps the internal dependency graph centralized and lets metadata tests detect accidental raw `../` path dependencies.

### 29.5 Registry-Shaped Library Roots

Every library `lib.rs` is a registry: crate docs, sorted `mod` declarations, sorted `pub mod` declarations, and sorted `pub use` re-exports. Business logic, `impl` blocks, and helper functions live in real modules. Public re-export set tests guard the pre-rider surface.

### 29.6 Library Print Refusal

Library crate roots deny `clippy::print_stdout` and `clippy::print_stderr`. User-facing output remains the responsibility of the `deadreckon` binary crate, where formatted CLI/TUI output belongs.

### 29.7 Error Taxonomy

`DeadreckonError`, `ProviderError`, and `SandboxError` expose exhaustive `is_retryable()` and `is_fatal()` methods. I/O interruptions, timeouts, reset/aborted connections, broken pipes, and held locks are retryable; schema, config, credential, routing, invalid-input, not-found, CLI, sandbox, cancellation, and status-less HTTP errors are fatal. These methods are vocabulary only in this pass; watchdog wiring remains a future behavior change.

### 29.8 Behavior Invariants

`tests/smoke_invariant.rs` protects the smoke-run narrative hash, and `tests/public_surface.rs` protects the public library re-export set. `tests/hygiene_config.rs` adds targeted guards for lints, formatting config, profile settings, internal dependency routing, registry shape, print refusal, binary size, cargo metadata stability, and error taxonomy.

---

## 30. Plans & Multi-Agent Orchestration

### 30.1 Mental Model

An orchestration plan is a file-backed task graph under `~/.deadreckon/plans/<plan-id>/`. It does not add fields to `PipelineState`; child work is still executed by normal `deadreckon run` subprocesses. The coordinator process owns only plan files: `plan.json`, `coordinator.json` while live, `messages.jsonl`, worker specs, child summaries, merge working state, and merge proofs.

Two modes are built:

- `full-plan`: `deadreckon orchestrate full-plan <goal>` or `deadreckon plan <goal> --mode full-plan --n <2..=6>` asks a read-only planner provider for task JSON, records planner/default-child/per-child providers, writes worker specs, and later `fork` starts independent ready tasks as a concurrent batch. Planner output must contain exactly the requested task count; single-task decompositions and values outside `2..=6` are refused before `plan.json` is saved.
- `review`: `deadreckon orchestrate review <goal> --coder-provider <id> --reviewer-provider <id>` writes a coder task and a reviewer task. The reviewer is launched with `deadreckon extend <coder-run-id> ...` after the coder completes, so parent history and `extended_from_parent` trace lineage are preserved.

### 30.2 Plan Files

`crates/deadreckon-core/src/plan.rs` defines `Plan`, `PlanTask`, `PlanProviders`, `PlanMessage`, `PlanEvent`, `PlanChildMarker`, and `CoordinatorState`. The durable layout is:

```text
~/.deadreckon/plans/<plan-id>/
  plan.json
  coordinator.json          # present only while fork is supervising
  messages.jsonl
  plan-events.jsonl
  docs/PLAN-NARRATIVE.md
  docs/PLAN-AS-BUILT.md
  docs/PLAN-DECISIONS.md
  docs/PLAN-CHILDREN.md
  docs/PLAN-DOCS-MANIFEST.json
  docs/plan-doc-input.json
  docs/_plan-docs.jsonl
  worker-specs/task-0.md
  summaries/task-0.md
  merge-working/
  merge-proofs/conflicts.json
  merge-proofs/repair-request.json
  merge-proofs/repair-plan.json
  merge-proofs/repair-run.json
```

`messages.jsonl` remains the typed coordinator mailbox. `plan-events.jsonl` is the append-only orchestration timeline: plan created/started, task ready/started/run-discovered/completed/blocked/failed/killed, merge started/conflicted/repair-planned/repair-started/repair-run-discovered/repaired/repair-failed/completed, and plan completed/failed/killed. Plan docs are written after a successful merge or on `deadreckon doc <plan-id> --polish`: the collector reads child run docs, child summaries, worker specs, merge repair evidence, and final result inventory in task-graph order, then writes `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, `PLAN-CHILDREN.md`, and `PLAN-DOCS-MANIFEST.json`. A configured doc provider can consolidate the input into cited prose; invalid, missing, or over-thin provider output falls back to deterministic docs rather than blocking merge/apply/export. The event stream is file-backed like `chain-events.jsonl`; child turn/tool traces stay in each child run's normal `events.jsonl`, `traces.jsonl`, and `spend.jsonl`.

Plan-result materialization is allowlisted. Merged libraries, plan apply worktrees, and exported plan artifacts receive `.deadreckon/docs/PLAN-*` plus public `docs/PLAN-*`; broad `.deadreckon` and child `docs/RUN-*` skip rules still prevent copying child internals. Synthetic `deadreckon:orchestrate-apply` runs rewrite their own `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, and `RUN-DECISIONS.md` as wrapper docs that identify the plan id/result run and link to consolidated `PLAN-*` docs instead of presenting zero-turn templated docs as the work product. `deadreckon doc`/`docs` resolves plan ids and plan-result wrapper run ids to these plan docs, and `show` reports plan doc status.

Every child run receives an inline copy of its worker spec in the prompt. The spec includes root goal, exact task scope, provider, role, dependency context, capability preview, and hygiene rules such as staying within scope and not spawning subagents. The current worker-spec posture borrows Claude Code's coordinator rules: the spec is the complete brief, children should not inspect sibling transcripts, corrections stay with the worker that has the failure context, and reviewer lanes verify independently rather than inheriting coder assumptions.

At launch time the coordinator rewrites the spec for dependent tasks with completed predecessor summaries, so later children receive concrete run ids, summary paths, changed-file context, and predecessor status rather than only a bare dependency id. Plan children also run with `--no-docs`; orchestration docs come from plan summaries and merge manifests, not from each child invoking provider-backed narrator work.

### 30.3 Verbs

- `plan <goal>` writes `plan.json` and worker specs. It previews provider roles, capability hints, task labels, dependencies, and next actions, then prints the shared provider role table and dependency/parallelism summary.
- `fork <plan-id>` runs ready child tasks through `deadreckon run`, using distinct plan-child scopes via `DEADRECKON_SCOPE_ROOT`. It writes typed progress/blocker messages, child summaries, and plan events for ready/running/run-discovered/terminal child states. While a child starts, the coordinator records the run id in `plans/<plan-id>/launch/<task-id>/run-id` so later plan-level kill/recovery commands can map a live process back to durable run state. Fork completion uses the same provider role table and dependency/parallelism summary as plan and orchestrate.
- `merge <plan-id>` composes completed child library artifacts into a new promoted run. The default strategy is `dag-aware`: if task B depends on task A and both changed the same file, B's version wins without prompting. True parallel conflicts write `merge-proofs/conflicts.json` and `merge-proofs/repair-request.json`, then automatically ask the repair provider for a JSON decision. Valid decisions can prefer a child file, synthesize only named conflict paths, or launch a normal repair child from `merge-working`; `--no-repair` restores raw conflict refusal, and `--repair-mode`, `--repair-provider`, and `--repair-attempts` are advanced controls. Merge start, conflict, repair, success, and plan completion are recorded as plan events. Merge and plan summaries show structured repair detail: mode, attempts, provider, conflict paths, repair sidecars, repair run status, latest repair event, and next action.
- `orchestrate review <goal>` and `orchestrate full-plan <goal>` are the one-command wrappers. Both print a preflight with mode, provider role table, dependency/parallelism summary, sandbox, caps, capabilities, merge-repair posture, and task rows before starting child work; headless callers pass `--yes`, `--preview` writes the plan and stops before `fork`, and automatic merge repair stays enabled unless an advanced/debug caller passes `--no-repair`.
- `attach <plan-id>` opens a plan TUI on TTYs and renders a plain summary off-TTY. The TUI shows child panes with provider/role/status, run prefixes, dependency state, turn/status, spend or token accounting, latest trace activity, acceptance/gate state, summary paths, and plan feed activity. `PlanEventBus` supplies plan events, snapshots, child run events, and repair run events; coordinator messages remain the fallback before events exist. `Enter` drills into the selected child run using the normal run attach view; quitting the child view returns to the plan selection with a parent-plan breadcrumb and back hint. Plan narrative refreshes are plan-keyed background jobs, visual toggles remain local, and render uses cached plan projections so a slow narrator cannot block selection, child drill-in, detach, or redraw.
- Headless flags are honored across this surface: `run --quiet` emits no success stdout, `run --plain --quiet` emits only the final plain status line, and `attach --plain` forces summary output instead of ratatui.
- `kill <plan-id>` reads `coordinator.json`, launch run-id sidecars, and child run state to signal the coordinator and live children, then marks discovered child states killed, releases their locks, and records task/plan kill events.
- `history grep <pattern>` searches durable trace or provenance JSONL, can restrict to a plan's child runs with `--plan <plan-id>`, includes matching plan events, and supports regex, scope, age, and limit filters.
- `show <id> --why-failed` explains the likely failure surface for a run or plan, including non-completed children, blocker messages, latest plan events, merge repair sidecars, and recent trace errors.

### 30.4 Merge Artifact

Merge creates a normal promoted run so existing `materialize`, `library`, and run inspection paths keep working. Plan ids resolve through the same prefix-matching path as run ids, so `apply <plan-id>`, `finish <plan-id>`, `export <plan-id>`, and `materialize <plan-id>` all accept either a full plan id or an unambiguous prefix and route through `resolve_plan_result_run` (`main.rs:10788`) onto the merged promoted run. `apply <plan-id>` lands the merged tree on the source branch (with `--autostash` / `--cleanup` honored exactly as for normal runs); `finish <plan-id>` picks `apply` for git worktrees and `export` for non-git sources; `export <plan-id> --dest <path>` writes the merged library to disk. The promoted library also gets `deadreckon-plan-manifest.json` with plan id, root goal, mode, provider roles, capability preview, task graph, child run ids, summary paths, coordinator message counts, and recorded conflicts.

Conflict bundles are versioned JSON objects. `conflicts.json` records the strategy, conflict path, child indexes, task ids, run ids, artifact roots, artifact file paths, content hashes, and dependency edges. `repair-request.json` adds root goal, task graph, worker-spec paths, child summary paths, recent plan events, and `merge-working` so the planner can decide without reading sibling transcripts. `repair-plan.json` stores the validated provider decision and rationale. If the planner chooses a repair child, `repair-run.json` stores the normal run id/scope/status, and the repaired promoted library is copied back into `merge-working` before the final plan merge run is created.

Generated run artifacts are intentionally excluded from merge composition: `.deadreckon/*`, `docs/RUN-*`, `target`, `node_modules`, `.next`, `dist`, and `build`.

### 30.5 Current Limits

The plan event feed is still durable-file backed for cross-process attach, and long-lived same-process plan writers are not yet all routed through a shared broadcaster. The TUI no longer owns raw `read_plan_events_lossy` polling, but child and repair run detail still ultimately comes from normal run files. There is no attach daemon, shared cross-surface broadcaster, or live diagnostic dashboard yet; those remain V1 candidates rather than hidden runtime dependencies. There is also no arbitrary child-to-child chat surface: children communicate through durable summaries and typed coordinator messages only.

---

## 31. Distribution & Self-Update

### 31.1 Install Receipts And Update Cache

The install channel is durable, not guessed on every command. `crates/deadreckon-core/src/install_receipt.rs` defines `Receipt` with `channel` (a lowercase enum: `npm`, `brew`, `shell`, `cargo`, or `source`), `channel_version`, `binary_path`, `installed_at` (ISO-8601 UTC), optional `install_source` and `platform_package`, and `receipt_version`. The receipt is stored at `~/.deadreckon/install-receipt.json`.

When a receipt is missing, normal `deadreckon update` detects the current binary path and persists the inferred receipt before routing the update. `deadreckon update --check` deliberately remains read-only and does not write that receipt. Detection recognizes npm package layouts, Homebrew Cellar paths, `~/.cargo/bin`, shell installer paths under `~/.local/share/deadreckon` or `%LOCALAPPDATA%/deadreckon`, and falls back to `source`.

`crates/deadreckon-core/src/update_cache.rs` stores `~/.deadreckon/update-check.json` with a 24-hour TTL. Startup update checks are opportunistic: they skip non-TTYs, `doctor`, `update`, source installs, and `DEADRECKON_UPDATE_CHECK=0`, then print only a stale-version hint instead of blocking the requested command.

### 31.2 User Update Flow

`deadreckon update --check` prints the current channel/version and the latest GitHub release result when available. Network failure degrades to the cache or the current version rather than turning routine checks into hard failures.

For native package-manager installs, `deadreckon update` prints the command the user should run:

```text
npm:  npm update -g deadreckon
brew: brew upgrade deadreckon
cargo: cargo binstall deadreckon || cargo install deadreckon
source: cargo install --path crates/deadreckon
```

Shell-channel installs are updated in place through `axoupdater`. Before mutating anything, the CLI resolves the target release and prints the current version, target version, installer/archive URL, checksum note, and planned backup directory. Interactive shells prompt for confirmation; non-interactive shells must pass `--yes`. After confirmation, the current binary is copied into `~/.deadreckon/update-backups/<timestamp>-<n>/`, the swap runs, failed swaps restore the backup, and successful swaps prune old backups down to the newest three and print `try: deadreckon doctor`.

### 31.3 Release Packaging

`dist-workspace.toml` is the release manifest. It pins `cargo-dist` 0.31.0, enables shell, PowerShell, and Homebrew installers, targets macOS arm64/x64, Linux arm64/x64, and Windows x64, and records glibc 2.28 for the Linux GNU builds. The bundled cargo-dist npm installer is intentionally disabled because npm is owned by the explicit wrapper packages under `npm/`.

`.github/workflows/release.yml` runs a dist plan on ordinary pushes and pull requests. The first job normalizes release lane metadata (`branch`, `rc`, `stable`, or `invalid_tag`) through `release/trust/release-trust.mjs` so publish, signing, attestation, npm, and Homebrew jobs share one policy. Official RC/stable tags fail preflight if macOS signing/notarization material is missing. The macOS job builds the cargo-dist archive first, imports the Developer ID certificate, then `release/trust/sign-macos-artifacts.mjs` extracts the packaged archive, signs the packaged binary, verifies it with `codesign`, submits it with `notarytool`, repacks the upload artifact, and writes trust status for the release manifest.

After local/global artifacts are built, the workflow generates `release.spdx.json`, `SHA256SUMS`, and `release-manifest.json`, verifies manifest/checksum coverage, verifies Homebrew formula integrity against `SHA256SUMS`, uploads those trust artifacts explicitly to the GitHub Release, appends verification commands to the release notes, and uses `actions/attest@v4` for official RC/stable artifact attestations. RC tags publish a GitHub prerelease only; stable tags publish GitHub Release, Homebrew, and npm. Stable Windows artifacts require `WINDOWS_CERT_PFX` and `WINDOWS_CERT_PWD`; the Windows job extracts the cargo-dist zip, signs `deadreckon.exe` with `signtool`, verifies it, repacks the zip, and records trust status for the manifest.

`docs/RELEASE.md` is the operator runbook. It names the Apple signing/notarization secrets, `HOMEBREW_TAP_TOKEN`, npm trusted-publishing/`NPM_TOKEN` options, release verification commands, and the Apple Developer ID export/base64/app-password checklist. It records that pushing a release tag is an operator action rather than something an agent should do.

### 31.4 npm And Homebrew

The npm distribution uses a small `deadreckon` wrapper package with optional dependencies on five platform packages: darwin-arm64, darwin-x64, linux-arm64, linux-x64, and win32-x64. `npm/scripts/prepare-release.mjs` repacks cargo-dist artifacts into those platform package directories and updates versions from the tag. The wrapper `postinstall` (`npm/deadreckon/scripts/postinstall.js`) runs at install time, detects which platform package was selected, and writes an `install-receipt.json` with `channel: "npm"`, `install_source: "npm:deadreckon@<version>"`, and `platform_package: "<detected>"` — no network needed at update time. `.github/workflows/publish-npm.yml` validates the stable release lane, then publishes the platform packages first and wrapper last with `npm publish --provenance`; it supports npm trusted publishing through OIDC or `NPM_TOKEN` plus provenance.

Homebrew publishing uses cargo-dist's generated formula as the starting point. `release/homebrew/patch-formula.mjs` then injects a `write_deadreckon_receipt!` method into the formula and calls it from the `install` block; the injected code writes `install-receipt.json` with `channel: "brew"` and `install_source: "brew:gdc/tap/deadreckon"` at install time. cargo-dist itself still owns release-archive SHA-256 pinning. The release workflow publishes the patched formula into `gdc/homebrew-tap` with `HOMEBREW_TAP_TOKEN`.

### 31.5 Current Limits

The release path is wired and depth-tested, but the first public release still requires operator setup: configure repository secrets/variables, create/push the version tag, watch the GitHub Actions release, and verify npm/Homebrew/GitHub artifacts after publishing. Local tests skip `cargo dist plan` when `cargo-dist` is not installed, so installing cargo-dist locally gives one more pre-tag confidence check.

---

## 32. Plan Observability

### 32.1 Event Stream

Plans now have their own append-only timeline at `~/.deadreckon/plans/<plan-id>/plan-events.jsonl`. Each row is a `PlanEvent` with UTC timestamp, `plan_id`, and a snake-case tagged `PlanEventKind`. The core helper pair is `append_plan_event` / `read_plan_events`, exported from `deadreckon-core`; the path helper is `DeadreckonPaths::plan_events`.

The stream is orchestration-level only. It records plan and task lifecycle edges, child run discovery, merge edges, semantic repair planning/execution edges, kill edges, and final failure/completion. It does not copy child turn/tool traces. A selected child remains a normal run with its own `events.jsonl`, `traces.jsonl`, provider logs, acceptance proofs, spend records, and attach renderer.

### 32.2 Emission Points

`plan` appends `plan_created` after `plan.json` is saved. `fork` appends `plan_started` exactly once per invocation, then `task_ready` for each task in the ready batch, `task_started` before each child spawn, `task_run_discovered` when a PID or run id becomes known, and one of `task_completed` / `task_failed` / `task_blocked` / `task_killed` on each terminal transition (including dependency-blocker events). `merge` appends `merge_started`, optional `merge_conflict`, then the optional repair sequence (`merge_repair_planned` / `merge_repair_started` / `merge_repair_run_discovered` / `merge_repaired` or `merge_repair_failed`), then `merge_completed` and `plan_completed`. Idempotency and ordering are covered by depth tests added in commit `4fcb7f9`. `kill <plan-id>` appends `task_killed` per child, then `plan_killed` and `plan_failed`, preserving discovered run ids and PIDs even if a child already reached a terminal state before the kill sweep loaded it so recovery commands can still map live processes back to durable state.

### 32.3 User Surfaces

`attach <plan-id>` now renders `PlanEventBus` feed activity in the plan activity pane, falling back to coordinator messages only before the event file exists. The feed replays durable plan events, emits snapshots when plan status or child run ids change, and multiplexes discovered child and repair run events without copying child traces into `plan-events.jsonl`. `Enter` on a child with a run id suspends the plan TUI and opens the existing run attach view with a breadcrumb like `plan <prefix> / task-1`; `q`, `Esc`, or `Ctrl-D` returns to the same plan attach context. Slow provider narration is decoupled from this path: polling a pending plan narrator returns immediately, visual mode changes are local state changes, and completing or cancelling the job only affects the next narrative projection.

Plain/off-TTY `attach <plan-id>` prints the latest plan event, merge repair status when sidecars exist, and an explicit `deadreckon attach <plan-id>` hint. `history grep <pattern> --plan <plan-id>` searches `plan-events.jsonl` before child run trace/provenance files, so repair events are grep-visible. `show <plan-id> --why-failed` includes the latest plan event and merge repair sidecar paths alongside failed child rows and blocker messages.

`attach <plan-id> --view narrative` uses the same plan feed plus current `Plan` state to render a plan-level operator story. The narrative pane lists plan status, task/role/provider rows, dependency and coordination notes, risks, next likely orchestration moves, and an agent or architecture visual. The narrative footer keeps `n`/`v`/`r` controls visible even before the selected task has a child run, then appends the `Enter waits`/`try: deadreckon fork` hint so raw activity remains one key away. It does not copy child traces into `plan-events.jsonl`; child runs remain normal runs with their own narrative projections and flight files.

### 32.4 Current Limits

The plan event stream is durable and replayable, and plan attach now subscribes to a single `PlanEventBus` feed abstraction. The feed is broadcast-capable in-process, but production plan writers still primarily communicate through append-only JSONL so cross-process attach remains reliable. A future embedded attach mode could pass a long-lived broadcaster through every plan writer for lower-latency same-process delivery. A broader attach daemon, shared broadcaster across run/plan/chain surfaces, and diagnostic dashboard for slow tick stages are deliberately out of scope for the current release.

---

## 33. Provider Flight Recorder & Rewind

### 33.1 Durable Flight Files

CLI-backed providers (`cli:*` and `cli-*`) are no longer represented only as one opaque `tool.cli_subagent` turn. `deadreckon-core/src/flight.rs` defines the durable flight layer:

- `flight-manifest.json` records one `FlightSession` per provider invocation with provider id, schema, DeadReckon turn, attempt number, status, source paths, and checkpoint policy.
- `flight-events.jsonl` records normalized provider-native rows with ordered `seq`, source path/line/hash, kind (`agent`, `thinking`, `tool`, `result`, `todo`, `tokens`, `session`, `checkpoint`, `warning`, `error`), file references, token usage, and optional checkpoint id.
- `checkpoints/<id>/manifest.json` records delta checkpoints with created/modified/deleted files, base turn snapshot, trigger (`provider_tool`, `file_quiet`, `provider_exit`, or `manual`), and optional full anchors.
- `rewind-events.jsonl` records preview/apply/refusal attempts for audit.

Checkpoints copy full after-bytes for created/modified files into `checkpoints/<id>/files/...` and record deleted files in the manifest. Materialization starts from the base turn snapshot or nearest anchor and replays deltas through the target checkpoint.

### 33.2 Runtime Recorder

`deadreckon-runtime/src/flight.rs` implements `ProviderFlightRecorder`. The run loop starts it after the pre-turn `snapshot_working(turn-1)` and before the CLI provider subprocess. The recorder loads provider descriptor ingest metadata, marks rerun/resume sessions at or after the new turn as `superseded`, assigns `flight-turn-<n>-attempt-<m>`, and spawns a sidecar polling loop while the provider runs.

The sidecar polls descriptor-discovered provider JSON/JSONL files and the working tree. New provider rows are normalized into `flight-events.jsonl`. Tool-like provider events trigger an immediate checkpoint if files changed. Quiet working-tree changes trigger a `file_quiet` checkpoint after the configured quiet window. Provider exit captures a final checkpoint if the tree differs from the latest checkpoint. Non-CLI HTTP/JSON-action providers keep the existing trace/provenance/snapshot behavior and do not create flight files.

### 33.3 User Surfaces

`deadreckon show <run-id> --flight` prints sessions, provider-native events, source log ranges, and checkpoints. `deadreckon show <run-id> --file <path>` filters that view to the events/checkpoints that mention or changed the file. JSON mode emits the same flight payload as structured data.

`deadreckon rewind <run-id> --to-turn <n>|--to-provider-event <seq>|--to-checkpoint <id>` defaults to preview. Preview materializes the target under `<run-root>/rewind-preview/<checkpoint-id>/` and lists the files that would change. `--apply` first refuses superseded checkpoints, then hash-guards changed files against DeadReckon's latest snapshot/current checkpoint expectation before copying only the guarded changed files into the run working directory. Unrelated user edits produce a refusal in `rewind-events.jsonl`.

The run attach TUI activity collector now reads `flight-events.jsonl` and still appends descriptor-ingested provider log lines when available. That makes completed CLI subturns durable and inspectable while preserving provider-log freshness during a long subprocess.

Narrative attach consumes flight evidence but does not rewrite it. Run snapshots cite flight rows such as `flight:<run-prefix>:seq:<n>`, checkpoint ids, file paths, run events, traces, and plan/task ids. Provider-backed summaries can rewrite prose and suggest labels for existing graph nodes, but they cannot add flight events, alter checkpoints, or create graph nodes without deterministic evidence.

### 33.4 Operation Modes And Limits

Worktree, copy, fresh, and in-place modes all work through the same mechanism because the recorder reads `PipelineState.working_dir` and writes under `PipelineState.run_root`. Rewind applies to the run working directory; exported directories and promoted library artifacts are not rewritten. Plan children keep their own flight files under their child run roots, and merged plan result runs do not copy child flight events. Imported runs can still have normalized import traces, but they do not get live filesystem checkpoints retroactively.

The recorder normalizes generic provider rows with schema-keyed heuristics rather than provider-specific semantic ASTs. Exact subturn rewind is only available where a checkpoint exists; provider events without a correlated checkpoint resolve to the nearest previous checkpoint or refuse.

---

## 34. Local Self-Improvement Loop

### 34.1 Experience Index

`deadreckon learn index` scans durable run roots under `DEADRECKON_HOME/runstate/` and writes a local experience index under `DEADRECKON_HOME/learning/`. The index is files, not `PipelineState` fields: `episodes/<scope>/<run-id>.json` summarizes terminal runs, `signals.jsonl` stores deterministic observations, `insights.jsonl` stores provider-backed reflection, `proposals/<proposal-id>.json` stores improvement proposals, `bundles/<bundle-id>.json` stores redacted import/export packets, `candidates/<candidate-id>/candidate.json` stores self-run attempts, `evals/<candidate-id>.json` stores verification results, `pr-events.jsonl` audits PR dry-runs/open attempts, and optional `policy.toml` controls local thresholds.

Episodes are redacted summaries of state, events, traces, spend, acceptance progress, plan/chain context, and flight files. The indexer skips live runs, tolerates corrupt run roots by counting them as skipped, writes unchanged episodes idempotently, and appends deterministic signals such as setup friction, provider gaps, acceptance gaps, slow paths, repeated failures, docs drift, and cost spikes. `deadreckon learn report` renders episode/signal/insight/proposal counts plus top signals, with JSON parity for scripts.

### 34.2 Insights And Proposals

Indexing and signal extraction are deterministic. `deadreckon learn propose` is the required reflection surface: it defaults to the current scope's local indexed evidence, builds a redacted prompt from indexed signals, calls the configured provider route through the existing `ProviderRouter`, and accepts only strict JSON containing both insights and proposals. `--bundle <path>` verifies and imports a redacted, hash-checked learning bundle before using its signals as the evidence source; `--scope` and `--all` only change local evidence selection. Every insight and proposal must cite known `signal_id`/`run_id` pairs, and every proposal must include testable done criteria. Invalid reflection JSON or missing provider routes refuse before any proposal file is written.

`deadreckon learn export <run-id|proposal-id> --redacted` writes a redacted JSON bundle with section hashes for episodes, signals, insights, and proposals. `deadreckon learn import-bundle <path> --preview` verifies schema, redaction, and hashes without writing state; `--yes` imports the bundle into the local learning files. Bundles redact `DEADRECKON_HOME`, project/home paths, and secret-like values, and they do not contain raw provider logs.

### 34.3 Self-Run Candidates

`deadreckon improve self <proposal-id|goal-file> --preview` prints the proposal, isolated-worktree posture, done criteria, provider resolver posture, and PR mode without side effects. `--yes` requires a clean source worktree, refuses a configured `defaults.sandbox = "none"` unless local learning policy allows it, creates `deadreckon/self/<candidate-id>` in a git worktree under `DEADRECKON_HOME/learning/candidates/<candidate-id>/worktree`, writes focused self-run acceptance criteria, then launches a normal `deadreckon run` in that isolated worktree using the existing provider resolver. The coordinator commits candidate changes locally with a deterministic author, records changed files and diff stats, runs focused learning verification, computes an evidence score, performs a simple redaction/secrets scan over the candidate diff, and writes candidate/eval/evidence files.

### 34.4 Evidence-Gated PR Opening

`deadreckon improve self <proposal-id> --pr-dry-run` finds the latest candidate for that proposal, evaluates the same evidence gate used by live open, writes the exact PR title/body to the candidate directory, and appends `pr-events.jsonl` without network or push. `--open-pr` first runs the same evidence gate, then calls a small PR adapter; the production adapter pushes the candidate branch and calls `gh pr create` with the generated body only if the gate is eligible. Tests use a fake adapter and verify it is not called when the gate refuses.

The gate requires explicit opt-in, isolated worktree evidence, non-empty proposal done criteria, accepted self-run, focused verification passing, redaction passing, an evidence score at or above policy, a changed head commit, and no high-risk paths when high-risk blocking is enabled. High-risk paths include acceptance/gate code, sandbox code, provider credential/config handling, release workflows/scripts, and privacy/redaction weakening. PR bodies are fixed-section evidence packets: Summary, Stimulus and Proposal, Evidence Packet, Verification, Risk Classification, Rollback, and Files Changed.

### 34.5 Privacy, Redaction, And Limits

Learning is local-first. No cloud sync, background telemetry, raw provider logs, credentials, or home-directory paths are exported by default. Provider-backed reflection sees redacted episode/signal summaries rather than raw provider-owned logs. Imported-only evidence can produce proposals, but it is not enough by itself to justify live PR opening without local corroborating run evidence. The production implementation does not train or fine-tune models, run multi-candidate evolutionary search, automatically change provider routing defaults, auto-merge into `main`, provide a learning TUI dashboard, or make audit logs cryptographically tamper-proof. Those remain V1 candidates.

---

## 35. Tamper-Evident Gate

### 35.1 The hollow-pass attack

The original gate signature proved that `dr-gate` wrote a marker for the check results it observed. It did not prove the checks were still meaningful. A run could delete a failing Rust test before default `cargo test`, edit the done-criteria file, or turn a shell check into `pytest || true`; the resulting checks would honestly pass and the marker signature would remain valid. The tamper-evident gate closes that release-blocking gap by refusing unambiguous contract subversion and surfacing ambiguous check-covered edits as caveats.

### 35.2 Touched-file set

`tamper::touched_files(run_root, working_dir)` uses two inputs. Modified and created files come from the union of `ProvenanceRecord.files` in `provenance.jsonl`, normalized relative to the run working directory. Deleted files come from the earliest `snapshots/turn-*` inventory diffed against the final working tree, because provenance rows do not reliably record deletions. The `.deadreckon/` subtree is ignored so run-owned docs and helper state do not count as tampering.

### 35.3 Check coverage and classification

`tamper::check_coverage` maps compiled `AcceptanceCheck` values to working-tree-relative paths:

- `file_exists` and `content_match` cover their rendered target path with `target` classification.
- `build_success` covers its rendered cwd as `build`, which is recorded but does not trigger a caveat by itself.
- `cargo_test` covers Rust test files under `tests/`, `*_test.rs`, and `.rs` files containing `#[test]` or `#[cfg(test)]`; first-snapshot test files are included so deleted tests are still covered.
- `shell` checks are linted for suppression patterns and best-effort scanned for existing working-tree path tokens; matching Rust test files classify as `test`, other paths as `unknown`.

### 35.4 Verdict policy

The verdict is intentionally heuristic:

- `refuse`: the run modified `acceptance.yaml`, a compiled check contains a suppression pattern such as `|| true`, `--no-verify`, or `--exit-zero`, or a covered test/target file was deleted. Refusal writes no marker and reuses the existing non-terminal gate-failure loop.
- `caveat`: a covered test or literal check target was modified or created. The marker is signed, but the caveat is visible in status and exit summaries.
- `clean`: no spec edits, no suppression lint findings, and no caveat-worthy covered-file touches. Production code edits exercised by tests stay clean; editing production code is the point of a run.

### 35.5 `acceptance-tamper.json` and signature binding

The proof file lives at `<run-root>/proofs/acceptance-tamper.json`:

```json
{
  "schema_version": 1,
  "run_id": "...",
  "verdict": "clean",
  "spec_modified": false,
  "lint_findings": [],
  "covered_files_touched": [],
  "caveats": [],
  "refusal_reasons": []
}
```

The `AcceptanceMarker` schema stays at version 1 and receives no new fields. `gate.rs::marker_signature` hashes the tamper file bytes alongside the nonce, marker fields, and check results. Missing tamper files hash as empty bytes for backward tolerance. A forged or edited tamper file invalidates marker validation, so an agent cannot erase a caveat after signing.

### 35.6 Surfacing

`status`, exit cards, `show --why-failed`, and attach activity derive a per-check gate line from marker/progress rows, for example `gate: PASSED 4/4` or `gate: FAILED 0/1 - cargo_test x auth::tests::expired_token`. Tamper proof data adds `tests modified this run: yes/no` and caveat text such as `accepted (caveat: agent modified test file tests/auth_test.rs this run)`. Exit cards render caveat gates with `Warn` tone.

### 35.7 Honest subscription spend

The same result surfaces no longer render subscription-only CLI routes as `~$0.000000`. Subscription-only spend reads `not metered (subscription) · wall <s>s · <n> turns`. Mixed routes show the metered total and append `+ subscription turns`.

### 35.8 Limits

This is tamper-evidence, not a causal soundness proof. It does not prove that a covered-file edit caused a pass, does not understand every language's test conventions, and does not sandbox the checks' own writes. Those larger designs are tracked in `docs/V1-CANDIDATES.md`.

---

## 36. Campaign Orchestration (one task, N orchestrators)

A **campaign** runs one large goal as N independent, separately-coordinated
workstreams and composes their results into a single promoted run. The launch
verb is the top-level `deadreckon campaign <goal> --n <2..=6>`, with
`deadreckon campaign repair <campaign-id>` as the manual recovery path for a
failed meta-merge. Campaign is a peer of `run`, `orchestrate`, and `chain` (not a
subcommand of `orchestrate`). Campaign logic
lives in `crates/deadreckon-core/src/campaign.rs`; the command handler and spawn
glue are in `crates/deadreckon/src/commands/campaign.rs`.

### 36.1 Mental model: fork→merge lifted one level

`orchestrate` splits a goal into parallel *runs* under one coordinator and merges
them. A campaign splits a goal into parallel *orchestrations*: each sub-goal is
launched as a full `orchestrate full-plan` subprocess (a depth-1
sub-orchestrator), and because a plan's merge output is itself a normal promoted
run (§30.4), the meta-merge composes those result runs with the same primitives
as plan merge. Nothing in spawn/isolation/merge is reinvented — only the meta
layer and its guardrails.

### 36.2 Files (no new struct fields)

A campaign adds no fields to `Plan`, `PlanTask`, or `PipelineState`. Its state is
file-backed under `~/.deadreckon/plans/<campaign-id>/`: `campaign.json` (the
`Campaign` + `SubGoal` model), `lineage.json` (nesting depth and ancestors),
`campaign-rollup.json` (the gate-verdict roll-up), `campaign-events.jsonl` (the
timeline), `launch/<sub-id>/sub-result.json` (each sub's reported result), and
`merge-working/` (the composed tree). Meta-merge conflicts and repair sidecars
live in `merge-proofs/`. The promoted result run carries a
`deadreckon-campaign-manifest.json`.

### 36.3 Depth cap and cycle guard (`CAMPAIGN_MAX_DEPTH = 2`)

A campaign sits at depth 0; its sub-orchestrators at depth 1. A depth-1 process
invoking `campaign` is refused (`campaign::guard`) — sub-orchestrators run plain
`orchestrate full-plan`, never another campaign. The cap is a hard constant. The
guard also refuses a sub-goal whose `task_key`/scope matches an ancestor (cycle).

### 36.4 Lineage env transport across the spawn boundary

A freshly spawned sub-orchestrator learns its depth before it has a plan dir via
`DEADRECKON_CAMPAIGN_DEPTH`/`_ROOT`/`_ANCESTOR_TASK_KEYS`/`_ANCESTOR_SCOPES`
(`campaign::parse_lineage`/`lineage_from_env`); the durable record is
`lineage.json`.

### 36.5 Tree budget allocation and aggregate enforcement

`--max-spend` is a *tree* ceiling. `campaign::allocate_budget` splits it evenly
(remainder-to-first) into per-sub `--max-spend` shares. The fork driver
(`run_campaign_fork`) is sequential precisely so it can sum leaf spend and refuse
the next launch once the aggregate reaches the ceiling
(`tree_budget_exhausted` → `budget_exhausted` event). A null budget is logged, not
treated as unbounded.

### 36.6 Sub-orchestrator spawn and result-run discovery

`build_sub_orchestrator_command` reuses the plan-child isolation idiom
(`DEADRECKON_SCOPE_ROOT` per launch dir) and runs `orchestrate full-plan … --yes`.
On completion the sub writes `sub-result.json` (plan id + merged run id), which the
coordinator reads (`discover_sub_result`).

### 36.7 Meta-merge via semantic repair

`mergeable_run_files` is the per-run file enumeration shared with plan merge
(behavior unchanged). Campaign roll-up now maps completed sub-results into a
synthetic repairable plan keyed by `sub-*`, composes those result trees through
the same DAG-aware merge path used by normal plan merges, and invokes the
semantic merge repair provider for true cross-sub same-path conflicts. Repair
sidecars are written under the campaign's `merge-proofs/`, and campaign events
record conflict, repair-planned, repair-started, repaired, or repair-failed
milestones before promotion. `deadreckon campaign repair <campaign-id>` reloads a
failed campaign with completed sub-results and a completed roll-up, then reruns
this same synthetic-plan merge/repair/promote path; it accepts `--repair-provider`,
`--repair-mode`, and `--repair-attempts`.

### 36.8 Gate-verdict roll-up and the no-laundering invariant

`build_rollup` reads each leaf run's tamper verdict (§35) and marker, computing a
worst-of `RollupVerdict`. A campaign reaches clean completion only when every sub
merged *and* no leaf was refused (`campaign_can_complete`); a refused leaf fails
the campaign and blocks promotion. The roll-up is written into the result run's
root and hashed into its acceptance-marker signature (§35 binding extended in
`gate::marker_signature`), so editing `campaign-rollup.json` after signing
invalidates the marker — nesting cannot launder a refused leaf into a clean pass.

### 36.9 attach / show --why-failed / repair / kill for campaigns

`resolve_campaign` matches a campaign id prefix and accepts `latest`/`last` for
the most recently created campaign. `attach <campaign-id>` is a first-class live
TTY surface: it opens a campaign ratatui view with the root goal, campaign status,
roll-up, aggregate spend, tree budget, selectable sub-plan cards, and a feed built
from `campaign-events.jsonl` plus each discovered sub-plan's `plan-events.jsonl`.
`Enter` drills into the selected sub-plan by suspending the campaign TUI and
calling the existing plan attach loop; the plan loop can then drill into a child
run with the existing run attach loop. Breadcrumbs include the campaign tier
(`campaign <id> / sub-* / plan <id> / task-*`), and `b`/Backspace returns through
the nested contexts. Off-TTY or `--plain` still prints the read-only sub rows +
roll-up summary with the explicit `deadreckon attach <sub-plan-id>` hint. `--json`
emits a structured campaign attach object with id, status, goal, tree budget,
aggregate spend, roll-up, and sub-plan rows.
`show <campaign-id> --why-failed` reports refused/caveat subs.
`campaign repair <campaign-id>` is state-changing and only accepts failed
campaigns; successful repair writes a new promoted campaign result run and marks
the campaign merged.
`kill <campaign-id>` cascades into each sub-plan via the existing plan-kill path,
then marks the campaign killed.

### 36.10 Current limits

Depth is capped at 2; sub-goals are independent (no cross-sub dependency edges);
campaign attach is navigated drill-in, not a flattened recursive event tree with
every campaign, plan, and run event in one pane. These are tracked in
`docs/V1-CANDIDATES.md`.

---

## 37. Effortless: the friendliness contract

Effortless is the production-release friendliness layer over the existing engine.
It makes the friendly path the default for first use, return-after-walking-away,
and vocabulary while keeping the core mechanisms unchanged: `dr-gate`, sandboxing,
promotion, providers, plan merge, and the campaign engine stay as built. The pass
adds no durable fields to `PipelineState`, `Plan`, `Campaign`, or provider schemas;
its durable outputs are existing files, new sidecar records where already
appropriate, and documentation.

### 37.1 The six-clause contract and the audit (`FRIENDLINESS-AUDIT.md`)

`docs/FRIENDLINESS-AUDIT.md` is the curated surface audit. It scores canonical
verbs against six clauses: auto-detect don't ask, preview before any state change,
refuse with a specific `try:` line, one-command rollback, one verdict plus one
primary action, and lifecycle hints. `crates/deadreckon/src/friendliness_contract.rs`
keeps the table close to code, and `cards_friendliness`/`coherence` tests make the
contract executable instead of a prose-only promise.

### 37.2 `deadreckon try` and the proof block

`deadreckon try` is the keyless smoke path. It uses the normal run creation,
runtime loop, smoke provider, promotion, and acceptance gate, then prints a proof
block: `gate: SIGNED by dr-gate`, the run narrative path, one provenance/lineage
line, and one next command. This path demonstrates the harness in roughly the
first ten seconds without asking for provider credentials.

### 37.3 Self-bootstrapping `start`

`deadreckon start` now resolves provider setup through the shared setup layer. If
exactly one subscription CLI is detected, the launch uses it ephemerally and tells
the operator how to make it permanent; no `deadreckon init` detour is required.
If none are usable, the refusal points first to `deadreckon try` and then to a
concrete provider config command. Preview, JSON, plain, quiet, and non-TTY flows
remain deterministic and state-free until launch confirmation.

### 37.4 One verdict + one primary action

Terminal completion, failure, blocked, paused, killed, preview, and no-op
surfaces now converge through the Verdict Surface contract in
`crates/deadreckon/src/verdict_surface.rs`. The invariant is one verdict label,
one `Recommended` command, and one `Explanation`/`Evidence` panel, with
secondary actions rendered below the primary action and suppressed by
`--no-hints` where appropriate. JSON outputs keep existing fields and add
`verdict` plus `primary_action` when the command reports a terminal outcome.

The rollout covers run exit cards, `status`, `finish`, `apply`, `materialize`,
plan/orchestrate/fork/merge, campaign, chain, recovery verbs, setup/diagnostic
commands, import/learning/doc surfaces, and TUI post-action footers. Focused
coverage includes `cargo test -p deadreckon verdict_surface`, the audit
burndown test `friendliness_one_verdict_primary_action_burndown_has_no_in_scope_failures`,
and command-family tests for run, plan/orchestrate, campaign, chain, recovery,
setup, diagnostics, import, learning, and doc paths. Larger layout work remains
V1: a full output-layout facade, card template engine, localization, and broad
command-matrix golden snapshots are still tracked in `docs/V1-CANDIDATES.md`.

### 37.5 Spend + gate-verdict consistency

The consistency sweep made honest spend and per-check gate status visible on the
surfaces people inspect after a run: exit card, `status`, `finish`, plan child
detail, and campaign child summaries. Subscription-only routes render as not
metered with wall time/turns; mixed HTTP + subscription routes render the metered
total plus a subscription note. Gate text comes from the tamper-evident marker or
acceptance progress rather than a loose success word.

### 37.6 Opt-in notifications (`[notify]`, channels, `notify.jsonl`)

Notifications are config-driven and opt-in. `[notify]` can enable native desktop,
command, or webhook channels for accepted, paused-at-cap, and failed transitions.
Notification context is intentionally small and redacted: transition, run id,
verdict, spend, and narrative path. Each attempt appends to run-local
`notify.jsonl`, and command-channel failures include a `try:` recovery detail.
There is no daemon or background service.

### 37.7 Provider-backed goal-shape routing

`start` performs one bounded read-only classifier call through the existing
provider router when a provider is available. The classifier may recommend one
verified run, an orchestration, or a campaign with count and rationale. Its result
is preview-scoped, validated, clamped, and advisory; deterministic fallback handles
no-provider and smoke-provider paths. Campaign-shaped recommendations are shown
as suggestions rather than silently launching a campaign.

### 37.8 Campaign friendliness

`deadreckon campaign` can omit `--n`; DeadReckon recommends a count from the same
goal-shape machinery and still lets the operator inspect before launch. The
preflight supports launch, edit a sub-goal, drop a sub-goal, change count, and
cancel with a concrete preview retry command. This changes presentation and
preflight ergonomics only; the depth cap, sub-orchestrator spawn, roll-up, and
meta-merge rules remain §36's campaign engine.

### 37.9 Vocabulary ("verified run") + error-footer coverage

`deadreckon-core::glossary` owns the guarantee noun: a completed accepted result
is a "verified run", verified by `dr-gate` ("the process that verifies the run").
The `def-done` command and `.deadreckon/acceptance.yaml` file are presented under
the umbrella "done contract" on primary user-facing surfaces. Completed cards
use the verdict word `VERIFIED`. Error footer coverage now includes a
parameterized refusal table that checks the final stderr line carries `try:`.

### 37.10 Limits

Effortless intentionally avoids durable schema churn and background effects. There
is one bounded classifier call, not an LLM control loop. Notifications are opt-in
and run only at lifecycle transitions, not through a daemon. Palettes,
localization, a card template engine, richer guided onboarding, a long-lived
notifier, and deeper multi-piece classification are V1 candidates.

---

## 38. Binary Module Layout (post-decompose)

The Decompose pass made the binary crate navigable without changing its public or
CLI contract. The public-surface baseline still covers only `deadreckon-core`,
`deadreckon-providers`, `deadreckon-runtime`, and `deadreckon-sandbox`; the binary
crate remains private implementation glue. The split moved the highest-churn
command families and pure terminal render layer out of the monolithic `main.rs`
while keeping `cli.rs`, the `Command` enum, every verb, every flag, every output
string, and every exit path behavior-compatible.

### 38.1 Why `main.rs` was split

Before this pass, `crates/deadreckon/src/main.rs` carried roughly 40.6k lines:
command handlers, cross-command helpers, and seven inline `#[cfg(test)]` modules
in one namespace. That shape made ordinary command changes hard to review because
tests, helpers, and unrelated command bodies interleaved in one file. The safety
argument was narrow: moving binary-private code cannot change the library public
surface, but it can change observable CLI behavior, so output characterization had
to land before moving code.

After the pass, the binary source is split across private modules. `main.rs` still
owns the Tokio entrypoint, tracing setup, the `main_inner` command match, shared
root-level config, try, status/show, and control helpers, and private glue used by
multiple command families. It is no longer the single home for the core command
families, guided start flow, provider detection/update handling, descriptor-backed
import handling, learning/self-improvement command handling, TUI render layer, or
lifted unit-test modules.

### 38.2 Characterization net

`crates/deadreckon/tests/characterization.rs` runs the built binary and compares
normalized stdout/stderr against goldens under
`crates/deadreckon/tests/goldens/characterization/`. It pins representative
user-facing output for:

- `plan --draft`
- `plan --quiet`
- `orchestrate ... --preview --json`
- `chain ... status`
- off-TTY attach narrative output
- canonical `try:` refusal footers

Those goldens stayed green through the command and render moves, the shared merge
helper extraction, command-existence dedupe, and P10 cleanup nits. They are the
explicit guard that the decompose work did not change CLI output shape.

### 38.3 Command modules and `main_inner`

`crates/deadreckon/src/commands/mod.rs` exposes only crate-private modules:

- `commands/chain/` owns the chain command family, conductor entrypoints, chain
  attach loop, and chain lifecycle verbs.
- `commands/acceptance.rs` owns `acceptance` and `def-done` command handling.
- `commands/run.rs` owns the supervised `run` command body.
- `commands/init.rs` owns `init` setup wiring.
- `commands/campaign.rs` owns campaign creation, preflight, fork/roll-up helpers,
  meta-merge repair, campaign attach state/feed helpers, attach summaries, and
  campaign failure reports.
- `commands/attach.rs` owns attach command dispatch and terminal event loops.
- `commands/attach_runtime.rs` owns attach-loop tick timing and asynchronous
  narrative-refresh job/request plumbing shared by run, plan, chain, and
  campaign attach.
- `commands/merge.rs` owns merge entrypoint and CLI repair-strategy parsing.
- `commands/orchestrate.rs` owns the orchestrate front door and mode/provider
  selection helpers.
- `commands/plan.rs` owns plan creation, fork, child-launch orchestration, and
  plan/fork output helpers.
- `commands/import.rs` owns descriptor-backed transcript import, concrete session
  selection, import manifests, and normalized imported trace/provenance rows.
- `commands/learning.rs` owns `learn` and `improve` command handling, including
  local evidence indexing, provider-backed reflection, self-run launch, and the
  evidence-gated self-improvement PR adapter seam.
- `commands/start.rs` owns the guided `start` flow: goal-shape classification,
  interactive launch-path/source/provider/done-criteria prompts, start preview
  materialization, history-based follow-up suggestions, and dispatch to run,
  campaign, review, or full-plan execution.
- `commands/providers.rs` owns setup-provider inspection and update surfaces:
  `detect`, `providers list`, install-receipt discovery, update cache refresh,
  shell-channel swap/backup handling, and provider registry table rendering.
- `commands/completion.rs` owns shell completion generation/installation and
  the init-time best-effort completion installer.
- `commands/doctor.rs` owns the `doctor` inspection surface, provider/sandbox
  diagnostics, and subscription-CLI version probes.
- `commands/inspection.rs` owns inventory and durable-history surfaces:
  `list`, `history grep`, and `library` list/search/show, plus crate-private
  plan/library list seams used by start/status.
- `commands/lifecycle.rs` owns run/plan lifecycle materialization and follow-up
  surfaces: `finish`, `export`/`materialize`, `apply`, `abandon`, `cleanup`,
  `extend`, parent markers, and lifecycle notification firing.
- `commands/doc.rs` owns `deadreckon doc` dispatch for run and plan docs,
  including doc-polish confirmation previews and spend estimation seams.

`main_inner` remains the dispatcher. It parses the unchanged `cli.rs` command enum,
sets plain-output policy where needed, and delegates to the command family module
or to the root helper that still owns the remaining status/resume/control surface.
The binary does not expose a public `run(cli)` facade and does not add a new
library crate.

### 38.4 TUI render layer

`crates/deadreckon/src/tui/` is private and presentation-oriented:

- `tui/attach_state.rs` owns attach TUI state, key reducers, panel focus,
  campaign parent breadcrumbs, and selection state.
- `tui/render.rs` owns pure state-to-frame rendering for run attach, plan attach,
  chain attach, campaign attach, Markdown docs, narrative panels, live files,
  process rows, and activity feeds.
- `tui/mod.rs` is the crate-private facade consumed by command loops and tests.

The event loops stay in command modules. Render functions do not call providers or
write state; provider-backed narrative refresh remains asynchronous and outside the
render path. P6 added pure-render snapshots for run and chain attach frames after
the seam existed.

### 38.5 Visibility discipline

The split widened only binary-internal access to `pub(crate)` where moved modules
or sibling `src` test modules needed it. It did not make binary helpers `pub`, did
not re-export binary internals from a library crate, and did not alter the
recorded library surface. Move commits were kept mechanical: relocation plus the
minimum visibility widening, with logic edits isolated in later cleanup commits.

### 38.6 The single public-surface rebaseline

The only deliberate public-surface change in the whole Decompose pass was P9:
`deadreckon_core::error::is_retryable_io_kind` became a shared public core helper
so providers and sandbox could delete verbatim local copies. The public-surface
baseline gained exactly that one path, and the P9 commit documents why the
rebaseline is behavior-identical and isolated.

### 38.7 Duplicated logic unified

Two duplicated implementation families were unified after the move phases:

- The merge composition loop now backs campaign result composition, plan merge
  working-tree composition, and full-plan dependency source assembly while
  preserving conflict/precedence semantics.
- Command existence lookup now flows through one private helper that preserves
  explicit-path handling and bare PATH search behavior.

P10 also pruned unused `tracing`/`chrono` declarations from the targeted library
crates, deleted confirmed dead helpers, hardened docs regex initialization with
BUG-tagged `expect` calls plus compile coverage, and applied small allocation nits
without moving characterization goldens.

### 38.8 Deliberately not done

The pass rejected broader churn that would not make the requested decomposition
more true or would create public-surface/behavior risk: core `pub mod` tightening,
Chain/Plan field encapsulation, a uniform `CommandHandler` trait, a public
binary-run facade or new `deadreckon-cli` crate, `#[source]`/sysexits behavior
changes, splitting `cli.rs`'s command enum, and integration-test submodule churn.
Those are recorded in `docs/V1-CANDIDATES.md` as explicit "not now" pointers.

---

## 39. Composable Seams (swap a worker, keep the gate)

Composable seams decompose four governance concerns without making the trust root
pluggable. The implementation lives primarily in
`crates/deadreckon-runtime/src/seam.rs`, `turn_loop.rs`, and the provider router
catalog override path.

### 39.1 The monolith critique and deadreckon's answer

Before this pass, changing policy decisions, model-catalog metadata, hook
side-effects, or event export meant changing Rust. The production answer is a
single subprocess seam contract for exactly those four workers. This makes the
harness thinner or thicker by config, not by fork.

The decomposition is intentionally not universal. `dr-gate` remains the fixed
acceptance root because a swappable trust root can self-attest. Seam workers can
observe or narrow behavior, but they cannot sign completion.

### 39.2 The one primitive: `SeamCommand`

`SeamCommand` is sandboxed JSON-over-stdio:

1. Resolve `[seams.<kind>]` from `config.toml`; absent means built-in behavior.
2. Spawn the configured argv through the sandbox layer with a timeout.
3. Send one JSON request on stdin and parse one JSON response from stdout.
4. Apply the fixed per-kind fail policy.

The allowed kinds are `policy`, `catalog`, `hooks`, and `event_sink`.
`[seams.gate]` and unknown kinds are hard config errors. Every run writes
`<run-root>/seams.json` with source, command basename, timeout, fail policy, and
`no_seams` resolution.

### 39.3 The four seams

- **policy:** receives `{function_id, command, working_dir}` for bash/write-file
  calls after the built-in sandbox floor allows the action. It returns
  `allow|deny` and is fail-closed.
- **model-catalog:** returns model context-window/pricing metadata. Router
  lookups prefer seam-provided entries and fall open to the built-in catalog.
- **hook-fanout:** observes `ToolCallStarted` and `ToolCallResult` events. It is
  fail-safe and cannot change decisions.
- **event-sink:** mirrors `RunEvent` values from a broadcast subscriber while
  `events.jsonl` remains the source of truth for attach and recovery.

### 39.4 The thin-thick slider

No `[seams]` entry means the built-in implementation runs. Adding one command
swaps exactly that worker. `deadreckon run ... --no-seams` and
`deadreckon start ... --no-seams` force every kind back to built-in behavior for
that launch. Run previews and `doctor` report which seams are external.

### 39.5 The non-swappable gate

`SeamKind` has no gate variant, config rejects `[seams.gate]`, and seam
subprocesses deny `<run-root>/gate/` and `<run-root>/proofs/`. Adversarial tests
cover marker/proof writes, `gate/nonce` reads, and signature validation with
seam sidecars present. Seam files do not alter `marker_signature` inputs.

### 39.6 Context-window compaction on the direct-API path

`crates/deadreckon-runtime/src/compaction.rs` bounds direct HTTP/API provider
history with deterministic elision. The threshold is
`fraction * context_window`; `context_window` comes from the router catalog
including catalog seams, or `fallback_context_window` when unknown. The goal and
done spec remain in the prompt; recent turns stay verbatim; the middle is
replaced by one deterministic marker. CLI providers are never compacted.

### 39.7 Per-run audit

Two files record the seam decisions without changing durable state schemas:

- `seams.json` records per-kind source/fail policy and whether `--no-seams`
  forced built-ins.
- `compaction.jsonl` appends one row per direct-API compaction with token
  estimates, kept/elided turn counts, context window, and source
  (`catalog|seam|fallback`).

`PipelineState`, `Plan`, `AcceptanceMarker`, check results, and provider entries
were not extended for this work.

### 39.8 Sandboxing and fail policies

Seam subprocesses inherit the same sandbox machinery used for provider/tool
subprocesses, with extra gate/proof deny paths. Fail policies are code-fixed:
policy fails closed, catalog fails open, and hooks/event-sink fail safe. The
policy seam can only narrow after `sandbox.toml`; it cannot widen filesystem or
network permissions.

### 39.9 Conformance kit

`docs/SEAMS.md` and `examples/seams/` make the seam protocol executable. The
examples include fixture JSON plus POSIX shell workers for policy allow/deny,
minimal catalog override, hooks JSONL, and event-sink JSONL. `deadreckon seams
validate <kind> --config <path> [--fixture <path>] [--json]` reads the same
`[seams]` config as runtime, dispatches through the same sandboxed primitive,
and reports the fixed fail policy in plain or JSON output.

Validation is diagnostic, not a new durable contract: it does not add registry,
version-negotiation, or schema state, and it still cannot make the gate
swappable.

### 39.10 Limits

This release does not add human approval, a persistent bus, a worker registry,
capability negotiation, or LLM-backed semantic compaction. Built-in telemetry is
not routed through hooks; hooks are additive observers. Those extensions are V1
candidates.

## 40. Uniform Surface (one Tone, one width, shared kv/table, hardened prompts)

The non-TUI presentation layer renders through one styling vocabulary and a small
set of shared primitives instead of per-call-site formatting. The attach TUI is a
separate slice and is out of scope here.

40.1 **One Tone, one width.** `ui::Tone` is the single tone enum, with one
`to_ansi()` table for line output and a derived `to_tui_color()` table; the TUI
status palette is derived from it so a status renders the same color on a line
and in a frame. `ui_card` consumes `ui::Tone` (its separate enum is gone). A free
status string resolves through an explicit `Status` classifier whose `Unknown`
class renders a visible default rather than a silent dim. `ui::display_width`
(strip ANSI, then Unicode display width) is the one width function behind every
pad/truncate/column site; `ui::pad_visible` pads by display columns so a colored
cell aligns like a plain one (no `{:<N}` over ANSI — a guard test enforces it).

40.2 **Shared kv/table primitives.** `kv_block_string` renders auto-aligned
`key: value` blocks (used by the status report's run-health/library/disk
sections); `columns` renders lowercase-header tables with display-width padding
(used by the library table; the run list shares the lowercase-header convention).
`ui::wrap_words` is the single word-wrap engine behind the kv, list, and campaign
wrappers.

40.3 **Hint discipline.** Completion surfaces route through
`completion_hints_enabled` so `--no-hints` / `DEADRECKON_HINTS=0` are honored
uniformly (the campaign completion surface previously ignored them).

40.4 **Hardened one-shot prompts.** `prompt::menu_step` is a pure, unit-tested key
dispatcher: multi-digit number entry, always-available Esc cancel, out-of-range
feedback, and a tall-list fallback to line mode. `prompt::ask_number(range)`
re-prompts on bad count input instead of aborting the command. `deadreckon start`
with no goal prompts interactively on a TTY (and prints a notice when prompts are
suppressed). Chain step glyphs gain an ASCII fallback under `--plain`/non-VT
terminals; cancel paths render a verdict surface with a Recommended next step.

40.5 **Not adopted (by decision).** No `console`/`dialoguer`/`inquire`/
`comfy-table`/`indicatif` — they introduce a second terminal stack or theme that
would break the byte-exact render tests. Only `unicode-width` (already transitive
via ratatui) is added directly.

---

*This document is canonical for the production-release reality of deadreckon. Future hardening passes (per the robustness rider) and feature passes (per the usability rider) will update sections 6, 9, 11, 13, 14, 18, 22, 31, 32, 37, and 38 in particular. Updated 2026-05-31 for Navigable campaign attach, the Decompose binary-module layout, Effortless friendliness, tamper-evident gate behavior, release posture, and plan-result docs; the last broad source audit remains the 2026-05-26 agent-team pass. Line numbers are best-effort locators — small, stable files (`state.rs`, `lock.rs`, `gate.rs`, `http.rs`, `commands.rs`, `process.rs`) are kept current, while `main.rs` (~11.9k lines after decomposition) and `turn_loop.rs`/`cli.rs` cite approximate positions or symbol names; always cross-check against the code before relying on a specific line.*
