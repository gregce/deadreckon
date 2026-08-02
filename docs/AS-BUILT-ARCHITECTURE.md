# AS-BUILT-ARCHITECTURE.md

**Subject:** deadreckon — a long-running, BYOK, sandboxed agentic CLI harness in Rust
**Frame:** Reference specification for the **production-release** as-built reality at `/Users/gdc/deadreckon/`. Modeled on `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` (the Printing Press).
**Last updated:** 2026-08-02 (unified execution-team selection, strict durable
admission, cumulative Job wall caps, stall prevention and current Soundings
launch/contract limits)
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
46. [Course: Launch Planning and Reshaping](#46-course-launch-planning-and-reshaping)

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
- **The agent cannot mark its own gate.** For strict durable Jobs, the trusted
  controller first materializes the approved contract. Keyless `dr-gate
  evaluate` runs inside the resolved sandbox. Only after that process group is
  gone does childless `dr-gate sign` receive the external HMAC key, revalidate
  the evidence and write the marker. Legacy-v1 compatibility runs retain their
  historical nonce validator.
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

**`deadreckon` (binary crate, `crates/deadreckon/src/`).** Clap parser
definitions (`cli.rs`), a root entrypoint/dispatcher and shared helpers
(`main.rs`), private command-family modules (`commands/`), private attach
render/state modules (`tui/`), and `dr-gate` as the two-command keyless
evaluator and trusted signer (`bin/dr-gate.rs`). Supporting modules:
`narrative.rs` (deterministic + provider-backed narrative projection),
`plan_event_bus.rs` (`PlanEventBus`/`PlanEventFeed`), `tui_events.rs`
(`TuiEventFeed`), `ui.rs` + `ui_card.rs` + `cards/` (CLI/TUI rendering
vocabulary and cards), `setup.rs` (provider/done-contract resolution),
`prompt.rs` (confirmation prompts), and `sleep.rs` (sleep-prevention).

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
| 50 | `verify` | post-loop verification; strict Jobs run contained keyless evaluation, then trusted HMAC signing |
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
│               │   └── acceptance-progress.jsonl  # reconstructed after strict evaluation; legacy may stream
│               ├── gate/
│               │   └── nonce      # legacy-v1 compatibility secret; strict Jobs use an external HMAC key
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

The following sequence describes the historical process-owned `run` path.
Strict durable Jobs use the Job supervisor and the two-phase gate in §58.5.

```
main.rs run_command()
  ↓
  paths = DeadreckonPaths::discover()
  ↓
  state = create_run(paths, RunOptions{...})       # state.rs:178-231
  │   ├── mint run_id (uuid simple form)
  │   ├── derive scope (paths.rs:67) and task_key (paths.rs:88)
  │   ├── create run_root/working/snapshots/proofs/gate/turns
  │   ├── write gate/nonce (legacy-v1 compatibility)
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
    save_state()                    # provider completion is a durable boundary
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

`ProviderRequest` carries `prompt`, `max_output_tokens`, optional `cwd`,
optional `output_path`, optional `sandbox_backend`, `workspace_access`, optional
`pid_file`, optional `cancellation_token`, optional `session_dir`, optional
`output_schema`, and optional request-scoped `capability_posture`. The last 4
fields support CLI conversation state, constrained output, cancellation and
Codex app-server approval answers without adding a second durable policy.

`ProviderResponse` carries `provider`, `model`, `content`, `usage`, `spend`, and
the JSON `trace` value.

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

**`cli:claude-code` (`crates/deadreckon-providers/src/cli_claude_code.rs`).** A
normal read-write worker invocation is:

```zsh
claude [--model <model>] --dangerously-skip-permissions -p "<prompt>"
```

Read-write workers use `--dangerously-skip-permissions` because no human is in
the loop. Read-only requests remove that flag and require an enforceable outer
sandbox. Structured-capable binaries add stream JSON and may resume the
run-scoped conversation. DeadReckon's outer Seatbelt, bubblewrap or Docker
profile still defines the filesystem boundary. Stdout is captured to
`request.output_path`.

**`cli:codex` (`crates/deadreckon-providers/src/cli_codex.rs`).** A fresh
invocation starts as:

```zsh
codex --ask-for-approval never exec [--model <model>] --skip-git-repo-check --sandbox <mode> [--json] [-o <last-message>] [--output-schema <schema>] -- "<prompt>"
```

`<mode>` is `read-only` for a read-only request. It is `workspace-write` when a
read-write request has no outer sandbox, and `danger-full-access` when an outer
sandbox provides the boundary. Resume uses `exec resume <id>` and inherits the
session's original sandbox policy.

The trailing `--` delimiter is non-negotiable: doc-polish prompts often begin with YAML frontmatter (`---`), which `clap`-based Codex CLIs otherwise interpret as an option-like argument. Adding `--` forces the prompt to be parsed as the positional value.

**Descriptor-backed CLI providers.** The provider registry now owns compiled-in TOML descriptors plus `providers.d` overrides. Generic CLI descriptors (`ProviderKind::Generic(id)` where the descriptor kind is `cli`) are launched by `GenericCliProvider`, which renders `exec_template.args_template` with `{prompt}`, `{sandbox}`, and `{cwd}` placeholders and applies the descriptor `model_arg` near the prompt without splitting prompt-value flags like `-p <prompt>`. `cli:gemini`, `cli:opencode`, `cli:copilot`, and `cli:pi` are built-in generic CLIs; `cli:claude-code` and `cli:codex` remain concrete adapters for compatibility with their established launch quirks. Copilot launches as `copilot -p <prompt> --output-format json --stream off --no-color --allow-all`; Pi launches as `pi --mode json --print <prompt>` so its default saved sessions remain available to the TUI.

**Shared subprocess machinery (`cli_common.rs`).** Builds a `SandboxSpec` with
explicit allowlists:

- Write allowlist: descriptor `sandbox_writes` for registered CLIs, with concrete compatibility fallbacks for codex and claude.
- Read allowlist: binary location + `~/.bun`, `~/.local`, `~/.npm-global`, `~/.opencode`.
- `allow_network: true` (CLI agents need outbound for their own API calls).

The common runner observes `cancellation_token` when one is present. Without a
token it waits for the child process to exit. It does not add a generic wall
timeout. Callers that promise a time bound must supply and enforce one.

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

`commands.rs` constructs `bwrap` args with:

- `--die-with-parent --unshare-pid --unshare-ipc --unshare-uts`
- a private `--tmpfs /tmp` before any workspace or provider path below `/tmp`
  is rebuilt, so the temporary mount cannot hide earlier bindings
- read-only mounts for existing standard Linux dynamic-loader roots (`/lib`,
  `/lib32`, `/lib64`, and `/libx32`), without which an approved dynamic
  executable appears missing even when its own file is mounted
- read-only mounts for existing directories on the effective `PATH` and for
  explicitly named runtime roots such as `CARGO_HOME`, `RUSTUP_HOME`,
  `NVM_DIR`, and `JAVA_HOME`; tool shells use `sh -c` so `/etc/profile` cannot
  discard those approved routes
- explicit destination parents for absolute allowlisted paths in bubblewrap's
  initially empty mount namespace
- `--ro-bind <path> <path>` for each existing entry in
  `system_read_allowlist(cwd, spec.read_allowlist)`, excluding host `/tmp`
- `--bind-try <path> <path>` for optional CLI state roots and other write
  allowlist entries, followed by the authoritative workspace mount
- an ephemeral `$HOME` applied after the workspace mount so the broader mount
  cannot hide it
- `--bind` or `--ro-bind` for `<cwd>` according to workspace access, then
  `--proc /proc`, `--dev /dev`, and `--chdir <cwd>`
- `--unshare-net` unless `allow_network=true`

### 11.6 Docker

Docker remains opt-in. Ordinary sandbox calls use the configured image and
mount policy. A strict Job gate uses a typed Docker execution identity instead
of trusting a mutable image name: the policy binds an immutable image ID,
Linux platform, static evaluator digest and fixed guest path. The controller
mounts the working tree and approved gate inputs with explicit read/write
roles, masks protected Job, proof, key, operator-capture and Git-control paths,
scrubs signing inputs, and applies the requested network policy.

Before launch, the controller durably records the Job ID, attempt, launch ID,
container name, expected labels, image ID, platform and cidfile. Reconciliation
inspects those labels before removing anything. Normal completion,
cancellation, abandoned-lease recovery and retry all prove the container is
absent before deleting the record. A missing or mismatched identity fails
closed as lost containment; it is never treated as reassuring cleanup.

Release archives carry both static Linux evaluator sidecars beside
`deadreckon`: arm64 and x86-64 musl builds. This lets a macOS or Windows
controller launch the platform-compatible evaluator inside a Linux container
without attempting to execute its host-native `dr-gate`.

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

The agent cannot be trusted to declare a run done. A strict durable Job can
only reach verified completion through a contained, deterministic evaluation
and a separate HMAC signing phase. The signing key never shares a process tree
with repository-controlled checks.

This section describes the current strict path first. Process-owned
compatibility runs and old version-1 markers retain the historical nonce path.
They must not be described as contained, two-key-verified Jobs.

### 13.2 `AcceptanceMarker`

`crates/deadreckon-core/src/gate.rs`:

```rust
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,           // "pass" | "fail"
    pub produced_by: String,
    pub issuer: String,           // "dr-gate" for a native proof
    pub proof_kind: AcceptanceProofKind,
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
    pub contained: bool,
    pub sandbox_backend: String,  // observed, never "auto"
    pub signature: String,        // HMAC-SHA-256 for version 2
    pub check_count: usize,
    pub checks: Vec<AcceptanceCheckResult>,
}
```

The marker lives at `<run_root>/proofs/turn-acceptance.json`. Version 2 binds
the issuer, native proof kind, check results, containment and observed backend.
Version 1 remains readable through the legacy validator.

### 13.3 `dr-gate` binary

`crates/deadreckon/src/bin/dr-gate.rs` has 2 commands.

The trusted controller first copies or generates the approved
`acceptance.yaml`. The evaluator refuses a missing, symlinked or non-regular
contract. Contract detection and persistence are controller work, not
evaluator side effects.

`dr-gate evaluate` then:

1. refuses `DEADRECKON_GATE_KEY`, `DEADRECKON_GATE_CONTAINED` and
   `DEADRECKON_GATE_SANDBOX_BACKEND`;
2. runs under the backend that the sandbox resolver actually selected;
3. runs every approved check and computes tamper facts;
4. writes no proof or Job control files;
5. returns one `GateEvaluation` as JSON on stdout.

The sandbox runner also scrubs those 3 inputs from inherited process
environment. For strict Jobs it puts the evaluator and descendants in a fresh
process group. It terminates residual descendants before it returns. This
prevents a delayed check process from surviving long enough to observe the
signing phase.

That fresh group is entered through a guarded release protocol. The helper
starts blocked on a private pipe in the worker's existing group. The runner
atomically writes and syncs a unique record containing evaluator launch ID, Job
attempt, outer launch ID, boot ID and process-start identity. Only after the
helper revalidates that record may it create the new process group, rewrite the
record as `running`, and execute the sandbox command. If the worker disappears
before release, EOF makes the helper exit without running a check. Per-launch
filenames prevent a later attempt from overwriting or deleting the earlier
identity. Linux process liveness treats terminal zombie/dead states as exited,
because `kill(pid, 0)` alone reports an unreaped zombie as present even though
it cannot own work. The internal boot-ID override used by restart tests crosses
the guarded validation boundary, then `dr-gate` removes it before repository
code executes.

If the resolved backend is `none`, a strict Job refuses before reading the
key. Otherwise, and only after evaluator cleanup, the controller starts
childless `dr-gate sign`. The signer receives the evaluation over stdin, the
HMAC key through `DEADRECKON_GATE_KEY`, and explicit containment plus the
observed backend as arguments. It:

1. rejects legacy containment environment;
2. validates the evaluation schema, run ID and canonical working directory;
3. reloads the approved contract and matches its digest, check count and every
   result to the approved checks;
4. recomputes tamper facts and requires an exact match;
5. reconstructs `acceptance-progress.jsonl` and
   `acceptance-tamper.json`;
6. writes a version-2 HMAC-SHA-256 marker bound to the observed backend.

The signer runs no repository-controlled checks and has no child process.

### 13.4 Validation

Current validation requires a version-2 native `dr-gate` proof, the correct
issuer and run identity, coherent containment and an HMAC-SHA-256 signature
from the Job's external key. Strict receipt validation also refuses
`sandbox_backend = none`, `contained = false`, synthetic controller markers
and missing key material.

Version-1 markers still validate against the run-local nonce during the
compatibility window. That compatibility does not upgrade them into native
strict proof.

### 13.5 Streaming progress

Strict evaluation does not stream progress into the protected proofs
directory. After strict revalidation, the signer reconstructs one
`AcceptanceProgressEntry` for each final result in
`proofs/acceptance-progress.jsonl`:

```rust
pub struct AcceptanceProgressEntry {
    pub checked_at: DateTime<Utc>,
    pub status: String,        // "started" | "running" | "passed" | "failed"
    pub index: usize,
    pub total: usize,
    pub result: Option<AcceptanceCheckResult>,
}
```

The TUI can read this file after signing, but it does not receive trusted
per-check progress while strict checks are still running. Historical
process-owned compatibility paths may still use the old live-streaming helper.
Progress remains observational. The version-2 marker is the load-bearing
deterministic proof.

### 13.7 Tamper evidence

The deterministic policy is tamper-evident, not a causal proof.
`crates/deadreckon-core/src/tamper.rs` builds a touched-file set from
`provenance.jsonl` plus the earliest `snapshots/turn-*` inventory. It maps
compiled checks to covered paths, lints commands for suppression patterns, and
classifies the run as `clean`, `caveat`, or `refuse`.

For a strict Job, the evaluator returns tamper facts without writing the proof.
The signer recomputes them from trusted inputs, requires an exact match, writes
`proofs/acceptance-tamper.json`, and includes its digest in the version-2 HMAC
marker. See §35 for the full policy and limits.

### 13.6 Where the gate is invoked

When the turn loop emits `Action::Done`, it routes through
`acceptance_gate_passed_or_record_failure`. The helper calls
`run_deterministic_completion_gate`, which owns the 2-phase sandboxed
evaluation and trusted signing sequence described above. It then validates the
marker.

- **If the gate passes:** the helper returns `true` and the loop continues into `promote_if_ready`.
- **If the gate fails:** the helper logs `acceptance.failed` to `traces.jsonl`, appends an explicit corrective hint to the run history (`"acceptance failed after turn N: <reason>. Continue by fixing the failing done criteria; do not declare done until dr-gate passes."`), emits a `RunEventKind::Error` event, records the reason in `state.failure_reason`, and **returns `false` — the run does not terminate**.

The agent sees the failure inside the next turn doc and can revise the working tree and re-declare `Done`. Only when the turn budget is exhausted does the run fail; at that point the accumulated reasons in `state.failure_reason` become the final `failure_reason` text (`turn_loop.rs:693-695`).

### 13.1 Default done-contract detection (polyglot floor + approved inference)

When a run has no operator `acceptance.yaml`, the gate no longer falls back to `cargo test`-or-`FileExists {working_dir}` — a hollow green for any non-Rust tree. `deadreckon-core::acceptance_defaults` resolves a real default contract.

**Deterministic detection floor.** `detect_project_kind(working_dir)` is pure, total, no-network, and runs no subprocess. It resolves a `ProjectKind` by sentinel files in a fixed precedence (first match wins; native kinds beat script-runners; lower row wins among natives):

| Kind | Sentinel | Compiled default (`Shell` unless noted) |
|---|---|---|
| Rust | `Cargo.toml` | `CargoTest` |
| Node | `package.json` with `scripts.test` | `<pm> test` (pm from `bun.lockb`/`pnpm-lock.yaml`/`yarn.lock`, else npm) |
| Deno | `deno.json[c]` | `deno test -A` |
| Go | `go.mod` | `go test ./...` |
| Python | `pyproject.toml`/`setup.py`/`setup.cfg` **and** visible tests | `python -m pytest -q` |
| Elixir | `mix.exs` | `mix test` |
| .NET | `*.csproj`/`*.fsproj`/`*.sln` | `dotnet test` |
| JVM | `pom.xml` → Maven; `build.gradle[.kts]` → Gradle (prefers `./gradlew`) | `mvn -q test` / `gradle test` / `./gradlew test` |
| Ruby | `Gemfile`/`Rakefile`/`spec/` | `bundle exec rspec` (spec/ + rspec in `Gemfile.lock`) else `bundle exec rake[ test]` |
| PHP | `composer.json` `scripts.test`; else `phpunit.xml[.dist]` | `composer test` / `vendor/bin/phpunit` |
| script-runner | `Makefile`/`justfile`/`Taskfile.yml` with a `test` target | `make test` / `just test` / `task test` |
| Unknown | none of the above | `FileExists {working_dir}` + caveat |

Script-runner detection is a textual, deterministic scan for a `test` entry-point — it proves the entry-point exists, then compiles the canonical invocation; it is the universal catch for ecosystems not in the native table. A bare `package.json` (no test script) or `pyproject.toml` (no visible tests) degrades to `Unknown` — a green "0 tests" is hollow.

**Generated spec is the audit record.** `compiled_acceptance_checks` and the dr-gate `evaluate_default_acceptance` path both route through `default_checks_for`, so the standalone binary and the in-process compile agree byte-for-byte. When detection fires with no operator spec, the compiled contract is serialized to `<run_root>/acceptance.yaml` with a `# generated by deadreckon detect: <kind>` header so the operator, `verdict`, and tamper see exactly what ran. An operator spec always wins verbatim and is never overwritten.

**Approval-gated inference (the trust rule).** `deadreckon run --infer-contract` is reachable only for an `Unknown` tree with no operator spec, and only interactively (a no-op under `--yes`/`--quiet`/`--json`/non-TTY). A cheap model (`commands::infer_contract`) *proposes* a test command from a redacted, bounded prompt (file **names** + first lines, treated as untrusted); the operator must **approve** it before it arms the gate, at which point it is written as a normal generated spec with a `# proposed by deadreckon --infer-contract (approved <ISO8601>): <model>` header. The gate is the trust boundary, so a model's proposal NEVER signs a marker without a human — the deterministic floor remains the only unattended marker-signer. No provider / low confidence falls back to the deterministic caveat; inference never fails a run.

**Friendliness.** The run preflight previews the resolved contract and its source (detected/operator/inferred); `--preview` prints a full kind+contract report; an `Unknown` tree carries a "no test contract detected" caveat into the verdict (no silent green); and a detected-but-unrunnable tree refuses with a `try: … --acceptance … (or --infer-contract)` footer.

### 13.8 `deadreckon verdict` — read-only "did it actually work?"

`verdict` is a read-only verb (`crates/deadreckon/src/commands/verdict.rs`) that answers one question about **any** run — native or imported — without touching its state. It composes the pieces above rather than adding a new engine: it re-runs the run's acceptance checks **now** through the gate's write-free `evaluate_acceptance_checks` (no spec, no progress, no state writes), reads (never overwrites) the original signed marker via `validate_acceptance_marker`, and counts changed files since the earliest snapshot through the same `tamper::touched_files` diff the gate uses.

This re-run is a read-side regression signal, not the strict Job completion
gate. It does not replace contained `dr-gate evaluate`, trusted signing or the
semantic judge, and it cannot issue a Job receipt.

**Three honest states**, decided by the pure `compute_verdict(had_marker, marker_valid, rerun_all_must_pass)`:

| State | Meaning |
|---|---|
| `VERIFIED` | A valid signed marker AND every must-pass check still passes when re-run now. |
| `REGRESSED` | A marker existed but no longer validates (forged/tampered), or a must-pass check now fails — the load-bearing new signal: work that silently broke. Never silently `VERIFIED`. |
| `UNVERIFIED` | No signed marker (imported, paused, or failed run). The declared checks are re-run fresh and reported, but the verdict never claims native gating. |

