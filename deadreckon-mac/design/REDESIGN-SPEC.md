# REDESIGN-SPEC.md — full redesign implementation spec

Companion to `/DESIGN.md` (the visual constitution — tokens and component rules live
there, not here). This file is the complete implementation contract for the redesign:
nomenclature, information architecture, flows, per-surface layouts, and the ordered
two-implementer plan.

Ground rules, restated as law:

1. **Simplify, never remove.** Every capability in the current app survives. Where this
   spec and current code disagree about what the binary can do, capability wins.
2. **Trust rules untouchable.** No override affordances. VERIFIED renders only from
   `receipt.verified == .valid`. Refusals verbatim, selectable, with only the binary's
   own try-lines as recovery. The app never invokes dr-gate. Kill/finish/extend
   semantics keep their precise mechanics in confirm bodies.
3. **Two vocabularies, never blended.** UI words are plain product language (this spec's
   lexicon). On-disk/CLI truth (`deadreckon …` commands, file names, envelope words, raw
   status labels, exit codes) appears only in monospace technical contexts — command
   wells, tooltips' provenance lines, the drawer, refusal bodies — and is never
   translated there.
4. **The mental model everywhere:** Projects contain Goals; a Goal executes as a Run;
   finished Runs await Review; Review = Approve / Send back / (Discard — see §A0 note);
   Stop replaces Kill in labels.

---

## A. NOMENCLATURE MAP

Every user-visible string in `Sources/` was audited (grep over `Text(`, `Button(`,
`TextField(`, `Label(`, `Toggle(`, `.help(`, `navigationTitle`, `window.title`,
`sectionTitle(`, plus Kit-surfaced words from `QueueSection`, `GlossaryText`,
`SpineSnapshot`, and `AttentionDerivation`). The tables below are normative: old → new.
Strings not listed (pure mono machine truth: file names, commands, envelope fields,
sha prefixes, `$` figures, counts) are unchanged by rule 3.

Implementation seam: a new app-target `Sources/Views/Lexicon.swift` owns every UI word.
`GlossaryText` (Kit) keeps mirroring `glossary.rs` and stays untouched — its words are
now treated as CLI truth that `Lexicon` translates for display. Views stop calling
`GlossaryText` directly for labels and call `Lexicon`; mono contexts keep quoting
`GlossaryText`/raw values.

### A0. Core nouns and verbs

| Old (UI) | New (UI) | Notes |
|---|---|---|
| Job (noun, user-facing) | Run | The durable Job row renders as a Run. "Goal" is what you write; the run is its execution. Mono contexts keep `job-…` ids and the word `job` where the CLI says it. |
| Lay Course | New Goal | Everywhere: button, sheet title, menu, shortcuts help. |
| Gate Queue (home surface) | Overview | The zero-selection center panel. The queue-as-home dissolves into sidebar + Overview (§B). |
| Fleet | Runs / "your runs" | "Reading the fleet" → "Reading your runs…"; "Fleet unavailable" → "Can't read your runs". |
| Promote (verb) | Approve | Sheet primary. Command well still shows `deadreckon finish … --yes --json`. |
| Kill (verb) | Stop | All labels. Confirm body keeps precise mechanics (SIGTERM/SIGKILL, ledger resolution) verbatim. |
| Send back + note… | Send back… | The note is inside the sheet; the label stays short. |
| Steer / Rudder | Guide | Decision recorded here: **Guide** (not "Message" — it is advisory course-correction, not chat, and PRODUCT.md's anti-goal forbids chat framing). Placeholder: "Guide the agent…". Verb button: "Send". |
| Inspect | Open | Row action, palette action, popover action. |
| Binnacle (promote sheet) | Review & Approve | Sheet title; "Opens the Binnacle" help strings die. |
| Harbor (health chips/strip) | System health | Sidebar footer + Settings > Info wording. |
| Pennant / Route | Agent & model | New Goal section. |
| Workbench / Chartroom | run view | Only ever appeared in help text; say "the run's view" or nothing. |
| Done contract | What done means | Section labels. The chip/legalese contexts may say "done contract" is the CLI's name for it, in the provenance tooltip. |
| Voyage ("Start a voyage") | run | Help string rewritten (§A3.1). |
| Provider (user-facing) | Agent | "Agents 2/2 ready". Mono keeps route ids (`claude-code`). Plain display names: `claude`/`claude-code`/`anthropic` → "Claude Code"; `codex`/`openai` → "Codex CLI"; `gemini`/`google` → "Gemini CLI"; `opencode` → "opencode"; unknown ids display verbatim mono. Binary-supplied `display_name` wins when present. |
| Supervisor (user-facing) | background service / "Service" | Chips: "service running/stopped". Mono/tooltips keep `supervisor` and file names. |
| doctor (user-facing) | Health check | Chip "health check OK"; tooltip keeps `doctor --json` provenance. |
| Attempt (user-facing) | attempt (unchanged) | Plain enough; appears only in evidence contexts. |
| Turn (user-facing) | turn (unchanged) | Plain enough. |

**Discard note (capability wins):** the mental model names Approve / Send back / Discard,
but the committed binary exposes no discard/delete envelope in the M1 verb set. The
Review sheet therefore ships Approve / Send back / Stop (Stop only enabled for a
non-terminal row) and NO Discard control. Register "discard verb" as a Rust-side gap in
CONTRACTS.md; do not fake it.

### A1. Sections and state words (Kit-derived, translated in Lexicon)

`QueueSection.title` (Kit, unchanged) → Lexicon display titles:

| Old | New | Where used |
|---|---|---|
| AT THE GATE (subtitle "awaiting your decision") | READY TO APPROVE — "verified; waiting for you" | Sidebar Needs-review group rows, Overview |
| NEEDS REVIEW (subtitle "stopped for your judgment; no proof claimed") | NEEDS YOUR REVIEW — "stopped for your judgment; nothing verified" | same |
| APPROACHING THE GATE | CHECKING | runs in verifying_checks / verifying_meaning |
| UNDERWAY | RUNNING | |
| WRECKED / BLOCKED | FAILED / STOPPED | |
| UNKNOWN STATE (subtitle "…the CLI remains authoritative") | UNREADABLE — "entries the app could not read; the CLI remains authoritative" | |

Phase words (`GlossaryText.phaseWord` stays; Lexicon overlays):

| Raw / old UI | New UI |
|---|---|
| queued | Queued |
| running | Working |
| verifying_checks / "verifying checks" | Running checks |
| verifying_meaning / "verifying meaning" | Judging result |
| waiting | Waiting |
| terminal | Finished (outcome word replaces it wherever an outcome exists) |
| unknown / "unknown state" | Unknown — shown with its verbatim reason in mono |

Outcome words:

| Raw / old UI | New UI |
|---|---|
| verified | Verified |
| needs_review / "needs review" | Needs your review |
| blocked | Blocked |
| budget_exhausted / "budget exhausted" | Budget used up |
| deadline_reached / "deadline reached" | Deadline reached |
| retry_exhausted / "retry exhausted" | Out of retries |
| cancelled | Stopped |
| failed | Failed |
| verified_proof_invalid / "proof invalid" | Proof invalid (strong danger chip; never softened) |

Stop-reason words (shown after "Waiting —"):

| Old | New |
|---|---|
| operator input required | needs your input |
| paused at spend cap | paused at its budget |
| paused at wall-clock cap | paused at its time limit |
| judge asked for revision | judge asked for changes |
| judge uncertain | judge unsure — your call |
| judge unavailable | judge unavailable |
| checks asked for revision | checks asked for changes |
| cancel requested | stopping… |
| transient provider error | agent error (retryable) |
| fatal provider error | agent error (fatal) |
| fatal gate error | checks error (fatal) |
| lost containment | sandbox breach — stopped |
| supervisor failure | service failure |
| corrupt history | history unreadable |
| deadline / attempt limit / verified | deadline / attempt limit / verified (unchanged) |
| legacy (unknown reason) | older run (reason unknown) |

Proof words: "VERIFIED" chip keeps the word **Verified** (strong success chip; tooltip
"Verified by the harness gate — the signed receipt validated", provenance line keeps
`verified by dr-gate` mono). "proof invalid" → "Proof invalid". "no proof expected"
stays. Spine words are re-formatted from `SpineSnapshot`'s structured fields (never
string-parsed; Kit parity with spine.rs is load-bearing and untouched):

