use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::narrative::{AttachViewMode, NarrativeVisualMode};

const TOP_LEVEL_TEMPLATE: &str = "\
{name} {version}
{about-with-newline}
{usage-heading}
  {usage}{after-help}

Options:
{options}";

const TOP_LEVEL_AFTER_HELP: &str = "\n\nRun `deadreckon help-all` for the full command catalog.";

const COMPLETION_HELP: &str = "\
Lifecycle:
  deadreckon completion install
  deadreckon completion install --shell zsh
  deadreckon completion zsh > ~/.zsh/completions/_deadreckon
  deadreckon completion bash > ~/.local/share/bash-completion/completions/deadreckon
  deadreckon completion fish > ~/.config/fish/completions/deadreckon.fish

The generated script comes from the real clap command tree, so subcommands,
aliases, flags, and shell values stay in sync with `deadreckon --help`.

`install` detects your shell, writes the completion file, and for zsh adds a
managed `.zshrc` block when needed.

Supported shells: bash, elvish, fish, powershell, zsh.";

const INIT_HELP: &str = "\
Lifecycle:
  deadreckon init
  deadreckon doctor
  deadreckon start \"build the thing\"

Use `deadreckon config provider` and `deadreckon config model` later to see or change defaults.";

const CONFIG_HELP: &str = "\
Subcommands:
  deadreckon config get defaults.provider
  deadreckon config set defaults.max_spend 15
  deadreckon config provider
  deadreckon config provider cli:codex
  deadreckon config remove-provider anthropic
  deadreckon config remove-provider
  deadreckon config model
  deadreckon config model gpt-5.1-codex --provider cli:codex

Lifecycle:
  Configure once, then `deadreckon start \"goal\"`. Direct run/orchestrate flags override these defaults.";

const DETECT_HELP: &str = "\
Lifecycle:
  deadreckon detect
  deadreckon detect cli:codex
  deadreckon detect --json
  deadreckon detect ollama --ping

Detect probes registered provider routes. CLI providers check PATH and version
output; API providers check credentials by default and only touch
endpoints when `--ping` is supplied.";

const PROVIDERS_HELP: &str = "\
Lifecycle:
  deadreckon providers list
  deadreckon providers list --all
  deadreckon providers list --models
  deadreckon start \"goal\"
  deadreckon run \"goal\" --provider cli:codex --model gpt-5.1-codex

`providers list` is registry-backed. By default it shows the configured
provider route; `--all` shows every built-in and override provider route.";

const UPDATE_HELP: &str = "\
Lifecycle:
  deadreckon update --check
  deadreckon update --yes

Update reads ~/.deadreckon/install-receipt.json to honor the channel that
installed deadreckon. npm, Homebrew, and cargo installs print the native upgrade
command; shell installs preview the target and backup path, then update in
place by the self-updater.";

const RUN_HELP: &str = "\
Power-user one-run launcher. Most users can begin with `deadreckon start \"goal\"`.

Lifecycle:
  deadreckon run \"build the thing\"
  deadreckon run --goal-file docs/goal.md
  deadreckon run @docs/goal.md
  deadreckon attach latest
  deadreckon status latest
  deadreckon finish latest

Common provider/model changes:
  deadreckon run \"goal\" --provider cli:codex
  deadreckon run \"goal\" --provider cli:codex --model gpt-5.1-codex
  deadreckon run \"goal\" --doc-provider cli:codex
  deadreckon config provider cli:codex
  deadreckon config model gpt-5.1-codex --provider cli:codex

Headless:
  deadreckon run \"goal\" --plain --quiet --sandbox none

Done criteria:
  deadreckon def-done \"build, test, and open in a browser\"
  deadreckon def-done add \"users can save drawings\"
  deadreckon def-done check
  deadreckon def-done show

Modes:
  In a git repo, the default is an isolated worktree.
  Use `--fresh`, `--from <dir>`, `--worktree`, or `--in-place --i-know-its-a-lot` to force a mode.";

const TRY_HELP: &str = "\
Lifecycle:
  deadreckon try
  deadreckon start \"build the real thing\"

Runs a keyless local smoke run in an isolated throwaway workspace, signs the
real dr-gate proof, and prints the proof, story, and lineage paths. It does not
need provider credentials and does not edit your current checkout.";

const START_HELP: &str = "\
Lifecycle:
  deadreckon start \"build the app\"
  deadreckon start --goal-file docs/goal.md
  deadreckon start @docs/goal.md
  deadreckon attach latest
  deadreckon status latest
  deadreckon list
  deadreckon finish latest

Existing project history:
  In a TTY, start can offer a new run, a follow-up from a completed run,
  a review pass, or a full-plan pass when this project already has runs.
  Non-TTY start stays deterministic and prints exact extend/orchestrate commands.

Power users:
  deadreckon run \"build the app\"
  deadreckon orchestrate \"build and review the app\"

Modes:
  auto picks a conservative path and explains why.
  run starts one supervised coding run.
  review runs a coder/reviewer orchestration.
  full-plan plans, forks, and merges multi-agent work.

Interactive:
  In a TTY, start uses selection prompts for mode, provider, done criteria,
  source mode, and final confirmation when flags do not decide them.
  Review/full-plan starts also ask for role providers and full-plan child count.
  Done-criteria prompts show what will be enforced and offer view/check/update
  paths before launch.
  Scripts can use --plain, --quiet, --json, --yes, or explicit --mode flags to
  keep start deterministic and non-prompting.";

const ORCHESTRATE_HELP: &str = "\
Power-user multi-agent launcher. `deadreckon start --mode review` and
`deadreckon start --mode full-plan` route through this machinery.

Lifecycle:
  deadreckon orchestrate review \"build the thing\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes
  deadreckon orchestrate full-plan \"build the thing\" --planner-provider cli:codex --provider cli:claude-code --yes
  deadreckon orchestrate full-plan --goal-file docs/goal.md --planner-provider cli:codex --provider cli:claude-code --yes
  deadreckon orchestrate @docs/goal.md
  deadreckon orchestrate \"build the thing\"
  deadreckon attach <plan-id>
  deadreckon merge <plan-id>
  deadreckon finish <plan-id>

Orchestrate is the one-command multi-agent wrapper. Review mode is a coder
provider route followed by a fresh reviewer/fixer provider route. Full-plan
mode asks a planner provider route for child work before fork and merge. Merge
repair is automatic by default; --no-repair is only for debugging raw conflict
bundles.";

const PLAN_HELP: &str = "\
Advanced orchestration building block. Most users should begin with
`deadreckon start --mode full-plan \"goal\"` or `deadreckon orchestrate`.

Lifecycle:
  deadreckon plan \"build the thing\" --mode review --coder-provider cli:claude-code --reviewer-provider cli:codex
  deadreckon fork <plan-id>
  deadreckon attach <plan-id>
  deadreckon merge <plan-id>

Plan writes ~/.deadreckon/plans/<plan-id>/plan.json plus worker specs. Full-plan
mode asks the planner provider route for a child graph. Review mode writes a
coder child followed by a fresh reviewer child.";

const FORK_HELP: &str = "\
Advanced orchestration building block. Most users reach this through
`deadreckon orchestrate` or `deadreckon start --mode full-plan`.

Lifecycle:
  deadreckon fork <plan-id>
  deadreckon attach <plan-id>
  deadreckon merge <plan-id>

Fork starts the plan coordinator. Each child is still a normal deadreckon run,
using the provider route recorded in plan.json unless you override it here.";

const MERGE_HELP: &str = "\
Advanced orchestration building block. `deadreckon finish <plan-id>` is the
normal completed-plan path.

Lifecycle:
  deadreckon merge <plan-id>
  deadreckon finish <plan-id>
  deadreckon apply <plan-id>
  deadreckon export <plan-id> --dest ./result

Merge composes completed child artifacts into one promoted deadreckon library
entry. The default merge is DAG-aware and automatically attempts bounded repair
for true parallel conflicts; use --no-repair for raw conflict refusal or
--strategy prefer-child --prefer-child <idx> to pick one child deliberately.

Strategy flags are scoped: merge --strategy is the plan merge strategy;
apply/finish use --git-strategy for git operations; chain uses --apply-mode
plus --apply-strategy for per-step git operations.";

const DONE_HELP: &str = "\
Lifecycle:
  deadreckon def-done \"builds, opens in a browser, and has no console errors\"
  deadreckon start \"finish the app\"

Common actions:
  deadreckon def-done \"plain-English definition of done\"
  deadreckon def-done add \"one more thing that must be true\"
  deadreckon def-done add browser
  deadreckon def-done check
  deadreckon def-done show

What it means:
  Write done criteria in English. deadreckon compiles them into checks for dr-gate.
  Start/run/orchestrate prompts should show, check, and update these criteria
  before launch instead of asking you to accept an opaque gate.";

const ACCEPTANCE_HELP: &str = "\
Advanced compatibility command. Most users should use `deadreckon def-done`.

Subcommands:
  deadreckon acceptance setup
  deadreckon acceptance setup \"build, load in a browser, and show no console errors\"
  deadreckon acceptance add browser
  deadreckon acceptance add \"users can add artwork and browse the gallery\"
  deadreckon acceptance init --preset auto
  deadreckon acceptance draft \"generate acceptance for this app\"
  deadreckon acceptance refine \"also require websocket reconnect tests\"
  deadreckon acceptance explain
  deadreckon acceptance check

