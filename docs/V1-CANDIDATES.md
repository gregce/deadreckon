# V1 Candidates

## Guided first-use follow-ups

- Durable launch profiles: save a reusable launch shape only after the current `start` path proves which choices users actually repeat. This would be new durable config, so it stays out of the schema-preserving production pass.
- Richer multi-piece goal classification: the production path has one bounded provider-backed goal-shape recommendation with deterministic fallback. V1 can add deeper LLM-backed multi-piece classification/decomposition once evidence limits, cost policy, explainability, and fixtures are explicit.
- Personalized onboarding: adapt setup copy and examples to a user's observed provider/source/done-contract patterns without adding telemetry or background profiling by default.
- Provider-specific setup wizards: offer richer guided configuration for individual CLIs or APIs once their install/auth flows are stable enough to document without making `start` brand-specific.
- Advanced `start` role pickers: the current picker reuses one selected provider route for review coder/reviewer or full-plan planner/child roles (per-role model flags landed in the stable-readiness pass; interactive per-role pickers did not). V1 can add role-specific and per-child provider/model selection once the simple picker proves useful.
- Production command release policy: after the default model proves itself, decide whether advanced verbs stay flat forever, move under namespaces, gain stronger aliases, or get deprecation windows. The production-release command model keeps every command callable and discoverable through `help-all`.

## Effortless follow-ups

- Themable palettes via config: production keeps one hard-coded palette in `ui.rs`; a theme system needs accessibility rules, snapshot strategy, and compatibility defaults.
- Localization hooks: status words, nouns, prompts, and command hints are English constants today. V1 can add localization once copy ownership and fallback behavior are explicit.
- Card template engine: cards are intentionally hand-built with `ui_card`/`ui` helpers. A template engine should wait until the layout grammar stabilizes enough to justify another abstraction.
- Long-lived notifier daemon: notifications are opt-in transition effects only. A daemon could batch, retry, and surface historical notifications after privacy, lifecycle, and process-supervision behavior are designed.
- Richer guided onboarding: provider-specific and repo-specific onboarding can build on `start`, `try`, and the done contract once it can stay local-first and avoid surprise telemetry.

## Uniform Surface follow-ups

- Complete table adoption: the shared `columns` primitive is used by the library table; the provider, plan-preflight, and chain tables keep their display-width-correct renderers and can migrate to `columns` incrementally for full structural uniformity.
- `comfy-table` for the plain-stdout path: reserved only if hand-rolled CJK/wrap/terminal-fit column math becomes a maintenance burden (add without the `tty` feature so it stays a pure String formatter).
- `anstyle-wincon` legacy-Windows console-API color translation: the hand-rolled ANSI escapes rely on the terminal honoring VT (fine on Windows Terminal/modern conhost); a wincon-aware fallback is deferred.
- Full confirm-modality audit: binary decisions use `confirm` and multi-way use `select_one`, but a complete run/finish/campaign sweep to guarantee the convention everywhere is incremental.
- Attach TUI uniformity is its own slice (`docs/goals/2026-06-05-0010-deadreckon-attach-tui-uniformity-goal.md`): one dispatcher, glyph, footer, and scroll indicator across run/plan/campaign/chain.
- In-frame text input / editable field: no call site today; re-evaluate `tui-input`/`tui-textarea` (pinned to ratatui-0.29-compatible releases) only when a real search/edit box is designed.

## Decompose follow-ups

- Core `pub mod` tightening with flat re-exports: Decompose left library module visibility alone because tightening it would rewrite deep-path callers and the public-surface baseline for no binary-layout gain.
- Chain/Plan field encapsulation behind transition methods: the current structs remain field-accessible because wrapping the existing call sites is a separate library API migration with higher behavior risk than the private binary split.
- Uniform `CommandHandler` trait: command families now live in private modules, but a trait-based `cli() -> exec()` framework remains cosmetic until there is repeated behavior worth abstracting.
- Public binary facade or `deadreckon-cli` crate: the binary still has no external frontend consumer, so there is no `pub run(cli) -> ExitCode` maintenance contract.
- `ProviderError` `#[source]` and sysexits exit codes: richer error chaining and exit taxonomy stay out of the behavior-preserving refactor because they would alter observable CLI behavior.
- `cli.rs` enum-per-family split: the clap command enum remains centralized to avoid rename churn and help/completion drift while command bodies move behind private modules.
- Integration-test submodule reorg: the lifted sibling test modules are intentionally boring; reshaping them into a deeper hierarchy can wait until test ownership pressure justifies it.

