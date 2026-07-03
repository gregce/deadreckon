# Product

## Register

product

## Users

DeadReckon is for engineers supervising coding agents during long-running,
local-first development work. They are usually in a terminal, often letting an
agent run unattended, and need to understand current state, risk, cost,
evidence, and the next safe action without reading a raw chat transcript.

## Product Purpose

DeadReckon is a Rust CLI harness around existing agent CLIs and API routes. It
creates durable run state, supervises agent turns in a sandbox, records spend
and provenance, gates completion through a binary-owned watchdog, and promotes
accepted work into auditable artifacts. Success means the operator can start,
watch, pause, resume, finish, apply, or abandon agent work with confidence that
the interface is reporting file-backed reality rather than model intent.

## Brand Personality

Calm, exacting, operator-grade. The product should feel like mission control for
software work: dense when density helps, restrained when attention is scarce,
and direct about uncertainty, failure, and recovery.

## Anti-references

Do not make DeadReckon look or behave like a marketing dashboard, a decorative
AI chat wrapper, or a framework showcase. Avoid ornamental motion, vague status
copy, hidden state, modal-first workflows, and any UI that asks the operator to
trust a summary without citing the durable artifact behind it.

## Design Principles

- Evidence before elegance: every important status, warning, and recommendation
  should be traceable to durable files or existing command paths.
- One control vocabulary: keys, footers, command mode, modals, and plain output
  should use the same verbs and recovery language across surfaces.
- Speed protects trust: quit, Escape, and input handling must stay responsive
  even while ledgers, panes, and summaries refresh.
- Density with hierarchy: show the voyage, current detail, timeline, and next
  action without forcing drill-in, but keep the current focus obvious.
- Decoration is optional information: motion and effects may acknowledge state,
  never carry state that disappears under reduced or disabled motion.

## Accessibility & Inclusion

The terminal UI must remain usable without color, without animation, and in
plain/off-TTY output. Motion respects `[ui] motion = reduced|off`, status is
encoded with text as well as glyphs, and narrow terminals preserve core status,
help, and next-action affordances.
