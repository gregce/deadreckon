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
use std::io::{self, BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
use deadreckon::notify::{
    NotifyContext, NotifyTransition, channels_for_config, load_notify_config, notify_run,
};
use deadreckon::proof_block::ProofBlock;
use deadreckon::sleep::{self, SleepPrefs, SleepPrevention};
use deadreckon::ui_card::{
    Card, CardOptions, HintLine, Section, TitleGlyph, TitleLine, Tone, render_card,
};
use deadreckon_core::flight::{
    CheckpointManifest, FLIGHT_EVENTS_JSONL, FlightEvent, FlightEventKind, FlightSessionStatus,
    RewindEvent, RewindMode, RewindStatus, RewindTarget, RewindTargetKind, append_rewind_event,
    build_working_file_index, list_checkpoint_manifests, materialize_checkpoint,
    read_flight_events, read_flight_manifest,
};
use deadreckon_core::glossary::{NOUN_DONE_CONTRACT, NOUN_VERIFIED_RUN};
use deadreckon_core::install_receipt::{Channel, detect_receipt, read_receipt, write_receipt};
use deadreckon_core::learning::{
    LearningAutoPrStatus, LearningCandidate, LearningCandidateDiff, LearningEval,
    LearningEvalCommand, LearningIndexOptions, LearningInsightProvider, LearningPrEvent,
    LearningProposal, LearningProposalTarget, LearningStimulus, PrDryRun, build_reflection_prompt,
    build_reflection_prompt_from_bundle, classify_candidate_risk, evaluate_auto_pr, evidence_score,
    export_learning_bundle, import_learning_bundle, index_learning, learning_report,
    load_learning_policy, persist_reflection, prepare_pr_dry_run, read_learning_bundle,
    read_proposal, record_pr_event, write_candidate, write_eval,
};
use deadreckon_core::paths::{sanitize_slug, task_key, workspace_scope};
use deadreckon_core::update_cache::{read_cache, write_cache};
use deadreckon_core::{
    AcceptanceMarker, AcceptanceProgressEntry, ApplyMode, ApplyStrategy, BranchPolicy, Chain,
    ChainEvent, ChainEventKind, ChainNewOptions, ChainStatus, ChainStepMarker, ChainStepStatus,
    CodebaseMode, CodebaseRecord, ConductorState, CoordinatorChild, CoordinatorState,
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DeadreckonError, DeadreckonPaths,
    DocKind, DocProviderSelection, DocProviderSource, DocsStatus, ModeFlags, OnFail, PhaseId,
    PhaseStatus, Plan, PlanChildMarker, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind,
    PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, PromotionManifest,
    ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunEvent, RunEventKind, RunListEntry,
    RunOptions, RunStatus, SpendRecord, TraceRecord, WorktreeOptions,
    acceptance_progress_path_for_run_root, acceptance_spec_path_for_run_root, acquire_lock,
    append_chain_event, append_parent_narrative_update, append_plan_event, append_plan_message,
    append_provenance, append_trace, apply_commit_body, cancel_marker_present,
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
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod cli;
mod commands;
mod friendliness_contract;
mod narrative;
mod plan_event_bus;
mod product;
mod prompt;
mod tui;
mod tui_events;
mod ui;

use crate::cli::{
    AcceptanceCommand, AcceptancePreset, CHAIN_HELP, ChainCommandArgs, Cli, CliDocKind,
    CliPlanMode, Commands, CompletionCommand, ConfigCommand, ExtendCommandArgs, ForkCommandArgs,
    HistoryCommand, HistoryKind, ImproveCommand, LearnCommand, LibraryCommand, MergeCommandArgs,
    OrchestrateCommand, PlanCommandArgs, ProvidersCommand, RunCommandArgs, StartCommandArgs,
};
use crate::narrative::{AttachViewMode, NarrativeVisualMode};
use crate::plan_event_bus::{PlanEventBus, PlanFeedEvent};
use crate::tui::{
    AttachPanel, AttachParentPlan, AttachTuiState, PlanAttachRenderState, RunNarrativeRenderInput,
    attach_activity_lines_for_tui, attach_panel_layout, ensure_run_narrative_projection,
    live_file_lines, plan_event_line, plan_event_summary, plan_final_gate_line,
    plan_provider_summary, plan_repair_label, plan_task_detail_lines, process_lines,
    provider_is_metered, render_attach, render_plan_attach, run_narrative_projection,
    run_narrative_projection_signature,
};
#[cfg(test)]
use crate::tui::{
    acceptance_activity_lines, attach_header_text, deadreckoning_status_text, meter_color,
    plan_attach_footer, threshold_color,
};
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
    let hint = match err {
        CliError::Exit { hint, .. } => hint.clone(),
        CliError::Provider(deadreckon_providers::ProviderError::MissingCredential(_))
        | CliError::Provider(deadreckon_providers::ProviderError::NoRoute(_)) => {
            "deadreckon try; then deadreckon config provider cli:codex".to_string()
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
        CliError::Core(_) | CliError::Provider(_) => "deadreckon doctor".to_string(),
    };
    try_footer(hint)
}

fn try_footer(hint: impl AsRef<str>) -> String {
    let hint = hint.as_ref().trim();
    if hint.starts_with("try:") {
        hint.to_string()
    } else {
        format!("try: {hint}")
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
            commands::init::init_command(
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
        Commands::Acceptance { command } => commands::acceptance::acceptance_command(command).await,
        Commands::Done {
            args,
            provider,
            model,
            force,
            spec,
            against,
        } => commands::acceptance::done_command(args, provider, model, force, spec, against).await,
        Commands::Try { plain, json } => {
            ui::set_plain_output(plain || json);
            try_command(plain, json).await
        }
        Commands::Start {
            goal,
            mode,
            provider,
            children,
            planner_provider,
            child_provider,
            coder_provider,
            reviewer_provider,
            preview,
            yes,
            fresh,
            worktree,
            from,
            allow_dirty,
            plain,
            quiet,
            json,
        } => {
            ui::set_plain_output(plain || json);
            start_command(StartCommandArgs {
                goal,
                mode,
                provider,
                children,
                planner_provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                preview,
                yes,
                fresh,
                worktree,
                from,
                allow_dirty,
                plain,
                quiet,
                json,
            })
            .await
        }
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
            commands::run::run_command(RunCommandArgs {
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
            let request = commands::orchestrate::orchestrate_request_from_cli(
                command,
                commands::orchestrate::BareOrchestrateArgs {
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
            commands::orchestrate::orchestrate_command(request).await
        }
        Commands::Campaign {
            goal,
            n,
            planner_provider,
            provider,
            max_spend,
            max_wall_seconds,
            sandbox,
            preview,
            yes,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            commands::campaign::campaign_command(commands::campaign::CampaignArgs {
                goal,
                n,
                planner_provider,
                provider,
                max_spend,
                max_wall_seconds,
                sandbox,
                preview,
                yes,
                no_hints,
                quiet,
                plain,
            })
            .await
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
            commands::plan::plan_command(PlanCommandArgs {
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
            no_repair,
            repair_provider,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            commands::plan::fork_command(ForkCommandArgs {
                plan_id,
                max_spend,
                max_wall_seconds,
                sandbox,
                provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                no_repair,
                repair_provider,
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
            commands::merge::merge_command(MergeCommandArgs {
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
            commands::chain::chain_command(ChainCommandArgs {
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
                kind,
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
            view,
            visual,
            narrative_provider,
            no_narrative_provider,
            narrative_max_spend,
            json,
            no_hints,
            plain,
        } => {
            ui::set_plain_output(plain || json);
            let narrative_provider = if no_narrative_provider {
                Some("none".to_string())
            } else {
                narrative_provider
            };
            commands::attach::attach_command(commands::attach::AttachCommandArgs {
                run_id,
                no_hints,
                plain,
                json,
                view,
                visual,
                narrative_provider,
                narrative_max_spend,
            })
            .await
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
        Commands::Learn { command } => learn_command(command).await,
        Commands::Improve { command } => improve_command(command).await,
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
    StartWatchKeep,
    SetupHealth,
    Control,
    FindMore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelpAllGroup {
    ProductionFlow,
    SetupProviders,
    PowerUserLaunch,
    Orchestration,
    ContinueRecover,
    ResultsInspect,
    LearningAdvanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandDiscovery {
    Public,
    Advanced,
    Compatibility,
    Pseudo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandAudience {
    Primary,
    SetupSupport,
    Advanced,
    Compatibility,
    Pseudo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommandHelpEntry {
    display: &'static str,
    clap_name: Option<&'static str>,
    purpose: &'static str,
    audience: CommandAudience,
    top_group: Option<TopHelpGroup>,
    all_group: Option<HelpAllGroup>,
}

const TYPICAL_FLOW_COMMANDS: &[&str] = &[
    product::PRODUCT_FIRST_COMMAND,
    "deadreckon attach latest",
    "deadreckon status",
    "deadreckon list",
    "deadreckon finish latest",
];

const COMMAND_HELP_CATALOG: &[CommandHelpEntry] = &[
    CommandHelpEntry {
        display: "init",
        clap_name: Some("init"),
        purpose: "configure deadreckon",
        audience: CommandAudience::SetupSupport,
        top_group: Some(TopHelpGroup::SetupHealth),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "doctor",
        clap_name: Some("doctor"),
        purpose: "check provider, sandbox, and local setup",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::SetupHealth),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "detect",
        clap_name: Some("detect"),
        purpose: "probe registered providers",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "providers",
        clap_name: Some("providers"),
        purpose: "list provider routes and models",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "config",
        clap_name: Some("config"),
        purpose: "read or update configuration",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "completion",
        clap_name: Some("completion"),
        purpose: "install or generate shell completions",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "def-done",
        clap_name: Some("def-done"),
        purpose: "write/check done criteria in English",
        audience: CommandAudience::SetupSupport,
        top_group: Some(TopHelpGroup::SetupHealth),
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "try",
        clap_name: Some("try"),
        purpose: "run a keyless local proof",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "start",
        clap_name: Some("start"),
        purpose: "begin supervised agent work",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "run",
        clap_name: Some("run"),
        purpose: "power-user one-run launcher",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::PowerUserLaunch),
    },
    CommandHelpEntry {
        display: "orchestrate",
        clap_name: Some("orchestrate"),
        purpose: "power-user multi-agent launcher",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::PowerUserLaunch),
    },
    CommandHelpEntry {
        display: "chain",
        clap_name: Some("chain"),
        purpose: "serial multi-step power tool",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::PowerUserLaunch),
    },
    CommandHelpEntry {
        display: "attach",
        clap_name: Some("attach"),
        purpose: "watch and understand a run, chain, or plan",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "status",
        clap_name: Some("status"),
        purpose: "see the latest run and next action",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "list",
        clap_name: Some("list"),
        purpose: "find runs and plans",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "finish",
        clap_name: Some("finish"),
        purpose: "apply or export completed work",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "plan",
        clap_name: Some("plan"),
        purpose: "write a multi-agent plan without starting it",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "fork",
        clap_name: Some("fork"),
        purpose: "start child runs for a plan",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "merge",
        clap_name: Some("merge"),
        purpose: "compose completed plan children",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::Orchestration),
    },
    CommandHelpEntry {
        display: "extend",
        clap_name: Some("extend"),
        purpose: "continue from a completed run",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "resume",
        clap_name: Some("resume"),
        purpose: "resume an incomplete run",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::Control),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "kill",
        clap_name: Some("kill"),
        purpose: "stop a run, chain, or plan",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::Control),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "cleanup",
        clap_name: Some("cleanup"),
        purpose: "remove stale or completed worktrees",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::Control),
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "undo",
        clap_name: Some("undo"),
        purpose: "restore an in-place snapshot",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "abandon",
        clap_name: Some("abandon"),
        purpose: "discard a temporary worktree run",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "update",
        clap_name: Some("update"),
        purpose: "check for or route self-updates",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::LearningAdvanced),
    },
    CommandHelpEntry {
        display: "history",
        clap_name: Some("history"),
        purpose: "search run traces and provenance",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::LearningAdvanced),
    },
    CommandHelpEntry {
        display: "learn",
        clap_name: Some("learn"),
        purpose: "index run evidence and propose improvements",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::LearningAdvanced),
    },
    CommandHelpEntry {
        display: "improve",
        clap_name: Some("improve"),
        purpose: "run evidence-backed self-improvement candidates",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::LearningAdvanced),
    },
    CommandHelpEntry {
        display: "apply",
        clap_name: Some("apply"),
        purpose: "merge a completed worktree run",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "export",
        clap_name: Some("materialize"),
        purpose: "copy a completed fresh/copy run (alias: materialize)",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "doc",
        clap_name: Some("doc"),
        purpose: "read or regenerate run docs",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "library",
        clap_name: Some("library"),
        purpose: "inspect promoted artifacts",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "show",
        clap_name: Some("show"),
        purpose: "show raw state, traces, and provenance",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "import",
        clap_name: Some("import"),
        purpose: "import other tool history",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::LearningAdvanced),
    },
    CommandHelpEntry {
        display: "acceptance",
        clap_name: Some("acceptance"),
        purpose: "advanced compatibility command for done criteria",
        audience: CommandAudience::Compatibility,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "help-all",
        clap_name: Some("help-all"),
        purpose: "show every command, including advanced commands",
        audience: CommandAudience::Pseudo,
        top_group: Some(TopHelpGroup::FindMore),
        all_group: None,
    },
    CommandHelpEntry {
        display: "<command> --help",
        clap_name: None,
        purpose: "detailed help for one command",
        audience: CommandAudience::Pseudo,
        top_group: Some(TopHelpGroup::FindMore),
        all_group: None,
    },
];

const HELP_ALL_GROUPS: &[(HelpAllGroup, &str)] = &[
    (HelpAllGroup::ProductionFlow, "production flow"),
    (HelpAllGroup::SetupProviders, "setup and provider tools"),
    (HelpAllGroup::PowerUserLaunch, "power-user launch paths"),
    (HelpAllGroup::Orchestration, "orchestration building blocks"),
    (HelpAllGroup::ContinueRecover, "continue and recover"),
    (HelpAllGroup::ResultsInspect, "results and inspection"),
    (
        HelpAllGroup::LearningAdvanced,
        "history, learning, and update",
    ),
];

const HELP_ALL_DISCOVERY_NOTE: &str = "Default help shows the production model; this full map keeps every power-user and advanced command easy to find. Compatibility aliases stay inline on their canonical command row.";

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
    match entry.audience {
        CommandAudience::Advanced => CommandDiscovery::Advanced,
        CommandAudience::Compatibility => CommandDiscovery::Compatibility,
        CommandAudience::Pseudo => CommandDiscovery::Pseudo,
        CommandAudience::Primary | CommandAudience::SetupSupport => CommandDiscovery::Public,
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
    println!("{}", product::PRODUCT_AUDIENCE);
    println!("{}", product::PRODUCT_HARNESS);
    println!("{}", product::PRODUCT_NOT_PROVIDER_REPLACEMENT);
    println!();
    println!("{}", ui_heading("Usage:"));
    println!("  {}", ui_command("deadreckon [command]"));
    println!();
    println!("{}", ui_heading("Production flow:"));
    for command in TYPICAL_FLOW_COMMANDS {
        println!("  {}", ui_command(command));
    }
    println!();
    print_top_help_group("Start, watch, keep:", TopHelpGroup::StartWatchKeep);
    println!();
    print_top_help_group("Setup and health:", TopHelpGroup::SetupHealth);
    println!();
    print_top_help_group("Control:", TopHelpGroup::Control);
    println!();
    print_top_help_group("Find more:", TopHelpGroup::FindMore);
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
    println!("{}", ui_heading("deadreckon full command map"));
    if COMMAND_HELP_CATALOG
        .iter()
        .any(|entry| command_discovery(entry) == CommandDiscovery::Advanced)
    {
        println!("{}", ui_muted(HELP_ALL_DISCOVERY_NOTE));
    }
    println!();
    println!("{}", ui_heading("production flow"));
    for command in TYPICAL_FLOW_COMMANDS {
        println!("  {}", ui_command(command));
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

fn auto_subscription_cli_provider(registry: &ProviderRegistry) -> Option<String> {
    setup::auto_subscription_cli_provider(registry)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartSelectedMode {
    Extend,
    Run,
    Review,
    FullPlan,
    Campaign,
}

impl StartSelectedMode {
    fn label(self) -> &'static str {
        match self {
            Self::Extend => "extend",
            Self::Run => "run",
            Self::Review => "review",
            Self::FullPlan => "full-plan",
            Self::Campaign => "campaign",
        }
    }

    fn path_label(self) -> &'static str {
        match self {
            Self::Extend => "follow-up run",
            Self::Run => "run",
            Self::Review => "review orchestration",
            Self::FullPlan => "full-plan orchestration",
            Self::Campaign => "campaign orchestration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartSelectionSource {
    ExplicitFlag,
    GoalShape,
    Heuristic,
    InteractiveChoice,
    Default,
}

impl StartSelectionSource {
    fn label(self) -> &'static str {
        match self {
            Self::ExplicitFlag => "explicit_flag",
            Self::GoalShape => "goal_shape",
            Self::Heuristic => "heuristic",
            Self::InteractiveChoice => "interactive_choice",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GoalShape {
    Single,
    Orchestrate,
    Campaign,
}

impl GoalShape {
    fn label(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Orchestrate => "orchestrate",
            Self::Campaign => "campaign",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GoalShapeSource {
    Provider,
    Fallback,
}

impl GoalShapeSource {
    fn label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GoalShapeRecommendation {
    schema_version: u8,
    goal: String,
    shape: GoalShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u8>,
    rationale: String,
    source: GoalShapeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartProviderSource {
    ExplicitFlag,
    Configured,
    Detected,
    Interactive,
    Missing,
}

impl StartProviderSource {
    fn label(self) -> &'static str {
        match self {
            Self::ExplicitFlag => "explicit_flag",
            Self::Configured => "configured",
            Self::Detected => "detected",
            Self::Interactive => "interactive",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartDoneCriteriaSource {
    Project,
    Generated,
    Manual,
    DefaultGate,
    Missing,
}

impl StartDoneCriteriaSource {
    fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Generated => "generated",
            Self::Manual => "manual",
            Self::DefaultGate => "default",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartSourceMode {
    ParentArtifact,
    Worktree,
    InitGit,
    Copy,
    Fresh,
    Missing,
}

impl StartSourceMode {
    fn label(self) -> &'static str {
        match self {
            Self::ParentArtifact => "parent-artifact",
            Self::Worktree => "worktree",
            Self::InitGit => "init-git",
            Self::Copy => "copy",
            Self::Fresh => "fresh",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartDoneAction {
    Existing,
    GenerateFromGoal,
    ManualText {
        text: String,
        overwrite_existing: bool,
    },
    DefaultGate,
    Missing,
}

trait StartPrompter {
    fn select_one(&mut self, prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice>;
    fn confirm(&mut self, question: &str, default_yes: bool) -> Result<bool>;
    fn input(&mut self, message: &str, default: Option<&str>) -> Result<String>;
}

struct TerminalStartPrompter;

impl StartPrompter for TerminalStartPrompter {
    fn select_one(&mut self, prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice> {
        prompt::select_one(&prompt)
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> Result<bool> {
        prompt::confirm(question, default_yes)
    }

    fn input(&mut self, message: &str, default: Option<&str>) -> Result<String> {
        prompt::open(message, default)
    }
}

#[derive(Debug, Clone, Copy)]
struct StartPromptEligibility {
    stdin_is_tty: bool,
    json: bool,
    plain: bool,
    quiet: bool,
    yes: bool,
}

impl StartPromptEligibility {
    fn from_args(args: &StartCommandArgs, stdin_is_tty: bool) -> Self {
        Self {
            stdin_is_tty,
            json: args.json,
            plain: args.plain,
            quiet: args.quiet,
            yes: args.yes,
        }
    }

    fn allows_prompts(self) -> bool {
        self.stdin_is_tty && !self.json && !self.plain && !self.quiet && !self.yes
    }
}

#[derive(Debug, Clone, Copy)]
struct StartLaunchInput<'a> {
    goal: &'a str,
    requested_mode: crate::cli::CliStartMode,
    stdin_is_tty: bool,
}

#[derive(Debug, Clone)]
struct StartLaunchDecision {
    goal: String,
    selected_mode: StartSelectedMode,
    selection_source: StartSelectionSource,
    reason: String,
    provider_source: StartProviderSource,
    provider_route: Option<String>,
    provider_label: String,
    child_count: Option<u8>,
    planner_provider_route: Option<String>,
    child_provider_route: Option<String>,
    child_provider_overrides: Vec<String>,
    coder_provider_route: Option<String>,
    reviewer_provider_route: Option<String>,
    done_criteria_source: StartDoneCriteriaSource,
    done_action: StartDoneAction,
    done_criteria_label: String,
    source_mode: StartSourceMode,
    source_mode_label: String,
    source_fresh: bool,
    source_worktree: bool,
    source_from: Option<PathBuf>,
    source_init_git: bool,
    source_allow_dirty: bool,
    base_run_id: Option<String>,
    base_run_label: Option<String>,
    history_action_label: Option<String>,
    history_next_actions: Vec<String>,
    goal_shape: Option<GoalShapeRecommendation>,
    requires_confirmation: bool,
    confirmed_by_start_picker: bool,
    try_lines: Vec<String>,
    recovery: Option<StartRecovery>,
}

#[derive(Debug, Clone)]
struct StartRecovery {
    message: String,
    try_lines: Vec<String>,
}

fn start_launch_decision(input: StartLaunchInput<'_>) -> StartLaunchDecision {
    let (selected_mode, selection_source, reason) = match input.requested_mode {
        crate::cli::CliStartMode::Run => (
            StartSelectedMode::Run,
            StartSelectionSource::ExplicitFlag,
            "explicit --mode run selected one supervised coding run".to_string(),
        ),
        crate::cli::CliStartMode::Review => (
            StartSelectedMode::Review,
            StartSelectionSource::ExplicitFlag,
            "explicit --mode review selected coder/reviewer orchestration".to_string(),
        ),
        crate::cli::CliStartMode::FullPlan => (
            StartSelectedMode::FullPlan,
            StartSelectionSource::ExplicitFlag,
            "explicit --mode full-plan selected multi-agent planning".to_string(),
        ),
        crate::cli::CliStartMode::Auto => start_auto_mode_decision(input.goal, input.stdin_is_tty),
    };
    StartLaunchDecision {
        goal: input.goal.to_string(),
        selected_mode,
        selection_source,
        reason,
        provider_source: StartProviderSource::Missing,
        provider_route: None,
        provider_label: StartProviderSource::Missing.label().to_string(),
        child_count: None,
        planner_provider_route: None,
        child_provider_route: None,
        child_provider_overrides: Vec::new(),
        coder_provider_route: None,
        reviewer_provider_route: None,
        done_criteria_source: StartDoneCriteriaSource::Missing,
        done_action: StartDoneAction::Missing,
        done_criteria_label: StartDoneCriteriaSource::Missing.label().to_string(),
        source_mode: StartSourceMode::Missing,
        source_mode_label: StartSourceMode::Missing.label().to_string(),
        source_fresh: false,
        source_worktree: false,
        source_from: None,
        source_init_git: false,
        source_allow_dirty: false,
        base_run_id: None,
        base_run_label: None,
        history_action_label: None,
        history_next_actions: Vec::new(),
        goal_shape: None,
        requires_confirmation: false,
        confirmed_by_start_picker: false,
        try_lines: Vec::new(),
        recovery: None,
    }
}

fn start_auto_mode_decision(
    goal: &str,
    stdin_is_tty: bool,
) -> (StartSelectedMode, StartSelectionSource, String) {
    let lower = goal.to_ascii_lowercase();
    if start_goal_recommends_review(&lower) {
        return (
            StartSelectedMode::Review,
            StartSelectionSource::Heuristic,
            "goal asks for review, hardening, validation, or a second pass".to_string(),
        );
    }
    if start_goal_recommends_full_plan(&lower) {
        return (
            StartSelectedMode::FullPlan,
            StartSelectionSource::Heuristic,
            "goal names parallel or separable workstreams that fit full-plan orchestration"
                .to_string(),
        );
    }
    if !stdin_is_tty {
        return (
            StartSelectedMode::Run,
            StartSelectionSource::Default,
            "non-interactive auto mode uses the conservative single supervised run".to_string(),
        );
    }
    (
        StartSelectedMode::Run,
        StartSelectionSource::Default,
        "goal looks focused enough for a single supervised run".to_string(),
    )
}

#[derive(Debug, Deserialize)]
struct ProviderGoalShapeDraft {
    shape: String,
    #[serde(default)]
    n: Option<u8>,
    #[serde(default)]
    rationale: Option<String>,
}

fn start_goal_shape_should_classify(
    args: &StartCommandArgs,
    eligibility: StartPromptEligibility,
) -> bool {
    matches!(args.mode, crate::cli::CliStartMode::Auto)
        && (eligibility.allows_prompts() || args.preview || args.json)
}

fn start_goal_shape_provider_route(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    args: &StartCommandArgs,
) -> Option<String> {
    goal_shape_provider_route(paths, defaults, args.provider.as_deref())
}

fn goal_shape_provider_route(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    explicit_provider: Option<&str>,
) -> Option<String> {
    if let Some(provider) = explicit_provider {
        return Some(provider.to_string());
    }
    provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::Planner,
            explicit_provider: None,
            explicit_model: None,
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: true,
        },
    )
    .ok()
    .and_then(|selection| selection.provider)
}

async fn classify_goal_shape_for_start(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    provider: Option<&str>,
    plain: bool,
) -> GoalShapeRecommendation {
    if let Some(provider) = provider
        && provider != "smoke"
        && !provider.starts_with("smoke:")
        && let Some(recommendation) =
            provider_goal_shape_recommendation(paths, cwd, goal, provider, plain).await
    {
        return recommendation;
    }
    fallback_goal_shape_recommendation(goal)
}

async fn provider_goal_shape_recommendation(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    provider: &str,
    plain: bool,
) -> Option<GoalShapeRecommendation> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(provider)).ok()?;
    let request = ProviderRequest {
        prompt: goal_shape_prompt(goal),
        max_output_tokens: 512,
        cwd: Some(cwd.to_path_buf()),
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        maybe_with_cli_wait_status(!plain, "classifying goal shape", router.complete(&request)),
    )
    .await
    .ok()?
    .ok()?;
    parse_provider_goal_shape(goal, &response.provider, &response.content)
}

fn goal_shape_prompt(goal: &str) -> String {
    format!(
        "You are a read-only goal-shape classifier for deadreckon. Do not write files, create temporary files, install packages, commit, delete, move, or mutate state.\n\nReturn JSON only: {{\"shape\":\"single|orchestrate|campaign\",\"n\":2,\"rationale\":\"one short line\"}}.\n\nRubric:\n- single: one cohesive change a single supervised run handles.\n- orchestrate: one project with parallelizable subtasks.\n- campaign: several independent projects, each warranting its own coordination.\n\nIf shape is orchestrate or campaign, include n from 2 through 6. Keep rationale short. Goal: {goal}"
    )
}

fn parse_provider_goal_shape(
    goal: &str,
    provider: &str,
    content: &str,
) -> Option<GoalShapeRecommendation> {
    let parsed = serde_json::from_str::<ProviderGoalShapeDraft>(content)
        .ok()
        .or_else(|| {
            commands::plan::json_slice(content, '{', '}')
                .and_then(|slice| serde_json::from_str::<ProviderGoalShapeDraft>(slice).ok())
        })?;
    let shape = parse_goal_shape(&parsed.shape)?;
    let rationale = parsed.rationale.unwrap_or_default().trim().to_string();
    if rationale.is_empty() {
        return None;
    }
    let n = goal_shape_count(shape, parsed.n);
    Some(GoalShapeRecommendation {
        schema_version: 1,
        goal: goal.to_string(),
        shape,
        n,
        rationale,
        source: GoalShapeSource::Provider,
        provider: Some(provider.to_string()),
    })
}

fn parse_goal_shape(value: &str) -> Option<GoalShape> {
    match value.trim().to_ascii_lowercase().as_str() {
        "single" | "run" => Some(GoalShape::Single),
        "orchestrate" | "orchestration" | "full-plan" | "full_plan" => Some(GoalShape::Orchestrate),
        "campaign" => Some(GoalShape::Campaign),
        _ => None,
    }
}

fn goal_shape_count(shape: GoalShape, n: Option<u8>) -> Option<u8> {
    match shape {
        GoalShape::Single => None,
        GoalShape::Orchestrate | GoalShape::Campaign => Some(n.unwrap_or(3).clamp(2, 6)),
    }
}

fn fallback_goal_shape_recommendation(goal: &str) -> GoalShapeRecommendation {
    let lower = goal.to_ascii_lowercase();
    let (shape, n, rationale) = if start_goal_recommends_full_plan(&lower) {
        (
            GoalShape::Orchestrate,
            Some(commands::orchestrate::recommend_child_count_for_goal(
                goal,
                CliPlanMode::FullPlan,
            )),
            "goal names parallel or separable workstreams".to_string(),
        )
    } else {
        let clauses = deterministic_campaign_clause_count(goal);
        if clauses >= 2 {
            (
                GoalShape::Campaign,
                Some((clauses as u8).clamp(2, 6)),
                format!("goal reads as {clauses} independent clauses"),
            )
        } else {
            (
                GoalShape::Single,
                None,
                format!("goal looks focused enough for one {NOUN_VERIFIED_RUN}"),
            )
        }
    };
    GoalShapeRecommendation {
        schema_version: 1,
        goal: goal.to_string(),
        shape,
        n,
        rationale,
        source: GoalShapeSource::Fallback,
        provider: None,
    }
}

fn deterministic_campaign_clause_count(goal: &str) -> usize {
    let lower = goal.to_ascii_lowercase();
    let normalized = lower
        .replace(", and ", "|")
        .replace(" and ", "|")
        .replace(" then ", "|")
        .replace([';', ','], "|");
    normalized
        .split('|')
        .map(str::trim)
        .filter(|clause| goal_shape_clause_is_nounish(clause))
        .count()
}

fn goal_shape_clause_is_nounish(clause: &str) -> bool {
    const STOP: &[&str] = &[
        "a", "an", "and", "as", "build", "create", "do", "fix", "for", "make", "the", "then", "to",
        "with",
    ];
    clause
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| word.len() >= 3)
        .any(|word| !STOP.contains(&word))
}

fn goal_shape_to_start_mode(shape: GoalShape) -> StartSelectedMode {
    match shape {
        GoalShape::Single => StartSelectedMode::Run,
        GoalShape::Orchestrate => StartSelectedMode::FullPlan,
        GoalShape::Campaign => StartSelectedMode::Campaign,
    }
}

fn apply_goal_shape_recommendation(
    decision: &mut StartLaunchDecision,
    recommendation: GoalShapeRecommendation,
) {
    if matches!(decision.selected_mode, StartSelectedMode::Review) {
        decision.goal_shape = Some(recommendation);
        return;
    }
    decision.selected_mode = goal_shape_to_start_mode(recommendation.shape);
    decision.selection_source = StartSelectionSource::GoalShape;
    decision.reason = format!(
        "{} suggested {}: {}",
        recommendation.source.label(),
        recommendation.shape.label(),
        recommendation.rationale
    );
    if matches!(
        recommendation.shape,
        GoalShape::Orchestrate | GoalShape::Campaign
    ) {
        decision.child_count = recommendation.n;
    }
    decision.goal_shape = Some(recommendation);
}

fn goal_shape_preview_path(paths: &DeadreckonPaths, scope: &str, goal: &str) -> PathBuf {
    paths
        .scope_root(scope)
        .join("preview")
        .join(format!("{}.json", task_key(goal)))
}

fn write_goal_shape_preview_record(
    paths: &DeadreckonPaths,
    scope: &str,
    recommendation: &GoalShapeRecommendation,
) -> Result<()> {
    let path = goal_shape_preview_path(paths, scope, &recommendation.goal);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(recommendation).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )?;
    Ok(())
}

fn start_goal_recommends_review(lower_goal: &str) -> bool {
    let words = [
        "review",
        "audit",
        "critique",
        "validate",
        "validation",
        "verify",
        "verification",
        "hardening",
        "harden",
        "cleanup",
    ];
    let phrases = ["second pass", "second-pass", "clean up"];
    words
        .iter()
        .any(|word| start_goal_contains_word(lower_goal, word))
        || phrases.iter().any(|phrase| lower_goal.contains(phrase))
}

fn start_goal_recommends_full_plan(lower_goal: &str) -> bool {
    let words = [
        "parallel",
        "parallelize",
        "workstream",
        "workstreams",
        "separable",
    ];
    let phrases = [
        "multiple independent",
        "many modules",
        "several modules",
        "frontend, docs",
        "api, frontend",
    ];
    words
        .iter()
        .any(|word| start_goal_contains_word(lower_goal, word))
        || phrases.iter().any(|phrase| lower_goal.contains(phrase))
}

fn start_goal_contains_word(lower_goal: &str, needle: &str) -> bool {
    lower_goal
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == needle)
}

fn maybe_prompt_start_mode(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    latest_completed_run: Option<&RunListEntry>,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    if !matches!(args.mode, crate::cli::CliStartMode::Auto) || decision.recovery.is_some() {
        return Ok(());
    }
    let recommended = decision.selected_mode;
    let mut choices = vec![
        prompt::SelectChoice::with_detail(
            "recommended",
            format!("Recommended: {}", recommended.path_label()),
            decision.reason.clone(),
        ),
        prompt::SelectChoice::with_detail(
            "run",
            "New single supervised run",
            "equivalent to --mode run",
        ),
    ];
    if let Some(run) = latest_completed_run {
        choices.push(prompt::SelectChoice::with_detail(
            format!("extend:{}", run.run_id),
            format!("Follow up from {}", run_prefix(&run.run_id)),
            format!("extends completed run: {}", run.goal),
        ));
    }
    choices.extend([
        prompt::SelectChoice::with_detail(
            "review",
            "New coder/reviewer pass",
            "equivalent to --mode review",
        ),
        prompt::SelectChoice::with_detail(
            "full-plan",
            "New full-plan pass",
            "equivalent to --mode full-plan",
        ),
        prompt::SelectChoice::with_detail(
            "campaign",
            "New campaign pass",
            "split independent projects into sub-orchestrators",
        ),
        prompt::SelectChoice::new("cancel", "Cancel"),
    ]);
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose launch path".to_string(),
        help: Some("Pick how DeadReckon should shape this goal.".to_string()),
        choices,
        default_index: 0,
    })?;
    match choice.id.as_str() {
        "recommended" => {
            decision.selection_source = StartSelectionSource::InteractiveChoice;
        }
        "run" => {
            decision.selected_mode = StartSelectedMode::Run;
            decision.selection_source = StartSelectionSource::InteractiveChoice;
            decision.reason = "interactive picker selected one supervised coding run".to_string();
        }
        choice if choice.starts_with("extend:") => {
            let run_id = choice["extend:".len()..].to_string();
            decision.selected_mode = StartSelectedMode::Extend;
            decision.selection_source = StartSelectionSource::InteractiveChoice;
            decision.reason =
                "interactive picker selected a follow-up from prior history".to_string();
            decision.base_run_label = Some(format!("run {}", run_prefix(&run_id)));
            decision.base_run_id = Some(run_id);
            decision.source_mode = StartSourceMode::ParentArtifact;
            decision.source_mode_label = "parent artifact".to_string();
        }
        "review" => {
            decision.selected_mode = StartSelectedMode::Review;
            decision.selection_source = StartSelectionSource::InteractiveChoice;
            decision.reason =
                "interactive picker selected coder/reviewer orchestration".to_string();
        }
        "full-plan" => {
            decision.selected_mode = StartSelectedMode::FullPlan;
            decision.selection_source = StartSelectionSource::InteractiveChoice;
            decision.reason = "interactive picker selected full-plan orchestration".to_string();
        }
        "campaign" => {
            decision.selected_mode = StartSelectedMode::Campaign;
            decision.selection_source = StartSelectionSource::InteractiveChoice;
            decision.reason = "interactive picker selected campaign orchestration".to_string();
        }
        _ => set_start_recovery(
            decision,
            "guided start cancelled before choosing a launch path",
            vec![format!(
                "deadreckon start \"{}\"",
                shell_display_quote(&decision.goal)
            )],
        ),
    }
    Ok(())
}

fn start_prompt_choice(
    id: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
) -> prompt::SelectChoice {
    prompt::SelectChoice::with_detail(id, label, detail)
}

fn start_detected_cli_provider_ids(paths: &DeadreckonPaths) -> Result<Vec<String>> {
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let mut ids = Vec::new();
    for descriptor in registry.iter() {
        if descriptor.kind == DescriptorKind::Cli
            && descriptor.subscription
            && descriptor
                .default_binary
                .as_deref()
                .is_some_and(command_exists)
        {
            push_unique(&mut ids, descriptor.id.clone());
        }
    }
    Ok(ids)
}

fn start_configured_provider_ids(paths: &DeadreckonPaths) -> Vec<String> {
    let Ok(config) = read_config(&paths.config_path()) else {
        return Vec::new();
    };
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
    ids
}

fn command_exists_in_paths(command: &str, paths: Option<std::ffi::OsString>) -> bool {
    let explicit = PathBuf::from(command);
    if explicit.components().count() > 1 {
        return explicit.is_file();
    }
    let Some(paths) = paths else {
        return false;
    };
    std::env::split_paths(&paths).any(|path| path.join(command).is_file())
}

fn command_exists(command: &str) -> bool {
    command_exists_in_paths(command, std::env::var_os("PATH"))
}

fn start_latest_extendable_run(
    paths: &DeadreckonPaths,
    cwd: &Path,
) -> Result<Option<RunListEntry>> {
    let scope = workspace_scope(cwd).map_err(CliError::from)?;
    let mut runs = list_runs(paths, Some(scope.as_str()))?
        .into_iter()
        .filter(|run| run.status == RunStatus::Completed)
        .filter(|run| start_run_is_extendable(paths, run))
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.updated_at);
    Ok(runs.pop())
}

fn start_run_is_extendable(paths: &DeadreckonPaths, run: &RunListEntry) -> bool {
    let Ok(state) = load_run(paths, &run.run_id) else {
        return false;
    };
    if !paths.library_dir(&state.scope, &state.run_id).is_dir() {
        return false;
    }
    !read_run_codebase_record(paths, &state)
        .ok()
        .is_some_and(|record| record.mode == CodebaseMode::InPlace)
}

fn add_start_history_actions(decision: &mut StartLaunchDecision, run: Option<&RunListEntry>) {
    let Some(run) = run else {
        return;
    };
    let prefix = run_prefix(&run.run_id);
    let goal = shell_display_quote(&decision.goal);
    let actions = vec![
        format!("deadreckon extend {prefix} \"{goal}\""),
        format!("deadreckon start \"{goal}\" --mode review --yes"),
        format!("deadreckon start \"{goal}\" --mode full-plan --yes"),
    ];
    decision.history_action_label = Some(format!(
        "extend: {}; review: {}; full-plan: {}",
        actions[0], actions[1], actions[2]
    ));
    decision.history_next_actions = actions;
}

fn start_provider_picker_choices(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    current: Option<&str>,
) -> Result<Vec<prompt::SelectChoice>> {
    let mut choices = Vec::new();
    let mut seen = Vec::new();
    if let Some(provider) = current {
        push_unique(&mut seen, provider.to_string());
        choices.push(start_prompt_choice(
            format!("route:{provider}"),
            format!("Use current route {provider}"),
            "selected for this guided start",
        ));
    }
    if let Some(provider) = defaults.provider.as_deref() {
        if seen.iter().any(|seen| seen == provider) {
            // The current route row is more specific than the config row.
        } else {
            push_unique(&mut seen, provider.to_string());
            choices.push(start_prompt_choice(
                format!("route:{provider}"),
                format!("Use configured default {provider}"),
                "current DeadReckon default provider",
            ));
        }
    }
    for provider in start_detected_cli_provider_ids(paths)? {
        if seen.iter().any(|seen| seen == &provider) {
            continue;
        }
        push_unique(&mut seen, provider.clone());
        choices.push(start_prompt_choice(
            format!("route:{provider}"),
            format!("Use detected CLI {provider}"),
            "ephemeral for this launch; config is not changed",
        ));
    }
    for provider in start_configured_provider_ids(paths) {
        if seen.iter().any(|seen| seen == &provider) {
            continue;
        }
        push_unique(&mut seen, provider.clone());
        choices.push(start_prompt_choice(
            format!("route:{provider}"),
            format!("Use configured route {provider}"),
            "ephemeral for this launch",
        ));
    }
    choices.push(start_prompt_choice(
        "typed",
        "Type another provider route",
        "advanced escape hatch",
    ));
    choices.push(prompt::SelectChoice::new(
        "cancel",
        "Cancel and show setup commands",
    ));
    Ok(choices)
}

fn prompt_start_provider(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let previous_route = decision.provider_route.clone();
    let previous_source = decision.provider_source;
    let Some(provider) = prompt_start_provider_route(
        decision,
        paths,
        defaults,
        setup::SetupProviderRoleRef::PrimaryRun,
        "Choose provider",
        "Pick the provider route for this launch. Defaults are not changed.",
        prompter,
    )?
    else {
        return Ok(());
    };

    decision.provider_source = if previous_route.as_deref() == Some(provider.as_str())
        && !matches!(previous_source, StartProviderSource::Detected)
    {
        previous_source
    } else {
        StartProviderSource::Interactive
    };
    decision.provider_route = Some(provider.clone());
    decision.provider_label = format!("{provider} ({})", decision.provider_source.label());
    Ok(())
}

fn prompt_start_provider_route(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    role: setup::SetupProviderRoleRef,
    title: &str,
    help: &str,
    prompter: &mut dyn StartPrompter,
) -> Result<Option<String>> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: title.to_string(),
        help: Some(help.to_string()),
        choices: start_provider_picker_choices(
            paths,
            defaults,
            decision.provider_route.as_deref(),
        )?,
        default_index: 0,
    })?;
    let route = if let Some(route) = choice.id.strip_prefix("route:") {
        route.to_string()
    } else if choice.id == "typed" {
        let route = prompter.input("provider route: ", None)?;
        if route.trim().is_empty() {
            set_start_recovery(
                decision,
                "no provider route selected",
                vec!["deadreckon providers list --all".to_string()],
            );
            return Ok(None);
        }
        route.trim().to_string()
    } else {
        set_start_recovery(
            decision,
            "provider setup is incomplete",
            vec![
                "deadreckon init".to_string(),
                "deadreckon detect".to_string(),
                "deadreckon providers list --all".to_string(),
            ],
        );
        return Ok(None);
    };

    let selection = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role,
            explicit_provider: Some(&route),
            explicit_model: None,
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: false,
            require_usable_route: true,
        },
    )?;
    Ok(Some(selection.provider.unwrap_or(route)))
}

fn resolve_explicit_start_provider(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    role: setup::SetupProviderRoleRef,
    route: &str,
) -> Result<String> {
    let selection = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role,
            explicit_provider: Some(route),
            explicit_model: None,
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: false,
            require_usable_route: true,
        },
    )?;
    Ok(selection.provider.unwrap_or_else(|| route.to_string()))
}

fn prompt_start_role_provider(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    role: setup::SetupProviderRoleRef,
    role_label: &str,
    prompter: &mut dyn StartPrompter,
) -> Result<Option<String>> {
    let title = format!("Choose {role_label} provider");
    let help =
        format!("Pick the {role_label} provider route for this launch. Defaults are not changed.");
    prompt_start_provider_route(decision, paths, defaults, role, &title, &help, prompter)
}

fn prompt_start_child_count(
    decision: &mut StartLaunchDecision,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let recommended = commands::orchestrate::recommend_child_count_for_goal(
        &decision.goal,
        CliPlanMode::FullPlan,
    );
    let mut choices = vec![start_prompt_choice(
        format!("n:{recommended}"),
        format!("Recommended: {recommended} children"),
        "based on goal complexity",
    )];
    for n in 2..=6 {
        choices.push(start_prompt_choice(
            format!("n:{n}"),
            format!("{n} children"),
            "full-plan child count",
        ));
    }
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose child count".to_string(),
        help: Some(
            "Pick how many implementation children the full-plan planner should create."
                .to_string(),
        ),
        choices,
        default_index: 0,
    })?;
    let Some(raw) = choice.id.strip_prefix("n:") else {
        set_start_recovery(
            decision,
            "guided start cancelled before choosing child count",
            vec![format!(
                "deadreckon start \"{}\" --mode full-plan --children {recommended}",
                shell_display_quote(&decision.goal)
            )],
        );
        return Ok(());
    };
    let n = raw.parse::<u8>().map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("child count is not a number: {raw}"),
            "enter a value from 2 through 6",
        ))
    })?;
    validate_task_count(usize::from(n)).map_err(CliError::Core)?;
    decision.child_count = Some(n);
    Ok(())
}