Lifecycle:
  Write criteria in English; deadreckon compiles them to `.deadreckon/acceptance.yaml`.
  `deadreckon run` automatically uses it, or pass `--acceptance <path>`.
  `dr-gate` remains the pass/fail authority at completion.

Packs:
  auto, basic, build, test, rust, node, static-site, browser, playwright, vite, nextjs, python";

pub(crate) const CHAIN_HELP: &str = "\
Serial multi-step power tool. Start with `deadreckon start \"goal\"` unless you
already know the ordered step sequence.

Chain subcommands:
  deadreckon chain plan \"large goal\" --n 4
  deadreckon chain \"step one\" \"step two\" --yes
  deadreckon chain run latest
  deadreckon chain attach latest
  deadreckon chain status latest
  deadreckon chain show latest --why-failed
  deadreckon chain pause latest --reason \"waiting on review\"
  deadreckon chain resume latest
  deadreckon chain kill latest
  deadreckon chain undo latest --step 2
  deadreckon chain extend latest \"new step goal\"
  deadreckon chain redo latest --step 2
  deadreckon chain hooks list

Lifecycle:
  plan/expand drafts provider-generated steps.
  run/resume executes the conductor.
  attach watches the chain TUI.
  pause/kill/undo/redo recover specific steps.
  extend adds a new step to an existing chain.

Apply flags:
  --apply-mode controls the chain policy: auto, preview, or manual.
  --apply-strategy controls the git strategy for applied steps: squash, merge, or cherry-pick.";

const DOCTOR_HELP: &str = "\
Lifecycle:
  deadreckon doctor
  deadreckon init
  deadreckon start \"goal\"

Doctor checks providers, CLI binaries, sandboxes, disk space, write permissions, and OS details.";

const SEAMS_HELP: &str = "\
Lifecycle:
  deadreckon seams validate policy --config examples/seams/config.toml
  deadreckon seams validate catalog --config examples/seams/config.toml --json
  deadreckon run \"goal\" --no-seams

Seams validate external policy, model-catalog, hooks, and event-sink workers
against fixture JSON before a run. The acceptance gate is not a seam.";

const LIST_HELP: &str = "\
Lifecycle:
  deadreckon list
  deadreckon list --all
  deadreckon show <short-id>
  deadreckon attach <short-id>
  deadreckon finish <short-id>

The default view is compact and scoped to the current project. It includes
normal runs and plans. Use `show` for full run details or
`attach <plan-id>` for orchestration progress.";

const LIBRARY_HELP: &str = "\
Advanced artifact inspection. `deadreckon finish <id>` is the normal way to
keep completed work.

Subcommands:
  deadreckon library list
  deadreckon library search gallery
  deadreckon library show latest

Lifecycle:
  Accepted runs are promoted into the library. Use this to inspect artifacts after completion.";

const FINISH_HELP: &str = "\
Lifecycle:
  deadreckon finish latest
  deadreckon finish <plan-id>
  deadreckon finish latest --autostash --cleanup
  deadreckon finish latest --dest ./finished-project

Finish chooses the right completed-run action:
  worktree run -> apply
  fresh/copy run -> export
  completed plan -> apply to source git, or export with --dest / non-git source
  in-place run -> show review guidance

It still respects confirmations unless you pass `--no-confirm`. When finish routes
to apply, --git-strategy selects squash, merge, or cherry-pick.";

const MATERIALIZE_HELP: &str = "\
Direct artifact copy operation. `deadreckon finish <id>` is the normal
completed-work path.

Lifecycle:
  deadreckon export latest --dest ./finished-project
  deadreckon export <plan-id> --dest ./finished-project
  deadreckon show latest
  deadreckon extend latest \"follow-up goal\"

Use export for completed fresh/copy runs and completed plans. `materialize` is an alias.
Worktree runs use `deadreckon apply` instead.";

const APPLY_HELP: &str = "\
Direct git apply operation. `deadreckon finish <id>` is the normal
completed-work path.

Lifecycle:
  deadreckon show latest
  deadreckon apply latest --autostash --cleanup
  deadreckon apply <plan-id> --cleanup
  deadreckon cleanup latest

Use apply for completed worktree runs and completed orchestration plans. It merges a temporary `dr/...` branch back into your checkout. Use --git-strategy for squash, merge, or cherry-pick.";

const ABANDON_HELP: &str = "\
Lifecycle:
  deadreckon cleanup latest
  deadreckon cleanup --completed

Abandon removes a run's temporary worktree and branch after you decide not to keep it. Most users should use cleanup.";

const CLEANUP_HELP: &str = "\
Lifecycle:
  deadreckon cleanup --completed
  deadreckon cleanup --stale --escalate
  deadreckon cleanup <run-id>

Cleanup removes temporary run worktrees and branches for abandoned, stale, or
completed worktree runs. It does not delete plan state, promoted library
artifacts, or directories exported with `deadreckon export`.";

const EXTEND_HELP: &str = "\
Follow-up launcher for completed runs. In a TTY, `deadreckon start \"goal\"`
can offer this path when the current project already has completed history.

Lifecycle:
  deadreckon extend latest \"add tests\"
  deadreckon attach latest
  deadreckon finish latest

Extend creates a new run from a completed parent artifact and includes parent context by default.";

const DOC_HELP: &str = "\
Direct run-doc reader/regenerator. `deadreckon attach --view narrative` and
`deadreckon finish <id>` surface the common reading paths.

Lifecycle:
  deadreckon doc latest
  deadreckon doc latest --kind as-built
  deadreckon doc latest --kind decisions
  deadreckon doc latest --export ./RUN-NARRATIVE.md

Docs are generated as part of accepted runs and are also shown in the TUI after completion.
The decisions doc is the implementation decision ledger: design decisions, deviations, tradeoffs, open questions, and multi-alternative decision details.
Docs can be regenerated with a provider-backed polish pass:
  deadreckon doc latest --polish
  deadreckon doc latest --polish --doc-provider cli:codex --overwrite
  deadreckon doc latest --polish --max-spend 0.25 --no-confirm";

const ATTACH_HELP: &str = "\
Lifecycle:
  deadreckon attach latest
  deadreckon attach latest --view narrative
  deadreckon attach <plan-id> --view narrative --visual agents
  deadreckon attach <chain-id>
  deadreckon attach <plan-id>
  deadreckon attach <plan-id>:task-0
  deadreckon status latest
  deadreckon finish latest

Attach opens the live TUI for a run, chain, or plan. The default activity view keeps raw logs visible; `--view narrative` shows cited prose plus an evidence-backed visual map. `q`, Esc, and Ctrl-D detach without killing the work.";

const KILL_HELP: &str = "\
Lifecycle:
  deadreckon kill latest
  deadreckon kill <chain-id>
  deadreckon kill <plan-id>
  deadreckon resume latest
  deadreckon cleanup --stale

Kill cancels a run, chain, or plan, writes durable state, and terminates supervised child processes.";

const RESUME_HELP: &str = "\
Lifecycle:
  deadreckon resume latest
  deadreckon resume latest --from-turn 2
  deadreckon attach latest

Resume reconstructs history from traces, skips partial trailing records, and continues the run.";

const UNDO_HELP: &str = "\
Lifecycle:
  deadreckon undo --run latest
  deadreckon show latest

Undo restores a run snapshot. It is mainly for in-place runs or recovery inside a run working directory.";

const SHOW_HELP: &str = "\
Raw inspection command. `deadreckon status` and `deadreckon list` are the
normal orienting commands.

Lifecycle:
  deadreckon show latest
  deadreckon show <plan-id>
  deadreckon show <plan-id>:task-0
  deadreckon show <plan-id> --why-failed
  deadreckon doc latest
  deadreckon finish latest

Show prints run, plan, or plan-child state, mode, lineage, traces, provenance, docs, and suggested next actions.";

const STATUS_HELP: &str = "\
Lifecycle:
  deadreckon status
  deadreckon attach latest
  deadreckon finish latest

Status explains the latest run or plan and what to do next. `next` is an alias.";

const IMPORT_HELP: &str = "\
Lifecycle:
  deadreckon import codex
  deadreckon import codex --preview
  deadreckon import codex --list
  deadreckon import codex --session <id-or-path>
  deadreckon import claude-code
  deadreckon import cli:copilot
  deadreckon import cli:pi
  deadreckon import cursor
  deadreckon show <imported-run-id>

Import is read-only and normalizes concrete provider transcript sessions into
deadreckon trace/provenance shape. CLI providers use their registry [ingest]
descriptors; Cursor remains a SQLite adapter.";

const HISTORY_HELP: &str = "\
Lifecycle:
  deadreckon history grep \"failed build\"
  deadreckon history grep \"TODO\" --kind provenance
  deadreckon history grep \"review\" --plan <plan-id>
  deadreckon history grep \"error|failed\" --regex --limit 20

History searches durable JSONL evidence, not terminal scrollback. By default it
searches the current project's run traces. Use --all to search every project.";

const LEARN_HELP: &str = "\
Lifecycle:
  deadreckon learn index
  deadreckon learn report
  deadreckon learn export <run-id|proposal-id> --redacted
  deadreckon learn import-bundle <path> --preview
  deadreckon learn propose --limit 3
  deadreckon improve self <proposal-id> --preview

