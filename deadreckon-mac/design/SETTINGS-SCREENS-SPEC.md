# SETTINGS-SCREENS-SPEC.md — settings architecture, remaining screens, app icon

Companion to `/DESIGN.md` (visual constitution — tokens and component rules live there)
and `design/REDESIGN-SPEC.md` (nomenclature + component grammar, normative). Scope
discipline is `/Users/gdc/deadreckon/docs/design/FULL-DRIVE-PROGRAM.md` (the drivability
matrix): deferred-by-design stays deferred; a power verb earns a screen only when a
daily/weekly operator benefits.

Operator bar (2026-08-07): every deadreckon setting exposed **with taste**; see all the
binaries; every other command that makes sense as a screen, built — extremely tasteful.

Ground rules, restated as law:

1. **The app writes nothing under `DEADRECKON_HOME`.** Every mutation in this spec is a
   CLI envelope dispatched through `MutationRunner`/`PlannedVerb` (the closed enum — no
   dr-gate case, no sign case, no force case, ever). Every write surface shows its
   command well. File truth is re-read after every write; optimistic UI is forbidden.
2. **PENDING envelopes decode fail-closed.** Six envelopes in §P are being built
   concurrently on the Rust side. Every field marked `PENDING` is a best guess to be
   reconciled against the landed Rust shape before any Swift decoder is written. An
   undecodable envelope renders the established pattern verbatim: "The CLI answered
   without a machine envelope (exit {n}); it said:" — never a guessed state.
3. **Capability probes, not hardcoded gap labels.** FULL-DRIVE finding 6: hardcoded
   "registered gap" strings drift. Every surface gated on a pending envelope probes at
   open (one cheap `--json` invocation, decode-or-degrade) and renders the degraded
   state with the CLI escape hatch named. When the envelope lands, the surface arms
   itself with zero label edits.
4. **Secrets never touch argv, logs, or the screen.** The one redaction rule
   (FULL-DRIVE trust posture): API keys travel over stdin, appear in no command well,
   no transcript, no error echo, and are never read back. The UI says "configured" or
   "not configured" — never the secret, never a last-4.

Live corroboration for this spec ran the vendored `Resources/bin/deadreckon_darwin_arm64`
(0.8.4) read-only against scratch homes on 2026-08-07. Facts cited as (live) below were
observed, not inferred.

---

## S. SETTINGS ARCHITECTURE

### S0. Window

A real macOS settings window: the existing `Settings {}` scene (⌘,) rebuilt from the
segmented-tab card into a two-pane fixed window, **840×600**.

- **Sidebar (200px, `sidebarBg`, 1px `border` seam):** six rows, 32px each, radius 6:
  SF Symbol 13pt `.medium` + label 13 medium. Selection = `well` fill + `borderHover`
  per DESIGN.md row rules; accent is never selection.

  | Row | Symbol | Content |
  |---|---|---|
  | General | `gearshape` | Defaults every new goal inherits |
  | Agents & Keys | `person.text.rectangle` | Provider routes, probes, models, API keys |
  | Service | `arrow.triangle.2.circlepath` | The background service (CLI: `supervisor`) |
  | Notifications | `bell` | Existing prefs, restyled |
  | Binaries | `shippingbox` | Every binary: vendored, gate helper, installed |
  | Health | `stethoscope` | Doctor findings + repair |

- **Content pane (`windowBg`):** scrolling; section content in bordered `panel` cards,
  internal padding 16, `label`-type group headers in `textSecondary` (scan-first).
  Info-row grammar reuses today's `infoRow` (170px label column, mono values,
  selectable, middle-truncated).
- **Deep link:** `@AppStorage("settings.section")` holds the selection. The sidebar
  health footer sets it to Health before `openSettings()`; notification-permission
  copy can set Notifications. No other navigation state.
- The current Info tab dissolves: app/vendored/handshake/`DEADRECKON_HOME` facts move
  to **Binaries**; nothing is lost (simplify, never remove).
- Naming note for the operator: the brief said "Runner service"; this spec titles the
  row **Service** because the committed lexicon (REDESIGN §A0) already renders
  `supervisor` as "service" everywhere else ("service running", "service stopped").
  One word, one meaning. Veto point if "Runner service" is wanted verbatim.

### S1. General — defaults every new goal inherits

Plain words, with the `config.toml` key in mono beneath each label — the operator's
exact ask. Values are file truth from `config show --json` (PENDING §P1); every control
dispatches `config set`/`config unset` (PENDING §P2) and re-reads. While a write is in
flight the row shows "writing…" in `textTertiary`; failures render the refusal card
inline, verbatim.

Rows (label 13 medium / key `mono` 10.5 `textTertiary` beneath / control right-aligned):

| Label | Key (mono, beneath) | Control | Notes |
|---|---|---|---|
| Agent | `defaults.provider` | popup: configured routes (plain name + route id mono) + "No default" | routes from `providers list --json` (live, real) |
| Model | `defaults.model` | popup: catalog for the chosen agent + "Agent's default" | catalog from `models --json` (live, real); "recommended" text tag |
| Spend cap | `defaults.max_spend` | `$` field, mono digits | caption "New runs start with this budget unless the goal sets one. $10 when unset." |
| Time limit | `defaults.cli_max_wall_seconds` | duration field with unit word (hours) | caption "10h (36,000s) when unset." — value stored in seconds; the field converts, the key line shows the raw seconds |
| Sandbox | `defaults.sandbox` | popup: backends from doctor's `sandboxes` | unavailable backends visible-disabled with the doctor note verbatim; `none` always carries its own note verbatim: "available but unsafe; use only when explicitly requested" |
| Prevent sleep | `defaults.prevent_sleep` | popup auto / on / off | daily-relevant: overnight runs; caption "Keeps the Mac awake while a run works (`caffeinate`)." |

Beneath the rows, one shared command well shows the **last dispatched** config command
and its verdict word (e.g. `deadreckon config set defaults.max_spend 15 --json` ·
"completed"). Each row's tooltip carries its exact command — quiet provenance, no
per-row wells.