| Spine fact | New UI rendering |
|---|---|
| alive .live(5) | "Last activity 5s ago" (live dot) |
| alive .stale(45) | "Quiet for 45s" (warn only when confirmed-stale rules say so) |
| alive .dead(reason) | "Ended — {reason}" |
| alive .done | "Done" |
| doing "executing - turn 4 - implement" | "Working on **implement** · turn 4" |
| onTrack | "$4.12 of $25.00 · 2.1h · turn 4" ("no budget cap" when nil) |
| wrong [] | "Nothing flagged" |
| wrong [a,…] | "{n} flagged — {first.summary}" (summaries verbatim) |
| next (CLI command) | mono line under NOW: `deadreckon attach r-…` labeled "CLI" |

### A2. Chips, dots, meta-language (shared atoms)

| Old | New |
|---|---|
| "decision needed" chip | "needs you" chip (warn) |
| "hb 12s" | "12s" beside the live dot; tooltip "Last heartbeat 12s ago — lease owner {id}, epoch {n}" |
| "no heartbeat 45s" | "no signal 45s" |
| "5/5 checks" | "5/5 checks" (unchanged; tooltip "From the signed record of check results, attempt {n}") |
| "notify tail N corrupt" | "notifications broken for N runs" (warn chip; tooltip verbatim reasons) |
| "doctor ok" / "doctor N warn" / "doctor N failed" / "doctor unknown" | "health OK" / "health: N warnings" / "health: N failures" / "health unknown" |
| "providers M/N ok" / "providers unknown" | "agents M/N ready" / "agents unknown" |
| "supervisor running/stopped/not installed/stale/unsupported/unknown" | "service running" / "service stopped" / "service not installed" / "service outdated" / "service unsupported" / "service unknown" |
| "updated 2m ago" | "updated 2m ago" (unchanged) |
| "N legacy runs" | "N older CLI runs" |
| "unreadable row" / "unknown id" | "unreadable entry" / "unknown id" |
| "queued #3" / "delivered #3" (steer chips) | "guidance queued #3" / "guidance delivered #3" |
| relative "now" | "now" (unchanged) |

### A3. Per-surface strings

#### A3.1 Main window (today `GateQueueView.swift` + header + strip)

| Old | New |
|---|---|
| Window title "deadreckon" | "deadreckon" (unchanged) |
| "Gate Queue" (title) | (dissolves — sidebar + "Overview" center; Overview headline: "Right now") |
| summary "2 at the gate · 1 need review · 3 underway · 1 wrecked · 1 unknown" | Built by Lexicon from counts: "2 ready to approve · 1 needs review · 3 running · 1 failed · 1 unreadable" ("no runs yet" when empty). Views stop using `GateQueue.summaryLine` (Kit string stays for tests). |
| "Filter" button + help "Filter the queue (Command-K)" | Search icon-button, help "Search runs (⌘K)" |
| "Lay Course" + help "Start a voyage (Command-N): preview before launch, the plan is the decision" | "+ New Goal" (primary), help "Start a new goal (⌘N) — everything is visible before anything runs" |
| Section action "Review at Gate" | "Review & Approve" |
| Row action "Inspect" | "Open" |
| Context menu "Promote…" / "Send back + note…" / "Kill…" / "Inspect" | "Review & Approve…" / "Send back…" / "Stop…" / "Open" |
| "Reading the fleet" | "Reading your runs…" |
| "The fleet is unavailable" | "Can't read your runs" |
| "The CLI remains authoritative: deadreckon list" | "The CLI still works: `deadreckon list`" (sentence 13pt, command mono) |
| Empty: "No jobs in the fleet" + "Lay a course (Command-N) or start from the CLI; either way the job surfaces here from the durable files and queues for your decision at the gate." | See §B4 empty states: "Start your first goal" + "Pick a project folder, say what you want done and what done means, and watch the run work. Everything the agent does lands here as evidence." CLI well unchanged: `deadreckon start "your goal"` / `deadreckon list`. |
| Harbor strip (bottom) | Sidebar footer (§B1): "All systems OK" or the degraded words from A2; "updated 2m ago". |

#### A3.2 Run detail (today `JobDetailView.swift`)

