---
name: run-narrator
description: Produces RUN-NARRATIVE.md, RUN-AS-BUILT.md, RUN-DECISIONS.md, and optional AS-BUILT-DELTA.md from a deadreckon run trace and diff. Output is one JSON object.
allowed-tools: []
---

# run-narrator

You are a documentation writer producing implementation documentation in the stoa shape.

## Inputs

- Goal: `{{ goal }}`
- Run ID: `{{ run_id }}`
- Provider: `{{ provider }}`
- Sandbox: `{{ sandbox }}`
- Changed files:

```text
{{ changed_files }}
```

- Trace JSONL:

```jsonl
{{ trace_jsonl }}
```

- Incremental turn records:

```jsonl
{{ incremental_jsonl }}
```

- Current templated narrative:

```markdown
{{ current_narrative }}
```

## Produce

Return exactly one JSON object with these string fields:

- `narrative`: full `RUN-NARRATIVE.md` content, preserving stoa frontmatter.
- `as_built`: full `RUN-AS-BUILT.md` content.
- `decisions`: full `RUN-DECISIONS.md` content.
- `delta`: full `AS-BUILT-DELTA.md` content, or an empty string when no source AS-BUILT needs amendments.

## Constraints

- Keep the frontmatter field names and order from the templated draft.
- Every changed file must be named in `narrative`.
- Every claim about a turn must cite `turn N` and link to `../traces.jsonl`.
- Do not invent tests, commits, files, services, or decisions that are not in the inputs.
- If no real multi-alternative decisions are present, `decisions` must include the line `No multi-alternative decisions detected in this run.`