**ADVANCED — the whole file** (disclosure, collapsed): the raw `config.toml` rendered
in a `monoS` well, selectable, with [Reveal in Finder] and caption "Everything the CLI
reads, verbatim. Keys not surfaced above (`defaults.doc_*`, `defaults.motion_policy`,
`defaults.plain`, `defaults.start_attach`, …) are TUI- or doc-pipeline-scoped and are
edited with `deadreckon config set` in Terminal." This is how *every* setting is
exposed with taste: six controls that matter daily, the full file honestly, zero noise.

Reviewer-role keys (`defaults.reviewer_provider`, `defaults.reviewer_model`) live in
Agents & Keys §S2 (they are agent routing, not launch defaults).

**Degraded (older binary, config envelopes absent):** the section's capability probe
(`config show --json`, decode-or-degrade) fails → controls render as read-only value
rows (from the raw file well) + one banner line: "This CLI version has no machine
envelopes for config yet — edit with `deadreckon config set KEY VALUE` in Terminal."
Copy affordance on the command. Never parse the prose verdicts of the text-only
`config get`/`set` (live: they print "completed config …" prose — not a protocol).

### S2. Agents & Keys

One bordered row-card per route from `providers list --json` + `models --json` (both
real today). Card anatomy, top row: provider mark tile (12–16px, per-agent color per
DESIGN §2) + plain display name 13 medium (`Lexicon.agentName`; binary
`display_name` wins) + route id `mono` 10.5 + status chip ("ready" / "not available" /
"skipped") + right: [Test] standard compact button.

- **[Test]** re-runs the probe surface (`providers list --json`) and re-renders this
  card from the fresh envelope. Failed probes render exactly as New Goal does: message
  verbatim in mono + `try:` fix lines as mono accent text-buttons. `detect --ping`
  stays CLI (deeper transport probe; weekly at most — one line in §R6).
- **Model default** row: popup of the route's catalog + "Agent's default"; writes
  `config model <id> --provider <route>` (text verb today; PENDING envelope rides §P2's
  `config set` grammar — reconcile which verb the Rust side arms).
- **Key state** row, per route kind:
  - Subscription CLI routes (`cli:claude-code`, `cli:codex`, …): no key UI. Caption:
    "Signs in with its own CLI — no key stored here." + the probe's own status words.
  - API-key routes (`api:…`): chip "configured" (neutral) or "not configured"
    (`textTertiary`) — from the PENDING `config show` redacted boolean (§P1) —
    followed by a SecureField "Paste API key…" + [Save key] standard button.
    Save dispatches `config set-key <route> --json` with the secret **over stdin**
    (§P3); on the success envelope the field clears and the chip flips. [Remove
    key…] quiet-danger, confirm sheet, dispatches the unset form (§P3 open question).
    Beneath, the one redaction rule made visible — command well shows
    `deadreckon config set-key api:anthropic --json` + caption "The key travels over
    stdin — it never appears in the command line, in logs, or in this window again."
- **ROLES** group (one card, after the routes): Reviewer agent
  (`defaults.reviewer_provider`) + Reviewer model (`defaults.reviewer_model`) popups —
  same grammar as §S1 rows. Caption: "The judge's sign-off runs on this route when
  set; the main agent's route otherwise."

Empty state (no routes configured): "No agents configured yet." + sentence "Pick an
agent in the first-run panel, or configure one in Terminal." + command well
`deadreckon config provider cli:claude-code`.

### S3. Service — the background service

The supervisor, rendered as **one honest verdict word + evidence**, with lifecycle
buttons. All states from `supervisor status --json` (PENDING two-source shape §P4;
live today: the healthy path is JSON but the checkpoint-absent refusal is prose on
stderr — the degraded rendering below covers exactly that until the envelope lands).

Layout, top to bottom:

1. **Verdict row:** state dot + one word, 15 semibold: Running (`success`) / Stopped
   (`warn`) / Not installed (`warn`) / Outdated (`warn`) / Running for a different
   home (`warn`) / Unsupported (`danger`) / Unknown (`textSecondary`, reason in
   tooltip). Words extend `Lexicon.serviceWord`; the foreign-home state is the typed
   state FULL-DRIVE B5a demands.
2. **Evidence disclosure ("Evidence ▸", open by default):** the envelope's
   `verdict.evidence` pairs verbatim in `monoS` rows — service-manager state, pid,
   launchd label, plist path, checkpoint age/generation/boot id, bound binary path.
   Two sources, both quoted; the app never averages them into a guess.
3. **Buttons by state:** Not installed/Outdated → [Install service…] (the section's
   primary, accent). Stopped → [Start service…] primary + [Install service…]
   standard (re-install/update). Running → [Stop service…] quiet-danger. Unknown →
   no buttons; the evidence and the escape hatch are the surface.
4. **Caption:** "The service claims queued runs, supervises them, and records proven
   cleanup. The CLI calls it the `supervisor`. Plists are written only by the CLI —
   the app never touches launchd files." (User-scope LaunchAgent only; unmanaged-unit
   refusals render verbatim — FULL-DRIVE trust posture.)

**Confirm sheets** (each shows exactly what will happen + the command well; resolution
only from the returned envelope, then a fresh `status` poll):

- Install (520): "Install the background service?" — body: "The CLI writes a per-user
  LaunchAgent that starts at login and supervises your runs. Nothing system-wide."
  Command well `deadreckon supervisor install --json`. Primary [Install].
- Start (520): same grammar, `supervisor start --json`, primary [Start].
- Stop (520): "Stop the background service?" — body: "Running goals lose their
  supervisor: no new runs start, crashed runs stay down, and cleanup recording pauses
  until it starts again. Runs themselves are durable — nothing is deleted." Typed
  confirm: type `stop` to arm (stroke `warn`→`success` on match — the typed-confirm
  input grammar). Destructive confirm fill [Stop Service]. The typed word is spent
  here and nowhere else in Settings: install/start are constructive and get plain
  confirms; stopping silently degrades every durable run, which is the severity the
  ceremony exists for.

**Degraded (older binary / prose refusal):** verdict "Unknown" + the stderr words
verbatim in mono + "check `deadreckon supervisor status` in Terminal" + copy
affordance. Never a guessed count, never a guessed state.

### S4. Notifications

Existing content, restyled into the section card — behavior unchanged (master toggle,
per-reason toggles from `AttentionPreferences`, macOS-permission paragraph, and the
per-run "Notification trouble" degradation list). No new mechanics. The one addition:
the section header sentence "Signals only — the app re-reads the files when you open
it." stays, verbatim from today.