Learn builds a local experience index from redacted run evidence. Indexing is
deterministic; proposal creation folds provider-backed reflection into
`learn propose` and requires cited signals before writing proposals.";

const IMPROVE_HELP: &str = "\
Lifecycle:
  deadreckon learn index
  deadreckon learn propose
  deadreckon improve self <proposal-id> --preview
  deadreckon improve self <proposal-id> --yes
  deadreckon improve self <proposal-id> --pr-dry-run

Self-improve uses an isolated worktree and evidence packet. PR opening is
opt-in and evidence-gated; --pr-dry-run produces the same body without network
or push.";

#[derive(Parser)]
#[command(
    name = "deadreckon",
    version,
    about = "Unattended agentic coding harness",
    long_about = "deadreckon runs long coding goals in an isolated worktree or sandbox, tracks durable state, and gives you explicit apply/export/cleanup steps.",
    help_template = TOP_LEVEL_TEMPLATE,
    after_help = TOP_LEVEL_AFTER_HELP
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    #[command(
        next_help_heading = "Setup",
        visible_alias = "setup",
        about = "Create ~/.deadreckon/config.toml and check the local setup",
        after_help = INIT_HELP
    )]
    Init {
        #[arg(
            long,
            help = "Default provider route, for example cli:codex or cli:claude-code"
        )]
        provider: Option<String>,
        #[arg(long, help = "Provider API key for API-backed providers")]
        api_key: Option<String>,
        #[arg(long, help = "Base URL for OpenAI-compatible providers")]
        base_url: Option<String>,
        #[arg(long, default_value_t = 10.0, help = "Default spend cap in USD")]
        max_spend: f64,
        #[arg(long, default_value = "auto", help = "Default sandbox backend")]
        sandbox: String,
        #[arg(long, help = "Use detected defaults without interactive prompts")]
        no_confirm: bool,
        #[arg(long, help = "Skip shell completion installation")]
        no_completion: bool,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "settings",
        about = "Read or update ~/.deadreckon/config.toml",
        after_help = CONFIG_HELP
    )]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "commands",
        about = "Show every command, including advanced commands",
        after_help = "Lifecycle:\n  deadreckon help-all\n  deadreckon <command> --help"
    )]
    HelpAll,
    #[command(
        next_help_heading = "Setup",
        visible_alias = "completions",
        about = "Generate shell tab-completion scripts",
        after_help = COMPLETION_HELP
    )]
    Completion {
        #[command(subcommand)]
        command: Option<CompletionCommand>,
    },
    #[command(
        next_help_heading = "Done Criteria",
        hide = true,
        about = "Create, refine, explain, and check done criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Acceptance {
        #[command(subcommand)]
        command: AcceptanceCommand,
    },
    #[command(
        name = "def-done",
        next_help_heading = "Done Criteria",
        about = "Write, add, show, and check done criteria in English",
        after_help = DONE_HELP
    )]
    Done {
        #[arg(
            value_name = "TEXT_OR_COMMAND",
            num_args = 0..,
            help = "Plain-English criteria, or add/check/show"
        )]
        args: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite generated criteria/helper files"
        )]
        force: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Compiled criteria file to show/check"
        )]
        spec: Option<PathBuf>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Working directory to check; defaults to current directory"
        )]
        against: Option<PathBuf>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Run a keyless local smoke proof",
        after_help = TRY_HELP
    )]
    Try {
        #[arg(long, help = "Plain output without ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Guided front door for a run or orchestration",
        after_help = START_HELP
    )]
    Start {
        #[arg(help = "Natural-language coding goal, or @path/to/goal.md")]
        goal: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            value_hint = clap::ValueHint::FilePath,
            help = "Read the natural-language goal from a file; relative paths try cwd, then git root"
        )]
        goal_file: Option<PathBuf>,
        #[arg(
            long,
            value_enum,
            default_value_t = CliStartMode::Auto,
            help = "Guided path: auto, run, review, or full-plan"
        )]
        mode: CliStartMode,
        #[arg(long, help = "Provider route for this start launch")]
        provider: Option<String>,
        #[arg(long, help = "Model for this start launch (per-role flags override)")]
        model: Option<String>,
        #[arg(long, value_name = "N", help = "Full-plan child count, 2 through 6")]
        children: Option<u8>,
        #[arg(long, help = "Planner provider route for full-plan start")]
        planner_provider: Option<String>,
        #[arg(
            long,
            value_name = "IDX=PROVIDER",
            help = "Per-child provider route override for full-plan start"
        )]
        child_provider: Vec<String>,
        #[arg(long, help = "Coder provider route for review start")]
        coder_provider: Option<String>,
        #[arg(long, help = "Reviewer provider route for review start")]
        reviewer_provider: Option<String>,
        #[arg(long, help = "Show the resolved launch preview without starting work")]
        preview: bool,
        #[arg(
            long = "plan",
            value_name = "FILE",
            value_hint = clap::ValueHint::FilePath,
            help = "Replay a saved launch-plan.json instead of planning"
        )]
        plan: Option<PathBuf>,
        #[arg(
            long,
            value_name = "USD",
            help = "Budget ceiling for this launch (also gates auto-accept)"
        )]
        max_spend: Option<f64>,
        #[arg(long, help = "Confirm the launch preview without prompting")]
        yes: bool,
        #[arg(long, help = "Force built-in governance seams for this launch")]
        no_seams: bool,
        #[arg(long, help = "Start from an empty workspace")]
        fresh: bool,
        #[arg(long, help = "Force a git worktree source mode")]
        worktree: bool,
        #[arg(
            long = "from",
            value_name = "PATH",
            help = "Copy this source directory into runstate before launching"
        )]
        from: Option<PathBuf>,
        #[arg(
            long,
            help = "Allow uncommitted source changes when using worktree mode"
        )]
        allow_dirty: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Start an unattended coding run",
        after_help = RUN_HELP
    )]
    Run {
        #[arg(help = "Natural-language coding goal, or @path/to/goal.md")]
        goal: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            value_hint = clap::ValueHint::FilePath,
            help = "Read the natural-language goal from a file; relative paths try cwd, then git root"
        )]
        goal_file: Option<PathBuf>,
        #[arg(long, help = "Start from an empty workspace")]
        fresh: bool,
        #[arg(long, help = "Force a git worktree run")]
        worktree: bool,
        #[arg(
            long = "from",
            help = "Copy this source directory into runstate before running"
        )]
        from: Option<PathBuf>,
        #[arg(
            long,
            help = "Edit the current directory directly; requires explicit acknowledgement"
        )]
        in_place: bool,
        #[arg(long, help = "Base git ref for worktree runs")]
        base: Option<String>,
        #[arg(
            long = "branch-name",
            alias = "branch",
            help = "Branch name for worktree runs"
        )]
        branch: Option<String>,
        #[arg(long, help = "Seed current dirty/untracked files into the worktree")]
        allow_dirty: bool,
        #[arg(long, help = "Initialize git in a plain directory before running")]
        init_git: bool,
        #[arg(long, help = "Confirm the run preflight preview without prompting")]
        yes: bool,
        #[arg(long, help = "Show the planned run without creating state")]
        preview: bool,
        #[arg(long, help = "Print a single-line preview")]
        brief: bool,
        #[arg(long, help = "Force built-in governance seams for this run")]
        no_seams: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(
            long,
            value_parser = ["auto", "on", "off"],
            help = "Prevent system sleep during the run: auto, on, or off"
        )]
        prevent_sleep: Option<String>,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(
            long,
            help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
        )]
        sandbox: Option<String>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override for this run")]
        model: Option<String>,
        #[arg(long, help = "Documentation provider route for generated docs")]
        doc_provider: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Done criteria file for this run; defaults to .deadreckon/acceptance.yaml when present"
        )]
        acceptance: Option<PathBuf>,
        #[arg(
            long,
            default_value = "default-coding",
            help = "Skill name under skills/"
        )]
        skill: String,
        #[arg(long, help = "Use the deterministic smoke provider")]
        smoke: bool,
        #[arg(long, help = "Allow high spend or in-place edits")]
        i_know_its_a_lot: bool,
        #[arg(long, help = "Skip safety confirmations after preflight in scripts")]
        no_confirm: bool,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Skip generated run documentation")]
        no_docs: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
        #[arg(
            long,
            help = "Stream live plain-English narration to stderr on a piped run"
        )]
        narrate: bool,
        #[arg(long, help = "Disable live narration entirely")]
        no_narrate: bool,
        #[arg(
            long,
            help = "Pin the narrator model id (keeps provider preference order)"
        )]
        narrator_model: Option<String>,
        #[arg(
            long,
            help = "For an unknown project tree with no acceptance.yaml, propose a test contract for you to approve before it arms the gate (interactive only)"
        )]
        infer_contract: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Plan, fork, and merge a multi-agent run",
        after_help = ORCHESTRATE_HELP
    )]
    Orchestrate {
        #[command(subcommand)]
        command: Option<OrchestrateCommand>,
        #[arg(
            help = "Natural-language coding goal for the interactive mode chooser, or @path/to/goal.md"
        )]
        goal: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Read the natural-language goal for the interactive mode chooser from a file; relative paths try cwd, then git root"
        )]
        goal_file: Option<PathBuf>,
        #[arg(long, help = "Per-child spend cap in USD for the interactive chooser")]
        max_spend: Option<f64>,
        #[arg(
            long,
            help = "Per-child wall-clock cap for CLI-backed turns in the interactive chooser"
        )]
        max_wall_seconds: Option<f64>,
        #[arg(
            long,
            help = "Sandbox backend for the interactive chooser: auto, sandbox-exec, bwrap, docker, or none"
        )]
        sandbox: Option<String>,
        #[arg(
            long,
            help = "Show the resolved orchestration preflight without starting work"
        )]
        preview: bool,
        #[arg(
            long,
            help = "Initialize git in a plain directory before orchestrating"
        )]
        init_git: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Done criteria file for child runs; defaults to .deadreckon/acceptance.yaml when present"
        )]
        acceptance: Option<PathBuf>,
        #[arg(long, help = "Confirm the orchestration preflight without prompting")]
        yes: bool,
        #[arg(long, help = "Debug only: disable automatic semantic merge repair")]
        no_repair: bool,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(
            long,
            help = "Narrate each child in plain English (per-child snapshots + parent aggregate)"
        )]
        narrate: bool,
        #[arg(long, help = "Disable child narration entirely")]
        no_narrate: bool,
        #[arg(long, help = "Pin the narrator model id for children")]
        narrator_model: Option<String>,
    },
    #[command(
        about = "Run one goal as N independent orchestrators, then compose their results (depth-capped at 2)",
        after_help = "Examples:\n  deadreckon campaign \"rebuild billing, notifications, and admin\" --yes\n  deadreckon campaign --goal-file docs/goal.md --yes\n  deadreckon campaign @docs/goal.md --yes\n  deadreckon campaign repair <campaign-id>"
    )]
    Campaign {
        #[command(subcommand)]
        command: Option<CampaignCommand>,
        #[arg(
            help = "Natural-language coding goal to split into N sub-orchestrators, or @path/to/goal.md"
        )]
        goal: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Read the natural-language goal to split from a file; relative paths try cwd, then git root"
        )]
        goal_file: Option<PathBuf>,
        #[arg(
            long,
            help = "Number of sub-orchestrators, 2 through 6; omitted lets DeadReckon recommend"
        )]
        n: Option<u8>,
        #[arg(long, help = "Planner provider route for sub-goal decomposition")]
        planner_provider: Option<String>,
        #[arg(long, help = "Default child provider route for the sub-orchestrators")]
        provider: Option<String>,
        #[arg(long, help = "Planner model for sub-goal decomposition")]
        planner_model: Option<String>,
        #[arg(long, help = "Default child model for the sub-orchestrators")]
        model: Option<String>,
        #[arg(
            long,
            help = "Tree-wide spend cap in USD, split across sub-orchestrators"
        )]
        max_spend: Option<f64>,
        #[arg(long, help = "Per-leaf wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(
            long,
            help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
        )]
        sandbox: Option<String>,
        #[arg(
            long,
            help = "Show the resolved campaign preflight without starting work"
        )]
        preview: bool,
        #[arg(long, help = "Confirm the campaign preflight without prompting")]
        yes: bool,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(
            long,
            help = "Narrate each child in plain English through the sub-orchestrators"
        )]
        narrate: bool,
        #[arg(long, help = "Disable child narration entirely")]
        no_narrate: bool,
        #[arg(long, help = "Pin the narrator model id for children")]
        narrator_model: Option<String>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Write an orchestration plan without starting child runs",
        after_help = PLAN_HELP
    )]
    Plan {
        #[arg(help = "Natural-language coding goal")]
        goal: String,
        #[arg(long, default_value_t = 3, help = "Full-plan child count, 2 through 6")]
        n: u8,
        #[arg(long, value_enum, default_value_t = CliPlanMode::FullPlan, help = "Plan mode")]
        mode: CliPlanMode,
        #[arg(long, help = "Planner provider route for full-plan decomposition")]
        planner_provider: Option<String>,
        #[arg(long, help = "Default child provider route for full-plan work")]
        provider: Option<String>,
        #[arg(
            long,
            value_name = "IDX=PROVIDER",
            help = "Per-child provider route override"
        )]
        child_provider: Vec<String>,
        #[arg(long, help = "Coder provider route for review mode")]
        coder_provider: Option<String>,
        #[arg(long, help = "Reviewer provider route for review mode")]
        reviewer_provider: Option<String>,
        #[arg(long, help = "Initialize git in a plain directory before planning")]
        init_git: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Done criteria file for child runs; defaults to .deadreckon/acceptance.yaml when present"
        )]
        acceptance: Option<PathBuf>,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Emit a machine-readable plan summary as JSON")]
        json: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Start child runs for an orchestration plan",
        after_help = FORK_HELP
    )]
    Fork {
        #[arg(help = "Plan id or unique prefix")]
        plan_id: String,
        #[arg(long, help = "Per-child spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Per-child wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(
            long,
            help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
        )]
        sandbox: Option<String>,
        #[arg(long, help = "Override the plan default child provider route")]
        provider: Option<String>,
        #[arg(
            long,
            value_name = "IDX=PROVIDER",
            help = "Per-child provider route override"
        )]
        child_provider: Vec<String>,
        #[arg(long, help = "Coder provider route override for review mode")]
        coder_provider: Option<String>,
        #[arg(long, help = "Reviewer provider route override for review mode")]
        reviewer_provider: Option<String>,
        #[arg(long, help = "Debug only: disable automatic dependency merge repair")]
        no_repair: bool,
        #[arg(long, help = "Repair provider route for dependency merge repair")]
        repair_provider: Option<String>,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Compose completed plan children into one promoted artifact",
        after_help = MERGE_HELP
    )]
    Merge {
        #[arg(help = "Plan id or unique prefix")]
        plan_id: String,
        #[arg(
            long,
            default_value = "dag-aware",
            help = "Plan merge strategy: dag-aware, fail-on-conflict, or prefer-child"
        )]
        strategy: String,
        #[arg(long, help = "Child index to prefer when --strategy prefer-child")]
        prefer_child: Option<u32>,
        #[arg(long, help = "Disable automatic semantic merge repair")]
        no_repair: bool,
        #[arg(
            long,
            help = "Repair provider route for merge repair planning and child runs"
        )]
        repair_provider: Option<String>,
        #[arg(
            long,
            default_value = "auto",
            help = "Repair mode: auto, prefer, synthesize, or child"
        )]
        repair_mode: String,
        #[arg(long, default_value_t = 1, help = "Maximum automatic repair attempts")]
        repair_attempts: u32,
        #[arg(long, help = "Confirm automatic repair actions without prompting")]
        yes: bool,
        #[arg(long, help = "Skip the merge gate marker warning path")]
        no_gate: bool,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Run a serial chain of coding goals",
        after_help = CHAIN_HELP
    )]
    Chain {
        #[arg(value_name = "ARG", num_args = 0.., help = "Step goals or chain subcommand")]
        args: Vec<String>,
        #[arg(long, help = "Read newline-separated step goals from a file")]
        from_file: Option<PathBuf>,
        #[arg(long, help = "Read newline-separated step goals from stdin")]
        from_stdin: bool,
        #[arg(long, help = "Write chain.json only; do not start the conductor")]
        draft: bool,
        #[arg(long, help = "Confirm the chain preflight preview without prompting")]
        yes: bool,
        #[arg(long, help = "Start the conductor in the background")]
        detach: bool,
        #[arg(
            long,
            default_value = "stack",
            help = "Branch policy: stack, base, or linear-merge"
        )]
        branch_policy: String,
        #[arg(
            long,
            default_value = "auto",
            help = "Chain apply mode: auto, preview, or manual"
        )]
        apply_mode: String,
        #[arg(
            long,
            default_value = "squash",
            help = "Git apply strategy for chain steps: squash, merge, or cherry-pick"
        )]
        apply_strategy: String,
        #[arg(long, help = "Glob allowlist for auto-apply")]
        apply_allowlist: Vec<String>,
        #[arg(
            long,
            default_value = "stop",
            help = "Failure policy: stop, skip, or continue"
        )]
        on_fail: String,
        #[arg(
            long,
            default_value_t = 2,
            help = "Consecutive failed steps before pausing"
        )]
        circuit_breaker_threshold: u32,
        #[arg(long, help = "Aggregate chain spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Aggregate chain wall-clock cap in seconds")]
        max_wall_seconds: Option<f64>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override")]
        model: Option<String>,
        #[arg(long, default_value = "auto", help = "Sandbox backend")]
        sandbox: String,
        #[arg(long, help = "Base git ref; defaults to current HEAD")]
        base: Option<String>,
        #[arg(
            long,
            default_value_t = 4,
            help = "Number of planner steps for chain plan"
        )]
        n: u8,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Reason for pause")]
        reason: Option<String>,
        #[arg(long, help = "Resume from this step index")]
        from_step: Option<u32>,
        #[arg(long, help = "Add to the aggregate spend cap on resume")]
        max_spend_add: Option<f64>,
        #[arg(long, help = "Reset the circuit breaker on resume")]
        reset_breaker: bool,
        #[arg(
            long = "escalate",
            alias = "force",
            help = "Escalate kill without SIGTERM grace"
        )]
        force: bool,
        #[arg(long, help = "Step index for redo")]
        step: Option<u32>,
        #[arg(long, help = "Patch the selected redo step goal")]
        extend: Option<String>,
        #[arg(long, help = "Allow redo of an already-applied step")]
        reapply: bool,
        #[arg(long, help = "Insert extension at this step index")]
        insert_at: Option<u32>,
        #[arg(long, help = "Skip destructive or follow-up confirmations")]
        no_confirm: bool,
        #[arg(long, help = "Print exact IDs and paths")]
        full: bool,
        #[arg(
            long = "all-scopes",
            alias = "all",
            help = "Show chains from all project scopes"
        )]
        all: bool,
        #[arg(long, help = "Explain the failure reason in chain show")]
        why_failed: bool,
        #[arg(long, help = "Emit machine-readable JSON for chain list/show/status")]
        json: bool,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "check",
        about = "Check providers, sandboxing, disk, and local prerequisites",
        after_help = DOCTOR_HELP
    )]
    Doctor {
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Setup",
        about = "Validate configured seam workers against fixture JSON",
        after_help = SEAMS_HELP
    )]
    Seams {
        #[command(subcommand)]
        command: SeamsCommand,
    },
    #[command(
        next_help_heading = "Setup",
        about = "Probe registered provider routes",
        after_help = DETECT_HELP
    )]
    Detect {
        #[arg(help = "Provider id to probe, for example cli:codex")]
        id: Option<String>,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
        #[arg(long, help = "Probe HTTP/local endpoints instead of only credentials")]
        ping: bool,
    },
    #[command(
        next_help_heading = "Setup",
        about = "List registered provider routes",
        after_help = PROVIDERS_HELP
    )]
    Providers {
        #[command(subcommand)]
        command: ProvidersCommand,
    },
    #[command(
        next_help_heading = "Inspection",
        about = "List selectable models per provider route"
    )]
    Models {
        #[arg(help = "Provider route (omit to list every credentialed route)")]
        provider: Option<String>,
        #[arg(long, help = "Include providers without credentials")]
        all: bool,
        #[arg(long, help = "Machine-readable JSON output")]
        json: bool,
    },
    #[command(
        next_help_heading = "Setup",
        about = "Check for updates or route through the install channel",
        after_help = UPDATE_HELP
    )]
    Update {
        #[arg(long, help = "Only check and print current/latest version")]
        check: bool,
        #[arg(
            long = "anyway",
            alias = "force",
            help = "Re-run the selected update path even at the same version"
        )]
        force: bool,
        #[arg(long, help = "Include prereleases when checking the latest release")]
        allow_prerelease: bool,
        #[arg(long, help = "Confirm shell-channel update preview without prompting")]
        yes: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "runs",
        about = "Show runs for the current project by default",
        after_help = LIST_HELP
    )]
    List {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Show runs from all project scopes")]
        all: bool,
        #[arg(long, help = "Use the table layout (default; kept for compatibility)")]
        full: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "artifacts",
        about = "Inspect promoted run artifacts in the deadreckon library",
        after_help = LIBRARY_HELP
    )]
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        about = "Route a completed run to the right finish action",
        after_help = FINISH_HELP
    )]
    Finish {
        #[arg(help = "Run id, plan id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(long, help = "Destination directory for fresh/copy exports")]
        dest: Option<PathBuf>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite a non-empty export destination"
        )]
        force: bool,
        #[arg(long, help = "Keep manifest.json in exported output")]
        include_manifest: bool,
        #[arg(
            long,
            default_value = "squash",
            long = "git-strategy",
            alias = "strategy",
            help = "Git apply strategy: squash, merge, or cherry-pick"
        )]
        strategy: String,
        #[arg(
            long = "into",
            alias = "branch",
            help = "Target branch for apply; defaults to the current branch"
        )]
        branch: Option<String>,
        #[arg(
            long,
            help = "Temporarily stash local changes and restore them after worktree apply"
        )]
        autostash: bool,
        #[arg(
            long,
            help = "Remove the temporary worktree/branch after a successful worktree apply"
        )]
        cleanup: bool,
        #[arg(long, help = "Skip destructive or follow-up confirmations")]
        no_confirm: bool,
        #[arg(long, help = "Commit message override for worktree apply")]
        message: Option<String>,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_aliases = ["export", "copy-out"],
        about = "Copy a completed fresh/copy run into a chosen directory",
        after_help = MATERIALIZE_HELP
    )]
    Materialize {
        #[arg(help = "Run id, plan id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Destination directory")]
        dest: Option<PathBuf>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite a non-empty destination"
        )]
        force: bool,
        #[arg(long, help = "Keep manifest.json in the exported output")]
        include_manifest: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "keep",
        about = "Merge a completed worktree run back into the source checkout",
        after_help = APPLY_HELP
    )]
    Apply {
        #[arg(help = "Run id, plan id, unique prefix, or latest")]
        run_id: String,
        #[arg(
            long,
            default_value = "squash",
            long = "git-strategy",
            alias = "strategy",
            help = "Git apply strategy: squash, merge, or cherry-pick"
        )]
        strategy: String,
        #[arg(
            long = "into",
            alias = "branch",
            help = "Target branch for apply; defaults to the current branch"
        )]
        branch: Option<String>,
        #[arg(long, help = "Skip destructive or follow-up confirmations")]
        no_confirm: bool,
        #[arg(
            long,
            help = "Temporarily stash local changes and restore them after apply"
        )]
        autostash: bool,
        #[arg(
            long,
            help = "Remove the temporary worktree/branch after a successful apply"
        )]
        cleanup: bool,
        #[arg(long, help = "Commit message override")]
        message: Option<String>,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        hide = true,
        visible_alias = "discard",
        about = "Remove a run's temporary worktree and branch",
        after_help = ABANDON_HELP
    )]
    Abandon {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Keep the temporary branch")]
        keep_branch: bool,
        #[arg(
            long = "anyway",
            alias = "force",
            help = "Clean even if the run is still marked running"
        )]
        force: bool,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        visible_aliases = ["prune", "clean"],
        about = "Clean stale or temporary deadreckon worktrees",
        after_help = CLEANUP_HELP
    )]
    Cleanup {
        #[arg(help = "Optional run id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(long = "all-scopes", alias = "all", help = "Search all project scopes")]
        all: bool,
        #[arg(long, help = "Include completed worktree runs not already abandoned")]
        completed: bool,
        #[arg(long, help = "Include stale running runs")]
        stale: bool,
        #[arg(long, help = "Skip destructive or follow-up confirmations")]
        no_confirm: bool,
        #[arg(
            long = "escalate",
            alias = "force",
            help = "Escalate stale run termination"
        )]
        force: bool,
        #[arg(long, help = "Force git worktree removal")]
        overwrite: bool,
        #[arg(long, help = "Keep temporary branches")]
        keep_branch: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        visible_alias = "follow-up",
        about = "Continue from a completed run with a follow-up goal",
        after_help = EXTEND_HELP
    )]
    Extend {
        #[arg(help = "Parent run id, unique prefix, or latest")]
        parent_run_id: String,
        #[arg(help = "Follow-up coding goal")]
        new_goal: String,
        #[arg(long, help = "Destination working directory for copy/fresh extensions")]
        dest: Option<PathBuf>,
        #[arg(long, help = "Maximum parent turns to include in context")]
        max_context_turns: Option<u32>,
        #[arg(long, help = "Do not include parent context")]
        no_context: bool,
        #[arg(long, help = "Spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override for this extended run")]
        model: Option<String>,
        #[arg(long, help = "Sandbox backend override")]
        sandbox: Option<String>,
        #[arg(long, help = "Skip generated run documentation")]
        no_docs: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
        #[arg(
            long,
            help = "Stream live plain-English narration to stderr on a piped run"
        )]
        narrate: bool,
        #[arg(long, help = "Disable live narration entirely")]
        no_narrate: bool,
        #[arg(
            long,
            help = "Pin the narrator model id (keeps provider preference order)"
        )]
        narrator_model: Option<String>,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "docs",
        about = "Print or regenerate generated run documentation",
        after_help = DOC_HELP
    )]
    Doc {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, value_enum, default_value_t = CliDocKind::Narrative, help = "Document kind")]
        kind: CliDocKind,
        #[arg(long, help = "Write the document to this path instead of stdout")]
        export: Option<PathBuf>,
        #[arg(
            long,
            help = "Use the documentation provider route to polish generated docs"
        )]
        polish: bool,
        #[arg(long, help = "Skip destructive or follow-up confirmations")]
        no_confirm: bool,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite existing export or polish output"
        )]
        force: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
        #[arg(long, help = "Documentation provider route for polish")]
        doc_provider: Option<String>,
        #[arg(
            long = "max-spend",
            alias = "budget-cap",
            help = "Spend cap in USD for documentation polish"
        )]
        budget_cap: Option<f64>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "watch",
        about = "Attach the live terminal UI to a run, chain, or plan",
        after_help = ATTACH_HELP
    )]
    Attach {
        #[arg(help = "Run id, chain id, plan id, plan-id:task-id, unique prefix, or latest")]
        run_id: String,
        #[arg(
            long,
            value_enum,
            default_value_t = AttachViewMode::Activity,
            help = "Attach view: raw activity, human narrative, or split"
        )]
        view: AttachViewMode,
        #[arg(
            long,
            value_enum,
            default_value_t = NarrativeVisualMode::Architecture,
            help = "Narrative visual: architecture, agents, files, evidence, or none"
        )]
        visual: NarrativeVisualMode,
        #[arg(
            long,
            help = "Narrative summarizer provider route override (default: cli:claude-code sonnet)"
        )]
        narrative_provider: Option<String>,
        #[arg(
            long,
            conflicts_with = "narrative_provider",
            help = "Use deterministic narrative facts only; do not call a narrative provider"
        )]
        no_narrative_provider: bool,
        #[arg(
            long = "narrative-max-spend",
            help = "Spend cap in USD for narrative refresh"
        )]
        narrative_max_spend: Option<f64>,
        #[arg(long, help = "Structured JSON output for the selected attach view")]
        json: bool,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "stop",
        about = "Cancel a run, chain, or plan",
        after_help = KILL_HELP
    )]
    Kill {
        #[arg(help = "Run id, chain id, plan id, unique prefix, or latest")]
        run_id: String,
        #[arg(
            long = "escalate",
            alias = "force",
            help = "Escalate subprocess termination"
        )]
        force: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Preview and accept a run's reshape proposal as a plan"
    )]
    Reshape {
        #[arg(help = "Run id or prefix carrying a reshape proposal, or `latest`")]
        run_id: String,
        #[arg(long, help = "Accept and dispatch without prompting")]
        yes: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
        #[arg(long, help = "Plain output without ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Suppress success chatter and post-action hints")]
        quiet: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "continue",
        about = "Resume an incomplete run",
        after_help = RESUME_HELP
    )]
    Resume {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Resume from this turn number")]
        from_turn: Option<u32>,
        #[arg(long, help = "Override wall-clock cap")]
        max_wall_seconds: Option<f64>,
        #[arg(long, help = "Skip document polishing while resuming")]
        no_docs: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        hide = true,
        visible_alias = "restore",
        about = "Restore an in-place run snapshot",
        after_help = UNDO_HELP
    )]
    Undo {
        #[arg(
            long,
            help = "Run id, unique prefix, or latest; defaults to current project's latest"
        )]
        run: Option<String>,
        #[arg(long, help = "Snapshot turn to restore")]
        turn: Option<u32>,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        about = "Preview or apply a provider checkpoint rewind"
    )]
    Rewind {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Rewind to the last provider checkpoint for this turn")]
        to_turn: Option<u32>,
        #[arg(
            long,
            help = "Rewind to the checkpoint attached to this provider event"
        )]
        to_provider_event: Option<u64>,
        #[arg(long, help = "Rewind to this checkpoint id")]
        to_checkpoint: Option<String>,
        #[arg(long, help = "Preview the rewind without changing files")]
        preview: bool,
        #[arg(long, help = "Apply the rewind after hash-guarding changed files")]
        apply: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        about = "Re-verify a run NOW: VERIFIED / REGRESSED / UNVERIFIED with evidence and one next action (read-only)"
    )]
    Verdict {
        #[arg(help = "Run id, unique prefix, or latest (default: latest)")]
        run_id: Option<String>,
        #[arg(long, help = "Compare several recent runs side by side")]
        all: bool,
        #[arg(long, help = "With --all: how many recent runs (default 10)")]
        limit: Option<usize>,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Suppress secondary actions and chatter")]
        quiet: bool,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        hide = true,
        visible_alias = "inspect",
        about = "Show full state, provenance, and trace details for a run, plan, or plan child",
        after_help = SHOW_HELP
    )]
    Show {
        #[arg(help = "Run id, plan id, plan-id:task-id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Only show trace/provenance records for this turn")]
        turn: Option<u32>,
        #[arg(long, help = "Explain the most likely failure cause")]
        why_failed: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
        #[arg(long, help = "Show provider-native flight events and checkpoints")]
        flight: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Show flight/checkpoint activity for a file"
        )]
        file: Option<PathBuf>,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        about = "Search run traces and provenance history",
        after_help = HISTORY_HELP
    )]
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "next",
        about = "Explain the current project's latest run and next action",
        after_help = STATUS_HELP
    )]
    Status {
        #[arg(help = "Optional run id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(
            long,
            long = "global",
            alias = "all",
            help = "Use the global latest run instead of the current project"
        )]
        all: bool,
        #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        hide = true,
        about = "Import read-only history from another coding tool",
        after_help = IMPORT_HELP
    )]
    Import {
        #[arg(help = "Source: codex, claude-code, cursor, or a cli:<provider> descriptor")]
        source: String,
        #[arg(long, help = "Show the selected import session without creating a run")]
        preview: bool,
        #[arg(long, help = "List discovered import sessions without creating a run")]
        list: bool,
        #[arg(long, help = "Import one explicit session id or source path")]
        session: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Match sessions for this cwd instead of the launch directory"
        )]
        cwd: Option<PathBuf>,
        #[arg(
            long,
            help = "Import all discovered source files; required for whole-root imports"
        )]
        all: bool,
        #[arg(
            long,
            value_name = "DURATION",
            help = "Discover sessions modified within a duration such as 10m, 2h, or 1d"
        )]
        since: Option<String>,
        #[arg(
            long,
            help = "Replace an existing imported run when source content changed"
        )]
        replace: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        about = "Index local run evidence and propose DeadReckon improvements",
        after_help = LEARN_HELP
    )]
    Learn {
        #[command(subcommand)]
        command: LearnCommand,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Run evidence-backed DeadReckon self-improvement candidates",
        after_help = IMPROVE_HELP
    )]
    Improve {
        #[command(subcommand)]
        command: ImproveCommand,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliDocKind {
    Narrative,
    AsBuilt,
    Decisions,
    Children,
    Delta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPlanMode {
    FullPlan,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliStartMode {
    Auto,
    Run,
    Review,
    FullPlan,
}

#[derive(Subcommand)]
pub(crate) enum OrchestrateCommand {
    #[command(
        about = "Run one coder provider route, then a fresh reviewer/fixer provider route",
        after_help = "Example:\n  deadreckon orchestrate review \"build the thing\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes"
    )]
    Review(OrchestrateReviewArgs),
    #[command(
        name = "full-plan",
        about = "Ask a planner provider route for child work, then fork and merge",
        after_help = "Example:\n  deadreckon orchestrate full-plan \"build the thing\" --planner-provider cli:codex --provider cli:claude-code --n 4 --yes"
    )]
    FullPlan(OrchestrateFullPlanArgs),
}

