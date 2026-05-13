# deadreckon — Hygiene Rider (codex-pattern adoption, zero behavior change)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-12-2039-deadreckon-hygiene-goal.md`.
It supersedes nothing in prior riders (2026-05-10-build,
2026-05-11-{audit-harden, autonomous-chain, codebase, distribute,
doc-depth, orchestrate, overnight, primary-flow, provider-registry,
robust, self-documenting, usability}) — their invariants still apply.
This rider adds workspace lint discipline, formatted imports, a tuned
release profile, library print refusal, internal-crate routing through
`[workspace.dependencies]`, registry-shaped library `lib.rs` files,
and an error taxonomy (`is_retryable`/`is_fatal`) — all without
changing any runtime behavior.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`~/.deadreckon/` (smoke runtime
`/Users/gdc/deadreckon/.deadreckon-smoke`).

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace version remains `0.1.0`.
- **No `PipelineState` schema changes.** No persisted-state file
  changes anywhere under `~/.deadreckon/`.
- **No new crates.** The five crates stay five
  (`deadreckon`, `-core`, `-runtime`, `-providers`, `-sandbox`).
- **No new CLI verbs, flags, or output strings.** The `make smoke`
  golden sha256 is the load-bearing assertion: if it changes, you
  changed behavior.
- **No observability rewiring.** `tracing` calls already in the code
  stay where they are; this rider does not add `#[instrument]`, does
  not change subscriber init, does not add `otel`.
- **No re-export removals.** Every symbol currently re-exported from a
  library `lib.rs` stays re-exported with the same path. The
  registry-shape pass reformats and groups, never deletes.
- **`is_retryable`/`is_fatal` ship without callers.** Wiring them into
  the run loop, gate, or watchdog is explicitly out of scope (V1).
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

### Overlap with peer riders — land non-conflicting

- **Distribute rider** (P3 routes channel hints with `try:`). This
  rider does not touch `try:` strings or printing UX.
- **Overnight rider** (`--plain`/`--quiet`, `ui_card`). This rider's
  library-print-refusal does **not** apply to the `deadreckon` binary
  crate; that crate keeps its `println!`/`ui_card` calls verbatim. If
  any library-crate site currently does direct stdout/stderr writes,
  it gets refactored to return a value the binary prints — but only
  if the lint actually flags it. Don't go searching for sites that
  the lint doesn't flag.
- **Provider-registry rider.** No provider-router shape changes.
- **Autonomous-chain rider.** Chain modules in `deadreckon-core` get
  the registry-shape `lib.rs` treatment but their public types stay
  identical.

## Data model (files, not fields)

This rider adds **two test-fixture files** and **zero runtime files**.

### `/Users/gdc/deadreckon/tests/.smoke-baseline`

One line: the sha256 of the `RUN-NARRATIVE.md` produced by `make
smoke` at HEAD before P1 begins. Read by `tests/smoke_invariant.rs`
(P1 depth test) which runs the smoke and asserts the hash matches.

```
<64 hex chars>  RUN-NARRATIVE.md
```

### `/Users/gdc/deadreckon/tests/.public-surface-baseline`

Per-crate sorted listing of every `pub use ...` re-export path from
each library crate's `lib.rs`. Captured at P1 by parsing `lib.rs`
files (not by running `cargo public-api` — keep it dependency-free).
Read by `tests/public_surface.rs` which re-parses at test time and
asserts equality.

```
crate: deadreckon-core
  artifacts::ProvenanceRecord
  artifacts::SpendRecord
  ...
crate: deadreckon-providers
  ...
```

The two baselines are written **once** in P1 from the pre-rider HEAD
and never edited again until P11 (where `public-surface-baseline` is
intentionally regenerated only if codex-style grouping reorders
exports — but the **set** must stay equal; P11 asserts set-equality
with the P1 capture, not byte-equality).

## The seven moves (specs)

### Move 1 — `[workspace.lints]` + `clippy.toml`