### S5. Binaries — THE operator ask: see all the binaries

Four groups of info rows + warnings. Sources: `Resources/bin/manifest.json` (real),
`BinaryLocator` verification state (real), `doctor --json`'s `binary_health` block
(real — live-corroborated fields: `installations[]{canonical_path, channel, version,
sha256, roles, locations, update_command}`, `conflicts[]`, `current_path`,
`current_version`, `path_selected`, `gate_helper_path`, `gate_helper_compatible`,
`gate_protocol_version`, `bundle_build_id`), and `fleet.binaryVersion` (the live
`--version` poll).

1. **THIS APP** — App version (CFBundle short + build).
2. **VENDORED CLI** (what the app executes):
   - `deadreckon_darwin_arm64` — version (`manifest.cliVersion`), commit
     (`gitCommit`, mono), path inside the bundle (mono, middle-truncated),
     [Reveal in Finder].
   - sha256 row: the pinned manifest hash (mono) + verification chip: "verified at
     launch" (success chip; BinaryLocator hashed the bytes against the pin) or the
     `BinaryLocatorError` words verbatim as a danger row (checksum mismatch renders
     both hashes — the error already carries them).
   - "CLI reports" — live `deadreckon --version` from the health poll (existing row).
   - `DEADRECKON_BIN` override row when the env var is set (existing row, kept):
     "Dev override in effect: manifest verification is skipped for this binary."
     (warn tint — an unverified binary is a fact worth a color).
   - `dr-gate` — the gate helper: pinned `gateSha256` (mono), path, [Reveal in
     Finder], plus from doctor: "gate protocol v{n}" + compatibility chip
     ("compatible" success / "incompatible" danger). Caption: "The app never invokes
     dr-gate; it is the harness's proof signer. Shown because you should be able to
     see every binary you're trusting."
3. **INSTALLED CLIS** (what the rest of the machine runs) — one row-card per
   `binary_health.installations[]` entry: channel chip (`shell` / `brew` / `source`,
   neutral) + version + canonical path mono + role facts in plain words ("on PATH",
   "PATH selects this one", "current process") + sha256 (mono, truncated, full on
   selection) + `update_command` in a command well (copyable — this IS the guided
   self-update handoff, §R6) + [Reveal in Finder].
4. **MISMATCH WARNINGS** — `binary_health.conflicts[]` verbatim, one warn row each
   (live example: "PATH selects /opt/homebrew/bin/deadreckon, but this process is
   running …/Resources/bin/deadreckon_darwin_arm64"; version skew 0.8.1 vs 0.8.4;
   missing install receipt). The binary already writes these sentences; the app
   quotes, never paraphrases. Zero conflicts → the group is absent (no "all clear"
   theater).
5. **HANDSHAKE & HOME** — CLI handshake status row (existing; the honest "no
   schema-version report yet" caption stays until the binary grows one) +
   `DEADRECKON_HOME` row with its source caption (existing).

Refresh: the group header carries "from the health check, {relative time}" + a quiet
[Refresh] that re-runs `doctor --json`. Binaries never blocks on doctor: manifest +
locator rows render instantly; doctor-sourced groups skeleton until the poll lands,
then show "doctor unavailable — {reason}" verbatim on failure.

### S6. Health

Doctor, as a surface. Source `doctor --json` (real today); repair via
`doctor --repair --json` (PENDING §P5 — live today the two flags conflict; the lift
is part of the concurrent batch).

1. **Verdict banner:** the envelope's `verdict.label` word set plain ("OK" /
   "blocked" verbatim) + `verdict.explanation` sentence (13pt `textSecondary`).
   Doctor's own words; the app adds no judgment.
2. **Findings table:** one 40px row per `findings[]` entry: state glyph (✓ `success`
   / ! `warn` / ✗ `danger`) + subject 13 medium + detail `mono` 11 `textSecondary`,
   middle-truncated, expandable on click to the full detail (selectable). Order
   verbatim (the binary's order is its triage order).
3. **[Repair] per repairable finding:** compact standard button on rows the envelope
   marks repairable (PENDING per-finding `repairable: bool`, §P5). Click → confirm
   sheet (520): "Repair {subject}?" + the envelope's repair description + command
   well `deadreckon doctor --repair --json` + primary [Repair]. On completion the
   repairs' outcomes render per §P5 and the findings table re-polls. Until the
   per-finding field lands, one section-level [Repair…] button renders instead,
   gated on today's real signals (`binary_health.repairable_receipt` /
   `repairable_active_installation` true, or a failed `supervisor service` finding),
   with the same confirm sheet — capability probe, not a dead control.
4. **[Run health check] standard button** — re-runs `doctor --json` (the section is
   also the manual refresh for every doctor consumer). `doctor --live` stays CLI: it
   spends a bounded real provider turn per route; money-spending diagnostics stay a
   typed decision in Terminal (one line in §R6).
5. **Raw report ▸** disclosure: the full JSON document in a `monoS` well, selectable
   — the evidence floor under every row above.

### S7. Settings ASCII sketches

General:

```
+--------------------------- Settings (840×600) ------------------------------+
| ⚙ General          #  GENERAL — defaults every new goal inherits            |
| ▤ Agents & Keys    #  +--------------------------------------------------+ |
| ↻ Service          #  | Agent                        [ Claude Code    ▾ ] | |
| ◷ Notifications    #  | defaults.provider                                 | |
| ▣ Binaries         #  | Model                        [ Agent default  ▾ ] | |
| + Health           #  | defaults.model                                    | |
|                    #  | Spend cap                    [$ 25.00           ] | |
|                    #  | defaults.max_spend                                | |
|                    #  |   New runs start with this budget unless the      | |
|                    #  |   goal sets one. $10 when unset.                  | |
|                    #  | Time limit                   [ 10      ] hours    | |
|                    #  | defaults.cli_max_wall_seconds                     | |
|                    #  | Sandbox                      [ sandbox-exec   ▾ ] | |
|                    #  | defaults.sandbox                                  | |
|                    #  | Prevent sleep                [ auto           ▾ ] | |
|                    #  | defaults.prevent_sleep                            | |
|                    #  |###################################################| |
|                    #  | deadreckon config set defaults.max_spend 25 --json| |
|                    #  |   completed                                       | |
|                    #  +--------------------------------------------------+ |
|                    #  ADVANCED — the whole file ▸                          |
+-----------------------------------------------------------------------------+
```