Run resolution defaults to the most-recently-updated run across every scope (so it works from any directory); an explicit id/prefix that is unknown or ambiguous refuses with a `try: deadreckon list` footer. Imported runs are detected by `import.json` in the run root and reported with `source:"imported"`. `verdict --all [--limit N]` re-verifies the recent runs into a one-screen comparison table; `--json` (single or `--all`) emits a stable machine envelope (`kind:"verdict"`, `status`, `checks`, `changed_files`, `source`, `next_actions`, `paths`).

**Read-only by construction.** The single write `verdict` performs is an additive audit sidecar at `<run_root>/proofs/verdict-<ts>.json`; it is never read back as authority (each invocation re-verifies live, so a stale sidecar can never mask a regression), and a read-only filesystem degrades to a swallowed write rather than a failed verdict. `verdict` never mutates `PipelineState`, advances a phase, signs a marker, or promotes.

---

## 14. Telemetry: Spend, Traces, Provenance, Events

Five JSONL files capture run evidence. Four (`spend.jsonl`, `traces.jsonl`,
`provenance.jsonl`, `events.jsonl`) live under `<run_root>/` directly. The
fifth, `proofs/acceptance-progress.jsonl`, is reconstructed by the trusted
signer after strict evaluation. Legacy compatibility evaluation may still
stream and truncate it. The signer also writes
`proofs/acceptance-tamper.json`, bound into the signed marker. Normal ledger
JSONL files use `append_json_line`, which opens in append mode and calls
`sync_all` after each line.

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

`deadreckon start "<goal>"` is the guided production command. It is a thin
CLI-layer decision helper, not a second runtime state machine. Each invocation
builds an ephemeral launch decision, prints the selected path and reason, and
either previews or dispatches to the durable Single, Graph or Campaign
scheduler described in §58. In an interactive TTY, `start` can prompt for the
launch shape, execution team, done contract, source mode and final confirmation.
Explicit flags skip the matching prompt. Previews remain state-free and this
path adds no fields to `PipelineState`.

Guided setup treats provider and model choice as one execution-team decision.
A uniform choice applies one provider/model pair to every required role. The
custom path can set planner and implementor, implementor and reviewer, or
individual full-plan implementor overrides. Full-plan also asks for 2 to 6
implementors. Provider-specific model choices come from the registry and its
cached model catalog when available. The exact role routes and models persist
in the launch plan, durable driver and recovery/replay inputs; a model chosen
for one provider does not leak to another provider. Legacy global `--model`
continues to work.

Within guided setup, DeadReckon resolves and validates the source exactly once
before provider discovery, role selection, done-contract authoring, writes or
final confirmation. Preview and dispatch consume that same ephemeral resolved
value. Provider resolution then uses configured defaults first and probes
installed subscription CLIs only when no default route is configured. A TTY
choice is launch-local and does not rewrite config. Non-TTY callers get
concrete `try:` lines for `init`, `detect`, `config provider`, or `providers
list --all`. Existing project criteria use the same `def-done` and
`.deadreckon/acceptance.yaml` contract as direct runs. Source resolution keeps
the direct-run safety posture: git worktree by default, explicit copy or fresh
modes, and explicit dirty-worktree consent.

History-aware `start` scans the current project scope for the newest completed, promoted, non-in-place run. When one exists, the TTY launch picker adds a "Follow up" choice that dispatches through `extend`; preview and JSON output also include exact commands for `deadreckon extend <run-id> "<goal>"`, `deadreckon start "<goal>" --mode review --yes`, and `deadreckon start "<goal>" --mode full-plan --yes`. This keeps scripted `start` deterministic while making it obvious how to continue prior work or launch a new orchestration pass.

Auto mode is advisory. When a usable provider exists, `start` makes one bounded read-only classifier call through the existing provider router to recommend a single verified run, review/full-plan orchestration, or campaign with a count and rationale. The validated recommendation is preview-scoped and state-free; no personal preference is persisted. Smoke/no-provider paths use deterministic fallback heuristics. In a TTY, the recommendation appears first in the picker and the user can override it with an explicit selection or flag.

`start --preview`, `run --preview`, and `orchestrate --preview` share launch
facts for the path, provider, done contract, workspace, watch, stop and finish.
Guided previews also show the exact provider/model pair for each execution role,
the implementor count and any child overrides. Successful launches add exact
`attach`, `status`, `kill`, and `finish` commands for the new Job. Existing
`run`, `extend`, `orchestrate`, and `campaign` remain the direct commands for
users who already know the path they want.

Prompt eligibility is deliberately narrow: `--json`, `--plain`, `--quiet`, `--yes`, and non-TTY execution never start the picker and never block on stdin. Those paths preserve deterministic JSON/recovery output and scriptable launch behavior. `--preview` may ask TTY users for selections, but it remains state-free; provider config is not written by a provider selection, and done-contract files are only generated for an actual launch after final confirmation.

`review` and `full-plan` now accept `--from`. The approved source is copied
into the Job before queueing, including dirty and untracked deliverables but
excluding Git and rebuildable/runtime output. Graph children work from that
controller-owned copy and never from the mutable operator path. When a done
contract must be authored, its files remain owned by the launch project while
the provider sees a bounded, redacted dossier of the resolved source. The
plain/card/JSON preview, frozen authority and Graph driver all report the same
source decision. §59 records the full Soundings boundary.

The CLI defaults are honest: `--sandbox` defaults to `auto`, `--max-spend` defaults to `$10` (with a confirmation gate above `$50`), `--provider` defaults to the highest-credentialed entry per the fallback chain, `--skill` defaults to `default-coding`.

`run` now starts codebase-aware by default. In a git repo it previews and then creates a `git worktree` on a `dr/...` branch; `--fresh` preserves the old empty-working-dir behavior, `--from <path>` uses copy mode, and `--in-place --i-know-its-a-lot` edits the source tree directly. Completed worktree runs hint `apply` / `discard`; copy and fresh runs hint `export` / `extend`. Run-id arguments accept unique prefixes and `latest` / `last` resolves to the latest run in the current project scope.

`completion install` is driven from the real clap command tree, so subcommand aliases, flags, and value-hint completions stay in sync with `deadreckon --help`. The handler detects the active shell via `$SHELL`, writes the script to a per-shell default path (e.g. `~/.zsh/completions/_deadreckon`, `~/.local/share/bash-completion/completions/deadreckon`), and for zsh adds a managed `# deadreckon completion` block to `~/.zshrc` unless `--no-rc` is passed. `init` invokes `try_install_completion_after_init` so first-time setup ships completions opt-out (`init --no-completion`). The per-shell stdout variants (`completion bash|zsh|fish|elvish|powershell`) print the script for users who manage their own shell config.

`run` startup details (`print_run_started`) are now also emitted at the top of `extend` and `resume` so extended/resumed runs surface their selected provider route and doc-provider source the same way fresh runs do. Interactive terminals receive a `deadreckoning_course` ASCII progress strip and a polled `cli_wait_status` line while a long turn is in flight; the status is cleared as soon as the loop reports back. `kill` against a loaded run now also persists `RunStatus::Killed` + `killed_at` + `failure_reason = "killed by user"` before returning so downstream tooling sees a consistent terminal state.

---

## 18. TUI (`attach`)

`attach_command` lives in `crates/deadreckon/src/commands/attach.rs`. The terminal loops delegate to the private `tui/` render/state facade for run, plan, chain, and campaign frames; provider refresh and narrative projection stay outside the render path. Historical `main.rs` line numbers for attach are obsolete after the Decompose pass. Helm (§47) is the current mission-control layer on top of those attach surfaces: the status spine, voyage tree, timeline, why panel, command-mode/modals, and motion policy are read-model UI, not durable state.

### 18.1 Behavior

- On a TTY: `attach_tui()` enables raw mode, alternate screen, and renders a `ratatui` UI.
- Off-TTY: prints a plain-text summary + locations.

`attach` dispatches by id kind: a run id opens the run TUI documented below, a chain id opens the chain attach view (`Chains`, §28), a plan id opens the plan attach TUI (`Plans`, §30.3 / §32.3), and a campaign id opens the campaign attach TUI (`Campaign Orchestration`, §36.9). These TUIs draw from the same palette (`ui::TUI_PALETTE`, §26.7), the same status-spine vocabulary (§47.1), and the same key conventions (`q`/`Esc`/`Ctrl-D` detach; `w` opens the why panel where evidence exists; `t` focuses the timeline where available; `Enter` zooms into the selected node but is no longer required to understand state). The voyage tree makes campaign, plan, chain, and run state visible above the fold; zoom/drill-in remains as a detail affordance with breadcrumbs, not the only comprehension path.

`attach <id> --view narrative` adds a calmer operator projection for runs, plans, chains, and plan child refs. The default remains `activity`, so raw tool/provider lines still open first unless the user requests the narrative view. In narrative mode, `n` toggles back to raw activity, `v` cycles `architecture -> agents -> files -> evidence -> none` on surfaces with a visual map, and `r` requests a provider-backed refresh when a configured route is available. While the TTY narrative view is open, meaningful run and plan events also request a provider refresh: errors, completions, tool milestones, docs checkpoints, acceptance running/pass/fail transitions, child-run discovery, task terminal states, and merge-repair milestones. Long-running quiet periods request a refresh after the narrative quiet window when the run or plan is still running. Provider refreshes are background jobs: manual/event/quiet refreshes coalesce while one is active, `q`/`Esc`/`Ctrl-D` detach remains immediate, and child zoom from plan attach cancels or suspends the in-flight narrator before opening the child run. Provider refreshes are bounded: the prompt is built from redacted evidence windows, the provider must return strict cited JSON, graph labels may only target deterministic graph ids, and failures persist a stale deterministic projection instead of breaking attach.

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
- **Acceptance meter**: derived from `AcceptanceLive`
  (`collect_acceptance_live`). The panel tails
  `proofs/acceptance-progress.jsonl` when present and pivots to a marker view
  once `turn-acceptance.json` is signed. Strict Jobs reconstruct final progress
  after evaluation; only legacy compatibility paths can show per-check
  `running` transitions from this file.
- **Center, left**: wide streaming list of tool calls + provider activity + recent events. Acceptance lines from `acceptance_activity_lines` are interleaved so the operator sees the same progress in the activity stream and the meter.
- **Narrative view**: `--view narrative` or `n` swaps the center-left activity pane for prose sections under the `Narrated` operator heading: freshness/coverage, headline, current work, architecture notes, risks, next likely action, and citations. Wide terminals split that pane with a right-side visual map where that surface owns one; narrow terminals collapse to prose first. Run narratives cite `proofs/acceptance-progress.jsonl` or `proofs/turn-acceptance.json` when acceptance evidence exists, so failed done criteria point at the durable proof artifact. Plain/off-TTY narrative attach prints the same projection with citations and ASCII map lines when `--visual` is not `none`; `--json --view narrative` emits the structured state, snapshot, and graph objects. Non-TTY narrative attach stays deterministic and does not call a provider unless a future explicit refresh surface opts in. Chain attach now supports the narrative view and keeps chain steps in the voyage pane with spine/timeline participation.
- **Completed docs view**: pressing `d` toggles the center-left panel from provider activity or narrative view to `RUN-NARRATIVE.md` rendered through `pulldown-cmark` into ratatui `Line`/`Span`s. Headings, bullets, inline code, fenced code blocks, links, task markers, math, and horizontal rules receive terminal styles and remain scrollable. The docs view remains a separate completed-run artifact rather than being merged into the live narrative projection.
- **Center, right**: narrower live files list with count/bytes in the panel title.
- **Bottom**: supervised PIDs + their `ps` lines (alive/dead annotation).
- **Footer**: action-first completed footer (`[d] Docs` / `[d] Activity`, `[a] Apply`, `[b] Abandon`, `[s] Show`) or context-specific scroll/detach/detail help while running. Helm adds a first-session cue (`Tab panes · w why · : commands`) and a sectioned `?` overlay; both are hints only and never replace static status. The footer's second line carries `deadreckoning_status_line` while long operations are in flight.

Campaign attach has its own campaign-shaped frame rather than reusing the run panel grid, but Helm projects it into the same voyage/detail/timeline/spine mental model. The TTY view shows campaign and sub-plan nodes with status, gate, spend, roll-up, tree budget, and breadcrumb context; the selected node drives the detail pane. `Enter` zooms into the selected sub-plan when a `sub_plan_id` exists, otherwise it keeps the campaign frame open. Off-TTY and `--plain` still print the read-only campaign summary with an explicit `deadreckon attach <sub-plan-id>` hint, and `--json` emits the structured campaign attach object instead of entering ratatui.

### 18.3 Data source and responsiveness contract

Each attach surface runs a budgeted tick loop. The render path is pure: it must not call providers, recurse through provider roots, append narrative snapshots, or reread unbounded JSONL files. Slow or potentially blocking work is either moved into an attach-owned cache/tailer or into a background refresh job whose completion is polled between frames.

Run attach uses `TuiEventFeed` for run events and `AttachJsonlTail` for `spend.jsonl`, `traces.jsonl`, and `flight-events.jsonl`, so redraws parse only appended complete rows after the first load and ignore partial trailing JSONL until it is complete. Live-file collection uses an attach-specific inventory walker that prunes heavy cache/profile directories before descent and caps displayed rows without losing total counts. Provider activity prefers current flight rows; descriptor-backed provider-log fallback scans are throttled by freshness, matched path, root mtime, and file mtime so a live attach does not recursively scan provider homes every frame.

`collect_provider_activity` resolves provider ingest through descriptor `[ingest]` metadata: candidate roots, env overrides, cwd matching, storage kind, file glob, freshness window, and schema key. `deadreckon import` reuses the same descriptor metadata for provider transcript discovery and adds import-only session selection, manifest writing, and normalized trace/provenance event creation. `cli:codex` reads `~/.codex/sessions/**.jsonl` and matches `session_meta.payload.cwd`; `cli:claude-code` reads `~/.claude/projects/<cwd-slug>/*.jsonl` using Claude Code's path-to-project mapping and matches top-level `cwd`; `cli:gemini` reads Gemini JSON/JSONL file logs; `cli:opencode` reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON; `cli:copilot` reads `~/.copilot/session-state/*.jsonl` plus nested `events.jsonl` and matches `data.context.cwd`; `cli:pi` reads `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`, validates the first nonblank row is a Pi `session`, and matches the header `cwd`. Schema-specific adapters only decode rows into common activity lines (`agent`, `thinking`, `tool`, `result`, `todo`, `tokens`) and normalize tool labels through `deadreckon_providers::taxonomy`.

