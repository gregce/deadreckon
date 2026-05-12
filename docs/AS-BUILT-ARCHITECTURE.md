# AS-BUILT-ARCHITECTURE.md

**Subject:** deadreckon — a long-running, BYOK, sandboxed agentic CLI harness in Rust
**Frame:** Reference specification for the **alpha-tier** as-built reality at `/Users/gdc/deadreckon/`. Modeled on `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` (the Printing Press).
**Last updated:** 2026-05-11
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

---

## 1. System Overview & Mental Model

deadreckon is a Rust 2024 CLI harness whose default flow is **unattended long-running coding tasks**: `deadreckon run <goal>` creates durable run state, picks a BYOK provider route, executes turns inside a platform-native sandbox, writes spend/provenance/trace records after every turn, and exits only when the LLM declares done, a budget caps the run, or the operator kills it. The CLI is the user-facing layer; the `deadreckon-core` library owns the deterministic primitives (state, locks, gates, snapshots, atomic file ops); `deadreckon-providers` and `deadreckon-sandbox` are pluggable layers underneath.

```
┌───────────────────────────────────────────────────────────────────────────┐
│ USER                                                                      │
│ deadreckon run "make a hello-world Rust binary" --max-spend 5             │
└────────────────────────────────┬──────────────────────────────────────────┘
                                 │
┌────────────────────────────────▼──────────────────────────────────────────┐
│ CLI LAYER (crates/deadreckon)                                             │
│   main.rs:316  run_command()                                              │
│   main.rs:940  resume_command()  ← refuses Completed at :947              │
│   main.rs:874  attach_command()  ← ratatui TUI                            │
│   main.rs:484  doctor_command()                                           │
│   main.rs:241  init_command()                                             │
│ ◆ clap parsing  ◆ ratatui rendering  ◆ post-run summary                   │
└────────────────────────────────┬──────────────────────────────────────────┘
                                 │ create_run → acquire_lock → run_turn_loop
                                 ▼
┌───────────────────────────────────────────────────────────────────────────┐
│ CORE LIBRARY (crates/deadreckon-core)                                     │
│                                                                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐ │
│  │ state.rs     │  │ lock.rs      │  │ promotion.rs │  │ turn_loop.rs │ │
│  │ ─Pipeline    │  │ ─PID-aware   │  │ ─atomic      │  │ ─the loop    │ │
│  │   State      │  │  locks       │  │  working→    │  │   load-      │ │
│  │ ─RunStatus   │  │ ─heartbeats  │  │  library swap │  │  bearing    │ │
│  │ ─PhaseId     │  │ ─stale       │  │ ─manifest.json│  │ ─snapshots  │ │
│  │              │  │  reclaim     │  │              │  │ ─dispatch   │ │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘ │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐ │
│  │ gate.rs      │  │ artifacts.rs │  │ paths.rs     │  │ events.rs    │ │
│  │ ─Acceptance  │  │ ─copy_tree   │  │ ─DeadreckonPaths│ ─RunEventBus │ │
│  │   Marker     │  │ ─snapshot    │  │ ─scope hash  │  │ ─broadcast   │ │
│  │ ─signature   │  │ ─append_*    │  │ ─task_key    │  │  channel     │ │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘ │
└──┬──────────────────────────┬─────────────────────────────────┬───────────┘
   │ ProviderRouter::complete │ run_sandbox(SandboxSpec)        │ tracing
   ▼                          ▼                                 ▼
┌─────────────────┐   ┌──────────────────────┐         ┌────────────────┐
│ PROVIDERS       │   │ SANDBOX              │         │ RUNTIME STATE  │
│ HTTP:           │   │ sandbox-exec (mac)   │         │ ~/.deadreckon/ │
│  anthropic      │   │ bwrap (linux)        │         │  runstate/...  │
│  openai         │   │ docker (opt-in)      │         │  library/...   │
│  openai-compat  │   │ none (passthrough)   │         │  locks/...     │
│ CLI:            │   │ auto (resolves)      │         │  config.toml   │
│  cli:claude-code│   │ ─SIGTERM 2s → SIGKILL│         └────────────────┘
│  cli:codex      │   │ ─PID persisted       │
│ TEST:           │   │ ─cancellation token  │
│  smoke (scripted)│  └──────────────────────┘
└─────────────────┘
```

Why this shape works:

