# DESIGN.md — the committed visual world

This file is the app's visual constitution. It replaces the previous theme's authority
(`Theme.swift`'s "Granola-style" warm-paper, serif, light-first world) in full. Where any
view, token, or habit in the current code disagrees with this document, this document wins.
Product truth stays in PRODUCT.md; implementation sequencing lives in
`design/REDESIGN-SPEC.md`.

## Character

A dark instrument panel for someone who runs machines overnight and reads the truth off
them in the morning. Near-black warm charcoal, structure drawn with crisp 1px lines —
nothing floats, nothing glows, panels meet at hairlines like machined parts. One warm
orange is the only voice raised, and it is spent only on the things that are alive or need
a hand: the primary action, the live marker, the count of decisions waiting. Everything
machine-true — paths, ids, commands — is set in monospace, because those strings are
evidence and evidence is quoted, not paraphrased. Green never celebrates; it states that a
proof validated. Red never shouts; it states that something is over. The app should feel
like Conductor's calm confidence with deadreckon's stricter conscience: dense, flat,
honest, quiet until a decision is yours.

## 1. Mode

- **Dark only.** The app never reads system appearance. `AppDelegate` forces
  `NSApp.appearance = NSAppearance(named: .darkAqua)` at launch so AppKit-owned chrome
  (sheets, pickers, toggles, menus) matches. There are no light/dark dynamic pairs;
  every token below is one hex value. Delete `Theme.dynamicColor` and all light values.
- Settings' Appearance copy states this plainly: the app is dark-only by design.

## 2. Color tokens (exact)

Surfaces (a 4-step elevation ladder; each step is a *material change drawn by a border*,
never a shadow):

| Token        | Hex       | Use |
|--------------|-----------|-----|
| `sidebarBg`  | `#121110` | Sidebar and popover list background — one step darker than content |
| `windowBg`   | `#151412` | Window/content background, sheet background |
| `panel`      | `#1D1C1A` | Cards, rows-at-rest surfaces, tab strips, drawer chrome |
| `panelHover` | `#21201D` | Hovered interactive rows/cards |
| `well`       | `#242220` | Inputs, text editors, code/command wells, selected rows |
| `border`     | `#32302C` | THE structural line: 1px component borders and every seam where surfaces meet |
| `borderHover`| `#3E3B36` | Hovered/selected component border |

Text:

| Token           | Hex       | Use |
|-----------------|-----------|-----|
| `textPrimary`   | `#ECEAE6` | Primary UI text and all monospace machine-truth |
| `textSecondary` | `#9B968E` | Secondary text, metadata, sidebar group headers |
| `textTertiary`  | `#6B675F` | Muted: placeholders, timestamps, section labels, disabled text. ~3:1 on `panel` — never the sole carrier of essential meaning |

Accent — exactly one:

| Token         | Hex       | Use |
|---------------|-----------|-----|
| `accent`      | `#E2703A` | The single warm orange. Permitted uses ONLY: (1) the one primary action per surface, (2) links / inline text-buttons, (3) live-run markers and the needs-you count badge, (4) keyboard focus rings, (5) the brand mark |
| `accentHover` | `#E8804F` | Hovered primary button fill |
| `accentDown`  | `#D0662F` | Pressed primary button fill |

Semantics — desaturated to sit inside the world; states, never decoration:

| Token         | Hex       | Use |
|---------------|-----------|-----|
| `success`     | `#7BAE7F` | Verified proof, passed checks, healthy heartbeat, service running |
| `warn`        | `#D9A048` | Degradation: stale heartbeat (confirmed), tail trouble, judge-stopped review states, non-launchable previews |
| `danger`      | `#C25B4E` | Failure marks, borders, ≥ semibold text: failed runs, invalid proof, refusals |
| `dangerText`  | `#CE6A5D` | Small/regular-weight red text (4.5:1-safe on all surfaces) |
| `dangerFill`  | `#A8453A` | Fill for the destructive confirm button only, with `textPrimary` label |
| `scrim`       | `rgba(0,0,0,0.55)` | Overlay backdrop (Command-K) |

Contrast facts (checked): `textPrimary` 13.2:1 on `well`; `textSecondary` 5.4:1 on `well`,
5.8:1 on `panel`; `accent` as text 5.4:1 on `panel`; `#151412` text on `accent` fill 5.8:1;
`textPrimary` on `dangerFill` 4.9:1; `dangerText` 5.1:1 on `windowBg`. `warn` and `success`
clear 7:1 as text on all surfaces. Do not re-tint these values per-view.

Provider marks keep per-agent identity colors but desaturated into this world:
claude `#C87850`, codex/openai `#5E9B8F`, gemini `#7189C9`, opencode `#9078B8`; unknown ids
pick deterministically from {accent, success, warn} by scalar sum. Marks are 12–16px
rounded-rect (radius 4) tiles: fill = mark color at 14%, glyph = mark color.

## 3. Type

System stack only. SF Pro for UI, SF Mono (`.monospaced` design) for machine truth. The
serif display face is deleted — no display faces anywhere.

Fixed scale (pt, fixed — never fluid):

| Step | Size / weight | Use |
|------|---------------|-----|
| `display` | 20 semibold | Empty-state and welcome headlines only |
| `title`   | 17 semibold | Sheet titles, the run-detail goal line |
| `heading` | 15 semibold | Overview card titles |
| `base`    | 13 regular / medium | Default UI text, rows, buttons (13 medium), form labels |
| `small`   | 11 regular | Secondary/meta lines, chips-adjacent captions |
| `caption` | 10 regular | Timestamps, counters, fine print |
| `label`   | 10.5 bold, +0.8pt tracking, UPPERCASE | Section headers (see §5) |
| `monoL`   | 12 mono | Command wells, primary technical lines |
| `mono`    | 11 mono | Ids, paths, branch names inline |
| `monoS`   | 10 mono | Dense evidence rows, ledger lines, diff bodies |

Rules: monospaced digits for every count, money, and duration. Line height default;
prose paragraphs (explanatory copy) max width 72ch. Machine-true strings (paths, ids,
commands, file names, branch names, flags, raw CLI words) are ALWAYS mono in
`textPrimary` or `textSecondary` — never restyled, never re-worded.

## 4. Space, size, radius, line

- **Spacing scale:** 4 / 8 / 12 / 16 / 20 / 24 / 32. Panel internal padding 12; sheet
  padding 20; window content gutters 16.
- **Row rhythm:** sidebar run rows 36px; list/queue rows 40px; overview decision cards
  ≥ 56px. Vertical list gaps 2px (sidebar) / 8px (cards).
- **Radii:** chips and small badges 4; inputs, buttons, command wells 6; cards, sheets'
  inner panels, tabs 8 — 8 is the maximum. Nothing is pill-shaped except tiny count
  badges (height 16, radius 9, min-width 16).
- **Borders draw the structure.** Every panel, card, input, button, and well has a 1px
  `border` stroke. Every place two surfaces meet gets a 1px `border` seam (sidebar/content,
  header/content, tabs/content, drawer/content). Dividers are 1px `border`, full-bleed.
- **Shadows:** none on in-flow surfaces. Overlays only (Command-K palette, menus) may use
  `black 25% / radius 24 / y 8` beneath their 1px border.
- **Focus:** keyboard focus ring is a 1px `accent` border replacing the resting border
  (inputs) or a 1px `accent` outline inset 0 (rows/buttons). No system glow
  (`.focusEffectDisabled()` where SwiftUI injects one).

## 5. Component rules

**Buttons** — flat panels with 1px borders. All are height 28 (compact 24 inside dense
rows), radius 6, horizontal padding 12, label 12.5–13 medium.

- *Standard:* `panel` fill, `border` stroke, `textPrimary` label. Hover: `well` fill,
  `borderHover`. Pressed: `#171614` fill. Disabled: label `textTertiary`, stroke `border`.
- *Primary — at most ONE per surface:* `accent` fill, `#151412` semibold label, no stroke.
  Hover `accentHover`, pressed `accentDown`. The disabled primary keeps its place:
  `panel` fill, `border` stroke, `textTertiary` label (never a gray accent).
- *Destructive confirm (final confirm inside a destructive sheet only):* `dangerFill`
  fill, `textPrimary` semibold label.
- *Quiet-danger (opens a destructive flow):* standard button with `danger` stroke at 55%
  and `dangerText` label.
- *Text-button / link:* `accent` text, no chrome; underline on hover. Used for inline
  fixes, "show output", disclosure toggles.
- Press response: opacity 0.85 while pressed, 120ms. No scale bounce (delete
  TactileButtonStyle's spring-scale; weight, not shrinking).

**Chips** — the one status atom. Height 18, radius 4, padding 6×2, text 10.5 semibold.
Anatomy: fill = semantic color at 10%, stroke = semantic color at 45%, text = semantic
color (use `dangerText` for red text). Strong variant (VERIFIED, PROOF INVALID, DIGEST
MISMATCH only): fill at 16%, stroke at 70%, text semibold. Neutral chip uses
`textSecondary`. No filled-solid chips — the border world carries state by line and hue,
and `onFill` inks are deleted.

**Count badges** — the only pills: 16px pill, `accent` fill, `#151412` 10 bold text.
Used solely for needs-you counts (sidebar group, menubar). Never for plain totals —
those are text.

**State dots** — 6px circles beside run names: `accent` = live (running/verifying; may
breathe, see §7), `success` = verified awaiting approval, `warn` = stopped for review /
paused for you, `danger` = failed, `textTertiary` = queued/waiting/finished-quiet. The
dot never appears without a word or a labeled row.

**Rows** — interactive list rows are borderless at rest inside their panel (structure
belongs to the panel), `panelHover` on hover, `well` + `borderHover` when selected.
Selection is neutral; accent marks liveness, not selection.

**Section headers** — `label` type (10.5 bold, +0.8 tracking, uppercase) in
`textTertiary`; sidebar group headers and any header the eye must scan-first use
`textSecondary`. Optional trailing count as plain text `(3)` in `textTertiary`. One
style, one place (`Theme.sectionTitle`), no per-view sub-scales.

**Inputs** — `well` fill, 1px `border`, radius 6, 13pt text, `textTertiary`
placeholder; focus swaps the stroke to `accent`. Multiline editors: same, padding 8,
min-height per surface. The typed-amount confirm field keeps its stroke semantic:
`warn` until matched, `success` when matched.

**Command wells** — the equivalent-command line every write surface shows: `well` fill,
1px `border`, radius 6, `monoL` text in `textSecondary`, selectable, full-width. This is
the home of CLI truth; UI words never leak into it and its words never get translated.

**Refusal card** — fill `danger` 6%, stroke `danger` 35%, radius 8. Title row 13
semibold `dangerText` in plain words; the binary's message verbatim in `mono`
`textPrimary`; each fix line as `try:` label + mono `accent` text-button. No override
control exists, ever.

**Sheets** — `windowBg` ground, title 17 semibold at top-left with an 11pt
`textSecondary` purpose line, 1px `border` seams above/below the scrolling body, footer
right-aligned with the surface's single primary at far right. Fixed widths: New Goal
680, Review & Approve 680, Stop 520, Send back 560.

**Tabs** — text tabs in a `panel` strip with a bottom seam: 12.5 medium, active =
`textPrimary` on `well` (radius 6, 1px `border`), inactive = `textSecondary`, hover =
`panelHover`. Counts in tab titles as plain text ("Changes 12").

**Empty states** — teach the surface: `display` or `heading` headline in
`textPrimary`, one 13pt `textSecondary` sentence, then either the surface's primary
action or a mono command well showing the CLI path. Never the word "nothing" alone;
never fake rows.

**Degraded states** — reasons verbatim in mono, `warn`/`danger` by severity, always
with the CLI escape hatch named. Honesty over polish is a component rule, not a
special case.

## 6. The one-accent discipline

Orange is budgeted. Per screen, in priority order: the primary action (at most one),
the live markers (dots + progress step currently executing), the needs-you count badge,
links/fix-lines, the focus ring. If a screen seems to want orange anywhere else, it is
asking for hierarchy, not color — solve it with weight, size, or a border. Green, amber,
and red are *states with fixed meanings* (§2) and never decorate. A surface with zero
live runs and zero decisions shows zero orange except its primary action.

## 7. Motion

150–250ms ease-out on state transitions (tab swap, drawer, hover). The single ambient
motion: the live state dot and the executing progress step may breathe opacity
1.0→0.55→1.0 over 1.6s. No page-load choreography, no scale bounces, no decorative
movement. Reduce Motion disables the breathe.

## 8. Iconography and brand

SF Symbols only, `.medium` weight, 10–13pt inline. The brand mark is a small filled
diamond (`diamond.fill` in `accent`) — abstract, non-nautical — used beside the wordmark
in the sidebar header and as the menubar glyph family: idle `diamond`, live
`diamond.fill`, attention `diamond.fill` + count, degraded `exclamationmark.circle`,
unavailable `exclamationmark.triangle`. No helm, sailboat, anchor, or water glyphs
anywhere. Provider marks per §2.

## 9. Voice

Plain product language everywhere a human reads; exact machine language everywhere a
machine is quoted. The two never mix in one string: UI words (Goal, Run, Checks, Review
& Approve, Send back, Stop, Guide) live in labels; CLI truth (`deadreckon finish r-…`,
`acceptance.yaml`, `steer`, exit codes, refusal text) lives in mono. Sentence case for
all labels and buttons; UPPERCASE only in §5 section headers. Numbers are facts:
"$4.12 of $25.00", "5 checks · 4 passed", "no signal for 45s" — never vibes, never
forecasts. Refusals and judge quotes render verbatim, inside quotation marks when
quoted. The full lexicon (old → new for every string) is in
`design/REDESIGN-SPEC.md` §A and is normative.