Add to `/Users/gdc/deadreckon/Cargo.toml`:

```toml
[workspace.lints.rust]
unsafe_code = "deny"
unused_must_use = "deny"

[workspace.lints.clippy]
await_holding_lock = "deny"
expect_used = "deny"
unwrap_used = "deny"
needless_borrow = "deny"
needless_collect = "deny"
needless_pass_by_value = "warn"
redundant_clone = "deny"
manual_flatten = "deny"
manual_map = "deny"
manual_find = "deny"
manual_let_else = "warn"
single_match_else = "warn"
explicit_iter_loop = "warn"
implicit_clone = "warn"
unnecessary_wraps = "warn"
```

Add `/Users/gdc/deadreckon/clippy.toml`:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-dbg-in-tests = true
large-error-threshold = 256
```

Each crate's `Cargo.toml` gains:

```toml
[lints]
workspace = true
```

### Move 2 — `rustfmt.toml`

`/Users/gdc/deadreckon/rustfmt.toml`:

```toml
edition = "2024"
imports_granularity = "Item"
group_imports = "StdExternalCrate"
reorder_imports = true
```

Apply with `cargo fmt --all` once and commit the formatting diff
**alone**, without any other change, to keep that commit reviewable.

### Move 3 — `[profile.release]` + `[profile.dev]`

Add to root `Cargo.toml`:

```toml
[profile.release]
lto = "fat"
codegen-units = 1
strip = "symbols"
split-debuginfo = "off"
panic = "unwind"          # match prior behavior; do not switch to abort

[profile.dev]
debug = "limited"         # faster compiles; backtraces still resolve
```

`panic = "unwind"` is explicit because the prior implicit default was
unwind; switching to abort would change behavior on panics.

### Move 4 — Internal crates in `[workspace.dependencies]`

Append to root `[workspace.dependencies]`:

```toml
deadreckon-core      = { path = "crates/deadreckon-core" }
deadreckon-providers = { path = "crates/deadreckon-providers" }
deadreckon-runtime   = { path = "crates/deadreckon-runtime" }
deadreckon-sandbox   = { path = "crates/deadreckon-sandbox" }
```

Each crate's `Cargo.toml` rewrites internal deps from raw path refs:

```toml
# before
deadreckon-core = { path = "../deadreckon-core" }

# after
deadreckon-core = { workspace = true }
```

### Move 5 — `lib.rs` as registry

Target shape (model:
`/Users/gdc/codex/codex-rs/protocol/src/lib.rs`):

```rust
//! <one-line crate description>

mod private_module_one;
mod private_module_two;

pub mod public_submodule_one;
pub mod public_submodule_two;

pub use private_module_one::{TypeA, TypeB, function_c};
pub use public_submodule_one::Facade;
```

Rules:

- One blank line between the `//!` block and the first `mod`.
- All `mod` lines together, then all `pub mod` lines, then all
  `pub use` lines. Sort each group alphabetically.
- No business logic in `lib.rs`. No type aliases unless they were
  there before. No `impl` blocks.
- The `pub use` set is identical to the pre-rider set, just sorted
  and grouped. **No additions, no removals.**
- The `Result<T>` alias defined alongside an `Error` enum stays
  defined in `error.rs` and is re-exported via `pub use error::{Error,
  Result};` — same shape codex uses.

### Move 6 — Library print refusal

At the root of every library crate's `lib.rs` (i.e.
`deadreckon-core`, `-runtime`, `-providers`, `-sandbox`), insert as
the first lines (above the `//!` doc):

```rust
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
```

The `deadreckon` binary crate is exempt. If the lint flags an actual
site in a library crate, refactor it to return a value the binary
prints — that's a behavior-equivalent move. If the smoke baseline
hash changes, you mis-refactored; revert.

### Move 7 — Error taxonomy

For each `Error` enum (`DeadreckonError`, `ProviderError`,
`SandboxError`, plus the `RuntimeError` to be confirmed in P10 — if
the runtime crate has no enum yet, add a minimal one without
swapping any existing `?`/`Result` plumbing), add:

