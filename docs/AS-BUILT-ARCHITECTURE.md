# AS-BUILT-ARCHITECTURE.md

**Subject:** deadreckon — a long-running, BYOK, sandboxed agentic CLI harness in Rust
**Frame:** Reference specification for the **alpha-tier** as-built reality at `/Users/gdc/deadreckon/`. Modeled on `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` (the Printing Press).
**Last updated:** 2026-05-13 (post orchestration milestone)
**Maturity:** alpha. Workspace version `0.1.0`. Build/test/clippy/fmt all green.

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
28. [Chains & Autonomous Goal Chaining](#28-chains--autonomous-goal-chaining)
29. [Workspace Hygiene](#29-workspace-hygiene)
30. [Plans & Multi-Agent Orchestration](#30-plans--multi-agent-orchestration)

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
│   main.rs      command handlers, ratatui rendering, post-run summary       │
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
  - `crates/deadreckon-core` — library for durable state, artifacts, gates, docs, locks, chains, and codebase-mode primitives.
  - `crates/deadreckon-runtime` — library for provider/sandbox orchestration, the turn loop, and doc polish.
  - `crates/deadreckon-providers` — library for provider config, adapters, and fallback routing.
  - `crates/deadreckon-sandbox` — library for platform-native sandbox backends and per-tool policy.
  - `crates/deadreckon` — binary (`deadreckon`) + binary (`dr-gate` at `src/bin/dr-gate.rs`).

### 2.2 Crate-by-crate

**`deadreckon-core` (`crates/deadreckon-core/src/lib.rs`).** Re-exports the public surface of the harness primitives. Modules:

| Module | Purpose |
|---|---|
| `state.rs` | `PipelineState`, `RunStatus`, `PhaseId`, `PhaseState`, `create_run`, `load_run`, atomic writes |
| `paths.rs` | `DeadreckonPaths`, `workspace_scope`, `task_key`, all path resolution |
| `lock.rs` | `LockState`, file locks via `fs2`, PID liveness via `nix::kill(pid, 0)`, heartbeat |
| `promotion.rs` | `promote_completed_run`, manifest writing, atomic working→library swap, crash recovery |
| `gate.rs` | `AcceptanceMarker`, signature validation, anti-self-attestation |
| `artifacts.rs` | `copy_tree`, `snapshot_working`, `append_{spend,trace,provenance}` |
| `cancel.rs` | cancellation markers and run-root cancel checks |
| `chain.rs` | autonomous goal-chain records and conductor state |
| `codebase.rs` | worktree/copy/fresh/in-place mode records and source materialization |
| `docs.rs` | deterministic run-doc templates, frontmatter, inventory, and promotion copies |
| `events.rs` | `RunEvent`, `RunEventBus`, `tokio::sync::broadcast` channel |
| `error.rs` | `DeadreckonError`, `Result<T>` |

**`deadreckon-runtime` (`crates/deadreckon-runtime/src/lib.rs`).** The orchestration layer that depends on core, providers, and sandbox.

| Module | Purpose |
|---|---|
| `turn_loop.rs` | `RunLoopConfig`, `RunLoopOutcome`, `run_turn_loop`, model action parsing, tool dispatch, cancellation, acceptance, and promotion |
| `polish.rs` | `polish_run_docs`, `PolishConfig`, skill resolution, polish input hashing, and nonfatal doc-provider polish |

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

**`deadreckon-sandbox` (`crates/deadreckon-sandbox/src/lib.rs`).** A facade over sandbox backend resolution, command construction, policy, doctor checks, and subprocess supervision.

| Module | Purpose |
|---|---|
| `backend.rs` | `SandboxBackend`, `SandboxError`, backend resolution |
| `spec.rs` | `SandboxSpec` |
| `commands.rs` | per-backend command/profile construction |
| `policy.rs` | `ToolSandboxPolicy` |
| `doctor.rs` | backend availability checks |
| `process.rs` | `run(SandboxSpec) -> SandboxRunOutput`, PID files, cancellation, SIGTERM/SIGKILL escalation |

**`deadreckon` (`crates/deadreckon/src/cli.rs`, `crates/deadreckon/src/main.rs`, and `crates/deadreckon/src/bin/dr-gate.rs`).** Clap parser definitions, command handlers, ratatui TUI for `attach`, and `dr-gate` as a standalone acceptance-marker writer.

### 2.3 Top-level documentation

- `README.md` — quickstart.
- `DESIGN.md` — intent + reference patterns (AS-BUILT §3–9, Claude Code mining notes).
- `CHANGELOG.md` — version history.
- `DEPENDENCIES.md` — Tier 1/2/3 rationale per dependency policy.
- `HOWTO.md` — usage guide.
- `docs/GAP-ANALYSIS.md` — outstanding gaps vs. requirements.
- `docs/MULTI-RUN.md` — multi-run sequencing semantics.
- `docs/RESUME-SEMANTICS.md` — resume behavior.
- `docs/V1-CANDIDATES.md` — deferred features.
- `docs/goals/` — eight dated goal/rider documents.

### 2.4 Skills

`/Users/gdc/deadreckon/skills/default-coding/SKILL.md` is a single Markdown skill loaded at runtime. The skill is opaque to the binary; the run records `skill_name` + `skill_path` in `PipelineState` and includes the skill in the prompt frame. Skill swapping is in scope but only one skill ships today.

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
| **Skill** | Agent-facing prose, judgment, prompt frame | `skills/default-coding/SKILL.md` | Markdown |
| **Binary** | State, locks, sandboxes, providers, gates | `crates/deadreckon*` | Rust |

The skill is invoked indirectly: it sits at the path recorded in `state.skill_path` and is read into the prompt frame by `build_prompt` in `crates/deadreckon-runtime/src/turn_loop.rs`. The binary never reaches into skill internals. New skills can be added under `skills/<name>/SKILL.md` and selected with `deadreckon run --skill <name>`.

This split lets each side do what it's good at:

- The Rust binary enforces invariants — locks, atomic file ops, signed acceptance markers, sandboxed subprocesses.
- The Markdown skill makes judgment calls — what to ask the LLM for, what tool sequence to prefer, when to declare done.

---

## 4. State Machine & Persistence

### 4.1 `PipelineState`

`crates/deadreckon-core/src/state.rs:77-110`:

```rust
pub struct PipelineState {
    pub version: u32,                       // STATE_VERSION = 1 (line 15)
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

`state.rs:17-26`:

```rust
pub enum RunStatus {
    Pending, Planned, Executing, Completed, Failed, Killed,
}
```

`state.rs:42-75`:

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

`state.rs:233-254` initializes seven phases for every new run:

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

`state.rs:353-373`:

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

JSONL files (`spend.jsonl`, `traces.jsonl`, `provenance.jsonl`, `events.jsonl`) use `append_json_line` (`state.rs:375-388`): open in append mode, write line + newline, `sync_all`.

### 4.6 Schema versioning

`STATE_VERSION = 1` (`state.rs:15`) gates future migrations. `load_state` rejects unknown versions; migrations would land here.

---

## 5. File-System Layout

### 5.1 Source tree (`/Users/gdc/deadreckon/`)

```
/Users/gdc/deadreckon/
├── Cargo.toml                    # workspace
├── Cargo.lock
├── README.md / DESIGN.md / CHANGELOG.md / DEPENDENCIES.md / HOWTO.md
├── crates/
│   ├── deadreckon-core/          # durable state, locks, gates, docs, artifacts
│   ├── deadreckon-runtime/       # provider loop, sandbox dispatch, doc polish
│   ├── deadreckon-providers/     # provider trait + adapters
│   ├── deadreckon-sandbox/       # platform-native sandboxes
│   └── deadreckon/               # CLI binary + dr-gate binary + tests
├── skills/
│   └── default-coding/SKILL.md
├── tests/                        # (currently empty; per-crate tests live in each crate)
├── docs/
│   ├── AS-BUILT-ARCHITECTURE.md  # this file
│   ├── GAP-ANALYSIS.md
│   ├── MULTI-RUN.md
│   ├── RESUME-SEMANTICS.md
│   ├── V1-CANDIDATES.md
│   └── goals/                    # eight dated goal/rider files
├── demo.cast                     # asciicast of an end-to-end smoke
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
│               ├── history.json   # narrative tool-call summaries
│               └── acceptance.yaml   # optional spec for dr-gate
├── locks/
│   └── <scope>--<task_key>.lock   # one lock per task
└── library/
    └── <scope>/
        └── <run_id>/
            ├── manifest.json      # PromotionManifest
            └── ...                # promoted working tree
```

The split between `runstate/` (mutable working state) and `library/` (durable promoted artifacts) is deliberate: `runstate/` is per-scope and ephemeral; `library/` is global and intended to outlive cleanup. The `state.working_dir` field starts pointing at `runstate/.../working/` and is rewritten to `library/<scope>/<run_id>/` after promotion.

### 5.3 Path derivation

`crates/deadreckon-core/src/paths.rs:9-65` exposes:

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
}
```

Scope (`paths.rs:67-86`) derives from `DEADRECKON_SCOPE_ROOT` env var, or the nearest `.git` root, or `cwd`. The literal scope string is `"<sanitized-basename>-<fnv1a32-hex>"` of the canonical path — unique per worktree, stable per checkout.

Task key (`paths.rs:88-99`) is `"<slug-of-goal>-<fnv1a32-hex-of-goal>"` (slug capped at 48 chars). Two runs with the same goal share a task key (and a lock).

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
2. `run_acceptance_gate(state)` succeeded (invoked the external `dr-gate` binary), **and**
3. `validate_acceptance_marker(state)` succeeded (signature + run_id check), **and**
4. `promote_if_ready(state)` swapped `working/` → `library/<scope>/<run_id>/`, **and**
5. `set_phase_status(PhaseId(60), Completed)` ran (which is the only path to `RunStatus::Completed`).

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
3. If `EWOULDBLOCK`, read existing `LockState` from disk. If owner matches **or** lock is stale, proceed. Otherwise refuse.
4. Stale detection: `acquired_at` age > `DEFAULT_STALE_AFTER` (30 min, `lock.rs:13`) **or** `pid_is_alive(pid)` returns false. PID liveness is `nix::sys::signal::kill(pid, 0)` (`lock.rs:235-245`): `ESRCH` → dead, anything else → alive.
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

`crates/deadreckon-core/src/promotion.rs:14-23`:

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

`promotion.rs:25-63`:

1. **Guard.** `validate_acceptance_marker(state)?` — refuses if marker missing / wrong run_id / unsigned.
2. **Recovery.** `recover_promotion()` — idempotent if a previous attempt half-completed (§8.4).
3. **Idempotency check.** If `library/<scope>/<run_id>/manifest.json` already exists, update state and return — no work to do.
4. **Staging.** Create `library/<scope>/.{run_id}.promoting/` (parent dir created if needed).
5. **Move.** `fs::rename(working_dir, staging)` — atomic on same filesystem.
6. **Manifest.** Write `manifest.json` inside staging.
7. **Final rename.** `fs::rename(staging, library/<scope>/<run_id>/)` — atomic.
8. **State update.** `state.working_dir = library_dir`; `state.promoted_library_dir = Some(library_dir)`; `save_state()`.

### 8.3 Where promotion happens

In `crates/deadreckon-runtime/src/turn_loop.rs`, **before** `set_phase_status(PhaseId(60), Completed)`. If promotion fails, the run never reaches `Completed`. The `working/` directory is the source of truth until promotion; after promotion, the library copy is canonical and `working/` is gone.

### 8.4 Crash recovery between rename steps

`promotion.rs:65-84` handles the half-completed states:

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
}
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
load_or_reconstruct_history()      # lines 69, 550-631
set Phase(40)=Executing            # line 70
save_state(); save_history()       # line 71

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
    response = router.complete(&request).await
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
        run_acceptance_gate(state)        # dr-gate
        validate_acceptance_marker(state) # signed?
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
            run_acceptance_gate; validate_marker; promote_if_ready
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

`ProviderRequest` (`lib.rs:83-91`) carries: `prompt`, `max_output_tokens`, optional `cwd`, optional `output_path`, optional `sandbox_backend`, optional `pid_file`, optional `cancellation_token`.

`ProviderResponse` (`lib.rs:94-102`) carries: `provider`, `model`, `content`, `usage`, `spend`, `trace` (JSON value).

### 10.2 Kinds

`lib.rs:52-61`:

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

A single `ProviderAdapter` (`lib.rs:135-323`) handles all three HTTP kinds via shared `reqwest::Client`:

| Kind | Endpoint | Auth header | Default model |
|---|---|---|---|
| Anthropic | `{base_url}/v1/messages` | `x-api-key: <key>` + `anthropic-version: 2023-06-01` | `claude-sonnet-4-5` |
| OpenAI | `{base_url}/chat/completions` | `Authorization: Bearer <key>` | configurable |
| OpenAI-compatible | `{base_url}/chat/completions` | `Authorization: Bearer <key>` | configurable |

Pricing defaults (`lib.rs:531-617`): Anthropic $3/$15 per million in/out; OpenAI $1.25/$10 per million; OpenAI-compatible $0/$0 (user-configured).

Response parsing: `parse_anthropic_response` (`lib.rs:688-705`) extracts `content[0].text` + `usage.{input_tokens, output_tokens}`; `parse_openai_response` (`lib.rs:669-686`) extracts `choices[0].message.content` + `usage.{prompt_tokens, completion_tokens}`. The `Action` tag-typed enum is parsed in the **turn loop**, not the provider; providers return text.

Cancellation: `tokio::select!` on `token.cancelled()` vs `client.post().send()` (`lib.rs:226-263`).

### 10.4 CLI sub-agent adapters

**`cli:claude-code` (`crates/deadreckon-providers/src/cli_claude_code.rs:1-127`).** Invocation:

```zsh
claude --dangerously-skip-permissions -p "<prompt>"
```

The `--dangerously-skip-permissions` flag is non-negotiable (no human is in the loop). The subprocess runs **inside** the deadreckon sandbox profile, so Claude's bypass only disables its own permission gate; deadreckon's outer Seatbelt/bwrap still scopes the process. Stdout is captured to `request.output_path` (the turn's `claude.out`).

**`cli:codex` (`crates/deadreckon-providers/src/cli_codex.rs:1-143`).** Invocation:

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

`crates/deadreckon-providers/src/router.rs`. Reads config (TOML), loads the provider registry with `providers.d` overrides, resolves a route list (`fallback` array > `default_provider` > built-in chain `cli:claude-code` → `cli:codex` → `anthropic` → `openai` → `openai-compatible`), and constructs a `Box<dyn Provider>` per route. Concrete providers handle Anthropic/OpenAI/OpenAI-compatible/smoke/Codex/Claude; descriptor-backed generic CLI providers handle any registered CLI descriptor that does not need a concrete adapter. On `complete()`:

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

`lib.rs:208-244`. Platform-conditional:

- macOS: probes `sandbox-exec` via `which`. Falls back to `None` with a warning if unavailable.
- Linux: probes `bwrap`. Falls back to `None` with a warning.
- Other: `None` with platform-unavailable warning.

The fallback to `None` is loud — the warning ends up in `SandboxRunOutput.warning` and is surfaced in the trace.

### 11.3 `run(SandboxSpec) -> SandboxRunOutput`

`lib.rs:111-169` is the single dispatch entry point. It:

1. Calls `build_command(spec)` to construct the per-backend invocation.
2. Spawns the child via `tokio::process::Command`, capturing stdout + stderr piped.
3. Persists the child PID to `spec.pid_file` (line 129) for `kill` supervision.
4. Reads stdout/stderr in parallel async tasks (`read_pipe`, lines 171–181).
5. Runs the cancellation `tokio::select!` (lines 131–151).
6. Returns `SandboxRunOutput { stdout, stderr, status_code, pid, backend, warning }`.

### 11.4 macOS Seatbelt profile

`lib.rs:390-441` generates a per-run profile string:

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

Optionally writes the profile to `spec.profile_dir` for debugging (`lib.rs:436-439`). Otherwise inline via `sandbox-exec -p '<profile>' -- <program> <args...>`.

### 11.5 Linux Bubblewrap

`lib.rs:303-357`. Constructs `bwrap` args with:

- `--die-with-parent --unshare-pid --unshare-ipc --unshare-uts`
- `--tmpfs <cwd>/.deadreckon-home` and `--setenv HOME <cwd>/.deadreckon-home` (ephemeral tmpfs `$HOME`)
- `--ro-bind <path> <path>` for each entry in `system_read_allowlist(cwd, spec.read_allowlist)`
- `--bind <path> <path>` for each entry in `spec.write_allowlist`
- `--bind <cwd> <cwd>`, `--proc /proc`, `--dev /dev`, `--chdir <cwd>`
- `--unshare-net` unless `allow_network=true`

### 11.6 Docker

`lib.rs:359-388`. Constructs `docker run --rm -v <cwd>:<cwd> -w <cwd> [--network none] [-e KEY=VAL]... rust:1 <program> <args...>`. Hardcoded base image is `rust:1`. Only the cwd is mounted.

### 11.7 None

`lib.rs:191-203`. No isolation. Always returns a warning: `"sandbox backend none is unsafe; use only for explicit local verification"`. The warning lands in the trace.

### 11.8 SIGTERM/SIGKILL escalation

`lib.rs:131-151`:

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

`signal_pid` is defined at `lib.rs:473-483` (uses `nix::sys::signal::kill`).

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

`kill_command` (`main.rs:885-921`) reads both, releases the lock, sets `state.status = Killed`, and signals every PID via `terminate_pid(pid, force)`. Non-force: graceful (default signal), wait 1.5s, then SIGKILL anything still alive.

### 12.3 What `kill` cannot do

`kill` does **not** flip a cancellation token in a running deadreckon process. If a deadreckon `run` is still executing, `kill` only signals its child PIDs; the deadreckon process itself relies on its own cancellation token tree, which `kill` doesn't reach across processes. In practice this works because killing the children causes the providers/sandbox to error out, which the turn loop sees and propagates as a failure — but the seam is not as clean as it could be (this is named in the robustness rider as a hardening target).

---

## 13. Acceptance Gate & Anti-Self-Attestation

### 13.1 The principle

The agent (LLM) **cannot** be trusted to declare a run done. `Completed` is reachable only via an acceptance marker signed by an external binary that the agent does not have keys to.

### 13.2 `AcceptanceMarker`

`crates/deadreckon-core/src/gate.rs:16-28`:

```rust
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,           // "pass" | "fail"
    pub produced_by: String,      // must be "dr-gate"
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
    pub signature: String,        // hash over fields + nonce
    pub check_count: usize,
}
```

Marker location: `<run_root>/proofs/turn-acceptance.json`.

### 13.3 `dr-gate` binary

`crates/deadreckon/src/bin/dr-gate.rs` (33 lines). Standalone binary that:

1. Reads `--run <id>` and `--working-dir <path>`.
2. Loads `acceptance.yaml` (if present) from the run root.
3. Runs each check (cargo test, file-exists, content-match, build-success).
4. If all required checks pass, writes a signed `AcceptanceMarker` to `proofs/turn-acceptance.json`.

The marker's `signature` is computed from `gate/nonce` (a UUID written at run-init time by the deadreckon binary at `state.rs:188`). Only `dr-gate` reads the nonce; the agent never has access.

### 13.4 Validation

`gate.rs:88-118`:

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

A forged marker by the agent fails the signature check at line 114.

### 13.5 Streaming progress

While checks are running, `evaluate_acceptance_checks_with_progress` (`gate.rs:228-244`) appends one `AcceptanceProgressEntry` per state transition to `proofs/acceptance-progress.jsonl`:

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

### 13.6 Where the gate is invoked

The CLI sub-agent path and regular Done path in `crates/deadreckon-runtime/src/turn_loop.rs` both call:

```rust
run_acceptance_gate(state)?;
validate_acceptance_marker(state)?;
promote_if_ready(state)?;
```

The first call invokes `dr-gate` as a subprocess. The second validates what `dr-gate` wrote. The third atomically swaps the working tree into the library.

Failure at any step prevents the run from reaching `Completed`.

---

## 14. Telemetry: Spend, Traces, Provenance, Events

Five append-only JSONL files capture every run's history. Four (`spend.jsonl`, `traces.jsonl`, `provenance.jsonl`, `events.jsonl`) live under `<run_root>/` directly; the fifth (`proofs/acceptance-progress.jsonl`, see §13.5) is gate-scoped and truncated per evaluation. All are written via `append_json_line` (`state.rs:375-388`), which opens in append mode and `sync_all`s after each line.

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

One line per LLM call. HTTP providers fill in token counts and USD; CLI providers fill in `subscription: true` + `wall_time_seconds`.

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

Structured `RunEvent` log: `TurnStarted { turn }`, `ToolCallStarted { tool_call_id, kind }`, `ToolCallResult { tool_call_id, status, latency_ms }`, `RunCompleted { outcome }`. Also published on a `tokio::sync::broadcast` channel (`events.rs`) for in-process subscribers (the TUI uses this when implemented; today the TUI polls files instead — see §22).

---

## 15. Resume Semantics

`crates/deadreckon/src/main.rs:940-1000` is the `resume_command` handler.

### 15.1 The Completed guard

`main.rs:947`:

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

`deadreckon import <source>` reads from external coding-agent histories and synthesizes a deadreckon run from them. Today's coverage:

| Source | Path (overridable via env) | Format |
|---|---|---|
| `claude-code` | `~/.claude/projects/` (`DEADRECKON_IMPORT_CLAUDE_ROOT`) | JSONL |
| `codex` | `~/.codex/sessions/` (`DEADRECKON_IMPORT_CODEX_ROOT`) | JSONL |
| `cursor` | `~/.cursor/chats/` (`DEADRECKON_IMPORT_CURSOR_ROOT`) | SQLite |

The handler creates an `imported-<hash>` run, parses the source, appends entries to `traces.jsonl` + `provenance.jsonl`, marks the run `Completed` (skipping the gate), and never writes back to the source. Current coverage is **inventory-level**: it produces a listing/summary but doesn't deeply normalize all fields. Round-trip parity (import → `show <id>` → render comparable to source) is a hardening target — see `docs/goals/2026-05-11-1400-deadreckon-robust-rider.md` §7.

---

## 17. CLI Surface

The `Commands` enum in `crates/deadreckon/src/main.rs` defines the CLI surface. Handlers and roles:

| Verb | Handler | Role |
|---|---|---|
| `init` | `main.rs:241` | Interactive setup of `~/.deadreckon/config.toml` |
| `config get/set` | `main.rs:279` | Non-interactive TOML edits |
| `run` | `main.rs:316` | Create + enter turn loop |
| `doctor` | `main.rs:484` | 8-point actionable preflight |
| `status` / `next` | `main.rs` | Current project's latest run, locations, and next action |
| `list` | `main.rs` | Project-scoped run inventory by default; `--all` for global history, `--full` for exact values |
| `apply` | `main.rs` | Apply a completed worktree run to the user's current branch |
| `abandon` / `discard` | `main.rs` | Remove a run's worktree branch/path or mark no-op modes abandoned |
| `materialize` / `export` | `main.rs` | Copy a completed fresh/copy artifact to a normal directory |
| `cleanup` / `prune` | `main.rs` | Clean abandoned, stale, or selected completed worktree runs |
| `attach` | `main.rs:874` | TUI on a live or completed run |
| `kill` | `main.rs:885` | Lock release + child PID termination |
| `resume` | `main.rs:940` | Re-enter the loop on a non-Completed run |
| `undo` | `main.rs:1002` | Restore snapshot to a target turn |
| `show` | `main.rs:1025` | Pretty-print full state + provenance + traces |
| `import` | `main.rs:1056` | Read-only import from claude/codex/cursor |
| `chain` | `main.rs:1246` | Create, plan, run, attach, pause/resume/kill, undo, extend, and redo serial autonomous chains |
| `completion` / `completions` | `main.rs` | Generate or install shell tab-completion scripts (bash, zsh, fish, elvish, powershell) |

The CLI defaults are honest: `--sandbox` defaults to `auto`, `--max-spend` defaults to `$10` (with a confirmation gate above `$50`), `--provider` defaults to the highest-credentialed entry per the fallback chain, `--skill` defaults to `default-coding`.

`run` now starts codebase-aware by default. In a git repo it previews and then creates a `git worktree` on a `dr/...` branch; `--fresh` preserves the old empty-working-dir behavior, `--from <path>` uses copy mode, and `--in-place --i-know-its-a-lot` edits the source tree directly. Completed worktree runs hint `apply` / `discard`; copy and fresh runs hint `export` / `extend`. Run-id arguments accept unique prefixes and `latest` / `last` resolves to the latest run in the current project scope.

`completion install` is driven from the real clap command tree, so subcommand aliases, flags, and value-hint completions stay in sync with `deadreckon --help`. The handler detects the active shell via `$SHELL`, writes the script to a per-shell default path (e.g. `~/.zsh/completions/_deadreckon`, `~/.local/share/bash-completion/completions/deadreckon`), and for zsh adds a managed `# deadreckon completion` block to `~/.zshrc` unless `--no-rc` is passed. `init` invokes `try_install_completion_after_init` so first-time setup ships completions opt-out (`init --no-completion`). The per-shell stdout variants (`completion bash|zsh|fish|elvish|powershell`) print the script for users who manage their own shell config.

`run` startup details (`print_run_started`) are now also emitted at the top of `extend` and `resume` so extended/resumed runs surface their selected provider route and doc-provider source the same way fresh runs do. Interactive terminals receive a `deadreckoning_course` ASCII progress strip and a polled `cli_wait_status` line while a long turn is in flight; the status is cleared as soon as the loop reports back. `kill` against a loaded run now also persists `RunStatus::Killed` + `killed_at` + `failure_reason = "killed by user"` before returning so downstream tooling sees a consistent terminal state.

---

## 18. TUI (`attach`)

`crates/deadreckon/src/main.rs:874-1289` houses `attach_command` plus the rendering helpers.

### 18.1 Behavior

- On a TTY: `attach_tui()` enables raw mode, alternate screen, and renders a `ratatui` UI.
- Off-TTY: prints a plain-text summary + locations.

### 18.2 Layout

`attach_panel_layout` (`main.rs:~10225`):

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
- **Acceptance meter**: derived from `AcceptanceLive` (`collect_acceptance_live` in `main.rs:~10172`). When `proofs/acceptance-progress.jsonl` exists, the panel tails it and surfaces `running 2/5`, `passed`, or `failed` with the offending check; once `turn-acceptance.json` is signed, it pivots to a marker view (`acceptance_live_from_marker`). Color thresholds are owned by `acceptance_color`.
- **Center, left**: wide streaming list of tool calls + provider activity + recent events. Acceptance lines from `acceptance_activity_lines` are interleaved so the operator sees the same progress in the activity stream and the meter.
- **Completed docs view**: pressing `d` toggles the center-left panel from provider activity to `RUN-NARRATIVE.md` rendered through `pulldown-cmark` into ratatui `Line`/`Span`s. Headings, bullets, inline code, fenced code blocks, links, task markers, math, and horizontal rules receive terminal styles and remain scrollable.
- **Center, right**: narrower live files list with count/bytes in the panel title.
- **Bottom**: supervised PIDs + their `ps` lines (alive/dead annotation).
- **Footer**: action-first completed footer (`[d] Docs` / `[d] Activity`, `[a] Apply`, `[b] Abandon`, `[s] Show`) or scroll/detach help while running. The footer's second line carries `deadreckoning_status_line` while long operations are in flight.

### 18.3 Data source

Today the TUI **polls** files on disk every 500 ms: `spend.jsonl`, `traces.jsonl`, `events.jsonl`, `proofs/acceptance-progress.jsonl`, `proofs/turn-acceptance.json`, plus provider-native logs when the active provider writes them. `collect_provider_activity` now resolves provider ingest through descriptor `[ingest]` metadata: candidate roots, env overrides, cwd matching, storage kind, file glob, freshness window, and schema key. `cli:codex` reads `~/.codex/sessions/**.jsonl` and matches `session_meta.payload.cwd`; `cli:claude-code` reads `~/.claude/projects/<cwd-slug>/*.jsonl` using Claude Code's path-to-project mapping and matches top-level `cwd`; `cli:gemini` reads Gemini JSON/JSONL file logs; `cli:opencode` reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON; `cli:copilot` reads `~/.copilot/session-state/*.jsonl` plus nested `events.jsonl` and matches `data.context.cwd`; `cli:pi` reads `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`, validates the first nonblank row is a Pi `session`, and matches the header `cwd`. The shared collector handles candidate discovery, recency filtering, run matching, stream capping, and context telemetry. Schema-specific adapters only decode rows into common activity lines (`agent`, `thinking`, `tool`, `result`, `todo`, `tokens`) and normalize tool labels through `deadreckon_providers::taxonomy`.

Same-process attaches use the `RunEventBus` broadcast channel directly; cross-process attaches still poll. The mixed model is acknowledged in the robustness rider as a future consolidation target.

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
provider route and model before state is created; `run --model <model>` and
`extend --model <model>` override one run; `deadreckon config provider
<route>` and `deadreckon config model <model> --provider <route>` persist the
defaults. CLI providers pass explicit model overrides through to the underlying
tool (`codex exec --model ...`, `claude --model ...`) and otherwise display
`provider default`.

Route resolution (`configured_route_names` in `crates/deadreckon-providers/src/router.rs`) now puts `default_provider` at the head of the chain, then appends `fallback` entries that aren't already present, then falls back to the built-in chain (`cli:claude-code` → `cli:codex` → `anthropic` → `openai` → `openai-compatible`) only if neither is configured. `read_config` (`config.rs`) backfills `default_provider` from a top-level `[defaults] provider` key when it's omitted, so the same TOML stanza drives both `init`-style defaults and the router. `--provider` on the CLI still short-circuits the whole chain.

### 19.2 BYOK posture

Three credential paths:

1. **HTTP key.** Set `api_key` directly or `api_key_env = "FOO"` to read at runtime.
2. **CLI subscription.** Run with `--provider cli:claude-code` or `cli:codex`. The binary's presence in `$PATH` is the credential. No key required.
3. **OpenAI-compatible.** Plug an OpenRouter or `llama.cpp` endpoint into `base_url` + `api_key`.

`deadreckon init` (`main.rs:241-277`) walks the user through option (1) or (2): it detects `claude` and `codex` in `$PATH` and offers them as default providers before asking for keys.

---

## 20. Testing Strategy

### 20.1 Test locations

- `crates/deadreckon-providers/tests/cli_providers.rs` (297 lines) — CLI provider routing, fake `claude`/`codex` binaries, output capture, spend.
- `crates/deadreckon-providers/tests/mock_server.rs` (220 lines) — axum-based OpenAI-compatible mock for HTTP provider tests.
- `crates/deadreckon-providers/src/lib.rs` (inline) and focused module tests — config parsing, spend math, credential check, smoke determinism.
- `crates/deadreckon/tests/agentic_loop.rs` (841 lines) — end-to-end: run, kill, resume, list, attach, import, doctor, stress.

### 20.2 Notable integration tests

| Test (in `agentic_loop.rs`) | What it proves |
|---|---|
| `mock_provider_records_three_turns_and_artifacts_match` (line 19) | mock-driven run produces 3 turns, ≥ 5 trace lines, 3 spend lines, signed acceptance marker, working files |
| `kill_mid_turn_sets_killed_and_stops_process` (line 65) | kill interrupts within 2 s, sets `Killed` |
| `resume_preserves_history_file` (line 103) | resume preserves `history.json` tool_call_ids |
| `cli_subagent_without_file_changes_fails_run` (line 134) | CLI provider with no file effects → `Failed` |
| `init_config_and_default_spend_work` (line 180) | `init` writes config; `config get/set` works; `--smoke` respects defaults |
| `high_spend_requires_confirmation_flag_in_scripts` (line 246) | `--max-spend 51` without `--i-know-its-a-lot` fails with a hint |
| `cli_wall_clock_budget_enforced` (line 266) | subscription run pauses at `--max-wall-seconds` cap |
| `kill_storm_no_leaks` (line 334) | 10 concurrent kills release all PIDs + locks |
| `doctor_fails_actionably` (line 408) | `doctor` output contains specific fix commands |
| `import_{claude_code,codex,cursor}_roundtrip` (lines 425+) | import normalizes external histories |
| `stress_5_concurrent_10min` (line 470, gated `DEADRECKON_STRESS=1`) | 5 concurrent scoped runs complete cleanly |

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

The codebase is more complete than a typical first pass, and the 2026-05-11 hardening pass replaced the earlier thin seams with depth-tested implementations where those seams were in alpha scope. Honest accounting per `docs/CHANGELOG.md`, `docs/GAP-ANALYSIS.md`, and `docs/AUDIT-2026-05-11.md`:

### Built and reliable

- Workspace, crates, build, lint, fmt, test discipline.
- Workspace lint discipline (deny-tier clippy + rustc), tuned release profile, registry-shaped library `lib.rs`, library print refusal, and error retryable/fatal taxonomy as vocabulary for future watchdog work.
- `PipelineState` shape, phase machine, atomic state writes, schema version.
- PID-aware locks + heartbeats + stale reclaim.
- Atomic working→library promotion with crash recovery.
- Sandbox dispatch for sandbox-exec / bwrap / docker / none + auto resolution.
- HTTP providers (Anthropic / OpenAI / OpenAI-compatible) with token-based spend.
- CLI providers (`cli:claude-code`, `cli:codex`) with wall-clock subscription spend.
- Descriptor-backed CLI providers with generic `exec_template` launch, registry-driven detection/init/listing, descriptor sandbox writes, and built-in `cli:gemini`, `cli:opencode`, `cli:copilot`, and `cli:pi` providers.
- Smoke provider (deterministic) for keyless tests.
- Turn loop with action parsing (Bash / WriteFile / Done) and CLI sub-agent path.
- Codebase-default running: worktree mode, copy mode, in-place mode, fresh-mode preservation, preflight + preview UX, and `codebase.json` files-not-fields metadata.
- `apply` and `abandon` for worktree rollback/apply lifecycle.
- `materialize`, `extend`, `undo`, `list`, and `show` integration with codebase mode metadata, including worktree extension branches chained from parent `dr/...` branches.
- UX consolidation: project-scoped `list`, `latest` run aliases, `status`/`next`, `cleanup`/`prune`, `export`/`discard` aliases, and TTY-aware formatted output.
- Self-documenting run artifacts in stoa shape: `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, optional `AS-BUILT-DELTA.md`, per-turn `_incremental.jsonl`, explicit `docs_checkpoint` run events, and `polish.json` schema v2.
- `deadreckon doc`, `list` DOCS status, doc-aware `apply` commit bodies, extend-parent narrative updates, diff coverage retry, the legacy repo/user/project `run-narrator` skill mechanism, and default split polish skills (`narrator-overview`, `narrator-phases`, `narrator-as-built`, `narrator-decisions`).
- Acceptance gate with signed marker; anti-self-attestation actually enforced.
- `init`, `config get/set`, `run`, `doctor`, `status`/`next`, `list`, `attach`, `kill`, `resume`, `undo`, `show`, `import`, `cleanup`/`prune`, `completion` verbs.
- Shell tab-completion via `completion install` / `completion {bash,zsh,fish,elvish,powershell}` driven from the live clap command tree; `init` opt-out installs completions and (for zsh) appends a managed `.zshrc` block.
- `ratatui` attach TUI with spend/context/acceptance telemetry, provider activity, in-TUI Markdown docs rendering, live files, process panel, scrollable panels, and completion action footer. Long operations surface a `deadreckoning` ASCII status line in CLI and footer alike.
- Descriptor-driven provider activity ingest for Codex, Claude Code, Gemini JSON/JSONL, OpenCode file-mode logs, GitHub Copilot CLI session-state JSONL, and Pi session JSONL, normalized into `agent` / `thinking` / `tool` / `result` / `todo` / `tokens` rows without rewriting provider-owned logs.
- Streaming acceptance progress: `proofs/acceptance-progress.jsonl` reports per-check `started`/`running`/`passed`/`failed` transitions while `dr-gate` is mid-evaluation; the attach TUI tails it alongside the signed marker.
- Extended runs carry the parent's `acceptance.yaml` into the child run and emit the same `print_run_started` startup details (provider route, doc-provider source) as fresh runs; resume does the same.
- `--max-spend` cap with pause-at-cap; `--max-wall-seconds` for subscription providers.
- Event-backed TUI attach: same-process attaches use `RunEventBus`; cross-process attaches replay `events.jsonl` incrementally.
- Cross-process cancellation: `kill` writes a durable cancel marker before signaling; the run loop observes it while provider calls are in flight and reports killed status through events.
- Partial-trace resume: resume reconstructs only completed tool boundaries and `resume --from-turn` truncates traces, spend records, and future snapshots together.
- Durable per-run `sandbox.toml` plus per-tool sandbox policy: bash/write-file paths get specific filesystem and network permissions; refusals include `try:` and are recorded in traces and provenance.
- YAML acceptance specs: `dr-gate` evaluates required/optional tests, file existence, content matches, shell commands, and build checks, then signs check-level proof results.
- Exhaustive local doctor: OS, sandbox binaries, provider binaries, config, runstate permissions, disk, and opt-in provider pings all produce actionable `try:` hints.
- Promoted library query surface: `deadreckon library list|search|show` reads library manifests and reverse materialization markers, filters by goal/date, and searches promoted run docs.
- Import parity hardening: Claude Code/Codex JSONL and Cursor SQLite imports preserve source metadata, deterministic run IDs, stable row ordering, and provenance paths; committed goldens cover normalized `show` output.
- CLI usability polish: root help includes command groups, `status` includes run health/library/disk blocks, and `DEADRECKON_HINTS=0` suppresses post-completion prompts.
- Autonomous sequential chains: `chain "..."`, `chain plan`/`expand`, `chain run`, `chain attach`, `chain status/show/list`, `chain pause/resume/kill`, `chain undo`, `chain extend`, and `chain redo`; chains use `latest`/`last` aliases, `chain.json`, `chain-events.jsonl`, a conductor lock, chain hooks, aggregate spend caps, green-policy auto-apply, and a multi-step ratatui timeline with single-run chain context.
- Mock HTTP server for tests; CLI provider tests with fake binaries; integration coverage for stress, import round-trips, lifecycle, codebase modes, docs, sandbox policy, and gate proof.

The hygiene rider is purely structural; it does not close prior thin items, but it raises the floor for every future rider.

### Hardening v2 closures

The previously named thin areas now have code paths and depth tests:

1. **TUI streaming.** `tui_events.rs` covers broadcast attach, JSONL replay, partial-line handling, and kill visibility.
2. **Resume from partial trace.** `turn_loop` tests cover mid-tool-call truncation and `--from-turn` cleanup.
3. **Cancellation model.** `kill` writes a cancel marker before signals; tests cover cross-process marker semantics, HTTP aborts, and kill storms.
4. **Wall-clock spend for CLI providers.** CLI providers accumulate wall time and caps; richer subscription-to-budget policy remains a future routing concern.
5. **Sandbox profiles.** `sandbox.toml` drives per-tool policy; policy blocks disallowed filesystem/network access and records refusals.
6. **Doctor.** Local setup checks are actionable and exhaustive for alpha; provider network pings are opt-in.
7. **Import normalization.** JSONL/SQLite imports now carry source path/line/row metadata and deterministic imported run IDs, with golden-file `show` round trips.
8. **Acceptance gate.** `acceptance.yaml` supports structured checks and signed per-check results.
9. **Multi-run coordination.** Scope-qualified locks, stale reclaim, same-scope refusal tests, and sequential chain coordination are in place; parallel/DAG scheduling remains out of scope.
10. **Promotion / library workflow.** Promotion is atomic and `library list|search|show` makes artifacts discoverable by scope, goal, date, and promoted-doc content.

### Not yet built (V1+ candidates per `docs/goals/2026-05-11-1400-deadreckon-usability-rider.md` and the V1 list in the robust rider)

- Sub-agent forking as a user-facing CLI verb.
- Hook system (pre/post tool call).
- MCP client surface.
- Cost-aware provider routing.
- Cloud sync of histories.
- Voice / meeting capture.
- Real-time multi-cursor TUI presence.

The codebase-mode rider adds capability; it does not close the robust-rider thin items above.

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

`cleanup` (alias `prune`) removes worktrees and temporary branches for already
abandoned runs by default, with opt-in `--completed`, `--stale`, `--all`, and
`--force` selectors. It leaves promoted library artifacts intact.

### 24.11 Integration With Existing Verbs

`materialize` refuses worktree runs with an `apply` hint and refuses in-place runs
with an `undo` hint. `list` shows `MODE`. `show` prints mode, branch, worktree,
and source lines. `undo` restores the original source path for in-place runs.
`extend` chains worktree children from the parent `dr/...` branch and records
`parent_branch` in the child's `codebase.json`; copy/fresh extension keeps the
library-seeding path, and in-place parents refuse with a `run --in-place` hint.
Extend now also carries the parent's acceptance spec into the child run via
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

Every run starts `working/.deadreckon/docs/` and writes three human-readable Markdown files:

- `RUN-NARRATIVE.md` for the chronological implementation story.
- `RUN-AS-BUILT.md` for the subsystem shape changed by the run.
- `RUN-DECISIONS.md` for detected multi-alternative decisions.

When the worktree has a nearby `AS-BUILT-ARCHITECTURE.md` or `AS-BUILT.md` and the diff is broad enough, deadreckon also emits `AS-BUILT-DELTA.md` as a proposed amendment.

### 25.2 Frontmatter

The docs use stoa-style bold frontmatter: Date, Last updated, Status, Run ID, Goal, optional Parent run, Commit span or working-directory mode, Owner, Provider, Sandbox, Spend, and Doc-writer. Fresh runs omit commit span; copy and in-place runs identify their working path.

### 25.3 Per-Turn Templating

After every successful tool/provider turn, `crates/deadreckon-runtime/src/turn_loop.rs` calls the turn-end documentation checkpoint. The deterministic record lands in `_incremental.jsonl`, rewrites the Markdown drafts, and emits a `docs_checkpoint` run event before the loop advances. This happens for both CLI sub-agent turns that complete in one provider process and JSON-action providers that may take many Bash/WriteFile/Done turns.

Each turn record carries the full provider response capped at 50 KB, a short response summary, per-file add/delete counts, largest diff-hunk excerpts, binary markers, optional stdout/stderr samples, trace citation, snapshot reference, and worktree commit SHA when available.

### 25.4 End-of-Run Polish Pass

Before acceptance/promotion, `polish_run_docs` first writes deterministic docs, then optionally runs provider-backed polish unless `--no-docs` is set. The default path resolves four repo/user/project skills in order: `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions`. Each subcall receives the same run evidence plus a focused prompt, uses a 16K output-token budget by default, retries once on malformed JSON, and contributes to the merged docs.

The legacy `run-narrator` single-call path remains available for custom installs that do not opt into split `doc_subskills`. Provider, JSON, or skill failures are nonfatal; templated docs remain and `polish.json` records `failed_subcall:<name>` when a split subcall failed. `--smoke` now implies `effective_no_docs` unless `--doc-provider` is explicitly passed, so deterministic smoke runs no longer attempt to call a live doc provider. `print_status_card` reads `polish.json` when `docs_status_for_state` reports `Failed` and surfaces a `polish failed (<reason>); fallback docs are still available` line so the templated docs are visibly distinct from a successful polish.

### 25.5 Phase And Decision Detection

`docs.rs` coalesces turns into 3-8 phases by file overlap and tool-kind continuity. Decision candidates are detected with case-insensitive marker regexes and a minimum response length so incidental short phrases do not become decisions.

### 25.6 Diff Coverage And Retry

After polish, deadreckon verifies every changed file appears in `RUN-NARRATIVE.md` by relative path or basename. Missing files trigger up to two targeted `narrator-phases` retries with an explicit omission list; other subskills are not re-run for phase coverage misses. Remaining omissions are logged as `docs.warning` traces and do not fail the run.

### 25.7 AS-BUILT-DELTA

The delta is generated for worktree runs whose source has an AS-BUILT file at the root or beside touched files and whose diff touches at least three files or adds public/exported API. Public docs are copied to `working/docs/`; the branch gets a `turn docs: deadreckon run docs for <id>` commit so `apply` carries docs forward.

### 25.8 Apply Commit Body

When `deadreckon apply` builds the default squash or merge message, it reads `RUN-NARRATIVE.md` and `RUN-DECISIONS.md` to include an executive summary, phase list, decision count, open-thread count, and a `docs/RUN-NARRATIVE.md` trace pointer. `--message` still overrides the generated body.

### 25.9 `deadreckon doc`

`deadreckon doc <run-id>` prints the narrative by default. `--kind as-built|decisions|delta` selects another artifact, `--export <path>` writes it to disk, and `--force` overwrites exports or a prior polish result. `--polish` prints a preview listing provider, provider source, subskills, token budget, budget cap, and inputs hash before it calls the doc provider; `--no-confirm` skips the prompt for scripts. `--doc-provider <route>` overrides the automatic doc provider and `--budget-cap <usd>` limits the polish pass.

### 25.10 Cost And Idempotency

`polish.json` stores a SHA-256 inputs hash over goal, traces, provenance, spend, incremental records, changed files, and source AS-BUILT content. Schema v2 records `doc_provider_source`, `subcalls[]` with skill/status/provider/tokens/cost/duration/retries, `merged_at`, and `diff_coverage`. A matching polished hash skips duplicate provider calls unless forced. CLI subscription providers report wall time rather than USD cost, but the doc-provider resolver still records whether the route came from a flag, config, auto-detected subscription CLI, run provider fallback, or no provider.

### 25.11 Skill Split Into Four Subskills

The default polish path resolves `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions` separately so each prompt owns one documentation surface. The legacy `run-narrator` skill remains as the single-call compatibility path.

### 25.12 Per-Turn Capture Richness

Turn docs preserve the provider response up to 50 KB, stdout/stderr up to 10 KB each, per-file add/delete counts, binary markers, and the largest textual hunk excerpt. These fields are stored in `_incremental.jsonl` rather than `PipelineState`.

### 25.13 Doc-Provider Auto-Resolution

Doc polish chooses `--doc-provider`, then `[defaults].doc_provider`, then in-PATH subscription CLIs (`cli:codex`, `cli:claude-code`), then the run provider. If none resolve, the command fails with an actionable `try:` hint instead of silently leaving `Doc-writer: templated only`.

### 25.14 Component Inference And Topology

The deterministic as-built seed maps changed paths into concrete layers such as Rust crates, frontend components/routes, tests, documentation, manifests, migrations, and CI. Unmapped files are omitted instead of grouped under `Project files`; topology ASCII is emitted only when at least three top-level directories changed.

### 25.15 Polish Preview And Budget Cap

`deadreckon doc <id> --polish` estimates the maximum output-token cost before calling the provider. Paid API routes are refused when the estimate exceeds `--budget-cap` or `[defaults].doc_polish_budget_cap_usd`; subscription CLI routes estimate as `$0.00 (subscription)`.

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

`stack` bases step N+1 on the SHA applied by step N. `base` bases every step on the original chain base SHA. `merge` follows stack semantics but forces `apply --strategy merge`, producing merge commits instead of squash commits.

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

`chain-events.jsonl` is the chain audit log: created, step started, run completed, apply started, applied/refused, step failed, paused/resumed/killed/completed, undo, hooks, extend, and redo. `promotion.rs` also emits `RunPromoted { library_dir }`, so a chain can attach provenance to the promoted inner run artifact.

### 28.9 Lifecycle Verbs

`chain pause`, `resume`, `kill`, `undo`, `extend`, and `redo` compose with the existing run lifecycle. Undo reverts applied SHAs in reverse order. Extend inserts or appends a step and can reopen a completed chain when inserting. Redo chooses a specified step, the first failed step, or the latest applied step; applied-step redo requires `--reapply`, which reverts before requeueing.

### 28.10 TUI Surfaces

`chain attach <id>` opens a ratatui step timeline on TTYs and falls back to a plain snapshot off-TTY. The timeline shows policy, spend, step dots/statuses/run prefixes, recent chain activity, and controls for drill/show, redo, extend, pause, kill, detach, and scrolling. Single-run `attach` reads `.deadreckon/chain-step.json`, renders a chain context banner, and exposes `[c] Chain` to drill out to `chain attach`.

### 28.11 Spend And Budgeting

`--max-spend` is aggregate. Each pending step receives `(remaining cap)/(remaining pending steps)` as its inner run cap. The conductor reads the completed run's state and adds actual spend/wall time back into `chain.json`. `resume --max-spend-add` increases the aggregate ceiling; `--reset-breaker` clears the consecutive failure counter.

### 28.12 Not Yet Built

Out of scope for this alpha pass: mid-chain provider replanning, parallel/DAG steps inside one chain, cross-machine handoff, cloud sync, and a richer conflict-resolution UI. Those remain V1 candidates.

- **Trace** — every LLM call and every tool dispatch, with latency + structured detail.
- **CLI sub-agent** — a `cli:*` provider whose `complete()` invocation is one whole turn (the sub-agent does its own tool calls inside). Detected by `response.trace["kind"] == "cli_subagent"`.
- **dr-gate** — the standalone binary at `crates/deadreckon/src/bin/dr-gate.rs` that owns acceptance verification. The agent cannot impersonate it.
- **BYOK** — Bring Your Own Key. In deadreckon this extends to subscriptions: a Claude Max or ChatGPT Pro user can drive deadreckon via `cli:*` providers without an API key.

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

- `split`: `deadreckon plan <goal> --n <2..=6>` asks a read-only planner provider for task JSON, records planner/default-child/per-child providers, writes worker specs, and later `fork` starts independent ready tasks as a concurrent batch. Planner output must contain exactly the requested task count; single-task decompositions and values outside `2..=6` are refused before `plan.json` is saved.
- `review`: `deadreckon orchestrate <goal> --mode review --coder-provider <id> --reviewer-provider <id>` writes a coder task and a reviewer task. The reviewer is launched with `deadreckon extend <coder-run-id> ...` after the coder completes, so parent history and `extended_from_parent` trace lineage are preserved.

### 30.2 Plan Files

`crates/deadreckon-core/src/plan.rs` defines `Plan`, `PlanTask`, `PlanProviders`, `PlanMessage`, `PlanChildMarker`, and `CoordinatorState`. The durable layout is:

```text
~/.deadreckon/plans/<plan-id>/
  plan.json
  coordinator.json          # present only while fork is supervising
  messages.jsonl
  worker-specs/task-0.md
  summaries/task-0.md
  merge-working/
  merge-proofs/conflicts.json
```

Every child run receives an inline copy of its worker spec in the prompt. The spec includes root goal, exact task scope, provider, role, dependency context, capability preview, and hygiene rules such as staying within scope and not spawning subagents. At launch time the coordinator rewrites the spec for dependent tasks with completed predecessor summaries, so later children receive concrete run ids, summary paths, changed-file context, and predecessor status rather than only a bare dependency id.

### 30.3 Verbs

- `plan <goal>` writes `plan.json` and worker specs. It previews provider roles, capability hints, task labels, dependencies, and next actions.
- `fork <plan-id>` runs ready child tasks through `deadreckon run`, using distinct plan-child scopes via `DEADRECKON_SCOPE_ROOT`. It writes typed progress/blocker messages and child summaries.
- `merge <plan-id>` composes completed child library artifacts into a new promoted run. It fails on conflicting file contents by default; `--strategy prefer-child --prefer-child <idx>` records the conflict and chooses that child.
- `orchestrate <goal>` is the one-command wrapper. In review mode it performs plan -> fork -> merge end to end.
- `attach <plan-id>` opens a plan TUI on TTYs and renders a plain summary off-TTY. The TUI shows child panes with provider/role/status, run prefixes, dependency state, turn/status, spend or token accounting, latest trace activity, acceptance/gate state, summary paths, and coordinator messages; `Enter` drills into the selected child run.
- Headless flags are honored across this surface: `run --quiet` emits no success stdout, `run --plain --quiet` emits only the final plain status line, and `attach --plain` forces summary output instead of ratatui.
- `kill <plan-id>` reads `coordinator.json` and child run state to signal the coordinator and live children.
- `history grep <pattern>` searches durable trace or provenance JSONL, can restrict to a plan's child runs with `--plan <plan-id>`, and supports regex, scope, age, and limit filters.
- `show <id> --why-failed` explains the likely failure surface for a run or plan, including non-completed children, blocker messages, and recent trace errors.

### 30.4 Merge Artifact

Merge creates a normal promoted run so existing `materialize`, `library`, and run inspection paths keep working. The promoted library also gets `deadreckon-plan-manifest.json` with plan id, root goal, mode, provider roles, capability preview, task graph, child run ids, summaries, and recorded conflicts.

Generated run artifacts are intentionally excluded from merge composition: `.deadreckon/*`, `docs/RUN-*`, `target`, `node_modules`, `.next`, `dist`, and `build`.

### 30.5 Current Limits

The first orchestration milestone is usable but not the full rider endpoint. The plan TUI reads child state/traces from disk on refresh; a broadcast-backed plan event stream remains future work.

---

*This document is canonical for the alpha-tier reality of deadreckon. Future hardening passes (per the robustness rider) and feature passes (per the usability rider) will update sections 6, 9, 11, 13, 14, 18, and 22 in particular. Last regenerated by an agent team from a deep code map; cross-check against the current code before relying on any specific line number.*
