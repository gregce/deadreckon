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

- Implement the SPEC in the working directory. Treat the user's goal plus any
  acceptance.md / acceptance.yaml content as the spec.
- As you work, maintain `implementation-notes.html` at the working-directory
  root. Keep it current with anything the owner should know about how the
  implementation interprets or diverges from the spec. Preserve these exact
  section IDs and headings because deadreckon uses them to render the published
  decision ledger:
  - `<section id="design-decisions">` with heading `Design decisions`: choices
    made where the spec was ambiguous.
  - `<section id="deviations">` with heading `Deviations`: intentional
    departures from the spec, with reasons.
  - `<section id="tradeoffs">` with heading `Tradeoffs`: alternatives
    considered and why the chosen path won.
  - `<section id="open-questions">` with heading `Open questions`: anything the
    owner should confirm or revise.
- Before reporting done, update `implementation-notes.html` after the latest
  documentable code/config/test/doc change. If there is nothing to report in a
  section, say "None" rather than deleting the section.
- deadreckon renders the same content into `RUN-DECISIONS.md`; treat the HTML
  file as the live working copy for that published decisions ledger.
- Keep each turn small enough to snapshot and undo.
- Prefer structural checks before declaring work complete.
- Do not write acceptance markers directly; let the binary write and validate gates.
- Treat provider credentials as BYOK secrets and never echo key values.