- **The CLI is thin.** It parses args, sets up state, hands off to the turn loop. All durability lives in `deadreckon-core`.
- **State is on disk before every meaningful change.** `state.json` is atomic-written via temp+rename after every phase transition, snapshot, spend record, and tool call.
- **The agent cannot mark its own gate.** `dr-gate` is a separate binary that signs an acceptance marker against a nonce only it can read; the deadreckon binary refuses to mark a run `Completed` without that signed marker.
- **Sandboxes are platform-native.** macOS uses Seatbelt; Linux uses Bubblewrap; Docker is opt-in. No daemon, no `bollard`, no Lima.
- **BYOK extends to subscriptions.** Subscription-bearing users drive deadreckon by routing turns through their local `claude` or `codex` CLIs; no API key required.

---

## 2. Component Map

### 2.1 Workspace shape

`/Users/gdc/deadreckon/Cargo.toml:1-41`:

- Edition `2024`, resolver `3`, workspace version `0.1.0`.
- Four workspace members:
  - `crates/deadreckon-core` — library (~3,900 LoC across 12 modules)
  - `crates/deadreckon-providers` — library (~1,300 LoC)
  - `crates/deadreckon-sandbox` — library (~640 LoC)
  - `crates/deadreckon` — binary (`deadreckon`, ~2,050 LoC) + binary (`dr-gate`, ~33 LoC at `src/bin/dr-gate.rs`)

### 2.2 Crate-by-crate

**`deadreckon-core` (`crates/deadreckon-core/src/lib.rs`).** Re-exports the public surface of the harness primitives. Modules:

| Module | Purpose |
|---|---|
| `state.rs` | `PipelineState`, `RunStatus`, `PhaseId`, `PhaseState`, `create_run`, `load_run`, atomic writes |
| `paths.rs` | `DeadreckonPaths`, `workspace_scope`, `task_key`, all path resolution |
| `lock.rs` | `LockState`, file locks via `fs2`, PID liveness via `nix::kill(pid, 0)`, heartbeat |
| `promotion.rs` | `promote_completed_run`, manifest writing, atomic working→library swap, crash recovery |
| `gate.rs` | `AcceptanceMarker`, signature validation, anti-self-attestation |
| `turn_loop.rs` | `RunLoopConfig`, `run_turn_loop` (load-bearing) |
| `artifacts.rs` | `copy_tree`, `snapshot_working`, `append_{spend,trace,provenance}` |
| `events.rs` | `RunEvent`, `RunEventBus`, `tokio::sync::broadcast` channel |
| `error.rs` | `DeadreckonError`, `Result<T>` |

**`deadreckon-providers` (`crates/deadreckon-providers/src/lib.rs`).** The `Provider` trait and seven adapters.

| Module / file | Adapter |
|---|---|
| `lib.rs:135-323` | `ProviderAdapter` (HTTP, used by `anthropic` / `openai` / `openai-compatible`) |
| `lib.rs:325-418` | `ScriptedSmokeProvider` (`smoke`, dev-only, deterministic) |
| `cli_claude_code.rs` | `CliClaudeCodeProvider` — shells `claude --dangerously-skip-permissions -p` |
| `cli_codex.rs` | `CliCodexProvider` — shells `codex --ask-for-approval never exec --skip-git-repo-check --sandbox <mode>` |
| `cli_common.rs` | shared subprocess + allowlist machinery |
| `lib.rs:420-511` | `ProviderRouter` — config-driven fallback chain |

**`deadreckon-sandbox` (`crates/deadreckon-sandbox/src/lib.rs`).** `SandboxBackend` (`Auto | SandboxExec | Bwrap | Docker | None`), `SandboxSpec`, `run(SandboxSpec) -> SandboxRunOutput`. Per-backend command construction at lines 284–388. Cancellation + SIGTERM/SIGKILL escalation at lines 131–151.

**`deadreckon` (`crates/deadreckon/src/main.rs` + `crates/deadreckon/src/bin/dr-gate.rs`).** Clap CLI with 12 verbs, ratatui TUI for `attach`, `dr-gate` as a standalone acceptance-marker writer.

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
| `sandbox-exec` | `deadreckon-sandbox/src/lib.rs:284` | macOS Seatbelt profile execution |
| `bwrap` | `deadreckon-sandbox/src/lib.rs:303` | Linux Bubblewrap container |
| `docker` | `deadreckon-sandbox/src/lib.rs:359` | Opt-in fallback |
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

