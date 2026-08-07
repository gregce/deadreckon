# PRODUCT.md — deadreckon (macOS operator console)

## What this is
A native macOS menubar + window app for driving deadreckon, the durable coding-agent harness. The operator starts goals, watches runs execute, steers or kills them, reviews the evidence, and approves verified results. The CLI/TUI remain as escape hatches; the app is the daily driver.

## Who uses it
One operator (a senior engineer) running several long, unattended coding agents at once — often overnight. They return on a notification, triage in seconds, inspect deeply when something needs judgment, and approve work only when the harness has proven it. They know products like Conductor, Cursor, and Linear; they should never need deadreckon's internal vocabulary to use this app.

## The job the app does (in priority order)
1. Always know what is going on — glanceable truth for every run, at all times.
2. See, inspect, and understand what happened — evidence, not vibes.
3. Show clear progression against a plan — where the run is, what's next.
4. Make the done contract and its checks first-class — what "done" means, verified how.
5. One obvious mental model: Projects contain Goals; a Goal executes as a Run; a finished Run is Reviewed and Approved (or sent back, or discarded).

## Product decisions (operator, 2026-08-07)
- **Nomenclature: full plain language.** New Goal, Runs, Checks, Review & Approve, Send back, Stop, Settings. Nautical terms (Lay Course, Gate Queue, Harbor, Rudder, Binnacle) do not appear in the UI; the CLI keeps them.
- **Primary noun: Goal → Run.** You create a Goal; its execution is a Run.
- **Theme: dark-only, hard-line.** One committed look per the operator's anchor (Conductor dark): charcoal surfaces, crisp 1px borders, warm orange accent. No light mode for now.
- **IA: projects → runs tree** in an always-visible left sidebar; selecting a run fills the center. Needs-review items badge and can group at top.
- **New Goal flow: project folder first**, then everything visible before preview (goal, agent, model, budget, what-done-means), mirroring the CLI's own decision order. No hidden steps.
- **Simplify, never remove.** All current capabilities stay; presentation and language get simpler. Trust rules are untouchable: no override affordances, verified-only claims, refusals rendered honestly.

## Constraints
- SwiftUI, macOS 14+, menubar-first (LSUIElement), vendored signed CLI is the only write path (JSON envelopes), file tails are the read path. DeadreckonKit holds logic; views stay thin.
- The app must remain honest under degradation (binary missing, supervisor down, empty fleet) — plain-language honesty, not jargon.

## Anti-goals
- Not a chat app; the center of a run is evidence and narrative, not a transcript you drive.
- No decorative dashboards; every element answers one of the five jobs above.
