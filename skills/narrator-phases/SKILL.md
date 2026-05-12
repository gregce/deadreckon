---
name: narrator-phases
description: Produces phase-by-phase prose for deadreckon RUN-NARRATIVE.md from turn records and diff samples.
output: json
inputs:
  - incremental_jsonl
  - diff_samples
  - tool_stdout
---

# narrator-phases

You are writing the phase body for a deadreckon run narrative.
Write for a maintainer trying to understand the delivered project, not for someone auditing every temporary path.

Return exactly one JSON object:

```json
{
  "phases": [
    {
      "title": "2-6 words",
      "commit_sha": "short sha or -",
      "prose": "one paragraph with [turn N] citations",
      "file_changes": ["`path`: +adds/-dels, what changed"],
      "citations": ["[turn N](../traces.jsonl#turn-N)"]
    }
  ]
}
```

## Inputs

- Incremental turn records:

```jsonl
{{ incremental_jsonl }}
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

- Write at least one prose paragraph per phase: intent -> tool calls -> changed files -> result.
- Reuse templated `auto_title` when no better title is supported by evidence.
- Cite `[turn N]` for every non-frontmatter claim.
- Quote a 1-3 line excerpt from the largest diff hunk inline.
- Include each documentable changed file in `file_changes` with `+adds/-dels`.
- Documentable files are user-authored source, config, manifests, tests, assets, and project docs that someone would intentionally maintain.
- Omit generated, vendor, cache, dependency, build-output, trace, snapshot, source-map, and run-artifact paths. Examples: `.next/`, `node_modules/`, `dist/`, `build/`, `target/`, `.turbo/`, `.cache/`, `.deadreckon/`, `*.map`, and `docs/RUN-*.md`.
- Do not omit documentable changed files; if a file is unclear, say what evidence exists instead of inventing a role.
- Keep phases compact. Large generated inventories are a documentation failure, not useful completeness.
