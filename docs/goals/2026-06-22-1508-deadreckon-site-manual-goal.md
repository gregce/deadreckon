GOAL: Build **The Manual** — an SVG-diagram "how it works" guide as a new `/manual` section of the Astro site at `/Users/gdc/deadreckon-site`, that teaches deadreckon's harness-of-harnesses through visuals first and clear prose second, every chapter mapped to a real AS-BUILT section. Today the site is one landing page (`src/pages/index.astro`); nothing explains the *mechanism* — the gate, the turn loop, isolation, promotion, orchestration — to a newcomer. This slice lands 14 interactive chapters + a clickable architecture map, each anchored by a warm "paper-card" SVG diagram (marketing-visual palette) inside the site's bone/signal-mint shell, with scroll reveals, hover file-path callouts, and a "Follow a run" stepper. Verifiable via `/impeccable`. Land it named The Manual.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-22-1508-deadreckon-site-manual-rider.md` — chapter map, phases, depth tests, diagram kit.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate of record; every chapter cites a section.
- `/Users/gdc/deadreckon/docs/CONCEPTS.md` — distilled mental model + voice.
- `/Users/gdc/deadreckon-site/src/pages/index.astro` + `src/styles/{tokens.css,main.css}` — site shell, `.dgm` SVG idiom, nav patterns.
- `/Users/gdc/extract-agentic-engineering/marketing/visuals/*.html` — diagram palette + card idiom.
- Prior `docs/goals/*` pairs set voice + discipline; their Rust toolchain does not apply.

**Posture.** Edits inside `/Users/gdc/deadreckon-site` only; `/Users/gdc/deadreckon` is read-only source-of-truth. Stay on Astro (`output: 'static'`), no new framework; interactivity is islands/vanilla JS, no backend, no runtime fetch beyond the existing release-tag call. No `git push`. Decisions → `deadreckon-site/docs/MANUAL-NOTES.md`, not silent scope.

**Comprehension is the contract.**

- Visual first: every chapter leads with a diagram; prose explains it.
- Every diagram goes through `DiagramCard` with `<title>`+`<desc>`, `viewBox`, `preserveAspectRatio`; keyboard- and screen-reader-navigable.
- `src/data/chapters.ts` is the single source of truth (nav, map, prev/next, meta, tests).
- Every chapter declares its AS-BUILT section; a depth test fails if the cited section is absent from a vendored snapshot.
- `prefers-reduced-motion` fully disables motion; every interaction degrades to a static, navigable manual.

**Chapters (14).** harness-of-harnesses · two layers (skill/binary) · run lifecycle (7 phases) · the turn loop · the gate (can't fake done) · done-in-English · isolation & sandbox · durable state & crash recovery · atomic promotion · providers & BYOK · multi-agent (orchestrate + campaign) · evidence-not-transcript · observability (attach/narrator/flight recorder) · the `~/.deadreckon` filesystem + telemetry. Plus a `/manual/` architecture-map index linking them.

**Interactivity.** Scroll reveals; hover/focus a node → callout with the real source path; a "Follow a run" stepper (init→…→complete); a clickable architecture map. All degrade to static.

**Phases.** Eleven in the rider. Each: named depth test first → implement → `bun run build` + `node --test test/` green → conventional-commit → CHANGELOG line. P11 runs `/impeccable`, captures screenshots, updates README/CHANGELOG.

**Verification.**

- Every rider depth test present and passing; `bun run build` emits all 14 chapter routes + the map index.
- Fidelity: every chapter's cited AS-BUILT section exists; no orphan chapters; `chapters.ts` drives all nav.
- Playwright smoke: map loads, nav to every chapter works, diagrams render, no console errors, reduced-motion honored, no broken internal links.
- `/impeccable` passes on the built manual.
- No edits outside `/Users/gdc/deadreckon-site`; no `git push`.

**Stop when** verification passes, `/impeccable` is green, the `deadreckon-site` README + CHANGELOG note The Manual, and all eleven phases are committed locally.
