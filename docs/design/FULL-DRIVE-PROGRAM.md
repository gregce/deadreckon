# FULL-DRIVE — driving all of deadreckon from the Mac app

**Bar (operator, 2026-08-07):** fully drive deadreckon from the Mac app; the CLI becomes an escape hatch by choice, not a requirement.
**Method:** three read-only audit workflows over the v0.8.4 binary and the APP-1..5 app, every contested claim live-corroborated against the built binary in scratch homes. Companion to `MAC-APP-OPERATOR-CONSOLE.md` (§7 gap register, as-built notes).

**Bar-met definition:** after Phase 5, every daily journey in the matrix is drivable from the app, the four dead-ends are closed, and the CLI's remaining role is exactly the deferred-by-design set — demonstrated by the Phase 3 terminal-never-opened transcript and the Phase 2 machine-restart drill, and pinned thereafter by the launch-protocol conformance kit.

---

## Part 1 — Full drivability matrix (every operator journey)

# Full-Drive Matrix — every CLI journey vs the Mac app (v0.8.4, live-corroborated 2026-08-07)

Classes: **drivable** (executes or reads in-app today) · **dead-end** (rendered as text/disabled, not executable) · **absent** (no surface) · **deferred-by-design** (scoped out with a cited rationale). Closure: **app-only** / **binary+app** / **conformance-only** / **closed** / **stays deferred**. Live probes ran `/Users/gdc/deadreckon/target/release/deadreckon` (0.8.4) against scratch `DEADRECKON_HOME`.

## A. Lifecycle