fn prompt_start_child_provider_overrides(
    decision: &mut StartLaunchDecision,
    n: u8,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose child provider overrides".to_string(),
        help: Some(format!(
            "Optional per-child routes. Child indexes are 0 through {}.",
            n.saturating_sub(1)
        )),
        choices: vec![
            start_prompt_choice(
                "none",
                "No per-child overrides",
                "all children use the default child provider",
            ),
            start_prompt_choice(
                "typed",
                "Type overrides",
                "comma-separated IDX=PROVIDER entries, for example 1=cli:codex",
            ),
            prompt::SelectChoice::new("cancel", "Cancel"),
        ],
        default_index: 0,
    })?;
    match choice.id.as_str() {
        "none" => {
            decision.child_provider_overrides.clear();
            Ok(())
        }
        "typed" => {
            let answer = prompter.input("child provider overrides: ", None)?;
            let overrides = answer
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            commands::plan::parse_child_provider_overrides(&overrides, n)?;
            decision.child_provider_overrides = overrides;
            Ok(())
        }
        _ => {
            set_start_recovery(
                decision,
                "guided start cancelled before choosing child provider overrides",
                vec![format!(
                    "deadreckon start \"{}\" --mode full-plan --yes",
                    shell_display_quote(&decision.goal)
                )],
            );
            Ok(())
        }
    }
}

fn resolve_start_orchestration_options(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    mut prompter: Option<&mut dyn StartPrompter>,
) -> Result<()> {
    if matches!(
        decision.selected_mode,
        StartSelectedMode::Run | StartSelectedMode::Extend
    ) {
        if start_orchestration_flags_present(args) {
            set_start_recovery(
                decision,
                "orchestration options require start --mode review or --mode full-plan",
                vec![format!(
                    "deadreckon start \"{}\" --mode full-plan --preview",
                    shell_display_quote(&decision.goal)
                )],
            );
        }
        return Ok(());
    }

    match decision.selected_mode {
        StartSelectedMode::FullPlan | StartSelectedMode::Campaign => {
            if let Some(n) = args.children {
                validate_task_count(usize::from(n)).map_err(CliError::Core)?;
                decision.child_count = Some(n);
            } else if decision.child_count.is_none() {
                if let Some(prompter) = prompter.as_mut() {
                    prompt_start_child_count(decision, &mut **prompter)?;
                    if decision.recovery.is_some() {
                        return Ok(());
                    }
                } else {
                    decision.child_count =
                        Some(commands::orchestrate::recommend_child_count_for_goal(
                            &decision.goal,
                            CliPlanMode::FullPlan,
                        ));
                }
            }

            if let Some(route) = args.planner_provider.as_deref() {
                decision.planner_provider_route = Some(resolve_explicit_start_provider(
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Planner,
                    route,
                )?);
            } else if args.provider.is_none()
                && let Some(prompter) = prompter.as_mut()
            {
                decision.planner_provider_route = prompt_start_role_provider(
                    decision,
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Planner,
                    "planner",
                    &mut **prompter,
                )?;
                if decision.recovery.is_some() {
                    return Ok(());
                }
            }

            if args.provider.is_none()
                && let Some(prompter) = prompter.as_mut()
            {
                decision.child_provider_route = prompt_start_role_provider(
                    decision,
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::DefaultChild,
                    "default child",
                    &mut **prompter,
                )?;
                if decision.recovery.is_some() {
                    return Ok(());
                }
            }

            if matches!(decision.selected_mode, StartSelectedMode::Campaign)
                && !args.child_provider.is_empty()
            {
                set_start_recovery(
                    decision,
                    "per-child provider overrides are only supported by start --mode full-plan",
                    vec![format!(
                        "deadreckon campaign \"{}\" --provider <provider>",
                        shell_display_quote(&decision.goal)
                    )],
                );
                return Ok(());
            }

            if matches!(decision.selected_mode, StartSelectedMode::FullPlan)
                && !args.child_provider.is_empty()
            {
                let n = decision.child_count.unwrap_or_else(|| {
                    commands::orchestrate::recommend_child_count_for_goal(
                        &decision.goal,
                        CliPlanMode::FullPlan,
                    )
                });
                commands::plan::parse_child_provider_overrides(&args.child_provider, n)?;
                decision.child_provider_overrides = args.child_provider.clone();
            } else if matches!(decision.selected_mode, StartSelectedMode::FullPlan)
                && let Some(prompter) = prompter.as_mut()
            {
                let n = decision.child_count.unwrap_or_else(|| {
                    commands::orchestrate::recommend_child_count_for_goal(
                        &decision.goal,
                        CliPlanMode::FullPlan,
                    )
                });
                prompt_start_child_provider_overrides(decision, n, &mut **prompter)?;
            }
        }
        StartSelectedMode::Review => {
            if args.children.is_some()
                || args.planner_provider.is_some()
                || !args.child_provider.is_empty()
            {
                set_start_recovery(
                    decision,
                    "full-plan options cannot be used with start --mode review",
                    vec![format!(
                        "deadreckon start \"{}\" --mode full-plan --preview",
                        shell_display_quote(&decision.goal)
                    )],
                );
                return Ok(());
            }
            if let Some(route) = args.coder_provider.as_deref() {
                decision.coder_provider_route = Some(resolve_explicit_start_provider(
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Coder,
                    route,
                )?);
            } else if args.provider.is_none()
                && let Some(prompter) = prompter.as_mut()
            {
                decision.coder_provider_route = prompt_start_role_provider(
                    decision,
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Coder,
                    "coder",
                    &mut **prompter,
                )?;
                if decision.recovery.is_some() {
                    return Ok(());
                }
            }
            if let Some(route) = args.reviewer_provider.as_deref() {
                decision.reviewer_provider_route = Some(resolve_explicit_start_provider(
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Reviewer,
                    route,
                )?);
            } else if args.provider.is_none()
                && let Some(prompter) = prompter.as_mut()
            {
                decision.reviewer_provider_route = prompt_start_role_provider(
                    decision,
                    paths,
                    defaults,
                    setup::SetupProviderRoleRef::Reviewer,
                    "reviewer",
                    &mut **prompter,
                )?;
            }
        }
        StartSelectedMode::Extend | StartSelectedMode::Run => {}
    }
    Ok(())
}

fn prompt_start_done_criteria(
    decision: &mut StartLaunchDecision,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: format!("Choose {NOUN_DONE_CONTRACT}"),
        help: Some(format!("No project {NOUN_DONE_CONTRACT} was found.")),
        choices: vec![
            start_prompt_choice(
                "default",
                "Use the default gate for this launch",
                "working directory exists, or cargo test for Rust projects",
            ),
            start_prompt_choice(
                "generate",
                "Create from the goal before launch",
                "uses the existing def-done compiler after final confirmation",
            ),
            start_prompt_choice(
                "manual",
                "Write criteria in English",
                "compiled through the existing def-done flow after confirmation",
            ),
            prompt::SelectChoice::new("cancel", "Cancel and show def-done command"),
        ],
        default_index: 0,
    })?;
    match choice.id.as_str() {
        "default" => {
            decision.done_criteria_source = StartDoneCriteriaSource::DefaultGate;
            decision.done_action = StartDoneAction::DefaultGate;
            decision.done_criteria_label = "default dr-gate behavior".to_string();
        }
        "generate" => {
            decision.done_criteria_source = StartDoneCriteriaSource::Generated;
            decision.done_action = StartDoneAction::GenerateFromGoal;
            decision.done_criteria_label = "create from goal before launch".to_string();
        }
        "manual" => {
            let text = prompter.input("definition of done: ", None)?;
            if text.trim().is_empty() {
                set_start_recovery(
                    decision,
                    format!("empty {NOUN_DONE_CONTRACT} was not saved"),
                    vec![format!(
                        "deadreckon def-done \"what should count as done\" && deadreckon start \"{}\"",
                        shell_display_quote(&decision.goal)
                    )],
                );
                return Ok(());
            }
            decision.done_criteria_source = StartDoneCriteriaSource::Manual;
            decision.done_action = StartDoneAction::ManualText {
                text: text.trim().to_string(),
                overwrite_existing: false,
            };
            decision.done_criteria_label = "write manual criteria before launch".to_string();
        }
        _ => set_start_recovery(
            decision,
            format!("{NOUN_DONE_CONTRACT} is missing for this repo"),
            vec![format!(
                "deadreckon def-done \"what should count as done\" && deadreckon start \"{}\"",
                shell_display_quote(&decision.goal)
            )],
        ),
    }
    Ok(())
}

fn done_criteria_inspection_try_lines(selection: &setup::DoneCriteriaSelection) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = selection.path.as_ref() {
        lines.push(format!(
            "deadreckon def-done show --spec {}",
            path.display()
        ));
        lines.push(format!(
            "deadreckon def-done check --spec {}",
            path.display()
        ));
    } else {
        lines.push("deadreckon def-done show".to_string());
        lines.push("deadreckon def-done check".to_string());
    }
    lines.push("deadreckon def-done \"what should count as done\"".to_string());
    lines
}

fn done_criteria_prompt_detail(selection: &setup::DoneCriteriaSelection) -> String {
    let checks = selection
        .checks
        .map(|checks| format!("{checks} check(s)"))
        .unwrap_or_else(|| {
            "working directory exists, or cargo test when Cargo.toml is present".to_string()
        });
    match selection.path.as_ref() {
        Some(path) => format!("{} from {}", checks, path.display()),
        None => checks,
    }
}

fn print_start_done_criteria_summary(selection: &setup::DoneCriteriaSelection) {
    println!("{}", ui_heading(NOUN_DONE_CONTRACT));
    print_kv_block(&[
        ("source", selection.source.as_str()),
        ("summary", &done_criteria_prompt_detail(selection)),
        ("view", "deadreckon def-done show"),
        ("check", "deadreckon def-done check"),
        (
            "update",
            "deadreckon def-done \"what should count as done\"",
        ),
    ]);
}

fn check_start_done_criteria(cwd: &Path, selection: &setup::DoneCriteriaSelection) -> Result<()> {
    let temp_root = std::env::temp_dir().join(format!(
        "deadreckon-start-done-check-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&temp_root)?;
    if let Some(path) = selection.path.as_ref() {
        fs::copy(path, acceptance_spec_path_for_run_root(&temp_root))?;
    }
    let result = evaluate_acceptance_checks(&temp_root, cwd);
    let _ = fs::remove_dir_all(&temp_root);
    println!("{}", ui_heading(format!("{NOUN_DONE_CONTRACT} check")));
    match result {
        Ok(results) => {
            let failed_required = results
                .iter()
                .any(|result| result.must_pass && !result.passed);
            if failed_required {
                println!("{}", ui_status(format!("{NOUN_DONE_CONTRACT} failed")));
            } else {
                println!("{}", ui_ok(format!("{NOUN_DONE_CONTRACT} passed")));
            }
            commands::acceptance::print_acceptance_results(&results);
        }
        Err(err) => {
            println!(
                "{}",
                ui_warn(format!("{NOUN_DONE_CONTRACT} check could not run: {err}"))
            );
        }
    }
    Ok(())
}

fn prompt_start_existing_done_criteria(
    decision: &mut StartLaunchDecision,
    cwd: &Path,
    selection: &setup::DoneCriteriaSelection,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    loop {
        let choice = prompter.select_one(prompt::SelectPrompt {
            title: format!("Review {NOUN_DONE_CONTRACT}"),
            help: Some(format!(
                "Current {NOUN_DONE_CONTRACT}: {}. You can view, check, update, keep, or cancel before launch.",
                done_criteria_prompt_detail(selection)
            )),
            choices: vec![
                start_prompt_choice(
                    "keep",
                    format!("Keep current {NOUN_DONE_CONTRACT}"),
                    done_criteria_prompt_detail(selection),
                ),
                start_prompt_choice(
                    "view",
                    "View current contract summary",
                    "prints source, path/check count, and manual commands",
                ),
                start_prompt_choice(
                    "check",
                    "Check current contract now",
                    "dry-runs the configured checks against this working tree",
                ),
                start_prompt_choice(
                    "update",
                    "Update contract before launch",
                    "writes new plain-English criteria through the def-done flow",
                ),
                prompt::SelectChoice::new(
                    "cancel",
                    format!("Cancel and show {NOUN_DONE_CONTRACT} commands"),
                ),
            ],
            default_index: 0,
        })?;

        match choice.id.as_str() {
            "keep" => {
                decision.done_criteria_source = StartDoneCriteriaSource::Project;
                decision.done_action = StartDoneAction::Existing;
                decision.done_criteria_label = selection.full_label();
                return Ok(());
            }
            "view" => print_start_done_criteria_summary(selection),
            "check" => check_start_done_criteria(cwd, selection)?,
            "update" => {
                let text = prompter.input("updated definition of done: ", None)?;
                if text.trim().is_empty() {
                    set_start_recovery(
                        decision,
                        format!("empty {NOUN_DONE_CONTRACT} was not saved"),
                        done_criteria_inspection_try_lines(selection),
                    );
                    return Ok(());
                }
                decision.done_criteria_source = StartDoneCriteriaSource::Manual;
                decision.done_action = StartDoneAction::ManualText {
                    text: text.trim().to_string(),
                    overwrite_existing: true,
                };
                decision.done_criteria_label = format!("update {NOUN_DONE_CONTRACT} before launch");
                return Ok(());
            }
            _ => {
                set_start_recovery(
                    decision,
                    format!("guided start cancelled before accepting the {NOUN_DONE_CONTRACT}"),
                    done_criteria_inspection_try_lines(selection),
                );
                return Ok(());
            }
        }
    }
}

fn prompt_start_non_git_mode(prompter: &mut dyn StartPrompter) -> Result<StartNonGitChoice> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose source mode".to_string(),
        help: Some("This directory is not a git repo.".to_string()),
        choices: vec![
            start_prompt_choice(
                "init",
                "Initialize git, then use worktree mode",
                "runs git init after final confirmation",
            ),
            start_prompt_choice(
                "copy",
                "Copy current directory into a run workspace",
                "leaves this directory untouched",
            ),
            start_prompt_choice(
                "fresh",
                "Fresh empty workspace",
                "starts with no source files",
            ),
            prompt::SelectChoice::new("cancel", "Cancel"),
        ],
        default_index: 0,
    })?;
    Ok(match choice.id.as_str() {
        "init" => StartNonGitChoice::Init,
        "copy" => StartNonGitChoice::Copy,
        "fresh" => StartNonGitChoice::Fresh,
        _ => StartNonGitChoice::Cancel,
    })
}

fn prompt_start_dirty_worktree(prompter: &mut dyn StartPrompter) -> Result<StartDirtyGitChoice> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose dirty worktree handling".to_string(),
        help: Some("The source repo has uncommitted changes.".to_string()),
        choices: vec![
            start_prompt_choice(
                "stop",
                "Stop and stash or commit first",
                "shows recovery commands",
            ),
            start_prompt_choice(
                "allow-dirty",
                "Seed dirty files into the worktree",
                "equivalent to --allow-dirty",
            ),
            prompt::SelectChoice::new("cancel", "Cancel"),
        ],
        default_index: 0,
    })?;
    Ok(match choice.id.as_str() {
        "allow-dirty" => StartDirtyGitChoice::AllowDirty,
        "cancel" => StartDirtyGitChoice::Cancel,
        _ => StartDirtyGitChoice::Stop,
    })
}

struct LaunchPreviewFacts<'a> {
    goal: &'a str,
    path: &'a str,
    suggestion: Option<String>,
    provider: &'a str,
    roles: Option<String>,
    base: Option<String>,
    history: Option<String>,
    done: &'a str,
    workspace: &'a str,
    watch: String,
    stop: String,
    finish: String,
    override_command: Option<String>,
}

fn launch_preview_rows(facts: &LaunchPreviewFacts<'_>) -> Vec<(String, String)> {
    let mut rows = vec![
        ("goal".to_string(), facts.goal.to_string()),
        ("path".to_string(), facts.path.to_string()),
    ];
    if let Some(suggestion) = facts.suggestion.as_ref() {
        rows.push(("suggestion".to_string(), suggestion.clone()));
    }
    rows.push(("provider".to_string(), facts.provider.to_string()));
    if let Some(roles) = facts.roles.as_ref() {
        rows.push(("roles".to_string(), roles.clone()));
    }
    if let Some(base) = facts.base.as_ref() {
        rows.push(("base".to_string(), base.clone()));
    }
    if let Some(history) = facts.history.as_ref() {
        rows.push(("history".to_string(), history.clone()));
    }
    rows.extend([
        ("done".to_string(), facts.done.to_string()),
        ("workspace".to_string(), facts.workspace.to_string()),
        ("watch".to_string(), facts.watch.clone()),
        ("stop".to_string(), facts.stop.clone()),
        ("finish".to_string(), facts.finish.clone()),
    ]);
    if let Some(command) = facts.override_command.as_ref() {
        rows.push(("override".to_string(), command.clone()));
    }
    rows
}