## Composable seams follow-ups

- Human-in-the-loop approval seam with pause/resume semantics and explicit operator UX.
- LLM-backed compaction summaries after cost, determinism, and evaluation policy are defined.
- Bus/WebSocket transport plus a long-lived worker registry for high-frequency seam workers.
- Packaged seam SDKs, published worker templates, and a registry-backed discovery flow beyond the local conformance examples.
- Seam versioning and capability negotiation.
- Routing built-in telemetry through the hook seam once durable-audit guarantees are preserved.
- Richer catalog capabilities beyond context-window and pricing metadata.

## General candidates

- Windows signing hardening: stable Windows artifacts now require Authenticode signing through CI secrets. V1 can move that key material to a managed signing service, hardware-backed certificate, or key-vault flow after the basic signed-artifact path proves out.
- Tamper-evidence hardening beyond the production gate: causal proof that a covered-file edit caused a pass, language-aware test detection beyond Rust heuristics, a separate signed tamper/audit log distinct from learning logs, fleet/plan-level tamper reporting, and sandboxing acceptance checks' own filesystem writes.
- Explicit sub-agent forking command: `deadreckon fork <run-id> --prompt "..."`, from AS-BUILT §10 and REPORT.md coordination needs.
- Richer provider retry policy: the turn loop now performs one bounded retry on transient provider failures (`ProviderError::Http` carries an explicit `retryable` flag tagged at each construction site for 408/429/5xx and transport blips; CLI rate-limit phrasings are recognized). V1 can add exponential multi-attempt backoff, `Retry-After` header parsing, per-provider retry budgets, and retry-aware route fallthrough ordering.
- OpenCode SQLite ingest: current provider CLI ingest reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON only. Add SQLite-backed discovery/parsing after choosing a dependency-light strategy and fixture shape.
- Richer import replay and analytics: production import writes normalized trace/provenance events plus `import.json`, but it does not yet rebuild provider-native turn graphs, replay sessions through deadreckon execution, or aggregate cross-run import analytics.
- Provider-session replay and bulk-agent registration: production flight rewind applies only DeadReckon file checkpoints and keeps provider-owned logs read-only. Replaying a provider session, mutating provider transcripts, registering large third-party CLI fleets, or sharing flight records across machines needs a separate design that preserves provenance and provider ownership boundaries.
- Richer flight rewind semantics: current checkpoints restore file bytes, not AST-aware patches or provider conversation state. V1 can add semantic diff previews, checkpoint compaction policy controls, cross-session analytics, and provider-specific event normalizers once the generic flight schema settles.
- Self-improvement beyond local PR gating: production learning is local, file-backed, deterministic before reflection, and PR-based. V1 can add opt-in cloud learning bundles, cross-machine sharing, richer benchmark/eval suites, multi-candidate evolutionary search, provider-routing learning from local evidence, semantic provider-session replay, tamper-evident audit logs, and any model-training/fine-tuning loop after the privacy and promotion policy is explicit.
- CLI stdout/run-output ingest and stdin prompt transport: Copilot and Pi now use saved provider sessions for the attach TUI. If a future provider needs `--no-session`, run-local JSONL stdout parsing, per-run `--session-dir`, or stdin prompt delivery for very large prompts, extend the descriptor schema with focused fixtures instead of adding one-off adapters.
- Rich semantic merge UI: merge repair is file-backed and CLI-first. A V1 UI could present side-by-side conflict versions, planner rationale, dependency graph context, and approval controls without requiring users to inspect `merge-proofs/` JSON directly.
- AST/language-aware merge engines: repair uses DAG precedence plus provider-planned file decisions. V1 can add parser-backed merges for common languages when the dependency weight and failure modes are clear.
- Plan documentation notebooks: production plan-result docs are file-backed and generated at merge/doc/apply/export time. V1 can add richer interactive notebooks, historical bulk regeneration, opt-in child-doc polish before consolidation, and shareable plan reports once privacy, cost, and provenance rules are explicit.
- Narrative beyond the shipped live narrator: live, continuity-carrying, model-driven run narration shipped in 0.2.0 (AS-BUILT §44) — a `dr run` narrates itself, attach renders the live beats, and the post-hoc doc seeds from them. Genuinely deferred: reading the narrator cadence/budget knobs from `[defaults]` in `config.toml`; a persistent streaming CLI session (one long-lived `claude`/`codex` process fed over `--input-format stream-json`) to amortize per-beat cold-start for higher-frequency narration; a long-lived cross-surface narrator daemon; richer trace DAG layout, learned summary preferences, historical narrative analytics, shareable/cloud observer views, graph layout engines, and team annotations once privacy and provenance rules are explicit.
- Attach responsiveness platform: attach remains file-backed and per-process. A V1 slice could add a long-lived attach daemon, one shared broadcaster across run/plan/chain surfaces, slow-stage telemetry, and a diagnostic dashboard for tick budgets/cache hit rates/provider-log scans after the durable-file contract has settled.
- Plan event bus hardening: plan attach now consumes a `PlanEventBus` feed over durable JSONL replay/tail plus a broadcast-capable runtime API, and it multiplexes child and repair run events. Future work can wire every same-process plan writer through a long-lived broadcaster if an embedded attach mode needs zero-file-hop delivery.
- Mass rename of stored status variants, especially `RunStatus::Executing` to `RunStatus::Running`. The coherence pass changed display text only.
- Themable palettes via config. Production keeps one hard-coded palette in `ui.rs`.
- Full output-layout facade: universal key/value rows, `try_line`/`next_action` helpers, table header helpers, stream-policy enforcement, and a generic lifecycle summary renderer for run/plan/chain/finish/apply/export/extend/resume/kill/cleanup.
- Orchestration UI polish beyond the live slice: richer interactive mode/child-count setup, a fuller output-layout facade, and deeper golden snapshots once the CLI layout settles. The live slice already added shared plan/fork/merge/orchestrate summaries, role tables, dependency/parallelism summaries, standard plan attach footers, structured merge-repair panels, and the production provider/done-contract setup resolver.
- Provider/done-contract setup hardening beyond the production resolver: golden snapshots for the exact setup rows, richer guided setup prompts, and any future durable config keys if V1 proves they are worth the schema cost.
- Command-matrix golden snapshots for help, summaries, prompts, table output, and JSON/plain/no-hints behavior once the CLI layout settles enough that snapshots catch regressions without making normal copy edits brittle.
- Localization hooks for status words, nouns, prompts, and command hints.
- Migration from hand-built status cards to a small template engine once the CLI layout stabilizes.
- Full command-family renames beyond hidden compatibility aliases, including removing `--force`, `--all`, `--branch`, and `--budget-cap` after the alias window closes.