Production run attaches — same-process and cross-process alike — read run events by tailing `events.jsonl` via `TuiEventFeed::file_tail`; `TuiEventFeed::from_broadcast` is `#[cfg(test)]` only. (The loop's `emit_event` writes the file and sends on the `RunEventBus` channel together, so the file tail stays current; the broadcast path is reserved for a future same-process attach.) Plan attach consumes `PlanEventBus` / `PlanEventFeed`, which owns `plan-events.jsonl` replay/tailing, emits plan snapshots, tolerates malformed or partial plan-event rows, and multiplexes discovered child and repair run `events.jsonl` streams into the plan activity pane. Chain attach keeps its own `AttachJsonlTail<ChainEvent>` for `chain-events.jsonl`, preserves the existing redo/extend/pause/kill controls, ignores partial last lines until complete, and shows an activity-read hint when chain event catch-up falls behind the tick budget. Campaign attach uses `CampaignEventFeed`: it tails `campaign-events.jsonl` with `JsonlTail`, rediscovering sub-plans from `campaign.json`, and tails each discovered sub-plan's `plan-events.jsonl` with the same read-side tailer. Helm's `TreeModel` folds the existing run/plan/chain/campaign events into a bounded read-model tree, so campaign/plan/run state is visible without mandatory drill-in while still allowing zoom into the existing plan/run attach loops. The production feeds remain durable-file backed for cross-process attach, with broadcast-capable APIs available for same-process streams.

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
cargo test --workspace --all-targets
cargo check --workspace --all-targets
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

---

## 21. Key Design Decisions

1. **No new `PipelineState` fields without a strong reason.** Parent lineage, materialization status, and other run-level metadata live in files inside the working tree (e.g., `.deadreckon/parent.json`), not on the state struct. This keeps `state.json` migration-safe.

2. **Two-layer split is non-negotiable.** Skills (Markdown) own judgment; the binary owns invariants. A skill cannot bypass the gate; a gate cannot read prompts.

3. **Anti-self-attestation separates evaluation from signing.** Strict Jobs run
   keyless checks under the resolved sandbox, reap the whole evaluator process
   group, then give the external HMAC key only to childless `dr-gate sign`.
   Legacy-v1 compatibility markers retain their nonce validator.

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

- Live narrator (§44): a `dr run` narrates itself in plain English as it works — a continuity-carrying, subscription-first model sidecar with a deterministic floor, a calm foreground block, headless `--narrate`, and attach/post-hoc convergence. This closes the prior thin gap where narration existed only at attach time and a piped run was silent.
- Orchestrated narration (§45): orchestrate/campaign children (subprocesses) now narrate file-only to their own `snapshots.jsonl` on the $0 floor (or a pinned `--narrator-model`); the plan attach surfaces each child's live headline (live-preferring read, density-capped), `dr orchestrate --narrate` prints a one-line-per-child stderr aggregate, and `dr attach <campaign> --view narrative` renders a campaign projection at plan parity. Closes the prior thin gap where only a top-level `dr run` narrated.
- Verdict — did-it-actually-work (§13.8/§37.11): `deadreckon verdict` is a read-only verb that re-verifies any run NOW (native or imported) into three honest states — `VERIFIED` / `REGRESSED` / `UNVERIFIED` — by re-running its acceptance checks through the gate's write-free path, reading (never overwriting) the signed marker, and diffing changed files since snapshot. A forged/tampered marker or a now-failing must-pass check reads `REGRESSED`, never a false `VERIFIED`; imported runs (Claude Code/Codex/aider) report `UNVERIFIED` with `source:"imported"`. `--all` compares recent runs, `--json` is at parity, and the only write is an additive `proofs/verdict-<ts>.json` audit sidecar never read back as authority. No `PipelineState`/`AcceptanceMarker` schema change; no run-state mutation or promotion. This closes the prior thin gap where there was no honest, after-the-fact "did it still work?" check for imported or long-finished runs.
- Polyglot done-contract (§13.1/§35.9): a run with no operator `acceptance.yaml` in a non-Rust tree no longer gets a hollow `FileExists` gate. A deterministic detector compiles a real default test command for Node/Deno/Python/Go/Elixir/.NET/JVM/Ruby/PHP and Make/just/Task script-runners, writes the generated spec for audit, and tamper covers non-Rust tests (deleting a JS/Py/Go test refuses like a deleted Rust test). Optional `--infer-contract` proposes a contract for an unknown tree that the operator must approve before it arms the gate. This closes the prior thin gap where "VERIFIED" on a non-Rust project could mean nothing was checked. No `AcceptanceCheck` schema variant added; inference never arms the gate without approval.
- Workspace, crates, build, lint, fmt, test discipline.
- Workspace lint discipline (deny-tier clippy + rustc), tuned release profile, registry-shaped library `lib.rs`, library print refusal, and error retryable/fatal taxonomy as vocabulary for future watchdog work.
- Binary module layout: the former 40.6k-line `crates/deadreckon/src/main.rs` has been split into private `commands/` and `tui/` modules behind `main_inner` dispatch. `cli.rs`, the `Command` enum, all verbs, all user-facing output, and the public library surface remain unchanged by that split.
- `PipelineState` shape, phase machine, atomic state writes, schema version.
- Keel protocol vocabulary (§52): the pure `deadreckon-protocol` crate owns
  the event, spend, trace, flight, and narrative-snapshot-reference wire
  types; `LedgerItem`, `LedgerFile`, one persistence policy, and generated
  checked schemas give readers and writers one vocabulary while the five
  existing JSONL paths and bytes remain unchanged.
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
- Release packaging: `cargo-dist` configuration covers five OS/architecture targets, shell and PowerShell installers, Linux glibc 2.28 metadata, lane-aware fail-closed macOS signing/notarization for official RC/stable tags, Authenticode signing for stable Windows artifacts, `SHA256SUMS`, `release-manifest.json`, `release.spdx.json`, GitHub artifact attestations, npm provenance, a no-network npm wrapper with five platform packages, and Homebrew tap publishing through `gregce/homebrew-tap`.
- Codebase-default running: worktree mode, copy mode, in-place mode, fresh-mode preservation, preflight + preview UX, and `codebase.json` files-not-fields metadata.
- `apply` and `abandon` for worktree rollback/apply lifecycle.
- `materialize`, `extend`, `undo`, `list`, and `show` integration with codebase mode metadata, including worktree extension branches chained from parent `dr/...` branches.
- UX consolidation: project-scoped `list`, `latest` run aliases, `status`/`next`, `cleanup`/`prune`, `export`/`discard` aliases, and TTY-aware formatted output.
- Self-documenting run artifacts in stoa shape: `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, optional `AS-BUILT-DELTA.md`, per-turn `_incremental.jsonl`, explicit `docs_checkpoint` run events, and `polish.json` schema v2.
- `deadreckon doc`, `list` DOCS status, doc-aware `apply` commit bodies, extend-parent narrative updates, diff coverage retry, the legacy repo/user/project `run-narrator` skill mechanism, and default split polish skills (`narrator-overview`, `narrator-phases`, `narrator-as-built`, `narrator-decisions`).
- Acceptance gate with signed marker; anti-self-attestation and tamper-evident hollow-pass detection are enforced without adding `PipelineState`, `Plan`, marker, or check-result fields.
- `init`, `config get/set`, `run`, `doctor`, `status`/`next`, `list`, `attach`, `kill`, `resume`, `undo`, `rewind`, `show`, `import`, `verdict`, `cleanup`/`prune`, `completion`, `learn`, and `improve` verbs.
- Shell tab-completion via `completion install` / `completion {bash,zsh,fish,elvish,powershell}` driven from the live clap command tree; `init` opt-out installs completions and (for zsh) appends a managed `.zshrc` block.
- `ratatui` attach TUI with spend/context/acceptance telemetry, provider activity, in-TUI Markdown docs rendering, live files, process panel, scrollable panels, campaign sub-plan cards, and completion action footer. Run, plan, chain, and campaign attach now share an explicit responsiveness contract: render paths are provider-free and write-free, JSONL streams are tailed or cached, provider narrative refreshes run in cancellable/coalesced background jobs, stale narrative snapshots survive redraw, and long operations surface a `deadreckoning` ASCII status line in CLI and footer alike.
- Helm attach (§47): a uniform five-question status spine, flattened voyage tree for campaign -> plan -> run and chain steps, event-driven input loop with input-to-frame latency instrumentation, in-frame chain input/modals, `:` command mode for existing chain verbs, `w` why evidence panel, scrubable turn timeline, chain narrative parity, sectioned help overlays, focused footer hints, and `[ui] motion = full|reduced|off` effects. This moves the prior flattened-campaign-tree and in-frame-input items from thin to shipped; the attach daemon, ratzilla web mirror, cross-machine attach, and provider pty embedding remain V1 candidates.
- Contract (§48): `.deadreckon/acceptance.yaml` remains the durable done-contract schema, but `acceptance` and `start` now compile it into a read model with stable per-check summaries, behavior/falsifiability labels, deterministic lint findings, and goal↔contract divergence. The compiler prompt sees the run goal, demands behavioral/falsifiable checks, bans source-scan-only contracts and `--if-present`-only build/test gates, and the Course plan/card/JSON carry compiled checks plus divergence.
- Unified execution teams (§17.1): guided `start` selects provider and model as
  one team decision, supports per-role and individual full-plan implementor
  overrides, and persists exact choices through preview, launch, recovery and
  replay.
- Durable stall prevention (§58): Job admission rejects unsealable placeholder
  contracts; cumulative active-attempt wall caps include controller work and
  restart gaps; owned process trees are reconciled before terminal status; Git
  pipe handling and SwiftPM artifact filtering remove 2 known local stalls.
- Descriptor-driven provider activity ingest for Codex, Claude Code, Gemini JSON/JSONL, OpenCode file-mode logs, GitHub Copilot CLI session-state JSONL, and Pi session JSONL, normalized into `agent` / `thinking` / `tool` / `result` / `todo` / `tokens` rows without rewriting provider-owned logs.
- Descriptor import hardening: `deadreckon import` accepts legacy aliases and provider descriptor ids, discovers CLI transcripts through descriptor `[ingest]`, selects concrete sessions by cwd or `--session`, writes `import.json`, refuses ambiguous/changed imports with `try:` lines, and normalizes trace/provenance rows for Codex, Claude Code, Gemini, OpenCode file-mode, GitHub Copilot CLI, Pi, and Cursor SQLite.
- Acceptance progress: strict Jobs reconstruct final per-check rows only after
  the signer revalidates the evaluation. Legacy process-owned paths may still
  stream `started`/`running`/`passed`/`failed` rows. The attach TUI tails the
  file alongside the signed marker.
- Extended runs carry the parent's `acceptance.yaml` into the child run and emit the same `print_run_started` startup details (provider route, doc-provider source) as fresh runs; resume does the same.
- `--max-spend` cap with pause-at-cap; `--max-wall-seconds` for subscription providers.
- Event-backed TUI attach: production run attaches (same- and cross-process) tail `events.jsonl` incrementally via `TuiEventFeed::file_tail` — `TuiEventFeed::from_broadcast` is `#[cfg(test)]` only. Plan attach uses `PlanEventBus` for durable replay/tail plus child/repair event multiplexing, chain attach tails `chain-events.jsonl` incrementally with partial-line tolerance, and campaign attach tails `campaign-events.jsonl` plus each discovered sub-plan's `plan-events.jsonl`.
- Cross-process cancellation: `kill` writes a durable cancel marker before signaling; the run loop observes it while provider calls are in flight and reports killed status through events.
- Partial-trace resume: resume reconstructs only completed tool boundaries and `resume --from-turn` truncates traces, spend records, and future snapshots together.
- Durable per-run `sandbox.toml` plus per-tool sandbox policy: bash/write-file paths get specific filesystem and network permissions; refusals include `try:` and are recorded in traces and provenance.
- YAML done-contract files (`acceptance.yaml`): the trusted controller
  materializes the approved contract. Keyless `dr-gate evaluate` runs its
  required or optional tests inside the resolved sandbox and returns results
  plus tamper facts without proof writes. Childless `dr-gate sign` revalidates
  them, reconstructs tamper and progress evidence, then signs with HMAC.
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
- Autonomous sequential chains (historical implementation, now
  characterization-only for execution and mutation): the stored model includes
  `chain "..."`, `chain plan`/`expand`, `chain run`, `chain attach`, `chain
  status/show/list`, `chain pause/resume/kill`, `chain undo`, `chain extend`,
  and `chain redo`; chains use `latest`/`last` aliases, `chain.json`,
  `chain-events.jsonl`, a conductor lock, chain hooks, aggregate spend caps,
  green-policy auto-apply, and a multi-step ratatui timeline with single-run
  chain context. The public product creates supported chains as Graph Jobs;
  public historical execution and mutation refuse before changing state.
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
12. **Course launch planning (§46).** `start` resolves the execution shape from a deterministic SignalBundle plus one clamped planner call, records the decision as `launch-plan.json` in every dispatched root, previews it on a golden-pinned course card, asks at most one question (the done contract, only when undetected), replays plans (`--plan`), emits a launch JSON envelope (`--json --yes`), collapses one-piece plans to runs, and accepts worker reshape proposals only through the explicit `reshape` verb. This closes the campaign/chain auto-detect friendliness cells and the launch JSON-parity gap; it does NOT add auto-reshape, per-piece task seeding, or campaign-level reshaping (V1).

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
- **Acceptance marker** — a signed JSON file
  (`proofs/turn-acceptance.json`). Strict Jobs get a version-2 native marker
  from childless `dr-gate sign`, bound by HMAC-SHA-256 to checks, tamper facts,
  containment and the observed backend. Version-1 compatibility markers use
  the historical run-local nonce.
- **Spend** — a record of LLM cost per turn. USD for HTTP providers; wall-clock seconds + `subscription: true` for CLI providers.
- **Provenance** — per-file attribution: which `tool_call_id` produced which file in which turn under which model.
- **Trace** — every LLM call and every tool dispatch, with latency + structured detail.
- **CLI sub-agent** — a `cli:*` provider whose `complete()` invocation is one whole turn (the sub-agent does its own tool calls inside). Detected by `response.trace["kind"] == "cli_subagent"`.
- **dr-gate** — the standalone binary at
  `crates/deadreckon/src/bin/dr-gate.rs`. Its keyless `evaluate` command runs
  approved checks. Its childless, key-bearing `sign` command revalidates and
  signs them. The agent cannot impersonate a native version-2 proof.
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

### 25.16 Helm Attach Reads Docs

Helm does not add a documentation schema. The docs pane still reads promoted `RUN-*` / `PLAN-*` Markdown through the existing renderer, the why panel cites gate/tamper/provider artifacts rather than generated prose, and the timeline derives turn stories and diff counts from `_incremental.jsonl`, flight checkpoints, spend rows, and proof files. Course artifacts (`launch-plan.json`, `reshape-proposal.json`, and the `reshape.proposed` trace) are displayed as read-side context only; Helm never writes or mutates them.

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

### 27.5 In-Frame Control Polish

Helm moves chain attach's destructive confirm, extend input, and command-mode input into ratatui modals instead of suspending the alternate screen. The modal primitive swallows keys until submit/cancel, `Esc` cancels, and confirmed commands dispatch to existing CLI verb paths. The remaining "press Enter to return" overlays around nested command output stay deferred in `docs/V1-CANDIDATES.md` because they need an explicit output-capture design.
The extend path now returns the public non-mutating refusal and durable
migration schedule; it does not update the stored chain.

Motion is policy-gated: `[ui] motion = full|reduced|off` resolves to reduced under non-TTY/replay defaults, all effects are under 800ms and input-preemptible, and `off` removes effect frames without hiding information.

---

## 28. Chains & Autonomous Goal Chaining

This section is a historical implementation reference for the stored-chain
model. The public product no longer enters its conductor or mutation path:
supported ordinary chain creation compiles one linear Graph Job; unsupported
policy-rich creation refuses before Job creation, planning, or state mutation;
historical `chain run|resume` refuses before state mutation or execution; and
`chain extend` plus `chain redo --extend` computes a proposed schedule but
refuses before saving it. Only the `deadreckon-characterization` binary can
exercise the conductor and mutations described below for tests. Read-only
inspection of stored historical chains remains public.

### 28.1 Mental Model

In the historical model, a chain is an ordered list of step goals plus branch,
apply, budget, and stop policy. The conductor is a CLI process entered only by
the characterization binary's `deadreckon chain ...` or `deadreckon chain run
<id>` path. It acquires a chain lock, spawns each step as a normal `deadreckon
run --worktree`, waits for that run to complete and pass acceptance, applies it
to the source branch when policy allows, then bases the next step on the
updated head.

Chain state is separate from `PipelineState`: no run schema fields were added. Files live under `~/.deadreckon/chains/<chain-id>/`.

### 28.2 `chain.json`

`crates/deadreckon-core/src/chain.rs` defines `Chain`, `ChainStep`, and `ConductorState`. The top-level chain records `chain_id`, `root_goal`, ordered `steps`, `branch_policy`, `apply_mode`, `apply_strategy`, `apply_allowlist`, `on_fail`, circuit-breaker counters, aggregate spend/wall caps, scope, base branch/SHA, cwd, provider/model/sandbox, status, pause/failure reason, conductor pid, timestamps, and deadreckon version.

Each `ChainStep` records index, goal, status, run id, applied timestamp/SHA, failure reason, step cap, and actual spend. `ConductorState` is the live-process pointer in `conductor.json`: conductor pid, live step, live run id, and live child pid.

### 28.3 Historical Create And Run Shape

The characterization-only path mirrors the old `run`:

```bash
deadreckon chain "step one" "step two" "step three" --yes
deadreckon chain plan "build a chess app" --n 6 --yes
```

In this historical model, `chain expand` is an alias for `chain plan`.
`--from-file` and `--from-stdin` accept newline-separated steps. `--draft`
writes `chain.json` without starting the conductor. Bare `deadreckon chain`
prints scoped status; `deadreckon chain run` resumes `latest`; `latest` and
`last` are accepted anywhere a chain id is expected. In the public binary,
supported new creation compiles a Graph Job and stored-chain execution refuses.

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

### 28.9 Historical Lifecycle Verbs

The retained model defines `chain pause`, `resume`, `kill`, `undo`, `extend`,
and `redo` against the old run lifecycle. Undo reverts applied SHAs in reverse
order. Extend inserts or appends a step and can reopen a completed chain when
inserting. Redo chooses a specified step, the first failed step, or the latest
applied step; applied-step redo requires `--reapply`, which reverts before
requeueing. Public historical execution and mutation refuse before these
effects; only the characterization binary exercises them.

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

When a receipt is missing, normal `deadreckon update` detects the current binary path and persists the inferred receipt before routing the update. A stale receipt for that same canonical executable is refreshed in memory and persisted by mutating update/setup flows. Update continues to honor an existing durable receipt when a package wrapper manages a distinct target; doctor reports that relationship, while its explicit repair path refuses to claim the other executable. `deadreckon update --check` deliberately remains read-only and does not write that receipt. Detection recognizes npm package layouts, Homebrew Cellar paths, `~/.cargo/bin`, shell installer paths under `~/.local/share/deadreckon` or `%LOCALAPPDATA%/deadreckon`, and falls back to `source`.

`deadreckon doctor` inventories the running executable, every executable found on `PATH`, known shell/Cargo/Homebrew locations, the receipt target, and the last supervisor checkpoint. The JSON report includes canonical paths, observed aliases, roles, channel, probed version, SHA-256, probe errors, and the native update command for each distinct binary. Version and ownership conflicts are warnings with concrete evidence. A differing version is an active conflict only when that installation is the running executable, the first executable selected by `PATH`, the receipt target, or the supervisor checkpoint; differing shadowed copies remain visible as advisories. `deadreckon doctor --repair` is an explicit mutation: it treats the invoked executable as authoritative, backs up and atomically repoints a conflicting shell-installer-owned `PATH` executable to that canonical binary (while refusing a downgrade), repairs a missing or stale receipt when it resolves to that executable, then installs or replaces and restarts only a DeadReckon-managed supervisor definition. It never overwrites an unmanaged service definition or an npm, Homebrew, Cargo, or arbitrary user-owned executable.

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

Homebrew publishing uses cargo-dist's generated formula as the starting point. `release/homebrew/patch-formula.mjs` then injects a `write_deadreckon_receipt!` method into the formula and calls it from the `install` block; the injected code writes `install-receipt.json` with `channel: "brew"` and `install_source: "brew:gregce/tap/deadreckon"` at install time. cargo-dist itself still owns release-archive SHA-256 pinning. The release workflow publishes the patched formula into `gregce/homebrew-tap` with `HOMEBREW_TAP_TOKEN`.

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

Under Helm (§47), the same plan events also feed the voyage tree and status spine. Task nodes show status, spend, and gate progress without requiring `Enter`; selecting a task drives the detail pane, while `Enter` remains the zoom path into the child run when one exists. Timeline marks are derived from the same durable plan/run rows and never from an in-memory broadcaster.

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

For strict Jobs, keyless evaluation returns the tamper object in memory. The
signer recomputes it from the approved contract and current evidence, requires
an exact match, then writes this file and the version-2 marker. The
HMAC-SHA-256 signature binds the tamper digest with the marker fields, check
results, containment and observed backend.

Version-1 compatibility markers keep the historical nonce-and-proof-bytes
signature. Missing tamper files retain their old empty-bytes tolerance only on
that legacy path.

### 35.6 Surfacing

`status`, exit cards, `show --why-failed`, and attach activity derive a
per-check gate line from marker and progress rows, for example `gate: PASSED
4/4`. On strict Jobs, progress rows appear after signing rather than live
during evaluation. Tamper proof data adds whether tests changed and any caveat.

### 35.7 Honest subscription spend

The same result surfaces no longer render subscription-only CLI routes as `~$0.000000`. Subscription-only spend reads `not metered (subscription) · wall <s>s · <n> turns`. Mixed routes show the metered total and append `+ subscription turns`.

### 35.8 Limits

This is tamper evidence, not a causal soundness proof. It does not prove that a
covered-file edit caused a pass, and it does not cover every language's idioms.
Strict Job checks run inside the resolved sandbox. They can write only through
the sandbox's working-directory policy and cannot write gate or proof paths.
Historical process-owned compatibility checks do not gain that strict
containment guarantee.

### 35.9 Cross-language tamper coverage

Tamper coverage is LLM-free and language-uniform — no model in the coverage/refuse path, no language-gated early-out. `check_coverage`'s `Shell` arm recognizes a known cross-language test runner by its program (`shell_program_is_test_runner`: `pytest`/`python -m pytest`, `go test`, `npm/pnpm/yarn/bun test`, `deno test`, `mix test`, `dotnet test`, `mvn … test`/`gradle test`/`./gradlew test`, `rspec`/`bundle exec rspec`/`rake test`, `phpunit`/`vendor/bin/phpunit`/`composer test`, `make/just/task test`, `jest`, `vitest`) and maps the ecosystem's conventional test files (`tests/`/`test/`/`spec/` directories, `*_test.go`, `*.test.*`, `*.spec.*`, `*Test.java`/`*Test.kt`, `*_test.exs`, `*Test.cs`/`*Tests.cs`, `test_*.py`/`*_test.py`) to `Test` coverage.

Because `classify` already refuses deletion of any `Test`/`Target`-covered file regardless of language, **deleting a covered JS/Py/Go/etc. test now refuses exactly like a deleted Rust test** — `evaluate` scans the earliest snapshot for conventional test files when a shell test-runner check is present (the deleted file is gone from the post-run tree). The suppression lint adds the cross-language evasions `--passWithNoTests` and a trailing `exit 0` to the existing `|| true`/`--no-verify`/`--exit-zero` table.

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
roll-up, aggregate spend, tree budget, a Helm voyage tree, and a feed built from
`campaign-events.jsonl` plus each discovered sub-plan's `plan-events.jsonl`.
The tree flattens campaign -> sub-plan -> task/run state enough to show status,
gate progress, and spend without mandatory drill-in. `Enter` zooms into the
selected sub-plan by suspending the campaign TUI and calling the existing plan
attach loop; the plan loop can then zoom into a child run with the existing run
attach loop. Breadcrumbs include the campaign tier (`campaign <id> / sub-* / plan
<id> / task-*`), and `b`/Backspace returns through the nested contexts. Off-TTY
or `--plain` still prints the read-only sub rows + roll-up summary with the
explicit `deadreckon attach <sub-plan-id>` hint. `--json` emits a structured
campaign attach object with id, status, goal, tree budget, aggregate spend,
roll-up, and sub-plan rows.
`show <campaign-id> --why-failed` reports refused/caveat subs.
`campaign repair <campaign-id>` is state-changing and only accepts failed
campaigns; successful repair writes a new promoted campaign result run and marks
the campaign merged.
`kill <campaign-id>` cascades into each sub-plan via the existing plan-kill path,
then marks the campaign killed.

### 36.10 Current limits

Depth is capped at 2 and sub-goals are independent (no cross-sub dependency
edges). Helm ships the operator-facing flattened tree, but richer campaign
features such as cross-sub dependency edges, dynamic tree-budget reallocation,
cross-machine campaign sharing, and a long-lived attach daemon remain in
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
Notifications do not need their own daemon. Watchkeeper later added the
separate, opt-in per-user supervisor service described in §58.8.

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

### 37.11 Verdict trust surface

`deadreckon verdict` (§13.8) is the read-only complement to the gate: where the
gate decides whether a run may *become* a verified run at build time, `verdict`
asks whether it *still is* one — for any run, including imports from Claude Code,
Codex, or aider that never had a deadreckon marker. It is registered in the
friendliness contract (`FRIENDLINESS-AUDIT.md`) as a read-only inspect verb:
auto-detects the latest run (no prompt), refuses unknown/ambiguous ids with a
`try: deadreckon list` footer, renders through `VerdictSurface` with one state and
one primary next action (`finish` when verified-now, `resume` when regressed),
and carries `Inspect`/`Compare` secondary actions that `--quiet` suppresses.
`preview-before-mutate` and `one-command-rollback` are `n-a` because it never
mutates. The honest third state matters: a run whose marker was forged or whose
checks silently broke reads `REGRESSED`, never a false `VERIFIED`, so "did it
actually work?" can be answered against durable artifacts long after the run.

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

- `commands/chain/` owns the chain command family, characterization-only
  conductor entrypoints, the chain attach loop, and chain lifecycle verbs.
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
cover marker/proof writes and signature validation with seam sidecars present.
Strict Jobs also deny the external HMAC key; legacy-v1 tests retain
`gate/nonce` read coverage. Seam files do not alter signature inputs.

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

## 41. Attach TUI Uniformity

41.1 The shared dispatcher + per-mode hook (run is the reference)
`tui::navigation::NavigableSurface` + `dispatch_navigation` own the common
navigation keys (arrows/`jk`, `Tab`/`BackTab`, `PgUp`/`PgDn`, `Home`/`End`,
`g`/`G`). Run (`RunNav`), plan (`PlanNav`), campaign (`CampaignNav`), and chain
(`ChainNav`) each implement the movement hooks for their own content model and
supply `mode_key` for mode-specific keys. The run panel's semantics are the
reference; plan and campaign gained the paging keys they lacked, all by routing
through the one dispatcher.

41.2 One glyph, one footer, one scroll indicator
`selection_glyph()` is the single selection cursor (`>`) at every call site.
`footer(items)` is the single footer builder — a uniform `<keys> <label>` list
joined by one separator — replacing the four prior styles; the brittle
`parent_plan_footer` string-`replace()` hack is gone (the back affordance and
parent breadcrumb are structural, placed first so a narrow terminal cannot
truncate the exit affordance). `scroll_indicator(offset, rows, total)` (shared
with the run `panel_title`) shows a `first-last/total` window on every list
panel; the chain steps panel windows to that range and the plan/campaign
headers show the selected-item position. The narrative views are also windowed:
the run narrative panel uses `panel_title`, and the plan narrative panel uses
`plan_narrative_title` over a fixed `PLAN_NARRATIVE_AREA_HEIGHT` row — so an
overflowing plan narrative shows `plan narrative first-last/total` and scrolls
instead of silently clipping. In plan narrative view the shared nav keys drive a
`NarrativeScrollNav` (clamped to `total - visible_rows`) that scrolls the prose
rather than moving the task cursor.

41.3 Confirm-before-destructive and exit/return semantics
`resolve_completion_key` is a pure state machine: Apply and Abandon arm a
`pending_confirm` (the footer prompts `confirm <action>? y run / any other key
cancel`) and only `y` fires them, so a single mistyped key is harmless. Abandon
is `x` (leaving `b` unambiguously "back"); the dead `d`->Docs mapping is removed.
The "press Enter to return" prompts (`wait_for_return` + `return_key_dismisses`)
accept Enter/q/Esc/Backspace, and Enter on an unloadable child raises an
"unavailable" notice instead of a silent no-op.

41.4 Empty-state and Windows-glyph rules
Empty list panels carry a one-line next step and never print an internal log
filename (`CAMPAIGN_EMPTY_HINT`). Run and plan share one `NARRATIVE_SPLIT_WIDTH`
breakpoint. `chain_step_glyph(status, plain)` substitutes an ASCII fallback
(with an inline legend) under `--plain` / `NO_COLOR` / a non-VT or Windows
terminal, so a step glyph never renders as a missing box. The byte-exact TUI
render tests remain the contract; goldens were updated deliberately per phase.

## 42. Interaction Model (banner, smart bare invocation, prompt engine)

- **Banner**: `ui.rs` owns a figlet "standard" wordmark (`BANNER_ART`) rendered
  with a per-character 256-color horizontal gradient (`BANNER_PALETTES`,
  twelve palettes, picked from the clock's subsecond nanos per invocation).
  `ui::print_banner` prints only when stdout is a terminal, and colors only
  when the stdout color gate is open — pipes, `--plain`, `NO_COLOR`, and
  `TERM=dumb` stay byte-clean. Shown by `print_top_help`, `print_help_all`,
  and the smart bare-invocation routes; never by clap subcommand `--help`.
- **Smart bare invocation** (`smart_bare_invocation`, main.rs): no
  `config.toml` → `first_run_welcome` (detected agent CLIs from
  `KNOWN_AGENT_CLIS`, the three get-going commands, and an on-TTY confirm
  that runs `init` directly); config present but the current scope has no
  runs → `directory_orientation` (source-mode note, production flow,
  `list --all` / `doctor` pointers); runs in scope → `status_command`, the
  prior default. Pinned by `tests/smart_default.rs`.
- **Prompt engine** (`prompt.rs`): one API — `select_one` / `confirm` /
  `ask_number` / `open` — with two render paths. Interactive (stdin+stdout
  TTYs and `DEADRECKON_PROMPT_LINE_MODE` unset): `inquire` renders arrow-key
  selects (`label — detail` items, default preselected, 12-row paging),
  styled confirms, validated `CustomType` number input, and text prompts,
  themed via one `RenderConfig` derived from the Tone palette and colorless
  when the stdout gate is off. Esc resolves to a choice whose id is
  `"cancel"` when present, else errors `Interrupted` ("prompt cancelled"),
  preserving the pre-inquire contract. Line mode (off-TTY or the env var):
  the original numbered prompts, byte-stable for scripts and the PTY test
  harnesses (which pin `DEADRECKON_PROMPT_LINE_MODE=1` and send `\r`).
- **Probe-before-ask**: `prompt_provider` (init) builds its menu from the
  registry — detected subscription CLIs first with live login-state hints
  via `probe_cli_auth`, API routes annotated with whether their env key is
  set, `openai-compatible` and a typed route as escape hatches. The legacy
  stderr non-git menu was unified into `select_one` with the same copy.
- **Dependency**: `inquire 0.9.4` (Tier 2, DEPENDENCIES.md) — crossterm
  backend; it dual-links crossterm 0.28 beside the workspace 0.29, which is
  acceptable because prompts and the ratatui TUI never run concurrently.

## 43. Stable Readiness (models, rescue, durability, release gates)

### 43.1 Model catalogs and resolution order

Every provider descriptor (`crates/deadreckon-providers/descriptors/*.toml`)
carries a populated `[[model_catalog]]` with exactly one `recommended = true`
entry per built-in descriptor (pinned by depth test; custom descriptors may
omit a recommendation, and more than one fails parsing closed). Entries are
`ModelEntry { id, context_window, aliases, recommended, .. }` — `recommended`
is serde-default so pre-existing descriptors parse unchanged. Resolution is
one rule everywhere: per-role flag → generic `--model` → `defaults.model`
(config) → the catalog's recommended entry → provider default. "provider
default" is a real catalog entry meaning *no* `--model` argv reaches the
CLI — pinned by argv test.

### 43.2 The models verb and picker surfaces

`deadreckon models [PROVIDER] [--all] [--json]`
(`commands/providers.rs::models_command`) renders the catalog with the
recommended entry, the configured default (`deadreckon config model`), cost
and context-window notes; `--json` emits `{provider, configured_default,
models: [{id, recommended, context_window, aliases, default}]}`. Interactive
`start` offers a model picker (`prompt_start_model`, start.rs) immediately
after the provider choice — skipped when `--model` was given or the catalog
has fewer than two entries; the cursor defaults to `defaults.model`, else
the recommended entry; choosing "provider default" stores `None`. Launch
previews and the orchestrate provider-roles table echo the resolved model.

### 43.3 Per-role models in plan.json (additive)

`PlanProviders` gained five serde-default fields — `planner_model`,
`default_child_model`, `coder_model`, `reviewer_model`, and `child_models`
(BTreeMap<u32, String>) — populated only when the matching role provider is
set; a pre-rider plan.json fixture parses unchanged (compat test).
`PipelineState` is untouched. Flags: `--planner-model` / `--model` /
`--child-model IDX=MODEL` on `orchestrate full-plan`, `--coder-model` /
`--reviewer-model` on `orchestrate review`, `--planner-model` / `--model`
on `campaign run`, `--model` on `start`/`run`/`chain`. Child spawns append
`--model` argv via `child_model_for_task`, which filters "provider default"
and empty strings; campaign sub-launches forward `--planner-model`/`--model`
(argv-pinned in campaign_spawn_tests).

### 43.4 TTY rescue at refusal sites

`provider_setup_selection` (main.rs) is the single funnel for
`setup::select_provider_setup`; when a `require_usable_route` request
refuses with a rescueable message ("needs credentials" / "not logged in")
and `prompt::is_tty()` holds (stdin+stdout TTYs — deliberately independent
of line mode so PTY tests drive it), the refusal becomes a
probe-before-ask picker: detected CLIs with live login hints, a "keep"
choice carrying the refusal's `try:` line, and cancel — keep/cancel
reproduce today's refusal verbatim. One rescue per launch; the retried
selection pins `explicit_provider` to the picked route. Off-TTY refusals
are byte-identical (pinned). All twenty call sites inherit the rescue with
zero per-site changes.

### 43.5 history.json reconstruction + atomic save; lock reclaim rules

`load_or_reconstruct_history` (turn_loop.rs) treats any history.json read
or parse failure like a missing file: warn on stderr (the flight recorder
is not open yet), rebuild from `traces.jsonl`, advance `state.turn` to the
last complete turn if ahead, and atomically re-save. `save_history` writes
through tempfile + rename (same pattern as `state.rs::atomic_write_json`),
pinned by an inode-change test. Lock reclaim (`lock.rs::acquire_lock`) now
requires a dead holder pid: an alive holder is never usurped however stale
its heartbeat (staleness is advisory, for `lock_status` displays only),
and the `LockHeld` refusal names the heartbeat age with
`deadreckon kill <run-id> --force` as the operator escape hatch.

### 43.6 Stable-lane gates and the operator checklist

`CHANGELOG.md` carries the `## 0.1.0` section, so
`release-trust.mjs validate --ref refs/tags/v0.1.0` fails only on the
cut-time version bumps; depth tests pin the lane asymmetry (stable
requires the changelog section and the npm-wrapper version match, rc
requires neither). `dist-workspace.toml` pins `checksum = "sha256"`;
the inner installer's embedded-sum verification for tar.xz is a
V1-CANDIDATES upgrade path, with `release/install.sh`'s SHA256SUMS
die-on-mismatch as the shipped integrity story (pinned).
`release/preflight-real.sh` (POSIX sh, refuses under CI) is the
operator-run stable-cut proof: per route, a real start to completion,
signed `turn-acceptance.json`, `apply`, then kill/resume — recording
binary versions in `release/known-good-providers.json` (schema_version 1).
`docs/RELEASE.md` holds the one-time "Stable v0.1.0 operator checklist"
(tap repo + token, npm trusted publishing, Windows Authenticode or a
consciously narrowed lane, version bumps, preflight, Windows smoke, tag).

## 44. Live Narrator (one rolling story, written live, rendered everywhere)

44.1 **Sidecar architecture.** A `dr run` now spawns an in-process narrator task. `run.rs` resolves a `NarratorConfig` from the surface (TTY on by default; `--narrate` opts in headless; `--no-narrate` disables; `--narrator-model` pins the model), builds a `RunEventBus` whose sender feeds `RunLoopConfig.event_sender`, and spawns `NarratorEngine` (`crates/deadreckon/src/narrator.rs`) subscribed to the bus. The engine reacts to per-turn `DocsCheckpoint`, reads the rich `TurnRecord`, and appends beats to `<run_root>/narrative/snapshots.jsonl`. The task flushes a final beat and stops on run completion or cancellation.

44.2 **Continuity.** Each beat feeds the model the prior narrative + only the windowed new turns + a carried rolling summary and asks it to amend/extend (`build_live_narrator_prompt` / `apply_live_narrator_response` in `narrative.rs`; voice in `skills/live-narrator/`). `NarratorWindow` carries only turns since the last beat; the rolling summary is bounded to 1200 chars, so per-beat input is a constant ceiling and total cost is O(turns), not O(turns²). Beats append to the schema-2 `snapshots.jsonl` (the `live` field carries `beat_seq`, `covers_turn`, `source`, `rolling_summary`); the prior beat is never overwritten.

44.3 **Subscription-first backend.** `select_narrator_route` prefers a free, logged-in CLI (`cli:claude-code --model haiku` → `cli:codex --model gpt-5.1-codex-mini`) then a cheap API model (`anthropic claude-haiku-4-5` → `openai gpt-4o-mini`) then the deterministic floor, gating CLIs on binary presence + a non-logged-out auth probe. The narrator builds its own `ProviderRouter`; the run's router stays on the big model. The deterministic projection remains the floor, so narration always works with no provider.

44.4 **Cadence.** `cadence_decision` is time-gated + coalesced — a beat fires on new work past the min gap or a turn burst, faster bursts coalesce, total beats are capped per run, and a long single turn escalates via the quiet timer. Between model beats a deterministic $0 ticker (`turn N · tool (elapsed)`) keeps a long turn from looking frozen.

44.5 **Surfaces.** Foreground: a calm few-lines-max block redrawn in place (`ForegroundBlock`; cursor control via `ui::cursor_clear_lines`). Headless `--narrate`: append-only, turn-stamped beats to stderr; stdout stays clean. Attach renders the live beats the run already wrote, with no provider call. (`effective_plain` — auto-plain when stdout is not a TTY — is unit-tested but intentionally **unwired**: the project renders rich box-drawing when piped, so auto-plain-on-pipe broke fixtures and was reverted; see `docs/V1-CANDIDATES.md`.)

44.6 **Spend isolation.** `NarratorLedger` tracks narrator spend against its own cap and degrades to the floor at the cap; `kind: "narrator"` `spend.jsonl` rows keep the run's spend math unaffected. Subscription backends record $0. (The "spend math ignores non-`loop` rows" guarantee was only made real in §45.7 — before that, `spend_summary` summed every row and a narrating run inflated its own totals.)

44.7 **Post-hoc convergence.** `complete_run_docs` seeds `current_narrative` from `live_narrative_digest` — the full accumulated beat history — so `RUN-NARRATIVE.md` consolidates the live story rather than re-deriving from the raw trace.

44.8 **Deferred.** Cadence/budget knobs use the rider defaults baked into `NarratorConfig`; reading them from `[defaults]` in `config.toml`, a persistent streaming CLI session, and a shared cross-surface daemon remain V1 candidates.

## 45. Orchestrated Narration (every child narrates; parent aggregates; campaign view)

The Live Narrator (§44) worked for a `dr run` only. Orchestrate/campaign children are **subprocesses** (`deadreckon run` / `deadreckon extend`), not in-process loops, so they got zero live beats: `run` children were gated on `io::stdin().is_terminal()` (a piped child returns `None`), and `extend` children re-entered `lifecycle.rs` with `event_sender: None` hardcoded. This section closes that gap.

45.1 **File-only child narration.** `resolve_narrator_config_for_child` (`narrator.rs`) returns a `NarratorConfig` with `foreground: false` AND `headless_append: false` — beats append to the child's own `<run_root>/narrative/snapshots.jsonl` but never touch the child's stdout (the parent scrapes the run-id via `parse_started_run_id`) or stderr (the parent captures failure summaries). The child path activates when the parent sets `DEADRECKON_NARRATE_CHILD=1` (`NARRATE_CHILD_ENV`); `resolve_narration(is_child, is_tty, …)` dispatches child vs. TTY. `run.rs` and both `extend` paths share one `build_run_narration` helper, so the shutdown-ordering contract (shut the narrator down after the awaited loop, before `child_pids.clear()`/`save_state`/lock release) lives in one place.

45.2 **Default child backend = deterministic floor.** Children narrate on the $0 `DeterministicFloor` unless `--narrator-model` is explicitly pinned (`child_narrator_backend_is_floor`). This avoids an N-children × `probe_cli_auth` storm and an N × per-child-budget dollar blast radius. The parent resolves the backend once and threads `--narrator-model` down; when narrating, child env also carries `DEADRECKON_AUTH_PROBE=0`.

45.3 **Flag parity + propagation.** `dr orchestrate` and `dr campaign` gain `--narrate/--no-narrate/--narrator-model`. `fork_command` threads them into each child's argv via the pure `child_argv` builder (`run_plan_child` appends `--narrate`/`--narrator-model`); a campaign appends them to the `orchestrate full-plan` sub-orchestrator argv (`build_sub_orchestrator_command`), so the campaign → orchestrate → run/extend chain narrates end to end. Both `extend` paths (in-place + worktree) are wired.

45.4 **Plan-attach surfacing reliability.** The per-child agent table caps at `PLAN_AGENT_TABLE_MAX` rows with a `+N more` overflow line. `latest_child_narrative_snapshot` reads via `read_latest_live_snapshot`, which prefers the latest `source: Live` beat over a later attach-time `Deterministic` projection — so an on-demand attach refresh can never mask a child's live headline (the prior `read_latest_snapshot` returned only the last row).

45.5 **Parent aggregate stderr line (Option D1).** When `dr orchestrate --narrate` is active, the parent tails each running child's `snapshots.jsonl` (reusing `plan_event_bus::JsonlTail`) and prints one capped line per active child to STDERR every ~2s, preferring each child's latest Live beat (`emit_parent_aggregate`). The aggregate is routed only to stderr; an `out`/`err` sink pair makes "never writes to stdout" a tested invariant.

45.6 **Campaign Narrative view (Option D2).** `build_campaign_projection` / `ensure_campaign_projection` (a new `Campaign` `NarrativeScope` variant) aggregate each sub-goal's freshest child narration — its merged run's live beat, else its sub-plan's snapshot — into an agent table, then render through the **same** `narrative_plain_lines` as a plan. `dr attach <campaign-id> --view narrative` (and `--json`) surface it at full section parity with plan attach.

45.7 **Spend-math fix.** `spend_summary` (`state.rs`) now counts only `kind: "loop"` rows and takes `total_usd` from the last loop row. Before this, every interactive run that narrated inflated its own tokens/turns/wall and could overwrite `total_usd` with a `kind: "narrator"` row — the latent leak referenced in §44.6.

45.8 **Deferred.** Wiring `effective_plain` (auto-plain-on-pipe), a parent aggregate for campaign at the orchestrate level (campaign currently relies on each sub-orchestrator's own aggregate), and a provider-backed campaign narrative graph remain V1 candidates.

---

## 46. Course: Launch Planning and Reshaping

### 46.1 Mental model

Course makes launch shape the planner's job, not the operator's — the same
inversion a database query planner performs. The goal is the query; the
detected done contract is the schema plus assertions; `start` is the planner;
the course card is EXPLAIN; the existing run/plan/campaign/chain machinery is
the executor; collapse and reshape are adaptive re-planning. Course invents no
execution engine: it decides and records *which existing engine runs, with
what pieces, under what money*. Everything lives in
`crates/deadreckon/src/commands/course.rs` plus a bounded turn-loop seam.

### 46.2 The SignalBundle

`collect_signal_bundle` computes five deterministic signals before any
provider call, and the bundle embeds verbatim into the launch plan as the
audit of what the decision saw:

- `analyze_goal_structure` — enumerated items, conjunction clauses, leading
  imperative verbs → a `strong` decomposability verdict.
- `contract_signal` — the Polyglot detector (§13.1) as a launch signal, so
  `start` and the gate agree on what "done" will mean before any spend.
- `scan_workspace` — Cargo/pnpm/npm-workspaces/go.work members (a parallelism
  map) plus a capped tree-size bucket.
- `history_signal` — prior runs by task key; verified history is the
  continuation signal.
- `budget_signal` — plan/campaign feasibility floors; a shape the money
  cannot fund is never proposed.

All five are pure, total, and provider-free; degraded inputs yield defaults,
never errors.

Source selection precedes this planning work. `start` canonicalizes one
`ResolvedStartSource` before a classifier or authoring provider can run. The
decision holds the durable source mode, requested provenance, inspection root,
contract-writer root and dirty-source posture. Unsupported or unreadable
source inputs therefore refuse before provider spend or filesystem mutation.
Course preview, contract authoring, Job authority and dispatch receive this
same value rather than resolving paths independently.

### 46.3 The deterministic ladder

`ladder_decision` resolves a shape from ordered rules; the first match
decides and every decision records which rule fired: (1) verified same-task
history → chain-extend continuation; (2) budget below the plan floor →
single; (2.5) explicit parallel-workstream keywords → plan (the proven
pre-Course heuristic, owned here so `start_goal_recommends_full_plan` cannot
drift); (3) strong decomposition + ≥2 workspace members → plan with n clamped
to members and 6; (4) strong decomposition alone → plan clamped to 4;
(default) single. **Campaign is structurally unreachable from the ladder** —
deterministic campaign selection is a spend hazard; campaign requires the
provider planner or the operator. A grid depth test sweeps goals × members ×
ceilings to keep it unreachable.

### 46.4 The provider planner

`classify_goal_shape_for_start` superseded the old text-only classifier: one
bounded call (5s timeout, 512 tokens, read-only prompt) whose prompt embeds
the SignalBundle, and whose typed draft (`shape`, `n`, `pieces` with per-piece
goals and done hints, `confidence`, `rationale`) is clamped by
`resolve_provider_course_plan`: confidence below the floor downgrades a
disagreeing shape to the ladder; budget-infeasible shapes downgrade; n and
pieces clamp into 2..=6; a one-piece plan collapses to single (46.10). Every
clamp is recorded in `clamps_applied`. Any parse miss falls back silently to
the ladder — a planner can never fail or stall a launch.

### 46.5 `launch-plan.json`

The durable decision artifact (schema 1), written into every dispatched root:
goal, shape (`single`/`plan`/`campaign`/`chain-extend`), pieces, n, per-role
providers, budget ceiling/split, contract summary with provenance
(`detected`/`operator`/`inferred`/`asked`/`none`), the embedded SignalBundle,
resolution (source `provider`/`ladder`/`operator`/`replay`, confidence,
rationale, clamp trail), escape hatches, `accepted_by`, and `parent` (set on
reshape lineage). Serde is additive-tolerant; an unknown schema refuses with
a `try:` footer. Direct verbs (`deadreckon run`) record a trivial operator
plan so every root carries the decision record however the launch began.
Saves are best-effort — a read-only filesystem cannot fail a launch.

Helm reads `launch-plan.json` as the authoritative budget ceiling and launch
shape label for the status spine and voyage labels when present. Older roots
without the file keep the inferred fallback; attach never writes the artifact.

### 46.6 The accept matrix

`accept_policy` weighs TTY × yes × confidence × ceiling × shape. Campaign
above the confirm line (default $25, unbounded counts as above) ALWAYS
confirms interactively or refuses in non-TTY — no flag overrides the
guardrail. `--yes` auto-accepts only when confidence clears the floor
(default 0.7) and the ceiling is under the auto-spend line (default $20).
Non-TTY without `--yes` refuses with `try:` instead of hanging. The asymmetry
is deliberate: wrong-single costs a retry; wrong-campaign costs real money.

### 46.7 The course card

`course_card` renders goal/shape/pieces/who/cost/done/why/escape through the
shared Card primitives — done contract and escape hatches never omitted — and
the 80-column plain layout is golden-pinned (whitespace is spec).
`prompt_course_card` drives sail/edit/force-single/abort through the
`StartPrompter` seam; forcing single collapses the plan and records the
operator override in the clamp trail.

### 46.8 The one-question flow

With no operator `acceptance.yaml`, a Polyglot-detected contract answers
"done" with zero questions (source `detected`, DefaultGate action — the gate
compiles the same detected default at run time). An unknown tree asks exactly
one question — "How will you know it worked?" — whose one-line answer
compiles through the existing def-done flow (source `asked`). Pressing Enter
asks the agent to draft practical criteria from the launch goal. `--yes` can
approve resolved criteria but cannot invent them: an unknown tree refuses
before Job creation and prints a `def-done` recovery command. The same strict
admission check rejects a contract whose only required check proves that the
pre-created working directory exists. Non-TTY execution without a contract
also refuses. The old four-choice done menu is gone.

For a source-copy launch, that agent does not inspect the empty destination.
It receives a deterministic, bounded dossier from the resolved source, while
the generated YAML, Markdown and helpers are written under the launch
project's `.deadreckon/` directory. Generated checks must use
`{working_dir}`; an absolute reference to the operator source is rejected.

### 46.9 Dispatch, replay, JSON parity

`dispatch_start_command` consumes the built plan and lands it in the
dispatched root (run root at creation via `run_command_with_launch_plan`,
plan/campaign dir on launch, extend target on success). `start --plan <file>`
replays a saved plan: schema-validated, re-clamped against `--max-spend` (a
plan whose budget exceeds the cap refuses naming both numbers), stamped
`replay`, identical shape/n dispatched; chain-extend plans refuse (they need
their parent). `start --json --yes` launches quietly and emits one machine
envelope `{kind:"launch", plan, dispatched ids, next_actions}`; `--json`
without `--yes` keeps the read-only preview. `start` gains `--max-spend`,
threaded into every dispatch arm.

### 46.10 Course correction: collapse and reshape

De-escalation: a decomposition of exactly one piece collapses to a single run
(recorded in the clamp trail) instead of inflating to n=2 or refusing.
Escalation: the turn loop's additive `reshape` action lets a worker propose
2–6 independent pieces; the loop records an INERT `reshape-proposal.json`
(launch-plan schema, `parent` set, no acceptance), a `reshape.proposed`
trace, and keeps working — non-terminal, never self-executing. `deadreckon
reshape <id>` previews the proposal on the course card and, only on explicit
acceptance (card sail or `--yes`; non-TTY refuses with `try:`), dispatches a
full-plan orchestration with the parent run recorded in the dispatched
plan's `launch-plan.json`.

Helm surfaces pending reshape proposals as attention/next-action context and as
timeline marks sourced from the `reshape.proposed` trace. A proposal is inert,
not a failure or pause, so it does not appear as a why-cause.

### 46.11 Start-then-watch and configuration

`[defaults] start_attach = true` drops an interactive launch straight into
attach after the lifecycle footer; JSON, quiet, preview, and non-TTY sessions
never auto-attach, and a failed attach cannot turn a successful launch into
an error. That makes Helm the default post-launch watch surface for interactive
launches without changing JSON or script behavior. Guardrail knobs
(`shape_confidence_floor`, auto-spend ceiling,
campaign confirm line) ship as compiled defaults in `course.rs`; config keys
for them are a follow-up (V1-CANDIDATES).

### 46.12 What stayed out

Auto-reshape without operator accept, campaign-level reshaping, piece-goal
seeding into dispatched plan tasks, learned shape priors, cross-machine
plans, chain-extend replay, and multi-contract monorepo planning are logged
in V1-CANDIDATES, not silently expanded.

---

## 47. Helm: mission-control attach

Helm is the stable attach slice that makes run, plan, chain, and campaign attach
answer the same operator questions in the same frame. It stays on ratatui 0.29
and adds no durable state schema: every pane is a read model over existing
`state.json`, run events, plan/campaign/chain events, spend, traces, flight
checkpoints, proofs, docs, `launch-plan.json`, and reshape proposal artifacts.

### 47.1 Status Spine Contract

The spine is a five-question table enforced in code (`tui::spine`). Every attach
surface computes non-placeholder answers:

| Question | Run | Plan | Chain | Campaign |
|---|---|---|---|---|
| Alive? | event age/status | plan event age/status | chain event age/status | campaign event age/status |
| Doing what? | current turn/tool/status | selected task/merge state | selected step/conductor state | selected sub/roll-up state |
| On track? | gate, spend, turns | task gate/spend/budget | step gate/spend/budget | aggregate gate/spend/tree budget |
| Anything wrong? | gate/tamper/provider/cap attention | failed/blocked child and repair state | paused/failed step or hook state | refused/caveat sub or merge repair state |
| What next? | one lifecycle command | fork/merge/attach/why command | resume/show/undo command | repair/attach/show command |

The band renders in the frame and plain attach summaries print the same five
answers. Pending Course reshape proposals are attention/next-action entries, not
pause/failure state.

### 47.2 Voyage Tree

`tui::tree` builds a bounded `TreeModel` from existing durable files. Runs
collapse to a one-node header, plans show tasks, chains show steps, and campaigns
show campaign -> sub-plan -> task/run state up to the existing depth cap. Nodes
carry status glyphs, gate progress, spend, and display-width-safe labels.
Selection drives the detail pane; `Enter` zooms into the selected node when a
deeper surface exists, but the tree is sufficient to understand current state.
Event folds update node status without rebuilding the tree.

### 47.3 Event Loop and Latency

Attach loops use crossterm `EventStream` plus durable tail wakeups/adaptive
backoff instead of a fixed 250ms poll. `AttachTickTiming` records load, JSONL
tailing, draw, input poll, provider refresh polling, and `InputToFrame`; the
budget tests pin input responsiveness and event-storm coalescing. The
input-to-frame budget defaults to 32ms and is operator-tunable via
`[ui] input_latency_budget_ms` (clamped to 8..=1000ms). Render paths
stay provider-free and write-free. Background narrative refresh jobs are polled
between frames and can improve a later frame, never block the current one.

### 47.4 Controls, Help, and Input

The shared navigation core owns pane focus, scroll, paging, zoom/back, and
detail toggles. `?` renders sectioned per-surface help, and footers lead with
the focused context while preserving detach on narrow terminals. A first-session
cue points at panes, why, and command mode.

Chain attach owns the general in-frame modal/input path: kill confirm, extend
input, and `:` command mode are ratatui modals backed by `tui-textarea`. The
extend modal dispatches to the public non-mutating refusal, which reports the
updated durable schedule rather than modifying the stored chain.
Its fixed table contains the existing chain verbs only (`attach`, `kill`,
`motion`, `q`, `reshape`, `resume`, `verdict`, `why`), with confirm preserved
for dispatching/destructive paths and nearest-match `try:` guidance for unknown
commands. Rudder (§51) adds a separate run-only `:steer <instruction>` modal.
Plan, campaign and chain surfaces do not advertise or accept `:steer`.

### 47.5 Why and Timeline

`tui::why` classifies deterministic cited causes from existing artifacts:
state pause/failure reasons, gate progress/proofs, tamper verdicts, provider
errors, cancel markers, and cap events. The why panel never renders an uncited
cause; a node with no failure artifacts says nothing wrong is recorded and
points back to narrative/activity.

`tui::timeline` builds a scrubable turn timeline from docs checkpoints,
flight checkpoints, spend rows, proof files, and reshape traces. `t` focuses
the band, Left/Right scrub entries, and the detail pane shows the selected turn
story and diff counts. Timeline marks are read-side annotations: GatePass,
GateFail, TamperCaveat, Reshape, and Checkpoint.

### 47.6 Motion Policy

Helm effects are decoration, never information. `[ui] motion =
full|reduced|off` resolves through config defaults; reduced is the non-TTY/replay
default and keeps only completion effects, while off renders zero effect frames.
The tachyonfx-backed registry has exactly three triggers: gate pass shimmer,
verdict/completion flash, and node-state glyph pulse. All are bounded under
800ms, input-preemptible, and every state they acknowledge is also visible
statically.

### 47.7 Dependencies and Deferrals

Tier-2 widget/effect crates are logged in `DEPENDENCIES.md`: `tui-tree-widget`
for the voyage pane, `tui-textarea` for in-frame input, and `tachyonfx` for the
bounded effects layer. Deferred follow-ups remain explicit in
`docs/V1-CANDIDATES.md`: a long-lived attach daemon, ratzilla/web mirror,
provider pty embedding, cross-machine attach, replay-with-original-timing, and
broader text input/search surfaces.

---

## 48. Contract: Goal-Aware, Execution-Oriented Done Criteria

Contract makes the definition of done a compiled read model before launch,
without changing the durable acceptance schema. The persisted artifacts remain
the Polyglot gate files from §13.1 and §35: `.deadreckon/acceptance.yaml`,
`.deadreckon/acceptance.md`, and helper scripts under
`.deadreckon/acceptance/`. `CompiledContract` is a projection over those files:
each check gets a stable summary, one of the existing check kinds
(`file_exists`, `content_match`, `shell`, `cargo_test`), a deterministic
`behavioral` label, a deterministic `can_fail` label, and the raw YAML node.

The acceptance compiler prompt now receives the run goal. `deadreckon
acceptance draft --goal <text>` exposes that directly, and `deadreckon start`
passes the actual launch goal when it materializes done criteria. The prompt
requires contracts to derive from the goal plus the user's request, prefer
checks that build/start/drive/assert or use known input -> known output tests,
treat source-text scanning as insufficient as the sole substantive check, make
every substantive check falsifiable, and avoid `--if-present` as the only
build/test gate. The anti-self-attestation rule stays in force.

The deterministic falsifiability lint is the floor. It flags contracts with no
behavioral checks, source-scan-only substantive gates, `--if-present`-only
build/test checks, and other unfalsifiable substantive checks. One critic pass
can run after a provider draft. It receives the goal, compiled contract and lint
findings, then can request at most one automatic redraft. If the critic is
unavailable, the lint floor still surfaces.

`start` reconciles the run goal against the compiled contract before launch.
The deterministic reconciliation splits the goal into clauses and reports
clauses whose salient terms appear in no check summary or raw node. This is a
drift signal, not a semantic proof, and it is surfaced with lint findings as
contract divergence. Under `--yes`, strong divergence refuses with a `try:`
path to review the contract instead of silently launching.

The human review path renders the real compiled checks, not just
`project (N checks)`. Existing done criteria can be accepted, viewed, checked,
re-prompted, or edited through the start review seam. The Course launch plan
from §46 now records compiled checks and divergence in its contract section;
the Course card lists `done 1`, `done 2`, etc., flags drift, and adds `d` as
the done-review action. `start --json` includes the same checks and divergence
in preview and launch envelopes.

### 48.1 Source-true, bounded contract authoring

Soundings keeps the persisted acceptance schema unchanged but tightens the
pre-launch controller. Guided authoring receives explicit writer and inspection
roots. Existing contracts are discovered at the writer root. New artifacts are
written there, while a deterministic dossier describes the resolved source.
The dossier has stable ordering and hard per-file/total caps; it exposes useful
manifest, source and test names while excluding `.git`, SpecStory history,
secrets, symlinks, runtime state and rebuildable output. Swift packages expose
their real product, target and test names, so a Cloudwing source cannot be
silently rewritten as an invented FlappyBird package.

Draft, critic and optional redraft use exact JSON output schemas and a
request-scoped structured-text-only posture. Codex runs ephemerally with tool,
MCP, web and user-rule/config surfaces disabled; Claude uses its equivalent
schema-only flags. API requests carry strict response formats and no tools. An
adapter that cannot enforce the posture fails closed. Capability probes are
cached for the binary/version process lifetime. `doc_provider` remains the
preferred authoring route.

The whole authoring sequence shares one wall deadline. The default is 120
seconds, configurable as `defaults.done_contract_max_wall_seconds` and clamped
to 30–600 seconds. The initial draft receives at most 60 seconds, the critic at
most 20, and redraft receives only the remaining time up to 60 seconds. Startup,
capability probing, provider work and cleanup all consume the same budget.
Cancellation first terminates and reaps the provider's full owned process group
and removes temporary schema, PID and partial-output files. The wait surface
reports stage, provider/model and cumulative elapsed/limit; it is not the
timeout mechanism.

The deterministic lint remains the floor. A provider cannot override a
deterministic `redraft`. Critic timeout can expose a lint-clean candidate only
for explicit human review; strict/non-interactive admission refuses. Redraft
receives the complete prior YAML, Markdown, helper bodies, dossier, lint and
normalized verdict. `reject` is an alias for `redraft` and retains its arrays.
There is still at most one critic and one redraft. A timed-out or invalid weak
candidate is never written as approved. A valid generated contract already on
disk is ordinary project state and is reused on retry without another provider
call.

Deferred Contract work stays out of the stable slice: first-class browser/HTTP
check kinds, a standalone `deadreckon contract` report verb, multi-round critic
repair, per-check provenance, semantic/embedding coverage, and auto-generating
missing project build/test harnesses are tracked in `docs/V1-CANDIDATES.md`.

---

## 49. Logbook: stable run inspection

Logbook makes post-run inspection a shared read model instead of several
commands re-parsing nearby files differently. `deadreckon_core::RunView` loads
a run's `state.json` and joins the surrounding durable artifacts into one
projection: identity, goal/status, verdict/signature, sandbox facts,
spend/wall-clock totals, full-run changed files, narrative/decision docs,
per-turn records, proof files, and missing artifacts. Missing optional files are
recorded in the projection rather than turning inspection into a panic path.
Since Keel (§52), RunView is a projection over one protocol vocabulary: its
event, spend, and trace inputs use the canonical `deadreckon-protocol` types,
while attach, history, verdict, docs, and flight inspection use those same
types rather than local copies. `PipelineState` remains application state and
is deliberately outside that wire vocabulary.

Changed-file evidence comes from snapshot diffs. `DiffSummary` and `FileDelta`
compare `snapshots/turn-{n}` directories, ignore `.git`, `target`, and
`.deadreckon`, count additions/removals for text files, and tolerate binary or
unreadable files without crashing. `RunView` uses the same primitive for the
full run (`turn-0` to current turn) and for every per-turn view.

The CLI surfaces are now projections over that same model:

- `deadreckon show <run> --diff` prints or JSON-renders the full-run snapshot
  diff.
- `deadreckon show <run> --turn <n>` prints or JSON-renders the turn's did/diff,
  model-exchange reference, sandbox events, spend delta, and final check
  outcome when present.
- `deadreckon show <run> --raw <artifact>` dumps stable run artifacts verbatim
  and refuses protected gate secrets, including the legacy-v1 nonce, with a
  `verdict` hint; its help points to the checked ledger schemas under
  `docs/schemas/*.schema.json`.
- `deadreckon report <run>` writes a static Markdown report by default, or
  self-contained HTML with `--html`, and JSON with `--json`; live/pending runs
  refuse with an attach command. The JSON projection is checked against
  `docs/schemas/projections/run-view.schema.json`.
- `deadreckon history grep --kind events` searches the durable run event ledger
  alongside the existing trace and provenance ledgers.

`verdict` derives marker presence/validity and changed-file counts from
`RunView` while still re-running acceptance checks live before deciding
Verified/Regressed/Unverified. `doc` resolves run narrative and decision docs
through the same view after optional polish. Attach narrative inputs carry an
optional `RunView`, so Helm can use the same snapshot diff facts when it builds
run narrative and architecture evidence.

Logbook closes the static-run-report slice of the old C3 observability/UI gap
and the read-side introspection mismatch between `show`, `verdict`, docs, and
attach. It does not close cross-run efficiency dashboards, CLI-provider context
telemetry, a web/desktop live mirror, syntax-highlighted diff browsing, or MCP
exposure; those remain V1 candidates.

## 50. Semaphore: The CLI Agent Wire Contracts

Before Semaphore the two workhorse CLI drivers (`cli:codex`, `cli:claude-code`)
scraped raw stdout as the whole response, hardcoded `usage: 0/0`, and started a
fresh conversation every turn. Both tools already published structured
contracts deadreckon ignored. Semaphore reads them.

**Shared machinery, two thin mirrors.** `deadreckon-providers::cli_contract`
holds everything provider-neutral: the `CliStreamEvent` vocabulary
(conversation id, usage, answer, tool row, failure, recognized, unknown), the
tolerant JSONL fold (`parse_stream`), degraded-mode detection, the per-run
`provider-session.json` record, live flight-row extraction, and the resume
helpers. Two mirror modules translate each binary's JSONL into that vocabulary
and nothing more: `codex_events` (wire per codex-rs `exec_events.rs`) and
`claude_events` (wire per fixtures recorded from the real binary, checked into
`crates/deadreckon-providers/tests/fixtures/semaphore/`). No codex/claude
specifics leak outside the two mirror modules. Pennant uses the same machinery
for descriptor TOML in §55 without a refactor.

**Feature detection, not version pinning.** Each binary is probed once per
process from `--help` (`codex exec --help`, `claude --help`) and the capability
set is cached. Absent flags disable the corresponding behavior with a caveat —
a binary that predates the structured flags keeps working, degrading to the old
raw-stdout path.

**Tolerant parsing is the law.** Unknown `type` tags become
`CliStreamEvent::Unknown` (counted, preserved, never fatal). A line that is not
JSON at all is counted as garbage and skipped. Output that yields no structured
event at all degrades to raw stdout and appends a `provider.contract.degraded`
caveat to the turn trace instead of failing the turn; the turn loop raises that
caveat on the attention channel (events.jsonl) so it isn't silently swallowed.

**Per-run conversation resume.** The conversation id is a file, not a
`PipelineState` field: `<run_root>/provider-session.json` (schema 1, provider,
conversation_id, created_at, last_turn_at, resume_failures). It is
provider-scoped — a run whose provider changes mid-life (rescue) ignores a
session recorded by a different provider name. Turn 1 persists the id; later
turns resume (`codex exec resume <id>` / `claude --resume <id>`). A resume that
exits nonzero with a session-not-found signature increments `resume_failures`,
retries once fresh, and records a `provider.session.reset` caveat;
`resume_failures >= 1` forces a fresh conversation next turn. Resume never
crosses runs or providers. `deadreckon show <run> --raw provider-session` dumps
the record.

**Real tokens; dollars unchanged.** Usage comes from `turn.completed.usage`
(codex) and `result.usage` (claude) into a real `ProviderUsage`, which flows
into the spend ledger per turn and renders on the subscription surface.
`SpendEstimate` stays `subscription: true, cost_usd: 0.0`; claude's reported
`total_cost_usd` is recorded in the turn trace detail as informational only —
billing semantics for subscription CLIs are out of scope.

**Answers from the structured result.** codex reads the final message from the
`--output-last-message` file; claude reads `result.result`. Raw stdout is used
only in degraded mode.

**Live flight ingestion (§33).** Tool/item events parsed from the stream are
carried on the response trace as `trace.flight_rows` and ingested into the
flight ledger during the turn. A descriptor `[ingest] live_contract` flag tells
the recorder the driver owns tool-row ingestion, so the post-hoc file scraper
yields for that provider — the two never double-count. Cross-reference §33
(Provider Flight Recorder & Rewind).

**Schema-constrained output.** `ProviderRequest.output_schema` becomes codex
`--output-schema <file>` where the binary is probed capable (§46 planner);
claude and incapable codex binaries record a caveat and proceed unconstrained.
Claude's own `--json-schema` is left for a follow-up — the probe is
forward-ready.

Per-provider contract table:

| | cli:codex | cli:claude-code |
|---|---|---|
| stream flag | `--json` | `--output-format stream-json --verbose` |
| conversation id | `thread.started.thread_id` | `system(init)/result.session_id` |
| resume | `exec resume <id>` (no `--sandbox`) | `--resume <id>` |
| usage | `turn.completed.usage` | `result.usage` |
| answer | `--output-last-message <file>` | `result.result` |
| failure | `turn.failed` / `error` | `result.is_error == true` |
| output schema | `--output-schema <file>` | caveat (not wired) |

Semaphore does not touch the app-server route, steering, interrupts, or
approvals. Rudder layers those controls onto the per-run session foundation in
§51. Semaphore only kept the machinery contract-shaped for the generic fleet;
Pennant adds descriptor contracts in §55.

## 51. Rudder: Steering the Running Child

Rudder adds an explicit `cli:codex-server` route for operators who need to
change the direction of a live Codex run. The route is opt-in and is not added
to the default fallback order. It reuses Semaphore's per-run sidecar (§50), the
existing sandbox and launch-plan capabilities, and Helm's run attach surface
(§47). It adds no `PipelineState` field.

### 51.1 Connection and thread model

The route starts `codex app-server` as a supervised child and speaks newline-
delimited JSON-RPC over stdin and stdout. It completes the
`initialize`/`initialized` handshake, then uses `thread/start` for a new run or
`thread/resume` when the run already has a Codex thread. Each provider instance
keeps one child for its turns; there is no shared daemon, socket or cross-run
server pool.

`provider-session.json` stays at schema 1. For this route its existing
`conversation_id` stores the Codex thread ID, while additive optional fields
record `route`, `server_pid` and `active_turn_id`. Writes remain atomic. The
active turn is set after `turn/start` and cleared after completion or failure,
which gives kill a durable way to tell whether protocol interruption is
possible. Child PIDs also use the existing supervised PID directory. Unknown
server notifications are recorded in the response trace and do not fail a
turn.

### 51.2 Durable steering inbox

Both `deadreckon steer <run-id> "instruction"` and the run-only Helm command
`:steer <instruction>` validate that the run is executing on
`cli:codex-server`. They append an entry to `<run_root>/steer-inbox.jsonl` with
timestamp, source (`cli` or `tui`), text and `pending` status. The file is an
append-only ledger. Reading it folds updates by timestamp and text to recover
the effective pending or delivered state after a restart.

Delivery is at-least-once. The provider sends pending entries with
`turn/steer`, including the current `expectedTurnId`, once after `turn/start`
and again while polling the live turn. It appends the `delivered` update only
after the server returns the same turn ID. A stale-turn precondition or process
loss leaves the entry pending so a later valid turn can retry it; neither path
silently drops the instruction.

Plain attach prints one `steer pending` or `steer delivered` line per effective
entry. Pending entries also appear in the run status spine's attention answer.
The `:steer` footer and help entry exist only on run attach; §47's chain command
table is unchanged.

### 51.3 Capability-answered approvals

The app-server thread runs with `sandbox = workspace-write` and
`approvalPolicy = on-request`. Deadreckon answers server approval requests from
the run's existing capability posture, built from `sandbox.toml` and the parent
launch plan's capability preview. It does not ask Codex to bypass those limits.

| Server request | Allow | Deny |
|---|---|---|
| `item/commandExecution/requestApproval` for network | network is `full`, or an extracted host matches the allowlist | network is denied, the host cannot be determined, or it is not allowlisted |
| `item/commandExecution/requestApproval` for global install | install capability is enabled | install capability is disabled |
| other command execution | command stays inside the workspace-write sandbox | command cannot be determined |
| `item/fileChange/requestApproval` | the grant root (defaulting to the working directory) is inside the working directory or an additional writable root | the grant root is outside the writable roots |

Wire replies use Codex's `accept` or `decline` decision. Every answer also
appends a `provider.approval` trace with the request kind, subject, capability,
decision and reason. This replaces the former danger-full-access inversion
with an auditable map that cannot widen the run's policy.

### 51.4 Interrupt and degradation rules

A normal `deadreckon kill` still writes the durable cancel marker first. When
`provider-session.json` names an active app-server turn, the provider sends
`turn/interrupt` and waits for `turn/completed` before the existing process
termination path runs. If the protocol grace period fails, child supervision
escalates to process kill. The explicit force-kill path remains the immediate
operator override.

If startup, the handshake or a live server request fails structurally, the
provider uses the existing `cli:codex` exec driver and marks the server route
degraded for the rest of that provider instance. The response still identifies
the requested route and carries a `provider.route.degraded` caveat. The turn
loop mirrors that caveat into run attention. Cancellation errors do not start a
replacement exec turn.

Fallback never marks a steer delivered. It records the pending count in the
trace, keeps the ledger entries pending and names them in attach attention, so
the operator can see that the run continued without accepting those directions.

## 52. Keel: The Protocol Crate

Keel places one pure persisted-wire vocabulary below the readers and writers.
`crates/deadreckon-protocol` owns the run event, spend, trace and flight line
types plus a pointer-only narrative snapshot reference. The crate depends only
on `serde`, `serde_json`, `schemars`, `chrono`, and `thiserror`. It has no I/O,
async runtime, or dependency on another DeadReckon crate; the
`protocol_crate_has_no_internal_dependencies` test enforces that direction.
Ledger consumers depend downward on the protocol crate, never the reverse.

### 52.1 One vocabulary, unchanged files

`LedgerItem` is the in-memory tagged union of `Event`, `Spend`, `Trace`,
`Flight`, and `NarrativeSnapshotRef`. Its tag and alias rules provide one name
for mixed-source inspection and future evolution, and an unknown tag folds to
`LedgerItem::Unknown` instead of making an older reader fail. `LedgerFile`
totally maps the five persisted kinds to their existing locations:

| Kind | Existing file | Bare line written today |
|---|---|---|
| Event | `events.jsonl` | `EventLine(RunEvent)` |
| Spend | `spend.jsonl` | `SpendLine(SpendRecord)` |
| Trace | `traces.jsonl` | `TraceLine(TraceRecord)` |
| Flight | `flight-events.jsonl` | `FlightLine(FlightEvent)` |
| Narrative snapshot reference | `narrative/snapshots.jsonl` | application-local snapshot body after reference policy routing |

The `EventLine`, `SpendLine`, `TraceLine`, `FlightLine`, and
`NarrativeSnapshotRefLine` wrappers are serde-transparent. The union's
`kind`/`value` envelope is not added to those files. Narrative snapshots keep
their application-local body because the protocol owns only the stable
`snapshot_id` and path reference. Recorded pre-Keel fixtures round-trip all
five ledgers byte-identically, and the characterization/smoke goldens pin the
normal `show`, verdict, report, and attach surfaces.

### 52.2 Persistence policy and writer boundary

`deadreckon-protocol/src/policy.rs` is the single pure answer to whether a
`LedgerItem` persists, which `LedgerFile` receives it, and what redaction must
happen before it reaches disk. Unknown items do not persist. Event tool
arguments and trace details recursively redact gate-secret keys, including the
legacy-v1 nonce vocabulary; spend, flight, and narrative references pass
through unchanged.

`deadreckon_core::ledger_io` is the I/O adapter above that policy.
`prepare_ledger_item` applies redaction and resolves the path;
`append_ledger_item` then unwraps the transparent per-file line type and calls
the existing JSONL appender. Events, spend, traces, and flight events use that
append path directly. The narrative writer calls `prepare_ledger_item` with a
`NarrativeSnapshotRef`, verifies `LedgerFile::NarrativeSnapshots`, then writes
the unchanged application-local snapshot body. Policy therefore governs all
five writers without moving I/O into the protocol crate or changing bytes.

### 52.3 Schemas are checked artifacts

`deadreckon_protocol::all_schemas` derives the tagged union schema and a schema
for every public wire line type from the same Rust definitions. The generated
files under `docs/schemas/` are checked artifacts: the protocol test compares
both their exact filename set and pretty-printed contents, so drift or deletion
fails verification. Intentional changes regenerate them with
`DEADRECKON_UPDATE_SCHEMAS=1 cargo test -p deadreckon-protocol`; the failure
footer and `docs/schemas/README.md` carry that command.

`deadreckon report --json` is an application projection rather than a ledger
line. Its real `RunView` type graph generates the separate checked
`docs/schemas/projections/run-view.schema.json`, and the report renderer is
validated against that schema. This keeps the protocol schema set exact while
still giving report consumers a truthful schema path.

### 52.4 Boundary of the stable slice

Keel relocates type ownership and makes the existing persistence decisions
explicit. It does not merge files, add envelopes to bare JSONL rows, compress
old runs, add an index, move `PipelineState`, or export TypeScript types. Those
layout and publication changes require migrations and remain the explicit §52
V1 candidates.

## 55. Pennant: Descriptor-Declared Contracts

Pennant extends the shared contract machinery from §50 to generic CLI
providers. A provider can now declare its structured output in descriptor TOML.
Adding a compatible CLI needs a descriptor edit and recorded fixtures, not a
new Rust driver.

### 55.1 Contract schema

The optional `[contract]` section has this shape:

```toml
[contract]
stream_args = ["--format", "json"]
dialect = "json-lines" # or "json-document"
conversation_id_path = "/sessionId"
usage_input_path = "/usage/input"
usage_output_path = "/usage/output"
cost_path = "/usage/cost"
answer_path = "/answer"
error_flag_path = "/exitCode"
error_message_path = "/error/message"
flight_event_paths = ["/toolCallId"]
resume_args = ["--resume", "{conversation_id}"]
probe_substring = "--format"
```

`stream_args` must be non-empty. Every declared path must be an RFC 6901 JSON
Pointer. A `json-document` contract cannot declare live flight selectors
because it has no event stream. Only `resume_args` can use the
`{conversation_id}` placeholder.

The registry validates the section separately from the provider descriptor. A
malformed contract produces a warning, drops only the contract and leaves the
provider usable through its old raw-output path. `deadreckon providers check
<id>` shows the warning and probe result. `deadreckon providers list` and its
plain output mark routes with `contract=yes|no`; JSON uses a boolean
`contract` field.

### 55.2 Pointer extraction and compatibility

`json-lines` parses each non-empty output line as one JSON value.
`json-document` parses stdout as one value. The extractor applies the declared
pointers without provider-specific branches.

Conversation IDs use the first value found. Usage, cost, answer, error and
flight facts use the latest matching event. Missing pointers disable only that
capability for the turn and add a caveat. Output with no structured events
falls back to raw stdout with the inherited `provider.contract.degraded`
caveat.

The driver adds `stream_args` only when the descriptor template does not
already contain them. A successful first turn stores the conversation ID in
the provider-scoped `provider-session.json` from §50. Later turns substitute it
into `resume_args`. Providers without `resume_args` stay fresh per turn without
warning.

`probe_substring` checks `binary --help` once per process. A miss disables the
contract and reports that the installed binary predates token accounting. The
contract-less branch remains unchanged, so old descriptors keep their previous
arguments, response content and trace shape.

### 55.3 Usage and live flight events

Descriptor usage enters the same `ProviderUsage` and spend-ledger path as the
codex and claude mirrors. `show` and `report` therefore render descriptor
tokens on subscription runs. Reported cost remains trace information and does
not change subscription billing.

`flight_event_paths` can select a root marker or a nested tool-request object.
Rows with the same tool-call ID collapse to the latest state while retaining
earlier metadata such as the tool name. The response carries the terminal rows
in `trace.flight_rows`. `[ingest] live_contract = true` makes the §33 file
scraper yield, so live and post-hoc ingestion do not double-count.

Fixtures recorded from the installed binaries live under
`crates/deadreckon-providers/tests/fixtures/pennant/`.

### 55.4 Provider onboarding results

| Route | Probe result | Contract status | Shipped behavior |
|---|---|---|---|
| `cli:pi` | Pi 0.79.1 | onboarded, JSON Lines | extracts session ID, input/output tokens, reported cost and answer; resumes with `--session`; ingests tool execution live |
| `cli:copilot` | GitHub Copilot CLI 1.0.45 | onboarded, JSON Lines | extracts session ID, output tokens and answer; resumes with `--resume=<id>`; ingests nested tool requests and execution events live |
| `cli:gemini` | Gemini CLI 0.42.0 | documented gap | the binary advertises `stream-json`, resume and session flags, but the installed credentials fail with `IneligibleTierError/UNSUPPORTED_CLIENT` before any JSON event; no contract was guessed |
| `cli:opencode` | OpenCode CLI 0.15.5 | documented gap | removed the rejected `--dangerously-skip-permissions` flag; real JSON output emitted answer, error and null-text events while exiting zero, which the pointer dialect cannot classify safely |

Copilot 1.0.45 emits several standalone JSON documents even with `--stream
off`, so its honest dialect is `json-lines`, not `json-document`. It exposes
output tokens but no input-token count in the recorded result. OpenCode needs a
richer event mirror before onboarding. Gemini needs a successful current
structured fixture before onboarding.

## 56. Shakedown: One Reference Resolver

### 56.1 The problem: per-verb cascades and two meanings of `latest`

Every id-taking verb hand-rolled its own resolution cascade, and no two covered
the same kinds in the same order. `show` probed campaign, plan child, run, plan
and missed chains entirely. `kill` probed campaign, run, plan, chain and missed
plan children. `status` and `verdict` saw runs only. `latest` meant "newest in
this scope" to `status` (`main.rs::latest_run`) and "newest across every scope"
to `verdict` (`verdict.rs::resolve_latest_run`), so the two verbs could land on
different runs from the same directory. 58 call sites across 14 files.

The operator-visible result was a closed loop between the two most-used
orientation verbs, reachable in thirty seconds on a clean checkout:

```
$ deadreckon status
error: not found: latest run for current project (deadreckon-6283b242)
  hint: try: run `deadreckon list` to find valid run ids or config keys
$ deadreckon list
0c11f68e  pending  orchestrate  full-plan  fork  Build from review shorthand
Recommended
deadreckon status latest        <- refuses identically
$ deadreckon status 0c11f68e    <- an id `list` just printed
error: not found: run 0c11f68e  <- false: the id exists, it is a plan
```

### 56.2 `ResolvedRef` and the acceptance matrix

`commands/reference.rs` is the only module that answers "what does this
reference name?". `ResolvedRef` is
`Job | Run | PlanChild | Plan | Chain | Campaign`.
`VERB_REF_SPECS` declares what each ID-taking verb accepts, and
`RefQuery` deliberately has **no** `accepts` field: a verb's accepted kinds are
derived from its name via that table, so the matrix the tests iterate is the one
the code obeys. Carrying `accepts` at the call site was a second source of truth
that could drift — the same shape of defect this section removes.

### 56.3 Probe order, prefix rule, ambiguity

Plan-child refs are checked first on *syntax*, not precedence: they contain `:`
or `/`, which a bare id never does. Every other kind is then probed — regardless
of what the verb accepts — and all matches are collected. `accepts` narrows the
decision, not the probe; probing only the accepted kinds is exactly what
produced `not found: run 0c11f68e` for an id that existed.

Exactly one match resolves. Two or more is a cross-kind ambiguity refusal naming
both full ids, because resolving it by verb would be guessing which the operator
meant. Within-kind ambiguity passes the loader's own candidate-id text through
`ambiguous_within_kind`, which always attaches a runnable `try:`.

### 56.4 One `latest`

`latest` / `last` / no argument all mean: the most recently updated item, among
the kinds the verb accepts, in the current scope; `--all` widens scope and
changes nothing else. Scope comes from the fields `list` already uses
(`Job::scope`, `PipelineState::scope`, `Plan::parent_scope`, `Chain::scope`).
The ordering key matches `list_plan_entries`, so `latest` resolves to what
`list` puts at the top. Campaigns carry no scope of their own and are therefore
candidates only under `--all`; they stay resolvable by explicit ID everywhere.

`verdict latest` became scope-bound as a result. `verdict --all` already means
"compare several recent runs", so there is no widening flag for it — the open
question of one uniform widening spelling is in V1-CANDIDATES.

### 56.5 Kind-aware refusals and the no-loop invariant

`refusal_for` is the one place a wrong-kind refusal is written. Every message
names the reference, the kind it actually is, and one command that accepts that
kind: `0c11f68e is a plan, not a run` + `try: deadreckon show 0c11f68e`.
`deadreckon list` is deliberately absent from wrong-kind refusals — an id that
came from `list` must never be sent back to `list`. It remains legal for typos
and ambiguity, which are references `list` did not hand over.

### 56.6 The cross-verb journey test

`docs/FRIENDLINESS-AUDIT.md` scored `status` and `verdict` as **pass** on
"Refuse with try:", citing the exact lines that formed the loop. Both verbs were
individually well-behaved; a per-verb matrix structurally cannot express "the
command this refusal names must accept this id".

`tests/coherence.rs` holds that sentence as a test. For every id `list` prints,
each of `status`, `show`, `verdict`, `attach`, `finish`, `kill` must either
succeed or refuse with a next command that is not `list` and that itself accepts
the id. `assert_refusal_leads_somewhere` accepts either of the codebase's two
idioms — a `try:` footer or a `VerdictSurface` `Recommended` line.
`every_id_taking_verb_declares_its_accepted_kinds` and
`every_verb_used_in_source_is_listed_in_the_acceptance_table` are the structural
guards against a sixth cascade growing back.

### 56.7 List folding and the secondary-action cap

One plan with six children rendered as seven peer rows, each repeating the
parent's four-line goal, each child's goal column carrying the launch prompt
("This is one full-plan child run in a larger plan. Root goal: ..."). Children
now fold under their parent and show `PlanTask::subject` — the operator-
meaningful name for that work — rather than a parsed prompt. A child whose
parent is not in the listing stays top-level. `list --json` stays flat.

`VerdictSurface::try_new` caps secondary actions at `MAX_SECONDARY_ACTIONS` (3),
spending the last slot on `deadreckon help-all` so truncation never implies
there is nothing else. `doctor` printed ten; it inherits the cap without any
doctor-specific code.

### 56.8 V1 boundaries

Out of scope and logged in `docs/V1-CANDIDATES.md`: a uniform scope-widening
flag across verbs, giving campaigns a real scope (a schema change), `list --json`
folding, a `RunView`-backed projection for plans/chains/campaigns, verb
namespacing, and a durable id index.

## 58. Watchkeeper: One Durable Job, Two-Key Completion

Watchkeeper adds a durable local Job above the existing run state. Guided
`start`, ordinary direct `run` and `orchestrate`, new chains, stored-plan
`fork`, direct campaigns, and public or guided run follow-ups have one
approved, queued, leased and supervised parent Job ID. Every durable shape
verifies a same-ID parent result with a native deterministic gate and a fresh
read-only semantic judge. The supervisor validates the combined receipt before
promotion. Explicit preview, in-place/uncontained, historical chain, and chain
extension paths do not share one posture: preview and explicit
in-place/uncontained execution remain callable foreground, untrusted escape
hatches; public historical chain execution refuses before mutation or
execution; and public chain extension refuses before mutation while offering a
durable replacement schedule. The old chain conductor and mutation path remain
reachable only from the characterization binary used by tests.

### 58.1 Current execution boundary

| Entry path | Persistence and owner | Completion authority | Honest posture |
|---|---|---|---|
| `deadreckon start --mode run "<goal>"` | Writes a Job before the first turn, then detaches `supervisor serve --once <job-id>` | Contained native gate plus fresh read-only semantic `achieved`, sealed into one two-key receipt | Durable Single Job; survives the launching terminal |
| `start --mode review|full-plan` | Writes a `Graph` Job, normalizes delivery to `AtEnd`, and supervises the established conductor under the same parent ID | Copies the merged result into the same-ID parent run, then runs the native gate, fresh semantic judge, receipt validation and promotion | Durable and verified Graph parent; `finish` exports the receipt-bound parent |
| An auto-selected guided campaign | Writes a `LegacyCampaign` Job and supervises the established conductor under the same parent ID | Recovers exact persisted sub-plans, revalidates the worst-of roll-up, then runs the parent gate, semantic judge, receipt validation and promotion | Durable and verified Campaign parent; live recovery drills remain outstanding |
| Ordinary `deadreckon run "<goal>"` | Freezes direct-run options into a Single Job and detaches the same supervisor path | The same contained native gate, semantic judgment and combined receipt as guided Single | Durable by default; preview, explicit `--in-place`, and explicit `--sandbox none` stay foreground and untrusted |
| Direct `orchestrate` and stored-plan `fork` | Freeze the graph into a `Graph` Job and supervise the established conductor under the parent lease | At-end same-ID parent gate, semantic judgment, receipt and promotion | Durable by default; preview and trusted internal child execution remain foreground |
| New `chain` with supported policy | Compiles the schedule into a linear `Graph` Job | Verifies the composed parent once at the end with both completion keys | Durable by default |
| Unsupported policy-rich new `chain` | Refuses before Job creation, planning, state mutation, or execution | None | No silent fallback to the process-owned conductor |
| Historical `chain run|resume` | Public binary refuses before state mutation or execution and prints a durable migration command | None | Stored chain remains inspectable; legacy conductor is characterization-only |
| `chain extend` or `chain redo --extend` | Public binary computes the proposed schedule without saving it, then refuses and prints a durable migration command | None | No mutation; legacy mutation is characterization-only |
| Direct `campaign` | Writes a `LegacyCampaign` Job before the conductor starts | Rebuilds the worst-of roll-up, then uses the same parent two-key sequence | Durable by default; preview remains foreground and live recovery drills remain outstanding |
| Installed `deadreckon supervisor` user service | launchd or systemd starts `supervisor serve`, which scans supported nonterminal Jobs | Uses the same shape-specific classifiers and receipt validators as detached one-shot supervision | Conditional restart-at-login posture; live active-service and reboot acceptance remain |
| Public `extend` or a follow-up selected by guided `start` | Freezes the completed parent state, promoted-artifact tree and verified receipt, then writes a Single Job before child work | Revalidates the frozen parent inputs before continuation evidence, then uses the normal two-key child receipt | Durable parent-bound continuation; launch-time `--dest` is refused and delivery remains a later `finish` |

The Graph row includes source-copy admission. Guided `review` and `full-plan`
accept `--from <dir>` after validating it before provider work. Job creation
hashes the deliverable source, copies it into a preparing directory below the
Job root, re-hashes both sides, atomically publishes `approved-source`, and only
then queues the Job. The launch plan retains the canonical external path as
provenance, while `Job.source_cwd` and every Graph child use the controller-owned
copy. Source mutation or removal after admission cannot redirect execution.
Campaign source replacement remains out of scope, and source flags unsupported
by a selected shape still refuse before authoring or writes.

Persisted durability and supervised durability are different claims. Every run
continues to write state, ledgers, snapshots, and evidence. A Watchkeeper Job
also has a renewable fenced owner, typed terminal reason, detached process
group, and immutable authority. Durable Single, Graph and Campaign Jobs also add
the combined parent completion receipt. Without the user service, the detached
supervisor can outlive its launching shell but is not a machine-restart
guarantee.

A real approved `start --yes` is admitted only when the managed per-user
service definition is current, the platform manager reports it enabled and
active, and the supervisor has published a live schema-version-2 checkpoint.
launchd enablement accepts both the historical boolean form and current macOS
textual `enabled` / `disabled` form. Service discovery captures environment
paths once so concurrent checks cannot redirect a context after discovery.
Read-only preview and JSON inspection without `--yes` do not require or mutate
service state. `deadreckon setup --supervisor` is the one-step setup path; a
non-interactive launch otherwise refuses before provider classification or Job
creation. This makes the machine-recovery prerequisite explicit instead of
quietly falling back to a terminal-owned detached process.

### 58.2 Job protocol, event history, and projection

`deadreckon-protocol/src/job.rs` owns the checked schema-version-1 wire types:
`Job`, `JobEvent`, `JobLease`, `JobAuthority`, `SemanticJudgment`, and
`CompletionReceipt`. Job phase, terminal outcome, and causal `StopReason` are
separate enums, so `terminal` does not erase whether execution was verified,
cancelled, blocked, budget-exhausted, retry-exhausted, or stopped for review.

One Job is stored under:

```text
~/.deadreckon/jobs/<job-id>/
  job.json
  job-events.jsonl
  projection.json
  lease.json
  launch-plan.json
  acceptance.yaml
  authority.json
  supervised-child.json
  supervisor.out
  supervisor.err
  receipt.json
```

`job-events.jsonl` is lifecycle truth. The reducer requires sequences beginning
at one with no gaps. An exact duplicate event ID with identical bytes is
idempotent; conflicting duplicates and gaps are corruption. A torn final line
is ignored with a caveat and prevents a new lease claim until repaired.
`projection.json` and `lease.json` are rebuildable/checkpoint views, not a
second authority. Rich run facts stay in the normal run directory and compose
into `JobView`.

Legacy run, plan, chain, and campaign adapters are read-only views. Guided
advanced starts instead write a real Job history plus `driver.json`, which maps
the parent Job to the existing plan or campaign artifact with the same ID. The
mutable mapping is navigation evidence, never completion authority.

### 58.3 Approval before execution

`commands/job.rs` freezes all mutable inputs before queueing:

1. save and sync the resolved `launch-plan.json`;
2. copy or materialize `acceptance.yaml`;
3. validate that the contract has required checks and is not the unknown-tree
   directory-exists placeholder;
4. hash the goal, contract, effective policy, launch plan, deliverable source
   tree, and source revision into `authority.json`;
5. save `job.json`;
6. append `created`, `contract_approved`, and `queued`;
7. only then spawn the detached supervisor.

The effective-policy digest includes the requested sandbox selector and
tool-capability policy, not only spend, wall-time, retry, and semantic-judge
limits. Authority does not claim which backend will resolve at runtime. The
runtime reconstructs the approved policy before provider execution. The
native marker and final receipt separately bind the backend that actually
resolved and whether it contained the gate.

Every public durable creation route accepts the same absolute `--deadline`,
including guided and ordinary start, direct run/orchestration/campaign,
stored-plan fork, reshape, supported chain and continuation. Deadlines are
checked before launch and while the active child is running. Expiry terminates
and reconciles the exact outer, gate-evaluator, Campaign-subprocess and
merge-repair process authorities before recording
`deadline_reached/deadline`; it never schedules a retry. Public wall caps are
validated as positive whole seconds instead of being silently rounded or
widened.

For a Single shape, the Job ID is also the root run ID from launch. For Graph
and Campaign shapes, the plan or campaign keeps that parent ID while child runs
keep their own IDs. After merge, the supervisor creates the same-ID parent
result run.

For a follow-up, public `extend` and guided `start` use the same Single-Job
scheduler. The launch plan carries a continuation signal with the completed
parent run and scope, parent-state SHA-256, promoted-library deliverable-tree
SHA-256, optional verified parent-receipt SHA-256, and the bounded context
selection. Before the child writes its parent marker, history, or first trace,
it reloads the parent and revalidates all frozen identities. Changed state,
artifact bytes, receipt, completion status, or Job ownership fail closed.

`ResolvedRef::Job` is part of the shared Shakedown resolver. Job-aware verbs use
the same `latest`, scope and wrong-kind rules as their siblings. `list`
suppresses backing duplicates. `finish` validates the combined receipt before
applying or exporting, so child gates cannot stand in for parent authority.

### 58.4 Fenced ownership and process supervision

Lease claims serialize on the same per-Job control lock as event append. A
claim records owner ID, monotonically increasing epoch, boot ID, PID, process
group, heartbeat, and expiry. Every supervisor-authored lifecycle event must
present the current owner/epoch/boot token; a stale worker cannot append after
reclaim. Heartbeats update only the lease checkpoint and do not create fake
phase transitions.

Capstan's process helper starts the worker in its own process group and writes
`supervised-child.json`. Cancellation records `cancel_requested` first, then
terminates the process group with bounded escalation. Supervisor exit is only
a wake-up: terminal classification comes from persisted run state, the
deterministic marker, semantic judgment, and final receipt.

The approved Job wall cap covers cumulative active-attempt time, not only time
reported by the provider. The supervisor rebuilds elapsed time from the
append-only `AttemptStarted` and `AttemptStopped` history. An attempt left open
across supervisor or machine restart keeps consuming the same allowance. The
supervisor checks the boundary before an attempt, while a child runs, after it
exits and during controller-side Git, evidence, gate and receipt work.

When the cap expires, the supervisor reconciles the outer worker, gate
evaluator, Campaign subprocess, merge-repair groups and tracked Docker
execution before recording `wall_cap`. If it cannot prove that all owned work
stopped, it records `LostContainment` instead of claiming clean budget
exhaustion. This closes the durable-run stall where completed provider work
could leave the Job waiting indefinitely in local post-processing.

The recovery policy is intentionally fail-closed:

- a same-boot replacement can retain a child only when the persisted process
  metadata still identifies a live process with the same boot and process-start
  identity;
- expired or changed-boot leases get a higher epoch;
- a Single Job can resume its exact persisted run under the frozen attempt,
  spend and wall-time policy;
- a Graph resume keeps the same pending or forked plan ID, and can reconstruct
  a missing root `driver.json` from the exact same-ID Plan without replanning;
- a Campaign resume keeps the same parent campaign ID and reconciles an exactly
  linked persisted sub-plan before it launches the next sub-plan; Campaign
  child Plan IDs are reserved before spawn and retained across every covered
  launch crash window;
- outer-worker guarded launch persists the prepared launch and attempt before
  spawn, then releases the blocked child only after its metadata and
  `ChildLinked` event are durable. A pre-release crash can relaunch the same
  logical attempt. Post-release recovery requires a valid release
  acknowledgement tied to that linked launch, plus matching boot and
  process-start identity, or it fails closed;
- a second guarded release surrounds strict gate evaluation: repository checks
  cannot start before their unique boot/process/attempt identity is synced, and
  cancellation or retry must reconcile every nested identity plus the outer
  worker group first;
- cancellation and deadline cleanup also enumerate separately grouped
  merge-repair children from authenticated authority records. Malformed,
  foreign or swapped records stop `LostContainment` instead of leaving an
  untracked repair process or claiming a clean stop;
- root planner spend and wall time are embedded before child work, restored
  after mapping-creation crashes, subtracted from the Job policy, and divided
  across Plan tasks or Campaign sub-plans rather than granting every child the
  whole parent cap;
- persisted Graph or Campaign budget evidence remains terminal across crashes
  after child metadata removal and between `attempt_stopped` and the final Job
  event; cancellation still wins if it races budget finalization;
- spawn failures consume the bounded attempt policy;
- missing identity or containment evidence stops with a typed reason instead
  of guessing.

Hermetic tests cover both guarded launch boundaries, same-ID root repair,
Campaign sub-plan reservation, boot/PID reuse refusal, corrupt nested identity
refusal, and typed restart classification. Public macOS tests additionally hold
open a real Seatbelt gate, prove operator cancellation reaps it without signing,
and SIGKILL the outer launcher to prove the old evaluator is gone before one
bounded retry. They do not prove recovery through a real machine reboot or an
active launchd/systemd restart; those remain operator acceptance gaps.

### 58.5 Deterministic gate and protected boundary

The strict gate has 2 contained phases. The trusted controller materializes the
approved `acceptance.yaml` before evaluation. It then starts keyless `dr-gate
evaluate` through the sandbox runner under the backend that actually resolves.
The evaluator receives no `GATE_*` inputs. It runs the approved checks, computes
tamper facts, writes no proof or Job control files, and returns JSON on stdout.

The sandbox runner scrubs inherited signing and containment inputs. For strict
Jobs a private release pipe holds the evaluator until its unique, synced
attempt/launch/boot/process identity exists; the helper then creates the fresh
evaluator process group and marks the record running before any check executes.
Cancellation and retry reconcile that group by identity, while corrupt or
reused identity blocks as `LostContainment`. The runner terminates residual
descendants before returning. A resolved backend of `none` refuses before the
controller reads any signing material.

Only then does the controller start childless `dr-gate sign`. The signer reads
the Job's HMAC key, strictly validates the evaluation against the run ID,
canonical working directory and approved contract, and recomputes tamper facts.
It reconstructs progress and tamper evidence and writes the version-2 marker
with the containment fact and observed backend. No repository-controlled check
runs in the signing phase.

Version-2 native markers use HMAC-SHA-256 over canonical bytes with
constant-time verification. Keys live outside the agent-visible run root under
`~/.deadreckon/gate-keys/` with owner-only permissions. Missing version-2 key
material fails verification. Old version-1 nonce markers remain explicitly
legacy rather than being upgraded by description.

Worker and gate-evaluator sandbox policy denies the key store and makes Job
authority, contract, receipt, lifecycle, proof, snapshot, and provenance
control paths inaccessible or read-only across Seatbelt, bubblewrap, Docker,
CLI providers, and the Codex app-server outer boundary. Symlink and
canonical-path cases are covered. A strict durable Job refuses
`sandbox_backend = none` before signing and cannot produce a receipt with
`contained = false`.

Trusted sandbox wrappers are resolved to canonical absolute system
executables before provider configuration is built; ambient `PATH` cannot
substitute a different `sandbox-exec`, `bwrap`, or Docker entry point. The
controller also seals an HMAC-authenticated sandbox-boundary observation for
the exact Job attempt, authority, contract, result tree, probe and resolved
backend. It records denial of gate-key reads, proof/control writes,
operator-capture reads and writes, and inherited signing inputs. The completion
receipt binds the observation digest, and later validation recomputes both the
observation signature and its current Job/result identities.

Provider output crosses a separate artifact boundary before it can become a
trusted result. Workspace paths are classified as deliverable,
evidence-only, lifecycle metadata, or disposable runtime output. Trusted
copies preserve regular files, executable mode, symlinks, and raw symlink
targets without following links; special filesystem entries fail closed.
Rust `target`, JavaScript `node_modules`, and SwiftPM `.build` directories are
disposable runtime output. DeadReckon omits them from source copies, snapshots,
provider checkpoints, result capture and evidence inventories.
Provider-created commits and index state are discarded, then DeadReckon stages
only deliverable paths and creates its own hook-free commit through a captured
Git control context outside the provider's authority.

That Git context captures and validates the original `.git` redirect, run
worktree path, linked-worktree Git directory, and common Git directory.
Privileged sanitization and trusted commits do not use a provider-rewritten
workspace router. Documentation providers receive a read-only isolated
workspace; only DeadReckon parses, writes, and commits the approved
documentation result. Merge-aware history scans preserve raw Unix path bytes
and reject private paths even when they appear only in a merge or are added
and later deleted. Platforms that cannot represent a path fail closed. Strict
result paths with active Git filters or `160000` gitlinks are refused rather
than incompletely modeled. Filter refusal happens before any reset, restore or
staging operation can refresh the worktree and inventories both current
workspace-guard paths and every path in the approved base tree. The mixed reset
also uses `--no-refresh`, so a racy timestamp cannot invoke an external clean,
smudge or process filter before the refusal.

Trusted Git commands that stream path input, including `check-attr --stdin`,
write stdin while collecting stdout and stderr. This keeps all pipes moving and
prevents large workspaces from deadlocking the supervisor during filter
inspection.

The real macOS public-command end-to-end test runs the approved shell check
under Seatbelt. It proves protected-path denial, inherited `GATE_*` scrubbing,
residual process-group cleanup and signing of the observed `sandbox-exec`
backend. An opt-in real Docker test separately proves the common key,
environment, network and control-path boundary while preserving ordinary
deliverable writes. It never pulls the required image. Three public strict
Docker Job tests use a statically linked Linux `dr-gate` to prove deterministic
completion followed by `NEEDS_REVIEW`, cancellation without retry or receipt,
and worker `SIGKILL` cleanup before one bounded retry. Live Linux/bubblewrap
remains outstanding. Host sandbox availability still matters: an `auto` request that
resolves to no real backend cannot produce a verified strict Job.

### 58.6 Independent semantic judge

For every durable Job, after deterministic checks pass,
`semantic_judge.rs` assembles a bounded
evidence pack containing the approved goal, frozen contract, check result,
authority facts, changed-file list, bounded diff, and implementation notes. A
fresh provider request has no worker session and uses the explicit read-only
workspace posture. The response schema permits only:

- `achieved`: only when coverage is non-empty, every claim is `met`, every
  claim cites allowed evidence, and `missing` is empty; persist the judgment
  after accounting and attempt receipt sealing;
- `revise`: for a Single Job, add bounded findings and continue within the
  remaining budgets; for Graph and Campaign parents, start a new bounded,
  fenced parent-only repair attempt over the merged result without rerunning
  successful leaves;
- `uncertain`: stop `NEEDS_REVIEW`.

An unavailable or malformed strict judge also becomes `NEEDS_REVIEW`. The judge
is never called after deterministic failure and cannot override it. Judge
tokens, wall time, route, model, and spend enter the normal spend/trace
evidence as `semantic_judge`. Single, Graph and Campaign paths reconstruct
their applicable execution/planner usage before judging, pass the remaining
wall budget as a cancellation deadline, and refuse receipt sealing when the
recorded judge response exceeds the remaining spend or wall policy.

### 58.7 Combined receipt and promotion

For a durable Job, `receipt.json` is supervisor-issued
`two_key_completion` evidence. Its
HMAC-SHA-256 signature covers identity, outcome, issuer, proof and stop reason;
authority, goal, contract, effective-policy and launch-plan digests;
deliverable source and result tree digests; optional source and result
revisions; deterministic-marker and semantic-judgment digests; containment;
and the resolved sandbox backend. Validation recomputes deliverable tree
digests; it does not trust mtimes. Receipt sealing repeats the evidence-backed
`achieved` invariant, so a persisted file cannot bypass the semantic response
parser during recovery.

The receipt schema does not contain a branch name, filesystem inventory,
merge-history inventory, or retention-ref name. For worktree results, sealing
and validation use the trusted codebase record and Git state to enforce the
approved base, result branch and revision, exact Git and filesystem identity,
ignored or uncommitted deliverables, `assume-unchanged` and `skip-worktree`
masking, and unexpected private history. Sealing separately creates
`refs/deadreckon/results/<job-hash>` to retain the signed revision; the ref name
is not a receipt field. During a verified worktree apply, `finish` compares
each signed deliverable delta path's Git entry and every introduced
delivery-history path with the sealed result. A failed final identity check
resets the target to its pre-delivery revision instead of leaving rejected
commits behind.

New Job promotion calls this validator. Changing the authority, launch plan,
contract, deterministic marker, semantic judgment, result bytes, containment
fact, or signature prevents `finish`. Legacy runs keep their historical marker
validator during the compatibility window and must not be presented as
two-key-verified Jobs.

Graph Jobs normalize delivery to `AtEnd`. After merge, the supervisor copies
the deliverable result tree into a run with the Job ID. It runs the contained
keyless-evaluate/HMAC-sign gate against the frozen contract, asks a fresh
read-only semantic judge, seals and validates the parent receipt, then
promotes. `finish` exports that receipt-bound parent.

Campaign Jobs use the same parent sequence. Before the native gate, the
supervisor validates the merged result marker, compares the stored and merged
roll-ups, and rebuilds the worst-of roll-up from current leaf evidence. A
refused, missing or changed roll-up fails the parent before the semantic judge.

Semantic `revise` for Graph and Campaign writes a fenced repair intent,
manifest and candidate under the same parent Job. The repair turn receives the
judge findings and edits only the merged parent result; completed leaf runs are
not relaunched or rewritten. Each round consumes a new Job attempt and launch
identity, stays inside the frozen attempt/spend/wall policy, and must finish
before the next deterministic gate and fresh semantic judgment.

The supervisor can recover a fully written candidate after an expired lease
without spawning a duplicate repair worker. It archives each round's intent,
manifest, candidate, deterministic marker and semantic judgment, and links
adjacent rounds by attempt, launch, lease and result-tree identity. Receipt
sealing and validation capture each proof file once, require a stable regular
non-symlink file, validate the complete lineage against Job events and current
result bytes, and bind the active repair manifest and candidate into the native
marker HMAC. Mutation, removal, identity drift or byte-identical symlink
substitution fails closed both before sealing and later in `finish`.

Cancellation is observed during both the parent repair turn and semantic
judging. Cancellation, budget exhaustion, retry exhaustion, provider failure
and review-required outcomes remain distinct terminal reasons. A deterministic
parent gate failure remains `FAILED`; `uncertain`, unavailable or malformed
semantic evidence remains `NEEDS_REVIEW`.

### 58.8 Per-user service posture

The operator namespace is explicit and discoverable:

```text
deadreckon setup --supervisor
deadreckon supervisor install
deadreckon supervisor start
deadreckon supervisor status
deadreckon supervisor stop
deadreckon supervisor serve [--once] [JOB_ID]
```

macOS uses
`~/Library/LaunchAgents/com.deadreckon.supervisor.plist`; Linux uses
`$XDG_CONFIG_HOME/systemd/user/deadreckon-supervisor.service` or the normal
`~/.config` fallback. Definitions pin the exact current executable,
`DEADRECKON_HOME`, and `PATH`, use restart-at-login/failure policy, and keep
service logs under the DeadReckon home. Linux stops the whole service control
group. Unsupported operating systems refuse installation.

Ordinary setup installs and starts the service as one explicit action. The
lower-level supervisor commands remain available for inspection and operator
maintenance. A current service status is not inferred from a PID alone: the
schema-version-2 checkpoint binds the boot ID, PID process-start identity,
instance ID and monotonic generation. After an explicit restart, launch
admission requires a fresh successor instance rather than accepting the old
checkpoint. Legacy or malformed checkpoints and symlinked/non-regular managed
units fail closed.

Installation is idempotent and atomic. It may replace an older definition only
when the DeadReckon ownership marker is present; a same-name unmanaged unit is
a hard refusal. A new install remains stopped until `start`. `stop` disables
and unloads the service but retains its managed definition, including when it
was created by an older DeadReckon binary.

Tests render both platform definitions and classify the stored service posture
as unsupported, uninstalled, unmanaged, stale or current. They do not prove
that `launchctl` or `systemctl` has an active service. They also do not prove a
real login or reboot.

### 58.9 Evidence and limits

The repository has focused tests for schemas, reduction, projection rebuild,
lease fencing/reclaim, process-group survival, frozen approval inputs, HMAC
markers, the two-phase gate, boundary denials, hostile marker search/forgery,
semantic read-only posture and decisions, receipt tamper refusal,
detached-parent survival, typed spawn failure, the shared Job resolver, guided
Graph and Campaign identity,
same-ID root mapping repair, guarded launch recovery, persisted Campaign
sub-plan recovery and reserved identities, aggregate root-planner spend/wall
enforcement, cumulative active-attempt wall enforcement across restarts,
active process-tree cleanup, strict contract admission, terminal budget
recovery after sidecar loss, cancellation races,
worst-of roll-up refusal, one- and multi-round Graph/Campaign semantic parent
repair, candidate-ready crash adoption, repair-lineage mutation and symlink
refusal, parent receipt crash recovery, provider-result persistence before
local post-processing, large bidirectional Git input, SwiftPM `.build`
exclusion, execution-team/model recovery, service rendering, fault boundaries,
and promotion enforcement.

`examples/watchkeeper-dogfood/` contains an operator-triggered public-command
harness, a 24-row/two-provider-slot matrix, a metrics schema/collector, a
human-review template, a credential-free adversarial runner, and a passive
operator-gated recorder for the 9 current live fault claims. The recorder
declares prerequisites, interventions, objective oracles, sanitized evidence
and cleanup, but never starts providers, signals processes, controls services,
changes networking, reboots, or calls `finish`.

Pass-capable recording uses a canonical `dr-capture` and sibling
`deadreckon` pair outside every Job-controlled source, working, run, merge and
repair root. The helper HMAC-authenticates an immutable Job/trial/provider
binding, every append-only exact-evidence event and history head, and the final
receipt. Preparation is idempotent for identical inputs, refuses conflicting
replacement, and requires the actual Job shape to match the selected trial's
closed shape declaration. Finalization deterministically reconstructs the
evaluation from protected evidence, then publishes only a sanitized envelope
containing the evaluation digest, protected receipt digest and HMAC publication
proof.
Operator-selected manual files remain a compatibility documentation path and
can never produce `passed`.

The binding also freezes a non-empty set of exact terminal
`JobOutcome`/`StopReason` pairs. It never treats the two fields as independent
wildcards. `verified/verified` follows the existing completion-lineage path and
requires the canonical valid `CompletionReceipt`. Any approved non-Verified
pair follows a separate terminal-lineage path: `dr-capture` requires no
`receipt.json`, re-reads the stable append-only Job history and projection,
checks the final event produced that exact result, and binds the authority,
history, terminal event, `job-view-after`, and public Job report into the HMAC
capture receipt. Failed deterministic checks can therefore never be relabeled
as verified completion.

The live provider network-loss claim has a separate signed observation path.
Prepare resolves the one declared worker route through the provider registry,
requires a non-loopback HTTP descriptor endpoint, and freezes the exact role,
route and endpoint in capture authority. The protected helper uses the
registry's bounded ping before, during and after the operator-controlled fault.
Before and unreachable observations must bracket the same live process,
launch, attempt and lease with one durable `ChildLinked` event. The restored
observation retains that affected identity; cleanup and pass refuse until it is
reachable. Pass then binds the exact later `AttemptStopped` to that attempt and
lease and requires its retry or an approved terminal result, plus the exact
after history and public report. This proves an observed endpoint transition
and ordered response, not which host command caused the outage.

The Campaign interruption claim is similarly narrow and pass-capable. Before
the fault it requires one ordered `sub_launch_prepared`, `sub_launched`,
`sub_process_launch_prepared`, `sub_process_released`, and
`sub_process_linked` authority chain for the protected sub-Plan. After a Job
lease reclaim it accepts exactly one `sub_process_adopted` event with the same
parent, sub, Plan, attempt, launch, PID, boot and process-start identities and
the newer fenced lease, followed by `sub_recovered`. The canonical adoption
event is itself the protected intervention evidence. A new launch fact,
foreign owner, stale Plan, duplicate adoption, changed intervention or reopened
completed Plan task fails the oracle. This proves persisted fenced adoption;
it deliberately does not claim that arbitrary external side effects are
globally exactly-once. The real provider interruption drill remains unrun.

The committed credential-free result records 13 passes, 0 failures, and 8
explicitly unproven live/host claims. The sanitized live result records 2
attempted tasks, 22 not run, and 0 verified.

That 8-claim count is historical evidence for source `e87c70f`. The current
recorder and runner list 9 open live claims because the stronger hostile live
Docker/provider/valid-receipt claim is now separate from the passing narrower
credential-free Docker lifecycle proof.

The macOS public-command end-to-end gate trial and the common Docker
control-boundary trial are real host evidence, not hermetic backend
simulations. The public strict Docker completion, cancellation and worker-death
rows are also bound to the committed clean source. The repository still does
not report verified completion rates from the planned live tasks, prove
false-acceptance or false-rejection rates, demonstrate live Linux/bubblewrap,
demonstrate a real machine restart, or demonstrate live Campaign interruption
recovery.
Direct run, orchestration, stored-plan fork, supported new chain, campaign, and
run-follow-up launches now share the Job scheduler. Public historical chain
execution and mutation refuse; their process-owned implementation is retained
only for characterization tests. Preview and explicit in-place/uncontained
execution remain foreground and untrusted. The operator script in
`docs/WATCHKEEPER-OPERATOR-ACCEPTANCE.md` separates tests available now from
open live claims.

## 59. Soundings: Source-True, Bounded Launch Admission

Soundings closes the gap between the project named by `start --from` and the
project that preflight, done-contract authoring and Graph execution actually
used. It adds no Job, launch-plan, PipelineState or acceptance-file schema. The
new state is an ephemeral controller decision plus Job-owned approved bytes.

### 59.1 One source decision before intelligence or mutation

After launch shape selection, `start` validates source compatibility and builds
one `ResolvedStartSource`: durable mode, requested provenance, canonical
inspection root, contract-writer root and dirty posture. This happens before
service mutation, provider classification/selection, contract authoring, file
writes or final confirmation. A second incompatible resolution is an error.
Preview, plain/card/JSON output, acceptance, Job authority and dispatch consume
the same object.

An authoring-needed preview names the source and its ownership plainly:
inspection root, writer root, structured provider/model route, cumulative
authoring limit, and `approved copy from <canonical source>`. An unsupported or
missing source refuses at this boundary and reaches no provider or contract
writer.

### 59.2 Approved Graph copies

`review` and `full-plan` support `--from`. The controller canonicalizes the
operator path, indexes deliverables, copies tracked and untracked deliverable
bytes into a Job-local preparing directory, indexes the copy and re-indexes the
source, and refuses any digest disagreement. A successful copy is atomically
renamed to `<job>/approved-source` and synced before the Job is queued. Git
metadata and disposable/runtime output are excluded by the canonical artifact
policy. The authority binds the approved tree digest; the launch plan keeps the
external path only as provenance; the Graph driver initializes Git and launches
children inside the approved copy. The original tree is never modified and can
change or disappear after admission without redirecting the Job.

### 59.3 Separate contract ownership from source inspection

`AcceptanceAuthoringContext` makes the two roots explicit. The launch project
owns `.deadreckon/acceptance.yaml`, `.deadreckon/acceptance.md` and helper
scripts. The resolved source supplies facts through a deterministic dossier.
The dossier is sorted, capped and visibly truncated; excludes credentials,
history, Git/runtime/build trees and symlinks; and extracts ecosystem manifests
such as SwiftPM product, target and test names. Generated YAML and helpers must
remain portable through `{working_dir}` and cannot embed the original absolute
source path. Direct `def-done` keeps its existing behavior by supplying the
same path for both roots.

### 59.4 Structured authoring under one deadline

Draft and critic calls carry exact output schemas. CLI adapters must prove a
structured-text-only posture; Codex authoring is ephemeral and disables tool,
web, MCP and user-rule/config surfaces, while Claude uses its corresponding
safe/schema flags. Tool-free API routes use strict response formats. Unsupported
adapters fail closed. Immutable capability probes are cached per binary/version.

Draft, critic and optional redraft share a 120-second default deadline. Draft
gets at most 60 seconds, critic 20, and redraft only the remaining time up to
60. The config key `defaults.done_contract_max_wall_seconds` is clamped to
30–600 seconds. Timeout or cancellation terminates and reaps the whole provider
process group and removes temporary files before returning. Initial failure
writes nothing. Critic/redraft failure cannot approve a weak candidate; only a
lint-clean draft may proceed to explicit human review. Redraft receives the full
prior candidate, helpers, dossier, lint and verdict. `reject` normalizes to
`redraft`; one critic and one redraft remain the ceiling.

### 59.5 Compatibility and proof boundary

Clean-current Single and Graph launches retain their established source modes.
Existing contracts require zero authoring calls and remain reusable after an
unrelated later refusal. Campaign `--from`, remote sources, persisted authoring
sessions, new acceptance check kinds and live-provider latency claims are not
part of Soundings. The durable contract, contained deterministic gate,
independent semantic judge and signed receipt from §58 remain the completion
authority.

Hermetic depth tests reproduce the empty-destination plus dirty/untracked
Cloudwing source, prove the contract sees Cloudwing facts, launch a Graph Job,
compare preview/authority/approved-copy digests, preserve the external source,
bound hung stages, reap descendants and retain legacy clean launch behavior.
The operator-facing reproduction and timeout/retry checks are in
`docs/SOUNDINGS-OPERATOR-ACCEPTANCE.md`.

---

*This document is canonical for the production-release reality of deadreckon. Future hardening passes (per the robustness rider) and feature passes (per the usability rider) will update sections 6, 9, 11, 13, 14, 18, 22, 31, 32, 37, and 38 in particular. Updated 2026-08-02 for Soundings source-true, bounded launch admission (§17, §46, §48, §58 and §59), unified execution-team selection (§17), strict durable contract admission, cumulative active-attempt wall enforcement, process reconciliation, early provider-result persistence, concurrent Git pipe draining and SwiftPM artifact exclusion (§9, §22 and §58). Updated 2026-07-31 for the public legacy-chain boundary (historical execution and mutation refuse before state change, unsupported policy-rich launch refuses before Job creation, and the characterization binary alone retains the old conductor), Watchkeeper durable run continuation, authenticated operator capture, sandbox-boundary observations, canonical sandbox-wrapper resolution and pre-refresh Git-filter refusal (§58); updated 2026-07-30 for Watchkeeper bounded Graph/Campaign parent repair and tamper-resistant repair lineage (§58), plus result-boundary and recovery hardening (§58: immutable execution policy, trusted Git routing, exact artifact/result/delivery identity, crash-safe promotion and cleanup, crash-atomic guarded launch, same-ID Plan/Campaign ownership and mapping repair, aggregate root-planner budgets, typed terminal recovery, and cancellation precedence); updated 2026-07-29 for Watchkeeper convergence (§58: durable ordinary direct execution, stored-plan fork and supported chains on the same Job scheduler, plus credential-free adversarial evidence); updated 2026-07-28 for Watchkeeper (§58: durable guided Jobs, fenced local supervision, protected HMAC gate, read-only semantic judge, parent receipts and promotion for Single, Graph and Campaign shapes, conditional service posture, and explicit dogfood limits); updated 2026-07-24 for Shakedown (§56: one reference resolver, one `latest`, kind-aware refusals, the cross-verb journey test, list folding, the secondary-action cap); updated 2026-07-16 for Rudder (§51: app-server connection, durable steering, capability-answered approvals, interrupt and degradation rules) and Pennant (§55: descriptor-declared CLI contracts, pointer extraction, Pi and Copilot onboarding, Gemini and OpenCode gaps); updated 2026-07-04 for Logbook (§49: shared RunView read model, snapshot diffs, show/report/history events, verdict/doc/attach projection parity) and Contract (§48: goal-aware compiled done contracts, falsifiability lint, critic/redraft, divergence, review/card/JSON surfacing); updated 2026-07-03 for Helm (§47: mission-control attach, spine/tree/timeline/why/command/motion); updated 2026-06-17 for Orchestrated Narration (§45: every orchestrate/campaign child narrates file-only, parent aggregate stderr line, campaign Narrative view) and the §44 corrections it implies; previously updated 2026-05-31 for Navigable campaign attach, the Decompose binary-module layout, Effortless friendliness, tamper-evident gate behavior, release posture, and plan-result docs. Line numbers are best-effort locators; always cross-check against the code before relying on a specific line.*