fn print_launch_preview_rows(rows: &[(String, String)]) {
    let refs = rows
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    print_kv_block(&refs);
}

fn start_launch_preview_facts(decision: &StartLaunchDecision) -> LaunchPreviewFacts<'_> {
    let override_command = match decision.selected_mode {
        StartSelectedMode::Run => Some("deadreckon start <goal> --mode review".to_string()),
        StartSelectedMode::Extend => Some("deadreckon start <goal> --mode run".to_string()),
        StartSelectedMode::Review | StartSelectedMode::FullPlan | StartSelectedMode::Campaign => {
            Some("deadreckon start <goal> --mode run".to_string())
        }
    };
    let suggestion = decision.goal_shape.as_ref().map(|recommendation| {
        let count = recommendation
            .n
            .map(|n| format!(" n={n}"))
            .unwrap_or_default();
        format!(
            "{}{} via {}: {}",
            recommendation.shape.label(),
            count,
            recommendation.source.label(),
            recommendation.rationale
        )
    });
    LaunchPreviewFacts {
        goal: &decision.goal,
        path: decision.selected_mode.path_label(),
        suggestion,
        provider: &decision.provider_label,
        roles: start_provider_role_summary(decision),
        base: decision.base_run_label.clone(),
        history: decision.history_action_label.clone(),
        done: &decision.done_criteria_label,
        workspace: &decision.source_mode_label,
        watch: "deadreckon attach <after-start>".to_string(),
        stop: "deadreckon kill <after-start>".to_string(),
        finish: "deadreckon finish <after-start>".to_string(),
        override_command,
    }
}

fn start_provider_role_summary(decision: &StartLaunchDecision) -> Option<String> {
    match decision.selected_mode {
        StartSelectedMode::Extend | StartSelectedMode::Run => None,
        StartSelectedMode::Review => {
            let route = decision.provider_route.as_deref()?;
            let coder = decision.coder_provider_route.as_deref().unwrap_or(route);
            let reviewer = decision.reviewer_provider_route.as_deref().unwrap_or(route);
            Some(format!("coder={coder}, reviewer={reviewer}"))
        }
        StartSelectedMode::FullPlan => {
            let route = decision.provider_route.as_deref()?;
            let planner = decision.planner_provider_route.as_deref().unwrap_or(route);
            let child = decision.child_provider_route.as_deref().unwrap_or(route);
            let n = decision.child_count.unwrap_or_else(|| {
                commands::orchestrate::recommend_child_count_for_goal(
                    &decision.goal,
                    CliPlanMode::FullPlan,
                )
            });
            let mut summary = format!("children={n}, planner={planner}, child={child}");
            if !decision.child_provider_overrides.is_empty() {
                summary.push_str(", overrides=");
                summary.push_str(&decision.child_provider_overrides.join(","));
            }
            Some(summary)
        }
        StartSelectedMode::Campaign => {
            let route = decision.provider_route.as_deref()?;
            let planner = decision.planner_provider_route.as_deref().unwrap_or(route);
            let child = decision.child_provider_route.as_deref().unwrap_or(route);
            let n = decision.child_count.unwrap_or_else(|| {
                commands::orchestrate::recommend_child_count_for_goal(
                    &decision.goal,
                    CliPlanMode::FullPlan,
                )
            });
            Some(format!("subs={n}, planner={planner}, child={child}"))
        }
    }
}

fn resolve_start_setup(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    prompter: Option<&mut dyn StartPrompter>,
    stdin_is_tty: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    if let Some(prompter) = prompter {
        resolve_start_provider(decision, args, &paths, &defaults, Some(&mut *prompter))?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_orchestration_options(
            decision,
            args,
            &paths,
            &defaults,
            Some(&mut *prompter),
        )?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        let cwd = std::env::current_dir()?;
        resolve_start_done_criteria(decision, &cwd, Some(&mut *prompter))?;
        if decision.recovery.is_none()
            && !matches!(
                decision.selected_mode,
                StartSelectedMode::Extend | StartSelectedMode::Campaign
            )
        {
            resolve_start_source_mode(
                decision,
                &paths,
                &cwd,
                Some(&mut *prompter),
                StartSourceModeRequest {
                    fresh: args.fresh,
                    worktree: args.worktree,
                    from: args.from.as_deref(),
                    allow_dirty: args.allow_dirty,
                    stdin_is_tty,
                },
            )?;
        }
    } else {
        resolve_start_provider(decision, args, &paths, &defaults, None)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_orchestration_options(decision, args, &paths, &defaults, None)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        let cwd = std::env::current_dir()?;
        resolve_start_done_criteria(decision, &cwd, None)?;
        if decision.recovery.is_none()
            && !matches!(
                decision.selected_mode,
                StartSelectedMode::Extend | StartSelectedMode::Campaign
            )
        {
            resolve_start_source_mode(
                decision,
                &paths,
                &cwd,
                None,
                StartSourceModeRequest {
                    fresh: args.fresh,
                    worktree: args.worktree,
                    from: args.from.as_deref(),
                    allow_dirty: args.allow_dirty,
                    stdin_is_tty,
                },
            )?;
        }
    }
    Ok(())
}

fn resolve_start_provider(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    mut prompter: Option<&mut dyn StartPrompter>,
) -> Result<()> {
    if let Some(provider) = args.provider.as_deref() {
        let provider = resolve_explicit_start_provider(
            paths,
            defaults,
            setup::SetupProviderRoleRef::PrimaryRun,
            provider,
        )?;
        decision.provider_source = StartProviderSource::ExplicitFlag;
        decision.provider_route = Some(provider.clone());
        decision.provider_label = format!("{provider} ({})", decision.provider_source.label());
        return Ok(());
    }

    let selection = provider_setup_selection(
        paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::PrimaryRun,
            explicit_provider: None,
            explicit_model: None,
            config_default_provider: defaults.provider.as_deref(),
            config_doc_provider: defaults.doc_provider.as_deref(),
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: None,
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: true,
        },
    )?;

    let Some(provider) = selection.provider.as_ref() else {
        if let Some(prompter) = prompter.as_mut() {
            prompt_start_provider(decision, paths, defaults, &mut **prompter)?;
            return Ok(());
        }
        decision.provider_source = StartProviderSource::Missing;
        decision.provider_label = "missing provider".to_string();
        set_start_recovery(
            decision,
            "provider setup is incomplete",
            vec![
                "deadreckon try".to_string(),
                "deadreckon config provider cli:codex".to_string(),
            ],
        );
        return Ok(());
    };

    decision.provider_source = match selection.source {
        setup::SetupProviderSource::AutoSubscription => StartProviderSource::Detected,
        setup::SetupProviderSource::Config
        | setup::SetupProviderSource::Flag
        | setup::SetupProviderSource::RunProvider
        | setup::SetupProviderSource::BuiltInDefault
        | setup::SetupProviderSource::None => StartProviderSource::Configured,
    };
    decision.provider_route = Some(provider.clone());
    if matches!(decision.provider_source, StartProviderSource::Detected) {
        decision.provider_label = detected_start_provider_label(provider);
        return Ok(());
    }
    decision.provider_label = format!("{provider} ({})", decision.provider_source.label());
    if let Some(prompter) = prompter.as_mut() {
        prompt_start_provider(decision, paths, defaults, &mut **prompter)?;
    }
    Ok(())
}

fn detected_start_provider_label(provider: &str) -> String {
    format!("{provider} (detected) - run deadreckon config provider {provider} to make permanent")
}

fn resolve_start_done_criteria(
    decision: &mut StartLaunchDecision,
    cwd: &Path,
    prompter: Option<&mut dyn StartPrompter>,
) -> Result<()> {
    let source = commands::acceptance::resolve_acceptance_source(cwd, None)?;
    if source.is_some() {
        let selection = commands::acceptance::done_criteria_selection(&source)?;
        if let Some(prompter) = prompter {
            prompt_start_existing_done_criteria(decision, cwd, &selection, prompter)?;
            return Ok(());
        }
        decision.done_criteria_source = StartDoneCriteriaSource::Project;
        decision.done_action = StartDoneAction::Existing;
        decision.done_criteria_label = selection.full_label();
        return Ok(());
    }

    if let Some(prompter) = prompter {
        prompt_start_done_criteria(decision, prompter)?;
        return Ok(());
    }

    decision.done_criteria_source = StartDoneCriteriaSource::Missing;
    decision.done_criteria_label = format!("missing {NOUN_DONE_CONTRACT}");
    set_start_recovery(
        decision,
        format!("{NOUN_DONE_CONTRACT} is missing for this repo"),
        vec![format!(
            "deadreckon def-done \"what should count as done\" && deadreckon start \"{}\"",
            shell_display_quote(&decision.goal)
        )],
    );
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct StartSourceModeRequest<'a> {
    fresh: bool,
    worktree: bool,
    from: Option<&'a Path>,
    allow_dirty: bool,
    stdin_is_tty: bool,
}

fn resolve_start_source_mode(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    cwd: &Path,
    mut prompter: Option<&mut dyn StartPrompter>,
    request: StartSourceModeRequest<'_>,
) -> Result<()> {
    let mut flags = ModeFlags {
        fresh: request.fresh,
        worktree: request.worktree,
        from: request.from.map(PathBuf::from),
        in_place: false,
        i_know_its_a_lot: false,
    };
    let explicit_mode = flags.fresh || flags.worktree || flags.from.is_some();
    if !explicit_mode && deadreckon_core::find_git_root(cwd)?.is_none() {
        if let Some(prompter) = prompter.as_mut() {
            match prompt_start_non_git_mode(&mut **prompter)? {
                StartNonGitChoice::Init => {
                    decision.source_mode = StartSourceMode::InitGit;
                    decision.source_mode_label = "git init, then worktree".to_string();
                    decision.source_init_git = true;
                    decision.source_worktree = true;
                    return Ok(());
                }
                StartNonGitChoice::Copy => {
                    if !matches!(decision.selected_mode, StartSelectedMode::Run) {
                        set_start_recovery(
                            decision,
                            "copy source mode is only supported by start --mode run",
                            vec![format!(
                                "deadreckon start \"{}\" --mode run --from .",
                                shell_display_quote(&decision.goal)
                            )],
                        );
                        return Ok(());
                    }
                    flags.from = Some(cwd.to_path_buf());
                }
                StartNonGitChoice::Fresh => {
                    if !matches!(decision.selected_mode, StartSelectedMode::Run) {
                        set_start_recovery(
                            decision,
                            "fresh source mode is only supported by start --mode run",
                            vec![format!(
                                "deadreckon start \"{}\" --mode run --fresh",
                                shell_display_quote(&decision.goal)
                            )],
                        );
                        return Ok(());
                    }
                    flags.fresh = true;
                }
                StartNonGitChoice::Cancel => {
                    set_start_recovery(
                        decision,
                        "guided start cancelled before choosing a source mode",
                        vec![format!(
                            "deadreckon start \"{}\"",
                            shell_display_quote(&decision.goal)
                        )],
                    );
                    return Ok(());
                }
            }
        } else {
            decision.source_mode = StartSourceMode::Missing;
            decision.source_mode_label = "missing source mode".to_string();
            set_start_recovery(
                decision,
                "non-interactive without a source mode",
                vec![
                    format!(
                        "deadreckon start \"{}\" --from .",
                        shell_display_quote(&decision.goal)
                    ),
                    format!(
                        "deadreckon start \"{}\" --fresh",
                        shell_display_quote(&decision.goal)
                    ),
                    "git init".to_string(),
                ],
            );
            return Ok(());
        }
    }

    let resolved_mode = resolve_mode(&flags, cwd, request.stdin_is_tty)?;
    match resolved_mode {
        ResolvedMode::Worktree { source_path, .. } => {
            let first = prepare_worktree_record(
                paths,
                WorktreeOptions {
                    run_id: Uuid::new_v4().simple().to_string(),
                    task_key: deadreckon_core::paths::task_key(&decision.goal),
                    source_path: source_path.clone(),
                    base_ref: None,
                    branch_name: None,
                    allow_dirty: request.allow_dirty,
                },
            );
            match first {
                Ok(_) => {
                    decision.source_mode = StartSourceMode::Worktree;
                    decision.source_mode_label = format!("worktree from {}", source_path.display());
                    decision.source_worktree = flags.worktree;
                    decision.source_allow_dirty = request.allow_dirty;
                }
                Err(DeadreckonError::InvalidInput(message))
                    if message.contains("working tree has uncommitted changes") =>
                {
                    if let Some(prompter) = prompter.as_mut() {
                        match prompt_start_dirty_worktree(&mut **prompter)? {
                            StartDirtyGitChoice::AllowDirty => {
                                if !matches!(decision.selected_mode, StartSelectedMode::Run) {
                                    set_start_recovery(
                                        decision,
                                        "allow-dirty source mode is only supported by start --mode run",
                                        vec![format!(
                                            "deadreckon start \"{}\" --mode run --allow-dirty",
                                            shell_display_quote(&decision.goal)
                                        )],
                                    );
                                    return Ok(());
                                }
                                prepare_worktree_record(
                                    paths,
                                    WorktreeOptions {
                                        run_id: Uuid::new_v4().simple().to_string(),
                                        task_key: deadreckon_core::paths::task_key(&decision.goal),
                                        source_path: source_path.clone(),
                                        base_ref: None,
                                        branch_name: None,
                                        allow_dirty: true,
                                    },
                                )?;
                                decision.source_mode = StartSourceMode::Worktree;
                                decision.source_mode_label = format!(
                                    "worktree from {} with dirty files",
                                    source_path.display()
                                );
                                decision.source_worktree = flags.worktree;
                                decision.source_allow_dirty = true;
                            }
                            StartDirtyGitChoice::Cancel => set_start_recovery(
                                decision,
                                "guided start cancelled before choosing dirty-worktree handling",
                                vec![format!(
                                    "deadreckon start \"{}\"",
                                    shell_display_quote(&decision.goal)
                                )],
                            ),
                            StartDirtyGitChoice::Stop => set_start_recovery(
                                decision,
                                message.lines().next().unwrap_or("working tree is dirty"),
                                vec![
                                    format!(
                                        "git stash && deadreckon start \"{}\"",
                                        shell_display_quote(&decision.goal)
                                    ),
                                    format!(
                                        "deadreckon start \"{}\" --allow-dirty",
                                        shell_display_quote(&decision.goal)
                                    ),
                                ],
                            ),
                        }
                    } else {
                        decision.source_mode = StartSourceMode::Worktree;
                        decision.source_mode_label = "dirty worktree".to_string();
                        set_start_recovery(
                            decision,
                            message.lines().next().unwrap_or("working tree is dirty"),
                            vec![
                                format!(
                                    "git stash && deadreckon start \"{}\"",
                                    shell_display_quote(&decision.goal)
                                ),
                                format!(
                                    "deadreckon start \"{}\" --allow-dirty",
                                    shell_display_quote(&decision.goal)
                                ),
                            ],
                        );
                    }
                }
                Err(err) => return Err(CliError::Core(err)),
            }
        }
        ResolvedMode::Copy { source_path } => {
            decision.source_mode = StartSourceMode::Copy;
            decision.source_mode_label = format!("copy from {}", source_path.display());
            decision.source_from = Some(source_path);
        }
        ResolvedMode::Fresh => {
            decision.source_mode = StartSourceMode::Fresh;
            decision.source_mode_label = "fresh".to_string();
            decision.source_fresh = true;
        }
        ResolvedMode::InPlace { source_path } => {
            decision.source_mode = StartSourceMode::Copy;
            decision.source_mode_label = format!("in-place from {}", source_path.display());
            decision.source_from = Some(source_path);
        }
    }
    Ok(())
}

enum StartNonGitChoice {
    Init,
    Copy,
    Fresh,
    Cancel,
}

enum StartDirtyGitChoice {
    Stop,
    AllowDirty,
    Cancel,
}

fn set_start_recovery(
    decision: &mut StartLaunchDecision,
    message: impl Into<String>,
    try_lines: Vec<String>,
) {
    decision.try_lines = try_lines.clone();
    decision.recovery = Some(StartRecovery {
        message: message.into(),
        try_lines,
    });
}

fn start_recovery_error(recovery: &StartRecovery) -> CliError {
    CliError::Exit {
        code: 1,
        message: recovery.message.clone(),
        hint: recovery
            .try_lines
            .iter()
            .map(|line| format!("try: {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn shell_display_quote(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn start_done_materialization_request(decision: &StartLaunchDecision) -> Option<(String, bool)> {
    match decision.done_action.clone() {
        StartDoneAction::GenerateFromGoal => Some((
            format!(
                "For this start, define practical acceptance checks for: {}",
                decision.goal
            ),
            false,
        )),
        StartDoneAction::ManualText {
            text,
            overwrite_existing,
        } => Some((text, overwrite_existing)),
        StartDoneAction::Existing | StartDoneAction::DefaultGate | StartDoneAction::Missing => None,
    }
}

async fn materialize_start_done_criteria(decision: &mut StartLaunchDecision) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Some((request, overwrite_existing)) = start_done_materialization_request(decision) else {
        return Ok(());
    };
    commands::acceptance::acceptance_agent_command_in_dir(
        &cwd,
        commands::acceptance::AcceptanceAgentMode::Draft,
        vec![request],
        decision.provider_route.clone(),
        None,
        overwrite_existing,
    )
    .await?;
    if let Some(source) = commands::acceptance::mark_generated_done_criteria(
        commands::acceptance::resolve_acceptance_source(&cwd, None)?,
    ) {
        let selection = commands::acceptance::done_criteria_selection(&Some(source))?;
        decision.done_criteria_source = StartDoneCriteriaSource::Generated;
        decision.done_action = StartDoneAction::Existing;
        decision.done_criteria_label = selection.full_label();
    }
    Ok(())
}

fn prompt_start_launch_confirmation(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    if args.preview || args.yes || args.quiet {
        return Ok(());
    }
    println!("{}", ui_heading("deadreckon start preview"));
    let rows = launch_preview_rows(&start_launch_preview_facts(decision));
    print_launch_preview_rows(&rows);
    decision.requires_confirmation = true;
    if prompter.confirm("start this launch?", true)? {
        decision.confirmed_by_start_picker = true;
        Ok(())
    } else {
        Err(start_recovery_error(&StartRecovery {
            message: "guided start cancelled before launch".to_string(),
            try_lines: vec![format!(
                "deadreckon start \"{}\" --preview",
                shell_display_quote(&decision.goal)
            )],
        }))
    }
}

async fn start_command(args: StartCommandArgs) -> Result<()> {
    let stdin_is_tty = io::stdin().is_terminal();
    let paths = DeadreckonPaths::discover();
    let cwd = std::env::current_dir()?;
    let latest_extendable_run = start_latest_extendable_run(&paths, &cwd)?;
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: &args.goal,
        requested_mode: args.mode,
        stdin_is_tty,
    });
    add_start_history_actions(&mut decision, latest_extendable_run.as_ref());
    let eligibility = StartPromptEligibility::from_args(&args, stdin_is_tty);
    if start_goal_shape_should_classify(&args, eligibility) {
        let defaults = config_defaults(&paths)?;
        let provider = start_goal_shape_provider_route(&paths, &defaults, &args);
        let recommendation = classify_goal_shape_for_start(
            &paths,
            &cwd,
            &args.goal,
            provider.as_deref(),
            args.plain,
        )
        .await;
        let scope = workspace_scope(&cwd)?;
        write_goal_shape_preview_record(&paths, &scope, &recommendation)?;
        apply_goal_shape_recommendation(&mut decision, recommendation);
    }
    let mut terminal_prompter = TerminalStartPrompter;
    if eligibility.allows_prompts() {
        maybe_prompt_start_mode(
            &mut decision,
            &args,
            latest_extendable_run.as_ref(),
            &mut terminal_prompter,
        )?;
        if decision.recovery.is_none() {
            resolve_start_setup(
                &mut decision,
                &args,
                Some(&mut terminal_prompter),
                stdin_is_tty,
            )?;
        }
    } else if decision.recovery.is_none() {
        resolve_start_setup(&mut decision, &args, None, stdin_is_tty)?;
    }
    if args.json {
        let mut next_actions = if decision.recovery.is_some() {
            decision.try_lines.clone()
        } else if matches!(decision.selected_mode, StartSelectedMode::Extend) {
            decision
                .base_run_id
                .as_ref()
                .map(|run_id| {
                    vec![format!(
                        "deadreckon extend {} \"{}\"",
                        run_prefix(run_id),
                        shell_display_quote(&decision.goal)
                    )]
                })
                .unwrap_or_else(|| vec!["deadreckon list".to_string()])
        } else {
            match decision.selected_mode {
                StartSelectedMode::Campaign => vec![format!(
                    "deadreckon campaign \"{}\" --n {} --yes",
                    shell_display_quote(&decision.goal),
                    decision.child_count.unwrap_or(3)
                )],
                _ => vec![format!(
                    "deadreckon start \"{}\" --mode {} --yes",
                    shell_display_quote(&decision.goal),
                    decision.selected_mode.label()
                )],
            }
        };
        if decision.recovery.is_none()
            && !matches!(decision.selected_mode, StartSelectedMode::Extend)
        {
            for action in &decision.history_next_actions {
                if !next_actions.iter().any(|existing| existing == action) {
                    next_actions.push(action.clone());
                }
            }
        }
        let payload = json!({
            "kind": "start",
            "goal": decision.goal,
            "selected_mode": decision.selected_mode.label(),
            "selection_source": decision.selection_source.label(),
            "reason": decision.reason,
            "provider": decision.provider_label,
            "provider_source": decision.provider_source.label(),
            "done_criteria": decision.done_criteria_label,
            "done_criteria_source": decision.done_criteria_source.label(),
            "source_mode": decision.source_mode.label(),
            "goal_shape": &decision.goal_shape,
            "requires_confirmation": decision.requires_confirmation,
            "will_start": false,
            "history_actions": decision.history_next_actions,
            "next_actions": next_actions,
            "try_lines": decision.try_lines
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    if args.preview {
        if !args.quiet {
            println!("{}", ui_heading("deadreckon start preview"));
            let rows = launch_preview_rows(&start_launch_preview_facts(&decision));
            print_launch_preview_rows(&rows);
        }
        return Ok(());
    }
    if decision.recovery.is_none() && eligibility.allows_prompts() {
        prompt_start_launch_confirmation(&mut decision, &args, &mut terminal_prompter)?;
    }
    if !args.quiet {
        println!("{}", ui_heading("guided start"));
        let mode = decision.selected_mode.label();
        let suggestion = decision.goal_shape.as_ref().map(|recommendation| {
            format!(
                "{} via {}: {}",
                recommendation.shape.label(),
                recommendation.source.label(),
                recommendation.rationale
            )
        });
        let mut rows: Vec<(&str, &str)> = vec![
            ("goal", decision.goal.as_str()),
            ("mode", mode),
            ("selection", decision.selection_source.label()),
            ("reason", decision.reason.as_str()),
        ];
        if let Some(suggestion) = suggestion.as_ref() {
            rows.push(("suggestion", suggestion.as_str()));
        }
        rows.extend([
            ("provider", decision.provider_label.as_str()),
            ("done", decision.done_criteria_label.as_str()),
            ("workspace", decision.source_mode_label.as_str()),
            (
                "confirmation",
                if decision.requires_confirmation {
                    "required"
                } else {
                    "not required"
                },
            ),
            ("preview", if args.preview { "yes" } else { "no" }),
            (
                "confirmed",
                if args.yes || decision.confirmed_by_start_picker {
                    "yes"
                } else {
                    "no"
                },
            ),
            ("plain", if args.plain { "yes" } else { "no" }),
        ]);
        print_kv_block(&rows);
    }
    if let Some(recovery) = decision.recovery.as_ref() {
        return Err(start_recovery_error(recovery));
    }
    materialize_start_done_criteria(&mut decision).await?;
    dispatch_start_command(args, &decision).await
}

async fn dispatch_start_command(
    args: StartCommandArgs,
    decision: &StartLaunchDecision,
) -> Result<()> {
    match decision.selected_mode {
        StartSelectedMode::Run => {
            let paths = DeadreckonPaths::discover();
            let before = start_run_ids(&paths)?;
            let goal = args.goal.clone();
            let quiet = args.quiet;
            let auto_confirm = args.yes || args.quiet || decision.confirmed_by_start_picker;
            let result = commands::run::run_command(RunCommandArgs {
                goal: args.goal,
                fresh: args.fresh || decision.source_fresh,
                worktree: args.worktree || decision.source_worktree,
                from: args.from.or_else(|| decision.source_from.clone()),
                in_place: false,
                base: None,
                branch: None,
                allow_dirty: args.allow_dirty || decision.source_allow_dirty,
                init_git: decision.source_init_git,
                yes: auto_confirm,
                preview: false,
                brief: false,
                plain: args.plain,
                prevent_sleep: None,
                quiet: args.quiet,
                max_spend: None,
                max_wall_seconds: None,
                sandbox: None,
                provider: decision.provider_route.clone(),
                model: None,
                doc_provider: None,
                acceptance: None,
                skill: "deadreckon".to_string(),
                smoke: false,
                i_know_its_a_lot: false,
                no_confirm: auto_confirm
                    || matches!(decision.done_action, StartDoneAction::DefaultGate),
                no_hints: args.quiet,
                no_docs: false,
                doc_skill: None,
            })
            .await;
            if result.is_ok()
                && !quiet
                && let Some(run) = newest_start_run(&paths, &before, &goal)?
            {
                print_start_lifecycle_footer("run", &run.run_id);
            }
            result
        }
        StartSelectedMode::Extend => {
            if start_source_flags_present(&args) {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "source mode flags are not used when start extends prior history",
                    "omit source flags or use deadreckon extend directly",
                )));
            }
            let parent_run_id = decision.base_run_id.clone().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "guided start did not select a parent run to extend",
                    "deadreckon list",
                ))
            })?;
            let paths = DeadreckonPaths::discover();
            let before = start_run_ids(&paths)?;
            let goal = args.goal.clone();
            let quiet = args.quiet;
            let result = extend_command(ExtendCommandArgs {
                parent_run_id,
                new_goal: args.goal,
                dest: None,
                max_context_turns: None,
                no_context: false,
                max_spend: None,
                max_wall_seconds: None,
                provider: decision.provider_route.clone(),
                model: None,
                sandbox: None,
                no_docs: false,
                doc_skill: None,
                post_actions: !args.quiet,
            })
            .await;
            if result.is_ok()
                && !quiet
                && let Some(run) = newest_start_run(&paths, &before, &goal)?
            {
                print_start_lifecycle_footer("run", &run.run_id);
            }
            result
        }
        StartSelectedMode::Campaign => {
            if start_source_flags_present(&args)
                || decision.source_fresh
                || decision.source_from.is_some()
                || decision.source_allow_dirty
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "source mode flags are only supported by start --mode run",
                    "omit source flags or use deadreckon campaign directly",
                )));
            }
            let paths = DeadreckonPaths::discover();
            let before = start_plan_ids(&paths)?;
            let goal = args.goal.clone();
            let quiet = args.quiet;
            let provider_route = decision.provider_route.clone();
            let planner_provider = decision
                .planner_provider_route
                .clone()
                .or_else(|| provider_route.clone());
            let child_provider = decision
                .child_provider_route
                .clone()
                .or_else(|| provider_route.clone());
            let result = commands::campaign::campaign_command(commands::campaign::CampaignArgs {
                goal: args.goal,
                n: decision.child_count,
                planner_provider,
                provider: child_provider,
                max_spend: None,
                max_wall_seconds: None,
                sandbox: None,
                preview: false,
                yes: args.yes || args.quiet || decision.confirmed_by_start_picker,
                no_hints: args.quiet,
                quiet: args.quiet,
                plain: args.plain,
            })
            .await;
            if result.is_ok()
                && !quiet
                && let Some(plan) = newest_start_plan(&paths, &before, &goal)?
            {
                print_start_lifecycle_footer("campaign", &plan.plan_id);
            }
            result
        }
        StartSelectedMode::Review | StartSelectedMode::FullPlan => {
            if start_source_flags_present(&args)
                || decision.source_fresh
                || decision.source_from.is_some()
                || decision.source_allow_dirty
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "source mode flags are only supported by start --mode run",
                    "omit source flags or use deadreckon run directly",
                )));
            }
            let paths = DeadreckonPaths::discover();
            let before = start_plan_ids(&paths)?;
            let goal = args.goal.clone();
            let quiet = args.quiet;
            let mode = match decision.selected_mode {
                StartSelectedMode::Extend
                | StartSelectedMode::Run
                | StartSelectedMode::Campaign => {
                    unreachable!("run, extend, and campaign handled above")
                }
                StartSelectedMode::Review => CliPlanMode::Review,
                StartSelectedMode::FullPlan => CliPlanMode::FullPlan,
            };
            let auto_confirm = args.yes || args.quiet || decision.confirmed_by_start_picker;
            let provider_route = decision.provider_route.clone();
            let planner_provider = decision
                .planner_provider_route
                .clone()
                .or_else(|| provider_route.clone());
            let child_provider = decision
                .child_provider_route
                .clone()
                .or_else(|| provider_route.clone());
            let coder_provider = decision
                .coder_provider_route
                .clone()
                .or_else(|| provider_route.clone());
            let reviewer_provider = decision.reviewer_provider_route.clone().or(provider_route);
            let result = commands::orchestrate::orchestrate_command(
                commands::orchestrate::OrchestrateRunArgs {
                    plan: PlanCommandArgs {
                        goal: args.goal,
                        n: decision.child_count.unwrap_or_else(|| {
                            commands::orchestrate::recommend_child_count_for_goal(
                                &decision.goal,
                                mode,
                            )
                        }),
                        mode,
                        max_spend: None,
                        max_wall_seconds: None,
                        sandbox: None,
                        planner_provider: if mode == CliPlanMode::FullPlan {
                            planner_provider
                        } else {
                            None
                        },
                        provider: if mode == CliPlanMode::FullPlan {
                            child_provider
                        } else {
                            None
                        },
                        child_provider: decision.child_provider_overrides.clone(),
                        coder_provider: if mode == CliPlanMode::Review {
                            coder_provider
                        } else {
                            None
                        },
                        reviewer_provider: if mode == CliPlanMode::Review {
                            reviewer_provider
                        } else {
                            None
                        },
                        init_git: decision.source_init_git,
                        acceptance: None,
                        skip_acceptance_prompt: auto_confirm
                            || matches!(decision.done_action, StartDoneAction::DefaultGate),
                        no_hints: args.quiet,
                        quiet: args.quiet,
                        json: false,
                        plain: args.plain,
                    },
                    preview: false,
                    yes: auto_confirm,
                    no_repair: false,
                },
            )
            .await;
            if result.is_ok()
                && !quiet
                && let Some(plan) = newest_start_plan(&paths, &before, &goal)?
            {
                print_start_lifecycle_footer("plan", &plan.plan_id);
            }
            result
        }
    }
}

