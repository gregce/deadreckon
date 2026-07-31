# deadreckon HOWTO

This repo builds a Rust CLI named `deadreckon` for unattended coding runs.
The CLI is the control plane; the TUI is an attachable live dashboard.

DeadReckon is for people who already use agent CLIs and need unattended,
sandboxed, auditable work with a real definition of done. It is the harness around
those harnesses: provider CLIs do the coding while DeadReckon owns isolation,
done criteria, lifecycle, logs, evidence, recovery, and promotion. It supervises
agent CLIs instead of replacing them.

For a first serious run:

```bash
deadreckon start "build the app"
deadreckon attach latest
deadreckon status latest
deadreckon list
deadreckon finish latest
```

## New User Path

Use `start` when you want DeadReckon to choose the right first path and tell you
exactly what is missing:

```bash
deadreckon start "build the app"
deadreckon attach latest
deadreckon status latest
deadreckon list
deadreckon finish latest
```

In a TTY, `start` presents selection prompts for mode, provider, missing done
criteria, source mode, and final confirmation. For scripts, pass explicit flags
with `--yes`, `--plain`, `--quiet`, or `--json` so it never waits for input.

If setup is incomplete, `start` stops before launching work and prints concrete
`try:` lines for setup, done criteria, or source mode. Paste the suggested
commands, then run the same `deadreckon start "build the app"` command again.

If this repo already has completed deadreckon history, TTY `start` can offer a
follow-up from the latest extendable run, a new review pass, or a new full-plan
pass. For scripts, use preview first:

```bash
deadreckon start "add settings" --preview --plain
deadreckon extend <run-id> "add settings"
deadreckon start "add settings" --mode review --yes
deadreckon start "add settings" --mode full-plan --yes
```

When done criteria already exist, TTY `start` shows the current criteria and
offers keep, view, check, update, or cancel before launch. You can inspect or
change the same contract directly:

```bash
deadreckon def-done show
deadreckon def-done check
deadreckon def-done "what should count as done"
```

### Durable guided-start posture

When `start` selects a single, review, full-plan, or campaign shape, it freezes
the approved goal, definition of done, policy, launch plan, source digest, and
source revision under one Job ID before any agent turn begins. It then starts
`deadreckon supervisor serve --once <job-id>` with detached stdio. The ordinary
commands accept that Job ID:

```bash
deadreckon status <job-id>
deadreckon attach <job-id>
deadreckon list
deadreckon finish <job-id>
```

Closing the launching terminal does not cancel this detached process. That is
process-level durability. For restart-at-login posture, explicitly install and
start the per-user service:

```bash
deadreckon supervisor install
deadreckon supervisor start
deadreckon supervisor status
```

On macOS this manages a user LaunchAgent. On Linux it manages a systemd user
unit. The generated definition pins the exact current binary,
`DEADRECKON_HOME`, and `PATH`; installation refuses to overwrite an unmanaged
same-name unit. `install` does not activate a new service. `start` enables and
reloads the current definition. To stop it without removing the definition:

```bash
deadreckon supervisor stop
```

The service definitions and lifecycle commands are implemented and tested
without invoking the host service manager in the test suite. A live
machine-restart drill is still an operator acceptance step; do not infer that
it ran from a successful unit-rendering test.

Durable Single, Graph and Campaign Jobs use a strict two-key completion rule:

1. The frozen deterministic checks pass under a real sandbox and produce a
   valid native `dr-gate` marker.
2. A fresh read-only semantic judge returns `achieved` for the approved goal,
   contract, diff, and cited evidence.

The semantic judge cannot override failed checks. For a Single Job, `revise`
gives the worker another bounded correction opportunity. For a Graph or
Campaign parent, `revise` starts a fenced, bounded parent-only repair attempt
over the merged result without rerunning successful children. `uncertain`, an
unavailable judge, an uncontained gate, or a receipt-sealing error produces
`NEEDS_REVIEW`.

