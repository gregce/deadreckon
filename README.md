# deadreckon

Rust agentic CLI harness for unattended long-running coding tasks. The current maturity tier is alpha: local-first run state, BYOK provider routing, disposable sandboxes, durable spend/provenance/traces, signed acceptance gates, and per-turn undo.

## Quickstart

```bash
cd /Users/gdc/deadreckon
cargo build --release
./target/release/deadreckon init
./target/release/deadreckon run "hello-world in rust"
./target/release/deadreckon attach <run-id>
```

Expected first-run output includes:

```text
wrote /Users/gdc/.deadreckon/config.toml
next: deadreckon run "describe the coding task"
```

Runtime state defaults to `/Users/gdc/.deadreckon/`. Set `DEADRECKON_HOME` for tests or isolated local runs.

Normal runs use the configured provider router, sandbox, spend cap, and wall-clock cap at `/Users/gdc/.deadreckon/config.toml`.
In a git repo, the default working mode is a new `git worktree` on a `dr/...`
branch under `/Users/gdc/.deadreckon/worktrees/`; the launch checkout is left
unchanged until you run `deadreckon apply`.
After `init`, provider/sandbox/caps are defaults; flags are only overrides:

```bash
deadreckon run "make a full task productivity tracker in nodejs that allows me to manage my day"
```

The `--smoke` flag is only for keyless local verification.

Keyless local verification:

```bash
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
```

## Lifecycle

```text
git repo: init -> run -> attach -> apply | abandon
copy/fresh:       run -> attach -> materialize -> extend
```

Start an unattended run:

```bash
deadreckon init --provider cli:codex --sandbox auto --no-confirm
deadreckon run "make a realtime chess app"
deadreckon attach <run-id>
```

`run` prints `started run <run-id>` and `attach: deadreckon attach
<run-id>` immediately, so you can attach from another terminal without going
through `list`.

Apply a completed worktree run back to your current branch:

```bash
deadreckon list
deadreckon apply <run-id>
deadreckon abandon <run-id>
```

Use `abandon` after inspection or after a successful apply to remove the
deadreckon worktree and temporary branch.

Materialize a completed copy or fresh artifact into an editable project
directory:

```bash
deadreckon run "make a realtime chess app" --fresh
deadreckon materialize <run-id> --dest ./realtime-chess
cd ./realtime-chess
```

Extend a completed run with a follow-up goal while preserving parent lineage:

```bash
deadreckon extend <run-id> "add spectator mode and rematch support"
deadreckon show <new-run-id>
```

Completed runs print a next-action menu by default. Worktree runs offer
`a` apply, `b` abandon, and `s` show. Copy/fresh runs offer `m` materialize,
`e` extend, and `s` show. In the TUI, the same keys are available after
completion. Use `--no-hints` on `run` or `attach` to suppress completion
guidance.