fn start_source_flags_present(args: &StartCommandArgs) -> bool {
    args.fresh || args.worktree || args.from.is_some() || args.allow_dirty
}

fn start_orchestration_flags_present(args: &StartCommandArgs) -> bool {
    args.children.is_some()
        || args.planner_provider.is_some()
        || !args.child_provider.is_empty()
        || args.coder_provider.is_some()
        || args.reviewer_provider.is_some()
}

fn start_run_ids(paths: &DeadreckonPaths) -> Result<BTreeSet<String>> {
    Ok(list_runs(paths, None)?
        .into_iter()
        .map(|run| run.run_id)
        .collect())
}

fn start_plan_ids(paths: &DeadreckonPaths) -> Result<BTreeSet<String>> {
    Ok(list_plan_entries(paths, None)?
        .into_iter()
        .map(|plan| plan.plan_id)
        .collect())
}

fn newest_start_run(
    paths: &DeadreckonPaths,
    before: &BTreeSet<String>,
    goal: &str,
) -> Result<Option<RunListEntry>> {
    let mut runs = list_runs(paths, None)?
        .into_iter()
        .filter(|run| run.goal == goal && !before.contains(&run.run_id))
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.updated_at);
    Ok(runs.pop())
}

fn newest_start_plan(
    paths: &DeadreckonPaths,
    before: &BTreeSet<String>,
    goal: &str,
) -> Result<Option<PlanListEntry>> {
    let mut plans = list_plan_entries(paths, None)?
        .into_iter()
        .filter(|plan| plan.goal == goal && !before.contains(&plan.plan_id))
        .collect::<Vec<_>>();
    plans.sort_by_key(|plan| plan.updated_at);
    Ok(plans.pop())
}

fn print_start_lifecycle_footer(kind: &str, id: &str) {
    let id = run_prefix(id);
    let attach = format!("deadreckon attach {id}");
    let status = format!("deadreckon status {id}");
    let kill = format!("deadreckon kill {id}");
    let finish = format!("deadreckon finish {id}");
    println!("{}", ui_heading("start lifecycle"));
    print_kv_block(&[
        ("target", kind),
        ("attach", attach.as_str()),
        ("status", status.as_str()),
        ("kill", kill.as_str()),
        ("finish", finish.as_str()),
    ]);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttachTickBudget {
    target_frame_ms: u64,
    max_sync_io_ms: u64,
    slow_warning_ms: u64,
}

impl Default for AttachTickBudget {
    fn default() -> Self {
        Self {
            target_frame_ms: 500,
            max_sync_io_ms: 80,
            slow_warning_ms: 180,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachSurface {
    Run,
    Plan,
    Chain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachLoopStage {
    LoadState,
    ReadJsonl,
    EventFeed,
    LiveCollect,
    PlanMessages,
    ProviderNarrativeRefresh,
    Draw,
    InputPoll,
}

impl AttachLoopStage {
    fn label(self) -> &'static str {
        match self {
            Self::LoadState => "load state",
            Self::ReadJsonl => "read jsonl",
            Self::EventFeed => "event feed",
            Self::LiveCollect => "live collect",
            Self::PlanMessages => "plan messages",
            Self::ProviderNarrativeRefresh => "narrative provider refresh",
            Self::Draw => "draw",
            Self::InputPoll => "input poll",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachWorkMode {
    UiSync,
    Background,
}

fn attach_loop_stage_work(surface: AttachSurface, stage: AttachLoopStage) -> AttachWorkMode {
    match (surface, stage) {
        (AttachSurface::Run | AttachSurface::Plan, AttachLoopStage::ProviderNarrativeRefresh) => {
            AttachWorkMode::Background
        }
        _ => AttachWorkMode::UiSync,
    }
}

#[derive(Debug, Clone)]
struct AttachStageTiming {
    stage: AttachLoopStage,
    elapsed: Duration,
    work: AttachWorkMode,
}

#[derive(Debug)]
struct AttachTickTiming {
    surface: AttachSurface,
    budget: AttachTickBudget,
    started_at: Instant,
    stages: Vec<AttachStageTiming>,
}

impl AttachTickTiming {
    fn new(surface: AttachSurface, budget: AttachTickBudget) -> Self {
        Self {
            surface,
            budget,
            started_at: Instant::now(),
            stages: Vec::new(),
        }
    }

    fn record(&mut self, stage: AttachLoopStage, elapsed: Duration) {
        self.stages.push(AttachStageTiming {
            stage,
            elapsed,
            work: attach_loop_stage_work(self.surface, stage),
        });
    }

    fn record_since(&mut self, stage: AttachLoopStage, started_at: Instant) {
        self.record(stage, started_at.elapsed());
    }

    fn total_elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    fn frame_exceeded(&self) -> bool {
        self.total_elapsed() > Duration::from_millis(self.budget.target_frame_ms)
    }

    fn slow_sync_stages(&self) -> Vec<&AttachStageTiming> {
        let max_sync = Duration::from_millis(self.budget.max_sync_io_ms);
        self.stages
            .iter()
            .filter(|stage| stage.work == AttachWorkMode::UiSync && stage.elapsed > max_sync)
            .collect()
    }

    fn slow_warning_stages(&self) -> Vec<&AttachStageTiming> {
        let slow = Duration::from_millis(self.budget.slow_warning_ms);
        self.stages
            .iter()
            .filter(|stage| stage.elapsed > slow)
            .collect()
    }

    fn slow_stage_labels(&self) -> Vec<&'static str> {
        self.slow_warning_stages()
            .into_iter()
            .map(|stage| stage.stage.label())
            .collect()
    }
}

#[derive(Debug, Clone)]
struct AttachNarrativeRefreshState {
    kind: NarrativeRefreshKind,
    started_at: DateTime<Utc>,
    token: CancellationToken,
    coalesced_requests: usize,
    completed: bool,
}

impl AttachNarrativeRefreshState {
    fn new(
        kind: NarrativeRefreshKind,
        started_at: DateTime<Utc>,
        token: CancellationToken,
    ) -> Self {
        Self {
            kind,
            started_at,
            token,
            coalesced_requests: 0,
            completed: false,
        }
    }

    fn start_notice(&self) -> String {
        format!(
            "{}: refresh running in background; q detaches immediately",
            self.kind.label()
        )
    }

    fn coalesce(&mut self, kind: NarrativeRefreshKind, now: DateTime<Utc>) -> String {
        self.coalesced_requests = self.coalesced_requests.saturating_add(1);
        format!(
            "{}: refresh already running in background ({}s); coalesced {}",
            self.kind.label(),
            (now - self.started_at).num_seconds().max(0),
            kind.label()
        )
    }

    fn completion_notice_once(&mut self, notice: String) -> Option<String> {
        if self.completed {
            return None;
        }
        self.completed = true;
        Some(notice)
    }

    fn cancel(&self) {
        self.token.cancel();
    }
}

struct AttachRunNarrativeRefreshJob {
    state: AttachNarrativeRefreshState,
    handle: tokio::task::JoinHandle<String>,
}

struct AttachRunNarrativeRefreshRequest {
    paths: DeadreckonPaths,
    state: deadreckon_core::PipelineState,
    spend: Vec<SpendRecord>,
    traces: Vec<TraceRecord>,
    events: Vec<RunEvent>,
    live: AttachLive,
    tui_state: AttachTuiState,
    config: NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
}

struct AttachPlanNarrativeRefreshJob {
    plan_id: String,
    state: AttachNarrativeRefreshState,
    handle: tokio::task::JoinHandle<String>,
}

struct AttachPlanNarrativeRefreshRequest {
    paths: DeadreckonPaths,
    plan: Plan,
    messages: Vec<PlanMessage>,
    plan_events: Vec<PlanEvent>,
    feed_events: Vec<PlanFeedEvent>,
    selected: usize,
    config: NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
}

struct PlanNarrativeRefreshInput<'a> {
    paths: &'a DeadreckonPaths,
    plan: &'a Plan,
    messages: &'a [PlanMessage],
    plan_events: &'a [PlanEvent],
    feed_events: &'a [PlanFeedEvent],
    selected: usize,
    config: &'a NarrativeAttachConfig,
}

fn start_or_coalesce_run_narrative_refresh_job(
    job: &mut Option<AttachRunNarrativeRefreshJob>,
    request: AttachRunNarrativeRefreshRequest,
    now: DateTime<Utc>,
) -> String {
    if let Some(active) = job.as_mut() {
        return active.state.coalesce(request.kind, now);
    }

    let token = CancellationToken::new();
    let refresh_state = AttachNarrativeRefreshState::new(request.kind, now, token.clone());
    let notice = refresh_state.start_notice();
    let handle = tokio::spawn(async move {
        let AttachRunNarrativeRefreshRequest {
            paths,
            state,
            spend,
            traces,
            events,
            live,
            tui_state,
            config,
            kind,
        } = request;
        refresh_run_narrative_with_provider_for_kind_with_token(
            &paths,
            &RunNarrativeRenderInput {
                state: &state,
                spend: &spend,
                traces: &traces,
                events: &events,
                live: &live,
                tui_state: &tui_state,
            },
            &config,
            kind,
            Some(token),
        )
        .await
        .unwrap_or_else(|err| {
            format!(
                "provider refresh failed: {}",
                one_line(&err.to_string(), 120)
            )
        })
    });
    *job = Some(AttachRunNarrativeRefreshJob {
        state: refresh_state,
        handle,
    });
    notice
}

fn start_or_coalesce_plan_narrative_refresh_job(
    job: &mut Option<AttachPlanNarrativeRefreshJob>,
    request: AttachPlanNarrativeRefreshRequest,
    now: DateTime<Utc>,
) -> String {
    if let Some(active) = job.as_mut()
        && active.plan_id == request.plan.plan_id
    {
        return active.state.coalesce(request.kind, now);
    }
    cancel_plan_narrative_refresh_job(job);

    let token = CancellationToken::new();
    let plan_id = request.plan.plan_id.clone();
    let refresh_state = AttachNarrativeRefreshState::new(request.kind, now, token.clone());
    let notice = refresh_state.start_notice();
    let handle = tokio::spawn(async move {
        let AttachPlanNarrativeRefreshRequest {
            paths,
            plan,
            messages,
            plan_events,
            feed_events,
            selected,
            config,
            kind,
        } = request;
        refresh_plan_narrative_with_provider_for_kind(PlanNarrativeProviderRefresh {
            paths: &paths,
            plan: &plan,
            messages: &messages,
            plan_events: &plan_events,
            feed_events: &feed_events,
            selected,
            config: &config,
            kind,
            cancellation_token: Some(token),
        })
        .await
        .unwrap_or_else(|err| {
            format!(
                "provider refresh failed: {}",
                one_line(&err.to_string(), 120)
            )
        })
    });
    *job = Some(AttachPlanNarrativeRefreshJob {
        plan_id,
        state: refresh_state,
        handle,
    });
    notice
}

async fn poll_run_narrative_refresh_job(
    job: &mut Option<AttachRunNarrativeRefreshJob>,
) -> Option<String> {
    if !job
        .as_ref()
        .is_some_and(|active| active.handle.is_finished())
    {
        return None;
    }
    let AttachRunNarrativeRefreshJob { mut state, handle } = job.take()?;
    match handle.await {
        Ok(notice) => state.completion_notice_once(notice),
        Err(err) if err.is_cancelled() => None,
        Err(err) => Some(format!(
            "provider refresh failed: {}",
            one_line(&err.to_string(), 120)
        )),
    }
}

async fn poll_plan_narrative_refresh_job(
    job: &mut Option<AttachPlanNarrativeRefreshJob>,
) -> Option<String> {
    if !job
        .as_ref()
        .is_some_and(|active| active.handle.is_finished())
    {
        return None;
    }
    let AttachPlanNarrativeRefreshJob {
        mut state, handle, ..
    } = job.take()?;
    match handle.await {
        Ok(notice) => state.completion_notice_once(notice),
        Err(err) if err.is_cancelled() => None,
        Err(err) => Some(format!(
            "provider refresh failed: {}",
            one_line(&err.to_string(), 120)
        )),
    }
}

fn cancel_run_narrative_refresh_job(job: &mut Option<AttachRunNarrativeRefreshJob>) -> bool {
    let Some(active) = job.take() else {
        return false;
    };
    active.state.cancel();
    active.handle.abort();
    true
}

fn cancel_plan_narrative_refresh_job(job: &mut Option<AttachPlanNarrativeRefreshJob>) -> bool {
    let Some(active) = job.take() else {
        return false;
    };
    active.state.cancel();
    active.handle.abort();
    true
}

fn run_narrative_refresh_request(
    paths: &DeadreckonPaths,
    input: &RunNarrativeRenderInput<'_>,
    config: &NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
) -> AttachRunNarrativeRefreshRequest {
    AttachRunNarrativeRefreshRequest {
        paths: paths.clone(),
        state: input.state.clone(),
        spend: input.spend.to_vec(),
        traces: input.traces.to_vec(),
        events: input.events.to_vec(),
        live: input.live.clone(),
        tui_state: input.tui_state.clone(),
        config: config.clone(),
        kind,
    }
}

fn plan_narrative_refresh_request(
    input: &PlanNarrativeRefreshInput<'_>,
    kind: NarrativeRefreshKind,
) -> AttachPlanNarrativeRefreshRequest {
    AttachPlanNarrativeRefreshRequest {
        paths: input.paths.clone(),
        plan: input.plan.clone(),
        messages: input.messages.to_vec(),
        plan_events: input.plan_events.to_vec(),
        feed_events: input.feed_events.to_vec(),
        selected: input.selected,
        config: input.config.clone(),
        kind,
    }
}

const TRY_GOAL: &str = "create a tiny Rust hello-world smoke project and verify it";

async fn try_command(plain: bool, json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let try_dir = paths
        .home()
        .join("try")
        .join(Uuid::new_v4().simple().to_string());
    fs::create_dir_all(&try_dir)?;
    let scope = workspace_scope(&try_dir)?;
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&try_dir)?;
    let run_result = commands::run::run_command(RunCommandArgs {
        goal: TRY_GOAL.to_string(),
        fresh: true,
        worktree: false,
        from: None,
        in_place: false,
        base: None,
        branch: None,
        allow_dirty: false,
        init_git: false,
        yes: true,
        preview: false,
        brief: false,
        plain,
        prevent_sleep: Some("off".to_string()),
        quiet: true,
        max_spend: Some(1.0),
        max_wall_seconds: Some(60.0),
        sandbox: Some("none".to_string()),
        provider: None,
        model: None,
        doc_provider: None,
        acceptance: None,
        skill: "default-coding".to_string(),
        smoke: true,
        i_know_its_a_lot: false,
        no_confirm: true,
        no_hints: true,
        no_docs: false,
        doc_skill: None,
    })
    .await;
    let restore_result = std::env::set_current_dir(&original_dir);
    run_result?;
    restore_result?;

    let run = list_runs(&paths, Some(&scope))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::NotFound(
                "try run state after smoke execution".to_string(),
            ))
        })?;
    let state = load_run(&paths, &run.run_id)?;
    if state.status != RunStatus::Completed {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("try run did not complete: {}", state.status),
            "deadreckon show latest --why-failed",
        )));
    }
    let proof = proof_block_for_state(
        &paths,
        &state,
        "deadreckon start \"build the real thing\"".to_string(),
    )?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "run_id": state.run_id,
                "gate": "SIGNED by dr-gate",
                "proof": proof.proof_path,
                "story": proof.story_path,
                "lineage": proof.lineage,
                "next": "deadreckon start \"build the real thing\"",
            }))?
        );
    } else {
        print!("{}", proof.render_text());
    }
    Ok(())
}

fn proof_block_for_state(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    next_command: String,
) -> Result<ProofBlock> {
    let proof_path = marker_path_for_run_root(&state.run_root);
    if !proof_path.is_file() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "signed proof {}",
            proof_path.display()
        ))));
    }
    let library_dir = state
        .promoted_library_dir
        .clone()
        .unwrap_or_else(|| paths.library_dir(&state.scope, &state.run_id));
    let story_path = library_dir
        .join("docs")
        .join(deadreckon_core::RUN_NARRATIVE);
    let lineage = try_lineage_line(state)?;
    Ok(ProofBlock {
        proof_path,
        story_path,
        lineage,
        next_command,
    })
}

fn try_lineage_line(state: &deadreckon_core::PipelineState) -> Result<String> {
    let records = read_jsonl::<ProvenanceRecord>(&state.run_root.join("provenance.jsonl"))?;
    let original_working_dir = state.run_root.join("working");
    for record in records.iter().rev() {
        if let Some(file) = record
            .files
            .iter()
            .filter_map(|path| {
                path.strip_prefix(&original_working_dir)
                    .or_else(|_| path.strip_prefix(&state.working_dir))
                    .ok()
            })
            .find(|path| !path.starts_with("target") && !path.starts_with(".deadreckon"))
        {
            let turn = record
                .prompt_id
                .strip_prefix("turn-")
                .map(|number| format!("turn {number}"))
                .unwrap_or_else(|| record.prompt_id.clone());
            let provider = state.provider.as_deref().unwrap_or("provider");
            return Ok(format!(
                "{} ← {} · {} · {}",
                file.display(),
                turn,
                provider,
                record.tool_call_id
            ));
        }
    }
    Ok(format!(
        "working tree ← turn {} · {} · provenance",
        state.turn,
        state.provider.as_deref().unwrap_or("provider")
    ))
}

#[cfg(test)]
mod acceptance_integrity_tests;

#[cfg(test)]
mod acceptance_render_tests;

#[cfg(test)]
mod command_exists_tests;

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

#[derive(Debug, Clone)]
struct AcceptanceDisplay {
    gate: String,
    gate_tone: Tone,
    tests_modified: Option<bool>,
    caveats: Vec<String>,
}

impl AcceptanceDisplay {
    fn status_line(&self) -> String {
        let mut parts = vec![self.gate.clone()];
        if let Some(tests_modified) = self.tests_modified {
            parts.push(format!(
                "tests modified this run: {}",
                if tests_modified { "yes" } else { "no" }
            ));
        }
        parts.extend(
            self.caveats
                .iter()
                .map(|caveat| format!("accepted (caveat: {caveat})")),
        );
        parts.join("; ")
    }
}

fn acceptance_status_line(state: &deadreckon_core::PipelineState) -> String {
    acceptance_display(state).status_line()
}

fn acceptance_status_value(state: &deadreckon_core::PipelineState) -> String {
    let line = acceptance_status_line(state);
    line.strip_prefix("gate: ").unwrap_or(&line).to_string()
}

fn acceptance_display(state: &deadreckon_core::PipelineState) -> AcceptanceDisplay {
    let tamper = deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)
        .ok()
        .flatten();
    let marker_path = marker_path_for_run_root(&state.run_root);
    if marker_path.exists()
        && let Ok(bytes) = fs::read(&marker_path)
        && let Ok(marker) = serde_json::from_slice::<AcceptanceMarker>(&bytes)
    {
        return acceptance_display_from_gate_line(marker_gate_line(&marker), tamper.as_ref());
    }
    let progress_path = acceptance_progress_path_for_run_root(&state.run_root);
    if progress_path.exists()
        && let Ok(entries) = read_jsonl::<AcceptanceProgressEntry>(&progress_path)
        && !entries.is_empty()
    {
        return acceptance_display_from_gate_line(progress_gate_line(&entries), tamper.as_ref());
    }
    let spec_path = acceptance_spec_path_for_run_root(&state.run_root);
    if spec_path.exists()
        && let Ok(raw) = fs::read_to_string(&spec_path)
        && let Ok(count) = commands::acceptance::acceptance_check_count(&raw)
    {
        return acceptance_configured_display(format!("configured ({count} checks)"));
    }
    acceptance_configured_display("default dr-gate behavior".to_string())
}

fn acceptance_display_from_gate_line(
    mut gate: String,
    tamper: Option<&deadreckon_core::tamper::AcceptanceTamper>,
) -> AcceptanceDisplay {
    if let Some(refusal) = tamper
        .filter(|tamper| tamper.verdict == deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse)
    {
        gate = format!("gate: REFUSED - {}", refusal.refusal_reasons.join("; "));
    }
    let tests_modified = tamper.map(tamper_tests_modified);
    let caveats = tamper
        .filter(|tamper| tamper.verdict == deadreckon_core::tamper::AcceptanceTamperVerdict::Caveat)
        .map(|tamper| tamper.caveats.clone())
        .unwrap_or_default();
    let gate_tone = if !caveats.is_empty() {
        Tone::Warn
    } else if gate.contains("FAILED") || gate.contains("REFUSED") {
        Tone::Bad
    } else {
        Tone::Neutral
    };
    AcceptanceDisplay {
        gate,
        gate_tone,
        tests_modified,
        caveats,
    }
}

fn acceptance_configured_display(gate: String) -> AcceptanceDisplay {
    AcceptanceDisplay {
        gate,
        gate_tone: Tone::Neutral,
        tests_modified: None,
        caveats: Vec::new(),
    }
}

fn tamper_tests_modified(tamper: &deadreckon_core::tamper::AcceptanceTamper) -> bool {
    tamper
        .covered_files_touched
        .iter()
        .any(|touch| touch.classification == deadreckon_core::tamper::CoverageClassification::Test)
}

fn marker_gate_line(marker: &AcceptanceMarker) -> String {
    let total = marker.checks.len().max(marker.check_count);
    let passed = if marker.checks.is_empty() {
        marker.check_count
    } else {
        marker.checks.iter().filter(|result| result.passed).count()
    };
    gate_line_from_results(total, passed, &marker.checks, marker.check_count > 0)
}

fn progress_gate_line(entries: &[AcceptanceProgressEntry]) -> String {
    let total = entries.iter().map(|entry| entry.total).max().unwrap_or(0);
    let results = entries
        .iter()
        .filter_map(|entry| entry.result.as_ref())
        .cloned()
        .collect::<Vec<_>>();
    let passed = results.iter().filter(|result| result.passed).count();
    gate_line_from_results(total, passed, &results, false)
}