Durable review and full-plan work always use at-end delivery. The conductor
first merges in isolation. The supervisor then copies the merged result into a
run with the parent Job ID, runs the native gate, asks a fresh read-only
semantic judge, validates the receipt and promotes that parent. `finish`
revalidates the receipt and result-tree digest before exporting the
receipt-bound parent.

A durable Campaign Job can recover an exactly linked persisted sub-plan. Before
parent verification, it rebuilds the campaign's worst-of roll-up from the leaf
evidence and compares it with the stored roll-up. A refused or changed roll-up
fails the parent gate and never reaches the semantic judge.

Public `deadreckon extend <run-id> "<goal>"` and a follow-up selected through
guided `start` create a durable Single Job. Before queueing, DeadReckon freezes
the completed parent identity, parent state digest, promoted artifact tree
digest, and—when the parent is a verified Job—its receipt digest. The child
revalidates those facts before it writes continuation evidence. Use
`deadreckon finish <job-id>` after the follow-up verifies; launch-time `--dest`
is refused so continuation cannot mutate an operator destination before
verification.

## Normal Single Run

Use `run` directly when you already know this should be one supervised coding
run:

```bash
deadreckon run "goal"
deadreckon attach latest
deadreckon finish latest
```

`run` is the canonical power-user command for source-mode flags, spend caps,
sandbox overrides, and one-run scripting. Its ordinary isolated form now
creates and detaches a durable Single Job with the same independent semantic
judge and combined receipt as `start --mode run`. Preview, explicit
`--in-place`, explicit `--sandbox none`, and historical chain-child execution
remain foreground compatibility paths; they cannot issue a trusted Job
receipt.

## Multi-Agent Work

Use `orchestrate` directly when the goal needs a coder/reviewer pass or a
planner-led split into child runs:

```bash
deadreckon orchestrate review "goal" --yes
deadreckon orchestrate full-plan "goal" --n 4 --yes
deadreckon attach latest
deadreckon finish latest
```

`start --mode review` and `start --mode full-plan` route through the same
orchestration machinery when you want the guided front door first. Ordinary
direct orchestration also creates a durable Graph Job. Its child graph still
uses the established conductor under one parent lease. The supervisor
verifies, receipts and promotes the same-ID parent result after the merge. A
semantic `revise` starts a bounded parent-only repair turn and retains the
successful child results. Repeated repair rounds are linked into the receipt
evidence; `uncertain` remains distinct from a requested revision.

New `chain` execution compiles supported chain policy into a durable linear
Graph Job; explicit `--sandbox none` and historical `chain run|resume
<chain-id>` remain untrusted legacy conductors. Direct `campaign` creates a
durable Campaign Job. Executing a stored plan with `fork` creates a durable
Graph Job. Preview and internal child modes remain foreground by design.
Historical chain extension remains process-owned; run follow-ups use durable
Single Jobs.

## Build And Install Alias

```bash
cd /path/to/deadreckon
make build
make alias-zsh
source ~/.zshrc
deadreckon --help
```

The alias points to:

```bash
<your checkout>/target/release/deadreckon
```

## Shell Tab Completion

`deadreckon init` installs shell completion automatically when it can detect
your shell. To install or repair it later:

```bash
deadreckon completion install
```

That writes the generated script to the normal location for your shell. For zsh
it also adds a managed `.zshrc` block that loads `~/.zsh/completions`.

Override detection or print a raw script:

```bash
deadreckon completion install --shell zsh
deadreckon completion zsh > ~/.zsh/completions/_deadreckon
```

## Make Targets

```bash
make help             # list targets
make build            # cargo build --release
make verify           # release build, tests, clippy, fmt check
make smoke            # keyless local smoke run
make doctor           # run deadreckon doctor
make stress           # 5 concurrent runs; defaults to 10 minutes
make clean-runtime    # remove repo-local smoke state
```

Override runtime locations or stress duration:

```bash
DEADRECKON_HOME=/tmp/deadreckon-home make smoke
STRESS_SECONDS=30 make stress
```

## Runtime State

By default, deadreckon writes runtime state to:

```bash
~/.deadreckon
```

For disposable tests, point it somewhere throwaway:

```bash
export DEADRECKON_HOME=/tmp/try-deadreckon
```

