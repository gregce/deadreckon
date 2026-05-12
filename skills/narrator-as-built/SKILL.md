---
name: narrator-as-built
description: Produces RUN-AS-BUILT.md for a deadreckon run using stoa subsystem-AS-BUILT conventions.
output: json
inputs:
  - goal
  - run_summary
  - source_layout
  - diff_samples
  - tool_stdout
---

# narrator-as-built

You are writing an as-built document for the subsystem changed by a deadreckon run.

Return exactly one JSON object:

```json
{
  "subject": "one line",
  "system_overview": "paragraph",
  "components": "markdown table",
  "topology": "fenced ASCII block or empty string",
  "file_layout": "fenced text tree/list",
  "external_interactions": ["bullet"],
  "cross_references": ["bullet"]
}
```

## Inputs

- Goal: `{{ goal }}`
- Run summary: `{{ run_summary }}`
- Source layout seed:

```markdown
{{ source_layout }}
```

- Diff samples:

```markdown
{{ diff_samples }}
```

- Tool stdout/stderr:

```markdown
{{ tool_stdout }}
```

## Requirements

- Components table columns must be exactly `Layer | Responsibilities | Key entrypoints`.
- Every component row must include at least one `file:line` entrypoint.
- The layer name `Project files` is forbidden. If the layer cannot be inferred, omit it.
- `system_overview` must include two short sub-paragraphs beginning `What's load-bearing:` and `Where the seams are:`.
- Include process/data topology ASCII when the seed has one; omit rather than emit a generic placeholder.
- Include wire protocols, local files, subprocesses, or external services touched by the run when present in the evidence.
- Link to `RUN-NARRATIVE.md`, `RUN-DECISIONS.md`, source AS-BUILT when available, traces, provenance, snapshots, and acceptance.
