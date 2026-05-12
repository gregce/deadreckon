---
name: narrator-overview
description: Produces overview sections for deadreckon RUN-NARRATIVE.md in the stoa implementation-doc shape.
output: json
inputs:
  - goal
  - run_summary
  - incremental_jsonl
  - diff_samples
  - tool_stdout
  - parent_narrative
---

# narrator-overview

You are writing the overview material for a deadreckon run narrative. Use newsroom voice: concrete, specific, no filler.
The reader wants a practical project handoff: what changed, how to use it, how it was checked, and what to inspect next.

Return exactly one JSON object:

```json
{
  "overview": "paragraph",
  "reading_order": "paragraph or empty string",
  "why_now": "2-4 sentences",
  "high_level_approach": "one paragraph",
  "open_threads": ["bullet"],
  "cross_references": ["bullet"]
}
```

## Inputs

- Goal: `{{ goal }}`
- Run summary: `{{ run_summary }}`
- Parent narrative, when this extends a prior run:

```markdown
{{ parent_narrative }}
```

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

- `reading_order`: emit only when the parent narrative is non-empty; point the reader to the parent and to what changed since.
- `why_now`: synthesize the goal and run context; do not say "the user asked".
- `high_level_approach`: name the actual approach from provider output, diff samples, and tool output, including pivots or superseded attempts.
- Put user-visible behavior and maintainer-relevant files ahead of deadreckon mechanics.
- `open_threads`: scan for TODO, out of scope, follow-up, punted, noted but not implemented, or provider warnings.
- `cross_references`: include traces, provenance, snapshots, branch/library artifacts, acceptance, and parent narrative when present, but keep this as an audit appendix instead of the main story.
- Do not mention generated/vendor/cache/build artifacts except as one concise verification note when relevant. Examples to omit: `.next/`, `.astro/`, `.output/`, `node_modules/`, `.venv/`, `.gradle/`, `CMakeFiles/`, `.dart_tool/`, `.terraform/`, `dist/`, `build/`, `target/`, `.turbo/`, `.cache/`, source maps, local `.env*` files, traces, snapshots, and run artifacts.
- Every non-frontmatter claim about work done must cite `[turn N]`.