#[derive(Subcommand)]
pub(crate) enum CampaignCommand {
    #[command(
        about = "Repair and promote a failed campaign merge",
        after_help = "Example:\n  deadreckon campaign repair <campaign-id> --repair-provider cli:codex"
    )]
    Repair(CampaignRepairCliArgs),
}

#[derive(Args)]
pub(crate) struct CampaignRepairCliArgs {
    #[arg(help = "Campaign id or unique prefix")]
    pub(crate) campaign_id: String,
    #[arg(long, help = "Repair provider route for campaign merge repair")]
    pub(crate) repair_provider: Option<String>,
    #[arg(
        long,
        default_value = "auto",
        help = "Repair mode: auto, prefer, synthesize, or child"
    )]
    pub(crate) repair_mode: String,
    #[arg(long, default_value_t = 1, help = "Maximum automatic repair attempts")]
    pub(crate) repair_attempts: u32,
    #[arg(long, help = "Suppress post-action hints")]
    pub(crate) no_hints: bool,
    #[arg(long, help = "Suppress success chatter and post-action hints")]
    pub(crate) quiet: bool,
    #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
    pub(crate) plain: bool,
}

#[derive(Args)]
pub(crate) struct OrchestrateReviewArgs {
    #[arg(help = "Natural-language coding goal, or @path/to/goal.md")]
    pub(crate) goal: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the natural-language goal from a file; relative paths try cwd, then git root"
    )]
    pub(crate) goal_file: Option<PathBuf>,
    #[arg(long, help = "Per-child spend cap in USD")]
    pub(crate) max_spend: Option<f64>,
    #[arg(long, help = "Per-child wall-clock cap for CLI-backed turns")]
    pub(crate) max_wall_seconds: Option<f64>,
    #[arg(
        long,
        help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
    )]
    pub(crate) sandbox: Option<String>,
    #[arg(long, help = "Coder provider route for review mode")]
    pub(crate) coder_provider: Option<String>,
    #[arg(long, help = "Reviewer provider route for review mode")]
    pub(crate) reviewer_provider: Option<String>,
    #[arg(long, help = "Coder model for review mode")]
    pub(crate) coder_model: Option<String>,
    #[arg(long, help = "Reviewer model for review mode")]
    pub(crate) reviewer_model: Option<String>,
    #[arg(
        long,
        help = "Initialize git in a plain directory before orchestrating"
    )]
    pub(crate) init_git: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Done criteria file for child runs; defaults to .deadreckon/acceptance.yaml when present"
    )]
    pub(crate) acceptance: Option<PathBuf>,
    #[arg(
        long,
        help = "Show the resolved orchestration preflight without starting work"
    )]
    pub(crate) preview: bool,
    #[arg(long, help = "Confirm the orchestration preflight without prompting")]
    pub(crate) yes: bool,
    #[arg(long, help = "Debug only: disable automatic semantic merge repair")]
    pub(crate) no_repair: bool,
    #[arg(long, help = "Suppress post-action hints")]
    pub(crate) no_hints: bool,
    #[arg(long, help = "Suppress success chatter and post-action hints")]
    pub(crate) quiet: bool,
    #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
    pub(crate) plain: bool,
    #[arg(
        long,
        help = "Narrate each child in plain English (per-child snapshots + parent aggregate)"
    )]
    pub(crate) narrate: bool,
    #[arg(long, help = "Disable child narration entirely")]
    pub(crate) no_narrate: bool,
    #[arg(long, help = "Pin the narrator model id for children")]
    pub(crate) narrator_model: Option<String>,
}

