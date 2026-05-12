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
- Do not invent decisions from ordinary implementation summaries.
- If the evidence is ambiguous, omit the entry rather than overstate it.