Agents & Keys:

```
|  AGENTS                                                                  |
|  +---------------------------------------------------------------------+|
|  | ▸C Claude Code   cli:claude-code   ((ready))              [Test]    ||
|  |    Model default   [ Agent's default ▾ ]                            ||
|  |    Signs in with its own CLI — no key stored here.                  ||
|  +---------------------------------------------------------------------+|
|  | ▸A Anthropic API  api:anthropic    ((not available))      [Test]    ||
|  |    error message verbatim…       try: `deadreckon config set-key …` ||
|  |    Key  ((not configured))  [ Paste API key…        ] [Save key]    ||
|  |    deadreckon config set-key api:anthropic --json                   ||
|  |    The key travels over stdin — never in the command line or logs.  ||
|  +---------------------------------------------------------------------+|
|  ROLES                                                                  |
|  | Reviewer agent  [ No override ▾ ]   Reviewer model  [ … ▾ ]         ||
```

Service:

```
|  BACKGROUND SERVICE                                                      |
|  ● Running                                                               |
|  Evidence ▾                                                              |
|    service manager   running · pid 79697                                 |
|    label             com.deadreckon.supervisor                           |
|    plist             ~/Library/LaunchAgents/com.deadreckon.….plist       |
|    checkpoint        generation 12 · age 4s · boot 7A2C…                 |
|    binary            ~/.local/share/deadreckon/bin/deadreckon           |
|                                                    [Stop service…]      |
|  The service claims queued runs, supervises them, and records proven     |
|  cleanup. The CLI calls it the `supervisor`.                             |
```

Binaries:

```
|  VENDORED CLI                                                            |
|  Vendored CLI      v0.8.4-17-ge2946a4      commit e2946a4…              |
|  sha256 (arm64)    90c435f2f30e…           ((verified at launch))       |
|  Path              …/Resources/bin/deadreckon_darwin_arm64  [Reveal]    |
|  CLI reports       deadreckon 0.8.4                                     |
|  dr-gate           sha256 159622c6…  gate protocol v1  ((compatible))   |
|  INSTALLED CLIS                                                          |
|  ((shell))  0.8.4  ~/.local/share/deadreckon/bin/deadreckon             |
|             on PATH · ┌ deadreckon update ┐              [Reveal]       |
|  ((brew))   0.8.1  /opt/homebrew/Cellar/deadreckon/0.8.1/bin/deadreckon |
|             PATH selects this one · ┌ brew upgrade … ┐   [Reveal]       |
|  MISMATCH WARNINGS                                                       |
|  ⚠ /opt/homebrew/…/deadreckon is version 0.8.1; the running binary is   |
|    version 0.8.4                                          (verbatim)    |
|  HANDSHAKE & HOME                                                        |
|  CLI handshake     health OK        DEADRECKON_HOME  ~/.deadreckon      |
```

Health:

```
|  HEALTH — blocked                                                        |
|  Doctor checked 17 setup areas and found 2 blocking setup issue(s). …    |
|  ✓ source              /Users/gdc/deadreckon/deadreckon-mac              |
|  ✗ config              …/config.toml missing              [Repair]      |
|  ✓ sandbox sandbox-exec found at /usr/bin/sandbox-exec                   |
|  ✗ supervisor service  the installed … points at a different binary or   |
|                        state directory                    [Repair]      |
|  …                                                                       |
|  [Run health check]                              Raw report ▸           |
```

---

## P. PENDING DECODE SHAPES (reconcile with the concurrent Rust envelopes)

Everything in this block is ⚠ **PENDING**: field names are this spec's best guess,
grounded in the binary's established envelope grammar (every surface probed live emits
`kind` / `status` / `paths` / `next_actions` / `try_lines` / a `verdict{kind, label,
explanation, evidence[[k,v]], recommended_command, subject}` block — doctor and
providers both do). The implementer MUST diff each shape against the landed Rust
serializer before writing a Swift decoder, then update CONTRACTS.md with the real
contract. Decoders fail closed (rule 2).

### P1. `config show --json` (read)

```json
{ "kind": "config", "status": "ok",
  "paths": { "config": "…/config.toml", "home": "…" },
  "present": true,
  "values": { "defaults.provider": { "value": "cli:claude-code", "set": true },
              "defaults.max_spend": { "value": 15, "set": true } },
  "keys": { "api:anthropic": { "configured": true } },
  "next_actions": [], "try_lines": [] }
```

Open questions: map vs array; whether unset keys appear with defaults; whether key
material is a separate verb (`config show-keys`?). Hard requirement either way: **key
values never serialize** — only a configured boolean (rule 4).

### P2. `config set KEY VALUE --json` / `config unset KEY --json` (write)

```json
{ "kind": "config_set", "status": "completed", "key": "defaults.max_spend",
  "value": 15, "paths": { "config": "…" },
  "verdict": { "label": "completed config defaults.max_spend", "explanation": "…" },
  "next_actions": ["deadreckon config get defaults.max_spend"], "try_lines": [] }
```

