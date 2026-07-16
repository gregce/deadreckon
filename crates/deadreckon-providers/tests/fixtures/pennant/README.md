# Pennant real-binary fixtures

These fixtures come from local, non-interactive invocations of the installed
binaries. Trimming removes incremental duplicates while preserving every event
shape used by a descriptor contract. When a provider includes opaque payloads,
the fixture is normalized to the contract-relevant fields with `jq`.

- `pi-simple.jsonl` and `pi-tool.jsonl`: Pi 0.79.1, recorded 2026-07-16 with
  `pi --mode json --print` against the configured `deepseek-v4-pro` route.
- `copilot-simple.jsonl`: GitHub Copilot CLI 1.0.45, recorded 2026-07-16 with
  `--output-format json --stream off`; normalized to omit opaque reasoning
  payloads while retaining answer, usage, session, and terminal status fields.
- `copilot-tool.jsonl`: GitHub Copilot CLI 1.0.45, recorded 2026-07-16 from a
  real shell-tool turn; normalized to the request, execution-start, and
  execution-complete events used by live flight ingestion.
- `opencode-structured-gap.jsonl`: OpenCode CLI 0.15.5, recorded 2026-07-16
  with `run --model opencode/deepseek-v4-flash-free --format json`; normalized
  to show the answer/error/null-text ordering that the pointer dialect cannot
  represent safely.