## Campaign orchestration (beyond depth-2)

- Depth greater than 2 / cycle-safe arbitrary recursion: the campaign cap is a hard `CAMPAIGN_MAX_DEPTH = 2`. Deeper nesting needs a recursion-safe coordinator, cross-level cycle detection, and a blast-radius story before it earns its complexity.
- Cross-sub dependency edges: campaign sub-goals are independent islands. A V1 could model dependencies between sub-orchestrators so later sub-goals can intentionally build on earlier campaign work instead of only reconciling at roll-up.
- Campaign tree analytics beyond Helm: Helm ships the operator-facing flattened campaign -> plan -> run voyage tree in attach. V1 can still add expansion filters, cross-level event correlation diagnostics, replay/export of original event timing, and long-lived shared broadcasters for lower-latency multi-process attach.
- Planner-chosen per-sub breadth and per-sub provider roles: each sub-orchestrator runs a fixed small `--n` today; the planner could size each front and choose its providers.
- Tree-budget strategies beyond even split: weighted allocation by sub size, dynamic reallocation from finished subs, and concurrent (non-sequential) sub launch once aggregate-budget accounting stays correct under concurrency.
- Sharing campaign records across machines.

## Attach TUI Uniformity (deferred)

- `attach --web` / ratzilla mirror: Helm stays on ratatui in the terminal. A web
  mirror should reuse the same render/read-model code only after the terminal
  contract is stable.
- Provider pty embedding: Helm renders captured provider activity and flight
  files. Full live pty emulation inside attach remains a separate terminal
  virtualization problem.
- Live in-frame prompts beyond Helm: Helm P9 moves chain attach's kill confirm
  and extend input into in-frame modals. Remaining "press Enter to return"
  overlays around nested command output and completion actions still suspend the
  alternate screen; a V1 pass can finish those return overlays once their output
  capture story is explicit.
- Broader TUI text input: Helm P9 adds the ratatui-0.29-compatible
  `tui-textarea` path for single-line chain input and command-mode plumbing.
  V1 can expand input widgets to search, filtering, or multi-line editing once
  those workflows are designed.