Per-run artifacts include:

```text
state.json
history.json
events.jsonl
traces.jsonl
spend.jsonl
provenance.jsonl
snapshots/turn-<N>/
proofs/turn-acceptance.json
working/ or library/<scope>/<run-id>/
```

For a durable Job, control truth and frozen authority live separately:

```text
~/.deadreckon/jobs/<job-id>/
  job.json
  job-events.jsonl
  projection.json
  lease.json
  launch-plan.json
  acceptance.yaml
  authority.json
  supervised-child.json
  receipt.json                 # after parent two-key verification
```

The HMAC key is outside the run and Job workspaces under
`~/.deadreckon/gate-keys/`. The worker sandbox denies that key store and makes
Job authority/proof paths read-only or inaccessible. Rich turn, spend, trace,
snapshot, and narrative evidence remains in the normal run directory; the Job
event log owns lifecycle truth.

Completed accepted runs are promoted to:

```bash
~/.deadreckon/library/<scope>/<run-id>/
```

The directory where you launch `deadreckon run` is recorded as `launch-dir`.
In a git repo, the normal default is a separate worktree on a `dr/...` branch.
Your checkout is left unchanged until `deadreckon apply <run-id>`.

## First Run

Interactive setup:

```bash
deadreckon init
deadreckon doctor
```

Non-interactive examples:

```bash
deadreckon init --provider cli:codex --sandbox auto --max-spend 10 --no-confirm
deadreckon init --provider anthropic --api-key "$ANTHROPIC_API_KEY" --sandbox auto --max-spend 10 --no-confirm
deadreckon init --provider openai --api-key "$OPENAI_API_KEY" --sandbox auto --max-spend 10 --no-confirm
```

Inspect or edit config:

```bash
deadreckon config get defaults.provider
deadreckon config provider
deadreckon config provider cli:codex
deadreckon config model
deadreckon config model gpt-5.1-codex --provider cli:codex
deadreckon config set defaults.max_spend 15
deadreckon config set defaults.sandbox auto
deadreckon config set providers.anthropic.api_key "$ANTHROPIC_API_KEY"
```

## Keyless Smoke Run

Use this to prove the harness works without provider keys:

```bash
export DEADRECKON_HOME=/tmp/try-deadreckon
rm -rf "$DEADRECKON_HOME"

deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
deadreckon list
deadreckon status
```

The default list is compact, project-scoped, and shows eight-character run IDs.
Commands accept unique prefixes, so `deadreckon show 861c51bf` works when that
prefix is unique. Most run-id arguments also accept `latest` or `last`, resolved
to the latest run for the current project. Use `--all` for global history and
`show` when you need full paths, IDs, traces, docs, and next actions.

Inspect the latest run:

```bash
deadreckon show latest
```

Then attach to or finish it:

```bash
deadreckon attach latest
```

## Normal Single Run Details

After `init`, the normal path in a git repo is:

```bash
deadreckon run "make a full task productivity tracker in nodejs that allows me to manage my day"
deadreckon attach latest
deadreckon status
deadreckon finish latest
deadreckon apply latest --autostash --cleanup
```

`run` prints the run id and attach command as soon as state is created:

```text
started run <short-id> (<full-id>)
attach: deadreckon attach <short-id>
```

Before it creates state or files, `run` prints a preview. In a clean git repo,
the preview shows `mode: worktree`, the `dr/...` branch, the base ref, and the
worktree path. Use `--yes` to skip the confirmation in scripts or `--preview`
to print the block and exit.

Mode overrides:

```bash
deadreckon run "goal" --fresh                         # old empty working dir
deadreckon run "goal" --from .                        # gitignore-aware copy
deadreckon run "goal" --worktree                      # force git worktree
deadreckon run "goal" --base main --branch-name dr/my-task # customize worktree branch
deadreckon run "goal" --allow-dirty                   # seed dirty files into worktree
deadreckon run "goal" --in-place --i-know-its-a-lot   # edit current dir directly
```

Outside git, interactive `run` offers git init, copy mode, or cancel.
Non-interactive runs outside git need an explicit mode:

