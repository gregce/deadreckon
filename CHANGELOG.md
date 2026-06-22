# Changelog

## 0.3.1 — Narration actually narrates — 2026-06-22

Found by the first real end-to-end campaign run (every prior check was a unit
test): the live narrator wrote zero beats in real runs, and campaign
sub-orchestrators crashed on `--narrate`. Three fixes, each with a regression
test that reproduces the failure:

- fix(narrator): the live narrator never wrote a beat in real runs. `emit()`
  awaited a model call inline in the engine loop; for short runs that call was
  still in flight at shutdown, the 5s grace timed out, and the runtime aborted
  the task before any beat was committed. The engine's cancel token is now
  threaded into the `ProviderRequest`, so shutdown interrupts the in-flight call
  and `emit()` falls through to a floor beat. This affected all narration,
  including `dr run --narrate` from 0.2.0.
- fix(narrator): on shutdown, buffered `DocsCheckpoint`/`RunCompleted` events
  are drained (window-fill only, no blocking model call) before the final floor
  flush — `cancel` and a non-empty `recv()` race in the `select!`, and if cancel
  won, a fast run produced zero beats.
- fix(cli): `orchestrate full-plan` and `orchestrate review` now accept
  `--narrate/--no-narrate/--narrator-model`. They were only on the bare
  `orchestrate` command, so every campaign sub-orchestrator (`orchestrate
  full-plan … --narrate`) exited with "unexpected argument '--narrate'". The
  propagation test now parses the real sub-orchestrator argv through clap.
- feat(campaign): campaign-parent live aggregate. With `dr campaign --narrate`
  the parent publishes each sub-orchestrator's plan id early, tails that sub's
  grandchild leaf runs, and prints one framed line per active sub-goal to
  stderr (`campaign <id> · sub-i (i/N) <status> · <freshest descendant beat>`).
  The sub-orchestrator's own per-task aggregate is suppressed under a campaign
  so the campaign parent owns the live surface.

## 0.3.0 — Orchestrated Narration — 2026-06-17

Extends the Live Narrator (0.2.0, AS-BUILT §44) to every orchestrated and
campaign child. Children are subprocesses, so they got zero live beats; now
each narrates file-only to its own `snapshots.jsonl`, the plan attach surfaces
each child's live headline, `dr orchestrate --narrate` prints a one-line-per-
child stderr aggregate, and `dr attach <campaign> --view narrative` renders a
campaign projection at plan parity. See AS-BUILT §45.


- P1: shared `build_run_narration` helper (extracted from run.rs) + a
  `resolve_narrator_config_for_child` that narrates orchestrate/campaign
  children FILE-ONLY (foreground=false, headless_append=false) so beats hit
  `snapshots.jsonl` but never a child's stdout/stderr. Children default to the
  deterministic floor unless a `--narrator-model` is pinned; the `dr run`
  TTY contract is unchanged. The child path activates via the
  `DEADRECKON_NARRATE_CHILD` env the parent sets.
- P2/P3: both `extend` paths (in-place + worktree) now wire the narrator —
  reviewer children re-entering `extend` narrate file-only via
  `build_run_narration` (previously `event_sender:None`). `extend`/and the
  shared `resolve_narration` thread `--narrate/--no-narrate/--narrator-model`;
  shutdown is bounded and runs before lock release. The `dr run` TTY contract is
  unchanged.
- P4/P5: `dr orchestrate` gains `--narrate/--no-narrate/--narrator-model`,
  threaded through `fork_command` to each spawned child's argv (`run_plan_child`
  appends `--narrate` + sets `DEADRECKON_NARRATE_CHILD=1` and
  `DEADRECKON_AUTH_PROBE=0`), so coder/full-plan children narrate file-only to
  their own `snapshots.jsonl`. Child argv building is extracted into a pure
  `child_argv` so propagation is unit-tested without spawning.
- P7: `spend_summary` now counts only `kind:"loop"` rows — a latent leak where
  `kind:"narrator"` rows (written by any narrating run) inflated tokens/turns/
  wall and could overwrite the run's `total_usd`. The total is taken from the
  last loop row.
- P6: `dr campaign` gains `--narrate/--no-narrate/--narrator-model`, appended to
  the `orchestrate full-plan` sub-orchestrator argv (`build_sub_orchestrator_command`),
  so the campaign → orchestrate → run/extend chain narrates end to end.
- P10: campaign Narrative view (Option D2) — `dr attach <campaign-id> --view
  narrative` (and `--json`) now renders a campaign-scoped projection built by
  `build_campaign_projection`, aggregating each sub-goal's freshest child
  narration (its merged run's live beat, else its sub-plan's snapshot) into an
  agent table. It flows through the same `narrative_plain_lines` renderer as a
  plan, so the campaign narrative has full section parity. Adds a `Campaign`
  `NarrativeScope` variant (additive).
- P11: docs — AS-BUILT §45 (Orchestrated Narration) added; §44.5/§44.6
  corrected (`effective_plain` is unwired; the spend-math guarantee was only
  made real in §45.7); §22 shipped list updated; V1-CANDIDATES gains an
  Orchestrated Narration follow-ups section.