#[derive(Args)]
pub(crate) struct OrchestrateFullPlanArgs {
    #[arg(help = "Natural-language coding goal, or @path/to/goal.md")]
    pub(crate) goal: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the natural-language goal from a file; relative paths try cwd, then git root"
    )]
    pub(crate) goal_file: Option<PathBuf>,
    #[arg(long, default_value_t = 3, help = "Full-plan child count, 2 through 6")]
    pub(crate) n: u8,
    #[arg(long, help = "Per-child spend cap in USD")]
    pub(crate) max_spend: Option<f64>,
    #[arg(long, help = "Per-child wall-clock cap for CLI-backed turns")]
    pub(crate) max_wall_seconds: Option<f64>,
    #[arg(
        long,
        help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
    )]
    pub(crate) sandbox: Option<String>,
    #[arg(long, help = "Planner provider route for full-plan decomposition")]
    pub(crate) planner_provider: Option<String>,
    #[arg(long, help = "Default child provider route for full-plan work")]
    pub(crate) provider: Option<String>,
    #[arg(long, help = "Planner model for full-plan decomposition")]
    pub(crate) planner_model: Option<String>,
    #[arg(long, help = "Default child model for full-plan work")]
    pub(crate) model: Option<String>,
    #[arg(
        long,
        value_name = "IDX=MODEL",
        help = "Per-child model override (like --child-provider)"
    )]
    pub(crate) child_model: Vec<String>,
    #[arg(
        long,
        value_name = "IDX=PROVIDER",
        help = "Per-child provider route override"
    )]
    pub(crate) child_provider: Vec<String>,
    #[arg(
        long,
        help = "Initialize git in a plain directory before orchestrating"
    )]
    pub(crate) init_git: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Done criteria file for child runs; defaults to .deadreckon/acceptance.yaml when present"
    )]
    pub(crate) acceptance: Option<PathBuf>,
    #[arg(
        long,
        help = "Show the resolved orchestration preflight without starting work"
    )]
    pub(crate) preview: bool,
    #[arg(long, help = "Confirm the orchestration preflight without prompting")]
    pub(crate) yes: bool,
    #[arg(long, help = "Debug only: disable automatic semantic merge repair")]
    pub(crate) no_repair: bool,
    #[arg(long, help = "Suppress post-action hints")]
    pub(crate) no_hints: bool,
    #[arg(long, help = "Suppress success chatter and post-action hints")]
    pub(crate) quiet: bool,
    #[arg(long, help = "Plain output without TUI, spinner, or ANSI affordances")]
    pub(crate) plain: bool,
    #[arg(
        long,
        help = "Narrate each child in plain English (per-child snapshots + parent aggregate)"
    )]
    pub(crate) narrate: bool,
    #[arg(long, help = "Disable child narration entirely")]
    pub(crate) no_narrate: bool,
    #[arg(long, help = "Pin the narrator model id for children")]
    pub(crate) narrator_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum HistoryKind {
    Trace,
    Provenance,
}

