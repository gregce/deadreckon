# Dependencies

Tier 1 crates are logged in commit messages. Tier 2 crates are listed here.

| crate | version | tier | purpose | alternatives_rejected | added_in_commit |
|---|---:|---:|---|---|---|
| toml | 0.9.12 | 2 | Parse `/Users/gdc/.deadreckon/config.toml` for BYOK provider routing. | Hand-rolled parsing would violate the structured-parser preference; JSON config would violate the rider's TOML path. | c881a88 |
| axum | 0.8.9 | 2 | Keyless OpenAI-compatible mock provider used by primary-flow integration tests. | Hand-written TCP HTTP would obscure the provider contract; adding a production server surface is avoided by keeping this as a dev-dependency. | 1b425dd |