```bash
deadreckon run "goal" --from . --yes
deadreckon run "goal" --fresh --yes
git init && deadreckon run "goal" --yes
```

The default config from `deadreckon init` supplies the provider, sandbox, `$10` spend cap, and `3600` second wall-clock cap. Use flags only when overriding those defaults:

```bash
deadreckon run "goal" --provider cli:codex
deadreckon run "goal" --provider cli:codex --model gpt-5.1-codex
deadreckon run "goal" --provider cli:claude-code
deadreckon run "goal" --provider cli:claude-code --model sonnet
deadreckon run "goal" --provider anthropic
deadreckon run "goal" --provider openai
deadreckon run "goal" --sandbox sandbox-exec
deadreckon run "goal" --sandbox none
deadreckon run "goal" --max-spend 5
deadreckon run "goal" --max-wall-seconds 1800
```

`deadreckon run --preview "goal"` prints the provider and model before creating
state. `--model` is per-run; `deadreckon config model ...` makes the model
default for that provider. For CLI providers, `provider default` means
deadreckon delegates model choice to Codex or Claude Code unless you pass
`--model`.

Run docs use their own provider route. By default deadreckon uses
`defaults.doc_provider`, then an installed subscription CLI (`cli:codex` before
`cli:claude-code`), then the run provider. Override it per run when needed:

```bash
deadreckon run "goal" --doc-provider cli:codex
deadreckon config set defaults.doc_provider cli:codex
```

Spend above `$50` requires confirmation:

```bash
deadreckon run "large goal" --max-spend 75 --i-know-its-a-lot
```

## TUI

Open the dashboard:

```bash
deadreckon attach <run-id>
```

The TUI shows:

```text
run id, status, phase, goal
working directory
per-turn timer
compact spend/context telemetry
wide center-left tool-call/provider activity stream
narrow center-right live files being generated
bottom supervised process/status panel
completed-run Markdown docs view
provider activity from live Codex session logs
```

Keys:

```text
Ctrl-D   detach without killing the run
q        quit
Esc      quit
Tab      move focus between panels
j/k      scroll focused panel
PgUp/PgDn scroll focused panel by a page
?        full key reference overlay (any key closes)
```

After a run completes, the TUI footer adds lifecycle actions:

```text
a        apply a completed worktree run (y confirms)
x        abandon/discard a completed worktree run (y confirms)
d        toggle formatted RUN-NARRATIVE.md docs in the main panel
m        export a completed copy/fresh artifact
e        extend a completed copy/fresh run with a follow-up goal
s        show run details
```

The docs view uses a Markdown parser and ratatui styles for headings, bullets,
inline code, fenced code blocks, links, horizontal rules, and task lists. Press
`d` again to return to provider activity. Scroll it with the same `j/k`,
arrow-key, page-key, and mouse-wheel bindings.

## Generated Docs

Every run writes `.deadreckon/docs/_incremental.jsonl` at turn boundaries and
emits a `docs_checkpoint` event. Completion then runs the doc polish step unless
you passed `--no-docs`.

```bash
deadreckon doc latest
deadreckon doc latest --kind as-built
deadreckon doc latest --kind decisions
deadreckon doc latest --polish --overwrite
deadreckon doc latest --polish --doc-provider cli:codex --max-spend 0.25
```

`deadreckon doc latest --kind decisions` prints the implementation decision
ledger: design decisions, deviations, tradeoffs, open questions, and
multi-alternative decision details. The live working copy for those sections is
`implementation-notes.html` in the run working directory.

The polish preview lists the provider, why it was selected, the four narrator
subskills, token budget, max spend, and inputs hash. `--no-confirm` skips the
prompt for scripts. Results are recorded in `polish.json` with one status line
per subskill.

The TUI does not start, kill, resume, or undo runs. Use CLI commands for those controls.

## Completion Actions

Completed `run` and `extend` commands show the same lifecycle actions in the CLI:

```text
completed action [a apply, x abandon, d docs, s show, q quit]:
completed action [m export, e extend, d docs, s show, q quit]:
```

