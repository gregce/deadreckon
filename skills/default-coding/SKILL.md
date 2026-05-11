---
name: default-coding
description: Default unattended coding skill for deadreckon V0.
allowed-tools:
  - Bash
  - Read
  - Write
  - Edit
---

# default-coding

Run unattended coding work through the binary-owned phase machine. The skill layer owns judgment and prose; `/Users/gdc/deadreckon/target/release/deadreckon` owns state, locks, sandboxes, provider routing, snapshots, provenance, spend, traces, and gates.

Rules:

- Keep each turn small enough to snapshot and undo.
- Prefer structural checks before declaring work complete.
- Do not write acceptance markers directly; let the binary write and validate gates.
- Treat provider credentials as BYOK secrets and never echo key values.