The text verb already speaks this verdict ("completed config defaults.max_spend …
reading the same key is the safest verification command" — live); the envelope is
that sentence, typed. `unset` mirrors with `"kind": "config_unset"`. Refusals must be
`kind:"error"` envelopes per the G1 rule.

### P3. `config set-key ROUTE --json` (write, secret over stdin)

```json
{ "kind": "config_set_key", "status": "completed", "route": "api:anthropic",
  "configured": true, "storage": "config",
  "next_actions": [], "try_lines": [] }
```

Contract demands: secret read from stdin to EOF; argv carries only the route; the
envelope never echoes any part of the key; `storage` names where it landed (config
file / keychain — whichever the Rust side chose). Removal form unresolved: `config
unset-key ROUTE` vs `set-key` with empty stdin — reconcile. App side: `CLIRunner`
grows an optional stdin payload that is excluded from every transcript and error path.

### P4. `supervisor status --json` (read — the two-source verdict)

```json
{ "kind": "supervisor_status",
  "service_manager": { "state": "running", "pid": 79697,
                       "label": "com.deadreckon.supervisor",
                       "plist_path": "~/Library/LaunchAgents/…" },
  "checkpoint": { "present": true, "age_seconds": 4, "generation": 12,
                  "boot_id": "…", "pid": 79697,
                  "home": "/Users/gdc/.deadreckon", "binary_path": "…" },
  "verdict": { "label": "running", "evidence": [["service manager", "running · pid 79697"],
               ["checkpoint", "generation 12 · age 4s"]], "explanation": "…" },
  "status": "ok", "next_actions": [], "try_lines": [] }
```

Verdict vocabulary the app types on: `running` / `stopped` / `not_installed` / `stale`
/ `unsupported` / `running_foreign_home` / `unknown`. The checkpoint-identity fields
(`generation`, `instance_id`, `boot_id`, `pid`, `process_start_identity`) exist in the
Rust source today (`supervisor_service.rs`); the foreign-home typed state is the one
FULL-DRIVE B5a names. The checkpoint-absent case must become an envelope (today: prose
on stderr, exit 1 — live) — that arming is the point of the concurrent work.

### P5. `doctor --repair --json` (write)

Doctor's existing document (real, live-corroborated) plus:

```json
{ "findings": [ { "status": "failed", "subject": "config", "detail": "…",
                  "repairable": true, "repair": "write a starter config.toml" } ],
  "repairs":  [ { "subject": "config", "status": "repaired", "detail": "…" },
                { "subject": "supervisor service", "status": "failed", "detail": "…" } ] }
```

Plus the flag-conflict lift (today `--json` + `--repair` conflict — live). Per-finding
`repairable` is the field the Health section wants; if the Rust side lands only the
top-level `repairable_*` booleans, §S6's fallback (one section-level Repair) ships
instead — capability probe decides at decode time.

### P6. `supervisor install|start|stop --json` (write)

```json
{ "kind": "supervisor_install", "status": "completed", "changed": true,
  "label": "com.deadreckon.supervisor", "plist_path": "~/Library/LaunchAgents/…",
  "verdict": { "label": "…", "evidence": [["…","…"]] },
  "next_actions": ["deadreckon supervisor status"], "try_lines": [] }
```

`start`/`stop` mirror (`kind`: `supervisor_start` / `supervisor_stop`). Unmanaged or
foreign units must refuse with `kind:"error"` envelopes rendered verbatim — the app
never offers force. Resolution discipline: the sheet resolves on the envelope, then
§S3 re-polls `status` before repainting the verdict (two sources, fresh).

### P7. Dispositions (used by §R1, same reconciliation rule)

- `undo <id> --json` — best guess `{ "kind": "undo", "status": "completed",
  "restored_files": 12, "snapshot_turn": 3, "next_actions": [] }`. No `--json` exists
  today (live) — the affordance stays capability-gated until it lands.
- `rewind <id> --to-checkpoint C --preview --json` / `--apply --json` — the happy
  path is real today (live: flags exist); the refusal path is prose (live) and needs
  G1 arming. Best-guess preview payload: `{ "kind": "rewind_preview", "checkpoint":
  "…", "files": [{ "path": "…", "change": "modified", "hash_guard": "clean|drifted" }],
  "will_apply": false }`; apply mirrors with `"kind": "rewind"` and the applied list.

---

## R. SCREEN SWEEP — judgment against the FULL-DRIVE matrix

### R1. Dispositions on run detail

**Undo after approve (matrix A11 — dead text today).** After an in-place apply whose
success envelope's own `next_actions` offers `deadreckon undo` (the existing
honest-claim rule, ReviewApproveSheet), the success band gains **[Undo…]** as a
standard button beside the existing sentence. Sheet (520): title "Undo this
approval?", goal line + run id mono; body: "Restores the project files to the
snapshot taken before the result was applied. The run stays finished — only the
applied files revert."; command well `deadreckon undo {id} --json`; destructive
confirm fill [Undo]. Resolution from the §P7 envelope; on success the sheet reports
the envelope's facts and the row re-reads from files. Capability-gated: until
`undo --json` lands, the button does not render and today's selectable command text
stands — the honest gap stays visible, never a dead control. Scope note: undo
eligibility is not a durable fact on the `list --json` rollup, so the affordance
lives only where the envelope just offered it (this sheet, this session); the app
never guesses undo-ability for older rows.

**Rewind in the Recorder tab (matrix A12 — permanently-disabled button today).**
The checkpoint card's [Rewind…] arms when the capability probe confirms the vendored
binary speaks `rewind --json` (it does at 0.8.4 — live; the probe exists because the
refusal path is not yet enveloped and older binaries lack the flag). Flow, preview
first, always:

1. Click [Rewind…] on a checkpoint card → RewindSheet (560) opens and immediately
   runs `rewind {id} --to-checkpoint {c} --preview --json`.
2. Preview renders: checkpoint facts (id mono, turn, trigger, "full snapshot" chip),
   the files that would change (path mono + change word), and the hash-guard state
   per file — "clean" quiet / "drifted" warn with the guard's words verbatim.
   Caption: "Rewind refuses to touch a file that changed since the checkpoint unless
   its hash still matches — the guard is the CLI's, not the app's."
3. [Rewind Run] destructive confirm fill → `--apply --json`; result facts verbatim;
   Recorder re-reads the manifest. A prose refusal (pre-arming binary) renders the
   established pattern: "The CLI answered without a machine envelope (exit {n}); it
   said:" — words verbatim, selectable.

```
+--------------------------- Rewind (560) -----------------------------+
| Rewind this run?                                                     |
| Ship the durable ledger rewrite · r-8f2…                             |
|######################################################################|
| Checkpoint  ckpt-91a2…  · turn 5 · provider checkpoint               |
|             ((full snapshot))                                        |
| FILES THAT WOULD CHANGE                                              |
|  modified  src/ledger.rs                        guard: clean         |
|  modified  src/tail.rs                          guard: drifted ⚠     |
|  Rewind refuses to touch a file that changed since the checkpoint    |
|  unless its hash still matches — the guard is the CLI's.             |
|  deadreckon rewind r-8f2… --to-checkpoint ckpt-91a2 --apply --json   |
|######################################################################|
|                                   [Cancel]        [ Rewind Run ]     |
+----------------------------------------------------------------------+
```