fn gate_line_from_results(
    total: usize,
    passed: usize,
    results: &[deadreckon_core::AcceptanceCheckResult],
    assume_passed_when_empty: bool,
) -> String {
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
        .or_else(|| results.iter().find(|result| !result.passed))
    {
        return format!(
            "gate: FAILED {}/{} - {} x {}",
            passed,
            total.max(results.len()),
            failed.kind,
            one_line(&failed.detail, 96)
        );
    }
    if results.is_empty() && !assume_passed_when_empty {
        return format!("gate: PENDING 0/{total}");
    }
    format!("gate: PASSED {}/{}", passed, total.max(passed))
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
    let workspace = codebase.mode.to_string();
    let done_label = acceptance.full_label();
    let launch_rows = launch_preview_rows(&LaunchPreviewFacts {
        goal,
        path: "run",
        suggestion: None,
        provider: agent,
        roles: None,
        base: None,
        history: None,
        done: &done_label,
        workspace: &workspace,
        watch: format!("deadreckon attach {run_id}"),
        stop: format!("deadreckon kill {run_id}"),
        finish: format!("deadreckon finish {run_id}"),
        override_command: Some("deadreckon orchestrate \"goal\"".to_string()),
    });
    let mut rows = vec![
        ("goal".to_string(), goal.to_string()),
        launch_rows
            .iter()
            .find(|(key, _)| key == "path")
            .cloned()
            .unwrap_or_else(|| ("path".to_string(), "run".to_string())),
        (
            "source".to_string(),
            format!("{} ({git_label})", cwd.display()),
        ),
        ("mode".to_string(), mode),
        launch_rows
            .iter()
            .find(|(key, _)| key == "workspace")
            .cloned()
            .unwrap_or_else(|| ("workspace".to_string(), workspace.clone())),
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
        (
            "done".to_string(),
            launch_rows
                .iter()
                .find(|(key, _)| key == "done")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| done_label.clone()),
        ),
        (NOUN_DONE_CONTRACT.to_string(), done_label),
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
    sections.push(Section::Blank);
    for label in ["watch", "stop", "finish"] {
        if let Some((_, command)) = launch_rows.iter().find(|(key, _)| key == label) {
            sections.push(Section::Command {
                label: label.to_string(),
                command: command.clone(),
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
            primary_action: None,
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

fn plan_docs_status_line(paths: &DeadreckonPaths, plan: &Plan) -> String {
    match read_plan_docs_manifest(paths, &plan.plan_id) {
        Ok(Some(manifest)) => format!(
            "{}; narrative {}",
            manifest.status,
            plan_doc_path(paths, &plan.plan_id, deadreckon_core::plan::PLAN_NARRATIVE).display()
        ),
        Ok(None) => "missing; run deadreckon doc <plan-id> --polish".to_string(),
        Err(err) => format!("unavailable: {err}"),
    }
}

const PLAN_AS_BUILT: &str = "PLAN-AS-BUILT.md";
const PLAN_DECISIONS: &str = "PLAN-DECISIONS.md";
const PLAN_CHILDREN: &str = "PLAN-CHILDREN.md";
const PLAN_DOCS_MANIFEST: &str = "PLAN-DOCS-MANIFEST.json";
const PLAN_DOC_INPUT: &str = "plan-doc-input.json";
const PLAN_DOC_PROVIDER_RESPONSE: &str = "plan-doc-provider-response.json";
const PLAN_DOC_PROVIDER_ERROR: &str = "plan-doc-provider-error.json";
const PLAN_DOC_EVENTS_JSONL: &str = "_plan-docs.jsonl";
const PLAN_DOC_SOURCE_EXCERPT_BYTES: usize = 30 * 1024;

#[derive(Debug, Clone)]
struct PlanDocRefreshOptions {
    provider: Option<String>,
    provider_source: String,
    budget_cap_usd: Option<f64>,
    force: bool,
}

#[derive(Debug, Clone)]
struct PlanWrapperDocContext {
    wrapper_run_id: String,
    merged_run_id: String,
}

#[derive(Debug, Clone)]
struct PlanDocTarget {
    plan: Plan,
    wrapper: Option<PlanWrapperDocContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocsManifest {
    schema_version: u32,
    plan_id: String,
    root_goal: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: String,
    input_hash: String,
    provider: PlanDocsProviderManifest,
    children: Vec<PlanDocsChildManifest>,
    outputs: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PlanDocsProviderManifest {
    route: Option<String>,
    source: String,
    calls: u32,
    cost_usd: f64,
    duration_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocsChildManifest {
    task_id: String,
    task_index: u32,
    depends_on: Vec<String>,
    child_run_id: Option<String>,
    status: String,
    provider: Option<String>,
    doc_sources: Vec<String>,
    doc_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocInput {
    schema_version: u32,
    plan_id: String,
    root_goal: String,
    mode: String,
    status: String,
    merged_run_id: Option<String>,
    task_order: Vec<String>,
    children: Vec<PlanDocChildInput>,
    result_inventory: Vec<String>,
    repair_summary: Vec<PlanDocKeyValue>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocChildInput {
    task_id: String,
    task_index: u32,
    subject: String,
    goal: String,
    role: String,
    provider: Option<String>,
    depends_on: Vec<String>,
    status: String,
    child_run_id: Option<String>,
    worker_spec: Option<PlanDocTextSource>,
    summary: Option<PlanDocTextSource>,
    docs: Vec<PlanDocTextSource>,
    doc_status: String,
    inventory: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocTextSource {
    evidence_id: String,
    path: String,
    bytes: usize,
    redactions: usize,
    excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanDocKeyValue {
    key: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderDocs {
    schema_version: u32,
    title: String,
    narrative: PlanProviderNarrative,
    as_built: PlanProviderAsBuilt,
    decisions: PlanProviderDecisions,
    children: Vec<PlanProviderChild>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderNarrative {
    summary: String,
    #[serde(default)]
    task_graph: Vec<PlanProviderItem>,
    #[serde(default)]
    phases: Vec<PlanProviderItem>,
    #[serde(default)]
    repairs: Vec<PlanProviderItem>,
    #[serde(default)]
    acceptance: Vec<PlanProviderItem>,
    #[serde(default)]
    open_threads: Vec<PlanProviderItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderAsBuilt {
    system_overview: String,
    #[serde(default)]
    components: Vec<PlanProviderItem>,
    #[serde(default)]
    changed_files: Vec<PlanProviderPathItem>,
    #[serde(default)]
    runtime_notes: Vec<PlanProviderItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderDecisions {
    #[serde(default)]
    decisions: Vec<PlanProviderItem>,
    #[serde(default)]
    tradeoffs: Vec<PlanProviderItem>,
    #[serde(default)]
    deferrals: Vec<PlanProviderItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderChild {
    task_id: String,
    summary: String,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderItem {
    title: Option<String>,
    text: String,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlanProviderPathItem {
    path: String,
    text: String,
    #[serde(default)]
    citations: Vec<String>,
}

fn plan_docs_dir(paths: &DeadreckonPaths, plan_id: &str) -> PathBuf {
    paths
        .plan_dir(plan_id)
        .join(deadreckon_core::plan::PLAN_DOCS_DIR)
}

fn plan_doc_path(paths: &DeadreckonPaths, plan_id: &str, file_name: &str) -> PathBuf {
    plan_docs_dir(paths, plan_id).join(file_name)
}

fn plan_doc_outputs() -> Vec<String> {
    [
        deadreckon_core::plan::PLAN_NARRATIVE,
        PLAN_AS_BUILT,
        PLAN_DECISIONS,
        PLAN_CHILDREN,
    ]
    .into_iter()
    .map(|name| format!("docs/{name}"))
    .collect()
}

fn plan_doc_kind_file_name(kind: CliDocKind) -> Option<&'static str> {
    match kind {
        CliDocKind::Narrative => Some(deadreckon_core::plan::PLAN_NARRATIVE),
        CliDocKind::AsBuilt => Some(PLAN_AS_BUILT),
        CliDocKind::Decisions => Some(PLAN_DECISIONS),
        CliDocKind::Children => Some(PLAN_CHILDREN),
        CliDocKind::Delta => None,
    }
}

fn run_doc_kind(kind: CliDocKind) -> Result<DocKind> {
    match kind {
        CliDocKind::Narrative => Ok(DocKind::Narrative),
        CliDocKind::AsBuilt => Ok(DocKind::AsBuilt),
        CliDocKind::Decisions => Ok(DocKind::Decisions),
        CliDocKind::Delta => Ok(DocKind::Delta),
        CliDocKind::Children => Err(CliError::Core(deadreckon_core::user_error(
            "`--kind children` is only available for orchestration plan docs",
            "deadreckon doc <plan-id> --kind children",
        ))),
    }
}

fn plan_docs_are_complete(paths: &DeadreckonPaths, plan: &Plan) -> bool {
    [
        deadreckon_core::plan::PLAN_NARRATIVE,
        PLAN_AS_BUILT,
        PLAN_DECISIONS,
        PLAN_CHILDREN,
    ]
    .into_iter()
    .all(|name| {
        let path = plan_doc_path(paths, &plan.plan_id, name);
        path.is_file()
            && fs::metadata(path)
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false)
    })
}

fn resolve_plan_doc_target(
    paths: &DeadreckonPaths,
    id: &str,
    loaded_run: Option<&deadreckon_core::PipelineState>,
) -> Result<Option<PlanDocTarget>> {
    if let Some(state) = loaded_run
        && let Some(wrapper) = plan_wrapper_context_from_run(paths, state)?
    {
        let plan = load_plan(paths, &wrapper.plan_id)?;
        return Ok(Some(PlanDocTarget {
            plan,
            wrapper: Some(PlanWrapperDocContext {
                wrapper_run_id: state.run_id.clone(),
                merged_run_id: wrapper.merged_run_id,
            }),
        }));
    }
    if !paths.plans_dir().is_dir() {
        return Ok(None);
    }
    match resolve_plan_id(paths, id) {
        Ok(plan_id) => Ok(Some(PlanDocTarget {
            plan: load_plan(paths, &plan_id)?,
            wrapper: None,
        })),
        Err(error) if error.to_string().contains("no plan") => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone)]
struct PlanApplyTraceContext {
    plan_id: String,
    merged_run_id: String,
}

fn plan_wrapper_context_from_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<Option<PlanApplyTraceContext>> {
    let traces_path = state.run_root.join("traces.jsonl");
    if !traces_path.exists() {
        return Ok(None);
    }
    let traces = read_jsonl::<TraceRecord>(&traces_path)?;
    for trace in traces.iter().rev() {
        if trace.event != "plan_result_apply_prepared" {
            continue;
        }
        let Some(plan_id) = trace.detail.get("plan_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(merged_run_id) = trace.detail.get("merged_run_id").and_then(Value::as_str) else {
            continue;
        };
        // Make sure the plan still exists before claiming this run is a plan wrapper.
        load_plan(paths, plan_id)?;
        return Ok(Some(PlanApplyTraceContext {
            plan_id: plan_id.to_string(),
            merged_run_id: merged_run_id.to_string(),
        }));
    }
    Ok(None)
}

fn ensure_plan_docs_deterministic(
    paths: &DeadreckonPaths,
    plan: &Plan,
) -> Result<PlanDocsManifest> {
    if plan_docs_are_complete(paths, plan)
        && let Some(manifest) = read_plan_docs_manifest(paths, &plan.plan_id)?
    {
        return Ok(manifest);
    }
    write_plan_docs_deterministic(paths, plan, None, "none", None)
}

fn read_plan_docs_manifest(
    paths: &DeadreckonPaths,
    plan_id: &str,
) -> Result<Option<PlanDocsManifest>> {
    let path = plan_doc_path(paths, plan_id, PLAN_DOCS_MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn write_plan_docs_deterministic(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: Option<String>,
    provider_source: &str,
    provider_error: Option<String>,
) -> Result<PlanDocsManifest> {
    let input = collect_plan_doc_input(paths, plan)?;
    write_plan_doc_input(paths, plan, &input)?;
    let input_hash = plan_doc_input_hash(&input)?;
    let mut warnings = input.warnings.clone();
    let status = if provider_error.is_some() {
        "failed_provider_fallback"
    } else {
        "deterministic"
    };
    if let Some(error) = provider_error {
        warnings.push(format!("provider fallback: {error}"));
    }
    write_plan_docs_from_fallback(paths, plan, &input, &warnings)?;
    let manifest = manifest_from_plan_doc_input(
        plan,
        &input,
        status,
        input_hash,
        PlanDocsProviderManifest {
            route: provider,
            source: provider_source.to_string(),
            calls: 0,
            cost_usd: 0.0,
            duration_ms: None,
        },
        warnings,
    );
    write_plan_docs_manifest(paths, &manifest)?;
    append_plan_doc_event(
        paths,
        plan,
        "written",
        &json!({ "status": manifest.status, "input_hash": manifest.input_hash }),
    )?;
    Ok(manifest)
}

async fn refresh_plan_docs(
    paths: &DeadreckonPaths,
    plan: &Plan,
    options: PlanDocRefreshOptions,
) -> Result<PlanDocsManifest> {
    if !options.force
        && plan_docs_are_complete(paths, plan)
        && let Some(manifest) = read_plan_docs_manifest(paths, &plan.plan_id)?
    {
        return Ok(manifest);
    }
    let input = collect_plan_doc_input(paths, plan)?;
    write_plan_doc_input(paths, plan, &input)?;
    let input_hash = plan_doc_input_hash(&input)?;
    append_plan_doc_event(
        paths,
        plan,
        "collected",
        &json!({ "input_hash": input_hash, "children": input.children.len() }),
    )?;
    let Some(provider) = options.provider.clone() else {
        return write_plan_docs_deterministic(paths, plan, None, "none", None);
    };
    let router = match ProviderRouter::from_config_path(&paths.config_path(), Some(&provider)) {
        Ok(router) => router,
        Err(err) => {
            write_plan_provider_error(paths, plan, &provider, &err.to_string())?;
            return write_plan_docs_deterministic(
                paths,
                plan,
                Some(provider),
                &options.provider_source,
                Some(err.to_string()),
            );
        }
    };
    let estimated_spend =
        estimate_doc_polish_spend(&router, &provider, DEFAULT_DOC_POLISH_TOKEN_BUDGET, 1)?;
    if let Some(cap) = options.budget_cap_usd
        && estimated_spend.cost_usd > cap
    {
        let reason = format!(
            "plan doc polish would cost about ${:.6}, above cap ${cap:.6}",
            estimated_spend.cost_usd
        );
        write_plan_provider_error(paths, plan, &provider, &reason)?;
        append_plan_doc_event(
            paths,
            plan,
            "provider_skipped",
            &json!({
                "provider": provider,
                "reason": reason,
                "estimated_cost_usd": estimated_spend.cost_usd,
                "budget_cap_usd": cap
            }),
        )?;
        return write_plan_docs_deterministic(
            paths,
            plan,
            Some(provider),
            &options.provider_source,
            Some(reason),
        );
    }
    let started = Instant::now();
    append_plan_doc_event(
        paths,
        plan,
        "provider_requested",
        &json!({ "provider": provider, "input_hash": input_hash }),
    )?;
    let response = router
        .complete(&ProviderRequest {
            prompt: plan_doc_provider_prompt(&input)?,
            max_output_tokens: 16_384,
            cwd: plan.parent_cwd.clone(),
            output_path: Some(plan_doc_path(paths, &plan.plan_id, "plan-doc-provider.out")),
            sandbox_backend: Some(SandboxBackend::None),
            pid_file: None,
            cancellation_token: None,
        })
        .await;
    let response = match response {
        Ok(response) => response,
        Err(err) => {
            write_plan_provider_error(paths, plan, &provider, &err.to_string())?;
            append_plan_doc_event(
                paths,
                plan,
                "provider_failed",
                &json!({ "provider": provider, "error": err.to_string() }),
            )?;
            return write_plan_docs_deterministic(
                paths,
                plan,
                Some(provider),
                &options.provider_source,
                Some(err.to_string()),
            );
        }
    };
    let duration_ms = started.elapsed().as_millis();
    write_json_pretty(
        &plan_doc_path(paths, &plan.plan_id, PLAN_DOC_PROVIDER_RESPONSE),
        &json!({
            "provider": response.provider,
            "model": response.model,
            "estimated_cost_usd": estimated_spend.cost_usd,
            "usage": response.usage,
            "spend": response.spend,
            "content": response.content,
        }),
    )?;
    append_plan_doc_event(
        paths,
        plan,
        "provider_completed",
        &json!({ "provider": response.provider, "duration_ms": duration_ms }),
    )?;
    let provider_docs = match serde_json::from_str::<PlanProviderDocs>(&response.content) {
        Ok(docs) => docs,
        Err(err) => {
            write_plan_provider_error(paths, plan, &response.provider, &err.to_string())?;
            return write_plan_docs_deterministic(
                paths,
                plan,
                Some(response.provider),
                &options.provider_source,
                Some(format!("provider JSON parse failed: {err}")),
            );
        }
    };
    if let Err(err) = validate_plan_provider_docs(&input, &provider_docs) {
        write_plan_provider_error(paths, plan, &response.provider, &err.to_string())?;
        return write_plan_docs_deterministic(
            paths,
            plan,
            Some(response.provider),
            &options.provider_source,
            Some(format!("provider validation failed: {err}")),
        );
    }
    write_plan_docs_from_provider(paths, plan, &input, &provider_docs)?;
    let manifest = manifest_from_plan_doc_input(
        plan,
        &input,
        "provider",
        input_hash,
        PlanDocsProviderManifest {
            route: Some(response.provider),
            source: options.provider_source,
            calls: 1,
            cost_usd: response.spend.cost_usd,
            duration_ms: Some(duration_ms),
        },
        input.warnings.clone(),
    );
    write_plan_docs_manifest(paths, &manifest)?;
    append_plan_doc_event(
        paths,
        plan,
        "validated",
        &json!({ "status": manifest.status, "input_hash": manifest.input_hash }),
    )?;
    Ok(manifest)
}

fn manifest_from_plan_doc_input(
    plan: &Plan,
    input: &PlanDocInput,
    status: &str,
    input_hash: String,
    provider: PlanDocsProviderManifest,
    warnings: Vec<String>,
) -> PlanDocsManifest {
    let now = Utc::now();
    PlanDocsManifest {
        schema_version: 1,
        plan_id: plan.plan_id.clone(),
        root_goal: plan.root_goal.clone(),
        created_at: now,
        updated_at: now,
        status: status.to_string(),
        input_hash,
        provider,
        children: input
            .children
            .iter()
            .map(|child| PlanDocsChildManifest {
                task_id: child.task_id.clone(),
                task_index: child.task_index,
                depends_on: child.depends_on.clone(),
                child_run_id: child.child_run_id.clone(),
                status: child.status.clone(),
                provider: child.provider.clone(),
                doc_sources: child.docs.iter().map(|doc| doc.path.clone()).collect(),
                doc_status: child.doc_status.clone(),
            })
            .collect(),
        outputs: plan_doc_outputs(),
        warnings,
    }
}

fn collect_plan_doc_input(paths: &DeadreckonPaths, plan: &Plan) -> Result<PlanDocInput> {
    let ordered_tasks = plan_tasks_in_doc_order(plan);
    let mut warnings = Vec::new();
    let children = ordered_tasks
        .iter()
        .map(|task| collect_plan_doc_child(paths, plan, task, &mut warnings))
        .collect::<Result<Vec<_>>>()?;
    let result_inventory = plan_result_inventory(paths, plan)?;
    let repair_summary = plan_merge_repair_summary_items(paths, plan)
        .into_iter()
        .map(|(key, value)| PlanDocKeyValue { key, value })
        .collect();
    Ok(PlanDocInput {
        schema_version: 1,
        plan_id: plan.plan_id.clone(),
        root_goal: plan.root_goal.clone(),
        mode: plan.mode.as_str().to_string(),
        status: plan_status_label(plan.status).to_string(),
        merged_run_id: plan.merged_run_id.clone(),
        task_order: ordered_tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect(),
        children,
        result_inventory,
        repair_summary,
        warnings,
    })
}

fn collect_plan_doc_child(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    warnings: &mut Vec<String>,
) -> Result<PlanDocChildInput> {
    let worker_spec = read_plan_doc_text_source(
        paths,
        &plan.plan_id,
        &paths.worker_spec(&plan.plan_id, &task.task_id),
        format!("worker:{}", task.task_id),
    )?;
    let summary_path = task
        .summary_path
        .as_ref()
        .map(|relative| paths.plan_dir(&plan.plan_id).join(relative))
        .unwrap_or_else(|| paths.child_summary(&plan.plan_id, &task.task_id));
    let summary = read_plan_doc_text_source(
        paths,
        &plan.plan_id,
        &summary_path,
        format!("summary:{}", task.task_id),
    )?;
    let mut docs = Vec::new();
    let mut inventory = Vec::new();
    if let Some(run_id) = task.child_run_id.as_deref() {
        match load_run(paths, run_id) {
            Ok(state) => {
                let root = child_artifact_root(paths, &state);
                docs.extend(read_child_doc_sources(paths, plan, task, &root)?);
                inventory = inventory_strings(&root, 200).unwrap_or_default();
            }
            Err(err) => warnings.push(format!(
                "{} references child run {run_id}, but it could not be loaded: {err}",
                task.task_id
            )),
        }
    }
    let doc_status = classify_plan_child_doc_status(&docs, summary.as_ref());
    if doc_status == "missing" {
        warnings.push(format!("{} has no child docs or summary", task.task_id));
    }
    Ok(PlanDocChildInput {
        task_id: task.task_id.clone(),
        task_index: task.index,
        subject: task.subject.clone(),
        goal: task.goal.clone(),
        role: plan_doc_role_label(task.role).to_string(),
        provider: task.provider.clone(),
        depends_on: task.depends_on.clone(),
        status: plan_task_status_label(task.status).to_string(),
        child_run_id: task.child_run_id.clone(),
        worker_spec,
        summary,
        docs,
        doc_status,
        inventory,
    })
}

fn plan_doc_role_label(role: PlanRole) -> &'static str {
    match role {
        PlanRole::Child => "child",
        PlanRole::Coder => "coder",
        PlanRole::Reviewer => "reviewer",
    }
}

fn read_child_doc_sources(
    paths: &DeadreckonPaths,
    plan: &Plan,
    task: &PlanTask,
    root: &Path,
) -> Result<Vec<PlanDocTextSource>> {
    let mut sources = Vec::new();
    for (kind, relative) in [
        (
            "narrative",
            format!(".deadreckon/docs/{}", deadreckon_core::RUN_NARRATIVE),
        ),
        (
            "as-built",
            format!(".deadreckon/docs/{}", deadreckon_core::RUN_AS_BUILT),
        ),
        (
            "decisions",
            format!(".deadreckon/docs/{}", deadreckon_core::RUN_DECISIONS),
        ),
        (
            "public-narrative",
            format!("docs/{}", deadreckon_core::RUN_NARRATIVE),
        ),
        (
            "public-as-built",
            format!("docs/{}", deadreckon_core::RUN_AS_BUILT),
        ),
        (
            "public-decisions",
            format!("docs/{}", deadreckon_core::RUN_DECISIONS),
        ),
    ] {
        let path = root.join(&relative);
        if let Some(source) = read_plan_doc_text_source(
            paths,
            &plan.plan_id,
            &path,
            format!("doc:{}:{kind}", task.task_id),
        )? {
            sources.push(source);
        }
    }
    Ok(sources)
}

fn read_plan_doc_text_source(
    paths: &DeadreckonPaths,
    plan_id: &str,
    path: &Path,
    evidence_id: String,
) -> Result<Option<PlanDocTextSource>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let (redacted, redactions) = redact_plan_doc_text(&raw);
            let excerpt = cap_utf8_local(&redacted, PLAN_DOC_SOURCE_EXCERPT_BYTES);
            Ok(Some(PlanDocTextSource {
                evidence_id,
                path: display_plan_doc_source_path(paths, plan_id, path),
                bytes: raw.len(),
                redactions,
                excerpt,
            }))
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn display_plan_doc_source_path(paths: &DeadreckonPaths, plan_id: &str, path: &Path) -> String {
    path.strip_prefix(paths.plan_dir(plan_id))
        .or_else(|_| path.strip_prefix(paths.home()))
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

fn classify_plan_child_doc_status(
    docs: &[PlanDocTextSource],
    summary: Option<&PlanDocTextSource>,
) -> String {
    let has_useful_doc = docs.iter().any(|doc| {
        !doc.excerpt.contains("Doc-writer: templated only")
            && !doc
                .excerpt
                .contains("No completed turns have been recorded yet")
            && doc.excerpt.trim().len() > 40
    });
    if has_useful_doc {
        return "polished".to_string();
    }
    if !docs.is_empty() {
        return "templated".to_string();
    }
    if summary.is_some_and(|source| !source.excerpt.trim().is_empty()) {
        return "summary_only".to_string();
    }
    "missing".to_string()
}

fn plan_tasks_in_doc_order(plan: &Plan) -> Vec<&PlanTask> {
    fn visit<'a>(
        task: &'a PlanTask,
        by_id: &BTreeMap<&'a str, &'a PlanTask>,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
        out: &mut Vec<&'a PlanTask>,
    ) {
        if visited.contains(task.task_id.as_str()) || !visiting.insert(task.task_id.as_str()) {
            return;
        }
        let mut deps = task
            .depends_on
            .iter()
            .filter_map(|dep| by_id.get(dep.as_str()).copied())
            .collect::<Vec<_>>();
        deps.sort_by_key(|dep| (dep.index, dep.task_id.clone()));
        for dep in deps {
            visit(dep, by_id, visiting, visited, out);
        }
        visiting.remove(task.task_id.as_str());
        visited.insert(task.task_id.as_str());
        out.push(task);
    }

    let mut tasks = plan.tasks.iter().collect::<Vec<_>>();
    tasks.sort_by_key(|task| (task.index, task.task_id.clone()));
    let by_id = tasks
        .iter()
        .map(|task| (task.task_id.as_str(), *task))
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut out = Vec::new();
    for task in tasks {
        visit(task, &by_id, &mut visiting, &mut visited, &mut out);
    }
    out
}

fn plan_result_inventory(paths: &DeadreckonPaths, plan: &Plan) -> Result<Vec<String>> {
    if let Some(run_id) = plan.merged_run_id.as_deref()
        && let Ok(state) = load_run(paths, run_id)
    {
        let library = paths.library_dir(&state.scope, &state.run_id);
        if library.is_dir() {
            return inventory_strings(&library, 500);
        }
    }
    let merge_working = paths.merge_working(&plan.plan_id);
    if merge_working.is_dir() {
        return inventory_strings(&merge_working, 500);
    }
    Ok(Vec::new())
}

fn inventory_strings(root: &Path, max: usize) -> Result<Vec<String>> {
    let mut files = inventory_files(root)?
        .into_iter()
        .filter_map(|path| {
            path.strip_prefix(root)
                .ok()
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        })
        .filter(|relative| !path_has_component(Path::new(relative), ".git"))
        .collect::<Vec<_>>();
    files.sort();
    files.truncate(max);
    Ok(files)
}

fn redact_plan_doc_text(raw: &str) -> (String, usize) {
    let Ok(secret_like) =
        Regex::new(r"(?i)(api[_-]?key|token|password|secret|authorization)(\s*[:=]\s*)\S+")
    else {
        return (raw.to_string(), 0);
    };
    let redactions = secret_like.find_iter(raw).count();
    let redacted = secret_like.replace_all(raw, "$1$2[REDACTED]").to_string();
    (redacted, redactions)
}

fn cap_utf8_local(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let mut end = max_bytes;
    while !raw.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}\n\n[truncated: {} bytes total]", &raw[..end], raw.len())
}

fn plan_doc_input_hash(input: &PlanDocInput) -> Result<String> {
    let bytes = serde_json::to_vec(input)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn write_plan_doc_input(paths: &DeadreckonPaths, plan: &Plan, input: &PlanDocInput) -> Result<()> {
    write_json_pretty(&plan_doc_path(paths, &plan.plan_id, PLAN_DOC_INPUT), input)
}

fn write_plan_docs_manifest(paths: &DeadreckonPaths, manifest: &PlanDocsManifest) -> Result<()> {
    write_json_pretty(
        &plan_doc_path(paths, &manifest.plan_id, PLAN_DOCS_MANIFEST),
        manifest,
    )
}

fn write_plan_provider_error(
    paths: &DeadreckonPaths,
    plan: &Plan,
    provider: &str,
    error: &str,
) -> Result<()> {
    write_json_pretty(
        &plan_doc_path(paths, &plan.plan_id, PLAN_DOC_PROVIDER_ERROR),
        &json!({
            "schema_version": 1,
            "provider": provider,
            "error": error,
            "recorded_at": Utc::now(),
        }),
    )
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn append_plan_doc_event(
    paths: &DeadreckonPaths,
    plan: &Plan,
    event: &str,
    detail: &Value,
) -> Result<()> {
    let path = plan_doc_path(paths, &plan.plan_id, PLAN_DOC_EVENTS_JSONL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    deadreckon_core::state::append_json_line(
        &path,
        &json!({
            "schema_version": 1,
            "timestamp": Utc::now(),
            "event": event,
            "plan_id": plan.plan_id,
            "detail": detail,
        }),
    )?;
    Ok(())
}

fn write_plan_docs_from_fallback(
    paths: &DeadreckonPaths,
    plan: &Plan,
    input: &PlanDocInput,
    warnings: &[String],
) -> Result<()> {
    let docs_dir = plan_docs_dir(paths, &plan.plan_id);
    fs::create_dir_all(&docs_dir)?;
    fs::write(
        docs_dir.join(deadreckon_core::plan::PLAN_NARRATIVE),
        render_plan_narrative_fallback(plan, input, warnings),
    )?;
    fs::write(
        docs_dir.join(PLAN_AS_BUILT),
        render_plan_as_built_fallback(plan, input, warnings),
    )?;
    fs::write(
        docs_dir.join(PLAN_DECISIONS),
        render_plan_decisions_fallback(plan, input, warnings),
    )?;
    fs::write(
        docs_dir.join(PLAN_CHILDREN),
        render_plan_children(plan, input),
    )?;
    Ok(())
}

fn render_plan_narrative_fallback(
    plan: &Plan,
    input: &PlanDocInput,
    warnings: &[String],
) -> String {
    let mut out = plan_doc_header("Plan Narrative", plan, input, "deterministic");
    out.push_str("## Reading Order\n\n");
    out.push_str("Start here, then read `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, and `PLAN-CHILDREN.md`. Child run docs and summaries are cited by task id below.\n\n");
    out.push_str("## Outcome\n\n");
    out.push_str(&format!(
        "Plan `{}` is `{}` and produced result run `{}`.\n\n",
        run_prefix(&plan.plan_id),
        plan_status_label(plan.status),
        plan.merged_run_id
            .as_deref()
            .map(run_prefix)
            .unwrap_or_else(|| "-".to_string())
    ));
    out.push_str("## Task Graph\n\n");
    for task_id in &input.task_order {
        if let Some(child) = input
            .children
            .iter()
            .find(|child| &child.task_id == task_id)
        {
            let deps = if child.depends_on.is_empty() {
                "none".to_string()
            } else {
                child.depends_on.join(", ")
            };
            out.push_str(&format!(
                "- `{}` after `{}` -> status `{}`; evidence `task:{}`{}\n",
                child.task_id,
                deps,
                child.status,
                child.task_id,
                child
                    .child_run_id
                    .as_deref()
                    .map(|run_id| format!(", `run:{}`", run_prefix(run_id)))
                    .unwrap_or_default()
            ));
        }
    }
    out.push_str("\n## Child Work\n\n");
    for child in &input.children {
        out.push_str(&format!("### {} - {}\n\n", child.task_id, child.subject));
        out.push_str(&format!(
            "- Provider: `{}`\n- Status: `{}`\n- Docs: `{}`\n",
            child.provider.as_deref().unwrap_or("-"),
            child.status,
            child.doc_status
        ));
        if let Some(run_id) = child.child_run_id.as_deref() {
            out.push_str(&format!("- Run: `{run_id}`\n"));
        }
        out.push('\n');
        if let Some(summary) = child.summary.as_ref() {
            out.push_str("Summary evidence: ");
            out.push_str(&format!("`{}`\n\n", summary.evidence_id));
            out.push_str(&first_paragraph(&summary.excerpt));
            out.push_str("\n\n");
        } else {
            out.push_str("No child summary was recorded.\n\n");
        }
    }
    if !input.repair_summary.is_empty() {
        out.push_str("## Merge And Repair\n\n");
        for item in &input.repair_summary {
            out.push_str(&format!("- {}: {}\n", item.key, item.value));
        }
        out.push('\n');
    }
    out.push_str("## Missing Evidence\n\n");
    if warnings.is_empty() {
        out.push_str("- No missing evidence was detected during deterministic consolidation.\n");
    } else {
        for warning in warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

fn render_plan_as_built_fallback(plan: &Plan, input: &PlanDocInput, warnings: &[String]) -> String {
    let mut out = plan_doc_header("Plan As Built", plan, input, "deterministic");
    out.push_str("## System Overview\n\n");
    out.push_str("This document consolidates the merged plan result from child summaries, child docs, and the final result inventory.\n\n");
    out.push_str("## Result Inventory\n\n");
    if input.result_inventory.is_empty() {
        out.push_str("- No merged result inventory was available.\n");
    } else {
        for path in input.result_inventory.iter().take(200) {
            out.push_str(&format!("- `{path}`\n"));
        }
    }
    out.push_str("\n## Child Contributions\n\n");
    for child in &input.children {
        out.push_str(&format!("### {}\n\n", child.task_id));
        out.push_str(&format!("{}\n\n", child.goal));
        if child.inventory.is_empty() {
            out.push_str("- No child artifact inventory was available.\n\n");
        } else {
            for path in child.inventory.iter().take(50) {
                out.push_str(&format!("- `{path}`\n"));
            }
            out.push('\n');
        }
    }
    if !warnings.is_empty() {
        out.push_str("## Gaps\n\n");
        for warning in warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

fn render_plan_decisions_fallback(
    plan: &Plan,
    input: &PlanDocInput,
    warnings: &[String],
) -> String {
    let mut out = plan_doc_header("Plan Decisions", plan, input, "deterministic");
    out.push_str("## Decisions And Tradeoffs\n\n");
    let mut wrote_decision = false;
    for child in &input.children {
        let decisions = child
            .docs
            .iter()
            .filter(|doc| doc.path.contains("DECISIONS"))
            .collect::<Vec<_>>();
        if decisions.is_empty() {
            out.push_str(&format!(
                "- `{}`: no explicit child decision doc was available; status `{}`.\n",
                child.task_id, child.doc_status
            ));
            continue;
        }
        wrote_decision = true;
        out.push_str(&format!("### {}\n\n", child.task_id));
        for decision in decisions {
            out.push_str(&format!("Evidence `{}`:\n\n", decision.evidence_id));
            out.push_str(&first_paragraph(&decision.excerpt));
            out.push_str("\n\n");
        }
    }
    if !wrote_decision {
        out.push_str("\nNo explicit multi-alternative decisions were found in child docs. Merge and orchestration assumptions are listed below when available.\n");
    }
    if !input.repair_summary.is_empty() {
        out.push_str("\n## Merge Repair Decisions\n\n");
        for item in &input.repair_summary {
            out.push_str(&format!("- {}: {}\n", item.key, item.value));
        }
    }
    if !warnings.is_empty() {
        out.push_str("\n## Deferrals And Missing Evidence\n\n");
        for warning in warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

fn render_plan_children(plan: &Plan, input: &PlanDocInput) -> String {
    let mut out = plan_doc_header("Plan Children", plan, input, "deterministic");
    out.push_str("## Child Index\n\n");
    out.push_str("| Task | Depends on | Provider | Status | Run | Docs |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for child in &input.children {
        out.push_str(&format!(
            "| `{}` | {} | `{}` | `{}` | `{}` | `{}` |\n",
            child.task_id,
            if child.depends_on.is_empty() {
                "none".to_string()
            } else {
                child.depends_on.join(", ")
            },
            child.provider.as_deref().unwrap_or("-"),
            child.status,
            child.child_run_id.as_deref().unwrap_or("-"),
            child.doc_status
        ));
    }
    out.push_str("\n## Evidence Sources\n\n");
    for child in &input.children {
        out.push_str(&format!("### {}\n\n", child.task_id));
        if let Some(source) = child.worker_spec.as_ref() {
            out.push_str(&format!(
                "- Worker spec: `{}` ({})\n",
                source.path, source.evidence_id
            ));
        }
        if let Some(source) = child.summary.as_ref() {
            out.push_str(&format!(
                "- Summary: `{}` ({})\n",
                source.path, source.evidence_id
            ));
        }
        for source in &child.docs {
            out.push_str(&format!(
                "- Doc: `{}` ({})\n",
                source.path, source.evidence_id
            ));
        }
        out.push('\n');
    }
    out
}

fn plan_doc_header(title: &str, plan: &Plan, input: &PlanDocInput, writer: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!("**Generated:** {}\n", Utc::now().to_rfc3339()));
    out.push_str(&format!("**Plan ID:** `{}`\n", plan.plan_id));
    out.push_str(&format!("**Goal:** {}\n", plan.root_goal));
    out.push_str(&format!("**Status:** {}\n", plan_status_label(plan.status)));
    out.push_str(&format!("**Mode:** {}\n", plan.mode.as_str()));
    if let Some(run_id) = input.merged_run_id.as_deref() {
        out.push_str(&format!("**Result run:** `{run_id}`\n"));
    }
    out.push_str(&format!("**Doc-writer:** plan-docs {writer}\n\n"));
    out
}

fn first_paragraph(raw: &str) -> String {
    raw.split("\n\n")
        .find(|part| !part.trim().is_empty())
        .unwrap_or(raw)
        .trim()
        .lines()
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
}

fn plan_doc_provider_prompt(input: &PlanDocInput) -> Result<String> {
    Ok(format!(
        "You are consolidating DeadReckon orchestration plan documentation.\n\
Return one strict JSON object with schema_version=1 and this shape:\n\
{{\"schema_version\":1,\"title\":\"...\",\"narrative\":{{\"summary\":\"...\",\"task_graph\":[],\"phases\":[],\"repairs\":[],\"acceptance\":[],\"open_threads\":[]}},\"as_built\":{{\"system_overview\":\"...\",\"components\":[],\"changed_files\":[],\"runtime_notes\":[]}},\"decisions\":{{\"decisions\":[],\"tradeoffs\":[],\"deferrals\":[]}},\"children\":[{{\"task_id\":\"task-0\",\"summary\":\"...\",\"citations\":[\"task:task-0\"]}}]}}\n\
Each item in task_graph, phases, repairs, acceptance, open_threads, components, runtime_notes, decisions, tradeoffs, and deferrals must be {{\"title\":null_or_string,\"text\":\"...\",\"citations\":[\"evidence-id\"]}}.\n\
Each changed_files item must be {{\"path\":\"relative/path\",\"text\":\"...\",\"citations\":[\"evidence-id\"]}}.\n\
Every concrete claim needs citations from the input evidence ids. Do not invent files; name missing evidence as missing.\n\n\
INPUT JSON:\n{}",
        serde_json::to_string_pretty(input)?
    ))
}

fn validate_plan_provider_docs(input: &PlanDocInput, docs: &PlanProviderDocs) -> Result<()> {
    if docs.schema_version != 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "plan docs provider response schema_version must be 1".to_string(),
        )));
    }
    if docs.title.trim().is_empty() || docs.narrative.summary.trim().is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "plan docs provider response was too small".to_string(),
        )));
    }
    let evidence = plan_doc_evidence_ids(input);
    for citation in provider_doc_citations(docs) {
        if !evidence.contains(citation.as_str()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown plan doc citation {citation}"
            ))));
        }
    }
    let known_paths = input
        .result_inventory
        .iter()
        .cloned()
        .chain(
            input
                .children
                .iter()
                .flat_map(|child| child.inventory.iter().cloned()),
        )
        .collect::<BTreeSet<_>>();
    for file in &docs.as_built.changed_files {
        if !known_paths.contains(&file.path) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "provider invented file path {}",
                file.path
            ))));
        }
    }
    let covered_children = docs
        .children
        .iter()
        .map(|child| child.task_id.as_str())
        .collect::<BTreeSet<_>>();
    for child in &input.children {
        if child.status == "completed" && !covered_children.contains(child.task_id.as_str()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "provider omitted completed child {}",
                child.task_id
            ))));
        }
    }
    Ok(())
}

fn plan_doc_evidence_ids(input: &PlanDocInput) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    ids.insert(format!("plan:{}", input.plan_id));
    for child in &input.children {
        ids.insert(format!("task:{}", child.task_id));
        if let Some(run_id) = child.child_run_id.as_deref() {
            ids.insert(format!("run:{}", run_prefix(run_id)));
            ids.insert(format!("run:{run_id}"));
        }
        if let Some(source) = child.worker_spec.as_ref() {
            ids.insert(source.evidence_id.clone());
        }
        if let Some(source) = child.summary.as_ref() {
            ids.insert(source.evidence_id.clone());
        }
        for source in &child.docs {
            ids.insert(source.evidence_id.clone());
        }
    }
    ids
}

fn provider_doc_citations(docs: &PlanProviderDocs) -> Vec<String> {
    let mut citations = Vec::new();
    for item in docs
        .narrative
        .task_graph
        .iter()
        .chain(docs.narrative.phases.iter())
        .chain(docs.narrative.repairs.iter())
        .chain(docs.narrative.acceptance.iter())
        .chain(docs.narrative.open_threads.iter())
        .chain(docs.as_built.components.iter())
        .chain(docs.as_built.runtime_notes.iter())
        .chain(docs.decisions.decisions.iter())
        .chain(docs.decisions.tradeoffs.iter())
        .chain(docs.decisions.deferrals.iter())
    {
        citations.extend(item.citations.clone());
    }
    for item in &docs.as_built.changed_files {
        citations.extend(item.citations.clone());
    }
    for child in &docs.children {
        citations.extend(child.citations.clone());
    }
    citations
}

fn write_plan_docs_from_provider(
    paths: &DeadreckonPaths,
    plan: &Plan,
    input: &PlanDocInput,
    docs: &PlanProviderDocs,
) -> Result<()> {
    let docs_dir = plan_docs_dir(paths, &plan.plan_id);
    fs::create_dir_all(&docs_dir)?;
    fs::write(
        docs_dir.join(deadreckon_core::plan::PLAN_NARRATIVE),
        render_provider_plan_narrative(plan, input, docs),
    )?;
    fs::write(
        docs_dir.join(PLAN_AS_BUILT),
        render_provider_plan_as_built(plan, input, docs),
    )?;
    fs::write(
        docs_dir.join(PLAN_DECISIONS),
        render_provider_plan_decisions(plan, input, docs),
    )?;
    fs::write(
        docs_dir.join(PLAN_CHILDREN),
        render_plan_children(plan, input),
    )?;
    Ok(())
}

fn render_provider_plan_narrative(
    plan: &Plan,
    input: &PlanDocInput,
    docs: &PlanProviderDocs,
) -> String {
    let mut out = plan_doc_header("Plan Narrative", plan, input, "provider");
    out.push_str(&format!(
        "## {}\n\n{}\n\n",
        docs.title, docs.narrative.summary
    ));
    render_provider_items(&mut out, "Task Graph", &docs.narrative.task_graph);
    render_provider_items(&mut out, "Phases", &docs.narrative.phases);
    render_provider_items(&mut out, "Repairs", &docs.narrative.repairs);
    render_provider_items(&mut out, "Acceptance", &docs.narrative.acceptance);
    render_provider_items(&mut out, "Open Threads", &docs.narrative.open_threads);
    out.push_str("## Children\n\n");
    for child in &docs.children {
        out.push_str(&format!(
            "### {}\n\n{}\n\n{}\n\n",
            child.task_id,
            child.summary,
            citation_suffix(&child.citations)
        ));
    }
    out
}

fn render_provider_plan_as_built(
    plan: &Plan,
    input: &PlanDocInput,
    docs: &PlanProviderDocs,
) -> String {
    let mut out = plan_doc_header("Plan As Built", plan, input, "provider");
    out.push_str(&format!(
        "## System Overview\n\n{}\n\n",
        docs.as_built.system_overview
    ));
    render_provider_items(&mut out, "Components", &docs.as_built.components);
    out.push_str("## Changed Files\n\n");
    for file in &docs.as_built.changed_files {
        out.push_str(&format!(
            "- `{}`: {} {}\n",
            file.path,
            file.text,
            citation_suffix(&file.citations)
        ));
    }
    out.push('\n');
    render_provider_items(&mut out, "Runtime Notes", &docs.as_built.runtime_notes);
    out
}

fn render_provider_plan_decisions(
    plan: &Plan,
    input: &PlanDocInput,
    docs: &PlanProviderDocs,
) -> String {
    let mut out = plan_doc_header("Plan Decisions", plan, input, "provider");
    render_provider_items(&mut out, "Decisions", &docs.decisions.decisions);
    render_provider_items(&mut out, "Tradeoffs", &docs.decisions.tradeoffs);
    render_provider_items(&mut out, "Deferrals", &docs.decisions.deferrals);
    out
}

fn render_provider_items(out: &mut String, heading: &str, items: &[PlanProviderItem]) {
    out.push_str(&format!("## {heading}\n\n"));
    if items.is_empty() {
        out.push_str("- None recorded.\n\n");
        return;
    }
    for item in items {
        if let Some(title) = item
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
        {
            out.push_str(&format!("### {title}\n\n"));
            out.push_str(&format!(
                "{} {}\n\n",
                item.text,
                citation_suffix(&item.citations)
            ));
        } else {
            out.push_str(&format!(
                "- {} {}\n",
                item.text,
                citation_suffix(&item.citations)
            ));
        }
    }
    out.push('\n');
}

fn citation_suffix(citations: &[String]) -> String {
    if citations.is_empty() {
        return "[missing citation]".to_string();
    }
    format!(
        "({})",
        citations
            .iter()
            .map(|citation| format!("`{citation}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn select_plan_doc_provider(
    paths: &DeadreckonPaths,
    plan: &Plan,
    flag: Option<&str>,
) -> Result<DocProviderSelection> {
    let defaults = config_defaults(paths)?;
    let run_provider = plan
        .providers
        .default_child
        .as_deref()
        .or(plan.providers.planner.as_deref())
        .or(plan.providers.coder.as_deref())
        .or(plan.providers.reviewer.as_deref())
        .or_else(|| plan.tasks.iter().find_map(|task| task.provider.as_deref()));
    let setup = doc_provider_setup_selection(paths, &defaults, flag, run_provider, false)?;
    Ok(doc_provider_selection_from_setup(&setup))
}

fn materialize_plan_docs_to_working(
    paths: &DeadreckonPaths,
    plan: &Plan,
    dest: &Path,
    wrapper: Option<&PlanWrapperDocContext>,
) -> Result<()> {
    ensure_plan_docs_deterministic(paths, plan)?;
    let source_dir = plan_docs_dir(paths, &plan.plan_id);
    let internal = dest.join(".deadreckon/docs");
    let public = dest.join("docs");
    fs::create_dir_all(&internal)?;
    fs::create_dir_all(&public)?;
    for name in [
        deadreckon_core::plan::PLAN_NARRATIVE,
        PLAN_AS_BUILT,
        PLAN_DECISIONS,
        PLAN_CHILDREN,
        PLAN_DOCS_MANIFEST,
    ] {
        let source = source_dir.join(name);
        if source.exists() {
            fs::copy(&source, internal.join(name))?;
            if name.starts_with("PLAN-") {
                fs::copy(&source, public.join(name))?;
            }
        }
    }
    if let Some(wrapper) = wrapper {
        write_plan_wrapper_run_docs(dest, plan, wrapper)?;
    }
    Ok(())
}

fn write_plan_wrapper_run_docs(
    dest: &Path,
    plan: &Plan,
    wrapper: &PlanWrapperDocContext,
) -> Result<()> {
    let internal = dest.join(".deadreckon/docs");
    let public = dest.join("docs");
    fs::create_dir_all(&internal)?;
    fs::create_dir_all(&public)?;
    let narrative = format!(
        "# Plan Result Wrapper\n\n**Run ID:** `{}`\n**Plan ID:** `{}`\n**Merged result run:** `{}`\n**Goal:** {}\n\nThis run materializes a completed plan result. It has no provider turns of its own; read the consolidated plan documentation instead.\n\n- [Plan narrative](./PLAN-NARRATIVE.md)\n- [Plan as built](./PLAN-AS-BUILT.md)\n- [Plan decisions](./PLAN-DECISIONS.md)\n- [Plan children](./PLAN-CHILDREN.md)\n",
        wrapper.wrapper_run_id, plan.plan_id, wrapper.merged_run_id, plan.root_goal
    );
    let as_built = format!(
        "# Plan Result Wrapper As Built\n\nThis synthetic apply run wraps plan `{}`. See [PLAN-AS-BUILT.md](./PLAN-AS-BUILT.md) for the consolidated as-built documentation.\n",
        plan.plan_id
    );
    let decisions = "# Plan Result Wrapper Decisions\n\nThis synthetic apply run made no implementation decisions. See [PLAN-DECISIONS.md](./PLAN-DECISIONS.md) for plan-level decisions and merge tradeoffs.\n"
        .to_string();
    for root in [internal, public] {
        fs::write(root.join(deadreckon_core::RUN_NARRATIVE), &narrative)?;
        fs::write(root.join(deadreckon_core::RUN_AS_BUILT), &as_built)?;
        fs::write(root.join(deadreckon_core::RUN_DECISIONS), &decisions)?;
    }
    Ok(())
}

fn commit_plan_apply_docs_update(
    worktree_path: &Path,
    plan: &Plan,
    merged_state: &deadreckon_core::PipelineState,
) -> Result<()> {
    git_status(worktree_path, &["add", "docs"])?;
    git_status(worktree_path, &["add", "-f", ".deadreckon/docs"])?;
    let staged = git_stdout(worktree_path, &["diff", "--cached", "--stat"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    git_status(
        worktree_path,
        &[
            "commit",
            "-m",
            &format!("docs for deadreckon plan {}", run_prefix(&plan.plan_id)),
            "-m",
            &format!(
                "Plan: {}\nResult run: {}\n\nConsolidated plan docs are included under docs/PLAN-* and .deadreckon/docs/PLAN-*.",
                plan.plan_id, merged_state.run_id
            ),
        ],
    )
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

#[cfg(test)]
mod campaign_spawn_tests;

#[cfg(test)]
mod effortless_consistency_tests;

fn task_status_label(status: PlanTaskStatus) -> &'static str {
    plan_task_status_label(status)
}

fn plan_mode_label(mode: PlanMode) -> &'static str {
    match mode {
        PlanMode::FullPlan => "full-plan",
        PlanMode::Review => "review",
    }
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
struct PlanMergeSource {
    task_id: String,
    task_index: u32,
    run_id: String,
    artifact_root: PathBuf,
}

#[derive(Debug, Clone)]
struct PlanMergeOutcome {
    working_dir: PathBuf,
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

fn mergeable_run_files_with_prefix_error(
    root: &Path,
    prefix_error: &str,
) -> Result<Vec<(PathBuf, PathBuf, u64)>> {
    let mut files = Vec::new();
    for file in inventory_files(root)? {
        let relative = file
            .strip_prefix(root)
            .map_err(|err| DeadreckonError::InvalidInput(format!("{prefix_error}: {err}")))?;
        if skip_plan_merge_file(relative) {
            continue;
        }
        let hash = file_hash(&file)?;
        files.push((relative.to_path_buf(), file.clone(), hash));
    }
    Ok(files)
}

#[cfg(test)]
fn mergeable_run_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf, u64)>> {
    mergeable_run_files_with_prefix_error(root, "merge source prefix error")
}

/// A same-path collision between two independent campaign sub-results.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposeConflict {
    path: PathBuf,
    first_label: String,
    second_label: String,
}

#[derive(Debug)]
#[allow(dead_code)] // merge_dir consumed by the campaign command in P9
struct ComposeResult {
    merge_dir: PathBuf,
    conflicts: Vec<ComposeConflict>,
}

struct ComposeFileSource<T> {
    root: PathBuf,
    data: T,
    prefix_error: &'static str,
}

enum ComposeMergeDecision<C> {
    KeepExisting,
    UseCurrent,
    RecordConflict { conflict: C, use_current: bool },
}

fn compose_merge_sources<T, S, C>(
    merge_dir: &Path,
    sources: &[ComposeFileSource<T>],
    mut make_seen: impl FnMut(&T, &Path, &Path, u64) -> S,
    mut decide_conflict: impl FnMut(&Path, &S, &S) -> ComposeMergeDecision<C>,
) -> Result<Vec<C>> {
    remove_if_exists(merge_dir)?;
    fs::create_dir_all(merge_dir)?;
    let mut seen: BTreeMap<PathBuf, (S, u64)> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for source in sources {
        for (relative, file, hash) in
            mergeable_run_files_with_prefix_error(&source.root, source.prefix_error)?
        {
            let current = make_seen(&source.data, &relative, &file, hash);
            let decision = match seen.get(&relative) {
                Some((_, previous_hash)) if *previous_hash == hash => continue,
                Some((previous, _)) => decide_conflict(&relative, previous, &current),
                None => {
                    copy_merge_file(&file, &merge_dir.join(&relative))?;
                    seen.insert(relative, (current, hash));
                    continue;
                }
            };
            match decision {
                ComposeMergeDecision::KeepExisting => {}
                ComposeMergeDecision::UseCurrent => {
                    copy_merge_file(&file, &merge_dir.join(&relative))?;
                    seen.insert(relative, (current, hash));
                }
                ComposeMergeDecision::RecordConflict {
                    conflict,
                    use_current,
                } => {
                    conflicts.push(conflict);
                    if use_current {
                        copy_merge_file(&file, &merge_dir.join(&relative))?;
                        seen.insert(relative, (current, hash));
                    }
                }
            }
        }
    }
    Ok(conflicts)
}

/// Compose several already-promoted result trees into one merge dir. Sub-results
/// are independent (no dependency edges), so this is fail-on-conflict: two roots
/// touching the same relative path with different content yields a conflict. The
/// first writer wins on disk; conflicts are reported so the campaign can fail.
fn compose_roots(roots: &[(String, PathBuf)], merge_dir: &Path) -> Result<ComposeResult> {
    let sources = roots
        .iter()
        .map(|(label, root)| ComposeFileSource {
            root: root.clone(),
            data: label.clone(),
            prefix_error: "merge source prefix error",
        })
        .collect::<Vec<_>>();
    let conflicts = compose_merge_sources(
        merge_dir,
        &sources,
        |label, _relative, _file, _hash| label.clone(),
        |relative, previous, current| ComposeMergeDecision::RecordConflict {
            conflict: ComposeConflict {
                path: relative.to_path_buf(),
                first_label: previous.clone(),
                second_label: current.clone(),
            },
            use_current: false,
        },
    )?;
    Ok(ComposeResult {
        merge_dir: merge_dir.to_path_buf(),
        conflicts,
    })
}

/// Resolve campaign sub-result run ids to their artifact roots and compose them.
#[allow(dead_code)] // wired by the campaign command in P9
fn compose_result_runs(
    paths: &DeadreckonPaths,
    run_ids: &[String],
    merge_dir: &Path,
) -> Result<ComposeResult> {
    let mut roots = Vec::new();
    for run_id in run_ids {
        let state = load_run(paths, run_id)?;
        roots.push((run_id.clone(), child_artifact_root(paths, &state)));
    }
    compose_roots(&roots, merge_dir)
}

fn compose_plan_merge_working(
    paths: &DeadreckonPaths,
    plan: &Plan,
    strategy: PlanMergeStrategy,
) -> Result<PlanMergeOutcome> {
    let merge_working = paths.merge_working(&plan.plan_id);
    let mut sources = Vec::new();
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
        sources.push(ComposeFileSource {
            root: child_root.clone(),
            data: PlanMergeSource {
                task_id: task.task_id.clone(),
                task_index: task.index,
                run_id: run_id.to_string(),
                artifact_root: child_root,
            },
            prefix_error: "merge source prefix error",
        });
    }
    let conflicts = compose_merge_sources(
        &merge_working,
        &sources,
        |source, _relative, file, hash| PlanMergeSeenFile {
            task_id: source.task_id.clone(),
            task_index: source.task_index,
            run_id: source.run_id.clone(),
            artifact_root: source.artifact_root.clone(),
            artifact_path: file.to_path_buf(),
            hash,
        },
        |relative, previous, current| match strategy {
            PlanMergeStrategy::FailOnConflict => ComposeMergeDecision::RecordConflict {
                conflict: plan_merge_conflict(plan, relative, previous, current, None),
                use_current: false,
            },
            PlanMergeStrategy::PreferChild(chosen) => ComposeMergeDecision::RecordConflict {
                conflict: plan_merge_conflict(plan, relative, previous, current, Some(chosen)),
                use_current: chosen == current.task_index,
            },
            PlanMergeStrategy::DagAware
                if commands::plan::plan_task_depends_on(
                    plan,
                    &current.task_id,
                    &previous.task_id,
                ) =>
            {
                ComposeMergeDecision::UseCurrent
            }
            PlanMergeStrategy::DagAware
                if commands::plan::plan_task_depends_on(
                    plan,
                    &previous.task_id,
                    &current.task_id,
                ) =>
            {
                ComposeMergeDecision::KeepExisting
            }
            PlanMergeStrategy::DagAware => ComposeMergeDecision::RecordConflict {
                conflict: plan_merge_conflict(plan, relative, previous, current, None),
                use_current: false,
            },
        },
    )?;
    write_plan_merge_conflicts(paths, plan, strategy, &conflicts)?;
    Ok(PlanMergeOutcome {
        working_dir: merge_working,
        conflicts,
    })
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
    write_plan_merge_conflicts_to(
        &paths.merge_proofs(&plan.plan_id),
        plan,
        strategy.as_str(),
        conflicts,
    )
}

fn write_plan_merge_conflicts_to(
    proof_dir: &Path,
    plan: &Plan,
    strategy: &str,
    conflicts: &[PlanMergeConflict],
) -> Result<()> {
    if conflicts.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(proof_dir)?;
    let path = proof_dir.join("conflicts.json");
    let bundle = PlanMergeConflictBundle {
        schema_version: 2,
        plan_id: &plan.plan_id,
        strategy,
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

#[derive(Debug, Clone)]
struct MergeRepairContext {
    working_dir: PathBuf,
    proof_dir: PathBuf,
    repair_scope: PathBuf,
}

impl MergeRepairContext {
    fn final_merge(paths: &DeadreckonPaths, plan: &Plan) -> Self {
        Self {
            working_dir: paths.merge_working(&plan.plan_id),
            proof_dir: paths.merge_proofs(&plan.plan_id),
            repair_scope: paths.merge_proofs(&plan.plan_id).join("repair-child"),
        }
    }

    fn dependency(paths: &DeadreckonPaths, plan: &Plan, task: &PlanTask) -> Self {
        let launch_dir = paths
            .plan_dir(&plan.plan_id)
            .join("launch")
            .join(&task.task_id);
        Self {
            working_dir: launch_dir.join("source"),
            proof_dir: launch_dir.join("merge-proofs"),
            repair_scope: launch_dir.join("repair-child"),
        }
    }
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
    context: &MergeRepairContext,
    provider: Option<&str>,
    conflicts: &[PlanMergeConflict],
) -> Result<PathBuf> {
    fs::create_dir_all(&context.proof_dir)?;
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
        merge_working: context.working_dir.clone(),
        task_graph,
        worker_specs,
        summary_paths,
        recent_events,
        conflicts,
    };
    let path = context.proof_dir.join("repair-request.json");
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

#[derive(Debug, Clone, Copy)]
struct MergeRepairOptions<'a> {
    provider: &'a str,
    mode: MergeRepairMode,
    attempts: u32,
    quiet: bool,
}

async fn run_merge_repair(
    paths: &DeadreckonPaths,
    plan: &Plan,
    context: &MergeRepairContext,
    options: &MergeRepairOptions<'_>,
    merge: &mut PlanMergeOutcome,
) -> Result<MergeRepairResult> {
    if options.attempts == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "merge repair attempts are disabled",
            "rerun without --repair-attempts 0",
        )));
    }
    let request_path = context.proof_dir.join("repair-request.json");
    let repair_plan = invoke_merge_repair_planner(
        paths,
        plan,
        options.provider,
        options.mode,
        &request_path,
        options.quiet,
    )
    .await?;
    validate_merge_repair_plan(&repair_plan, &merge.unresolved_conflicts(), options.mode)?;
    let repair_plan_path = context.proof_dir.join("repair-plan.json");
    fs::write(&repair_plan_path, serde_json::to_vec_pretty(&repair_plan)?)?;
    match repair_plan.decision.as_str() {
        "prefer_child" => {
            apply_prefer_child_repair(&repair_plan, merge)?;
            Ok(MergeRepairResult {
                strategy: "prefer_child".to_string(),
                repair_run_id: None,
            })
        }
        "synthesize" => {
            apply_synthesized_repair(&repair_plan, merge)?;
            Ok(MergeRepairResult {
                strategy: "synthesize".to_string(),
                repair_run_id: None,
            })
        }
        "spawn_repair_child" => {
            let run_id = execute_merge_repair_child(
                paths,
                plan,
                context,
                options.provider,
                &repair_plan,
                options.quiet,
            )
            .await?;
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
    if let Some(slice) = commands::plan::json_slice(content, '{', '}')
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
        copy_merge_file(&child.artifact_path, &merge.working_dir.join(&action.path))?;
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
    repair_plan: &MergeRepairPlan,
    merge: &mut PlanMergeOutcome,
) -> Result<()> {
    for action in &repair_plan.actions {
        let dest = merge.working_dir.join(&action.path);
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
    context: &MergeRepairContext,
    provider: &str,
    repair_plan: &MergeRepairPlan,
    quiet: bool,
) -> Result<String> {
    let repair_scope = context.repair_scope.clone();
    fs::create_dir_all(&repair_scope)?;
    let repair_goal = format!(
        "{}\n\nRoot goal: {}\nPlan: {}\nRepair request: {}\nRepair plan: {}\n\nResolve only the merge conflict paths named in the repair plan unless a build/test update is strictly required to make the repaired artifact coherent. Preserve completed child behavior and report files changed.",
        repair_plan
            .repair_goal
            .as_deref()
            .unwrap_or("Resolve orchestration merge conflicts."),
        plan.root_goal,
        plan.plan_id,
        context.proof_dir.join("repair-request.json").display(),
        context.proof_dir.join("repair-plan.json").display()
    );
    let merge_working = context.working_dir.clone();
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
    let run_id = commands::chain::parse_started_run_id(&stdout).ok_or_else(|| {
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
    write_merge_repair_run_record(paths, plan, context, &run_id, status)?;
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
    copy_repair_library_to_working(&context.working_dir, &library)?;
    Ok(run_id)
}

fn write_merge_repair_run_record(
    paths: &DeadreckonPaths,
    plan: &Plan,
    context: &MergeRepairContext,
    run_id: &str,
    status: &str,
) -> Result<()> {
    let path = context.proof_dir.join("repair-run.json");
    let state = load_run(paths, run_id).ok();
    let value = json!({
        "schema_version": 1,
        "plan_id": &plan.plan_id,
        "run_id": run_id,
        "scope": state.as_ref().map(|state| state.scope.clone()),
        "status": status,
        "source": &context.working_dir,
        "created_at": Utc::now(),
        "updated_at": Utc::now(),
    });
    fs::write(path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

fn copy_repair_library_to_working(working_dir: &Path, library: &Path) -> Result<()> {
    remove_if_exists(working_dir)?;
    fs::create_dir_all(working_dir)?;
    for file in inventory_files(library)? {
        let relative = file.strip_prefix(library).map_err(|err| {
            DeadreckonError::InvalidInput(format!("repair source prefix error: {err}"))
        })?;
        if skip_plan_merge_file(relative) {
            continue;
        }
        copy_merge_file(&file, &working_dir.join(relative))?;
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
    seed_plan_result_worktree(paths, plan, merged_state, &merged_source, worktree_path)?;

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
    materialize_plan_docs_to_working(
        paths,
        plan,
        &state.working_dir,
        Some(&PlanWrapperDocContext {
            wrapper_run_id: state.run_id.clone(),
            merged_run_id: merged_state.run_id.clone(),
        }),
    )?;
    commit_plan_apply_docs_update(&state.working_dir, plan, merged_state)?;
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
    paths: &DeadreckonPaths,
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
    materialize_plan_docs_to_working(paths, plan, worktree_path, None)?;

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
        "Docs: consolidated plan docs under docs/PLAN-* and .deadreckon/docs/PLAN-*".to_string(),
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
    commands::plan::print_orchestration_role_table(plan, true, None);
    commands::plan::print_orchestration_dependency_summary(plan);
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
    commands::plan::print_orchestration_role_table(plan, true, None);
    commands::plan::print_orchestration_dependency_summary(plan);
    println!("docs {}", plan_docs_status_line(paths, plan));
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
    let proof_block = if outcome_kind == OutcomeKind::Completed {
        completed_proof_block(state, &hints).ok()
    } else {
        None
    };
    let acceptance = acceptance_display(state);
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
        approximate_spend: spend.any_estimated_turn,
        spend_label: spend_summary_label(state, &spend, false),
        wall_seconds: spend.wall_seconds,
        diff: codebase
            .as_ref()
            .and_then(|record| branch_diff_summary(state, record).ok().flatten()),
        gate: acceptance.gate,
        gate_tone: acceptance.gate_tone,
        tests_modified: acceptance.tests_modified,
        gate_caveats: acceptance.caveats,
        working_dir: state.working_dir.clone(),
        proof_path: marker_path_for_run_root(&state.run_root),
        proof_block,
        hints,
    }
}

fn completed_proof_block(
    state: &deadreckon_core::PipelineState,
    hints: &[(String, String)],
) -> Result<ProofBlock> {
    let paths = DeadreckonPaths::discover();
    let next_command = hints
        .iter()
        .find(|(label, _)| matches!(label.as_str(), "apply" | "export" | "undo" | "finish"))
        .or_else(|| hints.first())
        .map(|(_, command)| command.clone())
        .unwrap_or_else(|| format!("deadreckon show {}", run_prefix(&state.run_id)));
    proof_block_for_state(&paths, state, next_command)
}

fn run_spend_label(state: &deadreckon_core::PipelineState, include_metered_cap: bool) -> String {
    let summary = deadreckon_core::state::spend_summary(state).unwrap_or_else(|_| {
        deadreckon_core::state::SpendSummary {
            total_usd: state.total_spend_usd,
            wall_seconds: state.total_wall_seconds,
            ..deadreckon_core::state::SpendSummary::default()
        }
    });
    spend_summary_label(state, &summary, include_metered_cap)
}

fn spend_summary_label(
    state: &deadreckon_core::PipelineState,
    summary: &deadreckon_core::state::SpendSummary,
    include_metered_cap: bool,
) -> String {
    let turns = summary.turns.max(state.turn as usize);
    let wall_seconds = if summary.wall_seconds > 0.0 {
        summary.wall_seconds
    } else {
        state.total_wall_seconds
    };
    let subscription_only = turns > 0
        && summary.subscription_turns == summary.turns
        && summary.total_usd == 0.0
        && (summary.any_subscription_turn || !provider_is_metered(state));
    if subscription_only {
        return format!(
            "not metered (subscription) · wall {:.1}s · {} turns",
            wall_seconds, turns
        );
    }
    let mut label = format!(
        "{}${:.6}",
        if summary.any_estimated_turn { "~" } else { "" },
        summary.total_usd
    );
    if summary.any_subscription_turn {
        label.push_str(" + subscription turns");
    }
    label.push_str(&format!(" · wall {:.1}s · {} turns", wall_seconds, turns));
    if include_metered_cap {
        label.push_str(" / ");
        label.push_str(
            &state
                .max_spend_usd
                .map(|cap| format!("${cap:.6}"))
                .unwrap_or_else(|| "uncapped".to_string()),
        );
    }
    label
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
        let library_dir = paths.library_dir(&state.scope, &state.run_id);
        materialize_plan_docs_to_working(&paths, plan, &library_dir, None)?;
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
        let library_dir = paths.library_dir(&state.scope, &state.run_id);
        materialize_plan_docs_to_working(&paths, plan, &library_dir, None)?;
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

    print_finish_consistency_summary(&state);

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
            let prefix = run_prefix(&state.run_id);
            println!(
                "{} {}",
                ui_ok("finished in-place run"),
                ui_id(&state.run_id)
            );
            println!("  working: {}", state.working_dir.display());
            print_action_block(
                &HintLine {
                    label: "next".to_string(),
                    command: format!("deadreckon show {prefix}"),
                },
                &[
                    HintLine {
                        label: "docs".to_string(),
                        command: format!("deadreckon doc {prefix} --kind decisions"),
                    },
                    HintLine {
                        label: "undo".to_string(),
                        command: format!("deadreckon undo --run {prefix}"),
                    },
                ],
            );
            Ok(())
        }
    }
}

fn print_finish_consistency_summary(state: &deadreckon_core::PipelineState) {
    println!("{}", ui_heading("run summary"));
    println!("  spend: {}", run_spend_label(state, false));
    println!("  gate: {}", acceptance_status_value(state));
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
    commands::acceptance::copy_existing_acceptance_into_run(
        &state,
        &[&state.cwd, &state.working_dir],
    )?;
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
    fire_lifecycle_notification(&paths, &state, &outcome).await;
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
    commands::acceptance::copy_existing_acceptance_into_run(
        &state,
        &[&state.cwd, &state.working_dir],
    )?;
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
    fire_lifecycle_notification(&paths, &state, &outcome).await;
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

fn notify_transition_for_outcome(outcome: &RunLoopOutcome) -> Option<NotifyTransition> {
    match outcome {
        RunLoopOutcome::Done => Some(NotifyTransition::Accepted),
        RunLoopOutcome::PausedAtCap => Some(NotifyTransition::Paused),
        RunLoopOutcome::Failed => Some(NotifyTransition::Failed),
        RunLoopOutcome::Killed => None,
    }
}

async fn fire_lifecycle_notification(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
) {
    let Some(transition) = notify_transition_for_outcome(outcome) else {
        return;
    };
    let Ok(config) = load_notify_config(paths) else {
        return;
    };
    if !config.enabled_for(transition) {
        return;
    }
    let channels = channels_for_config(&config);
    if channels.is_empty() {
        return;
    }
    let context = NotifyContext {
        transition,
        run_id: state.run_id.clone(),
        verdict: notification_verdict(state, outcome),
        spend: run_spend_label(state, false),
        narrative_path: doc_path_for_kind(&state.working_dir, DocKind::Narrative),
    };
    let _attempts = notify_run(state, &config, &context, &channels).await;
}

fn notification_verdict(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
) -> String {
    match outcome {
        RunLoopOutcome::Done => format!("{NOUN_VERIFIED_RUN} ({})", acceptance_status_value(state)),
        RunLoopOutcome::PausedAtCap => "paused at cap".to_string(),
        RunLoopOutcome::Failed => format!("failed run ({})", acceptance_status_value(state)),
        RunLoopOutcome::Killed => "killed run".to_string(),
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

async fn learn_command(command: LearnCommand) -> Result<()> {
    match command {
        LearnCommand::Index {
            scope,
            all,
            since,
            json,
        } => learn_index_command(scope, all, since.as_deref(), json),
        LearnCommand::Report { scope, limit, json } => {
            learn_report_command(scope.as_deref(), limit, json)
        }
        LearnCommand::Export {
            source,
            output,
            redacted,
            json,
        } => learn_export_command(&source, output, redacted, json),
        LearnCommand::ImportBundle {
            path,
            preview,
            yes,
            json,
        } => learn_import_bundle_command(&path, preview, yes, json),
        LearnCommand::Propose {
            scope,
            all,
            from_local,
            bundle,
            limit,
            json,
        } => learn_propose_command(scope, all, from_local, bundle, limit, json).await,
    }
}

async fn improve_command(command: ImproveCommand) -> Result<()> {
    match command {
        ImproveCommand::SelfRun {
            target,
            preview,
            yes,
            pr_dry_run,
            open_pr,
            json,
        } => improve_self_command(target, preview, yes, pr_dry_run, open_pr, json).await,
    }
}

fn learn_index_command(
    scope: Option<String>,
    all: bool,
    since: Option<&str>,
    json_output: bool,
) -> Result<()> {
    if since.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--since is reserved for a later learning index slice",
            "deadreckon learn index --all",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let scope = resolve_learning_scope(scope, all)?;
    let summary = index_learning(&paths, &LearningIndexOptions { scope })?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning indexed"));
    print_kv_block(&[
        ("episodes", &summary.indexed.to_string()),
        ("signals", &summary.signals_written.to_string()),
        ("skipped live", &summary.skipped_live.to_string()),
        ("skipped corrupt", &summary.skipped_corrupt.to_string()),
    ]);
    if summary.signals_written == 0 {
        println!(
            "{} try `{}`",
            ui_muted("next:"),
            ui_command("deadreckon learn report")
        );
    } else {
        println!(
            "{} try `{}`",
            ui_muted("next:"),
            ui_command("deadreckon learn propose")
        );
    }
    Ok(())
}

fn learn_report_command(scope: Option<&str>, limit: usize, json_output: bool) -> Result<()> {
    if limit == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--limit must be at least 1",
            "deadreckon learn report --limit 10",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let mut report = learning_report(&paths, scope)?;
    report.top_signals.truncate(limit);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning report"));
    print_kv_block(&[
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
    ]);
    if report.signals_by_kind.is_empty() {
        println!(
            "{} try `{}`",
            ui_muted("hint:"),
            ui_command("deadreckon learn index --all")
        );
        return Ok(());
    }
    println!();
    println!("{}", ui_heading("signals"));
    for (kind, count) in &report.signals_by_kind {
        println!("  {kind}: {count}");
    }
    if !report.top_signals.is_empty() {
        println!();
        println!("{}", ui_heading("top signals"));
        for signal in &report.top_signals {
            println!(
                "  {} {} {}",
                ui_id(&signal.signal_id),
                ui_status(&signal.kind),
                one_line(&signal.summary, 120)
            );
        }
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command("deadreckon learn propose")
    );
    Ok(())
}

fn learn_export_command(
    source: &str,
    output: Option<PathBuf>,
    _redacted: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let output = output.unwrap_or_else(|| {
        let source_slug = sanitize_slug(source);
        let bundle_id = format!("bundle-{}", source_slug);
        paths.learning_bundle_path(&bundle_id)
    });
    let report = export_learning_bundle(&paths, source, &output)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning bundle exported"));
    print_kv_block(&[
        ("bundle", report.bundle_id.as_str()),
        ("output", &report.output.display().to_string()),
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
        ("redaction", report.redaction.profile.as_str()),
    ]);
    if !report.redaction.findings.is_empty() {
        println!("redacted:");
        for finding in &report.redaction.findings {
            println!("  - {finding}");
        }
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command(format!(
            "deadreckon learn import-bundle {} --preview",
            report.output.display()
        ))
    );
    Ok(())
}

fn learn_import_bundle_command(
    path: &Path,
    preview: bool,
    yes: bool,
    json_output: bool,
) -> Result<()> {
    if preview && yes {
        return Err(CliError::Core(deadreckon_core::user_error(
            "choose either --preview or --yes",
            "deadreckon learn import-bundle <path> --preview",
        )));
    }
    let apply = yes;
    let paths = DeadreckonPaths::discover();
    let report = import_learning_bundle(&paths, path, apply)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{}",
        ui_heading(if apply {
            "learning bundle imported"
        } else {
            "learning bundle preview"
        })
    );
    print_kv_block(&[
        ("bundle", report.bundle_id.as_str()),
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
        ("applied", if report.applied { "yes" } else { "no" }),
    ]);
    if !apply {
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon learn import-bundle {} --yes",
                path.display()
            ))
        );
    }
    Ok(())
}

async fn learn_propose_command(
    scope: Option<String>,
    all: bool,
    from_local: bool,
    bundle: Option<PathBuf>,
    limit: usize,
    json_output: bool,
) -> Result<()> {
    if limit == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--limit must be at least 1",
            "deadreckon learn propose --limit 1",
        )));
    }
    if from_local && (scope.is_some() || all) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "`--from-local` is implicit; do not combine it with scope flags",
            "deadreckon learn propose --scope <scope>",
        )));
    }
    if bundle.is_some() && (scope.is_some() || all || from_local) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "`--bundle` cannot be combined with local evidence flags",
            "deadreckon learn propose --bundle <path>",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let prompt = if let Some(bundle_path) = bundle.as_deref() {
        let bundle = read_learning_bundle(bundle_path)?;
        import_learning_bundle(&paths, bundle_path, true)?;
        build_reflection_prompt_from_bundle(&paths, &bundle, limit)?
    } else {
        let scope = resolve_learning_scope(scope, all)?;
        build_reflection_prompt(&paths, scope.as_deref(), limit)?
    };
    let router = ProviderRouter::from_config_path(&paths.config_path(), None)?;
    let route = router.selected_route_info().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "no provider route resolves for learning reflection",
            "deadreckon config provider",
        ))
    })?;
    let response = router
        .complete(&ProviderRequest {
            prompt,
            max_output_tokens: 8_000,
            cwd: Some(std::env::current_dir()?),
            output_path: Some(paths.learning_dir().join("reflection.out")),
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        })
        .await?;
    let reflection_provider = LearningInsightProvider {
        route: response.provider,
        model: response.model,
    };
    let report = persist_reflection(&paths, &reflection_provider, &response.content, limit)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning proposals"));
    print_kv_block(&[
        ("provider route", route.name.as_str()),
        ("insights", &report.insights_written.to_string()),
        ("proposals", &report.proposals_written.to_string()),
    ]);
    for proposal in &report.proposals {
        println!(
            "  {} {}",
            ui_id(&proposal.proposal_id),
            one_line(&proposal.title, 100)
        );
    }
    if let Some(proposal) = report.proposals.first() {
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon improve self {} --preview",
                proposal.proposal_id
            ))
        );
    }
    Ok(())
}