Worktree runs use `a` to squash-apply changes to the current branch or `x` to
abandon the worktree and temporary branch. Copy/fresh runs use `m` to export the
library artifact into a normal directory, `e` to start a child run from the
completed artifact, `d` to read `RUN-NARRATIVE.md`, or `s` to inspect state and lineage. Pass `--no-hints` to
`run` or `attach` when scripting.

The CLI aliases use friendlier lifecycle names:

```bash
deadreckon finish latest                            # choose apply or export from run mode
deadreckon def-done "builds and passes tests"           # write done criteria
deadreckon export latest --dest ./finished-project   # copy an artifact to a normal directory
deadreckon keep latest --autostash --cleanup         # alias for apply
deadreckon abandon latest                            # remove a temporary worktree/branch
deadreckon status                                    # latest state and next action
deadreckon watch latest                              # alias for attach
deadreckon stop latest                               # alias for kill
deadreckon continue latest                           # alias for resume
deadreckon inspect latest                            # alias for show
deadreckon docs latest                               # alias for doc
deadreckon prune --completed                         # alias for cleanup
```

`extend` is still available for completed worktree runs from the CLI. It creates
a new `dr/...` branch from the parent run's branch, so the follow-up keeps the
parent changes without applying them to your checkout first:

```bash
deadreckon extend <worktree-run-id> "continue with the next change"
```

## List, Show, Kill, Resume

```bash
deadreckon list
deadreckon list --all
deadreckon status
deadreckon show <run-id>
deadreckon show latest
deadreckon inspect latest
deadreckon finish latest
deadreckon finish latest --dest ./finished-project
deadreckon finish latest --autostash --cleanup
deadreckon apply <run-id>
deadreckon apply <run-id> --autostash
deadreckon apply <run-id> --cleanup --no-confirm
deadreckon apply <run-id> --git-strategy merge
deadreckon apply <run-id> --git-strategy cherry-pick --no-confirm
deadreckon keep <run-id> --autostash --cleanup
deadreckon abandon <run-id>
deadreckon abandon <run-id> --keep-branch
deadreckon cleanup --completed
deadreckon cleanup --stale --escalate
deadreckon cleanup --all-scopes --completed --no-confirm
deadreckon prune --completed
deadreckon kill <run-id>
deadreckon stop <run-id>
deadreckon kill <run-id> --escalate
deadreckon resume <run-id>
deadreckon continue <run-id>
deadreckon resume <run-id> --from-turn 2
deadreckon resume <run-id> --max-wall-seconds 3600
```

`finish` is the easiest completion command. For a worktree run it routes to
`apply`; for a fresh/copy run it routes to `export`; for an in-place run it
prints review, docs, and undo guidance.

Re-running `deadreckon apply <run-id>` after the changes are already on the
target branch is safe: deadreckon reports `already applied` instead of creating
or failing an empty commit. Add `--cleanup` to remove the temporary worktree and
branch in that already-applied state.

`resume --from-turn N` truncates reconstructed history to turn `N` and continues with turn `N + 1`.

## Undo

Restore the previous turn snapshot:

```bash
deadreckon undo <run-id>
```

Restore a specific turn:

```bash
deadreckon undo <run-id> --turn 1
```

Undo takes the id positionally, like every other lifecycle verb. `--run <id>`
still works as a deprecated alias.

Given a chain id, undo unwinds that chain's last applied step instead:

```bash
deadreckon undo <chain-id>
```

## Ordered work runs one node at a time

A plan that lands each node as it passes (`apply: per-node` — what `chain`
does, and what `start` picks for a goal that spells out an order) is serial by
construction, even where nodes do not depend on each other. Each node branches
off the tip the previous node just landed on; running siblings in parallel
would race on that same base. If the pieces are independent and speed matters
more than incremental landing, at-end apply runs them in parallel and merges
once.

Undo restores the run working directory from `snapshots/turn-<N>/` and records
an undo trace. For in-place runs, undo restores the original source path because
the agent edited that directory directly.

## Done Criteria