## Logbook follow-ups (§49)

- Cross-run efficiency stats: Logbook makes single-run changed/spend/turn facts
  consistent; V1 can aggregate spend per accepted change, turns to done, retry
  loops, and regression rates across the library (`library stats` or
  `verdict --all --stats`).
- Context-health telemetry for CLI providers: Logbook records spend rows it can
  see, but CLI-provider token/context telemetry is still incomplete. Parse
  provider JSON usage where available and surface context headroom in attach
  once the provider fixtures are stable.
- Rich report UI: `deadreckon report --html` is static and self-contained.
  Live web/desktop mirrors, syntax-highlighted diff browsing, collapsible turn
  timelines, and shareable report bundles stay out until the terminal contract
  and privacy posture settle.
- MCP exposure: the shared `RunView` is a natural schema for a future
  `deadreckon mcp serve` inspection tool, but the stable slice only exposes it
  through the CLI and JSON.
- Report provenance polish: add per-field citation links and schema-versioned
  report manifests if external tools begin consuming report artifacts directly.

## Release integrity (embedded checksum verification)

- Inner-installer embedded checksums: the cargo-dist 0.31 generated shell
  installer prints "no checksums to verify" for tar.xz artifacts; integrity
  today is enforced one layer up, where `release/install.sh` verifies every
  downloaded artifact against the release's SHA256SUMS and dies on mismatch
  (and macOS archives are Developer ID signed + notarized). The upgrade path
  is a cargo-dist version whose shell installer verifies the embedded
  per-artifact sha256 fragments for tar.xz, at which point a fresh
  `curl | sh` transcript should show the inner installer verifying the
  artifact hash and the wrapper check becomes defense in depth.

## Orchestrated Narration follow-ups (0.3.0, §45)

- Wire `effective_plain` (auto-plain when stdout is not a TTY): the helper is
  unit-tested but intentionally unwired — the project renders rich box-drawing
  even when piped, and auto-plain-on-pipe broke the `cards_preview` fixture.
  A V1 needs a coherent piped-output story (which surfaces go plain, which
  stay rich) before flipping it on.
- Parent aggregate for campaign at the orchestrate level: the §45.5 stderr
  aggregate is wired into `dr orchestrate --narrate` only. A campaign relies on
  each sub-orchestrator emitting its own aggregate to its own stderr; a true
  campaign-level live aggregate (one calm line per sub-goal in the campaign
  parent) would need the campaign parent to tail each sub-plan's children.
- Provider-backed campaign narrative graph: `build_campaign_projection` ships a
  deterministic root→sub graph and never calls a model. A V1 could fold a
  provider beat over the aggregated sub headlines (mirroring the run/plan
  narrator) and build a richer cross-sub architecture graph.
- Live beats for the campaign view mid-run: the campaign projection folds each
  sub's freshest snapshot but does not itself emit schema-2 `live` beats, so a
  campaign has no rolling narrative of its own — only an aggregation of its
  children's. A V1 could give the campaign its own beat stream.

## Course follow-ups (Course, §46)

