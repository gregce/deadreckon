# deadreckon

Rust agentic CLI harness for unattended long-running coding tasks with local-first run state, BYOK provider routing, disposable sandboxes, durable spend/provenance/traces, and per-turn undo.

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
next: deadreckon doctor
```

Runtime state defaults to `/Users/gdc/.deadreckon/`. Set `DEADRECKON_HOME` for tests or isolated local runs.

Normal runs use the configured provider router at `/Users/gdc/.deadreckon/config.toml`.
The `--smoke` flag is only for keyless local verification.

Keyless local verification:

```bash
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
```