If a run has no custom acceptance spec, `dr-gate` checks that the working directory exists and runs `cargo test` when `Cargo.toml` is present.

The normal path is English first:

```bash
deadreckon def-done "build, load in a browser, and show no console errors"
deadreckon def-done add "users can save drawings"
deadreckon def-done check
deadreckon run "goal"
```

`def-done` asks the configured provider to convert plain-English criteria into `.deadreckon/acceptance.yaml`. Short pack names are available without a provider when they fit:

```bash
deadreckon def-done add build
deadreckon def-done add node
deadreckon def-done add static-site
deadreckon def-done add browser
deadreckon def-done add playwright
deadreckon def-done add vite
deadreckon def-done add nextjs
deadreckon def-done add python
```

Runs and chains prompt before starting when no project acceptance file exists. Helper scripts generated by packs or drafts live under `.deadreckon/acceptance/` and are copied into the run workspace before `dr-gate` executes.

The compiled YAML looks like this:

```yaml
name: notes check
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: content_match
    path: "{working_dir}/notes.md"
    pattern: "dead reckoning"
  - kind: build_success
    cwd: "{working_dir}"
```

Supported check kinds:

```text
cargo_test
file_exists
content_match
build_success
shell
```

`content_match` treats `pattern` as a regex when valid, with substring fallback for simple text. Shell, build, and cargo checks record duration plus clipped stdout/stderr so failures are readable.

The acceptance marker is signed with a run-local nonce. A marker written by the agent is refused.

## Import Existing Tool History

Read-only imports:

```bash
deadreckon import claude-code
deadreckon import codex
deadreckon import cursor
```

Default source roots:

```text
~/.claude/projects/
~/.codex/sessions/
~/.cursor/chats/
```

Override source roots for testing:

```bash
DEADRECKON_IMPORT_CLAUDE_ROOT=/tmp/claude deadreckon import claude-code
DEADRECKON_IMPORT_CODEX_ROOT=/tmp/codex deadreckon import codex
DEADRECKON_IMPORT_CURSOR_ROOT=/tmp/cursor deadreckon import cursor
```

Imports create synthetic `imported-<hash>` runs with normalized traces and provenance.

## Choosing Models

Every provider route ships a model catalog. List it:

```sh
deadreckon models                  # catalog for your default provider
deadreckon models cli:codex        # catalog for a specific route
deadreckon models --all            # every credentialed route
deadreckon models cli:codex --json # machine-readable
```

Each catalog has one recommended entry and marks your configured default
(`deadreckon config model <id>` sets it). "provider default" means
deadreckon passes no model argument and the CLI decides.

Pick a model at launch:

- `start`, `run`, and `chain` take `--model <id>`.
- Interactive `start` offers a model picker right after the provider
  choice (skipped when `--model` is given or the catalog has one entry).
- `orchestrate full-plan` adds per-role flags: `--planner-model`,
  `--model` (children), and `--child-model IDX=MODEL` per child index.
- `orchestrate review` takes `--coder-model` and `--reviewer-model`.
- `campaign run` takes `--planner-model` and `--model`.

Resolution order everywhere: per-role flag, then generic `--model`, then
`defaults.model` from config, then the catalog's recommended entry, then
the provider default. Previews and the provider-roles table echo the
resolved model so you can confirm before any spend.

## When A Provider Refuses To Launch

On a terminal, a launch that resolves to an unusable route (missing
credentials, logged-out CLI) does not dead-end: deadreckon opens the
provider picker with detected CLIs and their live login state, so you can
pick another route for that launch, keep the original route anyway, or
cancel to see the original refusal with its `try:` line. Off a terminal
(scripts, CI), the refusal is unchanged and exits nonzero.

## Sandboxes

Backends:

```text
auto
sandbox-exec
bwrap
docker
none
```

Examples:

```bash
deadreckon run "goal" --sandbox auto
deadreckon run "goal" --sandbox sandbox-exec
deadreckon run "goal" --sandbox docker
deadreckon run "goal" --sandbox none
```

Use `none` only for explicit local verification. `doctor` reports which sandbox binaries are available.

