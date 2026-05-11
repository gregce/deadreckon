# deadreckon

Rust agentic CLI harness for unattended long-running coding tasks with local-first run state, BYOK provider routing, disposable sandboxes, durable spend/provenance/traces, and per-turn undo.

```bash
cd /Users/gdc/deadreckon
cargo build --release
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon doctor
DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke ./target/release/deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
```

Runtime state defaults to `/Users/gdc/.deadreckon/`. Set `DEADRECKON_HOME` for tests or isolated local runs.

Normal runs use the configured provider router at `/Users/gdc/.deadreckon/config.toml`.
The `--smoke` flag is only for keyless local verification.