The skill is invoked indirectly: it sits at the path recorded in `state.skill_path` and is read into the prompt frame at `crates/deadreckon-core/src/turn_loop.rs:405-417` (`build_prompt`). The binary never reaches into skill internals. New skills can be added under `skills/<name>/SKILL.md` and selected with `deadreckon run --skill <name>`.

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
│   ├── deadreckon-core/          # state, locks, gates, turn loop
│   ├── deadreckon-providers/     # provider trait + adapters
│   ├── deadreckon-sandbox/       # platform-native sandboxes
│   └── deadreckon/               # binary + dr-gate binary + tests
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
│               │   └── turn-acceptance.json   # AcceptanceMarker
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
main.rs:316 run_command()
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
  outcome = run_turn_loop(state, router, RunLoopConfig{...})  # turn_loop.rs:62
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

In `turn_loop.rs` at lines 248 (CLI sub-agent path) and 386 (regular path), **before** `set_phase_status(PhaseId(60), Completed)`. If promotion fails, the run never reaches `Completed`. The `working/` directory is the source of truth until promotion; after promotion, the library copy is canonical and `working/` is gone.

### 8.4 Crash recovery between rename steps

`promotion.rs:65-84` handles the half-completed states:

- If `staging` exists and final dir doesn't: complete the rename.
- If both exist: the final rename happened but didn't atomically remove staging; clean up staging.
- If final dir exists but its `manifest.json` is missing: write the manifest.

This makes promotion crash-safe across a `kill -9` between the two renames.

---

## 9. The Turn Loop

The load-bearing function. Lives at `/Users/gdc/deadreckon/crates/deadreckon-core/src/turn_loop.rs:62`.

### 9.1 `RunLoopConfig`

`turn_loop.rs:25-35`:

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

`turn_loop.rs:62-66`:

```rust
pub async fn run_turn_loop(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: RunLoopConfig,
) -> Result<RunLoopOutcome>
```

### 9.3 Loop body (paraphrased; see `turn_loop.rs:62-403`)

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

Inline at `turn_loop.rs:254` (paraphrased):

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Action {
    Bash { tool_call_id: String, command: String },
    WriteFile { tool_call_id: String, path: PathBuf, content: String },
    Done { summary: Option<String> },
}
```

Providers return JSON; the loop parses one action per turn. The CLI sub-agent path is detected by `response.trace["kind"] == "cli_subagent"` (`turn_loop.rs:446`) **before** action parsing — those providers do their own tool calls inside the subprocess and return a narrative, not an action JSON.

### 9.5 No smoke fallback in the default path

`grep -r "coding_turn_script\|hardcoded_smoke" /Users/gdc/deadreckon/crates/` returns empty. The deterministic-script path lives entirely inside `ScriptedSmokeProvider` (`crates/deadreckon-providers/src/lib.rs:325-418`), reachable only via `deadreckon run --smoke` (which selects the `smoke` provider, not via a bypass of the run loop).

### 9.6 Error handling

The loop does **not** retry on errors:

- Provider error → propagates and the run fails.
- Tool-call non-zero exit → result fed back to history, next turn's prompt sees the failure; the model decides whether to retry.
- Acceptance failure → run fails; no auto-retry.

The bound is `max_turns` (currently 12), not an error budget.

---

## 10. Provider Model

### 10.1 The `Provider` trait

`crates/deadreckon-providers/src/lib.rs:126-133`:

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
codex --ask-for-approval never exec --skip-git-repo-check --sandbox <mode> "<prompt>"
```

`<mode>` (`cli_codex.rs:121-131`) is `workspace-write` when the outer sandbox is `None`/`SandboxBackend::None` (safer, codex limits itself to cwd), and `danger-full-access` when an outer sandbox is active (the outer sandbox is doing the isolating; codex needs full filesystem access inside).

**Shared subprocess machinery (`cli_common.rs:22-120`).** Builds a `SandboxSpec` with explicit allowlists (`cli_common.rs:154-166`):

- Write allowlist: `~/.codex` for codex, `~/.claude` for claude.
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

`crates/deadreckon-providers/src/lib.rs:325-418`. In-memory `VecDeque<String>` initialized with three responses:

1. `{"action": "bash", "tool_call_id": "smoke-bash-1", "command": "..."}`
2. `{"action": "write_file", "tool_call_id": "smoke-write-2", "path": "README.md", "content": "..."}`
3. `{"action": "done", "summary": "tiny Rust project created"}`