```rust
impl DeadreckonError {
    /// Transient — the operation may succeed on a retry.
    pub fn is_retryable(&self) -> bool {
        matches!(self,
            // case-by-case enumeration; default is `false`
        )
    }

    /// Unrecoverable — the watchdog should escalate, not retry.
    pub fn is_fatal(&self) -> bool {
        !self.is_retryable()
            && matches!(self, /* … */)
    }
}
```

Every variant must appear in **either** `is_retryable` or `is_fatal`
explicitly (no wildcard arms). The depth test is a `match` over every
variant that asserts at least one of the two methods returns `true`.

Decision rules:

- I/O errors with `ErrorKind::{Interrupted, WouldBlock,
  TimedOut, ConnectionReset, ConnectionAborted, BrokenPipe}` →
  retryable.
- Lock-held errors → retryable (someone else holds the lock; they'll
  release).
- Schema/parse errors (`Json`, `Toml`) → fatal.
- `InvalidInput`, `MissingCredential`, `InvalidConfig` → fatal.
- `NotFound` → fatal (caller decides; the error itself isn't
  transient).
- `NoRoute` (providers) → fatal.
- HTTP errors → retryable when status is 408/429/5xx; fatal
  otherwise. Since the existing `Http { provider, detail }` variant
  doesn't carry status, this rider keeps the conservative default of
  `is_fatal = true` for `Http` and notes "needs status field" in
  `docs/V1-CANDIDATES.md`. Do **not** add the status field in this
  rider.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on `make verify`; conventional-commit local
commit; one-line CHANGELOG entry.

### P1 — Capture baselines + invariant tests

- Write `tests/smoke_invariant.rs` (top-level integration test crate)
  that runs `make smoke` (or its inner cargo invocation) and asserts
  the resulting `RUN-NARRATIVE.md` sha256 matches
  `tests/.smoke-baseline`.
- Write `tests/public_surface.rs` that parses each library crate's
  `lib.rs`, collects every `pub use` path, sorts, and asserts equal
  to `tests/.public-surface-baseline`.
- Capture both baselines from pre-rider HEAD and commit them.

Depth tests (in `tests/`):

- `smoke_baseline_matches_pre_rider_head`
- `public_surface_set_matches_pre_rider_head`
- `public_surface_baseline_lists_all_four_library_crates`

### P2 — `[workspace.lints]` (warn-only first pass)

- Add the lint table from Move 1 with **every clippy rule set to
  `"warn"`** (not deny yet). Add `clippy.toml`. Add `[lints] workspace
  = true` to every crate.
- `cargo clippy --workspace` produces a list; **do not fix sites yet**.
  Snapshot the list to `tests/.clippy-warn-snapshot` for P3 to
  diff against.

Depth tests:

- `every_crate_inherits_workspace_lints`
- `clippy_toml_allows_unwrap_in_tests`
- `clippy_warn_snapshot_present`

### P3 — Fix warn sites + escalate to deny

- For each warn site in the snapshot, fix or `#[allow(...)]`-with-a-
  `// SAFETY:` style justification comment. Prefer fixes; allows are
  for cases the fix would change semantics.
- Once the workspace builds clean under `-D warnings`, change the
  Move-1 lint table from `"warn"` to `"deny"` for the listed
  always-deny rules. Keep the `"warn"`-tier rules as `"warn"`.
- Delete `tests/.clippy-warn-snapshot`.

Depth tests:

- `clippy_runs_clean_under_deny_warnings`
- `lint_table_denies_unwrap_used`
- `lint_table_denies_expect_used`
- `lint_table_denies_await_holding_lock`

### P4 — `rustfmt.toml` + one-shot format

- Add the `rustfmt.toml` from Move 2.
- Run `cargo fmt --all`. Commit the diff in a single
  `style: apply rustfmt with imports_granularity=Item` commit, with
  no other changes mixed in.