**Discard (matrix A10 / REDESIGN §A0 note) — the honest gap stays.** The operator's
mental model names a fourth decision; the binary did NOT grow a Job-level discard
verb. Live corroboration: `abandon` (alias `discard`) exists with `--json`, but it is
"Remove a run's temporary worktree and branch" — run-scoped workspace cleanup that
refuses on durable Job refs (that refusal envelope is the matrix's gold standard).
Shipping it as "Discard" on a Job row would lie about what it deletes. The Review
sheet keeps Approve / Send back / Stop; the CONTRACTS.md registration stands; the
verb lands in the app the day the binary speaks it for Jobs.

### R2. First-run experience — ONE panel, not a wizard

Shown as the Overview center when the fleet is empty AND setup is incomplete
(any of: doctor `config_present == false`, zero configured agent routes, service
not installed). All five rows visible at once with live status glyphs — a checklist
you can do in any order, on one surface, nothing hidden. When every row is green (or
the operator clicks "Set up later"), the panel yields to the standard "Start your
first goal" empty state (REDESIGN §B4). It returns on next launch only while setup
is incomplete and the fleet is still empty; once runs exist, Settings owns remediation.

Rows (each: status glyph + label 13 medium + fact/control right):

1. **CLI** — auto: "verified at launch · 0.8.4" (BinaryLocator + manifest) or the
   locator error verbatim (danger) with [Open Binaries settings].
2. **Agent** — radio of live probe rows (same atoms as New Goal §C3); failed probes
   visible-disabled with `try:` lines. Selecting writes `defaults.provider` (§P2;
   degraded: shows the command to copy).
3. **Key** — only when the chosen route is an API-key route: SecureField + [Save
   key] (§S2 grammar, stdin-backed). Subscription routes render "signs in with its
   own CLI" and a green glyph when the probe is ready.
4. **Background service** — verdict word + [Install & start…] (chains §S3's install
   then start confirms into one sheet listing both commands).
5. **Prove it** — [Run a test] runs `try --json` (real today — live shape:
   `{run_id, trust, trusted_job_receipt: false, gate, proof, story, lineage, next}`).
   While running: "Running the proof — a real tiny run in a throwaway workspace, no
   credentials needed…" (this can take minutes; the row shows elapsed time). On
   success: "proof signed" success chip + proof path mono + the binary's own trust
   words verbatim beneath: "local smoke gate evidence only; not a trusted Job
   receipt". Failure: refusal verbatim.

Footer: primary **[Start your first goal]** (opens New Goal) — enabled once rows 1–2
are green (key/service/proof are recommended, not gates; the New Goal preview will
refuse honestly if something is actually missing). Quiet text-button "Set up later".

```
+------------------------------ center (max 560) -----------------------------+
|                          Set up deadreckon                                  |
|   Five checks and you're running goals. Do them in any order.               |
|  +-----------------------------------------------------------------------+ |
|  | ✓ CLI                verified at launch · 0.8.4                        | |
|  | ✓ Agent              (•) Claude Code   ( ) Codex CLI ((not available))| |
|  |                          try: `codex login`                            | |
|  | – Key                signs in with its own CLI — nothing to store      | |
|  | ✗ Background service not installed          [Install & start…]        | |
|  | ○ Prove it           [Run a test]  — a real tiny run in a throwaway    | |
|  |                      workspace; no credentials needed                  | |
|  +-----------------------------------------------------------------------+ |
|                                          [[ Start your first goal ]]       |
|                                                       Set up later         |
+-----------------------------------------------------------------------------+
```

### R3. Library browser (matrix B7 — weekly value)

**Placement:** View > Library (⌘L) + a quiet "Library →" text-button in Overview's
RECENTLY FINISHED section header. It renders in the main window center (sidebar
selection cleared), not a separate window — one window, one center, selection-driven,
same as Overview.

**Content:** a modest table over `library list --json` (real — live shape:
`artifacts[]{ manifest{ run_id, scope, goal, promoted_at, source_working_dir,
provenance_hash, payload_files?, payload_bytes? }, path, materialized_count }`).

- Header: "Library" 17 semibold + count as plain text + scope toggle
  [This project | All projects] (`--all` exists — live) + local filter field
  (client-side over goal/scope/run id; `library search` has no `--json` — live —
  and a local filter over a weekly-sized list covers the journey; the binary verb
  stays CLI until it grows an envelope).
- Rows (40px): goal 13 medium · project (plain folder name from scope, full scope in
  tooltip mono) · promoted {relative time} · "{payload_files} files ·
  {payload_bytes formatted}" when present · run id mono truncated. Row actions
  (hover + context menu): [Reveal in Finder] (the artifact `path`), Copy run id.
  Clicking a row whose run id still exists in the fleet selects that run in the
  sidebar (evidence lives there); otherwise the row is inert facts.
- Empty state: "Nothing in the library yet" + "Approved results are promoted here —
  approve a verified run and it lands in the library." + command well
  `deadreckon library list`.
- Degraded: the standard unavailable banner with the CLI line, verbatim reason.

```
+------------------------------- center --------------------------------------+
|  Library                    12 artifacts   [This project | All projects]    |
|  [ Filter by goal, project, or run id…                        ]             |
|#############################################################################|
|  Fix flaky auth tests        deadreckon   2d ago   12 files · 84 KB  r-8f2… |
|  Migrate billing store       billing      5d ago    7 files · 41 KB  r-31c… |
|  Bump lockfile               deadreckon   2w ago    1 file  · 2 KB   r-99a… |
|                                                     [Reveal in Finder] ⋯    |
+-----------------------------------------------------------------------------+
```

### R4. Command sweep — in, with one-line reasons

| Journey | Disposition here |
|---|---|
| `config` (B2) | IN — Settings §S1/§S2; the last daily-adjacent journey with zero UI. |
| `supervisor` lifecycle (B5b) | IN — Settings §S3; the one chip gating all recovery gets its remedy. |
| `doctor` + repair (B4) | IN — Settings §S6; weekly, and the app already quotes doctor everywhere. |
| Binaries visibility | IN — Settings §S5; the operator's explicit ask, zero new binary work. |
| `try` (A2) | IN — first-run proof row §R2; the keyless first-hour proof. |
| `library list` (B7) | IN — §R3; weekly review inventory, read-only, envelope real. |
| `undo` (A11) | IN (capability-gated) — §R1; advertised after every approve, must stop being dead text. |
| `rewind` (A12) | IN (capability-gated) — §R1; the flag is real at 0.8.4, preview-first. |
| `providers check`-style probe (B3) | IN as [Test] per route §S2 — active probe, no new grammar. |

### R5. Command sweep — out, with one-line reasons

| Journey | Why it stays CLI |
|---|---|
| `import` (B6) | Power, rare, one-shot migration; a GUI adds ceremony to a command you run once. |
| `learn` (C5) | Power introspection over harness memory; no daily/weekly operator journey reads it. |
| `seams` (B10) | Validation tooling for seam authors, not operators. |
| `improve` (C6) | Self-modifying the harness from a GUI has the wrong blast-radius/trust profile — the matrix's own judgment, upheld. |
| `abandon`/`discard` (A10) | Run-worktree cleanup that refuses on Jobs (live); shipping it as "Discard" would lie — honest gap stands (§R1). |
| `doc` generate/polish (A15) | No `--json` (live); reading run docs is already drivable; generation waits for envelopes. |
| `report --html --open` (C1) | Nicety; the app IS the report surface. |
| History grep (C4) | Per-run Activity search covers the daily case; cross-scope grep is power. |
| `apply`/`materialize` (A9b) | Finish routing + destination flags already drive both journeys — the matrix's own call. |
| `detect --ping` / `doctor --live` | Both spend or probe beyond static checks; money/transport diagnostics stay typed decisions in Terminal. |
| Self-update (B8) | Guided handoff by design: §S5 shows every install + its own `update_command` copyable — in-app update would conflate the vendored app binary with the service-pinned install. |
| `follow` adoption (A5b) | Plumbing swap inside the tailing layer, not a screen; Phase 6 owns it. |
| Start-shape widening (A3b, deadline, `@goal-file`, mode) | Owned by the concurrent launcher workstream — not re-specified here. |
| Campaign/chain surfaces (A3e/f, D5) | Deferred-by-design v1.x per the matrix; no launch envelopes (live). |
| `completions` (B9) | Terminal-native by definition. |

---

## I. THE APP ICON

### I1. Design

The current asset is a navy anchor — nautical, which DESIGN.md §8 explicitly retired.
The new icon is the committed world made 1024px: **the diamond brand mark, hard-line
geometry, dark charcoal field, the one warm orange.**

- **Field:** the macOS icon-grid squircle (824×824 centered in the 1024 canvas,
  corner radius 185) filled `#1D1C1A` (`panel`) — the instrument's face.
- **Machined edge:** one inner stroke `#32302C` (`border`), 10 units wide, inset so
  its outer edge meets the tile edge — the same "panels meet at hairlines" line the
  app draws everywhere. Reads as a machined bevel at 128px+; disappears politely
  below 32px.
- **Mark:** a sharp 45° diamond, half-diagonal 270, centered at (512, 512) — points
  at (512,242), (782,512), (512,782), (242,512). Split along its vertical diagonal
  into two flat facets: left `#E2703A` (`accent`, lit), right `#D0662F`
  (`accentDown`, shade). No gradient — two flat fills meeting at a hard line, which
  is exactly this world's idea of depth. (A diamond carries half the visual mass of
  a square, so 65% of the tile width reads calm, not loud — verified against 250
  in renders; 270 wins small-size presence.)