- P9: parent aggregate stderr line (Option D1) — when `dr orchestrate --narrate`
  is active the parent tails each running child's `snapshots.jsonl` (reusing
  `JsonlTail`) and prints one capped line per active child to STDERR every ~2s,
  preferring each child's latest Live beat. The aggregate never touches stdout
  (the parent scrapes children's run-ids off its own stdout), enforced by a
  test-threaded sink.
- P8: plan-attach surfacing reliability — the per-child agent table caps at
  `PLAN_AGENT_TABLE_MAX` active children with a `+N more` overflow line, and
  `latest_child_narrative_snapshot` now reads via `read_latest_live_snapshot`,
  which prefers the latest Live beat over a later attach-time Deterministic
  projection so an on-demand refresh can never mask a child's live headline.

## 0.2.0 — Live Narrator

A `dr run` now narrates itself in plain English as it works — a
continuity-carrying, subscription-first, model-driven sidecar with a
deterministic floor — so an operator can glance at progress instead of reading
tool calls, edits, and JSONL. Phase detail follows.


- P1: narration snapshot schema 2 — additive `live` beat field (beat_seq,
  covers_turn, source, rolling_summary) that legacy schema-1 snapshots parse
  as absent; `SpendRecord.kind` (defaulted "loop", narrator rows write
  "narrator"); additive `RunLoopConfig.narrate: Option<NarratorConfig>` (None
  preserves every existing constructor).
- P2: narrator backend selection — subscription-first preference
  (claude-code/haiku → codex/gpt-5.1-codex-mini → anthropic/claude-haiku-4-5 →
  openai/gpt-4o-mini → deterministic floor). Pure `select_narrator_route` over
  an availability predicate, plus a registry-backed predicate that gates CLIs
  on binary presence + login state and HTTP on a non-empty API-key env var.
  `--narrator-model` overrides the model without changing provider order.
- P3: the run process now wires a `RunEventBus` into the turn loop and spawns an
  in-process narrator sidecar that drains run events and stops cleanly when the
  run finishes or is cancelled. On a TTY narration is on by default; off-TTY
  without `--narrate` the run is wired exactly as before (no bus, no task).
- P4: continuity — `build_live_narrator_prompt` feeds the model the prior
  narrative + only the windowed new turns + a rolling summary and asks it to
  amend/extend; `apply_live_narrator_response` merges the reply into a NEW
  appended beat (never overwriting the prior beat) and validates claims against
  prior-citation + new-turn (`turn:N`) evidence, so beats may add genuine new
  claims but never cite a turn outside the window. New `skills/live-narrator`
  prompt skill carries the voice.
- P5: `NarratorWindow` accumulates only the turns since the last beat (never the
  full trace) and folds them into a rolling summary bounded to 1200 chars by
  eliding older content — so each beat's model input is a constant ceiling and
  total narration cost is O(turns), not O(turns²). `turn_record_to_input` maps a
  persisted `TurnRecord` into the per-turn narrator input.
- P6: cadence — `cadence_decision` emits a model beat only when there is new
  work and either the min gap elapsed or a turn burst accumulated, coalescing
  faster bursts and capping total beats per run; a long single turn escalates
  to a beat via the quiet timer. Between model beats a deterministic $0 ticker
  (`turn N · tool (elapsed)`) keeps a long turn from looking frozen, with no
  provider call.
- P7: narrator spend isolation — `NarratorLedger` tracks the narrator's own
  spend against its per-run budget cap, fully separate from the run loop's
  totals; `narrator_should_use_model` degrades to the deterministic floor once
  the cap is hit (or the backend is the floor); `narrator_spend_record` writes
  `kind: "narrator"` rows so the run's spend math (which filters `kind: "loop"`)
  never counts narration. Subscription backends record $0.
- P8: foreground calm block — `live_block_lines` renders the headline plus the
  top current_work claims bounded to `narrate_lines` (a few lines max, never a
  stream); `ForegroundBlock` redraws in place (clearing the prior block) so the
  block updates rather than scrolls. Foreground narration is on by default on a
  TTY and off under `--no-narrate`.
- P9: headless narration — `dr run --narrate` streams append-only, turn-stamped
  beats (`[turn N] …`) to stderr, keeping stdout clean for piped consumers;
  `--no-narrate` disables narration and `--narrator-model` pins the model
  (validated against the catalog). Raw cursor-control ANSI moved to the `ui`
  module to honor the source coherence contract. The silent-piped-run progress
  decision (`effective_plain`) is unit-tested but deliberately not wired to the
  run surface — this project keeps rich rendering when piped (opting out only
  via `NO_COLOR`/`--plain`), so `--narrate` is the piped-progress path and the
  auto-plain wiring is a V1 candidate.
- P10: attach + post-hoc convergence — the attach Narrative view renders the
  live beats the run already wrote to `snapshots.jsonl` with no provider call;
  the post-hoc `RUN-NARRATIVE.md` seeds `current_narrative` from the full
  accumulated live narration (digest of every beat + the latest rolling
  summary), consolidating the live story rather than re-deriving from the raw
  trace. `--narrator-model` is validated against the catalog and refused with a
  `try: deadreckon models` hint; conflicting `--narrate`/`--no-narrate` is
  refused.

## 0.1.1

- Subscription CLI runs default to a ten-hour wall cap (was one hour):
  the fallback in run/extend resolution and the `cli_max_wall_seconds`
  value written by `init` are now 36000 seconds. Explicit
  `--max-wall-seconds` and configured `defaults.cli_max_wall_seconds`
  still win.
- Failure surfacing: plan- and campaign-level refusals name the
  underlying child failure reason (session limits, wall caps) instead of
  only their own layer's status; provider quota errors surface as
  resumable with the provider's stated reset time; refused campaign
  roll-ups recommend resuming the interrupted children, and
  `campaign repair` refuses honestly when subs never merged. Error
  footers interpolate real ids and drop the generic doctor hint when a
  specific try line exists.
- Homebrew publish job pulls the formula from the release assets (the
  v0.1.0 cut proved the artifact-bundle path wrong).

## 0.1.0 — Stable

Release highlights distilled from the 0.1.0-rc.2 through 0.1.0-rc.11
candidates; the sections below carry the full per-change record.

- First-class model selection: populated per-provider model catalogs, a
  `deadreckon models` verb, an interactive model picker in `start`, and
  per-role model flags (`--model`, `--planner-model`, `--coder-model`,
  `--reviewer-model`, `--child-model IDX=MODEL`) across run, chain,
  orchestrate, and campaign — echoed on previews and provider-role tables.
- Never-dead-end launches: an unusable resolved provider on a TTY drops into
  the probe-before-ask provider picker instead of refusing; off-TTY refusals
  are unchanged.
- Durability: history.json corruption falls back to traces.jsonl
  reconstruction with atomic re-save; lock reclaim never usurps an alive
  holder pid regardless of heartbeat age.
- One prompt engine (inquire) behind every interactive surface, a gradient
  wordmark banner, smart bare invocation, and a visually informative
  installer with SHA256SUMS verification.
- Release trust end to end: signed + notarized macOS archives, fail-closed
  rc/stable lanes, self-update re-homed and proven live, CI on every push
  with the full 54-binary suite green.
- Consciously narrowed for this cut: npm publishing (no npmjs token yet)
  and Windows Authenticode signing (no certificate yet) are deferred via
  explicit policy flags; the Windows zip ships unsigned, and Homebrew +
  curl-installer + GitHub release are the supported channels.

## Stable Readiness - 2026-06-10

- Populated model catalogs for every built-in provider descriptor, each with
  exactly one recommended entry; custom descriptors fail closed on multiple
  recommendations.
- `deadreckon models [PROVIDER] [--all] [--json]` — the catalog surface for
  choosing a model, marking the recommended entry and the configured default.
- Per-role model flags: `--model` on start/run/chain, `--planner-model` /
  `--model` / `--child-model IDX=MODEL` on orchestrate full-plan,
  `--coder-model` / `--reviewer-model` on orchestrate review, campaign
  equivalents — additive serde-default fields on plan.json, echoed in
  previews and the provider-roles table; "provider default" sends no model
  argv.
- Interactive `start` gains a model picker after the provider choice.
- Never-dead-end launches: unusable resolved routes on a TTY drop into the
  provider picker (keep/cancel reproduce the original refusal); off-TTY
  refusals byte-identical.
- history.json corruption falls back to traces.jsonl reconstruction with an
  atomic re-save; save_history writes via tempfile + rename.
- Lock reclaim never usurps an alive holder pid; LockHeld names the
  heartbeat age and the kill --force escape hatch.
- Stable-lane gates: `## 0.1.0` CHANGELOG section, lane-asymmetry depth
  tests (changelog + npm wrapper pins are stable-only), explicit
  checksum = "sha256", embedded-checksum upgrade path recorded.
- `release/preflight-real.sh` real-provider proof harness +
  `release/known-good-providers.json` (schema_version 1); stable v0.1.0
  operator checklist and Windows smoke checklist in docs/RELEASE.md;
  models/picker/rescue documented in HOWTO.md.

## Self-update that actually updates - 2026-06-10

- The axoupdater-backed `deadreckon update` pointed at the pre-re-home
  `gdc/deadreckon` repository in six places (release source, API URLs, brew
  tap hint), so every real update would 404. All update surfaces now point at
  `gregce/deadreckon`, and the portability guard gained a repo-slug list so
  `gdc/` references cannot return — which immediately caught the Homebrew tap
  still reading `gdc/homebrew-tap` in dist-workspace.toml, the release
  workflow, the formula patcher, and the manifest.
- Latest-release resolution is RC-era aware, mirroring the installer:
  `releases/latest` (stable) first, newest release of any kind as fallback —
  no more silent "up to date" while newer release candidates exist. When the
  resolved latest is itself a prerelease, the updater installs it without
  requiring `--pre`.
- Proven live end to end: a sandboxed shell install with an rc.7 receipt
  resolved rc.10 via `update --check`, swapped binaries with `update --yes`
  (rollback backup retained), and the cached startup hint
  ("deadreckon X is available...") fires on the next TTY command. The
  evidence panel's installer-asset URL is also fixed
  (releases/tag -> releases/download).

## Visual interaction overhaul: banner, smart bare invocation, one prompt engine - 2026-06-09

- Help surfaces gain a figlet wordmark with a per-character 256-color
  gradient (twelve palettes, picked per invocation) and a version tagline —
  TTY-only, so pipes and every output contract stay byte-clean.
- Bare `deadreckon` reads the room: no config on the machine → a first-run
  welcome listing detected agent CLIs with an on-TTY offer to run guided
  setup; configured but no runs in this directory → orientation (source mode
  start would use here, the production flow, where other runs live); runs
  present → status, as a returning operator expects.
- One prompt engine (inquire) powers every interactive surface: arrow-key
  selects with detection hints, styled confirms, validated number input, and
  text prompts, themed to the shared Tone palette and colorless under
  --plain/NO_COLOR. Off-TTY and DEADRECKON_PROMPT_LINE_MODE keep the
  original numbered line prompts byte-stable for scripts and tests. Existing
  pickers (start, campaign, orchestrate, config) inherit the upgrade through
  the shared API.
- The init provider prompt is a probe-before-ask menu: detected subscription
  CLIs lead with live login-state hints, API routes show whether their key is
  already exported, and a typed route stays reachable. The legacy hand-rolled
  stderr non-git menu is unified into the same engine.

## Installable macOS archives and user-verifiable SHA256SUMS - 2026-06-09

- rc.7's macOS `curl | sh` failed at the final `mv`: the signing step
  repacked archives with `tar -C dir .`, prefixing every member with `./`,
  which breaks the cargo-dist shell installer's layout resolution. The repack
  now packs explicit top-level names, and `verify-manifest` fails closed on
  any `./`-prefixed archive member so this cannot ship again.
- `SHA256SUMS` now records flat basenames (one entry per published asset,
  identical nested CI duplicates collapse, divergent content fails closed),
  so the runbook's documented `shasum -a 256 -c SHA256SUMS` works next to
  downloaded files — and the install wrapper's integrity check of
  `deadreckon-installer.sh` actually engages instead of always warning.

## Sleep-preview test race fix - 2026-06-09

- `prevent_sleep_linux_falls_back_when_systemd_inhibit_missing` read
  `DEADRECKON_SLEEP_INHIBITED`-dependent state without holding the test
  binary's `ENV_LOCK` while a sibling test mutates that variable under it —
  parallel scheduling decided the verdict (green in two CI runs, red in
  rc.6's verify). Both env-sensitive preview tests now hold the lock.

## CI on every push; platform-scoped hygiene baselines - 2026-06-09

- New `ci.yml` runs `cargo fmt --check` and the full workspace suite
  (`--no-fail-fast`, with `expect` installed) on ubuntu for every push and
  pull request — completing the release-trust-closure item so host couplings
  surface in branch feedback instead of release-candidate tags. The release
  verify step also uses `--no-fail-fast` so one red binary cannot hide the
  rest.
- The release-binary size baseline is per-OS (`tests/.size-baseline-macos`,
  `tests/.size-baseline-linux` — Mach-O and ELF sizes are not comparable),
  and the rustfmt-commit archaeology test skips on shallow clones, which
  cannot see repo history. Both failed rc.5's verify gate.

## Host-coupling sweep for the test suite - 2026-06-09

- Swept the suite for works-on-the-author-machine couplings after rc.4's
  verify gate caught three more: every test `git init` now pins
  `--initial-branch=main` (branch-name assertions no longer depend on the
  host's `init.defaultBranch`); the config provider/model shortcut test
  resolves a stub `codex` from a prepended PATH instead of requiring a real
  install; and the interactive prompt tests probe for `expect(1)` — skipping
  with a notice on dev machines, failing loudly when CI lacks it. The release
  verify job installs `expect` so the interactive coverage actually runs on
  the Linux gate.

## Platform-stable characterization goldens - 2026-06-09

- The characterization goldens embedded environment noise: raw temp-path
  *length* decided kv wrap points, path truncation points, and the smoke
  provider's prompt-length-derived token counts, so two goldens generated on
  macOS failed on the Linux release runner (the v0.1.0-rc.3 verify gate).
  Characterization workspaces now live at one fixed canonical path length on
  every platform and the goldens are regenerated to match.
- `DEADRECKON_UPDATE_GOLDENS=1` regenerates the characterization goldens
  instead of asserting, for the next time the pinned surface intentionally
  changes.

## Chain hook EPIPE fix - 2026-06-09

- A chain hook that exits (or closes stdin) without reading its advisory JSON
  payload no longer turns into `apply_refused_json_error__broken_pipe` — the
  payload write tolerates EPIPE and the hook's exit code stays the contract.
  This raced reliably on Linux CI runners (it broke the v0.1.0-rc.2 release
  verify gate) and is now pinned by a deterministic closed-pipe unit test plus
  a stdin-closing hook integration test.

## Workspace suite green; releases gated on it - 2026-06-09

- `cargo test --workspace` passes end to end (53 binaries) and the release
  workflow's verification step now runs the full suite —
  `cargo fmt --check && cargo test --workspace --locked` plus the release
  build — completing the release-trust-closure contract that
  `release_workflow_verification_matches_release_trust_contract` pins. No
  release ships on a suite that never ran.
- Fixed the three standing failures: the chain TTY test strips ANSI (the
  PTY-attached binary colorizes now) and invokes `script(1)` portably (BSD
  positional args on macOS, `-qec` on util-linux); the README first-screen
  coherence window covers the command table that moved below the install
  instructions; and two test fixtures in `prompt.rs`/`main.rs` build their
  ANSI escapes at runtime so `raw_ansi_escapes_stay_in_ui_module` holds.

## Self-healing turn loop - 2026-06-09

- The retryability taxonomy is finally load-bearing: transient provider
  errors (408/429/5xx, transport blips, CLI rate-limit phrasings) get one
  bounded retry with a 2s backoff inside the turn loop. The retry is audited —
  events.jsonl records "turn N hit a transient provider error; retrying once"
  and "retry succeeded; continuing" — so recovery is visible in attach, never
  silent. `ProviderError::Http` carries an explicit `retryable` flag set at
  construction; `is_fatal()` is now its exact complement.
- The router preserves the typed error when exactly one route was attempted,
  so retryability survives instead of being flattened into an opaque
  `NoRoute` string; multi-route fallthrough still aggregates.
- The HTTP client has real timeouts (30s connect / 600s request) — a stalled
  API connection can no longer hang an unattended run forever. HTTP error
  bodies are trimmed on a char boundary (the old byte slice could panic on
  multibyte error text exactly while reporting a failure).
- A provider error that survives the retry now persists `Failed` plus a
  `failure_reason` and emits the run-completed event before surfacing — a
  dead run shows as failed in `list`/`status` immediately instead of
  lingering as a zombie `Executing` until pid liveness is probed.

## Attach TUI help overlay and one abandon key - 2026-06-09

- Every attach surface (run, plan, campaign, chain) gains a `?` help overlay:
  a centered popup with the complete key reference for that surface; any key
  closes it. The footer shows the load-bearing subset and truncates on narrow
  terminals — the overlay is the full reference, one keystroke away, and every
  footer now advertises `? help`.
- One shared help-key rule (`handle_help_key`): `?` opens, any key while open
  closes, everything else flows to the surface's normal handling — a key
  pressed with the overlay open can never fire an action underneath it.
- One abandon key: the CLI completion prompt now advertises `x abandon`
  matching the attach TUI (where `b` is "back"); `b` stays accepted for muscle
  memory but is no longer documented. HOWTO's TUI key table caught up
  (`x abandon`, confirm keystrokes, the `?` overlay).

## Honest spend and a wall cap that binds mid-turn - 2026-06-09

- Subscription run spend now reads as the budget it really is —
  `not metered (subscription) · 23m of cli:claude-code · 7 turns` — instead of
  a raw seconds count; `status` adds a `billing` row ("subscription: cost is
  not metered, time is the budget") and the JSON gains an additive `billing`
  field.
- `--max-wall-seconds` now binds DURING a turn, for every provider kind: the
  provider call is bounded by the remaining wall budget, the in-flight
  subprocess is cancelled (not orphaned) with a bounded grace period, the cut
  turn's elapsed time is recorded honestly in `spend.jsonl`, and the run
  pauses at cap exactly like the spend cap. Previously the cap was checked
  only between turns and only for subscription-billed turns, so an API-billed
  run had no wall cap at all and a single hung turn was uncapped for everyone.
- Direct-API turns that report no wall time now accrue measured elapsed wall
  time, so wall accounting (and the cap) is universal.

## Provider login preflight - 2026-06-09

- Subscription CLI descriptors may declare an `[auth_probe]` (a local status
  subcommand, e.g. `claude auth status` / `codex login status`) with
  logged-in/logged-out markers and `login_try_lines`. Matching strips
  whitespace so JSON pretty-printing differences don't matter, and is
  fail-open: unsupported subcommands, stubs, and unexpected output classify as
  Unknown and behave exactly as binary presence did before.
- `deadreckon doctor` now distinguishes "CLI binary found; logged in" from
  "installed but not logged in (<detail>)", with the provider's own login
  command as the action.
- The shared provider-setup resolver probes login state on the launch path
  (`require_usable_route`) and refuses up front — `try: claude login` —
  instead of failing mid-run with raw subprocess stderr. Previews stay
  presence-only and never spawn the probe.

## Portability: no developer-machine paths in the shipped surface - 2026-06-09

- `DEADRECKON_HOME` now defaults to `~/.deadreckon` derived from the running
  user's home (`default_deadreckon_home()`), and the provider config default
  follows it (`default_config_path()`). The compiled-in `/Users/gdc/...`
  constants (`DEFAULT_DEADRECKON_HOME`, `SOURCE_ROOT`, `DEFAULT_CONFIG_PATH`)
  are gone; installed binaries work on any machine without env setup.
- Source-tree fallbacks (run/doc skills, chain hooks, self-improvement targets,
  learning redaction) resolve through `source_root()` — `$DEADRECKON_SOURCE_ROOT`
  override first, then the compile-time workspace — and degrade cleanly to the
  user tier when no checkout is visible.
- `release/install.sh` defaults to the latest GitHub release instead of a
  pinned RC tag; `DEADRECKON_TAG` still pins. The Makefile derives `ROOT` from
  `CURDIR` and `alias-zsh` edits `$HOME/.zshrc`.
- HOWTO.md is written for any machine (`~/.deadreckon`, `~/.zshrc`,
  `/tmp/try-deadreckon`) instead of the author's.
- New guard test (`tests/portability.rs`) fails the build if a
  developer-machine path reappears anywhere in crates, release scripts, the npm
  wrapper, the Makefile, or user-facing docs. Import goldens normalize the
  workspace as `<SOURCE_ROOT>`.

## Attach TUI Uniformity: narrative panels - 2026-06-08

- The plan narrative panel now windows its fixed-height view and shows a
  `plan narrative first-last/total` scroll indicator (`plan_narrative_title`),
  matching the run narrative panel — an overflowing plan narrative scrolls
  instead of silently clipping. In plan narrative view the shared nav keys drive
  a `NarrativeScrollNav` (clamped to `total - visible_rows`) that scrolls the
  prose rather than moving the task cursor. Closes the one "every list panel"
  gap left by the uniformity slice.

## Attach TUI Uniformity - 2026-06-05

- One shared key dispatcher (`tui::navigation::NavigableSurface` +
  `dispatch_navigation`) drives run, plan, campaign, and chain: arrows/jk,
  Tab/BackTab, PgUp/PgDn, Home/End/g/G behave identically everywhere, with each
  surface supplying only a mode hook. Plan and campaign gained the paging keys
  they lacked.
- One selection cursor (`selection_glyph()` -> `>`), one footer builder
  (`footer(items)`, replacing four divergent styles and deleting the
  parent-plan string-`replace()` hack), and one scroll-position indicator
  (`scroll_indicator()`) on every list panel.
- Apply and Abandon now require a two-step in-TUI confirm; a single mistyped key
  can no longer fire them. Abandon moved off `b` (now unambiguously "back") to
  `x`, and the dead `d`->Docs overload was removed.
- Uniform exit/return: the "press Enter to return" prompts accept
  Enter/q/Esc/Backspace, and Enter on an unloadable child shows an "unavailable"
  notice instead of a silent no-op.
- Friendly empty states (no leaked `*-events.jsonl` filenames), one
  `NARRATIVE_SPLIT_WIDTH` breakpoint shared by run and plan, and an ASCII
  fallback + legend for the chain step glyphs.

## Uniform Surface - 2026-06-05

- Added one `display_width()` (strip ANSI, then Unicode display width) and routed
  the line and card truncation/padding helpers through it so wide (CJK) and
  zero-width glyphs no longer miscount terminal columns.
- Collapsed the two divergent `Tone` enums into one shared enum with a single
  tone->ANSI table and a derived tone->ratatui::Color table, so a status renders
  the same color on a line and in a frame; replaced the silent status fallback
  with an explicit `Status` classifier where an unknown status stays visible
  rather than being dimmed into the background.
- Fixed the column-alignment bug where a colored id cell padded with `{:<N}` was
  short by its ANSI escape length: added a shared `pad_visible` (display-width
  padding) and routed the provider/library id columns through it, aligned the
  provider id/symbol column order across full and summary modes, and measured the
  run-list pad helpers by display width.
- Honored `--no-hints` / `DEADRECKON_HINTS=0` everywhere: fixed the campaign
  completion surface that bypassed the hint toggle, and routed the
  inspection/doc/chain completion surfaces through `completion_hints_enabled`
  so the toggle is respected uniformly.
- Added a shared `kv_block` primitive (auto-sized `key: value` on display width)
  and migrated the status report's run-health, library, and disk sections onto
  it, fixing the misaligned `gate:` and `scope artifacts:` lines so every colon
  lines up under the widest key.
- Added a shared `columns` table primitive (lowercase headers, display-width
  padding so colored cells align like plain ones) and migrated the library table
  onto it; lowercased the run-list header. Provider/plan/chain tables retain
  their display-width-correct renderers and can adopt `columns` incrementally.
- Hardened the selectable prompt menu: multi-digit number entry (menus with 10+
  choices are now reachable by number), `Esc` always cancels even without an
  explicit cancel choice, out-of-range digits show a `choose 1-N` notice, tall
  menus fall back to line mode instead of corrupting the screen, and the footer
  advertises the available keys. Key dispatch is factored into a pure, unit-tested
  `menu_step`.
- Added `prompt::ask_number(range)` that re-prompts on empty, non-numeric, or
  out-of-range input, and routed the campaign and orchestrate child-count prompts
  through it so a typo re-prompts instead of aborting the whole command.
- `deadreckon start` with no goal now prompts for one interactively on a TTY
  (and prints a one-line notice when prompts are suppressed) instead of erroring
  out. Confirm-vs-select modality is standardized incrementally (binary
  decisions use `confirm`, multi-way use `select_one`).
- Added one `wrap_words` engine (display-width-aware) and collapsed the kv-value,
  run-list, and campaign-facts wrappers onto it; gave the chain step glyphs an
  ASCII fallback under `--plain`/non-VT terminals (the Windows weak spot); and
  replaced the bare `println!("cancelled")` run paths with a verdict surface that
  carries a Recommended next step.
- Colorized the verdict surface (doctor, status hints, and every run/finish/
  campaign/chain/import/learning outcome): the verdict label is tone-coded by
  kind, section headers are bold, evidence keys are dimmed with their
  `passed`/`warning`/`failed` status words colored, and Recommended/Secondary
  commands are styled. Dimmed the status report's kv keys. Color is gated on a
  TTY, so `--plain`/`NO_COLOR`/piped output stays byte-identical.
- Swept the per-command raw output (chain, inspection, lifecycle, acceptance,
  plan, providers, campaign, doc, attach): 85 additional colorizations — section
  headings, ids/hashes, runnable commands, status words, and dimmed labels — all
  through the TTY-gated helpers. Fixed-width/padded table columns were left plain
  to preserve alignment (the ANSI-padding bug class).

## Release Trust - 2026-06-02

- Added a lane-aware release policy gate for branch/PR, RC, stable, and invalid
  tags so official RC/stable releases share one publish/signing/provenance
  contract while forks and PRs remain secret-free dry-runs.
- Hardened official releases to fail closed when macOS signing/notarization,
  Homebrew, npm provenance, attestation, manifest, checksum, or Windows signing
  policy requirements are missing.
- Moved macOS signing proof to the packaged cargo-dist artifact: CI now signs,
  verifies, notarizes, and repacks the archive contents before upload.
- Added release trust artifacts: `SHA256SUMS`, `release-manifest.json`,
  `release.spdx.json`, GitHub artifact attestations, Homebrew checksum
  verification, and npm `--provenance` publishing.
- Updated the release runbook with the Apple Developer ID checklist, npm
  trusted-publishing/token fallback, Windows Authenticode signing secrets, and
  artifact verification commands.

## Verdict Surface - 2026-06-02

- Added a shared Verdict Surface contract for terminal outcomes: one verdict,
  one `Recommended` command, one `Explanation`/`Evidence` panel, additive
  `verdict`/`primary_action` JSON, and subordinate secondary actions.
- Normalized run/lifecycle, plan/orchestrate/fork/merge, campaign, chain,
  recovery, setup/diagnostic, import, learning, and doc outcome surfaces through
  the shared contract while preserving command names, quiet/plain/json/no-hints
  behavior, and durable state schemas.
- Demoted competing TUI/help/preflight action hints from primary-looking
  `recommended:` rows to compact `next`/inspection guidance.
- Burned down the FRIENDLINESS-AUDIT one-primary-action failures and added a
  regression test that rejects new in-scope audit failures.

## Seam conformance kit - 2026-05-31

- Added `examples/seams/` with fixture JSON, a sample `[seams]` config, and
  POSIX shell workers for policy allow/deny, catalog override, hooks JSONL, and
  event-sink JSONL.
- Added `deadreckon seams validate <kind> --config <path> [--fixture <path>]
  [--json] [--sandbox <backend>]` so workers can be checked against the same
  sandboxed dispatch primitive used by runtime.
- Documented the seam protocol, fail policies, sandbox expectations,
  `--no-seams`, and the non-swappable gate boundary in `docs/SEAMS.md` and
  AS-BUILT §39.

## Composable seams (production release) - 2026-05-31

- P1: Added the runtime `SeamCommand` primitive, `[seams]` config parser with a
  hard non-swappable gate guard, sandboxed JSON-over-stdio dispatch with fixed
  per-kind fail policies, stdin/denylist support in the sandbox runner, and
  per-run `seams.json` audit writing.
- P2: Wired the policy seam into bash/write_file dispatch after the
  `sandbox.toml` floor, reusing the existing tool-refusal provenance path for
  denials while preserving builtin behavior when no policy seam is configured.
- P3: Added the model-catalog seam path: catalog responses can override route
  context windows and pricing at router construction, while malformed or absent
  catalog seams fall open to the built-in model list.
- P4: Added hook fanout for tool start/result events with fail-safe dispatch;
  hook outputs are observe-only, non-fatal, and covered by proof-subtree sandbox
  denial.
- P5: Added the event-sink seam as an additive `RunEvent` broadcast mirror while
  keeping `events.jsonl` as the source of truth for attach and failure recovery.
- P6: Added deterministic direct-API history compaction with `[compaction]`
  config, `compaction.jsonl` audit records, catalog/seam/fallback context-window
  sources, and full `history.json` retention.
- P7: Added `--no-seams` run/start controls, seam resolution in preview and
  doctor output, and policy-seam refusal footers with recovery commands.
- P8: Added adversarial trust-boundary tests proving seam workers cannot write
  markers/proofs, cannot read `gate/nonce`, and cannot affect gate signatures.
- P9: Added explicit seam config validation tests for unknown kinds, empty
  commands, and bad timeouts, plus an all-seams smoke run that writes
  `seams.json` and validates a gate marker.
- P10: Added resume-sweep coverage for seam re-resolution, deterministic
  compaction replay, and survival of `seams.json`/`compaction.jsonl`.
- P11: Documented composable seams in AS-BUILT §39, updated the shipped/thin
  accounting, and logged V1 seam follow-ups.
- Release summary: one uniform seam contract (sandboxed JSON-over-stdio
  subprocess, fixed per-kind fail policy) makes policy, model-catalog,
  hook-fanout, and event-sink swappable via `[seams]`; unconfigured seams keep
  built-in behavior and `--no-seams` forces all built-ins.
- Release summary: the acceptance gate stays deliberately non-swappable: no seam
  can write or redirect the marker, read `gate/nonce`, or alter the signature;
  seam workers run sandboxed.
- Release summary: deterministic, resume-safe context-window compaction closes
  the direct-API history gap in `compaction.jsonl`; CLI-provider paths are
  untouched.

## Navigable campaign attach (production release) - 2026-05-31

- Added campaign attach state/feed plumbing so `attach <campaign-id>` can refresh
  campaign snapshots, roll-up, aggregate spend, sub-plan spend, and a bounded
  campaign feed from `campaign-events.jsonl` plus each discovered sub-plan's
  `plan-events.jsonl`.
- Added a live ratatui campaign attach surface for TTYs: campaign header,
  selectable sub-plan cards, campaign feed, and footer controls for select,
  drill-in, back, refresh, and detach. Off-TTY/`--plain` keeps the read-only
  summary, while `--json` emits a structured campaign attach object.
- Wired campaign -> sub-plan -> child-run drill-in by suspending the campaign
  frame and reusing the existing plan/run attach loops unchanged, with campaign
  breadcrumbs threaded into plan and run attach views.
- Covered the feature with focused navigable attach tests for campaign event
  tailing, render text, key handling, nested suspend/resume depth, breadcrumbs,
  JSON/plain fallbacks, latest campaign resolution, and campaign tick timing.
- Updated AS-BUILT and V1 deferrals: campaign attach is now navigated production
  behavior; the remaining V1 work is a flattened recursive event tree.
- Rebaselined the release binary-size guard to the verified post-feature binary
  after adding the campaign TUI/feed code path.

## Decompose (maintainability refactor) - 2026-05-30

- P1 (`7ef2d5c`): Added a full-binary CLI characterization net for plan creation,
  quiet plan creation, start full-plan preview JSON, chain status, off-TTY attach,
  and canonical `try:` refusal footers, with normalized goldens under
  `crates/deadreckon/tests/goldens/characterization/`.
- P2 (`a6f8d57`): Added shared integration-test helpers under
  `crates/deadreckon/tests/common/` and migrated duplicated tempdir,
  command-construction, stdout/stderr, and success-assertion helpers without
  changing test assertions.
- P3a (`a601ae3`): Lifted `acceptance_integrity_tests` out of `main.rs` into a
  sibling `src` test module without changing test names or widening runtime
  visibility.
- P3b (`9a9892d`): Lifted `acceptance_render_tests` out of `main.rs` into a
  sibling `src` test module while preserving its four render-focused unit test
  names and private-helper access.
- P3c (`098b1cf`): Lifted `campaign_spawn_tests` out of `main.rs` into a
  sibling `src` test module while preserving its campaign orchestration helper
  coverage and private-helper access.
- P3d (`e15eb86`): Lifted `effortless_consistency_tests` out of `main.rs` into a
  sibling `src` test module while preserving its cross-surface consistency
  assertions and private-helper access.
- P3e (`02d8396`): Lifted `flight_cli_tests` out of `main.rs` into a sibling
  `src` test module while preserving CLI flight/log fixture coverage and
  private-helper access.
- P3f (`8e0f276`): Lifted `self_improve_pr_tests` out of `main.rs` into a
  sibling `src` test module while preserving self-improvement PR adapter
  coverage and private-helper access.
- P3g (`bf64b50`): Lifted `tui_tests` out of `main.rs` into a sibling `src`
  test module while preserving attach, plan, narrative, provider-log, and
  guided-start TUI coverage plus private-helper access.
- P4a (`1768d17`): Created the private `commands/` facade, moved the chain
  command family into `src/commands/chain/`, and routed the `main_inner` chain
  branch through `commands::chain` while keeping shared attach infrastructure in
  the crate root.
- P5a (`58cfbd4`): Moved the acceptance and def-done command family into
  `src/commands/acceptance.rs`, preserving the existing `main_inner` dispatch
  and keeping acceptance render helpers in the crate root for the later TUI
  split.
- P5b (`eb55274`): Moved the supervised `run` command body into
  `src/commands/run.rs`, with `main_inner`, `start`, and `try` now calling the
  private command module while shared preview/render helpers remain in the crate
  root.
- P5c (`d72bc9d`): Moved the `init` command body into `src/commands/init.rs`,
  keeping shared completion, config rendering, and provider-detection helpers in
  the crate root for later cleanup phases.
- P5d (`05e96ba`): Moved the campaign command family into
  `src/commands/campaign.rs`, keeping root/start/orchestrate/attach/show/kill
  call sites routed through the private command module.
- P5e (`d1e66f2`): Moved the attach command dispatch and terminal event loops
  into `src/commands/attach.rs`, leaving pure render/state helpers in the crate
  root for the P6 TUI extraction.
- P5f (`5ec29f0`): Moved the merge command entrypoint and CLI repair-strategy
  parsing into `src/commands/merge.rs`, keeping shared merge/repair helpers in
  the crate root for plan dependency composition and the later plan split.
- P5g (`7416c0b`): Moved the orchestrate front-door and interactive
  mode/provider selection helpers into `src/commands/orchestrate.rs`, keeping
  plan creation, fork, and shared render helpers in the crate root for the
  remaining plan split.
- P5h (`9c2a8cf`): Moved the plan/fork command family and child-launch
  orchestration helpers into `src/commands/plan.rs`, leaving plan result docs
  and shared TUI render helpers in the crate root for later phases.
- P6a (`8d94316`): Created the private `src/tui/` module and moved the
  run-attach TUI state, key handling, post-action notice, and panel layout
  helpers into `src/tui/attach_state.rs`.
- P6b (`e213718`): Moved the pure Markdown-to-ratatui line renderer into
  `src/tui/render.rs`, leaving the run-doc file lookup wrapper in the crate
  root while the TUI render module takes over presentation-only parsing.
- P6c (`f4dc268`): Moved pure run-attach activity, live-file, process, narrative
  item, panel title, and context-count render helpers into `src/tui/render.rs`
  while keeping the terminal draw loop and file/doc lookup wrappers in place.
- P6d (`e2481ae`): Moved the run-attach widget shell, header/footer/status,
  spend/context/acceptance panels, and live-files/process panels into
  `src/tui/render.rs`, leaving provider refresh, narrative projection caching,
  and docs file lookup wrappers in the crate root.
- P6e (`3bc3971`): Moved the plan-attach widget shell, narrative panel, footer,
  task-pane layout, activity feed formatting, and task detail rendering into
  `src/tui/render.rs`, while preserving shared plan summary/event helpers behind
  the private crate facade for existing command output.
- P6f (`5fd1063`): Moved the chain-attach TUI state, renderer, event-read
  hinting, header/footer text, timeline rows, and activity rows into
  `src/tui/render.rs`, leaving the chain command event loop and actions in
  `src/commands/chain/`.
- P6g (`c4fe1e7`): Moved the run narrative/docs widget rendering,
  `RunNarrativeRenderInput`, deterministic run narrative projection helpers,
  markdown-doc line rendering, and narrative line/count helpers into
  `src/tui/render.rs`, leaving provider refresh jobs and attach event loops in
  the command/root orchestration layer.
- P6h (`efda723`): Added the pure-render unit snapshots for run attach and
  chain attach frames, using the extracted `src/tui` render seams to lock the
  terminal frame shape before leaving the TUI extraction phase.
- P7 (`76600a8`): Added merge-conflict-path characterization and extracted the
  shared merge composition loop used by campaign result composition, final plan
  merge working trees, and full-plan dependency source assembly without changing
  conflict semantics or repair behavior.
- P8 (`3390ad8`): Added command-existence characterization for bare PATH lookup
  and explicit-path handling, then unified the start/setup/doctor command
  lookup call sites through one private helper without changing provider
  detection behavior.
- P9 (`242bfa3`): Added cross-crate retryable-I/O characterization, promoted
  `deadreckon_core::error::is_retryable_io_kind` as the single shared helper,
  reused it from providers and sandbox, and recorded the one justified public
  surface rebaseline for that new core path.
- P10 (`8b80969`): Pruned unused `tracing`/`chrono` dependencies, deleted
  confirmed dead helpers, hardened docs regex initialization with BUG-tagged
  `expect` calls plus compile coverage, and applied targeted allocation nits
  while keeping characterization goldens unchanged.
- P11: Documented the post-decompose binary layout in AS-BUILT §38, updated the
  built-vs-thin accounting, and logged the rejected command/API/test reshaping
  work in `docs/V1-CANDIDATES.md`.
- Post-P11a: Moved descriptor-backed `deadreckon import` handling into
  `src/commands/import.rs`, keeping `main_inner` as the only call boundary and
  reducing `main.rs` to roughly 20.2k lines.
- Post-P11b: Moved `deadreckon learn` and `deadreckon improve` command handling
  into `src/commands/learning.rs`, preserving the crate-private self-improvement
  PR adapter seam and reducing `main.rs` to roughly 19.4k lines.
- Post-P11c: Moved the guided `deadreckon start` flow into
  `src/commands/start.rs`, keeping shared launch-preview and command-existence
  helpers at the root and reducing `main.rs` to roughly 16.7k lines.
- Post-P11d: Moved `deadreckon detect`, `deadreckon providers list`, and
  `deadreckon update` handling into `src/commands/providers.rs`, leaving shared
  provider-id helpers at the root and reducing `main.rs` to roughly 16.0k lines.
- Post-P11e: Moved shell completion and `deadreckon doctor` handling into
  `src/commands/completion.rs` and `src/commands/doctor.rs`, leaving shared
  command-existence lookup at the root and reducing `main.rs` to roughly 15.4k
  lines.
- Post-P11f: Moved `deadreckon list`, `deadreckon history grep`, and
  `deadreckon library` handling into `src/commands/inspection.rs`, keeping only
  crate-private plan/library list seams for start/status and reducing `main.rs`
  to roughly 14.3k lines.
- Post-P11g: Moved `deadreckon doc` run/plan dispatch and doc-polish preview
  helpers into `src/commands/doc.rs`, leaving narrative attach provider
  selection at the root and reducing `main.rs` to roughly 14.0k lines.
- Post-P11h: Moved `finish`, `export`/`materialize`, `apply`, `abandon`,
  `cleanup`, `extend`, parent markers, and lifecycle notification firing into
  `src/commands/lifecycle.rs`, leaving status/resume/control helpers at the
  root and reducing `main.rs` to roughly 12.4k lines.
- Post-P11i: Moved attach-loop tick timing and async narrative-refresh job
  plumbing into `src/commands/attach_runtime.rs`, keeping attach/chain event
  loops as callers and reducing `main.rs` to roughly 11.9k lines.

## Effortless (production release) - 2026-05-28

- P1 (`c81b617`): Added the whole-surface friendliness contract table and `docs/FRIENDLINESS-AUDIT.md`, with depth tests proving every canonical top-level verb has one row per six-clause contract item.
- P2 (`bacf76f`): Added `deadreckon try`, a keyless local smoke run that uses the real turn loop and signed `dr-gate` proof, then prints the proof/story/lineage block and one next command.
- P3 (`bbf1e73`): Factored the proof-block renderer and surfaced the signed proof/story/lineage block on completed run exit cards.
- P4 (`e20cf54`): Made `deadreckon start` adopt a single detected subscription CLI inline, keep the provider picker for multiple detected CLIs, and refuse with `deadreckon try`/provider setup recovery when none are available.
- P5 (`663843f`): Added a shared primary-action slot to cards and made exit cards, status, and finish lead with one primary action while demoting secondary lifecycle actions.
- P6 (`85f1d31`): Swept spend and gate verdict rendering so exit cards, status, finish, plan child details, and campaign child summaries show honest subscription spend and per-check gate results.
- P7 (`0bef0f4`): Added opt-in `[notify]` parsing, bounded native/command/webhook channels, redacted notification context, and `notify.jsonl` attempt records.
- P8 (`823945b`): Fired enabled notifications on accepted, paused-at-cap, and failed lifecycle outcomes while disabled configs stay silent.
- P9 (`10dd47b`): Added bounded provider-backed goal-shape recommendations for `start`, preview-scoped classifier records, optional campaign `--n`, and editable campaign preflight controls.
- P10 (`7425883`): Unified the verified-run glossary, changed completed exit cards to the `VERIFIED` verdict, expanded refusal `try:` footer coverage, and added command-notification failure recovery hints.
- P11 (`c37ca2b`): Documented AS-BUILT §37 for the Effortless contract, updated shipped-vs-thin accounting, and logged the palette/localization/template/notifier/classification/onboarding deferrals in V1-CANDIDATES.

## Campaign Orchestration (production release) - 2026-05-28

- P1: Added `deadreckon-core::campaign` module with the nesting `Lineage` record, the `CAMPAIGN_MAX_DEPTH = 2` hard cap, and a `guard` that refuses a campaign at depth >= 1 or a sub-goal that cycles to an ancestor `task_key`/scope.
- P2: Added the file-backed `Campaign`/`SubGoal` model (`campaign.json`) with `build_sub_goals` decomposition validation (exactly-N planner output, non-empty, distinct sub-goals) and `Campaign::new` reusing `validate_task_count` (2..=6).
- P3: Added the sub-orchestrator spawn (`build_sub_orchestrator_command`, lineage env transport + `sub-result.json` sidecar) reusing the plan-child isolation idiom, and wired `orchestrate full-plan` to report its merged result when launched by a campaign.
- P4: Added `run_campaign_fork`, a sequential sub-orchestrator driver that records `campaign-events.jsonl` (`campaign_started`/`sub_launched`/`sub_merged`/`sub_failed`) and marks a failed sub without aborting its siblings.
- P5: Added the tree-budget allocator (`allocate_budget`, even split with remainder-to-first), aggregate-spend exhaustion enforcement that refuses the next sub launch (`tree_budget_exhausted` + `budget_exhausted` event), and the unbounded-budget warning.
- P6: Extracted the shared `mergeable_run_files` enumeration (used by plan merge unchanged) and added `compose_roots`/`compose_result_runs` for independent sub-results; a cross-sub file conflict is reported so the campaign fails rather than silently overwriting.
- P7: Added the gate-verdict roll-up (`CampaignRollup`, `worst_of`, `rollup_permits_completion`, `build_rollup`): any refused or unmerged leaf makes the whole campaign refused (the no-laundering invariant). The roll-up is bound into the result run's marker signature, so editing `campaign-rollup.json` after signing invalidates the marker.
- P8: Added `campaign_can_complete`: a campaign reaches completion only when every sub merged and the roll-up permits it; a refused sub never reaches a clean completed state.
- P9: Added the top-level `deadreckon campaign <goal> --n <2..=6>` verb (peer to run/orchestrate/chain): decomposes via the planner, guards depth/cycles, previews (`--preview`), forks N sub-orchestrators, rolls up verdicts, composes one promoted result run with a `deadreckon-campaign-manifest.json`, and refuses to promote on any refused leaf or cross-sub conflict.
- P10: Surfaced campaigns in `attach <campaign-id>` (sub rows + roll-up + breadcrumb), `show <campaign-id> --why-failed` (refused/caveat subs), and `kill <campaign-id>` (cascades into each sub-plan, then marks the campaign killed) via `resolve_campaign`.
- P11: Documented campaign orchestration in AS-BUILT §36 and logged depth>2, cross-level dependencies/merge-repair, recursive attach, planner-chosen breadth, and richer tree-budget strategies in V1-CANDIDATES.

## Tamper-Evident Gate (production release) - 2026-05-28

- Refuse to sign when a run edits `acceptance.yaml` or a compiled check carries a suppression pattern; downgrade to a surfaced caveat when a run modifies a check-covered test/target file; bind the tamper record into the marker signature.
- Surface per-check verdicts and a tests-modified flag on the exit card, status, and `--why-failed`.
- Render honest subscription spend with `not metered (subscription)` for subscription-only routes and a subscription note for mixed routes.

## Production release posture - 2026-05-28

- Replaced current product docs and generated run-doc front matter that still labeled DeadReckon as alpha with production-release posture language.
- Kept dated alpha changelog entries and old goal briefs as historical records while moving new user-facing wording to compatibility-release terminology.
- Removed live CLI and narrative fallback messages that described current behavior as an alpha slice.

## Plan Doc Consolidation (production release) - 2026-05-28

- Added consolidated orchestration plan docs: `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, `PLAN-CHILDREN.md`, and `PLAN-DOCS-MANIFEST.json`.
- Built a plan-doc input collector that reads child run docs, task summaries, worker specs, merge repair evidence, and final result inventory in task-graph order.
- Added provider-backed plan-doc consolidation with bounded input bundles, citation validation, invented-path checks, and deterministic fallback when provider output is unavailable or invalid.
- Materialized plan docs into merged libraries, plan apply worktrees, and exported artifacts without copying child internal logs.
- Rewrote synthetic plan-result apply `RUN-*` docs as wrappers that point to consolidated `PLAN-*` docs instead of showing empty zero-turn run docs.
- Extended `deadreckon doc`/`docs` and `show` so plan ids and plan-result wrapper runs resolve to plan documentation.

## Production command model (alpha) - 2026-05-27

- Reframed default help around the production flow: `start`, `attach`, `status`, `list`, `finish`, setup, and control commands.
- Kept power-user and advanced verbs callable and discoverable through `deadreckon help-all`, per-command help, and completions without crowding the first screen.
- Made `deadreckon start` history-aware for repos with completed promoted runs: TTY users can choose a follow-up, while preview/JSON output shows exact extend, review, and full-plan commands.
- Added done-criteria transparency to interactive `start` when project criteria already exist, with keep/view/check/update/cancel choices before launch.
- Updated README, HOWTO, AS-BUILT, the user-facing matrix, and focused tests without adding runtime schema or durable config.

## Start picker (alpha) - 2026-05-27

- Added selection-first TTY prompts to `deadreckon start` for launch mode, detected/configured provider routes, missing done-criteria action, non-git and dirty source handling, and final launch confirmation.
- Kept scripted surfaces deterministic: non-TTY, `--json`, `--plain`, `--quiet`, and `--yes` never enter the picker and continue to emit structured output or `try:` recovery lines.
- Let interactive users choose a detected CLI provider ephemerally for a launch without writing provider config.
- Routed selected provider routes into existing run/review/full-plan dispatch, with previews showing alpha role reuse for review and full-plan orchestration.
- Documented the picker behavior and remaining V1 deferrals without adding durable launch profiles or runtime state schemas.

## Guided first use (alpha) - 2026-05-26

- Reframed README/HOWTO first-run examples around provider-neutral `deadreckon start`, while keeping direct `run` and `orchestrate` paths documented for power users.
- Added a `start lifecycle` footer after successful guided launches so the new front door ends with exact attach, status, kill, and finish commands for the created run or plan.
- Locked `deadreckon start` JSON, plain, and quiet output behavior with focused coverage for structured recovery, ANSI-free previews, and quiet successful launches.
- Connected confirmed `deadreckon start` launches to the existing run and orchestrate handlers while keeping start previews state-free.
- Added source-mode recovery to `deadreckon start`, including `--fresh`, `--worktree`, `--from`, and `--allow-dirty` parsing plus non-git and dirty-worktree recovery that points to valid guided commands.
- Wired `deadreckon start` into shared provider setup and done-criteria recovery so missing providers, detected-but-unconfigured CLIs, and absent done criteria end with concrete `try:` lines instead of the placeholder launcher error.
- Shared launch preview rows for start, run, and orchestrate so previews name path, provider, done criteria, workspace, watch, stop, and finish actions.
- Added deterministic `start --mode auto` launch-decision heuristics for run, review, and full-plan paths.
- Added the visible `deadreckon start` parser and help surface for the guided front door.
- Clarified DeadReckon's audience as the harness around agent CLIs for unattended, sandboxed, auditable work, and pointed first-use help/docs at `deadreckon start`.
- Documented the guided first-use architecture and V1 deferrals in AS-BUILT and V1 candidates without adding durable launch state.

## TUI Responsiveness (alpha) - 2026-05-26

- Added in-memory attach tick budgets and loop-stage timing for run, plan, and chain attach surfaces, with provider narrative refresh classified as background work for the responsive attach scheduler.
- Moved run narrative attach refresh onto a coalesced background job so manual `r` redraws without awaiting the provider and detach cancels in-flight provider work.
- Routed run attach event and quiet-threshold narrative refreshes through the same background job, preserving failure notices until a later refresh replaces them.
- Moved plan narrative attach refresh onto a plan-keyed background job so manual, event, and quiet-threshold refreshes coalesce while child drill-in and detach cancel in-flight provider work.
- Replaced run attach live-file collection with an attach-specific inventory walker that prunes heavy cache/profile directories before descent and caps displayed files without losing total counts.
- Added attach-owned JSONL tail caches for run spend, trace, and flight activity streams so live run attach parses appended complete rows instead of rereading whole files each tick.
- Added live attach provider-log scan throttling so current flight rows delay fallback root scans, fallback matches are cached by freshness, and matched provider logs invalidate on mtime changes.
- Added run and plan narrative projection caches for attach rendering so redraws reuse covered projections, preserve stale provider snapshots, and avoid appending narrative snapshots from render paths.
- Added incremental chain activity tailing for chain attach, including partial-line tolerance and status hints when event reads fall behind.
- Added focused responsiveness smokes for slow narrative refreshes, large worktrees, and max-size chain timelines without invoking full release or stress suites.
- Documented the TUI responsiveness alpha contract and known limits: no attach daemon, no shared cross-surface broadcaster, and no diagnostic dashboard yet.

## Narrative Attach (alpha) - 2026-05-26

- Added `deadreckon attach --view narrative` for cited run and plan overviews, with `n` to return to raw activity and `v` to cycle architecture, agents, files, evidence, and no-visual modes.
- Added the `Narrated` operator heading for narrative attach projections so the calmer view has a clear product label.
- Defaulted provider-backed narrative refresh to local Claude Code on `sonnet`, while keeping `--narrative-provider` as an explicit route override.
- Added `--no-narrative-provider` for deterministic-only narrative attach when provider refresh is not desired.
- Added file-backed run/plan narrative projections under `narrative/state.json`, `narrative/snapshots.jsonl`, and `narrative/architecture-graph.json` without changing `PipelineState`.
- Added evidence-backed ASCII map rendering for run architecture, plan agents, touched files, and evidence chains, including plain/JSON attach output.
- Added redacted provider refresh on manual `r`: attach builds bounded prompts, validates structured claims and graph labels against known evidence, enforces budget/cadence guards, and falls back to deterministic stale facts when refresh is unavailable or rejected.
- Added TTY narrative-view refresh triggers for meaningful run and plan evidence, including errors, completions, tool milestones, docs checkpoints, child-run discovery, task terminal states, and merge-repair milestones.
- Added quiet-threshold TTY refresh attempts for running runs/plans when no meaningful narrative event arrives for the configured quiet window.
- Added narrative refresh triggers for acceptance running/pass/fail transitions so test evidence can update the operator summary without requiring raw-log watching.
- Added plan narrative roll-up from child run narrative snapshots so plan agent rows can cite the child's latest operator summary before falling back to child run state.
- Added plan file-map roll-up from child narrative graphs so plan-level visuals can show cross-agent touched file evidence.
- Kept plan narrative footer controls visible even when the selected child run is not available yet, preserving the one-key path back to raw activity.
- Added focused run/plan TUI render coverage for narrative panes, citations, agent rows, and visual-map hints.
- Added focused plain/JSON narrative attach coverage, including deterministic non-TTY fallback behavior and the explicit chain unsupported response.
- Added acceptance proof/progress citations to run narrative projections so failed done criteria point at the immutable acceptance artifact instead of only generic run state.
- Added focused run TUI mode coverage for narrative/activity toggling, visual cycling, narrow-terminal footers, and completed-run docs staying separate from narrative attach.
- Added command-level narrative attach smokes for flight-backed run output, file/evidence visuals, plan child refs, two-child plan agent visuals, and completed-run docs separation.
- Added final narrative attach guards for stale provider-refresh fallbacks, attach help copy, provider-neutral examples, and visual-map privacy/no-color documentation.
- Added focused coverage for narrative schemas, malformed snapshot tolerance, redaction, claim validation, graph validation, provider refresh validation, cadence/budget decisions, deterministic fallback, and plain map rendering.

## Self-Improvement Loop (alpha) - 2026-05-26

- Added file-backed learning state under `DEADRECKON_HOME/learning/` for episodes, deterministic signals, provider-backed insights, proposals, redacted bundles, candidates, evals, PR dry-run/open events, and local policy.
- Added `deadreckon learn index`, `deadreckon learn report`, required-reflection `deadreckon learn propose`, and redacted `learn export`/`learn import-bundle` so proposal creation uses a provider only after deterministic redacted evidence exists.
- Added `deadreckon improve self <proposal-id|goal-file>` preview, isolated-worktree candidate execution, evidence scoring, high-risk path classification, PR dry-run body generation, diff redaction checks, and a fake-testable live PR adapter gated behind explicit `--open-pr`.
- Added focused core and CLI coverage for learning paths, schema versioning, episode idempotency, bundle redaction/hash checks, signal extraction, proposal reflection validation, PR risk gating, learning CLI output, public-surface stability, PR dry-run, fake PR adapter behavior, and self-improve preview.

## Provider flight recorder and checkpoint rewind (alpha) - 2026-05-25

- Added durable `flight-manifest.json`, `flight-events.jsonl`, `checkpoints/<id>/`, and `rewind-events.jsonl` files for CLI-backed provider runs, with normalized provider-native events and delta checkpoints.
- Wrapped CLI provider execution in a polling flight recorder sidecar that ingests descriptor logs, watches working-tree changes, captures tool/quiet/exit checkpoints, and marks rerun sessions as superseded.
- Added `deadreckon show <run-id> --flight`, `deadreckon show <run-id> --file <path>`, and preview-first `deadreckon rewind` target resolution with hash-guarded `--apply`.
- Routed attach/TUI provider activity through flight events while keeping descriptor provider-log lines as the live fallback during long CLI subprocesses.

## Provider and done-criteria setup unification (alpha) - 2026-05-24

- Added a shared runtime setup resolver for provider roles and done-criteria sources so `init`, `config provider`, `run`, `extend`, `resume`, `orchestrate`, and doc polish use the same source labels, unknown-provider refusals, credential/install hints, and preview vocabulary.
- Switched run/orchestrate previews from `gate` to user-facing `done criteria` rows while preserving `.deadreckon/acceptance.yaml` as the technical file name and signed `dr-gate` as the enforcement mechanism.
- Updated `--acceptance` help text to describe done-criteria files, kept hidden `acceptance` compatibility wording advanced, and added focused coverage for unknown provider refusal plus run/orchestrate done-criteria preview parity.

## Descriptor import hardening (alpha) - 2026-05-20

- Reworked `deadreckon import` around descriptor-backed provider transcript discovery, concrete session selection, import manifests, and normalized trace/provenance events while preserving Cursor SQLite import.

## Implementation notes (alpha) - 2026-05-18

- Added root `implementation-notes.html` seeding for new runs, with required Design decisions, Deviations, Tradeoffs, and Open questions sections.
- Updated the default run prompt and CLI sub-agent prompt to frame work as `Implement the SPEC` and require the live notes file to stay current while files change.
- Made `RUN-DECISIONS.md` the converged implementation decision ledger by rendering the same four notes sections plus a separate evidence-filtered multi-alternative decision details section.
- Added done-time freshness checks so JSON-action providers and CLI sub-agents cannot complete after documentable implementation changes until `implementation-notes.html` is current.
- Updated `narrator-decisions` and split polish merging so implementation notes can feed the four interpretation sections without turning every note into a multi-alternative decision.
- Pointed lifecycle/doc hints toward `deadreckon doc <run-id> --kind decisions` as the primary inspection path for implementation interpretation.

## Orchestration live UX (alpha) - 2026-05-18

- Added shared orchestration role and dependency summaries across plan creation, orchestrate preflight/start, fork completion, plan attach summaries, and merge completion.
- Added provider role tables with route/model/source/notes rows for planner, default child, child overrides, coder, reviewer, and merge repair roles.
- Added explicit parallelism/dependency summaries that show which children start now, which wait, and which tasks unblock downstream work.
- Replaced terse merge repair plan summaries with structured repair detail covering mode, attempts, provider, conflict paths, sidecar paths, repair run status, latest repair event, and next action.
- Moved plan attach onto a `PlanEventBus` feed that replays `plan-events.jsonl`, tolerates partial/malformed event rows, emits plan snapshots, and multiplexes child and repair run events into the plan activity stream.
- Standardized plan attach footer grammar around detach, focus, child-run entry, back navigation, and `try:` lines.

## Coherence closure (alpha) - 2026-05-17

- Aligned top-level `attach` and `kill` id handling so run, chain, and plan ids all resolve through the normal lifecycle commands, with shared `attaching to <kind> <prefix>` and `killed <kind> <prefix>` banner wording.
- Clarified help for `attach`, `kill`, and `show` to name run, chain, plan, and `plan-id:task-id` support where the commands already accept those ids.
- Aligned provider setup wording so `doctor`, `detect`, and `providers list` use the same `kind=cli|http|local-http|scripted|custom` tokens and normal help says provider route instead of descriptor.
- Added coherence coverage for the updated help, orchestration help vocabulary, top-level chain attach/kill dispatch, provider kind vocabulary, status key/value layout, shared stderr error rendering, raw ANSI ownership, visual identity helpers, and plan-child show help.
- Refreshed README/HOWTO examples to use canonical `run`, `--branch-name`, `--overwrite`, `--max-spend`, `--git-strategy`, `--all-scopes`, and `--escalate` wording.
- Added `docs/PLAN-NARRATIVE.md` for merged plans so orchestration has one plan-level reading path built from child summaries.
- Rendered top help and `help-all` from one command catalog, with tests that catch duplicate rows and catalog entries that drift away from clap commands.
- Clarified the `help-all` discovery policy so documented advanced commands are distinct from compatibility aliases kept inline on canonical rows.
- Standardized `--plain` help across run, orchestration, lifecycle, and inspection commands as "without TUI, spinner, or ANSI affordances."
- Standardized cross-project flag help on "all project scopes" while keeping provider `--all` scoped to provider inventory.
- Renamed visible update override help from `--force` to `--anyway`, keeping `--force` as a hidden alpha alias.
- Aligned branch target wording so `run` names worktree branches with `--branch-name`, `apply`/`finish` target branches with `--into`, and apply output says changes landed `into` the target branch.
- Scoped strategy vocabulary so `merge --strategy` means plan composition, `apply`/`finish --git-strategy` means git apply behavior, and chain help separates `--apply-mode` from per-step `--apply-strategy`.
- Added a `help-all` output/scripting policy and aligned help for `--yes`, `--no-confirm`, `--quiet`, `--plain`, `--json`, and `--no-hints`.
- Added a provider-role glossary to `help-all` and aligned orchestration/doc help around provider routes for planner, child, coder, reviewer, repair, and documentation roles.
- Clarified cleanup help so it names temporary run worktrees/branches as its target and explicitly excludes plan state, promoted library artifacts, and exported directories.
- Made plan merge/result output keep the plan primary, with result run and artifact library labeled as secondary implementation details.
- Moved the CLI style facade into `ui.rs` and added coherence coverage so status tone mapping and public style helpers have one source of truth.
- Added standard JSON envelope fields across representative machine-readable surfaces and exposed `plan --json` for scriptable plan creation.
- Split note, warning, paused, and failed/killed style intents, and routed extended-run terminal outcomes through status tones.
- Rendered run, extend, and resume start summaries through the shared key/value block instead of bespoke provider/docs/state lines.
- Added a `help-all` spend-cap glossary for run, per-child, aggregate chain, and doc polish caps.
- Closed the user-facing matrix as an alpha record and moved larger output-layout, orchestration, provider/done-criteria, and snapshot work to V1 candidates.
- Made integration-test temp roots worktree-relative so coherence verification can run from a detached worktree.

## Semantic merge repair (alpha) - 2026-05-16

- Changed orchestration merge to default to DAG-aware composition, so descendant child artifacts can supersede ancestor file edits without a manual `prefer-child` retry.
- Added automatic bounded merge repair for true parallel conflicts: `merge` writes conflict/request/plan/run sidecars under `merge-proofs/`, invokes a repair provider by default, and can prefer a child file, synthesize conflict paths, or run a normal repair child from `merge-working`.
- Added repair controls for advanced/debug flows: `--no-repair`, `--repair-provider`, `--repair-mode auto|prefer|synthesize|child`, `--repair-attempts`, and `--strategy fail-on-conflict|dag-aware|prefer-child`.
- Added plan events for repair planning, repair start, repair child discovery, repaired merges, and repair failure; `show --why-failed`, plain plan summaries, and `history grep --plan` surface the new repair evidence.
- Updated `orchestrate` started/preflight output to say merge repair is automatic and to carry repair through the one-command flow, with `orchestrate ... --no-repair` kept as a debug-only raw conflict path.
- Added orchestration integration coverage for conflict bundles, repair requests, DAG merge precedence, planner prefer/synthesize/child repair, refusal validation, and headless `orchestrate --yes` auto-repair.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with the semantic merge repair model and sidecar layout.

## Plan observability (alpha) - 2026-05-15

- Added `plan-events.jsonl` as the orchestration-level event timeline for plan, task, child discovery, merge, failure, completion, and kill lifecycle edges.
- Added plan-event surfacing to `attach <plan-id>`, plain plan summaries, `history grep --plan`, and `show <plan-id> --why-failed`.
- Added plan attach drill-down/back context so a user can enter a selected child run's normal attach view and return to the parent plan/task.
- Hardened plan kill bookkeeping so discovered child run ids are preserved even if a child reaches a terminal state before the kill sweep inspects it.
- Hardened plan attach and kill recovery for partial `plan-events.jsonl` lines, missing child run roots, explicit `b`/Backspace back navigation, terminal failed-plan events, and sidecar-recovered child run ids.
- Hardened full-plan planning so build goals ask for implementation/verification child slices instead of research-only packets, and multiplayer/live/networked goals preview network capability correctly.
- Improved interactive `orchestrate` setup with goal-based mode and child-count recommendations, configured-provider guidance, optional child provider overrides, preflight warnings for research-only build plans, and a run-like started banner with attach/show/plan paths.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with `§32 Plan Observability` and amended `§22`/`§30` to reflect the file-backed plan event stream and remaining broadcast-bus limit.

## Distribution & self-update (alpha) - 2026-05-15

- Added install receipt and update-check cache files under `~/.deadreckon/` with channel detection for npm, Homebrew, shell, cargo, and source installs.
- Added `deadreckon update --check` plus npm/Homebrew/cargo/source channel routing; source installs refuse with a `try: cargo install --path crates/deadreckon` hint.
- Added shell-channel update backups and in-place swap plumbing through axoupdater, with deterministic backup/failure tests and pruning to the latest three backups.
- Added the cached startup stale-version hint, disabled for non-TTYs, source installs, `doctor`, `update`, and `DEADRECKON_UPDATE_CHECK=0`.
- Added cargo-dist release scaffolding for five OS/arch targets, shell/PowerShell installers, glibc 2.28 Linux metadata, and a push-time `dist plan` workflow check.
- Added guarded macOS Developer ID codesign/notarization steps and public release setup docs for the required Apple secrets.
- Added the npm wrapper package, five per-platform optional dependency templates, no-network receipt postinstall, and npm publish workflow wiring.
- Added Homebrew tap publishing for `gdc/homebrew-tap`, including a formula patch that writes the brew install receipt.
- Added first-run update receipt persistence plus shell-update previews, non-TTY `--yes` enforcement, and post-update doctor hints.
- Updated the as-built architecture docs with the distribution/self-update model and remaining operator release steps.

## Overnight UX (alpha) - 2026-05-14

- Added a shared `ui_card` renderer for run preview, run exit summaries, and completed attach footers with `--plain` / `NO_COLOR` behavior.
- Kept read-only inspection surfaces (`list`, `show`, and `status`) as quieter table/report output so they do not repeat the same run metadata inside card wrappers.
- Added `run --prevent-sleep <auto|on|off>` with macOS `caffeinate`, Linux `systemd-inhibit` re-exec/ready-file handling, run-local sleep metadata, and doctor sleep checks.
- Hardened production git invocations behind `deadreckon-core::git` with `GIT_TERMINAL_PROMPT=0` and commit-family GPG signing disabled.
- Added `spend_summary` replay so subscription or estimated turns render approximate spend with `~` without changing the numeric total.

## Orchestration prompt polish (alpha) - 2026-05-14

- Mined Claude Code's coordinator guidance into deadreckon worker specs: self-contained briefs, no sibling transcript peeking, concrete dependency summaries, correction vs fresh-review guidance, and skeptical reviewer posture.
- Planner prompts now ask for execution-order child DAGs with enough context for each child to run without the parent conversation.
- Plan children now run with `--no-docs`; plan-level summaries remain responsible for orchestration docs, avoiding accidental provider-backed narrator work in child runs.
- The coordinator now records each child run id under `plans/<plan-id>/launch/<task-id>/run-id`, so plan kill can map live child PIDs back to run state before marking children killed.
- Added/kept exact orchestration depth coverage for review-mode extension, child PID snapshots, kill cascade, prompt hygiene, and plan lifecycle friendliness.

## Coherence pass (alpha) - 2026-05-14

- Added one glossary for status words; `running` replaces `executing` in user-visible run and phase surfaces while stored enum variants stay unchanged.
- Added one style module and prompt builder; raw ANSI escapes now live in `ui.rs`, and every confirmation prompt uses the same `? question [Y/n]: ` or `? question [y/N]: ` shape.
- Added one key/value block for run and plan summaries, with lowercase keys and aligned colons.
- Standardized alpha flag names with hidden aliases: `--escalate`, `--overwrite`, `--anyway`, `--all-scopes`, `--global`, `--branch-name`, `--into`, `--max-spend`, and `--git-strategy`.
- Preserved the cyan `deadreckoning` banner, course strip, magenta IDs, spend gauge colors, and chain glyphs, with applied steps now using `◉`.
- Aligned attach and kill banners across runs, chains, and plans.
- Aligned `show --why-failed` and `chain show --why-failed` through one failure-summary layout, and added JSON output for list/status/show/doctor/provider/library inspection surfaces.
- Made `export` the visible copy-out word in help, completion prompts, docs, and refusal text while keeping `materialize` as an alpha compatibility alias/internal marker.

## Orchestration milestone (alpha) — 2026-05-13

- Renamed the multi-child orchestration mode from `split` to `full-plan`, added `deadreckon orchestrate review` and `deadreckon orchestrate full-plan` mode subcommands, and require `--yes` after the preflight in headless execution.
- Added file-backed orchestration plans with task DAG validation, provider roles, worker specs, coordinator messages, child summaries, and plan child markers without changing `PipelineState`.
- Added `deadreckon plan`, `fork`, `merge`, and review-mode `orchestrate` so a common coder -> reviewer -> merge flow can complete end to end.
- Added explicit planner/default-child/per-child/coder/reviewer provider resolution and persisted overrides into `plan.json`.
- Added merge conflict detection with `--strategy prefer-child --prefer-child <idx>` and promoted merge artifacts with `deadreckon-plan-manifest.json`.
- Added plan-aware `attach`, `show`, and `kill` so plan IDs participate in the normal lifecycle, including a basic multi-pane plan TUI with child drill-in.
- Added `deadreckon history grep <pattern>` for plan-aware trace/provenance search and `deadreckon show <id> --why-failed` for run or plan failure summaries.
- Review-mode orchestration now launches the reviewer lane as an `extend` of the coder run, preserving parent context and `extended_from_parent` trace lineage.
- Independent full-plan children now start as ready batches, with coordinator PID snapshots for every live child in the batch.
- Plan attach now surfaces child turn/status, spend or token accounting, latest trace activity, acceptance/gate state, capability preview, and final merged gate status in both the TUI and non-TTY summary.
- Headless orchestration flags now apply consistently: `run --plain --quiet` is accepted, `run --quiet` emits no success stdout, `attach --plain` bypasses the TUI, and `plan`/`fork`/`merge` preserve plain output.
- Added provider-backed planning depth coverage: planner prompts are asserted read-only, `--n` outside `2..=6` refuses before saving, one-task provider decompositions are rejected, and explicit planner/default-child/per-child providers are persisted.
- Coordinator launches now refresh each child worker spec with completed dependency summaries, so dependent child prompts include concrete predecessor context instead of only a plan-time dependency id.
- Merge manifests now include an explicit task graph, child summary paths, provider roles, and coordinator message counts for audit without replaying child transcripts.
- Added `show --why-failed` depth coverage for completed runs, failed run RCA traces, and plan blocker messages.
- Added P10 friendliness coverage for `try:` footers, quiet/plain headless output, review-mode provider hints, and plan ready/blocked task counts.
- Verified with focused orchestration tests plus core plan round-trips, clippy on the orchestration target, and `cargo fmt --check`; a broadcast-backed plan event stream remains a future slice.

## Copilot and Pi providers (alpha) - 2026-05-13

- Added built-in descriptor-backed `cli:copilot` and `cli:pi` providers with subscription auth, detection/install hints, model flags, sandbox read/write roots, and generic CLI routing coverage.
- Added Copilot session-state and Pi session JSONL TUI ingest, including cwd matching, tool/result/thinking rows, and context token telemetry without rewriting provider-owned logs.
- Kept verification focused on provider registry, CLI routing, detect/list UX, provider JSONL parsing, fmt, and crate-local clippy; the long full-suite commands remain out of this goal's default loop.

## Provider CLI ingest (alpha) — 2026-05-13

- Added optional descriptor `[ingest]` metadata and backfilled Codex/Claude Code so TUI provider activity is resolved by registry descriptors instead of provider-id conditionals.
- Added canonical tool-category normalization and schema-keyed provider activity parsers for Codex, Claude Code, Gemini JSON/JSONL, and OpenCode file-mode logs.
- Added descriptor-backed generic CLI launch through `exec_template`, including model flags, prompt delimiters, sandbox placeholders, descriptor sandbox writes, and subscription wall-time spend.
- Added built-in `cli:gemini` and `cli:opencode` descriptors with detection/install hints, `providers list` coverage, registry-order `init --no-confirm`, and stable `cli:` output filenames.
- Kept verification focused on provider/CLI/TUI surfaces; `make verify`, release builds, smoke, stress, and full-workspace tests remain out of this goal's default loop.

## Provider registry (alpha) — 2026-05-13

- P1: Added descriptor TOML, `ProviderDescriptor`, `ProviderRegistry`, override loading from `providers.d`, and shell-like custom command parsing; existing built-in providers now have compiled-in descriptors.
- P2: Existing provider defaults now come from descriptor TOML, `ProviderKind` supports generic descriptor IDs, and CLI sandbox write allowlists are descriptor-backed while preserving current adapter behavior.
- P3: Added descriptor-backed provider probes and `deadreckon detect [<id>]`, including PATH/version checks, credential checks, JSON output, and install `try:` hints.
- P4: Added `deadreckon providers list` with configured-only/default and `--all`, `--models`, and `--full` views backed by the registry.

## Workspace hygiene (alpha) — 2026-05-12

- P1: Captured smoke and public-surface baselines, added invariant tests, and made `make smoke` run fresh/non-interactive for deterministic verification.
- P2: Added warn-only `[workspace.lints]`, `clippy.toml`, per-crate lint inheritance, and a clippy warning snapshot for the P3 cleanup pass.
- P3: Promoted core workspace clippy rules to deny, removed the temporary warning snapshot, and added deny-level lint tests plus a `-D warnings` clippy guard.
- P4: Added `rustfmt.toml` and guard tests for the dedicated format commit and clean `cargo fmt --check`.
- P5: Tuned release/dev profiles and captured a release binary size baseline with slack guard.
- P6: Routed internal crates through `[workspace.dependencies]` and guarded the internal cargo metadata DAG.
- P7: Added library-crate print refusal while keeping the binary crate exempt.
- P8: Added registry-shape guard tests for `deadreckon-core`'s library root; no public surface changed.
- P9: Regrouped provider/runtime/sandbox library roots into registry shape and preserved the public re-export set.
- P10: Added exhaustive retryable/fatal taxonomy methods to core, provider, and sandbox errors while keeping runtime errors on the core taxonomy.
- P11: Updated `docs/AS-BUILT-ARCHITECTURE.md` with §29 Workspace Hygiene and amended §22 to mark the hygiene rider as structural, not a prior thin-item closure.

## Doc depth (alpha) — 2026-05-12

- Per-turn capture extended: full provider response (50 KB cap), per-file diff samples with largest-hunk excerpts, and bash stdout/stderr (10 KB cap each).
- Turn-end documentation is now an explicit run event for both CLI sub-agent turns and JSON-action provider turns; `_incremental.jsonl` is checkpointed before completion polish/acceptance/promotion.
- Templated narrative no longer truncates the title at 40 chars; per-turn outcomes no longer cut at 200 chars; phase prose synthesizes per-turn summaries instead of "deadreckon progressed through turn N".
- Component-table inference uses path rules (`crates/`, `skills/`, `docs/`, manifests, tests, routes, migrations, CI); generic "Project files" rows are not emitted.
- Process topology ASCII is generated only when at least three top-level directories changed.
- Provider-backed doc polish now defaults to four repo skills: `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions`, each with a 16K output budget and per-subcall status/cost recorded in `polish.json` schema v2.
- `deadreckon run` and `deadreckon doc --polish` expose doc-provider selection (`--doc-provider`) with flag/config/subscription/run-provider resolution, preview output, preflight `--budget-cap` refusal, and post-polish subcall summaries.

## Lifecycle help polish — 2026-05-12

- Added `deadreckon finish` / `done` as a completion intent command that routes completed worktree runs to `apply`, fresh/copy runs to `export`, and in-place runs to review guidance.
- Added lifecycle-oriented `--help` text to every top-level verb, including real `chain` subcommand examples and focused `deadreckon chain help <topic>` output.
- Expanded friendly aliases across the lifecycle: `setup`, `settings`, `check`, `runs`, `artifacts`, `keep`, `clean`, `follow-up`, `docs`, `watch`, `stop`, `continue`, `restore`, and `inspect`.

## Autonomous chaining (alpha) — 2026-05-11

- Added the chain data model foundation: `chain.json`, `chain-events.jsonl`, chain path helpers, chain lock task-key convention, and `RunPromoted` events after promotion.
- Added the first user-facing chain flow: `chain "..."`, `--from-file`, `--from-stdin`, `--draft`, preview/confirm, `chain run`, `chain list/status/show/attach`, and a foreground conductor that runs sequential steps through existing run/apply paths.
- Added provider-backed `chain plan` / `chain expand`, including JSON-array validation, duplicate/single-step refusal, and planner spend recording under the chain directory.
- Added chain policy depth: branch-policy stack/base behavior, aggregate per-step spend allocation, and chain hooks for `pre-step`, `post-step`, `on-promote`, and `on-chain-end` with hook events.
- Added chain-step context markers to inner runs and surfaced them in single-run `show` / non-TTY attach summaries.
- Added lifecycle depth for `latest`/`last`, `resume`, `extend`, `redo`, `undo`, pause refusals, and cascade `chain kill` that terminates the live inner run and conductor.
- Added the multi-step `chain attach` TUI with policy header, step timeline, chain activity stream, pause/kill/redo/extend controls, and single-run `attach` chain drill-out via `c`.
- Added policy gate coverage for allowlist refusal, manual apply pause, merge branch policy, on-fail stop/skip, and configurable circuit breaker thresholds.
- Completed the rider depth-test matrix under exact test names and tightened resume-after-manual-pause, quiet auto-apply, bounded undo, TTY auto-attach, preview diff, and aggregate wall-clock behavior.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with §28 Chains and refreshed §17/§22 chain accounting.

## Hardening v2 (alpha) — 2026-05-11

- Added `docs/AUDIT-2026-05-11.md` mapping the original 25 unmet needs to current evidence and the P2-P10 closure plan.
- Replaced TUI polling-only attach with event-backed attach: same-process broadcast plus cross-process `events.jsonl` replay.
- Hardened cross-process cancellation with durable cancel markers, provider abort coverage, and kill-storm tests.
- Hardened partial-trace resume and `resume --from-turn` so trace, spend, and snapshot tails are truncated together.
- Added durable `sandbox.toml` per run, per-tool sandbox policy, and refusal provenance for disallowed filesystem/network actions.
- Expanded `acceptance.yaml` support with required/optional checks, file/content/build/shell checks, and signed per-check proof results.
- Made `doctor` more actionable across providers, sandboxes, OS, permissions, disk, and opt-in provider pings.
- Added `deadreckon library list|search|show` for promoted artifacts, including goal/date filters and promoted-doc grep.
- Hardened Claude Code/Codex/Cursor import normalization with source metadata, deterministic imported run IDs, stable Cursor ordering, malformed JSONL errors, and committed show-output golden tests.
- Polished CLI help/status/completion UX, including command groups, run health/library/disk status blocks, and `DEADRECKON_HINTS=0`.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` and `docs/AUDIT-2026-05-11.md` with the Hardening v2 closure evidence.

## UX consolidation — 2026-05-11

- Added an in-TUI Markdown docs view for completed runs. Press `d` in `attach` to toggle a styled `RUN-NARRATIVE.md` rendering instead of dropping to plain terminal output.
- Made `deadreckon apply` idempotent when a run branch has already landed on the target branch; it now reports `already applied` and can still perform `--cleanup` instead of failing on an empty commit.
- Added explicit provider/model affordances: `run --model`, `extend --model`, model-aware run previews, and `deadreckon config provider|model` shortcuts.
- Made `deadreckon list` project-scoped by default, with `--all` for global history and `--full` for script-friendly full values.
- Added `latest` / `last` run-id aliases for user-facing run commands, resolved to the latest run in the current project.
- Added `deadreckon status` with `next` as an alias; running `deadreckon` with no subcommand now shows the current project's latest run and next action.
- Added `deadreckon cleanup` with `prune` as an alias for cleaned, stale, or completed worktree cleanup.
- Added friendlier command aliases: `export` for `materialize` and `discard` for `abandon`.
- Improved root and subcommand help text, terminal output formatting, TUI layout, completion action footer, and scoped workflow hints.

## Apply/list usability — 2026-05-11

- Made run-id arguments accept unique prefixes so compact `deadreckon list` IDs can be reused directly.
- Made `deadreckon list` compact by default with `--full` for scripts and exact full values.
- Added `deadreckon apply --autostash` for dirty checkouts and `--cleanup` to remove the run worktree/branch after a successful apply.

## Self-documenting runs (alpha) — 2026-05-11

- Added run-start doc scaffolding under `working/.deadreckon/docs/` with stoa-shaped `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, `_incremental.jsonl`, and `polish.json`.
- Added deterministic per-turn narrative chunks, phase coalescing, decision detection, trace/snapshot citations, worktree commit SHA capture, and optional `AS-BUILT-DELTA.md`.
- Added the `run-narrator` skill, provider-backed end-of-run polish with JSON retry, SHA-256 idempotency, diff coverage retry, and nonfatal polish failure statuses.
- Added `deadreckon doc <run-id>`, `list` DOCS status, doc-aware completion actions, extend-parent narrative updates, and generated `apply` commit bodies from run docs.
- Added 48 rider-named depth tests in `crates/deadreckon/tests/self_documenting.rs`.

## Codebase modes (alpha) — 2026-05-11

- P1: Added codebase mode records, fresh-mode metadata, and deterministic mode resolution plumbing without changing `PipelineState`.
- Added codebase-aware `run` defaults: clean git repos now run in an isolated `git worktree` on a `dr/...` branch, while the old empty-working-dir behavior remains behind `--fresh`.
- Added explicit copy (`--from`), worktree (`--worktree`, `--base`, `--branch`, `--allow-dirty`), and in-place (`--in-place --i-know-its-a-lot`) modes with single-screen preview and `--preview` / `--yes` scripting paths.
- Added worktree lifecycle verbs: `deadreckon apply <run-id>` with squash/merge/cherry-pick strategies and `deadreckon abandon <run-id>` with branch/worktree cleanup.
- Integrated codebase modes into `list`, `show`, `materialize`, `extend`, `undo`, run completion prompts, and TUI completion actions. Worktree runs now hint apply/abandon; copy/fresh runs continue to hint materialize/extend.
- Added worktree-aware `extend`: child worktree runs branch from the parent `dr/...` branch and record `parent_branch` in `codebase.json`; in-place parents refuse with a `run --in-place` hint.
- Added depth coverage for every rider-named codebase test, including dirty/refusal preflight, preview and non-git prompt UX, worktree/copy/in-place modes, apply conflict handling, abandon force cleanup, lifecycle hints, and extend integration.

## Lifecycle ergonomics

Phase commits: `4481617`, `556897d`, `91ab9a6`.

- Added `deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]` to copy completed library artifacts to user-owned paths with `.deadreckon/parent.json` provenance and library `.materialized-to` reverse markers.
- Added `deadreckon extend <run-id> "<new-goal>"` to create a fresh run from a completed parent artifact, seed the working tree, prepend a parent summary into `history.json`, and record lineage through marker files plus a synthetic trace.
- Added lifecycle hints after completed `run`/`attach`, `--no-hints` suppression, `list` materialization status, and `show` parent-lineage output.
- Kept `PipelineState` unchanged; lifecycle lineage lives in marker files.

## 0.1.0 - Robustness Milestone (alpha)

Implementation commit: `cec49f3`.

- Hardened the run loop with broadcast/file-backed events, per-turn timers, cancellation tokens, wall-clock CLI spend accounting, partial-trace resume, and `resume --from-turn`.
- Hardened sandbox execution with generated Seatbelt/bwrap policy inputs, tmp `$HOME`, network denial, persisted profiles, and adversarial path/network tests.
- Hardened acceptance by moving `dr-gate` to `acceptance.yaml`, signing markers with a run-local nonce, and refusing forged self-attestation.
- Hardened import normalization for Claude Code, Codex, and Cursor histories into deadreckon traces/provenance.
- Hardened multi-run coordination with scope-qualified lock files and same-scope refusal tests.
- Hardened library promotion with post-gate atomic move, manifest writing, and crash recovery.

Still thin: provider pings in `doctor` are intentionally conservative unless explicitly enabled, and the TUI uses durable event replay for cross-process attach because Tokio broadcast is in-process.
