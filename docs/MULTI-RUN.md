# Multi-Run Coordination

Lock ordering is scope first, run second.

Rules:

1. Compute the workspace scope from the git root or `DEADRECKON_SCOPE_ROOT`.
2. Acquire the scope/task lock at `/Users/gdc/.deadreckon/locks/<scope>--<task>.lock`.
3. Only after the scope lock is held, mutate files under `runstate/<scope>/runs/<run-id>/`.
4. Never acquire locks in the reverse order.
5. Release is idempotent; stale lock reclaim uses heartbeat age plus PID liveness.

Different scopes may run the same task key concurrently. A second run in the same scope/task is refused with the first run id and phase so the operator can attach, kill, or resume the owner.