Zero cost, no subscription. Reachable only via `--smoke` flag. The trace records `{"kind": "scripted_smoke", "remaining_steps": N}`.

### 10.6 `ProviderRouter` and fallback chain

`lib.rs:420-511`. Reads config (TOML), resolves a route list (`fallback` array > `default_provider` > built-in chain `cli:claude-code` → `cli:codex` → `anthropic` → `openai` → `openai-compatible`), constructs a `Box<dyn Provider>` per route. On `complete()`:

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

`crates/deadreckon-sandbox/src/lib.rs` is a single ~640-line module that abstracts four backends behind one entry point.

### 11.1 `SandboxBackend`

Lines 29–37:

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

### 13.5 Where the gate is invoked

`turn_loop.rs:246-248` (CLI sub-agent path) and `turn_loop.rs:384-386` (regular Done path) both call:

```rust
run_acceptance_gate(state)?;
validate_acceptance_marker(state)?;
promote_if_ready(state)?;
```

The first call invokes `dr-gate` as a subprocess (`turn_loop.rs:662-697`). The second validates what `dr-gate` wrote. The third atomically swaps the working tree into the library.

Failure at any step prevents the run from reaching `Completed`.

---

## 14. Telemetry: Spend, Traces, Provenance, Events

Four append-only JSONL files capture every run's history. All live under `<run_root>/`. All written via `append_json_line` (`state.rs:375-388`), which opens in append mode and `sync_all`s after each line.

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
8. The loop calls `load_or_reconstruct_history(state, from_turn)` (`turn_loop.rs:550-631`) which loads `history.json` if present, else reconstructs from `traces.jsonl`. With `from_turn = N`, history is truncated to N entries and `state.turn = N`.

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

The handler creates an `imported-<hash>` run, parses the source, appends entries to `traces.jsonl` + `provenance.jsonl`, marks the run `Completed` (skipping the gate), and never writes back to the source. Current coverage is **inventory-level**: it produces a listing/summary but doesn't deeply normalize all fields. Round-trip parity (import → `show <id>` → render comparable to source) is a hardening target — see `docs/goals/2026-05-11-deadreckon-robust-rider.md` §7.

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

The CLI defaults are honest: `--sandbox` defaults to `auto`, `--max-spend` defaults to `$10` (with a confirmation gate above `$50`), `--provider` defaults to the highest-credentialed entry per the fallback chain, `--skill` defaults to `default-coding`.

`run` now starts codebase-aware by default. In a git repo it previews and then creates a `git worktree` on a `dr/...` branch; `--fresh` preserves the old empty-working-dir behavior, `--from <path>` uses copy mode, and `--in-place --i-know-its-a-lot` edits the source tree directly. Completed worktree runs hint `apply` / `discard`; copy and fresh runs hint `export` / `extend`. Run-id arguments accept unique prefixes and `latest` / `last` resolves to the latest run in the current project scope.

---

## 18. TUI (`attach`)

`crates/deadreckon/src/main.rs:874-1289` houses `attach_command` plus the rendering helpers.

### 18.1 Behavior

- On a TTY: `attach_tui()` enables raw mode, alternate screen, and renders a `ratatui` UI.
- Off-TTY: prints a plain-text summary + locations.

### 18.2 Layout

`main.rs:1608-1616`:

```rust
let vertical = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(5),    // header + compact spend/context metrics
        Constraint::Min(10),      // tool calls + provider activity
        Constraint::Length(4),    // processes/status
        Constraint::Length(1),    // keybindings footer
    ])
    .split(area);
```

- **Header** (short run id, status, phase, provider, sandbox, turn timer, truncated goal, working/artifact path).
- **Spend meter** only for metered API providers; CLI subscription providers omit cost and emphasize context/wall time.
- **Context meter**: compact token/window summary with green/yellow/red thresholds.
- **Center, left**: wide streaming list of tool calls + provider activity + recent events, with priority ordering — turn summary → live working-tree diff → recent provider activity → recent `RunEvent`s → recent traces.
- **Completed docs view**: pressing `d` toggles the center-left panel from provider activity to `RUN-NARRATIVE.md` rendered through `pulldown-cmark` into ratatui `Line`/`Span`s. Headings, bullets, inline code, fenced code blocks, links, task markers, math, and horizontal rules receive terminal styles and remain scrollable.
- **Center, right**: narrower live files list with count/bytes in the panel title.
- **Bottom**: supervised PIDs + their `ps` lines (alive/dead annotation).
- **Footer**: action-first completed footer (`[d] Docs` / `[d] Activity`, `[a] Apply`, `[b] Abandon`, `[s] Show`) or scroll/detach help while running.

