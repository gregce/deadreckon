# Changelog

## 0.1.0 - Robustness Milestone (alpha)

Commit: `14a9351`.

- Hardened the run loop with broadcast/file-backed events, per-turn timers, cancellation tokens, wall-clock CLI spend accounting, partial-trace resume, and `resume --from-turn`.
- Hardened sandbox execution with generated Seatbelt/bwrap policy inputs, tmp `$HOME`, network denial, persisted profiles, and adversarial path/network tests.
- Hardened acceptance by moving `dr-gate` to `acceptance.yaml`, signing markers with a run-local nonce, and refusing forged self-attestation.
- Hardened import normalization for Claude Code, Codex, and Cursor histories into deadreckon traces/provenance.
- Hardened multi-run coordination with scope-qualified lock files and same-scope refusal tests.
- Hardened library promotion with post-gate atomic move, manifest writing, and crash recovery.

Still thin: provider pings in `doctor` are intentionally conservative unless explicitly enabled, and the TUI uses durable event replay for cross-process attach because Tokio broadcast is in-process.