- Piece-goal seeding into dispatched plan tasks: an accepted reshape (and a
  planner draft's pieces) currently informs `--n` and the audit record; the
  plan's own planner re-decomposes. Threading explicit piece goals into
  `PlanTask`s needs a planner-bypass path with its own fixtures.
- Auto-reshape policy: accepting a proposal without an operator (under
  budget headroom and a config gate) is deliberately out — the accept is the
  human checkpoint. Revisit only with a blast-radius story.
- Campaign-level reshaping and planner-chosen per-sub breadth beyond the
  existing clamp.
- Chain-extend replay: a `--plan` replay of a continuation needs its parent
  run; refused today.
- Config keys for the guardrail knobs (`shape_confidence_floor`,
  `shape_auto_spend_ceiling`, `campaign_confirm_line`) — compiled defaults
  ship first; keys land when real use proves the defaults wrong.
- Card `e`dit as a full in-frame editor (today it exits to flags); budget
  split editing on the card.
- Learned shape priors from run history (self-improvement loop integration)
  and cross-machine launch plans.

## Contract follow-ups (Contract, §48)

- First-class behavioral check kinds: browser-driver and HTTP assertion checks
  are still expressed as `shell` helpers. A V1 schema can add native kinds once
  migration and gate rendering are designed.
- Standalone contract report verb: reuse `acceptance` and `start` surfaces for
  now; a future `deadreckon contract` or renamed detect-report command could
  expose compiled checks, lint, and divergence without launching.
- Multi-round critic and self-repair loops: stable Contract caps provider
  critique at one critic pass plus one redraft. More repair rounds need budget,
  loop detection, and human-review semantics.
- Per-check provenance: the compiled model does not record which draft or
  critic note produced a check. A V1 provenance ledger would need a sidecar
  format and retention policy.
- Semantic goal coverage: reconciliation is deterministic keyword coverage plus
  the single critic. Embedding or semantic coverage remains out until privacy,
  cost, and explainability are explicit.
- Auto-generating missing build/test harnesses in the target project: the
  compiler may propose helpers under `.deadreckon/acceptance/`, but scaffolding
  the project itself is V1.

## Polyglot done-contract follow-ups (Polyglot, §13.1/§35.9)

- Standalone `detect` report command: the project-kind + contract report is
  surfaced via `deadreckon run --preview`, not a dedicated verb — the `detect`
  verb is already the provider-probe command. A renamed report verb (e.g.
  `deadreckon contract`) could expose it standalone.
- Inferred-contract `test_globs` → tamper: an approved inferred contract's
  explicit `test_globs` are not yet threaded into tamper coverage (coverage
  comes from the proposed command being a recognized test runner). Threading
  the globs needs either a side file or an `AcceptanceCheck` field; deferred to
  avoid a schema change.
- Monorepo / multi-package detection: detection resolves one contract for the
  working-dir root; per-workspace/per-package contracts in a monorepo are out
  of scope.
- Multi-command pipelines (lint && test && build) as a single inferred default.
- Languages beyond the shipped native set (Scala/Clojure/Swift/C/C++/Haskell/
  Dart) — reachable today via the script-runner row or approved inference; the
  native table is extensible.
- Auto-installing missing toolchains/deps so a detected command can actually run.
- Blanket pytest `-k 'not …'` skip detection in the suppression lint.

## Semaphore follow-ups (Semaphore, §50)

- Claude `--json-schema` structured output: the capability probe already
  detects it (`ClaudeCapabilities.json_schema`), but wiring `output_schema` into
  the claude driver is a follow-up — Semaphore emits a caveat and proceeds
  unconstrained for claude today.
- Metered pricing / billing semantics for subscription CLIs: claude's reported
  `total_cost_usd` lands in the turn trace detail as informational only;
  `SpendEstimate` stays subscription/$0. Turning reported cost into a real
  dollar ledger is out of scope.
- The app-server route, steering, interrupts, and approvals (Rudder) layer on
  top of the per-run session file; Semaphore only lays the session-file
  foundation.
- Parsing codex `reasoning` / claude thinking items into narrative: the flight
  ledger records tool rows verbatim; a thinking-to-narrative projection is
  deferred.
- Cross-source flight dedup by semantic identity: Semaphore dedupes by
  suppression (a `live_contract` provider's post-hoc file scraper yields to live
  ingestion). Matching individual on-disk rows to live rows by content would let
  both sources run — unnecessary while live ingestion is authoritative.

## Rudder follow-ups (Rudder, §51)

- A shared app-server daemon and Unix socket transport. Stable Rudder keeps one
  supervised stdio child inside the provider instance so lifecycle and failure
  ownership stay local to the run.
- Cross-run server reuse. Threads and inboxes are deliberately run-scoped today;
  pooling a process needs isolation, cleanup and credential-boundary rules.
- Map Codex `thread/fork` and rollback operations onto DeadReckon rewind. The
  two histories need an explicit identity and proof-preservation contract before
  they can move together.
- Live steering for `cli:claude-code` or other provider routes. The stable
  command refuses them until a provider exposes a steer-and-acknowledge wire
  contract with the same no-drop guarantees.

## Pennant follow-ups (Pennant, §55)

- Contract hot reload: the registry reads and validates descriptor contracts at
  process start. Reloading `providers.d` safely during a running command needs
  cache invalidation for descriptors and capability probes.
- Operator contract overrides in `config.toml`: provider route entries can
  override binaries, models and arguments, but they cannot declare or replace a
  `[contract]`. A route-level override needs validation and clear precedence
  against built-in and `providers.d` descriptors.
- Richer event-mirror escalation: JSON pointers cover stable scalar and nested
  event shapes. Providers such as OpenCode need a registered event mirror when
  answer, error and terminal semantics depend on event order or predicates.
  This must reuse Semaphore's shared machinery without forking the generic
  driver.
