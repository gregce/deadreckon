# deadreckon — The Manual Rider (an SVG-diagram "how it works" guide)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-06-22-1508-deadreckon-site-manual-goal.md`.

It is a **cross-repo, frontend** slice. The deliverable is a new `/manual`
section of the existing Astro site at `/Users/gdc/deadreckon-site`. Prior
deadreckon goal+rider pairs (`docs/goals/*`) supply the *voice and
discipline* — depth-tests-first, files-not-fields, no silent scope
expansion, no `git push` — but their Rust-toolchain invariants
(`cargo`, `PipelineState` schema, clippy) **do not apply** here. The
toolchain is Astro + Bun + `node --test`.

**All paths absolute.** Edit root `/Users/gdc/deadreckon-site`. Read-only
source-of-truth `/Users/gdc/deadreckon` (architecture facts) and
`/Users/gdc/extract-agentic-engineering/marketing/visuals` (diagram style).

## Posture (decided — do not redesign)

- **Edits stay inside `/Users/gdc/deadreckon-site`.** `/Users/gdc/deadreckon`
  is read-only; never write to it. Architecture facts come *from* it.
- **Stay on Astro, `output: 'static'`.** No new framework (no Next, no
  React app shell). Interactivity is Astro islands or vanilla `<script>`,
  no backend, no runtime data fetch beyond the existing release-tag call
  in `index.astro`.
- **Hybrid aesthetic.** The site shell stays bone-paper + signal-mint +
  JetBrains Mono (`src/styles/tokens.css`). The *diagrams* render in the
  warm paper-card palette lifted from
  `extract-agentic-engineering/marketing/visuals/how-agents-stop.html`
  (paper `#f3ece2`, navy `#1d3557`, amber, step/good/warm node tints,
  dark gate box). The two coexist; diagrams live inside a paper "card"
  framed against the bone page.
- **Comprehension is the product.** Visual first, prose second. A chapter
  that is prose with a decorative picture has failed the brief.
- **Single source of truth.** `src/data/chapters.ts` drives nav, the map
  index, prev/next, per-page meta, and the fidelity test. No chapter list
  is hardcoded anywhere else.
- **Fidelity to as-built.** Every chapter declares the AS-BUILT
  section(s) it maps to; a depth test fails the build if a cited section
  is absent from the vendored snapshot.
- **No `git push`.** Phased local commits only (in the `deadreckon-site`
  repo).
- **No V1 invention.** Anything beyond P1–P11 is logged as a note in
  `deadreckon-site` (e.g. `docs/MANUAL-NOTES.md`), not silently built.

## Data model (files, not fields)

### `src/data/chapters.ts` — the chapter registry (the spine)

```ts
export interface Chapter {
  slug: string;          // url segment, kebab: 'the-gate'
  order: number;         // 1..14, unique, contiguous
  title: string;         // 'The gate: done you can't fake'
  eyebrow: string;       // mono kicker, e.g. 'mechanism · 05'
  summary: string;       // one sentence; used in map + <meta description>
  asBuilt: string[];     // section ids that MUST exist in the snapshot, e.g. ['13','35']
  diagram: string;       // component basename under components/manual/diagrams/, must resolve
}
export const chapters: Chapter[]; // exactly 14, see Information Architecture
```

### `src/data/as-built-sections.json` — vendored fidelity snapshot

Generated once in P1 by reading the real
`/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` table of contents.
Self-contained inside the site repo so depth tests need no cross-repo read.

```json
{
  "generatedFrom": "/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md",
  "sections": [{ "id": "13", "title": "Acceptance Gate & Anti-Self-Attestation" }]
}
```

The fidelity test asserts: for every `chapter.asBuilt[i]` there is a
`sections[].id` match. No chapter may cite a section that does not exist.

## Information architecture (chapter → AS-BUILT map)

Exactly fourteen chapters plus the `/manual/` map index. Each chapter
leads with its core diagram.