- Run `make verify` afterwards.

Depth tests:

- `rustfmt_toml_pins_imports_granularity_item`
- `rustfmt_check_clean`
- `format_commit_touches_only_whitespace_and_imports` (CI-style: a
  test that loads the latest format commit's diff and asserts no
  identifier additions/removals — a parsed-tree compare via
  `syn::parse_file` of pre vs post is fine)

### P5 — Release + dev profile tuning

- Add the `[profile.release]` and `[profile.dev]` blocks from Move 3.
- Record the pre-tune binary size in `tests/.size-baseline` (one line:
  byte count of `target/release/deadreckon`). Build release; assert
  the new size is `≤` baseline + 5% slack (LTO sometimes grows then
  shrinks across builds; the assertion is "not catastrophically
  larger").

Depth tests:

- `release_profile_pins_lto_fat`
- `release_profile_pins_codegen_units_one`
- `release_profile_keeps_panic_unwind`
- `release_binary_size_within_baseline_slack`

### P6 — Internal crates in `[workspace.dependencies]`

- Append the four internal-crate entries to root
  `[workspace.dependencies]`. Rewrite every internal `path = "../foo"`
  to `{ workspace = true }` across the four crate Cargo.tomls.
- `cargo metadata` should resolve to the same DAG; assert that.

Depth tests:

- `internal_crates_listed_in_workspace_dependencies`
- `no_crate_uses_relative_path_for_internal_dep` (greps each crate
  Cargo.toml for `path = "../`)
- `cargo_metadata_resolves_same_dag` (snapshot of `cargo metadata
  --format-version=1` filtered to internal nodes; equal pre vs post)

### P7 — Library print refusal

- Insert the two `#![deny]` attributes at the top of each library
  crate's `lib.rs` (Move 6).
- Run clippy. Any flagged site in a library crate gets refactored to
  return-and-let-the-binary-print. Re-run `make smoke` and assert the
  baseline hash holds.

Depth tests:

- `library_crate_lib_rs_denies_print_stdout` (parse each lib.rs
  attribute list)
- `library_crate_lib_rs_denies_print_stderr`
- `binary_crate_does_not_inherit_print_deny` (the deadreckon bin's
  `main.rs` / `lib.rs` does not carry the deny attrs)
- `smoke_baseline_holds_after_print_refactor` (re-runs the P1 smoke
  invariant)

### P8 — Registry-shape `lib.rs` (deadreckon-core)

- Reformat `crates/deadreckon-core/src/lib.rs` to the Move-5 target
  shape: `//!` doc, `mod` lines (sorted), `pub mod` lines (sorted),
  `pub use` lines (sorted, grouped by source module).
- Run `tests/public_surface.rs` — it must still pass.

Depth tests:

- `core_lib_rs_module_declarations_grouped`
- `core_lib_rs_pub_use_paths_sorted`
- `core_lib_rs_contains_no_impl_block`
- `core_lib_rs_contains_no_fn_definition`

### P9 — Registry-shape `lib.rs` (providers, runtime, sandbox)

- Same treatment as P8 for `deadreckon-providers`,
  `deadreckon-runtime`, `deadreckon-sandbox`.
- For `deadreckon-runtime` (currently 28 lines, 2 modules), the
  registry shape is trivial; still apply for consistency.
- For `deadreckon-sandbox` (currently 215 lines but already close to
  registry shape with `mod` + `pub use`), keep `#[cfg(test)] mod
  tests` at the bottom — that's allowed; the rule is "no logic", and
  unit-test modules are not logic.

Depth tests:

- `providers_lib_rs_module_declarations_grouped`
- `runtime_lib_rs_module_declarations_grouped`
- `sandbox_lib_rs_module_declarations_grouped`
- `every_library_lib_rs_pub_use_set_unchanged_from_p1` (re-runs the
  P1 public-surface invariant with set-equality, not order-equality)

### P10 — Error taxonomy

- Add `is_retryable` and `is_fatal` to `DeadreckonError`,
  `ProviderError`, and the sandbox error enum (`SandboxError` from
  `deadreckon-sandbox/src/backend.rs`). For
  `deadreckon-runtime`: if no `Error` enum exists, define a minimal
  `RuntimeError` with one variant per existing `?`-source it
  currently relies on, but **do not** rewrite call sites to use it
  this rider; just define the enum + the two methods so future work
  can adopt it. If the runtime currently uses `Result<_,
  DeadreckonError>` end-to-end, skip enum creation and add the two
  methods to the existing alias path.
- Apply the Move-7 decision rules verbatim. Use exhaustive `match`
  arms (no wildcards).

Depth tests (one per crate):

- `deadreckon_error_every_variant_is_retryable_or_fatal`
- `provider_error_every_variant_is_retryable_or_fatal`
- `sandbox_error_every_variant_is_retryable_or_fatal`
- `deadreckon_error_io_interrupted_is_retryable`
- `provider_error_no_route_is_fatal`
- `provider_error_http_is_fatal_with_v1_followup_noted`
- `runtime_error_taxonomy_present` (or: a `compile_error!`-style
  static assertion that one of the two methods exists on the error
  type the runtime actually returns)

### P11 — Architecture doc update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## 29. Workspace Hygiene

  29.1 Centralized lints (`[workspace.lints]` + `clippy.toml`)
  29.2 Formatted imports (`rustfmt.toml`, `imports_granularity = "Item"`)
  29.3 Tuned profiles (`[profile.release]` LTO/strip, `[profile.dev]` limited debug)
  29.4 Internal crate routing through `[workspace.dependencies]`
  29.5 Registry-shaped library `lib.rs` (no logic, sorted `pub use` set)
  29.6 Library print refusal (`#![deny(clippy::print_stdout, clippy::print_stderr)]`)
  29.7 Error taxonomy (`is_retryable` / `is_fatal`, vocabulary only)
  29.8 Behavior invariants enforced by `tests/smoke_invariant.rs` and `tests/public_surface.rs`
  ```

- Update `§22 What's Built vs Scaffolding-Thin`:
  - Add to **Built and reliable**:
    "Workspace lint discipline (deny-tier clippy + rustc), tuned
    release profile, registry-shaped library `lib.rs`, library print
    refusal, error retryable/fatal taxonomy (vocabulary, not yet
    wired)."
  - This rider closes no prior thin item; note explicitly:
    "The hygiene rider is purely structural; it does not close prior
    thin items, but it raises the floor for every future rider."

- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Workspace hygiene (alpha) — 2026-05-12

  - Added `[workspace.lints]` (deny `unwrap_used`, `expect_used`, `await_holding_lock`, `redundant_clone`, `needless_borrow`, `manual_*`) and `clippy.toml` (test exemptions, `large-error-threshold = 256`); every crate inherits via `[lints] workspace = true`.
  - Added `rustfmt.toml` with `edition = "2024"` and `imports_granularity = "Item"`; one-shot format applied in a dedicated commit.
  - Tuned `[profile.release]` (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `split-debuginfo = "off"`) and `[profile.dev]` (`debug = "limited"`); panic strategy unchanged (`unwind`).
  - Routed the four internal crates through `[workspace.dependencies]`; downstream Cargo.tomls reference `{ workspace = true }`.
  - Reshaped every library crate's `lib.rs` into a registry: `//!` doc, sorted `mod` / `pub mod` / `pub use` blocks, no logic. Public re-export set is byte-for-byte preserved.
  - Added `#![deny(clippy::print_stdout, clippy::print_stderr)]` to every library crate root; the `deadreckon` binary stays exempt.
  - Added `is_retryable(&self) -> bool` and `is_fatal(&self) -> bool` to each error enum, with exhaustive `match` arms over every variant; no callers wired this milestone (vocabulary only).
  - Added `tests/smoke_invariant.rs` and `tests/public_surface.rs` plus the two baseline files at `tests/.smoke-baseline` and `tests/.public-surface-baseline` to enforce zero behavior drift.
  - Updated `docs/AS-BUILT-ARCHITECTURE.md` with `§29 Workspace Hygiene` and amended `§22`.
  ```

- No demo capture (this rider is invisible to users).

## Integration matrix (move × phase × invariant)

| Move | Phase(s) | Behavior invariant guarded by |
|---|---|---|
| 1. workspace lints | P2, P3 | smoke baseline (P1) |
| 2. rustfmt | P4 | parsed-tree pre/post compare |
| 3. profiles | P5 | size baseline (P5) + smoke baseline |
| 4. internal deps | P6 | `cargo metadata` DAG snapshot |
| 5. lib.rs registry | P8, P9 | public-surface baseline (P1) |
| 6. print refusal | P7 | smoke baseline |
| 7. error taxonomy | P10 | exhaustive-match depth tests |

## Error-footer canonical pairs

This rider adds no user-facing errors and no `try:` lines. The
existing footer pairs from prior riders stay verbatim.

## Config additions (none)

This rider adds no `~/.deadreckon/config.toml` knobs. The lint
table, formatter config, profiles, and `clippy.toml` live in the
source tree, not the runtime config.

## Out of scope (V1 candidates — log if surfaced)

- Wiring `is_retryable`/`is_fatal` into the run loop, gate, watchdog,
  or chain conductor.
- Adding an `Http { status: u16, ... }` variant to `ProviderError`
  (needed for accurate retryability of HTTP errors; this rider notes
  the gap and stays conservative).
- A `deadreckon-test-support` crate (codex pattern; deferred).
- A `deadreckon-otel` crate or any tracing rewiring.
- A `deadreckon-providers-mock` extracted crate.
- Migrating `Makefile` to `justfile`.
- A `cargo-audit` GitHub Action.
- `cargo nextest` adoption.
- `#[instrument]` annotation pass.
- `cargo-shear` / `cargo-machete` dead-dep cleanup.
- Deeper subdir grouping under `crates/` (e.g., `crates/utils/*`).
- `[patch.crates-io]` overrides.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):

