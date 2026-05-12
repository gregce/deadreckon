# deadreckon HOWTO

This repo builds a Rust CLI named `deadreckon` for unattended coding runs.
The CLI is the control plane; the TUI is an attachable live dashboard.

## Build And Install Alias

```bash
cd /Users/gdc/deadreckon
make build
make alias-zsh
source /Users/gdc/.zshrc
deadreckon --help
```

The alias points to:

```bash
/Users/gdc/deadreckon/target/release/deadreckon
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
/Users/gdc/.deadreckon
```

For disposable tests, set:

```bash
export DEADRECKON_HOME=/Users/gdc/deadreckon/.try-deadreckon
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

Completed accepted runs are promoted to:

```bash
/Users/gdc/.deadreckon/library/<scope>/<run-id>/
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
deadreckon config set defaults.max_spend 15
deadreckon config set defaults.sandbox auto
deadreckon config set providers.anthropic.api_key "$ANTHROPIC_API_KEY"
```

## Keyless Smoke Run

Use this to prove the harness works without provider keys:

```bash
export DEADRECKON_HOME=/Users/gdc/deadreckon/.try-deadreckon
rm -rf "$DEADRECKON_HOME"

deadreckon run "tiny hello rust" --smoke --sandbox none --max-spend 1
deadreckon list
deadreckon status
```

The default list is compact, project-scoped, and shows eight-character run IDs.
Commands accept unique prefixes, so `deadreckon show 861c51bf` works when that
prefix is unique. Most run-id arguments also accept `latest` or `last`, resolved
to the latest run for the current project. Use `--all` for global history and
`--full` for scripts or exact-copy output.

Capture the latest full run id:

```bash
RUN_ID=$(deadreckon list --full | awk 'NR==2 {print $1}')
```

Then inspect it:

```bash
deadreckon show "$RUN_ID"
deadreckon attach latest
```

## Normal Coding Run

After `init`, the normal path in a git repo is:

```bash
deadreckon run "make a full task productivity tracker in nodejs that allows me to manage my day"
deadreckon attach latest
deadreckon status
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
deadreckon run "goal" --base main --branch dr/my-task # customize worktree branch
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
deadreckon run "goal" --provider cli:claude-code
deadreckon run "goal" --provider anthropic
deadreckon run "goal" --provider openai
deadreckon run "goal" --sandbox sandbox-exec
deadreckon run "goal" --sandbox none
deadreckon run "goal" --max-spend 5
deadreckon run "goal" --max-wall-seconds 1800
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
```

After a run completes, the TUI footer adds lifecycle actions:

```text
a        apply a completed worktree run
b        abandon/discard a completed worktree run
d        print the run narrative docs
m        materialize/export a completed copy/fresh artifact
e        extend a completed copy/fresh run with a follow-up goal
s        show run details
```

The TUI does not start, kill, resume, or undo runs. Use CLI commands for those controls.

## Completion Actions

Completed `run` and `extend` commands show the same lifecycle actions in the CLI:

```text
completed action [a apply, b abandon, d docs, s show, q quit]:
completed action [m materialize, e extend, d docs, s show, q quit]:
```

Worktree runs use `a` to squash-apply changes to the current branch or `b` to
discard the worktree and temporary branch. Copy/fresh runs use `m` to copy the
library artifact into a normal directory, `e` to start a child run from the
completed artifact, `d` to read `RUN-NARRATIVE.md`, or `s` to inspect state and lineage. Pass `--no-hints` to
`run` or `attach` when scripting.

The CLI aliases use friendlier lifecycle names:

```bash
deadreckon export latest --dest ./finished-project   # alias for materialize
deadreckon discard latest                            # alias for abandon
deadreckon next                                      # alias for status
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
deadreckon next
deadreckon show <run-id>
deadreckon show latest
deadreckon apply <run-id>
deadreckon apply <run-id> --autostash
deadreckon apply <run-id> --cleanup --no-confirm
deadreckon apply <run-id> --strategy merge
deadreckon apply <run-id> --strategy cherry-pick --no-confirm
deadreckon abandon <run-id>
deadreckon discard <run-id>
deadreckon abandon <run-id> --keep-branch
deadreckon cleanup --completed
deadreckon cleanup --stale --force
deadreckon cleanup --all --completed --no-confirm
deadreckon prune --completed
deadreckon kill <run-id>
deadreckon kill <run-id> --force
deadreckon resume <run-id>
deadreckon resume <run-id> --from-turn 2
deadreckon resume <run-id> --max-wall-seconds 3600
```

`resume --from-turn N` truncates reconstructed history to turn `N` and continues with turn `N + 1`.

## Undo

Restore the previous turn snapshot:

```bash
deadreckon undo --run <run-id>
```

Restore a specific turn:

```bash
deadreckon undo --run <run-id> --turn 1
```

Undo restores the run working directory from `snapshots/turn-<N>/` and records
an undo trace. For in-place runs, undo restores the original source path because
the agent edited that directory directly.

## Acceptance Gates

If a run has no custom acceptance spec, `dr-gate` checks that the working directory exists and runs `cargo test` when `Cargo.toml` is present.

For custom checks, create:

```bash
/Users/gdc/.deadreckon/runstate/<scope>/runs/<run-id>/acceptance.yaml
```

Example:

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
```

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
/Users/gdc/.claude/projects/
/Users/gdc/.codex/sessions/
/Users/gdc/.cursor/chats/
```

Override source roots for testing:

```bash
DEADRECKON_IMPORT_CLAUDE_ROOT=/tmp/claude deadreckon import claude-code
DEADRECKON_IMPORT_CODEX_ROOT=/tmp/codex deadreckon import codex
DEADRECKON_IMPORT_CURSOR_ROOT=/tmp/cursor deadreckon import cursor
```

Imports create synthetic `imported-<hash>` runs with normalized traces and provenance.

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
deadreckon kill <run-id> --force
deadreckon resume <run-id>
```

If the alias does not exist in the current shell:

```bash
source /Users/gdc/.zshrc
alias deadreckon
```