| # | slug | Chapter | AS-BUILT § | Core diagram |
|---|---|---|---|---|
| 1 | `harness-of-harnesses` | The harness of harnesses | 1 | nested rings: deadreckon wraps your agent CLI wraps the model |
| 2 | `two-layers` | Two layers: skill vs binary | 2, 3 | layer stack (CLI/runtime/core) + Markdown-skill ⇄ Rust-binary split |
| 3 | `run-lifecycle` | The run lifecycle | 4, 6 | 7-phase gap-numbered machine (0·10·20·30·40·50·60) |
| 4 | `the-turn-loop` | The turn loop | 9 | model → tool_use → model; stop = no tool call |
| 5 | `the-gate` | The gate: done you can't fake | 13, 35 | nonce isolation + signed marker; agent has no read path |
| 6 | `done-in-english` | Done, written in English | 13 | one sentence → compiled checks (5 kinds) |
| 7 | `isolation-and-sandbox` | Isolation & the sandbox | 11, 24 | repo → worktree(4 modes) → sandbox(4 backends) |
| 8 | `durable-state` | Durable state & crash recovery | 4, 7, 15 | atomic temp+rename, per-turn snapshots, lock/heartbeat, resume |
| 9 | `atomic-promotion` | Atomic promotion & the library | 8 | working/ → .promoting/ → library/ swap, crash-safe |
| 10 | `providers-byok` | Providers & BYOK | 10, 16, 19 | router fallback; subscription-CLI / direct-API / smoke; import |
| 11 | `multi-agent` | Multi-agent: orchestrate & campaign | 30, 36 | review (coder+reviewer) · full-plan split/merge · campaign fan-out |
| 12 | `evidence-not-transcript` | Evidence, not a transcript | 14, 25 | promoted artifact anatomy: narrative/decisions/as-built/provenance/manifest |
| 13 | `observability` | Watching it work | 18, 27, 32, 33 | attach views + live narrator + flight recorder & rewind/checkpoints |
| 14 | `filesystem-and-telemetry` | The filesystem & the receipts | 5, 14 | `~/.deadreckon/` tree + spend/traces/provenance/events JSONL streams |

The map index is a clickable architecture map (SVG or CSS grid) built
from `chapters.ts`; clicking a node routes to that chapter.

## Diagram kit (the warm paper-card system)

- `src/styles/diagram.css` — token set namespaced `--dgm-*` so it never
  collides with `tokens.css`: `--dgm-paper`, `--dgm-paper-line`,
  `--dgm-navy`, `--dgm-amber`, `--dgm-step`, `--dgm-good`, `--dgm-warm`,
  `--dgm-gate` (dark). Plus the class vocabulary already proven in
  `index.astro`: `.dgm-box`, `.dgm-arrow`, `.dgm-title`, `.dgm-sub`,
  `.dgm-line`, `.dgm-note`, `.dgm-gatebox`, `.dgm-cap`, extended with
  node tints (`.dgm-n-step`, `.dgm-n-good`, `.dgm-n-warm`).
- `src/components/manual/DiagramCard.astro` — the paper-card frame.
  **Requires** `title` and `desc` props; renders
  `<figure class="diagram-figure"><svg role="img" aria-labelledby="…">`
  with `<title>`/`<desc>` wired to `aria-labelledby`, plus an eyebrow and
  an italic-serif caption slot. A diagram authored without title/desc
  must fail (TypeScript required props + a depth test).
- One SVG component per chapter under
  `src/components/manual/diagrams/<Diagram>.astro`, each used through
  `DiagramCard`. Conventions every diagram follows: explicit `viewBox`,
  `preserveAspectRatio="xMidYMid meet"`, `<marker>` arrowheads, no fixed
  pixel `width`/`height` on the root `<svg>` (responsive).

## Interactivity spec

- `src/scripts/reveal.ts` — IntersectionObserver adds `.is-in` to
  `.reveal` and diagram groups as they enter the viewport. Wrapped in a
  `prefers-reduced-motion: no-preference` guard; when motion is reduced,
  everything is shown immediately (no transform).
- **Hover/focus callouts** — diagram nodes carry `data-path` (the real
  source file, e.g. `crates/deadreckon-core/src/gate.rs`). On
  hover/focus/tap a callout shows the path. Nodes are focusable
  (`tabindex="0"`) and the callout is reachable by keyboard.
- `src/components/manual/RunPlayer.astro` (+ script) — a "Follow a run"
  stepper that walks init → plan → provider → sandbox → execute → verify
  → complete with prev/next controls and an optional auto-advance.
  Auto-advance is **off** under reduced-motion. Fully usable by keyboard.
- **Architecture map** on `/manual/` — built from `chapters.ts`; every
  node is an `<a>` to its chapter; keyboard navigable; degrades to a
  plain list if JS is off.
- **Every interaction degrades to static**: with JS disabled the page is
  a readable, navigable manual with all diagrams visible.

## Routes & components

- `/manual/` → `src/pages/manual/index.astro` (intro + architecture map).
- `/manual/<slug>/` → `src/pages/manual/[slug].astro` (Astro
  `getStaticPaths` from `chapters.ts`), directory format.
