GOAL: Put a watchkeeper over every approved goal: one durable job identity that survives terminal, worker, supervisor and machine interruption; stops for a typed bounded reason; requires deterministic checks plus an independent semantic judgment; and leaves a cryptographically bound receipt. Today `start` persists a launch decision but awaits foreground executors. Runs resume manually, chains alone detach, plan adoption is manual, the gate judges checks but not meaning, and its public seal is forgeable.

**Read first.**

- `docs/goals/2026-07-28-2321-deadreckon-watchkeeper-rider.md` — schemas, lifecycle, service contract, phases and depth tests.
- `docs/goals/2026-07-25-1147-deadreckon-binnacle-rider.md` — gate substrate, amended here instead of duplicated.
- `docs/goals/2026-07-11-1122-deadreckon-capstan-rider.md` — process supervision prerequisite.
- `docs/AS-BUILT-ARCHITECTURE.md`, `docs/MAP-OF-DEADRECKON.md`, and Graph commits through `da49212`.
- Temporal workflow execution and CLI lifecycle docs — durability and operator-semantics exemplar, not a distributed-system target.

**Posture.** Stable track. The local supervisor/queue is required by the original promise. `Job` is the operator object, `Plan` the graph executor, and a run the isolated evidence-bearing attempt. Existing artifacts remain readable. No cloud service, database server, arbitrary workflow language, push, release or live service installation in tests.

**Settled model.**

- `start` durably records an approved launch plan, immutable authority manifest, job and `queued` event before returning one id.
- A local supervisor claims jobs with expiring lease epochs, periodically heartbeats, supervises process groups, and reconciles abandoned attempts without duplicate execution.
- launchd/systemd-user restart the supervisor; other platforms report unsupported. Lazy spawn is a fallback, not an equivalent.
- One append-only job history is lifecycle truth. Existing run events, traces, spend, snapshots and docs remain rich leaf evidence.
- Completion is two-key: deterministic gate pass, then a fresh read-only semantic judge returns `achieved`, `revise`, or `uncertain`. Failed checks can never be overridden.
- Promotion requires a signed receipt bound to goal, contract, policy, base, result, proofs, containment and stop reason.
- `start`, `attach`, `status`, `list`, `finish` project `JobView`; advanced shape verbs remain compatibility creators/adapters during migration.

**Phases.** P1–P11 are in the rider. Each starts with named depth tests, then implementation, focused green tests and a local commit. Milestones run `make verify`. P11 updates architecture and public claims.

**Verification.**

- A queued smoke job returns its id before work finishes; closing the caller does not stop it.
- Killing the worker and then the supervisor recovers the same id, workspace, budgets and evidence with no concurrent duplicate attempt.
- Service-restart simulation resumes open work; service manifests are platform-tested without installing them.
- Spend, wall, retry, deadline, blocked, needs-review, cancelled and fatal stops remain distinct.
- A failed deterministic check never reaches semantic acceptance; `revise` returns to work; `uncertain` or unavailable strict judgment stops `NEEDS_REVIEW`.
- Mutation of the contract, policy, source/result digest, semantic judgment, containment or receipt invalidates promotion.
- Run, ordered graph, parallel graph and nested graph share the same job lifecycle and five-command journey.

**Stop when** rider tests and `make verify` pass; hostile-agent and crash-adoption suites pass or report unavailable backends honestly; a 20–30-task dogfood matrix and metrics artifact exist; public docs state only enforced guarantees; CHANGELOG is updated; and an operator checklist is ready.