fn resolve_learning_scope(scope: Option<String>, all: bool) -> Result<Option<String>> {
    if all && scope.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "use either --all or --scope, not both",
            "deadreckon learn index --all",
        )));
    }
    if all {
        return Ok(None);
    }
    scope.map_or_else(|| current_scope().map(Some), |scope| Ok(Some(scope)))
}

async fn improve_self_command(
    target: String,
    preview: bool,
    yes: bool,
    pr_dry_run: bool,
    open_pr: bool,
    json_output: bool,
) -> Result<()> {
    if [preview, yes, pr_dry_run, open_pr]
        .iter()
        .filter(|value| **value)
        .count()
        != 1
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "choose exactly one of --preview, --yes, --pr-dry-run, or --open-pr",
            "deadreckon improve self <proposal-id> --preview",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let proposal = load_self_improve_proposal(&paths, &target)?;
    if preview {
        return improve_self_preview(&proposal, json_output);
    }
    if pr_dry_run || open_pr {
        let candidate = latest_candidate_for_proposal(&paths, &proposal.proposal_id)?;
        let eval = read_candidate_eval(&paths, &candidate.candidate_id)?;
        let policy = load_learning_policy(&paths)?;
        let dry_run = prepare_pr_dry_run(
            &paths,
            &proposal,
            &candidate,
            &eval,
            &policy,
            pr_dry_run || open_pr,
        )?;
        if open_pr {
            let adapter = GhSelfImprovePrAdapter;
            let pr_url = open_self_improve_pr_if_eligible(
                &paths,
                &proposal.proposal_id,
                &candidate,
                &dry_run,
                &adapter,
            )?;
            println!("{}", ui_ok(format!("opened PR {pr_url}")));
            return Ok(());
        }
        if json_output {
            println!("{}", serde_json::to_string_pretty(&dry_run)?);
        } else {
            println!("{}", ui_heading("self-improve PR dry-run"));
            print_kv_block(&[
                ("branch", dry_run.branch.as_str()),
                ("title", dry_run.title.as_str()),
                (
                    "eligible",
                    if dry_run.decision.eligible {
                        "yes"
                    } else {
                        "no"
                    },
                ),
                ("body", &dry_run.body_path.display().to_string()),
            ]);
            if !dry_run.decision.reasons.is_empty() {
                println!("reasons:");
                for reason in &dry_run.decision.reasons {
                    println!("  - {reason}");
                }
            }
        }
        return Ok(());
    }
    run_self_improve_candidate(&paths, &proposal, json_output).await
}