| # | Journey | Crit | CLI surface (JSON state) | App today | What closes it |
|---|---|---|---|---|---|
| A1 | Define done (def-done / acceptance authoring + check) | daily | `def-done add/check/show`; **no `--json` anywhere** (live) | **dead-end** — named in refusal try_lines (WriteSurfaceRouter.swift:29-59) and LayCourse teach text "authoring stays CLI" (LayCourseSheet.swift:361-364); no authoring surface | **binary+app** (def-done envelopes + LayCourse contract step) — Phase 4 |
| A2 | Prove the harness (`try`) | setup-once | `--json` ✅ (live) | **absent** | **app-only** (onboarding proof step) — Phase 3 |
| A3a | Start, guided single-run mode | daily | `start --json` preview → `--plan --yes --json` execute ✅ (M1/G2); blocked-preview envelope live-verified on bare dir | **drivable** — LayCourse preview→replay incl. >$50 typed-amount arm (WriteCoordinators.swift:11-146) | **closed**; app-only field widening (`--deadline`, `@goal-file`, `--review-done`) + conformance fixtures — Phase 5 |
| A3b | Start `--mode review\|full-plan` (orchestrate shapes via the front door) | power → **promoted** | same protocol; live preview envelope carries `selected_mode` + mode-override evidence | **absent** — LayCourse passes no `--mode` | **app-only** + conformance mode-matrix — Phase 5. Judgment: launch+basic-observe promoted; voyage-tree UI stays v1.x (§6.2, MAC-APP-OPERATOR-CONSOLE.md:566) |
| A3c | `run` (single power launcher) | power | no `--json` **by design** (live: 0 hits); machine path is `start` (§26.9) | **deferred-by-design** — start subsumes the journey | **stays deferred**; CLI escape hatch |
| A3d | `orchestrate` verb | power | no `--json` (live) | **deferred-by-design** — same shapes reachable via A3b | **stays deferred** (verb); shape promoted via A3b |
| A3e | Campaign launch | power | no `--json` on launch (live); `kill` cascade enveloped (G1) | **absent**; §6.2 scoped v1.x | launch: **binary+app** (campaign launch envelope) — deferred until demanded; observe rows + kill + attach handoff: **app-only** — Phase 5b |
| A3f | Chain launch + recovery (pause/undo/redo/extend/hooks) | power | `--json` on list/show/status only (live: 1 hit); launch/recovery text-only | **absent**; §6.2 v1.x | launch: **binary+app**, deferred until demanded; rows-observe **app-only** — Phase 5b; recovery verbs stay CLI |
| A3g | plan / fork / merge building blocks | power | all three `--json` ✅ | **absent**; §6.2 v1.x | **app-only when demanded** (binary ready) — revisit after Phase 5 |
| A3h | Reshape (Course) | power | `--json` ✅ | **absent** | **app-only** — Phase 7 |
| A4 | Observe / attach cockpit | daily | TUI + `--json` snapshot | **drivable** — workbench is read-model parity (spine/narrative/activity/turns/timeline/changes/docs); `attach` hands off to Terminal.app with pasteboard degrade (TerminalLauncher.swift:4-9) | **closed**; TUI stays escape hatch by design |
| A5a | status / list | daily | ✅ M0/G3 + APP-2 labels; **live: empty-project `status --json` refusal is prose, not enveloped** | **drivable** (FleetStore/JobDetailStore) | **conformance-only** + tiny binary hygiene (arm refusal envelope) — Phase 0 register |
| A5b | `follow` stream | power (daily for drivers) | ✅ M3/G5, pinned by tests/follow_stream.rs + docs/TAILING.md | **absent-as-consumed** — app tails the same files directly (explicitly supported) | **app-only** adoption swap — Phase 6 |
| A6 | Steer | daily | ✅ M1-widened + G6 eligibility | **drivable** (SteerBar + quick-steer; delivered-flip on typed event; refusal downgrade) | **closed** |
| A7 | Kill | daily | ✅ G1 (campaign cascade = concatenated envelopes) | **drivable** (KillSheet; escalate toggle; terminal-event resolution) | **closed** |
| A8 | Extend / send-back | daily | ✅ G1+G9 | **drivable** (SendBackSheet `--note`; disarm/re-arm) | **closed** |
| A9a | Finish: dry-run → promote (apply/export routing) | daily | ✅ G1 + M2/G4 | **drivable** (PromoteSheet two-key gate; honest `unsupported` band for pre-M2 binaries) | **closed** |
| A9b | `apply` / `materialize` direct verbs | weekly | ✅ G1 both | **absent** as distinct affordances — finish routing + dest flags cover the journeys | **app-only** (optional) — Phase 1 |
| A10 | Abandon (the gate's fourth decision) | weekly, hit at daily gate | ✅ G1 — **live: refusal envelope verified perfect** | **absent** — gate menu offers only Promote/Send back/Kill/Inspect (GateQueueView.swift:186-200); TUI has `x` | **app-only** — Phase 1 |
| A11 | Undo | weekly (advertised after every promote) | ❌ no `--json` (live) | **dead-end** — success `next_actions` + destination subtitle name `deadreckon undo` as selectable text (PromoteSheet.swift:597-602, 474, 686-690) | **binary+app** — Phase 1 |
| A12 | Rewind (flight recorder) | power | `--json` flag ✅ (live) **but refusal NOT enveloped** (live) — app's "no --json envelope yet" label half-stale | **dead-end** — permanently disabled button (EvidenceRail.swift:604-609) | **binary** (arm G1 error envelope, S) **+ app** (preview→apply) — Phase 1 |
| A13 | Resume (legacy) | legacy | ❌; public resume retired (doc:55) | **deferred-by-design** — app already renders supervisor status + lease freshness, the sanctioned replacement | **stays deferred**; legacy runs via terminal |
| A14 | Cleanup | weekly | ❌ no `--json` (live) | **absent** | **binary+app** — envelope in Phase 1 binary batch; affordance Phase 7 |
| A15 | Doc (run story: generate/polish/export) | weekly | ❌ no `--json` (live) | **drivable for reading** (Docs tab opens generated files); generation/polish **absent** | **binary+app** — Phase 6 |

## B. Harness stewardship

| # | Journey | Crit | CLI surface | App today | What closes it |
|---|---|---|---|---|---|
| B1 | Init / first run | setup-once | ❌ `--json`; `--no-confirm` scripts; `setup --supervisor` exclusive | **absent** | **binary+app** — Phase 3 |
| B2 | Config incl. BYOK provider keys | setup-once | ❌ `--json` | **absent** — Settings "mutates nothing under DEADRECKON_HOME" (SettingsView.swift:9) | **binary+app** (envelopes + secret-safe key entry) — Phase 3 |
| B3 | Providers / models / detect | setup-once | ✅ all three | **drivable read** (harbor chip, LayCourse catalogs, probe-failure rows); `providers check` / `detect --ping` never invoked | **app-only** (active probe buttons) — Phase 3 |
| B4 | Doctor | weekly | ✅ `--json` but **conflicts with `--repair`** (live) | **drivable read** (chip + schema handshake); `--repair` / `--live` **absent** | **binary** (repair envelope; lift conflict) **+ app** — Phase 2 |
| B5a | Supervisor observe (`status`) | daily-invisible | `--json` ✅ but **live: refusal is prose on stderr** (scratch-home checkpoint-absent case) | **drivable** (queue chip states + popover dot) | **conformance-only** + binary hygiene (envelope the refusal; type the running-but-foreign-home state) — Phase 2 |
| B5b | Supervisor lifecycle (install/start/stop) | setup-once; total severity when down | ❌ no machine flags (live) | **absent** — observe-only; no PlannedVerb case (MutationRunner.swift:67-81); SMAppService used only for the app's own login item | **binary+app** — Phase 2 |
| B6 | Import other tools' history | power | ✅ | **absent** | **app-only** — Phase 7 |
| B7 | Library | weekly | `list --json` only (live: search ❌) | **absent** | **app-only** (list + local filter); binary for search/show parity — Phase 7 |
| B8 | Self-update / channel | monthly | ❌ | **absent** (Settings shows versions read-only) | judgment: guided handoff now; **binary+app later** (vendored-vs-installed skew is the real problem) — Phase 7 |
| B9 | Completions | setup-once | script output is the product | **deferred-by-design** — terminal-native concern | **stays deferred** |
| B10 | Seams validation | power | ✅ | **absent** | **app-only** — Phase 7 |

## C. Power / introspection

| # | Journey | Crit | CLI surface | App today | What closes it |
|---|---|---|---|---|---|
| C1 | Report | weekly | ✅ (`--html --open` extra) | **drivable** (PromoteSheet reads `report --json`); HTML export absent | **app-only** nicety — Phase 7 |
| C2 | Verdict (+`--receipt` G7, APP-3 Job refs) | daily-adjacent | ✅ (live: `--receipt` present) | **dead-end** — app renders "fresh verdict on JOB refs is a registered Rust gap" (PromoteSheet.swift:112-117), **stale vs APP-3 as-built** (doc:645) | **app-only** adoption (run `verdict <job> --receipt --json` at sheet open) + conformance — Phase 0 sweep + Phase 6 |
| C3 | Show diff/patch (+raw/why-failed/flight) | power | ✅ G10 + APP-3 Job delegation | **drivable** (Changes tab); Job-delegation caveat stale per APP-3 (JobDetailStore.swift:1078-1086 vs doc:664); `--raw/--why-failed/--flight` views absent | **app-only** — Phase 0 sweep + Phase 6 |
| C4 | History grep | power | ❌ | **absent** | **app-only feasible** (search over already-tailed ledgers); binary for cross-scope parity — Phase 7 |
| C5 | Learn | power | ✅ every subcommand | **absent** | **app-only** — Phase 7 |
| C6 | Improve self | power | ✅ | **absent** | judgment: **deferred-by-design** — self-modifying the harness from a GUI has the wrong blast-radius/trust profile; stays CLI |
| C7 | Id grammar / help-all discovery | daily-implicit | n/a | **drivable-by-construction** — selection-based navigation, no typed ids | **closed** |
| C8 | Schemas / TAILING as API | power | checked in | **drivable** — the app is built on them; Settings handshake row | **conformance-only** (the kit consumes them) |

## D. Attach-TUI keybinding journeys

| # | Journey | Crit | TUI surface | App today | What closes it |
|---|---|---|---|---|---|
| D1 | Universal: detach without killing, scroll, help | daily | q/Esc/Tab/j/k/? | **drivable** — window idioms; closing the app never kills work | **closed** |
| D2 | Helm: why panel, timeline scrub, tree zoom | daily | w/t/Enter/b | **drivable** for run/job (spine parity JobDetailView.swift:355-418, timeline tab); voyage-tree zoom deferred with its surfaces | zoom rides Phase 5b / stays v1.x |
| D3 | Narrative: toggle, visual cycle, provider refresh | daily | n/v/r | **partial** — narrative/raw tabs with trust split drivable; visual cycle (architecture/agents/files/evidence) and bounded provider-backed refresh **absent** | **app-only** — Phase 6 |
| D4 | Completed footer: a/x/m/e/s/d | daily | apply/abandon/export/extend/show/docs | **drivable except `x`** — a/m/e/s/d map to Promote/export-dest/SendBack/Inspect/Docs; abandon absent | Phase 1 closes `x` |
| D5 | Chain modal (`:` verbs, kill/extend) + campaign zoom | power | chain/campaign attach | **deferred-by-design** with chain/campaign surfaces (v1.x) | rides Phase 5b when promoted |
| D6 | Rudder `:steer` | daily | run-only modal | **drivable** (SteerBar) | **closed** |

**Tally (49 rows):** drivable or drivable-by-construction ~19 · partial/read-half ~4 · dead-end 4 (def-done, undo, rewind, verdict-adoption) · absent ~16 · deferred-by-design 6 (run, orchestrate-verb, resume, completions, improve-self, chain/campaign deep surfaces).

---

## Part 2 — Synthesis: live findings, judgments, phases

## Synthesis method and evidence base

Inputs: the v0.8.4 CLI journey inventory, the Mac-app affordance ledger, and `/Users/gdc/deadreckon/docs/design/MAC-APP-OPERATOR-CONSOLE.md` read in full (739 lines: three concepts, §6.2 composite recommendation, §7 gap register G1-G10 with as-built M0-M3/APP-2/APP-3 blocks, §8 roadmap, §9 shell). I corroborated every contested surface live against the built binary `/Users/gdc/deadreckon/target/release/deadreckon` (0.8.4) in throwaway `DEADRECKON_HOME` scratch dirs, read-only throughout. Mid-edit caveat honored: MAC-APP-OPERATOR-CONSOLE.md is untracked and concurrent workflows are editing start/def-done/LayCourse files, so app-side line numbers are snapshot-accurate only, and the conformance kit referenced throughout is itself in concurrent design — the program prescribes its extension contract, not its internals. Noted, not judged.

## Live findings that changed the synthesis

1. **`rewind` has `--json` at v0.8.4** — the app's permanently-disabled button reason ("no --json envelope yet", EvidenceRail.swift:604-609) is half-stale. Half, because a second probe showed rewind's refusal path is NOT enveloped (prose on stderr, empty stdout) — so the slice is small-binary (G1 arming) + app, not app-only and not the M-effort the app text implies.
2. **The G1 rule is not uniformly armed.** `abandon --json` refuses with a perfect `{"kind":"error",code,verb,message,try_lines}` envelope (the gold standard), but `status --json` (empty project) and `supervisor status --json` (instance checkpoint absent) refuse in prose with exit 1. The documented rule "once a parsed subcommand carries --json, every refusal is an envelope" (doc:595) holds for the nine G1 verbs, not the whole --json surface. This seeds Phase 0's conformance sweep with three concrete violations.
3. **The launch protocol is healthy and mode-aware.** `start "goal" --json` on a bare directory emits a full blocked-preview envelope (`will_start:false`, `done_criteria:"missing"`, `selected_mode`, mode-override evidence, try_lines, exit 0) — simultaneously proving the machine protocol, providing the kit's first blocked-preview fixture, and proving that review/full-plan launching is reachable app-side with zero binary work.
4. **The supervisor service is running** (launchctl: state running, pid 79697, program `/Users/gdc/.local/share/deadreckon/bin/deadreckon`) — correcting the prior's "the operator's machine runs degraded without the service" as a statement about today. Also live-demonstrated: supervisor health is two-source (service manager truth AND the per-home instance checkpoint), which Phase 2 turns into a typed state.
5. **Empty-home `list --json` returns a valid document** — the fresh-Mac app renders an honest empty fleet before any job exists.
6. **Stale honesty labels beyond rewind:** verdict-on-JOB-refs and show-diff Job delegation are both rendered as "registered Rust gaps" in the app while APP-3 as-built blocks (doc:645, 664) say the binary closed them. The lesson is systemic: hardcoded gap labels drift; Phase 0 replaces them with startup capability probes and kit coverage.

## The deferred-by-design judgment (rule 1, called explicitly)

§6.2 (doc:566) scoped plan/chain/campaign to "rows with child counts and open in attach... until a voyage-tree surface earns its way in (v1.x)". Under the operator's new bar I promote two things: (a) **launching review/full-plan** — app-only, since `start --mode --json` preview→plan→execute already reaches those shapes through the blessed protocol; (b) **basic observing** of all three tree shapes as rows + child counts + enveloped kill + attach handoff — which is exactly the §6.2 v1 line the app never actually implemented. I keep deferred: (c) voyage-tree drill-ins (v1.x stands — no daily-leverage evidence against it), and (d) **campaign/chain launch from the app**, because those verbs have no `--json` (live-verified) and launching through prose would breach the envelopes-only rule the app's whole trust posture rides; they promote only when a binary milestone adds launch envelopes, demand-gated. `run` and the bare `orchestrate` verb stay deferred-by-design permanently: start subsumes both journeys machine-side, per §26.9's own reasoning.

## Priors: validated and overturned (rule 2)

- **Supervisor near-top — validated as near-top, premise corrected.** The machine does not run degraded today (finding 4). The rank survives on the bar itself: a fresh Mac must not need a terminal, and the one chip gating all durable-job recovery currently has no in-app remedy. Phase 2, parallel with Phase 1.
- **Dispositions — promoted to first-among-equals.** Abandon is the missing fourth gate decision beside Promote/Send back/Kill; the app advertises `deadreckon undo` as dead text after every promote; and two of three verbs need zero-to-tiny binary work (abandon fully enveloped, rewind needs only arming). Highest leverage-per-effort in the program.
- **First-run/init + config (BYOK) — validated** (Phase 3), with one addition the priors missed: `try --json` as the keyless first-hour proof inside onboarding.
- **Contract authorship inserted (overturn-by-addition).** The priors omitted def-done, but the terminal-free bar fails at the first real job without it: the live preview blocks on `done_criteria:"missing"` and the app's own teach text routes to the terminal. It is the only remaining daily journey with zero JSON. Phase 4.
- **Launcher shapes then power verbs — validated** (Phases 5, 7).

## Trust posture (rule 3, cross-cutting)

Nothing in the program touches promotion authority: no dr-gate, no sign, no override, no force — the MutationRunner absences stay absences. New trust surface introduced: service-unit stewardship (user-scope LaunchAgent only, unmanaged-unit refusals rendered verbatim, plists written only by the binary), secret handling for BYOK keys (stdin/env not argv; the literal-command-line display gains its one redaction rule), and destructive dispositions (typed confirms, file-backed resolution, hash-guarded rewind, preview-first everywhere). The update verb is deliberately kept as a guided handoff because in-app update conflates the vendored app binary with the service-pinned install.

## Proof discipline (rule 4)

Every phase names a real-binary proof: scratch-home smoke drills for dispositions; the HOWTO.md:130-133 machine-restart drill (operator acceptance, never test-inferred) plus clean-account install/start/stop cycling for the service; a terminal-never-opened clean-account transcript for first run; the contract-flips-preview coupling for def-done; the mode-matrix protocol run for launchers; tailer-vs-follow A/B for observation. The conformance kit accretes one module per phase, seeded by the three live envelope violations, so "fully drive" ends as a pinned, re-runnable property of the binary+app pair rather than a claim.

Bar-met definition: after Phase 5, every daily journey in the matrix is drivable from the app, the four dead-ends are closed, and the CLI's remaining roles are exactly the deferred-by-design set — an escape hatch by choice, demonstrated by the Phase 3 transcript and the Phase 2 restart drill.

---

## Part 3 — Start-sequence gap matrix (launch dimensions, live-verified)

# Launch-dimension gap matrix — CLI vs Mac app (v0.8.4, verified live against the vendored binary)

Gap classes: **missing-UI** (binary can, app cannot express) · **missing-flag** (binary itself cannot express it non-interactively) · **dead-end-refusal** (app shows a fix it cannot perform) · **works-but-unproven** (composed path exists but no test drives the real binary through the app's argv) · **deferred-by-design** (design doc v1 scope bullet 5 scoped it to v1.x). Severity is for a daily operator launching from the app.

| # | Dimension | CLI capability | App capability today | Gap class | Severity |
|---|---|---|---|---|---|
| 1 | Goal | positional, `@file` expansion, `--goal-file` | free text after literal `--`; `@path.md` still expands (accidental — `--` defeats clap, not the positional's @-syntax, main.rs:833-843) | works-but-unproven (the `-- <goal>` + @-expansion interplay is in no Rust test) | Medium |
| 2 | Project source | `--from`, `--fresh`, `--worktree`, `--allow-dirty` (run-mode only), init-git picker | `--from` only (typed/NSOpenPanel); cwd is always `NSHomeDirectory()` (FleetCLIClient.swift:46) | missing-UI (`--fresh`; dirty-allow) + dead-end-refusal (dirty-worktree try line names `--allow-dirty`, non-git try lines name `--from .`/`--fresh`/`git init` — all inexpressible in-app) | High |
| 3 | **Contract lookup root** (live discovery) | explicit `.deadreckon/acceptance.yaml` resolves from **invocation cwd**; polyglot detection resolves from the **`--from` inspection root** (both verified live) | app always runs cwd=home + `--from`, so: repo with committed contract → `done_criteria_source:"missing"` → **blocked**; repo with both explicit + detectable → `"detected"` silently **ignores the committed contract**; a `~/.deadreckon/acceptance.yaml` **governs every app launch of every project** (all three verified live) | dead-end-refusal + silent wrong-contract (worse than dead-end) — arguably a binary seam bug, needs a decision | **Critical** |
| 4 | Done contract authoring | project file, detection, TTY one-question + review picker, `def-done`, `--review-done` (TTY-only); **no `--acceptance` on start**, **no `def-done --json`** (both confirmed live; registered CONTRACTS.md:1213-1222) | read-only band from preview; missing contract → refusal teaching `def-done` verbatim | dead-end-refusal + missing-flag | High |
| 5 | Mode / shape | `--mode auto\|run\|review\|full-plan`; campaign + extend picker/classifier/replay-only | none — binary auto decides; `selected_mode + reason` fact line only | missing-UI (operator cannot ask for review/full-plan; acceptable for v1 single-run scope, but the sheet cannot say so) | Medium |
| 6 | Child count | `--children` (full-plan), classifier `n` | none | missing-UI | Low (v1 scope) |
| 7 | Provider (primary) | `--provider` + precedence chain + login probes | radio from `providers list --json`; failed probes visible-disabled with try lines | works-but-unproven until corpus lands (this audit's live probe is the first real-binary drive of the app argv) | Medium |
| 8 | Model | `--model` + per-route catalogs | picker per provider + route default | works-but-unproven | Low |
| 9 | Catalog failure surfaces | `providers list --json`, `models --json` both emit typed failures | providers failure rendered; **`models --json` failure never rendered** — picker silently absent (WriteCoordinators.swift:188-200 vs LayCourseSheet gating); no retry without closing the sheet | dead-end-refusal (silent variant — not even shown) | Medium |
| 10 | Role routes (planner/coder/reviewer/child) | full flag set (full-plan; campaign refuses per-child) | none; reviewer-route refusal try line `--reviewer-provider …` inexpressible | missing-UI + dead-end-refusal | Medium |
| 11 | Spend cap | `--max-spend`; default $10; replay clamp | field → `--max-spend`; ceiling read from resolved plan | works-but-unproven (preview leg now proven live once, by this audit) | Medium |
| 12 | >$50 acknowledgment | `--i-know-its-a-lot` on start + replay (M1) | typed-amount match arms the flag; bypass impossible by construction (fake-tested) | works-but-unproven (`--plan` + `--i-know-its-a-lot` + `--from` combined in **zero** Rust tests) | **High (money path)** |
| 13 | Wall clock | **no flag on start** (config `defaults.cli_max_wall_seconds`, else 36,000s); `--max-wall-seconds` is run/orchestrate-only | nothing; wall arrives only inside the opaque plan | missing-flag | Medium |
| 14 | Attempts | no flag anywhere; hardcoded 3 | nothing (B4 mock's "attempts [3]" control has no binary landing slot) | missing-flag | Low |
| 15 | Deadline | `--deadline RFC3339`; replay override semantics | none | missing-UI | Low |
| 16 | Skill | run-only `--skill` | unreachable | missing-flag (on start) | Low |
| 17 | Sandbox | run/orchestrate/campaign/chain-only; config default | unreachable, not shown | missing-flag (on start) | Medium |
| 18 | Network capability | contract-embedded `capabilities.network` | rendered read-only in contract band | none (by design) | Low |
| 19 | Seams | `--no-seams` | none | missing-UI | Low |
| 20 | Narration / prevent-sleep | run/orchestrate/campaign flags; run-only `--prevent-sleep` | none | missing-flag (on start) | Low |
| 21 | Confirmation protocol | preview (`--json`, will_start:false) → persist `launch_plan` → `--plan F --yes --json`; `accepted_by` stamping | implemented exactly (LayCourseController), disarm-on-success/failure, in-flight guards — all **fake-tested only**; app's exact two-leg argv appears in no Rust test; `--from`-on-replay is a code-inspection fact | works-but-unproven | **Critical (the product's spine)** |
| 22 | Supervisor first-install | TTY confirm or pre-install (`setup --supervisor`); non-interactive start **fails closed** (start.rs:4020-4035, 4146-4156) | no setup/supervisor verb; virgin-machine first launch = refusal teaching a terminal command | dead-end-refusal | High (onboarding cliff) |
| 23 | Refusal actionability (cross-cutting) | refusals = `kind:"error"` envelopes (exit 1/2) **and** `kind:"start"` blocked previews (exit 0, no `launch_plan`) — verified live; **no machine-readable class field exists** (machine_json.rs:113-135: kind/code/verb/message/try_lines only) | RefusalView renders verbatim, selectable, **no action of any kind** (no copy, no terminal handoff, no routing) | dead-end-refusal (class-wide) | High |
| 24 | Attach-after-launch | config `start_attach` (TTY-only anyway) | "row appears when job.json lands" | none | Low |
| 25 | Extend (send-back) | `extend --acceptance` exists | in-app but contract always "inherited" (`--acceptance` not wired) | missing-UI | Low-Medium |
| 26 | Orchestrate / campaign / chain / plan-fork-merge launchers | full verbs; caveats: **bare `orchestrate` has no non-interactive parity** (refuses; parity only via `orchestrate review\|full-plan` subcommands), **no `--mode campaign` on start**, chain durable Jobs accept only default branch/apply/stop policy, per-launcher `--max-spend` semantics differ (job vs per-child vs tree-split vs aggregate) | absent entirely — `PlannedVerb` is a closed enum (deliberate); multi-agent shapes reachable only when the binary's auto mode selects them inside a previewed plan | **deferred-by-design** (v1.x per design doc scope bullet 5) — a v1.x needs: `--mode campaign` on start or a `campaign` PlannedVerb case, orchestrate-subcommand argv (not bare orchestrate), distinct spend-cap labeling per launcher, and conformance scenarios added to the same corpus before any UI ships | n/a now; noted for v1.x |

---

## Part 4 — The launch-protocol conformance mechanism

# The launch-protocol conformance mechanism

Design rule: every layer rides a seam the repo already trusts — the checked-baseline pattern (`/Users/gdc/deadreckon/tests/.public-surface-baseline` + `tests/public_surface.rs`), the external-observer contract test pattern (`crates/deadreckon/tests/tailing_contract.rs` + `docs/schemas/*.schema.json` with enum-vocabulary teeth), the real-binary golden pattern (`crates/deadreckon/tests/characterization.rs` + `tests/goldens/` + its normalization/fixed-length-tempdir machinery), the hermetic supervisor fixture (`crates/deadreckon/tests/common/mod.rs` `SupervisorServiceFixture`, incl. `configured_for_binary` for arbitrary binaries), and the vendor-manifest handshake (`deadreckon-mac/scripts/vendor-cli.sh` manifest pin + `BinaryLocator`'s documented `DEADRECKON_BIN` dev/test override). No parallel harness is invented.

## (a) Launch-protocol conformance suite — a shared checked corpus, verified from both sides

**Chosen design: golden-envelope corpus generated by the real binary under the app's exact argv, decoded by the app's actual Swift decoders — with the argv itself as checked data.** Not a Swift process-launching integration target as the primary, for one topological reason: CI (`.github/workflows/ci.yml`) is ubuntu-only and `swift test` currently runs nowhere automatically, so a Swift-side integration target would exercise the real binary only on a developer's Mac. The corpus splits the property so the binary half runs on **every push** in existing CI (a new `--test` file is picked up by `cargo test --workspace` with zero CI wiring) and the Swift half runs wherever `swift test` already runs, with the release script as the mandatory meeting point. A live Swift leg still exists as an optional third layer (below).

**Files**
- `/Users/gdc/deadreckon/docs/LAUNCH-PROTOCOL.md` — the contract doc, TAILING.md-style: names its conformance test, states the two-leg protocol, the two refusal shapes (`kind:"error"` envelope; `kind:"start"` blocked preview with exit 0 and no `launch_plan`), the cwd discipline, and the normalization rules.
- `/Users/gdc/deadreckon/tests/launch-protocol/manifest.json` — the scenario list. Each scenario: name; **argv template** with placeholders (`{GOAL}`, `{FROM}`, `{PLAN_FILE}`); world-setup recipe id (scratch home + smoke provider + repo shape); expected classification (`launchable | blocked | error | launch`); `required_fields` (the fields the sheet renders); `refusal_class` where applicable.
- `/Users/gdc/deadreckon/tests/launch-protocol/corpus/<scenario>.stdout.json` — captured real-binary stdout, normalized (scratch roots → `/CONFORMANCE/ROOT`, `created_at`/timestamps → fixed token; numbers untouched so `budget.ceiling_usd` integer-ness survives — the exact thing the Swift side's JSONSerialization decode depends on).
- `/Users/gdc/deadreckon/crates/deadreckon/tests/launch_protocol.rs` — Rust conformance test. For each scenario: build the scratch world (helpers already exist — `write_start_ready_setup` in orchestrate.rs, `fixed_length_tempdir` + `assert_capture_matches_golden` normalization in characterization.rs), run the **real binary** (`CARGO_BIN_EXE_deadreckon`, or `DEADRECKON_CONFORMANCE_BIN` when set — see (c)) with **exactly the manifest argv**, from a **neutral cwd** (the app's discipline — this is what found the contract-root break), compare normalized stdout to the corpus golden, and assert semantic invariants independent of the golden (preview never allocates `jobs/`, `will_start:false`, `launch_plan` present iff `launchable`, every refusal carries a registered class). Regen mode behind an env var, standard golden practice; blessing a regen cannot bypass the invariants.
- `/Users/gdc/deadreckon/deadreckon-mac/DeadreckonKit/Tests/DeadreckonKitTests/LaunchProtocolCorpusTests.swift` — in the **existing** test target. For each scenario: (1) **argv parity** — build the real `PlannedVerb.startPreview/.startExecute` from the scenario's inputs and assert `arguments` equals the manifest template (this pins flag order, the `--` separator, and `usd()` formatting from both sides to one file — the public-surface-baseline trick applied to argv); (2) **decode** — feed the corpus bytes to the ACTUAL decoders (`MutationResult.classify`, `StartPreviewEnvelope.init?(data:)`) and assert the declared classification, `isLaunchable`, `planCeilingUSD`, and non-nil for every `required_fields` entry; (3) refusal scenarios must decode to a class the handling registry covers (layer b). Corpus located via `#filePath` walk-up to the repo root; a missing corpus **fails** (never skips).

**Initial scenarios (all proven runnable in this audit)** — launchable via detected contract (cargo repo, neutral cwd, `--from`); blocked missing-contract with a committed acceptance.yaml (pins today's cwd-rooting as explicit behavior — finding F1); cwd-contract hijack (pins F6); non-git blocked preview; full preview argv `--provider X --model Y --max-spend N --from D --json -- goal`; >$50 preview (ceiling in plan; Swift asserts ack arming); replay execute → `kind:"launch"`, `accepted_by:"replay"`; replay + `--i-know-its-a-lot` + `--from`; a typed `kind:"error"` refusal (plan-ceiling-exceeds-cap clamp); `providers list --json` and `models --json` incl. a deterministic failed probe (configured route whose binary is absent from a stubbed PATH).

**Drift behavior** — binary renames a field or changes a refusal: Rust golden + invariants go red in CI the same day. App changes argv construction: Swift argv-parity goes red locally and at the release gate. A blessed corpus change the app cannot decode: Swift decode goes red at the gate — the corpus is one file set consumed by both, so it cannot be blessed into disagreement. A **new required picker/prompt**: every scenario runs prompt-impossible (non-TTY, `--json`) under the harness timeout, so a new interactive stop surfaces as a refusal-or-timeout scenario failure, never a silent hang.

## (b) Refusal-actionability rule — one registry, two enforcers

Live fact: refusals have **no machine-readable class** today (`machine_json.rs` error envelope = kind/code/verb/message/try_lines; blocked previews are `kind:"start"` with try_lines). The rule therefore has a data file and a small binary residue.

- `/Users/gdc/deadreckon/tests/launch-protocol/refusal-classes.json` — the enumerable registry: class name, meaning, and the app's declared handling ∈ `in_app_control` (a sheet control exists), `guided_fix` (app can route to a fix surface), `terminal_handoff` (copyable command, honestly labeled). Initial classes from this audit + inventory §3: `done_contract_missing`, `done_contract_divergence`, `source_not_git`, `source_dirty`, `source_from_invalid`, `provider_missing`, `provider_logged_out`, `reviewer_route_missing_schema`, `contract_review_requires_tty`, `supervisor_not_ready`, `spend_needs_acknowledgment`, `plan_ceiling_exceeds_cap`, `plan_kind_unsupported`.
- **Rust residue (S, register as a §7-style gap):** a `StartRefusalClass` enum threaded through start's launch-boundary refusal helper — compiler exhaustiveness replaces grep — serialized additively as `refusal_class` on start-verb error envelopes **and** top-level on blocked previews. Test in `launch_protocol.rs`: `StartRefusalClass::ALL` == registry file (baseline-compare, public-surface style); every corpus refusal carries a registered class. Until the field lands, the registry binds at **scenario granularity** (the manifest tags each refusal scenario with its class) — enforceable this week, upgraded to envelope granularity when the field ships.
- **Swift enforcer:** `LaunchRefusalHandling.registry` in DeadreckonKit; test asserts its keys equal the same checked file, that every corpus refusal scenario resolves to a declared handling, and that the unknown-class fallback exists and is `terminal_handoff` with a copy affordance — so a class added Rust-side first fails Swift registry-parity at the next `swift test`/release gate instead of shipping a silent dead-end, and even a mid-flight unknown class degrades visibly rather than dead-ending.

## (c) End-to-end launch smoke — declare → preview → plan replay → terminal

One Rust test, `launch_protocol.rs::app_argv_launch_smoke`, riding `SupervisorServiceFixture::configured_for_binary` (the hermetic supervisor: HOME/XDG/PATH redirected, fake service manager, real `supervisor serve` child, readiness on the v3 checkpoint — production admission evidence with zero host mutation): scratch `DEADRECKON_HOME`, `default_provider = "smoke"` (the scripted keyless provider), scratch cargo repo (detected contract — today's only app-launchable contract path, per finding F2), **neutral cwd**, then the app's exact preview argv → persist the envelope's `launch_plan` bytes → `start --plan F --yes --json` (+ `--from`) → assert `kind:"launch"` → poll `status <id> --json` to a terminal verified state, and tail `notify.jsonl`/`job-events.jsonl` with the TAILING.md reader discipline so the same run also witnesses the app's observation seams.

- **Locally**: `make test` (it is just a cargo test) or focused `cargo test -p deadreckon --test launch_protocol`.
- **In CI**: automatically, every push (ubuntu; sandbox `none`, smoke provider — same posture as `make smoke`/`smoke_invariant.rs`).
- **In the release script, pre-notarization**: `deadreckon-mac/scripts/release-app.sh` gains a conformance-gate stanza after the vendored-CLI codesign verify and before `xcodebuild`: (1) `swift test` in `DeadreckonKit` (corpus decode + argv parity + registry parity); (2) `DEADRECKON_CONFORMANCE_BIN="$ROOT/Resources/bin/deadreckon_darwin_$(host arch)" cargo test -p deadreckon --test launch_protocol` — the **vendored signed bytes**, not a fresh build, walk the full protocol (the fixture was built for arbitrary binaries; vendor-cli.sh's `list --json` smoke stanza is the precedent, extended). Non-zero exit stops the release before anything is signed or submitted.
- **Optional third leg (highest fidelity)**: `LaunchProtocolLiveTests.swift`, XCTSkip-gated on `DEADRECKON_BIN` + a scratch-home env var, driving the real `FleetCLIClient`/`CLIRunner` (process spawn, watchdog, env merge) through `LayCourseController.runPreview()`/`execute()` against the vendored binary — run only from the release gate, so default `swift test` stays hermetic.

## (d) Where each check lives, who runs it, what breaks loudly

| Check | Lives at | Runs | Breaks loudly when |
|---|---|---|---|
| Corpus generate/verify + semantic invariants + refusal registry (Rust side) | `crates/deadreckon/tests/launch_protocol.rs` + `tests/launch-protocol/` | every `cargo test` — CI on every push, `make test`/`make verify` locally | binary drifts: renamed/removed field, new refusal wording/class, new prompt, preview allocating state, replay semantics change |
| Argv parity + corpus decode + registry parity (Swift side) | `DeadreckonKitTests/LaunchProtocolCorpusTests.swift` | `swift test` locally; **mandatory at the release gate**; recommended later: a small macOS CI job | app drifts: PlannedVerb argv change, decoder field loss, undeclared refusal handling, corpus blessed into something the sheet cannot render |
| E2E launch smoke (declare→preview→replay→terminal) | same Rust test file, supervisor fixture | CI every push (cargo binary); release gate (vendored bytes via `DEADRECKON_CONFORMANCE_BIN`) | the composed protocol breaks anywhere end-to-end, incl. supervisor admission and money-path flags |
| Live app-runner leg (optional) | `LaunchProtocolLiveTests.swift` (XCTSkip-gated) | release gate only, `DEADRECKON_BIN` at vendored binary | process-spawn/timeout/env-merge seams drift |
| Binary identity | existing `vendor-cli.sh` manifest pin + `BinaryLocator` fail-closed | vendor time + app launch | vendored bytes ≠ manifest sha256 |

The one enforcement hole to close deliberately: `swift test` has no CI home. Until a macOS CI job exists, the release gate is the sole automatic meeting point — acceptable (nothing notarizes without it) but worth the small CI job as the durable fix.

---

## Part 5 — Quick wins

# Quick wins, ordered by leverage

1. **Decide and pin the contract-root semantics (the live-found critical).** Today, through the app's only possible invocation (cwd=home + `--from`), a repo's committed `.deadreckon/acceptance.yaml` is invisible (`done_criteria_source:"missing"` → blocked → def-done dead-end), detection silently outranks a committed contract, and a `~/.deadreckon/acceptance.yaml` would govern every app launch of every project. Two candidate fixes, either pinnable this week: (a) binary roots explicit-contract lookup at the `--from` inspection root (matching detection — likely the right semantics); or (b) app-side one-liner — run the start legs with process cwd = the chosen project directory (verified live: cwd=repo + `--from` repo → `project` contract + launchable copy-mode). The concurrent start/def-done/LayCourse workflow may already be on this; whichever behavior is chosen, the conformance scenario pins it.
2. **Land `tests/launch-protocol/` (manifest + corpus + `launch_protocol.rs`) with the ten read-only scenarios.** All machinery exists (`write_start_ready_setup`, `fixed_length_tempdir`, golden normalization); ~a day. From then on the binary half of the app protocol is checked on every push with zero CI wiring, and findings F1/F4/F6 become pinned behavior instead of folklore.
3. **`LaunchProtocolCorpusTests.swift` — argv parity + corpus decode in the existing test target.** Half a day. This is the first time the app's actual decoders meet real-binary bytes, and the first time both argv builders answer to one checked file (catches `usd()` formatting, flag order, `--` separator drift).
4. **Wire the release gate into `release-app.sh`.** ~10 lines before the build: `swift test`, then the conformance suite against the vendored host-arch binary via `DEADRECKON_CONFORMANCE_BIN`. Highest leverage per line: converts everything above plus the existing fake-driven Swift suite into a pre-notarization gate. Extend `vendor-cli.sh`'s smoke stanza from `list --json` to one app-argv preview while there (2 lines).
5. **E2E smoke test (`app_argv_launch_smoke`) on the supervisor fixture** — preview → plan replay → terminal verified state with the smoke provider, including the `--plan` + `--i-know-its-a-lot` + `--from` combination that currently appears in zero tests (the money path).
6. **Refusal registry at scenario granularity + smallest app actionability fixes**: check in `refusal-classes.json`, declare `LaunchRefusalHandling` with parity tests both sides; add a copy affordance to `RefusalView` try lines (turns every dead-end into an honest terminal handoff); render the `models --json` failure state (today silently missing picker); add a catalog retry button.
7. **Register the two Rust gaps in the design-doc §7 style**: `refusal_class` field on start-scoped refusals (S), and the contract-root decision from item 1 (with `--acceptance`-on-start / `def-done --json` already registered at CONTRACTS.md:1213-1222 as the durable fix for the def-done dead-end).
8. **Medium-term**: a small macOS CI job running `swift test` + the corpus tests, so app-side drift is caught on push rather than at release.
