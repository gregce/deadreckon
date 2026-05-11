# Dependencies

Tier 1 crates are logged in commit messages. Tier 2 crates are listed here.

| crate | version | tier | purpose | alternatives_rejected | added_in_commit |
|---|---:|---:|---|---|---|
| toml | 0.9.12 | 2 | Parse `/Users/gdc/.deadreckon/config.toml` for BYOK provider routing. | Hand-rolled parsing would violate the structured-parser preference; JSON config would violate the rider's TOML path. | pending |