fn improve_self_preview(proposal: &LearningProposal, json_output: bool) -> Result<()> {
    let payload = json!({
        "proposal_id": proposal.proposal_id,
        "title": proposal.title,
        "target": proposal.target,
        "risk": proposal.expected_risk,
        "done_criteria": proposal.done_criteria,
        "mode": "isolated-worktree",
        "provider": "existing resolver",
        "pr": "dry-run by default; live open requires evidence gate"
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("{}", ui_heading("self-improve preview"));
    print_kv_block(&[
        ("proposal", proposal.proposal_id.as_str()),
        ("title", proposal.title.as_str()),
        ("mode", "isolated worktree"),
        ("provider", "existing resolver"),
        ("PR", "dry-run until evidence gate passes"),
    ]);
    println!();
    println!("{}", ui_heading("done criteria"));
    for criterion in &proposal.done_criteria {
        println!("  - {criterion}");
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command(format!(
            "deadreckon improve self {} --yes",
            proposal.proposal_id
        ))
    );
    Ok(())
}

async fn run_self_improve_candidate(
    paths: &DeadreckonPaths,
    proposal: &LearningProposal,
    json_output: bool,
) -> Result<()> {
    let source_root = git_stdout(&std::env::current_dir()?, &["rev-parse", "--show-toplevel"])?;
    let source_root = PathBuf::from(source_root);
    let status = git_stdout(&source_root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "base worktree is dirty",
            "git status --short",
        )));
    }
    let policy = load_learning_policy(paths)?;
    let defaults = config_defaults(paths)?;
    if defaults.sandbox.as_deref() == Some("none") && !policy.self_run.allow_sandbox_none {
        return Err(CliError::Core(deadreckon_core::user_error(
            "self-improve refuses sandbox none",
            "deadreckon config sandbox auto",
        )));
    }
    let base_commit = git_stdout(&source_root, &["rev-parse", "HEAD"])?;
    let candidate_id = format!("cand-{}", Uuid::new_v4().simple());
    let branch = format!("deadreckon/self/{candidate_id}");
    let candidate_dir = paths.learning_candidate_dir(&candidate_id);
    let worktree = candidate_dir.join("worktree");
    fs::create_dir_all(&candidate_dir)?;
    run_git(
        &source_root,
        &[
            "worktree",
            "add",
            "-b",
            branch.as_str(),
            path_to_str(&worktree)?,
            "HEAD",
        ],
    )?;
    let goal_file = candidate_dir.join("goal.md");
    fs::write(&goal_file, &proposal.goal_text)?;
    let acceptance = candidate_dir.join("acceptance.yaml");
    fs::write(
        &acceptance,
        r#"
name: self-improvement-focused
checks:
  - kind: shell
    command: "cargo test -p deadreckon-core learning --lib"
"#,
    )?;

    let before_runs = list_runs(paths, None)?
        .into_iter()
        .map(|run| run.run_id)
        .collect::<BTreeSet<_>>();
    let exe = std::env::current_exe()?;
    let status = std::process::Command::new(exe)
        .current_dir(&worktree)
        .env("DEADRECKON_HOME", paths.home())
        .arg("run")
        .arg(&proposal.goal_text)
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--yes")
        .arg("--acceptance")
        .arg(&acceptance)
        .status()?;
    let run_id = newest_created_run_id(paths, &before_runs)?;
    run_git(&worktree, &["add", "-A"])?;
    let staged = git_stdout(&worktree, &["diff", "--cached", "--name-only"])?;
    if !staged.trim().is_empty() {
        let message = format!("self-improve: {}", one_line(&proposal.title, 64));
        run_git(
            &worktree,
            &[
                "-c",
                "user.name=deadreckon",
                "-c",
                "user.email=deadreckon@example.invalid",
                "commit",
                "-m",
                message.as_str(),
            ],
        )?;
    }
    let head_commit = git_stdout(&worktree, &["rev-parse", "HEAD"])?;
    let changed_files = git_stdout(&worktree, &["diff", "--name-only", "HEAD~1..HEAD"])
        .unwrap_or_default()
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let diff = diff_summary(&worktree, "HEAD~1..HEAD").unwrap_or_default();
    let diff_text = git_stdout(&worktree, &["diff", "HEAD~1..HEAD"]).unwrap_or_default();
    let risk = classify_candidate_risk(&changed_files);
    let mut candidate = LearningCandidate {
        version: 1,
        candidate_id: candidate_id.clone(),
        proposal_id: proposal.proposal_id.clone(),
        branch,
        base_commit,
        head_commit,
        run_id,
        worktree: worktree.clone(),
        diff: LearningCandidateDiff {
            files: changed_files.len() as u32,
            insertions: diff.0,
            deletions: diff.1,
            changed_files,
        },
        risk,
        status: if status.success() {
            "verified"
        } else {
            "rejected"
        }
        .to_string(),
        evidence_packet: "evidence.json".to_string(),
    };
    let verify_status = std::process::Command::new("cargo")
        .current_dir(&worktree)
        .args(["test", "-p", "deadreckon-core", "learning", "--lib"])
        .status()?;
    let mut eval = LearningEval {
        version: 1,
        candidate_id: candidate_id.clone(),
        evaluated_at: Utc::now(),
        accepted_run: status.success(),
        commands: vec![LearningEvalCommand {
            cmd: "cargo test -p deadreckon-core learning --lib".to_string(),
            status: verify_status.code().unwrap_or(1),
        }],
        docs_updated: candidate
            .diff
            .changed_files
            .iter()
            .any(|file| file.starts_with("docs/") || file == "CHANGELOG.md"),
        redaction_passed: !learning_text_has_sensitive(&diff_text, paths),
        evidence_score: 0.0,
        auto_pr: LearningAutoPrStatus {
            eligible: false,
            reasons: Vec::new(),
        },
    };
    eval.evidence_score = evidence_score(proposal, &candidate, &eval);
    let decision = evaluate_auto_pr(proposal, &candidate, &eval, &policy, false);
    eval.auto_pr.eligible = decision.eligible;
    eval.auto_pr.reasons = decision.reasons;
    if eval.evidence_score < policy.pr.min_evidence_score {
        candidate.status = "rejected".to_string();
    }
    write_candidate(paths, &candidate)?;
    write_eval(paths, &eval)?;
    let evidence_path = paths
        .learning_candidate_dir(&candidate_id)
        .join("evidence.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&json!({
            "proposal": proposal,
            "candidate": candidate,
            "eval": eval,
            "rollback": format!("git branch -D {} && git worktree remove {}", candidate.branch, candidate.worktree.display())
        }))?,
    )?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "candidate_id": candidate_id,
                "candidate": candidate,
                "eval": eval,
                "evidence": evidence_path,
            }))?
        );
    } else {
        println!("{}", ui_heading("self-improve candidate"));
        print_kv_block(&[
            ("candidate", candidate_id.as_str()),
            ("run", candidate.run_id.as_str()),
            ("branch", candidate.branch.as_str()),
            ("status", candidate.status.as_str()),
            ("evidence score", &format!("{:.2}", eval.evidence_score)),
        ]);
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon improve self {} --pr-dry-run",
                proposal.proposal_id
            ))
        );
    }
    Ok(())
}

fn load_self_improve_proposal(paths: &DeadreckonPaths, target: &str) -> Result<LearningProposal> {
    let path = PathBuf::from(target);
    if path.exists() {
        let goal_text = fs::read_to_string(&path)?;
        return Ok(LearningProposal {
            version: 1,
            proposal_id: format!("prop-{}", Uuid::new_v4().simple()),
            created_at: Utc::now(),
            title: path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("self-improvement")
                .to_string(),
            insights: Vec::new(),
            stimulus: Vec::<LearningStimulus>::new(),
            hypothesis: "manual goal file".to_string(),
            target: LearningProposalTarget {
                repo: "/Users/gdc/deadreckon".to_string(),
                scope: "manual".to_string(),
            },
            goal_text,
            done_criteria: vec!["focused verification passes".to_string()],
            expected_risk: "medium".to_string(),
            blocked_auto_pr_reasons: Vec::new(),
        });
    }
    read_proposal(paths, target).map_err(CliError::from)
}

fn latest_candidate_for_proposal(
    paths: &DeadreckonPaths,
    proposal_id: &str,
) -> Result<LearningCandidate> {
    let dir = paths.learning_candidates_dir();
    if !dir.exists() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no self-improvement candidate evidence exists",
            &format!("deadreckon improve self {proposal_id} --yes"),
        )));
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path().join("candidate.json");
        if !path.exists() {
            continue;
        }
        let candidate: LearningCandidate = serde_json::from_slice(&fs::read(&path)?)?;
        if candidate.proposal_id == proposal_id {
            let updated = fs::metadata(&path)?.modified().ok();
            candidates.push((updated, candidate));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates
        .into_iter()
        .map(|(_, candidate)| candidate)
        .next()
        .ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "no candidate evidence exists for this proposal",
                &format!("deadreckon improve self {proposal_id} --yes"),
            ))
        })
}

fn read_candidate_eval(paths: &DeadreckonPaths, candidate_id: &str) -> Result<LearningEval> {
    let path = paths.learning_eval_path(candidate_id);
    let data = fs::read(&path)?;
    serde_json::from_slice(&data).map_err(CliError::from)
}

fn newest_created_run_id(paths: &DeadreckonPaths, before: &BTreeSet<String>) -> Result<String> {
    list_runs(paths, None)?
        .into_iter()
        .find(|run| !before.contains(&run.run_id))
        .map(|run| run.run_id)
        .ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "self-run did not create a discoverable run",
                "deadreckon list --all",
            ))
        })
}

fn diff_summary(worktree: &Path, range: &str) -> Result<(u32, u32)> {
    let raw = git_stdout(worktree, &["diff", "--numstat", range])?;
    let mut insertions = 0u32;
    let mut deletions = 0u32;
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let added = parts.next().and_then(|value| value.parse::<u32>().ok());
        let removed = parts.next().and_then(|value| value.parse::<u32>().ok());
        insertions = insertions.saturating_add(added.unwrap_or(0));
        deletions = deletions.saturating_add(removed.unwrap_or(0));
    }
    Ok((insertions, deletions))
}

fn learning_text_has_sensitive(value: &str, paths: &DeadreckonPaths) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("ghp_")
        || lower.contains("github_pat_")
        || lower.contains("sk-")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("begin openssh private key")
        || lower.contains("begin private key")
        || value.contains(paths.home().to_string_lossy().as_ref())
        || std::env::var("HOME").is_ok_and(|home| !home.is_empty() && value.contains(&home))
}

trait SelfImprovePrAdapter {
    fn open_pr(&self, candidate: &LearningCandidate, dry_run: &PrDryRun) -> Result<String>;
}

struct GhSelfImprovePrAdapter;

impl SelfImprovePrAdapter for GhSelfImprovePrAdapter {
    fn open_pr(&self, candidate: &LearningCandidate, dry_run: &PrDryRun) -> Result<String> {
        run_git(
            &candidate.worktree,
            &["push", "-u", "origin", candidate.branch.as_str()],
        )?;
        let output = std::process::Command::new("gh")
            .current_dir(&candidate.worktree)
            .arg("pr")
            .arg("create")
            .arg("--title")
            .arg(&dry_run.title)
            .arg("--body-file")
            .arg(&dry_run.body_path)
            .arg("--head")
            .arg(&candidate.branch)
            .output()?;
        if !output.status.success() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "gh pr create failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn open_self_improve_pr_if_eligible(
    paths: &DeadreckonPaths,
    proposal_id: &str,
    candidate: &LearningCandidate,
    dry_run: &PrDryRun,
    adapter: &dyn SelfImprovePrAdapter,
) -> Result<String> {
    if !dry_run.decision.eligible {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("PR gate failed: {}", dry_run.decision.reasons.join("; ")),
            &format!("deadreckon improve self {proposal_id} --pr-dry-run"),
        )));
    }
    let pr_url = adapter.open_pr(candidate, dry_run)?;
    record_pr_event(
        paths,
        &LearningPrEvent {
            version: 1,
            timestamp: Utc::now(),
            candidate_id: candidate.candidate_id.clone(),
            mode: "open".to_string(),
            status: "opened".to_string(),
            branch: candidate.branch.clone(),
            pr_url: Some(pr_url.clone()),
            body_path: dry_run.body_path.to_string_lossy().to_string(),
            reason: None,
        },
    )?;
    Ok(pr_url)
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
    kind: CliDocKind,
    export: Option<PathBuf>,
    polish: bool,
    no_confirm: bool,
    force: bool,
    doc_skill: Option<String>,
    doc_provider: Option<String>,
    budget_cap: Option<f64>,
}