| Old | New |
|---|---|
| Sidebar "FLEET" | (dissolves into the projects tree, §B1) |
| Header run-id / "run {id}" | unchanged mono; label "run" dropped (id speaks) |
| "active 2.1h" / "wall 3.4h" | "active 2.1h" / "elapsed 3.4h"; tooltip "Work clock: 1.2h remaining, limited by {boundary}" → "1.2h left before {plain boundary word: its time limit / its budget}" with raw boundary in mono |
| "spend tail stopped" | "spend feed stopped" (tooltip verbatim reason) |
| Spine band labels "alive / doing / on track / wrong / next" | NOW band (§D): "WORKING ON / ON TRACK / ATTENTION / LAST ACTIVITY" per A1 spine table |
| "spine pending: no state.json for this job's attempt yet" | "Status pending — the run hasn't written its first state file yet" (`state.json` in mono tooltip) |
| "⟩ RUDDER" | "GUIDE" section label on the bottom bar |
| Steer placeholder "advisory note for the run's next turn…" | "Guide the agent — read at the next turn…" |
| "steer: no run attempt yet" | "Guide available once the run starts" |
| "steer unavailable — {reason}" | "Guide unavailable — {plain reason}" (reason words from QuickSteerController mapping, reviewed for plainness) |
| Button "Steer" | "Send" |
| "queueing steer…" | "queueing guidance…" |
| "queued #4" + "next turn boundary" | "guidance queued #4 — delivers at the next turn" |
| "delivered #4 · turn 9 · from the typed steer_delivered event" | "guidance delivered #4 · turn 9"; provenance tooltip "from the `steer_delivered` event" |
| "no machine envelope landed: {words}" | "The CLI answered without a machine envelope: {words}" (words mono) |
| Decision buttons "Promote…" / "Send back…" / "Kill…" | "Review & Approve…" (primary when terminal) / "Send back…" / "Stop…" (quiet-danger) |
| Help "Opens the Binnacle: two keys, contract, receipt, candidate preview." | "Review the evidence and approve the result" |
| Help "extend {id} with your note recorded as provenance" | "Start a follow-up run with your note attached" (command in the sheet's well) |
| Help "Opens the kill confirmation: CancelRequested, SIGTERM…" | "Stop this run — asks first, then force-quits; full details in the confirm" |
| "Open in Terminal" + help "deadreckon attach {id}" | unchanged (help stays the mono command) |
| "Automation denied — command copied to clipboard" | unchanged |
| Quarantined: "This row failed to decode from `list --json`; its facts cannot be trusted enough to open a workbench. The decoder said:" | "This entry could not be read from `list --json`, so its details can't be trusted enough to open. The decoder said:" (reason verbatim mono below) |

#### A3.3 Center tabs (today `DetailCenterTabs.swift`)

| Old | New |
|---|---|
| Tabs "Narrative / Activity / Turns / Timeline" | Tabs "Activity / Story / Changes / Checks / Docs / Recorder" (§D). Activity hosts a [Stream | Turns] toggle; Timeline dissolves (phases → progress band, density → Activity header sparkbar). |
| "No narrative beats yet for this attempt." | "No story yet for this run." |
| "snapshot #12" / "refreshed 30s ago" / "stale 5m" / "no snapshot yet" | "update #12" / "written 30s ago" / "stale 5m" / "no updates yet" |
| "overlay — unverified" chip + "provider-refreshed prose; not evidence" | "AI summary — unverified" + "written by the agent's model; not evidence" |
| "current work / risks / next likely" claim groups | "WORKING ON / RISKS / LIKELY NEXT" |
| "narrator error: {e}" | "story writer error: {e}" (e verbatim) |
| "N malformed snapshot rows skipped" | "N unreadable story rows skipped" |
| "Search the whole scrollback" | unchanged |
| "events tail stopped: {issue}" | "activity feed stopped: {issue}" |
| "No turns recorded yet." | unchanged |
| "turn 7" / "in 1.2k out 3.4k" / "$0.012" / "N entries" | unchanged (facts) |
| "traces tail stopped: {issue}" | "turn feed stopped: {issue}" |
| "PHASES" (timeline) | (band label "PLAN" in the progress band) |
| "No pipeline state yet for this attempt." | "The run hasn't posted its plan yet." |
| "EVENT DENSITY · N events" | "ACTIVITY · N events" (sparkbar in Activity header) |

#### A3.4 Evidence rail (today `EvidenceRail.swift` — content moves into tabs)

| Old | New |
|---|---|
| Tabs "Contract / Chg N / Flight / Docs" | "Checks" / "Changes N" / "Recorder" / "Docs" |
| "acceptance.yaml (frozen)" | "What done means" section; file fact line `acceptance.yaml` (frozen) stays mono beneath |
| "sha matches authority.json" | "matches the approved version" chip (success); tooltip `sha256 approved {…}` mono |
| "DIGEST MISMATCH vs authority.json" | "CHANGED SINCE APPROVAL" strong danger chip; tooltip carries both sha256 values mono |
| "digest unknown state" | "approval status unknown" |
| "network authority: deny" | "network: not allowed" / other values verbatim + warn tint; tooltip "capability compiled into the run's policy (`gate.network`)" |
| "no checks decoded from the frozen spec" | "no checks could be read from the frozen definition" |
| "waiting on report --json for the frozen spec" | "reading the run's report…" (`report --json` in tooltip) |
| "LIVE CHECKS" + "advisory rows · not evidence" | "CHECKS RUNNING NOW" + "as they stream — not evidence" |
| "no live gate rows (strict gates stream nothing; the file appears whole at sign time)" | "nothing streaming — strict checks appear all at once when the record is signed" |
| "must_pass" tag | "must pass" |
| "TWO KEYS" | "TWO SIGN-OFFS" |
| "marker: {status}" | "Checks record: {status}" (status verbatim word) + "signed" fact in tooltip |
| "contained" chip | "sandboxed" chip; tooltip backend name verbatim |
| "judge: achieved / pending / {decision}" | "Judged: achieved" / "Judge: pending" / "Judged: {decision verbatim}" |
| judge summary quote | unchanged — verbatim in quotes, never paraphrased |
| "RECEIPT EVIDENCE" | "RECORDED CHECK RESULTS" |
| "no recorded deterministic checks in the report" | "no check results recorded in the report" |
| "recorded at gate time, from report --json. A fresh verdict re-run is not available…" | "Recorded when the checks ran (`report --json`). The app can't re-run checks for a run today — a registered CLI gap." |
| "report unavailable: {issue}" | unchanged pattern, issue verbatim |
| "Δ 12 files · +340 −80" | unchanged (facts) |
| "Re-run show --diff --json" (help) | "Refresh the diff" (command stays in tooltip mono) |
| "Reading the run diff …" / "No source changes recorded." | unchanged |
| "patch truncated by the binary's byte budget" | "patch truncated by the CLI's size limit" |
| "(empty patch)" / "loading patch …" | unchanged |
| "FLIGHT RECORDER" | "RECORDER" |
| "N events this session · M sessions" | unchanged |
| "No flight manifest for this attempt yet." | "No recordings for this run yet." |
| "No checkpoints captured yet." | unchanged |
| "anchor" chip | "full snapshot" chip |
| "Preview rewind" (disabled) + help "Rewind is not among the M1 machine verbs…" | "Rewind…" (disabled) + help "Rewind isn't available from the app yet (no machine envelope in this CLI version) — use `deadreckon` in Terminal." |
| "No run docs found under .deadreckon/docs for this attempt." | "No documents written by this run yet." (path in tooltip mono) |
| "Open in the default editor" | unchanged |
| "{label} (clipped by the gate)" | "{label} (clipped when recorded)" |
| "hide output" / "show output" / "output ▸" | unchanged |

#### A3.5 Drawer (today `DetailDrawer.swift`)

| Old | New |
|---|---|
| "DRAWER" toggle | "CONSOLE" toggle |
| Tabs "Terminal / Raw events / Job events" | "Terminal / Raw events / Run log" |
| "torn tail" chip + help | "partial line" chip; help "An unterminated final line, kept and retried — an in-flight write, not corruption." |
| integrity chips (contiguous/corrupt/none labels from Kit) | displayed via Lexicon: "ledger intact" / "LEDGER BROKEN" (strong danger) / "no ledger yet"; helps re-worded: "Sequence 1..N verified continuously against `job-events.jsonl`." / "The strict ledger failed verification; showing unknown, never a guess." |
| "older output dropped; showing the last 256 KB (full file on disk)" | unchanged |
| "(empty)" / "(no events yet)" | unchanged |
| "N older raw lines dropped; showing the last M (full ledger in events.jsonl)" | unchanged |
| "last sequence N" | unchanged mono |
| "The strictly-sequenced Job lifecycle ledger (job-events.jsonl): sequence 1..N with no gaps, fsynced before the projection checkpoint. This chip is the app continuously verifying that contract; a gap renders the failure, never a guessed state." | "The run's strict lifecycle ledger (`job-events.jsonl`): sequence 1..N, no gaps. The app verifies it continuously; a gap shows as a failure, never a guess." |
| "projection: phase … · last_sequence … · lease epoch … · attempts …" | unchanged mono (machine truth) |
| "caveat: {c}" / "lease: owner … epoch … pid …" | unchanged mono |

#### A3.6 New Goal sheet (today `LayCourseSheet.swift`)

| Old | New |
|---|---|
| "Lay Course" + "preview before launch · the plan is the decision" | "New Goal" + "Everything is visible before anything runs." |
| "project: {path}" | path chip under the Project picker (mono) |
| "GOAL" | "GOAL" (unchanged); placeholder new: "What should the agent accomplish?" |
| "PROJECT" + field placeholder "resolved from the app's working directory unless set" | "PROJECT" first section; placeholder "Choose the folder the agent will work in"; recents list (§C) |
| "Choose…" | "Choose Folder…" |
| Caption "passed as --from (the source directory is copied into runstate before launch); the preview's source line below is the binary's own resolution" | "The folder is copied into the run's workspace before launch — your working tree is untouched until you approve." Provenance tooltip keeps `--from` mono. |
| "ROUTE (Pennant)" | "AGENT & MODEL" |
| "probing provider routes…" | "checking which agents are ready…" |
| "providers list failed: {r}" | "couldn't list agents: {r}" |
| probe words ok/failed/skipped (`providerProbeWord`) | "ready" / "not available" / "skipped"; message + `try:` lines verbatim mono |
| "model" + "route default" | "Model" + "Default for this agent" |
| "★" recommended | "recommended" text tag |
| "LIMITS" + "spend cap $" + "route default" + "above $50 the Start button becomes a typed confirmation" | "BUDGET" + "Up to $" + placeholder "agent default" + "Over $50, Start asks you to type the amount." |
| "LAUNCH PLAN PREVIEW" | "PREVIEW & START" |
| "Preview course" | "Preview the plan" |
| "nothing has run yet — the preview is read-only (will_start: false)" | "Nothing runs during preview." (envelope fact `will_start: false` in tooltip) |
| "course queued — run a fresh preview to lay another" | "Run started — preview again to start another." |
| fact labels "mode / route / source / done contract / network / will start" | "How / Agent / Source / Done means / Network / Start" — values verbatim from the envelope; "will start → not yet — Start replays this exact plan" |
| "try: {line}" | unchanged (mono accent) |
| "this preview is not launchable — the binary's try lines above are the fix" | "This plan can't start yet — the fixes above are the CLI's own suggestions." |
| "the contract was declared (see the declared file above), but the re-run preview still reports it missing — the binary's try lines above are the fix" | "Your definition of done was written (see the file above), but the fresh preview still reports it missing — the CLI's suggestions above are the fix." |
| "DONE CONTRACT" editor + "what should count as done, in plain English" | "WHAT DOES DONE MEAN?" + "Say it in plain English — checks are drafted from your words." |
| Placeholder "builds, opens in a browser, and has no console errors" | unchanged (good example) |
| "Draft contract" | "Draft checks" |
| "drafts checks from your words via the configured provider, then the binary writes .deadreckon/acceptance.yaml (and acceptance.md) in the project — the app itself writes nothing; on success the preview re-runs automatically" | "The agent drafts checks from your words; the CLI writes `acceptance.yaml` in the project (the app writes nothing). The preview re-runs by itself." |
| "drafting the contract… the binary is calling the provider" | "drafting checks — the agent is reading your words…" |
| "declared file {path}" / "drafted by {who}" | "written to {path}" / "drafted by {who}" |
| "Redefine…" + help | "Rewrite…" + "Reopens the plain-English editor; drafting again replaces the checks (the CLI's own `def-done`)." |
| "queueing the durable Job…" | "starting the run…" |
| "queued {id} · the row appears when job.json lands (file-backed, never optimistic)" | "Started {id} — it appears in the sidebar once its files land on disk." |
| "cap $60.00 — type the amount to arm:" | "Budget over $50 — type 60.00 to confirm:" |
| "⚓ Start — queues the Job, detaches" | "Start Run" (primary; help "Runs the previewed plan exactly: `start --plan … --yes`") |
| ">$50 help …--i-know-its-a-lot…" | "Budgets over $50 need the exact amount typed (the CLI's `--i-know-its-a-lot` acknowledgment)." |
| "Close" | "Cancel" (sheet-dismiss buttons across all sheets become "Cancel" before dispatch, "Close" after a result) |

#### A3.7 Review & Approve sheet (today `PromoteSheet.swift` + `SendBackSheet`)

| Old | New |
|---|---|
| "Promote {job-id}" | "Review & Approve" + goal line; run id mono beneath |
| "TWO-KEY COMPLETION (Binnacle)" | "TWO SIGN-OFFS" |
| "recorded by report --json · fresh verdict on JOB refs is a registered Rust gap" | "from the recorded report — the app can't re-run checks (registered CLI gap)" |
| "Key 1 · deterministic marker" | "Checks passed — signed record" |
| "Key 2 · semantic judgment" | "Judge's call" |
| "no receipt block recorded" / "no judgment recorded" | "no signed record found" / "no judgment recorded" |
| "status {s} · contained · {backend} · signature: {e}" | facts verbatim; "contained" → "sandboxed" |
| "CONTRACT · frozen acceptance.yaml" | "WHAT DONE MEANS — frozen `acceptance.yaml`" |
| "matches authority.json" / "DIGEST MISMATCH" | per A3.4 |
| "net: deny" | "network: not allowed" |
| "○/✓/✗" check rows | unchanged glyph grammar; unmatched result note "not recorded" |
| "clipped — recorded by the gate, not re-run here" | "clipped — recorded when the checks ran, not re-run here" |
| "RECEIPT" + "proof VERIFIED/invalid/…" | "PROOF" + chip per A1 |
| "no receipt on the rollup row" | "no proof recorded for this run" |
| "real finish re-validates the receipt fail-closed before AND after the atomic rename; any drift refuses with no operator override, by design" | "Approving re-validates the proof before and after the files move; any drift refuses — there is no override, by design." |
| "CANDIDATE · preview before mutate" | "RESULT PREVIEW — nothing moves yet" |
| "Refresh preview" + helps | "Refresh preview"; disabled help "Enter an export folder first — there is no default." / enabled help keeps `finish --dry-run --json` mono |
| "this preview was computed for a different destination — Refresh preview before trusting it" | "This preview was computed for a different destination — refresh before trusting it." |
| "promote preview requires the M2 binary" + "PROMOTE below still runs the real fail-closed finish; only the staged-file preview is missing." | "Result preview needs a newer CLI." + "Approve below still runs the real fail-closed apply; only this file list is missing." |
| "finish plan blocked — nothing will promote" | "Blocked — nothing will be applied" |
| "the binary reported status "blocked" without a receipt error message" | "The CLI reported "blocked" without an error message." |
| "real finish would refuse the same way; there is no override" | "Approving would refuse the same way; there is no override." |
| "report-only: real finish re-validates and re-stages from scratch" | "Preview only — approving re-validates and re-stages from scratch." |
| "IRREVERSIBLE: {steps}" | "IRREVERSIBLE: {steps}" (unchanged, danger) |
| "DESTINATION" | "WHERE THE RESULT GOES" |
| "Apply to the working tree" + "undoable afterwards: deadreckon undo" | "Apply to the project" + "undoable afterwards: `deadreckon undo`" |
| "--autostash" / "--cleanup" toggles | unchanged mono toggles |
| "Export to a directory (--dest)" | "Export to a folder" (`--dest` in mono beside the field) |
| "running finish — validate, stage, revalidate, rename, revalidate…" | "applying — validate, stage, revalidate, move, revalidate…" |
| "promoted · completed · 12 files" | "Approved · completed · 12 files" |
| "next: {action}" | unchanged (mono, verbatim) |
| "one-command rollback: deadreckon undo · the row updates from the files, not from this sheet" | "Undo with one command: `deadreckon undo` · the run updates from its files, not from this sheet" (undo claim only when the envelope offers it — rule preserved) |
| "PROMOTE — finish {id}" | "Approve" (primary accent; command well below shows `deadreckon finish {id} --yes --json`) |
| "promote disabled: {reason}" | "Approve disabled: {reason}" ("no export destination entered (--dest has no default)" → "no export folder entered — there is no default") |
| "Send back {id}" (sheet) | "Send back" + goal line |
| "queues a continuation Job under the parent's frozen contract; your note lands as typed provenance the next agentic turn can read" | "Starts a follow-up run with the same definition of done. Your note is recorded and the agent reads it first." |
| "FOLLOW-UP GOAL" | "WHAT TO DO NEXT" |
| "OPERATOR NOTE (--note, recorded on the parent)" | "YOUR NOTE — recorded on the run" (`--note` visible in the command well) |
| "queued {id} · contract {c} · note recorded/not recorded" | "Follow-up started {id} · done-definition {c} · note recorded/not recorded" |
| "I checked the fleet — re-arm Send back" | "I checked my runs — enable Send back again" |

#### A3.8 Stop sheet (today `KillSheet.swift`)

| Old | New |
|---|---|
| "Kill {id}" | "Stop this run?" + goal line, id mono |
| mechanics 1–4 | Kept precise, re-set in plain frame: 1 "A cancel request is written to the run's ledger (`CancelRequested`, sticky) and `cancel.marker` is written." 2 "The service sends SIGTERM to the run's process groups." 3 "2 seconds of grace, then SIGKILL." 4 "The service records the final Stopped event only after proven cleanup." |
| "This sheet resolves only when that terminal event lands in job-events.jsonl — never on an exit code." | "This sheet finishes only when that final event lands in `job-events.jsonl` — never on an exit code." |
| "Escalate subprocess termination (--escalate)" + "a separate, explicit choice — not the default" | "Also force-stop child processes (`--escalate`)" + caption unchanged |
| "dispatching kill…" | "sending stop…" |
| "cancel requested" chip + "signal {s} (escalated) · N processes" | "stop requested" chip (warn) + facts unchanged |
| "killed {id} · N processes" (cascade) | "stopped {id} · N processes" |
| "waiting for the supervisor's terminal event in job-events.jsonl…" | "waiting for the service's final event in the run log…" |
| "no machine envelope landed (exit {n}); the binary said:" | "The CLI answered without a machine envelope (exit {n}); it said:" |
| "resolution unavailable — {reason}" + "the ledger this sheet resolves on cannot be trusted; check `deadreckon status {id}` instead" | "Can't confirm the stop — {reason}" + "the ledger this sheet relies on can't be trusted; check `deadreckon status {id}` instead" |
| "terminal · cancelled (from job-events.jsonl)" | "Stopped — confirmed by the run log" (event word verbatim in tooltip) |
| "Kill" (button) | "Stop Run" (destructive confirm fill) |

#### A3.9 Menubar popover (today `MenuBarPopover.swift`)

| Old | New |
|---|---|
| header summary | Lexicon summary (A3.1) |
| "NEEDS DECISION" | "NEEDS YOU" |
| "UNDERWAY" | "RUNNING" |
| row verbs "Promote… / Send back… / Kill… / Inspect / Steer… / close steer" | "Review & Approve… / Send back… / Stop… / Open / Guide… / close" |
| helps "Opens the Binnacle sheet — the full evidence surface. The popover never promotes directly." | "Opens the full review sheet — the popover never approves directly." |
| "Opens the kill confirmation with the real semantics. The popover never kills directly." | "Opens the stop confirmation — the popover never stops a run directly." |
| "checking steerable{} via status --json…" | "checking if the run can take guidance…" |
| "steer unavailable — {reason}" / "eligibility unknown — {words}" | "guide unavailable — {reason}" / "can't tell yet — {words}" |
| "advisory note for the next turn…" | "Guide the agent…" |
| "queued #3 · next turn · delivery shows in the workbench" | "queued #3 — delivers at the next turn; status shows in the run view" |
| "delivered #3 on turn 9" | unchanged pattern with "guidance delivered" |
| "refused: {message}" | "refused: {message}" (verbatim; control stays downgraded) |
| "fleet quiet" / "nothing needs you right now" | "all quiet" / "nothing needs you right now" |
| "fleet unavailable — {reason}" | "can't read your runs — {reason}" |
| "reading the fleet" | "reading your runs…" |
| "supervisor running/stopped/unknown" | "service running/stopped/unknown" |
| "Open" / "Start Job" / "Quit" | "Open deadreckon" / "New Goal" / "Quit" |

#### A3.10 Settings (today `SettingsView.swift`)

| Old | New |
|---|---|
| Tabs "General / Notifications / Info" | unchanged |
| "Launch at login" + "Keep deadreckon in your menu bar so the fleet is watched and decisions reach you." | "…so your runs are watched and decisions reach you." |
| "Appearance" + "deadreckon follows the system appearance. Every color is a light/dark dynamic pair; there is no separate theme setting." | "Appearance" + "deadreckon is dark by design. There is no light mode." |
| "Notify me when a job needs attention" + detail | "Notify me when a run needs attention" + "Derived from the attention entries the CLI writes to `notify.jsonl`. Signals only — the app re-reads the files when you open it." |
| Reason rows (`AttentionDerivation.title`) | see A4 |
| Reason details ("A two-key receipt sealed; the result waits at the gate for your promote." etc.) | "Both sign-offs landed; the result waits for your approval." / "The run paused at a budget or time limit." / "The run waits for your review before it can continue." / "The service classified the run as blocked." / "…as failed." / "…as stopped." |
| macOS permission paragraph | unchanged in substance; "a grant there takes effect on the next notification, no relaunch needed." kept |
| "Notify tail trouble" + "Notifications from this job's current attempt are stopped: {r}" | "Notification trouble" + "Notifications from this run are stopped: {r}" |
| "Read-only facts" | unchanged |
| "Binary reports" + "Live deadreckon --version from the Harbor poll." | "CLI reports" + "Live `deadreckon --version` from the health poll." |
| "DEADRECKON_BIN override" + detail | unchanged (mono facts) |
| "Vendored CLI" rows / "No pinned hashes: run scripts/vendor-cli.sh to vendor a binary." | unchanged |
| "DEADRECKON_HOME" rows | unchanged |
| "Schema handshake" + detail | "CLI handshake" + "The bundled CLI has no schema-version report yet (registered gap); the health check is the honest signal until it lands." |
| "doctor ok/…" status values | "health OK" family per A2 |

#### A3.11 Command palette, menus, notifications, glyph

| Old | New |
|---|---|
| Palette placeholder "Filter by goal, id, or provider" | "Search runs — goal, id, or agent" |
| "No jobs in the fleet" / "No rows match" | "No runs yet" / "No matches" |
| "⏎ open · esc dismiss" | unchanged |
| Menu File > "New Job…" | "New Goal…" (⌘N) |
| Menu View > "Gate Queue" (⌘1) | "Overview" (⌘1) |
| Menu View > "Search Fleet" (⌘K) | "Search Runs…" (⌘K) |
| Menu "Job" | "Run" |
| Job > "Steer" | Run > "Guide…" (focuses the guide field) |
| Job > "Kill…" (⌘⌫) | Run > "Stop…" (⌘⌫) |
| Job > "Promote…" | Run > "Review & Approve…" |
| Job > "Open in Terminal" (⌘T) | unchanged |
| "About deadreckon" + credits "vendored CLI: {v}" | unchanged |
| Notification titles (Kit `AttentionDerivation`): "Verified, awaiting your promote" / "Needs your review" / "Paused at cap" / "Job blocked" / "Job failed" / "Job cancelled" | "Verified — ready to approve" / "Needs your review" / "Paused at a limit" / "Run blocked" / "Run failed" / "Run stopped" (Kit change, tests updated — the one permitted Kit edit, see §F) |
| Notification action "Review at Gate" | "Review & Approve" |
| Menubar glyphs helm/sailboat/anchor | diamond family per DESIGN.md §8 |
| Glyph help "deadreckon: fleet unavailable / N decisions waiting / N stale leases, supervisor stopped / N running / fleet quiet / reading the fleet" | "deadreckon: can't read runs" / "N need you" / "N runs quiet too long, service stopped" / "N running" / "all quiet" / "reading runs" |

### A4. Words that never change

`deadreckon`, all CLI verbs and flags in mono contexts, file names
(`acceptance.yaml`, `authority.json`, `job-events.jsonl`, `events.jsonl`,
`notify.jsonl`, `state.json`, `cancel.marker`, `supervisor.out/err`), ids, sha
digests, dollar figures, check kinds, judge decisions and summaries (verbatim,
quoted), refusal messages and try-lines, `try:` prefix, exit codes,
`DEADRECKON_HOME`/`DEADRECKON_BIN`, "Verified" as the proof word, and every
`next_actions` line from an envelope.

---

## B. INFORMATION ARCHITECTURE

### B1. Main window

`NavigationSplitView`-shaped two-pane window (min 980×620): left sidebar 240px on
`sidebarBg` with a 1px `border` seam; center on `windowBg`. The old
NavigationStack push (queue → detail) dies; selection in the sidebar swaps the center
in place (the current `FleetSidebarView` mechanic, promoted to the whole window). The
right evidence rail dies; its content becomes center tabs (§D — decision rationale:
the operator's complaint is an empty center; a 320px rail starves the center's
density, and the always-legible contract obligation is met by the header's contract
strip instead).

Sidebar, top to bottom:

1. **Header row (44px):** `diamond.fill` accent mark + "deadreckon" 13 semibold.
2. **[+ New Goal]** primary button, full-width, the window's one accent action.
3. **NEEDS YOU group** (only when non-empty): section header + accent count pill.
   Rows (40px): state dot, goal (13 medium, 1 line), state word 11 `textSecondary`
   ("Verified — ready to approve" / "Stopped for your review" / "Waiting — needs your
   input"). Sorted decision-first (existing queue order). Click = select run.
4. **Projects tree:** one group per `scope` (rollup fact), header = scope name 11
   semibold `textSecondary` + quiet run count, disclosure per project (expanded by
   default, state remembered). Rows (36px): state dot, goal 12.5, right-aligned quiet
   relative time. Runs sorted: live first, then needs-you, then recency. Terminal
   runs stay listed (they are the review inventory and history) — grouped at the
   bottom of each project after a hairline once they are approved/failed older than
   the top 5 (disclosure "12 finished ▸").
5. **Footer (fleet health, quiet):** dot + "All systems OK" or the worst degraded
   fact in plain words ("service stopped", "health: 2 warnings", "agents 1/2
   ready", "notifications broken for 1 run"); second line "updated 2m ago". Click
   opens Settings > Info. `unknown` states render as "unknown" with the reason in
   the tooltip — never a guessed count. Legacy runs line ("3 older CLI runs") lives
   in the tooltip.

Center: the selected run's detail (§D). No selection → **Overview** (§B2).
Command-K palette overlays the window as today (restyled per DESIGN.md; verbs
renamed). All sheets keep routing through `WriteSurfaceRouter`.

### B2. Overview (zero-selection center; replaces the Gate Queue home)

Answers outcome 1 in ten seconds. Vertical stack, max-width 760, centered:

1. Headline "Right now" (20) + Lexicon summary line (11 `textSecondary`).
2. **Needs you** — decision cards (56px rows in one bordered panel): dot, goal 13
   medium, state words + facts line (agent mark, `$x of $y`, "5/5 checks",
   updated), trailing [Review & Approve] / [Open] per state. Verified rows carry
   the Verified chip. Empty: the panel is simply absent.
3. **Running** — compact rows: accent dot (breathing), goal, "Working on {phase} ·
   turn N", spend, quiet [Open].
4. **Recently finished** — quiet rows (last 5 terminal non-decision runs): outcome
   word + time.
5. Footer line: health summary + "updated Ns ago".

### B3. Menubar popover (mirrors the tree's attention slice)

Width 360 on `sidebarBg`. Header: Lexicon summary. Sections NEEDS YOU (max 4) and
RUNNING (max 5) with the same row anatomy as the sidebar plus inline verbs
(A3.9); Guide expands inline exactly as today (eligibility lazy, refusal
downgrades). Footer: health line + [Open deadreckon] [New Goal] [Quit]. All
destructive verbs still open the window onto the full sheet; the popover never
fires one directly.

### B4. Empty and degraded app states

- **No runs at all (fresh install):** sidebar shows header + [+ New Goal] + health
  footer only. Center shows one inviting panel (max 480): display headline "Start
  your first goal", sentence "Pick a project folder, say what you want done and
  what done means, and watch the run work. Everything the agent does lands here as
  evidence.", primary [Choose a Project Folder…] (opens New Goal with the picker
  armed), and beneath a quiet mono well: `deadreckon start "your goal"` /
  `deadreckon list` with caption "The CLI works too — runs started there appear
  here."
- **Runs unavailable (binary missing / scan failed):** center banner panel: warn
  triangle, "Can't read your runs" (17), the reason verbatim (mono, selectable),
  "The CLI still works: `deadreckon list`". Sidebar shows header + New Goal
  (disabled when the binary is missing, with the reason as its help) + footer in
  its degraded words.
- **Loading:** skeleton rows (3 gray bars) in the sidebar, "Reading your runs…" in
  the footer. Never spinners mid-content.

---

## C. NEW GOAL FLOW (single scrollable sheet, 680×720)

Section order mirrors the CLI's decision order; everything visible, no hidden
steps. Sections are bordered panels with `label` headers; one primary per state.

1. **PROJECT (first).** Recent projects as a vertical radio list (up to 5; app-side
   MRU in UserDefaults, newest first, full path mono 11 with folder name 13
   medium) + [Choose Folder…]. The chosen path renders as a mono chip. Until a
   folder is chosen, later sections stay visible but the Preview action is
   disabled with the inline reason "choose a project folder first". Caption per
   A3.6 (copied-into-workspace honesty).
2. **GOAL.** Multiline editor (min 64px), autofocus once a project exists;
   placeholder "What should the agent accomplish?"
3. **AGENT & MODEL.** Radio rows from the live probe: plain display name 13
   medium + route id mono 10.5 + status chip ("ready" / "not available"). Failed
   probes visible-disabled with message + `try:` lines verbatim, exactly as today.
   Model picker beneath the selected agent: "Default for this agent" +
   catalog entries (mono ids, "recommended" tag).
4. **BUDGET.** "Up to $" field (mono), caption "Over $50, Start asks you to type
   the amount."
5. **WHAT DOES DONE MEAN?** If the preview (or a prior declare) resolved a
   contract: read-only check rows (kind mono, target mono middle-truncated, "must
   pass" chip), network line, source file fact, [Rewrite…]. Otherwise: the
   plain-English field + [Draft checks] + the honesty caption + drafting/refusal
   states — the existing def-done editor restyled and re-labeled (A3.6). The
   drafted checks preview renders inline immediately (verbatim envelope rows).
6. **PREVIEW & START.** [Preview the plan] (standard button; disabled until
   project + goal). Result panel in plain language, one fact per line:
   - "Run in" {folder, mono}
   - "Agent" {plain name} · model {id or default} — source noted verbatim
   - "Budget" up to $N (or "agent default")
   - "Done means" {criteria} · N checks · network {word}
   - Refusals/blocked plans render as refusal cards with the fix inline (the
     def-done editor opens right here when the missing piece is the done
     definition — existing behavior, restyled).
   - Disclosure "Command ▸" → command well with the exact `start --plan … --yes
     --json` line (and the preview's own line above it while previewing).
   Footer: typed-amount confirm field when required (stroke warn→success on
   match), then **[Start Run]** — the sheet's primary; enabled only when the
   envelope says launchable (`isLaunchable`) and acknowledgment passes. Success
   line per A3.6; the run appears only when its files land (never optimistic).

State-machine wiring is unchanged (`LayCourseController`, `LayCourseCatalog`,
`SpendAcknowledgement`); this is presentation + order + words.

---

## D. RUN DETAIL (dense center)

Vertical composition, all full-width, seams between every band:

1. **Header (~72px, `windowBg`):**
   - Row 1: goal 17 semibold (2-line max) — right: contextual actions. Terminal
     run: [Review & Approve] primary + [Send back…] standard; live run: [Stop…]
     quiet-danger; waiting-needs-input: nothing extra (guide bar is the tool).
     Overflow "⋯" menu: Open in Terminal (⌘T), Copy run id, Copy path.
   - Row 2 (11pt): state chip (plain phase/outcome word) · Verified/Proof-invalid
     chip when present · agent mark+name · run id mono (middle-truncated,
     selectable) · project scope · live dot + "12s" · "active 2.1h" ·
     "$4.12 of $25.00" (monospaced digits) · "5/5 checks" chip when present.
     Degradations inline: "spend feed stopped" warn chip with verbatim tooltip.
2. **PLAN band (~64px, `panel`):** label "PLAN". Horizontal step strip from
   `runState.phases` (names verbatim, mono-numbered): each step = dot + name 11;
   completed `success` check-dot, current `accent` breathing + name 11 semibold
   `textPrimary`, planned `textTertiary`, failed `danger` x-dot. Overflow
   scrolls horizontally. Fallback when no pipeline state: the five-stage
   lifecycle strip derived from `projection.phase` (Queued → Working → Checks →
   Judge → Review) with the same grammar and the caption "The run hasn't posted
   its plan yet." Right edge: "attempt N" quiet mono when > 1.
3. **NOW band (~56px, `windowBg`):** four labeled cells (label + 12.5 value):
   WORKING ON (spine.doing per A1) · ON TRACK ($ of $, elapsed, turn) ·
   ATTENTION ("Nothing flagged" or "2 flagged — {first}" warn) · LAST ACTIVITY
   ("5s ago" live / "Quiet for 45s" warn). Beneath, one quiet mono line:
   `CLI  deadreckon attach r-…` (the spine's next command, verbatim).
4. **DONE MEANS strip (~40px, `panel`):** always visible — outcome 4's anchor.
   "Done means:" label + criteria one-liner (13, truncated with tooltip) +
   rollup chips: "N checks", "M passed" (success when all), digest chip
   ("matches the approved version" / "CHANGED SINCE APPROVAL"), network word.
   Click anywhere → Checks tab. When no contract resolves: "No definition of
   done recorded — the CLI's preview would refuse to start this today" in warn.
5. **Evidence tabs (fill remaining height):** Activity · Story · Changes ·
   Checks · Docs · Recorder.
   - **Activity:** [Stream | Turns] toggle left of the search field. Stream =
     today's unbounded scrollback (search, pinned-tail). Turns = today's
     TurnsPane grouping. Header right: the activity sparkbar (per-turn entry
     counts, `border`-colored bars, current turn `accent`) + "N events".
   - **Story:** deterministic narrative body first-class; AI-summary overlay
     card per A3.3 (warn-bordered, labeled, never near decisions).
   - **Changes:** today's diffstat + on-demand patches, restyled (A/M/D glyphs,
     +/− aligned mono columns).
   - **Checks:** first-class rows merging today's Contract+Live+Receipt bands:
     each frozen check = row [state glyph ✓/✗/◌/○] kind mono + subject +
     "must pass" chip + duration + expandable clipped output; sections
     "CHECKS RUNNING NOW" (streaming, advisory-labeled) and "RECORDED CHECK
     RESULTS"; the TWO SIGN-OFFS block on top when terminal ("Verified by the
     harness gate" phrasing, judge quote verbatim); proof chip; report
     degradations verbatim.
   - **Docs:** run documents list (name mono, size, opens in editor).
   - **Recorder:** manifest facts + checkpoint cards ("full snapshot" chip,
     turn/trigger/files facts, disabled Rewind… with the honest help).
6. **Guide bar (36px, `windowBg`, seam above):** label "GUIDE" + single-line
   input "Guide the agent…" + [Send] (standard button; disabled per
   eligibility exactly as today, refusal downgrades) + status line beneath when
   active (queued/delivered/refused per A3.2). Hidden entirely for terminal
   runs (the header's review actions take over). ⌘G / Run > Guide… focuses it.
7. **Console drawer (collapsed 28px bar, `panel`):** "CONSOLE ▴" toggle +
   integrity chip + partial-line chip always visible; expanded 180px with tabs
   Terminal (supervisor.out/err split) · Raw events · Run log — content
   unchanged, restyled monoS on `well` wells.

Unreadable (quarantined) entry: centered panel with the A3.2 words, reason
verbatim mono — unchanged behavior.

---

## E. SCREEN INVENTORY (ASCII sketches)

Legend: `#` seam/border, `[Btn]` standard, `[[Btn]]` accent primary, `((chip))`,
`*` accent dot, `o` state dot.

### E1. Main window — Overview selected

```
+--------------------------------------------------------------------------+
| ◆ deadreckon        #  Right now                                         |
| [[ + New Goal ]]    #  2 ready to approve · 1 needs review · 3 running   |
|#####################|                                                    |
| NEEDS YOU      (3)  |  NEEDS YOU                                        |
| o Fix flaky auth    |  +----------------------------------------------+ |
|   Verified—approve  |  | o Fix flaky auth tests      ((Verified))     | |
| o Migrate billing   |  |   claude · $4.12 of $25 · 5/5 checks · 2m    | |
|   Stopped—review    |  |                       [Review & Approve]     | |
| o Bump deps         |  +----------------------------------------------+ |
|   Waiting—input     |  | o Migrate billing store   Stopped for review | |
|#####################|  |   codex · $12.40 of $20 · 1h   [Open]        | |
| deadreckon (proj)   |  +----------------------------------------------+ |
|  * Ship ledger  2m  |                                                    |
|  o Fuzz tailer  1h  |  RUNNING                                          |
|  12 finished ▸      |  | * Ship the ledger   Working on implement ·   | |
| billing (proj)      |  |   turn 7 · $3.20                    [Open]   | |
|  * Port webhooks 5m |                                                    |
|#####################|  RECENTLY FINISHED                                 |
| ● All systems OK    |  |  Approved · Bump lockfile · yesterday        | |
|   updated 12s ago   |                                                    |
+--------------------------------------------------------------------------+
```

### E2. Run detail

```
+---------------------------- center ----------------------------------+
| Ship the durable ledger rewrite            [Send back…] [[Review &   |
| ((Working)) ((Verified? no)) ▸C claude · r-8f2… · deadreckon ·      |    Approve]]
| *12s · active 2.1h · $4.12 of $25.00 · ((5/5 checks))               |
|######################################################################|
| PLAN   ✓ plan   ✓ scaffold   * implement   · test   · docs   att 2  |
|######################################################################|
| WORKING ON          ON TRACK             ATTENTION      LAST ACTIVITY|
| implement · turn 7  $4.12/$25 · 2.1h     Nothing flagged   5s ago    |
|   CLI  deadreckon attach r-8f2…                                      |
|######################################################################|
| Done means: builds, tests pass, no console errors                    |
|   ((5 checks)) ((4 passed)) ((matches approved)) network: deny  →    |
|######################################################################|
| Activity  Story  Changes 12  Checks  Docs  Recorder                  |
|#---------------------------------------------------------------------|
| [Stream|Turns]  Search the whole scrollback…      ▂▄▆▂▁ 1,204 events |
| 14:02:11  tool_call Edit src/ledger.rs                               |
| 14:02:13  tool_result ok (412ms)                                     |
| …                                                            (tail)  |
|######################################################################|
| GUIDE  [ Guide the agent — read at the next turn…        ]  [Send]   |
|######################################################################|
| CONSOLE ▾                       ((ledger intact)) ((partial line))   |
+----------------------------------------------------------------------+
```

### E3. New Goal sheet

```
+----------------------------- New Goal (680) --------------------------+
| New Goal                                                    [Cancel] |
| Everything is visible before anything runs.                          |
|######################################################################|
| PROJECT                                                              |
|  (•) ~/code/deadreckon        (recent)                               |
|  ( ) ~/code/billing           (recent)                               |
|  [Choose Folder…]   ~/code/deadreckon                                |
|  The folder is copied into the run's workspace before launch…        |
| GOAL                                                                 |
|  [ What should the agent accomplish?                        ]        |
| AGENT & MODEL                                                        |
|  (•) ▸C Claude Code   claude-code   ((ready))                        |
|  ( ) ▸O Codex CLI     codex         ((not available))                |
|        error message verbatim…    try: `codex login`                 |
|  Model  [ Default for this agent ▾ ]                                 |
| BUDGET                                                               |
|  Up to $[ 25.00 ]   Over $50, Start asks you to type the amount.     |
| WHAT DOES DONE MEAN?                                                 |
|  check  cargo-test ./…            ((must pass))                      |
|  check  no-console-errors                                            |
|  written to .deadreckon/acceptance.yaml     [Rewrite…]               |
| PREVIEW & START                                       [Preview plan] |
|  Run in    ~/code/deadreckon                                         |
|  Agent     Claude Code · model default                               |
|  Budget    up to $25.00                                              |
|  Done means  builds and tests pass · 5 checks · network deny         |
|  Command ▸  deadreckon start "…" --from … --plan … --yes --json      |
|######################################################################|
|                Budget over $50 — type 60.00 to confirm: [    ]       |
|                                                     [[ Start Run ]]  |
+----------------------------------------------------------------------+
```

### E4. Review & Approve sheet

```
+------------------------ Review & Approve (680) ----------------------+
| Review & Approve                ((Verified))              [Cancel]   |
| Ship the durable ledger rewrite · r-8f2…                             |
|######################################################################|
| TWO SIGN-OFFS            from the recorded report                    |
|  ✓ Checks passed — signed record   status completed · sandboxed      |
|  ✓ Judge's call — achieved                                           |
|    "The ledger rewrite satisfies the acceptance criteria…"           |
| WHAT DONE MEANS — frozen acceptance.yaml                             |
|  sha256 9f31ab20cc41…  ((matches the approved version)) net: deny    |
|  ✓ cargo-test   ./crates/…      ((must pass))   4.1s   [output ▸]    |
|  ✗ no-warnings  build log                        0.9s   [output ▸]   |
| PROOF                                                                |
|  ((Verified)) ((5/5 checks))                                         |
|  Approving re-validates the proof before and after the files move;   |
|  any drift refuses — there is no override, by design.                |
| RESULT PREVIEW — nothing moves yet          [Refresh preview]        |
|  12 files · +340 −80                                                 |
|  src/ledger.rs                              4.2 KB  9f31ab20         |
|  IRREVERSIBLE: none                                                  |
| WHERE THE RESULT GOES                                                |
|  (•) Apply to the project   undoable afterwards: deadreckon undo     |
|      [x] --autostash   [ ] --cleanup                                 |
|  ( ) Export to a folder   --dest [ /path/to/export        ]          |
|######################################################################|
|  deadreckon finish r-8f2… --yes --json                               |
|  [Send back…]  [Stop…]                            [[ Approve ]]      |
+----------------------------------------------------------------------+
```

### E5. Stop confirm

```
+--------------------------- Stop (520) -------------------------------+
| Stop this run?                                                       |
| Ship the durable ledger rewrite · r-8f2…                             |
|######################################################################|
|  1  A cancel request is written to the run's ledger (sticky) and     |
|     cancel.marker is written.                                        |
|  2  The service sends SIGTERM to the run's process groups.           |
|  3  2 seconds of grace, then SIGKILL.                                |
|  4  The service records the final Stopped event only after proven    |
|     cleanup.                                                         |
|  This sheet finishes only when that final event lands in             |
|  job-events.jsonl — never on an exit code.                           |
|                                                                      |
|  [ ] Also force-stop child processes (--escalate)                    |
|      a separate, explicit choice — not the default                   |
|  deadreckon kill r-8f2… --json                                       |
|  ((stop requested)) signal SIGTERM · 3 processes                     |
|  waiting for the service's final event in the run log…               |
|######################################################################|
|                                    [Cancel]      [ Stop Run ]        |
+----------------------------------------------------------------------+
```

### E6. Menubar popover

```
+----------- 360 ------------+
| 2 need you · 3 running     |
|############################|
| NEEDS YOU                  |
| o Fix flaky auth ((Verif.))|
|   [Review & Approve…][Open]|
| o Migrate billing  Stopped |
|   [Send back…] [Open]      |
| RUNNING                    |
| * Ship ledger  $4.12  t7   |
|   [Guide…] [Stop…] [Open]  |
|   [ Guide the agent… ][Send]|
|############################|
| ● service running · 12s ago|
| [Open deadreckon][New Goal]|
|                     [Quit] |
+----------------------------+
```

### E7. Settings

```
+--------------------- Settings (620×460) -----------------------------+
| Settings                                                             |
| [ General | Notifications | Info ]                                   |
| +------------------------------------------------------------------+ |
| | STARTUP                                                          | |
| |  Launch at login                                    [switch]     | |
| |  Keep deadreckon in your menu bar so your runs are watched…      | |
| |##################################################################| |
| | APPEARANCE                                                       | |
| |  deadreckon is dark by design. There is no light mode.           | |
| +------------------------------------------------------------------+ |
+----------------------------------------------------------------------+
```

### E8. Empty app / degraded

```
Empty:                              Unavailable:
+----------- center ----------+     +----------- center ----------+
|                             |     |  ⚠ Can't read your runs     |
|   Start your first goal     |     |  {reason, verbatim mono}    |
|   Pick a project folder,    |     |  The CLI still works:       |
|   say what you want done…   |     |  `deadreckon list`          |
|  [[Choose a Project Folder]]|     +-----------------------------+
|  ┌ deadreckon start "goal" ┐|
|  └ deadreckon list         ┘|
+-----------------------------+
```

---

## F. IMPLEMENTATION PLAN — two implementers, sequential, no collisions

Implementer 1 lands the world and the words IN PLACE (no file moves, no structural
changes). Implementer 2 then restructures views on top of the finished world.
Phase 2 starts only after Phase 1's gate passes. Kit is frozen except the one
named change (F1.6). `project.yml`/xcodegen regeneration happens only in Phase 2
(new/renamed files).

### Phase 1 — Implementer 1: theme + nomenclature sweep

1. `Sources/Views/Theme.swift` — REWRITE to DESIGN.md: dark-only constants (delete
   `dynamicColor` and every light value), the surface ladder, text tokens, accent
   trio, semantic set incl. `dangerText`/`dangerFill`; delete `warnFill`,
   `verifiedFill`, `onFill`, `scrim` light value, serif `display()`; new type
   helpers (`title/heading/base/small/caption/label/mono*`); button styles
   (standard/primary/dangerConfirm/quietDanger/text) replacing Tactile spring-scale
   with the 120ms opacity press; `StatusChip` → bordered chip per DESIGN.md §5
   (drop `filled:` API, add `strong:`); `CardBackground` → 1px border, no shadow;
   count-badge view; focus-ring modifier; provider colors desaturated (single hex
   each).
2. `Sources/DeadreckonApp.swift` — force `NSApp.appearance = .darkAqua` in
   `applicationDidFinishLaunching`; `MenuBarGlyph` diamond family + help words
   (A3.11).
3. NEW `Sources/Views/Lexicon.swift` — every UI word in this spec: section titles,
   phase/outcome/stop-reason/proof display words, chip words, summary-line
   builder from `GateQueue` counts, spine formatting from `SpineSnapshot` fields,
   agent display names, health words. Views may not inline user-facing literals
   that this file covers.
4. String + token sweep, in place, every file (labels only — no layout moves):
   `GateQueueView.swift`, `JobDetailView.swift`, `DetailCenterTabs.swift`,
   `EvidenceRail.swift`, `DetailDrawer.swift`, `LayCourseSheet.swift`,
   `PromoteSheet.swift`, `KillSheet.swift`, `MenuBarPopover.swift`,
   `SettingsView.swift`, `CommandPalette.swift`, `WriteSurfaceRouter.swift`
   (RefusalView/CommandLineView restyle), `AppCommands.swift` (menu titles per
   A3.11, `CommandMenu("Run")`).
5. Delete every filled-chip call site (`filled: true`) in favor of strong chips;
   replace `Theme.display(...)` call sites with the new scale.
6. The ONE Kit change: `AttentionDerivation` notification titles + action word
   (A3.11) and matching `AttentionCenterTests` expectations. Nothing else in Kit
   moves; `GlossaryText`, `QueueSection.title`, `SpineSnapshot.*Text` stay
   byte-identical (their strings remain for tests/CLI parity; UI reads Lexicon).
7. **Gate:** app builds; `swift test` in DeadreckonKit green; manual pass: every
   surface dark with 1px borders, no serif, no light-mode flash, no nautical or
   Job/Kill/Promote/Steer words anywhere visible, popover/sheets/palette/settings
   all in the new world.

### Phase 2 — Implementer 2: structural views

Order within the phase is dependency order; each step leaves the app buildable.

1. NEW `Sources/Views/SidebarView.swift` — projects tree (group by `row.scope`),
   NEEDS YOU group, count pill, health footer (consumes FleetStore.harbor +
   AttentionCenter), [+ New Goal].
2. NEW `Sources/Views/OverviewView.swift` (§B2) — reuses row atoms; retire
   `QueueRowView`/`QueueSectionHeader`/`HarborChips`/`HarborStripView` as their
   content moves in.
3. `GateQueueView.swift` → REWRITE as `MainWindowView.swift` — split layout,
   selection state (replaces NavigationStack path), keeps: ShellModel request
   consumption (openJob/reviewAtGate/search/focusSteer → selection + sheets),
   sheet routing, palette overlay, Escape layering, hidden shortcut buttons.
   `shell.openedItem` now publishes the sidebar selection.
4. `JobDetailView.swift` → `RunDetailView.swift` — drop the inner
   `FleetSidebarView` (window sidebar owns navigation); build header (contextual
   actions per §D1), PLAN band (runState.phases + projection fallback), NOW band
   (Lexicon spine), DONE MEANS strip (report.contract + row.gate + digest),
   guide bar (rename SteerBarView → GuideBar internally; keep
   SteerCoordinator/notification focus seam), keep WindowVisibilityObserver
   lifecycle exactly.
5. `DetailCenterTabs.swift` — six tabs; move `ContractChecksView` (as ChecksTab),
   `ChangesView`, `FlightView` (RecorderTab), `DocsView` in from
   `EvidenceRail.swift`; add Stream|Turns toggle + sparkbar to Activity; delete
   TimelinePane (band absorbed it); DELETE `EvidenceRail.swift`.
6. `LayCourseSheet.swift` → `NewGoalSheet.swift` — §C order, recents MRU
   (UserDefaults key `recentProjects`, capped 5, written on successful start),
   command-line disclosure, single primary. Controllers unchanged.
7. `PromoteSheet.swift` → `ReviewApproveSheet.swift` (+`SendBackSheet` restyle in
   file) — §E4 composition; gate logic (`PromoteGate`, live-row freshness,
   destination/preview coordination) untouched.
8. `KillSheet.swift` → `StopSheet.swift` — §E5; `KillCoordinator` untouched.
9. `MenuBarPopover.swift` — §B3/E6 composition on the shared row atoms.
10. `WriteSurfaceRouter.swift` — rename surface case `layCourse` → `newGoal`
    (mechanical), update call sites.
11. `AppCommands.swift` — wire ⌘1 Overview (clear selection), ⌘G guide focus.
12. `SettingsView.swift` — bordered tab row + panel styling only (content landed
    in Phase 1).
13. `project.yml` — regenerate project for renamed/new files (xcodegen), no
    target changes.
14. **Gate:** build; Kit tests green (no Kit edits this phase); operator
    acceptance script: (1) fresh-home empty state invites New Goal; (2) New Goal
    project-first flow starts a run that appears from files; (3) sidebar shows
    project→runs with needs-you pinned and badged; (4) run detail answers the
    five outcomes without opening the drawer; (5) refusal (missing done
    definition) renders plainly with the inline editor; (6) Review & Approve
    approves a verified run, refuses on tamper, no override anywhere; (7) Stop
    resolves only on the run-log event; (8) popover mirrors needs-you/running
    and never fires a destructive verb directly; (9) `rg -n
    "Lay Course|Gate Queue|Harbor|Rudder|Binnacle|Pennant|moored|wrecked|
    voyage|fleet|Promote|Kill\\b|Steer" Sources/Views` returns only mono/CLI
    quotations and comments.

### Explicit non-goals of this redesign

App icon redraw (current anchor asset stays until a brand pass), any new binary
verbs (Discard, rewind, fleet-wide spend), light mode, onboarding tours, and any
change to trust/write flows beyond words and layout.
