#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::needless_pass_by_value,
        clippy::redundant_clone
    )
)]

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod setup;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::{Command as ClapCommand, CommandFactory, Parser};
use clap_complete::{Shell, generate};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use deadreckon::cards::exit_summary::{
    BranchDiffSummary, ExitSummaryInput, OutcomeKind, build_exit_summary_card,
};
use deadreckon::sleep::{self, SleepPrefs, SleepPrevention};
use deadreckon::ui_card::{
    Card, CardOptions, HintLine, Section, TitleGlyph, TitleLine, render_card,
};
use deadreckon_core::flight::{
    CheckpointManifest, FlightEvent, FlightEventKind, FlightSessionStatus, RewindEvent, RewindMode,
    RewindStatus, RewindTarget, RewindTargetKind, append_rewind_event, build_working_file_index,
    list_checkpoint_manifests, materialize_checkpoint, read_flight_events, read_flight_manifest,
};
use deadreckon_core::install_receipt::{Channel, detect_receipt, read_receipt, write_receipt};
use deadreckon_core::paths::workspace_scope;
use deadreckon_core::plan::write_plan_narrative;
use deadreckon_core::update_cache::{read_cache, write_cache};
use deadreckon_core::{
    AcceptanceMarker, AcceptanceProgressEntry, ApplyMode, ApplyStrategy, BranchPolicy, Chain,
    ChainEvent, ChainEventKind, ChainNewOptions, ChainStatus, ChainStepMarker, ChainStepStatus,
    CodebaseMode, CodebaseRecord, ConductorState, CoordinatorChild, CoordinatorState,
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DeadreckonError, DeadreckonPaths,
    DocKind, DocProviderSelection, DocProviderSource, DocsStatus, ModeFlags, OnFail, PhaseId,
    PhaseStatus, Plan, PlanChildMarker, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind,
    PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, PromotionManifest,
    ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunEvent, RunListEntry, RunOptions,
    RunStatus, SpendRecord, TraceRecord, WorktreeOptions, acceptance_progress_path_for_run_root,
    acceptance_spec_path_for_run_root, acquire_lock, append_chain_event,
    append_parent_narrative_update, append_plan_event, append_plan_message, append_provenance,
    append_trace, apply_commit_body, cancel_marker_present,
    chain_status_label as glossary_chain_status_label,
    chain_step_status_label as glossary_chain_step_status_label, clear_cancel_marker,
    copy_source_to_working, copy_tree, create_run, create_worktree, doc_path_for_kind,
    docs_status_for_state, emit_event, evaluate_acceptance_checks, inventory_files, list_runs,
    load_chain, load_plan, load_run, marker_path_for_run_root, pid_is_alive, plan_status_label,
    plan_task_status_label, prepare_worktree_record, preview_git_state, promote_completed_run,
    read_chain_step_marker, read_codebase_record, read_plan_messages, record_for_resolved_mode,
    release_lock_file, resolve_mode, restore_snapshot, run_status_label, save_chain, save_plan,
    save_state, terminate_pid, validate_acceptance_marker, validate_task_count,
    write_acceptance_marker, write_cancel_marker, write_chain_step_marker, write_child_summary,
    write_coordinator_state, write_plan_child_marker, write_worker_spec,
};
use deadreckon_providers::registry::{
    DescriptorKind, IngestCwdMatch, IngestDescriptor, IngestStorage, ProbeStatus, ProviderProbe,
    ProviderProbeOptions, ProviderProbeResult, ProviderRegistry,
};
use deadreckon_providers::taxonomy::normalize_tool_category;
use deadreckon_providers::{
    ProviderKind, ProviderRequest, ProviderRouteInfo, ProviderRouter, ProviderUsage, SpendEstimate,
    read_config,
};
use deadreckon_runtime::{
    PolishConfig, RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, polish_run_docs, run_turn_loop,
};
use deadreckon_sandbox::SandboxBackend;
use pulldown_cmark::{
    CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options as MarkdownOptions,
    Parser as MarkdownParser, Tag, TagEnd,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod cli;
mod plan_event_bus;
mod prompt;
mod tui_events;
mod ui;

use crate::cli::{
    AcceptanceCommand, AcceptancePreset, CHAIN_HELP, ChainCommandArgs, Cli, CliPlanMode, Commands,
    CompletionCommand, ConfigCommand, ExtendCommandArgs, ForkCommandArgs, HistoryCommand,
    HistoryKind, LibraryCommand, MergeCommandArgs, OrchestrateCommand, PlanCommandArgs,
    ProvidersCommand, RunCommandArgs,
};
use crate::plan_event_bus::{PlanEventBus, PlanFeedEvent};
use crate::ui::{
    ui_command, ui_error, ui_heading, ui_id, ui_muted, ui_note, ui_ok, ui_status, ui_warn,
};

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Core(#[from] DeadreckonError),
    #[error(transparent)]
    Provider(#[from] deadreckon_providers::ProviderError),
    #[error(transparent)]
    Sandbox(#[from] deadreckon_sandbox::SandboxError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML decode error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("{message}")]
    Exit {
        code: i32,
        message: String,
        hint: String,
    },
}

type Result<T> = std::result::Result<T, CliError>;

impl CliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Exit { code, .. } => *code,
            Self::Core(_)
            | Self::Provider(_)
            | Self::Sandbox(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::TomlDe(_)
            | Self::TomlSer(_) => 1,
        }
    }
}

fn error_hint(err: &CliError) -> String {
    match err {
        CliError::Exit { hint, .. } => hint.clone(),
        CliError::Provider(deadreckon_providers::ProviderError::MissingCredential(_))
        | CliError::Provider(deadreckon_providers::ProviderError::NoRoute(_)) => {
            "run `deadreckon init` or `deadreckon config set providers.anthropic.api_key <KEY>`"
                .to_string()
        }
        CliError::Core(DeadreckonError::InvalidInput(message))
            if message.contains("max spend above $50") =>
        {
            "rerun with `--i-know-its-a-lot` or lower `--max-spend`".to_string()
        }
        CliError::Core(DeadreckonError::InvalidInput(message))
            if message.contains("update: channel = source") =>
        {
            "try: cargo install --path crates/deadreckon".to_string()
        }
        CliError::Core(DeadreckonError::NotFound(_)) => {
            "run `deadreckon list` to find valid run ids or config keys".to_string()
        }
        CliError::Core(DeadreckonError::LockHeld { .. }) => {
            "run `deadreckon list`, then `deadreckon attach <run-id>` or `deadreckon kill <run-id>`"
                .to_string()
        }
        CliError::Sandbox(_) => {
            "run `deadreckon doctor` to inspect sandbox availability".to_string()
        }
        CliError::TomlDe(_) | CliError::TomlSer(_) => format!(
            "check {} or rerun `deadreckon init`",
            DeadreckonPaths::discover().config_path().display()
        ),
        CliError::Io(_) => "check that the referenced path exists and is writable".to_string(),
        CliError::Json(_) => "inspect the referenced JSON file for invalid syntax".to_string(),
        CliError::Core(_) | CliError::Provider(_) => "run `deadreckon doctor`".to_string(),
    }
}

fn print_kv_block(items: &[(&str, &str)]) {
    let _ = ui::kv_block(ui::Stream::Stdout, items);
}

fn print_error(err: &CliError) {
    eprintln!("{} {err}", ui_error("error:"));
}

fn print_error_hint(err: &CliError) {
    let _ = ui::hint(ui::Stream::Stderr, error_hint(err));
}

#[tokio::main]
async fn main() {
    if let Err(err) = main_inner().await {
        let exit_code = err.exit_code();
        print_error(&err);
        print_error_hint(&err);
        std::process::exit(exit_code);
    }
}

async fn main_inner() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    if wants_top_level_help() {
        print_top_help();
        return Ok(());
    }

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Commands::Status {
        run_id: None,
        all: false,
        plain: false,
        json: false,
    });
    let startup_update_check = start_startup_update_check(&command);
    let result = match command {
        Commands::Init {
            provider,
            api_key,
            base_url,
            max_spend,
            sandbox,
            no_confirm,
            no_completion,
        } => {
            init_command(
                provider,
                api_key,
                base_url,
                max_spend,
                sandbox,
                no_confirm,
                no_completion,
            )
            .await
        }
        Commands::Config { command } => config_command(command),
        Commands::HelpAll => {
            print_help_all();
            Ok(())
        }
        Commands::Completion { command } => completion_command(command),
        Commands::Acceptance { command } => acceptance_command(command).await,
        Commands::Done {
            args,
            provider,
            model,
            force,
            spec,
            against,
        } => done_command(args, provider, model, force, spec, against).await,
        Commands::Run {
            goal,
            fresh,
            worktree,
            from,
            in_place,
            base,
            branch,
            allow_dirty,
            init_git,
            yes,
            preview,
            brief,
            plain,
            prevent_sleep,
            quiet,
            max_spend,
            max_wall_seconds,
            sandbox,
            provider,
            model,
            doc_provider,
            acceptance,
            skill,
            smoke,
            i_know_its_a_lot,
            no_confirm,
            no_hints,
            no_docs,
            doc_skill,
        } => {
            ui::set_plain_output(plain);
            run_command(RunCommandArgs {
                goal,
                fresh,
                worktree,
                from,
                in_place,
                base,
                branch,
                allow_dirty,
                init_git,
                yes,
                preview,
                brief,
                plain,
                prevent_sleep,
                quiet,
                max_spend,
                max_wall_seconds,
                sandbox,
                provider,
                model,
                doc_provider,
                acceptance,
                skill,
                smoke,
                i_know_its_a_lot,
                no_confirm,
                no_hints,
                no_docs,
                doc_skill,
            })
            .await
        }
        Commands::Orchestrate {
            command,
            goal,
            max_spend,
            max_wall_seconds,
            sandbox,
            preview,
            init_git,
            acceptance,
            yes,
            no_repair,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            let request = orchestrate_request_from_cli(
                command,
                BareOrchestrateArgs {
                    goal,
                    max_spend,
                    max_wall_seconds,
                    sandbox,
                    preview,
                    init_git,
                    acceptance,
                    yes,
                    no_repair,
                    no_hints,
                    quiet,
                    plain,
                },
            )?;
            ui::set_plain_output(request.plan.plain);
            orchestrate_command(request).await
        }
        Commands::Plan {
            goal,
            n,
            mode,
            planner_provider,
            provider,
            child_provider,
            coder_provider,
            reviewer_provider,
            init_git,
            acceptance,
            no_hints,
            quiet,
            json,
            plain,
        } => {
            ui::set_plain_output(plain || json);
            plan_command(PlanCommandArgs {
                goal,
                n,
                mode,
                max_spend: None,
                max_wall_seconds: None,
                sandbox: None,
                planner_provider,
                provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                init_git,
                acceptance,
                skip_acceptance_prompt: quiet || json,
                no_hints,
                quiet,
                json,
                plain,
            })
            .await
        }
        Commands::Fork {
            plan_id,
            max_spend,
            max_wall_seconds,
            sandbox,
            provider,
            child_provider,
            coder_provider,
            reviewer_provider,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            fork_command(ForkCommandArgs {
                plan_id,
                max_spend,
                max_wall_seconds,
                sandbox,
                provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                no_hints,
                quiet,
                plain,
            })
            .await
        }
        Commands::Merge {
            plan_id,
            strategy,
            prefer_child,
            no_repair,
            repair_provider,
            repair_mode,
            repair_attempts,
            yes,
            no_gate,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            merge_command(MergeCommandArgs {
                plan_id,
                strategy,
                prefer_child,
                no_repair,
                repair_provider,
                repair_mode,
                repair_attempts,
                yes,
                no_gate,
                no_hints,
                quiet,
                plain,
            })
            .await
        }
        Commands::Chain {
            args,
            from_file,
            from_stdin,
            draft,
            yes,
            detach,
            branch_policy,
            apply_mode,
            apply_strategy,
            apply_allowlist,
            on_fail,
            circuit_breaker_threshold,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            base,
            n,
            no_hints,
            quiet,
            plain,
            reason,
            from_step,
            max_spend_add,
            reset_breaker,
            force,
            step,
            extend,
            reapply,
            insert_at,
            no_confirm,
            full,
            all,
            why_failed,
            json,
        } => {
            chain_command(ChainCommandArgs {
                args,
                from_file,
                from_stdin,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
                reason,
                from_step,
                max_spend_add,
                reset_breaker,
                force,
                step,
                extend,
                reapply,
                insert_at,
                no_confirm,
                full,
                all,
                why_failed,
                json,
            })
            .await
        }
        Commands::Doctor { json } => doctor_command(json).await,
        Commands::Detect { id, json, ping } => detect_command(id, json, ping).await,
        Commands::Providers { command } => providers_command(command).await,
        Commands::Update {
            check,
            force,
            allow_prerelease,
            yes,
            quiet,
            plain,
        } => update_command(check, force, allow_prerelease, yes, quiet, plain).await,
        Commands::List {
            scope,
            all,
            full,
            plain,
            json,
        } => {
            ui::set_plain_output(plain);
            list_command(scope, all, full, plain, json)
        }
        Commands::Library { command } => library_command(command),
        Commands::Finish {
            run_id,
            dest,
            force,
            include_manifest,
            strategy,
            branch,
            autostash,
            cleanup,
            no_confirm,
            message,
        } => finish_command(
            run_id,
            dest,
            force,
            include_manifest,
            strategy,
            branch,
            autostash,
            cleanup,
            no_confirm,
            message,
        ),
        Commands::Materialize {
            run_id,
            dest,
            force,
            include_manifest,
        } => materialize_command(run_id, dest, force, include_manifest),
        Commands::Apply {
            run_id,
            strategy,
            branch,
            no_confirm,
            autostash,
            cleanup,
            message,
            plain,
        } => apply_command(
            run_id, strategy, branch, no_confirm, autostash, cleanup, message, plain,
        ),
        Commands::Abandon {
            run_id,
            keep_branch,
            force,
        } => abandon_command(run_id, keep_branch, force),
        Commands::Cleanup {
            run_id,
            all,
            completed,
            stale,
            no_confirm,
            force,
            overwrite,
            keep_branch,
        } => cleanup_command(CleanupCommandRequest {
            run_id,
            all,
            completed,
            stale,
            no_confirm,
            escalate: force,
            overwrite,
            keep_branch,
        }),
        Commands::Extend {
            parent_run_id,
            new_goal,
            dest,
            max_context_turns,
            no_context,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            no_docs,
            doc_skill,
        } => {
            extend_command(ExtendCommandArgs {
                parent_run_id,
                new_goal,
                dest,
                max_context_turns,
                no_context,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                no_docs,
                doc_skill,
                post_actions: completion_hints_enabled(false),
            })
            .await
        }
        Commands::Doc {
            run_id,
            kind,
            export,
            polish,
            no_confirm,
            force,
            doc_skill,
            doc_provider,
            budget_cap,
        } => {
            doc_command(DocCommandArgs {
                run_id,
                kind: kind.into(),
                export,
                polish,
                no_confirm,
                force,
                doc_skill,
                doc_provider,
                budget_cap,
            })
            .await
        }
        Commands::Attach {
            run_id,
            no_hints,
            plain,
        } => {
            ui::set_plain_output(plain);
            attach_command(run_id, no_hints, plain).await
        }
        Commands::Kill {
            run_id,
            force,
            plain,
        } => {
            ui::set_plain_output(plain);
            kill_command(run_id, force, plain)
        }
        Commands::Resume {
            run_id,
            from_turn,
            max_wall_seconds,
            no_docs,
            plain,
        } => {
            ui::set_plain_output(plain);
            resume_command(run_id, from_turn, max_wall_seconds, no_docs, plain).await
        }
        Commands::Undo { run, turn } => undo_command(run, turn),
        Commands::Rewind {
            run_id,
            to_turn,
            to_provider_event,
            to_checkpoint,
            preview,
            apply,
            plain,
            json,
        } => {
            ui::set_plain_output(plain);
            rewind_command(
                &run_id,
                &RewindCliOptions {
                    to_turn,
                    to_provider_event,
                    to_checkpoint,
                    preview,
                    apply,
                    json,
                },
            )
        }
        Commands::Show {
            run_id,
            turn,
            why_failed,
            plain,
            json,
            flight,
            file,
        } => {
            ui::set_plain_output(plain);
            show_command(
                &run_id,
                turn,
                why_failed,
                plain,
                json,
                flight,
                file.as_deref(),
            )
        }
        Commands::History { command } => history_command(command),
        Commands::Status {
            run_id,
            all,
            plain,
            json,
        } => {
            ui::set_plain_output(plain);
            status_command(run_id, all, plain, json)
        }
        Commands::Import {
            source,
            preview,
            list,
            session,
            cwd,
            all,
            since,
            replace,
            json,
        } => import_command(ImportCommandOptions {
            source,
            preview,
            list,
            session,
            cwd,
            all,
            since,
            replace,
            json,
        }),
    };
    print_startup_update_hint(startup_update_check).await;
    result
}

fn start_startup_update_check(
    command: &Commands,
) -> Option<tokio::task::JoinHandle<Option<String>>> {
    if !startup_update_check_enabled(command) {
        return None;
    }
    Some(tokio::spawn(async {
        let paths = DeadreckonPaths::discover();
        let Ok(receipt) = update_receipt_for_current_binary(&paths, false) else {
            return None;
        };
        if receipt.channel == Channel::Source {
            return None;
        }

        let now = Utc::now();
        match read_cache(&paths) {
            Ok(Some(cache)) if !cache.is_stale(now) => cache
                .update_available
                .then(|| startup_update_hint(&cache.latest_version)),
            Ok(_) => {
                let current = update_current_version(&receipt);
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(3),
                        resolve_latest_update(&paths, &current, false),
                    )
                    .await;
                });
                None
            }
            Err(_) => None,
        }
    }))
}

fn startup_update_check_enabled(command: &Commands) -> bool {
    std::env::var("DEADRECKON_UPDATE_CHECK").as_deref() != Ok("0")
        && startup_update_stdout_is_tty()
        && !matches!(command, Commands::Update { .. } | Commands::Doctor { .. })
}

fn startup_update_stdout_is_tty() -> bool {
    std::env::var_os("DEADRECKON_UPDATE_TEST_TTY").is_some() || io::stdout().is_terminal()
}

fn startup_update_hint(version: &str) -> String {
    format!("→ deadreckon {version} is available. Run `deadreckon update`.")
}

async fn print_startup_update_hint(check: Option<tokio::task::JoinHandle<Option<String>>>) {
    if let Some(check) = check
        && let Ok(Ok(Some(hint))) = tokio::time::timeout(Duration::from_millis(50), check).await
    {
        eprintln!("{hint}");
    }
}

fn wants_top_level_help() -> bool {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        return false;
    };
    if args.next().is_some() {
        return false;
    }
    matches!(arg.to_string_lossy().as_ref(), "-h" | "--help" | "help")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TopHelpGroup {
    CoreLifecycle,
    ContinueRecover,
    MoreHelp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelpAllGroup {
    SetupProviders,
    CoreLifecycle,
    Orchestration,
    ContinueRecover,
    InspectAdvanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandDiscovery {
    Public,
    Advanced,
    Compatibility,
    Pseudo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommandHelpEntry {
    display: &'static str,
    clap_name: Option<&'static str>,
    purpose: &'static str,
    top_group: Option<TopHelpGroup>,
    all_group: Option<HelpAllGroup>,
}

const TYPICAL_FLOW_COMMANDS: &[&str] = &[
    "deadreckon def-done \"builds, tests pass, and opens in a browser\"",
    "deadreckon run \"build the thing\"",
    "deadreckon attach latest",
    "deadreckon finish latest",
];

const COMMAND_HELP_CATALOG: &[CommandHelpEntry] = &[
    CommandHelpEntry {
        display: "init",
        clap_name: Some("init"),
        purpose: "configure deadreckon",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "doctor",
        clap_name: Some("doctor"),
        purpose: "check provider, sandbox, and local setup",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "detect",
        clap_name: Some("detect"),
        purpose: "probe registered providers",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "providers",
        clap_name: Some("providers"),
        purpose: "list provider routes and models",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "config",
        clap_name: Some("config"),
        purpose: "read or update configuration",
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "completion",
        clap_name: Some("completion"),
        purpose: "install or generate shell completions",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "def-done",
        clap_name: Some("def-done"),
        purpose: "write/check done criteria in English",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "run",
        clap_name: Some("run"),
        purpose: "start unattended coding work",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "orchestrate",
        clap_name: Some("orchestrate"),
        purpose: "run coder/reviewer or full-plan multi-agent work",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "chain",
        clap_name: Some("chain"),
        purpose: "run several coding steps in sequence",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "attach",
        clap_name: Some("attach"),
        purpose: "watch a run, chain, or plan in the TUI",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "status",
        clap_name: Some("status"),
        purpose: "see the latest run and next action",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "list",
        clap_name: Some("list"),
        purpose: "show runs and plans",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "finish",
        clap_name: Some("finish"),
        purpose: "apply or export completed work",
        top_group: Some(TopHelpGroup::CoreLifecycle),
        all_group: Some(HelpAllGroup::CoreLifecycle),
    },
    CommandHelpEntry {
        display: "plan",
        clap_name: Some("plan"),
        purpose: "write a multi-agent plan without starting it",
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "fork",
        clap_name: Some("fork"),
        purpose: "start child runs for a plan",
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "merge",
        clap_name: Some("merge"),
        purpose: "compose completed plan children",
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "extend",
        clap_name: Some("extend"),
        purpose: "continue from a completed run",
        top_group: Some(TopHelpGroup::ContinueRecover),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "resume",
        clap_name: Some("resume"),
        purpose: "resume an incomplete run",
        top_group: Some(TopHelpGroup::ContinueRecover),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "kill",
        clap_name: Some("kill"),
        purpose: "cancel a run, chain, or plan",
        top_group: Some(TopHelpGroup::ContinueRecover),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "cleanup",
        clap_name: Some("cleanup"),
        purpose: "remove stale or completed worktrees",
        top_group: Some(TopHelpGroup::ContinueRecover),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "undo",
        clap_name: Some("undo"),
        purpose: "restore an in-place snapshot",
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "abandon",
        clap_name: Some("abandon"),
        purpose: "discard a temporary worktree run",
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "update",
        clap_name: Some("update"),
        purpose: "check for or route self-updates",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "history",
        clap_name: Some("history"),
        purpose: "search run traces and provenance",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "apply",
        clap_name: Some("apply"),
        purpose: "merge a completed worktree run",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "export",
        clap_name: Some("materialize"),
        purpose: "copy a completed fresh/copy run (alias: materialize)",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "doc",
        clap_name: Some("doc"),
        purpose: "read or regenerate run docs",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "library",
        clap_name: Some("library"),
        purpose: "inspect promoted artifacts",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "show",
        clap_name: Some("show"),
        purpose: "show raw state, traces, and provenance",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "import",
        clap_name: Some("import"),
        purpose: "import other tool history",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "acceptance",
        clap_name: Some("acceptance"),
        purpose: "advanced compatibility command for done criteria",
        top_group: None,
        all_group: Some(HelpAllGroup::InspectAdvanced),
    },
    CommandHelpEntry {
        display: "help-all",
        clap_name: Some("help-all"),
        purpose: "show every command, including advanced commands (alias: commands)",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: None,
    },
    CommandHelpEntry {
        display: "<command> --help",
        clap_name: None,
        purpose: "detailed help for one command",
        top_group: Some(TopHelpGroup::MoreHelp),
        all_group: None,
    },
];

const HELP_ALL_GROUPS: &[(HelpAllGroup, &str)] = &[
    (HelpAllGroup::SetupProviders, "setup and providers"),
    (HelpAllGroup::CoreLifecycle, "core lifecycle"),
    (HelpAllGroup::Orchestration, "orchestration"),
    (HelpAllGroup::ContinueRecover, "continue and recover"),
    (HelpAllGroup::InspectAdvanced, "inspect and advanced"),
];

const HELP_ALL_DISCOVERY_NOTE: &str = "Advanced commands are documented here but hidden from short help; compatibility aliases stay inline on their canonical command row.";

const FLAG_POLICY_ROWS: &[(&str, &str)] = &[
    (
        "--yes",
        "confirms preflight previews for start/update-style commands",
    ),
    (
        "--no-confirm",
        "skips destructive or follow-up confirmations after a target is known",
    ),
    (
        "--quiet",
        "suppresses success chatter and post-action hints, never requested data or errors",
    ),
    (
        "--plain",
        "disables TUI, spinner, and ANSI affordances; it does not imply quiet",
    ),
    (
        "--json",
        "is for inspection/list surfaces; JSON wins over styling and hints",
    ),
    (
        "--no-hints",
        "suppresses optional next-step hints; DEADRECKON_HINTS=0 also disables them",
    ),
];

const PROVIDER_ROLE_ROWS: &[(&str, &str)] = &[
    (
        "--provider",
        "primary run provider route; in full-plan orchestration, the default child route",
    ),
    (
        "--planner-provider",
        "full-plan route that writes the child graph before fork",
    ),
    (
        "--child-provider IDX=PROVIDER",
        "per-child route override for full-plan work",
    ),
    (
        "--coder-provider",
        "review-mode route that performs the implementation pass",
    ),
    (
        "--reviewer-provider",
        "review-mode route that independently reviews or fixes the coder result",
    ),
    (
        "--doc-provider",
        "documentation polish route; defaults through config, then run provider",
    ),
    (
        "--repair-provider",
        "merge repair planning and repair-child route",
    ),
];

const CAP_POLICY_ROWS: &[(&str, &str)] = &[
    (
        "run cap",
        "`run --max-spend` limits one run's provider spend",
    ),
    (
        "per-child cap",
        "`orchestrate`/`fork --max-spend` limits each child run",
    ),
    (
        "aggregate chain cap",
        "`chain --max-spend` limits cumulative chain spend",
    ),
    (
        "doc polish cap",
        "`doc --max-spend` limits documentation polish spend",
    ),
];

fn command_discovery(entry: &CommandHelpEntry) -> CommandDiscovery {
    match entry.display {
        "apply" | "export" | "doc" | "library" | "show" | "import" | "undo" | "abandon" => {
            CommandDiscovery::Advanced
        }
        "acceptance" => CommandDiscovery::Compatibility,
        "<command> --help" => CommandDiscovery::Pseudo,
        _ => CommandDiscovery::Public,
    }
}

fn print_catalog_rows<'a>(rows: impl IntoIterator<Item = &'a CommandHelpEntry>) {
    let rows = rows.into_iter().collect::<Vec<_>>();
    let width = rows
        .iter()
        .map(|entry| entry.display.chars().count())
        .max()
        .unwrap_or(0);
    for entry in rows {
        println!(
            "  {:<width$} {}",
            ui_command(entry.display),
            entry.purpose,
            width = width
        );
    }
}

fn print_top_help_group(title: &str, group: TopHelpGroup) {
    println!("{}", ui_heading(title));
    print_catalog_rows(
        COMMAND_HELP_CATALOG
            .iter()
            .filter(|entry| entry.top_group == Some(group)),
    );
}

fn print_top_help() {
    println!(
        "{} {}",
        ui_heading("deadreckon"),
        ui_muted(env!("CARGO_PKG_VERSION"))
    );
    println!(
        "deadreckon runs long coding goals in an isolated worktree or sandbox, tracks durable state, and gives you explicit apply/export/cleanup steps."
    );
    println!();
    println!("{}", ui_heading("Usage:"));
    println!("  {}", ui_command("deadreckon [command]"));
    println!();
    println!("{}", ui_heading("Typical flow:"));
    for command in TYPICAL_FLOW_COMMANDS {
        println!("  {}", ui_command(command));
    }
    println!();
    print_top_help_group("Core lifecycle:", TopHelpGroup::CoreLifecycle);
    println!();
    print_top_help_group("Continue or recover:", TopHelpGroup::ContinueRecover);
    println!();
    print_top_help_group("More help:", TopHelpGroup::MoreHelp);
    println!();
    println!(
        "{} Run, chain, and plan ids accept unique prefixes where that command accepts the kind. {} means the newest item for the current project.",
        ui_heading("Note:"),
        ui_command("latest")
    );
    println!();
    println!("{}", ui_heading("Options:"));
    for (flag, purpose) in [
        ("-h, --help", "print help"),
        ("-V, --version", "print version"),
    ] {
        println!("  {:<18} {}", ui_command(flag), purpose);
    }
}

fn print_help_all() {
    println!("{}", ui_heading("deadreckon commands"));
    if COMMAND_HELP_CATALOG
        .iter()
        .any(|entry| command_discovery(entry) == CommandDiscovery::Advanced)
    {
        println!("{}", ui_muted(HELP_ALL_DISCOVERY_NOTE));
    }
    for (group, title) in HELP_ALL_GROUPS {
        println!();
        println!("{}", ui_heading(title));
        print_catalog_rows(
            COMMAND_HELP_CATALOG
                .iter()
                .filter(|entry| entry.all_group == Some(*group)),
        );
    }
    println!();
    println!("{}", ui_heading("output and scripting policy"));
    for (flag, purpose) in FLAG_POLICY_ROWS {
        println!("  {:<16} {}", ui_command(flag), purpose);
    }
    println!();
    println!("{}", ui_heading("provider roles"));
    println!(
        "{}",
        ui_muted(
            "Normal output says provider route/model/kind; descriptor is reserved for registry docs."
        )
    );
    for (flag, purpose) in PROVIDER_ROLE_ROWS {
        println!("  {:<32} {}", ui_command(flag), purpose);
    }
    println!();
    println!("{}", ui_heading("spend cap glossary"));
    for (label, purpose) in CAP_POLICY_ROWS {
        println!("  {:<22} {}", ui_command(label), purpose);
    }
    println!();
    println!(
        "Use {} for detailed help.",
        ui_command("deadreckon <command> --help")
    );
}

fn completion_command(command: Option<CompletionCommand>) -> Result<()> {
    match command.unwrap_or(CompletionCommand::Install {
        shell: None,
        path: None,
        no_rc: false,
    }) {
        CompletionCommand::Install { shell, path, no_rc } => {
            install_completion(shell, path, !no_rc)?;
        }
        CompletionCommand::Bash => write_completion_script(Shell::Bash, &mut io::stdout()),
        CompletionCommand::Elvish => write_completion_script(Shell::Elvish, &mut io::stdout()),
        CompletionCommand::Fish => write_completion_script(Shell::Fish, &mut io::stdout()),
        CompletionCommand::PowerShell => {
            write_completion_script(Shell::PowerShell, &mut io::stdout());
        }
        CompletionCommand::Zsh => write_completion_script(Shell::Zsh, &mut io::stdout()),
    }
    Ok(())
}

fn write_completion_script(shell: Shell, output: &mut dyn Write) {
    let mut command = completion_command_tree();
    let bin_name = command.get_name().to_string();
    generate(shell, &mut command, bin_name, output);
}

fn completion_command_tree() -> ClapCommand {
    unhide_completion_commands(Cli::command())
}

fn unhide_completion_commands(command: ClapCommand) -> ClapCommand {
    command
        .hide(false)
        .mut_subcommands(unhide_completion_commands)
}

fn install_completion(
    shell: Option<Shell>,
    path_override: Option<PathBuf>,
    update_rc: bool,
) -> Result<PathBuf> {
    let shell = shell.or_else(detect_completion_shell).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "could not detect your shell",
            "deadreckon completion install --shell zsh",
        ))
    })?;
    let path = path_override.unwrap_or_else(|| default_completion_path(shell));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut script = Vec::new();
    write_completion_script(shell, &mut script);
    fs::write(&path, script)?;
    println!("{} completion {}", ui_ok("installed"), path.display());
    if shell == Shell::Zsh && update_rc {
        ensure_zsh_completion_rc(&path)?;
    }
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("open a new shell, or source your shell rc file")
    );
    Ok(path)
}

fn try_install_completion_after_init() {
    match install_completion(None, None, true) {
        Ok(_) => {}
        Err(err) => {
            eprintln!("{} shell completion not installed: {err}", ui_note("note"));
            eprintln!(
                "{} {}",
                ui_command("try:"),
                ui_command("deadreckon completion install --shell zsh")
            );
        }
    }
}

fn detect_completion_shell() -> Option<Shell> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_string_lossy();
    match name.as_ref() {
        "bash" => Some(Shell::Bash),
        "elvish" => Some(Shell::Elvish),
        "fish" => Some(Shell::Fish),
        "pwsh" | "powershell" => Some(Shell::PowerShell),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

fn default_completion_path(shell: Shell) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match shell {
        Shell::Bash => home
            .join(".local")
            .join("share")
            .join("bash-completion")
            .join("completions")
            .join("deadreckon"),
        Shell::Elvish => home
            .join(".config")
            .join("elvish")
            .join("lib")
            .join("deadreckon-completers.elv"),
        Shell::Fish => home
            .join(".config")
            .join("fish")
            .join("completions")
            .join("deadreckon.fish"),
        Shell::PowerShell => home
            .join(".config")
            .join("powershell")
            .join("deadreckon-completions.ps1"),
        Shell::Zsh => home.join(".zsh").join("completions").join("_deadreckon"),
        _ => home.join(".deadreckon").join("completion"),
    }
}

fn ensure_zsh_completion_rc(completion_path: &Path) -> Result<()> {
    let Some(completion_dir) = completion_path.parent() else {
        return Ok(());
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let zshrc = home.join(".zshrc");
    let existing = fs::read_to_string(&zshrc).unwrap_or_default();
    let completion_dir = completion_dir.display();
    let block = format!(
        "\n# >>> deadreckon completion >>>\nfpath=({completion_dir} $fpath)\nautoload -Uz compinit\ncompinit\n# <<< deadreckon completion <<<\n"
    );
    if existing.contains("# >>> deadreckon completion >>>")
        || existing.contains(&format!("fpath=({completion_dir} $fpath)"))
    {
        println!("{} zshrc already loads completion dir", ui_ok("ready"));
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&zshrc)?;
    file.write_all(block.as_bytes())?;
    println!("{} {}", ui_ok("updated"), zshrc.display());
    Ok(())
}

async fn init_command(
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    max_spend: f64,
    sandbox: String,
    no_confirm: bool,
    no_completion: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    fs::create_dir_all(paths.home())?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let provider = match provider {
        Some(provider) => provider,
        None if no_confirm => {
            auto_subscription_cli_provider(&registry).unwrap_or_else(|| "anthropic".to_string())
        }
        None => prompt_provider()?,
    };
    provider_setup_selection(
        &paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::ConfigDefault,
            explicit_provider: Some(&provider),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: Some("anthropic"),
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: false,
        },
    )?;
    let api_key = api_key.or_else(|| {
        if provider.starts_with("cli:") {
            None
        } else {
            prompt::open("provider API key (leave blank to use env var): ", None).ok()
        }
    });
    let config = init_config_text(
        &provider,
        api_key.as_deref(),
        base_url.as_deref(),
        max_spend,
        &sandbox,
    );
    fs::write(paths.config_path(), config)?;
    let provider_setup = provider_setup_selection(
        &paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::ConfigDefault,
            explicit_provider: Some(&provider),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: Some("anthropic"),
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: false,
        },
    )?;
    println!("{} {}", ui_ok("wrote"), paths.config_path().display());
    print_provider_setup_rows(&[provider_setup]);
    doctor_command(false).await?;
    if !no_completion {
        try_install_completion_after_init();
    }
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("deadreckon run \"describe the coding goal\"")
    );
    Ok(())
}

fn auto_subscription_cli_provider(registry: &ProviderRegistry) -> Option<String> {
    setup::auto_subscription_cli_provider(registry)
}

fn setup_refusal_error(refusal: setup::SetupRefusal) -> CliError {
    let setup::SetupRefusal { message, try_line } = refusal;
    CliError::Core(deadreckon_core::user_error(&message, &try_line))
}

fn provider_setup_selection(
    paths: &DeadreckonPaths,
    request: setup::ProviderSetupRequest<'_>,
) -> Result<setup::ProviderSetupSelection> {
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let auto_subscription_provider = if request.allow_auto_subscription {
        auto_subscription_cli_provider(&registry)
    } else {
        None
    };
    let request = setup::ProviderSetupRequest {
        auto_subscription_provider: request
            .auto_subscription_provider
            .or(auto_subscription_provider.as_deref()),
        ..request
    };
    setup::select_provider_setup(&paths.config_path(), &registry, request)
        .map_err(setup_refusal_error)
}

fn doc_provider_setup_selection(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    flag: Option<&str>,
    run_provider: Option<&str>,
    require_usable_route: bool,
) -> Result<setup::ProviderSetupSelection> {
    provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::DocPolish,
            explicit_provider: flag,
            explicit_model: None,
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route,
        },
    )
}

fn doc_provider_selection_from_setup(
    selection: &setup::ProviderSetupSelection,
) -> DocProviderSelection {
    DocProviderSelection {
        provider: selection.provider.clone(),
        source: doc_provider_source_from_setup(selection.source),
    }
}

fn doc_provider_source_from_setup(source: setup::SetupProviderSource) -> DocProviderSource {
    match source {
        setup::SetupProviderSource::Flag => DocProviderSource::Flag,
        setup::SetupProviderSource::Config => DocProviderSource::Config,
        setup::SetupProviderSource::AutoSubscription => DocProviderSource::AutoSubscription,
        setup::SetupProviderSource::RunProvider => DocProviderSource::RunProvider,
        setup::SetupProviderSource::BuiltInDefault | setup::SetupProviderSource::None => {
            DocProviderSource::None
        }
    }
}

fn provider_override_from_setup(selection: &setup::ProviderSetupSelection) -> Option<String> {
    match selection.source {
        setup::SetupProviderSource::BuiltInDefault | setup::SetupProviderSource::None => None,
        setup::SetupProviderSource::Flag
        | setup::SetupProviderSource::Config
        | setup::SetupProviderSource::AutoSubscription
        | setup::SetupProviderSource::RunProvider => selection.provider.clone(),
    }
}

fn config_command(command: ConfigCommand) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    match command {
        ConfigCommand::Get { key } => {
            let root = load_config_value(&paths)?;
            match get_toml_path(&root, &key) {
                Some(value) => println!("{}", value_to_display(value)),
                None => {
                    return Err(CliError::Core(DeadreckonError::NotFound(format!(
                        "config key {key}"
                    ))));
                }
            }
        }
        ConfigCommand::Set { key, value } => {
            fs::create_dir_all(paths.home())?;
            let mut root = load_config_value(&paths)?;
            set_toml_path(&mut root, &key, parse_config_value(&value));
            fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
            println!("{} {key}", ui_ok("set"));
        }
        ConfigCommand::Provider { provider } => match provider {
            Some(provider) => {
                let selection = provider_setup_selection(
                    &paths,
                    setup::ProviderSetupRequest {
                        role: setup::SetupProviderRoleRef::ConfigDefault,
                        explicit_provider: Some(&provider),
                        explicit_model: None,
                        config_default_provider: None,
                        config_doc_provider: None,
                        run_provider: None,
                        auto_subscription_provider: None,
                        built_in_default_provider: None,
                        use_router_default: false,
                        allow_auto_subscription: false,
                        require_usable_route: false,
                    },
                )?;
                fs::create_dir_all(paths.home())?;
                let mut root = load_config_value(&paths)?;
                set_toml_path(
                    &mut root,
                    "defaults.provider",
                    toml::Value::String(provider.clone()),
                );
                set_toml_path(
                    &mut root,
                    "default_provider",
                    toml::Value::String(provider.clone()),
                );
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
                println!("{} default provider {}", ui_ok("set"), ui_id(&provider));
                print_provider_setup_rows(&[selection]);
            }
            None => print_provider_selection(&paths, None)?,
        },
        ConfigCommand::Model { model, provider } => match model {
            Some(model) => {
                fs::create_dir_all(paths.home())?;
                let mut root = load_config_value(&paths)?;
                let provider = provider
                    .or_else(|| active_provider_from_config(&root))
                    .ok_or_else(|| {
                        CliError::Core(deadreckon_core::user_error(
                            "no active provider configured",
                            "deadreckon config provider cli:codex",
                        ))
                    })?;
                set_provider_model(&mut root, &provider, &model);
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
                println!(
                    "{} model for {} -> {}",
                    ui_ok("set"),
                    ui_id(provider),
                    ui_id(model)
                );
            }
            None => print_provider_selection(&paths, provider.as_deref())?,
        },
    }
    Ok(())
}

fn print_provider_selection(paths: &DeadreckonPaths, provider: Option<&str>) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider)?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let defaults = config_defaults(paths).ok();
    let routes = router.route_info();
    let selected = router.selected_route_info();
    println!("{}", ui_heading("provider selection"));
    for route in routes {
        let marker = if selected
            .as_ref()
            .is_some_and(|selected| selected.name == route.name)
        {
            "*"
        } else {
            " "
        };
        let credential = if route.has_credential {
            "ready"
        } else {
            "missing"
        };
        let kind = registry
            .get(&route.name)
            .map(|descriptor| descriptor_kind_label(&descriptor.kind))
            .unwrap_or_else(|| provider_kind_label(&route.kind));
        println!(
            "{marker} {}  kind={}  model={}  credential={credential}",
            ui_id(route.name),
            kind,
            route.model
        );
    }
    if let Some(selected) = selected {
        println!(
            "{} {}",
            ui_command("try:"),
            ui_command(format!(
                "deadreckon run \"goal\" --provider {} --model <model>",
                selected.name
            ))
        );
        println!(
            "{} {}",
            ui_command("default model:"),
            ui_command(format!(
                "deadreckon config model <model> --provider {}",
                selected.name
            ))
        );
    }
    if let Ok(selection) = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::ConfigDefault,
            explicit_provider: provider,
            explicit_model: None,
            config_default_provider: defaults
                .as_ref()
                .and_then(|defaults| defaults.provider.as_deref()),
            config_doc_provider: defaults
                .as_ref()
                .and_then(|defaults| defaults.doc_provider.as_deref()),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: true,
            allow_auto_subscription: false,
            require_usable_route: false,
        },
    ) {
        print_provider_setup_rows(&[selection]);
    }
    Ok(())
}

fn print_provider_setup_rows(selections: &[setup::ProviderSetupSelection]) {
    if selections.is_empty() {
        return;
    }
    println!("{}", ui_heading("provider setup"));
    let rows = selections
        .iter()
        .map(|selection| (selection.role.label(), selection.row_value()))
        .collect::<Vec<_>>();
    let refs = rows
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    print_kv_block(&refs);
    for selection in selections {
        for warning in &selection.warnings {
            println!("  {}", ui_warn(warning));
        }
        for try_line in &selection.try_lines {
            println!("  {} {}", ui_command("try:"), ui_command(try_line));
        }
    }
}

fn active_provider_from_config(root: &toml::Value) -> Option<String> {
    get_toml_path(root, "defaults.provider")
        .or_else(|| get_toml_path(root, "default_provider"))
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

fn set_provider_model(root: &mut toml::Value, provider: &str, model: &str) {
    if !root.is_table() {
        *root = toml::Value::Table(Default::default());
    }
    let Some(root_table) = root.as_table_mut() else {
        return;
    };
    let providers = root_table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !providers.is_table() {
        *providers = toml::Value::Table(Default::default());
    }
    let Some(providers_table) = providers.as_table_mut() else {
        return;
    };
    let provider_entry = providers_table
        .entry(provider.to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !provider_entry.is_table() {
        *provider_entry = toml::Value::Table(Default::default());
    }
    if let Some(provider_table) = provider_entry.as_table_mut() {
        provider_table.insert("model".to_string(), toml::Value::String(model.to_string()));
    }
}

fn provider_kind_label(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic | ProviderKind::OpenAi => "http",
        ProviderKind::OpenAiCompatible => "local-http",
        ProviderKind::CliClaudeCode | ProviderKind::CliCodex => "cli",
        ProviderKind::ScriptedSmoke => "scripted",
        ProviderKind::Generic(_) => "custom",
    }
}

async fn detect_command(id: Option<String>, json_output: bool, ping: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let options = ProviderProbeOptions { ping };
    let requested_id = id.clone();
    let results = if let Some(id) = id {
        let Some(descriptor) = registry.get(&id) else {
            let message = format!("no provider '{id}' in registry");
            return Err(CliError::Core(deadreckon_core::user_error(
                &message,
                "deadreckon providers list",
            )));
        };
        vec![descriptor.probe(options).await]
    } else {
        registry.probe_all(options).await
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "provider_detect",
                "id": requested_id.as_deref().unwrap_or("all"),
                "status": "ok",
                "next_actions": ["deadreckon providers list"],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "providers": results,
            }))?
        );
    } else {
        print_detect_results(&results);
    }
    Ok(())
}

async fn providers_command(command: ProvidersCommand) -> Result<()> {
    match command {
        ProvidersCommand::List {
            models,
            all,
            full,
            json,
        } => providers_list_command(models, all, full, json).await,
    }
}

async fn update_command(
    check: bool,
    force: bool,
    allow_prerelease: bool,
    yes: bool,
    quiet: bool,
    plain: bool,
) -> Result<()> {
    ui::set_plain_output(plain);
    let paths = DeadreckonPaths::discover();
    let receipt = update_receipt_for_current_binary(&paths, !check)?;
    let current = update_current_version(&receipt);
    if check {
        let latest = resolve_latest_update(&paths, &current, allow_prerelease).await?;
        if !quiet {
            print_update_check(receipt.channel, &current, &latest);
        }
        return Ok(());
    }

    match receipt.channel {
        Channel::Npm | Channel::Brew | Channel::Cargo => {
            if !quiet {
                println!("channel: {}", receipt.channel.as_str());
                println!("current: {current}");
                println!("try: {}", channel_native_update_command(receipt.channel));
            }
            Ok(())
        }
        Channel::Source => Err(CliError::Core(DeadreckonError::InvalidInput(
            "update: channel = source; in-place swap not supported".to_string(),
        ))),
        Channel::Shell => {
            update_shell_channel(&paths, &receipt, force, allow_prerelease, yes, quiet).await
        }
    }
}

async fn update_shell_channel(
    paths: &DeadreckonPaths,
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    yes: bool,
    quiet: bool,
) -> Result<()> {
    if receipt.channel != Channel::Shell {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "update: shell updater requires shell receipt, got {}",
            receipt.channel.as_str()
        ))));
    }
    let current = update_current_version(receipt);
    let latest = resolve_latest_update(paths, &current, allow_prerelease).await?;
    let backup_dir = unique_shell_backup_dir(&shell_backup_root(paths));
    if !quiet {
        print_shell_update_preview(&current, &latest, &backup_dir);
    }
    confirm_shell_update(yes)?;
    let backup_dir = create_shell_update_backup(paths, receipt, backup_dir)?;
    match run_shell_swap(receipt, force, allow_prerelease, quiet).await {
        Ok(()) => {
            prune_shell_backups(paths)?;
            if !quiet {
                println!("channel: shell");
                println!("current: {current}");
                println!("backup: {}", backup_dir.display());
                println!("updated: {}", receipt.binary_path.display());
                println!("try: deadreckon doctor");
            }
            Ok(())
        }
        Err(source) => Err(shell_update_failure(receipt, &backup_dir, &source)),
    }
}

fn create_shell_update_backup(
    paths: &DeadreckonPaths,
    receipt: &deadreckon_core::install_receipt::Receipt,
    backup_dir: PathBuf,
) -> Result<PathBuf> {
    let root = shell_backup_root(paths);
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&backup_dir)?;
    fs::copy(&receipt.binary_path, backup_dir.join("deadreckon"))?;
    fs::write(
        backup_dir.join("receipt.json"),
        serde_json::to_vec_pretty(receipt)?,
    )?;
    Ok(backup_dir)
}

fn print_shell_update_preview(current: &str, latest: &LatestUpdate, backup_dir: &Path) {
    println!("channel: shell");
    println!("current: {current}");
    println!("target: {}", latest.version);
    println!("archive: {}", latest.archive_url());
    println!("sha256: {}", latest.sha256());
    println!("backup: {}", backup_dir.display());
}

fn confirm_shell_update(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive shell update requires --yes after reviewing preview",
            "deadreckon update --yes",
        )));
    }
    if prompt::confirm("apply this shell update?", false)? {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(
            "update cancelled by user".to_string(),
        )))
    }
}

fn shell_backup_root(paths: &DeadreckonPaths) -> PathBuf {
    paths.home().join("update-backups")
}

fn unique_shell_backup_dir(root: &Path) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%d%H%M%S%3f").to_string();
    let mut candidate = root.join(&stamp);
    let mut suffix = 1_u32;
    while candidate.exists() {
        candidate = root.join(format!("{stamp}-{suffix}"));
        suffix += 1;
    }
    candidate
}

async fn run_shell_swap(
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    quiet: bool,
) -> std::result::Result<(), String> {
    if std::env::var_os("DEADRECKON_UPDATE_TEST_SHELL_FAIL").is_some() {
        return Err("test requested swap failure".to_string());
    }
    if let Ok(replacement) = std::env::var("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT") {
        fs::copy(replacement, &receipt.binary_path).map_err(|err| err.to_string())?;
        return Ok(());
    }
    run_axoupdater_shell_update(receipt, force, allow_prerelease, quiet).await
}

#[cfg(feature = "selfupdate")]
async fn run_axoupdater_shell_update(
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    quiet: bool,
) -> std::result::Result<(), String> {
    let mut updater = axoupdater::AxoUpdater::new_for("deadreckon");
    updater.set_release_source(axoupdater::ReleaseSource {
        release_type: axoupdater::ReleaseSourceType::GitHub,
        owner: "gdc".to_string(),
        name: "deadreckon".to_string(),
        app_name: "deadreckon".to_string(),
    });
    let version = update_current_version(receipt)
        .parse::<axoupdater::Version>()
        .map_err(|err| err.to_string())?;
    updater
        .set_current_version(version)
        .map_err(|err| err.to_string())?;
    if let Some(parent) = receipt.binary_path.parent() {
        updater.set_install_dir(parent.to_string_lossy().to_string());
    }
    if allow_prerelease {
        updater.configure_version_specifier(axoupdater::UpdateRequest::LatestMaybePrerelease);
    }
    if force {
        updater.always_update(true);
    }
    if quiet {
        updater.disable_installer_output();
    }
    updater.run().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(not(feature = "selfupdate"))]
async fn run_axoupdater_shell_update(
    _receipt: &deadreckon_core::install_receipt::Receipt,
    _force: bool,
    _allow_prerelease: bool,
    _quiet: bool,
) -> std::result::Result<(), String> {
    Err("selfupdate feature is disabled".to_string())
}

fn prune_shell_backups(paths: &DeadreckonPaths) -> Result<()> {
    let root = shell_backup_root(paths);
    let mut backups = fs::read_dir(&root)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.path().is_dir().then_some(entry.path())
        })
        .collect::<Vec<_>>();
    backups.sort();
    let remove_count = backups.len().saturating_sub(3);
    for backup in backups.into_iter().take(remove_count) {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}

fn shell_update_failure(
    receipt: &deadreckon_core::install_receipt::Receipt,
    backup_dir: &Path,
    source: &str,
) -> CliError {
    CliError::Exit {
        code: 2,
        message: format!(
            "update: swap failed; prior binary preserved: {source}; backup {}",
            backup_dir.display()
        ),
        hint: format!(
            "try: cp {} {}",
            backup_dir.join("deadreckon").display(),
            receipt.binary_path.display()
        ),
    }
}

fn update_receipt_for_current_binary(
    paths: &DeadreckonPaths,
    persist_detected: bool,
) -> Result<deadreckon_core::install_receipt::Receipt> {
    if let Some(receipt) = read_receipt(paths)? {
        return Ok(receipt);
    }
    let binary = std::env::current_exe()?;
    let receipt = detect_receipt(&binary);
    if persist_detected {
        write_receipt(paths, &receipt)?;
    }
    Ok(receipt)
}

fn update_current_version(receipt: &deadreckon_core::install_receipt::Receipt) -> String {
    if receipt.channel_version.trim().is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        receipt.channel_version.clone()
    }
}

fn print_update_check(channel: Channel, current: &str, latest: &LatestUpdate) {
    println!("channel: {}", channel.as_str());
    println!("current: {current}");
    println!("latest: {}", latest.version);
    println!("release: {}", latest.release_url);
    if latest.update_available && matches!(channel, Channel::Npm | Channel::Brew | Channel::Cargo) {
        println!("try: {}", channel_native_update_command(channel));
    } else if latest.update_available && channel == Channel::Shell {
        println!("try: deadreckon update");
    }
}

fn channel_native_update_command(channel: Channel) -> &'static str {
    match channel {
        Channel::Npm => "bun update -g deadreckon",
        Channel::Brew => "brew upgrade gdc/tap/deadreckon",
        Channel::Cargo => "cargo binstall --force deadreckon",
        Channel::Shell => "deadreckon update",
        Channel::Source => "cargo install --path crates/deadreckon",
    }
}

#[derive(Debug, Clone)]
struct LatestUpdate {
    version: String,
    release_url: String,
    archive_url: Option<String>,
    sha256: Option<String>,
    update_available: bool,
}

impl LatestUpdate {
    fn archive_url(&self) -> String {
        self.archive_url.clone().unwrap_or_else(|| {
            format!(
                "{}/download/deadreckon-installer.sh",
                self.release_url.trim_end_matches('/')
            )
        })
    }

    fn sha256(&self) -> &str {
        self.sha256.as_deref().unwrap_or("see release checksums")
    }
}

async fn resolve_latest_update(
    paths: &DeadreckonPaths,
    current: &str,
    allow_prerelease: bool,
) -> Result<LatestUpdate> {
    let now = Utc::now();
    let cache = read_cache(paths)?;
    if let Some(cache) = cache.as_ref()
        && !cache.is_stale(now)
    {
        return Ok(LatestUpdate {
            version: cache.latest_version.clone(),
            release_url: cache.release_url.clone(),
            archive_url: None,
            sha256: None,
            update_available: cache.update_available,
        });
    }

    match fetch_latest_update(allow_prerelease).await {
        Ok(mut latest) => {
            latest.update_available = version_is_newer(current, &latest.version);
            write_cache(
                paths,
                &deadreckon_core::update_cache::Cache {
                    checked_at: now,
                    latest_version: latest.version.clone(),
                    current_version: current.to_string(),
                    release_url: latest.release_url.clone(),
                    update_available: latest.update_available,
                },
            )?;
            Ok(latest)
        }
        Err(_) => Ok(cache.map_or_else(
            || LatestUpdate {
                version: current.to_string(),
                release_url: "https://github.com/gdc/deadreckon/releases".to_string(),
                archive_url: None,
                sha256: None,
                update_available: false,
            },
            |cache| LatestUpdate {
                version: cache.latest_version,
                release_url: cache.release_url,
                archive_url: None,
                sha256: None,
                update_available: cache.update_available,
            },
        )),
    }
}

async fn fetch_latest_update(allow_prerelease: bool) -> std::result::Result<LatestUpdate, String> {
    if let Ok(delay_ms) = std::env::var("DEADRECKON_UPDATE_TEST_FETCH_DELAY_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
    {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    if std::env::var_os("DEADRECKON_UPDATE_TEST_OFFLINE").is_some() {
        return Err("offline test mode".to_string());
    }
    if let Ok(version) = std::env::var("DEADRECKON_UPDATE_TEST_LATEST_VERSION") {
        let release_url =
            std::env::var("DEADRECKON_UPDATE_TEST_RELEASE_URL").unwrap_or_else(|_| {
                format!("https://github.com/gdc/deadreckon/releases/tag/v{version}")
            });
        return Ok(LatestUpdate {
            version,
            release_url,
            archive_url: std::env::var("DEADRECKON_UPDATE_TEST_ARCHIVE_URL").ok(),
            sha256: std::env::var("DEADRECKON_UPDATE_TEST_SHA256").ok(),
            update_available: false,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|err| err.to_string())?;
    if allow_prerelease {
        let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL")
            .unwrap_or_else(|_| "https://api.github.com/repos/gdc/deadreckon/releases".to_string());
        let releases = client
            .get(url)
            .header(reqwest::header::USER_AGENT, "deadreckon-update")
            .send()
            .await
            .map_err(|err| err.to_string())?
            .error_for_status()
            .map_err(|err| err.to_string())?
            .json::<Vec<GithubRelease>>()
            .await
            .map_err(|err| err.to_string())?;
        let Some(release) = releases.into_iter().next() else {
            return Err("no releases found".to_string());
        };
        return Ok(release.into_latest_update());
    }

    let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL").unwrap_or_else(|_| {
        "https://api.github.com/repos/gdc/deadreckon/releases/latest".to_string()
    });
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, "deadreckon-update")
        .send()
        .await
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?
        .json::<GithubRelease>()
        .await
        .map(GithubRelease::into_latest_update)
        .map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

impl GithubRelease {
    fn into_latest_update(self) -> LatestUpdate {
        LatestUpdate {
            version: self.tag_name.trim_start_matches('v').to_string(),
            release_url: self.html_url,
            archive_url: None,
            sha256: None,
            update_available: false,
        }
    }
}

fn version_is_newer(current: &str, latest: &str) -> bool {
    let current = current.trim_start_matches('v');
    let latest = latest.trim_start_matches('v');
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(current), Ok(latest)) => latest > current,
        _ => latest != current,
    }
}

async fn providers_list_command(
    models: bool,
    all: bool,
    full: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let active = read_config(&paths.config_path())?.default_provider;
    let ids = if all {
        registry.ids()
    } else {
        configured_provider_ids(&paths)?
    };
    if json_output {
        let mut results = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            if let Some(descriptor) = registry.get(&id) {
                results.push(descriptor.probe(ProviderProbeOptions { ping: false }).await);
            } else {
                missing.push(id);
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "providers",
                "id": if all { "all" } else { "configured" },
                "status": "ok",
                "next_actions": ["deadreckon detect"],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "providers": results,
                "missing_providers": missing,
                "active": active,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("provider registry"));
    if ids.is_empty() {
        println!("{} no configured providers", ui_muted("-"));
        println!(
            "{} {}",
            ui_command("try:"),
            ui_command("deadreckon providers list --all")
        );
        return Ok(());
    }
    for id in ids {
        let Some(descriptor) = registry.get(&id) else {
            let marker = if active.as_deref() == Some(id.as_str()) {
                "*"
            } else {
                " "
            };
            println!(
                "{marker} {} {} not registered | {}",
                ui_warn("✗"),
                ui_id(&id),
                ui_command("deadreckon detect")
            );
            continue;
        };
        let result = descriptor.probe(ProviderProbeOptions { ping: false }).await;
        print_provider_list_row(&result, descriptor, active.as_deref(), full);
        if models {
            print_provider_models(descriptor);
        }
    }
    if !all {
        println!(
            "{} {}",
            ui_muted("hint:"),
            ui_command("deadreckon providers list --all")
        );
    }
    Ok(())
}

fn configured_provider_ids(paths: &DeadreckonPaths) -> Result<Vec<String>> {
    let config = read_config(&paths.config_path())?;
    let mut ids = Vec::new();
    if let Some(default_provider) = config.default_provider {
        push_unique(&mut ids, default_provider);
    }
    if let Some(fallback) = config.fallback {
        for provider in fallback {
            push_unique(&mut ids, provider);
        }
    }
    for provider in config.providers.into_keys() {
        push_unique(&mut ids, provider);
    }
    Ok(ids)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn print_provider_list_row(
    result: &ProviderProbeResult,
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
    active: Option<&str>,
    full: bool,
) {
    let symbol = match result.status {
        ProbeStatus::Ok => ui_ok("✓"),
        ProbeStatus::Failed => ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗"),
        ProbeStatus::Skipped => ui_muted("-"),
    };
    let marker = if active == Some(result.id.as_str()) {
        "*"
    } else {
        " "
    };
    let location = result.location.as_deref().unwrap_or("-");
    let version = result.version.as_deref().unwrap_or("-");
    let model = descriptor.default_model.as_deref().unwrap_or("-");
    if full {
        println!(
            "{marker} {} {} kind={} credential={} model={} metering={} location={} version={}",
            symbol,
            ui_id(&result.id),
            descriptor_kind_label(&descriptor.kind),
            result.credential,
            model,
            result.metering,
            location,
            version
        );
    } else {
        println!(
            "{marker} {:<20} {}  kind={:<10} credential={:<8} model={} metering={} location={} version={}",
            ui_id(&result.id),
            symbol,
            descriptor_kind_label(&descriptor.kind),
            result.credential,
            model,
            result.metering,
            location,
            version
        );
    }
}

fn print_provider_models(descriptor: &deadreckon_providers::registry::ProviderDescriptor) {
    if descriptor.model_catalog.is_empty() {
        println!("    {}", ui_muted("models: none"));
        return;
    }
    println!("    {}", ui_muted("models:"));
    for model in &descriptor.model_catalog {
        let aliases = if model.aliases.is_empty() {
            "-".to_string()
        } else {
            model.aliases.join(",")
        };
        let context = model
            .context_window
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let price = match (model.input_per_million, model.output_per_million) {
            (Some(input), Some(output)) => format!("${input:.3}/${output:.3} per 1M"),
            _ => "-".to_string(),
        };
        println!(
            "      {} aliases={} context={} price={}",
            ui_id(&model.id),
            aliases,
            context,
            price
        );
    }
}

fn descriptor_kind_label(kind: &DescriptorKind) -> &'static str {
    match kind {
        DescriptorKind::Http => "http",
        DescriptorKind::Cli => "cli",
        DescriptorKind::LocalHttp => "local-http",
        DescriptorKind::Scripted => "scripted",
    }
}

fn print_detect_results(results: &[ProviderProbeResult]) {
    println!("{}", ui_heading("provider detection"));
    for result in results {
        let symbol = match result.status {
            ProbeStatus::Ok => ui_ok("✓"),
            ProbeStatus::Failed => ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗"),
            ProbeStatus::Skipped => ui_muted("-"),
        };
        let location = result.location.as_deref().unwrap_or("-");
        let version = result.version.as_deref().unwrap_or("-");
        let message = result.message.as_deref().unwrap_or("");
        println!(
            "{:<20} {}  kind={:<10} credential={:<14} location={:<36} version={:<18} metering={}",
            ui_id(&result.id),
            symbol,
            descriptor_kind_label(&result.kind),
            result.credential,
            location,
            version,
            result.metering
        );
        if !message.is_empty() {
            println!("    {}", ui_muted(message));
        }
        if result.status == ProbeStatus::Failed {
            for line in &result.try_lines {
                println!("    {} {}", ui_command("try:"), ui_command(line));
            }
        }
    }
}

fn print_chain_help(topic: Option<&str>) {
    let topic = topic.unwrap_or("overview");
    match topic {
        "plan" | "expand" => {
            println!("{}", ui_heading("deadreckon chain plan"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain plan \"large goal\" --n 4")
            );
            println!(
                "purpose: ask the configured provider to split a large goal into ordered steps"
            );
            println!("next:    {}", ui_command("deadreckon chain run latest"));
        }
        "run" | "resume" => {
            println!("{}", ui_heading("deadreckon chain run"));
            println!("usage: {}", ui_command("deadreckon chain run latest"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain resume latest --from-step 2")
            );
            println!("purpose: execute or continue the conductor for a chain");
            println!("next:    {}", ui_command("deadreckon chain attach latest"));
        }
        "attach" | "watch" => {
            println!("{}", ui_heading("deadreckon chain attach"));
            println!("usage: {}", ui_command("deadreckon chain attach latest"));
            println!(
                "purpose: open the chain TUI, including step timeline and live inner run status"
            );
            println!("next:    {}", ui_command("deadreckon chain status latest"));
        }
        "status" | "list" => {
            println!("{}", ui_heading("deadreckon chain status/list"));
            println!("usage: {}", ui_command("deadreckon chain status latest"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain list --all-scopes")
            );
            println!("purpose: find chains, summarize progress, and see the next action");
            println!("next:    {}", ui_command("deadreckon chain show latest"));
        }
        "show" => {
            println!("{}", ui_heading("deadreckon chain show"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain show latest --why-failed")
            );
            println!("purpose: inspect steps, policies, failures, applied SHAs, and run ids");
            println!("next:    {}", ui_command("deadreckon chain resume latest"));
        }
        "pause" | "kill" => {
            println!("{}", ui_heading("deadreckon chain pause/kill"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain pause latest --reason \"waiting on review\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain kill latest --escalate")
            );
            println!(
                "purpose: stop the conductor intentionally; kill also cascades to the live inner run"
            );
            println!("next:    {}", ui_command("deadreckon chain resume latest"));
        }
        "undo" | "redo" => {
            println!("{}", ui_heading("deadreckon chain undo/redo"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain undo latest --step 2")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain redo latest --step 2")
            );
            println!("purpose: back out or rerun an applied step with bounded chain state changes");
            println!("next:    {}", ui_command("deadreckon chain show latest"));
        }
        "extend" => {
            println!("{}", ui_heading("deadreckon chain extend"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\" --insert-at 2")
            );
            println!("purpose: add a new step to an existing chain");
            println!("next:    {}", ui_command("deadreckon chain run latest"));
        }
        "hooks" => {
            println!("{}", ui_heading("deadreckon chain hooks"));
            println!("usage: {}", ui_command("deadreckon chain hooks list"));
            println!("purpose: list lifecycle hook names supported by the conductor");
        }
        _ => {
            println!("{}", ui_heading("deadreckon chain"));
            println!("{CHAIN_HELP}");
            println!();
            println!("More help:");
            println!("  {}", ui_command("deadreckon chain help plan"));
            println!("  {}", ui_command("deadreckon chain help run"));
            println!("  {}", ui_command("deadreckon chain help pause"));
            println!("  {}", ui_command("deadreckon chain help undo"));
        }
    }
}

async fn chain_command(args: ChainCommandArgs) -> Result<()> {
    let ChainCommandArgs {
        args,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n,
        no_hints,
        quiet,
        plain,
        reason,
        from_step,
        max_spend_add,
        reset_breaker,
        force,
        step,
        extend,
        reapply,
        insert_at,
        no_confirm,
        full,
        all,
        why_failed,
        json,
    } = args;
    let paths = DeadreckonPaths::discover();
    if args.first().is_some_and(|arg| arg == "help") {
        print_chain_help(args.get(1).map(String::as_str));
        return Ok(());
    }
    let Some(first) = args.first().map(String::as_str) else {
        if from_file.is_some() || from_stdin {
            let goals = collect_chain_goals(&[], from_file, from_stdin)?;
            return chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ());
        }
        eprintln!("using: chain status (scope: {})", current_scope()?);
        return chain_status_command(None, all, full, plain, json);
    };

    match first {
        "plan" | "expand" => {
            let root_goal = args.get(1).cloned().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "chain plan needs a goal",
                    "deadreckon chain plan \"build the app\" --n 4",
                ))
            })?;
            chain_plan_command(ChainCreateOptions {
                paths,
                root_goal,
                goals: Vec::new(),
                from_file,
                from_stdin,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
        }
        "run" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            if id == "latest" || id == "last" {
                let latest = resolve_chain_id(&paths, id, all)?;
                eprintln!("using: chain resume {}", chain_prefix(&latest));
                return chain_run_command(
                    &paths,
                    &latest,
                    ChainRunOptions {
                        detach,
                        quiet,
                        plain,
                        from_step,
                        max_spend_add,
                        reset_breaker,
                        apply_mode: Some(apply_mode),
                        skip_acceptance_prompt: yes,
                    },
                )
                .await;
            }
            let id = resolve_chain_id(&paths, id, all)?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                    skip_acceptance_prompt: yes,
                },
            )
            .await
        }
        "resume" => {
            let id = resolve_chain_id(
                &paths,
                args.get(1).map(String::as_str).unwrap_or("latest"),
                all,
            )?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                    skip_acceptance_prompt: yes,
                },
            )
            .await
        }
        "status" => chain_status_command(args.get(1).map(String::as_str), all, full, plain, json),
        "list" => chain_list_command(all, full, json),
        "show" => chain_show_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            why_failed,
            json,
        ),
        "attach" => chain_attach_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            plain,
        ),
        "pause" => chain_pause_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            reason,
        ),
        "kill" => chain_kill_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            force,
        ),
        "undo" => chain_undo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            no_confirm,
        ),
        "extend" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            let step_goal = args.get(2).cloned().or(extend).ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "chain extend needs a step goal",
                    "deadreckon chain extend latest \"add tests\"",
                ))
            })?;
            chain_extend_command(&paths, id, step_goal, insert_at, max_spend_add)
        }
        "redo" => chain_redo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            extend,
            reapply,
        ),
        "hooks" if args.get(1).is_some_and(|arg| arg == "list") => chain_hooks_list_command(),
        maybe_id if args.len() == 1 && looks_like_chain_id(maybe_id) => {
            Err(CliError::Core(deadreckon_core::user_error(
                &format!("did you mean `chain run {maybe_id}`?"),
                &format!("deadreckon chain run {maybe_id}"),
            )))
        }
        _ => {
            let goals = collect_chain_goals(&args, from_file, from_stdin)?;
            chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ())
        }
    }
}

struct ChainCreateOptions {
    paths: DeadreckonPaths,
    root_goal: String,
    goals: Vec<String>,
    from_file: Option<PathBuf>,
    from_stdin: bool,
    draft: bool,
    yes: bool,
    detach: bool,
    branch_policy: String,
    apply_mode: String,
    apply_strategy: String,
    apply_allowlist: Vec<String>,
    on_fail: String,
    circuit_breaker_threshold: u32,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: String,
    base: Option<String>,
    n: u8,
    no_hints: bool,
    quiet: bool,
    plain: bool,
}

struct ChainRunOptions {
    detach: bool,
    quiet: bool,
    plain: bool,
    from_step: Option<u32>,
    max_spend_add: Option<f64>,
    reset_breaker: bool,
    apply_mode: Option<String>,
    skip_acceptance_prompt: bool,
}

async fn chain_plan_command(options: ChainCreateOptions) -> Result<()> {
    let n = options.n.clamp(2, 12);
    let paths = options.paths.clone();
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        options.provider.as_deref(),
        options.model.as_deref(),
    )?;
    let prompt = chain_planner_prompt(&options.root_goal, n);
    let response = with_cli_wait_status(
        "drafting chain plan",
        router.complete(&ProviderRequest {
            prompt,
            max_output_tokens: u32::from(n) * 96,
            cwd: Some(std::env::current_dir()?),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        }),
    )
    .await
    .map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("chain planner provider failed: {err}"),
            "deadreckon chain \"step one\" \"step two\"",
        ))
    })?;
    let goals = parse_planner_goals(&response.content, n)?;
    let chain_id = chain_create_command(ChainCreateOptions { goals, ..options }).await?;
    append_chain_planner_spend(&paths, &chain_id, &response)?;
    Ok(())
}

async fn chain_create_command(options: ChainCreateOptions) -> Result<String> {
    let ChainCreateOptions {
        paths,
        root_goal,
        mut goals,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n: _,
        no_hints,
        quiet,
        plain,
    } = options;
    if goals.is_empty() {
        goals = collect_chain_goals(&[], from_file, from_stdin)?;
    }
    deadreckon_core::validate_goal_count(goals.len()).map_err(CliError::from)?;
    let cwd = std::env::current_dir()?;
    let git_root = deadreckon_core::find_git_root(&cwd)?.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "chains require a git repo",
            "cd into a repo or initialize one with git init",
        ))
    })?;
    let scope = workspace_scope(&cwd).map_err(CliError::from)?;
    let base_ref = base.unwrap_or_else(|| "HEAD".to_string());
    let base_sha = git_stdout(&git_root, &["rev-parse", &base_ref])?;
    let base_branch = git_stdout(&git_root, &["symbolic-ref", "--short", "HEAD"])
        .unwrap_or_else(|_| base_ref.clone());
    let chain = Chain::new(ChainNewOptions {
        root_goal,
        goals,
        scope,
        base_branch,
        base_sha,
        cwd: git_root.clone(),
        provider,
        model,
        sandbox,
        branch_policy: parse_branch_policy(&branch_policy)?,
        apply_mode: parse_apply_mode(&apply_mode)?,
        apply_strategy: parse_apply_strategy(&apply_strategy)?,
        apply_allowlist,
        on_fail: parse_on_fail(&on_fail)?,
        circuit_breaker_threshold,
        max_spend_usd: max_spend,
        max_wall_seconds,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .map_err(CliError::from)?;
    save_chain(&paths, &chain)?;
    append_chain_event(
        &paths,
        &chain.chain_id,
        ChainEventKind::ChainCreated,
        None,
        json!({ "steps": chain.steps.len(), "draft": draft }),
    )?;
    if !quiet {
        println!("{}", chain_preview(&chain));
    }
    if draft {
        if completion_hints_enabled(no_hints) && !quiet {
            println!(
                "drafted: {} with {} steps",
                chain.chain_id,
                chain.steps.len()
            );
            println!(
                "edit:    vim {}",
                paths.chain_json(&chain.chain_id).display()
            );
            println!(
                "run:     deadreckon chain run {}",
                chain_prefix(&chain.chain_id)
            );
        }
        return Ok(chain.chain_id);
    }
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive chain start requires --yes",
                "deadreckon chain --yes \"step one\" \"step two\"",
            )));
        }
        if !prompt::confirm("start the chain?", true)? {
            println!("cancelled");
            return Ok(chain.chain_id);
        }
    }
    let chain_id = chain.chain_id.clone();
    let auto_attach = chain_should_auto_attach(io::stdout().is_terminal(), detach, quiet, plain);
    chain_run_command(
        &paths,
        &chain_id,
        ChainRunOptions {
            detach: detach || auto_attach,
            quiet,
            plain,
            from_step: None,
            max_spend_add: None,
            reset_breaker: false,
            apply_mode: None,
            skip_acceptance_prompt: yes,
        },
    )
    .await?;
    if auto_attach {
        chain_attach_command(&paths, &chain_id, false)?;
    }
    Ok(chain_id)
}

async fn chain_run_command(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let chain_id = resolve_chain_id(paths, chain_id, false)?;
    ensure_chain_acceptance_before_start(paths, &chain_id, &options).await?;
    if options.detach {
        return detach_chain_conductor(paths, &chain_id, &options);
    }
    run_chain_conductor(paths, &chain_id, options).await
}

async fn ensure_chain_acceptance_before_start(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    if options.skip_acceptance_prompt || options.quiet || !io::stdin().is_terminal() {
        return Ok(());
    }
    let chain = load_chain(paths, chain_id)?;
    let _ = ensure_acceptance_before_start(
        &chain.cwd,
        None,
        &chain.root_goal,
        chain.provider.clone(),
        chain.model.clone(),
        false,
        "chain",
    )
    .await?;
    Ok(())
}

async fn run_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let mut chain = load_chain(paths, chain_id)?;
    if let Some(add) = options.max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    if options.reset_breaker {
        chain.circuit_breaker_consecutive_failures = 0;
    }
    if let Some(mode) = options.apply_mode.as_deref()
        && mode != "auto"
    {
        chain.apply_mode = parse_apply_mode(mode)?;
    }
    match chain.status {
        ChainStatus::Completed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("chain '{}' is completed", chain_prefix(&chain.chain_id)),
                &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
            )));
        }
        ChainStatus::Running if chain.conductor_pid.is_some_and(pid_is_alive) => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "chain '{}' is already running (pid {})",
                    chain_prefix(&chain.chain_id),
                    chain.conductor_pid.unwrap_or_default()
                ),
                &format!("deadreckon chain attach {}", chain_prefix(&chain.chain_id)),
            )));
        }
        _ => {}
    }
    let mut lock = acquire_lock(
        paths,
        &chain.task_key(),
        &chain.chain_id,
        &chain.scope,
        "chain",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    chain.status = ChainStatus::Running;
    chain.started_at.get_or_insert_with(Utc::now);
    chain.paused_reason = None;
    chain.conductor_pid = Some(std::process::id());
    save_chain(paths, &chain)?;
    let conductor = ConductorState {
        schema_version: 1,
        chain_id: chain.chain_id.clone(),
        conductor_pid: std::process::id(),
        started_at: Utc::now(),
        live_step: None,
        live_run_id: None,
        live_child_pid: None,
    };
    fs::create_dir_all(paths.chain_dir(&chain.chain_id))?;
    fs::write(
        paths.conductor_json(&chain.chain_id),
        serde_json::to_vec_pretty(&conductor)?,
    )?;

    let start_index = options.from_step.unwrap_or(0);
    let mut completed = true;
    for index in 0..chain.steps.len() {
        if (index as u32) < start_index {
            continue;
        }
        if matches!(
            chain.steps[index].status,
            ChainStepStatus::Applied | ChainStepStatus::Skipped
        ) {
            continue;
        }
        lock.heartbeat(format!("step-{index}"))?;
        let step_cap = per_step_spend_cap(&chain, index);
        let step_wall_cap = per_step_wall_cap(&chain, index);
        let base_ref = chain_step_base_ref(&chain)?;
        match invoke_chain_hook(
            paths,
            &chain,
            "pre-step",
            Some(index as u32),
            json!({
                "chain_id": chain.chain_id,
                "step_index": index,
                "step_goal": chain.steps[index].goal,
                "base_ref": base_ref
            }),
        )? {
            1 => {
                chain.steps[index].status = ChainStepStatus::Skipped;
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainStepFailed,
                    Some(index as u32),
                    json!({ "reason": "skipped_by_pre_step_hook" }),
                )?;
                save_chain(paths, &chain)?;
                continue;
            }
            2 => {
                pause_chain_at_step(
                    paths,
                    &mut chain,
                    index,
                    "paused_by_pre_step_hook".to_string(),
                )?;
                completed = false;
                break;
            }
            _ => {}
        }
        let state = if chain.steps[index].status == ChainStepStatus::Completed {
            let run_id = chain.steps[index].run_id.clone().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    &format!("step {} is completed but has no run id", index + 1),
                    &format!(
                        "deadreckon chain redo {} --step {}",
                        chain_prefix(&chain.chain_id),
                        index + 1
                    ),
                ))
            })?;
            load_run(paths, &run_id)?
        } else {
            chain.steps[index].status = ChainStepStatus::Running;
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainStepStarted,
                Some(index as u32),
                json!({ "goal": chain.steps[index].goal, "base": base_ref, "max_spend": step_cap, "max_wall_seconds": step_wall_cap }),
            )?;
            save_chain(paths, &chain)?;
            let run_id = match run_chain_step(
                paths,
                &chain,
                index,
                &base_ref,
                step_cap,
                step_wall_cap,
                options.quiet,
            )
            .await
            {
                Ok(run_id) => run_id,
                Err(err) => {
                    completed =
                        handle_chain_step_failure(paths, &mut chain, index, err.to_string())?;
                    if !completed {
                        break;
                    }
                    continue;
                }
            };
            chain.steps[index].run_id = Some(run_id.clone());
            let state = load_run(paths, &run_id)?;
            chain.steps[index].spend_usd = state.total_spend_usd;
            chain.total_spend_usd += state.total_spend_usd;
            chain.total_wall_seconds += state.total_wall_seconds;
            write_chain_step_marker(
                &state.working_dir,
                &ChainStepMarker::new(
                    &chain,
                    &chain.steps[index],
                    latest_applied_sha_before(&chain, index),
                ),
            )?;
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainRunCompleted,
                Some(index as u32),
                json!({ "run_id": run_id, "status": state.status.to_string() }),
            )?;
            if state.status != RunStatus::Completed {
                completed = handle_chain_step_failure(
                    paths,
                    &mut chain,
                    index,
                    format!("inner run {} ended {}", state.run_id, state.status),
                )?;
                if !completed {
                    break;
                }
                continue;
            }
            match invoke_chain_hook(
                paths,
                &chain,
                "post-step",
                Some(index as u32),
                json!({
                    "chain_id": chain.chain_id,
                    "step_index": index,
                    "run_id": state.run_id,
                    "status": state.status.to_string(),
                    "library_dir": state.promoted_library_dir
                }),
            )? {
                1 => {
                    pause_chain_at_step(
                        paths,
                        &mut chain,
                        index,
                        "paused_by_post_step_hook".to_string(),
                    )?;
                    completed = false;
                    break;
                }
                2 => {
                    completed = handle_chain_step_failure(
                        paths,
                        &mut chain,
                        index,
                        "refused_by_post_step_hook".to_string(),
                    )?;
                    if !completed {
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            chain.steps[index].status = ChainStepStatus::Completed;
            save_chain(paths, &chain)?;
            state
        };
        match chain.apply_mode {
            ApplyMode::Auto => {
                if let Err(err) =
                    auto_apply_chain_step(paths, &mut chain, index, &state.run_id, options.quiet)
                {
                    pause_chain_at_step(
                        paths,
                        &mut chain,
                        index,
                        format!("apply_refused_{}", compact_reason(&err.to_string())),
                    )?;
                    completed = false;
                    break;
                }
            }
            ApplyMode::Preview => {
                let diff_summary = preview_diff_summary_for_run(&state).unwrap_or_default();
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainApplyRefused,
                    Some(index as u32),
                    json!({ "reason": "apply_mode_preview", "diff_summary": diff_summary }),
                )?;
                if !options.quiet {
                    println!("preview diff for step {}:", index + 1);
                    if diff_summary.trim().is_empty() {
                        println!("  no diff");
                    } else {
                        println!("{diff_summary}");
                    }
                }
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_preview".to_string())?;
                completed = false;
                break;
            }
            ApplyMode::Manual => {
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_manual".to_string())?;
                completed = false;
                break;
            }
        }
        if chain_spend_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "cap".to_string())?;
            completed = false;
            break;
        }
        if chain_wall_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "wall_clock_cap".to_string())?;
            completed = false;
            break;
        }
    }
    if completed {
        let hook_status = invoke_chain_hook(
            paths,
            &chain,
            "on-chain-end",
            None,
            json!({
                "chain_id": chain.chain_id,
                "status": "completed",
                "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(),
                "total_spend_usd": chain.total_spend_usd
            }),
        )
        .unwrap_or_default();
        chain.status = ChainStatus::Completed;
        chain.completed_at = Some(Utc::now());
        chain.conductor_pid = None;
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainCompleted,
            None,
            json!({ "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(), "total_spend_usd": chain.total_spend_usd, "on_chain_end_status": hook_status }),
        )?;
        save_chain(paths, &chain)?;
        if !options.quiet {
            println!(
                "chained: {} done {}/{}",
                chain_prefix(&chain.chain_id),
                chain.steps.len(),
                chain.steps.len()
            );
            println!(
                "show:    deadreckon chain show {}",
                chain_prefix(&chain.chain_id)
            );
            println!("list:    deadreckon chain list");
        }
    } else if !options.quiet {
        print_chain_paused_footer(&chain);
    }
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
    let _ = lock.release();
    Ok(())
}

async fn run_chain_step(
    paths: &DeadreckonPaths,
    chain: &Chain,
    index: usize,
    base_ref: &str,
    step_cap: Option<f64>,
    step_wall_cap: Option<f64>,
    quiet: bool,
) -> Result<String> {
    let step = &chain.steps[index];
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&chain.cwd)
        .env("DEADRECKON_HOME", paths.home())
        .arg("run")
        .arg(&step.goal)
        .arg("--worktree")
        .arg("--base")
        .arg(base_ref)
        .arg("--yes")
        .arg("--no-confirm")
        .arg("--no-hints")
        .arg("--sandbox")
        .arg(&chain.sandbox);
    if let Some(provider) = chain.provider.as_deref() {
        command.arg("--provider").arg(provider);
    }
    if let Some(model) = chain.model.as_deref() {
        command.arg("--model").arg(model);
    }
    if let Some(max_wall) = step_wall_cap {
        command.arg("--max-wall-seconds").arg(max_wall.to_string());
    }
    if let Some(step_cap) = step_cap {
        command.arg("--max-spend").arg(format!("{step_cap:.6}"));
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let child_pid = child.id();
    update_conductor_live(paths, chain, Some(index as u32), None, Some(child_pid))?;

    let stdout = child.stdout.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stdout".to_string(),
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stderr".to_string(),
        ))
    })?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let stdout_thread = spawn_chain_step_reader(stdout, true, tx.clone());
    let stderr_thread = spawn_chain_step_reader(stderr, false, tx);
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut live_run_id: Option<String> = None;
    let status = loop {
        while let Ok((is_stdout, line)) = rx.try_recv() {
            if let Some(run_id) = capture_chain_step_output(
                is_stdout,
                &line,
                &mut stdout_text,
                &mut stderr_text,
                quiet,
            )? && live_run_id.as_deref() != Some(run_id.as_str())
            {
                update_conductor_live(
                    paths,
                    chain,
                    Some(index as u32),
                    Some(run_id.clone()),
                    Some(child_pid),
                )?;
                live_run_id = Some(run_id);
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok((is_stdout, line)) = rx.try_recv() {
        if let Some(run_id) =
            capture_chain_step_output(is_stdout, &line, &mut stdout_text, &mut stderr_text, quiet)?
            && live_run_id.as_deref() != Some(run_id.as_str())
        {
            update_conductor_live(
                paths,
                chain,
                Some(index as u32),
                Some(run_id.clone()),
                Some(child_pid),
            )?;
            live_run_id = Some(run_id);
        }
    }
    update_conductor_live(paths, chain, None, None, None)?;
    if !status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "step {} run failed: {}{}",
            index + 1,
            stdout_text,
            stderr_text
        ))));
    }
    live_run_id
        .or_else(|| parse_started_run_id(&stdout_text))
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "could not find inner run id in run output\ntry: deadreckon list".to_string(),
            ))
        })
}

fn spawn_chain_step_reader<R: Read + Send + 'static>(
    reader: R,
    is_stdout: bool,
    tx: std::sync::mpsc::Sender<(bool, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(reader);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx.send((is_stdout, line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn capture_chain_step_output(
    is_stdout: bool,
    line: &str,
    stdout_text: &mut String,
    stderr_text: &mut String,
    quiet: bool,
) -> Result<Option<String>> {
    if is_stdout {
        stdout_text.push_str(line);
        if !quiet {
            print!("{line}");
            io::stdout().flush()?;
        }
        Ok(parse_started_run_id(stdout_text))
    } else {
        stderr_text.push_str(line);
        if !quiet {
            eprint!("{line}");
            io::stderr().flush()?;
        }
        Ok(None)
    }
}

fn auto_apply_chain_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    run_id: &str,
    quiet: bool,
) -> Result<()> {
    let state = load_run(paths, run_id)?;
    validate_acceptance_marker(&state)?;
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    if !git_stdout(git_root, &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("step '{}' refused auto-apply (dirty target)", index + 1),
            &format!(
                "git -C {} stash && deadreckon chain resume {}",
                git_root.display(),
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    if !chain.apply_allowlist.is_empty() {
        let branch = record.branch_name.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "missing branch_name".to_string(),
            ))
        })?;
        let files = git_stdout(
            git_root,
            &["diff", "--name-only", &format!("HEAD..{branch}")],
        )?;
        for file in files.lines().filter(|line| !line.trim().is_empty()) {
            if !chain
                .apply_allowlist
                .iter()
                .any(|pattern| allowlist_matches(pattern, file))
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "step '{}' refused auto-apply (outside_allowlist {file})",
                        index + 1
                    ),
                    &format!(
                        "deadreckon chain resume {} --apply-mode preview",
                        chain_prefix(&chain.chain_id)
                    ),
                )));
            }
        }
    }
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    let files_changed = git_stdout(
        git_root,
        &["diff", "--name-only", &format!("HEAD..{branch}")],
    )?
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    let diff_stat =
        git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")]).unwrap_or_default();
    match invoke_chain_hook(
        paths,
        chain,
        "on-promote",
        Some(index as u32),
        json!({
            "chain_id": chain.chain_id,
            "step_index": index,
            "run_id": run_id,
            "diff_stat": diff_stat,
            "files_changed": files_changed
        }),
    )? {
        1 => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' paused by hook on-promote", index + 1),
                &format!("deadreckon chain resume {}", chain_prefix(&chain.chain_id)),
            )));
        }
        2 => {
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainApplyRefused,
                Some(index as u32),
                json!({ "reason": "refused_by_hook_on_promote" }),
            )?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' refused by hook on-promote", index + 1),
                "inspect ~/.deadreckon/hooks/chain/on-promote",
            )));
        }
        _ => {}
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplyStarted,
        Some(index as u32),
        json!({ "run_id": run_id }),
    )?;
    if quiet {
        apply_command_quiet(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
        )?;
    } else {
        apply_command(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
            false,
        )?;
    }
    let applied_sha = git_stdout(git_root, &["rev-parse", "HEAD"])?;
    chain.steps[index].status = ChainStepStatus::Applied;
    chain.steps[index].applied_at = Some(Utc::now());
    chain.steps[index].applied_sha = Some(applied_sha.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplied,
        Some(index as u32),
        json!({ "run_id": run_id, "applied_sha": applied_sha }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

// SAFETY: Chain failure reasons are owned when they are persisted and emitted as JSON.
#[allow(clippy::needless_pass_by_value)]
fn handle_chain_step_failure(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<bool> {
    chain.steps[index].status = ChainStepStatus::Failed;
    chain.steps[index].fail_reason = Some(reason.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepFailed,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    match chain.on_fail {
        OnFail::Stop => {
            pause_chain_at_step(paths, chain, index, "step_failed".to_string())?;
            Ok(false)
        }
        OnFail::Skip => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            chain.circuit_breaker_consecutive_failures += 1;
            if chain.circuit_breaker_consecutive_failures >= chain.circuit_breaker_threshold {
                pause_chain_at_step(paths, chain, index, "circuit_breaker_open".to_string())?;
                return Ok(false);
            }
            save_chain(paths, chain)?;
            Ok(true)
        }
        OnFail::Continue => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            save_chain(paths, chain)?;
            Ok(true)
        }
    }
}

// SAFETY: Pause reasons are command-boundary values that are stored and emitted atomically.
#[allow(clippy::needless_pass_by_value)]
fn pause_chain_at_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<()> {
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.clone());
    chain.conductor_pid = None;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

fn detach_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    fs::create_dir_all(paths.chain_dir(chain_id))?;
    let stdout = fs::File::create(paths.chain_dir(chain_id).join("conductor.out"))?;
    let stderr = fs::File::create(paths.chain_dir(chain_id).join("conductor.err"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("chain")
        .arg("run")
        .arg(chain_id)
        .arg("--quiet")
        .env("DEADRECKON_HOME", paths.home())
        .stdout(stdout)
        .stderr(stderr)
        .stdin(std::process::Stdio::null());
    if options.plain {
        command.arg("--plain");
    }
    let child = command.spawn()?;
    println!(
        "chain {} detached (pid {})",
        chain_prefix(chain_id),
        child.id()
    );
    println!("attach: deadreckon chain attach {}", chain_prefix(chain_id));
    Ok(())
}

fn read_conductor_state(paths: &DeadreckonPaths, chain_id: &str) -> Result<Option<ConductorState>> {
    let path = paths.conductor_json(chain_id);
    match fs::read(&path) {
        Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn write_conductor_state(paths: &DeadreckonPaths, state: &ConductorState) -> Result<()> {
    let path = paths.conductor_json(&state.chain_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn update_conductor_live(
    paths: &DeadreckonPaths,
    chain: &Chain,
    live_step: Option<u32>,
    live_run_id: Option<String>,
    live_child_pid: Option<u32>,
) -> Result<()> {
    let mut conductor =
        read_conductor_state(paths, &chain.chain_id)?.unwrap_or_else(|| ConductorState {
            schema_version: 1,
            chain_id: chain.chain_id.clone(),
            conductor_pid: chain.conductor_pid.unwrap_or_else(std::process::id),
            started_at: chain.started_at.unwrap_or_else(Utc::now),
            live_step: None,
            live_run_id: None,
            live_child_pid: None,
        });
    conductor.live_step = live_step;
    conductor.live_run_id = live_run_id;
    conductor.live_child_pid = live_child_pid;
    write_conductor_state(paths, &conductor)
}

// SAFETY: Hook payloads are owned JSON messages written once to child process stdin.
#[allow(clippy::needless_pass_by_value)]
fn invoke_chain_hook(
    paths: &DeadreckonPaths,
    chain: &Chain,
    hook: &str,
    step_index: Option<u32>,
    payload: Value,
) -> Result<i32> {
    let Some(path) = resolve_chain_hook(paths, &chain.cwd, hook) else {
        return Ok(0);
    };
    let mut child = std::process::Command::new(&path)
        .env("DEADRECKON_CHAIN_ID", &chain.chain_id)
        .env("DEADRECKON_HOME", paths.home())
        .env(
            "DEADRECKON_STEP_INDEX",
            step_index
                .map(|index| index.to_string())
                .unwrap_or_else(|| "-".to_string()),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        serde_json::to_writer(&mut *stdin, &payload)?;
        stdin.write_all(b"\n")?;
    }
    let output = child.wait_with_output()?;
    let stdout = truncate_text(&String::from_utf8_lossy(&output.stdout), 4096);
    let stderr = truncate_text(&String::from_utf8_lossy(&output.stderr), 4096);
    let code = output.status.code().unwrap_or(1);
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainHookInvoked,
        step_index,
        json!({
            "hook": hook,
            "path": path,
            "status": code,
            "stdout": stdout,
            "stderr": stderr
        }),
    )?;
    Ok(code)
}

fn resolve_chain_hook(paths: &DeadreckonPaths, cwd: &Path, hook: &str) -> Option<PathBuf> {
    [
        cwd.join(".deadreckon/hooks/chain").join(hook),
        paths.home().join("hooks/chain").join(hook),
        PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn chain_status_command(
    id: Option<&str>,
    all: bool,
    full: bool,
    _plain: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if let Some(id) = id {
        return chain_show_command(&paths, id, false, json_output);
    }
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if json_output {
        print_chains_json(&chains)?;
        return Ok(());
    }
    if chains.is_empty() {
        println!("no chains in scope");
        println!("try: deadreckon chain \"step one\" \"step two\"");
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_list_command(all: bool, full: bool, json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if json_output {
        print_chains_json(&chains)?;
        return Ok(());
    }
    if chains.is_empty() {
        println!("no chains");
        println!("try: deadreckon chain \"step one\" \"step two\"");
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_show_command(
    paths: &DeadreckonPaths,
    id: &str,
    why_failed: bool,
    json_output: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "chain",
                "id": &chain.chain_id,
                "status": chain_status_label(&chain),
                "next_actions": [format!("deadreckon chain attach {}", chain_prefix(&chain.chain_id))],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "chain": paths.chain_json(&chain.chain_id),
                },
                "chain": chain,
            }))?
        );
        return Ok(());
    }
    if why_failed {
        show_chain_why_failed(&chain);
        return Ok(());
    }
    print_chain_header(paths, &chain);
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}{}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 72),
            step.run_id
                .as_deref()
                .map(|run_id| format!(" run={}", run_prefix(run_id)))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn print_chains_json(chains: &[Chain]) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "kind": "chain_list",
            "id": "chains",
            "status": "ok",
            "next_actions": ["deadreckon chain status latest"],
            "try_lines": Vec::<String>::new(),
            "paths": {
                "home": paths.home(),
            },
            "chains": chains,
        }))?
    );
    Ok(())
}

fn chain_progress(chain: &Chain) -> String {
    format!(
        "{}/{}",
        chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count(),
        chain.steps.len()
    )
}

fn chain_spend_label(chain: &Chain) -> String {
    format!(
        "${:.6} / {}",
        chain.total_spend_usd,
        chain
            .max_spend_usd
            .map(|value| format!("${value:.6}"))
            .unwrap_or_else(|| "uncapped".to_string())
    )
}

fn chain_policy_label(chain: &Chain) -> String {
    format!(
        "branch={} apply={} strategy={} on-fail={} base={}@{}",
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        apply_strategy_label(chain_apply_strategy(chain)),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha)
    )
}

fn chain_header_items(paths: &DeadreckonPaths, chain: &Chain) -> Vec<(&'static str, String)> {
    vec![
        (
            "chain",
            format!("{} ({})", chain_prefix(&chain.chain_id), chain.chain_id),
        ),
        ("status", chain_status_label(chain).to_string()),
        ("steps", chain_progress(chain)),
        ("spend", chain_spend_label(chain)),
        ("policy", chain_policy_label(chain)),
        ("cwd", chain.cwd.display().to_string()),
        (
            "path",
            paths.chain_json(&chain.chain_id).display().to_string(),
        ),
    ]
}

fn print_chain_header(paths: &DeadreckonPaths, chain: &Chain) {
    println!("{}", ui_heading("chain"));
    let items = chain_header_items(paths, chain);
    let _ = ui::kv_block(ui::Stream::Stdout, &items);
}

fn chain_attach_command(paths: &DeadreckonPaths, id: &str, plain: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    if io::stdout().is_terminal() && !plain {
        print_attach_banner("chain", &id);
        return chain_attach_tui(paths, &id);
    }
    print_chain_attach_snapshot(&chain);
    Ok(())
}

fn print_chain_attach_snapshot(chain: &Chain) {
    let paths = DeadreckonPaths::discover();
    print_chain_header(&paths, chain);
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 80)
        );
    }
    println!("[r] redo  [e] extend  [p] pause  [k] kill  [Ctrl-D] detach  [q] quit");
}

fn chain_attach_summary_line(chain: &Chain) -> String {
    let spend = chain_spend_label(chain).replace(" / ", "/");
    format!(
        "{}  status {}  steps {}  spend {}",
        chain_prefix(&chain.chain_id),
        chain_status_label(chain),
        chain_progress(chain),
        spend
    )
}

fn chain_attach_tui(paths: &DeadreckonPaths, chain_id: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = ChainAttachTuiState::default();

    let result = loop {
        let chain = load_chain(paths, chain_id)?;
        let events = read_jsonl::<ChainEvent>(&paths.chain_events(chain_id))?;
        tui_state.clamp(&chain);
        terminal.draw(|frame| render_chain_attach(frame, &chain, &events, &tui_state))?;

        if event::poll(std::time::Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key) => match key.code {
                    KeyCode::Enter => {
                        suspend_tui(&mut terminal)?;
                        if let Some(run_id) = chain
                            .steps
                            .get(tui_state.selected_step)
                            .and_then(|step| step.run_id.clone())
                        {
                            let _ = show_command(&run_id, None, false, false, false, false, None);
                        } else {
                            eprintln!("selected step has no run yet");
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('r') => {
                        suspend_tui(&mut terminal)?;
                        let action = chain_redo_command(
                            paths,
                            chain_id,
                            Some(tui_state.selected_step as u32 + 1),
                            None,
                            false,
                        );
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('e') => {
                        suspend_tui(&mut terminal)?;
                        let goal = prompt::open("new chain step goal: ", None)?;
                        if !goal.trim().is_empty() {
                            let action = chain_extend_command(paths, chain_id, goal, None, None);
                            if let Err(err) = &action {
                                print_error(err);
                            }
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('p') => {
                        suspend_tui(&mut terminal)?;
                        let action =
                            chain_pause_command(paths, chain_id, Some("user_paused".to_string()));
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('k') => {
                        suspend_tui(&mut terminal)?;
                        if prompt::confirm("kill chain?", false)?
                            && let Err(err) = chain_kill_command(paths, chain_id, false)
                        {
                            print_error(&err);
                        }
                        let _ = prompt::open("press Enter to return to chain attach...", None);
                        resume_tui(&mut terminal)?;
                    }
                    _ => tui_state.handle_key(key, &chain),
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => tui_state.scroll(1, &chain),
                    MouseEventKind::ScrollUp => tui_state.scroll(-1, &chain),
                    _ => {}
                },
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

#[derive(Debug, Default)]
struct ChainAttachTuiState {
    selected_step: usize,
    events_scroll: u16,
}

impl ChainAttachTuiState {
    fn clamp(&mut self, chain: &Chain) {
        if chain.steps.is_empty() {
            self.selected_step = 0;
            self.events_scroll = 0;
            return;
        }
        self.selected_step = self.selected_step.min(chain.steps.len() - 1);
    }

    fn handle_key(&mut self, key: KeyEvent, chain: &Chain) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1, chain),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.scroll(1, chain),
            KeyCode::PageUp => {
                self.events_scroll = self.events_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.events_scroll = self.events_scroll.saturating_add(8);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected_step = 0;
                self.events_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected_step = chain.steps.len().saturating_sub(1);
            }
            _ => {}
        }
        self.clamp(chain);
    }

    fn scroll(&mut self, delta: isize, chain: &Chain) {
        if chain.steps.is_empty() {
            return;
        }
        let next = (self.selected_step as isize + delta)
            .clamp(0, chain.steps.len().saturating_sub(1) as isize);
        self.selected_step = next as usize;
    }
}

fn render_chain_attach(
    frame: &mut ratatui::Frame<'_>,
    chain: &Chain,
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(rows[1]);

    frame.render_widget(
        Paragraph::new(chain_attach_header_text(chain)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("deadreckon chain"),
        ),
        rows[0],
    );
    let timeline = chain_timeline_lines(chain, tui_state)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(timeline).block(Block::default().borders(Borders::ALL).title("steps")),
        body[0],
    );
    let event_lines = chain_activity_lines(events, tui_state)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(event_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("chain activity"),
        ),
        body[1],
    );
    frame.render_widget(Paragraph::new(chain_attach_footer_text(chain)), rows[2]);
}

fn chain_attach_header_text(chain: &Chain) -> String {
    format!(
        "{}\npolicy branch={} apply={} strategy={} on-fail={}\nbase {}@{}  cwd {}",
        chain_attach_summary_line(chain),
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        apply_strategy_label(chain_apply_strategy(chain)),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha),
        one_line(&chain.cwd.display().to_string(), 96)
    )
}

fn chain_timeline_lines(chain: &Chain, tui_state: &ChainAttachTuiState) -> Vec<Line<'static>> {
    chain
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if index == tui_state.selected_step {
                ">"
            } else {
                " "
            };
            let run = step
                .run_id
                .as_deref()
                .map(|run_id| format!(" run {}", run_prefix(run_id)))
                .unwrap_or_default();
            let mut spans = vec![
                Span::styled(marker.to_string(), Style::default().fg(Color::Cyan)),
                Span::raw(format!(
                    " {} step {:>2} {:<8} {}{}",
                    chain_step_dot(step.status),
                    step.index + 1,
                    chain_step_status_label(step.status),
                    one_line(&step.goal, 54),
                    run
                )),
            ];
            if let Some(reason) = step.fail_reason.as_deref() {
                spans.push(Span::styled(
                    format!("  {}", one_line(reason, 32)),
                    Style::default().fg(Color::Red),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn chain_activity_lines(
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) -> Vec<Line<'static>> {
    let start = usize::from(tui_state.events_scroll).min(events.len());
    events
        .iter()
        .rev()
        .skip(start)
        .take(240)
        .map(|event| {
            let step = event
                .step_index
                .map(|index| format!(" step {}", index + 1))
                .unwrap_or_default();
            let detail = if event.detail.is_null() {
                String::new()
            } else {
                format!(" {}", one_line(&event.detail.to_string(), 120))
            };
            Line::from(format!(
                "{} {}{}{}",
                event.timestamp.format("%H:%M:%S"),
                chain_event_label(&event.event),
                step,
                detail
            ))
        })
        .collect()
}

fn chain_attach_footer_text(chain: &Chain) -> String {
    if chain.status == ChainStatus::Paused {
        format!(
            "paused: {} | try: show --why-failed | try: resume | try: resume --apply-mode preview | try: undo | q detach",
            chain.paused_reason.as_deref().unwrap_or("paused")
        )
    } else {
        "[Enter] drill  [r] redo  [e] extend  [p] pause  [k] kill  [Ctrl-D/q/Esc] detach  j/k move  PgUp/PgDn activity".to_string()
    }
}

fn chain_event_label(event: &ChainEventKind) -> &'static str {
    match event {
        ChainEventKind::ChainCreated => "created",
        ChainEventKind::ChainStepStarted => "step started",
        ChainEventKind::ChainRunCompleted => "run completed",
        ChainEventKind::ChainApplyStarted => "apply started",
        ChainEventKind::ChainApplied => "applied",
        ChainEventKind::ChainApplyRefused => "apply refused",
        ChainEventKind::ChainStepFailed => "step failed",
        ChainEventKind::ChainPaused => "paused",
        ChainEventKind::ChainResumed => "resumed",
        ChainEventKind::ChainKilled => "killed",
        ChainEventKind::ChainCompleted => "completed",
        ChainEventKind::ChainUndoStarted => "undo started",
        ChainEventKind::ChainUndoneStep => "undone step",
        ChainEventKind::ChainHookInvoked => "hook",
        ChainEventKind::ChainStepExtended => "extended",
        ChainEventKind::ChainStepRedone => "redone",
    }
}

fn chain_pause_command(paths: &DeadreckonPaths, id: &str, reason: Option<String>) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status != ChainStatus::Running {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("cannot pause '{}' chain", chain_status_label(&chain)),
            &format!("deadreckon chain status {}", chain_prefix(&chain.chain_id)),
        )));
    }
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.unwrap_or_else(|| "user_paused".to_string()));
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        None,
        json!({ "reason": chain.paused_reason }),
    )?;
    println!("paused {}", chain_prefix(&chain.chain_id));
    println!(
        "try: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_kill_command(paths: &DeadreckonPaths, id: &str, force: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let conductor = read_conductor_state(paths, &id)?;
    let mut signaled_pids = BTreeSet::new();
    if let Some(run_id) = conductor
        .as_ref()
        .and_then(|state| state.live_run_id.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Running)
                .and_then(|step| step.run_id.clone())
        })
        && let Ok(mut state) = load_run(paths, &run_id)
    {
        kill_loaded_run(paths, &mut state, force)?;
        signaled_pids.extend(supervised_pids(&state));
    }
    if let Some(pid) = conductor.as_ref().and_then(|state| state.live_child_pid) {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if let Some(pid) = conductor
        .as_ref()
        .map(|state| state.conductor_pid)
        .or(chain.conductor_pid)
        && pid != std::process::id()
    {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if !force {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while signaled_pids
            .iter()
            .any(|pid| *pid != std::process::id() && pid_is_alive(*pid))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for pid in &signaled_pids {
            if *pid != std::process::id() && pid_is_alive(*pid) {
                terminate_pid(*pid, true)?;
            }
        }
    }
    chain.status = ChainStatus::Killed;
    chain.failure_reason = Some("killed by user".to_string());
    chain.conductor_pid = None;
    for step in &mut chain.steps {
        if step.status == ChainStepStatus::Running {
            step.status = ChainStepStatus::Failed;
            step.fail_reason = Some("killed by user".to_string());
        }
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainKilled,
        None,
        json!({ "force": force }),
    )?;
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
    print_kill_banner("chain", &chain_prefix(&chain.chain_id), force, None);
    Ok(())
}

fn chain_undo_command(
    paths: &DeadreckonPaths,
    id: &str,
    through_step: Option<u32>,
    no_confirm: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let mut applied = chain
        .steps
        .iter()
        .filter(|step| step.status == ChainStepStatus::Applied)
        .filter(|step| through_step.is_none_or(|limit| step.index < limit))
        .filter_map(|step| {
            step.applied_sha
                .as_deref()
                .map(|sha| (step.index, sha.to_string()))
        })
        .collect::<Vec<_>>();
    if applied.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "nothing to undo",
            &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
        )));
    }
    if !no_confirm && io::stdin().is_terminal() {
        if !prompt::confirm("undo applied chain commits?", false)? {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive chain undo requires --no-confirm",
            &format!(
                "deadreckon chain undo {} --no-confirm",
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainUndoStarted,
        None,
        json!({ "count": applied.len() }),
    )?;
    applied.reverse();
    for (index, sha) in applied {
        git_status(&chain.cwd, &["revert", "--no-edit", &sha])?;
        if let Some(step) = chain.steps.iter_mut().find(|step| step.index == index) {
            step.status = ChainStepStatus::Undone;
        }
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainUndoneStep,
            Some(index),
            json!({ "sha": sha }),
        )?;
    }
    chain.status = ChainStatus::Undone;
    save_chain(paths, &chain)?;
    println!("undone {}", chain_prefix(&chain.chain_id));
    Ok(())
}

fn chain_extend_command(
    paths: &DeadreckonPaths,
    id: &str,
    step_goal: String,
    insert_at: Option<u32>,
    max_spend_add: Option<f64>,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status == ChainStatus::Completed && insert_at.is_none() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "cannot extend completed chain at end",
            &format!(
                "deadreckon chain extend {} \"...\" --insert-at <N>",
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    if let Some(add) = max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    let insert = insert_at
        .map(|value| value.saturating_sub(1) as usize)
        .unwrap_or(chain.steps.len())
        .min(chain.steps.len());
    chain.steps.insert(
        insert,
        deadreckon_core::ChainStep::new(insert as u32, step_goal),
    );
    for (index, step) in chain.steps.iter_mut().enumerate() {
        step.index = index as u32;
    }
    if chain.status == ChainStatus::Completed {
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("extended".to_string());
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepExtended,
        Some(insert as u32),
        json!({ "insert_at": insert }),
    )?;
    println!("extended {}", chain_prefix(&chain.chain_id));
    println!(
        "next: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_redo_command(
    paths: &DeadreckonPaths,
    id: &str,
    step: Option<u32>,
    extend: Option<String>,
    reapply: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let index = step
        .map(|step| step.saturating_sub(1))
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Failed)
                .map(|step| step.index)
        })
        .or_else(|| {
            chain
                .steps
                .iter()
                .rev()
                .find(|step| step.status == ChainStepStatus::Applied)
                .map(|step| step.index)
        })
        .ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "no failed or applied step to redo",
                &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
            ))
        })? as usize;
    let step = chain.steps.get_mut(index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("step {} does not exist", index + 1),
            &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
        ))
    })?;
    if step.status == ChainStepStatus::Applied && !reapply {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("step '{}' already applied; redo needs --reapply", index + 1),
            &format!(
                "deadreckon chain redo {} --step {} --reapply",
                chain_prefix(&chain.chain_id),
                index + 1
            ),
        )));
    }
    if reapply && let Some(sha) = step.applied_sha.as_deref() {
        git_status(&chain.cwd, &["revert", "--no-edit", sha])?;
    }
    let prior_goal = step.goal.clone();
    if let Some(extend) = extend {
        step.goal = extend;
    }
    step.status = ChainStepStatus::Pending;
    step.run_id = None;
    step.applied_at = None;
    step.applied_sha = None;
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepRedone,
        Some(index as u32),
        json!({ "prior_goal": prior_goal, "new_goal": chain.steps[index].goal }),
    )?;
    println!("redo queued {}", chain_prefix(&chain.chain_id));
    println!(
        "next: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_hooks_list_command() -> Result<()> {
    let paths = DeadreckonPaths::discover();
    for hook in ["pre-step", "post-step", "on-promote", "on-chain-end"] {
        let project = std::env::current_dir()?
            .join(".deadreckon/hooks/chain")
            .join(hook);
        let user = paths.home().join("hooks/chain").join(hook);
        let repo = PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook);
        let (tier, path) = if project.exists() {
            ("project", project)
        } else if user.exists() {
            ("user", user)
        } else if repo.exists() {
            ("repo", repo)
        } else {
            ("missing", user)
        };
        println!("{hook}\t{tier}\t{}", path.display());
    }
    Ok(())
}

fn collect_chain_goals(
    args: &[String],
    from_file: Option<PathBuf>,
    from_stdin: bool,
) -> Result<Vec<String>> {
    let mut goals = Vec::new();
    goals.extend(args.iter().cloned());
    if let Some(path) = from_file {
        goals.extend(parse_goal_lines(&fs::read_to_string(&path).map_err(
            |source| {
                CliError::Core(DeadreckonError::Io {
                    path: path.clone(),
                    source,
                })
            },
        )?));
    }
    if from_stdin {
        if io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "--from-stdin needs a pipe",
                "printf 'g1\\ng2\\n' | deadreckon chain --from-stdin --yes",
            )));
        }
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        goals.extend(parse_goal_lines(&raw));
    }
    Ok(goals
        .into_iter()
        .map(|goal| goal.trim().to_string())
        .filter(|goal| !goal.is_empty())
        .collect())
}

fn parse_goal_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect()
}

fn chain_planner_prompt(goal: &str, n: u8) -> String {
    format!(
        "You are decomposing a coding goal into an ordered serial chain.\n\
Output a JSON array of <= {n} strings, each <= 160 chars, each a concrete next step. \
Each step builds on the previous step's result. No prose, no commentary. Goal: {goal:?}."
    )
}

fn parse_planner_goals(raw: &str, n: u8) -> Result<Vec<String>> {
    let raw = raw.trim();
    let json_text = if raw.starts_with("```") {
        raw.lines()
            .filter(|line| !line.trim_start().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        raw.to_string()
    };
    let value = serde_json::from_str::<Value>(json_text.trim()).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("chain plan returned invalid JSON: {err}"),
            "deadreckon chain \"step one\" \"step two\"",
        ))
    })?;
    let array = value.as_array().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "chain plan must return a JSON array of strings",
            "deadreckon chain \"step one\" \"step two\"",
        ))
    })?;
    let mut seen = BTreeSet::new();
    let mut goals = Vec::new();
    for item in array.iter().take(usize::from(n)) {
        let Some(goal) = item.as_str().map(str::trim).filter(|goal| !goal.is_empty()) else {
            continue;
        };
        if goal.chars().count() > 160 {
            return Err(CliError::Core(deadreckon_core::user_error(
                "chain plan produced a step longer than 160 chars",
                "ask for fewer steps or use explicit `deadreckon chain \"...\" \"...\"`",
            )));
        }
        let key = goal
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !seen.insert(key) {
            return Err(CliError::Core(deadreckon_core::user_error(
                "chain plan produced duplicate steps",
                "rerun with --n 3 or provide explicit steps",
            )));
        }
        goals.push(goal.to_string());
    }
    deadreckon_core::validate_goal_count(goals.len()).map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("decomposition produced {} goals; need >= 2", goals.len()),
            "deadreckon chain plan \"goal\" --n 3",
        ))
    })?;
    Ok(goals)
}

fn append_chain_planner_spend(
    paths: &DeadreckonPaths,
    chain_id: &str,
    response: &deadreckon_providers::ProviderResponse,
) -> Result<()> {
    let path = paths.chain_dir(chain_id).join("spend.jsonl");
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    serde_json::to_writer(
        &mut file,
        &json!({
            "timestamp": Utc::now(),
            "kind": "chain.planner",
            "provider": response.provider,
            "model": response.model,
            "input_tokens": response.spend.input_tokens,
            "output_tokens": response.spend.output_tokens,
            "cost_usd": response.spend.cost_usd,
        }),
    )?;
    file.write_all(b"\n")?;
    Ok(())
}

fn chain_preview(chain: &Chain) -> String {
    let cap = chain
        .max_spend_usd
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "uncapped".to_string());
    let mut lines = vec![
        format!("chain preview {}", chain_prefix(&chain.chain_id)),
        format!("scope {}", chain.scope),
        format!("cwd {}", chain.cwd.display()),
        format!("base {}@{}", chain.base_branch, short_sha(&chain.base_sha)),
        format!(
            "policy branch={} apply={} strategy={} on-fail={}",
            branch_policy_label(chain.branch_policy),
            apply_mode_label(chain.apply_mode),
            apply_strategy_label(chain.apply_strategy),
            on_fail_label(chain.on_fail)
        ),
        format!(
            "provider {} model {} sandbox {} max-spend {}",
            chain.provider.as_deref().unwrap_or("default"),
            chain.model.as_deref().unwrap_or("default"),
            chain.sandbox,
            cap
        ),
        "steps".to_string(),
    ];
    for step in &chain.steps {
        lines.push(format!("  {}. {}", step.index + 1, step.goal));
    }
    lines.join("\n")
}

fn print_chain_table(chains: &[Chain], full: bool) {
    println!("CHAIN     STATUS     STEPS  SPEND       UPDATED                  GOAL");
    for chain in chains {
        let id = if full {
            chain.chain_id.clone()
        } else {
            chain_prefix(&chain.chain_id)
        };
        let done = chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count();
        let updated = chain
            .completed_at
            .or(chain.started_at)
            .unwrap_or(chain.created_at);
        println!(
            "{:<9} {:<10} {:>2}/{:<2} ${:<9.6} {:<24} {}",
            id,
            chain_status_label(chain),
            done,
            chain.steps.len(),
            chain.total_spend_usd,
            updated,
            truncate_text(&chain.root_goal, 80)
        );
    }
}

// SAFETY: Chain list filters are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn list_chain_records(paths: &DeadreckonPaths, scope: Option<String>) -> Result<Vec<Chain>> {
    if !paths.chains_dir().exists() {
        return Ok(Vec::new());
    }
    let mut chains = Vec::new();
    for entry in fs::read_dir(paths.chains_dir())? {
        let entry = entry?;
        let path = entry.path().join("chain.json");
        if !path.exists() {
            continue;
        }
        let chain = serde_json::from_slice::<Chain>(&fs::read(&path)?)
            .map_err(|source| DeadreckonError::Json { path, source })?;
        if scope.as_deref().is_some_and(|scope| chain.scope != scope) {
            continue;
        }
        chains.push(chain);
    }
    chains.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(chains)
}

fn resolve_chain_id(paths: &DeadreckonPaths, id: &str, all: bool) -> Result<String> {
    let scope = if all { None } else { Some(current_scope()?) };
    let chains = list_chain_records(paths, scope)?;
    if matches!(id, "latest" | "last") {
        return chains
            .first()
            .map(|chain| chain.chain_id.clone())
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "no chains in scope",
                    "deadreckon chain \"step one\" \"step two\"",
                ))
            });
    }
    let matches = chains
        .iter()
        .filter(|chain| chain.chain_id.starts_with(id))
        .map(|chain| chain.chain_id.clone())
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(CliError::Core(deadreckon_core::user_error(
            &format!("no chain '{id}'"),
            "deadreckon chain list",
        ))),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous chain id prefix {id}; matches {}",
                matches.join(", ")
            ),
            "deadreckon chain list --full",
        ))),
    }
}

fn parse_branch_policy(value: &str) -> Result<BranchPolicy> {
    match value {
        "stack" => Ok(BranchPolicy::Stack),
        "base" => Ok(BranchPolicy::Base),
        "linear-merge" | "merge" => Ok(BranchPolicy::Merge),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown branch policy {other}"),
            "use --branch-policy stack|base|linear-merge",
        ))),
    }
}

fn parse_apply_mode(value: &str) -> Result<ApplyMode> {
    match value {
        "auto" => Ok(ApplyMode::Auto),
        "preview" => Ok(ApplyMode::Preview),
        "manual" => Ok(ApplyMode::Manual),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown apply mode {other}"),
            "use --apply-mode auto|preview|manual",
        ))),
    }
}

fn parse_apply_strategy(value: &str) -> Result<ApplyStrategy> {
    match value {
        "squash" => Ok(ApplyStrategy::Squash),
        "merge" => Ok(ApplyStrategy::Merge),
        "cherry-pick" => Ok(ApplyStrategy::CherryPick),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown chain git apply strategy {other}"),
            "use --apply-strategy squash|merge|cherry-pick",
        ))),
    }
}

fn parse_on_fail(value: &str) -> Result<OnFail> {
    match value {
        "stop" => Ok(OnFail::Stop),
        "skip" => Ok(OnFail::Skip),
        "continue" => Ok(OnFail::Continue),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown on-fail policy {other}"),
            "use --on-fail stop|skip|continue",
        ))),
    }
}

fn chain_step_base_ref(chain: &Chain) -> Result<String> {
    match chain.branch_policy {
        BranchPolicy::Base => Ok(chain.base_sha.clone()),
        BranchPolicy::Stack | BranchPolicy::Merge => git_stdout(&chain.cwd, &["rev-parse", "HEAD"]),
    }
}

fn chain_should_auto_attach(
    stdout_is_terminal: bool,
    detach: bool,
    quiet: bool,
    plain: bool,
) -> bool {
    stdout_is_terminal && !detach && !quiet && !plain
}

fn per_step_spend_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_spend_usd?;
    let remaining = (max - chain.total_spend_usd).max(0.0);
    let pending = chain
        .steps
        .iter()
        .skip(index)
        .filter(|step| {
            !matches!(
                step.status,
                ChainStepStatus::Applied | ChainStepStatus::Skipped
            )
        })
        .count()
        .max(1);
    Some(remaining / pending as f64)
}

fn per_step_wall_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_wall_seconds?;
    let remaining = (max - chain.total_wall_seconds).max(0.0);
    let pending = chain
        .steps
        .iter()
        .skip(index)
        .filter(|step| {
            !matches!(
                step.status,
                ChainStepStatus::Applied | ChainStepStatus::Skipped
            )
        })
        .count()
        .max(1);
    Some(remaining / pending as f64)
}

fn chain_spend_cap_hit(chain: &Chain) -> bool {
    chain
        .max_spend_usd
        .is_some_and(|max| chain.total_spend_usd >= max)
}

fn chain_wall_cap_hit(chain: &Chain) -> bool {
    chain
        .max_wall_seconds
        .is_some_and(|max| chain.total_wall_seconds >= max)
}

fn preview_diff_summary_for_run(state: &deadreckon_core::PipelineState) -> Result<String> {
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")])
}

fn latest_applied_sha_before(chain: &Chain, index: usize) -> Option<String> {
    chain
        .steps
        .iter()
        .take(index)
        .rev()
        .find_map(|step| step.applied_sha.clone())
}

fn parse_started_run_id(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(start) = line.find('(')
            && let Some(end) = line[start + 1..].find(')')
        {
            let value = &line[start + 1..start + 1 + end];
            if value.len() >= 16 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn allowlist_matches(pattern: &str, file: &str) -> bool {
    pattern == "*"
        || pattern == file
        || file.starts_with(pattern.trim_end_matches('*'))
        || file.starts_with(pattern.trim_end_matches('/'))
}

fn looks_like_chain_id(value: &str) -> bool {
    value.len() >= 6 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn compact_reason(reason: &str) -> String {
    reason
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(96)
        .collect()
}

fn chain_prefix(chain_id: &str) -> String {
    id_prefix(chain_id)
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn branch_policy_label(value: BranchPolicy) -> &'static str {
    match value {
        BranchPolicy::Stack => "stack",
        BranchPolicy::Base => "base",
        BranchPolicy::Merge => "linear-merge",
    }
}

fn apply_mode_label(value: ApplyMode) -> &'static str {
    match value {
        ApplyMode::Auto => "auto",
        ApplyMode::Preview => "preview",
        ApplyMode::Manual => "manual",
    }
}

fn apply_strategy_label(value: ApplyStrategy) -> &'static str {
    match value {
        ApplyStrategy::Squash => "squash",
        ApplyStrategy::Merge => "merge",
        ApplyStrategy::CherryPick => "cherry-pick",
    }
}

fn chain_apply_strategy(chain: &Chain) -> ApplyStrategy {
    if chain.branch_policy == BranchPolicy::Merge {
        ApplyStrategy::Merge
    } else {
        chain.apply_strategy
    }
}

fn on_fail_label(value: OnFail) -> &'static str {
    match value {
        OnFail::Stop => "stop",
        OnFail::Skip => "skip",
        OnFail::Continue => "continue",
    }
}

fn chain_status_label(chain: &Chain) -> &'static str {
    glossary_chain_status_label(chain.status)
}

fn chain_step_status_label(status: ChainStepStatus) -> &'static str {
    glossary_chain_step_status_label(status)
}

fn chain_step_dot(status: ChainStepStatus) -> &'static str {
    match status {
        ChainStepStatus::Pending => "○",
        ChainStepStatus::Running => "●",
        ChainStepStatus::Completed => "◐",
        ChainStepStatus::Failed => "✗",
        ChainStepStatus::Skipped => "↷",
        ChainStepStatus::Applied => "◉",
        ChainStepStatus::Undone => "↶",
    }
}

fn print_chain_paused_footer(chain: &Chain) {
    let reason = chain.paused_reason.as_deref().unwrap_or("paused");
    println!("chain {} paused ({reason})", chain_prefix(&chain.chain_id));
    println!(
        "  try: deadreckon chain show {} --why-failed",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain resume {} --apply-mode preview",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain undo {}",
        chain_prefix(&chain.chain_id)
    );
}

async fn run_command(args: RunCommandArgs) -> Result<()> {
    let RunCommandArgs {
        goal,
        fresh,
        worktree,
        from,
        in_place,
        base,
        branch,
        allow_dirty,
        init_git,
        yes,
        preview,
        brief,
        plain,
        prevent_sleep,
        quiet,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        model,
        doc_provider,
        acceptance,
        skill,
        smoke,
        i_know_its_a_lot,
        no_confirm,
        no_hints,
        no_docs,
        doc_skill,
    } = args;
    let auto_confirm = yes || no_confirm || quiet;
    let effective_no_hints = no_hints || quiet;
    if smoke && provider.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --provider".to_string(),
        )));
    }
    if smoke && model.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --model".to_string(),
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let plain = plain || defaults.plain.unwrap_or(false) || std::env::var_os("NO_COLOR").is_some();
    ui::set_plain_output(plain);
    let prevent_sleep_prefs =
        SleepPrefs::parse(prevent_sleep.as_deref(), defaults.prevent_sleep.as_deref())
            .map_err(|err| CliError::Core(DeadreckonError::InvalidInput(err)))?;
    if !preview
        && let Some(exit_code) =
            sleep::maybe_reexec_for_linux(prevent_sleep_prefs, io::stdin().is_terminal())?
    {
        std::process::exit(exit_code);
    }
    let primary_setup = if smoke {
        provider_setup_selection(
            &paths,
            setup::ProviderSetupRequest {
                role: setup::SetupProviderRoleRef::PrimaryRun,
                explicit_provider: Some("smoke"),
                explicit_model: model.as_deref(),
                config_default_provider: defaults.provider.as_deref(),
                config_doc_provider: defaults.doc_provider.as_deref(),
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )?
    } else {
        provider_setup_selection(
            &paths,
            setup::ProviderSetupRequest {
                role: setup::SetupProviderRoleRef::PrimaryRun,
                explicit_provider: provider.as_deref(),
                explicit_model: model.as_deref(),
                config_default_provider: defaults.provider.as_deref(),
                config_doc_provider: defaults.doc_provider.as_deref(),
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: true,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )?
    };
    let provider_override = provider_override_from_setup(&primary_setup);
    let router = if smoke {
        ProviderRouter::smoke()
    } else {
        ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            provider_override.as_deref(),
            model.as_deref(),
        )?
    };
    let selected_route = router.selected_route_info();
    let effective_provider = selected_route
        .as_ref()
        .map(|route| route.name.clone())
        .or(primary_setup.provider.clone());
    let effective_max_spend = max_spend.or(defaults.max_spend).or(Some(10.0));
    let effective_max_wall_seconds = max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .or(Some(3600.0));
    let effective_doc_skill = doc_skill
        .or(defaults.doc_skill.clone())
        .unwrap_or_else(|| "run-narrator".to_string());
    let doc_provider_setup = doc_provider_setup_selection(
        &paths,
        &defaults,
        doc_provider.as_deref(),
        effective_provider.as_deref(),
        false,
    )?;
    let mut doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup);
    let effective_no_docs = no_docs || (smoke && doc_provider.is_none());
    if effective_no_docs {
        doc_provider_selection = DocProviderSelection {
            provider: None,
            source: DocProviderSource::None,
        };
    }
    if max_spend.is_none() && !quiet {
        let cap = effective_max_spend.unwrap_or(10.0);
        println!(
            "using default --max-spend ${cap:.0} (override with --max-spend or in config defaults.max_spend)"
        );
    }
    confirm_spend_cap(effective_max_spend, i_know_its_a_lot, no_confirm)?;
    let cwd = std::env::current_dir()?;
    let acceptance_source = ensure_acceptance_before_start(
        &cwd,
        acceptance.as_deref(),
        &goal,
        provider.clone(),
        model.clone(),
        auto_confirm || preview,
        "run",
    )
    .await?;
    let acceptance_preview = done_criteria_selection(&acceptance_source)?;
    let sleep_preview = sleep::preview(prevent_sleep_prefs, io::stdin().is_terminal());
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    if init_git {
        init_git_repo(&cwd)?;
    }
    let run_id = Uuid::new_v4().simple().to_string();
    let mut mode_flags = ModeFlags {
        fresh,
        worktree,
        from,
        in_place,
        i_know_its_a_lot,
    };
    let explicit_mode =
        mode_flags.fresh || mode_flags.worktree || mode_flags.from.is_some() || mode_flags.in_place;
    if !explicit_mode
        && deadreckon_core::find_git_root(&cwd)?.is_none()
        && io::stdin().is_terminal()
    {
        match prompt_non_git_mode()? {
            NonGitChoice::Init => {
                init_git_repo(&cwd)?;
                mode_flags.worktree = true;
            }
            NonGitChoice::Copy => mode_flags.from = Some(cwd.clone()),
            NonGitChoice::Cancel => {
                println!("cancelled");
                return Ok(());
            }
        }
    }
    let resolved_mode = resolve_mode(&mode_flags, &cwd, io::stdin().is_terminal())?;
    let mut codebase = match &resolved_mode {
        ResolvedMode::Worktree { source_path, .. } => prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: run_id.clone(),
                task_key: deadreckon_core::paths::task_key(&goal),
                source_path: source_path.clone(),
                base_ref: base,
                branch_name: branch,
                allow_dirty,
            },
        )?,
        _ => record_for_resolved_mode(resolved_mode.clone()),
    };
    if codebase.mode == CodebaseMode::Fresh {
        codebase.source_path = None;
    }
    let preview_text = run_preview(&RunPreview {
        goal: &goal,
        cwd: &cwd,
        codebase: &codebase,
        provider: effective_provider.as_deref(),
        provider_source: primary_setup.source.as_str(),
        route: selected_route.as_ref(),
        sandbox: &backend.to_string(),
        doc_provider: doc_provider_selection.provider.as_deref(),
        doc_provider_source: doc_provider_selection.source.as_str(),
        max_spend: effective_max_spend,
        max_wall_seconds: effective_max_wall_seconds,
        acceptance: &acceptance_preview,
        sleep: &sleep_preview,
        brief,
        plain,
        run_id: &run_id,
    });
    if preview {
        eprintln!("{preview_text}");
        return Ok(());
    }
    if !quiet {
        eprintln!("{preview_text}");
    }
    if !auto_confirm {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive without --yes",
                "--yes, --quiet, or run interactively",
            )));
        }
        if !prompt::confirm("continue?", true)? {
            println!("cancelled");
            return Ok(());
        }
    }
    if codebase.mode == CodebaseMode::Worktree {
        create_worktree(&codebase)?;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal,
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: skill,
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase.clone()),
        },
    )?;
    if let Some(source_path) = codebase
        .source_path
        .as_ref()
        .filter(|_| codebase.mode == CodebaseMode::Copy)
    {
        copy_source_to_working(source_path, &state.working_dir)?;
        deadreckon_core::write_codebase_record(&state.working_dir, &codebase)?;
    }
    copy_acceptance_into_run(&state, &acceptance_source)?;
    let mut lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "run",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    state.child_pids = vec![std::process::id()];
    save_state(&state)?;

    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
        }
        return Ok(());
    }
    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
        }
        return Ok(());
    }
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    if run_cancelled_before_turn_loop(&paths, &mut state)? {
        lock.release()?;
        if !quiet {
            print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
        }
        return Ok(());
    }
    lock.heartbeat("turn-loop")?;
    let _sleep_handle = match sleep::arm(prevent_sleep_prefs, &state.working_dir)? {
        SleepPrevention::Active { handle } => Some(handle),
        SleepPrevention::Skipped { reason } => {
            if prevent_sleep_prefs == SleepPrefs::On && !quiet {
                eprintln!(
                    "sleep prevention skipped: {}",
                    sleep::skip_reason_label(reason)
                );
                if let Some(try_line) = sleep_try_line(reason) {
                    eprintln!("try: {try_line}");
                }
            }
            None
        }
        SleepPrevention::Reexeced { exit_code } => {
            std::process::exit(exit_code);
        }
    };
    if !quiet {
        print_run_started(
            &state,
            selected_route.as_ref(),
            primary_setup.source.as_str(),
            doc_provider_selection.provider.as_deref(),
            doc_provider_selection.source.as_str(),
        );
    }
    let wait_label = format!(
        "run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let run_id_for_plain = state.run_id.clone();
    let turn_loop = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: effective_provider.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider: doc_provider_selection.provider.clone(),
                doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                doc_subskills: effective_doc_subskills(&defaults),
                token_budget: defaults
                    .doc_polish_token_budget
                    .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET),
                budget_cap_usd: defaults.doc_polish_budget_cap_usd,
                doc_skill: effective_doc_skill,
                no_docs: effective_no_docs,
            },
        },
    );
    let outcome = if plain && !quiet {
        with_plain_run_wait_status(paths.clone(), run_id_for_plain, turn_loop).await?
    } else {
        maybe_with_cli_wait_status(!plain && !quiet, &wait_label, turn_loop).await?
    };
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    if !quiet {
        print_exit_summary_card(&state, &outcome, plain);
    }
    if completed && completion_hints_enabled(effective_no_hints) {
        complete_run_actions(&state, !auto_confirm).await?;
    }
    Ok(())
}

fn run_cancelled_before_turn_loop(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
) -> Result<bool> {
    if !cancel_marker_present(state) {
        return Ok(false);
    }
    if let Ok(latest) = load_run(paths, &state.run_id)
        && latest.status == RunStatus::Killed
    {
        *state = latest;
        return Ok(true);
    }
    state.status = RunStatus::Killed;
    state.failure_reason = Some("run cancelled before provider turn".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(state)?;
    emit_event(
        state,
        None,
        deadreckon_core::RunEventKind::RunCompleted {
            status: "killed".to_string(),
        },
    )?;
    Ok(true)
}

const PROJECT_ACCEPTANCE_DIR: &str = ".deadreckon";
const PROJECT_ACCEPTANCE_YAML: &str = "acceptance.yaml";
const PROJECT_ACCEPTANCE_MD: &str = "acceptance.md";
const PROJECT_ACCEPTANCE_HELPERS: &str = "acceptance";

#[derive(Clone, Debug)]
struct AcceptanceSource {
    path: PathBuf,
    source: setup::DoneCriteriaSource,
    companion_doc: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct AcceptanceDraft {
    yaml: String,
    markdown: String,
    files: BTreeMap<PathBuf, String>,
}

async fn acceptance_command(command: AcceptanceCommand) -> Result<()> {
    match command {
        AcceptanceCommand::Setup {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Add {
            request,
            provider,
            model,
            force,
        } => acceptance_add_command(request, provider, model, force).await,
        AcceptanceCommand::Init { preset, force } => acceptance_init_command(preset, force),
        AcceptanceCommand::Draft {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Refine {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Explain { spec } => acceptance_explain_command(spec),
        AcceptanceCommand::Check { spec, against } => acceptance_check_command(spec, against),
    }
}

async fn done_command(
    args: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
    spec: Option<PathBuf>,
    against: Option<PathBuf>,
) -> Result<()> {
    let Some(first) = args.first().map(String::as_str) else {
        return acceptance_explain_command(spec);
    };
    match first {
        "add" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done add needs a criterion",
                    "deadreckon def-done add \"users can save drawings\"",
                )));
            }
            acceptance_add_command(request, provider, model, force).await
        }
        "check" => acceptance_check_command(spec, against),
        "show" | "explain" => acceptance_explain_command(spec),
        "edit" | "refine" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done edit needs a requested change",
                    "deadreckon def-done edit \"also require the gallery to persist\"",
                )));
            }
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        "help" => {
            print_done_help();
            Ok(())
        }
        _ => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, args, provider, model, true).await
        }
    }
}

fn print_done_help() {
    println!("{}", ui_heading("deadreckon def-done"));
    println!("usage:");
    println!(
        "  {}",
        ui_command("deadreckon def-done \"builds, opens in a browser, and has no console errors\"")
    );
    println!(
        "  {}",
        ui_command("deadreckon def-done add \"users can save drawings\"")
    );
    println!("  {}", ui_command("deadreckon def-done check"));
    println!("  {}", ui_command("deadreckon def-done show"));
}

#[derive(Clone, Copy)]
enum AcceptanceAgentMode {
    Draft,
    Refine,
}

fn acceptance_init_command(preset: AcceptancePreset, force: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let preset = match preset {
        AcceptancePreset::Auto => detect_acceptance_preset(&cwd),
        other => other,
    };
    let draft = acceptance_template_for_preset(preset, &cwd);
    write_project_acceptance(&cwd, &draft, force, false)?;
    print_acceptance_written(&cwd, "template", acceptance_check_count(&draft.yaml)?);
    Ok(())
}

async fn acceptance_agent_command(
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

async fn acceptance_agent_command_in_dir(
    cwd: &Path,
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    let existing_yaml = read_optional_text(&yaml_path)?;
    let existing_md = read_optional_text(&md_path)?;
    if matches!(mode, AcceptanceAgentMode::Refine) && existing_yaml.is_none() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no project done criteria found",
            "deadreckon def-done \"what should count as done\"",
        )));
    }
    let request = acceptance_request_text(&request, mode)?;
    if !force && yaml_path.exists() && matches!(mode, AcceptanceAgentMode::Draft) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance.yaml already exists",
            "deadreckon def-done add \"one more criterion\" or rerun with --overwrite",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let selected_provider = provider.or(defaults.doc_provider).or(defaults.provider);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        selected_provider.as_deref(),
        model.as_deref(),
    )?;
    let route = router.selected_route_info();
    let prompt = acceptance_agent_prompt(
        mode,
        &request,
        cwd,
        existing_yaml.as_deref(),
        existing_md.as_deref(),
    )?;
    let response = with_cli_wait_status(
        match mode {
            AcceptanceAgentMode::Draft => "compiling done criteria",
            AcceptanceAgentMode::Refine => "refining done criteria",
        },
        router.complete(&ProviderRequest {
            prompt,
            max_output_tokens: 6_000,
            cwd: Some(cwd.to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        }),
    )
    .await
    .map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("done criteria provider failed: {err}"),
            "deadreckon def-done \"builds and passes tests\"",
        ))
    })?;
    let draft = parse_acceptance_agent_response(&response.content)?;
    acceptance_check_count(&draft.yaml)?;
    write_project_acceptance(cwd, &draft, true, true)?;
    let route_label = route
        .map(|route| format!("{} / {}", route.name, route.model))
        .unwrap_or_else(|| "configured provider".to_string());
    print_acceptance_written(
        cwd,
        &format!("agent draft via {route_label}"),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

async fn acceptance_add_command(
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let joined = request.join(" ");
    if let Some(pack) = AcceptancePack::from_request(&joined) {
        return acceptance_add_pack_command(&cwd, pack, force);
    }
    let mode = if project_acceptance_yaml(&cwd).exists() {
        AcceptanceAgentMode::Refine
    } else {
        AcceptanceAgentMode::Draft
    };
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

fn acceptance_add_pack_command(cwd: &Path, pack: AcceptancePack, force: bool) -> Result<()> {
    let mut draft = if project_acceptance_yaml(cwd).exists() {
        let yaml = fs::read_to_string(project_acceptance_yaml(cwd))?;
        let markdown = read_optional_text(&project_acceptance_md(cwd))?
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        }
    } else {
        AcceptanceDraft {
            yaml: "name: project acceptance\nchecks: []\n".to_string(),
            markdown: "# Done Criteria\n\n".to_string(),
            files: BTreeMap::new(),
        }
    };
    let pack_draft = acceptance_pack_draft(pack, cwd);
    draft.yaml = append_acceptance_checks(&draft.yaml, &pack_draft.yaml)?;
    if !draft.markdown.ends_with('\n') {
        draft.markdown.push('\n');
    }
    draft.markdown.push('\n');
    draft.markdown.push_str(pack_draft.markdown.trim());
    draft.markdown.push('\n');
    draft.files.extend(pack_draft.files);
    write_project_acceptance(cwd, &draft, force, true)?;
    print_acceptance_written(
        cwd,
        &format!("{} pack", pack.name()),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum AcceptancePack {
    Auto,
    Basic,
    Build,
    Test,
    Rust,
    Node,
    StaticSite,
    Browser,
    Playwright,
    Vite,
    NextJs,
    Python,
}

impl AcceptancePack {
    fn from_request(request: &str) -> Option<Self> {
        let normalized = request.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "auto" => Some(Self::Auto),
            "basic" => Some(Self::Basic),
            "build" => Some(Self::Build),
            "test" | "tests" => Some(Self::Test),
            "rust" | "cargo" => Some(Self::Rust),
            "node" | "npm" | "javascript" | "typescript" => Some(Self::Node),
            "static" | "static-site" | "static site" | "html" => Some(Self::StaticSite),
            "browser" | "smoke" | "browser-smoke" => Some(Self::Browser),
            "playwright" | "e2e" => Some(Self::Playwright),
            "vite" => Some(Self::Vite),
            "next" | "nextjs" | "next.js" => Some(Self::NextJs),
            "python" | "py" => Some(Self::Python),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Basic => "basic",
            Self::Build => "build",
            Self::Test => "test",
            Self::Rust => "rust",
            Self::Node => "node",
            Self::StaticSite => "static-site",
            Self::Browser => "browser",
            Self::Playwright => "playwright",
            Self::Vite => "vite",
            Self::NextJs => "nextjs",
            Self::Python => "python",
        }
    }
}

// SAFETY: Acceptance paths are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn acceptance_explain_command(spec: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    if let Some(path) = path {
        let raw = fs::read_to_string(&path)?;
        let count = acceptance_check_count(&raw)?;
        println!("{}", ui_heading("done criteria"));
        println!("  spec:   {}", path.display());
        println!("  checks: {count}");
        if path == project_acceptance_yaml(&cwd)
            && let Some(markdown) = read_optional_text(&project_acceptance_md(&cwd))?
        {
            println!();
            println!("{}", markdown.trim());
        }
        println!();
        print_acceptance_yaml_summary(&raw)?;
    } else {
        println!("{}", ui_heading("done criteria"));
        println!("  spec:   default dr-gate behavior");
        println!("  checks: working directory exists, or cargo test when Cargo.toml is present");
        println!();
        println!(
            "{}",
            ui_command("deadreckon def-done \"what should count as done\"")
        );
    }
    Ok(())
}

// SAFETY: Acceptance check paths are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn acceptance_check_command(spec: Option<PathBuf>, against: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let working_dir = against.unwrap_or(cwd.clone());
    let spec_path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    let temp_root = std::env::temp_dir().join(format!(
        "deadreckon-acceptance-check-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&temp_root)?;
    if let Some(spec_path) = spec_path.as_ref() {
        fs::copy(spec_path, acceptance_spec_path_for_run_root(&temp_root))?;
    }
    let result = evaluate_acceptance_checks(&temp_root, &working_dir);
    let _ = fs::remove_dir_all(&temp_root);
    match result {
        Ok(results) => {
            let failed_required = results
                .iter()
                .any(|result| result.must_pass && !result.passed);
            if failed_required {
                println!("{}", ui_status("done criteria failed"));
            } else {
                println!("{}", ui_ok("done criteria passed"));
            }
            println!("working {}", working_dir.display());
            if let Some(spec_path) = spec_path {
                println!("spec    {}", spec_path.display());
            } else {
                println!("spec    default dr-gate behavior");
            }
            print_acceptance_results(&results);
            if let Some(failed) = results
                .iter()
                .find(|result| result.must_pass && !result.passed)
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("required done criterion failed: {}", failed.detail),
                    "fix the project or run `deadreckon def-done edit \"tighten or correct the checks\"`",
                )));
            }
            Ok(())
        }
        Err(err) => Err(CliError::Core(deadreckon_core::user_error(
            &format!("done criteria check failed: {err}"),
            "fix the project or edit .deadreckon/acceptance.yaml, then rerun `deadreckon def-done check`",
        ))),
    }
}

fn print_acceptance_results(results: &[deadreckon_core::AcceptanceCheckResult]) {
    for result in results {
        let mark = if result.passed {
            ui_ok("✓")
        } else if result.must_pass {
            ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗")
        } else {
            ui_warn("!")
        };
        let requirement = if result.must_pass {
            "required"
        } else {
            "optional"
        };
        let elapsed = result
            .duration_ms
            .map(|ms| format!(" ({:.1}s)", ms as f64 / 1000.0))
            .unwrap_or_default();
        println!(
            "  {mark} {:<13} {:<8} {}{}",
            result.kind, requirement, result.detail, elapsed
        );
        if !result.passed {
            if let Some(command) = result.command.as_deref() {
                println!("      command: {}", ui_command(command));
            }
            if let Some(stderr) = result.stderr.as_deref() {
                println!("      stderr:  {}", one_line(stderr, 140));
            }
            if let Some(stdout) = result.stdout.as_deref() {
                println!("      stdout:  {}", one_line(stdout, 140));
            }
        }
    }
}

fn acceptance_request_text(request: &[String], mode: AcceptanceAgentMode) -> Result<String> {
    let joined = request.join(" ").trim().to_string();
    if !joined.is_empty() {
        return Ok(joined);
    }
    if io::stdin().is_terminal() {
        let prompt_text = match mode {
            AcceptanceAgentMode::Draft => "what should count as done? ",
            AcceptanceAgentMode::Refine => "how should done criteria change? ",
        };
        let answer = prompt::open(prompt_text, None)?;
        if !answer.trim().is_empty() {
            return Ok(answer.trim().to_string());
        }
    }
    match mode {
        AcceptanceAgentMode::Draft => Ok(
            "Draft practical acceptance criteria for this project and its likely build/test flow."
                .to_string(),
        ),
        AcceptanceAgentMode::Refine => Err(CliError::Core(deadreckon_core::user_error(
            "refine needs a requested change",
            "deadreckon def-done add \"also require tests for the gallery\"",
        ))),
    }
}

fn acceptance_agent_prompt(
    mode: AcceptanceAgentMode,
    request: &str,
    cwd: &Path,
    existing_yaml: Option<&str>,
    existing_md: Option<&str>,
) -> Result<String> {
    let mode_label = match mode {
        AcceptanceAgentMode::Draft => "draft",
        AcceptanceAgentMode::Refine => "refine",
    };
    let project = acceptance_project_summary(cwd)?;
    Ok(format!(
        "\
You are helping configure deadreckon acceptance criteria for an unattended coding run.
The user writes acceptance in plain English. Convert it into executable checks that dr-gate can run.

Return JSON only, with exactly these keys:
{{\"acceptance_yaml\":\"...\",\"acceptance_md\":\"...\",\"files\":{{}}}}

The YAML must be valid deadreckon acceptance.yaml. Use only these check kinds:
- file_exists with path
- content_match with path and pattern
- shell with command and optional cwd
- cargo_test

Use {{working_dir}} for paths inside the run. Prefer stable, automatable checks over subjective claims.
Do not include self-attestation checks, provider-output checks, or instructions that the agent can satisfy by writing a marker.
For Node projects, prefer `npm run build --if-present` and `npm test --if-present` shell checks.
For static apps, require the main HTML/CSS/JS files and one or two content_match checks for requested behavior.
If a criterion needs a helper script, include it in files under `.deadreckon/acceptance/` and call it from a shell check.
The acceptance_md must restate the user's criteria in readable English before listing the executable checks.
Keep the YAML concise and include at least one required check.

Mode: {mode_label}
User request: {request}

Project summary:
{project}

Existing acceptance.yaml:
{existing_yaml}

Existing acceptance.md:
{existing_md}
",
        existing_yaml = existing_yaml.unwrap_or("(none)"),
        existing_md = existing_md.unwrap_or("(none)")
    ))
}

fn acceptance_project_summary(cwd: &Path) -> Result<String> {
    let mut lines = vec![format!("root: {}", cwd.display())];
    if cwd.join("Cargo.toml").exists() {
        lines.push("stack: rust".to_string());
    }
    if cwd.join("package.json").exists() {
        lines.push("stack: node".to_string());
        if let Ok(package) = fs::read_to_string(cwd.join("package.json"))
            && let Ok(value) = serde_json::from_str::<Value>(&package)
            && let Some(scripts) = value.get("scripts").and_then(Value::as_object)
        {
            let mut script_names = scripts.keys().cloned().collect::<Vec<_>>();
            script_names.sort();
            lines.push(format!("package scripts: {}", script_names.join(", ")));
        }
    }
    let files = project_file_sample(cwd, 80)?;
    lines.push("files:".to_string());
    for file in files {
        lines.push(format!("  - {}", file.display()));
    }
    Ok(lines.join("\n"))
}

fn project_file_sample(cwd: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    fn walk(
        root: &Path,
        current: &Path,
        depth: usize,
        out: &mut Vec<PathBuf>,
        limit: usize,
    ) -> io::Result<()> {
        if depth > 3 || out.len() >= limit {
            return Ok(());
        }
        let mut entries = fs::read_dir(current)?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | "target" | "node_modules" | ".deadreckon" | "dist" | "build"
            ) {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, depth + 1, out, limit)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(cwd, cwd, 0, &mut files, limit)?;
    Ok(files)
}

fn parse_acceptance_agent_response(content: &str) -> Result<AcceptanceDraft> {
    let cleaned = strip_code_fence(content.trim());
    if let Ok(value) = serde_json::from_str::<Value>(&cleaned)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(json) = extract_json_object(&cleaned)
        && let Ok(value) = serde_json::from_str::<Value>(&json)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(yaml) = extract_fenced_block(&cleaned, &["yaml", "yml"]).or_else(|| {
        if cleaned.contains("checks:") {
            Some(cleaned.clone())
        } else {
            None
        }
    }) {
        let markdown = extract_fenced_block(&cleaned, &["markdown", "md"])
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        acceptance_check_count(&yaml)?;
        return Ok(AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        });
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "provider did not return acceptance JSON or YAML",
        "rerun `deadreckon def-done ...` or use `deadreckon def-done check` after editing criteria",
    )))
}

fn acceptance_json_payload(value: &Value) -> Result<Option<AcceptanceDraft>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let yaml = object
        .get("acceptance_yaml")
        .or_else(|| object.get("yaml"))
        .and_then(Value::as_str);
    let Some(yaml) = yaml else {
        return Ok(None);
    };
    let markdown = object
        .get("acceptance_md")
        .or_else(|| object.get("markdown"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| acceptance_markdown_from_yaml(yaml));
    acceptance_check_count(yaml)?;
    let mut files = BTreeMap::new();
    if let Some(file_map) = object.get("files").and_then(Value::as_object) {
        for (path, body) in file_map {
            let Some(body) = body.as_str() else {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("acceptance helper {path} must be a string"),
                    "return files as {\".deadreckon/acceptance/name\": \"contents\"}",
                )));
            };
            let path = PathBuf::from(path);
            validate_acceptance_helper_path(&path)?;
            files.insert(path, body.to_string());
        }
    }
    Ok(Some(AcceptanceDraft {
        yaml: yaml.to_string(),
        markdown,
        files,
    }))
}

fn strip_code_fence(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines().collect::<Vec<_>>();
    if lines.len() >= 2 && lines.last().is_some_and(|line| line.trim() == "```") {
        lines.remove(0);
        lines.pop();
        return lines.join("\n");
    }
    trimmed.to_string()
}

fn extract_json_object(value: &str) -> Option<String> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(value[start..=end].to_string())
}

fn extract_fenced_block(value: &str, languages: &[&str]) -> Option<String> {
    let mut in_block = false;
    let mut capture = false;
    let mut lines = Vec::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_block {
                if capture {
                    return Some(lines.join("\n"));
                }
                in_block = false;
                continue;
            }
            in_block = true;
            capture = languages
                .iter()
                .any(|language| rest.trim().eq_ignore_ascii_case(language));
            lines.clear();
            continue;
        }
        if in_block && capture {
            lines.push(line);
        }
    }
    None
}

async fn ensure_acceptance_before_start(
    cwd: &Path,
    override_path: Option<&Path>,
    goal: &str,
    provider: Option<String>,
    model: Option<String>,
    skip_prompt: bool,
    noun: &str,
) -> Result<Option<AcceptanceSource>> {
    let existing = resolve_acceptance_source(cwd, override_path)?;
    if existing.is_some() || override_path.is_some() || skip_prompt || !io::stdin().is_terminal() {
        return Ok(existing);
    }
    println!("{}", ui_heading("done criteria"));
    println!("No done criteria found.");
    println!("Write the definition of done in English; deadreckon will compile it for dr-gate.");
    if !prompt::confirm(&format!("write done criteria before this {noun}?"), true)? {
        println!("using default gate: working directory exists, or cargo test for Rust projects");
        return Ok(existing);
    }
    let request = prompt::open("definition of done (Enter for a practical default): ", None)?;
    let request = if request.trim().is_empty() {
        format!("For this {noun}, define practical acceptance checks for: {goal}")
    } else {
        request.trim().to_string()
    };
    match acceptance_agent_command_in_dir(
        cwd,
        AcceptanceAgentMode::Draft,
        vec![request],
        provider,
        model,
        false,
    )
    .await
    {
        Ok(()) => resolve_acceptance_source(cwd, None).map(mark_generated_done_criteria),
        Err(err) => {
            println!("{}", ui_status("done criteria draft failed"));
            println!("  {err}");
            if !prompt::confirm("use a detected local check template instead?", true)? {
                return Ok(existing);
            }
            let preset = detect_acceptance_preset(cwd);
            let draft = acceptance_template_for_preset(preset, cwd);
            write_project_acceptance(cwd, &draft, false, false)?;
            print_acceptance_written(
                cwd,
                "detected template",
                acceptance_check_count(&draft.yaml)?,
            );
            resolve_acceptance_source(cwd, None).map(mark_generated_done_criteria)
        }
    }
}

fn resolve_acceptance_source(
    cwd: &Path,
    override_path: Option<&Path>,
) -> Result<Option<AcceptanceSource>> {
    if let Some(path) = override_path {
        let path = absolute_from(cwd, path);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("done criteria file not found: {}", path.display()),
                "deadreckon def-done \"what should count as done\"",
            )));
        }
        return Ok(Some(AcceptanceSource {
            path,
            source: setup::DoneCriteriaSource::ExplicitPath,
            companion_doc: None,
        }));
    }
    let project_yaml = project_acceptance_yaml(cwd);
    if project_yaml.is_file() {
        let project_md = project_acceptance_md(cwd);
        return Ok(Some(AcceptanceSource {
            path: project_yaml,
            source: setup::DoneCriteriaSource::ProjectFile,
            companion_doc: project_md.is_file().then_some(project_md),
        }));
    }
    Ok(None)
}

fn mark_generated_done_criteria(source: Option<AcceptanceSource>) -> Option<AcceptanceSource> {
    source.map(|mut source| {
        source.source = setup::DoneCriteriaSource::Generated;
        source
    })
}

fn done_criteria_selection(
    source: &Option<AcceptanceSource>,
) -> Result<setup::DoneCriteriaSelection> {
    match source {
        Some(source) => {
            let raw = fs::read_to_string(&source.path)?;
            let checks = Some(acceptance_check_count(&raw)?);
            Ok(match source.source {
                setup::DoneCriteriaSource::ExplicitPath => setup::DoneCriteriaSelection::explicit(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::ProjectFile => setup::DoneCriteriaSelection::project(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::Generated => setup::DoneCriteriaSelection::generated(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::DefaultGate => {
                    setup::DoneCriteriaSelection::default_gate()
                }
            })
        }
        None => Ok(setup::DoneCriteriaSelection::default_gate()),
    }
}

fn copy_acceptance_into_run(
    state: &deadreckon_core::PipelineState,
    source: &Option<AcceptanceSource>,
) -> Result<()> {
    let Some(source) = source else {
        return Ok(());
    };
    fs::copy(
        &source.path,
        acceptance_spec_path_for_run_root(&state.run_root),
    )?;
    if let Some(doc) = source.companion_doc.as_ref() {
        fs::copy(doc, state.run_root.join(PROJECT_ACCEPTANCE_MD))?;
    }
    let Some(project_dir) = source.path.parent() else {
        return Ok(());
    };
    let helper_source = project_dir.join(PROJECT_ACCEPTANCE_HELPERS);
    if helper_source.is_dir() {
        let helper_dest = state
            .working_dir
            .join(PROJECT_ACCEPTANCE_DIR)
            .join(PROJECT_ACCEPTANCE_HELPERS);
        if !same_path_best_effort(&helper_source, &helper_dest) {
            copy_tree(&helper_source, &helper_dest)?;
        }
    }
    Ok(())
}

fn copy_existing_acceptance_into_run(
    state: &deadreckon_core::PipelineState,
    candidate_roots: &[&Path],
) -> Result<()> {
    let source = resolve_existing_acceptance_source(candidate_roots)?;
    copy_acceptance_into_run(state, &source)
}

fn resolve_existing_acceptance_source(
    candidate_roots: &[&Path],
) -> Result<Option<AcceptanceSource>> {
    for root in candidate_roots {
        if let Some(source) = resolve_acceptance_source(root, None)? {
            return Ok(Some(source));
        }
    }
    Ok(None)
}

fn same_path_best_effort(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn resolve_acceptance_path_for_command(cwd: &Path, spec: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(spec) = spec {
        let path = absolute_from(cwd, spec);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("done criteria file not found: {}", path.display()),
                "deadreckon def-done \"what should count as done\"",
            )));
        }
        return Ok(Some(path));
    }
    let project = project_acceptance_yaml(cwd);
    Ok(project.is_file().then_some(project))
}

fn absolute_from(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn project_acceptance_yaml(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR)
        .join(PROJECT_ACCEPTANCE_YAML)
}

fn project_acceptance_md(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_MD)
}

fn write_project_acceptance(
    cwd: &Path,
    draft: &AcceptanceDraft,
    force: bool,
    allow_existing: bool,
) -> Result<()> {
    let dir = cwd.join(PROJECT_ACCEPTANCE_DIR);
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    if !allow_existing && !force && (yaml_path.exists() || md_path.exists()) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance files already exist",
            "deadreckon def-done add \"one more criterion\" or rerun with --overwrite",
        )));
    }
    fs::create_dir_all(&dir)?;
    fs::write(&yaml_path, ensure_trailing_newline(&draft.yaml))?;
    fs::write(&md_path, ensure_trailing_newline(&draft.markdown))?;
    for (relative_path, body) in &draft.files {
        let path = validate_acceptance_helper_path(relative_path)?;
        let absolute = cwd.join(path);
        if !force && absolute.exists() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("acceptance helper already exists: {}", absolute.display()),
                "rerun with --overwrite or edit the helper manually",
            )));
        }
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(absolute, ensure_trailing_newline(body))?;
    }
    Ok(())
}

fn validate_acceptance_helper_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    let required_prefix = Path::new(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_HELPERS);
    if !path.starts_with(&required_prefix) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    Ok(path.to_path_buf())
}

fn print_acceptance_written(cwd: &Path, source: &str, checks: usize) {
    println!("{}", ui_ok("done criteria configured"));
    println!("source  {source}");
    println!("checks  {checks}");
    println!("yaml    {}", project_acceptance_yaml(cwd).display());
    println!("notes   {}", project_acceptance_md(cwd).display());
    println!();
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("deadreckon def-done check")
    );
    println!(
        "{} {}",
        ui_command("run: "),
        ui_command("deadreckon run \"goal\"")
    );
}

fn detect_acceptance_preset(cwd: &Path) -> AcceptancePreset {
    if cwd.join("Cargo.toml").exists() {
        AcceptancePreset::Rust
    } else if cwd.join("package.json").exists() {
        AcceptancePreset::Node
    } else if cwd.join("index.html").exists() || cwd.join("public/index.html").exists() {
        AcceptancePreset::StaticSite
    } else {
        AcceptancePreset::Basic
    }
}

fn acceptance_template_for_preset(preset: AcceptancePreset, cwd: &Path) -> AcceptanceDraft {
    let yaml = match preset {
        AcceptancePreset::Auto => unreachable!("auto is resolved before template generation"),
        AcceptancePreset::Rust => {
            "\
name: rust project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/Cargo.toml\"
  - kind: cargo_test
"
        }
        AcceptancePreset::Node => {
            "\
name: node project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/package.json\"
  - kind: shell
    command: \"npm run build --if-present\"
    cwd: \"{working_dir}\"
  - kind: shell
    command: \"npm test --if-present\"
    cwd: \"{working_dir}\"
    must_pass: false
"
        }
        AcceptancePreset::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/public/index.html\"
  - kind: shell
    command: \"test -s public/index.html\"
    cwd: \"{working_dir}\"
"
            } else {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/index.html\"
  - kind: shell
    command: \"test -s index.html\"
    cwd: \"{working_dir}\"
"
            }
        }
        AcceptancePreset::Basic => {
            "\
name: basic project acceptance
checks:
  - kind: shell
    command: \"test -d .\"
    cwd: \"{working_dir}\"
"
        }
    }
    .to_string();
    let markdown = format!(
        "\
# Done Criteria

These checks define what `deadreckon` should verify before promoting a completed run.

{}
",
        acceptance_markdown_from_yaml(&yaml)
    );
    AcceptanceDraft {
        yaml,
        markdown,
        files: BTreeMap::new(),
    }
}

fn acceptance_pack_draft(pack: AcceptancePack, cwd: &Path) -> AcceptanceDraft {
    let pack = match pack {
        AcceptancePack::Auto => match detect_acceptance_preset(cwd) {
            AcceptancePreset::Rust => AcceptancePack::Rust,
            AcceptancePreset::Node => AcceptancePack::Node,
            AcceptancePreset::StaticSite => AcceptancePack::StaticSite,
            AcceptancePreset::Basic | AcceptancePreset::Auto => AcceptancePack::Basic,
        },
        other => other,
    };
    let yaml = match pack {
        AcceptancePack::Basic => {
            "name: basic acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Build => {
            if cwd.join("Cargo.toml").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: build_success\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("package.json").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Test => {
            if cwd.join("Cargo.toml").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: cargo_test\n"
            } else if cwd.join("package.json").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Rust => {
            "name: rust acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/Cargo.toml\"\n  - kind: cargo_test\n"
        }
        AcceptancePack::Node => {
            "name: node acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/public/index.html\"\n  - kind: shell\n    command: \"test -s public/index.html\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/index.html\"\n  - kind: shell\n    command: \"test -s index.html\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Browser => {
            "name: browser acceptance pack\nchecks:\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Playwright => {
            "name: playwright acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present && (npm run preview --if-present -- --host 127.0.0.1 > .deadreckon/acceptance/preview.log 2>&1 & pid=$!; trap 'kill $pid 2>/dev/null || true' EXIT; sleep 3; DEADRECKON_BASE_URL=${DEADRECKON_BASE_URL:-http://127.0.0.1:4173} npx --yes playwright test .deadreckon/acceptance/playwright-smoke.spec.mjs --reporter=line)\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Vite => {
            "name: vite acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs dist\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::NextJs => {
            "name: nextjs acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Python => {
            "name: python acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Auto => unreachable!("auto pack resolved above"),
    }
    .to_string();

    let mut files = BTreeMap::new();
    if matches!(
        pack,
        AcceptancePack::Browser | AcceptancePack::Vite | AcceptancePack::Playwright
    ) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/browser-smoke.mjs"),
            browser_smoke_script().to_string(),
        );
    }
    if matches!(pack, AcceptancePack::Playwright) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/playwright-smoke.spec.mjs"),
            playwright_smoke_spec().to_string(),
        );
    }
    AcceptanceDraft {
        markdown: format!(
            "# Done Criteria\n\nAdded the `{}` pack.\n\n{}",
            pack.name(),
            acceptance_markdown_from_yaml(&yaml)
        ),
        yaml,
        files,
    }
}

fn append_acceptance_checks(existing_raw: &str, addition_raw: &str) -> Result<String> {
    let mut existing = acceptance_yaml_value(existing_raw)?;
    let addition = acceptance_yaml_value(addition_raw)?;
    let mut checks = yaml_mapping_get(&addition, "checks")
        .map(yaml_items)
        .unwrap_or_default()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance pack did not contain checks",
            "try `deadreckon def-done add browser` or `deadreckon def-done \"what should count as done\"`",
        )));
    }
    let mapping = existing.as_mapping_mut().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml must be a mapping",
            "run `deadreckon def-done \"what should count as done\" --overwrite`",
        ))
    })?;
    let key = serde_yaml::Value::String("checks".to_string());
    let entry = mapping
        .entry(key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));
    match entry {
        serde_yaml::Value::Sequence(existing_checks) => existing_checks.append(&mut checks),
        other => {
            let mut merged = yaml_items(other).into_iter().cloned().collect::<Vec<_>>();
            merged.append(&mut checks);
            *other = serde_yaml::Value::Sequence(merged);
        }
    }
    serde_yaml::to_string(&existing).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("failed to write acceptance.yaml: {source}"),
            "run `deadreckon def-done \"what should count as done\" --overwrite`",
        ))
    })
}

fn browser_smoke_script() -> &'static str {
    r#"#!/usr/bin/env node
const fs = require('fs');
const http = require('http');
const path = require('path');

const requestedRoot = process.argv[2];
const candidates = [requestedRoot, 'dist', 'build', 'public', '.'].filter(Boolean);
const root = candidates.find((candidate) => fs.existsSync(path.join(candidate, 'index.html')));
if (!root) {
  console.error('No index.html found in dist, build, public, or project root.');
  process.exit(1);
}

const mime = new Map([
  ['.html', 'text/html'],
  ['.js', 'text/javascript'],
  ['.css', 'text/css'],
  ['.json', 'application/json'],
  ['.svg', 'image/svg+xml'],
]);

const server = http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
  const safePath = path.normalize(urlPath === '/' ? '/index.html' : urlPath).replace(/^(\.\.[/\\])+/, '');
  const file = path.join(root, safePath);
  if (!file.startsWith(path.resolve(root)) && path.isAbsolute(file)) {
    res.writeHead(403);
    res.end('forbidden');
    return;
  }
  fs.readFile(file, (err, body) => {
    if (err) {
      res.writeHead(404);
      res.end('missing');
      return;
    }
    res.writeHead(200, { 'content-type': mime.get(path.extname(file)) || 'application/octet-stream' });
    res.end(body);
  });
});

server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  try {
    const response = await fetch(`http://127.0.0.1:${port}/`);
    const body = await response.text();
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    if (!/<html[\s>]/i.test(body)) throw new Error('index response did not look like HTML');
    console.log(`loaded ${root}/index.html over HTTP`);
  } catch (error) {
    console.error(error.message || String(error));
    process.exitCode = 1;
  } finally {
    server.close();
  }
});
"#
}

fn playwright_smoke_spec() -> &'static str {
    r#"import { test, expect } from '@playwright/test';

test('app loads without browser console errors', async ({ page }) => {
  const errors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(process.env.DEADRECKON_BASE_URL || 'http://127.0.0.1:4173/', {
    waitUntil: 'networkidle',
  });
  await expect(page.locator('body')).toBeVisible();
  expect(errors).toEqual([]);
});
"#
}

fn acceptance_markdown_from_yaml(raw: &str) -> String {
    match acceptance_check_count(raw) {
        Ok(count) => format!(
            "Configured checks: {count}. Run `deadreckon def-done check` before starting long work."
        ),
        Err(_) => "Run `deadreckon def-done check` before starting long work.".to_string(),
    }
}

fn print_acceptance_yaml_summary(raw: &str) -> Result<()> {
    let root = acceptance_yaml_value(raw)?;
    println!("{}", ui_heading("checks"));
    for line in acceptance_summary_lines(&root) {
        println!("  {line}");
    }
    Ok(())
}

fn acceptance_summary_lines(root: &serde_yaml::Value) -> Vec<String> {
    let mut lines = Vec::new();
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(root, key) {
            for item in yaml_items(value) {
                lines.push(describe_acceptance_item(key, item));
            }
        }
    }
    if lines.is_empty() {
        lines.push("no recognized checks".to_string());
    }
    lines
}

fn describe_acceptance_item(group: &str, item: &serde_yaml::Value) -> String {
    if let Some(command) = item.as_str() {
        return format!("{group}: shell {}", one_line(command, 96));
    }
    let Some(mapping) = item.as_mapping() else {
        return format!("{group}: {}", one_line(&format!("{item:?}"), 120));
    };
    let kind = yaml_mapping_get(item, "kind")
        .and_then(serde_yaml::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            if mapping.len() == 1 {
                mapping
                    .keys()
                    .next()
                    .and_then(serde_yaml::Value::as_str)
                    .map(ToString::to_string)
            } else {
                None
            }
        })
        .unwrap_or_else(|| group.to_string());
    let path = yaml_mapping_get(item, "path")
        .and_then(serde_yaml::Value::as_str)
        .or_else(|| yaml_mapping_get(item, "cwd").and_then(serde_yaml::Value::as_str));
    let command = yaml_mapping_get(item, "command").and_then(serde_yaml::Value::as_str);
    let pattern = yaml_mapping_get(item, "pattern").and_then(serde_yaml::Value::as_str);
    let detail = command
        .or(path)
        .or(pattern)
        .map(|value| one_line(value, 96))
        .unwrap_or_else(|| one_line(&format!("{item:?}"), 96));
    format!("{group}: {kind} {detail}")
}

fn acceptance_check_count(raw: &str) -> Result<usize> {
    let root = acceptance_yaml_value(raw)?;
    let mut count = 0;
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(&root, key) {
            count += yaml_items(value).len();
        }
    }
    if count == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml does not contain any recognized checks",
            "run `deadreckon def-done \"what should count as done\"`",
        )));
    }
    Ok(count)
}

fn acceptance_yaml_value(raw: &str) -> Result<serde_yaml::Value> {
    serde_yaml::from_str(raw).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance.yaml: {source}"),
            "deadreckon def-done \"what should count as done\"",
        ))
    })
}

fn yaml_mapping_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    value
        .as_mapping()?
        .get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_items(value: &serde_yaml::Value) -> Vec<&serde_yaml::Value> {
    match value {
        serde_yaml::Value::Sequence(items) => items.iter().collect(),
        serde_yaml::Value::Null => Vec::new(),
        value => vec![value],
    }
}

fn read_optional_text(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn ensure_trailing_newline(value: &str) -> String {
    let mut value = value.trim().to_string();
    value.push('\n');
    value
}

fn acceptance_status_line(state: &deadreckon_core::PipelineState) -> String {
    let marker_path = marker_path_for_run_root(&state.run_root);
    if marker_path.exists()
        && let Ok(bytes) = fs::read(&marker_path)
        && let Ok(marker) = serde_json::from_slice::<AcceptanceMarker>(&bytes)
    {
        return format!("passed by dr-gate ({} checks)", marker.check_count);
    }
    let spec_path = acceptance_spec_path_for_run_root(&state.run_root);
    if spec_path.exists()
        && let Ok(raw) = fs::read_to_string(&spec_path)
        && let Ok(count) = acceptance_check_count(&raw)
    {
        return format!("configured ({count} checks)");
    }
    "default dr-gate behavior".to_string()
}

#[derive(Debug, Default)]
struct ConfigDefaults {
    provider: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
    cli_max_wall_seconds: Option<f64>,
    prevent_sleep: Option<String>,
    plain: Option<bool>,
    doc_provider: Option<String>,
    doc_skill: Option<String>,
    doc_subskills: Option<Vec<String>>,
    doc_polish_token_budget: Option<u32>,
    doc_polish_budget_cap_usd: Option<f64>,
}

fn config_defaults(paths: &DeadreckonPaths) -> Result<ConfigDefaults> {
    let root = load_config_value(paths)?;
    Ok(ConfigDefaults {
        provider: get_toml_path(&root, "defaults.provider")
            .or_else(|| get_toml_path(&root, "default_provider"))
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        sandbox: get_toml_path(&root, "defaults.sandbox")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        max_spend: get_toml_path(&root, "defaults.max_spend")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.max_spend")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
        cli_max_wall_seconds: get_toml_path(&root, "defaults.cli_max_wall_seconds")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.cli_max_wall_seconds")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
        prevent_sleep: get_toml_path(&root, "defaults.prevent_sleep")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        plain: get_toml_path(&root, "defaults.plain").and_then(toml::Value::as_bool),
        doc_provider: get_toml_path(&root, "defaults.doc_provider")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        doc_skill: get_toml_path(&root, "defaults.doc_skill")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        doc_subskills: get_toml_path(&root, "defaults.doc_subskills")
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            }),
        doc_polish_token_budget: get_toml_path(&root, "defaults.doc_polish_token_budget")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u32::try_from(value).ok()),
        doc_polish_budget_cap_usd: get_toml_path(&root, "defaults.doc_polish_budget_cap_usd")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.doc_polish_budget_cap_usd")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
    })
}

fn effective_doc_subskills(defaults: &ConfigDefaults) -> Vec<String> {
    defaults.doc_subskills.clone().unwrap_or_else(|| {
        DEFAULT_DOC_SUBSKILLS
            .iter()
            .map(|skill| (*skill).to_string())
            .collect()
    })
}

fn confirm_spend_cap(
    max_spend: Option<f64>,
    i_know_its_a_lot: bool,
    no_confirm: bool,
) -> Result<()> {
    let Some(max_spend) = max_spend else {
        return Ok(());
    };
    if max_spend <= 50.0 || i_know_its_a_lot || no_confirm {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "max spend above $50 requires --i-know-its-a-lot or --no-confirm in scripts"
                .to_string(),
        )));
    }
    eprintln!("{} --max-spend is ${max_spend:.2}", ui_warn("warning:"));
    if prompt::confirm(
        &format!("continue with --max-spend ${max_spend:.2}?"),
        false,
    )? {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(
            "run cancelled by spend confirmation".to_string(),
        )))
    }
}

async fn doctor_command(json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let source = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if json_output {
        let sandbox_checks = deadreckon_sandbox::doctor()
            .into_iter()
            .map(|backend| {
                json!({
                    "backend": backend.backend,
                    "available": backend.available,
                    "path": backend.path,
                    "note": backend.note,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "doctor",
                "id": &source,
                "status": "ok",
                "next_actions": ["deadreckon detect", "deadreckon providers list"],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "source": &source,
                "home": paths.home(),
                "config_path": paths.config_path(),
                "config_present": paths.config_path().exists(),
                "sandboxes": sandbox_checks,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("deadreckon doctor"));
    println!(
        "{} source {} | {} cd {}",
        ui_ok("✓"),
        source.display(),
        ui_command("try:"),
        source.display()
    );
    println!(
        "{} home {} | {} DEADRECKON_HOME={}",
        ui_ok("✓"),
        paths.home().display(),
        ui_command("try:"),
        paths.home().display()
    );
    for backend in deadreckon_sandbox::doctor() {
        if backend.available {
            let path = backend
                .path
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default();
            let version = backend
                .path
                .as_ref()
                .and_then(|path| command_version(path))
                .map(|version| format!(" ({version})"))
                .unwrap_or_default();
            println!(
                "{} sandbox {} found{}{} | {} deadreckon run \"goal\" --sandbox {} --preview",
                ui_ok("✓"),
                backend.backend,
                path,
                version,
                ui_command("try:"),
                backend.backend
            );
        } else {
            println!("{} sandbox {} missing", ui_warn("✗"), backend.backend);
            println!("    {} {}", ui_command("fix:"), backend.note);
        }
    }
    if paths.config_path().exists() {
        match load_config_value(&paths) {
            Ok(root) => {
                println!(
                    "{} {} present and parseable | {} deadreckon config provider",
                    ui_ok("✓"),
                    paths.config_path().display(),
                    ui_command("try:")
                );
                doctor_providers(&paths, &root).await?;
            }
            Err(err) => {
                println!(
                    "{} {} is not parseable",
                    ui_warn("✗"),
                    paths.config_path().display()
                );
                println!(
                    "    {} check TOML syntax or rerun `deadreckon init` ({err})",
                    ui_command("fix:")
                );
            }
        }
    } else {
        println!("{} {} missing", ui_warn("✗"), paths.config_path().display());
        println!("    {} deadreckon init", ui_command("fix:"));
    }
    let defaults = config_defaults(&paths).unwrap_or_default();
    if defaults.provider.is_some() || paths.config_path().exists() {
        println!(
            "{} provider defaults configured | {} deadreckon config provider",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else if command_exists("claude") || command_exists("codex") {
        println!(
            "{} cli subscription provider available | {} deadreckon init --no-confirm",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else {
        println!("{} no provider configured", ui_warn("✗"));
        println!(
            "    {} deadreckon init or deadreckon config set providers.anthropic.api_key <KEY>",
            ui_command("fix:")
        );
    }
    doctor_disk_and_permissions(&paths);
    doctor_os();
    doctor_sleep_prevention();
    doctor_subscription_binary("claude");
    doctor_subscription_binary("codex");
    Ok(())
}

fn doctor_sleep_prevention() {
    let preview = sleep::preview(SleepPrefs::On, true);
    match preview.mode {
        sleep::SleepMode::Caffeinate | sleep::SleepMode::SystemdInhibit => {
            println!(
                "{} sleep prevention {} | {} deadreckon run \"goal\" --prevent-sleep auto",
                ui_ok("✓"),
                preview.label(),
                ui_command("try:")
            );
        }
        sleep::SleepMode::None => {
            println!(
                "{} sleep prevention disabled | {} deadreckon run \"goal\" --prevent-sleep on",
                ui_warn("✗"),
                ui_command("try:")
            );
        }
        sleep::SleepMode::Unsupported => {
            let fix = if cfg!(target_os = "linux") {
                "sudo apt install systemd"
            } else if cfg!(target_os = "macos") {
                "check /usr/bin/caffeinate"
            } else {
                "--prevent-sleep off (Windows native prevention is a V1 candidate)"
            };
            println!("{} sleep prevention unsupported", ui_warn("✗"));
            println!("    {} {fix}", ui_command("fix:"));
        }
    }
}

async fn doctor_providers(paths: &DeadreckonPaths, root: &toml::Value) -> Result<()> {
    let Some(providers) = root.get("providers").and_then(toml::Value::as_table) else {
        println!("{} providers table missing", ui_warn("✗"));
        println!("    {} deadreckon init", ui_command("fix:"));
        return Ok(());
    };
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    for (name, entry) in providers {
        let kind = entry
            .get("kind")
            .and_then(toml::Value::as_str)
            .unwrap_or(name);
        let kind_label = registry
            .get(name)
            .map(|descriptor| descriptor_kind_label(&descriptor.kind).to_string())
            .unwrap_or_else(|| config_provider_kind_label(kind).to_string());
        if kind.contains("cli") || name.starts_with("cli:") {
            let binary = entry
                .get("binary")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| {
                    if name.contains("claude") {
                        "claude"
                    } else {
                        "codex"
                    }
                });
            if command_exists(binary) || PathBuf::from(binary).exists() {
                println!(
                    "{} provider {name} kind={kind_label} CLI binary {binary} found | {} deadreckon run \"goal\" --provider {name} --preview",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            } else {
                println!(
                    "{} provider {name} kind={kind_label} CLI binary {binary} missing",
                    ui_warn("✗")
                );
                println!(
                    "    {} install {binary} or set providers.\"{name}\".binary",
                    ui_command("fix:")
                );
            }
        } else if provider_has_key(entry) {
            if std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
                doctor_provider_ping(paths, name, &kind_label).await?;
            } else {
                println!(
                    "{} provider {name} kind={kind_label} credential present; ping skipped | {} DEADRECKON_DOCTOR_PING=1 deadreckon doctor",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            }
        } else {
            println!(
                "{} provider {name} kind={kind_label} credential missing",
                ui_warn("✗")
            );
            println!(
                "    {} deadreckon config set providers.{name}.api_key <KEY>",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

fn config_provider_kind_label(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("cli") {
        "cli"
    } else if kind.contains("compatible") || kind.contains("local") {
        "local-http"
    } else if kind.contains("smoke") || kind.contains("script") {
        "scripted"
    } else if kind.contains("anthropic") || kind.contains("open-ai") || kind.contains("openai") {
        "http"
    } else {
        "custom"
    }
}

async fn doctor_provider_ping(paths: &DeadreckonPaths, name: &str, kind_label: &str) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(name))?;
    let request = ProviderRequest {
        prompt: "Reply with OK only.".to_string(),
        max_output_tokens: 8,
        cwd: None,
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        router.complete(&request),
    )
    .await
    {
        Ok(Ok(response)) => println!(
            "{} provider {name} kind={kind_label} ping ok model {} | {} deadreckon run \"goal\" --provider {name} --preview",
            ui_ok("✓"),
            response.model,
            ui_command("try:")
        ),
        Ok(Err(err)) => {
            println!(
                "{} provider {name} kind={kind_label} ping failed",
                ui_warn("✗")
            );
            println!(
                "    {} check credentials or set a fallback provider ({err})",
                ui_command("fix:")
            );
        }
        Err(_) => {
            println!(
                "{} provider {name} kind={kind_label} ping timed out",
                ui_warn("✗")
            );
            println!(
                "    {} check network/provider status or unset DEADRECKON_DOCTOR_PING",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

fn provider_has_key(entry: &toml::Value) -> bool {
    entry
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || entry
            .get("api_key_env")
            .and_then(toml::Value::as_str)
            .and_then(std::env::var_os)
            .is_some()
}

fn doctor_disk_and_permissions(paths: &DeadreckonPaths) {
    if let Err(err) = fs::create_dir_all(paths.runstate_dir()) {
        println!(
            "{} runstate dir {} not writable",
            ui_warn("✗"),
            paths.runstate_dir().display()
        );
        println!(
            "    {} mkdir -p {} && chmod u+w {}",
            ui_command("fix:"),
            paths.runstate_dir().display(),
            paths.runstate_dir().display()
        );
        println!("    detail: {err}");
        return;
    }
    let probe = paths.runstate_dir().join(".doctor-write-test");
    match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => println!(
            "{} runstate dir {} writable | {} deadreckon run \"goal\" --preview",
            ui_ok("✓"),
            paths.runstate_dir().display(),
            ui_command("try:")
        ),
        Err(err) => {
            println!(
                "{} runstate dir {} not writable",
                ui_warn("✗"),
                paths.runstate_dir().display()
            );
            println!(
                "    {} chmod u+w {}",
                ui_command("fix:"),
                paths.runstate_dir().display()
            );
            println!("    detail: {err}");
        }
    }
    match free_kb(paths.home()) {
        Some(kb) if kb < 1_048_576 => {
            println!(
                "{} disk space low: {} MB free in {}",
                ui_warn("✗"),
                kb / 1024,
                paths.home().display()
            );
            println!(
                "    {} free at least 1 GB or set DEADRECKON_HOME to a larger disk",
                ui_command("fix:")
            );
        }
        Some(kb) => println!(
            "{} disk space {} MB free in {} | {} deadreckon status",
            ui_ok("✓"),
            kb / 1024,
            paths.home().display(),
            ui_command("try:")
        ),
        None => {
            println!(
                "{} disk space check unavailable for {}",
                ui_warn("✗"),
                paths.home().display()
            );
            println!(
                "    {} run `df -Pk {}` manually",
                ui_command("fix:"),
                paths.home().display()
            );
        }
    }
}

fn doctor_os() {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os macOS {version} | {} sw_vers -productVersion",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os Linux kernel {version} | {} uname -r",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
}

fn doctor_subscription_binary(binary: &str) {
    if command_exists(binary) {
        let provider = if binary == "claude" {
            "cli:claude-code"
        } else {
            "cli:codex"
        };
        println!(
            "{} subscription binary {binary} {} | {} deadreckon config provider {provider}",
            ui_ok("✓"),
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string()),
            ui_command("try:")
        );
    } else {
        println!("{} subscription binary {binary} missing", ui_warn("✗"));
        println!(
            "    {} install {binary} or choose another provider with `deadreckon config set defaults.provider <name>`",
            ui_command("fix:")
        );
    }
}

fn command_version(path: &std::path::Path) -> Option<String> {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            text.lines().next().unwrap_or_default().trim().to_string()
        })
        .filter(|line| !line.is_empty())
}

fn init_git_repo(cwd: &Path) -> Result<()> {
    if deadreckon_core::find_git_root(cwd)?.is_some() {
        return Ok(());
    }
    run_git(cwd, &["init", "-b", "main"])?;
    run_git(cwd, &["config", "user.email", "deadreckon@example.invalid"])?;
    run_git(cwd, &["config", "user.name", "deadreckon"])?;
    run_git(cwd, &["add", "-A"])?;
    run_git(
        cwd,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "initial commit (deadreckon init)",
        ],
    )
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = deadreckon_core::git::run_git(cwd, args)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(CliError::Core(DeadreckonError::InvalidInput(
            if stderr.is_empty() {
                format!("git {:?} failed", args)
            } else {
                stderr
            },
        )))
    }
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = deadreckon_core::git::run_git(cwd, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))))
    }
}

fn git_ref_exists(cwd: &Path, reference: &str) -> bool {
    deadreckon_core::git::run_git(cwd, &["rev-parse", "--verify", "--quiet", reference])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<()> {
    git_stdout(cwd, args).map(|_| ())
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "path is not valid UTF-8: {}",
            path.display()
        )))
    })
}

struct RunPreview<'a> {
    goal: &'a str,
    cwd: &'a Path,
    codebase: &'a CodebaseRecord,
    provider: Option<&'a str>,
    provider_source: &'a str,
    route: Option<&'a ProviderRouteInfo>,
    sandbox: &'a str,
    doc_provider: Option<&'a str>,
    doc_provider_source: &'a str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    acceptance: &'a setup::DoneCriteriaSelection,
    sleep: &'a sleep::SleepPreview,
    brief: bool,
    plain: bool,
    run_id: &'a str,
}

fn run_preview(input: &RunPreview<'_>) -> String {
    let &RunPreview {
        goal,
        cwd,
        codebase,
        provider,
        provider_source,
        route,
        sandbox,
        doc_provider,
        doc_provider_source,
        max_spend,
        max_wall_seconds,
        acceptance,
        sleep,
        brief,
        plain,
        run_id,
    } = input;
    let mode = codebase.mode.to_string();
    let agent = route
        .map(|route| route.name.as_str())
        .or(provider)
        .unwrap_or("-");
    let model = route.map(|route| route.model.as_str()).unwrap_or("unknown");
    let caps = format!(
        "spend {}, wall {}",
        max_spend
            .map(|cap| format!("<= ${cap:.0}"))
            .unwrap_or_else(|| "uncapped".to_string()),
        format_wall_cap(max_wall_seconds)
    );
    if brief {
        return format!(
            "mode={} branch={} base={} wt={} provider={} model={} docs={} cap={}/{} done_criteria={}",
            mode,
            codebase.branch_name.as_deref().unwrap_or("-"),
            codebase.base_ref.as_deref().unwrap_or("-"),
            codebase
                .worktree_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            agent,
            model,
            doc_provider.unwrap_or("templated"),
            max_spend
                .map(|cap| format!("${cap:.0}"))
                .unwrap_or_else(|| "uncapped".to_string()),
            format_wall_cap(max_wall_seconds),
            acceptance.brief_label()
        );
    }

    let git_label = match preview_git_state(cwd) {
        Ok(Some(git)) => format!("git: clean, branch={} @ {}", git.branch, git.head_sha),
        _ => "not git".to_string(),
    };
    let mut rows = vec![
        ("goal".to_string(), goal.to_string()),
        (
            "source".to_string(),
            format!("{} ({git_label})", cwd.display()),
        ),
        ("mode".to_string(), mode),
    ];
    if codebase.mode == CodebaseMode::Worktree {
        rows.extend([
            (
                "branch".to_string(),
                codebase.branch_name.as_deref().unwrap_or("-").to_string(),
            ),
            (
                "base ref".to_string(),
                format!(
                    "{} ({})",
                    codebase.base_ref.as_deref().unwrap_or("-"),
                    codebase
                        .base_sha
                        .as_deref()
                        .map(|sha| sha.chars().take(8).collect::<String>())
                        .unwrap_or_else(|| "-".to_string())
                ),
            ),
            (
                "worktree".to_string(),
                codebase
                    .worktree_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ]);
    } else if let Some(source_path) = codebase.source_path.as_ref() {
        rows.push(("source-copy".to_string(), source_path.display().to_string()));
    }
    if codebase.mode == CodebaseMode::InPlace {
        rows.push((
            "warning".to_string(),
            "SOURCE-IS-USER-TREE; undo uses runstate snapshots".to_string(),
        ));
    }
    rows.extend([
        (
            "provider".to_string(),
            format!("{agent} ({provider_source})"),
        ),
        ("model".to_string(), model.to_string()),
        (
            "docs".to_string(),
            format!(
                "{} ({doc_provider_source})",
                doc_provider.unwrap_or("templated only")
            ),
        ),
        ("sandbox".to_string(), sandbox.to_string()),
        ("caps".to_string(), caps),
        ("sleep".to_string(), sleep.label()),
        ("done criteria".to_string(), acceptance.full_label()),
    ]);
    if max_spend.is_some_and(|cap| cap > 50.0) {
        rows.push((
            "confirmation".to_string(),
            "high spend acknowledged before run state is created".to_string(),
        ));
    }
    let mut sections = vec![Section::KeyValue { rows }];
    match codebase.mode {
        CodebaseMode::Worktree => {
            sections.push(Section::Blank);
            sections.push(Section::Command {
                label: "on success".to_string(),
                command: format!("deadreckon apply {run_id}"),
            });
            sections.push(Section::Command {
                label: "on fail".to_string(),
                command: format!("deadreckon cleanup {run_id}"),
            });
        }
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            sections.push(Section::Blank);
            sections.push(Section::Command {
                label: "on success".to_string(),
                command: format!("deadreckon export {run_id} --dest <path>"),
            });
            sections.push(Section::Command {
                label: "inspect".to_string(),
                command: format!("deadreckon show {run_id}"),
            });
        }
        CodebaseMode::InPlace => {
            sections.push(Section::Blank);
            sections.push(Section::Command {
                label: "rollback".to_string(),
                command: format!("deadreckon undo --run {run_id}"),
            });
            sections.push(Section::Command {
                label: "inspect".to_string(),
                command: format!("deadreckon show {run_id}"),
            });
        }
    }
    render_card(
        &Card {
            title: TitleLine {
                glyph: TitleGlyph::Preview,
                label: "deadreckon run preview".to_string(),
            },
            subtitle: Some(goal.to_string()),
            sections,
            hints: vec![HintLine {
                label: "run".to_string(),
                command: "rerun with --yes to skip this confirmation".to_string(),
            }],
        },
        &card_options(ui::Stream::Stderr, plain),
    )
}

fn card_options(stream: ui::Stream, plain: bool) -> CardOptions {
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    CardOptions {
        color: ui::enabled(stream),
        plain: plain || no_color_env,
        terminal_columns: terminal_columns(),
        no_color_env,
    }
}

fn terminal_columns() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            crossterm::terminal::size()
                .ok()
                .map(|(cols, _)| cols as usize)
        })
}

fn sleep_try_line(reason: sleep::SkipReason) -> Option<&'static str> {
    match reason {
        sleep::SkipReason::UnavailableBinary if cfg!(target_os = "macos") => {
            Some("macOS bundles caffeinate; check $PATH or run \"/usr/bin/caffeinate -di\"")
        }
        sleep::SkipReason::UnavailableBinary if cfg!(target_os = "linux") => {
            Some("sudo apt install systemd")
        }
        sleep::SkipReason::Unsupported => {
            Some("--prevent-sleep off (Windows native prevention is a V1 candidate)")
        }
        sleep::SkipReason::AlreadyInhibited
        | sleep::SkipReason::NonTty
        | sleep::SkipReason::UserDisabled
        | sleep::SkipReason::UnavailableBinary => None,
    }
}

struct OrchestrateRunArgs {
    plan: PlanCommandArgs,
    preview: bool,
    yes: bool,
    no_repair: bool,
}

struct BareOrchestrateArgs {
    goal: Option<String>,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<String>,
    preview: bool,
    init_git: bool,
    acceptance: Option<PathBuf>,
    yes: bool,
    no_repair: bool,
    no_hints: bool,
    quiet: bool,
    plain: bool,
}

fn orchestrate_request_from_cli(
    command: Option<OrchestrateCommand>,
    bare: BareOrchestrateArgs,
) -> Result<OrchestrateRunArgs> {
    match command {
        Some(OrchestrateCommand::Review(args)) => Ok(OrchestrateRunArgs {
            plan: PlanCommandArgs {
                goal: args.goal,
                n: 2,
                mode: CliPlanMode::Review,
                max_spend: args.max_spend.or(bare.max_spend),
                max_wall_seconds: args.max_wall_seconds.or(bare.max_wall_seconds),
                sandbox: args.sandbox.or(bare.sandbox),
                planner_provider: None,
                provider: None,
                child_provider: Vec::new(),
                coder_provider: args.coder_provider,
                reviewer_provider: args.reviewer_provider,
                init_git: args.init_git || bare.init_git,
                acceptance: args.acceptance.or(bare.acceptance),
                skip_acceptance_prompt: args.yes
                    || args.preview
                    || args.quiet
                    || bare.yes
                    || bare.preview
                    || bare.quiet,
                no_hints: args.no_hints || bare.no_hints,
                quiet: args.quiet || bare.quiet,
                json: false,
                plain: args.plain || bare.plain,
            },
            preview: args.preview || bare.preview,
            yes: args.yes || bare.yes,
            no_repair: args.no_repair || bare.no_repair,
        }),
        Some(OrchestrateCommand::FullPlan(args)) => Ok(OrchestrateRunArgs {
            plan: PlanCommandArgs {
                goal: args.goal,
                n: args.n,
                mode: CliPlanMode::FullPlan,
                max_spend: args.max_spend.or(bare.max_spend),
                max_wall_seconds: args.max_wall_seconds.or(bare.max_wall_seconds),
                sandbox: args.sandbox.or(bare.sandbox),
                planner_provider: args.planner_provider,
                provider: args.provider,
                child_provider: args.child_provider,
                coder_provider: None,
                reviewer_provider: None,
                init_git: args.init_git || bare.init_git,
                acceptance: args.acceptance.or(bare.acceptance),
                skip_acceptance_prompt: args.yes
                    || args.preview
                    || args.quiet
                    || bare.yes
                    || bare.preview
                    || bare.quiet,
                no_hints: args.no_hints || bare.no_hints,
                quiet: args.quiet || bare.quiet,
                json: false,
                plain: args.plain || bare.plain,
            },
            preview: args.preview || bare.preview,
            yes: args.yes || bare.yes,
            no_repair: args.no_repair || bare.no_repair,
        }),
        None => interactive_orchestrate_request(bare),
    }
}

fn interactive_orchestrate_request(bare: BareOrchestrateArgs) -> Result<OrchestrateRunArgs> {
    let BareOrchestrateArgs {
        goal,
        max_spend,
        max_wall_seconds,
        sandbox,
        preview,
        init_git,
        acceptance,
        yes,
        no_repair,
        no_hints,
        quiet,
        plain,
    } = bare;
    let Some(goal) = goal else {
        return Err(CliError::Core(deadreckon_core::user_error(
            "orchestrate requires a mode or goal",
            "deadreckon orchestrate review \"goal\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes",
        )));
    };
    if !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive orchestrate requires an explicit mode",
            "deadreckon orchestrate review \"goal\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let default_provider = resolve_provider_name(
        &paths,
        setup::SetupProviderRoleRef::DefaultChild,
        defaults.provider,
    )?;
    let recommended_mode = recommend_orchestration_mode(&goal);
    println!("{}", ui_heading("Orchestration mode"));
    println!(
        "  recommendation: {} - {}",
        ui_command(plan_mode_label(match recommended_mode {
            CliPlanMode::FullPlan => PlanMode::FullPlan,
            CliPlanMode::Review => PlanMode::Review,
        })),
        orchestration_mode_recommendation_reason(&goal, recommended_mode)
    );
    println!(
        "  {} focused implementation with one coder provider, then a fresh reviewer/fixer",
        ui_command("review")
    );
    println!(
        "  {} planner provider decomposes broad product work into child implementation agents before fork and merge",
        ui_command("full-plan")
    );
    let mode = prompt_orchestration_mode(recommended_mode)?;
    print_orchestrate_provider_choices(&paths, default_provider.as_deref())?;
    let mut plan = PlanCommandArgs {
        goal,
        n: recommend_child_count_for_goal("", mode),
        mode,
        max_spend,
        max_wall_seconds,
        sandbox,
        planner_provider: None,
        provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        init_git,
        acceptance,
        skip_acceptance_prompt: yes || preview || quiet,
        no_hints,
        quiet,
        json: false,
        plain,
    };
    match mode {
        CliPlanMode::FullPlan => {
            plan.n = prompt_child_count(recommend_child_count_for_goal(&plan.goal, mode))?;
            plan.planner_provider = prompt_provider_role("planner", default_provider.as_deref())?;
            plan.provider = prompt_provider_role("default child", default_provider.as_deref())?;
            plan.child_provider = prompt_child_provider_overrides(plan.n)?;
        }
        CliPlanMode::Review => {
            plan.coder_provider = prompt_provider_role("coder", default_provider.as_deref())?;
            plan.reviewer_provider = prompt_provider_role("reviewer", default_provider.as_deref())?;
        }
    }
    Ok(OrchestrateRunArgs {
        plan,
        preview,
        yes,
        no_repair,
    })
}

fn recommend_orchestration_mode(goal: &str) -> CliPlanMode {
    let lower = goal.to_ascii_lowercase();
    let broad_product = [
        "make a full",
        "build a full",
        "create a full",
        "fully",
        "from scratch",
        "app",
        "game",
        "site",
        "multiplayer",
        "realtime",
        "real-time",
        "live",
        "server",
        "client",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let focused_change = [
        "fix ", "bug", "change ", "refactor", "review", "audit", "explain", "docs", "rename",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if broad_product && !focused_change {
        CliPlanMode::FullPlan
    } else {
        CliPlanMode::Review
    }
}

fn orchestration_mode_recommendation_reason(goal: &str, mode: CliPlanMode) -> &'static str {
    match mode {
        CliPlanMode::FullPlan => {
            if goal.to_ascii_lowercase().contains("multiplayer") {
                "goal looks like broad product work with separable implementation slices"
            } else {
                "goal looks broad enough to decompose before execution"
            }
        }
        CliPlanMode::Review => "goal looks focused enough for coder plus reviewer",
    }
}

fn recommend_child_count_for_goal(goal: &str, mode: CliPlanMode) -> u8 {
    if mode == CliPlanMode::Review {
        return 2;
    }
    let lower = goal.to_ascii_lowercase();
    let complexity = [
        "multiplayer",
        "realtime",
        "real-time",
        "live",
        "physics",
        "terrain",
        "server",
        "database",
        "auth",
        "deploy",
        "mobile",
        "game",
    ]
    .iter()
    .filter(|needle| lower.contains(*needle))
    .count();
    match complexity {
        0 | 1 => 3,
        2 | 3 => 4,
        _ => 5,
    }
}

fn prompt_orchestration_mode(default: CliPlanMode) -> Result<CliPlanMode> {
    let default_label = match default {
        CliPlanMode::Review => "review",
        CliPlanMode::FullPlan => "full-plan",
    };
    let answer = prompt::open(&format!("mode [{default_label}]: "), None)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "r" | "review" => Ok(CliPlanMode::Review),
        "f" | "full" | "full-plan" | "full_plan" | "plan" => Ok(CliPlanMode::FullPlan),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown orchestration mode {other}"),
            "choose review or full-plan",
        ))),
    }
}

fn prompt_child_count(default: u8) -> Result<u8> {
    let answer = prompt::open(&format!("children [{default}]: "), None)?;
    if answer.trim().is_empty() {
        return Ok(default);
    }
    let n = answer.trim().parse::<u8>().map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("child count is not a number: {answer}"),
            "enter a value from 2 through 6",
        ))
    })?;
    validate_task_count(usize::from(n)).map_err(CliError::Core)?;
    Ok(n)
}

fn prompt_child_provider_overrides(n: u8) -> Result<Vec<String>> {
    println!(
        "  optional: route specific child indices 0..{} to another provider, e.g. 1=cli:codex",
        n.saturating_sub(1)
    );
    let answer = prompt::open("child provider overrides []: ", None)?;
    if answer.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(answer
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn prompt_provider_role(role: &str, default: Option<&str>) -> Result<Option<String>> {
    let prompt_text = match default {
        Some(default) => format!("{role} provider [{default}]: "),
        None => format!("{role} provider: "),
    };
    let answer = prompt::open(&prompt_text, None)?;
    let provider = answer.trim();
    if provider.is_empty() {
        return Ok(default.map(ToString::to_string));
    }
    Ok(Some(provider.to_string()))
}

fn print_orchestrate_provider_choices(
    paths: &DeadreckonPaths,
    default_provider: Option<&str>,
) -> Result<()> {
    let configured = configured_provider_ids(paths)?;
    println!("{}", ui_heading("Providers"));
    println!(
        "  default: {}",
        default_provider
            .map(ui_command)
            .unwrap_or_else(|| ui_muted("none configured"))
    );
    if configured.is_empty() {
        println!("  configured: none");
        println!(
            "  {} {}",
            ui_command("try:"),
            ui_command("deadreckon providers list --all")
        );
    } else {
        println!("  configured: {}", configured.join(", "));
    }
    println!("  planner creates the child graph; child/coder/reviewer providers execute work.");
    Ok(())
}

async fn orchestrate_command(args: OrchestrateRunArgs) -> Result<()> {
    let quiet = args.plan.quiet;
    let plain = args.plan.plain;
    let no_hints = args.plan.no_hints;
    let max_spend = args.plan.max_spend;
    let max_wall_seconds = args.plan.max_wall_seconds;
    let sandbox = args.plan.sandbox.clone();
    if !prepare_orchestration_source(args.plan.init_git, quiet)? {
        return Ok(());
    }
    let plan = create_orchestration_plan(args.plan).await?;
    let plan_id = plan.plan_id.clone();
    if !quiet {
        print_orchestrate_preflight(
            &plan,
            max_spend,
            max_wall_seconds,
            sandbox.as_deref(),
            args.no_repair,
        );
    }
    if args.preview {
        return Ok(());
    }
    confirm_orchestration_start(&plan, args.yes)?;
    if !quiet {
        print_orchestrate_started(
            &plan,
            max_spend,
            max_wall_seconds,
            sandbox.as_deref(),
            args.no_repair,
        );
    }
    fork_command(ForkCommandArgs {
        plan_id: plan_id.clone(),
        max_spend,
        max_wall_seconds,
        sandbox,
        provider: None,
        child_provider: Vec::new(),
        coder_provider: None,
        reviewer_provider: None,
        no_hints,
        quiet,
        plain,
    })
    .await?;
    merge_command(MergeCommandArgs {
        plan_id,
        strategy: "dag-aware".to_string(),
        prefer_child: None,
        no_repair: args.no_repair,
        repair_provider: None,
        repair_mode: "auto".to_string(),
        repair_attempts: 1,
        yes: true,
        no_gate: false,
        no_hints,
        quiet,
        plain,
    })
    .await
}

async fn plan_command(args: PlanCommandArgs) -> Result<()> {
    let quiet = args.quiet;
    let no_hints = args.no_hints;
    let json_output = args.json;
    if !prepare_orchestration_source(args.init_git, quiet)? {
        return Ok(());
    }
    let plan = create_orchestration_plan(args).await?;
    if json_output {
        print_plan_json(&plan)?;
        return Ok(());
    }
    if !quiet {
        print_plan_created(&plan, no_hints);
    }
    Ok(())
}

fn prepare_orchestration_source(init_git: bool, quiet: bool) -> Result<bool> {
    let cwd = std::env::current_dir()?;
    if init_git {
        init_git_repo(&cwd)?;
        return Ok(true);
    }
    if deadreckon_core::find_git_root(&cwd)?.is_some() || quiet || !io::stdin().is_terminal() {
        return Ok(true);
    }
    match prompt_non_git_mode()? {
        NonGitChoice::Init => {
            init_git_repo(&cwd)?;
            Ok(true)
        }
        NonGitChoice::Copy => Ok(true),
        NonGitChoice::Cancel => {
            println!("cancelled");
            Ok(false)
        }
    }
}

async fn create_orchestration_plan(args: PlanCommandArgs) -> Result<Plan> {
    let PlanCommandArgs {
        goal,
        n,
        mode,
        max_spend: _,
        max_wall_seconds: _,
        sandbox: _,
        planner_provider,
        provider,
        child_provider,
        coder_provider,
        reviewer_provider,
        init_git: _,
        acceptance,
        skip_acceptance_prompt,
        no_hints: _,
        quiet,
        json: _,
        plain,
    } = args;
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--goal must be non-empty",
            "deadreckon plan \"your goal\"",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let cwd = std::env::current_dir()?;
    let scope = workspace_scope(&cwd)?;
    let plan_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let plan_mode = match mode {
        CliPlanMode::FullPlan => PlanMode::FullPlan,
        CliPlanMode::Review => PlanMode::Review,
    };
    let mut providers = resolve_plan_providers(
        &paths,
        &defaults,
        plan_mode,
        planner_provider,
        provider,
        coder_provider,
        reviewer_provider,
    )?;
    let acceptance_provider = match plan_mode {
        PlanMode::FullPlan => providers
            .planner
            .clone()
            .or_else(|| providers.default_child.clone()),
        PlanMode::Review => providers
            .coder
            .clone()
            .or_else(|| providers.reviewer.clone())
            .or_else(|| providers.default_child.clone()),
    };
    let acceptance_source = ensure_acceptance_before_start(
        &cwd,
        acceptance.as_deref(),
        &goal,
        acceptance_provider,
        None,
        skip_acceptance_prompt || quiet,
        "orchestration",
    )
    .await?;
    let mut tasks = match plan_mode {
        PlanMode::FullPlan => {
            validate_task_count(usize::from(n)).map_err(CliError::Core)?;
            let overrides = parse_child_provider_overrides(&child_provider, n)?;
            providers.children = overrides.clone();
            build_full_plan_tasks(&paths, &goal, n, &providers, &overrides, &cwd, plain).await?
        }
        PlanMode::Review => build_review_plan_tasks(&goal, &providers),
    };
    for task in &mut tasks {
        task.worker_spec = deadreckon_core::worker_spec_relative_path(&task.task_id);
    }
    let mut plan = Plan::new(
        goal,
        plan_mode,
        tasks,
        providers,
        Some(scope),
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(CliError::Core)?;
    plan.parent_cwd = Some(plan_cwd);
    plan.acceptance_path = acceptance_source.as_ref().map(|source| source.path.clone());
    plan.capability_preview = infer_capability_preview(&plan.root_goal);
    for task in &plan.tasks {
        let spec = render_worker_spec(&plan, task);
        write_worker_spec(&paths, &plan.plan_id, &task.task_id, &spec)?;
    }
    save_plan(&paths, &plan)?;
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::PlanCreated {
            mode: plan.mode,
            task_count: plan.tasks.len(),
        },
    )?;
    Ok(plan)
}

fn resolve_plan_providers(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    mode: PlanMode,
    planner_provider: Option<String>,
    provider: Option<String>,
    coder_provider: Option<String>,
    reviewer_provider: Option<String>,
) -> Result<PlanProviders> {
    let default_child = resolve_provider_name(
        paths,
        setup::SetupProviderRoleRef::DefaultChild,
        provider.or(defaults.provider.clone()),
    )?;
    let planner = match mode {
        PlanMode::FullPlan => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Planner,
            planner_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::Review => None,
    };
    let coder = match mode {
        PlanMode::Review => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Coder,
            coder_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::FullPlan => None,
    };
    let reviewer = match mode {
        PlanMode::Review => resolve_provider_name(
            paths,
            setup::SetupProviderRoleRef::Reviewer,
            reviewer_provider
                .or(default_child.clone())
                .or(defaults.provider.clone()),
        )?,
        PlanMode::FullPlan => None,
    };
    Ok(PlanProviders {
        planner,
        default_child,
        coder,
        reviewer,
        children: BTreeMap::new(),
    })
}

fn resolve_provider_name(
    paths: &DeadreckonPaths,
    role: setup::SetupProviderRoleRef,
    provider: Option<String>,
) -> Result<Option<String>> {
    if provider
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        return Ok(provider);
    }
    let selection = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role,
            explicit_provider: provider.as_deref(),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: true,
            allow_auto_subscription: false,
            require_usable_route: false,
        },
    )?;
    Ok(selection.provider.or(provider))
}

fn parse_child_provider_overrides(values: &[String], n: u8) -> Result<BTreeMap<u32, String>> {
    let mut overrides = BTreeMap::new();
    for value in values {
        let Some((idx, provider)) = value.split_once('=') else {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("child provider override must be IDX=PROVIDER: {value}"),
                "--child-provider 1=cli:codex",
            )));
        };
        let index = idx.trim().parse::<u32>().map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                &format!("child provider index is not a number: {idx}"),
                "--child-provider 1=cli:codex",
            ))
        })?;
        if index >= u32::from(n) {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("child provider index {index} outside 0..{n}"),
                "--child-provider 1=cli:codex",
            )));
        }
        let provider = provider.trim();
        if provider.is_empty() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "child provider must be non-empty",
                "--child-provider 1=cli:codex",
            )));
        }
        overrides.insert(index, provider.to_string());
    }
    Ok(overrides)
}

async fn build_full_plan_tasks(
    paths: &DeadreckonPaths,
    goal: &str,
    n: u8,
    providers: &PlanProviders,
    overrides: &BTreeMap<u32, String>,
    cwd: &Path,
    plain: bool,
) -> Result<Vec<PlanTask>> {
    let drafts = if providers
        .planner
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        deterministic_plan_drafts(goal, n)
    } else {
        provider_plan_drafts(paths, goal, n, providers.planner.as_deref(), cwd, plain).await?
    };
    if drafts.len() != usize::from(n) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("provider returned {} children; need {n}", drafts.len()),
            "deadreckon plan ... --provider <other>",
        )));
    }
    let mut tasks = Vec::new();
    for (index, draft) in drafts.into_iter().enumerate() {
        let index = index as u32;
        let provider = overrides
            .get(&index)
            .cloned()
            .or_else(|| providers.default_child.clone());
        let mut task = PlanTask::new(index, draft.subject, draft.goal, PlanRole::Child, provider);
        task.active_form = draft.active_form.unwrap_or_else(|| task.subject.clone());
        task.depends_on = draft.depends_on;
        tasks.push(task);
    }
    Ok(tasks)
}

fn build_review_plan_tasks(goal: &str, providers: &PlanProviders) -> Vec<PlanTask> {
    let mut coder = PlanTask::new(
        0,
        "Implement requested change",
        goal,
        PlanRole::Coder,
        providers.coder.clone(),
    );
    coder.active_form = "Coding implementation".to_string();
    let mut reviewer = PlanTask::new(
        1,
        "Review and fix implementation",
        format!(
            "Review the completed implementation for: {goal}. Write .deadreckon/REVIEW.md first, then apply only fixes tied to findings and acceptance."
        ),
        PlanRole::Reviewer,
        providers.reviewer.clone(),
    );
    reviewer.active_form = "Reviewing implementation".to_string();
    reviewer.depends_on = vec![coder.task_id.clone()];
    vec![coder, reviewer]
}

#[derive(Debug, Deserialize)]
struct PlannerDraft {
    subject: String,
    goal: String,
    #[serde(default)]
    active_form: Option<String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PlannerObjectDraft {
    tasks: Vec<PlannerDraft>,
}

async fn provider_plan_drafts(
    paths: &DeadreckonPaths,
    goal: &str,
    n: u8,
    planner_provider: Option<&str>,
    cwd: &Path,
    plain: bool,
) -> Result<Vec<PlannerDraft>> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), planner_provider)?;
    let prompt = planner_prompt(goal, n);
    let request = ProviderRequest {
        prompt,
        max_output_tokens: 4096,
        cwd: Some(cwd.to_path_buf()),
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    let response =
        maybe_with_cli_wait_status(!plain, "planning child graph", router.complete(&request))
            .await?;
    parse_planner_response(&response.content)
}

fn planner_prompt(goal: &str, n: u8) -> String {
    format!(
        "You are a read-only planning agent for deadreckon. Do not write files, create temporary files, install packages, commit, delete, move, or mutate state. Inspect only if your provider supports read-only tools.\n\nReturn JSON only. Shape: {{\"tasks\":[{{\"subject\":\"imperative label\",\"goal\":\"self-contained child goal\",\"active_form\":\"present-progress text\",\"depends_on\":[\"task-0\"]}}]}}. Return exactly {n} child entries in the tasks array. Dependencies must refer to earlier child ids task-0..task-{} and form a DAG.\n\nChild hygiene:\n- Prefer child ids in execution order; earlier children should unblock later children.\n- For build/product goals, child goals must be implementation or verification slices that create or edit project files and move toward runnable behavior.\n- Do not return research-only, sourcing-only, architecture-only, or roadmap-only children unless the user explicitly asked for planning or research documentation.\n- Split independent implementation work into separate children; use research only as a dependency that directly unblocks concrete implementation.\n- Give each child enough context to run without seeing the user conversation, including likely files/modules/features and acceptance checks.\n- Never write \"based on the other worker\"; include the concrete dependency output the child will need.\n\nGoal: {goal}",
        n.saturating_sub(1)
    )
}

fn parse_planner_response(content: &str) -> Result<Vec<PlannerDraft>> {
    if let Ok(object) = serde_json::from_str::<PlannerObjectDraft>(content) {
        return Ok(object.tasks);
    }
    if let Ok(tasks) = serde_json::from_str::<Vec<PlannerDraft>>(content) {
        return Ok(tasks);
    }
    if let Some(slice) = json_slice(content, '{', '}')
        && let Ok(object) = serde_json::from_str::<PlannerObjectDraft>(slice)
    {
        return Ok(object.tasks);
    }
    if let Some(slice) = json_slice(content, '[', ']')
        && let Ok(tasks) = serde_json::from_str::<Vec<PlannerDraft>>(slice)
    {
        return Ok(tasks);
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "planner provider did not return a valid child JSON object",
        "deadreckon plan ... --planner-provider <other>",
    )))
}

fn json_slice(content: &str, open: char, close: char) -> Option<&str> {
    let start = content.find(open)?;
    let end = content.rfind(close)?;
    (end >= start).then_some(&content[start..=end])
}

fn deterministic_plan_drafts(goal: &str, n: u8) -> Vec<PlannerDraft> {
    (0..n)
        .map(|index| PlannerDraft {
            subject: match index {
                0 => "Create foundation".to_string(),
                1 => "Add behavior".to_string(),
                _ => format!("Complete slice {}", index + 1),
            },
            goal: format!("{goal} (child {} of {n})", index + 1),
            active_form: Some(match index {
                0 => "Creating foundation".to_string(),
                1 => "Adding behavior".to_string(),
                _ => format!("Completing slice {}", index + 1),
            }),
            depends_on: Vec::new(),
        })
        .collect()
}

fn infer_capability_preview(goal: &str) -> deadreckon_core::CapabilityPreview {
    let lower = goal.to_ascii_lowercase();
    let deploy = ["deploy", "vercel", "netlify", "production"]
        .iter()
        .any(|needle| lower.contains(needle));
    let global_install = ["install globally", "global install", "npm -g"]
        .iter()
        .any(|needle| lower.contains(needle));
    let networked = [
        "api",
        "websocket",
        "web socket",
        "multiplayer",
        "online",
        "networked",
        "real-time",
        "real time",
        "realtime",
        "live",
        "server",
        "client/server",
        "asset source",
        "asset sourcing",
        "terrain data",
        "mapbox",
        "cesium",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let network = if deploy || networked {
        deadreckon_core::NetworkCapability::Allowlist
    } else {
        deadreckon_core::NetworkCapability::Deny
    };
    let mut notes = Vec::new();
    if deploy {
        notes.push(
            "goal mentions deployment; require explicit capability before deploy".to_string(),
        );
    }
    if global_install {
        notes.push("goal mentions global install; require explicit capability".to_string());
    }
    deadreckon_core::CapabilityPreview {
        network,
        deploy,
        global_install,
        filesystem: vec!["working directory".to_string()],
        notes,
    }
}

fn render_worker_spec(plan: &Plan, task: &PlanTask) -> String {
    let dependencies = if task.depends_on.is_empty() {
        "none".to_string()
    } else {
        task.depends_on.join(", ")
    };
    let acceptance_line = plan
        .acceptance_path
        .as_ref()
        .map(|path| {
            format!(
                "- dr-gate will enforce configured done criteria from {}.",
                path.display()
            )
        })
        .unwrap_or_else(|| {
            "- dr-gate will use the default local gate if no project done criteria exist."
                .to_string()
        });
    format!(
        "# deadreckon worker spec: {}\n\nRoot goal: {}\nChild id: {}\nRole: {:?}\nProvider: {}\nDependencies satisfied before start: {}\n\n## Scope\n{}\n\n## Capability constraints\n- network: {:?}\n- deploy: {}\n- global install: {}\n- filesystem: {}\n\n## Coordination rules\n- Treat this file as the complete brief; do not assume access to the parent conversation.\n- Do not inspect, tail, or summarize sibling child transcripts; wait for dependency summaries included below.\n- If correcting your own failed check, keep the same context and fix the root cause.\n- If acting as reviewer, approach the artifact with fresh assumptions and verify independently.\n- Report blockers as concrete file paths, command output, or acceptance failures.\n\n## Done criteria\n{}\n- Stay within this child's scope.\n- Verify relevant behavior before reporting done.\n- Do not spawn subagents or orchestrate more children.\n- Do not editorialize between tool calls.\n- Report scope, result, key files, files changed, and issues.\n",
        task.subject,
        plan.root_goal,
        task.task_id,
        task.role,
        task.provider.as_deref().unwrap_or("config default"),
        dependencies,
        task.goal,
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install,
        plan.capability_preview.filesystem.join(", "),
        acceptance_line
    )
}

fn render_launch_worker_spec(paths: &DeadreckonPaths, plan: &Plan, task: &PlanTask) -> String {
    let mut spec = render_worker_spec(plan, task);
    let dependency_summaries = task
        .depends_on
        .iter()
        .filter_map(|dependency| plan.task_by_id(dependency))
        .filter_map(|dependency| {
            let summary_path = dependency.summary_path.as_ref()?;
            let absolute = paths.plan_dir(&plan.plan_id).join(summary_path);
            let raw = fs::read_to_string(&absolute).ok()?;
            Some((dependency, absolute, truncate_for_worker_spec(&raw)))
        })
        .collect::<Vec<_>>();
    if dependency_summaries.is_empty() {
        return spec;
    }
    spec.push_str("\n## Dependency summaries\n");
    for (dependency, absolute, summary) in dependency_summaries {
        spec.push_str(&format!(
            "\n### {} - {}\n\nSummary path: {}\n\n{}\n",
            dependency.task_id,
            dependency.subject,
            absolute.display(),
            summary.trim()
        ));
    }
    spec
}

fn truncate_for_worker_spec(raw: &str) -> String {
    const MAX_CHARS: usize = 4_000;
    let mut chars = raw.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n\n... truncated ...")
    } else {
        truncated
    }
}

fn plan_source_label(plan: &Plan) -> String {
    let Some(cwd) = plan.parent_cwd.as_ref() else {
        return "current directory at fork time".to_string();
    };
    let git_label = match preview_git_state(cwd) {
        Ok(Some(git)) => format!("git branch={} @ {}", git.branch, git.head_sha),
        _ => "not git; child runs use copy mode".to_string(),
    };
    format!("{} ({git_label})", cwd.display())
}

fn plan_acceptance_label(plan: &Plan) -> String {
    let Some(path) = plan.acceptance_path.as_ref() else {
        return setup::DoneCriteriaSelection::default_gate().full_label();
    };
    let checks = fs::read_to_string(path)
        .ok()
        .and_then(|raw| acceptance_check_count(&raw).ok());
    setup::DoneCriteriaSelection::project(path.clone(), None, checks).full_label()
}

fn plan_next_actions(plan: &Plan) -> Vec<String> {
    let id = run_prefix(&plan.plan_id);
    match plan.status {
        PlanStatus::Pending => vec![format!("deadreckon fork {id}")],
        PlanStatus::Forked => {
            if plan
                .tasks
                .iter()
                .all(|task| task.status == PlanTaskStatus::Completed)
            {
                vec![format!("deadreckon merge {id}")]
            } else {
                vec![format!("deadreckon attach {id}")]
            }
        }
        PlanStatus::Merged => vec![format!("deadreckon finish {id}")],
        PlanStatus::Failed => vec![format!("deadreckon show {id} --why-failed")],
    }
}

fn plan_paths_json(plan: &Plan) -> Value {
    let paths = DeadreckonPaths::discover();
    json!({
        "plan": paths.plan_json(&plan.plan_id),
        "events": paths.plan_events(&plan.plan_id),
        "directory": paths.plan_dir(&plan.plan_id),
    })
}

fn print_plan_json(plan: &Plan) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "kind": "plan",
            "id": &plan.plan_id,
            "status": plan_status_label(plan.status),
            "next_actions": plan_next_actions(plan),
            "try_lines": Vec::<String>::new(),
            "paths": plan_paths_json(plan),
            "plan": plan,
        }))?
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationRoleRow {
    role: String,
    route: String,
    model: String,
    source: String,
    notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationDependencyRow {
    child: String,
    status: String,
    starts: String,
    waits_for: String,
    unblocks: String,
}

fn orchestration_provider_role_rows(
    plan: &Plan,
    repair_enabled: bool,
    repair_provider: Option<&str>,
) -> Vec<OrchestrationRoleRow> {
    let mut rows = Vec::new();
    match plan.mode {
        PlanMode::FullPlan => {
            rows.push(orchestration_role_row(
                "planner",
                plan.providers.planner.as_deref(),
                "plan",
                "writes child graph",
            ));
            rows.push(orchestration_role_row(
                "default child",
                plan.providers.default_child.as_deref(),
                "plan",
                "runs ready children",
            ));
            let mut seen_overrides = BTreeSet::new();
            for (index, route) in &plan.providers.children {
                seen_overrides.insert(*index);
                rows.push(orchestration_role_row(
                    format!("child task-{index}"),
                    Some(route.as_str()),
                    "override",
                    "per-child route",
                ));
            }
            for task in &plan.tasks {
                let default_route = plan.providers.default_child.as_deref();
                if task.provider.as_deref().is_some()
                    && task.provider.as_deref() != default_route
                    && !seen_overrides.contains(&task.index)
                {
                    rows.push(orchestration_role_row(
                        format!("child {}", task.task_id),
                        task.provider.as_deref(),
                        "task",
                        one_line(&task.subject, 30),
                    ));
                }
            }
        }
        PlanMode::Review => {
            rows.push(orchestration_role_row(
                "coder",
                plan.providers.coder.as_deref(),
                "plan",
                "implementation pass",
            ));
            rows.push(orchestration_role_row(
                "reviewer",
                plan.providers.reviewer.as_deref(),
                "plan",
                "independent review",
            ));
        }
    }
    if repair_enabled {
        let derived = repair_provider
            .or(plan.providers.planner.as_deref())
            .or(plan.providers.default_child.as_deref())
            .or(plan.providers.reviewer.as_deref())
            .or(plan.providers.coder.as_deref());
        rows.push(orchestration_role_row(
            "repair",
            derived,
            if repair_provider.is_some() {
                "flag"
            } else {
                "derived"
            },
            "merge repair planning",
        ));
    } else {
        rows.push(OrchestrationRoleRow {
            role: "repair".to_string(),
            route: "disabled".to_string(),
            model: "-".to_string(),
            source: "--no-repair".to_string(),
            notes: "raw conflict refusal".to_string(),
        });
    }
    rows
}

fn orchestration_role_row(
    role: impl Into<String>,
    route: Option<&str>,
    source: impl Into<String>,
    notes: impl Into<String>,
) -> OrchestrationRoleRow {
    OrchestrationRoleRow {
        role: role.into(),
        route: route.unwrap_or("config default").to_string(),
        model: "-".to_string(),
        source: if route.is_some() {
            source.into()
        } else {
            "config".to_string()
        },
        notes: notes.into(),
    }
}

fn orchestration_role_table_lines(rows: &[OrchestrationRoleRow]) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<14} {:<22} {:<8} {:<12} {}",
        "role", "route", "model", "source", "notes"
    )];
    lines.extend(rows.iter().map(|row| {
        format!(
            "{:<14} {:<22} {:<8} {:<12} {}",
            row.role, row.route, row.model, row.source, row.notes
        )
    }));
    lines
}

fn print_orchestration_role_table(
    plan: &Plan,
    repair_enabled: bool,
    repair_provider: Option<&str>,
) {
    println!("provider roles");
    for line in orchestration_role_table_lines(&orchestration_provider_role_rows(
        plan,
        repair_enabled,
        repair_provider,
    )) {
        println!("  {line}");
    }
}

fn orchestration_dependency_rows(plan: &Plan) -> Vec<OrchestrationDependencyRow> {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .map(|task| task.task_id.as_str())
        .collect::<BTreeSet<_>>();
    plan.tasks
        .iter()
        .map(|task| {
            let blockers = task
                .depends_on
                .iter()
                .filter(|dependency| !completed.contains(dependency.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let starts = match task.status {
                PlanTaskStatus::Pending if blockers.is_empty() => "now".to_string(),
                PlanTaskStatus::Pending => format!("after {}", blockers.join(",")),
                PlanTaskStatus::Running => "already running".to_string(),
                PlanTaskStatus::Completed => "done".to_string(),
                PlanTaskStatus::Failed => "failed".to_string(),
                PlanTaskStatus::Killed => "killed".to_string(),
            };
            let unblocks = plan
                .tasks
                .iter()
                .filter(|candidate| candidate.depends_on.iter().any(|dep| dep == &task.task_id))
                .map(|candidate| candidate.task_id.clone())
                .collect::<Vec<_>>();
            OrchestrationDependencyRow {
                child: task.task_id.clone(),
                status: task_status_label(task.status).to_string(),
                starts,
                waits_for: if blockers.is_empty() {
                    "-".to_string()
                } else {
                    blockers.join(",")
                },
                unblocks: if unblocks.is_empty() {
                    "-".to_string()
                } else {
                    unblocks.join(",")
                },
            }
        })
        .collect()
}

fn orchestration_parallelism_lines(plan: &Plan) -> Vec<String> {
    let rows = orchestration_dependency_rows(plan);
    let starts_now = rows
        .iter()
        .filter(|row| row.starts == "now")
        .map(|row| row.child.clone())
        .collect::<Vec<_>>();
    let waits = rows
        .iter()
        .filter(|row| row.waits_for != "-")
        .map(|row| format!("{} after {}", row.child, row.waits_for))
        .collect::<Vec<_>>();
    vec![
        format!(
            "starts now: {}",
            if starts_now.is_empty() {
                "-".to_string()
            } else {
                starts_now.join(", ")
            }
        ),
        format!(
            "waits: {}",
            if waits.is_empty() {
                "-".to_string()
            } else {
                waits.join("; ")
            }
        ),
    ]
}

fn print_orchestration_dependency_summary(plan: &Plan) {
    println!("parallelism");
    for line in orchestration_parallelism_lines(plan) {
        println!("  {line}");
    }
    println!("dependencies");
    println!(
        "  {:<10} {:<10} {:<18} {:<18} unblocks",
        "child", "status", "starts", "waits_for"
    );
    for row in orchestration_dependency_rows(plan) {
        println!(
            "  {:<10} {:<10} {:<18} {:<18} {}",
            row.child, row.status, row.starts, row.waits_for, row.unblocks
        );
    }
}

fn print_plan_created(plan: &Plan, no_hints: bool) {
    println!(
        "{} {} ({})",
        ui_ok("plan"),
        ui_id(run_prefix(&plan.plan_id)),
        plan.plan_id
    );
    let ready = plan.ready_pending_task_indices().len();
    let pending = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Pending)
        .count();
    let blocked = pending.saturating_sub(ready);
    let children = format!(
        "{} ({} ready / {} blocked)",
        plan.tasks.len(),
        ready,
        blocked
    );
    let providers = match plan.mode {
        PlanMode::FullPlan => format!(
            "planner={} default-child={}",
            plan.providers.planner.as_deref().unwrap_or("-"),
            plan.providers.default_child.as_deref().unwrap_or("-")
        ),
        PlanMode::Review => format!(
            "coder={} reviewer={}",
            plan.providers.coder.as_deref().unwrap_or("-"),
            plan.providers.reviewer.as_deref().unwrap_or("-")
        ),
    };
    let capabilities = format!(
        "network={:?} deploy={} install={}",
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install
    );
    let source = plan_source_label(plan);
    let gate = plan_acceptance_label(plan);
    let items = [
        ("status", plan_status_label(plan.status)),
        ("mode", plan_mode_label(plan.mode)),
        ("children", children.as_str()),
        ("providers", providers.as_str()),
        ("source", source.as_str()),
        ("done criteria", gate.as_str()),
        ("capabilities", capabilities.as_str()),
    ];
    print_kv_block(&items);
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            "-".to_string()
        } else {
            task.depends_on.join(",")
        };
        println!(
            "  {} {} [{}] provider={} deps={}",
            task.task_id,
            task.subject,
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-"),
            deps
        );
    }
    if !no_hints {
        println!(
            "{} {}/plans/{}/plan.json",
            ui_command("edit:"),
            DeadreckonPaths::discover().home().display(),
            plan.plan_id
        );
        println!(
            "{} deadreckon fork {}",
            ui_command("fork:"),
            run_prefix(&plan.plan_id)
        );
    }
}

fn print_orchestrate_preflight(
    plan: &Plan,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<&str>,
    no_repair: bool,
) {
    println!(
        "{} {} ({})",
        ui_heading("orchestrate preflight"),
        ui_id(run_prefix(&plan.plan_id)),
        plan.plan_id
    );
    let children = format!(
        "{} ({})",
        plan.tasks.len(),
        orchestration_mode_summary(plan)
    );
    let spend = max_spend
        .map(|value| format!("${value:.2} per child"))
        .unwrap_or_else(|| "config default".to_string());
    let wall = max_wall_seconds
        .map(|value| format!("{value:.0}s per child"))
        .unwrap_or_else(|| "config default".to_string());
    let sandbox = sandbox.unwrap_or("config default").to_string();
    let capabilities = format!(
        "network={:?} deploy={} install={}",
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install
    );
    let providers = plan_provider_summary(plan);
    let source = plan_source_label(plan);
    let gate = plan_acceptance_label(plan);
    let repair = plan_repair_label(plan, no_repair);
    let items = [
        ("mode", plan_mode_label(plan.mode)),
        ("children", children.as_str()),
        ("providers", providers.as_str()),
        ("source", source.as_str()),
        ("done criteria", gate.as_str()),
        ("merge repair", repair.as_str()),
        ("sandbox", sandbox.as_str()),
        ("spend", spend.as_str()),
        ("wall", wall.as_str()),
        ("capabilities", capabilities.as_str()),
    ];
    print_kv_block(&items);
    print_orchestration_role_table(plan, !no_repair, None);
    print_orchestration_dependency_summary(plan);
    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            "-".to_string()
        } else {
            task.depends_on.join(",")
        };
        println!(
            "  {} {} [{}] provider={} deps={}",
            task.task_id,
            task.subject,
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-"),
            deps
        );
    }
    let warnings = implementation_plan_warnings(plan);
    if !warnings.is_empty() {
        println!("{}", ui_warn("preflight warnings"));
        for warning in warnings {
            println!("  - {warning}");
        }
        println!(
            "  {} {}",
            ui_command("try:"),
            ui_command(format!(
                "deadreckon attach {} --plain",
                run_prefix(&plan.plan_id)
            ))
        );
    }
    println!(
        "{} {}/plans/{}/plan.json",
        ui_command("plan:"),
        DeadreckonPaths::discover().home().display(),
        plan.plan_id
    );
}

fn print_orchestrate_started(
    plan: &Plan,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<&str>,
    no_repair: bool,
) {
    println!(
        "{} {}",
        ui_ok("started orchestration"),
        ui_id(format!("{} ({})", run_prefix(&plan.plan_id), plan.plan_id))
    );
    let children = plan.tasks.len().to_string();
    let providers = plan_provider_summary(plan);
    let source = plan_source_label(plan);
    let gate = plan_acceptance_label(plan);
    let repair = plan_repair_label(plan, no_repair);
    let sandbox = sandbox.unwrap_or("config default").to_string();
    let spend = max_spend
        .map(|value| format!("${value:.2} per child"))
        .unwrap_or_else(|| "config default".to_string());
    let wall = max_wall_seconds
        .map(|value| format!("{value:.0}s per child"))
        .unwrap_or_else(|| "config default".to_string());
    let paths = DeadreckonPaths::discover();
    let plan_path = paths.plan_json(&plan.plan_id);
    let plan_path_display = plan_path.to_string_lossy().to_string();
    let events_path_display = paths
        .plan_events(&plan.plan_id)
        .to_string_lossy()
        .to_string();
    let items = [
        ("mode", plan_mode_label(plan.mode)),
        ("children", children.as_str()),
        ("providers", providers.as_str()),
        ("source", source.as_str()),
        ("done criteria", gate.as_str()),
        ("merge repair", repair.as_str()),
        ("sandbox", sandbox.as_str()),
        ("spend", spend.as_str()),
        ("wall", wall.as_str()),
        ("plan", plan_path_display.as_str()),
        ("events", events_path_display.as_str()),
    ];
    print_kv_block(&items);
    print_orchestration_role_table(plan, !no_repair, None);
    print_orchestration_dependency_summary(plan);
    println!(
        "{} {}",
        ui_command("attach:"),
        ui_command(format!("deadreckon attach {}", run_prefix(&plan.plan_id)))
    );
    println!(
        "{} {}",
        ui_command("show:"),
        ui_command(format!("deadreckon show {}", run_prefix(&plan.plan_id)))
    );
    println!(
        "{} {}",
        ui_command("child:"),
        ui_command(format!(
            "deadreckon attach {}:task-0",
            run_prefix(&plan.plan_id)
        ))
    );
    println!(
        "{} {}",
        ui_command("when done:"),
        ui_command(format!("deadreckon finish {}", run_prefix(&plan.plan_id)))
    );
    println!(
        "{} {}",
        ui_command("history:"),
        ui_command(format!(
            "deadreckon history grep <pattern> --plan {}",
            run_prefix(&plan.plan_id)
        ))
    );
    let _ = io::stdout().flush();
}

fn implementation_plan_warnings(plan: &Plan) -> Vec<String> {
    if plan.mode != PlanMode::FullPlan || user_requested_planning(&plan.root_goal) {
        return Vec::new();
    }
    let weak_tasks = plan
        .tasks
        .iter()
        .filter(|task| task_looks_non_implementation(task))
        .map(|task| task.task_id.clone())
        .collect::<Vec<_>>();
    if weak_tasks.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "{} task(s) look research/design/roadmap-only for a build goal: {}. Preview/edit/re-plan before starting if these should build working software.",
        weak_tasks.len(),
        weak_tasks.join(", ")
    )]
}

fn user_requested_planning(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    ["research", "plan", "roadmap", "architecture", "design doc"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn task_looks_non_implementation(task: &PlanTask) -> bool {
    let text = format!("{} {}", task.subject, task.goal).to_ascii_lowercase();
    let planning_terms = [
        "research",
        "source ",
        "sourcing",
        "architecture",
        "design ",
        "roadmap",
        "document",
        "decision record",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    let implementation_terms = [
        "implement ",
        "build",
        "create",
        "add",
        "wire",
        "test",
        "verify",
        "fix",
        "scaffold",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    planning_terms && !implementation_terms
}

fn orchestration_mode_summary(plan: &Plan) -> &'static str {
    match plan.mode {
        PlanMode::FullPlan => "planner -> children -> merge -> final gate",
        PlanMode::Review => "coder -> reviewer/fixer -> final gate",
    }
}

fn confirm_orchestration_start(plan: &Plan, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        let command = match plan.mode {
            PlanMode::FullPlan => {
                "deadreckon orchestrate full-plan \"goal\" --planner-provider cli:codex --provider cli:claude-code --yes"
            }
            PlanMode::Review => {
                "deadreckon orchestrate review \"goal\" --coder-provider cli:claude-code --reviewer-provider cli:codex --yes"
            }
        };
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive orchestrate requires --yes after reviewing preflight",
            command,
        )));
    }
    if !prompt::confirm("start this orchestration?", true)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "orchestration cancelled by user".to_string(),
        )));
    }
    Ok(())
}

async fn fork_command(args: ForkCommandArgs) -> Result<()> {
    let ForkCommandArgs {
        plan_id,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        child_provider,
        coder_provider,
        reviewer_provider,
        no_hints,
        quiet,
        plain,
    } = args;
    let paths = DeadreckonPaths::discover();
    let resolved_id = resolve_plan_id(&paths, &plan_id)?;
    let mut plan = load_plan(&paths, &resolved_id)?;
    if plan.status != PlanStatus::Pending {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} is {}",
                run_prefix(&plan.plan_id),
                plan_status_label(plan.status)
            ),
            match plan.status {
                PlanStatus::Forked => "deadreckon merge <plan-id>",
                PlanStatus::Merged => "deadreckon finish <plan-id>",
                PlanStatus::Failed => "deadreckon show <plan-id> --why-failed",
                PlanStatus::Pending => "deadreckon fork <plan-id>",
            },
        )));
    }
    apply_fork_provider_overrides(
        &mut plan,
        provider,
        &child_provider,
        coder_provider,
        reviewer_provider,
    )?;
    let defaults = config_defaults(&paths)?;
    let sandbox = sandbox
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    let parent_cwd = std::env::current_dir()?;

    plan.status = PlanStatus::Forked;
    plan.forked_at = Some(Utc::now());
    save_plan(&paths, &plan)?;
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)?;
    write_coordinator_snapshot(&paths, &plan, None)?;

    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        let ready = plan.ready_pending_task_indices();
        if !ready.is_empty() {
            made_progress = true;
        }
        for &task_index in &ready {
            let task_id = plan.tasks[task_index].task_id.clone();
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::TaskReady {
                    task_id: task_id.clone(),
                    task_index,
                },
            )?;
            mark_plan_task_status(&mut plan, task_index, PlanTaskStatus::Running)?;
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::TaskStarted {
                    task_id: task_id.clone(),
                    task_index,
                },
            )?;
            append_plan_message(
                &paths,
                &plan.plan_id,
                &PlanMessage::new(
                    "coordinator",
                    &task_id,
                    PlanMessageKind::Progress,
                    format!("{task_id} started"),
                    json!({ "task_index": task_index }),
                )?,
            )?;
        }
        if ready.is_empty() {
            continue;
        }
        save_plan(&paths, &plan)?;
        write_coordinator_snapshot(&paths, &plan, None)?;

        let (pid_tx, pid_rx) = std::sync::mpsc::channel::<(usize, u32)>();
        let mut handles = Vec::new();
        for task_index in ready {
            let paths_for_child = paths.clone();
            let plan_for_child = plan.clone();
            let parent_cwd_for_child = parent_cwd.clone();
            let sandbox_for_child = sandbox.clone();
            let pid_tx_for_child = pid_tx.clone();
            handles.push((
                task_index,
                tokio::task::spawn_blocking(move || {
                    run_plan_child(PlanChildLaunch {
                        paths: &paths_for_child,
                        plan: &plan_for_child,
                        task_index,
                        parent_cwd: &parent_cwd_for_child,
                        sandbox: &sandbox_for_child,
                        max_spend,
                        max_wall_seconds,
                        quiet,
                        plain,
                        pid_sender: Some(pid_tx_for_child),
                    })
                }),
            ));
        }
        drop(pid_tx);
        let mut live_children = BTreeMap::new();
        while live_children.len() < handles.len() {
            match pid_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok((task_index, pid)) => {
                    live_children.insert(task_index, pid);
                    if let Some(task) = plan.tasks.get(task_index) {
                        append_plan_event(
                            &paths,
                            &plan.plan_id,
                            PlanEventKind::TaskRunDiscovered {
                                task_id: task.task_id.clone(),
                                task_index,
                                run_id: task.child_run_id.clone(),
                                pid: Some(pid),
                            },
                        )?;
                    }
                    write_coordinator_snapshot_live(&paths, &plan, &live_children)?;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        for (task_index, handle) in handles {
            let outcome = handle.await.map_err(|err| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "child join failed: {err}"
                )))
            })?;
            let task_id = plan.tasks[task_index].task_id.clone();
            match outcome {
                Ok(run_id) => {
                    let state = load_run(&paths, &run_id)?;
                    let status = plan_status_from_run_status(state.status);
                    let summary =
                        summarize_child_run(&paths, &plan, &plan.tasks[task_index], &state);
                    write_child_summary(&paths, &plan.plan_id, &task_id, &summary)?;
                    let marker = plan_child_marker(&paths, &plan, &plan.tasks[task_index], &state);
                    write_plan_child_marker(&state.working_dir, &marker)?;
                    let library_dir = paths.library_dir(&state.scope, &state.run_id);
                    if library_dir.is_dir() {
                        write_plan_child_marker(&library_dir, &marker)?;
                    }
                    {
                        let task = &mut plan.tasks[task_index];
                        task.child_run_id = Some(run_id.clone());
                        task.child_scope = Some(state.scope.clone());
                        task.summary_path =
                            Some(deadreckon_core::child_summary_relative_path(&task.task_id));
                        task.status = status;
                    }
                    append_plan_event(
                        &paths,
                        &plan.plan_id,
                        PlanEventKind::TaskRunDiscovered {
                            task_id: task_id.clone(),
                            task_index,
                            run_id: Some(run_id.clone()),
                            pid: live_children.get(&task_index).copied(),
                        },
                    )?;
                    append_task_terminal_plan_event(&paths, &plan, task_index, status, &run_id)?;
                    append_plan_message(
                        &paths,
                        &plan.plan_id,
                        &PlanMessage::new(
                            "coordinator",
                            &task_id,
                            if status == PlanTaskStatus::Completed {
                                PlanMessageKind::Progress
                            } else {
                                PlanMessageKind::Blocker
                            },
                            format!("{task_id} {}", task_status_label(status)),
                            json!({
                                "task_index": task_index,
                                "run_id": run_id,
                                "run_status": state.status.to_string(),
                            }),
                        )?,
                    )?;
                    save_plan(&paths, &plan)?;
                    write_coordinator_snapshot(&paths, &plan, None)?;
                }
                Err(error) => {
                    mark_plan_task_status(&mut plan, task_index, PlanTaskStatus::Failed)?;
                    append_plan_event(
                        &paths,
                        &plan.plan_id,
                        PlanEventKind::TaskFailed {
                            task_id: task_id.clone(),
                            task_index,
                            reason: error.to_string(),
                        },
                    )?;
                    append_plan_message(
                        &paths,
                        &plan.plan_id,
                        &PlanMessage::new(
                            "coordinator",
                            &task_id,
                            PlanMessageKind::Blocker,
                            format!("{task_id} failed"),
                            json!({ "task_index": task_index, "error": error.to_string() }),
                        )?,
                    )?;
                    save_plan(&paths, &plan)?;
                    write_coordinator_snapshot(&paths, &plan, None)?;
                }
            }
        }
    }

    mark_blocked_pending_tasks(&paths, &mut plan)?;
    mark_failed_fork_plan_terminal(&paths, &mut plan)?;
    save_plan(&paths, &plan)?;
    let _ = fs::remove_file(paths.coordinator_json(&plan.plan_id));
    if !quiet {
        print_fork_finished(&plan, no_hints);
    }
    Ok(())
}

fn resolve_plan_id(paths: &DeadreckonPaths, id: &str) -> Result<String> {
    let mut plans = fs::read_dir(paths.plans_dir())
        .map_err(|source| DeadreckonError::Io {
            path: paths.plans_dir(),
            source,
        })?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join("plan.json").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect::<Vec<_>>();
    plans.sort();
    if matches!(id, "latest" | "last") {
        plans.sort_by_key(|plan_id| {
            fs::metadata(paths.plan_json(plan_id))
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        return plans.last().cloned().ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "no plans",
                "deadreckon plan \"your goal\"",
            ))
        });
    }
    let matches = plans
        .into_iter()
        .filter(|plan_id| plan_id.starts_with(id))
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(CliError::Core(deadreckon_core::user_error(
            &format!("no plan {id}"),
            "deadreckon plan \"your goal\"",
        ))),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous plan id prefix {id}; matches {}",
                matches.join(", ")
            ),
            "use a longer plan id prefix",
        ))),
    }
}

#[derive(Debug, Clone)]
struct PlanResultRun {
    plan: Plan,
    state: deadreckon_core::PipelineState,
}

fn resolve_plan_result_run(
    paths: &DeadreckonPaths,
    id: &str,
    verb: &str,
) -> Result<Option<PlanResultRun>> {
    let plan_id = match resolve_plan_id(paths, id) {
        Ok(plan_id) => plan_id,
        Err(error) if error.to_string().contains("no plan") => return Ok(None),
        Err(error) => return Err(error),
    };
    let plan = load_plan(paths, &plan_id)?;
    let Some(run_id) = merged_run_id_for_completed_plan(&plan, verb)? else {
        return Ok(None);
    };
    let state = load_run(paths, &run_id)?;
    Ok(Some(PlanResultRun { plan, state }))
}

fn merged_run_id_for_completed_plan(plan: &Plan, verb: &str) -> Result<Option<String>> {
    match plan.status {
        PlanStatus::Merged => {}
        PlanStatus::Pending => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("plan {} has not started yet", run_prefix(&plan.plan_id)),
                &format!("deadreckon fork {}", run_prefix(&plan.plan_id)),
            )));
        }
        PlanStatus::Forked => {
            let ready_to_merge = plan
                .tasks
                .iter()
                .all(|task| task.status == PlanTaskStatus::Completed);
            let try_line = if ready_to_merge {
                format!("deadreckon merge {}", run_prefix(&plan.plan_id))
            } else {
                format!("deadreckon attach {}", run_prefix(&plan.plan_id))
            };
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "plan {} is still {}; cannot {verb} it yet",
                    run_prefix(&plan.plan_id),
                    plan_status_label(plan.status)
                ),
                &try_line,
            )));
        }
        PlanStatus::Failed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "plan {} failed; no completed result to {verb}",
                    run_prefix(&plan.plan_id)
                ),
                &format!("deadreckon show {} --why-failed", run_prefix(&plan.plan_id)),
            )));
        }
    }
    let run_id = plan.merged_run_id.clone().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} is completed but has no result run id",
                run_prefix(&plan.plan_id)
            ),
            &format!("deadreckon show {}", run_prefix(&plan.plan_id)),
        ))
    })?;
    Ok(Some(run_id))
}

fn default_plan_materialize_dest(plan: &Plan) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(
            deadreckon_core::paths::task_key(&plan.root_goal)
                .chars()
                .take(24)
                .collect::<String>(),
        )
}

fn plan_apply_git_root(plan: &Plan) -> Result<Option<PathBuf>> {
    let Some(parent_cwd) = plan.parent_cwd.as_ref() else {
        return Ok(None);
    };
    deadreckon_core::find_git_root(parent_cwd).map_err(CliError::from)
}

fn print_plan_result_context(plan: &Plan, state: &deadreckon_core::PipelineState) {
    println!(
        "{} {} -> secondary run {}",
        ui_heading("plan result:"),
        ui_id(run_prefix(&plan.plan_id)),
        ui_id(run_prefix(&state.run_id))
    );
}

#[derive(Debug, Clone, Serialize)]
struct PlanChildSelection {
    plan_id: String,
    task_id: String,
    run_id: String,
}

fn resolve_plan_child_ref(paths: &DeadreckonPaths, id: &str) -> Result<Option<PlanChildSelection>> {
    let Some((plan_ref, child_ref)) = id
        .split_once(':')
        .or_else(|| id.split_once('/'))
        .map(|(plan_ref, child_ref)| (plan_ref.trim(), child_ref.trim()))
    else {
        return Ok(None);
    };
    if plan_ref.is_empty() || child_ref.is_empty() {
        return Ok(None);
    }
    let plan_id = resolve_plan_id(paths, plan_ref)?;
    let plan = load_plan(paths, &plan_id)?;
    let task = resolve_plan_child_task(&plan, child_ref).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} has no child {child_ref}",
                run_prefix(&plan.plan_id)
            ),
            &format!("deadreckon show {}", run_prefix(&plan.plan_id)),
        ))
    })?;
    let run_id = task.child_run_id.clone().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "{} in plan {} has no run id yet",
                task.task_id,
                run_prefix(&plan.plan_id)
            ),
            &format!("deadreckon attach {}", run_prefix(&plan.plan_id)),
        ))
    })?;
    Ok(Some(PlanChildSelection {
        plan_id: plan.plan_id.clone(),
        task_id: task.task_id.clone(),
        run_id,
    }))
}

fn resolve_plan_child_task<'a>(plan: &'a Plan, child_ref: &str) -> Option<&'a PlanTask> {
    if let Some(task) = plan.task_by_id(child_ref) {
        return Some(task);
    }
    let normalized = child_ref.strip_prefix("task-").unwrap_or(child_ref);
    normalized
        .parse::<u32>()
        .ok()
        .and_then(|index| plan.tasks.iter().find(|task| task.index == index))
}

fn apply_fork_provider_overrides(
    plan: &mut Plan,
    provider: Option<String>,
    child_provider: &[String],
    coder_provider: Option<String>,
    reviewer_provider: Option<String>,
) -> Result<()> {
    match plan.mode {
        PlanMode::FullPlan => {
            if let Some(provider) = provider {
                plan.providers.default_child = Some(provider.clone());
                for task in &mut plan.tasks {
                    task.provider = Some(provider.clone());
                }
            }
            let overrides = parse_child_provider_overrides(child_provider, plan.n as u8)?;
            for (index, provider) in overrides {
                plan.providers.children.insert(index, provider.clone());
                let task = plan.tasks.get_mut(index as usize).ok_or_else(|| {
                    CliError::Core(deadreckon_core::user_error(
                        &format!("child provider index {index} outside 0..{}", plan.n),
                        "--child-provider 1=cli:codex",
                    ))
                })?;
                task.provider = Some(provider);
            }
        }
        PlanMode::Review => {
            if let Some(provider) = coder_provider {
                plan.providers.coder = Some(provider.clone());
                if let Some(task) = plan
                    .tasks
                    .iter_mut()
                    .find(|task| task.role == PlanRole::Coder)
                {
                    task.provider = Some(provider);
                }
            }
            if let Some(provider) = reviewer_provider {
                plan.providers.reviewer = Some(provider.clone());
                if let Some(task) = plan
                    .tasks
                    .iter_mut()
                    .find(|task| task.role == PlanRole::Reviewer)
                {
                    task.provider = Some(provider);
                }
            }
        }
    }
    Ok(())
}

fn append_task_terminal_plan_event(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task_index: usize,
    status: PlanTaskStatus,
    run_id: &str,
) -> Result<()> {
    let task = plan.tasks.get(task_index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("no child index {task_index}"),
            "deadreckon plan \"your goal\"",
        ))
    })?;
    let event = match status {
        PlanTaskStatus::Completed => PlanEventKind::TaskCompleted {
            task_id: task.task_id.clone(),
            task_index,
            run_id: Some(run_id.to_string()),
            status: "completed".to_string(),
        },
        PlanTaskStatus::Failed => PlanEventKind::TaskFailed {
            task_id: task.task_id.clone(),
            task_index,
            reason: format!("run {run_id} failed"),
        },
        PlanTaskStatus::Killed => PlanEventKind::TaskKilled {
            task_id: task.task_id.clone(),
            task_index,
            run_id: Some(run_id.to_string()),
        },
        PlanTaskStatus::Pending | PlanTaskStatus::Running => PlanEventKind::TaskBlocked {
            task_id: task.task_id.clone(),
            task_index,
            reason: format!("run {run_id} ended {}", task_status_label(status)),
        },
    };
    append_plan_event(paths, &plan.plan_id, event)?;
    Ok(())
}

fn mark_plan_task_status(plan: &mut Plan, task_index: usize, status: PlanTaskStatus) -> Result<()> {
    let task = plan.tasks.get_mut(task_index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("no child index {task_index}"),
            "deadreckon plan \"your goal\"",
        ))
    })?;
    task.status = status;
    Ok(())
}

fn write_coordinator_snapshot(
    paths: &DeadreckonPaths,
    plan: &Plan,
    live_child: Option<(usize, u32)>,
) -> Result<()> {
    let live_children = live_child.into_iter().collect::<BTreeMap<_, _>>();
    write_coordinator_snapshot_live(paths, plan, &live_children)
}

fn write_coordinator_snapshot_live(
    paths: &DeadreckonPaths,
    plan: &Plan,
    live_children: &BTreeMap<usize, u32>,
) -> Result<()> {
    let children = plan
        .tasks
        .iter()
        .enumerate()
        .map(|(index, task)| CoordinatorChild {
            child_index: task.index,
            task_id: task.task_id.clone(),
            run_id: task.child_run_id.clone(),
            pid: live_children.get(&index).copied(),
            scope: task.child_scope.clone(),
            provider: task.provider.clone(),
            role: task.role,
            status: task.status,
        })
        .collect::<Vec<_>>();
    write_coordinator_state(
        paths,
        &plan.plan_id,
        &CoordinatorState {
            schema_version: 1,
            plan_id: plan.plan_id.clone(),
            coordinator_pid: std::process::id(),
            started_at: plan.forked_at.unwrap_or_else(Utc::now),
            children,
        },
    )?;
    Ok(())
}

fn plan_child_source_dir(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task_index: usize,
    parent_cwd: &Path,
) -> Result<PathBuf> {
    let task = &plan.tasks[task_index];
    let plan_cwd = plan.parent_cwd.as_deref().unwrap_or(parent_cwd);
    if task.depends_on.is_empty() {
        return Ok(plan_cwd.to_path_buf());
    }

    let dependencies = plan_dependency_artifacts(paths, plan, task)?;
    if task.role == PlanRole::Reviewer && dependencies.len() == 1 {
        Ok(dependencies[0].root.clone())
    } else {
        compose_dependency_source_dir(paths, plan, task, &dependencies)
    }
}

#[derive(Debug, Clone)]
struct PlanDependencyArtifact {
    task_id: String,
    index: u32,
    root: PathBuf,
}

#[derive(Debug, Clone)]
struct DependencySourceFile {
    task_id: String,
    hash: u64,
}

fn plan_dependency_artifacts(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
) -> Result<Vec<PlanDependencyArtifact>> {
    let mut dependencies = Vec::new();
    for dependency in &task.depends_on {
        let dependency_task = plan.task_by_id(dependency).ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                &format!("task {} depends on unknown {dependency}", task.task_id),
                "edit the plan so depends_on references earlier task ids",
            ))
        })?;
        if dependency_task.status != PlanTaskStatus::Completed {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "task {} dependency {} is {}",
                    task.task_id,
                    dependency_task.task_id,
                    task_status_label(dependency_task.status)
                ),
                "wait for dependencies to complete before forking dependent children",
            )));
        }
        let run_id = dependency_task.child_run_id.as_deref().ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                &format!(
                    "task {} dependency {} has no run id",
                    task.task_id, dependency
                ),
                "deadreckon fork <plan-id>",
            ))
        })?;
        let state = load_run(paths, run_id)?;
        dependencies.push(PlanDependencyArtifact {
            task_id: dependency_task.task_id.clone(),
            index: dependency_task.index,
            root: child_artifact_root(paths, &state),
        });
    }
    dependencies.sort_by_key(|dependency| dependency.index);
    Ok(dependencies)
}

fn compose_dependency_source_dir(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    dependencies: &[PlanDependencyArtifact],
) -> Result<PathBuf> {
    let source_dir = paths
        .plan_dir(&plan.plan_id)
        .join("launch")
        .join(&task.task_id)
        .join("source");
    remove_if_exists(&source_dir)?;
    fs::create_dir_all(&source_dir)?;

    let mut seen = BTreeMap::<PathBuf, DependencySourceFile>::new();
    for dependency in dependencies {
        for file in inventory_files(&dependency.root)? {
            let relative = file.strip_prefix(&dependency.root).map_err(|err| {
                DeadreckonError::InvalidInput(format!("dependency source prefix error: {err}"))
            })?;
            if skip_plan_merge_file(relative) {
                continue;
            }
            let hash = file_hash(&file)?;
            match seen.get(relative).cloned() {
                Some(previous) if previous.hash == hash => {}
                Some(previous)
                    if plan_task_depends_on(plan, &dependency.task_id, &previous.task_id) =>
                {
                    copy_merge_file(&file, &source_dir.join(relative))?;
                    seen.insert(
                        relative.to_path_buf(),
                        DependencySourceFile {
                            task_id: dependency.task_id.clone(),
                            hash,
                        },
                    );
                }
                Some(previous)
                    if plan_task_depends_on(plan, &previous.task_id, &dependency.task_id) => {}
                Some(previous) => {
                    return Err(CliError::Core(deadreckon_core::user_error(
                        &format!(
                            "dependency source conflict at {} between {} and {}",
                            relative.display(),
                            previous.task_id,
                            dependency.task_id
                        ),
                        "split the tasks so only one dependency owns that file, or merge them before the dependent child",
                    )));
                }
                None => {
                    copy_merge_file(&file, &source_dir.join(relative))?;
                    seen.insert(
                        relative.to_path_buf(),
                        DependencySourceFile {
                            task_id: dependency.task_id.clone(),
                            hash,
                        },
                    );
                }
            }
        }
    }
    Ok(source_dir)
}

fn plan_task_depends_on(plan: &Plan, task_id: &str, dependency_id: &str) -> bool {
    let mut stack = vec![task_id.to_string()];
    let mut seen = BTreeSet::new();
    while let Some(next) = stack.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let Some(task) = plan.task_by_id(&next) else {
            continue;
        };
        if task
            .depends_on
            .iter()
            .any(|dependency| dependency == dependency_id)
        {
            return true;
        }
        stack.extend(task.depends_on.iter().cloned());
    }
    false
}

struct PlanChildLaunch<'a> {
    paths: &'a DeadreckonPaths,
    plan: &'a Plan,
    task_index: usize,
    parent_cwd: &'a Path,
    sandbox: &'a str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    quiet: bool,
    plain: bool,
    pid_sender: Option<std::sync::mpsc::Sender<(usize, u32)>>,
}

fn run_plan_child(launch: PlanChildLaunch<'_>) -> Result<String> {
    let PlanChildLaunch {
        paths,
        plan,
        task_index,
        parent_cwd,
        sandbox,
        max_spend,
        max_wall_seconds,
        quiet,
        plain,
        pid_sender,
    } = launch;
    let task = &plan.tasks[task_index];
    let source_dir = plan_child_source_dir(paths, plan, task_index, parent_cwd)?;
    let worker_spec_path = paths.worker_spec(&plan.plan_id, &task.task_id);
    let worker_spec = render_launch_worker_spec(paths, plan, task);
    write_worker_spec(paths, &plan.plan_id, &task.task_id, &worker_spec)?;
    let prompt = plan_child_prompt(plan, task, &worker_spec, &worker_spec_path);
    let launch_dir = paths
        .plan_dir(&plan.plan_id)
        .join("launch")
        .join(&task.task_id);
    fs::create_dir_all(&launch_dir)?;

    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&source_dir)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_HINTS", "0")
        .env("DEADRECKON_SCOPE_ROOT", &launch_dir);
    let review_parent_run_id = review_parent_run_id(plan, task);
    if let Some(parent_run_id) = review_parent_run_id.as_deref() {
        command
            .arg("extend")
            .arg(parent_run_id)
            .arg(prompt)
            .arg("--no-docs");
    } else {
        command
            .arg("run")
            .arg(prompt)
            .arg("--from")
            .arg(&source_dir)
            .arg("--yes")
            .arg("--no-confirm")
            .arg("--no-hints")
            .arg("--no-docs");
        if plain {
            command.arg("--plain");
        }
        if let Some(acceptance_path) = plan.acceptance_path.as_deref() {
            command.arg("--acceptance").arg(acceptance_path);
        }
    }
    command.arg("--sandbox").arg(sandbox);
    if let Some(max_spend) = max_spend {
        command.arg("--max-spend").arg(format!("{max_spend:.6}"));
    }
    if let Some(max_wall_seconds) = max_wall_seconds {
        command
            .arg("--max-wall-seconds")
            .arg(max_wall_seconds.to_string());
    }
    if task
        .provider
        .as_deref()
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        if review_parent_run_id.is_some() {
            command.arg("--provider").arg("smoke");
        } else {
            command.arg("--smoke");
        }
    } else if let Some(provider) = task.provider.as_deref() {
        command.arg("--provider").arg(provider);
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(sender) = pid_sender {
        let _ = sender.send((task_index, child.id()));
    } else {
        write_coordinator_snapshot(paths, plan, Some((task_index, child.id())))?;
    }
    let stdout = child.stdout.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture child stdout".to_string(),
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture child stderr".to_string(),
        ))
    })?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let stdout_thread = spawn_chain_step_reader(stdout, true, tx.clone());
    let stderr_thread = spawn_chain_step_reader(stderr, false, tx);
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut live_run_id = None;
    let status = loop {
        while let Ok((is_stdout, line)) = rx.try_recv() {
            if let Some(run_id) = capture_chain_step_output(
                is_stdout,
                &line,
                &mut stdout_text,
                &mut stderr_text,
                quiet,
            )? {
                let _ = fs::write(launch_dir.join("run-id"), &run_id);
                live_run_id = Some(run_id);
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok((is_stdout, line)) = rx.try_recv() {
        if let Some(run_id) =
            capture_chain_step_output(is_stdout, &line, &mut stdout_text, &mut stderr_text, quiet)?
        {
            let _ = fs::write(launch_dir.join("run-id"), &run_id);
            live_run_id = Some(run_id);
        }
    }
    let run_id = live_run_id.or_else(|| parse_started_run_id(&stdout_text));
    if let Some(run_id) = run_id.as_ref() {
        let _ = fs::write(launch_dir.join("run-id"), run_id);
    }
    if !status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "child {} failed: {}{}",
            task.task_id, stdout_text, stderr_text
        ))));
    }
    run_id.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("could not find run id for child {}", task.task_id),
            "deadreckon list",
        ))
    })
}

fn review_parent_run_id(plan: &Plan, task: &PlanTask) -> Option<String> {
    if task.role != PlanRole::Reviewer {
        return None;
    }
    task.depends_on
        .first()
        .and_then(|dependency| plan.task_by_id(dependency))
        .and_then(|parent_task| parent_task.child_run_id.clone())
}

fn plan_child_prompt(plan: &Plan, task: &PlanTask, spec: &str, spec_path: &Path) -> String {
    let role_note = match task.role {
        PlanRole::Reviewer => {
            "This is a fresh review/fix lane. Write .deadreckon/REVIEW.md first, then apply only fixes tied to findings and acceptance."
        }
        PlanRole::Coder => "This is the coding lane for review-mode orchestration.",
        PlanRole::Child => "This is one full-plan child run in a larger plan.",
    };
    format!(
        "{role_note}\n\nRoot goal: {}\nPlan: {}\nTask: {}\nWorker spec path: {}\n\n{}\n",
        plan.root_goal,
        plan.plan_id,
        task.task_id,
        spec_path.display(),
        spec
    )
}

fn plan_status_from_run_status(status: RunStatus) -> PlanTaskStatus {
    match status {
        RunStatus::Completed => PlanTaskStatus::Completed,
        RunStatus::Killed => PlanTaskStatus::Killed,
        RunStatus::Failed => PlanTaskStatus::Failed,
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => PlanTaskStatus::Running,
    }
}

fn summarize_child_run(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    state: &deadreckon_core::PipelineState,
) -> String {
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let files = inventory_files(&state.working_dir).unwrap_or_default();
    let file_lines = files
        .iter()
        .take(20)
        .map(|file| format!("- {}", file.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Child Summary: {}\n\nPlan: {}\nTask: {}\nRole: {:?}\nProvider: {}\nRun: {}\nStatus: {}\nWorking: {}\nLibrary: {}\n\n## Goal\n\n{}\n\n## Files\n\n{}\n",
        task.subject,
        plan.plan_id,
        task.task_id,
        task.role,
        task.provider.as_deref().unwrap_or("config default"),
        state.run_id,
        state.status,
        state.working_dir.display(),
        library_dir.display(),
        task.goal,
        if file_lines.is_empty() {
            "- no files recorded".to_string()
        } else {
            file_lines
        }
    )
}

fn plan_child_marker(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    state: &deadreckon_core::PipelineState,
) -> PlanChildMarker {
    PlanChildMarker {
        schema_version: 1,
        kind: "plan_child".to_string(),
        parent_plan_id: plan.plan_id.clone(),
        parent_scope: plan
            .parent_scope
            .clone()
            .unwrap_or_else(|| state.scope.clone()),
        parent_goal: plan.root_goal.clone(),
        task_id: task.task_id.clone(),
        child_index: task.index,
        task_goal: task.goal.clone(),
        worker_spec: paths.worker_spec(&plan.plan_id, &task.task_id),
        provider: task.provider.clone(),
        role: task.role,
        created_at: Utc::now(),
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn mark_blocked_pending_tasks(paths: &DeadreckonPaths, plan: &mut Plan) -> Result<()> {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .map(|task| task.task_id.clone())
        .collect::<BTreeSet<_>>();
    let blockers = plan
        .tasks
        .iter()
        .filter(|task| task.status != PlanTaskStatus::Completed)
        .map(|task| task.task_id.clone())
        .collect::<BTreeSet<_>>();
    let pending = plan
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| task.status == PlanTaskStatus::Pending)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in pending {
        let missing = plan.tasks[index]
            .depends_on
            .iter()
            .filter(|dependency| !completed.contains(*dependency))
            .cloned()
            .collect::<Vec<_>>();
        let blocked_by = missing
            .iter()
            .filter(|dependency| blockers.contains(*dependency))
            .cloned()
            .collect::<Vec<_>>();
        let task_id = plan.tasks[index].task_id.clone();
        plan.tasks[index].status = PlanTaskStatus::Failed;
        append_plan_event(
            paths,
            &plan.plan_id,
            PlanEventKind::TaskBlocked {
                task_id: task_id.clone(),
                task_index: index,
                reason: format!(
                    "missing dependencies: {}",
                    if missing.is_empty() {
                        "unknown".to_string()
                    } else {
                        missing.join(", ")
                    }
                ),
            },
        )?;
        append_plan_message(
            paths,
            &plan.plan_id,
            &PlanMessage::new(
                "coordinator",
                &task_id,
                PlanMessageKind::Blocker,
                format!("{task_id} blocked"),
                json!({ "missing_dependencies": missing, "blocked_by": blocked_by }),
            )?,
        )?;
    }
    Ok(())
}

fn mark_failed_fork_plan_terminal(paths: &DeadreckonPaths, plan: &mut Plan) -> Result<()> {
    let all_terminal = plan.tasks.iter().all(|task| {
        !matches!(
            task.status,
            PlanTaskStatus::Pending | PlanTaskStatus::Running
        )
    });
    let has_failure = plan
        .tasks
        .iter()
        .any(|task| matches!(task.status, PlanTaskStatus::Failed | PlanTaskStatus::Killed));
    if all_terminal && has_failure && plan.status != PlanStatus::Failed {
        plan.status = PlanStatus::Failed;
        append_plan_event(
            paths,
            &plan.plan_id,
            PlanEventKind::PlanFailed {
                reason: "one or more child tasks failed or were blocked".to_string(),
            },
        )?;
    }
    Ok(())
}

fn task_status_label(status: PlanTaskStatus) -> &'static str {
    plan_task_status_label(status)
}

fn plan_mode_label(mode: PlanMode) -> &'static str {
    match mode {
        PlanMode::FullPlan => "full-plan",
        PlanMode::Review => "review",
    }
}

fn print_fork_finished(plan: &Plan, no_hints: bool) {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    println!(
        "{} {} done with {}/{} completed",
        ui_ok("forked"),
        ui_id(run_prefix(&plan.plan_id)),
        completed,
        plan.tasks.len()
    );
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
    if !no_hints {
        println!(
            "{} {}",
            ui_command("attach:"),
            ui_command(format!("deadreckon attach {}", run_prefix(&plan.plan_id)))
        );
        if plan.tasks.iter().any(|task| task.child_run_id.is_some()) {
            println!(
                "{} {}",
                ui_command("child:"),
                ui_command(format!(
                    "deadreckon attach {}:task-0",
                    run_prefix(&plan.plan_id)
                ))
            );
        }
        println!(
            "{} {}",
            ui_command("merge:"),
            ui_command(format!("deadreckon merge {}", run_prefix(&plan.plan_id)))
        );
    }
}

async fn merge_command(args: MergeCommandArgs) -> Result<()> {
    let MergeCommandArgs {
        plan_id,
        strategy,
        prefer_child,
        no_repair,
        repair_provider,
        repair_mode,
        repair_attempts,
        yes: _yes,
        no_gate,
        no_hints,
        quiet,
        plain: _plain,
    } = args;
    let paths = DeadreckonPaths::discover();
    let resolved_id = resolve_plan_id(&paths, &plan_id)?;
    let mut plan = load_plan(&paths, &resolved_id)?;
    if !matches!(plan.status, PlanStatus::Forked | PlanStatus::Failed) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} is {}",
                run_prefix(&plan.plan_id),
                plan_status_label(plan.status)
            ),
            "deadreckon fork <plan-id>",
        )));
    }
    if let Some(task) = plan
        .tasks
        .iter()
        .find(|task| task.status != PlanTaskStatus::Completed)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("child {} is {}", task.index, task_status_label(task.status)),
            "wait, or run deadreckon kill <plan-id>",
        )));
    }
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::MergeStarted)?;
    let strategy = parse_merge_strategy(&strategy, prefer_child)?;
    if let PlanMergeStrategy::PreferChild(chosen) = strategy
        && !plan.tasks.iter().any(|task| task.index == chosen)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown child index {chosen}"),
            "deadreckon merge <plan-id> --strategy prefer-child --prefer-child 1",
        )));
    }
    let repair_mode = parse_merge_repair_mode(&repair_mode)?;
    let mut merge = compose_plan_merge_working(&paths, &plan, strategy)?;
    let unresolved_conflicts = merge.unresolved_conflicts();
    if !unresolved_conflicts.is_empty() {
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeConflict {
                conflict_count: unresolved_conflicts.len(),
            },
        )?;
        let repair_disabled = no_repair
            || repair_attempts == 0
            || matches!(strategy, PlanMergeStrategy::FailOnConflict);
        let provider = if repair_disabled {
            None
        } else {
            resolve_merge_repair_provider(&paths, &plan, repair_provider.as_deref())?
        };
        write_merge_repair_request(&paths, &plan, provider.as_deref(), &unresolved_conflicts)?;
        if repair_disabled {
            let reason = format!(
                "merge conflict at {}",
                unresolved_conflicts
                    .iter()
                    .map(|conflict| conflict.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            record_plan_merge_failure(&paths, &mut plan, &reason)?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("{reason}; automatic repair disabled"),
                &format!(
                    "inspect {}",
                    paths
                        .merge_proofs(&plan.plan_id)
                        .join("conflicts.json")
                        .display()
                ),
            )));
        }
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairPlanned {
                conflict_count: unresolved_conflicts.len(),
                provider: provider.clone(),
            },
        )?;
        let Some(provider) = provider else {
            let reason = "merge repair needs a configured provider".to_string();
            append_plan_event(
                &paths,
                &plan.plan_id,
                PlanEventKind::MergeRepairFailed {
                    reason: reason.clone(),
                },
            )?;
            record_plan_merge_failure(&paths, &mut plan, &reason)?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("{reason}; conflicts remain"),
                "deadreckon providers list --all",
            )));
        };
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairStarted {
                mode: repair_mode.as_str().to_string(),
            },
        )?;
        match run_merge_repair(
            &paths,
            &plan,
            &provider,
            repair_mode,
            repair_attempts,
            &mut merge,
            quiet,
        )
        .await
        {
            Ok(repaired) => {
                append_plan_event(
                    &paths,
                    &plan.plan_id,
                    PlanEventKind::MergeRepaired {
                        strategy: repaired.strategy,
                        repair_run_id: repaired.repair_run_id,
                    },
                )?;
                write_plan_merge_conflicts(&paths, &plan, strategy, &merge.conflicts)?;
            }
            Err(error) => {
                let reason = error.to_string();
                append_plan_event(
                    &paths,
                    &plan.plan_id,
                    PlanEventKind::MergeRepairFailed {
                        reason: reason.clone(),
                    },
                )?;
                record_plan_merge_failure(&paths, &mut plan, &reason)?;
                return Err(error);
            }
        }
    }
    let merged_run = create_merged_plan_run(&paths, &plan, no_gate)?;
    plan.status = PlanStatus::Merged;
    plan.merged_at = Some(Utc::now());
    plan.merged_run_id = Some(merged_run.run_id.clone());
    save_plan(&paths, &plan)?;
    append_plan_event(
        &paths,
        &plan.plan_id,
        PlanEventKind::MergeCompleted {
            merged_run_id: merged_run.run_id.clone(),
        },
    )?;
    append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanCompleted)?;
    let _plan_narrative = write_plan_narrative(&paths, &plan)?;
    let library_dir = paths.library_dir(&merged_run.scope, &merged_run.run_id);
    write_plan_merge_manifest(&paths, &library_dir, &plan, &merge.conflicts)?;
    if !quiet {
        print_merge_finished(&paths, &plan, &merged_run, &library_dir, no_hints);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum PlanMergeStrategy {
    FailOnConflict,
    DagAware,
    PreferChild(u32),
}

impl PlanMergeStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::FailOnConflict => "fail-on-conflict",
            Self::DagAware => "dag-aware",
            Self::PreferChild(_) => "prefer-child",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeRepairMode {
    Auto,
    Prefer,
    Synthesize,
    Child,
}

impl MergeRepairMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Prefer => "prefer",
            Self::Synthesize => "synthesize",
            Self::Child => "child",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanMergeConflict {
    path: PathBuf,
    first_child: u32,
    second_child: u32,
    chosen_child: Option<u32>,
    #[serde(default)]
    children: Vec<PlanMergeConflictChild>,
    #[serde(default)]
    deterministic_resolution: Option<PlanMergeDeterministicResolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanMergeConflictChild {
    task_id: String,
    task_index: u32,
    run_id: String,
    artifact_root: PathBuf,
    artifact_path: PathBuf,
    hash: String,
    depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanMergeDeterministicResolution {
    kind: String,
    chosen_task_id: Option<String>,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct PlanMergeConflictBundle<'a> {
    schema_version: u32,
    plan_id: &'a str,
    strategy: &'a str,
    conflicts: &'a [PlanMergeConflict],
}

#[derive(Debug, Clone)]
struct PlanMergeSeenFile {
    task_id: String,
    task_index: u32,
    run_id: String,
    artifact_root: PathBuf,
    artifact_path: PathBuf,
    hash: u64,
}

#[derive(Debug, Clone)]
struct PlanMergeOutcome {
    conflicts: Vec<PlanMergeConflict>,
}

impl PlanMergeOutcome {
    fn unresolved_conflicts(&self) -> Vec<PlanMergeConflict> {
        self.conflicts
            .iter()
            .filter(|conflict| conflict.chosen_child.is_none())
            .cloned()
            .collect()
    }
}

fn parse_merge_strategy(strategy: &str, prefer_child: Option<u32>) -> Result<PlanMergeStrategy> {
    match strategy {
        "fail-on-conflict" => Ok(PlanMergeStrategy::FailOnConflict),
        "dag-aware" => Ok(PlanMergeStrategy::DagAware),
        "prefer-child" => prefer_child
            .map(PlanMergeStrategy::PreferChild)
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "plan merge strategy prefer-child needs --prefer-child <idx>",
                    "deadreckon merge <plan-id> --strategy prefer-child --prefer-child 1",
                ))
            }),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown plan merge strategy {other}"),
            "use --strategy dag-aware, fail-on-conflict, or prefer-child --prefer-child <idx>",
        ))),
    }
}

fn parse_merge_repair_mode(mode: &str) -> Result<MergeRepairMode> {
    match mode {
        "auto" => Ok(MergeRepairMode::Auto),
        "prefer" => Ok(MergeRepairMode::Prefer),
        "synthesize" => Ok(MergeRepairMode::Synthesize),
        "child" => Ok(MergeRepairMode::Child),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown repair mode {other}"),
            "use --repair-mode auto|prefer|synthesize|child",
        ))),
    }
}

fn compose_plan_merge_working(
    paths: &DeadreckonPaths,
    plan: &Plan,
    strategy: PlanMergeStrategy,
) -> Result<PlanMergeOutcome> {
    let merge_working = paths.merge_working(&plan.plan_id);
    remove_if_exists(&merge_working)?;
    fs::create_dir_all(&merge_working)?;
    let mut seen: BTreeMap<PathBuf, PlanMergeSeenFile> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for task in plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
    {
        let run_id = task.child_run_id.as_deref().ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                &format!("child {} has no run id", task.index),
                "deadreckon fork <plan-id>",
            ))
        })?;
        let state = load_run(paths, run_id)?;
        let child_root = child_artifact_root(paths, &state);
        for file in inventory_files(&child_root)? {
            let relative = file.strip_prefix(&child_root).map_err(|err| {
                DeadreckonError::InvalidInput(format!("merge source prefix error: {err}"))
            })?;
            if skip_plan_merge_file(relative) {
                continue;
            }
            let hash = file_hash(&file)?;
            let current = PlanMergeSeenFile {
                task_id: task.task_id.clone(),
                task_index: task.index,
                run_id: run_id.to_string(),
                artifact_root: child_root.clone(),
                artifact_path: file.clone(),
                hash,
            };
            match seen.get(relative).cloned() {
                Some(previous) if previous.hash != hash => match strategy {
                    PlanMergeStrategy::FailOnConflict => {
                        conflicts.push(plan_merge_conflict(
                            plan, relative, &previous, &current, None,
                        ));
                    }
                    PlanMergeStrategy::PreferChild(chosen) => {
                        conflicts.push(plan_merge_conflict(
                            plan,
                            relative,
                            &previous,
                            &current,
                            Some(chosen),
                        ));
                        if chosen == task.index {
                            copy_merge_file(&file, &merge_working.join(relative))?;
                            seen.insert(relative.to_path_buf(), current);
                        }
                    }
                    PlanMergeStrategy::DagAware
                        if plan_task_depends_on(plan, &current.task_id, &previous.task_id) =>
                    {
                        copy_merge_file(&file, &merge_working.join(relative))?;
                        seen.insert(relative.to_path_buf(), current);
                    }
                    PlanMergeStrategy::DagAware
                        if plan_task_depends_on(plan, &previous.task_id, &current.task_id) => {}
                    PlanMergeStrategy::DagAware => {
                        conflicts.push(plan_merge_conflict(
                            plan, relative, &previous, &current, None,
                        ));
                    }
                },
                Some(_) => {}
                None => {
                    copy_merge_file(&file, &merge_working.join(relative))?;
                    seen.insert(relative.to_path_buf(), current);
                }
            }
        }
    }
    write_plan_merge_conflicts(paths, plan, strategy, &conflicts)?;
    Ok(PlanMergeOutcome { conflicts })
}

fn plan_merge_conflict(
    plan: &Plan,
    relative: &Path,
    previous: &PlanMergeSeenFile,
    current: &PlanMergeSeenFile,
    chosen_child: Option<u32>,
) -> PlanMergeConflict {
    let deterministic_resolution = chosen_child.map(|chosen| PlanMergeDeterministicResolution {
        kind: "manual_prefer_child".to_string(),
        chosen_task_id: plan
            .tasks
            .iter()
            .find(|task| task.index == chosen)
            .map(|task| task.task_id.clone()),
        reason: format!("user selected child {chosen}"),
    });
    PlanMergeConflict {
        path: relative.to_path_buf(),
        first_child: previous.task_index,
        second_child: current.task_index,
        chosen_child,
        children: vec![
            plan_merge_conflict_child(plan, previous),
            plan_merge_conflict_child(plan, current),
        ],
        deterministic_resolution,
    }
}

fn plan_merge_conflict_child(plan: &Plan, file: &PlanMergeSeenFile) -> PlanMergeConflictChild {
    let depends_on = plan
        .task_by_id(&file.task_id)
        .map(|task| task.depends_on.clone())
        .unwrap_or_default();
    PlanMergeConflictChild {
        task_id: file.task_id.clone(),
        task_index: file.task_index,
        run_id: file.run_id.clone(),
        artifact_root: file.artifact_root.clone(),
        artifact_path: file.artifact_path.clone(),
        hash: format!("{:016x}", file.hash),
        depends_on,
    }
}

fn write_plan_merge_conflicts(
    paths: &DeadreckonPaths,
    plan: &Plan,
    strategy: PlanMergeStrategy,
    conflicts: &[PlanMergeConflict],
) -> Result<()> {
    if conflicts.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(paths.merge_proofs(&plan.plan_id))?;
    let path = paths.merge_proofs(&plan.plan_id).join("conflicts.json");
    let bundle = PlanMergeConflictBundle {
        schema_version: 2,
        plan_id: &plan.plan_id,
        strategy: strategy.as_str(),
        conflicts,
    };
    fs::write(&path, serde_json::to_vec_pretty(&bundle)?)?;
    Ok(())
}

fn record_plan_merge_failure(paths: &DeadreckonPaths, plan: &mut Plan, reason: &str) -> Result<()> {
    plan.status = PlanStatus::Failed;
    save_plan(paths, plan)?;
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::PlanFailed {
            reason: reason.to_string(),
        },
    )?;
    Ok(())
}

fn resolve_merge_repair_provider(
    paths: &DeadreckonPaths,
    plan: &Plan,
    override_provider: Option<&str>,
) -> Result<Option<String>> {
    if let Some(provider) = override_provider {
        return Ok(Some(provider.to_string()));
    }
    if let Some(provider) = plan.providers.planner.as_ref() {
        return Ok(Some(provider.clone()));
    }
    if let Some(provider) = plan.providers.default_child.as_ref() {
        return Ok(Some(provider.clone()));
    }
    Ok(config_defaults(paths)?.provider)
}

#[derive(Debug, Clone, Serialize)]
struct MergeRepairRequest<'a> {
    schema_version: u32,
    plan_id: &'a str,
    root_goal: &'a str,
    provider: Option<&'a str>,
    created_at: DateTime<Utc>,
    merge_working: PathBuf,
    task_graph: Vec<Value>,
    worker_specs: BTreeMap<String, String>,
    summary_paths: BTreeMap<String, String>,
    recent_events: Vec<PlanEvent>,
    conflicts: &'a [PlanMergeConflict],
}

fn write_merge_repair_request(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: Option<&str>,
    conflicts: &[PlanMergeConflict],
) -> Result<PathBuf> {
    fs::create_dir_all(paths.merge_proofs(&plan.plan_id))?;
    let worker_specs = plan
        .tasks
        .iter()
        .map(|task| {
            (
                task.task_id.clone(),
                paths
                    .worker_spec(&plan.plan_id, &task.task_id)
                    .display()
                    .to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let summary_paths = plan
        .tasks
        .iter()
        .filter_map(|task| {
            Some((
                task.task_id.clone(),
                paths
                    .plan_dir(&plan.plan_id)
                    .join(task.summary_path.as_ref()?)
                    .display()
                    .to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let task_graph = plan
        .tasks
        .iter()
        .map(|task| {
            json!({
                "task_id": &task.task_id,
                "index": task.index,
                "subject": &task.subject,
                "role": task.role,
                "provider": &task.provider,
                "depends_on": &task.depends_on,
                "status": task.status,
                "child_run_id": &task.child_run_id,
            })
        })
        .collect::<Vec<_>>();
    let recent_events = read_plan_events_lossy(paths, &plan.plan_id)
        .into_iter()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let request = MergeRepairRequest {
        schema_version: 1,
        plan_id: &plan.plan_id,
        root_goal: &plan.root_goal,
        provider,
        created_at: Utc::now(),
        merge_working: paths.merge_working(&plan.plan_id),
        task_graph,
        worker_specs,
        summary_paths,
        recent_events,
        conflicts,
    };
    let path = paths
        .merge_proofs(&plan.plan_id)
        .join("repair-request.json");
    fs::write(&path, serde_json::to_vec_pretty(&request)?)?;
    Ok(path)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct MergeRepairPlan {
    #[serde(default)]
    schema_version: Option<u32>,
    decision: String,
    rationale: String,
    #[serde(default)]
    actions: Vec<MergeRepairAction>,
    #[serde(default)]
    repair_goal: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct MergeRepairAction {
    path: PathBuf,
    action: String,
    #[serde(default)]
    chosen_task_id: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    preserve: Vec<String>,
}

#[derive(Debug, Clone)]
struct MergeRepairResult {
    strategy: String,
    repair_run_id: Option<String>,
}

async fn run_merge_repair(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: &str,
    mode: MergeRepairMode,
    attempts: u32,
    merge: &mut PlanMergeOutcome,
    quiet: bool,
) -> Result<MergeRepairResult> {
    if attempts == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "merge repair attempts are disabled",
            "rerun without --repair-attempts 0",
        )));
    }
    let request_path = paths
        .merge_proofs(&plan.plan_id)
        .join("repair-request.json");
    let repair_plan =
        invoke_merge_repair_planner(paths, plan, provider, mode, &request_path, quiet).await?;
    validate_merge_repair_plan(&repair_plan, &merge.unresolved_conflicts(), mode)?;
    let repair_plan_path = paths.merge_proofs(&plan.plan_id).join("repair-plan.json");
    fs::write(&repair_plan_path, serde_json::to_vec_pretty(&repair_plan)?)?;
    match repair_plan.decision.as_str() {
        "prefer_child" => {
            apply_prefer_child_repair(paths, plan, &repair_plan, merge)?;
            Ok(MergeRepairResult {
                strategy: "prefer_child".to_string(),
                repair_run_id: None,
            })
        }
        "synthesize" => {
            apply_synthesized_repair(paths, plan, &repair_plan, merge)?;
            Ok(MergeRepairResult {
                strategy: "synthesize".to_string(),
                repair_run_id: None,
            })
        }
        "spawn_repair_child" => {
            let run_id =
                execute_merge_repair_child(paths, plan, provider, &repair_plan, quiet).await?;
            for conflict in &mut merge.conflicts {
                if conflict.chosen_child.is_none() {
                    conflict.deterministic_resolution = Some(PlanMergeDeterministicResolution {
                        kind: "planner_repair_child".to_string(),
                        chosen_task_id: None,
                        reason: repair_plan.rationale.clone(),
                    });
                }
            }
            Ok(MergeRepairResult {
                strategy: "spawn_repair_child".to_string(),
                repair_run_id: Some(run_id),
            })
        }
        "refuse" => Err(CliError::Core(deadreckon_core::user_error(
            &format!("repair planner refused: {}", repair_plan.rationale),
            &format!("inspect {}", repair_plan_path.display()),
        ))),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unsupported repair decision {other}"),
            "rerun with another --repair-provider",
        ))),
    }
}

async fn invoke_merge_repair_planner(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: &str,
    mode: MergeRepairMode,
    request_path: &Path,
    quiet: bool,
) -> Result<MergeRepairPlan> {
    let request_json = fs::read_to_string(request_path)?;
    let router = if provider == "smoke" || provider.starts_with("smoke:") {
        ProviderRouter::smoke()
    } else {
        ProviderRouter::from_config_path(&paths.config_path(), Some(provider))?
    };
    let prompt = format!(
        "You are a read-only merge repair planner for deadreckon. Do not write files or mutate state.\n\nReturn JSON only with this shape: {{\"decision\":\"prefer_child|synthesize|spawn_repair_child|refuse\",\"rationale\":\"short reason\",\"actions\":[{{\"path\":\"relative conflict path\",\"action\":\"prefer_child|write_synthesized|repair_child\",\"chosen_task_id\":\"task-id or null\",\"content\":\"only for write_synthesized\",\"preserve\":[\"semantic requirement\"]}}],\"repair_goal\":\"only for spawn_repair_child\"}}.\n\nAllowed repair mode: {}.\nOnly choose paths listed in the request conflicts. For prefer_child, chosen_task_id must be one of that conflict's child task ids. For synthesize, include full file content and only synthesize listed conflict paths. For spawn_repair_child, provide a precise repair_goal that preserves the root goal and child summaries.\n\nRepair request JSON:\n{}",
        mode.as_str(),
        request_json
    );
    let cwd = plan
        .parent_cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| paths.home().to_path_buf()));
    let request = ProviderRequest {
        prompt,
        max_output_tokens: 8192,
        cwd: Some(cwd),
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    let response =
        maybe_with_cli_wait_status(!quiet, "planning merge repair", router.complete(&request))
            .await?;
    parse_merge_repair_response(&response.content)
}

fn parse_merge_repair_response(content: &str) -> Result<MergeRepairPlan> {
    if let Ok(plan) = serde_json::from_str::<MergeRepairPlan>(content) {
        return Ok(plan);
    }
    if let Some(slice) = json_slice(content, '{', '}')
        && let Ok(plan) = serde_json::from_str::<MergeRepairPlan>(slice)
    {
        return Ok(plan);
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "repair provider did not return valid repair JSON",
        "rerun with another --repair-provider",
    )))
}

fn validate_merge_repair_plan(
    repair_plan: &MergeRepairPlan,
    conflicts: &[PlanMergeConflict],
    mode: MergeRepairMode,
) -> Result<()> {
    let decision = repair_plan.decision.as_str();
    let allowed = match mode {
        MergeRepairMode::Auto => matches!(
            decision,
            "prefer_child" | "synthesize" | "spawn_repair_child" | "refuse"
        ),
        MergeRepairMode::Prefer => matches!(decision, "prefer_child" | "refuse"),
        MergeRepairMode::Synthesize => matches!(decision, "synthesize" | "refuse"),
        MergeRepairMode::Child => matches!(decision, "spawn_repair_child" | "refuse"),
    };
    if !allowed {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "repair decision {decision} is not allowed by --repair-mode {}",
                mode.as_str()
            ),
            "rerun with --repair-mode auto",
        )));
    }
    if decision == "refuse" {
        return Ok(());
    }
    if !matches!(
        decision,
        "prefer_child" | "synthesize" | "spawn_repair_child"
    ) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown repair decision {decision}"),
            "rerun with another --repair-provider",
        )));
    }
    let conflict_paths = conflicts
        .iter()
        .map(|conflict| conflict.path.clone())
        .collect::<BTreeSet<_>>();
    for action in &repair_plan.actions {
        validate_relative_repair_path(&action.path)?;
        if !conflict_paths.contains(&action.path) {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "repair planner chose non-conflict path {}",
                    action.path.display()
                ),
                "use --repair-mode child for broader integration work",
            )));
        }
        match decision {
            "prefer_child" => {
                if action.action != "prefer_child" {
                    return Err(CliError::Core(deadreckon_core::user_error(
                        "prefer_child repair requires prefer_child actions",
                        "rerun with --repair-mode auto",
                    )));
                }
                let chosen = action.chosen_task_id.as_deref().ok_or_else(|| {
                    CliError::Core(deadreckon_core::user_error(
                        "prefer_child action missing chosen_task_id",
                        "rerun with another --repair-provider",
                    ))
                })?;
                let Some(conflict) = conflicts
                    .iter()
                    .find(|conflict| conflict.path == action.path)
                else {
                    continue;
                };
                if !conflict
                    .children
                    .iter()
                    .any(|child| child.task_id == chosen)
                {
                    return Err(CliError::Core(deadreckon_core::user_error(
                        &format!("repair planner chose unknown task id {chosen}"),
                        "inspect merge-proofs/repair-plan.json",
                    )));
                }
            }
            "synthesize" => {
                if action.action != "write_synthesized" || action.content.is_none() {
                    return Err(CliError::Core(deadreckon_core::user_error(
                        "synthesize repair requires write_synthesized actions with content",
                        "rerun with another --repair-provider",
                    )));
                }
            }
            "spawn_repair_child" => {
                if action.action != "repair_child" {
                    return Err(CliError::Core(deadreckon_core::user_error(
                        "repair child decisions require repair_child actions",
                        "rerun with another --repair-provider",
                    )));
                }
            }
            _ => {}
        }
    }
    if decision != "spawn_repair_child" && repair_plan.actions.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "repair planner returned no actions",
            "rerun with another --repair-provider",
        )));
    }
    if decision == "spawn_repair_child"
        && repair_plan
            .repair_goal
            .as_deref()
            .is_none_or(|goal| goal.trim().is_empty())
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "repair child decision missing repair_goal",
            "rerun with another --repair-provider",
        )));
    }
    Ok(())
}

fn validate_relative_repair_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("unsafe repair path {}", path.display()),
            "planner must choose a relative conflict path",
        )));
    }
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("unsafe repair path {}", path.display()),
                "planner must choose a relative conflict path",
            )));
        }
    }
    Ok(())
}

fn apply_prefer_child_repair(
    paths: &DeadreckonPaths,
    plan: &Plan,
    repair_plan: &MergeRepairPlan,
    merge: &mut PlanMergeOutcome,
) -> Result<()> {
    for action in &repair_plan.actions {
        let chosen = action.chosen_task_id.as_deref().ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "prefer_child action missing chosen_task_id",
                "rerun with another --repair-provider",
            ))
        })?;
        let conflict = merge
            .conflicts
            .iter_mut()
            .find(|conflict| conflict.path == action.path)
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    &format!("repair path {} is not a conflict", action.path.display()),
                    "inspect merge-proofs/repair-plan.json",
                ))
            })?;
        let child = conflict
            .children
            .iter()
            .find(|child| child.task_id == chosen)
            .cloned()
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    &format!("repair planner chose unknown task id {chosen}"),
                    "inspect merge-proofs/repair-plan.json",
                ))
            })?;
        copy_merge_file(
            &child.artifact_path,
            &paths.merge_working(&plan.plan_id).join(&action.path),
        )?;
        conflict.chosen_child = Some(child.task_index);
        conflict.deterministic_resolution = Some(PlanMergeDeterministicResolution {
            kind: "planner_prefer_child".to_string(),
            chosen_task_id: Some(child.task_id),
            reason: repair_plan.rationale.clone(),
        });
    }
    Ok(())
}

fn apply_synthesized_repair(
    paths: &DeadreckonPaths,
    plan: &Plan,
    repair_plan: &MergeRepairPlan,
    merge: &mut PlanMergeOutcome,
) -> Result<()> {
    for action in &repair_plan.actions {
        let dest = paths.merge_working(&plan.plan_id).join(&action.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            dest,
            action.content.as_deref().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "synthesize action missing content",
                    "rerun with another --repair-provider",
                ))
            })?,
        )?;
        if let Some(conflict) = merge
            .conflicts
            .iter_mut()
            .find(|conflict| conflict.path == action.path)
        {
            conflict.deterministic_resolution = Some(PlanMergeDeterministicResolution {
                kind: "planner_synthesize".to_string(),
                chosen_task_id: None,
                reason: repair_plan.rationale.clone(),
            });
        }
    }
    Ok(())
}

async fn execute_merge_repair_child(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: &str,
    repair_plan: &MergeRepairPlan,
    quiet: bool,
) -> Result<String> {
    let repair_scope = paths.merge_proofs(&plan.plan_id).join("repair-child");
    fs::create_dir_all(&repair_scope)?;
    let repair_goal = format!(
        "{}\n\nRoot goal: {}\nPlan: {}\nRepair request: {}\nRepair plan: {}\n\nResolve only the merge conflict paths named in the repair plan unless a build/test update is strictly required to make the repaired artifact coherent. Preserve completed child behavior and report files changed.",
        repair_plan
            .repair_goal
            .as_deref()
            .unwrap_or("Resolve orchestration merge conflicts."),
        plan.root_goal,
        plan.plan_id,
        paths
            .merge_proofs(&plan.plan_id)
            .join("repair-request.json")
            .display(),
        paths
            .merge_proofs(&plan.plan_id)
            .join("repair-plan.json")
            .display()
    );
    let merge_working = paths.merge_working(&plan.plan_id);
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&merge_working)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_HINTS", "0")
        .env("DEADRECKON_SCOPE_ROOT", &repair_scope)
        .arg("run")
        .arg(repair_goal)
        .arg("--from")
        .arg(&merge_working)
        .arg("--yes")
        .arg("--no-confirm")
        .arg("--no-hints")
        .arg("--no-docs")
        .arg("--sandbox")
        .arg("none");
    if provider == "smoke" || provider.starts_with("smoke:") {
        command.arg("--smoke");
    } else {
        command.arg("--provider").arg(provider);
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn()?;
    let pid = child.id();
    let output = maybe_with_cli_wait_status(!quiet, "running merge repair child", async move {
        child.wait_with_output()
    })
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = parse_started_run_id(&stdout).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "could not find repair run id",
            "deadreckon list --all",
        ))
    })?;
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::MergeRepairRunDiscovered {
            run_id: run_id.clone(),
            pid: Some(pid),
        },
    )?;
    let status = if output.status.success() {
        "completed"
    } else {
        "failed"
    };
    write_merge_repair_run_record(paths, plan, &run_id, status)?;
    if !output.status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "repair child failed: {stdout}{stderr}"
        ))));
    }
    let state = load_run(paths, &run_id)?;
    let library = paths.library_dir(&state.scope, &state.run_id);
    if !library.is_dir() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("repair run {} has no promoted library", run_prefix(&run_id)),
            &format!("deadreckon attach {}", run_prefix(&run_id)),
        )));
    }
    copy_repair_library_to_merge_working(paths, plan, &library)?;
    Ok(run_id)
}

fn write_merge_repair_run_record(
    paths: &DeadreckonPaths,
    plan: &Plan,
    run_id: &str,
    status: &str,
) -> Result<()> {
    let path = paths.merge_proofs(&plan.plan_id).join("repair-run.json");
    let state = load_run(paths, run_id).ok();
    let value = json!({
        "schema_version": 1,
        "plan_id": &plan.plan_id,
        "run_id": run_id,
        "scope": state.as_ref().map(|state| state.scope.clone()),
        "status": status,
        "source": paths.merge_working(&plan.plan_id),
        "created_at": Utc::now(),
        "updated_at": Utc::now(),
    });
    fs::write(path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

fn copy_repair_library_to_merge_working(
    paths: &DeadreckonPaths,
    plan: &Plan,
    library: &Path,
) -> Result<()> {
    let merge_working = paths.merge_working(&plan.plan_id);
    remove_if_exists(&merge_working)?;
    fs::create_dir_all(&merge_working)?;
    for file in inventory_files(library)? {
        let relative = file.strip_prefix(library).map_err(|err| {
            DeadreckonError::InvalidInput(format!("repair source prefix error: {err}"))
        })?;
        if skip_plan_merge_file(relative) {
            continue;
        }
        copy_merge_file(&file, &merge_working.join(relative))?;
    }
    Ok(())
}

fn child_artifact_root(paths: &DeadreckonPaths, state: &deadreckon_core::PipelineState) -> PathBuf {
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if library_dir.is_dir() {
        library_dir
    } else {
        state.working_dir.clone()
    }
}

fn skip_plan_merge_file(relative: &Path) -> bool {
    relative == Path::new("manifest.json")
        || relative == Path::new(deadreckon_core::IMPLEMENTATION_NOTES_HTML)
        || relative.starts_with(".deadreckon")
        || path_has_component(relative, ".git")
        || path_has_component(relative, "target")
        || path_has_component(relative, "node_modules")
        || path_has_component(relative, ".next")
        || path_has_component(relative, "dist")
        || path_has_component(relative, "build")
        || relative
            .strip_prefix("docs")
            .ok()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("RUN-"))
}

fn file_hash(path: &Path) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    fs::read(path)?.hash(&mut hasher);
    Ok(hasher.finish())
}

fn copy_merge_file(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, dest)?;
    Ok(())
}

fn skip_plan_apply_file(relative: &Path) -> bool {
    skip_plan_merge_file(relative) || relative == Path::new("deadreckon-plan-manifest.json")
}

fn create_merged_plan_run(
    paths: &DeadreckonPaths,
    plan: &Plan,
    no_gate: bool,
) -> Result<deadreckon_core::PipelineState> {
    let cwd = std::env::current_dir()?;
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!("merge orchestration plan {}", plan.root_goal),
            cwd,
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )?;
    remove_if_exists(&state.working_dir)?;
    copy_tree(&paths.merge_working(&plan.plan_id), &state.working_dir)?;
    if no_gate {
        eprintln!(
            "{}",
            ui_warn("merge gate skipped; recording synthetic acceptance marker")
        );
    }
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )?;
    state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
    save_state(&state)?;
    promote_completed_run(paths, &mut state)?;
    Ok(state)
}

fn prepare_plan_result_apply_state(
    paths: &DeadreckonPaths,
    plan: &Plan,
    merged_state: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    let git_root = plan_apply_git_root(plan)?.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan {} source is not a git repo",
                run_prefix(&plan.plan_id)
            ),
            &format!(
                "deadreckon export {} --dest <path>",
                run_prefix(&plan.plan_id)
            ),
        ))
    })?;
    let merged_source = paths.library_dir(&merged_state.scope, &merged_state.run_id);
    if !merged_source.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library missing for plan result {}",
            merged_state.run_id
        ))));
    }

    let run_id = Uuid::new_v4().simple().to_string();
    let record = plan_apply_worktree_record(paths, plan, &git_root, &run_id)?;
    create_worktree(&record)?;
    let worktree_path = record.worktree_path.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing plan apply worktree_path".to_string(),
        ))
    })?;
    seed_plan_result_worktree(plan, merged_state, &merged_source, worktree_path)?;

    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!(
                "{} (deadreckon plan {})",
                plan.root_goal,
                run_prefix(&plan.plan_id)
            ),
            cwd: git_root,
            sandbox: "none".to_string(),
            provider: Some("deadreckon:orchestrate-apply".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: Some(run_id),
            codebase: Some(record),
        },
    )?;
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )?;
    state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: state.turn,
            event: "plan_result_apply_prepared".to_string(),
            latency_ms: None,
            detail: json!({
                "plan_id": plan.plan_id,
                "merged_run_id": merged_state.run_id,
                "source": merged_source.display().to_string(),
                "working_dir": state.working_dir.display().to_string(),
            }),
        },
    )?;
    save_state(&state)?;
    Ok(state)
}

fn plan_apply_worktree_record(
    paths: &DeadreckonPaths,
    plan: &Plan,
    git_root: &Path,
    run_id: &str,
) -> Result<CodebaseRecord> {
    let scope = workspace_scope(git_root)?;
    let branch_name = format!(
        "dr/plan-{}-{}",
        run_prefix(&plan.plan_id),
        run_prefix(run_id)
    );
    if git_ref_exists(git_root, &branch_name) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("branch {branch_name} already exists"),
            "retry the apply or pass --cleanup after a successful apply",
        )));
    }
    let (base_ref, base_sha, parent_branch) = plan_apply_base(paths, plan, git_root)?;
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Worktree;
    record.source_path = Some(git_root.to_path_buf());
    record.source_git_root = Some(git_root.to_path_buf());
    record.branch_name = Some(branch_name);
    record.base_ref = Some(base_ref);
    record.base_sha = Some(base_sha);
    record.parent_branch = parent_branch;
    record.worktree_path = Some(plan_apply_worktree_path(paths, &scope, run_id));
    Ok(record)
}

fn plan_apply_base(
    paths: &DeadreckonPaths,
    plan: &Plan,
    git_root: &Path,
) -> Result<(String, String, Option<String>)> {
    for task in &plan.tasks {
        let Some(child_run_id) = task.child_run_id.as_deref() else {
            continue;
        };
        let Ok(child) = load_run(paths, child_run_id) else {
            continue;
        };
        let Ok(record) = read_run_codebase_record(paths, &child) else {
            continue;
        };
        if record.source_git_root.as_deref() != Some(git_root) {
            continue;
        }
        let Some(base_sha) = record.base_sha.as_deref() else {
            continue;
        };
        if git_status(
            git_root,
            &["cat-file", "-e", &format!("{base_sha}^{{commit}}")],
        )
        .is_err()
        {
            continue;
        }
        return Ok((
            base_sha.to_string(),
            base_sha.to_string(),
            record.base_ref.clone(),
        ));
    }

    let base_ref = git_stdout(git_root, &["symbolic-ref", "--short", "HEAD"])
        .unwrap_or_else(|_| "HEAD".into());
    let base_sha = git_stdout(git_root, &["rev-parse", &base_ref])?;
    Ok((base_ref.clone(), base_sha, Some(base_ref)))
}

fn plan_apply_worktree_path(paths: &DeadreckonPaths, scope: &str, run_id: &str) -> PathBuf {
    let stem = format!(
        "{}-{}",
        deadreckon_core::paths::sanitize_slug(scope),
        run_prefix(run_id)
    );
    let root = paths.home().join("worktrees");
    let mut candidate = root.join(&stem);
    let mut suffix = 2;
    while candidate.exists()
        && fs::read_dir(&candidate)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        candidate = root.join(format!("{stem}-{suffix}"));
        suffix += 1;
    }
    candidate
}

fn seed_plan_result_worktree(
    plan: &Plan,
    merged_state: &deadreckon_core::PipelineState,
    merged_source: &Path,
    worktree_path: &Path,
) -> Result<()> {
    for file in inventory_files(worktree_path)? {
        let relative = file.strip_prefix(worktree_path).map_err(|err| {
            DeadreckonError::InvalidInput(format!("plan apply worktree prefix error: {err}"))
        })?;
        if path_has_component(relative, ".git") {
            continue;
        }
        remove_if_exists(&file)?;
    }
    for file in inventory_files(merged_source)? {
        let relative = file.strip_prefix(merged_source).map_err(|err| {
            DeadreckonError::InvalidInput(format!("plan apply source prefix error: {err}"))
        })?;
        if skip_plan_apply_file(relative) {
            continue;
        }
        copy_merge_file(&file, &worktree_path.join(relative))?;
    }

    git_status(worktree_path, &["add", "-A"])?;
    let staged = git_stdout(worktree_path, &["diff", "--cached", "--stat"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    git_status(
        worktree_path,
        &[
            "commit",
            "-m",
            &plan_apply_commit_subject(plan),
            "-m",
            &plan_apply_commit_body(plan, merged_state),
        ],
    )
}

fn plan_apply_commit_subject(plan: &Plan) -> String {
    format!(
        "{} (deadreckon plan {})",
        one_line(&plan.root_goal, 72),
        run_prefix(&plan.plan_id)
    )
}

fn plan_apply_commit_body(plan: &Plan, merged_state: &deadreckon_core::PipelineState) -> String {
    let mut lines = vec![
        format!("Plan: {}", plan.plan_id),
        format!("Result run: {}", merged_state.run_id),
        String::new(),
        "Children:".to_string(),
    ];
    for task in &plan.tasks {
        lines.push(format!(
            "- {}: {}{}",
            task.task_id,
            task.child_run_id.as_deref().unwrap_or("-"),
            task.provider
                .as_deref()
                .map(|provider| format!(" ({provider})"))
                .unwrap_or_default()
        ));
    }
    lines.join("\n")
}

fn write_plan_merge_manifest(
    paths: &DeadreckonPaths,
    library_dir: &Path,
    plan: &Plan,
    conflicts: &[PlanMergeConflict],
) -> Result<()> {
    let messages = read_plan_messages(paths, &plan.plan_id).unwrap_or_default();
    let mut message_counts_by_type = BTreeMap::<String, usize>::new();
    for message in &messages {
        let key = format!("{:?}", message.kind).to_ascii_lowercase();
        *message_counts_by_type.entry(key).or_default() += 1;
    }
    let task_graph = plan
        .tasks
        .iter()
        .map(|task| {
            json!({
                "task_id": &task.task_id,
                "index": task.index,
                "role": task.role,
                "provider": &task.provider,
                "depends_on": &task.depends_on,
            })
        })
        .collect::<Vec<_>>();
    let summary_paths = plan
        .tasks
        .iter()
        .filter_map(|task| {
            Some((
                task.task_id.clone(),
                task.summary_path.as_ref()?.display().to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let manifest = json!({
        "schema_version": 1,
        "kind": "plan_merge",
        "plan_id": &plan.plan_id,
        "root_goal": &plan.root_goal,
        "mode": plan.mode,
        "merged_at": plan.merged_at,
        "merged_run_id": &plan.merged_run_id,
        "providers": &plan.providers,
        "capability_preview": &plan.capability_preview,
        "tasks": &plan.tasks,
        "task_graph": task_graph,
        "summary_paths": summary_paths,
        "coordinator_messages": {
            "total": messages.len(),
            "by_type": message_counts_by_type,
        },
        "conflicts": conflicts,
    });
    fs::write(
        library_dir.join("deadreckon-plan-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

fn print_merge_finished(
    paths: &DeadreckonPaths,
    plan: &Plan,
    merged_run: &deadreckon_core::PipelineState,
    library_dir: &Path,
    no_hints: bool,
) {
    println!(
        "{} {}",
        ui_ok("completed plan"),
        ui_id(run_prefix(&plan.plan_id))
    );
    println!("result run (secondary) {}", run_prefix(&merged_run.run_id));
    println!("artifact library {}", library_dir.display());
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
    let repair_summary = plan_merge_repair_summary_items(paths, plan);
    if !repair_summary.is_empty() {
        println!("merge repair");
        let repair_items = repair_summary
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        print_kv_block(&repair_items);
    }
    if !no_hints {
        println!(
            "{} {}",
            ui_command("finish:"),
            ui_command(format!("deadreckon finish {}", run_prefix(&plan.plan_id)))
        );
        if plan_apply_git_root(plan).ok().flatten().is_some() {
            println!(
                "{} {}",
                ui_command("apply:"),
                ui_command(format!("deadreckon apply {}", run_prefix(&plan.plan_id)))
            );
        }
        println!(
            "{} {}",
            ui_command("export:"),
            ui_command(format!(
                "deadreckon export {} --dest ./{}",
                run_prefix(&plan.plan_id),
                deadreckon_core::paths::task_key(&plan.root_goal)
                    .chars()
                    .take(24)
                    .collect::<String>()
            ))
        );
    }
}

fn print_plan_summary(paths: &DeadreckonPaths, plan: &Plan, show_hints: bool) {
    println!(
        "{} {} ({})",
        ui_heading("plan"),
        ui_id(run_prefix(&plan.plan_id)),
        plan.plan_id
    );
    let goal = one_line(&plan.root_goal, 120);
    let providers = match plan.mode {
        PlanMode::FullPlan => format!(
            "planner={} default-child={}",
            plan.providers.planner.as_deref().unwrap_or("-"),
            plan.providers.default_child.as_deref().unwrap_or("-")
        ),
        PlanMode::Review => format!(
            "coder={} reviewer={}",
            plan.providers.coder.as_deref().unwrap_or("-"),
            plan.providers.reviewer.as_deref().unwrap_or("-")
        ),
    };
    let capabilities = format!(
        "network={:?} deploy={} install={}",
        plan.capability_preview.network,
        plan.capability_preview.deploy,
        plan.capability_preview.global_install
    );
    let items = [
        ("status", plan_status_label(plan.status)),
        ("mode", plan_mode_label(plan.mode)),
        ("goal", goal.as_str()),
        ("providers", providers.as_str()),
        ("capabilities", capabilities.as_str()),
    ];
    print_kv_block(&items);
    print_orchestration_role_table(plan, true, None);
    print_orchestration_dependency_summary(plan);
    if let Some(line) = plan_final_gate_line(paths, plan) {
        println!("final gate {line}");
    }
    let plan_events = read_plan_events_lossy(paths, &plan.plan_id);
    if let Some(event) = plan_events.last() {
        println!("latest plan event {}", plan_event_line(event));
    }
    let repair_summary = plan_merge_repair_summary_items(paths, plan);
    if !repair_summary.is_empty() {
        println!("merge repair");
        let repair_items = repair_summary
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        print_kv_block(&repair_items);
    }
    println!("children");
    for task in &plan.tasks {
        println!(
            "  {} {:<9} {:<9} provider={} run={} {}",
            task.task_id,
            format!("{:?}", task.role).to_ascii_lowercase(),
            task_status_label(task.status),
            task.provider.as_deref().unwrap_or("-"),
            task.child_run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string()),
            one_line(&task.subject, 60)
        );
        for detail in plan_task_detail_lines(paths, plan, task, 100)
            .into_iter()
            .skip(4)
        {
            println!("    {detail}");
        }
        if show_hints && let Some(run_id) = task.child_run_id.as_deref() {
            let child_ref = format!("{}:{}", run_prefix(&plan.plan_id), task.task_id);
            println!(
                "    {} {}",
                ui_command("drill:"),
                ui_command(format!("deadreckon attach {child_ref}"))
            );
            println!(
                "    {} {}",
                ui_command("show:"),
                ui_command(format!("deadreckon show {child_ref}"))
            );
            println!("    run id {}", run_prefix(run_id));
        }
    }
    let messages = read_plan_messages(paths, &plan.plan_id).unwrap_or_default();
    if let Some(message) = messages.last() {
        println!(
            "latest {} -> {} {:?}: {}",
            message.from, message.to, message.kind, message.summary
        );
    }
    if let Some(merged_run_id) = plan.merged_run_id.as_deref() {
        println!("result run (secondary) {}", run_prefix(merged_run_id));
    }
    if show_hints {
        println!(
            "{} {}",
            ui_command("attach:"),
            ui_command(format!("deadreckon attach {}", run_prefix(&plan.plan_id)))
        );
        match plan.status {
            PlanStatus::Pending => {
                println!(
                    "{} {}",
                    ui_command("fork:"),
                    ui_command(format!("deadreckon fork {}", run_prefix(&plan.plan_id)))
                );
            }
            PlanStatus::Forked => {
                println!(
                    "{} {}",
                    ui_command("merge:"),
                    ui_command(format!("deadreckon merge {}", run_prefix(&plan.plan_id)))
                );
            }
            PlanStatus::Merged => {
                if plan.merged_run_id.is_some() {
                    println!(
                        "{} {}",
                        ui_command("finish:"),
                        ui_command(format!("deadreckon finish {}", run_prefix(&plan.plan_id)))
                    );
                    if plan_apply_git_root(plan).ok().flatten().is_some() {
                        println!(
                            "{} {}",
                            ui_command("apply:"),
                            ui_command(format!("deadreckon apply {}", run_prefix(&plan.plan_id)))
                        );
                    }
                    println!(
                        "{} {}",
                        ui_command("export:"),
                        ui_command(format!("deadreckon export {}", run_prefix(&plan.plan_id)))
                    );
                }
            }
            PlanStatus::Failed => {
                println!(
                    "{} {}",
                    ui_command("why:"),
                    ui_command(format!("deadreckon show {}", run_prefix(&plan.plan_id)))
                );
            }
        }
    }
}

fn plan_merge_repair_status_line(paths: &DeadreckonPaths, plan: &Plan) -> Option<String> {
    let proofs = paths.merge_proofs(&plan.plan_id);
    let repair_plan = proofs.join("repair-plan.json");
    let repair_run = proofs.join("repair-run.json");
    let conflicts = proofs.join("conflicts.json");
    if repair_run.is_file()
        && let Ok(raw) = fs::read_to_string(&repair_run)
        && let Ok(value) = serde_json::from_str::<Value>(&raw)
    {
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("recorded");
        let run = value
            .get("run_id")
            .and_then(Value::as_str)
            .map(run_prefix)
            .unwrap_or_else(|| "-".to_string());
        return Some(format!("run {run} {status} ({})", repair_run.display()));
    }
    if repair_plan.is_file()
        && let Ok(raw) = fs::read_to_string(&repair_plan)
        && let Ok(value) = serde_json::from_str::<Value>(&raw)
    {
        let decision = value
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("recorded");
        let rationale = value
            .get("rationale")
            .and_then(Value::as_str)
            .map(|text| format!(": {}", one_line(text, 80)))
            .unwrap_or_default();
        return Some(format!("{decision}{rationale} ({})", repair_plan.display()));
    }
    conflicts
        .is_file()
        .then(|| format!("conflicts recorded ({})", conflicts.display()))
}

fn plan_merge_repair_summary_items(paths: &DeadreckonPaths, plan: &Plan) -> Vec<(String, String)> {
    let proofs = paths.merge_proofs(&plan.plan_id);
    let repair_request = proofs.join("repair-request.json");
    let repair_plan = proofs.join("repair-plan.json");
    let repair_run = proofs.join("repair-run.json");
    let conflicts = proofs.join("conflicts.json");
    if !repair_request.is_file()
        && !repair_plan.is_file()
        && !repair_run.is_file()
        && !conflicts.is_file()
    {
        return Vec::new();
    }

    let events = read_plan_events_lossy(paths, &plan.plan_id);
    let mut mode = None::<String>;
    let mut provider = None::<String>;
    let mut event_conflicts = None::<usize>;
    let mut repair_run_id = None::<String>;
    let mut latest_repair_event = None::<String>;
    for event in &events {
        match &event.event {
            PlanEventKind::MergeConflict { conflict_count } => {
                event_conflicts = Some(*conflict_count);
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            PlanEventKind::MergeRepairPlanned {
                conflict_count,
                provider: planned_provider,
            } => {
                event_conflicts = Some(*conflict_count);
                provider = planned_provider.clone();
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            PlanEventKind::MergeRepairStarted { mode: repair_mode } => {
                mode = Some(repair_mode.clone());
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => {
                repair_run_id = Some(run_id.clone());
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            PlanEventKind::MergeRepaired {
                repair_run_id: run, ..
            } => {
                if let Some(run) = run {
                    repair_run_id = Some(run.clone());
                }
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            PlanEventKind::MergeRepairFailed { .. } => {
                latest_repair_event = Some(plan_event_summary(&event.event));
            }
            _ => {}
        }
    }

    let (conflict_count, conflict_paths) = merge_conflict_summary(&conflicts);
    let repair_plan_value = read_json_value(&repair_plan);
    let repair_run_value = read_json_value(&repair_run);
    if provider.is_none()
        && let Some(value) = read_json_value(&repair_request)
    {
        provider = value
            .get("provider")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    if repair_run_id.is_none()
        && let Some(value) = repair_run_value.as_ref()
    {
        repair_run_id = value
            .get("run_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    let mut items = Vec::new();
    items.push(("enabled".to_string(), "automatic".to_string()));
    items.push((
        "mode".to_string(),
        mode.or_else(|| {
            repair_plan_value
                .as_ref()
                .and_then(|value| value.get("decision"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "auto".to_string()),
    ));
    items.push((
        "attempts".to_string(),
        "1 default unless command flag changed this merge".to_string(),
    ));
    items.push((
        "provider".to_string(),
        provider.unwrap_or_else(|| "config default".to_string()),
    ));
    items.push((
        "conflicts".to_string(),
        match conflict_count.or(event_conflicts) {
            Some(count) if conflict_paths.is_empty() => count.to_string(),
            Some(count) => format!("{count}: {}", conflict_paths.join(", ")),
            None if !conflict_paths.is_empty() => conflict_paths.join(", "),
            None => "-".to_string(),
        },
    ));
    if repair_request.is_file() {
        items.push(("request".to_string(), repair_request.display().to_string()));
    }
    if repair_plan.is_file() {
        let decision = repair_plan_value
            .as_ref()
            .and_then(|value| value.get("decision"))
            .and_then(Value::as_str)
            .unwrap_or("recorded");
        items.push((
            "repair plan".to_string(),
            format!("{decision} ({})", repair_plan.display()),
        ));
    }
    if repair_run.is_file() || repair_run_id.is_some() {
        let status = repair_run_value
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("recorded");
        let run = repair_run_id
            .as_deref()
            .map(run_prefix)
            .unwrap_or_else(|| "-".to_string());
        items.push(("repair run".to_string(), format!("{run} {status}")));
    }
    if let Some(event) = latest_repair_event {
        items.push(("latest event".to_string(), event));
    }
    items.push((
        "next action".to_string(),
        match plan.status {
            PlanStatus::Pending => format!("deadreckon fork {}", run_prefix(&plan.plan_id)),
            PlanStatus::Forked => format!("deadreckon merge {}", run_prefix(&plan.plan_id)),
            PlanStatus::Merged => format!("deadreckon finish {}", run_prefix(&plan.plan_id)),
            PlanStatus::Failed => {
                format!("deadreckon show {} --why-failed", run_prefix(&plan.plan_id))
            }
        },
    ));
    items
}

fn merge_conflict_summary(path: &Path) -> (Option<usize>, Vec<String>) {
    let Some(value) = read_json_value(path) else {
        return (None, Vec::new());
    };
    let conflicts = value
        .get("conflicts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let paths = conflicts
        .iter()
        .filter_map(|conflict| conflict.get("path").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    (Some(conflicts.len()), paths)
}

fn read_json_value(path: &Path) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
}

fn format_wall_cap(max_wall_seconds: Option<f64>) -> String {
    let Some(seconds) = max_wall_seconds else {
        return "uncapped".to_string();
    };
    if seconds >= 3600.0 && seconds % 3600.0 == 0.0 {
        format!("{:.0}h", seconds / 3600.0)
    } else if seconds >= 60.0 && seconds % 60.0 == 0.0 {
        format!("{:.0}m", seconds / 60.0)
    } else {
        format!("{seconds:.0}s")
    }
}

fn free_kb(path: &std::path::Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

fn prompt_provider() -> Result<String> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let detected =
        auto_subscription_cli_provider(&registry).unwrap_or_else(|| "anthropic".to_string());
    let answer = prompt::open(&format!("provider [{detected}]: "), None)?;
    Ok(if answer.trim().is_empty() {
        detected
    } else {
        answer.trim().to_string()
    })
}

enum NonGitChoice {
    Init,
    Copy,
    Cancel,
}

fn prompt_non_git_mode() -> Result<NonGitChoice> {
    eprintln!("deadreckon: this is not a git repo. options:");
    eprintln!("  [1] git init for me, then run with worktree mode (recommended)");
    eprintln!("  [2] copy mode - agent works on a copy in ~/.deadreckon/runstate/...");
    eprintln!("  [3] cancel");
    let answer = prompt::open("choose [1]: ", None)?;
    Ok(match answer.trim() {
        "" | "1" => NonGitChoice::Init,
        "2" => NonGitChoice::Copy,
        "3" => NonGitChoice::Cancel,
        _ => NonGitChoice::Cancel,
    })
}

async fn with_cli_wait_status<F, T>(label: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    if !ui::enabled(ui::Stream::Stderr) {
        return future.await;
    }
    tokio::pin!(future);
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(180));
    let started = std::time::Instant::now();
    let mut tick = 0usize;
    loop {
        tokio::select! {
            result = &mut future => {
                clear_cli_wait_status();
                return result;
            }
            _ = interval.tick() => {
                tick = tick.wrapping_add(1);
                print_cli_wait_status(label, started.elapsed(), tick);
            }
        }
    }
}

async fn maybe_with_cli_wait_status<F, T>(enabled: bool, label: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    if enabled {
        with_cli_wait_status(label, future).await
    } else {
        future.await
    }
}

async fn with_plain_run_wait_status<F, T>(paths: DeadreckonPaths, run_id: String, future: F) -> T
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    let started = std::time::Instant::now();
    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = interval.tick() => {
                eprintln!("{}", plain_run_progress_line(&paths, &run_id, started.elapsed()));
            }
        }
    }
}

fn plain_run_progress_line(
    paths: &DeadreckonPaths,
    run_id: &str,
    elapsed: std::time::Duration,
) -> String {
    match load_run(paths, run_id) {
        Ok(state) => format!(
            "[{}] turn={} tool=- spend=${:.6} status={} elapsed={}s",
            run_prefix(&state.run_id),
            state.turn,
            state.total_spend_usd,
            state.status,
            elapsed.as_secs()
        ),
        Err(_) => format!(
            "[{}] turn=? tool=- spend=? status=running elapsed={}s",
            run_prefix(run_id),
            elapsed.as_secs()
        ),
    }
}

fn print_exit_summary_card(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
    plain: bool,
) {
    print!("{}", render_exit_summary_card(state, outcome, plain));
}

fn render_exit_summary_card(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
    plain: bool,
) -> String {
    let input = exit_summary_input(state, outcome);
    let card = build_exit_summary_card(&input);
    render_card(&card, &card_options(ui::Stream::Stdout, plain))
}

fn exit_summary_input(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
) -> ExitSummaryInput {
    let spend = deadreckon_core::state::spend_summary(state).unwrap_or_else(|_| {
        deadreckon_core::state::SpendSummary {
            total_usd: state.total_spend_usd,
            wall_seconds: state.total_wall_seconds,
            ..deadreckon_core::state::SpendSummary::default()
        }
    });
    let codebase = read_codebase_record(&state.working_dir).ok();
    let branch = codebase
        .as_ref()
        .and_then(|record| record.branch_name.clone());
    let prefix = run_prefix(&state.run_id);
    let outcome_kind = match outcome {
        RunLoopOutcome::Done => OutcomeKind::Completed,
        RunLoopOutcome::PausedAtCap => OutcomeKind::Paused,
        RunLoopOutcome::Killed => OutcomeKind::Killed,
        RunLoopOutcome::Failed => OutcomeKind::Failed,
    };
    let hints = exit_summary_hints(state, outcome_kind, codebase.as_ref(), &prefix);
    ExitSummaryInput {
        run_id: state.run_id.clone(),
        goal: state.goal.clone(),
        provider: state
            .provider
            .clone()
            .unwrap_or_else(|| "provider".to_string()),
        branch,
        outcome: outcome_kind,
        turns: state.turn,
        input_tokens: spend.input_tokens,
        output_tokens: spend.output_tokens,
        spend_usd: spend.total_usd,
        approximate_spend: spend.any_subscription_turn || spend.any_estimated_turn,
        wall_seconds: spend.wall_seconds,
        diff: codebase
            .as_ref()
            .and_then(|record| branch_diff_summary(state, record).ok().flatten()),
        gate: acceptance_status_line(state),
        working_dir: state.working_dir.clone(),
        proof_path: marker_path_for_run_root(&state.run_root),
        hints,
    }
}

fn exit_summary_hints(
    state: &deadreckon_core::PipelineState,
    outcome: OutcomeKind,
    codebase: Option<&CodebaseRecord>,
    prefix: &str,
) -> Vec<(String, String)> {
    match outcome {
        OutcomeKind::Completed => {
            let mut hints = vec![
                ("attach".to_string(), format!("deadreckon attach {prefix}")),
                ("show".to_string(), format!("deadreckon show {prefix}")),
            ];
            match codebase
                .map(|record| record.mode)
                .unwrap_or(CodebaseMode::Fresh)
            {
                CodebaseMode::Worktree => {
                    hints.push(("apply".to_string(), format!("deadreckon apply {prefix}")));
                }
                CodebaseMode::Copy | CodebaseMode::Fresh => {
                    hints.push((
                        "export".to_string(),
                        format!("deadreckon export {prefix} --dest <path>"),
                    ));
                }
                CodebaseMode::InPlace => {
                    hints.push((
                        "undo".to_string(),
                        format!("deadreckon undo --run {prefix}"),
                    ));
                }
            }
            hints
        }
        OutcomeKind::Paused => vec![
            ("attach".to_string(), format!("deadreckon attach {prefix}")),
            ("resume".to_string(), format!("deadreckon resume {prefix}")),
            ("show".to_string(), format!("deadreckon show {prefix}")),
        ],
        OutcomeKind::Killed | OutcomeKind::Failed => vec![
            (
                "why".to_string(),
                format!("deadreckon show {prefix} --why-failed"),
            ),
            ("resume".to_string(), format!("deadreckon resume {prefix}")),
            (
                "state".to_string(),
                state.state_path().display().to_string(),
            ),
        ],
    }
}

fn branch_diff_summary(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
) -> Result<Option<BranchDiffSummary>> {
    if record.mode != CodebaseMode::Worktree {
        return Ok(None);
    }
    let Some(base_ref) = record.base_ref.as_deref() else {
        return Ok(None);
    };
    if !state.working_dir.join(".git").exists() {
        return Ok(None);
    }
    let range = format!("{base_ref}...HEAD");
    let numstat = git_stdout(&state.working_dir, &["diff", "--numstat", &range])?;
    let name_status = git_stdout(&state.working_dir, &["diff", "--name-status", &range])?;
    let mut summary = BranchDiffSummary::default();
    for line in numstat.lines() {
        let mut parts = line.split('\t');
        let added = parts.next().unwrap_or("0");
        let deleted = parts.next().unwrap_or("0");
        if let Ok(value) = added.parse::<u64>() {
            summary.lines_added = summary.lines_added.saturating_add(value);
        }
        if let Ok(value) = deleted.parse::<u64>() {
            summary.lines_deleted = summary.lines_deleted.saturating_add(value);
        }
    }
    for line in name_status.lines() {
        let status = line.chars().next().unwrap_or('M');
        match status {
            'A' => summary.files_added += 1,
            'D' => summary.files_deleted += 1,
            _ => summary.files_updated += 1,
        }
    }
    if summary.lines_added == 0
        && summary.lines_deleted == 0
        && summary.files_added == 0
        && summary.files_updated == 0
        && summary.files_deleted == 0
    {
        return Ok(None);
    }
    Ok(Some(summary))
}

fn print_cli_wait_status(label: &str, elapsed: std::time::Duration, tick: usize) {
    let line = cli_wait_status_line(label, elapsed, tick);
    let _ = ui::replace_current_line(ui::Stream::Stderr, line);
}

fn clear_cli_wait_status() {
    let _ = ui::clear_current_line(ui::Stream::Stderr);
}

fn cli_wait_status_line(label: &str, elapsed: std::time::Duration, tick: usize) -> String {
    let course = deadreckoning_course_ascii(18, tick);
    format!(
        "{} {} {}  {}s",
        ui::render(ui::Stream::Stderr, ui::Tone::Heading, "deadreckoning"),
        ui::render(ui::Stream::Stderr, ui::Tone::Command, course),
        label,
        elapsed.as_secs()
    )
}

fn deadreckoning_course_ascii(width: usize, tick: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut chars = Vec::with_capacity(width);
    let cursor = tick % width;
    for index in 0..width {
        let ch = if index == cursor {
            '*'
        } else if (index + tick).is_multiple_of(7) {
            '^'
        } else if (index + tick).is_multiple_of(3) {
            '.'
        } else {
            '-'
        };
        chars.push(ch);
    }
    chars.into_iter().collect()
}

fn init_config_text(
    provider: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    max_spend: f64,
    sandbox: &str,
) -> String {
    let fallback = match provider {
        "cli:claude-code" => {
            "[\"cli:claude-code\", \"cli:codex\", \"anthropic\", \"openai\"]".to_string()
        }
        "cli:codex" => {
            "[\"cli:codex\", \"cli:claude-code\", \"anthropic\", \"openai\"]".to_string()
        }
        provider if provider.starts_with("cli:") => format!(
            "[\"{}\", \"cli:claude-code\", \"cli:codex\", \"anthropic\", \"openai\"]",
            escape_toml_string(provider)
        ),
        "openai" => "[\"openai\", \"anthropic\", \"cli:codex\", \"cli:claude-code\"]".to_string(),
        "openai-compatible" => "[\"openai-compatible\", \"openai\", \"anthropic\"]".to_string(),
        _ => "[\"anthropic\", \"openai\", \"cli:claude-code\", \"cli:codex\"]".to_string(),
    };
    let mut out = format!(
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\ndoc_provider = \"{provider}\"\ndoc_skill = \"run-narrator\"\ndoc_subskills = [\"narrator-overview\", \"narrator-phases\", \"narrator-as-built\", \"narrator-decisions\"]\ndoc_polish_token_budget = 16384\nmax_spend = {max_spend}\ncli_max_wall_seconds = 3600\nprevent_sleep = \"auto\"\nplain = false\nsandbox = \"{sandbox}\"\n\n"
    );
    match provider {
        "cli:claude-code" => {
            out.push_str("[providers.\"cli:claude-code\"]\nkind = \"cli-claude-code\"\nbinary = \"claude\"\nextra_args = []\n");
        }
        "cli:codex" => {
            out.push_str("[providers.\"cli:codex\"]\nkind = \"cli-codex\"\nbinary = \"codex\"\nextra_args = []\n");
        }
        provider if provider.starts_with("cli:") => {
            let provider = escape_toml_string(provider);
            out.push_str(&format!(
                "[providers.\"{provider}\"]\nkind = \"{provider}\"\nextra_args = []\n"
            ));
        }
        "openai" => {
            out.push_str("[providers.openai]\nkind = \"open-ai\"\n");
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"OPENAI_API_KEY\"\n");
            }
        }
        "openai-compatible" => {
            out.push_str("[providers.openai-compatible]\nkind = \"open-ai-compatible\"\n");
            if let Some(url) = base_url.filter(|url| !url.is_empty()) {
                out.push_str(&format!("base_url = \"{}\"\n", escape_toml_string(url)));
            }
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"OPENAI_COMPATIBLE_API_KEY\"\n");
            }
        }
        _ => {
            out.push_str("[providers.anthropic]\nkind = \"anthropic\"\n");
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"ANTHROPIC_API_KEY\"\n");
            }
        }
    }
    out
}

fn load_config_value(paths: &DeadreckonPaths) -> Result<toml::Value> {
    match fs::read_to_string(paths.config_path()) {
        Ok(raw) => Ok(toml::from_str(&raw)?),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Ok(toml::Value::Table(Default::default()))
        }
        Err(source) => Err(source.into()),
    }
}

fn get_toml_path<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cursor = root;
    for part in key.split('.') {
        cursor = cursor.get(part)?;
    }
    Some(cursor)
}

fn set_toml_path(root: &mut toml::Value, key: &str, value: toml::Value) {
    let parts = key.split('.').collect::<Vec<_>>();
    let mut cursor = root;
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !cursor.is_table() {
            *cursor = toml::Value::Table(Default::default());
        }
        let Some(table) = cursor.as_table_mut() else {
            return;
        };
        cursor = table
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()));
    }
    if !cursor.is_table() {
        *cursor = toml::Value::Table(Default::default());
    }
    if let Some(table) = cursor.as_table_mut()
        && let Some(last) = parts.last()
    {
        table.insert((*last).to_string(), value);
    }
}

fn parse_config_value(value: &str) -> toml::Value {
    toml::from_str::<toml::Value>(&format!("value = {value}"))
        .ok()
        .and_then(|mut doc| doc.as_table_mut().and_then(|table| table.remove("value")))
        .unwrap_or_else(|| toml::Value::String(value.to_string()))
}

fn value_to_display(value: &toml::Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| value.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
struct ParentMarker {
    schema_version: u32,
    kind: String,
    parent_run_id: String,
    parent_scope: String,
    parent_goal: String,
    parent_completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    materialized_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_turns_included: Option<u32>,
    deadreckon_version: String,
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|dir| {
            let path = dir.join(name);
            path.is_file()
        })
}

// SAFETY: Materialize arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn materialize_command(
    run_id: String,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let (state, plan_context, dest) = match load_cli_run(&paths, &run_id) {
        Ok(state) => (state, None, dest),
        Err(run_error) => match resolve_plan_result_run(&paths, &run_id, "export")? {
            Some(result) => {
                let dest = dest.or_else(|| Some(default_plan_materialize_dest(&result.plan)));
                (result.state, Some(result.plan), dest)
            }
            None => return Err(run_error),
        },
    };
    if let Some(plan) = plan_context.as_ref() {
        print_plan_result_context(plan, &state);
    }
    let materialized = materialize_completed_run(&paths, &state, dest, force, include_manifest)?;
    print_materialized(&materialized);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_command(
    run_id: Option<String>,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
    strategy: String,
    branch: Option<String>,
    autostash: bool,
    cleanup: bool,
    no_confirm: bool,
    message: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let requested = run_id.unwrap_or_else(|| "latest".to_string());
    let (state, plan_context, dest) = match load_cli_run(&paths, &requested) {
        Ok(state) => (state, None, dest),
        Err(run_error) => match resolve_plan_result_run(&paths, &requested, "finish")? {
            Some(result) => {
                if dest.is_none() && plan_apply_git_root(&result.plan)?.is_some() {
                    println!(
                        "{} {}",
                        ui_heading("finish:"),
                        ui_command(format!(
                            "deadreckon apply {}",
                            run_prefix(&result.plan.plan_id)
                        ))
                    );
                    return apply_command_inner(
                        requested, strategy, branch, no_confirm, autostash, cleanup, message,
                        false, false,
                    );
                }
                let dest =
                    Some(dest.unwrap_or_else(|| default_plan_materialize_dest(&result.plan)));
                (result.state, Some(result.plan), dest)
            }
            None => return Err(run_error),
        },
    };
    let finish_ref = plan_context
        .as_ref()
        .map(|plan| run_prefix(&plan.plan_id))
        .unwrap_or_else(|| run_prefix(&state.run_id));
    if let Some(plan) = plan_context.as_ref() {
        print_plan_result_context(plan, &state);
    }
    match state.status {
        RunStatus::Completed => {}
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is still {}", state.run_id, state.status),
                &format!("deadreckon attach {}", run_prefix(&state.run_id)),
            )));
        }
        RunStatus::Failed | RunStatus::Killed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is {}", state.run_id, state.status),
                &format!("deadreckon resume {}", run_prefix(&state.run_id)),
            )));
        }
    }

    let mode = read_codebase_record(&state.working_dir)
        .map(|record| record.mode)
        .unwrap_or(CodebaseMode::Fresh);
    match mode {
        CodebaseMode::Worktree => {
            println!(
                "{} {}",
                ui_heading("finish:"),
                ui_command(format!("deadreckon apply {}", run_prefix(&state.run_id)))
            );
            apply_command(
                state.run_id,
                strategy,
                branch,
                no_confirm,
                autostash,
                cleanup,
                message,
                false,
            )
        }
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            println!(
                "{} {}",
                ui_heading("finish:"),
                ui_command(format!("deadreckon export {finish_ref}"))
            );
            materialize_completed_run(&paths, &state, dest, force, include_manifest)
                .map(|materialized| print_materialized(&materialized))
        }
        CodebaseMode::InPlace => {
            println!(
                "{} {}",
                ui_ok("finished in-place run"),
                ui_id(&state.run_id)
            );
            println!("  working: {}", state.working_dir.display());
            println!(
                "  review:  {}",
                ui_command(format!("deadreckon show {}", run_prefix(&state.run_id)))
            );
            println!(
                "  docs:    {}",
                ui_command(format!(
                    "deadreckon doc {} --kind decisions",
                    run_prefix(&state.run_id)
                ))
            );
            println!(
                "  undo:    {}",
                ui_command(format!(
                    "deadreckon undo --run {}",
                    run_prefix(&state.run_id)
                ))
            );
            Ok(())
        }
    }
}

#[derive(Debug)]
struct MaterializedRun {
    run_id: String,
    source: PathBuf,
    dest: PathBuf,
}

fn materialize_completed_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<MaterializedRun> {
    ensure_completed_run(state, "run")?;
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        match record.mode {
            CodebaseMode::Worktree => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "export is for copy/fresh runs; run was worktree",
                    &format!("deadreckon apply {}", state.run_id),
                )));
            }
            CodebaseMode::InPlace => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "export is not needed; run edited the source in-place",
                    "deadreckon undo",
                )));
            }
            CodebaseMode::Copy | CodebaseMode::Fresh => {}
        }
    }
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if !library_dir.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library missing for run {}; was promotion successful?",
            state.run_id
        ))));
    }

    let dest = absolute_dest(dest.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(run_prefix(&state.run_id))
    }))?;
    refuse_dest_inside_home(paths, &dest, "export")?;
    prepare_empty_dest(&dest, force)?;

    copy_tree(&library_dir, &dest)?;
    if !include_manifest {
        remove_if_exists(&dest.join("manifest.json"))?;
    }
    remove_if_exists(&dest.join(".materialized-to"))?;
    write_parent_marker(
        &dest.join(".deadreckon").join("parent.json"),
        &materialized_parent_marker(state),
    )?;
    normalize_permissions(&dest)?;
    append_materialized_marker(&library_dir, &dest)?;

    Ok(MaterializedRun {
        run_id: state.run_id.clone(),
        source: library_dir,
        dest,
    })
}

fn print_materialized(materialized: &MaterializedRun) {
    println!("{} {}", ui_ok("exported run"), ui_id(&materialized.run_id));
    println!("  source: {}", materialized.source.display());
    println!("  dest:   {}", materialized.dest.display());
}

#[allow(clippy::too_many_arguments)]
fn apply_command(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
    plain: bool,
) -> Result<()> {
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        false,
        plain,
    )
}

fn apply_command_quiet(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
) -> Result<()> {
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        true,
        false,
    )
}

// SAFETY: Apply arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
fn apply_command_inner(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
    quiet: bool,
    plain: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match load_cli_run(&paths, &run_id) {
        Ok(state) => state,
        Err(run_error) => match resolve_plan_result_run(&paths, &run_id, "apply")? {
            Some(result) => {
                if !quiet {
                    print_plan_result_context(&result.plan, &result.state);
                }
                prepare_plan_result_apply_state(&paths, &result.plan, &result.state)?
            }
            None => return Err(run_error),
        },
    };
    if state.status != RunStatus::Completed {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("run {} is {}", state.run_id, state.status),
            &format!("deadreckon resume {}", state.run_id),
        )));
    }
    let record = read_codebase_record(&state.working_dir)?;
    if record.mode != CodebaseMode::Worktree {
        return Err(CliError::Core(apply_mode_error(&state.run_id, record.mode)));
    }
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    let target =
        target_branch.unwrap_or(git_stdout(git_root, &["symbolic-ref", "--short", "HEAD"])?);
    let diff_stat = git_stdout(
        git_root,
        &["diff", "--stat", &format!("{target}..{branch}")],
    )
    .unwrap_or_default();
    if diff_stat.trim().is_empty() {
        if !quiet {
            print_already_applied(&state, branch, &target);
        }
        let summary =
            (!quiet).then(|| render_exit_summary_card(&state, &RunLoopOutcome::Done, plain));
        finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)?;
        if let Some(summary) = summary {
            print!("{summary}");
        }
        return Ok(());
    }
    if !quiet {
        eprintln!(
            "{}",
            ui::render(ui::Stream::Stderr, ui::Tone::Heading, "changes to apply:")
        );
        eprintln!("{diff_stat}");
    }

    if !no_confirm && io::stdin().is_terminal() {
        if !prompt::confirm("apply these changes?", true)? {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm && !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive apply requires --no-confirm",
            &format!("deadreckon apply {} --no-confirm", state.run_id),
        )));
    }

    let autostash = prepare_apply_autostash(git_root, &state.run_id, autostash, no_confirm)?;

    let (commit_subject, commit_body) = match message {
        Some(message) => (message, None),
        None => (
            format!(
                "{} (deadreckon run {})",
                state.goal.lines().next().unwrap_or("deadreckon run"),
                state.run_id.chars().take(8).collect::<String>()
            ),
            Some(apply_commit_body(&state)),
        ),
    };
    let full_merge_message = commit_body
        .as_ref()
        .map(|body| format!("{commit_subject}\n\n{body}"))
        .unwrap_or_else(|| commit_subject.clone());
    match strategy.as_str() {
        "merge" => git_status(
            git_root,
            &["merge", "--no-ff", branch, "-m", &full_merge_message],
        )
        .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?,
        "squash" => {
            git_status(git_root, &["merge", "--squash", branch])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?;
            let staged_stat = git_stdout(git_root, &["diff", "--cached", "--stat"])?;
            if staged_stat.trim().is_empty() {
                if let Some(stash) = autostash.as_ref() {
                    restore_apply_autostash(git_root, &state.run_id, stash)?;
                }
                if !quiet {
                    print_already_applied(&state, branch, &target);
                }
                let summary = (!quiet)
                    .then(|| render_exit_summary_card(&state, &RunLoopOutcome::Done, plain));
                finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)?;
                if let Some(summary) = summary {
                    print!("{summary}");
                }
                return Ok(());
            }
            if let Some(body) = commit_body.as_deref() {
                git_status(git_root, &["commit", "-m", &commit_subject, "-m", body])?;
            } else {
                git_status(git_root, &["commit", "-m", &commit_subject])?;
            }
        }
        "cherry-pick" => {
            let base = record.base_sha.as_deref().unwrap_or("HEAD");
            git_status(git_root, &["cherry-pick", &format!("{base}..{branch}")])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, &err))?;
        }
        other => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown git apply strategy {other}"
            ))));
        }
    }
    if let Some(stash) = autostash.as_ref() {
        restore_apply_autostash(git_root, &state.run_id, stash)?;
    }
    if !quiet {
        println!(
            "{} {} into {}",
            ui_ok("applied"),
            ui_id(&state.run_id),
            target
        );
        println!("{}", git_stdout(git_root, &["log", "-1", "--stat"])?);
    }
    let summary = (!quiet).then(|| render_exit_summary_card(&state, &RunLoopOutcome::Done, plain));
    finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)?;
    if let Some(summary) = summary {
        print!("{summary}");
    }
    Ok(())
}

fn print_already_applied(state: &deadreckon_core::PipelineState, branch: &str, target: &str) {
    println!(
        "{} {} into {}",
        ui_ok("already applied"),
        ui_id(&state.run_id),
        target
    );
    println!("  run branch:    {branch}");
    println!("  target branch: {target}");
    println!("  reason: no file changes remain between the run branch and target branch");
}

fn finish_apply_cleanup(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    cleanup: bool,
    no_confirm: bool,
    quiet: bool,
) -> Result<()> {
    let cleanup_now = cleanup || should_prompt_cleanup(no_confirm)?;
    if cleanup_now {
        cleanup_worktree_run(state, record, false, false, CleanupReason::Applied)?;
    } else if !quiet {
        println!(
            "{} {}",
            ui_command("next:"),
            ui_command(format!("deadreckon cleanup {}", run_prefix(&state.run_id)))
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ApplyAutoStash {
    refname: String,
}

fn prepare_apply_autostash(
    git_root: &Path,
    run_id: &str,
    requested: bool,
    no_confirm: bool,
) -> Result<Option<ApplyAutoStash>> {
    let dirty = git_stdout(git_root, &["status", "--porcelain"])?;
    if dirty.trim().is_empty() {
        return Ok(None);
    }

    eprintln!(
        "{}",
        ui::render(
            ui::Stream::Stderr,
            ui::Tone::Warn,
            "working tree has uncommitted changes:",
        )
    );
    for line in dirty.lines().take(30) {
        eprintln!("  {line}");
    }
    if dirty.lines().count() > 30 {
        eprintln!("  ...");
    }

    let mut should_stash = requested;
    if !should_stash && !no_confirm && io::stdin().is_terminal() {
        should_stash =
            prompt::confirm("stash these changes during apply and restore after?", true)?;
    }

    if !should_stash {
        return Err(CliError::Core(deadreckon_core::user_error(
            "your working tree has uncommitted changes",
            &apply_dirty_hint(run_id, no_confirm),
        )));
    }

    let marker = format!(
        "deadreckon apply {} autostash {}",
        run_prefix(run_id),
        Utc::now().timestamp_millis()
    );
    git_status(git_root, &["stash", "push", "-u", "-m", &marker])?;
    let refname = find_stash_by_marker(git_root, &marker)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "git stash succeeded but the new stash could not be found".to_string(),
        ))
    })?;
    eprintln!("stashed local changes as {refname}");
    Ok(Some(ApplyAutoStash { refname }))
}

fn apply_dirty_hint(run_id: &str, no_confirm: bool) -> String {
    let mut hint = format!("deadreckon apply {run_id} --autostash");
    if no_confirm {
        hint.push_str(" --no-confirm");
    }
    hint
}

fn find_stash_by_marker(git_root: &Path, marker: &str) -> Result<Option<String>> {
    let output = git_stdout(git_root, &["stash", "list", "--format=%gd%x00%s"])?;
    for line in output.lines() {
        if let Some((refname, subject)) = line.split_once('\0')
            && subject.contains(marker)
        {
            return Ok(Some(refname.to_string()));
        }
    }
    Ok(None)
}

fn restore_apply_autostash(git_root: &Path, run_id: &str, stash: &ApplyAutoStash) -> Result<()> {
    git_status(git_root, &["stash", "pop", &stash.refname]).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("applied {run_id}, but restoring autostash produced conflicts: {err}"),
            &format!(
                "resolve conflicts, then inspect `git stash list` before dropping {}",
                stash.refname
            ),
        ))
    })
}

fn apply_merge_error(run_id: &str, autostash: &Option<ApplyAutoStash>, err: &CliError) -> CliError {
    CliError::Core(deadreckon_core::user_error(
        &format!("merge produced conflicts: {err}"),
        &apply_conflict_hint(run_id, autostash),
    ))
}

fn apply_conflict_hint(run_id: &str, autostash: &Option<ApplyAutoStash>) -> String {
    let mut hint = format!("resolve, then git commit && deadreckon cleanup {run_id}");
    if let Some(stash) = autostash {
        hint.push_str(&format!(
            "; restore local changes with git stash pop {}",
            stash.refname
        ));
    }
    hint
}

fn should_prompt_cleanup(no_confirm: bool) -> Result<bool> {
    if no_confirm || !io::stdin().is_terminal() {
        return Ok(false);
    }
    prompt::confirm("remove deadreckon worktree and temporary branch now?", true)
}

// SAFETY: Abandon arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn abandon_command(run_id: String, keep_branch: bool, force: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        println!("nothing to abandon for run {}", state.run_id);
        return Ok(());
    };
    if record.mode == CodebaseMode::InPlace {
        return Err(CliError::Core(deadreckon_core::user_error(
            "cannot abandon in-place edits",
            "deadreckon undo",
        )));
    }
    if state.status == RunStatus::Executing {
        if !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is {}", state.run_id, run_status_label(state.status)),
                &format!("deadreckon kill {} --escalate", state.run_id),
            )));
        }
        let _ = kill_loaded_run(&paths, &mut state, true);
    }
    cleanup_worktree_run(
        &state,
        &record,
        keep_branch,
        force,
        CleanupReason::Abandoned,
    )
}

struct CleanupCommandRequest {
    run_id: Option<String>,
    all: bool,
    completed: bool,
    stale: bool,
    no_confirm: bool,
    escalate: bool,
    overwrite: bool,
    keep_branch: bool,
}

fn cleanup_command(args: CleanupCommandRequest) -> Result<()> {
    let CleanupCommandRequest {
        run_id,
        all,
        completed,
        stale,
        no_confirm,
        escalate,
        overwrite,
        keep_branch,
    } = args;
    let paths = DeadreckonPaths::discover();
    if let Some(run_id) = run_id {
        let mut state = load_cli_run(&paths, &run_id)?;
        if state.status == RunStatus::Executing {
            if !escalate {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("run {} is {}", state.run_id, run_status_label(state.status)),
                    &format!("deadreckon cleanup {} --escalate", state.run_id),
                )));
            }
            let _ = kill_loaded_run(&paths, &mut state, escalate);
        }
        let record = read_codebase_record(&state.working_dir)?;
        cleanup_worktree_run(
            &state,
            &record,
            keep_branch,
            overwrite,
            CleanupReason::Cleaned,
        )?;
        return Ok(());
    }

    let candidates = cleanup_candidates(&paths, all, completed, stale)?;
    if candidates.is_empty() {
        println!("no cleanup candidates");
        if !completed {
            let _ = ui::hint(
                ui::Stream::Stderr,
                "use `deadreckon cleanup --completed` to discard completed worktree runs",
            );
        }
        if !all {
            let _ = ui::hint(
                ui::Stream::Stderr,
                "use `deadreckon cleanup --all-scopes` to search every project",
            );
        }
        return Ok(());
    }

    println!("cleanup candidates:");
    for candidate in &candidates {
        println!(
            "  {:<8} {:<10} {:<16} {}",
            run_prefix(&candidate.state.run_id),
            candidate.state.status,
            candidate.reason,
            one_line(&candidate.state.goal, 72)
        );
    }
    if !no_confirm {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive cleanup requires --no-confirm",
                "deadreckon cleanup --no-confirm",
            )));
        }
        if !prompt::confirm("clean these runs?", false)? {
            println!("cancelled");
            return Ok(());
        }
    }

    for mut candidate in candidates {
        if candidate.state.status == RunStatus::Executing {
            let _ = kill_loaded_run(&paths, &mut candidate.state, escalate);
        }
        cleanup_worktree_run(
            &candidate.state,
            &candidate.record,
            keep_branch,
            overwrite,
            CleanupReason::Cleaned,
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct CleanupCandidate {
    state: deadreckon_core::PipelineState,
    record: CodebaseRecord,
    reason: String,
}

fn cleanup_candidates(
    paths: &DeadreckonPaths,
    all: bool,
    include_completed: bool,
    include_stale: bool,
) -> Result<Vec<CleanupCandidate>> {
    let scope = if all { None } else { Some(current_scope()?) };
    let mut candidates = Vec::new();
    for run in list_runs(paths, scope.as_deref())? {
        let Ok(state) = load_run(paths, &run.run_id) else {
            continue;
        };
        let Ok(record) = read_codebase_record(&state.working_dir) else {
            continue;
        };
        if record.mode != CodebaseMode::Worktree {
            continue;
        }
        let abandoned = state.run_root.join("abandoned.json").exists();
        let stale = include_stale && is_stale_executing(&state);
        let completed = include_completed && state.status == RunStatus::Completed && !abandoned;
        if abandoned || stale || completed {
            let reason = if abandoned {
                "cleaned".to_string()
            } else if stale {
                "stale".to_string()
            } else {
                "completed".to_string()
            };
            candidates.push(CleanupCandidate {
                state,
                record,
                reason,
            });
        }
    }
    Ok(candidates)
}

#[derive(Debug, Clone, Copy)]
enum CleanupReason {
    Abandoned,
    Applied,
    Cleaned,
}

impl CleanupReason {
    fn marker(self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::Applied => "applied",
            Self::Cleaned => "cleaned",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::Applied | Self::Cleaned => "cleaned",
        }
    }
}

fn cleanup_worktree_run(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    keep_branch: bool,
    force: bool,
    reason: CleanupReason,
) -> Result<()> {
    let mut removed = Vec::new();
    if record.mode == CodebaseMode::Worktree
        && let (Some(git_root), Some(worktree)) = (
            record.source_git_root.as_ref(),
            record.worktree_path.as_ref(),
        )
    {
        if worktree.exists() {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(path_to_str(worktree)?);
            let _ = git_status(git_root, &args);
            removed.push(worktree.display().to_string());
        }
        if !keep_branch
            && let Some(branch) = record.branch_name.as_deref()
            && git_stdout(git_root, &["rev-parse", "--verify", branch]).is_ok()
        {
            let _ = git_status(git_root, &["branch", "-D", branch]);
            removed.push(format!("branch {branch}"));
        }
    }
    write_abandoned_marker(state, reason)?;
    println!("{} {}", ui_ok(reason.label()), ui_id(&state.run_id));
    for item in removed {
        println!("  removed: {item}");
    }
    Ok(())
}

fn apply_mode_error(run_id: &str, mode: CodebaseMode) -> DeadreckonError {
    let hint = match mode {
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            format!("deadreckon export {run_id} --dest <path>")
        }
        CodebaseMode::InPlace => "deadreckon undo to revert if needed".to_string(),
        CodebaseMode::Worktree => format!("deadreckon apply {run_id}"),
    };
    deadreckon_core::user_error(
        &format!("apply requires worktree mode; run was {mode}"),
        &hint,
    )
}

async fn extend_command(args: ExtendCommandArgs) -> Result<()> {
    let ExtendCommandArgs {
        parent_run_id,
        new_goal,
        dest,
        max_context_turns,
        no_context,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        no_docs,
        doc_skill,
        post_actions,
    } = args;
    let new_goal = new_goal.trim().to_string();
    if new_goal.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--goal must be non-empty".to_string(),
        )));
    }

    let paths = DeadreckonPaths::discover();
    let parent = load_cli_run(&paths, &parent_run_id)?;
    if parent.status != RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent {} is {}; use 'deadreckon resume' for incomplete runs",
            parent.run_id, parent.status
        ))));
    }
    let parent_codebase = read_run_codebase_record(&paths, &parent).ok();
    if parent_codebase
        .as_ref()
        .is_some_and(|record| record.mode == CodebaseMode::InPlace)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "extend is not available for in-place runs",
            &format!(
                "deadreckon run --in-place --i-know-its-a-lot {:?}",
                new_goal
            ),
        )));
    }
    let parent_library = paths.library_dir(&parent.scope, &parent.run_id);
    if !parent_library.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(
            "parent library missing; cannot extend".to_string(),
        )));
    }

    let defaults = config_defaults(&paths)?;
    let primary_setup = provider_setup_selection(
        &paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::PrimaryRun,
            explicit_provider: provider.as_deref(),
            explicit_model: model.as_deref(),
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: true,
            allow_auto_subscription: false,
            require_usable_route: false,
        },
    )?;
    let provider_override = provider_override_from_setup(&primary_setup);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        provider_override.as_deref(),
        model.as_deref(),
    )?;
    let selected_route = router.selected_route_info();
    let effective_provider = selected_route
        .as_ref()
        .map(|route| route.name.clone())
        .or(primary_setup.provider.clone());
    let effective_max_spend = max_spend.or(defaults.max_spend).or(Some(10.0));
    let effective_max_wall_seconds = max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .or(Some(3600.0));
    let effective_doc_skill = doc_skill
        .or(defaults.doc_skill.clone())
        .unwrap_or_else(|| "run-narrator".to_string());
    let doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup_selection(
        &paths,
        &defaults,
        None,
        effective_provider.as_deref(),
        false,
    )?);
    let doc_subskills = effective_doc_subskills(&defaults);
    let doc_token_budget = defaults
        .doc_polish_token_budget
        .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
    let doc_budget_cap = defaults.doc_polish_budget_cap_usd;
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    let cwd = if parent.cwd.exists() {
        parent.cwd.clone()
    } else {
        std::env::current_dir()?
    };
    let context_turns = context_turns(max_context_turns, no_context);
    if let Some(parent_record) = parent_codebase
        .as_ref()
        .filter(|record| record.mode == CodebaseMode::Worktree)
    {
        return extend_worktree_command(ExtendWorktreeArgs {
            paths,
            parent,
            parent_record: parent_record.clone(),
            new_goal,
            effective_provider,
            effective_max_spend,
            effective_max_wall_seconds,
            doc_provider: doc_provider_selection.provider.clone(),
            doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
            doc_subskills: doc_subskills.clone(),
            doc_token_budget,
            doc_budget_cap,
            doc_skill: effective_doc_skill,
            no_docs,
            backend,
            router,
            selected_route: selected_route.clone(),
            provider_source: primary_setup.source.as_str().to_string(),
            post_actions,
            context_turns,
        })
        .await;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: None,
            codebase: None,
        },
    )?;
    align_extended_run_with_parent(&paths, &mut state, &parent)?;

    let mut lock = match acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    ) {
        Ok(lock) => lock,
        Err(error) => {
            cleanup_new_run(&state);
            return Err(error.into());
        }
    };
    deadreckon_core::state::write_current_pointer(&paths, &state)?;

    if let Some(dest) = dest {
        let dest = absolute_dest(dest)?;
        refuse_dest_inside_home(&paths, &dest, "extend")?;
        prepare_empty_dest(&dest, false)?;
        remove_if_exists(&state.working_dir)?;
        state.working_dir = dest;
    }
    seed_working_from_library(&parent_library, &state.working_dir)?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        &extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    copy_existing_acceptance_into_run(&state, &[&state.cwd, &state.working_dir])?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    print_run_started(
        &state,
        selected_route.as_ref(),
        primary_setup.source.as_str(),
        doc_provider_selection.provider.as_deref(),
        doc_provider_selection.source.as_str(),
    );
    let wait_label = format!(
        "extended run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let outcome = with_cli_wait_status(
        &wait_label,
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: effective_provider,
                max_spend_usd: effective_max_spend,
                max_wall_seconds: effective_max_wall_seconds,
                sandbox_backend: backend,
                max_turns: 12,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(paths.config_path()),
                    doc_provider: doc_provider_selection.provider.clone(),
                    doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                    doc_subskills,
                    token_budget: doc_token_budget,
                    budget_cap_usd: doc_budget_cap,
                    doc_skill: effective_doc_skill,
                    no_docs,
                },
            },
        ),
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    print_extended_run_outcome(&state, &outcome);
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true)).await?;
    }
    Ok(())
}

struct ExtendWorktreeArgs {
    paths: DeadreckonPaths,
    parent: deadreckon_core::PipelineState,
    parent_record: CodebaseRecord,
    new_goal: String,
    effective_provider: Option<String>,
    effective_max_spend: Option<f64>,
    effective_max_wall_seconds: Option<f64>,
    doc_provider: Option<String>,
    doc_provider_source: Option<String>,
    doc_subskills: Vec<String>,
    doc_token_budget: u32,
    doc_budget_cap: Option<f64>,
    doc_skill: String,
    no_docs: bool,
    backend: SandboxBackend,
    router: ProviderRouter,
    selected_route: Option<ProviderRouteInfo>,
    provider_source: String,
    post_actions: bool,
    context_turns: Option<u32>,
}

async fn extend_worktree_command(args: ExtendWorktreeArgs) -> Result<()> {
    let ExtendWorktreeArgs {
        paths,
        parent,
        parent_record,
        new_goal,
        effective_provider,
        effective_max_spend,
        effective_max_wall_seconds,
        doc_provider,
        doc_provider_source,
        doc_subskills,
        doc_token_budget,
        doc_budget_cap,
        doc_skill,
        no_docs,
        backend,
        router,
        selected_route,
        provider_source,
        post_actions,
        context_turns,
    } = args;
    let parent_branch = parent_record.branch_name.clone();
    let source_git_root = parent_record.source_git_root.clone().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent worktree record missing source_git_root".to_string(),
        ))
    })?;
    let base_ref = parent_branch
        .as_deref()
        .filter(|branch| git_ref_exists(&source_git_root, &format!("refs/heads/{branch}")))
        .map(str::to_string);
    let run_id = Uuid::new_v4().simple().to_string();
    let mut codebase = prepare_worktree_record(
        &paths,
        WorktreeOptions {
            run_id: run_id.clone(),
            task_key: deadreckon_core::paths::task_key(&new_goal),
            source_path: source_git_root.clone(),
            base_ref,
            branch_name: None,
            allow_dirty: false,
        },
    )?;
    codebase.parent_branch = parent_branch.or_else(|| codebase.base_ref.clone());
    create_worktree(&codebase)?;
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd: source_git_root,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase.clone()),
        },
    )?;

    let mut lock = acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        &extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    copy_existing_acceptance_into_run(&state, &[&state.cwd, &state.working_dir])?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
                "mode": "worktree",
                "base_ref": codebase.base_ref.clone(),
                "parent_branch": codebase.parent_branch.clone(),
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    print_run_started(
        &state,
        selected_route.as_ref(),
        &provider_source,
        doc_provider.as_deref(),
        doc_provider_source.as_deref().unwrap_or("none"),
    );
    let wait_label = format!(
        "extended run {} running; attach in another terminal",
        run_prefix(&state.run_id)
    );
    let outcome = with_cli_wait_status(
        &wait_label,
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: effective_provider,
                max_spend_usd: effective_max_spend,
                max_wall_seconds: effective_max_wall_seconds,
                sandbox_backend: backend,
                max_turns: 12,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(paths.config_path()),
                    doc_provider,
                    doc_provider_source,
                    doc_subskills,
                    token_budget: doc_token_budget,
                    budget_cap_usd: doc_budget_cap,
                    doc_skill,
                    no_docs,
                },
            },
        ),
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    print_extended_run_outcome(&state, &outcome);
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true)).await?;
    }
    Ok(())
}

fn run_loop_outcome_status(outcome: &RunLoopOutcome) -> &'static str {
    match outcome {
        RunLoopOutcome::Done => "completed",
        RunLoopOutcome::PausedAtCap => "paused",
        RunLoopOutcome::Killed => "killed",
        RunLoopOutcome::Failed => "failed",
    }
}

fn print_extended_run_outcome(state: &deadreckon_core::PipelineState, outcome: &RunLoopOutcome) {
    let status = run_loop_outcome_status(outcome);
    println!(
        "{} extended run {}",
        ui_status(status),
        ui_id(&state.run_id)
    );
}

fn align_extended_run_with_parent(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
) -> Result<()> {
    let old_scope = state.scope.clone();
    let old_task_key = state.task_key.clone();
    let old_pointer = paths.current_pointer_path(&old_scope, &old_task_key);
    let desired_root = paths.run_root(&parent.scope, &state.run_id);
    if state.run_root != desired_root {
        if let Some(parent_dir) = desired_root.parent() {
            fs::create_dir_all(parent_dir)?;
        }
        fs::rename(&state.run_root, &desired_root)?;
        state.run_root = desired_root;
        state.working_dir = state.run_root.join("working");
        state.scope = parent.scope.clone();
    }
    state.task_key = parent.task_key.clone();
    state.cwd = parent.cwd.clone();
    state.updated_at = Utc::now();
    let new_pointer = paths.current_pointer_path(&state.scope, &state.task_key);
    if old_pointer != new_pointer {
        remove_if_exists(&old_pointer)?;
    }
    save_state(state)?;
    Ok(())
}

fn cleanup_new_run(state: &deadreckon_core::PipelineState) {
    let _ = remove_if_exists(&state.run_root);
}

fn seed_working_from_library(library_dir: &Path, working_dir: &Path) -> Result<()> {
    copy_tree(library_dir, working_dir)?;
    remove_if_exists(&working_dir.join("manifest.json"))?;
    remove_if_exists(&working_dir.join(".materialized-to"))?;
    Ok(())
}

fn context_turns(max_context_turns: Option<u32>, no_context: bool) -> Option<u32> {
    if no_context {
        return None;
    }
    let turns = max_context_turns.unwrap_or(5);
    if turns == 0 { None } else { Some(turns) }
}

fn extended_parent_marker(
    parent: &deadreckon_core::PipelineState,
    new_goal: &str,
    context_turns: Option<u32>,
) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "extended".to_string(),
        parent_run_id: parent.run_id.clone(),
        parent_scope: parent.scope.clone(),
        parent_goal: parent.goal.clone(),
        parent_completed_at: parent.updated_at,
        materialized_at: None,
        extended_at: Some(Utc::now()),
        new_goal: Some(new_goal.to_string()),
        context_turns_included: context_turns,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_history(
    state: &deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
    context_turns: Option<u32>,
) -> Result<()> {
    let history = vec![parent_summary(parent, context_turns)];
    fs::write(
        state.run_root.join("history.json"),
        serde_json::to_vec_pretty(&history)?,
    )?;
    Ok(())
}

fn parent_summary(parent: &deadreckon_core::PipelineState, context_turns: Option<u32>) -> String {
    let spend = read_jsonl::<SpendRecord>(&parent.run_root.join("spend.jsonl")).unwrap_or_default();
    let traces =
        read_jsonl::<TraceRecord>(&parent.run_root.join("traces.jsonl")).unwrap_or_default();
    let spend_label = if spend.iter().any(|record| record.subscription)
        || parent
            .provider
            .as_deref()
            .is_some_and(|provider| provider.starts_with("cli:"))
    {
        "subscription".to_string()
    } else {
        format!("${:.6}", parent.total_spend_usd)
    };
    let acceptance = if parent.run_root.join("proofs/turn-acceptance.json").exists() {
        "dr-gate accepted"
    } else {
        "not recorded"
    };
    let mut summary = format!(
        "# Previous run summary ({})\n\n**Original goal.** {}\n**Completed.** {}\n**Total turns.** {}\n**Total spend.** {}\n**Acceptance.** {}\n",
        parent.run_id,
        parent.goal,
        parent.updated_at.to_rfc3339(),
        parent.turn,
        spend_label,
        acceptance
    );
    if let Some(max_turns) = context_turns {
        let mut recent = traces
            .iter()
            .filter(|trace| trace.turn > 0)
            .rev()
            .take(max_turns as usize)
            .map(trace_one_liner)
            .collect::<Vec<_>>();
        recent.reverse();
        summary.push_str(&format!(
            "\n## Recent activity (last {} turns)\n\n",
            max_turns
        ));
        if recent.is_empty() {
            summary.push_str("- no trace activity recorded\n");
        } else {
            for line in recent {
                summary.push_str(&format!("- {line}\n"));
            }
        }
    }
    summary
}

fn trace_one_liner(trace: &TraceRecord) -> String {
    let detail = trace
        .detail
        .get("tool_call_id")
        .and_then(Value::as_str)
        .or_else(|| trace.detail.get("summary").and_then(Value::as_str))
        .unwrap_or("");
    if detail.is_empty() {
        format!("turn {}: {}", trace.turn, trace.event)
    } else {
        format!(
            "turn {}: {} {}",
            trace.turn,
            trace.event,
            one_line(detail, 90)
        )
    }
}

fn ensure_completed_run(state: &deadreckon_core::PipelineState, label: &str) -> Result<()> {
    if state.status != RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} {} is not completed (status={}); use 'deadreckon resume' first",
            state.run_id, state.status
        ))));
    }
    Ok(())
}

fn materialized_parent_marker(state: &deadreckon_core::PipelineState) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "materialized".to_string(),
        parent_run_id: state.run_id.clone(),
        parent_scope: state.scope.clone(),
        parent_goal: state.goal.clone(),
        parent_completed_at: state.updated_at,
        materialized_at: Some(Utc::now()),
        extended_at: None,
        new_goal: None,
        context_turns_included: None,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_marker(path: &Path, marker: &ParentMarker) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(marker)?)?;
    Ok(())
}

fn append_materialized_marker(library_dir: &Path, dest: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(library_dir.join(".materialized-to"))?;
    writeln!(file, "{}\t{}", Utc::now().to_rfc3339(), dest.display())?;
    Ok(())
}

fn write_abandoned_marker(
    state: &deadreckon_core::PipelineState,
    reason: CleanupReason,
) -> Result<()> {
    let path = state.run_root.join("abandoned.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "run_id": state.run_id,
            "abandoned_at": Utc::now(),
            "reason": reason.marker(),
        }))?,
    )?;
    Ok(())
}

fn prepare_empty_dest(dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        let non_empty = !path_is_empty_dir(dest)?;
        if non_empty && !force {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "dest {} is not empty (use --overwrite or pass a fresh path)",
                dest.display()
            ))));
        }
        if force {
            remove_if_exists(dest)?;
        }
    }
    fs::create_dir_all(dest)?;
    Ok(())
}

fn path_is_empty_dir(path: &Path) -> Result<bool> {
    if path.is_dir() {
        Ok(fs::read_dir(path)?.next().is_none())
    } else {
        Ok(false)
    }
}

fn default_materialize_dest(state: &deadreckon_core::PipelineState) -> PathBuf {
    state
        .cwd
        .join(state.task_key.chars().take(24).collect::<String>())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(CliError::from),
        Ok(_) => fs::remove_file(path).map_err(CliError::from),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn absolute_dest(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn refuse_dest_inside_home(paths: &DeadreckonPaths, dest: &Path, verb: &str) -> Result<()> {
    let home = paths.home();
    if dest.starts_with(home) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "refusing to {verb} back into runstate (pick a path outside ~/.deadreckon/)"
        ))));
    }
    Ok(())
}

fn run_prefix(run_id: &str) -> String {
    id_prefix(run_id)
}

fn id_prefix(id: &str) -> String {
    id.chars().take(8).collect()
}

fn normalize_permissions(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        normalize_permissions_inner(root)?;

        fn normalize_permissions_inner(path: &Path) -> Result<()> {
            let metadata = fs::symlink_metadata(path)?;
            let mut permissions = metadata.permissions();
            if metadata.is_dir() {
                permissions.set_mode(0o755);
                fs::set_permissions(path, permissions)?;
                for entry in fs::read_dir(path)? {
                    normalize_permissions_inner(&entry?.path())?;
                }
            } else if metadata.is_file() {
                permissions.set_mode(0o644);
                fs::set_permissions(path, permissions)?;
            }
            Ok(())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
    Ok(())
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn list_command(
    scope: Option<String>,
    all: bool,
    _full: bool,
    _plain: bool,
    json_output: bool,
) -> Result<()> {
    // REPORT.md: Workspace Inventory & Run Queue is a local scan over durable
    // runstate, not a live daemon query.
    let paths = DeadreckonPaths::discover();
    let effective_scope = if all {
        None
    } else {
        Some(scope.unwrap_or(current_scope()?))
    };
    let runs = list_runs(&paths, effective_scope.as_deref())?;
    let plans = list_plan_entries(&paths, effective_scope.as_deref())?;
    if json_output {
        let runs = runs
            .iter()
            .map(|run| {
                json!({
                    "run_id": &run.run_id,
                    "scope": &run.scope,
                    "goal": &run.goal,
                    "status": run_status_label(run.status),
                    "updated_at": run.updated_at,
                    "state_path": &run.state_path,
                })
            })
            .collect::<Vec<_>>();
        let plans = plans
            .iter()
            .map(|plan| {
                json!({
                    "plan_id": &plan.plan_id,
                    "scope": &plan.scope,
                    "goal": &plan.goal,
                    "status": plan_status_label(plan.status),
                    "mode": plan_mode_label(plan.mode),
                    "updated_at": plan.updated_at,
                    "plan_path": &plan.plan_path,
                    "children": {
                        "completed": plan.completed_children,
                        "total": plan.total_children,
                    },
                    "result_run_id": &plan.merged_run_id,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "list",
                "id": effective_scope.as_deref().unwrap_or("all-scopes"),
                "status": "ok",
                "next_actions": [
                    "deadreckon status latest",
                    "deadreckon attach <id>",
                    "deadreckon show <id>",
                ],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                },
                "runs": runs,
                "plans": plans,
            }))?
        );
        return Ok(());
    }
    let mut entries = runs
        .into_iter()
        .map(ListEntry::Run)
        .chain(plans.into_iter().map(ListEntry::Plan))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| Reverse(entry.updated_at()));
    if entries.is_empty() {
        match effective_scope.as_deref() {
            Some(scope) => {
                println!("no runs for current project ({scope})");
                let _ = ui::hint(
                    ui::Stream::Stderr,
                    "use `deadreckon list --all` to see every project",
                );
            }
            None => println!("no runs"),
        }
        return Ok(());
    }
    let header = list_header();
    println!("{}", ui_heading(header));
    let goal_width = list_goal_width();
    for entry in entries {
        match entry {
            ListEntry::Run(run) => {
                print_list_row(&ListRow {
                    id: run_prefix(&run.run_id),
                    status: run_status_label(run.status).to_string(),
                    age: relative_age(run.updated_at),
                    scope: run.scope.clone(),
                    kind: "run".to_string(),
                    mode: codebase_mode_status(&paths, &run),
                    action: next_action_label_for_entry(&paths, &run),
                    goal: run.goal.clone(),
                    goal_width,
                    orchestration: false,
                });
            }
            ListEntry::Plan(plan) => {
                print_list_row(&ListRow {
                    id: run_prefix(&plan.plan_id),
                    status: plan_status_label(plan.status).to_string(),
                    age: relative_age(plan.updated_at),
                    scope: plan.scope.clone(),
                    kind: "orchestrate".to_string(),
                    mode: plan_mode_label(plan.mode).to_string(),
                    action: plan_action_label(&plan),
                    goal: plan.goal.clone(),
                    goal_width,
                    orchestration: true,
                });
            }
        }
    }
    println!("{} run and plan ids accept prefixes", ui_muted("hint:"));
    println!(
        "      use `{}`, `{}`, `{}`, or `{}`",
        ui_command("deadreckon status latest"),
        ui_command("deadreckon list --all"),
        ui_command("deadreckon attach <id>"),
        ui_command("deadreckon show <id>")
    );
    Ok(())
}

const LIST_ID_WIDTH: usize = 8;
const LIST_STATUS_WIDTH: usize = 10;
const LIST_AGE_WIDTH: usize = 7;
const LIST_SCOPE_WIDTH: usize = 24;
const LIST_KIND_WIDTH: usize = 13;
const LIST_MODE_WIDTH: usize = 10;
const LIST_ACTION_WIDTH: usize = 16;
const LIST_GOAL_MAX_LINES: usize = 4;

struct ListRow {
    id: String,
    status: String,
    age: String,
    scope: String,
    kind: String,
    mode: String,
    action: String,
    goal: String,
    goal_width: usize,
    orchestration: bool,
}

fn list_header() -> String {
    format!(
        "{}  {}  {}  {}  {}  {}  {}  GOAL",
        pad_plain("ID", LIST_ID_WIDTH),
        pad_plain("STATUS", LIST_STATUS_WIDTH),
        pad_plain("AGE", LIST_AGE_WIDTH),
        pad_plain("SCOPE", LIST_SCOPE_WIDTH),
        pad_plain("KIND", LIST_KIND_WIDTH),
        pad_plain("MODE", LIST_MODE_WIDTH),
        pad_plain("ACTION", LIST_ACTION_WIDTH)
    )
}

fn print_list_row(row: &ListRow) {
    let first_prefix = format!(
        "{}  {}  {}  {}  {}  {}  {}  ",
        pad_rendered(&row.id, LIST_ID_WIDTH, Some(ui_id)),
        pad_plain(&row.status, LIST_STATUS_WIDTH),
        pad_plain(&row.age, LIST_AGE_WIDTH),
        pad_plain(&row.scope, LIST_SCOPE_WIDTH),
        pad_rendered(
            &row.kind,
            LIST_KIND_WIDTH,
            row.orchestration.then_some(ui_warn),
        ),
        pad_plain(&row.mode, LIST_MODE_WIDTH),
        pad_plain(&row.action, LIST_ACTION_WIDTH)
    );
    let continuation_prefix = " ".repeat(list_prefix_width());
    let goal_lines = wrap_list_goal(&row.goal, row.goal_width);
    for (index, line) in goal_lines.iter().enumerate() {
        if index == 0 {
            println!("{first_prefix}{line}");
        } else {
            println!("{continuation_prefix}{line}");
        }
    }
}

fn list_prefix_width() -> usize {
    LIST_ID_WIDTH
        + LIST_STATUS_WIDTH
        + LIST_AGE_WIDTH
        + LIST_SCOPE_WIDTH
        + LIST_KIND_WIDTH
        + LIST_MODE_WIDTH
        + LIST_ACTION_WIDTH
        + 14
}

fn list_goal_width() -> usize {
    if !io::stdout().is_terminal() {
        return 72;
    }
    let terminal_width = crossterm::terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(180);
    terminal_width.saturating_sub(list_prefix_width()).max(24)
}

fn pad_plain(value: &str, width: usize) -> String {
    let plain = truncate_text(value, width);
    let padding = width.saturating_sub(plain.chars().count());
    format!("{plain}{}", " ".repeat(padding))
}

fn pad_rendered(value: &str, width: usize, render: Option<fn(String) -> String>) -> String {
    let plain = truncate_text(value, width);
    let padding = width.saturating_sub(plain.chars().count());
    let rendered = render.map_or_else(|| plain.clone(), |render| render(plain.clone()));
    format!("{rendered}{}", " ".repeat(padding))
}

fn wrap_list_goal(value: &str, width: usize) -> Vec<String> {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return vec![String::new()];
    }
    let mut lines = wrap_words(&compact, width.max(8));
    let truncated = lines.len() > LIST_GOAL_MAX_LINES;
    if truncated {
        lines.truncate(LIST_GOAL_MAX_LINES);
        if let Some(last) = lines.last_mut() {
            *last = ellipsize_goal_line(last, width);
        }
    }
    lines
}

fn ellipsize_goal_line(value: &str, width: usize) -> String {
    if width <= 3 {
        return ".".repeat(width);
    }
    let prefix = width - 3;
    format!("{}...", value.chars().take(prefix).collect::<String>())
}

fn wrap_words(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split(' ') {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.extend(split_long_word(word, width));
            continue;
        }
        let next_len = if current.is_empty() {
            word_len
        } else {
            current.chars().count() + 1 + word_len
        };
        if next_len <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn split_long_word(word: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        if current.chars().count() >= width {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[derive(Debug)]
enum ListEntry {
    Run(RunListEntry),
    Plan(PlanListEntry),
}

impl ListEntry {
    fn updated_at(&self) -> DateTime<Utc> {
        match self {
            Self::Run(run) => run.updated_at,
            Self::Plan(plan) => plan.updated_at,
        }
    }
}

#[derive(Debug)]
struct PlanListEntry {
    plan_id: String,
    merged_run_id: Option<String>,
    scope: String,
    goal: String,
    status: PlanStatus,
    mode: PlanMode,
    updated_at: DateTime<Utc>,
    plan_path: PathBuf,
    completed_children: usize,
    total_children: usize,
}

fn list_plan_entries(
    paths: &DeadreckonPaths,
    scope_filter: Option<&str>,
) -> Result<Vec<PlanListEntry>> {
    let root = paths.plans_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut plans = Vec::new();
    for entry in fs::read_dir(&root).map_err(|source| DeadreckonError::Io {
        path: root.clone(),
        source,
    })? {
        let entry = entry?;
        let plan_path = entry.path().join("plan.json");
        if !plan_path.is_file() {
            continue;
        }
        let Some(plan_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let plan = load_plan(paths, &plan_id)?;
        if scope_filter.is_some_and(|scope| plan.parent_scope.as_deref() != Some(scope)) {
            continue;
        }
        let updated_at = fs::metadata(&plan_path)
            .and_then(|metadata| metadata.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| plan.merged_at.or(plan.forked_at).unwrap_or(plan.created_at));
        let completed_children = plan
            .tasks
            .iter()
            .filter(|task| task.status == PlanTaskStatus::Completed)
            .count();
        let total_children = plan.tasks.len();
        plans.push(PlanListEntry {
            plan_id: plan.plan_id,
            merged_run_id: plan.merged_run_id,
            scope: plan.parent_scope.unwrap_or_else(|| "global".to_string()),
            goal: plan.root_goal,
            status: plan.status,
            mode: plan.mode,
            updated_at,
            plan_path,
            completed_children,
            total_children,
        });
    }
    plans.sort_by_key(|plan| Reverse(plan.updated_at));
    Ok(plans)
}

fn plan_action_label(plan: &PlanListEntry) -> String {
    match plan.status {
        PlanStatus::Pending => "fork".to_string(),
        PlanStatus::Forked if plan.completed_children == plan.total_children => {
            format!("merge {}/{}", plan.completed_children, plan.total_children)
        }
        PlanStatus::Forked => format!("attach {}/{}", plan.completed_children, plan.total_children),
        PlanStatus::Merged if plan.merged_run_id.is_some() => "finish".to_string(),
        PlanStatus::Merged => "show".to_string(),
        PlanStatus::Failed => "show failure".to_string(),
    }
}

enum HistoryMatcher {
    Substring(String),
    Regex(Regex),
}

impl HistoryMatcher {
    fn new(pattern: String, regex: bool) -> Result<Self> {
        if regex {
            let compiled = Regex::new(&pattern).map_err(|err| {
                CliError::Core(deadreckon_core::user_error(
                    &format!("invalid regex: {err}"),
                    "re-quote or escape the pattern",
                ))
            })?;
            Ok(Self::Regex(compiled))
        } else {
            Ok(Self::Substring(pattern))
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Substring(pattern) => line.contains(pattern),
            Self::Regex(pattern) => pattern.is_match(line),
        }
    }
}

fn history_command(command: HistoryCommand) -> Result<()> {
    match command {
        HistoryCommand::Grep {
            pattern,
            plan,
            scope,
            all,
            since,
            kind,
            limit,
            regex,
        } => history_grep_command(HistoryGrepRequest {
            pattern,
            plan,
            scope,
            all,
            since,
            kind,
            limit,
            regex,
        }),
    }
}

struct HistoryGrepRequest {
    pattern: String,
    plan: Option<String>,
    scope: Option<String>,
    all: bool,
    since: Option<String>,
    kind: HistoryKind,
    limit: usize,
    regex: bool,
}

fn history_grep_command(args: HistoryGrepRequest) -> Result<()> {
    let HistoryGrepRequest {
        pattern,
        plan,
        scope,
        all,
        since,
        kind,
        limit,
        regex,
    } = args;
    if limit == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--limit must be at least 1",
            "deadreckon history grep \"pattern\" --limit 20",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let matcher = HistoryMatcher::new(pattern.clone(), regex)?;
    let cutoff = parse_history_since(since)?;
    let plan_children = plan
        .as_deref()
        .map(|plan_id| history_plan_children(&paths, plan_id))
        .transpose()?;
    let effective_scope = if plan_children.is_some() || all {
        scope
    } else {
        Some(scope.unwrap_or(current_scope()?))
    };
    let runs = list_runs(&paths, effective_scope.as_deref())?;
    let mut printed = 0usize;
    let mut total_matches = 0usize;
    println!(
        "{} {} {}",
        ui_heading("history grep"),
        ui_muted(history_kind_label(kind)),
        ui_muted(if regex { "regex" } else { "substring" })
    );
    if let Some(plan_id) = plan.as_deref() {
        let plan_id = resolve_plan_id(&paths, plan_id)?;
        for event in read_plan_events_lossy(&paths, &plan_id) {
            if let Some(cutoff) = cutoff
                && event.timestamp < cutoff
            {
                continue;
            }
            let line = plan_event_line(&event);
            if !matcher.is_match(&line) {
                continue;
            }
            total_matches += 1;
            if printed >= limit {
                continue;
            }
            println!(
                "{} {} plan-events | {}",
                ui_id(run_prefix(&plan_id)),
                event.timestamp.to_rfc3339(),
                one_line(&line, 220)
            );
            printed += 1;
        }
    }
    for run in runs {
        if let Some(children) = plan_children.as_ref()
            && !children.contains(&run.run_id)
        {
            continue;
        }
        let state = load_run(&paths, &run.run_id)?;
        let path = state.run_root.join(history_kind_file(kind));
        if !path.exists() || !history_file_within_cutoff(&path, cutoff)? {
            continue;
        }
        let fallback_timestamp = history_file_timestamp(&path)?;
        for line in fs::read_to_string(&path)?.lines() {
            if !matcher.is_match(line) {
                continue;
            }
            total_matches += 1;
            if printed >= limit {
                continue;
            }
            let timestamp = history_line_timestamp(kind, line).unwrap_or(fallback_timestamp);
            println!(
                "{} {} {} | {}",
                ui_id(run_prefix(&run.run_id)),
                timestamp.to_rfc3339(),
                run.scope,
                one_line(line.trim(), 220)
            );
            printed += 1;
        }
    }
    if total_matches == 0 {
        println!("no matches for {pattern:?}");
        println!(
            "{} try `{}` or `{}`",
            ui_muted("hint:"),
            ui_command("deadreckon history grep <pattern> --all"),
            ui_command("deadreckon show <run-id>")
        );
    } else if total_matches > printed {
        println!("... ({} more)", total_matches - printed);
    }
    Ok(())
}

fn history_plan_children(paths: &DeadreckonPaths, plan_id: &str) -> Result<BTreeSet<String>> {
    let plan_id = resolve_plan_id(paths, plan_id)?;
    let plan = load_plan(paths, &plan_id)?;
    Ok(plan
        .tasks
        .iter()
        .filter_map(|task| task.child_run_id.clone())
        .collect())
}

fn history_kind_file(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Trace => "traces.jsonl",
        HistoryKind::Provenance => "provenance.jsonl",
    }
}

fn history_kind_label(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Trace => "trace",
        HistoryKind::Provenance => "provenance",
    }
}

fn parse_history_since(value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.len() < 2 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "invalid --since duration",
            "use a duration like 7d, 24h, or 30m",
        )));
    }
    let (amount, unit) = value.split_at(value.len() - 1);
    let amount = amount.parse::<i64>().map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            "invalid --since duration",
            "use a duration like 7d, 24h, or 30m",
        ))
    })?;
    if amount < 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "invalid --since duration",
            "use a positive duration like 7d, 24h, or 30m",
        )));
    }
    let duration = match unit {
        "d" => ChronoDuration::days(amount),
        "h" => ChronoDuration::hours(amount),
        "m" => ChronoDuration::minutes(amount),
        _ => {
            return Err(CliError::Core(deadreckon_core::user_error(
                "invalid --since duration unit",
                "use d, h, or m, for example 7d",
            )));
        }
    };
    Ok(Some(Utc::now() - duration))
}

fn history_file_within_cutoff(path: &Path, cutoff: Option<DateTime<Utc>>) -> Result<bool> {
    let Some(cutoff) = cutoff else {
        return Ok(true);
    };
    Ok(history_file_timestamp(path)? >= cutoff)
}

fn history_file_timestamp(path: &Path) -> Result<DateTime<Utc>> {
    let modified = fs::metadata(path)?.modified()?;
    Ok(DateTime::<Utc>::from(modified))
}

fn history_line_timestamp(kind: HistoryKind, line: &str) -> Option<DateTime<Utc>> {
    match kind {
        HistoryKind::Trace => serde_json::from_str::<TraceRecord>(line)
            .ok()
            .map(|record| record.timestamp),
        HistoryKind::Provenance => serde_json::from_str::<ProvenanceRecord>(line)
            .ok()
            .map(|record| record.timestamp),
    }
}

#[derive(Debug, Clone)]
struct LibraryEntry {
    manifest: PromotionManifest,
    path: PathBuf,
    materialized_count: usize,
}

fn library_command(command: LibraryCommand) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    match command {
        LibraryCommand::List {
            scope,
            all,
            goal,
            since,
            until,
            full,
            json,
        } => {
            let filter = LibraryFilter::new(goal, since, until)?;
            let entries =
                filter_library_entries(library_entries(&paths, scope.clone(), all)?, &filter);
            if json {
                let artifacts = entries
                    .iter()
                    .map(|entry| {
                        json!({
                            "manifest": &entry.manifest,
                            "path": &entry.path,
                            "materialized_count": entry.materialized_count,
                        })
                    })
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "kind": "library_list",
                        "id": scope.as_deref().unwrap_or(if all { "all-scopes" } else { "current-scope" }),
                        "status": "ok",
                        "next_actions": ["deadreckon finish <id>", "deadreckon export <id>"],
                        "try_lines": Vec::<String>::new(),
                        "paths": {
                            "home": paths.home(),
                        },
                        "artifacts": artifacts,
                    }))?
                );
                return Ok(());
            }
            if entries.is_empty() {
                print_empty_library_hint(scope.as_deref(), all);
                return Ok(());
            }
            print_library_table(&entries, full);
        }
        LibraryCommand::Search {
            query,
            scope,
            all,
            since,
            until,
        } => {
            let filter = LibraryFilter::new(None, since, until)?;
            let needle = query.to_lowercase();
            let entries = filter_library_entries(library_entries(&paths, scope, all)?, &filter)
                .into_iter()
                .filter(|entry| library_entry_matches_query(entry, &needle))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                println!("no library artifacts matched {query:?}");
                println!(
                    "{} try `{}`",
                    ui_muted("hint:"),
                    ui_command("deadreckon library list --all")
                );
                return Ok(());
            }
            print_library_table(&entries, false);
        }
        LibraryCommand::Show { run_id, scope } => {
            let entry = resolve_library_entry(&paths, &run_id, scope)?;
            print_library_entry(&entry);
        }
    }
    Ok(())
}

struct LibraryFilter {
    goal: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

impl LibraryFilter {
    fn new(goal: Option<String>, since: Option<String>, until: Option<String>) -> Result<Self> {
        Ok(Self {
            goal: goal.map(|goal| goal.to_lowercase()),
            since: parse_library_date_filter("--since", since)?,
            until: parse_library_date_filter("--until", until)?,
        })
    }
}

fn filter_library_entries(entries: Vec<LibraryEntry>, filter: &LibraryFilter) -> Vec<LibraryEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            let manifest = &entry.manifest;
            filter
                .goal
                .as_ref()
                .is_none_or(|goal| manifest.goal.to_lowercase().contains(goal))
                && filter
                    .since
                    .is_none_or(|since| manifest.promoted_at >= since)
                && filter
                    .until
                    .is_none_or(|until| manifest.promoted_at <= until)
        })
        .collect()
}

fn library_entry_matches_query(entry: &LibraryEntry, needle: &str) -> bool {
    let manifest = &entry.manifest;
    manifest.run_id.to_lowercase().contains(needle)
        || manifest.scope.to_lowercase().contains(needle)
        || manifest.goal.to_lowercase().contains(needle)
        || library_docs_contain(&entry.path, needle)
}

fn library_docs_contain(path: &Path, needle: &str) -> bool {
    inventory_files(path)
        .unwrap_or_default()
        .into_iter()
        .filter(|file| {
            matches!(
                file.extension().and_then(|ext| ext.to_str()),
                Some("md" | "txt" | "json" | "jsonl" | "toml")
            )
        })
        .any(|file| {
            fs::read_to_string(file)
                .ok()
                .is_some_and(|raw| raw.to_lowercase().contains(needle))
        })
}

fn parse_library_date_filter(label: &str, value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Ok(date_time) = DateTime::parse_from_rfc3339(&value) {
        return Ok(Some(date_time.with_timezone(&Utc)));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
        let date_time = if label == "--until" {
            date.and_hms_nano_opt(23, 59, 59, 999_999_999)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        let Some(date_time) = date_time else {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "invalid {label} date {value:?}\ntry: deadreckon library list {label} 2026-05-11"
            ))));
        };
        return Ok(Some(date_time.and_utc()));
    }
    Err(CliError::Core(DeadreckonError::InvalidInput(format!(
        "invalid {label} date {value:?}\ntry: deadreckon library list {label} 2026-05-11"
    ))))
}

fn library_entries(
    paths: &DeadreckonPaths,
    scope: Option<String>,
    all: bool,
) -> Result<Vec<LibraryEntry>> {
    let mut entries = Vec::new();
    let library_root = paths.home().join("library");
    if let Some(scope) = scope {
        scan_library_scope(paths, &scope, &mut entries)?;
    } else if all {
        if library_root.exists() {
            for scope_entry in fs::read_dir(&library_root)? {
                let scope_entry = scope_entry?;
                if !scope_entry.file_type()?.is_dir() {
                    continue;
                }
                let scope_name = scope_entry.file_name().to_string_lossy().to_string();
                if scope_name.starts_with('.') {
                    continue;
                }
                scan_library_scope(paths, &scope_name, &mut entries)?;
            }
        }
    } else {
        scan_library_scope(paths, &current_scope()?, &mut entries)?;
    }
    entries.sort_by(|left, right| {
        right
            .manifest
            .promoted_at
            .cmp(&left.manifest.promoted_at)
            .then_with(|| left.manifest.run_id.cmp(&right.manifest.run_id))
    });
    Ok(entries)
}

fn scan_library_scope(
    paths: &DeadreckonPaths,
    scope: &str,
    entries: &mut Vec<LibraryEntry>,
) -> Result<()> {
    let scope_dir = paths.home().join("library").join(scope);
    if !scope_dir.exists() {
        return Ok(());
    }
    for run_entry in fs::read_dir(scope_dir)? {
        let run_entry = run_entry?;
        if !run_entry.file_type()?.is_dir() {
            continue;
        }
        let path = run_entry.path();
        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest: PromotionManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        entries.push(LibraryEntry {
            materialized_count: materialized_marker_count(&path),
            manifest,
            path,
        });
    }
    Ok(())
}

fn materialized_marker_count(library_dir: &Path) -> usize {
    fs::read_to_string(library_dir.join(".materialized-to"))
        .ok()
        .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0)
}

fn resolve_library_entry(
    paths: &DeadreckonPaths,
    run_id: &str,
    scope: Option<String>,
) -> Result<LibraryEntry> {
    let entries = library_entries(paths, scope, false)?;
    if matches!(run_id, "latest" | "last") {
        return entries.into_iter().next().ok_or_else(|| {
            CliError::Core(DeadreckonError::NotFound(
                "no library artifacts for this project".to_string(),
            ))
        });
    }
    let matches = entries
        .into_iter()
        .filter(|entry| entry.manifest.run_id.starts_with(run_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library artifact {run_id}"
        )))),
        [entry] => Ok(entry.clone()),
        many => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "run id prefix {run_id:?} matched {} library artifacts; use more characters",
            many.len()
        )))),
    }
}

fn print_empty_library_hint(scope: Option<&str>, all: bool) {
    match scope {
        Some(scope) => println!("no library artifacts for scope {scope}"),
        None if all => println!("no library artifacts"),
        None => println!("no library artifacts for current project"),
    }
    println!(
        "{} completed fresh/copy runs are promoted automatically; use `{}` to inspect all scopes",
        ui_muted("hint:"),
        ui_command("deadreckon library list --all")
    );
}

fn print_library_table(entries: &[LibraryEntry], full: bool) {
    if full {
        println!(
            "{}",
            ui_heading("RUN\tSCOPE\tPROMOTED\tMATERIALIZED\tPATH\tGOAL")
        );
        for entry in entries {
            let manifest = &entry.manifest;
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                manifest.run_id,
                manifest.scope,
                manifest.promoted_at,
                entry.materialized_count,
                entry.path.display(),
                manifest.goal
            );
        }
        return;
    }

    println!(
        "{}",
        ui_heading(format!(
            "{:<8}  {:<7}  {:<26}  {:<12}  GOAL",
            "RUN", "AGE", "SCOPE", "EXPORTED"
        ))
    );
    for entry in entries {
        let manifest = &entry.manifest;
        println!(
            "{:<8}  {:<7}  {:<26}  {:<12}  {}",
            ui_id(run_prefix(&manifest.run_id)),
            relative_age(manifest.promoted_at),
            truncate_text(&manifest.scope, 26),
            materialized_count_label(entry.materialized_count),
            truncate_text(&one_line(&manifest.goal, 88), 88)
        );
    }
    println!(
        "{} use `{}` or `{}`",
        ui_muted("hint:"),
        ui_command("deadreckon library show <run-id>"),
        ui_command("deadreckon export <run-id> --dest <path>")
    );
}

fn materialized_count_label(count: usize) -> String {
    match count {
        0 => "no".to_string(),
        1 => "yes (1)".to_string(),
        count => format!("yes ({count})"),
    }
}

fn print_library_entry(entry: &LibraryEntry) {
    let manifest = &entry.manifest;
    println!("{}", ui_heading("library artifact"));
    println!("run:        {}", ui_id(&manifest.run_id));
    println!("scope:      {}", manifest.scope);
    println!("promoted:   {}", manifest.promoted_at);
    println!(
        "exported:   {}",
        materialized_count_label(entry.materialized_count)
    );
    println!("path:       {}", entry.path.display());
    println!("source:     {}", manifest.source_working_dir.display());
    println!("provenance: {}", manifest.provenance_hash);
    println!("goal:       {}", manifest.goal);
    println!();
    println!(
        "next:       {}",
        ui_command(format!(
            "deadreckon export {}",
            run_prefix(&manifest.run_id)
        ))
    );
}

fn current_scope() -> Result<String> {
    let cwd = std::env::current_dir()?;
    workspace_scope(&cwd).map_err(CliError::from)
}

fn load_cli_run(paths: &DeadreckonPaths, run_id: &str) -> Result<deadreckon_core::PipelineState> {
    load_cli_run_with_scope(paths, run_id, false)
}

fn load_cli_run_with_scope(
    paths: &DeadreckonPaths,
    run_id: &str,
    all: bool,
) -> Result<deadreckon_core::PipelineState> {
    if matches!(run_id, "latest" | "last") {
        latest_run(paths, all)
    } else {
        load_run(paths, run_id).map_err(CliError::from)
    }
}

fn next_action_label_for_entry(
    paths: &DeadreckonPaths,
    run: &deadreckon_core::RunListEntry,
) -> String {
    load_run(paths, &run.run_id)
        .map(|state| next_action_label(paths, &state))
        .unwrap_or_else(|_| "-".to_string())
}

fn next_action_label(paths: &DeadreckonPaths, state: &deadreckon_core::PipelineState) -> String {
    if state.run_root.join("abandoned.json").exists() {
        return cleanup_action_label(state);
    }
    if is_stale_executing(state) {
        return "cleanup --stale".to_string();
    }
    match state.status {
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => "attach".to_string(),
        RunStatus::Failed | RunStatus::Killed => "resume".to_string(),
        RunStatus::Completed => match read_run_codebase_record(paths, state)
            .map(|record| record.mode)
            .unwrap_or(CodebaseMode::Fresh)
        {
            CodebaseMode::Worktree => "finish (apply)".to_string(),
            CodebaseMode::Copy | CodebaseMode::Fresh => "finish (export)".to_string(),
            CodebaseMode::InPlace => "finish (review)".to_string(),
        },
    }
}

fn cleanup_action_label(state: &deadreckon_core::PipelineState) -> String {
    match cleanup_marker_reason(state).as_deref() {
        Some("abandoned") => "abandoned".to_string(),
        Some("applied" | "cleaned" | "cleanup") => "done".to_string(),
        _ if state.status == RunStatus::Completed => "done".to_string(),
        _ => "cleaned".to_string(),
    }
}

fn cleanup_marker_reason(state: &deadreckon_core::PipelineState) -> Option<String> {
    let raw = fs::read_to_string(state.run_root.join("abandoned.json")).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value
        .get("reason")
        .or_else(|| value.get("action"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn relative_age(updated_at: DateTime<Utc>) -> String {
    let seconds = (Utc::now() - updated_at).num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 60 * 60 * 24 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let prefix = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(prefix).collect::<String>())
}

fn codebase_mode_status(paths: &DeadreckonPaths, run: &deadreckon_core::RunListEntry) -> String {
    let Ok(state) = load_run(paths, &run.run_id) else {
        return "-".to_string();
    };
    read_run_codebase_record(paths, &state)
        .map(|record| record.mode.to_string())
        .unwrap_or_else(|_| "-".to_string())
}

fn read_run_codebase_record(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<CodebaseRecord> {
    let mut bases = vec![state.working_dir.clone()];
    if let Some(library_dir) = state.promoted_library_dir.as_ref() {
        bases.push(library_dir.clone());
    }
    bases.push(paths.library_dir(&state.scope, &state.run_id));
    for turn in (0..=state.turn).rev() {
        bases.push(
            state
                .run_root
                .join("snapshots")
                .join(format!("turn-{turn}")),
        );
    }
    for base in bases {
        if let Ok(record) = read_codebase_record(&base) {
            return Ok(record);
        }
    }
    read_codebase_record(&state.working_dir).map_err(CliError::from)
}

struct DocCommandArgs {
    run_id: String,
    kind: DocKind,
    export: Option<PathBuf>,
    polish: bool,
    no_confirm: bool,
    force: bool,
    doc_skill: Option<String>,
    doc_provider: Option<String>,
    budget_cap: Option<f64>,
}

async fn doc_command(args: DocCommandArgs) -> Result<()> {
    let DocCommandArgs {
        run_id,
        kind,
        export,
        polish,
        no_confirm,
        force,
        doc_skill,
        doc_provider,
        budget_cap,
    } = args;
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    if polish {
        if state.status != RunStatus::Completed {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "run {} is {}; docs are not yet polished",
                    state.run_id, state.status
                ),
                &format!("deadreckon resume {} or omit --polish", state.run_id),
            )));
        }
        let defaults = config_defaults(&paths)?;
        let setup_selection = doc_provider_setup_selection(
            &paths,
            &defaults,
            doc_provider.as_deref(),
            state.provider.as_deref(),
            false,
        )?;
        let selection = doc_provider_selection_from_setup(&setup_selection);
        let Some(provider) = selection.provider.clone() else {
            return Err(CliError::Core(deadreckon_core::user_error(
                "deadreckon: no doc provider available",
                "deadreckon config set defaults.doc_provider cli:codex\ntry: install codex or claude; deadreckon auto-detects subscription CLIs on the next run",
            )));
        };
        let subskills = effective_doc_subskills(&defaults);
        let token_budget = defaults
            .doc_polish_token_budget
            .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
        let budget_cap = budget_cap.or(defaults.doc_polish_budget_cap_usd);
        let router = ProviderRouter::from_config_path(&paths.config_path(), Some(&provider))?;
        let estimated_spend =
            estimate_doc_polish_spend(&router, &provider, token_budget, subskills.len())?;
        if let Some(cap) = budget_cap
            && estimated_spend.cost_usd > cap
        {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "doc polish would cost about ${:.6}, above cap ${cap:.6}",
                    estimated_spend.cost_usd
                ),
                &format!(
                    "deadreckon doc {} --polish --max-spend {:.2} --no-confirm",
                    state.run_id, estimated_spend.cost_usd
                ),
            )));
        }
        if !no_confirm && completion_hints_enabled(false) && io::stdin().is_terminal() {
            print_doc_polish_preview(
                &state,
                &provider,
                selection.source.as_str(),
                &subskills,
                token_budget,
                budget_cap,
                &estimated_spend,
            )?;
            if !prompt::confirm("polish docs now?", true)? {
                println!("cancelled");
                return Ok(());
            }
        } else if !no_confirm && !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive doc polish requires --no-confirm",
                &format!("deadreckon doc {} --polish --no-confirm", state.run_id),
            )));
        }
        with_cli_wait_status(
            "polishing run docs",
            polish_run_docs(
                &mut state,
                &router,
                &PolishConfig {
                    home: paths.home().to_path_buf(),
                    doc_skill: doc_skill
                        .or(defaults.doc_skill)
                        .unwrap_or_else(|| "run-narrator".to_string()),
                    doc_provider: Some(provider),
                    doc_provider_source: Some(selection.source.as_str().to_string()),
                    doc_subskills: subskills,
                    token_budget,
                    budget_cap_usd: budget_cap,
                    no_llm: false,
                    force,
                },
            ),
        )
        .await?;
        if completion_hints_enabled(false)
            && let Some(record) = deadreckon_runtime::read_polish_record(&state)?
        {
            print_doc_polish_summary(&record);
        }
        save_state(&state)?;
    }
    let Some(path) = doc_path_for_kind(&state.working_dir, kind) else {
        if kind == DocKind::Delta {
            return Err(CliError::Core(deadreckon_core::user_error(
                "no delta produced; this run did not affect a project AS-BUILT",
                "deadreckon doc <run-id> --kind narrative",
            )));
        }
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "{} for run {}",
            kind.file_name(),
            state.run_id
        ))));
    };
    if let Some(dest) = export {
        if dest.exists() && !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("dest {} exists", dest.display()),
                "--overwrite or pick a fresh path",
            )));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &dest)?;
        println!("exported {} to {}", kind.file_name(), dest.display());
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
}

fn print_doc_polish_preview(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<()> {
    print!(
        "{}",
        doc_polish_preview_text(
            state,
            provider,
            provider_source,
            subskills,
            token_budget,
            budget_cap,
            estimated_spend,
        )?
    );
    Ok(())
}

fn doc_polish_preview_text(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<String> {
    let hash = deadreckon_runtime::inputs_hash(state)?;
    let mut out = String::new();
    out.push_str(&format!("{}\n", ui_heading("polish preview:")));
    out.push_str(&format!(
        "  provider:  {} ({provider_source})\n",
        ui_id(provider)
    ));
    out.push_str(&format!("  subskills: {}\n", subskills.join(", ")));
    out.push_str(&format!(
        "  budget:    {} tokens per subcall\n",
        token_budget
    ));
    out.push_str(&format!(
        "  estimate:  {}\n",
        doc_polish_cost_label(estimated_spend)
    ));
    out.push_str(&format!(
        "  cost cap:  {}\n",
        budget_cap
            .map(|cap| format!("${cap:.2}"))
            .unwrap_or_else(|| "provider/account default".to_string())
    ));
    out.push_str(&format!("  inputs:    {}\n", &hash[..hash.len().min(12)]));
    Ok(out)
}

fn estimate_doc_polish_spend(
    router: &ProviderRouter,
    provider: &str,
    token_budget: u32,
    subskill_count: usize,
) -> Result<SpendEstimate> {
    let calls = subskill_count.max(1) as u64;
    router
        .estimate_for_route(
            Some(provider),
            ProviderUsage {
                input_tokens: 0,
                output_tokens: u64::from(token_budget) * calls,
            },
        )
        .map_err(CliError::from)
}

fn doc_polish_cost_label(estimate: &SpendEstimate) -> String {
    if estimate.subscription {
        "$0.00 (subscription)".to_string()
    } else {
        format!(
            "${:.6} for up to {} output tokens",
            estimate.cost_usd, estimate.output_tokens
        )
    }
}

fn print_doc_polish_summary(record: &deadreckon_runtime::PolishRecord) {
    println!("{}", ui_heading("doc polish:"));
    println!("  status:   {}", record.status);
    if let Some(provider) = record.provider.as_deref() {
        println!("  provider: {provider}");
    }
    println!("  cost:     ${:.6}", record.cost_usd);
    if !record.subcalls.is_empty() {
        println!("  subcalls:");
        for subcall in &record.subcalls {
            println!(
                "    {} {:<18} {} in / {} out ${:.6}",
                ui_status(&subcall.status),
                subcall.skill,
                subcall.tokens_in,
                subcall.tokens_out,
                subcall.cost_usd
            );
        }
    }
}

async fn attach_command(run_id: String, no_hints: bool, plain: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut parent_plan = None;
    let state = if let Some(selection) = resolve_plan_child_ref(&paths, &run_id)? {
        parent_plan = Some(AttachParentPlan {
            plan_id: selection.plan_id,
            task_id: selection.task_id,
        });
        load_run(&paths, &selection.run_id)?
    } else {
        match load_cli_run(&paths, &run_id) {
            Ok(state) => state,
            Err(run_error) => {
                if let Ok(plan_id) = resolve_plan_id(&paths, &run_id) {
                    let plan = load_plan(&paths, &plan_id)?;
                    let show_hints = completion_hints_enabled(no_hints);
                    if io::stdout().is_terminal() && !plain {
                        print_attach_banner("plan", &plan.plan_id);
                        attach_plan_tui(&paths, &plan.plan_id, show_hints).await?;
                    } else {
                        print_plan_summary(&paths, &plan, show_hints);
                    }
                    return Ok(());
                }
                if resolve_chain_id(&paths, &run_id, false).is_ok() {
                    return chain_attach_command(&paths, &run_id, plain);
                }
                return Err(run_error);
            }
        }
    };
    let run_id = state.run_id.clone();
    let show_hints = completion_hints_enabled(no_hints);
    if io::stdout().is_terminal() && !plain {
        print_attach_banner("run", &run_id);
        if parent_plan.is_some() {
            attach_tui_with_parent(&paths, &run_id, show_hints, parent_plan).await?;
        } else {
            attach_tui(&paths, &run_id, show_hints).await?;
        }
        let state = load_run(&paths, &run_id)?;
        if state.status == RunStatus::Completed && show_hints {
            print_exit_summary_card(&state, &RunLoopOutcome::Done, plain);
            print_chain_context_for_working(&state.working_dir);
            print_lifecycle_hints(&state);
        }
        return Ok(());
    }
    if let Some(parent_plan) = parent_plan.as_ref() {
        println!(
            "plan {} / {} -> run {}",
            run_prefix(&parent_plan.plan_id),
            parent_plan.task_id,
            run_prefix(&state.run_id)
        );
    }
    if state.status == RunStatus::Completed && show_hints {
        print_exit_summary_card(&state, &RunLoopOutcome::Done, plain);
        print_chain_context_for_working(&state.working_dir);
        print_lifecycle_hints(&state);
    } else {
        print_run_summary(&state);
    }
    Ok(())
}

// SAFETY: Kill arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn kill_command(run_id: String, force: bool, plain: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = match load_cli_run(&paths, &run_id) {
        Ok(state) => state,
        Err(run_error) => {
            if let Ok(plan_id) = resolve_plan_id(&paths, &run_id) {
                return kill_plan_command(&paths, &plan_id, force);
            }
            if let Ok(chain_id) = resolve_chain_id(&paths, &run_id, false) {
                return chain_kill_command(&paths, &chain_id, force);
            }
            return Err(run_error);
        }
    };
    kill_loaded_run(&paths, &mut state, force)?;
    print_kill_banner("run", &run_prefix(&state.run_id), force, None);
    print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain);
    Ok(())
}

fn kill_plan_command(paths: &DeadreckonPaths, plan_id: &str, force: bool) -> Result<()> {
    let mut plan = load_plan(paths, plan_id)?;
    let mut killed = 0_u32;
    if let Ok(raw) = fs::read_to_string(paths.coordinator_json(plan_id))
        && let Ok(coordinator) = serde_json::from_str::<CoordinatorState>(&raw)
    {
        for child in coordinator.children {
            if let Some(task) = plan.tasks.get_mut(child.child_index as usize) {
                if task.child_run_id.is_none()
                    && let Some(run_id) = child.run_id.as_ref()
                {
                    task.child_run_id = Some(run_id.clone());
                }
                if task.child_scope.is_none()
                    && let Some(scope) = child.scope.as_ref()
                {
                    task.child_scope = Some(scope.clone());
                }
            }
            if let Some(pid) = child.pid
                && pid_is_alive(pid)
            {
                terminate_pid(pid, force)?;
                killed += 1;
            }
        }
        if pid_is_alive(coordinator.coordinator_pid) {
            terminate_pid(coordinator.coordinator_pid, force)?;
            killed += 1;
        }
    }
    for task_index in 0..plan.tasks.len() {
        let mut task_run_ids = Vec::new();
        if let Some(run_id) = plan.tasks[task_index].child_run_id.as_ref() {
            task_run_ids.push(run_id.clone());
        }
        let launch_dir = paths
            .plan_dir(&plan.plan_id)
            .join("launch")
            .join(&plan.tasks[task_index].task_id);
        if let Ok(run_id) = fs::read_to_string(launch_dir.join("run-id")) {
            task_run_ids.push(run_id.trim().to_string());
        }
        if launch_dir.is_dir()
            && let Ok(scope) = workspace_scope(&launch_dir)
            && let Ok(runs) = list_runs(paths, Some(scope.as_str()))
        {
            for run in runs {
                task_run_ids.push(run.run_id);
            }
        }
        task_run_ids.sort();
        task_run_ids.dedup();
        for run_id in task_run_ids {
            if let Ok(mut state) = load_run(paths, &run_id) {
                if plan.tasks[task_index].child_run_id.is_none() {
                    plan.tasks[task_index].child_run_id = Some(state.run_id.clone());
                    append_plan_event(
                        paths,
                        &plan.plan_id,
                        PlanEventKind::TaskRunDiscovered {
                            task_id: plan.tasks[task_index].task_id.clone(),
                            task_index,
                            run_id: Some(state.run_id.clone()),
                            pid: None,
                        },
                    )?;
                }
                if plan.tasks[task_index].child_scope.is_none() {
                    plan.tasks[task_index].child_scope = Some(state.scope.clone());
                }
                if !matches!(
                    state.status,
                    RunStatus::Pending | RunStatus::Planned | RunStatus::Executing
                ) {
                    continue;
                }
                kill_loaded_run(paths, &mut state, force)?;
                append_plan_event(
                    paths,
                    &plan.plan_id,
                    PlanEventKind::TaskKilled {
                        task_id: plan.tasks[task_index].task_id.clone(),
                        task_index,
                        run_id: Some(state.run_id.clone()),
                    },
                )?;
                killed += 1;
            }
        }
        if matches!(
            plan.tasks[task_index].status,
            PlanTaskStatus::Pending | PlanTaskStatus::Running
        ) {
            plan.tasks[task_index].status = PlanTaskStatus::Killed;
            append_plan_event(
                paths,
                &plan.plan_id,
                PlanEventKind::TaskKilled {
                    task_id: plan.tasks[task_index].task_id.clone(),
                    task_index,
                    run_id: plan.tasks[task_index].child_run_id.clone(),
                },
            )?;
        }
    }
    plan.status = PlanStatus::Failed;
    save_plan(paths, &plan)?;
    append_plan_event(paths, &plan.plan_id, PlanEventKind::PlanKilled)?;
    append_plan_event(
        paths,
        &plan.plan_id,
        PlanEventKind::PlanFailed {
            reason: "killed by user".to_string(),
        },
    )?;
    print_kill_banner("plan", &run_prefix(plan_id), force, Some(killed));
    Ok(())
}

fn print_kill_banner(kind: &str, id: &str, force: bool, processes: Option<u32>) {
    println!("{}", kill_banner(kind, id, force, processes));
}

fn kill_banner(kind: &str, id: &str, force: bool, processes: Option<u32>) -> String {
    let forcefully = if force { " forcefully" } else { "" };
    match processes {
        Some(count) => format!("killed {kind} {id}{forcefully} ({count} processes signalled)"),
        None => format!("killed {kind} {id}{forcefully}"),
    }
}

fn attach_banner(kind: &str, id: &str) -> String {
    format!("attaching to {kind} {}", id_prefix(id))
}

fn print_attach_banner(kind: &str, id: &str) {
    eprintln!("{}", attach_banner(kind, id));
}

fn kill_loaded_run(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
    force: bool,
) -> Result<()> {
    let pids = supervised_pids(state);
    write_cancel_marker(state, "killed by user")?;
    release_lock_file(paths, &state.scope, &state.task_key)?;
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(state)?;
    emit_event(
        state,
        None,
        deadreckon_core::RunEventKind::RunCompleted {
            status: "killed".to_string(),
        },
    )?;
    for pid in &pids {
        if *pid != std::process::id() {
            let _ = terminate_pid(*pid, force);
        }
    }
    if force {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        let any_alive = pids
            .iter()
            .any(|pid| *pid != std::process::id() && deadreckon_core::pid_is_alive(*pid));
        if !any_alive || std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for pid in &pids {
        if *pid != std::process::id() && deadreckon_core::pid_is_alive(*pid) {
            let _ = terminate_pid(*pid, true);
        }
    }
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = state.killed_at.or_else(|| Some(Utc::now()));
    state.updated_at = Utc::now();
    save_state(state)?;
    Ok(())
}

fn supervised_pids(state: &deadreckon_core::PipelineState) -> Vec<u32> {
    let mut pids = state.child_pids.iter().copied().collect::<BTreeSet<_>>();
    let pid_dir = state.run_root.join("child-pids");
    if let Ok(entries) = std::fs::read_dir(pid_dir) {
        for entry in entries.flatten() {
            if let Ok(raw) = std::fs::read_to_string(entry.path()) {
                for line in raw.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        pids.insert(pid);
                    }
                }
            }
        }
    }
    pids.into_iter().collect()
}

fn is_stale_executing(state: &deadreckon_core::PipelineState) -> bool {
    if state.status != RunStatus::Executing {
        return false;
    }
    let pids = supervised_pids(state);
    if pids.is_empty() {
        return Utc::now()
            .signed_duration_since(state.updated_at)
            .num_minutes()
            >= 5;
    }
    !pids
        .into_iter()
        .any(|pid| pid != std::process::id() && deadreckon_core::pid_is_alive(pid))
}

async fn resume_command(
    run_id: String,
    from_turn: Option<u32>,
    max_wall_seconds: Option<f64>,
    no_docs: bool,
    plain: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    if state.status == RunStatus::Completed {
        println!("run {} is already completed", state.run_id);
        return Ok(());
    }
    let mut lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "resume",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    state.failure_reason = None;
    state.pause_reason = None;
    state.killed_at = None;
    state.status = RunStatus::Planned;
    clear_cancel_marker(&state)?;
    if let Some(max_wall_seconds) = max_wall_seconds {
        state.max_wall_seconds = Some(max_wall_seconds);
    }
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;
    lock.heartbeat("resume-turn-loop")?;
    let provider = state.provider.clone();
    let backend: SandboxBackend = state.sandbox.parse()?;
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider.as_deref())?;
    let selected_route = router.selected_route_info();
    let defaults = config_defaults(&paths)?;
    let doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup_selection(
        &paths,
        &defaults,
        None,
        provider.as_deref(),
        false,
    )?);
    let max_spend_usd = state.max_spend_usd;
    let max_wall_seconds = state.max_wall_seconds;
    let wait_label = format!(
        "resuming run {} from durable state",
        run_prefix(&state.run_id)
    );
    print_run_started_with_label(
        "resumed run",
        &state,
        selected_route.as_ref(),
        "run_provider",
        doc_provider_selection.provider.as_deref(),
        doc_provider_selection.source.as_str(),
    );
    let outcome = with_cli_wait_status(
        &wait_label,
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider,
                max_spend_usd,
                max_wall_seconds,
                sandbox_backend: backend,
                max_turns: 12,
                from_turn,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(paths.config_path()),
                    doc_provider: doc_provider_selection.provider,
                    doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                    doc_subskills: effective_doc_subskills(&defaults),
                    token_budget: defaults
                        .doc_polish_token_budget
                        .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET),
                    budget_cap_usd: defaults.doc_polish_budget_cap_usd,
                    doc_skill: defaults
                        .doc_skill
                        .unwrap_or_else(|| "run-narrator".to_string()),
                    no_docs,
                },
            },
        ),
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;
    print_exit_summary_card(&state, &outcome, plain);
    Ok(())
}

fn undo_command(run: Option<String>, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match run {
        Some(run_id) => load_cli_run(&paths, &run_id)?,
        None => latest_run(&paths, false)?,
    };
    let target_turn = turn.unwrap_or_else(|| state.turn.saturating_sub(1));
    let restore_state = undo_restore_state(&state)?;
    restore_snapshot(&restore_state, target_turn)?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: target_turn,
            event: "undo.restore_snapshot".to_string(),
            latency_ms: None,
            detail: json!({ "snapshot": format!("turn-{target_turn}") }),
        },
    )?;
    println!("restored run {} to turn {}", state.run_id, target_turn);
    Ok(())
}

fn undo_restore_state(
    state: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(state.clone());
    };
    if record.mode != CodebaseMode::InPlace {
        return Ok(state.clone());
    }
    let source_path = record.source_path.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "in-place codebase record missing source_path".to_string(),
        ))
    })?;
    let mut restore_state = state.clone();
    restore_state.working_dir = source_path.clone();
    Ok(restore_state)
}

#[derive(Debug)]
struct RewindCliOptions {
    to_turn: Option<u32>,
    to_provider_event: Option<u64>,
    to_checkpoint: Option<String>,
    preview: bool,
    apply: bool,
    json: bool,
}

#[derive(Debug)]
struct ResolvedRewindTarget {
    target: RewindTarget,
    checkpoint_id: String,
}

fn rewind_command(run_id: &str, options: &RewindCliOptions) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, run_id)?;
    let mode = rewind_mode(options)?;
    let resolved = resolve_rewind_target(&state, options)?;
    let checkpoint = read_checkpoint_by_id(&state, &resolved.checkpoint_id)?;
    let preview_dir = state
        .run_root
        .join("rewind-preview")
        .join(&resolved.checkpoint_id);
    materialize_checkpoint(&state, &resolved.checkpoint_id, &preview_dir)?;
    let files = changed_files_between_dirs(&state.working_dir, &preview_dir)?;

    if mode == RewindMode::Apply {
        ensure_checkpoint_applyable(&state, &checkpoint)?;
        if let Err(reason) = hash_guard_rewind_apply(&state, &preview_dir, &files) {
            append_rewind_event(
                &state,
                &RewindEvent {
                    version: 1,
                    timestamp: Utc::now(),
                    run_id: state.run_id.clone(),
                    target: resolved.target,
                    mode,
                    status: RewindStatus::Refused,
                    files: files.clone(),
                    reason: Some(reason.clone()),
                },
            )?;
            return Err(CliError::Core(DeadreckonError::InvalidInput(reason)));
        }
        apply_materialized_files(&state, &preview_dir, &files)?;
    }

    append_rewind_event(
        &state,
        &RewindEvent {
            version: 1,
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            target: resolved.target.clone(),
            mode,
            status: RewindStatus::Ok,
            files: files.clone(),
            reason: None,
        },
    )?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "run_id": state.run_id,
                "mode": rewind_mode_label(mode),
                "target": resolved.target,
                "checkpoint_id": resolved.checkpoint_id,
                "preview_dir": preview_dir,
                "files": files,
            }))?
        );
        return Ok(());
    }

    match mode {
        RewindMode::Preview => {
            println!(
                "rewind preview {} -> {}",
                resolved.checkpoint_id,
                preview_dir.display()
            );
        }
        RewindMode::Apply => {
            println!("rewound {} to {}", state.run_id, resolved.checkpoint_id);
        }
    }
    if files.is_empty() {
        println!("files: none");
    } else {
        println!("files:");
        for path in files {
            println!("  {}", path.display());
        }
    }
    Ok(())
}

fn rewind_mode(options: &RewindCliOptions) -> Result<RewindMode> {
    if options.preview && options.apply {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "choose only one of --preview or --apply".to_string(),
        )));
    }
    if options.apply {
        Ok(RewindMode::Apply)
    } else {
        Ok(RewindMode::Preview)
    }
}

fn resolve_rewind_target(
    state: &deadreckon_core::PipelineState,
    options: &RewindCliOptions,
) -> Result<ResolvedRewindTarget> {
    let target_count = usize::from(options.to_turn.is_some())
        + usize::from(options.to_provider_event.is_some())
        + usize::from(options.to_checkpoint.is_some());
    if target_count != 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "choose exactly one of --to-turn, --to-provider-event, or --to-checkpoint".to_string(),
        )));
    }
    let checkpoints = list_checkpoint_manifests(state)?;
    if checkpoints.is_empty() {
        return Err(CliError::Core(DeadreckonError::NotFound(
            "no provider checkpoints for run".to_string(),
        )));
    }

    if let Some(checkpoint_id) = options.to_checkpoint.as_ref() {
        if checkpoints
            .iter()
            .any(|checkpoint| checkpoint.checkpoint_id == *checkpoint_id)
        {
            return Ok(ResolvedRewindTarget {
                target: RewindTarget {
                    kind: RewindTargetKind::Checkpoint,
                    id: checkpoint_id.clone(),
                },
                checkpoint_id: checkpoint_id.clone(),
            });
        }
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "checkpoint {checkpoint_id}"
        ))));
    }

    if let Some(turn) = options.to_turn {
        let Some(checkpoint) = checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.deadreckon_turn == turn)
            .max_by(|left, right| left.checkpoint_id.cmp(&right.checkpoint_id))
        else {
            return Err(CliError::Core(DeadreckonError::NotFound(format!(
                "checkpoint for turn {turn}"
            ))));
        };
        return Ok(ResolvedRewindTarget {
            target: RewindTarget {
                kind: RewindTargetKind::Turn,
                id: turn.to_string(),
            },
            checkpoint_id: checkpoint.checkpoint_id.clone(),
        });
    }

    let Some(seq) = options.to_provider_event else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "choose exactly one of --to-turn, --to-provider-event, or --to-checkpoint".to_string(),
        )));
    };
    let events = read_flight_events(state)?;
    let event = events
        .iter()
        .find(|event| event.seq == seq)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::NotFound(format!("provider event {seq}")))
        })?;
    let checkpoint_id = event.checkpoint_id.clone().or_else(|| {
        checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.flight_session_id == event.flight_session_id
                    && checkpoint.attempt == event.attempt
                    && checkpoint
                        .provider_event_seq
                        .is_some_and(|value| value <= seq)
            })
            .max_by(|left, right| left.checkpoint_id.cmp(&right.checkpoint_id))
            .map(|checkpoint| checkpoint.checkpoint_id.clone())
    });
    let Some(checkpoint_id) = checkpoint_id else {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "checkpoint for provider event {seq}"
        ))));
    };
    Ok(ResolvedRewindTarget {
        target: RewindTarget {
            kind: RewindTargetKind::ProviderEvent,
            id: seq.to_string(),
        },
        checkpoint_id,
    })
}

fn read_checkpoint_by_id(
    state: &deadreckon_core::PipelineState,
    checkpoint_id: &str,
) -> Result<CheckpointManifest> {
    list_checkpoint_manifests(state)?
        .into_iter()
        .find(|checkpoint| checkpoint.checkpoint_id == checkpoint_id)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::NotFound(format!(
                "checkpoint {checkpoint_id}"
            )))
        })
}

fn ensure_checkpoint_applyable(
    state: &deadreckon_core::PipelineState,
    checkpoint: &CheckpointManifest,
) -> Result<()> {
    let manifest = read_flight_manifest(state)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::NotFound(
            "flight-manifest.json for run".to_string(),
        ))
    })?;
    let status = manifest
        .sessions
        .iter()
        .find(|session| session.flight_session_id == checkpoint.flight_session_id)
        .map(|session| session.status);
    if matches!(status, Some(FlightSessionStatus::Superseded)) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "cannot apply a superseded checkpoint; inspect it with show --flight".to_string(),
        )));
    }
    Ok(())
}

fn hash_guard_rewind_apply(
    state: &deadreckon_core::PipelineState,
    target_dir: &Path,
    files: &[PathBuf],
) -> std::result::Result<(), String> {
    let current = build_working_file_index(&state.working_dir).map_err(|err| err.to_string())?;
    let expected_dir = latest_expected_current_dir(state).map_err(|err| err.to_string())?;
    let expected = build_working_file_index(&expected_dir).map_err(|err| err.to_string())?;
    let target = build_working_file_index(target_dir).map_err(|err| err.to_string())?;
    for path in files {
        let current_hash = current.files.get(path).map(|fingerprint| &fingerprint.hash);
        let expected_hash = expected
            .files
            .get(path)
            .map(|fingerprint| &fingerprint.hash);
        let target_hash = target.files.get(path).map(|fingerprint| &fingerprint.hash);
        if current_hash != expected_hash && current_hash != target_hash {
            return Err(format!(
                "refusing rewind because {} has unrelated edits",
                path.display()
            ));
        }
    }
    Ok(())
}

fn latest_expected_current_dir(state: &deadreckon_core::PipelineState) -> Result<PathBuf> {
    let snapshot = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{}", state.turn));
    if snapshot.exists() {
        return Ok(snapshot);
    }
    let Some(checkpoint) = list_checkpoint_manifests(state)?
        .into_iter()
        .max_by(|left, right| left.checkpoint_id.cmp(&right.checkpoint_id))
    else {
        return Ok(state.working_dir.clone());
    };
    let expected_dir = state
        .run_root
        .join("rewind-preview")
        .join(".expected-current");
    materialize_checkpoint(state, &checkpoint.checkpoint_id, &expected_dir)?;
    Ok(expected_dir)
}

fn changed_files_between_dirs(left: &Path, right: &Path) -> Result<Vec<PathBuf>> {
    let left = build_working_file_index(left)?;
    let right = build_working_file_index(right)?;
    let paths = left
        .files
        .keys()
        .chain(right.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    Ok(paths
        .into_iter()
        .filter(|path| {
            left.files.get(path).map(|fingerprint| &fingerprint.hash)
                != right.files.get(path).map(|fingerprint| &fingerprint.hash)
        })
        .collect())
}

fn apply_materialized_files(
    state: &deadreckon_core::PipelineState,
    target_dir: &Path,
    files: &[PathBuf],
) -> Result<()> {
    for relative in files {
        let source = target_dir.join(relative);
        let dest = state.working_dir.join(relative);
        if source.exists() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &dest)?;
        } else if dest.exists() {
            fs::remove_file(&dest)?;
        }
    }
    Ok(())
}

fn rewind_mode_label(mode: RewindMode) -> &'static str {
    match mode {
        RewindMode::Preview => "preview",
        RewindMode::Apply => "apply",
    }
}

#[derive(Debug)]
struct WhyFailedReport {
    kind: &'static str,
    id: String,
    status: String,
    reason: Option<String>,
    evidence: Vec<String>,
    try_lines: Vec<String>,
}

fn render_why_failed(report: WhyFailedReport) {
    println!("{} {} failure summary", report.kind, report.id);
    let mut items = vec![("status", report.status)];
    if let Some(reason) = report.reason {
        items.push(("reason", reason));
    }
    let _ = ui::kv_block(ui::Stream::Stdout, &items);
    if !report.evidence.is_empty() {
        println!("evidence:");
        for line in report.evidence {
            println!("  - {line}");
        }
    }
    for line in report.try_lines {
        println!("try: {line}");
    }
}

fn show_chain_why_failed(chain: &Chain) {
    let has_failed_step = chain
        .steps
        .iter()
        .any(|step| step.status == ChainStepStatus::Failed || step.fail_reason.is_some());
    if chain.status == ChainStatus::Completed && !has_failed_step {
        println!("no failures detected");
        return;
    }

    let mut evidence = Vec::new();
    let mut try_lines = Vec::new();
    for step in &chain.steps {
        if step.status != ChainStepStatus::Failed && step.fail_reason.is_none() {
            continue;
        }
        evidence.push(format!(
            "step {} status {} run {}",
            step.index + 1,
            chain_step_status_label(step.status),
            step.run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string())
        ));
        if let Some(reason) = step.fail_reason.as_deref() {
            evidence.push(format!("step {} reason: {reason}", step.index + 1));
        }
        if let Some(run_id) = step.run_id.as_deref() {
            try_lines.push(format!(
                "deadreckon show {} --why-failed",
                run_prefix(run_id)
            ));
        }
    }

    render_why_failed(WhyFailedReport {
        kind: "chain",
        id: chain_prefix(&chain.chain_id),
        status: chain_status_label(chain).to_string(),
        reason: chain
            .failure_reason
            .clone()
            .or_else(|| chain.paused_reason.clone()),
        evidence,
        try_lines,
    });
}

fn show_plan_why_failed(paths: &DeadreckonPaths, plan: &Plan) {
    if plan.status == PlanStatus::Merged
        && plan
            .tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed)
    {
        println!("no failures detected");
        return;
    }
    let mut evidence = Vec::new();
    let mut try_lines = Vec::new();
    for task in &plan.tasks {
        if task.status == PlanTaskStatus::Completed {
            continue;
        }
        evidence.push(format!(
            "child {} {} status {} run {}",
            task.index,
            task.task_id,
            task_status_label(task.status),
            task.child_run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string())
        ));
        if let Some(run_id) = task.child_run_id.as_deref() {
            try_lines.push(format!(
                "deadreckon show {} --why-failed",
                run_prefix(run_id)
            ));
        }
        if let Some(summary) = task.summary_path.as_ref() {
            evidence.push(format!(
                "  summary {}",
                paths.plan_dir(&plan.plan_id).join(summary).display()
            ));
        }
    }
    let blockers = read_plan_messages(paths, &plan.plan_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|message| message.kind == PlanMessageKind::Blocker)
        .collect::<Vec<_>>();
    for message in blockers.iter().rev().take(3) {
        evidence.push(format!(
            "blocker {} -> {}: {}",
            message.from, message.to, message.summary
        ));
    }
    if let Some(event) = read_plan_events_lossy(paths, &plan.plan_id).last() {
        evidence.push(format!("latest plan event {}", plan_event_line(event)));
    }
    if let Some(line) = plan_merge_repair_status_line(paths, plan) {
        evidence.push(format!("merge repair {line}"));
    }
    let proofs = paths.merge_proofs(&plan.plan_id);
    if proofs.join("conflicts.json").is_file() {
        evidence.push(format!(
            "merge conflicts {}",
            proofs.join("conflicts.json").display()
        ));
    }
    if proofs.join("repair-request.json").is_file() {
        evidence.push(format!(
            "repair request {}",
            proofs.join("repair-request.json").display()
        ));
    }
    render_why_failed(WhyFailedReport {
        kind: "plan",
        id: run_prefix(&plan.plan_id),
        status: plan_status_label(plan.status).to_string(),
        reason: None,
        evidence,
        try_lines,
    });
}

fn show_run_why_failed(state: &deadreckon_core::PipelineState) -> Result<()> {
    if state.status == RunStatus::Completed {
        println!("no failures detected");
        return Ok(());
    }
    let traces = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
    let evidence = traces
        .iter()
        .rev()
        .filter(|trace| {
            trace.event.contains("error")
                || trace.event.contains("failed")
                || trace.detail.get("exit_code").is_some()
                || trace.detail.get("stderr").is_some()
        })
        .take(3)
        .map(|trace| {
            format!(
                "turn {} {} {}",
                trace.turn,
                trace.event,
                one_line(&trace.detail.to_string(), 200)
            )
        })
        .collect::<Vec<_>>();
    render_why_failed(WhyFailedReport {
        kind: "run",
        id: run_prefix(&state.run_id),
        status: run_status_label(state.status).to_string(),
        reason: state.failure_reason.clone(),
        evidence,
        try_lines: Vec::new(),
    });
    Ok(())
}

fn show_flight(
    state: &deadreckon_core::PipelineState,
    turn: Option<u32>,
    file: Option<&Path>,
    json_output: bool,
) -> Result<()> {
    let manifest = read_flight_manifest(state)?;
    let file_filter = file.and_then(|path| normalize_flight_file_filter(path, &state.working_dir));
    let events = read_flight_events(state)?;
    let checkpoints = list_checkpoint_manifests(state)?;
    let filtered_checkpoints = checkpoints
        .iter()
        .filter(|checkpoint| turn.is_none_or(|turn| checkpoint.deadreckon_turn == turn))
        .filter(|checkpoint| {
            file_filter
                .as_ref()
                .is_none_or(|file| checkpoint_matches_file(checkpoint, file))
        })
        .cloned()
        .collect::<Vec<_>>();
    let checkpoint_ids = filtered_checkpoints
        .iter()
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<BTreeSet<_>>();
    let filtered_events = events
        .into_iter()
        .filter(|event| turn.is_none_or(|turn| event.deadreckon_turn == turn))
        .filter(|event| {
            file_filter.as_ref().is_none_or(|file| {
                event_matches_file(event, file)
                    || event
                        .checkpoint_id
                        .as_ref()
                        .is_some_and(|checkpoint_id| checkpoint_ids.contains(checkpoint_id))
            })
        })
        .collect::<Vec<_>>();

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "flight",
                "id": &state.run_id,
                "available": manifest.is_some(),
                "turn": turn,
                "file": file_filter,
                "manifest": manifest,
                "events": filtered_events,
                "checkpoints": filtered_checkpoints,
            }))?
        );
        return Ok(());
    }

    let Some(manifest) = manifest else {
        println!(
            "no flight recorder data for run {}",
            run_prefix(&state.run_id)
        );
        println!("try: deadreckon show {}", run_prefix(&state.run_id));
        return Ok(());
    };

    println!("flight {}", ui_id(run_prefix(&state.run_id)));
    if let Some(turn) = turn {
        println!("turn {turn}");
    }
    if let Some(file) = file_filter.as_ref() {
        println!("file {}", file.display());
    }

    println!("sessions:");
    for session in &manifest.sessions {
        if turn.is_some_and(|turn| session.deadreckon_turn != turn) {
            continue;
        }
        println!(
            "  turn {} attempt {} {} {} schema {}",
            session.deadreckon_turn,
            session.attempt,
            session.provider,
            flight_session_status_label(session.status),
            session.schema
        );
        for source in &session.source_paths {
            println!(
                "    source {}:{}-{} {}",
                source.path.display(),
                source.first_line,
                source.last_line,
                source.content_hash
            );
        }
    }

    println!("events:");
    if filtered_events.is_empty() {
        println!("  none");
    }
    for event in &filtered_events {
        let checkpoint = event
            .checkpoint_id
            .as_deref()
            .map(|id| format!(" checkpoint={id}"))
            .unwrap_or_default();
        let source = event
            .source_path
            .as_ref()
            .zip(event.source_line)
            .map(|(path, line)| format!(" {}:{line}", path.display()))
            .unwrap_or_default();
        println!(
            "  #{:06} turn {} {}{}{} {}",
            event.seq,
            event.deadreckon_turn,
            flight_event_kind_label(event.kind),
            checkpoint,
            source,
            one_line(&event.summary, 120)
        );
    }

    println!("checkpoints:");
    if filtered_checkpoints.is_empty() {
        println!("  none");
    }
    for checkpoint in &filtered_checkpoints {
        println!(
            "  {} turn {} attempt {} files {}{}",
            checkpoint.checkpoint_id,
            checkpoint.deadreckon_turn,
            checkpoint.attempt,
            checkpoint.files.len(),
            if checkpoint.full_anchor {
                " anchor"
            } else {
                ""
            }
        );
        if file_filter.is_some() {
            for change in &checkpoint.files {
                println!("    {:?} {}", change.change, change.path.display());
            }
        }
    }
    Ok(())
}

fn normalize_flight_file_filter(path: &Path, working_dir: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        path.strip_prefix(working_dir).ok().map(Path::to_path_buf)
    } else {
        Some(path.to_path_buf())
    }
}

fn event_matches_file(event: &FlightEvent, file: &Path) -> bool {
    event.files.iter().any(|path| path == file)
}

fn checkpoint_matches_file(checkpoint: &CheckpointManifest, file: &Path) -> bool {
    checkpoint.files.iter().any(|change| change.path == file)
}

fn flight_event_kind_label(kind: FlightEventKind) -> &'static str {
    match kind {
        FlightEventKind::Agent => "agent",
        FlightEventKind::Thinking => "thinking",
        FlightEventKind::Tool => "tool",
        FlightEventKind::Result => "result",
        FlightEventKind::Todo => "todo",
        FlightEventKind::Tokens => "tokens",
        FlightEventKind::Session => "session",
        FlightEventKind::Checkpoint => "checkpoint",
        FlightEventKind::Warning => "warning",
        FlightEventKind::Error => "error",
    }
}

fn flight_session_status_label(status: FlightSessionStatus) -> &'static str {
    match status {
        FlightSessionStatus::Running => "running",
        FlightSessionStatus::Completed => "completed",
        FlightSessionStatus::Failed => "failed",
        FlightSessionStatus::Killed => "killed",
        FlightSessionStatus::Superseded => "superseded",
    }
}

fn show_command(
    run_id: &str,
    turn: Option<u32>,
    why_failed: bool,
    _plain: bool,
    json_output: bool,
    flight: bool,
    file: Option<&Path>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut child_context: Option<PlanChildSelection> = None;
    let state = if let Some(selection) = resolve_plan_child_ref(&paths, run_id)? {
        let state = load_run(&paths, &selection.run_id)?;
        child_context = Some(selection);
        state
    } else {
        match load_cli_run(&paths, run_id) {
            Ok(state) => state,
            Err(run_error) => {
                if let Ok(plan_id) = resolve_plan_id(&paths, run_id) {
                    let plan = load_plan(&paths, &plan_id)?;
                    if json_output {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "kind": "plan",
                                "id": &plan.plan_id,
                                "status": plan_status_label(plan.status),
                                "next_actions": plan_next_actions(&plan),
                                "try_lines": Vec::<String>::new(),
                                "paths": plan_paths_json(&plan),
                                "plan": plan,
                            }))?
                        );
                        return Ok(());
                    }
                    if why_failed {
                        show_plan_why_failed(&paths, &plan);
                        return Ok(());
                    }
                    print_plan_summary(&paths, &plan, true);
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                    return Ok(());
                }
                return Err(run_error);
            }
        }
    };
    if flight || file.is_some() {
        return show_flight(&state, turn, file, json_output);
    }
    if json_output {
        let status = run_status_label(state.status);
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "run",
                "id": &state.run_id,
                "status": status,
                "next_actions": [next_action_label(&paths, &state)],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "state": state.state_path(),
                    "run_root": &state.run_root,
                    "working": &state.working_dir,
                    "artifact": &state.promoted_library_dir,
                },
                "run": state,
                "plan_child": child_context,
            }))?
        );
        return Ok(());
    }
    if why_failed {
        return show_run_why_failed(&state);
    }
    if let Some(selection) = child_context.as_ref() {
        println!(
            "plan {} / {} -> run {}",
            run_prefix(&selection.plan_id),
            selection.task_id,
            run_prefix(&selection.run_id)
        );
    }
    print_run_locations(&state);
    if let Some(line) = chain_context_line_for_working(&state.working_dir)? {
        println!("{line}");
    }
    if let Some(marker) = read_parent_marker(&state.working_dir)?
        && marker.kind == "extended"
    {
        println!("Extended from {}", marker.parent_run_id);
    }
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        println!("Mode {}", record.mode);
        if let Some(branch) = record.branch_name.as_deref() {
            println!("Branch {branch}");
        }
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("Worktree {}", worktree.display());
        }
        if let Some(source) = record.source_path.as_ref() {
            println!("Source {}", source.display());
        }
    }
    println!("Acceptance {}", acceptance_status_line(&state));
    let run_acceptance = acceptance_spec_path_for_run_root(&state.run_root);
    if run_acceptance.exists() {
        println!("AcceptanceSpec {}", run_acceptance.display());
    }
    println!("{}", serde_json::to_string_pretty(&state)?);
    let provenance_path = state.run_root.join("provenance.jsonl");
    if provenance_path.exists() {
        println!("provenance:");
        let raw = std::fs::read_to_string(provenance_path)?;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let record: ProvenanceRecord = serde_json::from_str(line)?;
            if turn.is_some_and(|turn| record.prompt_id != format!("turn-{turn}")) {
                continue;
            }
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
    }
    let traces_path = state.run_root.join("traces.jsonl");
    if traces_path.exists() {
        println!("traces:");
        let raw = std::fs::read_to_string(traces_path)?;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let record: TraceRecord = serde_json::from_str(line)?;
            if turn.is_some_and(|turn| record.turn != turn) {
                continue;
            }
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
    }
    Ok(())
}

fn read_parent_marker(root: &Path) -> Result<Option<ParentMarker>> {
    let path = root.join(".deadreckon/parent.json");
    match fs::read(&path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)?;
            if value.get("parent_run_id").is_none() {
                return Ok(None);
            }
            Ok(Some(serde_json::from_value(value)?))
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

#[derive(Debug)]
struct ImportCommandOptions {
    source: String,
    preview: bool,
    list: bool,
    session: Option<String>,
    cwd: Option<PathBuf>,
    all: bool,
    since: Option<String>,
    replace: bool,
    json: bool,
}

#[derive(Debug, Clone)]
enum ImportSourceKind {
    Provider,
    Cursor,
}

#[derive(Debug, Clone)]
struct ResolvedImportSource {
    alias: String,
    source: String,
    schema: String,
    storage: IngestStorage,
    roots: Vec<PathBuf>,
    cwd_match: IngestCwdMatch,
    cwd_match_path: Option<String>,
    file_glob: Option<String>,
    id_prefix: Option<String>,
    kind: ImportSourceKind,
}

#[derive(Debug, Clone, Serialize)]
struct ImportCandidate {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    paths: Vec<PathBuf>,
    root: PathBuf,
    updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_count_hint: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ImportMode {
    Session,
    All,
}

#[derive(Debug, Serialize, Deserialize)]
struct ImportManifest {
    version: u32,
    source: String,
    source_alias: String,
    schema: String,
    storage: String,
    cwd: PathBuf,
    mode: String,
    session_id: Option<String>,
    session_paths: Vec<PathBuf>,
    content_hash: String,
    imported_at: DateTime<Utc>,
    source_started_at: Option<DateTime<Utc>>,
    source_updated_at: Option<DateTime<Utc>>,
    rows_seen: usize,
    events_imported: usize,
    provenance_records: usize,
    raw_rows_stored: bool,
    reimport_command: String,
}

#[derive(Debug, Clone)]
struct ImportParseResult {
    rows_seen: usize,
    events: Vec<ImportedEvent>,
    source_started_at: Option<DateTime<Utc>>,
    source_updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ImportedEvent {
    timestamp: Option<DateTime<Utc>>,
    source_event: String,
    role: Option<String>,
    summary: String,
    tool_name: Option<String>,
    tool_category: Option<String>,
    tool_call_id: Option<String>,
    files: Vec<PathBuf>,
    usage: Option<ImportedUsage>,
    source_path: PathBuf,
    source_line: Option<usize>,
    raw_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct ImportedUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ImportedTraceDetail<'a> {
    import_version: u32,
    source: &'a str,
    schema: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    source_path: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_line: Option<usize>,
    source_event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_category: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    files: &'a [PathBuf],
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<&'a ImportedUsage>,
    raw_hash: &'a str,
}

#[derive(Debug, Serialize)]
struct ImportJsonOutput<'a> {
    kind: &'a str,
    source: &'a str,
    schema: &'a str,
    roots: &'a [PathBuf],
    candidates: &'a [ImportCandidate],
    try_lines: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImportCompletedJson<'a> {
    kind: &'a str,
    run_id: &'a str,
    manifest_path: PathBuf,
    manifest: &'a ImportManifest,
    try_lines: Vec<String>,
}

// SAFETY: Import options are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn import_command(options: ImportCommandOptions) -> Result<()> {
    // Import is a read-only recovery bridge. It reads provider transcript roots
    // through descriptor [ingest] metadata and writes only deadreckon run state.
    let paths = DeadreckonPaths::discover();
    let cwd = options
        .cwd
        .as_deref()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        .unwrap_or(std::env::current_dir()?);
    let since = import_since(&options)?;
    let resolved = resolve_import_source(&paths, &options, &cwd)?;
    let candidates = discover_import_candidates(&resolved, &options, &cwd, since);

    if options.list {
        print_import_candidates(&resolved, &candidates, options.json, "import_candidates")?;
        return Ok(());
    }

    if candidates.is_empty() {
        let stale = stale_import_candidates(&resolved, &options, &cwd, since);
        if !stale.is_empty() {
            return Err(import_invalid(format!(
                "no fresh import candidates for {}; stale candidates were found\n{}\ntry: deadreckon import {} --since 1d --preview\ntry: deadreckon import {} --session <id-or-path>",
                resolved.alias,
                import_candidate_table(&stale),
                resolved.alias,
                resolved.alias
            )));
        }
    }

    let (selected, mode) = select_import_candidates(&resolved, &options, &candidates)?;
    if options.preview {
        print_import_selection(&resolved, &selected, mode, options.json)?;
        return Ok(());
    }

    let (run_id, manifest) = normalize_import(&paths, &resolved, &selected, mode, &options, &cwd)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ImportCompletedJson {
                kind: "import_completed",
                run_id: &run_id,
                manifest_path: paths
                    .run_root(&workspace_scope(&cwd).map_err(CliError::from)?, &run_id)
                    .join("import.json"),
                manifest: &manifest,
                try_lines: vec![
                    format!("deadreckon show {run_id}"),
                    manifest.reimport_command.clone(),
                ],
            })?
        );
        return Ok(());
    }

    println!("source {}", resolved.source);
    println!("schema {}", resolved.schema);
    println!("mode {}", import_mode_label(mode));
    if let Some(session_id) = manifest.session_id.as_deref() {
        println!("session {session_id}");
    }
    for path in &manifest.session_paths {
        println!("path {}", path.display());
    }
    println!("imported {run_id}");
    println!("events {}", manifest.events_imported);
    println!("provenance {}", manifest.provenance_records);
    println!(
        "manifest {}",
        paths
            .run_root(&manifest_scope(&cwd)?, &run_id)
            .join("import.json")
            .display()
    );
    println!("try: deadreckon show {run_id}");
    println!("try: {}", manifest.reimport_command);
    Ok(())
}

fn manifest_scope(cwd: &Path) -> Result<String> {
    workspace_scope(cwd).map_err(CliError::from)
}

fn normalize_import(
    paths: &DeadreckonPaths,
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
    options: &ImportCommandOptions,
    cwd: &Path,
) -> Result<(String, ImportManifest)> {
    let source_paths = import_source_paths(resolved, selected)?;
    let content_hash = sha256_for_paths(&source_paths)?;
    let imported_id = import_run_id(resolved, selected, mode);
    let scope = workspace_scope(cwd).map_err(CliError::from)?;
    let existing_root = paths.run_root(&scope, &imported_id);
    if existing_root.exists() {
        if let Some(previous) = read_import_manifest(&existing_root)?
            && previous.content_hash != content_hash
            && !options.replace
        {
            return Err(import_invalid(format!(
                "existing import run {} has changed content\nold {}\nnew {}\ntry: deadreckon import {} --session {} --replace",
                imported_id,
                previous.content_hash,
                content_hash,
                resolved.alias,
                shell_arg(&selected_session_arg(selected))
            )));
        }
        fs::remove_dir_all(&existing_root)?;
    }

    let imported_at = Utc::now();
    let parsed = parse_import_candidates(resolved, selected)?;
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!(
                "imported {} {}",
                import_display_source(&resolved.source),
                import_mode_label(mode)
            ),
            cwd: cwd.to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some(format!("import:{}", resolved.source)),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: Some(imported_id.clone()),
            codebase: None,
        },
    )?;

    let session_id = import_session_id(selected, mode);
    let mut provenance_records = 0usize;
    for (idx, event) in parsed.events.iter().enumerate() {
        let turn = (idx + 1) as u32;
        append_trace(
            &state,
            &TraceRecord {
                timestamp: event.timestamp.unwrap_or(imported_at),
                run_id: state.run_id.clone(),
                turn,
                event: format!("import.{}", import_display_source(&resolved.source)),
                latency_ms: None,
                detail: serde_json::to_value(ImportedTraceDetail {
                    import_version: 1,
                    source: &resolved.source,
                    schema: &resolved.schema,
                    session_id: session_id.as_deref(),
                    source_path: &event.source_path,
                    source_line: event.source_line,
                    source_event: &event.source_event,
                    role: event.role.as_deref(),
                    summary: &event.summary,
                    tool_name: event.tool_name.as_deref(),
                    tool_category: event.tool_category.as_deref(),
                    tool_call_id: event.tool_call_id.as_deref(),
                    files: &event.files,
                    usage: event.usage.as_ref(),
                    raw_hash: &event.raw_hash,
                })?,
            },
        )?;
        if !event.files.is_empty() {
            provenance_records += 1;
            append_provenance(
                &state,
                &ProvenanceRecord {
                    timestamp: event.timestamp.unwrap_or(imported_at),
                    prompt_id: format!("turn-{turn}"),
                    model: format!("import:{}", resolved.source),
                    tool_call_id: event
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| format!("imported-{turn}")),
                    session_id: state.run_id.clone(),
                    files: event.files.clone(),
                },
            )?;
        }
    }

    state.turn = parsed.events.len() as u32;
    state.status = RunStatus::Completed;
    state.updated_at = Utc::now();
    save_state(&state)?;

    let manifest = ImportManifest {
        version: 1,
        source: resolved.source.clone(),
        source_alias: resolved.alias.clone(),
        schema: resolved.schema.clone(),
        storage: ingest_storage_label(&resolved.storage).to_string(),
        cwd: cwd.to_path_buf(),
        mode: import_mode_label(mode).to_string(),
        session_id,
        session_paths: source_paths,
        content_hash,
        imported_at,
        source_started_at: parsed.source_started_at,
        source_updated_at: parsed.source_updated_at,
        rows_seen: parsed.rows_seen,
        events_imported: parsed.events.len(),
        provenance_records,
        raw_rows_stored: false,
        reimport_command: reimport_command(resolved, selected, mode),
    };
    fs::write(
        state.run_root.join("import.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok((imported_id, manifest))
}

fn resolve_import_source(
    paths: &DeadreckonPaths,
    options: &ImportCommandOptions,
    cwd: &Path,
) -> Result<ResolvedImportSource> {
    let source = options.source.trim();
    if source == "cursor" {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.home().to_path_buf());
        let root = std::env::var_os("DEADRECKON_IMPORT_CURSOR_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cursor/chats"));
        return Ok(ResolvedImportSource {
            alias: source.to_string(),
            source: "cursor".to_string(),
            schema: "cursor-sqlite".to_string(),
            storage: IngestStorage::Json,
            roots: vec![root],
            cwd_match: IngestCwdMatch::None,
            cwd_match_path: None,
            file_glob: Some("*.db".to_string()),
            id_prefix: Some("cursor:".to_string()),
            kind: ImportSourceKind::Cursor,
        });
    }

    let descriptor_id = import_descriptor_id(source).ok_or_else(|| {
        import_invalid(format!(
            "unknown import source {source}; accepted sources: {}\ntry: deadreckon import codex --list",
            accepted_import_sources().join(", ")
        ))
    })?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let descriptor = registry.get(&descriptor_id).ok_or_else(|| {
        import_invalid(format!(
            "unknown import source {source}; accepted sources: {}\ntry: deadreckon import codex --list",
            accepted_import_sources().join(", ")
        ))
    })?;
    let ingest = descriptor.ingest.clone().ok_or_else(|| {
        import_invalid(format!(
            "{} has no importable descriptor [ingest]\ntry: deadreckon providers list --all",
            descriptor.id
        ))
    })?;
    let schema = ingest.schema.trim();
    if schema.is_empty() {
        return Err(import_invalid(format!(
            "{} has an empty descriptor ingest schema\ntry: deadreckon providers list --all",
            descriptor.id
        )));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.home().to_path_buf());
    let working_dirs = vec![cwd.to_string_lossy().to_string()];
    let roots = provider_ingest_roots_for_working_dirs(
        &ingest,
        &home,
        &working_dirs,
        options.all || options.session.is_some(),
    );
    let storage = ingest.storage.clone().unwrap_or(IngestStorage::Jsonl);
    Ok(ResolvedImportSource {
        alias: source.to_string(),
        source: descriptor.id.clone(),
        schema: schema.to_string(),
        storage,
        roots,
        cwd_match: ingest.cwd_match.clone(),
        cwd_match_path: ingest.cwd_match_path.clone(),
        file_glob: ingest.file_glob.clone(),
        id_prefix: ingest.id_prefix.clone(),
        kind: ImportSourceKind::Provider,
    })
}

fn discover_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let effective_since = if options.all || options.session.is_some() {
        import_long_ago()
    } else {
        since
    };
    let mut candidates = match &resolved.kind {
        ImportSourceKind::Provider => {
            discover_provider_import_candidates(resolved, cwd, effective_since)
        }
        ImportSourceKind::Cursor => discover_cursor_import_candidates(resolved, effective_since),
    };
    if let Some(session) = options.session.as_deref()
        && !candidates
            .iter()
            .any(|candidate| import_candidate_matches(candidate, session))
        && Path::new(session).exists()
    {
        candidates.push(candidate_from_explicit_path(resolved, Path::new(session)));
    }
    candidates.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn stale_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    if options.all || options.session.is_some() {
        return Vec::new();
    }
    let mut candidates = match &resolved.kind {
        ImportSourceKind::Provider => {
            discover_provider_import_candidates(resolved, cwd, import_long_ago())
        }
        ImportSourceKind::Cursor => discover_cursor_import_candidates(resolved, import_long_ago()),
    };
    candidates.retain(|candidate| candidate.updated_at < since);
    candidates.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn discover_provider_import_candidates(
    resolved: &ResolvedImportSource,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let spec = ProviderJsonlLogSpec {
        schema: resolved.schema.clone(),
        roots: resolved.roots.clone(),
        since,
        cwd_match: resolved.cwd_match.clone(),
        cwd_match_path: resolved.cwd_match_path.clone(),
        storage: resolved.storage.clone(),
        file_glob: resolved.file_glob.clone(),
    };
    let working_dirs = vec![cwd.to_string_lossy().to_string()];
    let mut candidates = Vec::new();
    for root in &resolved.roots {
        let mut files = Vec::new();
        collect_recent_provider_files(root, &spec, &mut files, 0);
        for (path, updated_at) in files {
            let matched_cwd = provider_jsonl_session_matches_run(&spec, &path, &working_dirs)
                .then(|| cwd.to_path_buf());
            let session_id = provider_import_session_id(resolved, &path);
            let id = import_candidate_id(resolved, session_id.as_deref(), &path);
            candidates.push(ImportCandidate {
                id,
                session_id,
                paths: vec![path.clone()],
                root: root.clone(),
                updated_at,
                matched_cwd,
                row_count_hint: import_row_count_hint(resolved, &path),
            });
        }
    }
    candidates
}

fn discover_cursor_import_candidates(
    resolved: &ResolvedImportSource,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let mut candidates = Vec::new();
    for root in &resolved.roots {
        let Ok(files) = inventory_files(root) else {
            continue;
        };
        for path in files {
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if !matches!(extension, "sqlite" | "sqlite3" | "db") {
                continue;
            }
            let Some(updated_at) = fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(DateTime::<Utc>::from)
            else {
                continue;
            };
            if updated_at < since {
                continue;
            }
            let session_id = path.file_stem().and_then(|stem| stem.to_str()).map(|stem| {
                let prefix = resolved.id_prefix.as_deref().unwrap_or("cursor:");
                format!("{prefix}{stem}")
            });
            let id = import_candidate_id(resolved, session_id.as_deref(), &path);
            candidates.push(ImportCandidate {
                id,
                session_id,
                paths: vec![path],
                root: root.clone(),
                updated_at,
                matched_cwd: None,
                row_count_hint: None,
            });
        }
    }
    candidates
}

fn candidate_from_explicit_path(resolved: &ResolvedImportSource, path: &Path) -> ImportCandidate {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let updated_at = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    let root = resolved
        .roots
        .iter()
        .find(|root| path.starts_with(root))
        .cloned()
        .unwrap_or_else(|| explicit_import_root(resolved, &path));
    let session_id = provider_import_session_id(resolved, &path).or_else(|| {
        path.file_stem().and_then(|stem| stem.to_str()).map(|stem| {
            let prefix = resolved.id_prefix.as_deref().unwrap_or("");
            format!("{prefix}{stem}")
        })
    });
    ImportCandidate {
        id: import_candidate_id(resolved, session_id.as_deref(), &path),
        session_id,
        paths: vec![path.clone()],
        root,
        updated_at,
        matched_cwd: None,
        row_count_hint: import_row_count_hint(resolved, &path),
    }
}

fn explicit_import_root(resolved: &ResolvedImportSource, path: &Path) -> PathBuf {
    if resolved.storage == IngestStorage::OpenCodeStorage
        && let Some(root) = path.ancestors().nth(4)
    {
        return root.to_path_buf();
    }
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn select_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    candidates: &[ImportCandidate],
) -> Result<(Vec<ImportCandidate>, ImportMode)> {
    if candidates.is_empty() {
        return Err(import_invalid(format!(
            "no import candidates for {}\nresolved roots:\n{}\ntry: deadreckon import {} --all --preview",
            resolved.alias,
            resolved_roots_lines(&resolved.roots),
            resolved.alias
        )));
    }
    if options.all {
        return Ok((candidates.to_vec(), ImportMode::All));
    }
    if let Some(session) = options.session.as_deref() {
        let selected = candidates
            .iter()
            .filter(|candidate| import_candidate_matches(candidate, session))
            .cloned()
            .collect::<Vec<_>>();
        return match selected.len() {
            0 => Err(import_invalid(format!(
                "no import candidate matched session {session}\n{}\ntry: deadreckon import {} --list",
                import_candidate_table(candidates),
                resolved.alias
            ))),
            1 => Ok((selected, ImportMode::Session)),
            _ => Err(import_invalid(format!(
                "session {session} matched multiple import candidates\n{}\ntry: deadreckon import {} --session {}",
                import_candidate_table(&selected),
                resolved.alias,
                shell_arg(session)
            ))),
        };
    }

    let cwd_matched = candidates
        .iter()
        .filter(|candidate| {
            resolved.cwd_match == IngestCwdMatch::None || candidate.matched_cwd.is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    if cwd_matched.len() == 1 {
        return Ok((cwd_matched, ImportMode::Session));
    }
    if cwd_matched.len() > 1 {
        return Err(import_invalid(format!(
            "ambiguous import candidates for {}\n{}\ntry: deadreckon import {} --session <id-or-path>",
            resolved.alias,
            import_candidate_table(&cwd_matched),
            resolved.alias
        )));
    }
    if candidates.len() == 1 {
        return Ok((vec![candidates[0].clone()], ImportMode::Session));
    }
    Err(import_invalid(format!(
        "no cwd-matched import session for {}; {} recent candidates need an explicit session\n{}\ntry: deadreckon import {} --session <id-or-path>",
        resolved.alias,
        candidates.len(),
        import_candidate_table(candidates),
        resolved.alias
    )))
}

fn parse_import_candidates(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
) -> Result<ImportParseResult> {
    let mut rows_seen = 0usize;
    let mut events = Vec::new();
    let mut source_started_at: Option<DateTime<Utc>> = None;
    let mut source_updated_at: Option<DateTime<Utc>> = None;
    for candidate in selected {
        let parsed = match &resolved.kind {
            ImportSourceKind::Provider => parse_provider_import_candidate(resolved, candidate)?,
            ImportSourceKind::Cursor => parse_cursor_import_candidate(candidate)?,
        };
        rows_seen += parsed.rows_seen;
        for timestamp in parsed
            .source_started_at
            .into_iter()
            .chain(parsed.source_updated_at)
        {
            source_started_at = Some(
                source_started_at
                    .map(|existing| existing.min(timestamp))
                    .unwrap_or(timestamp),
            );
            source_updated_at = Some(
                source_updated_at
                    .map(|existing| existing.max(timestamp))
                    .unwrap_or(timestamp),
            );
        }
        events.extend(parsed.events);
    }
    events.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_provider_import_candidate(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
) -> Result<ImportParseResult> {
    if resolved.storage == IngestStorage::OpenCodeStorage {
        return parse_opencode_import_candidate(resolved, candidate);
    }
    let mut rows_seen = 0usize;
    let mut events = Vec::new();
    for path in &candidate.paths {
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let raw = fs::read_to_string(path)?;
            let value = serde_json::from_str::<Value>(&raw).map_err(|err| {
                import_invalid(format!(
                    "malformed JSON at {}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
                    path.display(),
                    path.display(),
                    resolved.alias,
                    shell_arg(&candidate.id)
                ))
            })?;
            rows_seen += import_json_value_row_count(&value);
            events.extend(import_events_from_json_value(
                resolved, candidate, path, None, &value, &raw,
            ));
            continue;
        }
        for (line_idx, line) in fs::read_to_string(path)?.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            rows_seen += 1;
            let value = serde_json::from_str::<Value>(line).map_err(|err| {
                import_invalid(format!(
                    "malformed JSONL at {}:{}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
                    path.display(),
                    line_idx + 1,
                    path.display(),
                    resolved.alias,
                    shell_arg(&candidate.id)
                ))
            })?;
            events.extend(import_events_from_json_value(
                resolved,
                candidate,
                path,
                Some(line_idx + 1),
                &value,
                line,
            ));
        }
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_cursor_import_candidate(candidate: &ImportCandidate) -> Result<ImportParseResult> {
    let Some(path) = candidate.paths.first() else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let output = std::process::Command::new("sqlite3")
        .arg("-json")
        .arg(path)
        .arg("select rowid as source_rowid, * from messages order by rowid")
        .output();
    let output = output.map_err(|err| {
        import_invalid(format!(
            "sqlite3 is required to import Cursor history from {}: {err}\ntry: install sqlite3 or pass a JSONL-capable provider source",
            path.display()
        ))
    })?;
    if !output.status.success() {
        return Err(import_invalid(format!(
            "failed to query Cursor database {}: {}\ntry: install sqlite3 or pass a JSONL-capable provider source",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let values: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|err| {
        import_invalid(format!(
            "sqlite3 returned invalid JSON for {}: {err}",
            path.display()
        ))
    })?;
    let mut events = Vec::new();
    for value in &values {
        let raw = serde_json::to_string(value)?;
        let mut event = generic_import_event(
            "cursor-sqlite",
            candidate,
            path,
            value
                .get("source_rowid")
                .and_then(Value::as_u64)
                .and_then(|row| usize::try_from(row).ok()),
            value,
            &raw,
        );
        event.source_event = "message".to_string();
        events.push(event);
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen: values.len(),
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_opencode_import_candidate(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
) -> Result<ImportParseResult> {
    let Some(session_path) = candidate.paths.first() else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let session_raw = fs::read_to_string(session_path)?;
    let session = serde_json::from_str::<Value>(&session_raw).map_err(|err| {
        import_invalid(format!(
            "malformed JSON at {}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
            session_path.display(),
            session_path.display(),
            resolved.alias,
            shell_arg(&candidate.id)
        ))
    })?;
    let Some(session_id) = session.get("id").and_then(Value::as_str) else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let root = session_path
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| candidate.root.clone());
    let messages = read_json_entries_sorted(&root.join("storage/message").join(session_id));
    let mut rows_seen = 1usize;
    let mut events = Vec::new();
    events.extend(import_events_from_json_value(
        resolved,
        candidate,
        session_path,
        None,
        &session,
        &session_raw,
    ));
    for (message_path, message, message_raw) in messages {
        rows_seen += 1;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let timestamp = import_timestamp(&message);
        let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let parts = read_json_entries_sorted(&root.join("storage/part").join(message_id));
        if parts.is_empty() {
            let mut event = generic_import_event(
                &resolved.schema,
                candidate,
                &message_path,
                None,
                &message,
                &message_raw,
            );
            event.role = role.clone();
            event.timestamp = timestamp;
            events.push(event);
            continue;
        }
        for (part_path, part, part_raw) in parts {
            rows_seen += 1;
            let mut event =
                opencode_import_event(candidate, &part_path, &part, &part_raw, role.as_deref());
            event.timestamp = import_timestamp(&part).or(timestamp);
            events.push(event);
        }
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn import_events_from_json_value(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    match resolved.schema.as_str() {
        "codex-cli" => codex_import_events(candidate, path, line, value, raw),
        "claude-code" => claude_import_events(candidate, path, line, value, raw),
        "gemini" => gemini_import_events(candidate, path, line, value, raw),
        "copilot-cli" => copilot_import_events(candidate, path, line, value, raw),
        "pi" => pi_import_events(candidate, path, line, value, raw),
        "opencode" => vec![generic_import_event(
            &resolved.schema,
            candidate,
            path,
            line,
            value,
            raw,
        )],
        _ => vec![generic_import_event(
            &resolved.schema,
            candidate,
            path,
            line,
            value,
            raw,
        )],
    }
}

fn codex_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let mut event = generic_import_event("codex-cli", candidate, path, line, value, raw);
    match (
        value.get("type").and_then(Value::as_str),
        payload.get("type").and_then(Value::as_str),
    ) {
        (Some("session_meta"), _) => {
            event.source_event = "session_meta".to_string();
            event.summary = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(|cwd| format!("session cwd {cwd}"))
                .unwrap_or_else(|| "session metadata".to_string());
        }
        (Some("event_msg"), Some("agent_message")) => {
            event.source_event = "agent_message".to_string();
            event.role = Some("assistant".to_string());
            if let Some(message) = payload.get("message").and_then(Value::as_str) {
                event.summary = one_line(message, 180);
            }
        }
        (Some("event_msg"), Some("token_count")) => {
            event.source_event = "usage".to_string();
            event.summary = "token count".to_string();
            event.usage = codex_usage(payload);
        }
        (Some("response_item"), Some("function_call")) => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args_value =
                serde_json::from_str::<Value>(args).unwrap_or(Value::String(args.to_string()));
            event.source_event = "tool_call".to_string();
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, &args_value), 160)
            );
            event.files = collect_import_paths(&args_value);
        }
        (Some("response_item"), Some("function_call_output")) => {
            event.source_event = "tool_result".to_string();
            event.role = Some("tool".to_string());
            event.tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = payload
                .get("output")
                .and_then(Value::as_str)
                .map(|output| format!("result {}", one_line(output, 160)))
                .unwrap_or_else(|| event.summary.clone());
        }
        _ => {}
    }
    vec![event]
}

fn claude_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let row_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let Some(message) = value.get("message") else {
        return vec![generic_import_event(
            "claude-code",
            candidate,
            path,
            line,
            value,
            raw,
        )];
    };
    let usage = message.get("usage").and_then(import_usage_from_value);
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        let mut event = generic_import_event("claude-code", candidate, path, line, value, raw);
        event.source_event = row_type.to_string();
        event.usage = usage;
        return vec![event];
    };
    let mut events = Vec::new();
    for part in content {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.role = Some(row_type.to_string());
        event.usage = usage.clone();
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                event.source_event = "message".to_string();
                event.summary = part
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| one_line(text, 180))
                    .unwrap_or_else(|| "message".to_string());
            }
            Some("thinking") => {
                event.source_event = "thinking".to_string();
                event.summary = part
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(|text| format!("thinking {}", one_line(text, 160)))
                    .unwrap_or_else(|| "thinking".to_string());
            }
            Some("tool_use") => {
                let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = part.get("input").unwrap_or(&Value::Null);
                event.source_event = "tool_call".to_string();
                event.tool_name = Some(name.to_string());
                event.tool_category = Some(provider_tool_label(name).to_string());
                event.tool_call_id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                event.summary = format!(
                    "tool {} {}",
                    provider_tool_label(name),
                    one_line(&claude_tool_summary(name, input), 160)
                );
                event.files = collect_import_paths(input);
            }
            Some("tool_result") => {
                event.source_event = "tool_result".to_string();
                event.tool_call_id = part
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                event.summary = format!(
                    "result {}",
                    one_line(
                        &claude_content_text(part.get("content").unwrap_or(&Value::Null)),
                        160
                    )
                );
                event.files = collect_import_paths(part);
            }
            Some(other) => {
                event.source_event = other.to_string();
                event.summary = one_line(&json_value_text(part), 180);
                event.files = collect_import_paths(part);
            }
            None => {
                event.source_event = row_type.to_string();
                event.summary = one_line(&json_value_text(part), 180);
            }
        }
        events.push(event);
    }
    if events.is_empty() {
        vec![generic_import_event(
            "claude-code",
            candidate,
            path,
            line,
            value,
            raw,
        )]
    } else {
        events
    }
}

fn gemini_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        let mut events = Vec::new();
        for message in messages {
            let raw = serde_json::to_string(message).unwrap_or_else(|_| raw.to_string());
            events.extend(gemini_import_events(candidate, path, line, message, &raw));
        }
        return events;
    }
    let mut events = Vec::new();
    let usage = gemini_usage(value);
    for thought in value
        .get("thoughts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.source_event = "thinking".to_string();
        event.role = Some("assistant".to_string());
        event.usage = usage.clone();
        event.summary = format!("thinking {}", one_line(&json_value_text(thought), 160));
        events.push(event);
    }
    for text in gemini_content_texts(value.get("content").unwrap_or(&Value::Null)) {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.source_event = "message".to_string();
        event.role = Some("assistant".to_string());
        event.usage = usage.clone();
        event.summary = one_line(&text, 180);
        events.push(event);
    }
    if let Some(tool_calls) = value.get("toolCalls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let name = tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = tool_call.get("args").unwrap_or(&Value::Null);
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_call".to_string();
            event.role = Some("assistant".to_string());
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, args), 160)
            );
            event.files = collect_import_paths(args);
            events.push(event);
        }
    }
    if events.is_empty() {
        vec![generic_import_event(
            "gemini", candidate, path, line, value, raw,
        )]
    } else {
        events
    }
}

fn copilot_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let data = value.get("data").unwrap_or(&Value::Null);
    let usage = value.get("usage").and_then(import_usage_from_value);
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str) {
        Some("assistant.message") => {
            if let Some(reasoning) = data.get("reasoningText").and_then(Value::as_str)
                && !reasoning.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "thinking".to_string();
                event.role = Some("assistant".to_string());
                event.usage = usage.clone();
                event.summary = format!("thinking {}", one_line(reasoning, 160));
                events.push(event);
            }
            if let Some(content) = data.get("content").and_then(Value::as_str)
                && !content.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "message".to_string();
                event.role = Some("assistant".to_string());
                event.usage = usage.clone();
                event.summary = one_line(content, 180);
                events.push(event);
            }
            if let Some(tool_requests) = data.get("toolRequests").and_then(Value::as_array) {
                for request in tool_requests {
                    let name = request
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let input = provider_arguments_value(request.get("arguments"));
                    let mut event = import_base_event(candidate, path, line, raw);
                    event.timestamp = import_timestamp(value);
                    event.source_event = "tool_call".to_string();
                    event.role = Some("assistant".to_string());
                    event.tool_name = Some(name.to_string());
                    event.tool_category = Some(provider_tool_label(name).to_string());
                    event.tool_call_id = request
                        .get("id")
                        .or_else(|| request.get("toolCallId"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    event.summary = format!(
                        "tool {} {}",
                        provider_tool_label(name),
                        one_line(&json_tool_summary(name, &input), 160)
                    );
                    event.files = collect_import_paths(&input);
                    event.usage = usage.clone();
                    events.push(event);
                }
            }
        }
        Some("assistant.reasoning") => {
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "thinking".to_string();
            event.role = Some("assistant".to_string());
            event.usage = usage;
            event.summary = data
                .get("text")
                .or_else(|| data.get("content"))
                .and_then(Value::as_str)
                .map(|text| format!("thinking {}", one_line(text, 160)))
                .unwrap_or_else(|| "thinking".to_string());
            events.push(event);
        }
        Some("tool.execution_complete") => {
            let result = data.get("result").unwrap_or(&Value::Null);
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_result".to_string();
            event.role = Some("tool".to_string());
            event.tool_call_id = data
                .get("toolCallId")
                .or_else(|| data.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!("result {}", one_line(&json_value_text(result), 160));
            event.files = collect_import_paths(result);
            event.usage = usage;
            events.push(event);
        }
        Some("session.model_change") => {
            let mut event = generic_import_event("copilot-cli", candidate, path, line, value, raw);
            event.source_event = "model_change".to_string();
            events.push(event);
        }
        _ => {}
    }
    if events.is_empty() {
        vec![generic_import_event(
            "copilot-cli",
            candidate,
            path,
            line,
            value,
            raw,
        )]
    } else {
        events
    }
}

fn pi_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    if value.get("type").and_then(Value::as_str) == Some("session") {
        let mut event = generic_import_event("pi", candidate, path, line, value, raw);
        event.source_event = "session".to_string();
        event.summary = "session header".to_string();
        return vec![event];
    }
    let message = value.get("message").unwrap_or(value);
    let usage = message.get("usage").and_then(import_usage_from_value);
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut events = Vec::new();
    match role.as_deref() {
        Some("assistant") => {
            let content = message.get("content").unwrap_or(&Value::Null);
            if let Some(text) = content.as_str()
                && !text.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "message".to_string();
                event.role = role.clone();
                event.usage = usage.clone();
                event.summary = one_line(text, 180);
                events.push(event);
            }
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    let mut event = import_base_event(candidate, path, line, raw);
                    event.timestamp = import_timestamp(value);
                    event.role = role.clone();
                    event.usage = usage.clone();
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            event.source_event = "message".to_string();
                            event.summary = block
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|text| one_line(text, 180))
                                .unwrap_or_else(|| "message".to_string());
                        }
                        Some("thinking") => {
                            event.source_event = "thinking".to_string();
                            event.summary = block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .map(|text| format!("thinking {}", one_line(text, 160)))
                                .unwrap_or_else(|| "thinking".to_string());
                        }
                        Some("toolCall") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = normalize_pi_tool_arguments(provider_arguments_value(
                                block.get("arguments"),
                            ));
                            event.source_event = "tool_call".to_string();
                            event.tool_name = Some(name.to_string());
                            event.tool_category = Some(provider_tool_label(name).to_string());
                            event.tool_call_id = block
                                .get("id")
                                .or_else(|| block.get("toolCallId"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            event.summary = format!(
                                "tool {} {}",
                                provider_tool_label(name),
                                one_line(&json_tool_summary(name, &input), 160)
                            );
                            event.files = collect_import_paths(&input);
                        }
                        Some(other) => {
                            event.source_event = other.to_string();
                            event.summary = one_line(&json_value_text(block), 180);
                            event.files = collect_import_paths(block);
                        }
                        None => {
                            event.source_event = "message".to_string();
                            event.summary = one_line(&json_value_text(block), 180);
                        }
                    }
                    events.push(event);
                }
            }
        }
        Some("toolResult") => {
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_result".to_string();
            event.role = role;
            event.usage = usage;
            event.summary = format!(
                "result {}",
                one_line(
                    &json_value_text(message.get("content").unwrap_or(&Value::Null)),
                    160
                )
            );
            event.files = collect_import_paths(message);
            events.push(event);
        }
        _ => {}
    }
    if events.is_empty() {
        vec![generic_import_event(
            "pi", candidate, path, line, value, raw,
        )]
    } else {
        events
    }
}

fn opencode_import_event(
    candidate: &ImportCandidate,
    path: &Path,
    value: &Value,
    raw: &str,
    role: Option<&str>,
) -> ImportedEvent {
    let mut event = generic_import_event("opencode", candidate, path, None, value, raw);
    event.role = role.map(ToString::to_string);
    event.source_event = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("part")
        .to_string();
    match value.get("type").and_then(Value::as_str) {
        Some("text") => {
            event.source_event = "message".to_string();
            event.summary = value
                .get("content")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .map(|text| one_line(text, 180))
                .unwrap_or_else(|| "message".to_string());
        }
        Some("reasoning") => {
            event.source_event = "thinking".to_string();
            event.summary = value
                .get("content")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .map(|text| format!("thinking {}", one_line(text, 160)))
                .unwrap_or_else(|| "thinking".to_string());
        }
        Some("tool") => {
            let name = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let input = value
                .pointer("/state/input")
                .or_else(|| value.get("input"))
                .unwrap_or(&Value::Null);
            event.source_event = "tool_call".to_string();
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = value
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, input), 160)
            );
            event.files = collect_import_paths(input);
        }
        _ => {}
    }
    event
}

fn generic_import_event(
    schema: &str,
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> ImportedEvent {
    let mut event = import_base_event(candidate, path, line, raw);
    event.timestamp = import_timestamp(value);
    event.source_event = import_source_event(value);
    event.role = value
        .get("role")
        .or_else(|| value.pointer("/message/role"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    event.summary = import_summary(schema, value);
    event.tool_name = import_tool_name(value);
    event.tool_category = event
        .tool_name
        .as_deref()
        .map(provider_tool_label)
        .map(ToString::to_string);
    event.tool_call_id = import_tool_call_id(value);
    event.files = collect_import_paths(value);
    event.usage = import_usage_for_schema(schema, value);
    event
}

fn import_base_event(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    raw: &str,
) -> ImportedEvent {
    ImportedEvent {
        timestamp: None,
        source_event: "row".to_string(),
        role: None,
        summary: "imported row".to_string(),
        tool_name: None,
        tool_category: None,
        tool_call_id: None,
        files: Vec::new(),
        usage: None,
        source_path: path.to_path_buf(),
        source_line: line,
        raw_hash: sha256_for_str(raw),
    }
    .with_candidate_tool_id(candidate)
}

trait ImportedEventExt {
    fn with_candidate_tool_id(self, candidate: &ImportCandidate) -> Self;
}

impl ImportedEventExt for ImportedEvent {
    fn with_candidate_tool_id(mut self, candidate: &ImportCandidate) -> Self {
        if self.tool_call_id.is_none() {
            self.tool_call_id = candidate.session_id.clone();
        }
        self
    }
}

fn import_since(options: &ImportCommandOptions) -> Result<DateTime<Utc>> {
    if options.all || options.session.is_some() {
        return Ok(import_long_ago());
    }
    let Some(raw) = options.since.as_deref() else {
        return Ok(Utc::now() - ChronoDuration::minutes(2));
    };
    let duration = parse_import_duration(raw).ok_or_else(|| {
        import_invalid(format!(
            "invalid import --since duration {raw}; use values like 10m, 2h, or 1d\ntry: deadreckon import {} --since 10m --list",
            options.source
        ))
    })?;
    Ok(Utc::now() - duration)
}

fn parse_import_duration(raw: &str) -> Option<ChronoDuration> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (number, unit) = trimmed.split_at(
        trimmed
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value = number.parse::<i64>().ok()?;
    match unit {
        "" | "m" | "min" | "mins" | "minute" | "minutes" => Some(ChronoDuration::minutes(value)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(ChronoDuration::hours(value)),
        "d" | "day" | "days" => Some(ChronoDuration::days(value)),
        "s" | "sec" | "secs" | "second" | "seconds" => Some(ChronoDuration::seconds(value)),
        _ => None,
    }
}

fn import_long_ago() -> DateTime<Utc> {
    Utc::now() - ChronoDuration::days(36_500)
}

fn accepted_import_sources() -> Vec<&'static str> {
    vec![
        "codex",
        "claude-code",
        "gemini",
        "opencode",
        "copilot",
        "pi",
        "cursor",
        "cli:claude-code",
        "cli:codex",
        "cli:gemini",
        "cli:opencode",
        "cli:copilot",
        "cli:pi",
    ]
}

fn import_descriptor_id(source: &str) -> Option<String> {
    Some(
        match source {
            "codex" | "cli:codex" => "cli:codex",
            "claude-code" | "cli:claude-code" => "cli:claude-code",
            "gemini" | "cli:gemini" => "cli:gemini",
            "opencode" | "cli:opencode" => "cli:opencode",
            "copilot" | "cli:copilot" => "cli:copilot",
            "pi" | "cli:pi" => "cli:pi",
            _ if source.starts_with("cli:") => source,
            _ => return None,
        }
        .to_string(),
    )
}

fn import_invalid(message: String) -> CliError {
    CliError::Core(DeadreckonError::InvalidInput(message))
}

fn print_import_candidates(
    resolved: &ResolvedImportSource,
    candidates: &[ImportCandidate],
    json_output: bool,
    kind: &str,
) -> Result<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&ImportJsonOutput {
                kind,
                source: &resolved.source,
                schema: &resolved.schema,
                roots: &resolved.roots,
                candidates,
                try_lines: vec![format!(
                    "deadreckon import {} --session <id-or-path>",
                    resolved.alias
                )],
            })?
        );
        return Ok(());
    }
    println!("source {}", resolved.source);
    println!("schema {}", resolved.schema);
    println!("roots:");
    for root in &resolved.roots {
        println!("  {}", root.display());
    }
    if candidates.is_empty() {
        println!("candidates 0");
        println!("try: deadreckon import {} --all --preview", resolved.alias);
        return Ok(());
    }
    println!("candidates {}", candidates.len());
    print!("{}", import_candidate_table(candidates));
    println!(
        "try: deadreckon import {} --session <id-or-path>",
        resolved.alias
    );
    Ok(())
}

fn print_import_selection(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
    json_output: bool,
) -> Result<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&ImportJsonOutput {
                kind: "import_preview",
                source: &resolved.source,
                schema: &resolved.schema,
                roots: &resolved.roots,
                candidates: selected,
                try_lines: vec![reimport_command(resolved, selected, mode)],
            })?
        );
        return Ok(());
    }
    println!("preview import");
    println!("source {}", resolved.source);
    println!("schema {}", resolved.schema);
    println!("mode {}", import_mode_label(mode));
    print!("{}", import_candidate_table(selected));
    println!("try: {}", reimport_command(resolved, selected, mode));
    Ok(())
}

fn import_mode_label(mode: ImportMode) -> &'static str {
    match mode {
        ImportMode::Session => "session",
        ImportMode::All => "all",
    }
}

fn ingest_storage_label(storage: &IngestStorage) -> &'static str {
    match storage {
        IngestStorage::Jsonl => "jsonl",
        IngestStorage::Json => "json",
        IngestStorage::JsonOrJsonl => "json-or-jsonl",
        IngestStorage::OpenCodeStorage => "opencode-storage",
    }
}

fn import_display_source(source: &str) -> &str {
    source.strip_prefix("cli:").unwrap_or(source)
}

fn import_source_paths(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for candidate in selected {
        for path in &candidate.paths {
            if resolved.storage == IngestStorage::OpenCodeStorage {
                for related in opencode_related_paths(path)? {
                    paths.insert(related);
                }
            } else {
                paths.insert(path.clone());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn opencode_related_paths(session_path: &Path) -> Result<Vec<PathBuf>> {
    let raw = fs::read_to_string(session_path)?;
    let session = serde_json::from_str::<Value>(&raw)?;
    let Some(session_id) = session.get("id").and_then(Value::as_str) else {
        return Ok(vec![session_path.to_path_buf()]);
    };
    let root = session_path
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            session_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let mut paths = BTreeSet::new();
    paths.insert(session_path.to_path_buf());
    for (message_path, message, _) in
        read_json_entries_sorted(&root.join("storage/message").join(session_id))
    {
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        paths.insert(message_path);
        if let Some(message_id) = message_id {
            for (part_path, _, _) in
                read_json_entries_sorted(&root.join("storage/part").join(message_id))
            {
                paths.insert(part_path);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn sha256_for_paths(paths: &[PathBuf]) -> Result<String> {
    let mut hasher = Sha256::new();
    for path in paths {
        hasher.update(fs::read(path)?);
        hasher.update([0xff]);
    }
    Ok(format!(
        "sha256:{}",
        hex_digest(hasher.finalize().as_slice())
    ))
}

fn sha256_for_str(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    format!("sha256:{}", hex_digest(digest.as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn import_run_id(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
) -> String {
    let identity = match mode {
        ImportMode::All => {
            let roots = resolved
                .roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join("|");
            format!("{}:all:{roots}", resolved.source)
        }
        ImportMode::Session => {
            let selected_identity = selected_session_identity(selected);
            format!("{}:session:{selected_identity}", resolved.source)
        }
    };
    format!("imported-{:016x}", stable_hash(&identity))
}

fn selected_session_identity(selected: &[ImportCandidate]) -> String {
    selected
        .iter()
        .flat_map(|candidate| candidate.paths.iter())
        .map(|path| {
            path.canonicalize()
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn read_import_manifest(run_root: &Path) -> Result<Option<ImportManifest>> {
    let path = run_root.join("import.json");
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn selected_session_arg(selected: &[ImportCandidate]) -> String {
    selected
        .first()
        .and_then(|candidate| candidate.session_id.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            selected
                .first()
                .and_then(|candidate| candidate.paths.first())
                .map(|path| path.display().to_string())
        })
        .unwrap_or_else(|| "<id-or-path>".to_string())
}

fn import_session_id(selected: &[ImportCandidate], mode: ImportMode) -> Option<String> {
    match mode {
        ImportMode::All => None,
        ImportMode::Session => selected
            .first()
            .and_then(|candidate| candidate.session_id.clone())
            .or_else(|| selected.first().map(|candidate| candidate.id.clone())),
    }
}

fn reimport_command(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
) -> String {
    match mode {
        ImportMode::All => format!("deadreckon import {} --all --replace", resolved.alias),
        ImportMode::Session => format!(
            "deadreckon import {} --session {} --replace",
            resolved.alias,
            shell_arg(&selected_session_arg(selected))
        ),
    }
}

fn shell_arg(raw: &str) -> String {
    if raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-' | '/' | '.' | '='))
    {
        return raw.to_string();
    }
    format!("'{}'", raw.replace('\'', "'\\''"))
}

fn provider_ingest_roots_for_working_dirs(
    ingest: &IngestDescriptor,
    home: &Path,
    working_dirs: &[String],
    include_all_roots: bool,
) -> Vec<PathBuf> {
    let env_value = ingest.env_var.as_deref().and_then(std::env::var_os);
    let base_roots = provider_ingest_base_roots(ingest, home, env_value.as_deref());
    let mut roots = Vec::new();
    match ingest.cwd_match {
        IngestCwdMatch::ClaudeProjectDir if !include_all_roots => {
            for base in &base_roots {
                for working_dir in working_dirs {
                    roots.push(base.join(claude_project_name_for_workdir(working_dir)));
                }
            }
        }
        _ => roots.extend(base_roots),
    }
    dedup_pathbufs(&mut roots);
    roots
}

fn provider_import_session_id(resolved: &ResolvedImportSource, path: &Path) -> Option<String> {
    let raw_id = match resolved.storage {
        IngestStorage::OpenCodeStorage | IngestStorage::Json => fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| session_id_from_value(&resolved.schema, &value)),
        IngestStorage::Jsonl | IngestStorage::JsonOrJsonl => {
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                fs::read_to_string(path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .and_then(|value| session_id_from_value(&resolved.schema, &value))
            } else {
                jsonl_session_id(path, &resolved.schema)
            }
        }
    }
    .or_else(|| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToString::to_string)
    })?;
    let prefix = resolved.id_prefix.as_deref().unwrap_or("");
    if prefix.is_empty() || raw_id.starts_with(prefix) {
        Some(raw_id)
    } else {
        Some(format!("{prefix}{raw_id}"))
    }
}

fn jsonl_session_id(path: &Path, schema: &str) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);
    for line in reader.lines().map_while(std::result::Result::ok).take(80) {
        let value = serde_json::from_str::<Value>(&line).ok()?;
        if let Some(id) = session_id_from_value(schema, &value) {
            return Some(id);
        }
    }
    None
}

fn session_id_from_value(schema: &str, value: &Value) -> Option<String> {
    for pointer in [
        "/session_id",
        "/sessionId",
        "/conversation_id",
        "/conversationId",
        "/id",
        "/payload/session_id",
        "/payload/sessionId",
        "/payload/id",
        "/data/session_id",
        "/data/sessionId",
        "/data/conversationId",
        "/message/session_id",
    ] {
        if let Some(id) = value.pointer(pointer).and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.to_string());
        }
    }
    if schema == "codex-cli"
        && value.get("type").and_then(Value::as_str) == Some("session_meta")
        && let Some(cwd) = value.pointer("/payload/cwd").and_then(Value::as_str)
    {
        return Some(format!("cwd-{:016x}", stable_hash(cwd)));
    }
    None
}

fn import_candidate_id(
    resolved: &ResolvedImportSource,
    session_id: Option<&str>,
    path: &Path,
) -> String {
    session_id.map(ToString::to_string).unwrap_or_else(|| {
        let prefix = resolved.id_prefix.as_deref().unwrap_or("");
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("session");
        format!("{prefix}{stem}")
    })
}

fn import_row_count_hint(resolved: &ResolvedImportSource, path: &Path) -> Option<usize> {
    if resolved.storage == IngestStorage::OpenCodeStorage {
        return opencode_related_paths(path).ok().map(|paths| paths.len());
    }
    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
        return fs::read_to_string(path)
            .ok()
            .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count());
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|value| import_json_value_row_count(&value))
}

fn import_json_value_row_count(value: &Value) -> usize {
    value
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(1)
}

fn import_candidate_matches(candidate: &ImportCandidate, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }
    let query_path = Path::new(query);
    let canonical_query = query_path.canonicalize().ok();
    if candidate.id == query
        || candidate.session_id.as_deref().is_some_and(|session_id| {
            session_id == query || strip_import_id_prefix(session_id) == query
        })
        || strip_import_id_prefix(&candidate.id) == query
    {
        return true;
    }
    candidate.paths.iter().any(|path| {
        path.display().to_string() == query
            || path.file_name().and_then(|name| name.to_str()) == Some(query)
            || path.file_stem().and_then(|stem| stem.to_str()) == Some(query)
            || canonical_query
                .as_ref()
                .is_some_and(|canonical| path.canonicalize().ok().as_ref() == Some(canonical))
    })
}

fn strip_import_id_prefix(id: &str) -> &str {
    id.split_once(':').map(|(_, rest)| rest).unwrap_or(id)
}

fn import_candidate_table(candidates: &[ImportCandidate]) -> String {
    let mut out = String::new();
    for candidate in candidates {
        let rows = candidate
            .row_count_hint
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string());
        let cwd = candidate
            .matched_cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let first_path = candidate
            .paths
            .first()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "  {}  updated={}  rows={}  cwd={}  path={}\n",
            candidate.id,
            candidate.updated_at.to_rfc3339(),
            rows,
            cwd,
            first_path
        ));
    }
    out
}

fn resolved_roots_lines(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return "  -".to_string();
    }
    roots
        .iter()
        .map(|root| format!("  {}", root.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_json_entries_sorted(dir: &Path) -> Vec<(PathBuf, Value, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut values = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let raw = fs::read_to_string(&path).ok()?;
            let value = serde_json::from_str::<Value>(&raw).ok()?;
            Some((path, value, raw))
        })
        .collect::<Vec<_>>();
    values.sort_by_key(|(_, value, _)| opencode_time_value(value));
    values
}

fn empty_import_parse_result(updated_at: DateTime<Utc>) -> ImportParseResult {
    ImportParseResult {
        rows_seen: 0,
        events: Vec::new(),
        source_started_at: None,
        source_updated_at: Some(updated_at),
    }
}

fn source_time_bounds(
    events: &[ImportedEvent],
    fallback_updated_at: DateTime<Utc>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let mut timestamps = events.iter().filter_map(|event| event.timestamp);
    let Some(first) = timestamps.next() else {
        return (None, Some(fallback_updated_at));
    };
    let mut min = first;
    let mut max = first;
    for timestamp in timestamps {
        min = min.min(timestamp);
        max = max.max(timestamp);
    }
    (Some(min), Some(max.max(fallback_updated_at)))
}

fn import_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    for pointer in [
        "/timestamp",
        "/created_at",
        "/createdAt",
        "/updated_at",
        "/updatedAt",
        "/message/created_at",
        "/message/timestamp",
    ] {
        if let Some(timestamp) = value.pointer(pointer)
            && let Some(parsed) = timestamp_from_value(timestamp)
        {
            return Some(parsed);
        }
    }
    for pointer in [
        "/time/created",
        "/time/start",
        "/time/end",
        "/time/updated",
        "/created",
        "/start",
        "/end",
    ] {
        if let Some(timestamp) = value
            .pointer(pointer)
            .and_then(Value::as_i64)
            .and_then(DateTime::<Utc>::from_timestamp_millis)
        {
            return Some(timestamp);
        }
    }
    None
}

fn timestamp_from_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(raw) = value.as_str() {
        return DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc));
    }
    value
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
}

fn import_usage_for_schema(schema: &str, value: &Value) -> Option<ImportedUsage> {
    match schema {
        "codex-cli" => value.get("payload").and_then(codex_usage),
        "gemini" => gemini_usage(value),
        _ => value
            .get("usage")
            .or_else(|| value.pointer("/message/usage"))
            .or_else(|| value.get("tokens"))
            .and_then(import_usage_from_value),
    }
}

fn codex_usage(payload: &Value) -> Option<ImportedUsage> {
    let usage = payload
        .pointer("/info/total_token_usage")
        .or_else(|| payload.get("usage"))
        .unwrap_or(payload);
    let mut imported = import_usage_from_value(usage)?;
    imported.context_window = payload
        .pointer("/info/model_context_window")
        .and_then(Value::as_u64)
        .or(imported.context_window);
    Some(imported)
}

fn gemini_usage(value: &Value) -> Option<ImportedUsage> {
    let tokens = value.get("tokens")?;
    let input = tokens.get("input").and_then(Value::as_u64);
    let output = tokens.get("output").and_then(Value::as_u64);
    let cache = tokens.get("cached").and_then(Value::as_u64);
    if input.is_none() && output.is_none() && cache.is_none() {
        return None;
    }
    Some(ImportedUsage {
        input_tokens: input,
        output_tokens: output,
        cache_tokens: cache,
        context_window: Some(1_000_000),
    })
}

fn import_usage_from_value(value: &Value) -> Option<ImportedUsage> {
    let input = number_field_any(
        value,
        &[
            "inputTokens",
            "input_tokens",
            "input",
            "prompt_tokens",
            "promptTokens",
        ],
    );
    let output = number_field_any(
        value,
        &[
            "outputTokens",
            "output_tokens",
            "output",
            "completion_tokens",
            "completionTokens",
        ],
    );
    let cache_read = number_field_any(
        value,
        &[
            "cacheReadTokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cacheRead",
            "cache.read",
        ],
    )
    .unwrap_or(0);
    let cache_write = number_field_any(
        value,
        &[
            "cacheCreationTokens",
            "cacheWriteTokens",
            "cache_creation_tokens",
            "cache_creation_input_tokens",
            "cache_write_tokens",
            "cacheCreation",
            "cacheWrite",
            "cache.write",
        ],
    )
    .unwrap_or(0);
    let cache = (cache_read + cache_write > 0).then_some(cache_read + cache_write);
    if input.is_none() && output.is_none() && cache.is_none() {
        return None;
    }
    Some(ImportedUsage {
        input_tokens: input,
        output_tokens: output,
        cache_tokens: cache,
        context_window: number_field_any(value, &["context_window", "contextWindow"]),
    })
}

fn import_source_event(value: &Value) -> String {
    value
        .get("type")
        .or_else(|| value.pointer("/payload/type"))
        .or_else(|| value.pointer("/message/role"))
        .and_then(Value::as_str)
        .unwrap_or("row")
        .to_string()
}

fn import_summary(schema: &str, value: &Value) -> String {
    for pointer in [
        "/content",
        "/message/content",
        "/payload/message",
        "/payload/output",
        "/data/content",
        "/data/text",
        "/data/result",
        "/text",
        "/summary",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return one_line(text, 180);
        }
    }
    if let Some(tool) = import_tool_name(value) {
        return format!("tool {}", provider_tool_label(&tool));
    }
    if let Some(usage) = import_usage_for_schema(schema, value) {
        return format!(
            "tokens input {} output {} cache {}",
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
            usage.cache_tokens.unwrap_or(0)
        );
    }
    one_line(&json_value_text(value), 180)
}

fn import_tool_name(value: &Value) -> Option<String> {
    for pointer in [
        "/tool_name",
        "/toolName",
        "/name",
        "/payload/name",
        "/data/name",
        "/tool",
    ] {
        if let Some(name) = value.pointer(pointer).and_then(Value::as_str)
            && !name.trim().is_empty()
        {
            return Some(name.to_string());
        }
    }
    None
}

fn import_tool_call_id(value: &Value) -> Option<String> {
    for pointer in [
        "/tool_call_id",
        "/toolCallId",
        "/call_id",
        "/callId",
        "/id",
        "/payload/call_id",
        "/payload/id",
        "/data/toolCallId",
        "/data/id",
    ] {
        if let Some(id) = value.pointer(pointer).and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.to_string());
        }
    }
    value
        .get("source_rowid")
        .and_then(Value::as_u64)
        .map(|row| format!("cursor-row-{row}"))
}

fn collect_import_paths(value: &Value) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    collect_import_paths_inner(value, None, &mut paths);
    paths.into_iter().collect()
}

fn collect_import_paths_inner(value: &Value, key: Option<&str>, paths: &mut BTreeSet<PathBuf>) {
    match value {
        Value::String(text) => {
            if key.is_some_and(import_path_key) && looks_like_import_path(text) {
                paths.insert(PathBuf::from(text));
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_import_paths_inner(item, key, paths);
            }
        }
        Value::Object(map) => {
            for (child_key, child_value) in map {
                collect_import_paths_inner(child_value, Some(child_key), paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn import_path_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "file"
            | "files"
            | "file_path"
            | "filePath"
            | "notebook_path"
            | "notebookPath"
            | "target_file"
            | "targetFile"
            | "source_file"
            | "sourceFile"
            | "destination"
            | "dest"
            | "uri"
            | "paths"
    )
}

fn looks_like_import_path(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 512
        && !trimmed.contains('\n')
        && !trimmed.starts_with("http://")
        && !trimmed.starts_with("https://")
        && (trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.contains('.')
            || trimmed.starts_with('~'))
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn latest_run(paths: &DeadreckonPaths, all: bool) -> Result<deadreckon_core::PipelineState> {
    let scope = if all { None } else { Some(current_scope()?) };
    let latest = list_runs(paths, scope.as_deref())?
        .into_iter()
        .next()
        .ok_or_else(|| {
            DeadreckonError::NotFound(match scope {
                Some(scope) => format!("latest run for current project ({scope})"),
                None => "latest run".to_string(),
            })
        })?;
    load_run(paths, &latest.run_id).map_err(CliError::from)
}

fn status_command(run_id: Option<String>, all: bool, plain: bool, json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match run_id {
        Some(run_id) => load_cli_run_with_scope(&paths, &run_id, all)?,
        None => latest_run(&paths, all)?,
    };
    if json_output {
        let next_action = next_action_label(&paths, &state);
        let status = run_status_label(state.status);
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "run_status",
                "id": &state.run_id,
                "status": status,
                "next_actions": [&next_action],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "state": state.state_path(),
                    "run_root": &state.run_root,
                    "working": &state.working_dir,
                    "artifact": &state.promoted_library_dir,
                },
                "run": &state,
                "status_label": status,
                "next_action": next_action,
            }))?
        );
        return Ok(());
    }
    print_status_report(&state, plain);
    print_lifecycle_hints(&state);
    Ok(())
}

fn print_status_report(state: &deadreckon_core::PipelineState, _plain: bool) {
    let paths = DeadreckonPaths::discover();
    let short = run_prefix(&state.run_id);
    let phase = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
        .unwrap_or_else(|| "-".to_string());
    let next_action = next_action_label(&paths, state);
    let stale = is_stale_executing(state);
    let supervised = supervised_pids(state);
    println!("deadreckon status");
    let run = format!("{short} ({})", state.run_id);
    let state_line = format!("{} -> {next_action}", run_status_label(state.status));
    let updated = format!("{} ago", relative_age(state.updated_at));
    let spend_summary = deadreckon_core::state::spend_summary(state).ok();
    let total_spend = spend_summary
        .as_ref()
        .map(|summary| summary.total_usd)
        .unwrap_or(state.total_spend_usd);
    let approximate_spend = spend_summary
        .as_ref()
        .is_some_and(|summary| summary.any_subscription_turn || summary.any_estimated_turn);
    let spend = format!(
        "{}${:.6} / {}",
        if approximate_spend { "~" } else { "" },
        total_spend,
        state
            .max_spend_usd
            .map(|cap| format!("${cap:.6}"))
            .unwrap_or_else(|| "uncapped".to_string())
    );
    let wall = format!(
        "{:.1}s / {}",
        state.total_wall_seconds,
        format_wall_cap(state.max_wall_seconds)
    );
    let goal = one_line(&state.goal, 110);
    let provider = state.provider.as_deref().unwrap_or("-");
    let mut rows = vec![
        ("run".to_string(), run),
        ("state".to_string(), state_line),
        ("phase".to_string(), phase),
        ("scope".to_string(), state.scope.clone()),
        ("updated".to_string(), updated),
        ("provider".to_string(), provider.to_string()),
        ("sandbox".to_string(), state.sandbox.clone()),
        ("spend".to_string(), spend),
        ("wall".to_string(), wall),
        ("goal".to_string(), goal),
    ];
    if let Some(sleep) = sleep_status_for_working(&state.working_dir) {
        rows.push(("sleep".to_string(), sleep));
    }
    let row_refs = rows
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    print_kv_block(&row_refs);
    print_run_locations(state);
    if let Ok(Some(line)) = chain_context_line_for_working(&state.working_dir) {
        println!("  chain:    {line}");
    }
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        println!("  mode:     {}", record.mode);
        if let Some(branch) = record.branch_name.as_deref() {
            println!("  branch:   {branch}");
        }
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("  worktree: {}", worktree.display());
        }
    }
    println!();
    println!("{}", ui_heading("run health"));
    println!("  next:     {next_action}");
    println!("  stale:    {}", if stale { "yes" } else { "no" });
    println!("  pids:     {}", supervised.len());
    if let Some(reason) = state.pause_reason.as_deref() {
        println!("  paused:   {}", one_line(reason, 100));
    }
    if let Some(reason) = state.failure_reason.as_deref() {
        println!("  failure:  {}", one_line(reason, 100));
    }
    println!("  gate:     {}", acceptance_status_line(state));
    let docs_status = docs_status_for_state(state);
    println!("  docs:     {docs_status}");
    if docs_status == DocsStatus::Failed
        && let Ok(Some(record)) = deadreckon_runtime::read_polish_record(state)
    {
        let detail = record.error.as_deref().unwrap_or(record.status.as_str());
        println!(
            "  docs:     polish failed ({}); fallback docs are still available",
            one_line(detail, 72)
        );
    }

    println!();
    println!("{}", ui_heading("library"));
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let manifest_present = library_dir.join("manifest.json").exists();
    let artifact_count = library_entries(&paths, Some(state.scope.clone()), false)
        .map(|entries| entries.len())
        .unwrap_or(0);
    println!("  scope artifacts: {artifact_count}");
    println!(
        "  current:  {}",
        if manifest_present {
            library_dir.display().to_string()
        } else {
            "not promoted".to_string()
        }
    );
    println!(
        "  exported: {}",
        materialized_count_label(materialized_marker_count(&library_dir))
    );

    println!();
    println!("{}", ui_heading("disk"));
    match free_kb(paths.home()) {
        Some(kb) => {
            let mb = kb / 1024;
            println!("  home:     {} MB free in {}", mb, paths.home().display());
            if mb > 10_240 {
                println!(
                    "  tip:      {}",
                    ui_command("deadreckon cleanup --completed")
                );
            } else {
                println!("  warning:  low disk; clean old worktrees and artifacts soon");
            }
        }
        None => println!("  home:     unavailable; run deadreckon doctor"),
    }
}

fn sleep_status_for_working(working_dir: &Path) -> Option<String> {
    let path = sleep::metadata_path(working_dir);
    let raw = fs::read_to_string(path).ok()?;
    let metadata: sleep::SleepMetadata = serde_json::from_str(&raw).ok()?;
    let mode = match metadata.mode {
        sleep::SleepMode::Caffeinate => "caffeinate",
        sleep::SleepMode::SystemdInhibit => "systemd-inhibit",
        sleep::SleepMode::None => "none",
        sleep::SleepMode::Unsupported => "unsupported",
    };
    Some(match (metadata.pid, metadata.skip_reason) {
        (Some(pid), _) if deadreckon_core::pid_is_alive(pid) => format!("{mode} pid={pid}"),
        (Some(pid), _) => format!("{mode} pid={pid} stale"),
        (None, Some(reason)) => format!("{mode} ({})", sleep::skip_reason_label(reason)),
        (None, None) => mode.to_string(),
    })
}

fn print_run_summary(state: &deadreckon_core::PipelineState) {
    println!("run {} ({})", run_prefix(&state.run_id), state.run_id);
    let status = run_status_label(state.status);
    let spend_summary = deadreckon_core::state::spend_summary(state).ok();
    let total_spend = spend_summary
        .as_ref()
        .map(|summary| summary.total_usd)
        .unwrap_or(state.total_spend_usd);
    let approximate_spend = spend_summary
        .as_ref()
        .is_some_and(|summary| summary.any_subscription_turn || summary.any_estimated_turn);
    let spend = format!(
        "{}${:.6}",
        if approximate_spend { "~" } else { "" },
        total_spend
    );
    let mut items = vec![
        ("status".to_string(), status.to_string()),
        ("goal".to_string(), state.goal.clone()),
        ("spend".to_string(), spend),
    ];
    if let Some(phase) = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
    {
        items.push(("phase".to_string(), phase));
    }
    if let Some(sleep) = sleep_status_for_working(&state.working_dir) {
        items.push(("sleep".to_string(), sleep));
    }
    if let Ok(Some(marker)) = read_parent_marker(&state.working_dir) {
        let label = match marker.kind.as_str() {
            "extended" => format!("extended from {}", run_prefix(&marker.parent_run_id)),
            "materialized" => format!("exported from {}", run_prefix(&marker.parent_run_id)),
            other => format!("{other} from {}", run_prefix(&marker.parent_run_id)),
        };
        items.push(("lineage".to_string(), label));
    }
    let item_refs = items
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    print_kv_block(&item_refs);
    print_run_locations(state);
    print_chain_context_for_working(&state.working_dir);
}

fn print_chain_context_for_working(working_dir: &Path) {
    if let Some(line) = chain_context_line_for_working(working_dir).ok().flatten() {
        println!("{line}");
        if let Ok(Some(marker)) = read_chain_step_marker(working_dir) {
            println!(
                "[c] Chain deadreckon chain attach {}",
                chain_prefix(&marker.chain_id)
            );
        }
    }
}

fn print_run_locations(state: &deadreckon_core::PipelineState) {
    let state_path = state.state_path().display().to_string();
    let launch_dir = state.cwd.display().to_string();
    if let Some(library_dir) = state.promoted_library_dir.as_ref() {
        let artifact = library_dir.display().to_string();
        let items = [
            ("state", state_path.as_str()),
            ("launch-dir", launch_dir.as_str()),
            ("artifact", artifact.as_str()),
        ];
        print_kv_block(&items);
        println!("note: launch-dir is unchanged; completed output lives in artifact");
    } else {
        let working = state.working_dir.display().to_string();
        let items = [
            ("state", state_path.as_str()),
            ("launch-dir", launch_dir.as_str()),
            ("working", working.as_str()),
        ];
        print_kv_block(&items);
    }
}

fn chain_context_line_for_working(working_dir: &Path) -> Result<Option<String>> {
    let Some(marker) = read_chain_step_marker(working_dir)? else {
        return Ok(None);
    };
    let paths = DeadreckonPaths::discover();
    let chain = load_chain(&paths, &marker.chain_id).ok();
    let total_steps = chain.as_ref().map(|chain| chain.steps.len()).unwrap_or(0);
    let policy = chain
        .as_ref()
        .map(|chain| branch_policy_label(chain.branch_policy))
        .unwrap_or("unknown");
    let apply = chain
        .as_ref()
        .map(|chain| apply_mode_label(chain.apply_mode))
        .unwrap_or("unknown");
    let prior = marker
        .prior_applied_sha
        .as_deref()
        .map(short_sha)
        .unwrap_or_else(|| "none".to_string());
    Ok(Some(format!(
        "chain {} · step {}/{} · policy: {} | apply={} · prev: {}",
        chain_prefix(&marker.chain_id),
        marker.step_index + 1,
        total_steps,
        policy,
        apply,
        prior
    )))
}

fn print_run_started(
    state: &deadreckon_core::PipelineState,
    route: Option<&ProviderRouteInfo>,
    provider_source: &str,
    doc_provider: Option<&str>,
    doc_provider_source: &str,
) {
    print_run_started_with_label(
        "started run",
        state,
        route,
        provider_source,
        doc_provider,
        doc_provider_source,
    );
}

fn print_run_started_with_label(
    label: &str,
    state: &deadreckon_core::PipelineState,
    route: Option<&ProviderRouteInfo>,
    provider_source: &str,
    doc_provider: Option<&str>,
    doc_provider_source: &str,
) {
    println!(
        "{} {}",
        ui_ok(label),
        ui_id(format!("{} ({})", run_prefix(&state.run_id), state.run_id))
    );
    let mut rows = Vec::new();
    if let Some(route) = route {
        rows.push((
            "provider".to_string(),
            format!("{} ({provider_source})", route.name),
        ));
        rows.push(("model".to_string(), route.model.clone()));
    } else if let Some(provider) = state.provider.as_deref() {
        rows.push((
            "provider".to_string(),
            format!("{provider} ({provider_source})"),
        ));
    }
    rows.push((
        "docs".to_string(),
        format!(
            "{} ({doc_provider_source})",
            doc_provider.unwrap_or("templated only")
        ),
    ));
    rows.push((
        "notes".to_string(),
        state
            .working_dir
            .join(deadreckon_core::IMPLEMENTATION_NOTES_HTML)
            .display()
            .to_string(),
    ));
    rows.push((
        "state".to_string(),
        state.state_path().display().to_string(),
    ));
    rows.push((
        "attach".to_string(),
        format!("deadreckon attach {}", run_prefix(&state.run_id)),
    ));
    let item_refs = rows
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    print_kv_block(&item_refs);
    let _ = io::stdout().flush();
}

fn print_lifecycle_hints(state: &deadreckon_core::PipelineState) {
    if let Ok(record) = read_codebase_record(&state.working_dir)
        && record.mode == CodebaseMode::Worktree
    {
        println!("{}", ui_heading("next actions:"));
        println!(
            "  finish:  {}",
            ui_command(format!(
                "deadreckon finish {} --autostash --cleanup",
                run_prefix(&state.run_id)
            ))
        );
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("  inspect: cd {} && git status", worktree.display());
        }
        println!(
            "  apply:   {}",
            ui_command(format!("deadreckon apply {}", run_prefix(&state.run_id)))
        );
        println!(
            "  cleanup: {}",
            ui_command(format!(
                "deadreckon apply {} --autostash --cleanup",
                run_prefix(&state.run_id)
            ))
        );
        println!(
            "  cleanup: {}",
            ui_command(format!("deadreckon cleanup {}", run_prefix(&state.run_id)))
        );
        println!(
            "  docs:    {}",
            ui_command(format!(
                "deadreckon doc {} --kind decisions",
                run_prefix(&state.run_id)
            ))
        );
        return;
    }
    let task_prefix = state.task_key.chars().take(24).collect::<String>();
    println!("{}", ui_heading("next actions:"));
    println!(
        "  finish: {}",
        ui_command(format!(
            "deadreckon finish {} --dest ./{}",
            run_prefix(&state.run_id),
            task_prefix
        ))
    );
    println!(
        "  export: {}",
        ui_command(format!(
            "deadreckon export {} --dest ./{}",
            run_prefix(&state.run_id),
            task_prefix
        ))
    );
    println!(
        "  extend: {}",
        ui_command(format!(
            "deadreckon extend {} '<your follow-up goal>'",
            run_prefix(&state.run_id)
        ))
    );
    println!(
        "  show:   {}",
        ui_command(format!("deadreckon show {}", run_prefix(&state.run_id)))
    );
    println!(
        "  docs:   {}",
        ui_command(format!(
            "deadreckon doc {} --kind decisions",
            run_prefix(&state.run_id)
        ))
    );
}

async fn complete_run_actions(
    state: &deadreckon_core::PipelineState,
    allow_prompt: bool,
) -> Result<()> {
    print_lifecycle_hints(state);
    if allow_prompt && io::stdin().is_terminal() && io::stdout().is_terminal() {
        completion_action_loop(state).await?;
    }
    Ok(())
}

fn completion_hints_enabled(no_hints: bool) -> bool {
    if no_hints {
        return false;
    }
    !std::env::var("DEADRECKON_HINTS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

async fn completion_action_loop(state: &deadreckon_core::PipelineState) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    loop {
        let prompt_text = if is_worktree_run(state) {
            "completed action [a apply, b abandon, d docs, s show, q quit]: "
        } else {
            "completed action [m export, e extend, d docs, s show, q quit]: "
        };
        let answer = prompt::open(prompt_text, None)?;
        match completion_action_from_input(&answer) {
            Some(CompletionAction::Materialize) => prompt_materialize_action(&paths, state)?,
            Some(CompletionAction::Extend) => prompt_extend_action(state).await?,
            Some(CompletionAction::Apply) => apply_command(
                state.run_id.clone(),
                "squash".to_string(),
                None,
                false,
                false,
                false,
                None,
                false,
            )?,
            Some(CompletionAction::Abandon) => abandon_command(state.run_id.clone(), false, false)?,
            Some(CompletionAction::Docs) => {
                Box::pin(doc_command(DocCommandArgs {
                    run_id: state.run_id.clone(),
                    kind: DocKind::Narrative,
                    export: None,
                    polish: false,
                    no_confirm: true,
                    force: false,
                    doc_skill: None,
                    doc_provider: None,
                    budget_cap: None,
                }))
                .await?
            }
            Some(CompletionAction::Show) => {
                show_command(&state.run_id, None, false, false, false, false, None)?
            }
            Some(CompletionAction::Quit) | None => break,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionAction {
    Materialize,
    Extend,
    Apply,
    Abandon,
    Docs,
    Show,
    Quit,
}

impl CompletionAction {
    fn label(self) -> &'static str {
        match self {
            Self::Materialize => "export",
            Self::Extend => "extend",
            Self::Apply => "apply",
            Self::Abandon => "abandon",
            Self::Docs => "docs",
            Self::Show => "show",
            Self::Quit => "quit",
        }
    }

    fn success_detail(self) -> &'static str {
        match self {
            Self::Materialize => {
                "the completed artifact was exported; destination was printed above"
            }
            Self::Extend => {
                "the follow-up run was created or completed; check list/status for its run id"
            }
            Self::Apply => {
                "changes were applied to the source branch; cleanup may have removed the worktree"
            }
            Self::Abandon => "temporary worktree and branch cleanup finished",
            Self::Docs => "documentation was printed in the terminal",
            Self::Show => "run details were printed in the terminal",
            Self::Quit => "no action was taken",
        }
    }
}

fn completion_action_from_input(input: &str) -> Option<CompletionAction> {
    match input.trim().to_ascii_lowercase().as_str() {
        "m" | "export" | "materialize" => Some(CompletionAction::Materialize),
        "e" | "extend" => Some(CompletionAction::Extend),
        "a" | "apply" => Some(CompletionAction::Apply),
        "b" | "abandon" => Some(CompletionAction::Abandon),
        "d" | "doc" | "docs" => Some(CompletionAction::Docs),
        "s" | "show" => Some(CompletionAction::Show),
        "q" | "quit" | "" => Some(CompletionAction::Quit),
        _ => None,
    }
}

fn prompt_materialize_action(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<()> {
    let default_dest = default_materialize_dest(state);
    let answer = prompt::open(&format!("export dest [{}]: ", default_dest.display()), None)?;
    let dest = if answer.trim().is_empty() {
        default_dest
    } else {
        PathBuf::from(answer.trim())
    };
    let dest = absolute_dest(dest)?;
    let force = if dest.exists() && !path_is_empty_dir(&dest)? {
        prompt::confirm("destination is not empty; overwrite?", false)?
    } else {
        false
    };
    let materialized = materialize_completed_run(paths, state, Some(dest), force, false)?;
    print_materialized(&materialized);
    Ok(())
}

async fn prompt_extend_action(state: &deadreckon_core::PipelineState) -> Result<()> {
    let goal = prompt::open("follow-up goal: ", None)?;
    if goal.trim().is_empty() {
        println!("extend skipped; follow-up goal was empty");
        return Ok(());
    }
    let dest = prompt::open("extension working dest [runstate working dir]: ", None)?;
    let dest = if dest.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(dest.trim()))
    };
    extend_command(ExtendCommandArgs {
        parent_run_id: state.run_id.clone(),
        new_goal: goal,
        dest,
        max_context_turns: None,
        no_context: false,
        max_spend: state.max_spend_usd,
        max_wall_seconds: state.max_wall_seconds,
        provider: state.provider.clone(),
        model: None,
        sandbox: Some(state.sandbox.clone()),
        no_docs: false,
        doc_skill: None,
        post_actions: false,
    })
    .await
}

fn is_worktree_run(state: &deadreckon_core::PipelineState) -> bool {
    read_codebase_record(&state.working_dir)
        .map(|record| record.mode == CodebaseMode::Worktree)
        .unwrap_or(false)
}

async fn attach_plan_tui(paths: &DeadreckonPaths, plan_id: &str, show_hints: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0_usize;
    let mut plan = load_plan(paths, plan_id)?;
    let mut messages: Vec<PlanMessage>;
    let mut plan_events = Vec::<PlanEvent>::new();
    let mut feed_events = Vec::<PlanFeedEvent>::new();
    let mut feed = PlanEventBus::file_tail(paths.clone(), plan_id.to_string());

    let result = loop {
        for event in feed.refresh(Duration::ZERO).await {
            match event {
                PlanFeedEvent::Plan { event } => {
                    plan_events.push(event.clone());
                    feed_events.push(PlanFeedEvent::Plan { event });
                }
                PlanFeedEvent::Snapshot { plan: snapshot } => {
                    plan = (*snapshot).clone();
                    feed_events.push(PlanFeedEvent::Snapshot { plan: snapshot });
                }
                other => feed_events.push(other),
            }
        }
        if plan_events.len() > 1_000 {
            let drain = plan_events.len().saturating_sub(1_000);
            plan_events.drain(0..drain);
        }
        if feed_events.len() > 1_000 {
            let drain = feed_events.len().saturating_sub(1_000);
            feed_events.drain(0..drain);
        }
        messages = read_plan_messages(paths, plan_id).unwrap_or_default();
        if selected >= plan.tasks.len() {
            selected = plan.tasks.len().saturating_sub(1);
        }
        terminal.draw(|frame| {
            render_plan_attach(
                frame,
                paths,
                &plan,
                &PlanAttachRenderState {
                    messages: &messages,
                    plan_events: &plan_events,
                    feed_events: &feed_events,
                    selected,
                    show_hints,
                },
            )
        })?;
        if event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key)
                    if matches!(
                        key.code,
                        KeyCode::Right | KeyCode::Down | KeyCode::Tab | KeyCode::Char('j')
                    ) =>
                {
                    selected = (selected + 1).min(plan.tasks.len().saturating_sub(1));
                }
                Event::Key(key)
                    if matches!(key.code, KeyCode::Left | KeyCode::Up | KeyCode::Char('k')) =>
                {
                    selected = selected.saturating_sub(1);
                }
                Event::Key(key) if key.code == KeyCode::Enter => {
                    if let Some(run_id) = plan
                        .tasks
                        .get(selected)
                        .and_then(|task| task.child_run_id.as_deref())
                    {
                        if load_run(paths, run_id).is_err() {
                            continue;
                        }
                        let parent_plan = plan.tasks.get(selected).map(|task| AttachParentPlan {
                            plan_id: plan.plan_id.clone(),
                            task_id: task.task_id.clone(),
                        });
                        suspend_tui(&mut terminal)?;
                        let child_result =
                            attach_tui_with_parent(paths, run_id, show_hints, parent_plan).await;
                        if let Err(err) = &child_result {
                            print_error(err);
                            let _ = prompt::open("press Enter to return to plan attach...", None);
                        }
                        resume_tui(&mut terminal)?;
                    }
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

struct PlanAttachRenderState<'a> {
    messages: &'a [PlanMessage],
    plan_events: &'a [PlanEvent],
    feed_events: &'a [PlanFeedEvent],
    selected: usize,
    show_hints: bool,
}

fn render_plan_attach(
    frame: &mut ratatui::Frame<'_>,
    paths: &DeadreckonPaths,
    plan: &Plan,
    state: &PlanAttachRenderState<'_>,
) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(10),
            Constraint::Length(7),
            Constraint::Length(2),
        ])
        .split(area);
    let counts = plan_task_counts(plan);
    let header = vec![
        Line::from(vec![
            Span::styled("plan ", Style::default().fg(Color::Cyan)),
            Span::styled(
                run_prefix(&plan.plan_id),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  status {}  mode {:?}  children {}/{}/{}",
                plan_status_label(plan.status),
                plan.mode,
                counts.0,
                counts.1,
                plan.tasks.len()
            )),
        ]),
        Line::from(one_line(
            &plan.root_goal,
            area.width.saturating_sub(4) as usize,
        )),
        Line::from(plan_provider_summary(plan)),
        Line::from(orchestration_parallelism_lines(plan).join("  ")),
        Line::from(format!(
            "capabilities network={:?} deploy={} install={}{}",
            plan.capability_preview.network,
            plan.capability_preview.deploy,
            plan.capability_preview.global_install,
            plan_final_gate_line(paths, plan)
                .map(|line| format!("  final {line}"))
                .unwrap_or_default()
        )),
    ];
    frame.render_widget(
        Paragraph::new(header)
            .block(
                Block::default()
                    .title("deadreckon plan")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        vertical[0],
    );

    let task_area = vertical[1];
    let panes = plan_task_pane_layout(task_area, plan.tasks.len());
    for (index, task) in plan.tasks.iter().enumerate() {
        let Some(rect) = panes.get(index).copied() else {
            continue;
        };
        let is_selected = index == state.selected;
        let title = format!(
            "{} {} {}",
            if is_selected { "*" } else { " " },
            task.task_id,
            task_status_label(task.status)
        );
        let lines =
            plan_task_detail_lines(paths, plan, task, rect.width.saturating_sub(4) as usize)
                .into_iter()
                .enumerate()
                .map(|(line_index, line)| {
                    if line_index == 0 {
                        Line::from(vec![
                            Span::styled(
                                line.split_once("  ")
                                    .map(|(role, _)| role.to_string())
                                    .unwrap_or_else(|| line.clone()),
                                Style::default().fg(Color::Magenta),
                            ),
                            Span::raw(
                                line.split_once("  ")
                                    .map(|(_, rest)| format!("  {rest}"))
                                    .unwrap_or_default(),
                            ),
                        ])
                    } else {
                        Line::from(line)
                    }
                })
                .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(if is_selected {
                            Style::default().fg(ui::TUI_PALETTE.border_focused)
                        } else {
                            Style::default().fg(ui::TUI_PALETTE.border_idle)
                        }),
                )
                .wrap(Wrap { trim: true }),
            rect,
        );
    }

    let activity = plan_activity_lines(
        state.plan_events,
        state.messages,
        state.feed_events,
        vertical[2].height.saturating_sub(2) as usize,
    );
    let activity_title = if !state.feed_events.is_empty() {
        "plan feed"
    } else if state.plan_events.is_empty() {
        "coordinator messages"
    } else {
        "plan events"
    };
    frame.render_widget(
        List::new(activity).block(Block::default().title(activity_title).borders(Borders::ALL)),
        vertical[2],
    );
    let footer = plan_attach_footer(paths, plan, state.selected, state.show_hints);
    frame.render_widget(Paragraph::new(footer), vertical[3]);
}

fn plan_attach_footer(
    paths: &DeadreckonPaths,
    plan: &Plan,
    selected: usize,
    show_hints: bool,
) -> String {
    let mut footer =
        "q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  Enter child run  |  b/Backspace back from child"
            .to_string();
    if let Some(task) = plan.tasks.get(selected) {
        match task.child_run_id.as_deref() {
            None => {
                footer = format!(
                    "q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  Enter waits for child run  |  try: deadreckon fork {}",
                    run_prefix(&plan.plan_id)
                );
            }
            Some(run_id) if load_run(paths, run_id).is_err() => {
                footer =
                    "q/Esc/Ctrl-D detach  |  arrows/Tab focus child  |  child detail unavailable  |  try: deadreckon list --all"
                        .to_string();
            }
            Some(_) => {}
        }
    }
    if show_hints && !footer.contains("try:") {
        footer.push_str("  |  merge after fork");
    }
    footer
}

fn plan_task_counts(plan: &Plan) -> (usize, usize) {
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let running = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Running)
        .count();
    (completed, running)
}

fn plan_provider_summary(plan: &Plan) -> String {
    match plan.mode {
        PlanMode::FullPlan => format!(
            "planner {}  default child {}",
            plan.providers.planner.as_deref().unwrap_or("-"),
            plan.providers.default_child.as_deref().unwrap_or("-")
        ),
        PlanMode::Review => format!(
            "coder {}  reviewer {}",
            plan.providers.coder.as_deref().unwrap_or("-"),
            plan.providers.reviewer.as_deref().unwrap_or("-")
        ),
    }
}

fn plan_repair_label(plan: &Plan, no_repair: bool) -> String {
    if no_repair {
        return "disabled (--no-repair)".to_string();
    }
    let provider = plan
        .providers
        .planner
        .as_deref()
        .or(plan.providers.default_child.as_deref())
        .unwrap_or("config default");
    format!("automatic via {provider}")
}

fn plan_task_pane_layout(
    area: ratatui::layout::Rect,
    task_count: usize,
) -> Vec<ratatui::layout::Rect> {
    if task_count == 0 {
        return Vec::new();
    }
    let rows = if task_count <= 3 { 1 } else { 2 };
    let row_constraints =
        std::iter::repeat_n(Constraint::Ratio(1, rows as u32), rows).collect::<Vec<_>>();
    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);
    let mut rects = Vec::new();
    for row in 0..rows {
        let remaining = task_count.saturating_sub(rects.len());
        let columns = remaining.min(if rows == 1 { task_count } else { 3 }).max(1);
        let col_constraints =
            std::iter::repeat_n(Constraint::Ratio(1, columns as u32), columns).collect::<Vec<_>>();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(row_chunks[row]);
        for col in cols.iter().copied().take(remaining) {
            rects.push(col);
            if rects.len() == task_count {
                break;
            }
        }
    }
    rects
}

fn plan_activity_lines(
    plan_events: &[PlanEvent],
    messages: &[PlanMessage],
    feed_events: &[PlanFeedEvent],
    max: usize,
) -> Vec<ListItem<'static>> {
    let mut lines = if !feed_events.is_empty() {
        feed_events
            .iter()
            .rev()
            .take(max.max(1))
            .map(|event| ListItem::new(Line::from(plan_feed_event_line(event))))
            .collect::<Vec<_>>()
    } else if plan_events.is_empty() {
        messages
            .iter()
            .rev()
            .take(max.max(1))
            .map(|message| {
                ListItem::new(Line::from(format!(
                    "{} -> {} {:?}: {}",
                    message.from, message.to, message.kind, message.summary
                )))
            })
            .collect::<Vec<_>>()
    } else {
        plan_events
            .iter()
            .rev()
            .take(max.max(1))
            .map(|event| ListItem::new(Line::from(plan_event_line(event))))
            .collect::<Vec<_>>()
    };
    if lines.is_empty() {
        lines.push(ListItem::new(Line::from("no plan activity yet")));
    }
    lines
}

fn plan_feed_event_line(event: &PlanFeedEvent) -> String {
    match event {
        PlanFeedEvent::Plan { event } => plan_event_line(event),
        PlanFeedEvent::ChildRun {
            task_id,
            run_id,
            event,
        } => format!(
            "{} child {} run {} {}",
            event.timestamp.format("%H:%M:%S"),
            task_id,
            run_prefix(run_id),
            event_line(event, false)
        ),
        PlanFeedEvent::RepairRun { run_id, event } => format!(
            "{} repair run {} {}",
            event.timestamp.format("%H:%M:%S"),
            run_prefix(run_id),
            event_line(event, false)
        ),
        PlanFeedEvent::Snapshot { plan } => format!(
            "{} snapshot status {} children {}",
            Utc::now().format("%H:%M:%S"),
            plan_status_label(plan.status),
            plan.tasks.len()
        ),
        PlanFeedEvent::Warning { message } => {
            format!("{} warning {message}", Utc::now().format("%H:%M:%S"))
        }
    }
}

fn plan_event_line(event: &PlanEvent) -> String {
    format!(
        "{} {}",
        event.timestamp.format("%H:%M:%S"),
        plan_event_summary(&event.event)
    )
}

fn plan_event_summary(event: &PlanEventKind) -> String {
    match event {
        PlanEventKind::PlanCreated { mode, task_count } => {
            format!(
                "plan created mode {} tasks {task_count}",
                plan_mode_label(*mode)
            )
        }
        PlanEventKind::PlanStarted => "plan started".to_string(),
        PlanEventKind::TaskReady { task_id, .. } => format!("{task_id} ready"),
        PlanEventKind::TaskStarted { task_id, .. } => format!("{task_id} started"),
        PlanEventKind::TaskRunDiscovered {
            task_id,
            run_id,
            pid,
            ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            let pid = pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} run discovered run {run} pid {pid}")
        }
        PlanEventKind::TaskCompleted {
            task_id,
            run_id,
            status,
            ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} completed run {run} status {status}")
        }
        PlanEventKind::TaskBlocked {
            task_id, reason, ..
        } => {
            format!("{task_id} blocked: {reason}")
        }
        PlanEventKind::TaskFailed {
            task_id, reason, ..
        } => {
            format!("{task_id} failed: {reason}")
        }
        PlanEventKind::TaskKilled {
            task_id, run_id, ..
        } => {
            let run = run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string());
            format!("{task_id} killed run {run}")
        }
        PlanEventKind::MergeStarted => "merge started".to_string(),
        PlanEventKind::MergeConflict { conflict_count } => {
            format!("merge conflict count {conflict_count}")
        }
        PlanEventKind::MergeRepairPlanned {
            conflict_count,
            provider,
        } => format!(
            "merge repair planned for {conflict_count} conflict(s) with {}",
            provider.as_deref().unwrap_or("no provider")
        ),
        PlanEventKind::MergeRepairStarted { mode } => {
            format!("merge repair started mode {mode}")
        }
        PlanEventKind::MergeRepairRunDiscovered { run_id, pid } => {
            format!(
                "merge repair run {} pid {}",
                run_prefix(run_id),
                pid.map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        }
        PlanEventKind::MergeRepaired {
            strategy,
            repair_run_id,
        } => format!(
            "merge repaired via {strategy}{}",
            repair_run_id
                .as_deref()
                .map(|run_id| format!(" run {}", run_prefix(run_id)))
                .unwrap_or_default()
        ),
        PlanEventKind::MergeRepairFailed { reason } => {
            format!("merge repair failed: {reason}")
        }
        PlanEventKind::MergeCompleted { merged_run_id } => {
            format!("merge completed run {}", run_prefix(merged_run_id))
        }
        PlanEventKind::PlanCompleted => "plan completed".to_string(),
        PlanEventKind::PlanFailed { reason } => format!("plan failed: {reason}"),
        PlanEventKind::PlanKilled => "plan killed".to_string(),
    }
}

fn plan_final_gate_line(paths: &DeadreckonPaths, plan: &Plan) -> Option<String> {
    let run_id = plan.merged_run_id.as_deref()?;
    let state = load_run(paths, run_id).ok()?;
    Some(acceptance_status_line(&state))
}

fn plan_task_detail_lines(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    width: usize,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  provider {}",
            format!("{:?}", task.role).to_ascii_lowercase(),
            task.provider.as_deref().unwrap_or("-")
        ),
        one_line(&task.subject, width),
        format!(
            "status {}  run {}",
            task_status_label(task.status),
            task.child_run_id
                .as_deref()
                .map(run_prefix)
                .unwrap_or_else(|| "-".to_string())
        ),
        format!(
            "deps {}",
            if task.depends_on.is_empty() {
                "ready".to_string()
            } else {
                task.depends_on.join(",")
            }
        ),
    ];
    if let Some(run_id) = task.child_run_id.as_deref()
        && let Ok(state) = load_run(paths, run_id)
    {
        lines.push(format!("turn {}  run-status {}", state.turn, state.status));
        lines.extend(plan_child_accounting_lines(&state));
        if let Some(trace) = latest_trace_line(&state) {
            lines.push(trace);
        }
        lines.push(format!("gate {}", acceptance_status_line(&state)));
    } else if let Some(run_id) = task.child_run_id.as_deref() {
        lines.push(format!("child detail unavailable {}", run_prefix(run_id)));
    }
    if let Some(summary) = task.summary_path.as_ref() {
        lines.push(format!(
            "summary {}",
            paths.plan_dir(&plan.plan_id).join(summary).display()
        ));
    }
    lines
}

fn plan_child_accounting_lines(state: &deadreckon_core::PipelineState) -> Vec<String> {
    let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl")).unwrap_or_default();
    if spend.is_empty() {
        return vec![format!(
            "spend ${:.6}  context waiting",
            state.total_spend_usd
        )];
    }
    let total_cost = spend
        .last()
        .map(|record| record.total_cost_usd)
        .unwrap_or(state.total_spend_usd);
    let total_tokens = spend
        .iter()
        .map(|record| record.input_tokens + record.output_tokens)
        .sum::<u64>();
    let wall = spend
        .last()
        .and_then(|record| record.wall_time_seconds)
        .map(|seconds| format!("  wall {seconds:.0}s"))
        .unwrap_or_default();
    if spend.iter().any(|record| record.subscription)
        || state
            .provider
            .as_deref()
            .is_some_and(|provider| provider.starts_with("cli:"))
    {
        vec![format!("tokens {}{wall}", format_count(total_tokens))]
    } else {
        vec![format!(
            "spend ${total_cost:.6}  tokens {}{wall}",
            format_count(total_tokens)
        )]
    }
}

fn latest_trace_line(state: &deadreckon_core::PipelineState) -> Option<String> {
    let trace = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))
        .unwrap_or_default()
        .into_iter()
        .last()?;
    Some(format!(
        "latest turn {} {} {}",
        trace.turn,
        trace.event,
        one_line(&trace.detail.to_string(), 80)
    ))
}

async fn attach_tui(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
) -> Result<()> {
    attach_tui_with_parent(paths, run_id, show_completion_actions, None).await
}

async fn attach_tui_with_parent(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
    parent_plan: Option<AttachParentPlan>,
) -> Result<()> {
    let initial_state = load_run(paths, run_id)?;
    let mut event_feed =
        tui_events::TuiEventFeed::file_tail(initial_state.run_root.join(RUN_EVENTS_JSONL));
    let mut events = event_feed.refresh(std::time::Duration::ZERO).await?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = AttachTuiState {
        show_completion_actions,
        parent_plan,
        ..AttachTuiState::default()
    };

    let result = loop {
        let state = load_run(paths, run_id)?;
        let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl"))?;
        let traces = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
        events.extend(event_feed.refresh(std::time::Duration::ZERO).await?);
        let live = collect_attach_live(&state);
        let terminal_size = terminal.size()?;
        let terminal_area =
            ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height);
        let panel_layout = attach_panel_layout(terminal_area);
        let panel_counts = attach_panel_counts(&state, &spend, &traces, &events, &live, &tui_state);
        tui_state.clamp(panel_counts, panel_layout.rows);
        terminal.draw(|frame| {
            render_attach(frame, &state, &spend, &traces, &events, &live, &tui_state)
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key)
                    if tui_state.parent_plan.is_some() && attach_should_return_to_plan(key) =>
                {
                    break Ok(());
                }
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key)
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.is_empty()
                        && read_chain_step_marker(&state.working_dir)?.is_some() =>
                {
                    if let Some(marker) = read_chain_step_marker(&state.working_dir)? {
                        suspend_tui(&mut terminal)?;
                        let action = chain_attach_command(paths, &marker.chain_id, false);
                        if let Err(err) = &action {
                            print_error(err);
                        }
                        resume_tui(&mut terminal)?;
                    }
                }
                Event::Key(key)
                    if tui_state.show_completion_actions
                        && state.status == RunStatus::Completed =>
                {
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        tui_state.toggle_docs();
                    } else if let Some(notice) =
                        handle_tui_completion_key(&mut terminal, paths, &state, key).await?
                    {
                        tui_state.record_post_action(notice);
                    } else {
                        tui_state.handle_key(key, panel_counts, panel_layout.rows);
                    }
                }
                Event::Key(key) => tui_state.handle_key(key, panel_counts, panel_layout.rows),
                Event::Mouse(mouse) => {
                    if let Some(panel) = panel_layout.panel_at(mouse.column, mouse.row) {
                        tui_state.focused_panel = panel;
                    }
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            tui_state.scroll_focused(3, panel_counts, panel_layout.rows)
                        }
                        MouseEventKind::ScrollUp => {
                            tui_state.scroll_focused(-3, panel_counts, panel_layout.rows)
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn handle_tui_completion_key(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    key: KeyEvent,
) -> Result<Option<AttachActionNotice>> {
    let action = match key.code {
        KeyCode::Char('m') => CompletionAction::Materialize,
        KeyCode::Char('e') => CompletionAction::Extend,
        KeyCode::Char('a') => CompletionAction::Apply,
        KeyCode::Char('b') => CompletionAction::Abandon,
        KeyCode::Char('d') => CompletionAction::Docs,
        KeyCode::Char('s') => CompletionAction::Show,
        _ => return Ok(None),
    };

    suspend_tui(terminal)?;
    let action_result = match action {
        CompletionAction::Materialize => prompt_materialize_action(paths, state),
        CompletionAction::Extend => prompt_extend_action(state).await,
        CompletionAction::Apply => apply_command(
            state.run_id.clone(),
            "squash".to_string(),
            None,
            false,
            false,
            false,
            None,
            false,
        ),
        CompletionAction::Abandon => abandon_command(state.run_id.clone(), false, false),
        CompletionAction::Docs => {
            Box::pin(doc_command(DocCommandArgs {
                run_id: state.run_id.clone(),
                kind: DocKind::Narrative,
                export: None,
                polish: false,
                no_confirm: true,
                force: false,
                doc_skill: None,
                doc_provider: None,
                budget_cap: None,
            }))
            .await
        }
        CompletionAction::Show => {
            show_command(&state.run_id, None, false, false, false, false, None)
        }
        CompletionAction::Quit => Ok(()),
    };
    if let Err(err) = &action_result {
        print_error(err);
        print_error_hint(err);
    }
    let _ = prompt::open("press Enter to return to attach...", None);
    resume_tui(terminal)?;
    Ok(Some(AttachActionNotice {
        action,
        success: action_result.is_ok(),
    }))
}

fn suspend_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    Ok(())
}

fn attach_should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn attach_should_return_to_plan(key: KeyEvent) -> bool {
    attach_should_quit(key)
        || matches!(key.code, KeyCode::Backspace)
        || (key.code == KeyCode::Char('b') && key.modifiers.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachPanel {
    Activity,
    Files,
    Processes,
}

impl AttachPanel {
    fn next(self) -> Self {
        match self {
            Self::Activity => Self::Files,
            Self::Files => Self::Processes,
            Self::Processes => Self::Activity,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Activity => Self::Processes,
            Self::Files => Self::Activity,
            Self::Processes => Self::Files,
        }
    }
}

#[derive(Debug, Clone)]
struct AttachParentPlan {
    plan_id: String,
    task_id: String,
}

#[derive(Debug)]
struct AttachTuiState {
    focused_panel: AttachPanel,
    activity_scroll: usize,
    docs_scroll: usize,
    docs_open: bool,
    files_scroll: usize,
    processes_scroll: usize,
    show_completion_actions: bool,
    post_action_notice: Option<AttachActionNotice>,
    parent_plan: Option<AttachParentPlan>,
}

impl Default for AttachTuiState {
    fn default() -> Self {
        Self {
            focused_panel: AttachPanel::Activity,
            activity_scroll: 0,
            docs_scroll: 0,
            docs_open: false,
            files_scroll: 0,
            processes_scroll: 0,
            show_completion_actions: true,
            post_action_notice: None,
            parent_plan: None,
        }
    }
}

impl AttachTuiState {
    fn handle_key(&mut self, key: KeyEvent, counts: AttachPanelCounts, rows: AttachPanelRows) {
        match key.code {
            KeyCode::Tab => self.focused_panel = self.focused_panel.next(),
            KeyCode::BackTab => self.focused_panel = self.focused_panel.previous(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_focused(-1, counts, rows),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_focused(1, counts, rows),
            KeyCode::PageUp => {
                self.scroll_focused(-page_delta(self.focused_panel, rows), counts, rows)
            }
            KeyCode::PageDown => {
                self.scroll_focused(page_delta(self.focused_panel, rows), counts, rows)
            }
            KeyCode::Home | KeyCode::Char('g') => self.set_focused_scroll(0),
            KeyCode::End | KeyCode::Char('G') => {
                self.set_focused_scroll(max_panel_scroll(self.focused_panel, counts, rows))
            }
            _ => {}
        }
        self.clamp(counts, rows);
    }

    fn toggle_docs(&mut self) {
        self.docs_open = !self.docs_open;
        self.focused_panel = AttachPanel::Activity;
        self.post_action_notice = None;
    }

    fn record_post_action(&mut self, notice: AttachActionNotice) {
        self.docs_open = false;
        self.focused_panel = AttachPanel::Activity;
        self.activity_scroll = 0;
        self.docs_scroll = 0;
        self.files_scroll = 0;
        self.processes_scroll = 0;
        self.post_action_notice = Some(notice);
    }

    fn scroll_focused(&mut self, delta: isize, counts: AttachPanelCounts, rows: AttachPanelRows) {
        let current = self.focused_scroll();
        let max = max_panel_scroll(self.focused_panel, counts, rows);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        self.set_focused_scroll(next.min(max));
    }

    fn clamp(&mut self, counts: AttachPanelCounts, rows: AttachPanelRows) {
        self.activity_scroll =
            self.activity_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.docs_scroll =
            self.docs_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.files_scroll =
            self.files_scroll
                .min(max_panel_scroll(AttachPanel::Files, counts, rows));
        self.processes_scroll =
            self.processes_scroll
                .min(max_panel_scroll(AttachPanel::Processes, counts, rows));
    }

    fn focused_scroll(&self) -> usize {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll,
            AttachPanel::Activity => self.activity_scroll,
            AttachPanel::Files => self.files_scroll,
            AttachPanel::Processes => self.processes_scroll,
        }
    }

    fn set_focused_scroll(&mut self, offset: usize) {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll = offset,
            AttachPanel::Activity => self.activity_scroll = offset,
            AttachPanel::Files => self.files_scroll = offset,
            AttachPanel::Processes => self.processes_scroll = offset,
        }
    }
}

#[derive(Debug, Clone)]
struct AttachActionNotice {
    action: CompletionAction,
    success: bool,
}

impl AttachActionNotice {
    fn lines(&self) -> Vec<String> {
        let status = if self.success { "finished" } else { "failed" };
        let mut lines = vec![format!("{} action {status}", self.action.label())];
        if self.success {
            lines.push(self.action.success_detail().to_string());
            lines.push("next: q detach | deadreckon status | deadreckon list".to_string());
        } else {
            lines.push("see the terminal output above for the error and suggested fix".to_string());
            lines.push("next: retry the action or q detach".to_string());
        }
        lines
    }
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelCounts {
    activity: usize,
    files: usize,
    processes: usize,
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelRows {
    activity: usize,
    files: usize,
    processes: usize,
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelLayout {
    header: ratatui::layout::Rect,
    activity: ratatui::layout::Rect,
    files: ratatui::layout::Rect,
    processes: ratatui::layout::Rect,
    footer: ratatui::layout::Rect,
    rows: AttachPanelRows,
}

impl AttachPanelLayout {
    fn panel_at(&self, column: u16, row: u16) -> Option<AttachPanel> {
        if rect_contains(self.activity, column, row) {
            Some(AttachPanel::Activity)
        } else if rect_contains(self.files, column, row) {
            Some(AttachPanel::Files)
        } else if rect_contains(self.processes, column, row) {
            Some(AttachPanel::Processes)
        } else {
            None
        }
    }
}

fn attach_panel_layout(area: ratatui::layout::Rect) -> AttachPanelLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(area);
    let center = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(60), Constraint::Length(44)])
        .split(vertical[1]);
    AttachPanelLayout {
        header: vertical[0],
        activity: center[0],
        files: center[1],
        processes: vertical[2],
        footer: vertical[3],
        rows: AttachPanelRows {
            activity: panel_inner_rows(center[0]),
            files: panel_inner_rows(center[1]),
            processes: panel_inner_rows(vertical[2]),
        },
    }
}

fn attach_panel_counts(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> AttachPanelCounts {
    AttachPanelCounts {
        activity: if tui_state.docs_open && state.status == RunStatus::Completed {
            render_markdown_doc_lines(state).len()
        } else {
            attach_activity_lines_for_tui(state, spend, traces, events, live, tui_state).len()
        },
        files: live_file_lines(live).len(),
        processes: process_lines(live).len(),
    }
}

fn panel_inner_rows(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

fn rect_contains(rect: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn page_delta(panel: AttachPanel, rows: AttachPanelRows) -> isize {
    let rows = panel_rows(panel, rows).max(1);
    rows.saturating_sub(1).max(1) as isize
}

fn max_panel_scroll(panel: AttachPanel, counts: AttachPanelCounts, rows: AttachPanelRows) -> usize {
    panel_count(panel, counts).saturating_sub(panel_rows(panel, rows))
}

fn panel_count(panel: AttachPanel, counts: AttachPanelCounts) -> usize {
    match panel {
        AttachPanel::Activity => counts.activity,
        AttachPanel::Files => counts.files,
        AttachPanel::Processes => counts.processes,
    }
}

fn panel_rows(panel: AttachPanel, rows: AttachPanelRows) -> usize {
    match panel {
        AttachPanel::Activity => rows.activity,
        AttachPanel::Files => rows.files,
        AttachPanel::Processes => rows.processes,
    }
}

#[derive(Debug, Default)]
struct AttachLive {
    file_count: usize,
    total_bytes: u64,
    files: Vec<LiveFile>,
    pids: Vec<LivePid>,
    provider_activity: Vec<String>,
    provider_context_tokens: Option<u64>,
    provider_context_window: Option<u64>,
    acceptance: AcceptanceLive,
    working_dir_exists: bool,
}

#[derive(Debug, Clone)]
struct AcceptanceLive {
    status: AcceptanceUiStatus,
    total: usize,
    completed: usize,
    passed: usize,
    failed: usize,
    required_failed: usize,
    latest_detail: Option<String>,
    progress_lines: Vec<String>,
}

impl Default for AcceptanceLive {
    fn default() -> Self {
        Self {
            status: AcceptanceUiStatus::DefaultGate,
            total: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            required_failed: 0,
            latest_detail: None,
            progress_lines: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptanceUiStatus {
    DefaultGate,
    Configured,
    Running,
    Passed,
    Failed,
}

#[derive(Debug)]
struct LiveFile {
    path: String,
    bytes: u64,
    modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct LivePid {
    pid: u32,
    alive: bool,
    command: String,
}

fn collect_attach_live(state: &deadreckon_core::PipelineState) -> AttachLive {
    let mut files = inventory_files(&state.working_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| {
            !path_has_component(path, "node_modules") && !path_has_component(path, ".git")
        })
        .filter_map(|path| live_file(&state.working_dir, &path))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    let file_count = files.len();
    let total_bytes = files.iter().map(|file| file.bytes).sum();
    let pids = supervised_pids(state)
        .into_iter()
        .map(live_pid)
        .collect::<Vec<_>>();
    let provider_activity = collect_provider_activity(state);
    let acceptance = collect_acceptance_live(state);
    AttachLive {
        file_count,
        total_bytes,
        files,
        pids,
        provider_context_tokens: provider_activity.context_tokens,
        provider_context_window: provider_activity.context_window,
        provider_activity: provider_activity.lines,
        acceptance,
        working_dir_exists: state.working_dir.exists(),
    }
}

fn collect_acceptance_live(state: &deadreckon_core::PipelineState) -> AcceptanceLive {
    let marker_path = marker_path_for_run_root(&state.run_root);
    if marker_path.exists()
        && let Ok(bytes) = fs::read(&marker_path)
        && let Ok(marker) = serde_json::from_slice::<AcceptanceMarker>(&bytes)
    {
        return acceptance_live_from_marker(&marker);
    }

    let progress_path = acceptance_progress_path_for_run_root(&state.run_root);
    if progress_path.exists()
        && let Ok(entries) = read_jsonl::<AcceptanceProgressEntry>(&progress_path)
        && !entries.is_empty()
    {
        return acceptance_live_from_progress(&entries);
    }

    let spec_path = acceptance_spec_path_for_run_root(&state.run_root);
    if spec_path.exists()
        && let Ok(raw) = fs::read_to_string(&spec_path)
        && let Ok(count) = acceptance_check_count(&raw)
    {
        return AcceptanceLive {
            status: AcceptanceUiStatus::Configured,
            total: count,
            latest_detail: Some("runs after provider completion".to_string()),
            ..AcceptanceLive::default()
        };
    }

    AcceptanceLive {
        latest_detail: Some("inferred from project files".to_string()),
        ..AcceptanceLive::default()
    }
}

fn acceptance_live_from_marker(marker: &AcceptanceMarker) -> AcceptanceLive {
    let total = marker.checks.len().max(marker.check_count);
    let passed = if marker.checks.is_empty() {
        marker.check_count
    } else {
        marker.checks.iter().filter(|result| result.passed).count()
    };
    let failed = marker.checks.iter().filter(|result| !result.passed).count();
    let required_failed = marker
        .checks
        .iter()
        .filter(|result| result.must_pass && !result.passed)
        .count();
    let status = if required_failed > 0 {
        AcceptanceUiStatus::Failed
    } else {
        AcceptanceUiStatus::Passed
    };
    AcceptanceLive {
        status,
        total,
        completed: marker.checks.len().max(marker.check_count),
        passed,
        failed,
        required_failed,
        latest_detail: marker.checks.last().map(|result| result.detail.clone()),
        progress_lines: marker
            .checks
            .iter()
            .rev()
            .take(8)
            .map(acceptance_result_line)
            .collect(),
    }
}

fn acceptance_live_from_progress(entries: &[AcceptanceProgressEntry]) -> AcceptanceLive {
    let total = entries.iter().map(|entry| entry.total).max().unwrap_or(0);
    let mut results = entries
        .iter()
        .filter_map(|entry| entry.result.clone())
        .collect::<Vec<_>>();
    if results.len() > total && total > 0 {
        results = results.split_off(results.len() - total);
    }
    let passed = results.iter().filter(|result| result.passed).count();
    let failed = results.iter().filter(|result| !result.passed).count();
    let required_failed = results
        .iter()
        .filter(|result| result.must_pass && !result.passed)
        .count();
    let completed = results.len();
    let latest = entries.last();
    let status = if required_failed > 0 {
        AcceptanceUiStatus::Failed
    } else if completed >= total && total > 0 {
        AcceptanceUiStatus::Passed
    } else if latest.is_some_and(|entry| entry.status == "running" || entry.status == "started") {
        AcceptanceUiStatus::Running
    } else {
        AcceptanceUiStatus::Configured
    };
    let latest_detail = latest.and_then(|entry| {
        entry
            .result
            .as_ref()
            .map(|result| result.detail.clone())
            .or_else(|| {
                if entry.status == "running" && entry.total > 0 {
                    Some(format!("checking {} of {}", entry.index, entry.total))
                } else {
                    None
                }
            })
    });
    let mut progress_lines = entries
        .iter()
        .rev()
        .filter_map(|entry| {
            entry
                .result
                .as_ref()
                .map(acceptance_result_line)
                .or_else(|| {
                    if entry.status == "running" && entry.total > 0 {
                        Some(format!(
                            "• checking {} of {}",
                            entry.index.min(entry.total),
                            entry.total
                        ))
                    } else {
                        None
                    }
                })
        })
        .take(8)
        .collect::<Vec<_>>();
    if progress_lines.is_empty() && total > 0 {
        progress_lines.push(format!("• waiting to check {total} criteria"));
    }
    AcceptanceLive {
        status,
        total,
        completed,
        passed,
        failed,
        required_failed,
        latest_detail,
        progress_lines,
    }
}

fn acceptance_result_line(result: &deadreckon_core::AcceptanceCheckResult) -> String {
    let mark = if result.passed {
        "✓"
    } else if result.must_pass {
        "✗"
    } else {
        "!"
    };
    format!("{mark} {} {}", result.kind, one_line(&result.detail, 120))
}

fn live_file(root: &Path, path: &Path) -> Option<LiveFile> {
    let metadata = fs::metadata(path).ok()?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    Some(LiveFile {
        path: relative.display().to_string(),
        bytes: metadata.len(),
        modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
    })
}

fn path_has_component(path: &Path, component: &str) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy() == component)
}

fn live_pid(pid: u32) -> LivePid {
    let alive = deadreckon_core::pid_is_alive(pid);
    let command = if alive {
        std::process::Command::new("ps")
            .args(["-o", "stat=,etime=,command=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout).ok()
                } else {
                    None
                }
            })
            .map(|raw| one_line(raw.trim(), 110))
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| "alive".to_string())
    } else {
        "not running".to_string()
    };
    LivePid {
        pid,
        alive,
        command,
    }
}

#[derive(Debug, Default)]
struct ProviderActivity {
    lines: Vec<String>,
    context_tokens: Option<u64>,
    context_window: Option<u64>,
}

#[derive(Debug)]
struct ProviderJsonlLogSpec {
    schema: String,
    roots: Vec<PathBuf>,
    since: DateTime<Utc>,
    cwd_match: IngestCwdMatch,
    cwd_match_path: Option<String>,
    storage: IngestStorage,
    file_glob: Option<String>,
}

fn collect_provider_activity(state: &deadreckon_core::PipelineState) -> ProviderActivity {
    let mut flight = collect_flight_provider_activity(state);
    let Some(spec) = provider_jsonl_log_spec(state) else {
        return flight;
    };
    let fallback = collect_jsonl_provider_activity(state, &spec);
    if flight.lines.is_empty() {
        return fallback;
    }
    if !fallback.lines.is_empty() {
        flight.lines.extend(
            fallback
                .lines
                .into_iter()
                .map(|line| format!("provider log {line}")),
        );
        flight.context_tokens = flight.context_tokens.or(fallback.context_tokens);
        flight.context_window = flight.context_window.or(fallback.context_window);
    }
    cap_provider_activity(flight, 240)
}

fn collect_flight_provider_activity(state: &deadreckon_core::PipelineState) -> ProviderActivity {
    let Ok(events) = read_flight_events(state) else {
        return ProviderActivity::default();
    };
    let mut activity = ProviderActivity::default();
    for event in events {
        if let Some(usage) = event.usage.as_ref() {
            activity.context_tokens = Some(usage.input_tokens + usage.output_tokens);
            activity.context_window = usage.context_window;
        }
        activity.lines.push(flight_activity_line(&event));
    }
    cap_provider_activity(activity, 240)
}

fn flight_activity_line(event: &FlightEvent) -> String {
    let checkpoint = event
        .checkpoint_id
        .as_deref()
        .map(|id| format!(" checkpoint {id}"))
        .unwrap_or_default();
    let file_count = if event.files.is_empty() {
        String::new()
    } else {
        format!(" files {}", event.files.len())
    };
    format!(
        "flight #{:06} turn {} {}{}{} {}",
        event.seq,
        event.deadreckon_turn,
        flight_event_kind_label(event.kind),
        checkpoint,
        file_count,
        one_line(&event.summary, 120)
    )
}

fn provider_jsonl_log_spec(state: &deadreckon_core::PipelineState) -> Option<ProviderJsonlLogSpec> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home()).ok()?;
    provider_jsonl_log_spec_from_registry(state, &registry, &home)
}

fn provider_jsonl_log_spec_from_registry(
    state: &deadreckon_core::PipelineState,
    registry: &ProviderRegistry,
    home: &Path,
) -> Option<ProviderJsonlLogSpec> {
    let provider = state.provider.as_deref()?;
    let ingest = registry.get(provider)?.ingest.as_ref()?;
    let schema = ingest.schema.trim();
    if schema.is_empty() {
        return None;
    }
    let freshness_minutes = ingest.freshness_minutes.unwrap_or(2).max(0);
    let since = state.started_at - ChronoDuration::minutes(freshness_minutes);
    let storage = ingest.storage.clone().unwrap_or(IngestStorage::Jsonl);
    Some(ProviderJsonlLogSpec {
        schema: schema.to_string(),
        roots: provider_ingest_roots(ingest, state, home),
        since,
        cwd_match: ingest.cwd_match.clone(),
        cwd_match_path: ingest.cwd_match_path.clone(),
        storage,
        file_glob: ingest.file_glob.clone(),
    })
}

fn collect_jsonl_provider_activity(
    state: &deadreckon_core::PipelineState,
    spec: &ProviderJsonlLogSpec,
) -> ProviderActivity {
    let working_dirs = run_working_dirs(state);
    let mut candidates = Vec::new();
    for root in &spec.roots {
        collect_recent_provider_files(root, spec, &mut candidates, 0);
    }
    candidates.sort_by(|left, right| right.1.cmp(&left.1));
    for (path, _) in candidates {
        if !provider_jsonl_session_matches_run(spec, &path, &working_dirs) {
            continue;
        }
        let mut activity = ProviderActivity::default();
        if !provider_jsonl_activity_file(&spec.schema, &path, &mut activity) {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            for line in raw.lines() {
                let mut parsed = provider_jsonl_activity_lines(&spec.schema, line, &mut activity);
                activity.lines.append(&mut parsed);
            }
        }
        if activity.lines.is_empty() {
            continue;
        }
        append_provider_log_line(&mut activity, &path);
        return cap_provider_activity(activity, 240);
    }
    ProviderActivity::default()
}

fn run_working_dirs(state: &deadreckon_core::PipelineState) -> Vec<String> {
    vec![
        state.working_dir.to_string_lossy().to_string(),
        state.run_root.join("working").to_string_lossy().to_string(),
    ]
}

fn provider_jsonl_session_matches_run(
    spec: &ProviderJsonlLogSpec,
    path: &Path,
    working_dirs: &[String],
) -> bool {
    match spec.cwd_match {
        IngestCwdMatch::None => true,
        IngestCwdMatch::SessionMeta => jsonl_session_meta_cwd_matches(path, working_dirs),
        IngestCwdMatch::TopLevel | IngestCwdMatch::ClaudeProjectDir => {
            jsonl_top_level_cwd_matches(path, working_dirs, 80)
                || matches!(spec.cwd_match, IngestCwdMatch::ClaudeProjectDir)
        }
        IngestCwdMatch::JsonPointer => spec
            .cwd_match_path
            .as_deref()
            .is_some_and(|pointer| jsonl_pointer_cwd_matches(path, pointer, working_dirs, 80)),
        IngestCwdMatch::DirectoryField => {
            json_file_field_cwd_matches(path, "directory", working_dirs)
        }
    }
}

fn provider_jsonl_activity_lines(
    schema: &str,
    line: &str,
    activity: &mut ProviderActivity,
) -> Vec<String> {
    match schema {
        "codex-cli" => codex_activity_line(line, activity)
            .into_iter()
            .collect::<Vec<_>>(),
        "claude-code" => claude_activity_lines(line, activity),
        "copilot-cli" => copilot_activity_lines(line, activity),
        "pi" => pi_activity_lines(line, activity),
        _ => Vec::new(),
    }
}

fn provider_jsonl_activity_file(
    schema: &str,
    path: &Path,
    activity: &mut ProviderActivity,
) -> bool {
    match schema {
        "gemini" => {
            parse_gemini_activity_file(path, activity);
            true
        }
        "opencode" => {
            parse_opencode_activity_file(path, activity);
            true
        }
        "pi" => {
            parse_pi_activity_file(path, activity);
            true
        }
        _ => false,
    }
}

fn append_provider_log_line(activity: &mut ProviderActivity, path: &Path) {
    activity.lines.push(format!(
        "{} provider log {}",
        format_age(
            fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(DateTime::<Utc>::from),
        ),
        path.display()
    ));
}

fn cap_provider_activity(mut activity: ProviderActivity, cap: usize) -> ProviderActivity {
    activity.lines = activity
        .lines
        .into_iter()
        .rev()
        .take(cap)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    activity
}

fn jsonl_session_meta_cwd_matches(path: &Path, working_dirs: &[String]) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = io::BufReader::new(file);
    for line in reader.lines().map_while(std::result::Result::ok).take(8) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(cwd) = value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        return working_dirs.iter().any(|working_dir| working_dir == cwd);
    }
    false
}

fn jsonl_top_level_cwd_matches(path: &Path, working_dirs: &[String], scan_lines: usize) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = io::BufReader::new(file);
    for line in reader
        .lines()
        .map_while(std::result::Result::ok)
        .take(scan_lines)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(Value::as_str)
            && working_dirs.iter().any(|working_dir| working_dir == cwd)
        {
            return true;
        }
    }
    false
}

fn jsonl_pointer_cwd_matches(
    path: &Path,
    pointer: &str,
    working_dirs: &[String],
    scan_lines: usize,
) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = io::BufReader::new(file);
    for line in reader
        .lines()
        .map_while(std::result::Result::ok)
        .take(scan_lines)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value
            .pointer(&json_pointer_path(pointer))
            .and_then(Value::as_str)
            .is_some_and(|cwd| working_dirs.iter().any(|working_dir| working_dir == cwd))
        {
            return true;
        }
    }
    false
}

fn json_file_field_cwd_matches(path: &Path, field: &str, working_dirs: &[String]) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|cwd| working_dirs.iter().any(|working_dir| working_dir == cwd))
}

fn json_pointer_path(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    format!("/{}", path.replace('.', "/"))
}

fn claude_project_name_for_workdir(working_dir: &str) -> String {
    let resolved = fs::canonicalize(working_dir).unwrap_or_else(|_| PathBuf::from(working_dir));
    let raw = resolved.to_string_lossy();
    let mut name = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            name.push(ch);
        } else {
            name.push('-');
        }
    }
    if !name.starts_with('-') {
        name.insert(0, '-');
    }
    name
}

fn provider_ingest_roots(
    ingest: &IngestDescriptor,
    state: &deadreckon_core::PipelineState,
    home: &Path,
) -> Vec<PathBuf> {
    provider_ingest_roots_for_working_dirs(ingest, home, &run_working_dirs(state), false)
}

fn provider_ingest_base_roots(
    ingest: &IngestDescriptor,
    home: &Path,
    env_value: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let mut roots = env_value
        .filter(|value| !value.is_empty())
        .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            ingest
                .default_dirs
                .iter()
                .map(|path| expand_home_path_for(path, home))
                .collect()
        });
    dedup_pathbufs(&mut roots);
    roots
}

fn expand_home_path_for(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    if let Ok(rest) = path.strip_prefix("~") {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn dedup_pathbufs(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn collect_recent_provider_files(
    root: &Path,
    spec: &ProviderJsonlLogSpec,
    files: &mut Vec<(PathBuf, DateTime<Utc>)>,
    depth: usize,
) {
    if depth == 0 && spec.storage == IngestStorage::OpenCodeStorage {
        collect_recent_opencode_session_files(root, spec.since, files);
        return;
    }
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_recent_provider_files(&path, spec, files, depth + 1);
            continue;
        }
        if !provider_file_matches_spec(&path, spec) {
            continue;
        }
        let Some(modified_at) = metadata.modified().ok().map(DateTime::<Utc>::from) else {
            continue;
        };
        if modified_at >= spec.since {
            files.push((path, modified_at));
        }
    }
}

fn collect_recent_opencode_session_files(
    root: &Path,
    since: DateTime<Utc>,
    files: &mut Vec<(PathBuf, DateTime<Utc>)>,
) {
    let session_root = root.join("storage/session");
    let Ok(projects) = fs::read_dir(session_root) else {
        return;
    };
    for project in projects.flatten() {
        let project_path = project.path();
        let Ok(metadata) = project.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(project_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(modified_at) = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(DateTime::<Utc>::from)
            else {
                continue;
            };
            if modified_at >= since {
                files.push((path, modified_at));
            }
        }
    }
}

fn provider_file_matches_spec(path: &Path, spec: &ProviderJsonlLogSpec) -> bool {
    if let Some(glob) = spec.file_glob.as_deref()
        && let Some(extension) = glob.strip_prefix("*.")
    {
        return path.extension().and_then(|value| value.to_str()) == Some(extension);
    }
    match spec.storage {
        IngestStorage::Jsonl => path.extension().and_then(|value| value.to_str()) == Some("jsonl"),
        IngestStorage::Json => path.extension().and_then(|value| value.to_str()) == Some("json"),
        IngestStorage::JsonOrJsonl => matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("json") | Some("jsonl")
        ),
        IngestStorage::OpenCodeStorage => {
            path.extension().and_then(|value| value.to_str()) == Some("json")
        }
    }
}

fn codex_activity_line(line: &str, activity: &mut ProviderActivity) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let timestamp = short_timestamp(value.get("timestamp").and_then(Value::as_str));
    let payload = value.get("payload")?;
    match (
        value.get("type").and_then(Value::as_str),
        payload.get("type").and_then(Value::as_str),
    ) {
        (Some("event_msg"), Some("task_started")) => Some(format!("{timestamp} codex started")),
        (Some("event_msg"), Some("agent_message")) => payload
            .get("message")
            .and_then(Value::as_str)
            .map(|message| format!("{timestamp} agent {}", one_line(message, 140))),
        (Some("event_msg"), Some("token_count")) => {
            let usage = payload.get("info")?.get("total_token_usage")?;
            let total = usage.get("total_tokens").and_then(Value::as_u64)?;
            let window = payload
                .get("info")
                .and_then(|info| info.get("model_context_window"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            activity.context_tokens = Some(total);
            activity.context_window = Some(window);
            let rate = payload
                .get("rate_limits")
                .and_then(|limits| limits.get("primary"))
                .and_then(|primary| primary.get("used_percent"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            Some(format!(
                "{timestamp} tokens {total}/{window} rate {rate:.0}%"
            ))
        }
        (Some("response_item"), Some("function_call")) => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(format!(
                "{timestamp} tool {} {}",
                provider_tool_label(name),
                one_line(&tool_call_summary(name, args), 140)
            ))
        }
        (Some("response_item"), Some("function_call_output")) => payload
            .get("output")
            .and_then(Value::as_str)
            .map(|output| format!("{timestamp} result {}", one_line(output, 140))),
        _ => None,
    }
}

fn claude_activity_lines(line: &str, activity: &mut ProviderActivity) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let timestamp = short_timestamp(value.get("timestamp").and_then(Value::as_str));
    let row_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match row_type {
        "assistant" => claude_assistant_activity_lines(&value, &timestamp, activity),
        "user" => claude_user_activity_lines(&value, &timestamp),
        "attachment" => claude_attachment_activity_line(&value, &timestamp)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn claude_assistant_activity_lines(
    value: &Value,
    timestamp: &str,
    activity: &mut ProviderActivity,
) -> Vec<String> {
    let Some(message) = value.get("message") else {
        return Vec::new();
    };
    if let Some(usage) = message.get("usage")
        && let Some(tokens) = claude_usage_tokens(usage)
    {
        activity.context_tokens = Some(tokens);
        activity.context_window = Some(200_000);
    }
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for part in content {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    lines.push(format!("{timestamp} agent {}", one_line(text, 140)));
                }
            }
            Some("thinking") => {
                if let Some(text) = part.get("thinking").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    lines.push(format!("{timestamp} thinking {}", one_line(text, 140)));
                }
            }
            Some("tool_use") => {
                let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = part.get("input").unwrap_or(&Value::Null);
                lines.push(format!(
                    "{timestamp} tool {} {}",
                    provider_tool_label(name),
                    one_line(&claude_tool_summary(name, input), 140)
                ));
            }
            _ => {}
        }
    }
    lines
}

fn claude_user_activity_lines(value: &Value, timestamp: &str) -> Vec<String> {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for part in content {
        if part.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let text = claude_content_text(part.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        let prefix = if part
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "error"
        } else {
            "result"
        };
        lines.push(format!("{timestamp} {prefix} {}", one_line(&text, 140)));
    }
    lines
}

fn claude_attachment_activity_line(value: &Value, timestamp: &str) -> Option<String> {
    let attachment = value.get("attachment")?;
    if attachment.get("type").and_then(Value::as_str) != Some("todo_reminder") {
        return None;
    }
    let items = attachment.get("content")?.as_array()?;
    let total = items.len();
    let completed = items
        .iter()
        .filter(|item| item.get("status").and_then(Value::as_str) == Some("completed"))
        .count();
    let active = items
        .iter()
        .filter(|item| item.get("status").and_then(Value::as_str) == Some("in_progress"))
        .count();
    Some(format!(
        "{timestamp} todo {completed}/{total} done  {active} active"
    ))
}

fn claude_content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let Some(items) = value.as_array() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.get("text")
                .or_else(|| item.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn copilot_activity_lines(line: &str, activity: &mut ProviderActivity) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let timestamp = short_timestamp(value.get("timestamp").and_then(Value::as_str));
    let mut lines = Vec::new();
    if let Some(usage) = value.get("usage")
        && let Some(line) = usage_activity_line(&timestamp, usage, activity, Some(258_400))
    {
        lines.push(line);
    }
    match value.get("type").and_then(Value::as_str) {
        Some("assistant.message") => {
            let data = value.get("data").unwrap_or(&Value::Null);
            if let Some(reasoning) = data.get("reasoningText").and_then(Value::as_str)
                && !reasoning.trim().is_empty()
            {
                lines.push(format!("{timestamp} thinking {}", one_line(reasoning, 140)));
            }
            if let Some(content) = data.get("content").and_then(Value::as_str)
                && !content.trim().is_empty()
            {
                lines.push(format!("{timestamp} agent {}", one_line(content, 140)));
            }
            if let Some(tool_requests) = data.get("toolRequests").and_then(Value::as_array) {
                for request in tool_requests {
                    let name = request
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let input = provider_arguments_value(request.get("arguments"));
                    lines.push(format!(
                        "{timestamp} tool {} {}",
                        provider_tool_label(name),
                        one_line(&json_tool_summary(name, &input), 140)
                    ));
                }
            }
            if let Some(output) = data.get("outputTokens").and_then(Value::as_u64) {
                lines.push(format!("{timestamp} tokens output {output}"));
            }
        }
        Some("assistant.reasoning") => {
            let data = value.get("data").unwrap_or(&Value::Null);
            let text = data
                .get("text")
                .or_else(|| data.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("reasoning");
            lines.push(format!("{timestamp} thinking {}", one_line(text, 140)));
        }
        Some("tool.execution_complete") => {
            let data = value.get("data").unwrap_or(&Value::Null);
            let result = data.get("result").unwrap_or(&Value::Null);
            if !result.is_null() {
                lines.push(format!(
                    "{timestamp} result {}",
                    one_line(&json_value_text(result), 140)
                ));
            }
        }
        _ => {}
    }
    lines
}

fn pi_activity_lines(line: &str, activity: &mut ProviderActivity) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let timestamp = pi_activity_timestamp(&value);
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return Vec::new();
    }
    let message = value.get("message").unwrap_or(&Value::Null);
    let mut lines = Vec::new();
    if let Some(usage) = message.get("usage")
        && let Some(line) = usage_activity_line(&timestamp, usage, activity, Some(1_000_000))
    {
        lines.push(line);
    }
    match message.get("role").and_then(Value::as_str) {
        Some("assistant") => {
            lines.extend(pi_assistant_content_lines(&timestamp, message));
        }
        Some("toolResult") => {
            let content = message.get("content").unwrap_or(&Value::Null);
            if !content.is_null() {
                lines.push(format!(
                    "{timestamp} result {}",
                    one_line(&json_value_text(content), 140)
                ));
            }
        }
        _ => {}
    }
    lines
}

fn parse_pi_activity_file(path: &Path, activity: &mut ProviderActivity) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let mut saw_session_header = false;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_session_header = value.get("type").and_then(Value::as_str) == Some("session");
        break;
    }
    if !saw_session_header {
        return;
    }
    for line in raw.lines() {
        let mut parsed = pi_activity_lines(line, activity);
        activity.lines.append(&mut parsed);
    }
}

fn pi_assistant_content_lines(timestamp: &str, message: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let content = message.get("content").unwrap_or(&Value::Null);
    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            lines.push(format!("{timestamp} agent {}", one_line(text, 140)));
        }
        return lines;
    }
    let Some(blocks) = content.as_array() else {
        return lines;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    lines.push(format!("{timestamp} agent {}", one_line(text, 140)));
                }
            }
            Some("thinking") => {
                let text = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or("thinking");
                lines.push(format!("{timestamp} thinking {}", one_line(text, 140)));
            }
            Some("toolCall") => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input =
                    normalize_pi_tool_arguments(provider_arguments_value(block.get("arguments")));
                lines.push(format!(
                    "{timestamp} tool {} {}",
                    provider_tool_label(name),
                    one_line(&json_tool_summary(name, &input), 140)
                ));
            }
            _ => {}
        }
    }
    lines
}

fn provider_arguments_value(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    if let Some(raw) = value.as_str() {
        return serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
    }
    value.clone()
}

fn normalize_pi_tool_arguments(mut value: Value) -> Value {
    let Some(map) = value.as_object_mut() else {
        return value;
    };
    if map.contains_key("description") {
        return value;
    }
    if let Some(intent) = map.remove("agent__intent").or_else(|| map.remove("_i")) {
        map.insert("description".to_string(), intent);
    }
    value
}

fn usage_activity_line(
    timestamp: &str,
    usage: &Value,
    activity: &mut ProviderActivity,
    context_window: Option<u64>,
) -> Option<String> {
    let input = number_field_any(usage, &["inputTokens", "input_tokens", "input"]);
    let output = number_field_any(usage, &["outputTokens", "output_tokens", "output"]);
    let cache_read = number_field_any(
        usage,
        &[
            "cacheReadTokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cacheRead",
            "cache.read",
        ],
    );
    let cache_write = number_field_any(
        usage,
        &[
            "cacheCreationTokens",
            "cacheWriteTokens",
            "cache_creation_tokens",
            "cache_creation_input_tokens",
            "cache_write_tokens",
            "cacheCreation",
            "cacheWrite",
            "cache.write",
        ],
    );
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    let context = input.unwrap_or(0) + cache_read.unwrap_or(0) + cache_write.unwrap_or(0);
    if context > 0 {
        activity.context_tokens = Some(context);
        activity.context_window = context_window;
    }
    Some(format!(
        "{timestamp} tokens input {} output {} cache {}",
        input.unwrap_or(0),
        output.unwrap_or(0),
        cache_read.unwrap_or(0) + cache_write.unwrap_or(0)
    ))
}

fn number_field_any(value: &Value, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        value
            .pointer(&json_pointer_path(name))
            .or_else(|| value.get(*name))
            .and_then(Value::as_u64)
    })
}

fn pi_activity_timestamp(value: &Value) -> String {
    if let Some(timestamp) = value.get("timestamp").and_then(Value::as_str) {
        return short_timestamp(Some(timestamp));
    }
    value
        .pointer("/message/timestamp")
        .and_then(Value::as_i64)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|timestamp| timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn provider_tool_label(name: &str) -> &str {
    let category = normalize_tool_category(name);
    if category == "Other" { name } else { category }
}

fn parse_gemini_activity_file(path: &Path, activity: &mut ProviderActivity) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    if let Ok(value) = serde_json::from_str::<Value>(&raw)
        && (value.get("messages").is_some() || value.get("sessionId").is_some())
    {
        if let Some(messages) = value.get("messages").and_then(Value::as_array) {
            for message in messages {
                let lines = gemini_activity_lines_from_value(message, activity);
                activity.lines.extend(lines);
            }
        } else {
            let lines = gemini_activity_lines_from_value(&value, activity);
            activity.lines.extend(lines);
        }
        return;
    }
    let mut records = BTreeMap::new();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let row_type = value.get("type").and_then(Value::as_str);
        if !matches!(row_type, Some("user") | Some("gemini")) {
            continue;
        }
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            records.insert(id.to_string(), value);
        } else {
            let key = format!("__row_{}", records.len());
            records.insert(key, value);
        }
    }
    for value in records.values() {
        let lines = gemini_activity_lines_from_value(value, activity);
        activity.lines.extend(lines);
    }
}

fn gemini_activity_lines_from_value(value: &Value, activity: &mut ProviderActivity) -> Vec<String> {
    if value.get("type").and_then(Value::as_str) != Some("gemini") {
        return Vec::new();
    }
    apply_gemini_tokens(value, activity);
    let timestamp = short_timestamp(value.get("timestamp").and_then(Value::as_str));
    let mut lines = Vec::new();
    if let Some(thoughts) = value.get("thoughts").and_then(Value::as_array) {
        for thought in thoughts {
            let subject = thought.get("subject").and_then(Value::as_str).unwrap_or("");
            let description = thought
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let text = [subject, description]
                .into_iter()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                lines.push(format!("{timestamp} thinking {}", one_line(&text, 140)));
            }
        }
    }
    for text in gemini_content_texts(value.get("content").unwrap_or(&Value::Null)) {
        if !text.trim().is_empty() {
            lines.push(format!("{timestamp} agent {}", one_line(&text, 140)));
        }
    }
    if let Some(tool_calls) = value.get("toolCalls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let name = tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = tool_call.get("args").unwrap_or(&Value::Null);
            lines.push(format!(
                "{timestamp} tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, args), 140)
            ));
            if let Some(results) = tool_call.get("result").and_then(Value::as_array) {
                for result in results {
                    if let Some(output) = result.pointer("/functionResponse/response/output") {
                        lines.push(format!(
                            "{timestamp} result {}",
                            one_line(&json_value_text(output), 140)
                        ));
                    }
                }
            }
        }
    }
    lines
}

fn gemini_content_texts(value: &Value) -> Vec<String> {
    if let Some(text) = value.as_str() {
        return vec![text.to_string()];
    }
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn apply_gemini_tokens(value: &Value, activity: &mut ProviderActivity) {
    let Some(tokens) = value.get("tokens") else {
        return;
    };
    let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
    let cached = tokens.get("cached").and_then(Value::as_u64).unwrap_or(0);
    let total = input + cached;
    if total > 0 {
        activity.context_tokens = Some(total);
        activity.context_window = Some(1_000_000);
    }
}

fn parse_opencode_activity_file(path: &Path, activity: &mut ProviderActivity) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(session) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let Some(session_id) = session.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(root) = path.ancestors().nth(4) else {
        return;
    };
    let messages = read_json_values_sorted(&root.join("storage/message").join(session_id));
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        apply_opencode_tokens(&message, activity);
        let Some(message_id) = message.get("id").and_then(Value::as_str) else {
            continue;
        };
        let timestamp = timestamp_from_millis(opencode_time_value(&message));
        let parts = read_json_values_sorted(&root.join("storage/part").join(message_id));
        for part in parts {
            apply_opencode_tokens(&part, activity);
            match part.get("type").and_then(Value::as_str) {
                Some("text") if role == "assistant" => {
                    let text = part
                        .get("content")
                        .or_else(|| part.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !text.trim().is_empty() {
                        activity
                            .lines
                            .push(format!("{timestamp} agent {}", one_line(text, 140)));
                    }
                }
                Some("reasoning") if role == "assistant" => {
                    let text = part
                        .get("content")
                        .or_else(|| part.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !text.trim().is_empty() {
                        activity
                            .lines
                            .push(format!("{timestamp} thinking {}", one_line(text, 140)));
                    }
                }
                Some("tool") if role == "assistant" => {
                    let name = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
                    let input = part
                        .pointer("/state/input")
                        .or_else(|| part.get("input"))
                        .unwrap_or(&Value::Null);
                    activity.lines.push(format!(
                        "{timestamp} tool {} {}",
                        provider_tool_label(name),
                        one_line(&json_tool_summary(name, input), 140)
                    ));
                }
                _ => {}
            }
        }
    }
}

fn read_json_values_sorted(dir: &Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut values = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let raw = fs::read_to_string(path).ok()?;
            serde_json::from_str::<Value>(&raw).ok()
        })
        .collect::<Vec<_>>();
    values.sort_by_key(opencode_time_value);
    values
}

fn opencode_time_value(value: &Value) -> i64 {
    let time = value.get("time").unwrap_or(&Value::Null);
    for key in ["created", "start", "end", "updated"] {
        if let Some(value) = time.get(key).and_then(Value::as_i64)
            && value > 0
        {
            return value;
        }
    }
    0
}

fn apply_opencode_tokens(value: &Value, activity: &mut ProviderActivity) {
    let Some(tokens) = value.get("tokens") else {
        return;
    };
    let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
    let cache_read = tokens
        .pointer("/cache/read")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = tokens
        .pointer("/cache/write")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = input + cache_read + cache_write;
    if total > 0 {
        activity.context_tokens = Some(total);
    }
}

fn timestamp_from_millis(value: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(value)
        .map(|timestamp| timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn json_tool_summary(name: &str, input: &Value) -> String {
    match name {
        "read_file" | "read" | "write_file" | "write" | "edit_file" | "edit" => input
            .get("path")
            .or_else(|| input.get("file_path"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        "run_command" | "execute_command" | "run_shell_command" | "bash" => input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        _ => input.to_string(),
    }
}

fn json_value_text(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn claude_usage_tokens(usage: &Value) -> Option<u64> {
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = input + output + cache_creation + cache_read;
    (total > 0).then_some(total)
}

fn claude_tool_summary(name: &str, input: &Value) -> String {
    match name {
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("notebook_path"))
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        "Glob" => input
            .get("pattern")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        "Grep" => {
            let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("");
            let path = input.get("path").and_then(Value::as_str).unwrap_or("");
            if path.is_empty() {
                pattern.to_string()
            } else {
                format!("{pattern} in {path}")
            }
        }
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| format!("{} todos", todos.len()))
            .unwrap_or_else(|| input.to_string()),
        "Task" => input
            .get("description")
            .or_else(|| input.get("prompt"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| input.to_string()),
        _ => input.to_string(),
    }
}

fn tool_call_summary(name: &str, args: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(args) else {
        return one_line(args, 140);
    };
    match name {
        "exec_command" => value
            .get("cmd")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string()),
        "update_plan" => value
            .get("plan")
            .and_then(Value::as_array)
            .map(|plan| {
                plan.iter()
                    .filter_map(|item| {
                        Some(format!(
                            "{}:{}",
                            item.get("status")?.as_str()?,
                            item.get("step")?.as_str()?
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn short_timestamp(timestamp: Option<&str>) -> String {
    timestamp
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn render_attach(
    frame: &mut ratatui::Frame<'_>,
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let area = frame.area();
    let layout = attach_panel_layout(area);

    let metered_provider = provider_is_metered(state);
    let top_constraints = if metered_provider {
        vec![
            Constraint::Percentage(45),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
        ]
    } else {
        vec![
            Constraint::Percentage(66),
            Constraint::Percentage(17),
            Constraint::Percentage(17),
        ]
    };
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(top_constraints)
        .split(layout.header);
    let header = Paragraph::new(attach_header_text_for_state(
        state,
        top[0].width,
        tui_state.parent_plan.as_ref(),
    ))
    .block(Block::default().borders(Borders::ALL).title("deadreckon"));
    frame.render_widget(header, top[0]);
    if metered_provider {
        render_spend(frame, top[1], state);
        render_context(frame, top[2], spend, live);
        render_acceptance(frame, top[3], live);
    } else {
        render_context(frame, top[1], spend, live);
        render_acceptance(frame, top[2], live);
    }

    if tui_state.docs_open && state.status == RunStatus::Completed {
        render_run_docs(frame, layout.activity, state, tui_state);
    } else {
        let trace_lines =
            attach_activity_lines_for_tui(state, spend, traces, events, live, tui_state);
        let stream_rows = layout.activity.height.saturating_sub(2) as usize;
        let trace_items = visible_items(&trace_lines, tui_state.activity_scroll, stream_rows);
        frame.render_widget(
            List::new(trace_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "tool calls / provider activity",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.activity_scroll,
                        stream_rows,
                        trace_lines.len(),
                    )),
            ),
            layout.activity,
        );
    }
    render_live_files(frame, layout.files, live, tui_state);
    render_processes(frame, layout.processes, live, tui_state);
    let tick = (Utc::now().timestamp_millis() / 180).max(0) as usize;
    let status_line = deadreckoning_status_line(
        state,
        &turn_timer(events, spend, traces, state),
        layout.footer.width,
        tick,
    );
    frame.render_widget(
        Paragraph::new(vec![
            status_line,
            Line::from(footer_for_state(state, tui_state)),
        ]),
        layout.footer,
    );
}

fn provider_is_metered(state: &deadreckon_core::PipelineState) -> bool {
    !state
        .provider
        .as_deref()
        .is_some_and(|provider| provider.starts_with("cli:") || provider.starts_with("import:"))
}

#[cfg(test)]
fn attach_header_text(state: &deadreckon_core::PipelineState, width: u16) -> String {
    attach_header_text_for_state(state, width, None)
}

fn attach_header_text_for_state(
    state: &deadreckon_core::PipelineState,
    width: u16,
    parent_plan: Option<&AttachParentPlan>,
) -> String {
    let path_label = if state.promoted_library_dir.is_some() {
        "artifact"
    } else {
        "working"
    };
    let chain_prefix = chain_context_line_for_working(&state.working_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let plan_prefix = parent_plan
        .map(|parent| format!("plan {} / {}", run_prefix(&parent.plan_id), parent.task_id))
        .unwrap_or_default();
    let context = [plan_prefix, chain_prefix]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("  |  ");
    let context_prefix = if context.is_empty() {
        String::new()
    } else {
        format!("{context}  |  ")
    };
    let usable_width = usize::from(width).saturating_sub(4);
    let run_line = one_line(
        &format!(
            "{}run {}  provider {}  sandbox {}",
            context_prefix,
            run_prefix(&state.run_id),
            state.provider.as_deref().unwrap_or("-"),
            state.sandbox
        ),
        usable_width,
    );
    let goal_line = one_line(&format!("goal {}", state.goal), usable_width);
    let path_line = one_line(
        &format!("{} {}", path_label, state.working_dir.display()),
        usable_width,
    );
    format!("{run_line}\n{goal_line}\n{path_line}")
}

fn footer_for_state(state: &deadreckon_core::PipelineState, tui_state: &AttachTuiState) -> String {
    let chain_suffix = if read_chain_step_marker(&state.working_dir)
        .ok()
        .flatten()
        .is_some()
    {
        "  [c] Chain"
    } else {
        ""
    };
    if state.run_root.join("abandoned.json").exists() {
        let footer = format!(
            "worktree cleaned or abandoned  |  q detach  |  deadreckon status/list{chain_suffix}"
        );
        return parent_plan_footer(footer, tui_state.parent_plan.as_ref());
    }
    let base = if tui_state.show_completion_actions && state.status == RunStatus::Completed {
        if is_worktree_run(state) {
            if tui_state.docs_open {
                "[d] Activity  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else {
                "[d] Docs  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        } else {
            if tui_state.docs_open {
                "[d] Activity  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else {
                "[d] Docs  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        }
    } else {
        "Detach: q Esc Ctrl-D  |  Focus: Tab  |  Scroll: j/k Up/Down PgUp/PgDn mouse".to_string()
    };
    parent_plan_footer(
        format!("{base}{chain_suffix}"),
        tui_state.parent_plan.as_ref(),
    )
}

fn parent_plan_footer(footer: String, parent_plan: Option<&AttachParentPlan>) -> String {
    let Some(parent_plan) = parent_plan else {
        return footer;
    };
    let mut footer = footer
        .replace(
            "q/Esc/Ctrl-D detach",
            "b/Backspace/q/Esc/Ctrl-D back to plan",
        )
        .replace(
            "Detach: q Esc Ctrl-D",
            "Back to plan: b Backspace q Esc Ctrl-D",
        )
        .replace("q detach", "b/Backspace/q back to plan");
    footer.push_str(&format!(
        "  |  parent plan {} {}",
        run_prefix(&parent_plan.plan_id),
        parent_plan.task_id
    ));
    footer
}

fn deadreckoning_status_line(
    state: &deadreckon_core::PipelineState,
    turn_label: &str,
    width: u16,
    tick: usize,
) -> Line<'static> {
    let text = deadreckoning_status_text(state, turn_label, width, tick);
    let split = text.find("  ").unwrap_or(text.len());
    let (prefix, rest) = text.split_at(split);
    Line::from(vec![
        Span::styled(
            prefix.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(rest.to_string(), Style::default().fg(Color::Blue)),
    ])
}

fn deadreckoning_status_text(
    state: &deadreckon_core::PipelineState,
    turn_label: &str,
    width: u16,
    tick: usize,
) -> String {
    let status = run_status_label(state.status);
    let phase = state
        .active_phase()
        .map(|phase| phase.name.as_str())
        .unwrap_or("phase");
    let prefix = format!("deadreckoning {status}  turn {turn_label}  {phase}  ");
    let max_width = usize::from(width).saturating_sub(1);
    let course_width = max_width.saturating_sub(prefix.chars().count()).max(8);
    let mut text = format!("{prefix}{}", deadreckoning_course_ascii(course_width, tick));
    if max_width > 0 {
        text = truncate_text(&text, max_width);
    }
    text
}

fn render_spend(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
) {
    let cap = state.max_spend_usd.unwrap_or({
        if state.total_spend_usd <= 0.0 {
            1.0
        } else {
            state.total_spend_usd
        }
    });
    let spend_ratio = (state.total_spend_usd / cap).clamp(0.0, 1.0);
    let title = if spend_ratio >= 0.6 {
        format!("{:.0}% of budget", spend_ratio * 100.0)
    } else {
        "spend".to_string()
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(title))
            .gauge_style(Style::default().fg(meter_color(spend_ratio, state)))
            .ratio(spend_ratio)
            .label(spend_meter_label(state, cap, spend_ratio)),
        area,
    );
}

fn spend_meter_label(state: &deadreckon_core::PipelineState, cap: f64, spend_ratio: f64) -> String {
    let base = format!("${:.6} / ${:.6}", state.total_spend_usd, cap);
    if spend_ratio >= 0.6 {
        format!("{base}  ({:.0}% of budget)", spend_ratio * 100.0)
    } else {
        base
    }
}

fn render_context(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    spend: &[SpendRecord],
    live: &AttachLive,
) {
    let (token_total, context_window) = context_totals(spend, live);
    let context_ratio = if context_window == 0 {
        0.0
    } else {
        token_total as f64 / context_window as f64
    };
    let detail = if token_total == 0 {
        format!(
            "waiting for telemetry\n{} window",
            format_count(context_window)
        )
    } else if context_ratio >= 1.0 {
        format!(
            "{} used\n{} window\n{context_ratio:.1}x cumulative",
            format_count(token_total),
            format_count(context_window)
        )
    } else {
        format!(
            "{} used\n{} window\n{:.0}% of window",
            format_count(token_total),
            format_count(context_window),
            context_ratio * 100.0
        )
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("context"))
            .style(Style::default().fg(threshold_color(context_ratio.clamp(0.0, 1.0))))
            .alignment(Alignment::Center),
        area,
    );
}

fn render_acceptance(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
) {
    let acceptance = &live.acceptance;
    let color = acceptance_color(acceptance.status);
    let latest = acceptance
        .latest_detail
        .as_deref()
        .map(|detail| one_line(detail, usize::from(area.width).saturating_sub(4)));
    let detail = match acceptance.status {
        AcceptanceUiStatus::DefaultGate => {
            "default gate\ninferred checks\nno project spec".to_string()
        }
        AcceptanceUiStatus::Configured => format!(
            "configured\n{} checks\n{}",
            acceptance.total,
            latest.as_deref().unwrap_or("waiting for verify")
        ),
        AcceptanceUiStatus::Running => format!(
            "running\n{} / {} checked\n{}",
            acceptance.completed,
            acceptance.total,
            latest.as_deref().unwrap_or("checking criteria")
        ),
        AcceptanceUiStatus::Passed => format!(
            "passed\n{} / {} checks\n{}",
            acceptance.passed,
            acceptance.total,
            latest.as_deref().unwrap_or("dr-gate accepted")
        ),
        AcceptanceUiStatus::Failed => format!(
            "failed\n{} pass  {} fail\n{}",
            acceptance.passed,
            acceptance.failed,
            latest.as_deref().unwrap_or("required check failed")
        ),
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("acceptance"))
            .style(Style::default().fg(color))
            .alignment(Alignment::Center),
        area,
    );
}

fn acceptance_color(status: AcceptanceUiStatus) -> Color {
    match status {
        AcceptanceUiStatus::DefaultGate => ui::TUI_PALETTE.acceptance_default,
        AcceptanceUiStatus::Configured => ui::TUI_PALETTE.acceptance_configured,
        AcceptanceUiStatus::Running => ui::TUI_PALETTE.acceptance_running,
        AcceptanceUiStatus::Passed => ui::TUI_PALETTE.acceptance_passed,
        AcceptanceUiStatus::Failed => ui::TUI_PALETTE.acceptance_failed,
    }
}

fn context_totals(spend: &[SpendRecord], live: &AttachLive) -> (u64, u64) {
    let token_total = live.provider_context_tokens.unwrap_or_else(|| {
        spend
            .iter()
            .map(|record| record.input_tokens + record.output_tokens)
            .sum::<u64>()
    });
    let context_window = live.provider_context_window.unwrap_or(200_000).max(1);
    (token_total, context_window)
}

fn render_turn_summary(spend: &[SpendRecord], show_cost: bool) -> Vec<String> {
    if spend.is_empty() {
        vec!["provider turn in progress; results land when the provider exits".to_string()]
    } else {
        spend
            .iter()
            .rev()
            .take(3)
            .map(|record| {
                let tokens = record.input_tokens + record.output_tokens;
                if show_cost {
                    format!(
                        "turn {}  {}  {} tokens  ${:.6}",
                        record.turn, record.model, tokens, record.cost_usd
                    )
                } else if let Some(seconds) = record.wall_time_seconds {
                    format!(
                        "turn {}  {}  {} tokens  {:.0}s wall",
                        record.turn,
                        record.model,
                        tokens,
                        seconds.max(0.0)
                    )
                } else {
                    format!("turn {}  {}  {} tokens", record.turn, record.model, tokens)
                }
            })
            .collect()
    }
}

fn attach_activity_lines_for_tui(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(notice) = tui_state.post_action_notice.as_ref() {
        lines.extend(notice.lines());
        lines.push(String::new());
    }
    lines.extend(attach_activity_lines(state, spend, traces, events, live));
    lines
}

fn attach_activity_lines(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
) -> Vec<String> {
    let metered_provider = provider_is_metered(state);
    let mut lines = render_turn_summary(spend, metered_provider);
    lines.extend(acceptance_activity_lines(&live.acceptance));
    if state.status == RunStatus::Executing && live.file_count > 0 {
        lines.push(format!(
            "live working tree: {} files, latest changes visible before provider exit",
            live.file_count
        ));
    }
    lines.extend(live.provider_activity.iter().rev().cloned());
    lines.extend(
        events
            .iter()
            .rev()
            .map(|event| event_line(event, metered_provider)),
    );
    lines.extend(traces.iter().rev().map(|record| {
        format!(
            "trace turn {}  {}  {:?}ms",
            record.turn, record.event, record.latency_ms
        )
    }));
    lines
}

fn acceptance_activity_lines(acceptance: &AcceptanceLive) -> Vec<String> {
    match acceptance.status {
        AcceptanceUiStatus::DefaultGate | AcceptanceUiStatus::Configured => Vec::new(),
        AcceptanceUiStatus::Running => {
            let mut lines = vec![format!(
                "acceptance running: {} / {} checked",
                acceptance.completed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().cloned());
            lines.push(String::new());
            lines
        }
        AcceptanceUiStatus::Passed => {
            let mut lines = vec![format!(
                "acceptance passed: {} / {} checks",
                acceptance.passed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().take(4).cloned());
            lines.push(String::new());
            lines
        }
        AcceptanceUiStatus::Failed => {
            let mut lines = vec![format!(
                "acceptance failed: {} required failures, {} / {} passed",
                acceptance.required_failed, acceptance.passed, acceptance.total
            )];
            lines.extend(acceptance.progress_lines.iter().cloned());
            lines.push(String::new());
            lines
        }
    }
}

fn render_run_docs(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
    tui_state: &AttachTuiState,
) {
    let lines = render_markdown_doc_lines(state);
    let rows = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(lines.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "run docs / narrative",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.docs_scroll,
                        rows,
                        lines.len(),
                    )),
            )
            .wrap(Wrap { trim: false })
            .scroll((tui_state.docs_scroll as u16, 0)),
        area,
    );
}

fn render_markdown_doc_lines(state: &deadreckon_core::PipelineState) -> Vec<Line<'static>> {
    let Some(path) = doc_path_for_kind(&state.working_dir, DocKind::Narrative) else {
        return vec![Line::styled(
            "No narrative docs found for this run.",
            Style::default().fg(Color::Yellow),
        )];
    };
    match fs::read_to_string(&path) {
        Ok(raw) => markdown_to_tui_lines(&raw),
        Err(err) => vec![Line::styled(
            format!("Unable to read {}: {err}", path.display()),
            Style::default().fg(Color::Red),
        )],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlock {
    Paragraph,
    Heading(HeadingLevel),
    Item,
}

fn markdown_to_tui_lines(markdown: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let parser = MarkdownParser::new_ext(markdown, options);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut block: Option<MarkdownBlock> = None;
    let mut inline_style = Style::default();
    let mut code_block = false;

    for event in parser {
        match event {
            MarkdownEvent::Start(Tag::Heading { level, .. }) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Heading(level));
            }
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::End(TagEnd::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                current.push(Span::styled("  - ", Style::default().fg(Color::Cyan)));
                block = Some(MarkdownBlock::Item);
            }
            MarkdownEvent::End(TagEnd::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
            }
            MarkdownEvent::Start(Tag::CodeBlock(kind)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                let language = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        format!(" {}", language)
                    }
                    _ => String::new(),
                };
                lines.push(Line::styled(
                    format!("```{language}"),
                    Style::default().fg(Color::DarkGray),
                ));
                code_block = true;
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                code_block = false;
                lines.push(Line::styled("```", Style::default().fg(Color::DarkGray)));
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Strong) => {
                inline_style = inline_style.add_modifier(Modifier::BOLD);
            }
            MarkdownEvent::End(TagEnd::Strong) => {
                inline_style = inline_style.remove_modifier(Modifier::BOLD);
            }
            MarkdownEvent::Start(Tag::Emphasis) => {
                inline_style = inline_style.add_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => {
                inline_style = inline_style.remove_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                inline_style = inline_style
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED);
                if !dest_url.is_empty() {
                    current.push(Span::styled(
                        "",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                }
            }
            MarkdownEvent::End(TagEnd::Link) => {
                inline_style = Style::default();
            }
            MarkdownEvent::Text(text) => {
                if code_block {
                    for line in text.lines() {
                        lines.push(Line::styled(
                            format!("  {line}"),
                            Style::default().fg(Color::LightGreen),
                        ));
                    }
                } else {
                    current.push(Span::styled(text.into_string(), inline_style));
                }
            }
            MarkdownEvent::Code(code) => current.push(Span::styled(
                code.into_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            MarkdownEvent::SoftBreak => current.push(Span::raw(" ")),
            MarkdownEvent::HardBreak => {
                flush_markdown_line(&mut lines, &mut current, block);
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::Rule => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    "────────────────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => current.push(
                Span::styled(html.into_string(), Style::default().fg(Color::DarkGray)),
            ),
            MarkdownEvent::InlineMath(math) => current.push(Span::styled(
                math.into_string(),
                Style::default().fg(Color::Magenta),
            )),
            MarkdownEvent::DisplayMath(math) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    math.into_string(),
                    Style::default().fg(Color::Magenta),
                ));
            }
            MarkdownEvent::Start(_)
            | MarkdownEvent::End(_)
            | MarkdownEvent::FootnoteReference(_) => {}
            MarkdownEvent::TaskListMarker(checked) => current.push(Span::styled(
                if checked { "[x] " } else { "[ ] " },
                Style::default().fg(Color::Cyan),
            )),
        }
    }
    flush_markdown_line(&mut lines, &mut current, block.take());
    if lines.is_empty() {
        lines.push(Line::styled(
            "Narrative docs are empty.",
            Style::default().fg(Color::Yellow),
        ));
    }
    lines
}

fn flush_markdown_line(
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    block: Option<MarkdownBlock>,
) {
    if current.is_empty() {
        return;
    }
    let style = match block.unwrap_or(MarkdownBlock::Paragraph) {
        MarkdownBlock::Heading(HeadingLevel::H1) => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(HeadingLevel::H2) => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(_) => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Item => Style::default().fg(Color::White),
        MarkdownBlock::Paragraph => Style::default(),
    };
    let mut spans = Vec::new();
    if matches!(block, Some(MarkdownBlock::Heading(level)) if level != HeadingLevel::H1) {
        spans.push(Span::styled("▸ ", Style::default().fg(Color::Cyan)));
    }
    spans.extend(current.drain(..).map(|span| span.patch_style(style)));
    lines.push(Line::from(spans));
}

fn render_live_files(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = live_file_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.files_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Files,
                ))
                .title(panel_title(
                    &format!(
                        "live files  {} files  {}",
                        live.file_count,
                        format_bytes(live.total_bytes)
                    ),
                    tui_state.focused_panel == AttachPanel::Files,
                    tui_state.files_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn live_file_lines(live: &AttachLive) -> Vec<String> {
    if !live.working_dir_exists {
        return vec!["working tree was removed after cleanup".to_string()];
    }
    if live.files.is_empty() {
        return vec!["no files yet".to_string()];
    }
    let mut lines = Vec::new();
    lines.extend(live.files.iter().map(|file| {
        format!(
            "{:>7} {:>8}  {}",
            format_age(file.modified_at),
            format_bytes(file.bytes),
            file.path
        )
    }));
    lines
}

fn render_processes(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = process_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.processes_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Processes,
                ))
                .title(panel_title(
                    "processes",
                    tui_state.focused_panel == AttachPanel::Processes,
                    tui_state.processes_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn process_lines(live: &AttachLive) -> Vec<String> {
    if live.pids.is_empty() {
        vec!["no supervised pids".to_string()]
    } else {
        live.pids
            .iter()
            .map(|pid| {
                let status = if pid.alive { "alive" } else { "dead" };
                format!("{} {} {}", pid.pid, status, pid.command)
            })
            .collect()
    }
}

fn visible_items(lines: &[String], offset: usize, rows: usize) -> Vec<ListItem<'static>> {
    lines
        .iter()
        .skip(offset.min(lines.len()))
        .take(rows)
        .map(|line| ListItem::new(line.clone()))
        .collect()
}

fn panel_border_style(focused: AttachPanel, panel: AttachPanel) -> Style {
    if focused == panel {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

fn panel_title(title: &str, focused: bool, offset: usize, rows: usize, total: usize) -> String {
    let marker = if focused { "*" } else { " " };
    if total <= rows || total == 0 {
        format!("{marker}{title}")
    } else {
        let first = offset.saturating_add(1).min(total);
        let last = offset.saturating_add(rows).min(total);
        format!("{marker}{title} {first}-{last}/{total}")
    }
}

fn event_line(event: &RunEvent, show_cost: bool) -> String {
    match &event.event {
        deadreckon_core::RunEventKind::TurnStarted { turn } => {
            format!("turn {turn} started")
        }
        deadreckon_core::RunEventKind::ToolCallStarted {
            turn,
            tool_call_id,
            tool_name,
            ..
        } => format!("turn {turn} {tool_name} {tool_call_id} started"),
        deadreckon_core::RunEventKind::ToolCallResult {
            turn,
            tool_call_id,
            status,
            preview,
        } => format!("turn {turn} {tool_call_id} {status} {preview}"),
        deadreckon_core::RunEventKind::TokenUsageDelta {
            turn,
            input_tokens,
            output_tokens,
        } => format!("turn {turn} tokens +{}", input_tokens + output_tokens),
        deadreckon_core::RunEventKind::SpendDelta {
            turn,
            cost_usd,
            wall_time_seconds,
            ..
        } => {
            if show_cost {
                format!(
                    "turn {turn} spend +${cost_usd:.6} wall {}s",
                    wall_time_seconds.unwrap_or(0.0)
                )
            } else {
                format!("turn {turn} wall {}s", wall_time_seconds.unwrap_or(0.0))
            }
        }
        deadreckon_core::RunEventKind::DocsCheckpoint { turn, path, status } => {
            format!("turn {turn} docs {status} {}", path.display())
        }
        deadreckon_core::RunEventKind::RunCompleted { status } => {
            format!("run {status}")
        }
        deadreckon_core::RunEventKind::RunPromoted { library_dir } => {
            format!("promoted {}", library_dir.display())
        }
        deadreckon_core::RunEventKind::Error { turn, message } => {
            format!("turn {} error {message}", turn.unwrap_or_default())
        }
    }
}

fn turn_timer(
    events: &[RunEvent],
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    state: &deadreckon_core::PipelineState,
) -> String {
    let Some(started) = events.iter().rev().find(|event| {
        matches!(
            event.event,
            deadreckon_core::RunEventKind::TurnStarted { .. }
        )
    }) else {
        return "-".to_string();
    };
    if state.status == RunStatus::Executing {
        let elapsed = Utc::now()
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s running");
    }
    if let Some(done_at) = events.iter().rev().find_map(|event| match event.event {
        deadreckon_core::RunEventKind::RunCompleted { .. } => Some(event.timestamp),
        _ => None,
    }) {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    if let Some(seconds) = spend
        .iter()
        .rev()
        .find_map(|record| record.wall_time_seconds)
    {
        return format!("{:.0}s done", seconds.max(0.0));
    }
    if let Some(ms) = traces.iter().rev().find_map(|record| record.latency_ms) {
        return format!("{:.0}s done", ms as f64 / 1000.0);
    }
    if let Some(done_at) = events
        .iter()
        .rev()
        .find(|event| {
            !matches!(
                event.event,
                deadreckon_core::RunEventKind::TurnStarted { .. }
            )
        })
        .map(|event| event.timestamp)
    {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    "done".to_string()
}

fn meter_color(ratio: f64, state: &deadreckon_core::PipelineState) -> Color {
    if state.pause_reason.as_deref() == Some("spend cap reached") {
        ui::TUI_PALETTE.spend_pause_cap
    } else {
        threshold_color(ratio)
    }
}

fn threshold_color(ratio: f64) -> Color {
    if ratio >= 0.8 {
        ui::TUI_PALETTE.spend_high
    } else if ratio >= 0.6 {
        ui::TUI_PALETTE.spend_mid
    } else {
        ui::TUI_PALETTE.spend_low
    }
}

fn format_age(modified_at: Option<DateTime<Utc>>) -> String {
    let Some(modified_at) = modified_at else {
        return "-".to_string();
    };
    let seconds = Utc::now()
        .signed_duration_since(modified_at)
        .num_seconds()
        .max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let shortened = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}

fn read_jsonl<T: DeserializeOwned>(path: &std::path::Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect::<Vec<T>>())
}

fn read_plan_events_lossy(paths: &DeadreckonPaths, plan_id: &str) -> Vec<PlanEvent> {
    read_jsonl::<PlanEvent>(&paths.plan_events(plan_id)).unwrap_or_default()
}

#[cfg(test)]
mod flight_cli_tests {
    use super::*;
    use deadreckon_core::flight::{
        CheckpointBase, CheckpointBaseKind, CheckpointCaptureRequest, CheckpointTrigger,
        FlightManifest, FlightSession, FlightUsage, append_flight_event, capture_delta_checkpoint,
        write_flight_manifest,
    };
    use tempfile::TempDir;

    fn checkpoint_fixture() -> (TempDir, deadreckon_core::PipelineState) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "flight rewind".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        deadreckon_core::snapshot_working(&state, 0).expect("snapshot");
        (temp, state)
    }

    fn write_manifest(state: &deadreckon_core::PipelineState, status: FlightSessionStatus) {
        let mut manifest = FlightManifest::new(state.run_id.clone());
        manifest.sessions.push(FlightSession {
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            provider: "cli:test".to_string(),
            schema: "test".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            status,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            source_paths: Vec::new(),
        });
        write_flight_manifest(state, &manifest).expect("manifest");
    }

    fn capture_fixture_checkpoint(state: &deadreckon_core::PipelineState) {
        let before = build_working_file_index(&state.working_dir).expect("before");
        let source = state.working_dir.join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("src");
        std::fs::write(&source, "pub fn value() -> u8 { 1 }\n").expect("source");
        let after = build_working_file_index(&state.working_dir).expect("after");
        capture_delta_checkpoint(
            state,
            &before,
            &after,
            CheckpointCaptureRequest {
                checkpoint_id: "cp-000001".to_string(),
                flight_session_id: "flight-turn-1-attempt-1".to_string(),
                deadreckon_turn: 1,
                attempt: 1,
                provider_event_seq: Some(3),
                trigger: CheckpointTrigger::ProviderExit,
                base: CheckpointBase {
                    kind: CheckpointBaseKind::TurnSnapshot,
                    id: "turn-0".to_string(),
                },
                full_anchor: false,
            },
        )
        .expect("checkpoint");
    }

    #[test]
    fn rewind_target_resolves_provider_event_checkpoint() {
        let (_temp, state) = checkpoint_fixture();
        write_manifest(&state, FlightSessionStatus::Completed);
        capture_fixture_checkpoint(&state);
        append_flight_event(
            &state,
            &FlightEvent {
                version: 1,
                seq: 3,
                run_id: state.run_id.clone(),
                flight_session_id: "flight-turn-1-attempt-1".to_string(),
                deadreckon_turn: 1,
                attempt: 1,
                provider: "cli:test".to_string(),
                schema: "test".to_string(),
                timestamp: Some(Utc::now()),
                source_path: None,
                source_line: None,
                source_event: "{}".to_string(),
                raw_hash: "sha256:test".to_string(),
                kind: FlightEventKind::Tool,
                role: None,
                summary: "tool".to_string(),
                tool_name: Some("write_file".to_string()),
                tool_category: None,
                files: vec![PathBuf::from("src/lib.rs")],
                usage: None,
                checkpoint_id: Some("cp-000001".to_string()),
            },
        )
        .expect("event");
        let resolved = resolve_rewind_target(
            &state,
            &RewindCliOptions {
                to_turn: None,
                to_provider_event: Some(3),
                to_checkpoint: None,
                preview: true,
                apply: false,
                json: false,
            },
        )
        .expect("target");
        assert_eq!(resolved.checkpoint_id, "cp-000001");
        assert_eq!(resolved.target.kind, RewindTargetKind::ProviderEvent);
    }

    #[test]
    fn rewind_apply_hash_guard_refuses_unrelated_file_edits() {
        let (_temp, mut state) = checkpoint_fixture();
        write_manifest(&state, FlightSessionStatus::Completed);
        capture_fixture_checkpoint(&state);
        state.turn = 1;
        deadreckon_core::snapshot_working(&state, 1).expect("snapshot");
        let target_dir = state.run_root.join("rewind-preview/cp-000001-test");
        materialize_checkpoint(&state, "cp-000001", &target_dir).expect("materialize");
        std::fs::write(state.working_dir.join("src/lib.rs"), "user edit\n").expect("edit");
        let result = hash_guard_rewind_apply(&state, &target_dir, &[PathBuf::from("src/lib.rs")]);
        assert!(result.is_err());
        assert!(result.expect_err("refusal").contains("unrelated edits"));
    }

    #[test]
    fn attach_provider_activity_uses_flight_events() {
        let (_temp, state) = checkpoint_fixture();
        append_flight_event(
            &state,
            &FlightEvent {
                version: 1,
                seq: 1,
                run_id: state.run_id.clone(),
                flight_session_id: "flight-turn-1-attempt-1".to_string(),
                deadreckon_turn: 1,
                attempt: 1,
                provider: "cli:test".to_string(),
                schema: "test".to_string(),
                timestamp: Some(Utc::now()),
                source_path: None,
                source_line: None,
                source_event: "{}".to_string(),
                raw_hash: "sha256:test".to_string(),
                kind: FlightEventKind::Tool,
                role: None,
                summary: "edited src/lib.rs".to_string(),
                tool_name: Some("write_file".to_string()),
                tool_category: None,
                files: vec![PathBuf::from("src/lib.rs")],
                usage: Some(FlightUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    context_window: Some(100),
                }),
                checkpoint_id: Some("cp-000001".to_string()),
            },
        )
        .expect("event");
        let activity = collect_provider_activity(&state);
        assert!(activity.lines.join("\n").contains("flight #000001"));
        assert_eq!(activity.context_tokens, Some(15));
        assert_eq!(activity.context_window, Some(100));
    }
}

#[cfg(test)]
mod tui_tests {
    use std::io::Write;

    use super::{
        AcceptanceLive, AcceptanceUiStatus, AttachActionNotice, AttachLive, AttachPanel,
        AttachPanelCounts, AttachPanelRows, AttachParentPlan, AttachTuiState, COMMAND_HELP_CATALOG,
        ChainAttachTuiState, CommandDiscovery, CommandHelpEntry, CompletionAction, HELP_ALL_GROUPS,
        PlanAttachRenderState, PlanFeedEvent, ProviderActivity, ProviderJsonlLogSpec, TopHelpGroup,
        acceptance_activity_lines, attach_banner, attach_header_text, attach_should_return_to_plan,
        chain_activity_lines, chain_attach_footer_text, chain_attach_header_text,
        chain_should_auto_attach, chain_step_dot, chain_timeline_lines, chain_wall_cap_hit,
        claude_project_name_for_workdir, cli_wait_status_line, collect_jsonl_provider_activity,
        command_discovery, completion_action_from_input, completion_hints_enabled,
        deadreckoning_course_ascii, deadreckoning_status_text, doc_polish_preview_text,
        implementation_plan_warnings, kill_banner, live_file_lines, markdown_to_tui_lines,
        max_panel_scroll, meter_color, orchestration_dependency_rows,
        orchestration_parallelism_lines, orchestration_provider_role_rows,
        orchestration_role_table_lines, per_step_wall_cap, plan_attach_footer,
        plan_merge_repair_summary_items, provider_ingest_base_roots, provider_jsonl_activity_lines,
        provider_jsonl_log_spec_from_registry, provider_jsonl_session_matches_run,
        read_plan_events_lossy, recommend_child_count_for_goal, recommend_orchestration_mode,
        render_attach, render_plan_attach, threshold_color,
    };
    use crate::cli::{Cli, CliPlanMode};
    use chrono::Utc;
    use clap::CommandFactory;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use deadreckon_core::{
        ApplyMode, ApplyStrategy, BranchPolicy, CapabilityPreview, Chain, ChainEvent,
        ChainEventKind, ChainNewOptions, ChainStatus, ChainStepStatus, DeadreckonPaths,
        NetworkCapability, OnFail, Plan, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind,
        PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, RunEvent,
        RunEventKind, RunOptions, SpendRecord, append_plan_event, create_run, save_plan,
    };
    use deadreckon_providers::SpendEstimate;
    use deadreckon_providers::registry::{
        IngestCwdMatch, IngestDescriptor, IngestStorage, ProviderRegistry,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Line;

    fn chain_fixture() -> Chain {
        let mut chain = Chain::new(ChainNewOptions {
            root_goal: "build app".to_string(),
            goals: vec!["first step".to_string(), "second step".to_string()],
            scope: "scope".to_string(),
            base_branch: "main".to_string(),
            base_sha: "abcdef123456".to_string(),
            cwd: std::path::PathBuf::from("/tmp/project"),
            provider: Some("smoke".to_string()),
            model: None,
            sandbox: "none".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Auto,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 2,
            max_spend_usd: Some(5.0),
            max_wall_seconds: Some(600.0),
            deadreckon_version: "0.1.0".to_string(),
        })
        .expect("chain");
        chain.steps[0].status = ChainStepStatus::Applied;
        chain.steps[0].run_id = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
        chain.steps[1].status = ChainStepStatus::Running;
        chain
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    fn test_tempdir() -> tempfile::TempDir {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
        std::fs::create_dir_all(&root).expect("test tmp");
        tempfile::TempDir::new_in(root).expect("temp")
    }

    #[test]
    fn command_help_catalog_rows_are_unique() {
        let mut top_rows = std::collections::BTreeSet::new();
        let mut help_all_rows = std::collections::BTreeSet::new();

        for entry in COMMAND_HELP_CATALOG {
            if entry.top_group.is_some() {
                assert!(
                    top_rows.insert(entry.display),
                    "duplicate top-help row for {}",
                    entry.display
                );
            }
            if entry.all_group.is_some() {
                assert!(
                    help_all_rows.insert(entry.display),
                    "duplicate help-all row for {}",
                    entry.display
                );
            }
        }

        assert!(top_rows.contains("help-all"));
        assert!(top_rows.contains("<command> --help"));
        assert!(help_all_rows.contains("export"));
        assert!(!help_all_rows.contains("materialize"));
    }

    #[test]
    fn command_help_catalog_points_at_real_clap_commands() {
        let handle = std::thread::Builder::new()
            .name("command-help-catalog-clap-tree".to_string())
            // Clap's generated command tree is large enough to overflow the
            // default libtest worker stack on macOS.
            .stack_size(8 * 1024 * 1024)
            .spawn(assert_command_help_catalog_points_at_real_clap_commands)
            .expect("spawn clap command tree test");
        if let Err(payload) = handle.join() {
            std::panic::resume_unwind(payload);
        }
    }

    fn assert_command_help_catalog_points_at_real_clap_commands() {
        let clap_names = Cli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect::<std::collections::BTreeSet<_>>();

        for CommandHelpEntry {
            display, clap_name, ..
        } in COMMAND_HELP_CATALOG
        {
            let Some(clap_name) = clap_name else {
                continue;
            };
            assert!(
                clap_names.contains(*clap_name),
                "catalog row {display} points at missing clap command {clap_name}"
            );
        }
    }

    #[test]
    fn command_help_catalog_covers_expected_sections() {
        let mut top_groups = std::collections::BTreeSet::new();
        let mut all_groups = std::collections::BTreeSet::new();
        for entry in COMMAND_HELP_CATALOG {
            if let Some(group) = entry.top_group {
                top_groups.insert(format!("{group:?}"));
            }
            if let Some(group) = entry.all_group {
                all_groups.insert(format!("{group:?}"));
            }
        }

        for group in [
            TopHelpGroup::CoreLifecycle,
            TopHelpGroup::ContinueRecover,
            TopHelpGroup::MoreHelp,
        ] {
            assert!(top_groups.contains(&format!("{group:?}")));
        }
        for (group, _) in HELP_ALL_GROUPS {
            assert!(all_groups.contains(&format!("{group:?}")));
        }
    }

    #[test]
    fn command_help_catalog_classifies_advanced_and_compatibility_surfaces() {
        let entry = |name: &str| {
            COMMAND_HELP_CATALOG
                .iter()
                .find(|entry| entry.display == name)
                .unwrap_or_else(|| panic!("missing catalog row {name}"))
        };

        for name in ["apply", "export", "abandon", "doc", "show"] {
            assert_eq!(command_discovery(entry(name)), CommandDiscovery::Advanced);
        }
        assert_eq!(
            command_discovery(entry("acceptance")),
            CommandDiscovery::Compatibility
        );
        assert_eq!(command_discovery(entry("run")), CommandDiscovery::Public);
        assert!(
            COMMAND_HELP_CATALOG
                .iter()
                .all(|entry| entry.display != "materialize"),
            "materialize must stay an inline compatibility alias, not a catalog row"
        );
    }

    #[test]
    fn attach_banner_names_kind_and_prefix() {
        let id = "aaaabbbbccccdddd1111222233334444";
        assert_eq!(attach_banner("run", id), "attaching to run aaaabbbb");
        assert_eq!(attach_banner("chain", id), "attaching to chain aaaabbbb");
        assert_eq!(attach_banner("plan", id), "attaching to plan aaaabbbb");
    }

    #[test]
    fn kill_banner_names_kind_prefix_and_plan_process_count() {
        assert_eq!(
            kill_banner("run", "aaaabbbb", false, None),
            "killed run aaaabbbb"
        );
        assert_eq!(
            kill_banner("chain", "aaaabbbb", true, None),
            "killed chain aaaabbbb forcefully"
        );
        assert_eq!(
            kill_banner("plan", "aaaabbbb", true, Some(3)),
            "killed plan aaaabbbb forcefully (3 processes signalled)"
        );
    }

    fn doc_preview_state() -> (tempfile::TempDir, deadreckon_core::PipelineState) {
        let temp = test_tempdir();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "preview docs".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("cli:codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("state");
        (temp, state)
    }

    fn full_plan_fixture(task_count: usize) -> (tempfile::TempDir, DeadreckonPaths, Plan) {
        let temp = test_tempdir();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let tasks = (0..task_count)
            .map(|index| {
                PlanTask::new(
                    index as u32,
                    format!("Task {index}"),
                    format!("Do child {index}"),
                    PlanRole::Child,
                    Some(if index == 1 {
                        "smoke:reviewer".to_string()
                    } else {
                        "smoke:child".to_string()
                    }),
                )
            })
            .collect::<Vec<_>>();
        let mut plan = Plan::new(
            "build orchestrated app",
            PlanMode::FullPlan,
            tasks,
            PlanProviders {
                planner: Some("smoke:planner".to_string()),
                default_child: Some("smoke:child".to_string()),
                coder: None,
                reviewer: None,
                children: [(1, "smoke:reviewer".to_string())].into(),
            },
            Some("scope".to_string()),
            "0.1.0",
        )
        .expect("plan");
        plan.capability_preview = CapabilityPreview {
            network: NetworkCapability::Allowlist,
            deploy: true,
            global_install: false,
            filesystem: vec!["working directory".to_string()],
            notes: Vec::new(),
        };
        (temp, paths, plan)
    }

    fn review_plan_fixture() -> (tempfile::TempDir, DeadreckonPaths, Plan) {
        let temp = test_tempdir();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let coder = PlanTask::new(
            0,
            "Implement requested change",
            "Build the change",
            PlanRole::Coder,
            Some("smoke:coder".to_string()),
        );
        let mut reviewer = PlanTask::new(
            1,
            "Review and fix implementation",
            "Review the coder result",
            PlanRole::Reviewer,
            Some("smoke:reviewer".to_string()),
        );
        reviewer.depends_on = vec![coder.task_id.clone()];
        let plan = Plan::new(
            "tiny hello rust",
            PlanMode::Review,
            vec![coder, reviewer],
            PlanProviders {
                planner: None,
                default_child: None,
                coder: Some("smoke:coder".to_string()),
                reviewer: Some("smoke:reviewer".to_string()),
                children: Default::default(),
            },
            Some("scope".to_string()),
            "0.1.0",
        )
        .expect("plan");
        (temp, paths, plan)
    }

    fn render_plan_attach_text(
        paths: &DeadreckonPaths,
        plan: &Plan,
        messages: &[PlanMessage],
        plan_events: &[PlanEvent],
        selected: usize,
    ) -> String {
        render_plan_attach_text_with_feed(paths, plan, messages, plan_events, &[], selected)
    }

    fn render_plan_attach_text_with_feed(
        paths: &DeadreckonPaths,
        plan: &Plan,
        messages: &[PlanMessage],
        plan_events: &[PlanEvent],
        feed_events: &[PlanFeedEvent],
        selected: usize,
    ) -> String {
        let backend = TestBackend::new(140, 34);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_plan_attach(
                    frame,
                    paths,
                    plan,
                    &PlanAttachRenderState {
                        messages,
                        plan_events,
                        feed_events,
                        selected,
                        show_hints: true,
                    },
                )
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        let mut text = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                text.push_str(buffer.cell((x, y)).expect("cell").symbol());
            }
            text.push('\n');
        }
        text
    }

    fn render_attach_text(
        state: &deadreckon_core::PipelineState,
        spend: &[SpendRecord],
        live: &AttachLive,
    ) -> String {
        render_attach_text_with_tui_state(state, spend, live, AttachTuiState::default())
    }

    fn render_attach_text_with_tui_state(
        state: &deadreckon_core::PipelineState,
        spend: &[SpendRecord],
        live: &AttachLive,
        tui_state: AttachTuiState,
    ) -> String {
        let backend = TestBackend::new(140, 34);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_attach(frame, state, spend, &[], &[], live, &tui_state))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        let mut text = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                text.push_str(buffer.cell((x, y)).expect("cell").symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn orchestration_mode_recommendation_prefers_full_plan_for_broad_products() {
        assert_eq!(
            recommend_orchestration_mode("make a fully multiplayer live flight simulator"),
            CliPlanMode::FullPlan
        );
        assert_eq!(
            recommend_orchestration_mode("fix the provider table spacing"),
            CliPlanMode::Review
        );
    }

    #[test]
    fn orchestration_child_count_scales_with_goal_complexity() {
        assert_eq!(
            recommend_child_count_for_goal("fix a typo", CliPlanMode::Review),
            2
        );
        assert_eq!(
            recommend_child_count_for_goal(
                "make a multiplayer realtime physics terrain game with server",
                CliPlanMode::FullPlan
            ),
            5
        );
    }

    #[test]
    fn orchestration_preflight_snapshot_captures_provider_roles_and_parallelism() {
        let (_temp, _paths, mut plan) = full_plan_fixture(3);
        plan.tasks[2].depends_on = vec!["task-0".to_string()];

        let role_lines =
            orchestration_role_table_lines(&orchestration_provider_role_rows(&plan, true, None));
        let parallelism = orchestration_parallelism_lines(&plan);

        assert!(
            role_lines
                .iter()
                .any(|line| line.contains("planner") && line.contains("smoke:planner")),
            "{role_lines:#?}"
        );
        assert!(
            role_lines
                .iter()
                .any(|line| line.contains("child task-1") && line.contains("smoke:reviewer")),
            "{role_lines:#?}"
        );
        assert!(
            role_lines
                .iter()
                .any(|line| line.contains("repair") && line.contains("smoke:planner")),
            "{role_lines:#?}"
        );
        assert!(
            parallelism
                .iter()
                .any(|line| line.contains("starts now: task-0, task-1")),
            "{parallelism:#?}"
        );
        assert!(
            parallelism
                .iter()
                .any(|line| line.contains("waits: task-2 after task-0")),
            "{parallelism:#?}"
        );
    }

    #[test]
    fn orchestrate_preflight_prints_provider_role_table() {
        let (_temp, _paths, plan) = review_plan_fixture();

        let rows = orchestration_provider_role_rows(&plan, true, Some("smoke:repair"));

        assert!(rows.iter().any(|row| {
            row.role == "coder" && row.route == "smoke:coder" && row.source == "plan"
        }));
        assert!(rows.iter().any(|row| {
            row.role == "reviewer" && row.route == "smoke:reviewer" && row.source == "plan"
        }));
        assert!(rows.iter().any(|row| {
            row.role == "repair" && row.route == "smoke:repair" && row.source == "flag"
        }));
    }

    #[test]
    fn orchestrate_preflight_names_ready_parallel_children() {
        let (_temp, _paths, mut plan) = full_plan_fixture(3);
        plan.tasks[1].depends_on = vec!["task-0".to_string()];
        plan.tasks[2].status = PlanTaskStatus::Completed;

        let rows = orchestration_dependency_rows(&plan);
        let lines = orchestration_parallelism_lines(&plan);

        assert!(rows.iter().any(|row| {
            row.child == "task-0" && row.starts == "now" && row.unblocks == "task-1"
        }));
        assert!(rows.iter().any(|row| {
            row.child == "task-1" && row.starts == "after task-0" && row.waits_for == "task-0"
        }));
        assert!(
            lines.iter().any(|line| line.contains("starts now: task-0")),
            "{lines:#?}"
        );
    }

    #[test]
    fn merge_repair_summary_snapshot_captures_current_status() {
        let (_temp, paths, mut plan) = full_plan_fixture(2);
        plan.status = PlanStatus::Failed;
        save_plan(&paths, &plan).expect("save plan");
        let proofs = paths.merge_proofs(&plan.plan_id);
        std::fs::create_dir_all(&proofs).expect("proofs");
        std::fs::write(
            proofs.join("conflicts.json"),
            serde_json::json!({
                "schema_version": 2,
                "plan_id": plan.plan_id,
                "strategy": "dag-aware",
                "conflicts": [{ "path": "src/lib.rs" }]
            })
            .to_string(),
        )
        .expect("conflicts");
        std::fs::write(
            proofs.join("repair-request.json"),
            serde_json::json!({ "provider": "smoke:repair" }).to_string(),
        )
        .expect("request");
        std::fs::write(
            proofs.join("repair-plan.json"),
            serde_json::json!({
                "decision": "spawn_repair_child",
                "rationale": "needs integration"
            })
            .to_string(),
        )
        .expect("plan");
        std::fs::write(
            proofs.join("repair-run.json"),
            serde_json::json!({
                "run_id": "11112222333344445555666677778888",
                "status": "failed"
            })
            .to_string(),
        )
        .expect("run");
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairPlanned {
                conflict_count: 1,
                provider: Some("smoke:repair".to_string()),
            },
        )
        .expect("planned");
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairStarted {
                mode: "child".to_string(),
            },
        )
        .expect("started");

        let summary = plan_merge_repair_summary_items(&paths, &plan);

        assert!(
            summary
                .iter()
                .any(|(key, value)| key == "mode" && value == "child"),
            "{summary:#?}"
        );
        assert!(
            summary
                .iter()
                .any(|(key, value)| key == "provider" && value == "smoke:repair"),
            "{summary:#?}"
        );
        assert!(
            summary
                .iter()
                .any(|(key, value)| key == "conflicts" && value.contains("src/lib.rs")),
            "{summary:#?}"
        );
        assert!(
            summary
                .iter()
                .any(|(key, value)| key == "repair run" && value.contains("11112222 failed")),
            "{summary:#?}"
        );
        assert!(
            summary.iter().any(|(key, value)| key == "next action"
                && value.contains("deadreckon show")
                && value.contains("--why-failed")),
            "{summary:#?}"
        );
    }

    #[test]
    fn preflight_warns_on_research_only_full_plan_tasks_for_build_goal() {
        let (_temp, _paths, mut plan) = full_plan_fixture(2);
        plan.root_goal = "make a fully multiplayer live flight simulator".to_string();
        plan.tasks[0].subject = "research flight sim architecture".to_string();
        plan.tasks[0].goal = "Research and document architecture options".to_string();
        plan.tasks[1].subject = "produce phased implementation roadmap".to_string();
        plan.tasks[1].goal = "Produce a roadmap document".to_string();

        let warnings = implementation_plan_warnings(&plan);

        assert_eq!(warnings.len(), 1, "{warnings:#?}");
        assert!(warnings[0].contains("task-0"), "{warnings:#?}");
        assert!(warnings[0].contains("task-1"), "{warnings:#?}");
    }

    #[test]
    fn attach_plan_shows_n_panes() {
        let (_temp, paths, plan) = full_plan_fixture(4);

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

        assert!(text.contains("task-0 pending"), "{text}");
        assert!(text.contains("task-1 pending"), "{text}");
        assert!(text.contains("task-2 pending"), "{text}");
        assert!(text.contains("task-3 pending"), "{text}");
        assert!(text.contains("children 0/0/4"), "{text}");
    }

    #[test]
    fn attach_plan_shows_provider_and_role_per_pane() {
        let (_temp, paths, plan) = review_plan_fixture();

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 1);

        assert!(
            text.contains("coder smoke:coder  reviewer smoke:reviewer"),
            "{text}"
        );
        assert!(text.contains("coder  provider smoke:coder"), "{text}");
        assert!(text.contains("reviewer  provider smoke:reviewer"), "{text}");
    }

    #[test]
    fn attach_plan_shows_task_dependency_and_message_summary() {
        let (_temp, paths, mut plan) = full_plan_fixture(2);
        plan.tasks[1].depends_on = vec!["task-0".to_string()];
        plan.tasks[1].status = PlanTaskStatus::Failed;
        plan.status = PlanStatus::Forked;
        let message = PlanMessage::new(
            "coordinator",
            "task-1",
            PlanMessageKind::Blocker,
            "task-1 waiting on task-0",
            serde_json::json!({ "dependency": "task-0" }),
        )
        .expect("message");

        let text = render_plan_attach_text(&paths, &plan, &[message], &[], 1);

        assert!(text.contains("deps task-0"), "{text}");
        assert!(
            text.contains("coordinator -> task-1 Blocker: task-1 waiting on task-0"),
            "{text}"
        );
    }

    #[test]
    fn attach_plan_prefers_plan_events_for_activity() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        let event = PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::TaskBlocked {
                task_id: "task-1".to_string(),
                task_index: 1,
                reason: "task-1 blocked by task-0".to_string(),
            },
        };

        let text = render_plan_attach_text(&paths, &plan, &[], &[event], 1);

        assert!(text.contains("plan events"), "{text}");
        assert!(
            text.contains("task-1 blocked: task-1 blocked by task-0"),
            "{text}"
        );
    }

    #[test]
    fn plan_attach_tails_plan_events_without_restart() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        save_plan(&paths, &plan).expect("save plan");
        append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)
            .expect("append started");

        let text = render_plan_attach_text(
            &paths,
            &plan,
            &[],
            &read_plan_events_lossy(&paths, &plan.plan_id),
            0,
        );
        assert!(text.contains("plan started"), "{text}");

        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::TaskBlocked {
                task_id: "task-1".to_string(),
                task_index: 1,
                reason: "later event".to_string(),
            },
        )
        .expect("append blocked");
        let text = render_plan_attach_text(
            &paths,
            &plan,
            &[],
            &read_plan_events_lossy(&paths, &plan.plan_id),
            0,
        );

        assert!(text.contains("task-1 blocked: later event"), "{text}");
    }

    #[test]
    fn plan_attach_activity_prefers_plan_events_over_messages() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        let message = PlanMessage::new(
            "coordinator",
            "task-1",
            PlanMessageKind::Blocker,
            "message-only blocker",
            serde_json::json!({}),
        )
        .expect("message");
        let event = PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::TaskBlocked {
                task_id: "task-1".to_string(),
                task_index: 1,
                reason: "event blocker".to_string(),
            },
        };

        let text = render_plan_attach_text(&paths, &plan, &[message], &[event], 1);

        assert!(text.contains("plan events"), "{text}");
        assert!(text.contains("event blocker"), "{text}");
        assert!(!text.contains("message-only blocker"), "{text}");
    }

    #[test]
    fn attach_plan_receives_live_plan_child_and_repair_events() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        let child_run_id = "11112222333344445555666677778888".to_string();
        let repair_run_id = "99998888777766665555444433332222".to_string();
        let feed_events = vec![
            PlanFeedEvent::Plan {
                event: PlanEvent {
                    timestamp: Utc::now(),
                    plan_id: plan.plan_id.clone(),
                    event: PlanEventKind::PlanStarted,
                },
            },
            PlanFeedEvent::ChildRun {
                task_id: "task-0".to_string(),
                run_id: child_run_id.clone(),
                event: RunEvent {
                    timestamp: Utc::now(),
                    run_id: child_run_id,
                    event: RunEventKind::TurnStarted { turn: 2 },
                },
            },
            PlanFeedEvent::RepairRun {
                run_id: repair_run_id.clone(),
                event: RunEvent {
                    timestamp: Utc::now(),
                    run_id: repair_run_id,
                    event: RunEventKind::RunCompleted {
                        status: "completed".to_string(),
                    },
                },
            },
        ];

        let text = render_plan_attach_text_with_feed(&paths, &plan, &[], &[], &feed_events, 0);

        assert!(text.contains("plan feed"), "{text}");
        assert!(text.contains("task-0"), "{text}");
        assert!(text.contains("turn 2 started"), "{text}");
        assert!(text.contains("repair run"), "{text}");
        assert!(text.contains("run completed"), "{text}");
    }

    #[test]
    fn plan_attach_handles_partial_plan_event_line() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        save_plan(&paths, &plan).expect("save plan");
        append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)
            .expect("append started");
        std::fs::OpenOptions::new()
            .append(true)
            .open(paths.plan_events(&plan.plan_id))
            .expect("open events")
            .write_all(b"{\"kind\":\"partial\"\n")
            .expect("partial line");

        let text = render_plan_attach_text(
            &paths,
            &plan,
            &[],
            &read_plan_events_lossy(&paths, &plan.plan_id),
            0,
        );

        assert!(text.contains("plan started"), "{text}");
    }

    #[test]
    fn attach_plan_shows_capability_preview() {
        let (_temp, paths, plan) = full_plan_fixture(2);

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

        assert!(
            text.contains("capabilities network=Allowlist deploy=true install=false"),
            "{text}"
        );
        assert!(
            text.contains("planner smoke:planner  default child smoke:child"),
            "{text}"
        );
    }

    #[test]
    fn attach_plan_enter_drills_then_esc_returns() {
        let (_temp, paths, plan) = full_plan_fixture(2);

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

        assert!(text.contains("Enter waits for child run"), "{text}");
        assert!(text.contains("q/Esc/Ctrl-D detach"), "{text}");
    }

    #[test]
    fn plan_attach_footer_snapshot_captures_back_navigation_grammar() {
        let (_temp, paths, plan) = full_plan_fixture(2);

        let footer = plan_attach_footer(&paths, &plan, 0, true);

        assert!(footer.starts_with("q/Esc/Ctrl-D detach"), "{footer}");
        assert!(footer.contains("arrows/Tab focus child"), "{footer}");
        assert!(footer.contains("Enter waits for child run"), "{footer}");
        assert!(footer.contains("try: deadreckon fork"), "{footer}");
    }

    #[test]
    fn attach_plan_enter_opens_selected_child_run_detail() {
        let (temp, paths, mut plan) = full_plan_fixture(2);
        let state = create_run(
            &paths,
            RunOptions {
                goal: "child detail".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        plan.tasks[0].child_run_id = Some(state.run_id);

        let footer = plan_attach_footer(&paths, &plan, 0, true);

        assert!(footer.contains("Enter child run"), "{footer}");
        assert!(!footer.contains("try: deadreckon fork"), "{footer}");
    }

    #[test]
    fn attach_plan_back_returns_to_same_selected_task() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        let text = render_plan_attach_text(&paths, &plan, &[], &[], 1);

        assert!(attach_should_return_to_plan(KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::NONE
        )));
        assert!(attach_should_return_to_plan(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )));
        assert!(text.contains("* task-1"), "{text}");
    }

    #[test]
    fn attach_plan_enter_without_run_id_shows_try_footer() {
        let (_temp, paths, plan) = full_plan_fixture(2);

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

        assert!(text.contains("Enter waits for child run"), "{text}");
        assert!(
            text.contains(&format!("try: deadreckon fork {}", &plan.plan_id[..8])),
            "{text}"
        );
    }

    #[test]
    fn plan_attach_overview_breadcrumb_names_plan() {
        let (_temp, paths, plan) = full_plan_fixture(2);

        let text = render_plan_attach_text(&paths, &plan, &[], &[], 0);

        assert!(text.contains("deadreckon plan"), "{text}");
        assert!(text.contains(&plan.plan_id[..8]), "{text}");
    }

    #[test]
    fn child_attach_from_plan_names_parent_and_back_action() {
        let (_temp, state) = doc_preview_state();
        let tui_state = AttachTuiState {
            parent_plan: Some(AttachParentPlan {
                plan_id: "99998888777766665555444433332222".to_string(),
                task_id: "task-1".to_string(),
            }),
            ..AttachTuiState::default()
        };

        let text =
            render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

        assert!(text.contains("plan 99998888 / task-1"), "{text}");
        assert!(
            text.contains("Back to plan: b Backspace q Esc Ctrl-D"),
            "{text}"
        );
        assert!(text.contains("parent plan 99998888 task-1"), "{text}");
    }

    #[test]
    fn plan_attach_child_breadcrumb_names_task_and_run() {
        let (_temp, state) = doc_preview_state();
        let tui_state = AttachTuiState {
            parent_plan: Some(AttachParentPlan {
                plan_id: "99998888777766665555444433332222".to_string(),
                task_id: "task-1".to_string(),
            }),
            ..AttachTuiState::default()
        };

        let text =
            render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

        assert!(text.contains("plan 99998888 / task-1"), "{text}");
        assert!(
            text.contains(&format!("run {}", &state.run_id[..8])),
            "{text}"
        );
    }

    #[test]
    fn plan_attach_child_footer_includes_back_hint() {
        let (_temp, state) = doc_preview_state();
        let tui_state = AttachTuiState {
            parent_plan: Some(AttachParentPlan {
                plan_id: "99998888777766665555444433332222".to_string(),
                task_id: "task-1".to_string(),
            }),
            ..AttachTuiState::default()
        };

        let text =
            render_attach_text_with_tui_state(&state, &[], &AttachLive::default(), tui_state);

        assert!(
            text.contains("Back to plan: b Backspace q Esc Ctrl-D"),
            "{text}"
        );
    }

    #[test]
    fn attach_plan_ctrl_d_detaches_does_not_kill() {
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        assert!(super::attach_should_quit(key));
    }

    #[test]
    fn attach_plan_q_detaches_from_child_without_killing() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        assert!(super::attach_should_quit(key));
    }

    #[test]
    fn malformed_plan_event_line_does_not_break_attach() {
        let (_temp, paths, plan) = full_plan_fixture(2);
        save_plan(&paths, &plan).expect("save plan");
        append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)
            .expect("append started");
        std::fs::OpenOptions::new()
            .append(true)
            .open(paths.plan_events(&plan.plan_id))
            .expect("open events")
            .write_all(b"not-json\n")
            .expect("bad line");

        let text = render_plan_attach_text(
            &paths,
            &plan,
            &[],
            &read_plan_events_lossy(&paths, &plan.plan_id),
            0,
        );

        assert!(text.contains("plan started"), "{text}");
    }

    #[test]
    fn tui_budget_callout_appears_above_60_percent() {
        let (_temp, mut state) = doc_preview_state();
        state.provider = Some("openai".to_string());
        state.total_spend_usd = 6.5;
        state.max_spend_usd = Some(10.0);

        let text = render_attach_text(&state, &[], &AttachLive::default());

        assert!(text.contains("$6.500000 / $10.000000"), "{text}");
        assert!(text.contains("65% of budget"), "{text}");
    }

    #[test]
    fn polish_preview_block_lists_provider_and_subskills() {
        let (_temp, state) = doc_preview_state();
        let estimate = SpendEstimate {
            provider: "cli:codex".to_string(),
            model: "provider default".to_string(),
            input_tokens: 0,
            output_tokens: 65_536,
            cost_usd: 0.0,
            subscription: true,
            wall_time_seconds: None,
        };
        let text = doc_polish_preview_text(
            &state,
            "cli:codex",
            "auto_subscription",
            &[
                "narrator-overview".to_string(),
                "narrator-phases".to_string(),
                "narrator-as-built".to_string(),
                "narrator-decisions".to_string(),
            ],
            16_384,
            Some(0.0),
            &estimate,
        )
        .expect("preview");
        assert!(text.contains("provider:"));
        assert!(text.contains("cli:codex"));
        assert!(text.contains("narrator-overview, narrator-phases"));
        assert!(text.contains("$0.00 (subscription)"));
    }

    #[test]
    fn polish_preview_suppressed_by_hints_env() {
        assert!(!completion_hints_enabled(true));
    }

    fn counts() -> AttachPanelCounts {
        AttachPanelCounts {
            activity: 20,
            files: 10,
            processes: 3,
        }
    }

    fn rows() -> AttachPanelRows {
        AttachPanelRows {
            activity: 5,
            files: 4,
            processes: 4,
        }
    }

    #[test]
    fn chain_attach_renders_step_timeline_with_status_dots() {
        let chain = chain_fixture();
        let tui_state = ChainAttachTuiState::default();

        let lines = chain_timeline_lines(&chain, &tui_state)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("◉ step  1 applied"));
        assert!(lines[0].contains("run aaaaaaaa"));
        assert!(lines[1].contains("● step  2 running"));
    }

    #[test]
    fn chain_attach_header_shows_policy_apply_mode_on_fail() {
        let chain = chain_fixture();
        let header = chain_attach_header_text(&chain);

        assert!(header.contains("status pending"));
        assert!(header.contains("policy branch=stack apply=auto strategy=squash on-fail=stop"));
        assert!(header.contains("spend $0.000000/$5.000000"));
    }

    #[test]
    fn chain_attach_activity_lists_newest_events_first() {
        let chain = chain_fixture();
        let events = vec![
            ChainEvent {
                timestamp: Utc::now(),
                chain_id: chain.chain_id.clone(),
                event: ChainEventKind::ChainCreated,
                step_index: None,
                detail: serde_json::json!({ "goal": "build app" }),
            },
            ChainEvent {
                timestamp: Utc::now(),
                chain_id: chain.chain_id.clone(),
                event: ChainEventKind::ChainStepStarted,
                step_index: Some(1),
                detail: serde_json::json!({ "goal": "second step" }),
            },
        ];

        let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("step started step 2"));
        assert!(lines[1].contains("created"));
    }

    #[test]
    fn chain_attach_paused_footer_lists_try_lines() {
        let mut chain = chain_fixture();
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("apply_refused_conflict".to_string());

        let footer = chain_attach_footer_text(&chain);

        assert!(footer.contains("paused: apply_refused_conflict"));
        assert!(footer.contains("show --why-failed"));
        assert!(footer.contains("resume --apply-mode preview"));
        assert!(footer.contains("undo"));
    }

    #[test]
    fn chain_default_auto_attaches_when_stdout_tty() {
        assert!(chain_should_auto_attach(true, false, false, false));
        assert!(!chain_should_auto_attach(false, false, false, false));
        assert!(!chain_should_auto_attach(true, true, false, false));
        assert!(!chain_should_auto_attach(true, false, true, false));
        assert!(!chain_should_auto_attach(true, false, false, true));
    }

    #[test]
    fn chain_attach_budget_bar_thresholds_60_80_percent() {
        assert_eq!(threshold_color(0.59), Color::Green);
        assert_eq!(threshold_color(0.60), Color::Yellow);
        assert_eq!(threshold_color(0.79), Color::Yellow);
        assert_eq!(threshold_color(0.80), Color::Red);
    }

    #[test]
    fn spend_gauge_uses_gradient_and_pause_cap_palette() {
        let (_temp, mut state) = doc_preview_state();

        assert_eq!(meter_color(0.30, &state), Color::Green);
        assert_eq!(meter_color(0.70, &state), Color::Yellow);
        assert_eq!(meter_color(0.90, &state), Color::Red);

        state.pause_reason = Some("spend cap reached".to_string());
        assert_eq!(meter_color(0.90, &state), Color::Magenta);
    }

    #[test]
    fn deadreckoning_course_animation_moves() {
        let first = deadreckoning_course_ascii(16, 0);
        let second = deadreckoning_course_ascii(16, 1);

        assert_ne!(first, second);
        assert!(first.contains('*'));
        assert_eq!(first.chars().count(), 16);
    }

    #[test]
    fn deadreckoning_course_strip_matches_identity_golden() {
        assert_eq!(deadreckoning_course_ascii(18, 0), "*--.--.^-.--.-^.--");
    }

    #[test]
    fn chain_step_glyphs_match_identity_set() {
        assert_eq!(chain_step_dot(ChainStepStatus::Pending), "○");
        assert_eq!(chain_step_dot(ChainStepStatus::Running), "●");
        assert_eq!(chain_step_dot(ChainStepStatus::Completed), "◐");
        assert_eq!(chain_step_dot(ChainStepStatus::Failed), "✗");
        assert_eq!(chain_step_dot(ChainStepStatus::Skipped), "↷");
        assert_eq!(chain_step_dot(ChainStepStatus::Applied), "◉");
        assert_eq!(chain_step_dot(ChainStepStatus::Undone), "↶");
    }

    #[test]
    fn attach_footer_status_names_running_state() {
        let (_temp, mut state) = doc_preview_state();
        state.status = deadreckon_core::RunStatus::Executing;
        state.current_phase_id = deadreckon_core::PhaseId(40);

        let text = deadreckoning_status_text(&state, "42s running", 100, 3);

        assert!(text.contains("deadreckoning running"), "{text}");
        assert!(text.contains("turn 42s running"), "{text}");
        assert!(text.contains("execute"), "{text}");
        assert!(text.contains('*'), "{text}");
    }

    #[test]
    fn attach_header_is_identity_strip_without_live_status() {
        let (_temp, mut state) = doc_preview_state();
        state.status = deadreckon_core::RunStatus::Executing;
        state.current_phase_id = deadreckon_core::PhaseId(40);

        let text = attach_header_text(&state, 96);

        assert!(text.contains("run "), "{text}");
        assert!(text.contains("provider cli:codex"), "{text}");
        assert!(text.contains("sandbox none"), "{text}");
        assert!(text.contains("goal preview docs"), "{text}");
        assert!(text.contains("working "), "{text}");
        assert!(!text.contains("status executing"), "{text}");
        assert!(!text.contains("phase 40"), "{text}");
        assert!(!text.contains("turn "), "{text}");
    }

    #[test]
    fn acceptance_activity_lines_surface_running_and_failed_checks() {
        let acceptance = AcceptanceLive {
            status: AcceptanceUiStatus::Failed,
            total: 3,
            completed: 2,
            passed: 1,
            failed: 1,
            required_failed: 1,
            latest_detail: Some("npm test exited with status 1".to_string()),
            progress_lines: vec![
                "✗ shell npm test exited with status 1".to_string(),
                "✓ file_exists package.json exists".to_string(),
            ],
        };

        let lines = acceptance_activity_lines(&acceptance).join("\n");

        assert!(lines.contains("acceptance failed"), "{lines}");
        assert!(lines.contains("1 required failures"), "{lines}");
        assert!(lines.contains("npm test"), "{lines}");
    }

    fn test_log_spec(cwd_match: IngestCwdMatch) -> ProviderJsonlLogSpec {
        ProviderJsonlLogSpec {
            schema: "test".to_string(),
            roots: Vec::new(),
            since: Utc::now(),
            cwd_match,
            cwd_match_path: None,
            storage: IngestStorage::Jsonl,
            file_glob: Some("*.jsonl".to_string()),
        }
    }

    #[test]
    fn provider_log_spec_uses_descriptor_roots_for_codex() {
        let (_temp, state) = doc_preview_state();
        let registry = ProviderRegistry::builtin().expect("registry");
        let home = std::path::PathBuf::from("/tmp/deadreckon-home");

        let spec = provider_jsonl_log_spec_from_registry(&state, &registry, &home)
            .expect("codex log spec");

        assert_eq!(spec.schema, "codex-cli");
        assert_eq!(spec.cwd_match, IngestCwdMatch::SessionMeta);
        assert!(spec.roots.contains(&home.join(".codex/sessions")));
        assert!(spec.roots.contains(&home.join(".codex/archived_sessions")));
    }

    #[test]
    fn provider_log_spec_honors_ingest_env_override() {
        let home = std::path::PathBuf::from("/tmp/deadreckon-home");
        let first = std::path::PathBuf::from("/tmp/codex-one");
        let second = std::path::PathBuf::from("/tmp/codex-two");
        let env_value =
            std::env::join_paths([first.as_path(), second.as_path()]).expect("join env paths");
        let ingest = IngestDescriptor {
            env_var: Some("CODEX_SESSIONS_DIR".to_string()),
            default_dirs: vec![std::path::PathBuf::from("~/.codex/sessions")],
            ..IngestDescriptor::default()
        };

        let roots = provider_ingest_base_roots(&ingest, &home, Some(env_value.as_os_str()));

        assert_eq!(roots, [first, second]);
    }

    #[test]
    fn claude_ingest_roots_remain_workdir_scoped_and_deduped() {
        let (_temp, mut state) = doc_preview_state();
        state.provider = Some("cli:claude-code".to_string());
        let registry = ProviderRegistry::builtin().expect("registry");
        let home = std::path::PathBuf::from("/tmp/deadreckon-home");

        let spec = provider_jsonl_log_spec_from_registry(&state, &registry, &home)
            .expect("claude log spec");
        let expected = home
            .join(".claude/projects")
            .join(claude_project_name_for_workdir(
                &state.working_dir.to_string_lossy(),
            ));

        assert_eq!(spec.schema, "claude-code");
        assert_eq!(spec.cwd_match, IngestCwdMatch::ClaudeProjectDir);
        assert!(spec.roots.contains(&expected), "{:?}", spec.roots);
        let mut deduped = spec.roots.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(spec.roots, deduped);
    }

    #[test]
    fn provider_jsonl_matchers_cover_session_meta_and_top_level_cwd() {
        let temp = test_tempdir();
        let working_dir = temp.path().join("work");
        let working_dirs = vec![working_dir.to_string_lossy().to_string()];

        let codex = temp.path().join("codex.jsonl");
        std::fs::write(
            &codex,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                working_dir.display()
            ),
        )
        .expect("codex jsonl");
        let codex_spec = test_log_spec(IngestCwdMatch::SessionMeta);
        assert!(provider_jsonl_session_matches_run(
            &codex_spec,
            &codex,
            &working_dirs
        ));

        let claude = temp.path().join("claude.jsonl");
        std::fs::write(
            &claude,
            format!(
                "{{\"type\":\"assistant\",\"cwd\":\"{}\",\"message\":{{\"content\":[]}}}}\n",
                working_dir.display()
            ),
        )
        .expect("claude jsonl");
        let claude_spec = test_log_spec(IngestCwdMatch::TopLevel);
        assert!(provider_jsonl_session_matches_run(
            &claude_spec,
            &claude,
            &working_dirs
        ));
    }

    #[test]
    fn cwd_match_directory_field_matches_opencode_json() {
        let temp = test_tempdir();
        let working_dir = temp.path().join("work");
        let working_dirs = vec![working_dir.to_string_lossy().to_string()];
        let path = temp.path().join("opencode.json");
        std::fs::write(
            &path,
            format!(r#"{{"id":"s1","directory":"{}"}}"#, working_dir.display()),
        )
        .expect("opencode json");

        let mut spec = test_log_spec(IngestCwdMatch::DirectoryField);
        spec.storage = IngestStorage::Json;

        assert!(provider_jsonl_session_matches_run(
            &spec,
            &path,
            &working_dirs
        ));
    }

    #[test]
    fn provider_jsonl_activity_dispatches_codex_and_claude_rows() {
        let mut codex = ProviderActivity::default();
        let codex_lines = provider_jsonl_activity_lines(
            "codex-cli",
            r#"{"type":"event_msg","timestamp":"2026-05-13T02:34:17Z","payload":{"type":"agent_message","message":"Working on it"}}"#,
            &mut codex,
        );
        assert_eq!(codex_lines.len(), 1);
        assert!(codex_lines[0].contains("agent Working on it"));
        let codex_tool_lines = provider_jsonl_activity_lines(
            "codex-cli",
            r#"{"type":"response_item","timestamp":"2026-05-13T02:34:18Z","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}"}}"#,
            &mut codex,
        );
        assert_eq!(codex_tool_lines.len(), 1);
        assert!(codex_tool_lines[0].contains("tool Bash cargo test"));

        let mut claude = ProviderActivity::default();
        let claude_lines = provider_jsonl_activity_lines(
            "claude-code",
            r#"{"type":"assistant","timestamp":"2026-05-13T02:34:17.615Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4},"content":[{"type":"text","text":"Adding tests"},{"type":"tool_use","name":"Bash","input":{"command":"npm test"}}]}}"#,
            &mut claude,
        );
        assert_eq!(claude.context_tokens, Some(10));
        assert_eq!(claude.context_window, Some(200_000));
        assert_eq!(claude_lines.len(), 2);
        assert!(claude_lines[0].contains("agent Adding tests"));
        assert!(claude_lines[1].contains("tool Bash npm test"));
    }

    #[test]
    fn provider_jsonl_copilot_activity_parses_assistant_message_and_usage() {
        let mut activity = ProviderActivity::default();
        let lines = provider_jsonl_activity_lines(
            "copilot-cli",
            r#"{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","usage":{"inputTokens":10,"output_tokens":4,"cacheReadTokens":2,"cacheWriteTokens":1},"data":{"reasoningText":"Need a plan","content":"I will edit the file","outputTokens":4}}"#,
            &mut activity,
        );
        let joined = lines.join("\n");

        assert!(
            joined.contains("tokens input 10 output 4 cache 3"),
            "{joined}"
        );
        assert!(joined.contains("thinking Need a plan"), "{joined}");
        assert!(joined.contains("agent I will edit the file"), "{joined}");
        assert!(joined.contains("tokens output 4"), "{joined}");
        assert_eq!(activity.context_tokens, Some(13));
        assert_eq!(activity.context_window, Some(258_400));
    }

    #[test]
    fn provider_jsonl_copilot_activity_parses_tool_request_and_result() {
        let mut activity = ProviderActivity::default();
        let tool_lines = provider_jsonl_activity_lines(
            "copilot-cli",
            r#"{"type":"assistant.message","timestamp":"2026-05-13T02:34:18Z","data":{"toolRequests":[{"toolCallId":"t1","name":"bash","arguments":{"command":"cargo test"}}]}}"#,
            &mut activity,
        );
        let result_lines = provider_jsonl_activity_lines(
            "copilot-cli",
            r#"{"type":"tool.execution_complete","timestamp":"2026-05-13T02:34:19Z","data":{"toolCallId":"t1","result":"tests passed"}}"#,
            &mut activity,
        );

        assert!(tool_lines.join("\n").contains("tool Bash cargo test"));
        assert!(result_lines.join("\n").contains("result tests passed"));
    }

    #[test]
    fn provider_jsonl_copilot_activity_ignores_unrelated_event_rows() {
        let mut activity = ProviderActivity::default();
        let lines = provider_jsonl_activity_lines(
            "copilot-cli",
            r#"{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{"sessionId":"s1"}}"#,
            &mut activity,
        );

        assert!(lines.is_empty());
    }

    #[test]
    fn provider_jsonl_pi_activity_parses_text_thinking_tool_and_result_blocks() {
        let mut activity = ProviderActivity::default();
        let assistant_lines = provider_jsonl_activity_lines(
            "pi",
            r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to inspect"},{"type":"text","text":"I will inspect the file"},{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"cargo test"}}]}}"#,
            &mut activity,
        );
        let result_lines = provider_jsonl_activity_lines(
            "pi",
            r#"{"type":"message","timestamp":"2026-05-13T02:34:18Z","message":{"role":"toolResult","toolCallId":"t1","content":"ok"}}"#,
            &mut activity,
        );
        let joined = assistant_lines.join("\n");

        assert!(joined.contains("thinking Need to inspect"), "{joined}");
        assert!(joined.contains("agent I will inspect the file"), "{joined}");
        assert!(joined.contains("tool Bash cargo test"), "{joined}");
        assert!(result_lines.join("\n").contains("result ok"));
    }

    #[test]
    fn provider_jsonl_pi_activity_normalizes_intent_argument_description() {
        let mut activity = ProviderActivity::default();
        let lines = provider_jsonl_activity_lines(
            "pi",
            r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","content":[{"type":"toolCall","name":"Task","arguments":{"agent__intent":"review docs","prompt":"read files"}}]}}"#,
            &mut activity,
        );

        assert!(lines.join("\n").contains(r#""description":"review docs""#));
    }

    #[test]
    fn provider_jsonl_pi_activity_extracts_usage_context_tokens() {
        let mut activity = ProviderActivity::default();
        let lines = provider_jsonl_activity_lines(
            "pi",
            r#"{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{"role":"assistant","usage":{"input":10,"output":5,"cache":{"read":3,"write":2}},"content":"Done"}}"#,
            &mut activity,
        );
        let joined = lines.join("\n");

        assert!(
            joined.contains("tokens input 10 output 5 cache 5"),
            "{joined}"
        );
        assert!(joined.contains("agent Done"), "{joined}");
        assert_eq!(activity.context_tokens, Some(15));
        assert_eq!(activity.context_window, Some(1_000_000));
    }

    #[test]
    fn schema_dispatch_unknown_schema_is_quiet() {
        let mut activity = ProviderActivity::default();
        let lines = provider_jsonl_activity_lines(
            "unknown-schema",
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hidden"}}"#,
            &mut activity,
        );

        assert!(lines.is_empty());
    }

    #[test]
    fn provider_jsonl_copilot_ingest_discovers_bare_session_state_jsonl() {
        let temp = test_tempdir();
        let (_state_temp, mut state) = doc_preview_state();
        state.provider = Some("cli:copilot".to_string());
        let home = temp.path().join("home");
        let session_dir = home.join(".copilot/session-state");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(
            session_dir.join("abc.jsonl"),
            format!(
                r#"{{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{{"context":{{"cwd":"{}"}}}}}}
{{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","data":{{"content":"Copilot edited the file"}}}}
"#,
                state.working_dir.display()
            ),
        )
        .expect("copilot session");
        let registry = ProviderRegistry::builtin().expect("registry");
        let spec =
            provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("copilot spec");

        let activity = collect_jsonl_provider_activity(&state, &spec);
        let lines = activity.lines.join("\n");

        assert!(lines.contains("agent Copilot edited the file"), "{lines}");
        assert!(lines.contains("provider log"), "{lines}");
    }

    #[test]
    fn provider_jsonl_copilot_ingest_discovers_nested_events_jsonl() {
        let temp = test_tempdir();
        let (_state_temp, mut state) = doc_preview_state();
        state.provider = Some("cli:copilot".to_string());
        let home = temp.path().join("home");
        let session_dir = home.join(".copilot/session-state/sess-1");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(
            session_dir.join("events.jsonl"),
            format!(
                r#"{{"type":"session.start","timestamp":"2026-05-13T02:34:16Z","data":{{"context":{{"cwd":"{}"}}}}}}
{{"type":"assistant.message","timestamp":"2026-05-13T02:34:17Z","data":{{"content":"Nested event worked"}}}}
"#,
                state.working_dir.display()
            ),
        )
        .expect("events");
        let registry = ProviderRegistry::builtin().expect("registry");
        let spec =
            provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("copilot spec");

        let activity = collect_jsonl_provider_activity(&state, &spec);

        assert!(activity.lines.join("\n").contains("Nested event worked"));
    }

    #[test]
    fn provider_jsonl_copilot_ingest_json_pointer_cwd_matches_session_start() {
        let temp = test_tempdir();
        let working_dir = temp.path().join("work");
        let path = temp.path().join("copilot.jsonl");
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"session.start","data":{{"context":{{"cwd":"{}"}}}}}}"#,
                working_dir.display()
            ),
        )
        .expect("copilot jsonl");
        let mut spec = test_log_spec(IngestCwdMatch::JsonPointer);
        spec.cwd_match_path = Some("data.context.cwd".to_string());

        assert!(provider_jsonl_session_matches_run(
            &spec,
            &path,
            &[working_dir.to_string_lossy().to_string()]
        ));
    }

    #[test]
    fn provider_jsonl_pi_ingest_discovers_session_jsonl_under_encoded_cwd_dir() {
        let temp = test_tempdir();
        let (_state_temp, mut state) = doc_preview_state();
        state.provider = Some("cli:pi".to_string());
        let home = temp.path().join("home");
        let session_dir = home.join(".pi/agent/sessions/--tmp-work--");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(
            session_dir.join("pi-session.jsonl"),
            format!(
                r#"{{"type":"session","id":"s1","timestamp":"2026-05-13T02:34:16Z","cwd":"{}"}}
{{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{{"role":"assistant","content":"Pi edited the file"}}}}
"#,
                state.working_dir.display()
            ),
        )
        .expect("pi session");
        let registry = ProviderRegistry::builtin().expect("registry");
        let spec =
            provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("pi spec");

        let activity = collect_jsonl_provider_activity(&state, &spec);
        let lines = activity.lines.join("\n");

        assert!(lines.contains("agent Pi edited the file"), "{lines}");
        assert!(lines.contains("provider log"), "{lines}");
    }

    #[test]
    fn provider_jsonl_pi_ingest_rejects_jsonl_without_session_header() {
        let temp = test_tempdir();
        let (_state_temp, mut state) = doc_preview_state();
        state.provider = Some("cli:pi".to_string());
        let home = temp.path().join("home");
        let session_dir = home.join(".pi/agent/sessions/--tmp-work--");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(
            session_dir.join("not-pi.jsonl"),
            format!(
                r#"{{"type":"note","cwd":"{}"}}
{{"type":"message","timestamp":"2026-05-13T02:34:17Z","message":{{"role":"assistant","content":"Should not show"}}}}
"#,
                state.working_dir.display()
            ),
        )
        .expect("not pi");
        let registry = ProviderRegistry::builtin().expect("registry");
        let spec =
            provider_jsonl_log_spec_from_registry(&state, &registry, &home).expect("pi spec");

        let activity = collect_jsonl_provider_activity(&state, &spec);

        assert!(activity.lines.is_empty(), "{:?}", activity.lines);
    }

    #[test]
    fn provider_jsonl_pi_ingest_top_level_cwd_matches_session_header() {
        let temp = test_tempdir();
        let working_dir = temp.path().join("work");
        let path = temp.path().join("pi.jsonl");
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"session","id":"s1","cwd":"{}"}}"#,
                working_dir.display()
            ),
        )
        .expect("pi jsonl");
        let spec = test_log_spec(IngestCwdMatch::TopLevel);

        assert!(provider_jsonl_session_matches_run(
            &spec,
            &path,
            &[working_dir.to_string_lossy().to_string()]
        ));
    }

    #[test]
    fn gemini_json_object_fixture_emits_agent_tool_result_and_tokens() {
        let temp = test_tempdir();
        let (_state_temp, state) = doc_preview_state();
        let root = temp.path().join("gemini");
        std::fs::create_dir_all(&root).expect("gemini root");
        std::fs::write(
            root.join("session-test.json"),
            r#"{
  "sessionId": "s1",
  "messages": [{
    "type": "gemini",
    "timestamp": "2026-05-13T02:34:17Z",
    "thoughts": [{"subject": "Plan", "description": "Read the file"}],
    "content": "I will inspect the file",
    "tokens": {"input": 10, "cached": 2, "output": 3},
    "toolCalls": [{
      "name": "read_file",
      "args": {"path": "src/main.rs"},
      "result": [{"functionResponse": {"id": "r1", "response": {"output": "file contents"}}}]
    }]
  }]
}"#,
        )
        .expect("gemini fixture");
        let spec = ProviderJsonlLogSpec {
            schema: "gemini".to_string(),
            roots: vec![root],
            since: Utc::now() - chrono::Duration::minutes(1),
            cwd_match: IngestCwdMatch::None,
            cwd_match_path: None,
            storage: IngestStorage::JsonOrJsonl,
            file_glob: None,
        };

        let activity = collect_jsonl_provider_activity(&state, &spec);
        let lines = activity.lines.join("\n");

        assert!(lines.contains("thinking Plan Read the file"), "{lines}");
        assert!(lines.contains("agent I will inspect the file"), "{lines}");
        assert!(lines.contains("tool Read src/main.rs"), "{lines}");
        assert!(lines.contains("result file contents"), "{lines}");
        assert_eq!(activity.context_tokens, Some(12));
        assert_eq!(activity.context_window, Some(1_000_000));
    }

    #[test]
    fn gemini_jsonl_fixture_emits_activity_and_tokens() {
        let temp = test_tempdir();
        let (_state_temp, state) = doc_preview_state();
        let root = temp.path().join("gemini");
        std::fs::create_dir_all(&root).expect("gemini root");
        std::fs::write(
            root.join("session-test.jsonl"),
            r#"{"type":"user","id":"u1","timestamp":"2026-05-13T02:34:16Z","content":"hello"}
{"type":"gemini","id":"g1","timestamp":"2026-05-13T02:34:17Z","content":[{"text":"Done"}],"tokens":{"input":4,"cached":1},"toolCalls":[{"name":"run_command","args":{"command":"cargo test"}}]}
"#,
        )
        .expect("gemini jsonl fixture");
        let spec = ProviderJsonlLogSpec {
            schema: "gemini".to_string(),
            roots: vec![root],
            since: Utc::now() - chrono::Duration::minutes(1),
            cwd_match: IngestCwdMatch::None,
            cwd_match_path: None,
            storage: IngestStorage::JsonOrJsonl,
            file_glob: None,
        };

        let activity = collect_jsonl_provider_activity(&state, &spec);
        let lines = activity.lines.join("\n");

        assert!(lines.contains("agent Done"), "{lines}");
        assert!(lines.contains("tool Bash cargo test"), "{lines}");
        assert_eq!(activity.context_tokens, Some(5));
        assert_eq!(activity.context_window, Some(1_000_000));
    }

    #[test]
    fn opencode_storage_fixture_emits_agent_thinking_tool_and_tokens() {
        let temp = test_tempdir();
        let (_state_temp, state) = doc_preview_state();
        let root = temp.path().join("opencode");
        let session_dir = root.join("storage/session/project");
        let message_dir = root.join("storage/message/s1");
        let part_dir = root.join("storage/part/m1");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::create_dir_all(&message_dir).expect("message dir");
        std::fs::create_dir_all(&part_dir).expect("part dir");
        std::fs::write(
            session_dir.join("s1.json"),
            format!(
                r#"{{"id":"s1","directory":"{}","time":{{"created":1770000000000}}}}"#,
                state.working_dir.display()
            ),
        )
        .expect("session");
        std::fs::write(
            message_dir.join("m1.json"),
            r#"{"id":"m1","sessionID":"s1","role":"assistant","time":{"created":1770000000000}}"#,
        )
        .expect("message");
        std::fs::write(
            part_dir.join("01.json"),
            r#"{"id":"p1","messageID":"m1","type":"reasoning","content":"Need to edit","time":{"created":1770000000001}}"#,
        )
        .expect("reasoning");
        std::fs::write(
            part_dir.join("02.json"),
            r#"{"id":"p2","messageID":"m1","type":"text","content":"Editing now","time":{"created":1770000000002}}"#,
        )
        .expect("text");
        std::fs::write(
            part_dir.join("03.json"),
            r#"{"id":"p3","messageID":"m1","type":"tool","tool":"bash","state":{"input":{"command":"cargo test"}},"time":{"created":1770000000003}}"#,
        )
        .expect("tool");
        std::fs::write(
            part_dir.join("04.json"),
            r#"{"id":"p4","messageID":"m1","type":"step-finish","tokens":{"input":7,"cache":{"read":2,"write":1}},"time":{"created":1770000000004}}"#,
        )
        .expect("tokens");
        let spec = ProviderJsonlLogSpec {
            schema: "opencode".to_string(),
            roots: vec![root],
            since: Utc::now() - chrono::Duration::minutes(1),
            cwd_match: IngestCwdMatch::DirectoryField,
            cwd_match_path: None,
            storage: IngestStorage::OpenCodeStorage,
            file_glob: Some("*.json".to_string()),
        };

        let activity = collect_jsonl_provider_activity(&state, &spec);
        let lines = activity.lines.join("\n");

        assert!(lines.contains("thinking Need to edit"), "{lines}");
        assert!(lines.contains("agent Editing now"), "{lines}");
        assert!(lines.contains("tool Bash cargo test"), "{lines}");
        assert_eq!(activity.context_tokens, Some(10));
    }

    #[test]
    fn cli_wait_status_mentions_work_and_elapsed_seconds() {
        let text = cli_wait_status_line(
            "compiling done criteria",
            std::time::Duration::from_secs(7),
            2,
        );

        assert!(text.contains("deadreckoning"), "{text}");
        assert!(text.contains("compiling done criteria"), "{text}");
        assert!(text.contains("7s"), "{text}");
    }

    #[test]
    fn chain_attach_shows_aggregate_spend_in_header() {
        let mut chain = chain_fixture();
        chain.total_spend_usd = 1.25;
        let header = chain_attach_header_text(&chain);

        assert!(header.contains("spend $1.250000/$5.000000"), "{header}");
    }

    #[test]
    fn chain_attach_focused_step_streams_provider_activity() {
        let chain = chain_fixture();
        let events = vec![ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain.chain_id.clone(),
            event: ChainEventKind::ChainRunCompleted,
            step_index: Some(1),
            detail: serde_json::json!({ "run_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "status": "completed" }),
        }];

        let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("run completed step 2"), "{lines:?}");
        assert!(lines[0].contains("bbbbbbbb"), "{lines:?}");
    }

    #[test]
    fn chain_attach_tab_pages_focus_between_steps() {
        let chain = chain_fixture();
        let mut tui_state = ChainAttachTuiState::default();

        tui_state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &chain);
        assert_eq!(tui_state.selected_step, 1);
    }

    #[test]
    fn chain_attach_ctrl_d_detaches_does_not_kill_conductor() {
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        assert!(super::attach_should_quit(key));
    }

    #[test]
    fn chain_attach_enter_drills_to_single_run_tui_esc_returns() {
        let footer = chain_attach_footer_text(&chain_fixture());

        assert!(footer.contains("[Enter] drill"));
        assert!(footer.contains("detach"));
    }

    #[test]
    fn chain_attach_r_invokes_redo_with_confirm() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[r] redo"));
    }

    #[test]
    fn chain_attach_e_invokes_extend_with_prompt() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[e] extend"));
    }

    #[test]
    fn chain_attach_p_pauses_chain() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[p] pause"));
    }

    #[test]
    fn chain_attach_k_kills_chain_with_confirm() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[k] kill"));
    }

    #[test]
    fn chain_attach_plain_emits_periodic_snapshot_no_ansi() {
        let snapshot = chain_attach_header_text(&chain_fixture());
        let ansi_start = format!("{}[", char::from(27));

        assert!(!snapshot.contains(&ansi_start), "{snapshot}");
        assert!(snapshot.contains("policy branch=stack"), "{snapshot}");
    }

    #[test]
    fn chain_attach_paused_footer_lists_four_try_lines() {
        let mut chain = chain_fixture();
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("cap".to_string());

        let footer = chain_attach_footer_text(&chain);

        assert_eq!(footer.matches("try:").count(), 4, "{footer}");
    }

    #[test]
    fn chain_wall_clock_cap_pauses_chain() {
        let mut chain = chain_fixture();
        chain.max_wall_seconds = Some(10.0);
        chain.total_wall_seconds = 10.0;

        assert!(chain_wall_cap_hit(&chain));
    }

    #[test]
    fn chain_per_step_wall_cap_is_remaining_over_remaining_steps() {
        let mut chain = chain_fixture();
        chain.steps[0].status = ChainStepStatus::Pending;
        chain.steps[1].status = ChainStepStatus::Pending;
        chain.max_wall_seconds = Some(12.0);
        chain.total_wall_seconds = 2.0;

        assert_eq!(per_step_wall_cap(&chain, 0), Some(5.0));
    }

    #[test]
    fn tui_scroll_offsets_clamp_to_panel_content() {
        let mut state = AttachTuiState::default();
        state.scroll_focused(100, counts(), rows());
        assert_eq!(state.activity_scroll, 15);

        state.scroll_focused(-100, counts(), rows());
        assert_eq!(state.activity_scroll, 0);

        state.focused_panel = AttachPanel::Processes;
        state.scroll_focused(10, counts(), rows());
        assert_eq!(state.processes_scroll, 0);
    }

    #[test]
    fn tui_focus_and_page_keys_move_active_panel_only() {
        let mut state = AttachTuiState::default();
        state.handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(state.focused_panel, AttachPanel::Files);

        state.handle_key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(state.files_scroll, 3);
        assert_eq!(state.activity_scroll, 0);

        state.handle_key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(
            state.files_scroll,
            max_panel_scroll(AttachPanel::Files, counts(), rows())
        );

        state.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            counts(),
            rows(),
        );
        assert_eq!(state.focused_panel, AttachPanel::Activity);
    }

    #[test]
    fn completion_action_parser_accepts_short_and_long_forms() {
        assert_eq!(
            completion_action_from_input("m"),
            Some(CompletionAction::Materialize)
        );
        assert_eq!(
            completion_action_from_input("export"),
            Some(CompletionAction::Materialize)
        );
        assert_eq!(
            completion_action_from_input("extend"),
            Some(CompletionAction::Extend)
        );
        assert_eq!(
            completion_action_from_input("S"),
            Some(CompletionAction::Show)
        );
        assert_eq!(
            completion_action_from_input(""),
            Some(CompletionAction::Quit)
        );
        assert_eq!(completion_action_from_input("wat"), None);
    }

    #[test]
    fn markdown_renderer_styles_headings_lists_and_code() {
        let lines = markdown_to_tui_lines(
            "# Summary\n\nImplemented `apply`.\n\n- safer checkout\n\n```rust\nfn main() {}\n```\n",
        );
        let joined = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Summary"), "{joined}");
        assert!(joined.contains("Implemented apply."), "{joined}");
        assert!(joined.contains("- safer checkout"), "{joined}");
        assert!(joined.contains("fn main() {}"), "{joined}");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.as_ref() == "apply"
                    && span.style.fg == Some(Color::Yellow)
                    && span.style.add_modifier.contains(Modifier::BOLD)),
            "inline code should keep its styling"
        );
    }

    #[test]
    fn docs_toggle_uses_activity_panel_scroll_slot() {
        let mut state = AttachTuiState::default();
        state.toggle_docs();
        state.scroll_focused(4, counts(), rows());
        assert!(state.docs_open);
        assert_eq!(state.docs_scroll, 4);
        assert_eq!(state.activity_scroll, 0);
    }

    #[test]
    fn post_completion_action_resets_docs_view_and_explains_next_step() {
        let mut state = AttachTuiState {
            docs_open: true,
            docs_scroll: 42,
            activity_scroll: 10,
            files_scroll: 3,
            processes_scroll: 2,
            ..AttachTuiState::default()
        };

        state.record_post_action(AttachActionNotice {
            action: CompletionAction::Apply,
            success: true,
        });

        assert!(!state.docs_open);
        assert_eq!(state.focused_panel, AttachPanel::Activity);
        assert_eq!(state.activity_scroll, 0);
        assert_eq!(state.docs_scroll, 0);
        assert_eq!(state.files_scroll, 0);
        assert_eq!(state.processes_scroll, 0);
        let notice = state
            .post_action_notice
            .as_ref()
            .expect("post-action notice")
            .lines()
            .join("\n");
        assert!(notice.contains("apply action finished"), "{notice}");
        assert!(notice.contains("q detach"), "{notice}");
    }

    #[test]
    fn live_files_explain_cleaned_worktree() {
        let live = AttachLive {
            working_dir_exists: false,
            ..AttachLive::default()
        };

        assert_eq!(
            live_file_lines(&live),
            vec!["working tree was removed after cleanup".to_string()]
        );
    }
}