struct DocPlanCommandArgs {
    target: PlanDocTarget,
    kind: CliDocKind,
    export: Option<PathBuf>,
    polish: bool,
    force: bool,
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
    let loaded_state = load_cli_run(&paths, &run_id);
    if let Some(target) = match loaded_state.as_ref() {
        Ok(state) => resolve_plan_doc_target(&paths, &run_id, Some(state))?,
        Err(_) => resolve_plan_doc_target(&paths, &run_id, None)?,
    } {
        return doc_plan_command(
            &paths,
            DocPlanCommandArgs {
                target,
                kind,
                export,
                polish,
                force,
                doc_provider,
                budget_cap,
            },
        )
        .await;
    }
    let mut state = loaded_state?;
    let kind = run_doc_kind(kind)?;
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

async fn doc_plan_command(paths: &DeadreckonPaths, args: DocPlanCommandArgs) -> Result<()> {
    let DocPlanCommandArgs {
        target,
        kind,
        export,
        polish,
        force,
        doc_provider,
        budget_cap,
    } = args;
    let Some(file_name) = plan_doc_kind_file_name(kind) else {
        return Err(CliError::Core(deadreckon_core::user_error(
            "plan docs do not produce AS-BUILT-DELTA.md",
            "deadreckon doc <plan-id> --kind narrative",
        )));
    };
    if polish {
        let selection = select_plan_doc_provider(paths, &target.plan, doc_provider.as_deref())?;
        let defaults = config_defaults(paths)?;
        refresh_plan_docs(
            paths,
            &target.plan,
            PlanDocRefreshOptions {
                provider: selection.provider,
                provider_source: selection.source.as_str().to_string(),
                budget_cap_usd: budget_cap.or(defaults.doc_polish_budget_cap_usd),
                force: true,
            },
        )
        .await?;
    } else {
        ensure_plan_docs_deterministic(paths, &target.plan)?;
    }
    if let Some(wrapper) = target.wrapper.as_ref()
        && let Ok(state) = load_run(paths, &wrapper.wrapper_run_id)
    {
        let _ = materialize_plan_docs_to_working(
            paths,
            &target.plan,
            &state.working_dir,
            Some(wrapper),
        );
    }
    let path = plan_doc_path(paths, &target.plan.plan_id, file_name);
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
        println!("exported {file_name} to {}", dest.display());
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

#[derive(Debug, Clone, Default)]
struct NarrativeAttachConfig {
    provider: Option<String>,
    max_spend_usd: Option<f64>,
}

const DEFAULT_NARRATIVE_PROVIDER_ROUTE: &str = "cli:claude-code";
const DEFAULT_NARRATIVE_MODEL: &str = "sonnet";

#[derive(Debug, Clone, PartialEq, Eq)]
struct NarrativeProviderSelection {
    route: Option<String>,
    model: Option<String>,
}

fn narrative_provider_selection(explicit_provider: Option<&str>) -> NarrativeProviderSelection {
    match explicit_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        Some("none" | "off" | "deterministic") => NarrativeProviderSelection {
            route: None,
            model: None,
        },
        Some(provider) => NarrativeProviderSelection {
            route: Some(provider.to_string()),
            model: None,
        },
        None => NarrativeProviderSelection {
            route: Some(DEFAULT_NARRATIVE_PROVIDER_ROUTE.to_string()),
            model: Some(DEFAULT_NARRATIVE_MODEL.to_string()),
        },
    }
}

// SAFETY: Kill arguments are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn kill_command(run_id: String, force: bool, plain: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if let Some((campaign_dir, mut campaign)) =
        commands::campaign::resolve_campaign(&paths, &run_id)?
    {
        for sub_plan_id in commands::campaign::campaign_kill_targets(&campaign) {
            // Cascade into each sub-orchestrator's plan (and thus its children).
            let _ = kill_command(sub_plan_id, force, plain);
        }
        campaign.status = deadreckon_core::campaign::CampaignStatus::Killed;
        deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)?;
        deadreckon_core::campaign::append_campaign_event(
            &campaign_dir,
            "campaign_killed",
            serde_json::json!({ "subs": campaign.n }),
        )?;
        if !plain {
            println!(
                "campaign {} killed ({} subs)",
                run_prefix(&campaign.campaign_id),
                campaign.n
            );
        }
        return Ok(());
    }
    let mut state = match load_cli_run(&paths, &run_id) {
        Ok(state) => state,
        Err(run_error) => {
            if let Ok(plan_id) = resolve_plan_id(&paths, &run_id) {
                return kill_plan_command(&paths, &plan_id, force);
            }
            if let Ok(chain_id) = commands::chain::resolve_chain_id(&paths, &run_id, false) {
                return commands::chain::chain_kill_command(&paths, &chain_id, force);
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
    fire_lifecycle_notification(&paths, &state, &outcome).await;
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
            commands::chain::chain_step_status_label(step.status),
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
        id: commands::chain::chain_prefix(&chain.chain_id),
        status: commands::chain::chain_status_label(chain).to_string(),
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
    let mut evidence = acceptance_failure_evidence_lines(state);
    evidence.extend(
        traces
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
            }),
    );
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

fn acceptance_failure_evidence_lines(state: &deadreckon_core::PipelineState) -> Vec<String> {
    let acceptance = acceptance_display(state);
    let mut lines = Vec::new();
    if acceptance.gate.contains("FAILED")
        || acceptance.gate.contains("PASSED")
        || acceptance.tests_modified.is_some()
    {
        lines.push(acceptance.status_line());
    }
    if let Some(tamper) =
        deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)
            .ok()
            .flatten()
    {
        lines.extend(
            tamper
                .refusal_reasons
                .iter()
                .map(|reason| format!("acceptance refused: {reason}")),
        );
    }
    lines
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
    if let Some((campaign_dir, campaign)) = commands::campaign::resolve_campaign(&paths, run_id)? {
        let rollup = deadreckon_core::campaign::read_campaign_rollup(&campaign_dir).ok();
        let report = if why_failed {
            commands::campaign::campaign_why_failed_report(&campaign, rollup.as_ref())
        } else {
            commands::campaign::campaign_attach_summary(Some(&paths), &campaign, rollup.as_ref())
        };
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "campaign_id": campaign.campaign_id,
                    "status": commands::campaign::campaign_status_text(campaign.status),
                    "n": campaign.n,
                    "merged_run_id": campaign.merged_run_id,
                    "rollup": rollup
                        .as_ref()
                        .map(|r| commands::campaign::rollup_verdict_text(r.rollup_verdict)),
                }))
                .unwrap_or_default()
            );
        } else {
            print!("{report}");
        }
        return Ok(());
    }
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
                                "next_actions": commands::plan::plan_next_actions(&plan),
                                "try_lines": Vec::<String>::new(),
                                "paths": commands::plan::plan_paths_json(&plan),
                                "docs": {
                                    "status": plan_docs_status_line(&paths, &plan),
                                    "narrative": plan_doc_path(&paths, &plan.plan_id, deadreckon_core::plan::PLAN_NARRATIVE),
                                    "as_built": plan_doc_path(&paths, &plan.plan_id, PLAN_AS_BUILT),
                                    "decisions": plan_doc_path(&paths, &plan.plan_id, PLAN_DECISIONS),
                                    "children": plan_doc_path(&paths, &plan.plan_id, PLAN_CHILDREN),
                                },
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
    let plan_wrapper_context = plan_wrapper_context_from_run(&paths, &state)?;
    if json_output {
        let status = run_status_label(state.status);
        let plan_result = plan_wrapper_context.as_ref().map(|context| {
            json!({
                "plan_id": &context.plan_id,
                "merged_run_id": &context.merged_run_id,
                "docs": plan_doc_path(&paths, &context.plan_id, deadreckon_core::plan::PLAN_NARRATIVE),
            })
        });
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
                "plan_result": plan_result,
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
    if let Some(context) = plan_wrapper_context.as_ref() {
        let plan = load_plan(&paths, &context.plan_id)?;
        println!(
            "plan result wrapper {} -> plan {} merged {}",
            run_prefix(&state.run_id),
            run_prefix(&context.plan_id),
            run_prefix(&context.merged_run_id)
        );
        println!("docs {}", plan_docs_status_line(&paths, &plan));
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
    let spend = run_spend_label(state, true);
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
    println!("  action:   {next_action}");
    println!("  stale:    {}", if stale { "yes" } else { "no" });
    println!("  pids:     {}", supervised.len());
    if let Some(reason) = state.pause_reason.as_deref() {
        println!("  paused:   {}", one_line(reason, 100));
    }
    if let Some(reason) = state.failure_reason.as_deref() {
        println!("  failure:  {}", one_line(reason, 100));
    }
    println!("  gate: {}", acceptance_status_value(state));
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
    let spend = run_spend_label(state, false);
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
                commands::chain::chain_prefix(&marker.chain_id)
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
        .map(|chain| commands::chain::branch_policy_label(chain.branch_policy))
        .unwrap_or("unknown");
    let apply = chain
        .as_ref()
        .map(|chain| commands::chain::apply_mode_label(chain.apply_mode))
        .unwrap_or("unknown");
    let prior = marker
        .prior_applied_sha
        .as_deref()
        .map(commands::chain::short_sha)
        .unwrap_or_else(|| "none".to_string());
    Ok(Some(format!(
        "chain {} · step {}/{} · policy: {} | apply={} · prev: {}",
        commands::chain::chain_prefix(&marker.chain_id),
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
    let (primary, secondary) = lifecycle_actions(state);
    print_action_block(&primary, &secondary);
}

fn lifecycle_actions(state: &deadreckon_core::PipelineState) -> (HintLine, Vec<HintLine>) {
    let prefix = run_prefix(&state.run_id);
    match state.status {
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            return (
                HintLine {
                    label: "next".to_string(),
                    command: format!("deadreckon attach {prefix}"),
                },
                vec![
                    HintLine {
                        label: "status".to_string(),
                        command: format!("deadreckon status {prefix}"),
                    },
                    HintLine {
                        label: "stop".to_string(),
                        command: format!("deadreckon kill {prefix}"),
                    },
                ],
            );
        }
        RunStatus::Failed | RunStatus::Killed => {
            return (
                HintLine {
                    label: "next".to_string(),
                    command: format!("deadreckon show {prefix} --why-failed"),
                },
                vec![
                    HintLine {
                        label: "resume".to_string(),
                        command: format!("deadreckon resume {prefix}"),
                    },
                    HintLine {
                        label: "state".to_string(),
                        command: state.state_path().display().to_string(),
                    },
                ],
            );
        }
        RunStatus::Completed => {}
    }

    if let Ok(record) = read_codebase_record(&state.working_dir)
        && record.mode == CodebaseMode::Worktree
    {
        let mut secondary = Vec::new();
        if let Some(worktree) = record.worktree_path.as_ref() {
            secondary.push(HintLine {
                label: "inspect".to_string(),
                command: format!("cd {} && git status", worktree.display()),
            });
        }
        secondary.extend([
            HintLine {
                label: "apply".to_string(),
                command: format!("deadreckon apply {prefix}"),
            },
            HintLine {
                label: "cleanup".to_string(),
                command: format!("deadreckon apply {prefix} --autostash --cleanup"),
            },
            HintLine {
                label: "cleanup".to_string(),
                command: format!("deadreckon cleanup {prefix}"),
            },
            HintLine {
                label: "docs".to_string(),
                command: format!("deadreckon doc {prefix} --kind decisions"),
            },
        ]);
        return (
            HintLine {
                label: "next".to_string(),
                command: format!("deadreckon finish {prefix} --autostash --cleanup"),
            },
            secondary,
        );
    }

    if let Ok(record) = read_codebase_record(&state.working_dir)
        && record.mode == CodebaseMode::InPlace
    {
        return (
            HintLine {
                label: "next".to_string(),
                command: format!("deadreckon finish {prefix}"),
            },
            vec![
                HintLine {
                    label: "show".to_string(),
                    command: format!("deadreckon show {prefix}"),
                },
                HintLine {
                    label: "docs".to_string(),
                    command: format!("deadreckon doc {prefix} --kind decisions"),
                },
                HintLine {
                    label: "undo".to_string(),
                    command: format!("deadreckon undo --run {prefix}"),
                },
            ],
        );
    }
    let task_prefix = state.task_key.chars().take(24).collect::<String>();
    (
        HintLine {
            label: "next".to_string(),
            command: format!("deadreckon finish {prefix} --dest ./{task_prefix}"),
        },
        vec![
            HintLine {
                label: "export".to_string(),
                command: format!("deadreckon export {prefix} --dest ./{task_prefix}"),
            },
            HintLine {
                label: "extend".to_string(),
                command: format!("deadreckon extend {prefix} '<your follow-up goal>'"),
            },
            HintLine {
                label: "show".to_string(),
                command: format!("deadreckon show {prefix}"),
            },
            HintLine {
                label: "docs".to_string(),
                command: format!("deadreckon doc {prefix} --kind decisions"),
            },
        ],
    )
}

fn print_action_block(primary: &HintLine, secondary: &[HintLine]) {
    println!("{}", ui_heading("primary action:"));
    print_action_line(primary);
    if !secondary.is_empty() {
        println!("{}", ui_heading("secondary actions:"));
        for action in secondary {
            print_action_line(action);
        }
    }
}

fn print_action_line(action: &HintLine) {
    println!("  {}: {}", action.label, ui_command(&action.command));
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
                    kind: CliDocKind::Narrative,
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

#[derive(Debug, Default, Clone)]
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

#[derive(Debug, Default)]
struct AttachLiveInventory {
    file_count: usize,
    total_bytes: u64,
    files: Vec<LiveFile>,
}

const ATTACH_LIVE_FILE_DISPLAY_LIMIT: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttachJsonlSignature {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
struct AttachJsonlTail<T> {
    path: PathBuf,
    offset: u64,
    rows: Vec<T>,
    signature: Option<AttachJsonlSignature>,
    refresh_count: usize,
    last_read_bytes: usize,
    last_appended_rows: usize,
    partial_bytes: usize,
}

impl<T> AttachJsonlTail<T>
where
    T: DeserializeOwned,
{
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            rows: Vec::new(),
            signature: None,
            refresh_count: 0,
            last_read_bytes: 0,
            last_appended_rows: 0,
            partial_bytes: 0,
        }
    }

    fn reset_to_path(&mut self, path: PathBuf) {
        if self.path != path {
            self.path = path;
            self.offset = 0;
            self.rows.clear();
            self.signature = None;
            self.refresh_count = 0;
            self.last_read_bytes = 0;
            self.last_appended_rows = 0;
            self.partial_bytes = 0;
        }
    }

    fn rows(&self) -> &[T] {
        &self.rows
    }

    fn refresh(&mut self) -> Result<&[T]> {
        self.last_read_bytes = 0;
        self.last_appended_rows = 0;
        let Ok(metadata) = fs::metadata(&self.path) else {
            self.offset = 0;
            self.rows.clear();
            self.signature = None;
            self.partial_bytes = 0;
            return Ok(&self.rows);
        };
        let modified = metadata.modified().ok();
        let len = metadata.len();
        let current_signature = AttachJsonlSignature { len, modified };
        if self.signature == Some(current_signature) {
            return Ok(&self.rows);
        }
        if len < self.offset
            || (len == self.offset
                && self
                    .signature
                    .is_some_and(|previous| previous.modified != modified))
        {
            self.offset = 0;
            self.rows.clear();
            self.partial_bytes = 0;
        }
        if len == self.offset {
            self.signature = Some(current_signature);
            self.partial_bytes = 0;
            return Ok(&self.rows);
        }

        let mut file = fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.refresh_count = self.refresh_count.saturating_add(1);
        self.last_read_bytes = bytes.len();
        let Some(complete_len) = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
        else {
            self.partial_bytes = bytes.len();
            return Ok(&self.rows);
        };
        let complete = String::from_utf8_lossy(&bytes[..complete_len]);
        let before_len = self.rows.len();
        for line in complete.lines().filter(|line| !line.trim().is_empty()) {
            if let Ok(row) = serde_json::from_str(line) {
                self.rows.push(row);
            }
        }
        self.last_appended_rows = self.rows.len().saturating_sub(before_len);
        self.partial_bytes = bytes.len().saturating_sub(complete_len);
        self.offset = self.offset.saturating_add(complete_len as u64);
        self.signature = Some(AttachJsonlSignature {
            len: self.offset,
            modified,
        });
        Ok(&self.rows)
    }
}

#[derive(Debug)]
struct AttachProviderActivityCache {
    flight_tail: AttachJsonlTail<FlightEvent>,
    provider_log_cache: AttachProviderLogScanCache,
}

impl AttachProviderActivityCache {
    fn new(state: &deadreckon_core::PipelineState) -> Self {
        Self {
            flight_tail: AttachJsonlTail::new(state.run_root.join(FLIGHT_EVENTS_JSONL)),
            provider_log_cache: AttachProviderLogScanCache::default(),
        }
    }

    fn refresh(&mut self, state: &deadreckon_core::PipelineState) -> ProviderActivity {
        self.flight_tail
            .reset_to_path(state.run_root.join(FLIGHT_EVENTS_JSONL));
        let flight = self
            .flight_tail
            .refresh()
            .map(collect_flight_provider_activity_from_events)
            .unwrap_or_default();
        let Some(spec) = provider_jsonl_log_spec(state) else {
            return flight;
        };
        let fallback =
            self.provider_log_cache
                .refresh(state, &spec, !flight.lines.is_empty(), Instant::now());
        combine_provider_activity(flight, fallback)
    }
}

#[derive(Debug, Default)]
struct AttachNarrativeProjectionCache {
    signature: Option<u64>,
    projection: Option<narrative::NarrativeProjection>,
    refresh_count: usize,
}

impl AttachNarrativeProjectionCache {
    fn refresh_run(
        &mut self,
        input: &RunNarrativeRenderInput<'_>,
    ) -> Result<narrative::NarrativeProjection> {
        let signature = run_narrative_projection_signature(input);
        if self.signature == Some(signature)
            && let Some(projection) = self.projection.as_ref()
        {
            return Ok(projection.clone());
        }
        let projection = ensure_run_narrative_projection(input)?;
        self.signature = Some(signature);
        self.projection = Some(projection.clone());
        self.refresh_count = self.refresh_count.saturating_add(1);
        Ok(projection)
    }

    fn refresh_plan(
        &mut self,
        input: &narrative::PlanNarrativeInput<'_>,
    ) -> Result<narrative::NarrativeProjection> {
        let signature = plan_narrative_projection_signature(input);
        if self.signature == Some(signature)
            && let Some(projection) = self.projection.as_ref()
        {
            return Ok(projection.clone());
        }
        let projection = narrative::ensure_plan_projection(input)?;
        self.signature = Some(signature);
        self.projection = Some(projection.clone());
        self.refresh_count = self.refresh_count.saturating_add(1);
        Ok(projection)
    }

    fn invalidate(&mut self) {
        self.signature = None;
    }
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

#[derive(Debug, Clone)]
struct LiveFile {
    path: String,
    bytes: u64,
    modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct LivePid {
    pid: u32,
    alive: bool,
    command: String,
}

fn collect_attach_live(state: &deadreckon_core::PipelineState) -> AttachLive {
    let provider_activity = collect_provider_activity(state);
    collect_attach_live_with_provider_activity(state, provider_activity)
}

fn collect_attach_live_with_provider_activity(
    state: &deadreckon_core::PipelineState,
    provider_activity: ProviderActivity,
) -> AttachLive {
    let inventory = attach_live_inventory(&state.working_dir);
    let pids = supervised_pids(state)
        .into_iter()
        .map(live_pid)
        .collect::<Vec<_>>();
    let acceptance = collect_acceptance_live(state);
    AttachLive {
        file_count: inventory.file_count,
        total_bytes: inventory.total_bytes,
        files: inventory.files,
        pids,
        provider_context_tokens: provider_activity.context_tokens,
        provider_context_window: provider_activity.context_window,
        provider_activity: provider_activity.lines,
        acceptance,
        working_dir_exists: state.working_dir.exists(),
    }
}

fn attach_live_inventory(root: &Path) -> AttachLiveInventory {
    let mut inventory = AttachLiveInventory::default();
    if !root.exists() {
        return inventory;
    }
    collect_attach_inventory_dir(root, root, &mut inventory);
    inventory.files.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then(left.path.cmp(&right.path))
    });
    inventory.files.truncate(ATTACH_LIVE_FILE_DISPLAY_LIMIT);
    inventory
}

fn collect_attach_inventory_dir(root: &Path, dir: &Path, inventory: &mut AttachLiveInventory) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if attach_live_inventory_should_prune_dir(root, &path) {
                continue;
            }
            collect_attach_inventory_dir(root, &path, inventory);
        } else if file_type.is_file()
            && let Some(file) = live_file(root, &path)
        {
            inventory.file_count = inventory.file_count.saturating_add(1);
            inventory.total_bytes = inventory.total_bytes.saturating_add(file.bytes);
            inventory.files.push(file);
        }
    }
}

fn attach_live_inventory_should_prune_dir(root: &Path, path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    if matches!(
        name.as_ref(),
        ".git"
            | ".deadreckon"
            | ".cache"
            | ".next"
            | ".turbo"
            | "build"
            | "coverage"
            | "dist"
            | "node_modules"
            | "playwright-report"
            | "target"
            | "test-results"
    ) {
        return true;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .parent()
        .is_some_and(|parent| path_has_component(parent, ".tmp"))
        && name.starts_with("chrome-profile")
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
        && let Ok(count) = commands::acceptance::acceptance_check_count(&raw)
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

#[derive(Debug, Default, Clone)]
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

#[derive(Debug, Default)]
struct ProviderJsonlActivityScan {
    activity: ProviderActivity,
    matched_path: Option<PathBuf>,
    matched_modified: Option<SystemTime>,
}

#[derive(Debug, Default)]
struct AttachProviderLogScanCache {
    last_scan_at: Option<Instant>,
    matched_path: Option<PathBuf>,
    matched_modified: Option<SystemTime>,
    activity: ProviderActivity,
    root_scan_count: usize,
}

const PROVIDER_LOG_SCAN_FRESHNESS: Duration = Duration::from_secs(5);
const PROVIDER_LOG_SCAN_DELAY_WITH_FLIGHT: Duration = Duration::from_secs(30);

impl AttachProviderLogScanCache {
    fn refresh(
        &mut self,
        state: &deadreckon_core::PipelineState,
        spec: &ProviderJsonlLogSpec,
        flight_has_lines: bool,
        now: Instant,
    ) -> ProviderActivity {
        if flight_has_lines && self.last_scan_at.is_none() && self.activity.lines.is_empty() {
            self.last_scan_at = Some(now);
            return self.activity.clone();
        }
        let window = if flight_has_lines {
            PROVIDER_LOG_SCAN_DELAY_WITH_FLIGHT
        } else {
            PROVIDER_LOG_SCAN_FRESHNESS
        };
        let changed = self.matched_log_changed();
        let due = self
            .last_scan_at
            .is_none_or(|last_scan| now.duration_since(last_scan) >= window);
        if !changed && !due {
            return self.activity.clone();
        }

        self.root_scan_count = self.root_scan_count.saturating_add(spec.roots.len().max(1));
        let scan = collect_jsonl_provider_activity_scan(state, spec);
        self.last_scan_at = Some(now);
        self.matched_path = scan.matched_path;
        self.matched_modified = scan.matched_modified;
        self.activity = scan.activity;
        self.activity.clone()
    }

    fn matched_log_changed(&self) -> bool {
        let Some(path) = self.matched_path.as_ref() else {
            return false;
        };
        fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            != self.matched_modified
    }
}

fn collect_provider_activity(state: &deadreckon_core::PipelineState) -> ProviderActivity {
    collect_provider_activity_from_flight(state, collect_flight_provider_activity(state))
}

fn collect_provider_activity_from_flight(
    state: &deadreckon_core::PipelineState,
    flight: ProviderActivity,
) -> ProviderActivity {
    let Some(spec) = provider_jsonl_log_spec(state) else {
        return flight;
    };
    let fallback = collect_jsonl_provider_activity(state, &spec);
    combine_provider_activity(flight, fallback)
}

fn combine_provider_activity(
    mut flight: ProviderActivity,
    fallback: ProviderActivity,
) -> ProviderActivity {
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
    collect_flight_provider_activity_from_events(&events)
}

fn collect_flight_provider_activity_from_events(events: &[FlightEvent]) -> ProviderActivity {
    let mut activity = ProviderActivity::default();
    for event in events {
        if let Some(usage) = event.usage.as_ref() {
            activity.context_tokens = Some(usage.input_tokens + usage.output_tokens);
            activity.context_window = usage.context_window;
        }
        activity.lines.push(flight_activity_line(event));
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
    collect_jsonl_provider_activity_scan(state, spec).activity
}

fn collect_jsonl_provider_activity_scan(
    state: &deadreckon_core::PipelineState,
    spec: &ProviderJsonlLogSpec,
) -> ProviderJsonlActivityScan {
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
        return ProviderJsonlActivityScan {
            activity: cap_provider_activity(activity, 240),
            matched_modified: fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
            matched_path: Some(path),
        };
    }
    ProviderJsonlActivityScan::default()
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

fn plan_narrative_projection_signature(input: &narrative::PlanNarrativeInput<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    input.plan.plan_id.hash(&mut hasher);
    format!("{:?}", input.plan.status).hash(&mut hasher);
    input.plan.tasks.len().hash(&mut hasher);
    for task in &input.plan.tasks {
        task.task_id.hash(&mut hasher);
        format!("{:?}", task.status).hash(&mut hasher);
        task.child_run_id.hash(&mut hasher);
        task.summary_path.hash(&mut hasher);
        task.depends_on.hash(&mut hasher);
    }
    input.messages.len().hash(&mut hasher);
    input
        .messages
        .last()
        .map(|message| (&message.from, &message.to, &message.summary))
        .hash(&mut hasher);
    input.plan_events.len().hash(&mut hasher);
    input
        .plan_events
        .last()
        .map(|event| format!("{:?}", event.event))
        .hash(&mut hasher);
    input.feed_events.len().hash(&mut hasher);
    input
        .feed_events
        .last()
        .map(|event| format!("{event:?}"))
        .hash(&mut hasher);
    input.selected.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NarrativeRefreshKind {
    Manual,
    Event(&'static str),
    QuietThreshold,
}

impl NarrativeRefreshKind {
    fn is_manual(self) -> bool {
        matches!(self, Self::Manual)
    }

    fn is_meaningful_delta(self) -> bool {
        matches!(self, Self::Event(_))
    }

    fn resets_quiet_timer(self) -> bool {
        matches!(self, Self::Event(_))
    }

    fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual refresh",
            Self::Event(reason) => reason,
            Self::QuietThreshold => "quiet threshold",
        }
    }
}

#[derive(Debug, Clone)]
struct NarrativeQuietRefreshTracker {
    last_meaningful_at: DateTime<Utc>,
    last_quiet_attempt_at: Option<DateTime<Utc>>,
}

impl NarrativeQuietRefreshTracker {
    fn new(now: DateTime<Utc>) -> Self {
        Self {
            last_meaningful_at: now,
            last_quiet_attempt_at: None,
        }
    }

    fn observe_event_trigger(&mut self, trigger: Option<NarrativeRefreshKind>, now: DateTime<Utc>) {
        if trigger.is_some_and(NarrativeRefreshKind::resets_quiet_timer) {
            self.last_meaningful_at = now;
            self.last_quiet_attempt_at = None;
        }
    }

    fn maybe_trigger(
        &mut self,
        is_running: bool,
        quiet_seconds: u64,
        now: DateTime<Utc>,
    ) -> Option<NarrativeRefreshKind> {
        if !is_running || quiet_seconds == 0 {
            return None;
        }
        if elapsed_seconds(self.last_meaningful_at, now) < quiet_seconds {
            return None;
        }
        if self
            .last_quiet_attempt_at
            .is_some_and(|last| elapsed_seconds(last, now) < quiet_seconds)
        {
            return None;
        }
        self.last_quiet_attempt_at = Some(now);
        Some(NarrativeRefreshKind::QuietThreshold)
    }
}

fn elapsed_seconds(start: DateTime<Utc>, now: DateTime<Utc>) -> u64 {
    now.signed_duration_since(start).num_seconds().max(0) as u64
}

#[derive(Debug, Default, Clone)]
struct NarrativeAcceptanceRefreshTracker {
    latest: Option<AcceptanceRefreshSignature>,
}

impl NarrativeAcceptanceRefreshTracker {
    fn observe(&mut self, acceptance: &AcceptanceLive) -> Option<NarrativeRefreshKind> {
        let next = AcceptanceRefreshSignature::from(acceptance);
        let previous = self.latest.replace(next);
        if previous.is_none() || previous == Some(next) {
            return None;
        }
        match acceptance.status {
            AcceptanceUiStatus::Running => Some(NarrativeRefreshKind::Event("acceptance running")),
            AcceptanceUiStatus::Passed => Some(NarrativeRefreshKind::Event("acceptance passed")),
            AcceptanceUiStatus::Failed => Some(NarrativeRefreshKind::Event("acceptance failed")),
            AcceptanceUiStatus::DefaultGate | AcceptanceUiStatus::Configured => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AcceptanceRefreshSignature {
    status: AcceptanceUiStatus,
    total: usize,
    completed: usize,
    passed: usize,
    failed: usize,
    required_failed: usize,
}

impl From<&AcceptanceLive> for AcceptanceRefreshSignature {
    fn from(acceptance: &AcceptanceLive) -> Self {
        Self {
            status: acceptance.status,
            total: acceptance.total,
            completed: acceptance.completed,
            passed: acceptance.passed,
            failed: acceptance.failed,
            required_failed: acceptance.required_failed,
        }
    }
}

fn run_narrative_refresh_trigger(events: &[RunEvent]) -> Option<NarrativeRefreshKind> {
    events.iter().find_map(|event| match &event.event {
        RunEventKind::Error { .. } => Some(NarrativeRefreshKind::Event("run error")),
        RunEventKind::RunCompleted { .. } => Some(NarrativeRefreshKind::Event("run completed")),
        RunEventKind::DocsCheckpoint { .. } => Some(NarrativeRefreshKind::Event("docs checkpoint")),
        RunEventKind::ToolCallResult { status, .. }
            if status != "ok" && status != "success" && status != "completed" =>
        {
            Some(NarrativeRefreshKind::Event("tool result"))
        }
        RunEventKind::ToolCallStarted { .. } => Some(NarrativeRefreshKind::Event("tool started")),
        _ => None,
    })
}

fn plan_narrative_refresh_trigger(events: &[PlanFeedEvent]) -> Option<NarrativeRefreshKind> {
    events.iter().find_map(|event| match event {
        PlanFeedEvent::Plan { event } => match &event.event {
            PlanEventKind::TaskCompleted { .. } => {
                Some(NarrativeRefreshKind::Event("plan child completed"))
            }
            PlanEventKind::TaskBlocked { .. } => {
                Some(NarrativeRefreshKind::Event("plan child blocked"))
            }
            PlanEventKind::TaskFailed { .. } => {
                Some(NarrativeRefreshKind::Event("plan child failed"))
            }
            PlanEventKind::TaskKilled { .. } => {
                Some(NarrativeRefreshKind::Event("plan child killed"))
            }
            PlanEventKind::TaskRunDiscovered { .. } => {
                Some(NarrativeRefreshKind::Event("child run discovered"))
            }
            PlanEventKind::MergeRepairStarted { .. } => {
                Some(NarrativeRefreshKind::Event("merge repair started"))
            }
            PlanEventKind::MergeRepaired { .. } => {
                Some(NarrativeRefreshKind::Event("merge repaired"))
            }
            PlanEventKind::MergeRepairFailed { .. } => {
                Some(NarrativeRefreshKind::Event("merge repair failed"))
            }
            PlanEventKind::PlanCompleted | PlanEventKind::PlanFailed { .. } => {
                Some(NarrativeRefreshKind::Event("plan terminal"))
            }
            _ => None,
        },
        PlanFeedEvent::ChildRun { event, .. } | PlanFeedEvent::RepairRun { event, .. } => {
            run_narrative_refresh_trigger(std::slice::from_ref(event))
        }
        PlanFeedEvent::Warning { .. } => Some(NarrativeRefreshKind::Event("plan feed warning")),
        PlanFeedEvent::Snapshot { .. } => None,
    })
}

async fn refresh_run_narrative_with_provider_for_kind_with_token(
    paths: &DeadreckonPaths,
    input: &RunNarrativeRenderInput<'_>,
    config: &NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
    cancellation_token: Option<CancellationToken>,
) -> Result<String> {
    let state = input.state;
    let spend = input.spend;
    let traces = input.traces;
    let events = input.events;
    let live = input.live;
    let tui_state = input.tui_state;
    let projection = run_narrative_projection(state, spend, traces, events, live, tui_state)?;
    let selection = narrative_provider_selection(config.provider.as_deref());
    let refreshed =
        refresh_narrative_projection_with_provider(NarrativeProjectionProviderRefresh {
            paths,
            projection,
            route: selection.route.clone(),
            model: selection.model.clone(),
            config,
            kind,
            cwd: Some(state.working_dir.clone()),
            output_path: state.run_root.join("narrative/provider-refresh.out"),
            cancellation_token,
        })
        .await?;
    narrative::persist_run_projection(state, &refreshed)?;
    Ok(narrative_refresh_notice(&refreshed))
}

struct PlanNarrativeProviderRefresh<'a> {
    paths: &'a DeadreckonPaths,
    plan: &'a Plan,
    messages: &'a [PlanMessage],
    plan_events: &'a [PlanEvent],
    feed_events: &'a [PlanFeedEvent],
    selected: usize,
    config: &'a NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
    cancellation_token: Option<CancellationToken>,
}

async fn refresh_plan_narrative_with_provider_for_kind(
    request: PlanNarrativeProviderRefresh<'_>,
) -> Result<String> {
    let PlanNarrativeProviderRefresh {
        paths,
        plan,
        messages,
        plan_events,
        feed_events,
        selected,
        config,
        kind,
        cancellation_token,
    } = request;
    let projection = narrative::ensure_plan_projection(&narrative::PlanNarrativeInput {
        paths,
        plan,
        messages,
        plan_events,
        feed_events,
        selected,
    })?;
    let selection = narrative_provider_selection(config.provider.as_deref());
    let refreshed =
        refresh_narrative_projection_with_provider(NarrativeProjectionProviderRefresh {
            paths,
            projection,
            route: selection.route.clone(),
            model: selection.model.clone(),
            config,
            kind,
            cwd: std::env::current_dir().ok(),
            output_path: paths
                .plan_dir(&plan.plan_id)
                .join("narrative/provider-refresh.out"),
            cancellation_token,
        })
        .await?;
    narrative::persist_plan_projection(paths, plan, &refreshed)?;
    Ok(narrative_refresh_notice(&refreshed))
}

struct NarrativeProjectionProviderRefresh<'a> {
    paths: &'a DeadreckonPaths,
    projection: narrative::NarrativeProjection,
    route: Option<String>,
    model: Option<String>,
    config: &'a NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
    cwd: Option<PathBuf>,
    output_path: PathBuf,
    cancellation_token: Option<CancellationToken>,
}

async fn refresh_narrative_projection_with_provider(
    request: NarrativeProjectionProviderRefresh<'_>,
) -> Result<narrative::NarrativeProjection> {
    let NarrativeProjectionProviderRefresh {
        paths,
        projection,
        route,
        model,
        config,
        kind,
        cwd,
        output_path,
        cancellation_token,
    } = request;
    let policy = narrative::NarrativeRefreshPolicy {
        provider_route: route.clone(),
        max_spend_usd: config.max_spend_usd,
        manual: kind.is_manual(),
        meaningful_delta: kind.is_meaningful_delta(),
        now: Utc::now(),
    };
    match narrative::provider_refresh_decision(&projection.state, &policy) {
        narrative::NarrativeRefreshDecision::Eligible => {}
        narrative::NarrativeRefreshDecision::NoProvider => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                route,
                "no narrative provider configured; deterministic fallback is current",
            ));
        }
        narrative::NarrativeRefreshDecision::OverBudget => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                route,
                "narrative provider refresh skipped because the spend cap is exhausted",
            ));
        }
        narrative::NarrativeRefreshDecision::TooSoon => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                route,
                "narrative provider refresh skipped by cadence",
            ));
        }
        narrative::NarrativeRefreshDecision::CallLimitReached => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                route,
                "narrative provider refresh skipped because the attach call limit is reached",
            ));
        }
    }
    let Some(route) = route else {
        return Ok(narrative::projection_with_provider_failure(
            &projection,
            None,
            "no narrative provider configured; deterministic fallback is current",
        ));
    };
    let prompt = narrative::build_provider_prompt(&projection)?;
    let router = match ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        Some(&route),
        model.as_deref(),
    ) {
        Ok(router) => router,
        Err(err) => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                Some(route),
                format!("provider route unavailable: {err}"),
            ));
        }
    };
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let response = match router
        .complete(&ProviderRequest {
            prompt: prompt.prompt,
            max_output_tokens: 2_000,
            cwd,
            output_path: Some(output_path),
            sandbox_backend: None,
            pid_file: None,
            cancellation_token,
        })
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return Ok(narrative::projection_with_provider_failure(
                &projection,
                Some(route),
                format!("provider refresh failed: {err}"),
            ));
        }
    };
    match narrative::apply_provider_response(
        &projection,
        &response.content,
        narrative::NarrativeProviderRefresh {
            route: response.provider,
            model: response.model,
            cost_usd: response.spend.cost_usd,
            subscription_seconds: response.spend.wall_time_seconds,
        },
    ) {
        Ok(refreshed) => Ok(refreshed),
        Err(err) => Ok(narrative::projection_with_provider_failure(
            &projection,
            Some(route),
            format!("provider output rejected: {err}"),
        )),
    }
}

fn narrative_refresh_notice(projection: &narrative::NarrativeProjection) -> String {
    match &projection.state.latest_status {
        narrative::NarrativeStatus::Fresh => format!(
            "provider narrative refreshed via {}",
            projection
                .state
                .provider
                .route
                .as_deref()
                .unwrap_or("provider")
        ),
        _ => projection
            .state
            .last_error
            .as_ref()
            .map(|error| format!("provider refresh skipped; {}", one_line(error, 120)))
            .unwrap_or_else(|| {
                "provider refresh skipped; deterministic narrative is current".to_string()
            }),
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
mod flight_cli_tests;

#[cfg(test)]
mod self_improve_pr_tests;

#[cfg(test)]
mod tui_tests;
