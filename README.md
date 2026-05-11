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