- `syn` (dev-dep only) — used by `tests/public_surface.rs` and the
  P4 parsed-tree comparator. Already transitively pulled by
  `serde_derive`; promote to a direct dev-dep on the top-level
  `tests/` crate (or wherever the integration tests live).

Tier 2 (architectural, log to `DEPENDENCIES.md`): none.

Tier 3 (blocked): same blocks as prior riders. In particular: no
`cargo-public-api` (heavy dep, redundant with the `syn`-based
parser).

## Engineering invariants (do not violate)

- **Zero functional-behavior change.** The smoke baseline is the
  oracle. If you change it, you went out of scope.
- **Public re-export set is preserved.** The public-surface baseline
  is the oracle. P11 may regenerate the file, but only as a
  set-equality regeneration — never to admit additions or removals.
- **No new persisted state.** Nothing under `~/.deadreckon/` changes.
- **`is_retryable`/`is_fatal` exhaustively cover every variant.** No
  wildcard arms. A new variant added in a later rider must update
  both methods or fail to compile.
- **The format commit (P4) is mechanical.** A reviewer reads only the
  diff and sees only whitespace + import reordering. No identifier
  added or removed.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- The P4 formatting commit is its own commit, with subject
  `style: apply rustfmt with imports_granularity=Item` and no body
  changes other than format.
- The P11 doc commit is its own commit, separate from any code
  commit, with subject
  `docs(architecture): add §29 Workspace Hygiene; amend §22`.
- After P11, no demo capture. This rider is invisible to users.
- If a phase reveals a V1-architecture decision (e.g.,
  `ProviderError::Http` needs a status field; `RuntimeError` needs to
  exist), stop and log it in `docs/V1-CANDIDATES.md`; do not
  silently expand scope into wiring or schema changes.