- `src/layouts/ManualLayout.astro` — wraps `Base.astro`, adds chapter
  chrome: eyebrow, title, AS-BUILT badge, prev/next, in-page TOC.
- `src/components/Header.astro` — add a "Manual" nav link with active
  state on `/manual` routes.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail;
implement; green on `cd /Users/gdc/deadreckon-site && bun run build &&
node --test test/`; conventional-commit local commit; one-line CHANGELOG
entry. Depth tests live in `/Users/gdc/deadreckon-site/test/*.test.mjs`
(`node --test`) and assert over the built `dist/` (parsed with `linkedom`)
and over `src/data/chapters.ts`.

### P1 — Section snapshot, chapter registry, scaffold

- Generate `src/data/as-built-sections.json` from the real AS-BUILT TOC
  (one-time read of `/Users/gdc/deadreckon`).
- Author `src/data/chapters.ts` with all 14 entries per the IA table.
- Create `ManualLayout.astro`, `/manual/index.astro` (stub),
  `/manual/[slug].astro` with `getStaticPaths` over the registry, and
  prev/next + breadcrumb derived from `order`.
- Add "Manual" to `Header.astro`.

Depth tests (`test/structure.test.mjs`):
- `chapter_registry_has_exactly_fourteen_entries`
- `chapter_slugs_and_orders_are_unique_and_contiguous`
- `manual_index_route_is_emitted`
- `every_chapter_route_is_emitted_from_registry`

### P2 — Diagram design system

- `src/styles/diagram.css` (warm `--dgm-*` tokens + class vocabulary).
- `DiagramCard.astro` with required `title`/`desc`, `role="img"`,
  `aria-labelledby`, eyebrow + caption slots.

Depth tests (`test/diagram-kit.test.mjs`):
- `diagram_tokens_are_defined_in_diagram_css`
- `diagram_card_renders_role_img_with_aria_labelledby`
- `diagram_card_without_title_or_desc_fails_build` (negative fixture)

### P3 — Authoring-kit proof (first two diagrams)

- Build the chapter-1 (nested harness) and chapter-2 (layer/split)
  diagrams through `DiagramCard`, establishing the SVG conventions.

Depth tests (`test/svg-contract.test.mjs`, scan `dist/`):
- `every_manual_svg_has_title_desc_and_viewbox`
- `every_manual_svg_sets_preserve_aspect_ratio`
- `no_manual_svg_hardcodes_pixel_width_height`

### P4 — Chapters 1–4 (big idea, two layers, lifecycle, turn loop)

- Full diagram + explanatory prose for each.

Depth tests (`test/chapters-1-4.test.mjs`):
- `chapters_1_to_4_each_have_a_diagram_and_prose_body`
- `lifecycle_diagram_shows_all_seven_phase_ids`
- `each_rendered_chapter_shows_its_as_built_badge`

### P5 — Chapters 5–7 (gate, done-in-english, isolation/sandbox)

Depth tests (`test/chapters-5-7.test.mjs`):
- `gate_chapter_names_dr_gate_and_the_nonce`
- `done_chapter_lists_the_five_check_kinds` (cargo_test, file_exists,
  content_match, build_success, shell)
- `sandbox_chapter_lists_four_modes_and_four_backends`

### P6 — Chapters 8–10 (state/recovery, promotion, providers)

Depth tests (`test/chapters-8-10.test.mjs`):
- `state_chapter_explains_atomic_write_and_per_turn_snapshots`
- `promotion_diagram_shows_working_staging_library_path`
- `providers_chapter_lists_subscription_api_and_smoke_routes`

### P7 — Chapters 11–12 (multi-agent, evidence)

Depth tests (`test/chapters-11-12.test.mjs`):
- `multiagent_chapter_covers_review_fullplan_and_campaign`
- `evidence_chapter_lists_run_docs_provenance_and_manifest`

### P8 — Chapters 13–14 (observability, filesystem/telemetry)

Depth tests (`test/chapters-13-14.test.mjs`):
- `observability_chapter_covers_attach_narrator_and_flight_recorder`
- `filesystem_chapter_renders_the_deadreckon_home_tree`
- `all_fourteen_chapters_have_a_diagram_and_a_prose_body`

### P9 — Interactivity

- `reveal.ts`, hover/focus `data-path` callouts, `RunPlayer.astro`
  stepper, the clickable architecture map. All degrade to static; all
  honor reduced-motion.

