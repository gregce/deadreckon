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
init -> run -> list -> attach -> materialize -> extend
                                  |
                                  v
                           users' working dir
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

Materialize a completed artifact into an editable project directory:

```bash
deadreckon list
deadreckon materialize <run-id> --dest ./realtime-chess
cd ./realtime-chess
```

Extend a completed run with a follow-up goal while preserving parent lineage:

```bash
deadreckon extend <run-id> "add spectator mode and rematch support"
deadreckon show <new-run-id>
```

Completed runs print a next-action menu by default. In an interactive CLI,
choose `m` to materialize, `e` to extend, `s` to show details, or `q` to quit.
In the TUI, the same keys are available after completion. Use `--no-hints` on
`run` or `attach` to suppress completion guidance.