#[derive(Subcommand)]
pub(crate) enum HistoryCommand {
    #[command(about = "Search trace or provenance JSONL")]
    Grep {
        #[arg(help = "Substring or regex to search for")]
        pattern: String,
        #[arg(long, help = "Restrict to children of this orchestration plan")]
        plan: Option<String>,
        #[arg(long, help = "Restrict to one project scope")]
        scope: Option<String>,
        #[arg(long, help = "Search all project scopes")]
        all: bool,
        #[arg(long, help = "Only search files modified within 7d, 24h, or 30m")]
        since: Option<String>,
        #[arg(long, value_enum, default_value_t = HistoryKind::Trace, help = "Search trace or provenance JSONL")]
        kind: HistoryKind,
        #[arg(long, default_value_t = 100, help = "Maximum matches to print")]
        limit: usize,
        #[arg(long, help = "Treat the pattern as a regular expression")]
        regex: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum LearnCommand {
    #[command(about = "Index completed local runs into redacted episodes and signals")]
    Index {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Index runs from all project scopes")]
        all: bool,
        #[arg(
            long,
            help = "Reserved for future duration filtering; currently accepted for command stability"
        )]
        since: Option<String>,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(about = "Summarize indexed episodes, signals, insights, and proposals")]
    Report {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, default_value_t = 10, help = "Maximum signals to display")]
        limit: usize,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(about = "Export a redacted learning bundle for a run or proposal")]
    Export {
        #[arg(help = "Indexed run id or proposal id")]
        source: String,
        #[arg(long, value_name = "PATH", help = "Bundle output path")]
        output: Option<PathBuf>,
        #[arg(long, help = "Confirm redacted output; bundles are always redacted")]
        redacted: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(about = "Preview or import a redacted learning bundle")]
    ImportBundle {
        #[arg(value_name = "PATH", help = "Redacted learning bundle path")]
        path: PathBuf,
        #[arg(long, help = "Preview without writing local learning state")]
        preview: bool,
        #[arg(long, help = "Import the bundle into local learning state")]
        yes: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(about = "Reflect on redacted evidence and write improvement proposals")]
    Propose {
        #[arg(long, help = "Filter local indexed evidence to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Use local indexed evidence from all scopes")]
        all: bool,
        #[arg(long, hide = true)]
        from_local: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Use a redacted learning bundle as the evidence source"
        )]
        bundle: Option<PathBuf>,
        #[arg(
            long,
            default_value_t = 3,
            help = "Maximum insights/proposals to write"
        )]
        limit: usize,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ImproveCommand {
    #[command(
        name = "self",
        about = "Run a local self-improvement candidate against DeadReckon"
    )]
    SelfRun {
        #[arg(help = "Proposal id or goal file")]
        target: String,
        #[arg(long, help = "Preview without creating a worktree or run")]
        preview: bool,
        #[arg(long, help = "Create and run the self-improvement candidate")]
        yes: bool,
        #[arg(long, help = "Generate PR title/body without network or push")]
        pr_dry_run: bool,
        #[arg(long, help = "Open a PR only after evidence gate success")]
        open_pr: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AcceptanceCommand {
    #[command(
        about = "Create acceptance criteria from plain English",
        after_help = ACCEPTANCE_HELP
    )]
    Setup {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "Plain-English definition of done")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite existing .deadreckon/acceptance files"
        )]
        force: bool,
    },
    #[command(
        about = "Add an acceptance pack or plain-English criterion",
        after_help = ACCEPTANCE_HELP
    )]
    Add {
        #[arg(
            value_name = "PACK_OR_REQUEST",
            num_args = 1..,
            help = "Pack name like browser/node/playwright, or an English criterion"
        )]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for English criteria")]
        provider: Option<String>,
        #[arg(long, help = "Model override for English criteria")]
        model: Option<String>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite generated helper files"
        )]
        force: bool,
    },
    #[command(about = "Write a local acceptance template", after_help = ACCEPTANCE_HELP)]
    Init {
        #[arg(
            long,
            default_value = "auto",
            help = "Template: auto, rust, node, static-site, or basic"
        )]
        preset: AcceptancePreset,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite existing .deadreckon/acceptance files"
        )]
        force: bool,
    },
    #[command(
        about = "Ask the configured provider to draft acceptance criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Draft {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "What should count as done")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite existing .deadreckon/acceptance files"
        )]
        force: bool,
    },
    #[command(
        about = "Ask the configured provider to improve existing acceptance criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Refine {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "Requested acceptance change")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(
            long = "overwrite",
            alias = "force",
            help = "Overwrite existing .deadreckon/acceptance files"
        )]
        force: bool,
    },
    #[command(about = "Explain the active acceptance criteria", after_help = ACCEPTANCE_HELP)]
    Explain {
        #[arg(long, value_name = "PATH", help = "Done criteria file to explain")]
        spec: Option<PathBuf>,
    },
    #[command(
        about = "Dry-run acceptance checks against a working directory",
        after_help = ACCEPTANCE_HELP
    )]
    Check {
        #[arg(long, value_name = "PATH", help = "Done criteria file to check")]
        spec: Option<PathBuf>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Working directory to evaluate; defaults to current directory"
        )]
        against: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum AcceptancePreset {
    Auto,
    Rust,
    Node,
    StaticSite,
    Basic,
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    #[command(about = "Print one config value")]
    Get {
        #[arg(help = "Dotted key, for example defaults.provider")]
        key: String,
    },
    #[command(about = "Set one config value")]
    Set {
        #[arg(help = "Dotted key, for example defaults.provider")]
        key: String,
        #[arg(help = "TOML value or plain string")]
        value: String,
    },
    #[command(about = "Show or set the default provider route")]
    Provider {
        #[arg(help = "Provider route to make default, for example cli:codex")]
        provider: Option<String>,
    },
    #[command(
        name = "remove-provider",
        visible_alias = "rm-provider",
        about = "Remove a provider route from configured defaults and fallback"
    )]
    RemoveProvider {
        #[arg(help = "Provider route to remove; omit to choose from configured routes")]
        provider: Option<String>,
    },
    #[command(about = "Show or set the model for a provider route")]
    Model {
        #[arg(help = "Model to set; omit to show the active route/model")]
        model: Option<String>,
        #[arg(
            long,
            help = "Provider route to update; defaults to the active provider"
        )]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SeamsCommand {
    #[command(
        about = "Run one configured seam worker against a fixture",
        after_help = SEAMS_HELP
    )]
    Validate {
        #[arg(value_enum, help = "Seam kind to validate")]
        kind: CliSeamKind,
        #[arg(long, value_name = "PATH", help = "TOML config containing [seams]")]
        config: PathBuf,
        #[arg(long, value_name = "PATH", help = "Fixture JSON to send on stdin")]
        fixture: Option<PathBuf>,
        #[arg(
            long,
            default_value = "auto",
            value_parser = ["auto", "sandbox-exec", "bwrap", "docker", "none"],
            help = "Sandbox backend for the validation subprocess"
        )]
        sandbox: String,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CliSeamKind {
    Policy,
    Catalog,
    Hooks,
    #[value(name = "event-sink", alias = "event_sink")]
    EventSink,
}

