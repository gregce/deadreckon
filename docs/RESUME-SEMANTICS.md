# Resume Semantics

deadreckon resumes from durable runstate, not model memory.

Ordering:

1. Load `state.json`.
2. Read `traces.jsonl` line by line and keep only complete JSON entries.
3. Reconstruct tool-result history from complete `tool.*` trace entries when `history.json` is missing or `--from-turn` is supplied.
4. If `--from-turn N` is supplied, truncate history to turn `N`, set `state.turn = N`, and continue with turn `N + 1`.
5. Ignore any partial trailing trace entry. The next turn replays from the last complete tool boundary.

This makes a kill between provider response and tool dispatch recoverable: the partial trace is treated as advisory residue, never as completed work.
