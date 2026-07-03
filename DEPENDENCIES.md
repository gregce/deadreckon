# Dependencies

Tier 1 crates are logged in commit messages. Tier 2 crates are listed here.

| crate | version | tier | purpose | alternatives_rejected | added_in_commit |
|---|---:|---:|---|---|---|
| toml | 0.9.12 | 2 | Parse `~/.deadreckon/config.toml` for BYOK provider routing. | Hand-rolled parsing would violate the structured-parser preference; JSON config would violate the rider's TOML path. | c881a88 |
| axum | 0.8.9 | 2 | Keyless OpenAI-compatible mock provider used by primary-flow integration tests. | Hand-written TCP HTTP would obscure the provider contract; adding a production server surface is avoided by keeping this as a dev-dependency. | 1b425dd |
| ignore | 0.4.25 | 1 | `.gitignore`-aware source walking for codebase copy mode. | Ad hoc ignore parsing would miss `.ignore`, parent, and global gitignore rules. | codebase-modes |
| serde_yaml | 0.9.34 | 1 | Parse `acceptance.yaml` for dr-gate checks. | YAML is prescribed by the robustness rider. | cec49f3 |
| tokio-util | 0.7.18 | 1 | Hierarchical `CancellationToken` for run, provider, and sandbox cancellation. | Homegrown cancellation would be less reliable than Tokio's standard utility. | cec49f3 |
| regex | 1.12.3 | 1 | Detect decision markers and deterministic auto-title phrases for self-documenting run drafts. | Hand-rolled matching would be weaker than the rider's regex contract. | self-documenting-runs |
| sha2 | 0.10.9 | 1 | Compute SHA-256 polish input hashes for idempotent doc-provider calls. | `DefaultHasher` is not stable or cryptographic and would violate the rider hash requirement. | self-documenting-runs |
| pulldown-cmark | 0.13.3 | 1 | Parse run Markdown docs for styled in-TUI rendering. | Raw Markdown dumping is hard to read; hand-rolled parsing would miss common Markdown constructs and regress formatting. | tui-markdown-docs |
| inquire | 0.9.4 | 2 | Interactive prompt engine: arrow-key selects with hints, styled confirms, validated number input, themed to the shared Tone palette. | dialoguer (console-backed, duplicate styling stack), cliclack (opinionated flow primitives beyond need), retaining the bespoke raw-mode menu (no paging, no validation, more unsafe-adjacent terminal code to maintain). | ux-overhaul |
| tui-tree-widget | 0.23.0 | 2 | Helm voyage pane tree widget, pinned to the newest ratatui 0.29-compatible release. | Hand-rolling a correct scrollable tree would expand P5 scope; 0.24.0 targets the ratatui split-core line used by ratatui 0.30 examples, so 0.23.0 is the compatible pin. | helm-p5 |
| futures-util | 0.3.32 | 1 | Provides `StreamExt::next` for crossterm `EventStream` in Helm's event-driven attach loop. | Hand-polling the stream would add unsafe-adjacent pinning boilerplate around a standard async utility. | helm-p7 |
| tui-textarea | 0.7.0 | 2 | Helm in-frame single-line input widget, pinned to the ratatui 0.29-compatible textarea line. | `ratatui-textarea` 0.8+ targets the split ratatui-core/widgets ecosystem and 0.4 targets ratatui 0.24; hand-rolled editing would duplicate cursor/delete behavior before command mode. | helm-p9 |