### 18.3 Data source

Today the TUI **polls** files on disk every 500 ms: `spend.jsonl`, `traces.jsonl`, `events.jsonl`, plus `~/.codex/sessions/` for codex-specific provider activity. The `RunEventBus` broadcast channel exists in `deadreckon-core::events` but the TUI does not yet subscribe to it — switching from poll-driven to stream-driven is a robustness-rider hardening target (§1 of `docs/goals/2026-05-11-deadreckon-robust-rider.md`).

---

## 19. Configuration & BYOK

### 19.1 `config.toml`

Lives at `/Users/gdc/.deadreckon/config.toml` (overridable via `DEADRECKON_HOME`). Schema (`crates/deadreckon-providers/src/lib.rs:104-124`):

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

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
model = "claude-sonnet-4-5"
input_cost_per_million = 3.0
output_cost_per_million = 15.0
```

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
- `crates/deadreckon-providers/src/lib.rs:721-826` (inline) — config parsing, spend math, credential check, smoke determinism.
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

The codebase is more complete than a typical V0 but several layers remain scaffolding rather than finished systems. Honest accounting per the most recent retrospective (`docs/CHANGELOG.md` + `docs/GAP-ANALYSIS.md`):

### Built and reliable

- Workspace, crates, build, lint, fmt, test discipline.
- `PipelineState` shape, phase machine, atomic state writes, schema version.
- PID-aware locks + heartbeats + stale reclaim.
- Atomic working→library promotion with crash recovery.
- Sandbox dispatch for sandbox-exec / bwrap / docker / none + auto resolution.
- HTTP providers (Anthropic / OpenAI / OpenAI-compatible) with token-based spend.
- CLI providers (`cli:claude-code`, `cli:codex`) with wall-clock subscription spend.
- Smoke provider (deterministic) for keyless tests.
- Turn loop with action parsing (Bash / WriteFile / Done) and CLI sub-agent path.
- Codebase-default running: worktree mode, copy mode, in-place mode, fresh-mode preservation, preflight + preview UX, and `codebase.json` files-not-fields metadata.
- `apply` and `abandon` for worktree rollback/apply lifecycle.
- `materialize`, `extend`, `undo`, `list`, and `show` integration with codebase mode metadata, including worktree extension branches chained from parent `dr/...` branches.
- UX consolidation: project-scoped `list`, `latest` run aliases, `status`/`next`, `cleanup`/`prune`, `export`/`discard` aliases, and TTY-aware formatted output.
- Self-documenting run artifacts in stoa shape: `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, optional `AS-BUILT-DELTA.md`, per-turn `_incremental.jsonl`, and `polish.json`.
- `deadreckon doc`, `list` DOCS status, doc-aware `apply` commit bodies, extend-parent narrative updates, diff coverage retry, and the repo/user/project `run-narrator` skill mechanism.
- Acceptance gate with signed marker; anti-self-attestation actually enforced.
- `init`, `config get/set`, `run`, `doctor`, `status`/`next`, `list`, `attach`, `kill`, `resume`, `undo`, `show`, `import`, `cleanup`/`prune` verbs.
- `ratatui` attach TUI with spend/context telemetry, provider activity, in-TUI Markdown docs rendering, live files, process panel, scrollable panels, and completion action footer.
- `--max-spend` cap with pause-at-cap; `--max-wall-seconds` for subscription providers.
- Mock HTTP server for tests; CLI provider tests with fake binaries; 13 integration tests including stress and import round-trips.

### Scaffolding-thin (named in `docs/goals/2026-05-11-deadreckon-robust-rider.md`)