Depth tests (`test/interactivity.test.mjs` over `dist/`; runtime smoke
via Playwright in verification):
- `diagram_nodes_with_data_path_are_focusable`
- `run_player_exposes_prev_and_next_controls`
- `architecture_map_links_every_chapter_in_the_registry`
- `motion_scripts_are_guarded_by_prefers_reduced_motion`

### P10 — Cross-cutting friendliness pass

- Responsive diagrams (horizontal scroll/scale on narrow viewports),
  keyboard nav, prev/next + in-page TOC, internal-link integrity, copy
  buttons on command snippets, per-chapter `<meta description>` + OG from
  the registry, `public/sitemap.xml` updated with manual routes, Header
  active state.

Depth tests (`test/polish.test.mjs` over `dist/`):
- `no_internal_link_in_manual_is_broken`
- `every_chapter_page_has_a_meta_description`
- `every_chapter_has_prev_next_except_first_and_last`
- `sitemap_includes_all_manual_routes`

### P11 — Docs + CHANGELOG + /impeccable capture (doc only; no depth test)

- Add a "The Manual" section to `deadreckon-site/README.md`.
- Run `/impeccable` against the built manual; address its findings.
- Capture screenshots (desktop + mobile, light) under
  `deadreckon-site/docs/manual-shots/`.
- Append to `deadreckon-site` CHANGELOG (create if absent):
  ```
  ## The Manual (alpha) — 2026-06-22

  - SVG-diagram "how it works" guide at /manual (14 chapters + map)
  ```
- Log any deferred ideas in `deadreckon-site/docs/MANUAL-NOTES.md`.

## Integration matrix (feature × guarantee)

| Feature | Static fallback | Keyboard | Reduced-motion | Mobile |
|---|---|---|---|---|
| Scroll reveals | all shown | n/a | no transform | ok |
| Hover callouts | inline/visible | focus shows | unaffected | tap shows |
| Run player | shows step 1, all steps listed | prev/next | no autoplay | ok |
| Architecture map | plain chapter list | tab to links | unaffected | stacks |

## Empty / edge states

- JS disabled → full readable manual, every diagram visible, nav works.
- Narrow viewport → diagrams scale or scroll; no clipped text.
- A chapter whose `asBuilt` cites a missing section → build fails (P1 test).

## Config additions

None expected. If a sitemap generator is introduced it must be a build-time
static one; the existing `public/sitemap.xml` may instead be edited by hand.

## Out of scope (explicitly not in this milestone)

- Full-text search over the manual.
- Internationalization / multiple languages.
- A dark-mode toggle (the diagrams are a fixed warm palette by design).
- MDX/CMS authoring or a content pipeline beyond `chapters.ts` + `.astro`.
- Embedding or replaying a *real* deadreckon run (the player is scripted).
- Video, audio, or animated GIF assets.
- Per-chapter comments, analytics dashboards, or A/B testing.

## Dependencies (Tier 1 / 2 / 3 policy)

- **Tier 1 (utility, free):** `linkedom` as a dev-only dependency for
  parsing `dist/` HTML in `node --test`. Playwright is already available
  in the session (MCP) for runtime smokes — add no Playwright npm dep.
- **Tier 2 (architectural, log to a deps note):** none expected. Adding
  any runtime framework or Astro integration requires a logged
  justification in `deadreckon-site/docs/MANUAL-NOTES.md` first.
- **Tier 3 (blocked):** no headless-browser npm dependency, no analytics
  SDK, no backend service.

## Engineering invariants (do not violate)

- **`chapters.ts` is the single source of truth.** Nav, map, prev/next,
  meta, and tests derive from it; no second hardcoded chapter list.
- **One named depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **Every diagram goes through `DiagramCard`** with `title`+`desc`. No raw
  `<svg>` without an accessible name.
- **No new framework; `output: 'static'` stays.** No runtime data fetch
  beyond the existing release-tag call.
- **`prefers-reduced-motion` fully disables motion**, and every
  interaction has a no-JS fallback. These are depth-tested, not optional.
- **`/Users/gdc/deadreckon` is read-only.** Architecture facts are quoted
  from it but never written to it.
- **No silent expansion.** Anything beyond P1–P11 → `MANUAL-NOTES.md`.

## Process invariants

- Phased local commits only, in the `deadreckon-site` repo. No `git push`.
- Each phase ends with `bun run build && node --test test/` green and a
  CHANGELOG line.
- After P11, the `/impeccable` review must pass and screenshots are
  captured under `deadreckon-site/docs/manual-shots/`.
- If a phase reveals a decision bigger than this milestone, stop and log
  it in `MANUAL-NOTES.md`; do not silently expand scope.
