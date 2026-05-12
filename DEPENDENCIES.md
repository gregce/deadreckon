# Dependencies

Tier 1 crates are logged in commit messages. Tier 2 crates are listed here.

| crate | version | tier | purpose | alternatives_rejected | added_in_commit |
|---|---:|---:|---|---|---|
| toml | 0.9.12 | 2 | Parse `/Users/gdc/.deadreckon/config.toml` for BYOK provider routing. | Hand-rolled parsing would violate the structured-parser preference; JSON config would violate the rider's TOML path. | c881a88 |
| axum | 0.8.9 | 2 | Keyless OpenAI-compatible mock provider used by primary-flow integration tests. | Hand-written TCP HTTP would obscure the provider contract; adding a production server surface is avoided by keeping this as a dev-dependency. | 1b425dd |
| ignore | 0.4.25 | 1 | `.gitignore`-aware source walking for codebase copy mode. | Ad hoc ignore parsing would miss `.ignore`, parent, and global gitignore rules. | codebase-modes |
| serde_yaml | 0.9.34 | 1 | Parse `acceptance.yaml` for dr-gate checks. | YAML is prescribed by the robustness rider. | cec49f3 |
| tokio-util | 0.7.18 | 1 | Hierarchical `CancellationToken` for run, provider, and sandbox cancellation. | Homegrown cancellation would be less reliable than Tokio's standard utility. | cec49f3 |
| regex | 1.12.3 | 1 | Detect decision markers and deterministic auto-title phrases for self-documenting run drafts. | Hand-rolled matching would be weaker than the rider's regex contract. | self-documenting-runs |
| sha2 | 0.10.9 | 1 | Compute SHA-256 polish input hashes for idempotent doc-provider calls. | `DefaultHasher` is not stable or cryptographic and would violate the rider hash requirement. | self-documenting-runs |
| pulldown-cmark | 0.13.3 | 1 | Parse run Markdown docs for styled in-TUI rendering. | Raw Markdown dumping is hard to read; hand-rolled parsing would miss common Markdown constructs and regress formatting. | tui-markdown-docs |
