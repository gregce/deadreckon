---
name: narrator-decisions
description: Produces RUN-DECISIONS.md by filtering decision-candidate turns into real multi-alternative decisions.
output: json
inputs:
  - incremental_jsonl
  - trace_jsonl
  - diff_samples
---

# narrator-decisions

You are writing the decisions artifact for a deadreckon run.

Return exactly one JSON object:

```json
{
  "decisions": [
    {
      "title": "short title",
      "turn": 1,
      "considered": ["alternative"],
      "chosen": "choice",
      "why": "reason with evidence",
      "files_affected": ["path"],
      "citations": ["[turn 1](../traces.jsonl#turn-1)"]
    }
  ]
}
```

Return `{ "decisions": [] }` when no real multi-alternative decision appears.

## Inputs

- Incremental turn records:

```jsonl
{{ incremental_jsonl }}
```

- Trace JSONL:

```jsonl
{{ trace_jsonl }}
```

- Diff samples:

```markdown
{{ diff_samples }}
```

## Requirements

- Inspect only turns with `decision_candidate: true`; filter false positives.
- A real decision must include alternatives considered, the chosen path, why it was chosen, files affected, and citations.
- `files_affected` must name only documentable user-authored source, config, manifest, test, asset, or project-doc files.
- Omit generated/vendor/cache/build-output/local-secret paths such as `.next/`, `.astro/`, `.output/`, `node_modules/`, `.venv/`, `.gradle/`, `CMakeFiles/`, `.dart_tool/`, `.terraform/`, `dist/`, `build/`, `target/`, `.turbo/`, `.cache/`, source maps, `.env*`, traces, snapshots, and run artifacts.
- Do not invent decisions from ordinary implementation summaries.
- If the evidence is ambiguous, omit the entry rather than overstate it.
