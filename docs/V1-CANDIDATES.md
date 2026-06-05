# V1 Candidates

## Guided first-use follow-ups

- Durable launch profiles: save a reusable launch shape only after the current `start` path proves which choices users actually repeat. This would be new durable config, so it stays out of the schema-preserving production pass.
- Richer multi-piece goal classification: the production path has one bounded provider-backed goal-shape recommendation with deterministic fallback. V1 can add deeper LLM-backed multi-piece classification/decomposition once evidence limits, cost policy, explainability, and fixtures are explicit.
- Personalized onboarding: adapt setup copy and examples to a user's observed provider/source/done-contract patterns without adding telemetry or background profiling by default.
- Provider-specific setup wizards: offer richer guided configuration for individual CLIs or APIs once their install/auth flows are stable enough to document without making `start` brand-specific.
- Advanced `start` role pickers: the current picker reuses one selected provider route for review coder/reviewer or full-plan planner/child roles. V1 can add role-specific and per-child provider selection once the simple picker proves useful.
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
- Provider HTTP retry taxonomy: `ProviderError::Http` currently carries provider/detail text but no HTTP status field, so the hygiene taxonomy treats it as fatal. Add a status/code field before retrying 408, 429, or 5xx provider failures.
- OpenCode SQLite ingest: current provider CLI ingest reads OpenCode file-mode `storage/session`, `storage/message`, and `storage/part` JSON only. Add SQLite-backed discovery/parsing after choosing a dependency-light strategy and fixture shape.
- Richer import replay and analytics: production import writes normalized trace/provenance events plus `import.json`, but it does not yet rebuild provider-native turn graphs, replay sessions through deadreckon execution, or aggregate cross-run import analytics.
- Provider-session replay and bulk-agent registration: production flight rewind applies only DeadReckon file checkpoints and keeps provider-owned logs read-only. Replaying a provider session, mutating provider transcripts, registering large third-party CLI fleets, or sharing flight records across machines needs a separate design that preserves provenance and provider ownership boundaries.
- Richer flight rewind semantics: current checkpoints restore file bytes, not AST-aware patches or provider conversation state. V1 can add semantic diff previews, checkpoint compaction policy controls, cross-session analytics, and provider-specific event normalizers once the generic flight schema settles.
- Self-improvement beyond local PR gating: production learning is local, file-backed, deterministic before reflection, and PR-based. V1 can add opt-in cloud learning bundles, cross-machine sharing, richer benchmark/eval suites, multi-candidate evolutionary search, provider-routing learning from local evidence, semantic provider-session replay, tamper-evident audit logs, and any model-training/fine-tuning loop after the privacy and promotion policy is explicit.
- CLI stdout/run-output ingest and stdin prompt transport: Copilot and Pi now use saved provider sessions for the attach TUI. If a future provider needs `--no-session`, run-local JSONL stdout parsing, per-run `--session-dir`, or stdin prompt delivery for very large prompts, extend the descriptor schema with focused fixtures instead of adding one-off adapters.
- Rich semantic merge UI: merge repair is file-backed and CLI-first. A V1 UI could present side-by-side conflict versions, planner rationale, dependency graph context, and approval controls without requiring users to inspect `merge-proofs/` JSON directly.
- AST/language-aware merge engines: repair uses DAG precedence plus provider-planned file decisions. V1 can add parser-backed merges for common languages when the dependency weight and failure modes are clear.
- Plan documentation notebooks: production plan-result docs are file-backed and generated at merge/doc/apply/export time. V1 can add richer interactive notebooks, historical bulk regeneration, opt-in child-doc polish before consolidation, and shareable plan reports once privacy, cost, and provenance rules are explicit.
- Narrative attach beyond the current terminal view: the view is file-backed, manually refreshable, and terminal-first. V1 can add a long-lived narrator daemon, richer trace DAG layout, learned summary preferences, historical narrative analytics, shareable/cloud observer views, graph layout engines, and team annotations once privacy and provenance rules are explicit.
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
- Flattened campaign live attach with a true event tree: production `attach <campaign-id>` is a navigated campaign TUI that drills into existing plan/run TUIs. A V1 could extend `PlanEventBus` to a hierarchy and stream the entire campaign -> plan -> run tree in one pane with expansion, filtering, and cross-level event correlation.
- Planner-chosen per-sub breadth and per-sub provider roles: each sub-orchestrator runs a fixed small `--n` today; the planner could size each front and choose its providers.
- Tree-budget strategies beyond even split: weighted allocation by sub size, dynamic reallocation from finished subs, and concurrent (non-sequential) sub launch once aggregate-budget accounting stays correct under concurrency.
- Sharing campaign records across machines.