#[derive(Subcommand)]
pub(crate) enum ProvidersCommand {
    #[command(about = "List registered provider routes")]
    List {
        #[arg(long, help = "Also list model catalog entries")]
        models: bool,
        #[arg(
            long,
            help = "Show every built-in and override provider route, not only configured routes"
        )]
        all: bool,
        #[arg(long, help = "Print exact IDs and paths for scripts")]
        full: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum CompletionCommand {
    #[command(about = "Detect the shell and install the generated completion script")]
    Install {
        #[arg(
            long,
            value_enum,
            help = "Shell override; defaults to $SHELL detection"
        )]
        shell: Option<clap_complete::Shell>,
        #[arg(long, value_name = "PATH", help = "Install path override")]
        path: Option<PathBuf>,
        #[arg(long, help = "Skip the managed zshrc block")]
        no_rc: bool,
    },
    #[command(about = "Print bash completion script to stdout")]
    Bash,
    #[command(about = "Print elvish completion script to stdout")]
    Elvish,
    #[command(about = "Print fish completion script to stdout")]
    Fish,
    #[command(
        name = "powershell",
        about = "Print PowerShell completion script to stdout"
    )]
    PowerShell,
    #[command(about = "Print zsh completion script to stdout")]
    Zsh,
}

#[derive(Subcommand)]
pub(crate) enum LibraryCommand {
    #[command(about = "List promoted artifacts for the current project")]
    List {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Show artifacts from all project scopes")]
        all: bool,
        #[arg(long, help = "Filter promoted goals by case-insensitive text")]
        goal: Option<String>,
        #[arg(
            long,
            help = "Only show artifacts promoted on or after YYYY-MM-DD or RFC3339"
        )]
        since: Option<String>,
        #[arg(
            long,
            help = "Only show artifacts promoted on or before YYYY-MM-DD or RFC3339"
        )]
        until: Option<String>,
        #[arg(long, help = "Print full TSV-style values for scripts")]
        full: bool,
        #[arg(long, help = "Emit machine-readable JSON")]
        json: bool,
    },
    #[command(about = "Search promoted artifact goals, scopes, and run ids")]
    Search {
        #[arg(help = "Case-insensitive search text")]
        query: String,
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Search artifacts from all project scopes")]
        all: bool,
        #[arg(
            long,
            help = "Only search artifacts promoted on or after YYYY-MM-DD or RFC3339"
        )]
        since: Option<String>,
        #[arg(
            long,
            help = "Only search artifacts promoted on or before YYYY-MM-DD or RFC3339"
        )]
        until: Option<String>,
    },
    #[command(about = "Show manifest and materialization details for one artifact")]
    Show {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
    },
}