## Doctor

```bash
deadreckon doctor
```

Checks include:

```text
config present and parseable
provider credentials or CLI binaries
sandbox binaries and versions
disk space
runstate write permissions
OS/kernel sanity
Claude/Codex subscription CLI availability
```

HTTP provider pings are conservative. Enable live ping behavior explicitly:

```bash
DEADRECKON_DOCTOR_PING=1 deadreckon doctor
```

## Stress And Verification

Full normal verification:

```bash
make verify
```

Ten-minute 5-run stress:

```bash
make stress
```

Short stress while developing:

```bash
STRESS_SECONDS=30 make stress
```

The stress test asserts completed states, unique scopes, provenance identity, and no leftover lock files.

## Configuration

Runtime config lives at `~/.deadreckon/config.toml`. Set `DEADRECKON_HOME` to
relocate all runtime state (config, runstate, library) for isolated local runs
or tests.

Switch providers, models, or defaults:

```bash
deadreckon init --provider cli:codex --sandbox auto --max-spend 10
deadreckon init --provider anthropic --api-key "$ANTHROPIC_API_KEY"
deadreckon config provider cli:claude-code
deadreckon config model sonnet --provider cli:claude-code
deadreckon config set defaults.max_spend 15
```

Override per run:

```bash
deadreckon run "goal" --provider cli:codex --model gpt-5.1-codex
deadreckon run --preview "goal"     # show route and model, don't start
```

### Sandbox Backends

| Backend | What it is |
|---|---|
| `auto` | Picks the right native sandbox for your OS (default) |
| `sandbox-exec` | macOS native |
| `bwrap` | Linux native (bubblewrap) |
| `docker` | Opt-in container sandbox |
| `none` | Off (unsafe for real unattended work) |

Check what your machine supports with `deadreckon doctor`. See
[Sandboxes](#sandboxes) above for run examples.

## Full Command Reference

The production model is a small set of verbs; every other command stays
findable through `deadreckon help-all`, `<command> --help`, and completion.

Default production model:

```text
start         begin supervised agent work
attach        open the live dashboard
status        latest run and next action for this project
list          find runs and plans
finish        choose apply or export from completed work
doctor        check config, providers, sandboxes, disk, runtime
init          create local config
def-done      compile English "done" criteria into checks
kill          stop a live run, chain, or plan
resume        continue an interrupted run
cleanup       clean abandoned, stale, or completed worktrees
help-all      show every advanced and compatibility command
```

Power-user and advanced commands:

```text
run           start one unattended coding run directly
orchestrate   one-command review / full-plan multi-agent runs
chain         plan and run ordered multi-step work
plan          write an orchestration plan (no child runs yet)
fork          start a plan's ready child runs
merge         compose plan children into one promoted artifact
apply         apply a completed worktree run to your branch
export        copy a completed artifact to a normal directory
extend        continue from a completed run
show          inspect state, lineage, spend, files
doc           print or export run documentation
rewind        preview or apply a provider flight checkpoint
history       search durable traces and provenance
library       query promoted run artifacts
providers     list provider routes (detect probes availability)
detect        probe registered providers
config        inspect or edit config keys
update        check for or apply self-updates
learn         index run evidence and propose improvements
improve       run evidence-gated self-improvement candidates
import        normalize histories from other coding tools
undo          restore a previous turn snapshot
abandon       remove a worktree run and temporary branch
completion    install shell tab-completion
```

Aliases: `keep` → `apply`, `materialize` → `export`, `discard` → `abandon`,
`prune` → `cleanup`, `follow-up` → `extend`, `continue` → `resume`,
`stop` → `kill`, `next` → `status`.

## Troubleshooting

Run:

```bash
deadreckon doctor
deadreckon list
deadreckon show <run-id>
```

Common fixes:

```bash
deadreckon init
deadreckon config set defaults.provider cli:codex
deadreckon config set defaults.sandbox auto
deadreckon kill <run-id> --escalate
deadreckon resume <run-id>
```

If the alias does not exist in the current shell:

```bash
source ~/.zshrc
alias deadreckon
```