1. **TUI streaming.** Poll-driven from disk; should be event-driven via the `RunEventBus` broadcast channel.
2. **Resume from partial trace.** Works at run/state level; doesn't yet handle truly mid-tool-call truncation with grace.
3. **Cancellation model.** Single-process tokens work; cross-process kill is signal-based and can't cancel an in-flight `reqwest` task in a separate deadreckon process.
4. **Wall-clock spend for CLI providers.** Tracked, capped — but the budget-to-wall-time mapping isn't yet rich.
5. **Sandbox profiles.** Functional, not yet a hardened policy comparable to Claude Code's per-tool permission layer.
6. **Doctor.** Useful, not yet exhaustive (no provider-ping, OS-version sanity is shallow).
7. **Import normalization.** Inventory-level; not full round-trip parity.
8. **Acceptance gate.** Basic — runs `cargo test` when applicable. A full YAML-driven spec language is planned.
9. **Multi-run coordination.** Locks and scopes work; there's no scheduler or queue.
10. **Promotion / library workflow.** Atomic swap works; the library doesn't yet have a richer query/listing surface.

### Not yet built (V1+ candidates per `docs/goals/2026-05-11-deadreckon-usability-rider.md` and the V1 list in the robust rider)

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
not dominate the common path. `list --all` scans every scope; `list --full`
prints exact TSV-style values for scripts. `status` (alias `next`) prints the
latest current-project run, its artifact/worktree locations, and the next
recommended action. Running `deadreckon` with no subcommand dispatches to
`status`.

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

After every successful tool/provider turn, `turn_loop.rs` calls `append_turn_doc`. The deterministic record lands in `_incremental.jsonl` and rewrites the Markdown drafts with turn sections containing tool kind, latency, files, outcome, trace citation, snapshot reference, and worktree commit SHA when available.

### 25.4 End-of-Run Polish Pass

Before acceptance/promotion, `polish_run_docs` resolves `run-narrator` in project, user, then repo order. It substitutes run placeholders, sends one doc-provider completion unless `--no-docs` is set, parses JSON into the three docs plus optional delta, retries once on malformed JSON, and records status/cost/hash/provider in `polish.json`. Provider, JSON, or skill failures are nonfatal; templated docs remain.

### 25.5 Phase And Decision Detection

`docs.rs` coalesces turns into 3-8 phases by file overlap and tool-kind continuity. Decision candidates are detected with case-insensitive marker regexes and a minimum response length so incidental short phrases do not become decisions.

### 25.6 Diff Coverage And Retry

After polish, deadreckon verifies every changed file appears in `RUN-NARRATIVE.md` by relative path or basename. Missing files trigger up to two polish retries with an explicit omission list. Remaining omissions are logged as `docs.warning` traces and do not fail the run.

### 25.7 AS-BUILT-DELTA

The delta is generated for worktree runs whose source has an AS-BUILT file at the root or beside touched files and whose diff touches at least three files or adds public/exported API. Public docs are copied to `working/docs/`; the branch gets a `turn docs: deadreckon run docs for <id>` commit so `apply` carries docs forward.

### 25.8 Apply Commit Body

When `deadreckon apply` builds the default squash or merge message, it reads `RUN-NARRATIVE.md` and `RUN-DECISIONS.md` to include an executive summary, phase list, decision count, open-thread count, and a `docs/RUN-NARRATIVE.md` trace pointer. `--message` still overrides the generated body.

### 25.9 `deadreckon doc`

`deadreckon doc <run-id>` prints the narrative by default. `--kind as-built|decisions|delta` selects another artifact, `--export <path>` writes it to disk, `--force` overwrites exports, and `--polish --no-confirm` requests a fresh doc polish turn.

### 25.10 Cost And Idempotency

`polish.json` stores a SHA-256 inputs hash over goal, traces, provenance, spend, incremental records, changed files, and source AS-BUILT content. A matching polished hash skips duplicate provider calls unless forced. CLI subscription providers report wall time rather than USD cost.
- **Trace** — every LLM call and every tool dispatch, with latency + structured detail.
- **CLI sub-agent** — a `cli:*` provider whose `complete()` invocation is one whole turn (the sub-agent does its own tool calls inside). Detected by `response.trace["kind"] == "cli_subagent"`.
- **dr-gate** — the standalone binary at `crates/deadreckon/src/bin/dr-gate.rs` that owns acceptance verification. The agent cannot impersonate it.
- **BYOK** — Bring Your Own Key. In deadreckon this extends to subscriptions: a Claude Max or ChatGPT Pro user can drive deadreckon via `cli:*` providers without an API key.

---

*This document is canonical for the alpha-tier reality of deadreckon. Future hardening passes (per the robustness rider) and feature passes (per the usability rider) will update sections 6, 9, 11, 13, 14, 18, and 22 in particular. Last regenerated by an agent team from a deep code map; cross-check against the current code before relying on any specific line number.*