- **Color accounting:** two hues — charcoal (field + hairline edge) and orange (two
  facet values). Flat, no gradients, no shadows, no gloss.

Small-size test (run mentally and then against renders): at 16px the tile is ~13px,
the diamond ~8px; the facet values are 8% apart in luminance so the mark reads as one
solid orange diamond on a charcoal tile — survives. At 32px the facet is a hint; at
128px the edge line and facet both read; at 512/1024 the geometry is the craft. No
element exists that dies at 16px and leaves residue (nothing thinner than the edge
line, and the edge line's disappearance costs nothing).

**Master:** `design/icon.svg` (1024 viewBox, written alongside this spec — the source
of truth). Implementer A renders the `AppIcon.appiconset` set from it (§W1); render
each pixel size directly from the SVG (16, 32, 64, 128, 256, 512, 1024 for @1x/@2x
slots) — never downscale a single PNG.

### I2. Menubar template glyph alignment

The menubar keeps the SF Symbols diamond family (`diamond` idle, `diamond.fill`
live/attention, `.medium` weight) — already implemented, correctly template-tinted,
and weight-matched to the count text. Alignment rule with the new icon: same family,
same reading — a diamond, point-up, no rotation, no embellishment. SF's diamond has
slightly eased corner joins vs. the icon's miter points; at 16–18pt menubar sizes the
difference is sub-pixel and does not break the family.

Optional exactness step (only if the operator wants pixel-true kinship):
`design/menubar-diamond-template.svg` (written alongside this spec) is a 16×16
template glyph — the same 45° square with miter joins, half-diagonal 7, centered;
filled variant for live, 1.5-unit stroke variant for idle. Export as a template PDF
asset (`MenuBarDiamond`) and swap `Image(systemName:)` for `Image("MenuBarDiamond")`
with `.renderingMode(.template)`. Not required for v1; the SF family is faithful.

---

## W. IMPLEMENTATION PLAN — two implementers, sequential, no collisions

Implementer A lands first (icon + settings + binaries + the shared write plumbing);
Implementer B builds screens + dispositions + first-run on top of A's landed
plumbing. Shared files (`MutationRunner`, `Lexicon`, `WriteSurfaceRouter`) are edited
by A first; B rebases. Kit logic lives in DeadreckonKit; views stay thin. Every step
leaves the app buildable; the gate after each phase: `xcodegen generate` +
`xcodebuild -project deadreckon.xcodeproj -scheme deadreckon -configuration Debug
build` + `swift test --package-path DeadreckonKit`.

### W1. Implementer A — icon, settings, binaries

1. `design/icon.svg` — source of truth (this shape's own output; done).
2. NEW `scripts/render-appicon.sh` — renders every `Assets.xcassets/
   AppIcon.appiconset/icon_*.png` slot from `design/icon.svg` at exact pixel sizes
   (rsvg-convert or qlmanage+sips; each size rendered from the vector, not scaled).
3. `Assets.xcassets/AppIcon.appiconset/*.png` — regenerated (10 slots, filenames
   unchanged so `Contents.json` stands).
4. `DeadreckonKit/Sources/DeadreckonKit/Services/CLIRunner.swift` — optional
   `stdin: Data?` on `run`/`runDetailed` (nil = nullDevice, unchanged behavior);
   stdin bytes excluded from every transcript, log, and error path (rule 4).
5. `DeadreckonKit/Sources/DeadreckonKit/Services/MutationRunner.swift` — new
   `PlannedVerb` cases: `configSet(key:value:)`, `configUnset(key:)`,
   `configSetKey(route:)` (NO secret payload — the secret rides only the dispatch
   call's stdin parameter, never the Equatable enum), `supervisorInstall`,
   `supervisorStart`, `supervisorStop`, `doctorRepair`. Argv per §P; timeouts:
   config 60s, supervisor 120s, repair 300s.
6. NEW Kit services (fake-runner tested like every store):
   - `ConfigStore.swift` — capability probe (`config show --json` decode-or-degrade),
     values/keys read model, write dispatch + re-read cycle (§S1/S2).
   - `ServiceController.swift` — `supervisor status --json` poll + typed verdict
     (§P4, incl. prose-refusal degradation), install/start/stop dispatch +
     post-write re-poll (§S3).
   - `DoctorStore.swift` — full `doctor --json` document retention (raw JSON for
     the disclosure), findings read model, repair dispatch (§S6, §P5).
7. `DeadreckonKit/Sources/DeadreckonKit/Models/MutationEnvelopes.swift` — decode
   types for §P1–P6 (each marked with its PENDING reconciliation comment).
8. `Sources/Views/SettingsView.swift` — REWRITE to §S0–S6: sidebar + six sections;
   confirm sheets local to the window; `@AppStorage("settings.section")` deep link.
9. `Sources/Views/Lexicon.swift` — settings words: section titles, service verdict
   words (+ "running for a different home"), repair words, config row labels/captions.
10. `Sources/Views/SidebarView.swift` — footer sets the Health deep link before
    `openSettings()` (one line).
11. Kit tests: `ConfigStoreTests`, `ServiceControllerTests`, `DoctorStoreTests`
    (fixtures from §P shapes + the live doctor document), `MutationRunnerTests` argv
    additions (incl. asserting `configSetKey` argv contains no secret and CLIRunner
    transcript redaction).
12. `CONTRACTS.md` — register: the six envelope decode contracts (PENDING→real as
    they reconcile), the capability-probe rule, the stdin redaction rule, the
    settings write-then-re-read discipline.
13. No xcodegen needed (no app-target file added/renamed) unless confirm sheets are
    split into new files — if so, `project.yml` regenerate rides this phase.

Acceptance (operator, from the seat): (1) ⌘, opens the six-section window, dark,
bordered, no tab strip; (2) every General row shows its `config.toml` key in mono and
a change survives an app relaunch (file truth); (3) an API key saved via the secure
field never appears anywhere afterward — window, command well, or `log stream`; (4)
Service shows one verdict word + evidence and Install/Start/Stop round-trip against a
scratch home, Stop demanding the typed word; (5) Binaries lists vendored CLI + dr-gate
+ every installed CLI with hashes and reveals each in Finder, and shows the 0.8.1 brew
skew warning verbatim; (6) Health lists doctor findings with Repair on repairable
rows and the raw JSON behind the disclosure; (7) against an older binary (no config
envelopes), General degrades to read-only + terminal handoff, no dead controls.

### W2. Implementer B — screens, dispositions, first-run

1. `DeadreckonKit/Sources/DeadreckonKit/Services/MutationRunner.swift` — cases
   `rewindPreview(id:checkpoint:)`, `rewindApply(id:checkpoint:)`, `undo(id:)`
   (§P7 argv; rebase on A).
2. NEW Kit: `LibraryStore.swift` (`library list --json` decode — real manifest
   shape §R3 — + `--all` toggle + client filter), `SetupDerivation.swift` (the
   §R2 completeness facts from doctor/providers/service inputs),
   `TryController.swift` (`try --json` dispatch, long-timeout 600s, live shape §R2).
3. `Sources/Views/OverviewView.swift` — first-run panel (§R2) replacing the empty
   state while setup incomplete; "Library →" link in RECENTLY FINISHED header.
4. NEW `Sources/Views/LibraryView.swift` (§R3) + `Sources/Views/RewindSheet.swift`
   (§R1) — `project.yml` regenerate (xcodegen) for both.
5. `Sources/Views/DetailCenterTabs.swift` — Recorder checkpoint card: [Rewind…]
   arms via capability probe → routes `.rewind`; the disabled-help text becomes the
   probe's honest degraded words (no more hardcoded gap label).
6. `Sources/Views/ReviewApproveSheet.swift` — [Undo…] on apply-success per §R1
   (envelope-offered, capability-gated).
7. `Sources/Views/WriteSurfaceRouter.swift` — surface cases `.rewind(row,
   checkpoint)`, `.undo(row)`; RefusalView reused as-is.
8. `Sources/Views/MainWindowView.swift` — center routing for Library (sidebar
   selection nil + library flag; Escape/⌘1 return to Overview).
9. `Sources/AppCommands.swift` — View > Library (⌘L).
10. Kit tests: `LibraryStoreTests` (real-shape fixture), `SetupDerivationTests`,
    `TryControllerTests`, argv tests for rewind/undo, capability-gating tests
    (no envelope → no control).
11. `CONTRACTS.md` — register: rewind preview-first + hash-guard rendering contract,
    undo honest-claim extension, library read contract, try trust-words contract
    (the "untrusted local smoke diagnostic" line always renders verbatim).

Acceptance (operator): (1) fresh scratch home + empty fleet → the one setup panel,
five rows, any order; [Run a test] produces a signed-proof line with the CLI's trust
words; (2) completing setup yields the standard empty state, and it never returns
once runs exist; (3) ⌘L shows the library table against a home with promoted
artifacts; Reveal opens the artifact folder; filter narrows live; (4) on a run with
checkpoints, Rewind previews files + guard states before any change, applies only
from the destructive confirm, and renders a prose refusal verbatim on a pre-arming
binary; (5) after an in-place approve whose envelope offers undo, [Undo…] appears
and resolves from its envelope — and does not appear on an older binary; (6) the
Review sheet still has no Discard control.
