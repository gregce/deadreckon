# V1 Candidates

- Explicit sub-agent forking command: `deadreckon fork <run-id> --prompt "..."`, from AS-BUILT §10 and REPORT.md coordination needs.
- Provider HTTP retry taxonomy: `ProviderError::Http` currently carries provider/detail text but no HTTP status field, so the hygiene taxonomy treats it as fatal. Add a status/code field before retrying 408, 429, or 5xx provider failures.