pub(crate) struct RunCommandArgs {
    pub(crate) goal: String,
    pub(crate) fresh: bool,
    pub(crate) worktree: bool,
    pub(crate) from: Option<PathBuf>,
    pub(crate) in_place: bool,
    pub(crate) base: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) allow_dirty: bool,
    pub(crate) init_git: bool,
    pub(crate) yes: bool,
    pub(crate) preview: bool,
    pub(crate) brief: bool,
    pub(crate) no_seams: bool,
    pub(crate) plain: bool,
    pub(crate) prevent_sleep: Option<String>,
    pub(crate) quiet: bool,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) sandbox: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) doc_provider: Option<String>,
    pub(crate) acceptance: Option<PathBuf>,
    pub(crate) skill: String,
    pub(crate) smoke: bool,
    pub(crate) i_know_its_a_lot: bool,
    pub(crate) no_confirm: bool,
    pub(crate) no_hints: bool,
    pub(crate) no_docs: bool,
    pub(crate) doc_skill: Option<String>,
    pub(crate) narrate: bool,
    pub(crate) no_narrate: bool,
    pub(crate) narrator_model: Option<String>,
    pub(crate) infer_contract: bool,
}

pub(crate) struct StartCommandArgs {
    pub(crate) goal: String,
    pub(crate) mode: CliStartMode,
    pub(crate) plan: Option<PathBuf>,
    pub(crate) max_spend: Option<f64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) children: Option<u8>,
    pub(crate) planner_provider: Option<String>,
    pub(crate) child_provider: Vec<String>,
    pub(crate) coder_provider: Option<String>,
    pub(crate) reviewer_provider: Option<String>,
    pub(crate) preview: bool,
    pub(crate) yes: bool,
    pub(crate) no_seams: bool,
    pub(crate) fresh: bool,
    pub(crate) worktree: bool,
    pub(crate) from: Option<PathBuf>,
    pub(crate) allow_dirty: bool,
    pub(crate) plain: bool,
    pub(crate) quiet: bool,
    pub(crate) json: bool,
}

pub(crate) struct PlanCommandArgs {
    pub(crate) goal: String,
    pub(crate) n: u8,
    pub(crate) mode: CliPlanMode,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) sandbox: Option<String>,
    pub(crate) planner_provider: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) child_provider: Vec<String>,
    pub(crate) coder_provider: Option<String>,
    pub(crate) reviewer_provider: Option<String>,
    pub(crate) planner_model: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) child_model: Vec<String>,
    pub(crate) coder_model: Option<String>,
    pub(crate) reviewer_model: Option<String>,
    pub(crate) init_git: bool,
    pub(crate) acceptance: Option<PathBuf>,
    pub(crate) skip_acceptance_prompt: bool,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
}

pub(crate) struct ForkCommandArgs {
    pub(crate) plan_id: String,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) sandbox: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) child_provider: Vec<String>,
    pub(crate) coder_provider: Option<String>,
    pub(crate) reviewer_provider: Option<String>,
    pub(crate) no_repair: bool,
    pub(crate) repair_provider: Option<String>,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) completion_surface: bool,
    pub(crate) narrate: bool,
    pub(crate) narrator_model: Option<String>,
}

pub(crate) struct MergeCommandArgs {
    pub(crate) plan_id: String,
    pub(crate) strategy: String,
    pub(crate) prefer_child: Option<u32>,
    pub(crate) no_repair: bool,
    pub(crate) repair_provider: Option<String>,
    pub(crate) repair_mode: String,
    pub(crate) repair_attempts: u32,
    pub(crate) yes: bool,
    pub(crate) no_gate: bool,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) completion_surface: bool,
}

pub(crate) struct ChainCommandArgs {
    pub(crate) args: Vec<String>,
    pub(crate) from_file: Option<PathBuf>,
    pub(crate) from_stdin: bool,
    pub(crate) draft: bool,
    pub(crate) yes: bool,
    pub(crate) detach: bool,
    pub(crate) branch_policy: String,
    pub(crate) apply_mode: String,
    pub(crate) apply_strategy: String,
    pub(crate) apply_allowlist: Vec<String>,
    pub(crate) on_fail: String,
    pub(crate) circuit_breaker_threshold: u32,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) sandbox: String,
    pub(crate) base: Option<String>,
    pub(crate) n: u8,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) reason: Option<String>,
    pub(crate) from_step: Option<u32>,
    pub(crate) max_spend_add: Option<f64>,
    pub(crate) reset_breaker: bool,
    pub(crate) force: bool,
    pub(crate) step: Option<u32>,
    pub(crate) extend: Option<String>,
    pub(crate) reapply: bool,
    pub(crate) insert_at: Option<u32>,
    pub(crate) no_confirm: bool,
    pub(crate) full: bool,
    pub(crate) all: bool,
    pub(crate) why_failed: bool,
    pub(crate) json: bool,
}

pub(crate) struct ExtendCommandArgs {
    pub(crate) parent_run_id: String,
    pub(crate) new_goal: String,
    pub(crate) dest: Option<PathBuf>,
    pub(crate) max_context_turns: Option<u32>,
    pub(crate) no_context: bool,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) sandbox: Option<String>,
    pub(crate) no_docs: bool,
    pub(crate) doc_skill: Option<String>,
    pub(crate) post_actions: bool,
    pub(crate) narrate: bool,
    pub(crate) no_narrate: bool,
    pub(crate) narrator_model: Option<String>,
}
