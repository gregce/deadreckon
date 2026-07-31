#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::needless_pass_by_value,
        clippy::redundant_clone
    )
)]

use std::cell::RefCell;
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
use deadreckon::verdict_surface::{ExplanationPanel, VerdictKind, VerdictSurface};
use deadreckon_core::flight::{
    ArtifactFileFingerprint, ArtifactFileKind, CheckpointManifest, FLIGHT_EVENTS_JSONL,
    FLIGHT_MANIFEST_JSON, FlightSessionStatus, RewindEvent, RewindMode, RewindStatus, RewindTarget,
    RewindTargetKind, append_rewind_event, build_deliverable_file_index, build_working_file_index,
    list_checkpoint_manifests, materialize_checkpoint, read_flight_events, read_flight_manifest,
};
use deadreckon_core::glossary::{NOUN_DONE_CONTRACT, NOUN_VERIFIED_RUN};
use deadreckon_core::install_receipt::{Channel, detect_receipt, read_receipt, write_receipt};
use deadreckon_core::learning::{
    LearningAutoPrStatus, LearningCandidate, LearningCandidateDiff, LearningEval,
    LearningEvalCommand, LearningIndexOptions, LearningInsightProvider, LearningPrEvent,
    LearningProposal, LearningProposalTarget, LearningStimulus, build_reflection_prompt,
    build_reflection_prompt_from_bundle, classify_candidate_risk, evaluate_auto_pr, evidence_score,
    export_learning_bundle, import_learning_bundle, index_learning, learning_report,
    load_learning_policy, persist_reflection, prepare_pr_dry_run, read_learning_bundle,
    read_proposal, record_pr_event, write_candidate, write_eval,
};
use deadreckon_core::paths::{sanitize_slug, task_key, workspace_scope};
use deadreckon_core::state::load_state;
use deadreckon_core::update_cache::{read_cache, write_cache};
use deadreckon_core::{
    AcceptanceMarker, AcceptanceProgressEntry, ApplyMode, ApplyStrategy, BranchPolicy, Chain,
    ChainEvent, ChainEventKind, ChainNewOptions, ChainStatus, ChainStepMarker, ChainStepStatus,
    CodebaseMode, CodebaseRecord, ConductorState, CoordinatorChild, CoordinatorState,
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DeadreckonError, DeadreckonPaths,
    DocKind, DocProviderSelection, DocProviderSource, DocsStatus, ModeFlags, OnFail, PhaseId,
    PhaseStatus, Plan, PlanChildMarker, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind,
    PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, PromotionManifest,
    ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunListEntry, RunOptions, RunStatus,
    WorktreeOptions, acceptance_progress_path_for_run_root, acceptance_spec_path_for_run_root,
    acquire_lock, append_chain_event, append_parent_narrative_update, append_plan_event,
    append_plan_message, append_provenance, append_trace, apply_commit_body, cancel_marker_present,
    chain_status_label as glossary_chain_status_label,
    chain_step_status_label as glossary_chain_step_status_label, clear_cancel_marker,
    copy_artifact_path, copy_deliverable_tree, copy_source_to_working, copy_tree, create_run,
    create_worktree, doc_path_for_kind, docs_status_for_state, emit_event,
    evaluate_acceptance_checks, inventory_files, list_runs, load_chain, load_plan, load_run,
    marker_path_for_run_root, pid_is_alive, plan_status_label, plan_task_status_label,
    prepare_worktree_record, preview_git_state, promote_completed_run, read_chain_step_marker,
    read_codebase_record, read_plan_messages, record_for_resolved_mode, release_lock_file,
    remove_artifact_path, resolve_mode, restore_snapshot, run_status_label, save_chain, save_plan,
    save_state, terminate_pid, validate_acceptance_marker, validate_task_count,
    write_acceptance_marker, write_cancel_marker, write_chain_step_marker, write_child_summary,
    write_coordinator_state, write_plan_child_marker, write_worker_spec,
};
use deadreckon_protocol::{
    FlightEvent, FlightEventKind, RunEvent, RunEventKind, SpendRecord, TraceRecord,
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
    PolishConfig, RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, SeamKind, SeamRunCtx,
    SeamsConfig, polish_run_docs, read_seams_config, resolve_catalog_override, run_turn_loop,
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
mod narrator;
mod plan_event_bus;
mod product;
mod prompt;
mod tui;
mod tui_events;
mod ui;

// Shakedown moved id resolution into `commands::reference`. Re-exporting at the
// crate root keeps it reachable through the `use super::super::*` that every
// command module already relies on, so no call site had to change with the move.
pub(crate) use commands::reference::{PlanChildSelection, resolve_plan_id};

use crate::cli::{
    AcceptanceCommand, AcceptancePreset, CHAIN_HELP, CampaignCommand, ChainCommandArgs, Cli,
    CliDocKind, CliPlanMode, CliSeamKind, Commands, CompletionCommand, ConfigCommand,
    ExtendCommandArgs, ForkCommandArgs, HistoryCommand, HistoryKind, ImproveCommand, LearnCommand,
    LibraryCommand, MergeCommandArgs, OrchestrateCommand, PlanCommandArgs, ProvidersCommand,
    RunCommandArgs, SeamsCommand, StartCommandArgs, SupervisorCommand,
};
use crate::narrative::{AttachViewMode, NarrativeVisualMode};
use crate::plan_event_bus::{PlanEventBus, PlanFeedEvent};
use crate::tui::{
    AttachCampaignParent, AttachPanel, AttachParentPlan, AttachTuiState, PlanAttachRenderState,
    RunNarrativeRenderInput, attach_activity_lines_for_tui, attach_panel_layout,
    ensure_run_narrative_projection, live_file_lines, plan_event_line, plan_event_summary,
    plan_final_gate_line, plan_provider_summary, plan_repair_label, plan_task_detail_lines,
    process_lines, provider_is_metered, render_attach, render_campaign_attach, render_plan_attach,
    run_narrative_projection, run_narrative_projection_signature,
};
#[cfg(test)]
use crate::tui::{
    acceptance_activity_lines, attach_header_text, deadreckoning_status_text, meter_color,
    plan_attach_footer, threshold_color,
};
use crate::ui::{ui_command, ui_error, ui_heading, ui_id, ui_muted, ui_ok, ui_status, ui_warn};

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
    RetryableInterruption { message: String },
    #[error("{message}")]
    Exit {
        code: i32,
        message: String,
        hint: String,
    },
    #[error("{surface}")]
    Surface { code: i32, surface: String },
}

type Result<T> = std::result::Result<T, CliError>;

impl CliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Exit { code, .. } => *code,
            Self::Surface { code, .. } => *code,
            Self::Core(_)
            | Self::Provider(_)
            | Self::Sandbox(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::TomlDe(_)
            | Self::TomlSer(_)
            | Self::RetryableInterruption { .. } => 1,
        }
    }
}

fn error_hint(err: &CliError) -> String {
    let hint = match err {
        CliError::Exit { hint, .. } => hint.clone(),
        CliError::Surface { .. } => String::new(),
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
        CliError::Core(DeadreckonError::InvalidInput(message))
            if message.contains("[seams.gate]") =>
        {
            "remove [seams.gate]; the acceptance gate is the trust root".to_string()
        }
        CliError::Core(DeadreckonError::InvalidInput(message))
            if message.contains("unknown seam kind") =>
        {
            "use policy / catalog / hooks / event_sink; see `deadreckon doctor`".to_string()
        }
        CliError::Core(DeadreckonError::InvalidInput(message)) if message.contains("[seams.") => {
            "check the named [seams] command and timeout in config.toml; see `deadreckon doctor`"
                .to_string()
        }
        CliError::Core(DeadreckonError::NotFound(_)) => {
            "run `deadreckon list` to find valid run ids or config keys".to_string()
        }
        CliError::Core(DeadreckonError::LockHeld { run_id, .. }) => {
            format!(
                "`deadreckon attach {run_id}` to watch it, or `deadreckon kill {run_id} --force` if it is wedged"
            )
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
        CliError::RetryableInterruption { .. } => {
            "deadreckon status latest; the durable Job will retry within its approved policy"
                .to_string()
        }
        // Errors that already carry a specific `try:` line (user_error packs
        // one into the message) must not get the generic doctor hint stacked
        // on top — it reads as a second, usually wrong, recommendation.
        CliError::Core(_) | CliError::Provider(_) => {
            if err.to_string().contains("\ntry: ") {
                String::new()
            } else {
                "deadreckon doctor".to_string()
            }
        }
    };
    if hint.trim().is_empty() {
        String::new()
    } else {
        try_footer(hint)
    }
}

fn try_footer(hint: impl AsRef<str>) -> String {
    let hint = hint.as_ref().trim();
    if hint.starts_with("try:") {
        hint.to_string()
    } else {
        format!("try: {hint}")
    }
}

fn seam_fail_policy_label(kind: SeamKind) -> &'static str {
    match kind.fail_policy() {
        deadreckon_runtime::FailPolicy::Closed => "closed",
        deadreckon_runtime::FailPolicy::Open => "open",
        deadreckon_runtime::FailPolicy::Safe => "safe",
    }
}

fn seam_preview_label(seams: &SeamsConfig) -> String {
    if seams.no_seams {
        return "builtin (--no-seams)".to_string();
    }
    let external = SeamKind::all()
        .into_iter()
        .filter(|kind| seams.command_for(*kind).is_some())
        .map(SeamKind::config_key)
        .collect::<Vec<_>>();
    if external.is_empty() {
        "builtin".to_string()
    } else {
        format!("external: {}", external.join(", "))
    }
}

fn print_kv_block<K, V>(items: &[(K, V)])
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let _ = write_kv_block(items);
}

fn write_kv_block<K, V>(items: &[(K, V)]) -> io::Result<()>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    print!("{}", kv_block_string("", items));
    io::stdout().flush()
}

/// The shared `key: value` block primitive. The key column is sized to the widest
/// key by display width (ANSI-aware), `indent` prefixes every line, and values
/// wrap to the terminal. Use this instead of hand-aligned `println!("  k:  v")`
/// blocks so every kv surface aligns the same way.
fn kv_block_string<K, V>(indent: &str, items: &[(K, V)]) -> String
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let key_width = items
        .iter()
        .map(|(key, _)| ui::display_width(key.as_ref()))
        .max()
        .unwrap_or(0);
    let prefix_width = ui::display_width(indent) + key_width + 2;
    let value_width = kv_value_width(prefix_width);
    let mut out = String::new();
    for (key, value) in items {
        let mut first = true;
        for logical_line in value
            .as_ref()
            .lines()
            .chain(value.as_ref().is_empty().then_some(""))
        {
            for line in wrap_kv_value(logical_line, value_width) {
                if first {
                    first = false;
                    out.push_str(indent);
                    // Dim the key (label) so it reads as subordinate to its value;
                    // ui::render is gated on a TTY, so plain output is unchanged.
                    out.push_str(&ui::render(
                        ui::Stream::Stdout,
                        ui::Tone::Muted,
                        ui::pad_visible(key.as_ref(), key_width),
                    ));
                    out.push_str(": ");
                    out.push_str(&line);
                } else {
                    out.push_str(&" ".repeat(prefix_width));
                    out.push_str(&line);
                }
                out.push('\n');
            }
        }
    }
    out
}

fn kv_value_width(prefix_width: usize) -> usize {
    let terminal_width = crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|columns| columns.parse::<usize>().ok())
        })
        .filter(|columns| *columns > 1)
        .unwrap_or(120)
        .saturating_sub(1);
    terminal_width.saturating_sub(prefix_width).max(20)
}

fn wrap_kv_value(value: &str, width: usize) -> Vec<String> {
    ui::wrap_words(value, width)
}

fn print_error(err: &CliError) {
    if let CliError::Surface { surface, .. } = err {
        eprint!("{surface}");
        if !surface.ends_with('\n') {
            eprintln!();
        }
        return;
    }
    eprintln!("{} {err}", ui_error("error:"));
}

fn print_error_hint(err: &CliError) {
    let hint = error_hint(err);
    if hint.trim().is_empty() {
        return;
    }
    let _ = ui::hint(ui::Stream::Stderr, hint);
}

fn goal_input_error(
    command_label: &str,
    message: impl Into<String>,
    primary_command: impl Into<String>,
) -> CliError {
    let message = message.into();
    let primary_command = primary_command.into();
    CliError::Surface {
        code: 1,
        surface: goal_input_surface(command_label, &message, &primary_command)
            .render_plain(!completion_hints_enabled(false)),
    }
}

fn goal_input_surface(command_label: &str, message: &str, primary_command: &str) -> VerdictSurface {
    let why = if message.contains("could not read") {
        "DeadReckon cannot launch from a goal file it could not read; choose a readable goal source before starting."
    } else if message.contains("either a positional goal or --goal-file") {
        "DeadReckon needs exactly one goal source, so it refused to guess between inline text and a file."
    } else if message.contains("goal is empty") {
        "DeadReckon needs non-empty goal text before it can create run, plan, or campaign state."
    } else {
        "DeadReckon cannot launch until it has one non-empty goal source."
    };
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        command_label,
        None,
        ExplanationPanel::new(
            message.to_string(),
            why.to_string(),
            [
                ("command".to_string(), command_label.to_string()),
                ("reason".to_string(), message.to_string()),
            ],
        ),
        [("Recommended", primary_command.trim().to_string())],
        std::iter::empty::<(&str, String)>(),
    )
}

fn resolve_required_goal_input(
    command_label: &str,
    positional: Option<String>,
    goal_file: Option<PathBuf>,
    missing_hint: &str,
) -> Result<String> {
    resolve_optional_goal_input(command_label, positional, goal_file)?.ok_or_else(|| {
        goal_input_error(
            command_label,
            format!("{command_label} goal required"),
            missing_hint.to_string(),
        )
    })
}

fn resolve_optional_goal_input(
    command_label: &str,
    positional: Option<String>,
    goal_file: Option<PathBuf>,
) -> Result<Option<String>> {
    match (positional, goal_file) {
        (Some(_), Some(_)) => Err(goal_input_error(
            command_label,
            format!("{command_label} accepts either a positional goal or --goal-file, not both"),
            format!("deadreckon {command_label} --goal-file docs/goal.md"),
        )),
        (Some(goal), None) => {
            if let Some(path) = goal.strip_prefix('@') {
                if path.is_empty() {
                    return Err(goal_input_error(
                        command_label,
                        format!("{command_label} @file goal is missing a path"),
                        format!("deadreckon {command_label} @docs/goal.md"),
                    ));
                }
                return read_goal_file(command_label, Path::new(path)).map(Some);
            }
            let goal = normalize_goal_text(&goal);
            validate_goal_text(command_label, &goal)?;
            Ok(Some(goal))
        }
        (None, Some(path)) => read_goal_file(command_label, &path).map(Some),
        (None, None) => Ok(None),
    }
}

fn read_goal_file(command_label: &str, path: &Path) -> Result<String> {
    let resolved_path = resolve_goal_file_path(path)?;
    let contents = fs::read_to_string(&resolved_path).map_err(|err| {
        goal_input_error(
            command_label,
            format!(
                "could not read {command_label} goal file {}; resolved to {} ({err})",
                path.display(),
                resolved_path.display()
            ),
            format!("deadreckon {command_label} --goal-file docs/goal.md"),
        )
    })?;
    let goal = normalize_goal_text(&contents);
    validate_goal_text(command_label, &goal)?;
    Ok(goal)
}

fn resolve_goal_file_path(path: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = expand_goal_file_home(path, home.as_deref());
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir()?;
    let project_root = deadreckon_core::find_git_root(&cwd)?;
    Ok(resolve_goal_file_path_from(
        &path,
        &cwd,
        project_root.as_deref(),
    ))
}

fn resolve_goal_file_path_from(path: &Path, cwd: &Path, project_root: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let cwd_candidate = cwd.join(path);
    if cwd_candidate.exists() {
        return cwd_candidate;
    }
    if let Some(project_root) = project_root {
        let project_candidate = project_root.join(path);
        if project_candidate.exists() {
            return project_candidate;
        }
    }
    cwd_candidate
}

fn expand_goal_file_home(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(home) = home else {
        return path.to_path_buf();
    };
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn normalize_goal_text(goal: &str) -> String {
    goal.strip_prefix('\u{feff}')
        .unwrap_or(goal)
        .trim_matches(|ch| ch == '\r' || ch == '\n')
        .to_string()
}

fn validate_goal_text(command_label: &str, goal: &str) -> Result<()> {
    if goal.trim().is_empty() {
        return Err(goal_input_error(
            command_label,
            format!("{command_label} goal is empty"),
            format!("deadreckon {command_label} \"goal\""),
        ));
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = main_inner().await {
        // A prompt the user aborted (Ctrl-C / Esc / EOF) is not a failure: exit
        // calmly with the interrupt convention (130), no "error: I/O error"
        // line and no path hint.
        if is_prompt_cancelled(&err) {
            eprintln!(
                "{}",
                ui::render(ui::Stream::Stderr, ui::Tone::Muted, "cancelled")
            );
            std::process::exit(130);
        }
        let exit_code = err.exit_code();
        print_error(&err);
        print_error_hint(&err);
        std::process::exit(exit_code);
    }
}

/// True when the error is an aborted interactive prompt. The prompt engine
/// surfaces Ctrl-C / Esc / EOF as an interrupted I/O error; without this it
/// would be rendered as a generic I/O error with a "referenced path" hint.
fn is_prompt_cancelled(err: &CliError) -> bool {
    matches!(err, CliError::Io(source) if source.kind() == std::io::ErrorKind::Interrupted)
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
    commands::graph_job::authorize_delegated_invocation_if_present()?;
    let Some(command) = cli.command else {
        return smart_bare_invocation().await;
    };
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
        Commands::Completion { command } => commands::completion::completion_command(command),
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
            goal_file,
            mode,
            plan,
            max_spend,
            provider,
            model,
            children,
            planner_provider,
            child_provider,
            coder_provider,
            reviewer_provider,
            preview,
            review_done,
            yes,
            no_seams,
            fresh,
            worktree,
            from,
            allow_dirty,
            plain,
            quiet,
            json,
        } => {
            ui::set_plain_output(plain || json);
            let resolved_goal = resolve_optional_goal_input("start", goal, goal_file)?;
            let allows_prompts =
                std::io::stdin().is_terminal() && !yes && !json && !plain && !quiet;
            // A --plan replay carries its goal in the plan file.
            let goal = if plan.is_some() {
                resolved_goal.unwrap_or_default()
            } else {
                commands::start::resolve_start_goal(resolved_goal.as_deref(), allows_prompts)?
            };
            commands::start::start_command(StartCommandArgs {
                goal,
                mode,
                plan,
                max_spend,
                provider,
                model,
                children,
                planner_provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                preview,
                review_done,
                yes,
                no_seams,
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
        Commands::Supervisor { command } => match command {
            SupervisorCommand::Serve { once, job_id } => {
                commands::supervisor::supervisor_serve_command(once, job_id).await
            }
            SupervisorCommand::Drive { job_id } => {
                commands::graph_job::drive_job_command(job_id).await
            }
            SupervisorCommand::Resume { job_id } => trusted_supervisor_resume_command(job_id).await,
            SupervisorCommand::Launch {
                job_id,
                attempt,
                launch_id,
                release_token_sha256,
            } => commands::supervisor::supervisor_launch_command(
                &job_id,
                attempt,
                launch_id,
                &release_token_sha256,
            ),
            SupervisorCommand::Install => {
                commands::supervisor_service::supervisor_service_install_command()
            }
            SupervisorCommand::Start => {
                commands::supervisor_service::supervisor_service_start_command()
            }
            SupervisorCommand::Status { json } => {
                commands::supervisor_service::supervisor_service_status_command(json)
            }
            SupervisorCommand::Stop => {
                commands::supervisor_service::supervisor_service_stop_command()
            }
        },
        Commands::Run {
            goal,
            run_id,
            tamper_baseline,
            goal_file,
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
            no_seams,
            plain,
            prevent_sleep,
            quiet,
            max_spend,
            max_wall_seconds,
            sandbox,
            untrusted,
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
            narrate,
            no_narrate,
            narrator_model,
            infer_contract,
        } => {
            ui::set_plain_output(plain);
            let goal = resolve_required_goal_input(
                "run",
                goal,
                goal_file,
                "deadreckon run --goal-file docs/goal.md --yes",
            )?;
            commands::run::run_command(RunCommandArgs {
                goal,
                run_id,
                durable_source_cwd: None,
                continuation: None,
                tamper_baseline,
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
                no_seams,
                plain,
                prevent_sleep,
                quiet,
                max_spend,
                max_wall_seconds,
                sandbox,
                untrusted,
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
                narrate,
                no_narrate,
                narrator_model,
                infer_contract,
            })
            .await
        }
        Commands::Orchestrate {
            command,
            goal,
            goal_file,
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
            narrate,
            no_narrate,
            narrator_model,
        } => {
            ui::set_plain_output(plain);
            if let Err(message) = crate::narrator::validate_narration_flags(narrate, no_narrate) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
            }
            let request = commands::orchestrate::orchestrate_request_from_cli(
                command,
                commands::orchestrate::BareOrchestrateArgs {
                    goal,
                    goal_file,
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
                    narrate,
                    no_narrate,
                    narrator_model,
                },
            )?;
            ui::set_plain_output(request.plan.plain);
            commands::orchestrate::orchestrate_command(request).await
        }
        Commands::Campaign {
            command,
            goal,
            goal_file,
            n,
            planner_provider,
            provider,
            planner_model,
            model,
            max_spend,
            max_wall_seconds,
            sandbox,
            acceptance,
            preview,
            yes,
            no_hints,
            quiet,
            plain,
            narrate,
            no_narrate,
            narrator_model,
        } => {
            ui::set_plain_output(plain);
            if let Err(message) = crate::narrator::validate_narration_flags(narrate, no_narrate) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(message)));
            }
            if let Some(command) = command {
                match command {
                    CampaignCommand::Repair(repair) => {
                        ui::set_plain_output(repair.plain);
                        return commands::campaign::campaign_repair_command(
                            commands::campaign::CampaignRepairArgs {
                                campaign_id: repair.campaign_id,
                                repair_provider: repair.repair_provider,
                                repair_mode: repair.repair_mode,
                                repair_attempts: repair.repair_attempts,
                                no_hints: repair.no_hints,
                                quiet: repair.quiet,
                            },
                        )
                        .await;
                    }
                }
            }
            let goal = resolve_required_goal_input(
                "campaign",
                goal,
                goal_file,
                "deadreckon campaign --goal-file docs/goal.md --yes",
            )?;
            let args = commands::campaign::CampaignArgs {
                goal,
                n,
                planner_provider,
                provider,
                planner_model,
                model,
                max_spend,
                max_wall_seconds,
                sandbox,
                acceptance,
                preview,
                yes,
                no_hints,
                quiet,
                plain,
                narrate,
                no_narrate,
                narrator_model,
            };
            if args.preview {
                commands::campaign::campaign_command(args).await
            } else {
                commands::campaign::schedule_campaign_job(args)
            }
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
                apply: deadreckon_core::plan::ApplyWhen::AtEnd,
                max_spend: None,
                max_wall_seconds: None,
                sandbox: None,
                planner_provider,
                provider,
                child_provider,
                coder_provider,
                reviewer_provider,
                planner_model: None,
                model: None,
                child_model: Vec::new(),
                coder_model: None,
                reviewer_model: None,
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
            yes,
            no_hints,
            quiet,
            plain,
        } => {
            ui::set_plain_output(plain);
            commands::plan::fork_command_from_cli(ForkCommandArgs {
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
                yes,
                no_hints,
                quiet,
                plain,
                completion_surface: true,
                narrate: false,
                narrator_model: None,
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
                completion_surface: true,
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
            acceptance,
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
                acceptance,
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
        Commands::Doctor { json } => commands::doctor::doctor_command(json).await,
        Commands::Seams { command } => commands::seams::seams_command(command).await,
        Commands::Detect { id, json, ping } => {
            commands::providers::detect_command(id, json, ping).await
        }
        Commands::Providers { command } => commands::providers::providers_command(command).await,
        Commands::Models {
            provider,
            all,
            json,
        } => commands::providers::models_command(provider.as_deref(), all, json),
        Commands::Update {
            check,
            force,
            allow_prerelease,
            yes,
            quiet,
            plain,
        } => {
            commands::providers::update_command(check, force, allow_prerelease, yes, quiet, plain)
                .await
        }
        Commands::List {
            scope,
            all,
            full,
            plain,
            json,
        } => {
            ui::set_plain_output(plain);
            commands::inspection::list_command(scope, all, full, plain, json)
        }
        Commands::Library { command } => commands::inspection::library_command(command),
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
        } => commands::lifecycle::finish_command(
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
        } => {
            let paths = DeadreckonPaths::discover();
            let dest = resolve_external_dest(&paths, dest, "export")?;
            commands::lifecycle::materialize_command(run_id, dest, force, include_manifest)
        }
        Commands::Apply {
            run_id,
            strategy,
            branch,
            no_confirm,
            autostash,
            cleanup,
            message,
            plain,
        } => commands::lifecycle::apply_command(
            run_id, strategy, branch, no_confirm, autostash, cleanup, message, plain,
        ),
        Commands::Abandon {
            run_id,
            keep_branch,
            force,
        } => commands::lifecycle::abandon_command(run_id, keep_branch, force),
        Commands::Cleanup {
            run_id,
            all,
            completed,
            stale,
            no_confirm,
            force,
            overwrite,
            keep_branch,
        } => commands::lifecycle::cleanup_command(commands::lifecycle::CleanupCommandRequest {
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
            acceptance,
            yes,
            max_context_turns,
            no_context,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            no_docs,
            doc_skill,
            narrate,
            no_narrate,
            narrator_model,
        } => {
            let paths = DeadreckonPaths::discover();
            let dest = resolve_external_dest(&paths, dest, "extend")?;
            commands::lifecycle::extend_command(ExtendCommandArgs {
                parent_run_id,
                new_goal,
                dest,
                acceptance,
                yes,
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
                narrate,
                no_narrate,
                narrator_model,
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
            commands::doc::doc_command(commands::doc::DocCommandArgs {
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
            why,
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
                why,
                view,
                visual,
                narrative_provider,
                narrative_max_spend,
            })
            .await
        }
        Commands::Steer { run_id, text } => commands::steer::steer_command(&run_id, text),
        Commands::Kill {
            run_id,
            force,
            plain,
        } => {
            ui::set_plain_output(plain);
            kill_command(run_id, force, plain)
        }
        Commands::Reshape {
            run_id,
            yes,
            json,
            plain,
            quiet,
        } => {
            ui::set_plain_output(plain || json);
            commands::course::reshape_command(commands::course::ReshapeArgs {
                run_id,
                yes,
                json,
                plain,
                quiet,
            })
            .await
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
        // The positional id wins; --run stays for anyone who scripted it.
        Commands::Undo { id, run, turn } => undo_command(id.or(run).as_deref(), turn),
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
        Commands::Verdict {
            run_id,
            all,
            limit,
            json,
            plain,
            quiet,
        } => {
            ui::set_plain_output(plain);
            commands::verdict::verdict_command(commands::verdict::VerdictArgs {
                run_id,
                all,
                limit,
                json,
                plain,
                quiet,
            })
            .await
        }
        Commands::Report {
            run_id,
            html,
            dest,
            open,
            json,
            plain,
        } => {
            ui::set_plain_output(plain);
            commands::report::report_command(commands::report::ReportCommandArgs {
                run_id,
                html,
                dest,
                open,
                json,
                plain,
            })
        }
        Commands::Show {
            run_id,
            turn,
            diff,
            raw,
            why_failed,
            plain,
            json,
            flight,
            file,
        } => {
            ui::set_plain_output(plain);
            show_command(ShowCommandArgs {
                run_id: &run_id,
                turn,
                diff,
                raw: raw.as_deref(),
                why_failed,
                json_output: json,
                flight,
                file: file.as_deref(),
            })
        }
        Commands::History { command } => commands::inspection::history_command(command),
        Commands::Status {
            run_id,
            all,
            plain,
            json,
        } => {
            ui::set_plain_output(plain);
            status_command(run_id.as_deref(), all, plain, json)
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
        } => commands::import::import_command(commands::import::ImportCommandOptions {
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
        Commands::Learn { command } => commands::learning::learn_command(command).await,
        Commands::Improve { command } => commands::learning::improve_command(command).await,
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
        let Ok(receipt) = commands::providers::update_receipt_for_current_binary(&paths, false)
        else {
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
                let current = commands::providers::update_current_version(&receipt);
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(3),
                        commands::providers::resolve_latest_update(&paths, &current, false),
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
        && !matches!(
            command,
            Commands::Update { .. } | Commands::Doctor { .. } | Commands::Supervisor { .. }
        )
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
        display: "models",
        clap_name: Some("models"),
        purpose: "list selectable models per provider route",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::SetupProviders),
    },
    CommandHelpEntry {
        display: "seams",
        clap_name: Some("seams"),
        purpose: "validate configured seam workers",
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
        display: "campaign",
        clap_name: Some("campaign"),
        purpose: "parallel multi-orchestrator power tool",
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
        purpose: "watch a durable Job or legacy work",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "status",
        clap_name: Some("status"),
        purpose: "see the latest Job or legacy work item",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::StartWatchKeep),
        all_group: Some(HelpAllGroup::ProductionFlow),
    },
    CommandHelpEntry {
        display: "list",
        clap_name: Some("list"),
        purpose: "find Jobs and legacy work",
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
        display: "reshape",
        clap_name: Some("reshape"),
        purpose: "accept a run's reshape proposal as a plan",
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
        display: "supervisor",
        clap_name: Some("supervisor"),
        purpose: "manage the durable local Job supervisor",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ContinueRecover),
    },
    CommandHelpEntry {
        display: "steer",
        clap_name: Some("steer"),
        purpose: "guide an executing Codex app-server run",
        audience: CommandAudience::Primary,
        top_group: Some(TopHelpGroup::Control),
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
        display: "rewind",
        clap_name: Some("rewind"),
        purpose: "preview or apply a provider checkpoint rewind",
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
        display: "verdict",
        clap_name: Some("verdict"),
        purpose: "re-verify current result evidence",
        audience: CommandAudience::Advanced,
        top_group: None,
        all_group: Some(HelpAllGroup::ResultsInspect),
    },
    CommandHelpEntry {
        display: "report",
        clap_name: Some("report"),
        purpose: "write a static Job or run evidence report",
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

#[cfg(test)]
mod command_help_catalog_tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_visible_top_level_command_is_discoverable_in_the_catalog() {
        // The production command tree is large enough to overflow the test
        // harness's default thread stack while clap derives every nested
        // subcommand. Build it on an explicitly bounded larger stack.
        let visible = std::thread::Builder::new()
            .name("help-catalog-clap-tree".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                Cli::command()
                    .get_subcommands()
                    .filter(|command| !command.is_hide_set())
                    .map(|command| command.get_name().to_string())
                    .collect::<BTreeSet<_>>()
            })
            .expect("spawn clap command-tree builder")
            .join()
            .expect("build clap command tree");
        let catalog = COMMAND_HELP_CATALOG
            .iter()
            .filter_map(|entry| entry.clap_name)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let missing = visible.difference(&catalog).cloned().collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "visible top-level commands missing from help-all catalog: {missing:?}"
        );
    }

    #[test]
    fn every_advanced_catalog_command_has_a_help_all_section() {
        let missing = COMMAND_HELP_CATALOG
            .iter()
            .filter(|entry| {
                command_discovery(entry) == CommandDiscovery::Advanced && entry.all_group.is_none()
            })
            .map(|entry| entry.display)
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "advanced commands missing a help-all section: {missing:?}"
        );
    }
}

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

/// Bare `deadreckon` reads the room instead of assuming: a machine with no
/// setup gets a welcome and the guided path, a configured machine in a
/// directory with no runs gets oriented help, and a directory with runs gets
/// its status — the command a returning operator actually wants.
async fn smart_bare_invocation() -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if !paths.config_path().exists() {
        return first_run_welcome().await;
    }
    let scope_has_runs = current_scope()
        .ok()
        .and_then(|scope| deadreckon_core::state::list_runs(&paths, Some(&scope)).ok())
        .is_some_and(|runs| !runs.is_empty());
    if scope_has_runs {
        return status_command(None, false, false, false);
    }
    directory_orientation();
    Ok(())
}

const KNOWN_AGENT_CLIS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("copilot", "GitHub Copilot CLI"),
    ("opencode", "OpenCode"),
    ("pi", "Pi"),
];

/// First contact on this machine: greet, show what was detected, and point at
/// (or directly offer) the guided setup. Never errors — a welcome that fails
/// is worse than no welcome.
async fn first_run_welcome() -> Result<()> {
    ui::print_banner(env!("CARGO_PKG_VERSION"));
    println!(
        "{}",
        ui_heading("Welcome — this machine has no deadreckon setup yet.")
    );
    println!("{}", product::PRODUCT_AUDIENCE);
    println!("{}", product::PRODUCT_HARNESS);
    println!();
    println!("{}", ui_heading("Detected agent CLIs:"));
    let mut found_any = false;
    for (binary, label) in KNOWN_AGENT_CLIS {
        if command_exists(binary) {
            found_any = true;
            println!("  {} {label} ({binary})", ui_ok("✓"));
        }
    }
    if !found_any {
        println!(
            "  {}",
            ui_muted("none found — BYO API key works too (anthropic / openai routes)")
        );
    }
    println!();
    println!("{}", ui_heading("Get going:"));
    println!(
        "  {}   {}",
        ui_command("deadreckon init"),
        ui_muted("guided setup (about a minute)")
    );
    println!(
        "  {}    {}",
        ui_command("deadreckon try"),
        ui_muted("keyless proof run of the whole harness")
    );
    println!(
        "  {}   {}",
        ui_command("deadreckon help"),
        ui_muted("the command map")
    );
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        println!();
        if prompt::confirm("run guided setup now?", true)? {
            return commands::init::init_command(
                None,
                None,
                None,
                10.0,
                "auto".to_string(),
                false,
                false,
            )
            .await;
        }
        println!("{}", ui_muted("whenever you're ready: deadreckon init"));
    }
    Ok(())
}

/// Configured machine, but this directory has no runs: orient instead of
/// erroring. Explains what start would do *here* and how to find runs made
/// elsewhere.
fn directory_orientation() {
    ui::print_banner(env!("CARGO_PKG_VERSION"));
    println!(
        "{}",
        ui_heading("No deadreckon runs in this directory yet.")
    );
    let in_git = std::env::current_dir()
        .map(|cwd| cwd.join(".git").exists())
        .unwrap_or(false);
    if in_git {
        println!(
            "{}",
            ui_muted(
                "source: git repo — start works in an isolated worktree; your checkout only changes on apply"
            )
        );
    } else {
        println!(
            "{}",
            ui_muted("source: plain directory — start will offer git init, copy, or fresh")
        );
    }
    println!();
    println!("{}", ui_heading("The flow:"));
    for command in TYPICAL_FLOW_COMMANDS {
        println!("  {}", ui_command(command));
    }
    println!();
    println!(
        "  {} {}",
        ui_muted("runs from other directories:"),
        ui_command("deadreckon list --all")
    );
    println!(
        "  {} {}",
        ui_muted("setup and health:        "),
        ui_command("deadreckon doctor")
    );
}

fn print_top_help() {
    ui::print_banner(env!("CARGO_PKG_VERSION"));
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
        "{} Job, run, chain, plan, and campaign ids accept unique prefixes where that command accepts the kind. {} means the newest item for the current project.",
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
    ui::print_banner(env!("CARGO_PKG_VERSION"));
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

fn auto_subscription_cli_provider(registry: &ProviderRegistry) -> Option<String> {
    setup::auto_subscription_cli_provider(registry)
}

fn start_prompt_choice(
    id: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
) -> prompt::SelectChoice {
    prompt::SelectChoice::with_detail(id, label, detail)
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

struct LaunchPreviewFacts<'a> {
    goal: &'a str,
    path: &'a str,
    suggestion: Option<String>,
    provider: &'a str,
    model: Option<String>,
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
    if let Some(model) = facts.model.as_ref().filter(|model| !model.is_empty()) {
        rows.push(("model".to_string(), model.clone()));
    }
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

fn shell_display_quote(value: &str) -> String {
    value.replace('"', "\\\"")
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
    match setup::select_provider_setup(&paths.config_path(), &registry, request) {
        Ok(selection) => Ok(selection),
        // Never dead-end where a menu can rescue: an unusable route on a TTY
        // offers the probe-before-ask picker instead of refusing. Off-TTY
        // (scripts, pipes, CI) the refusal below is byte-identical to before
        // the rescue existed. One rescue per launch — a second failure
        // refuses exactly as today.
        Err(refusal)
            if request.require_usable_route
                && prompt::is_tty()
                && refusal_is_rescueable(&refusal) =>
        {
            match rescue_provider_selection(&refusal, &registry)? {
                Some(route) => setup::select_provider_setup(
                    &paths.config_path(),
                    &registry,
                    setup::ProviderSetupRequest {
                        explicit_provider: Some(route.as_str()),
                        ..request
                    },
                )
                .map_err(setup_refusal_error),
                None => Err(setup_refusal_error(refusal)),
            }
        }
        Err(refusal) => Err(setup_refusal_error(refusal)),
    }
}

fn refusal_is_rescueable(refusal: &setup::SetupRefusal) -> bool {
    refusal.message.contains("needs credentials") || refusal.message.contains("not logged in")
}

/// The rescue menu: the same detected-CLI/API-route choices init shows, with
/// the failing route's escape hatches appended. Returns None for keep/cancel
/// (the original refusal is then surfaced verbatim).
fn rescue_provider_selection(
    refusal: &setup::SetupRefusal,
    registry: &ProviderRegistry,
) -> Result<Option<String>> {
    let mut choices = provider_picker_choices(registry);
    choices.push(prompt::SelectChoice::with_detail(
        "keep",
        "Keep the configured route anyway",
        refusal.try_line.clone(),
    ));
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let selection = prompt::select_one(&prompt::SelectPrompt {
        title: format!("{} — pick another route for this launch", refusal.message),
        help: Some(
            "this launch only; make it permanent with: deadreckon config provider <route>"
                .to_string(),
        ),
        choices,
        default_index: 0,
    })?;
    Ok(match selection.id.as_str() {
        "keep" | "cancel" => None,
        route => Some(route.to_string()),
    })
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

async fn provider_router_for_run_with_catalog_seam(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    backend: SandboxBackend,
    provider_override: Option<&str>,
    model: Option<&str>,
    no_seams: bool,
) -> Result<ProviderRouter> {
    let seams = read_seams_config(&paths.config_path(), no_seams)?;
    let seam_ctx = SeamRunCtx {
        run_root: state.run_root.clone(),
        working_dir: state.working_dir.clone(),
        sandbox_backend: backend,
    };
    let catalog_override = resolve_catalog_override(&seams, &seam_ctx).await;
    ProviderRouter::from_config_path_with_model_and_catalog_override(
        &paths.config_path(),
        provider_override,
        model,
        catalog_override.as_ref(),
    )
    .map_err(Into::into)
}

fn config_command(command: ConfigCommand) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    match command {
        ConfigCommand::Get { key } => {
            let root = load_config_value(&paths)?;
            match get_toml_path(&root, &key) {
                Some(value) => println!("{}", value_to_display(value)),
                None => {
                    return Err(CliError::Surface {
                        code: 1,
                        surface: config_missing_key_surface(&paths, &key)
                            .render_plain(!completion_hints_enabled(false)),
                    });
                }
            }
        }
        ConfigCommand::Set { key, value } => {
            fs::create_dir_all(paths.home())?;
            let mut root = load_config_value(&paths)?;
            set_toml_path(&mut root, &key, parse_config_value(&value));
            fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
            print!(
                "{}",
                config_set_surface(&paths, &key, &value)
                    .render_plain(!completion_hints_enabled(false))
            );
        }
        ConfigCommand::Provider { provider } => match provider {
            Some(provider) => {
                let selection = config_provider_setup_selection(
                    &paths,
                    &provider,
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
                print!(
                    "{}",
                    config_provider_surface(&paths, &provider, &selection)
                        .render_plain(!completion_hints_enabled(false))
                );
            }
            None => print_provider_selection(&paths, None)?,
        },
        ConfigCommand::RemoveProvider { provider } => {
            fs::create_dir_all(paths.home())?;
            let mut root = load_config_value(&paths)?;
            let Some(provider) = resolve_config_provider_removal_target(&paths, &root, provider)?
            else {
                return Ok(());
            };
            let removal = remove_config_provider(&paths, &mut root, &provider)?;
            if removal.changed() {
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
            }
            print!(
                "{}",
                config_remove_provider_surface(&paths, &removal)
                    .render_plain(!completion_hints_enabled(false))
            );
        }
        ConfigCommand::Model { model, provider } => match model {
            Some(model) => {
                fs::create_dir_all(paths.home())?;
                let mut root = load_config_value(&paths)?;
                let provider = provider
                    .or_else(|| active_provider_from_config(&root))
                    .ok_or_else(|| CliError::Surface {
                        code: 1,
                        surface: config_model_missing_provider_surface(&paths, &model)
                            .render_plain(!completion_hints_enabled(false)),
                    })?;
                set_provider_model(&mut root, &provider, &model);
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
                print!(
                    "{}",
                    config_model_surface(&paths, &provider, &model)
                        .render_plain(!completion_hints_enabled(false))
                );
            }
            None => print_provider_selection(&paths, provider.as_deref())?,
        },
    }
    Ok(())
}

fn config_provider_setup_selection(
    paths: &DeadreckonPaths,
    provider: &str,
    request: setup::ProviderSetupRequest<'_>,
) -> Result<setup::ProviderSetupSelection> {
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    setup::select_provider_setup(&paths.config_path(), &registry, request)
        .map_err(|refusal| config_setup_refusal_surface_error(paths, "provider", provider, refusal))
}

#[derive(Debug)]
struct ConfigProviderRemoval {
    provider: String,
    removed_default_provider: bool,
    removed_defaults_provider: bool,
    removed_doc_provider: bool,
    removed_fallback_entries: usize,
    removed_provider_table: bool,
    promoted_provider: Option<String>,
}

impl ConfigProviderRemoval {
    fn changed(&self) -> bool {
        self.removed_default_provider
            || self.removed_defaults_provider
            || self.removed_doc_provider
            || self.removed_fallback_entries > 0
            || self.removed_provider_table
            || self.promoted_provider.is_some()
    }

    fn removed_locations(&self) -> Vec<String> {
        let mut locations = Vec::new();
        if self.removed_default_provider {
            locations.push("default_provider".to_string());
        }
        if self.removed_defaults_provider {
            locations.push("defaults.provider".to_string());
        }
        if self.removed_doc_provider {
            locations.push("defaults.doc_provider".to_string());
        }
        if self.removed_fallback_entries > 0 {
            locations.push(format!(
                "fallback entries={}",
                self.removed_fallback_entries
            ));
        }
        if self.removed_provider_table {
            locations.push("providers table".to_string());
        }
        locations
    }
}

fn resolve_config_provider_removal_target(
    paths: &DeadreckonPaths,
    root: &toml::Value,
    provider: Option<String>,
) -> Result<Option<String>> {
    if let Some(provider) = provider {
        return Ok(Some(provider));
    }
    let ids = configured_provider_ids_from_root(root);
    if ids.is_empty() {
        print!(
            "{}",
            config_remove_provider_empty_surface(paths)
                .render_plain(!completion_hints_enabled(false))
        );
        return Ok(None);
    }
    let mut choices = ids
        .iter()
        .map(|id| {
            prompt::SelectChoice::with_detail(
                id.clone(),
                id.clone(),
                config_provider_location_labels(root, id).join(", "),
            )
        })
        .collect::<Vec<_>>();
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let choice = prompt::select_one(&prompt::SelectPrompt {
        title: "Remove provider".to_string(),
        help: Some(
            "Choose a configured route to remove from defaults, fallback, and local provider config."
                .to_string(),
        ),
        choices,
        default_index: 0,
    })?;
    if choice.id == "cancel" {
        println!("cancelled");
        return Ok(None);
    }
    Ok(Some(choice.id))
}

fn remove_config_provider(
    paths: &DeadreckonPaths,
    root: &mut toml::Value,
    provider: &str,
) -> Result<ConfigProviderRemoval> {
    let removed_default_provider = remove_toml_path_if_string(root, "default_provider", provider);
    let removed_defaults_provider = remove_toml_path_if_string(root, "defaults.provider", provider);
    let removed_doc_provider = remove_toml_path_if_string(root, "defaults.doc_provider", provider);
    let removed_fallback_entries = remove_provider_from_fallback(root, provider);
    let removed_provider_table = remove_provider_table(root, provider);
    let needs_promotion =
        removed_default_provider || removed_defaults_provider || removed_doc_provider;
    let promoted_provider = if needs_promotion {
        let promoted = choose_config_provider_replacement(paths, root, provider)?;
        if let Some(promoted) = promoted.as_deref() {
            if removed_default_provider || removed_defaults_provider {
                set_toml_path(
                    root,
                    "default_provider",
                    toml::Value::String(promoted.to_string()),
                );
                set_toml_path(
                    root,
                    "defaults.provider",
                    toml::Value::String(promoted.to_string()),
                );
            }
            if removed_doc_provider {
                set_toml_path(
                    root,
                    "defaults.doc_provider",
                    toml::Value::String(promoted.to_string()),
                );
            }
        }
        promoted
    } else {
        None
    };

    Ok(ConfigProviderRemoval {
        provider: provider.to_string(),
        removed_default_provider,
        removed_defaults_provider,
        removed_doc_provider,
        removed_fallback_entries,
        removed_provider_table,
        promoted_provider,
    })
}

fn choose_config_provider_replacement(
    paths: &DeadreckonPaths,
    root: &toml::Value,
    removed_provider: &str,
) -> Result<Option<String>> {
    let mut candidates = configured_provider_ids_from_root(root)
        .into_iter()
        .filter(|id| id != removed_provider)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(None);
    }
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    candidates.sort_by_key(|id| config_provider_replacement_score(&registry, id));
    Ok(candidates.into_iter().next())
}

fn config_provider_replacement_score(registry: &ProviderRegistry, id: &str) -> u8 {
    let ready_cli = registry.get(id).is_some_and(|descriptor| {
        descriptor.kind == DescriptorKind::Cli
            && descriptor
                .default_binary
                .as_deref()
                .is_some_and(command_exists)
    });
    if ready_cli {
        return 0;
    }
    if id.starts_with("cli:") {
        return 1;
    }
    if id == "smoke" || id.starts_with("smoke:") {
        return 2;
    }
    3
}

fn configured_provider_ids_from_root(root: &toml::Value) -> Vec<String> {
    let mut ids = Vec::new();
    push_toml_string_id(root, "default_provider", &mut ids);
    push_toml_string_id(root, "defaults.provider", &mut ids);
    push_toml_string_id(root, "defaults.doc_provider", &mut ids);
    if let Some(fallback) = get_toml_path(root, "fallback").and_then(toml::Value::as_array) {
        for item in fallback {
            if let Some(provider) = item.as_str() {
                push_unique(&mut ids, provider.to_string());
            }
        }
    }
    if let Some(providers) = get_toml_path(root, "providers").and_then(toml::Value::as_table) {
        for provider in providers.keys() {
            push_unique(&mut ids, provider.clone());
        }
    }
    ids
}

fn push_toml_string_id(root: &toml::Value, key: &str, ids: &mut Vec<String>) {
    if let Some(value) = get_toml_path(root, key).and_then(toml::Value::as_str) {
        push_unique(ids, value.to_string());
    }
}

fn config_provider_location_labels(root: &toml::Value, provider: &str) -> Vec<String> {
    let mut labels = Vec::new();
    if toml_path_string_equals(root, "default_provider", provider) {
        labels.push("default".to_string());
    }
    if toml_path_string_equals(root, "defaults.provider", provider) {
        labels.push("run default".to_string());
    }
    if toml_path_string_equals(root, "defaults.doc_provider", provider) {
        labels.push("doc default".to_string());
    }
    if get_toml_path(root, "fallback")
        .and_then(toml::Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(provider)))
    {
        labels.push("fallback".to_string());
    }
    if get_toml_path(root, "providers")
        .and_then(toml::Value::as_table)
        .is_some_and(|providers| providers.contains_key(provider))
    {
        labels.push("local config".to_string());
    }
    if labels.is_empty() {
        labels.push("configured route".to_string());
    }
    labels
}

fn config_setup_refusal_surface_error(
    paths: &DeadreckonPaths,
    subject: &str,
    target: &str,
    refusal: setup::SetupRefusal,
) -> CliError {
    let setup::SetupRefusal { message, try_line } = refusal;
    CliError::Surface {
        code: 1,
        surface: config_refusal_surface(paths, subject, target, &message, &try_line)
            .render_plain(!completion_hints_enabled(false)),
    }
}

fn config_refusal_surface(
    paths: &DeadreckonPaths,
    subject: &str,
    target: &str,
    message: &str,
    primary: &str,
) -> VerdictSurface {
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "config",
        Some(subject),
        ExplanationPanel::new(
            format!("DeadReckon could not update config {subject} {target}."),
            format!("The config mutation was blocked before writing because {message}."),
            vec![
                ("config", paths.config_path().display().to_string()),
                (subject, target.to_string()),
                ("reason", message.to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        vec![("Secondary", "deadreckon config provider")],
    )
}

fn config_missing_key_surface(paths: &DeadreckonPaths, key: &str) -> VerdictSurface {
    let primary = format!("deadreckon config set {key} <value>");
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "config",
        Some(key),
        ExplanationPanel::new(
            format!("DeadReckon could not read config key {key}."),
            "The key is not present in the current config file, so there is no value to display.",
            vec![
                ("config", paths.config_path().display().to_string()),
                ("key", key.to_string()),
                ("reason", "missing config key".to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon config provider")],
    )
}

fn print_provider_selection(paths: &DeadreckonPaths, provider: Option<&str>) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider)?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let defaults = config_defaults(paths).ok();
    let routes = router.route_info();
    let selected = router.selected_route_info();
    println!("{}", ui_heading("provider selection"));
    for route in &routes {
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
            ui_id(&route.name),
            kind,
            route.model
        );
    }
    println!();
    let setup_selection = provider_setup_selection(
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
    )
    .ok();
    print!(
        "{}",
        provider_selection_surface(selected.as_ref(), setup_selection.as_ref(), routes.len())
            .render_plain(!completion_hints_enabled(false))
    );
    Ok(())
}

fn config_set_surface(paths: &DeadreckonPaths, key: &str, value: &str) -> VerdictSurface {
    let primary = format!("deadreckon config get {key}");
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "config",
        Some(key),
        ExplanationPanel::new(
            format!("DeadReckon wrote config key {key}."),
            "The config mutation completed; reading the same key is the safest verification command.",
            vec![
                ("config", paths.config_path().display().to_string()),
                ("key", key.to_string()),
                ("value", value.to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon doctor")],
    )
}

fn config_remove_provider_surface(
    paths: &DeadreckonPaths,
    removal: &ConfigProviderRemoval,
) -> VerdictSurface {
    let changed = removal.changed();
    let primary = if changed {
        "deadreckon config provider"
    } else {
        "deadreckon config remove-provider"
    };
    let kind = if changed {
        VerdictKind::Completed
    } else {
        VerdictKind::Noop
    };
    let mut evidence = vec![
        (
            "config".to_string(),
            paths.config_path().display().to_string(),
        ),
        ("provider".to_string(), removal.provider.clone()),
    ];
    let removed = removal.removed_locations();
    evidence.push((
        "removed".to_string(),
        if removed.is_empty() {
            "none".to_string()
        } else {
            removed.join(", ")
        },
    ));
    if let Some(promoted) = removal.promoted_provider.as_deref() {
        evidence.push(("new default".to_string(), promoted.to_string()));
    }
    VerdictSurface::must_new(
        kind,
        "config",
        Some("provider"),
        ExplanationPanel::new(
            if changed {
                format!(
                    "DeadReckon removed {} from configured provider selection.",
                    removal.provider
                )
            } else {
                format!(
                    "DeadReckon found no configured reference to {}.",
                    removal.provider
                )
            },
            if changed {
                "Built-in provider routes still appear in `providers list --all`; this only changes your local defaults, fallback chain, and local provider table."
            } else {
                "The provider registry is built in, but this route was not selected by the current local config."
            },
            evidence,
        ),
        vec![("Recommended", primary)],
        vec![
            ("Secondary", "deadreckon providers list"),
            ("Secondary", "deadreckon start \"goal\""),
        ],
    )
}

fn config_remove_provider_empty_surface(paths: &DeadreckonPaths) -> VerdictSurface {
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "config",
        Some("provider"),
        ExplanationPanel::new(
            "DeadReckon found no configured provider routes to remove.",
            "Built-in provider routes are still available in the registry; remove-provider only edits local config selections.",
            vec![(
                "config".to_string(),
                paths.config_path().display().to_string(),
            )],
        ),
        vec![("Recommended", "deadreckon config provider cli:codex")],
        vec![("Secondary", "deadreckon providers list --all")],
    )
}

fn config_provider_surface(
    paths: &DeadreckonPaths,
    provider: &str,
    selection: &setup::ProviderSetupSelection,
) -> VerdictSurface {
    let setup_try = selection.try_lines.first().cloned();
    let kind = if setup_try.is_some() {
        VerdictKind::Blocked
    } else {
        VerdictKind::Completed
    };
    let primary = setup_try.unwrap_or_else(|| "deadreckon doctor".to_string());
    let what = if kind == VerdictKind::Blocked {
        format!("DeadReckon saved {provider} as the default provider, but setup is incomplete.")
    } else {
        format!("DeadReckon saved {provider} as the default provider.")
    };
    let why = if kind == VerdictKind::Blocked {
        "The config write completed, but the selected provider still needs setup before it can run work."
    } else {
        "The provider route is configured; doctor is the safest next command to verify the whole setup."
    };
    let mut evidence = vec![
        (
            "config".to_string(),
            paths.config_path().display().to_string(),
        ),
        ("provider".to_string(), provider.to_string()),
        ("setup".to_string(), selection.row_value()),
    ];
    if !selection.warnings.is_empty() {
        evidence.push(("warnings".to_string(), selection.warnings.join("; ")));
    }
    VerdictSurface::must_new(
        kind,
        "config",
        Some("provider"),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        vec![
            ("Secondary", "deadreckon config provider"),
            ("Secondary", "deadreckon run \"goal\""),
        ],
    )
}

fn config_model_surface(paths: &DeadreckonPaths, provider: &str, model: &str) -> VerdictSurface {
    let primary = format!("deadreckon config model --provider {provider}");
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "config",
        Some("model"),
        ExplanationPanel::new(
            format!("DeadReckon wrote model {model} for provider {provider}."),
            "The model override is configured; reading the provider model is the safest verification command.",
            vec![
                ("config", paths.config_path().display().to_string()),
                ("provider", provider.to_string()),
                ("model", model.to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon doctor")],
    )
}

fn config_model_missing_provider_surface(paths: &DeadreckonPaths, model: &str) -> VerdictSurface {
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "config",
        Some("model"),
        ExplanationPanel::new(
            format!(
                "DeadReckon could not write model {model} because no active provider is configured."
            ),
            "Model overrides are provider-scoped; configure the default provider before saving a model without --provider.",
            vec![
                ("config", paths.config_path().display().to_string()),
                ("model", model.to_string()),
                ("provider", "none".to_string()),
            ],
        ),
        vec![("Recommended", "deadreckon config provider cli:codex")],
        vec![("Secondary", "deadreckon config provider")],
    )
}

fn provider_selection_surface(
    selected: Option<&ProviderRouteInfo>,
    selection: Option<&setup::ProviderSetupSelection>,
    route_count: usize,
) -> VerdictSurface {
    let setup_try = selection
        .and_then(|selection| selection.try_lines.first())
        .cloned();
    let kind = if setup_try.is_some() {
        VerdictKind::Blocked
    } else {
        VerdictKind::Verified
    };
    let primary = setup_try.unwrap_or_else(|| {
        selected
            .map(|route| format!("deadreckon run \"goal\" --provider {}", route.name))
            .unwrap_or_else(|| "deadreckon config provider cli:codex".to_string())
    });
    let what = match selected {
        Some(route) => format!(
            "DeadReckon read provider routes and selected {}.",
            route.name
        ),
        None => "DeadReckon read provider routes, but no active provider was selected.".to_string(),
    };
    let why = if kind == VerdictKind::Blocked {
        "The provider listing found an incomplete setup; the recommended command fixes the first missing prerequisite."
    } else {
        "The provider listing is usable; the recommended command starts work with the selected route."
    };
    let mut evidence = vec![("routes".to_string(), route_count.to_string())];
    if let Some(route) = selected {
        evidence.push(("selected".to_string(), route.name.clone()));
        evidence.push(("model".to_string(), route.model.clone()));
        evidence.push((
            "credential".to_string(),
            if route.has_credential {
                "ready".to_string()
            } else {
                "missing".to_string()
            },
        ));
    }
    if let Some(selection) = selection {
        evidence.push(("setup".to_string(), selection.row_value()));
        if !selection.warnings.is_empty() {
            evidence.push(("warnings".to_string(), selection.warnings.join("; ")));
        }
    }
    let mut secondary = vec![("Secondary", "deadreckon doctor".to_string())];
    if let Some(route) = selected {
        secondary.push((
            "Secondary",
            format!("deadreckon config model <model> --provider {}", route.name),
        ));
    }
    VerdictSurface::must_new(
        kind,
        "config",
        Some("provider"),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|(label, command)| (*label, command.as_str())),
    )
}

fn print_provider_setup_rows(selections: &[setup::ProviderSetupSelection]) {
    if selections.is_empty() {
        return;
    }
    for line in provider_setup_row_lines(selections) {
        println!("{line}");
    }
}

fn provider_setup_row_lines(selections: &[setup::ProviderSetupSelection]) -> Vec<String> {
    if selections.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![ui_heading("provider setup")];
    for selection in selections {
        lines.push(format!(
            "{} {}",
            ui_command(format!("{}:", selection.role.label())),
            selection.row_value()
        ));
    }
    for selection in selections {
        for warning in &selection.warnings {
            lines.push(format!("  {}", ui_warn(warning)));
        }
        for try_line in &selection.try_lines {
            lines.push(format!(
                "  {} {}",
                ui_command("setup:"),
                ui_command(try_line)
            ));
        }
    }
    lines
}

#[cfg(test)]
mod provider_setup_row_tests {
    use super::*;

    fn selection_with_try_lines(try_lines: Vec<&str>) -> setup::ProviderSetupSelection {
        setup::ProviderSetupSelection {
            role: setup::SetupProviderRole::ConfigDefault,
            provider: Some("cli:codex".to_string()),
            model: Some("provider default".to_string()),
            source: setup::SetupProviderSource::Flag,
            kind: Some("subscription-cli".to_string()),
            credential: Some("missing".to_string()),
            install_hint: None,
            warnings: vec![
                "provider cli:codex needs credentials or an installed binary".to_string(),
            ],
            try_lines: try_lines.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn provider_setup_rows_do_not_print_a_second_recommended_command() {
        let rendered = provider_setup_row_lines(&[selection_with_try_lines(vec![
            "npm i -g @openai/codex",
            "deadreckon providers list --all",
        ])])
        .join("\n");

        assert!(rendered.contains("provider setup"), "{rendered}");
        assert!(rendered.contains("default:"), "{rendered}");
        assert!(!rendered.contains("recommended:"), "{rendered}");
        assert!(
            rendered.contains("setup: npm i -g @openai/codex"),
            "{rendered}"
        );
        assert!(
            rendered.contains("setup: deadreckon providers list --all"),
            "{rendered}"
        );
        assert!(!rendered.contains("try:"), "{rendered}");
    }

    #[test]
    fn provider_setup_rows_without_selections_stay_empty() {
        assert!(provider_setup_row_lines(&[]).is_empty());
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

fn descriptor_kind_label(kind: &DescriptorKind) -> &'static str {
    match kind {
        DescriptorKind::Http => "http",
        DescriptorKind::Cli => "cli",
        DescriptorKind::LocalHttp => "local-http",
        DescriptorKind::Scripted => "scripted",
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
        run_id: None,
        durable_source_cwd: None,
        continuation: None,
        tamper_baseline: None,
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
        no_seams: false,
        plain,
        prevent_sleep: Some("off".to_string()),
        quiet: true,
        max_spend: Some(1.0),
        max_wall_seconds: Some(60.0),
        sandbox: Some("none".to_string()),
        untrusted: true,
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
        narrate: false,
        no_narrate: false,
        narrator_model: None,
        infer_contract: false,
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
                "trust": "untrusted local smoke diagnostic",
                "trusted_job_receipt": false,
                "gate": "local smoke gate evidence only; not a trusted Job receipt",
                "proof": proof.proof_path,
                "story": proof.story_path,
                "lineage": proof.lineage,
                "next": "deadreckon start \"build the real thing\"",
            }))?
        );
    } else {
        println!(
            "UNTRUSTED LOCAL SMOKE DIAGNOSTIC\nThis checks the local Run path only; it cannot issue a trusted Job receipt."
        );
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
        Tone::Negative
    } else {
        Tone::Plain
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
        gate_tone: Tone::Plain,
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
    start_attach: Option<bool>,
    start_confirm_contract: Option<bool>,
    model: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
    cli_max_wall_seconds: Option<f64>,
    prevent_sleep: Option<String>,
    plain: Option<bool>,
    ui_motion: Option<String>,
    ui_input_latency_budget_ms: Option<u64>,
    doc_provider: Option<String>,
    doc_skill: Option<String>,
    doc_subskills: Option<Vec<String>>,
    doc_polish_token_budget: Option<u32>,
    doc_polish_budget_cap_usd: Option<f64>,
}

fn config_defaults(paths: &DeadreckonPaths) -> Result<ConfigDefaults> {
    let root = load_config_value(paths)?;
    Ok(ConfigDefaults {
        start_attach: get_toml_path(&root, "defaults.start_attach").and_then(toml::Value::as_bool),
        start_confirm_contract: get_toml_path(&root, "start.confirm_contract")
            .and_then(toml::Value::as_bool),
        provider: get_toml_path(&root, "defaults.provider")
            .or_else(|| get_toml_path(&root, "default_provider"))
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        model: get_toml_path(&root, "defaults.model")
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
        ui_motion: get_toml_path(&root, "ui.motion")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        ui_input_latency_budget_ms: get_toml_path(&root, "ui.input_latency_budget_ms")
            .and_then(toml::Value::as_integer)
            .and_then(|ms| u64::try_from(ms).ok()),
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

impl ConfigDefaults {
    fn motion_policy(&self, stdout_is_tty: bool, replay_mode: bool) -> tui::MotionPolicy {
        tui::MotionPolicy::from_config_value(self.ui_motion.as_deref(), stdout_is_tty, replay_mode)
    }
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
    seams: &'a str,
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
        seams,
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
            "mode={} branch={} base={} wt={} provider={} model={} docs={} seams={} cap={}/{} done_criteria={}",
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
            seams,
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
        model: Some(model.to_string()).filter(|model| model != "provider default"),
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
        ("seams".to_string(), seams.to_string()),
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

#[cfg(test)]
mod seam_surface_tests {
    use super::*;

    #[test]
    fn preview_lists_active_external_seams() {
        let temp = tempfile::TempDir::new().expect("temp");
        let codebase = CodebaseRecord::fresh();
        let acceptance = setup::DoneCriteriaSelection::default_gate();
        let sleep = sleep::SleepPreview {
            mode: sleep::SleepMode::None,
            binary: None,
            skip_reason: None,
        };

        let preview = run_preview(&RunPreview {
            goal: "ship seams",
            cwd: temp.path(),
            codebase: &codebase,
            provider: Some("smoke"),
            provider_source: "flag",
            route: None,
            sandbox: "none",
            doc_provider: None,
            doc_provider_source: "none",
            max_spend: Some(1.0),
            max_wall_seconds: Some(60.0),
            acceptance: &acceptance,
            sleep: &sleep,
            seams: "external: policy, event_sink",
            brief: false,
            plain: true,
            run_id: "run123",
        });

        assert!(preview.contains("seams"));
        assert!(preview.contains("external: policy, event_sink"));
    }

    #[test]
    fn sleep_skipped_surface_has_one_primary_action() {
        let temp = tempfile::TempDir::new().expect("temp");
        let surface = sleep_skipped_surface(
            "abcdef1234567890",
            temp.path(),
            sleep::SkipReason::Unsupported,
        );
        let rendered = surface.render_plain(false);

        assert!(
            rendered.starts_with("no-op sleep prevention abcdef12"),
            "{rendered}"
        );
        assert!(rendered.contains("Explanation\n"), "{rendered}");
        assert!(rendered.contains("Evidence\n"), "{rendered}");
        assert_eq!(rendered.matches("\nRecommended\n").count(), 1, "{rendered}");
        assert!(
            rendered.contains("Recommended\ndeadreckon config set defaults.prevent_sleep off"),
            "{rendered}"
        );
        assert!(!rendered.contains("try:"), "{rendered}");
    }
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

fn sleep_skipped_surface(
    run_id: &str,
    working_dir: &Path,
    reason: sleep::SkipReason,
) -> VerdictSurface {
    let id = run_prefix(run_id);
    let reason_label = sleep::skip_reason_label(reason);
    let primary = sleep_primary_command(reason);
    let secondary = sleep_secondary_commands(reason);
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "sleep prevention",
        Some(&id),
        ExplanationPanel::new(
            format!("DeadReckon skipped sleep prevention for run {id}."),
            format!(
                "The run continues, so this is no-op; the inhibitor was not armed because {reason_label}."
            ),
            vec![
                ("run".to_string(), id.clone()),
                ("reason".to_string(), reason_label.to_string()),
                ("prevent_sleep".to_string(), "on".to_string()),
                (
                    "metadata".to_string(),
                    sleep::metadata_path(working_dir).display().to_string(),
                ),
                ("working_dir".to_string(), working_dir.display().to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        secondary,
    )
}

fn sleep_primary_command(reason: sleep::SkipReason) -> &'static str {
    match reason {
        sleep::SkipReason::UnavailableBinary if cfg!(target_os = "linux") => {
            "sudo apt install systemd"
        }
        sleep::SkipReason::UnavailableBinary if cfg!(target_os = "macos") => {
            "/usr/bin/caffeinate -di"
        }
        sleep::SkipReason::UnavailableBinary
        | sleep::SkipReason::Unsupported
        | sleep::SkipReason::NonTty
        | sleep::SkipReason::UserDisabled
        | sleep::SkipReason::AlreadyInhibited => "deadreckon config set defaults.prevent_sleep off",
    }
}

fn sleep_secondary_commands(reason: sleep::SkipReason) -> Vec<(&'static str, &'static str)> {
    match reason {
        sleep::SkipReason::UnavailableBinary
            if cfg!(target_os = "linux") || cfg!(target_os = "macos") =>
        {
            vec![(
                "Silence future skips",
                "deadreckon config set defaults.prevent_sleep off",
            )]
        }
        _ => Vec::new(),
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
            return Err(CliError::Surface {
                code: 1,
                surface: pending_plan_result_surface(plan, verb)
                    .render_plain(!completion_hints_enabled(false)),
            });
        }
        PlanStatus::Forked => {
            let ready_to_merge = plan
                .tasks
                .iter()
                .all(|task| task.status == PlanTaskStatus::Completed);
            return Err(CliError::Surface {
                code: 1,
                surface: forked_plan_result_surface(plan, verb, ready_to_merge)
                    .render_plain(!completion_hints_enabled(false)),
            });
        }
        PlanStatus::Failed => {
            return Err(CliError::Surface {
                code: 1,
                surface: failed_plan_result_surface(plan, verb)
                    .render_plain(!completion_hints_enabled(false)),
            });
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

fn pending_plan_result_surface(plan: &Plan, verb: &str) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let primary = format!("deadreckon fork {id}");
    VerdictSurface::must_new(
        VerdictKind::Blocked,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            "The plan has not started yet.",
            format!(
                "DeadReckon has no completed result to {verb} until the plan is forked and merged."
            ),
            [
                ("plan".to_string(), id.clone()),
                (
                    "status".to_string(),
                    plan_status_label(plan.status).to_string(),
                ),
                ("requested verb".to_string(), verb.to_string()),
                ("tasks".to_string(), plan.tasks.len().to_string()),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
}

fn forked_plan_result_surface(plan: &Plan, verb: &str, ready_to_merge: bool) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let primary = if ready_to_merge {
        format!("deadreckon merge {id}")
    } else {
        format!("deadreckon attach {id}")
    };
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let kind = if ready_to_merge {
        VerdictKind::Blocked
    } else {
        VerdictKind::Paused
    };
    let why = if ready_to_merge {
        format!(
            "All child tasks are complete, but DeadReckon has no completed result to {verb} until merge creates the result run."
        )
    } else {
        format!(
            "DeadReckon has no completed result to {verb} while child tasks are still running or waiting."
        )
    };
    VerdictSurface::must_new(
        kind,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            format!(
                "The plan is still {}; cannot {verb} it yet.",
                plan_status_label(plan.status)
            ),
            why,
            [
                ("plan".to_string(), id.clone()),
                (
                    "status".to_string(),
                    plan_status_label(plan.status).to_string(),
                ),
                ("requested verb".to_string(), verb.to_string()),
                (
                    "tasks".to_string(),
                    format!("{completed}/{} completed", plan.tasks.len()),
                ),
                ("ready to merge".to_string(), ready_to_merge.to_string()),
            ],
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
}

fn failed_plan_result_surface(plan: &Plan, verb: &str) -> VerdictSurface {
    let id = run_prefix(&plan.plan_id);
    let failed_tasks = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Failed)
        .map(|task| task.task_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut evidence = vec![
        ("plan".to_string(), id.clone()),
        (
            "status".to_string(),
            plan_status_label(plan.status).to_string(),
        ),
        ("requested verb".to_string(), verb.to_string()),
    ];
    if !failed_tasks.is_empty() {
        evidence.push(("failed tasks".to_string(), failed_tasks));
    }

    let primary = format!("deadreckon show {id} --why-failed");
    VerdictSurface::must_new(
        VerdictKind::Failed,
        "plan",
        Some(&id),
        ExplanationPanel::new(
            "The plan failed before producing a completed result run.",
            format!(
                "DeadReckon has no completed result to {verb}; inspect the failure evidence before trying to finish, apply, or export it."
            ),
            evidence,
        ),
        [("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
}

fn default_plan_materialize_dest(plan: &Plan) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(dest_slug(&deadreckon_core::paths::task_key(
            &plan.root_goal,
        )))
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
    let estimated_spend = commands::doc::estimate_doc_polish_spend(
        &router,
        &provider,
        DEFAULT_DOC_POLISH_TOKEN_BUDGET,
        1,
    )?;
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
    let provider_cwd = plan.parent_cwd.clone().unwrap_or(std::env::current_dir()?);
    let mut request = ProviderRequest::enforceably_read_only(
        plan_doc_provider_prompt(&input)?,
        16_384,
        provider_cwd,
    );
    request.output_path = Some(plan_doc_path(paths, &plan.plan_id, "plan-doc-provider.out"));
    let response = router.complete(&request).await;
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
    stage_plan_result_changes(worktree_path)?;
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

#[cfg(test)]
mod campaign_spawn_tests;
#[cfg(test)]
mod failure_surfacing_tests;

#[cfg(test)]
mod effortless_consistency_tests;

#[cfg(test)]
mod navigable_tests;

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
    hash: ArtifactFileFingerprint,
}

#[derive(Debug, Clone)]
struct PlanMergeSource {
    task_id: String,
    task_index: u32,
    run_id: String,
    artifact_root: PathBuf,
}

#[derive(Debug, Clone)]
struct MergeableRunArtifact {
    relative: PathBuf,
    artifact_path: PathBuf,
    fingerprint: ArtifactFileFingerprint,
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
) -> Result<Vec<MergeableRunArtifact>> {
    let mut files = Vec::new();
    for (relative, fingerprint) in build_deliverable_file_index(root)?.files {
        if skip_plan_merge_file(&relative) {
            continue;
        }
        let artifact_path = root.join(&relative);
        if artifact_path.strip_prefix(root).is_err() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{prefix_error}: {} is outside {}",
                artifact_path.display(),
                root.display()
            ))));
        }
        files.push(MergeableRunArtifact {
            relative,
            artifact_path,
            fingerprint,
        });
    }
    Ok(files)
}

#[cfg(test)]
fn mergeable_run_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf, ArtifactFileFingerprint)>> {
    Ok(
        mergeable_run_files_with_prefix_error(root, "merge source prefix error")?
            .into_iter()
            .map(|artifact| {
                (
                    artifact.relative,
                    artifact.artifact_path,
                    artifact.fingerprint,
                )
            })
            .collect(),
    )
}

/// A same-path collision between two independent campaign sub-results.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposeConflict {
    path: PathBuf,
    first_label: String,
    second_label: String,
}

#[cfg(test)]
#[derive(Debug)]
struct ComposeResult {
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
    mut make_seen: impl FnMut(&T, &Path, &Path, ArtifactFileFingerprint) -> S,
    mut decide_conflict: impl FnMut(&Path, &S, &S) -> ComposeMergeDecision<C>,
) -> Result<Vec<C>> {
    let inventories = sources
        .iter()
        .map(|source| mergeable_run_files_with_prefix_error(&source.root, source.prefix_error))
        .collect::<Result<Vec<_>>>()?;
    reject_artifact_hierarchy_collisions(&inventories)?;

    remove_if_exists(merge_dir)?;
    fs::create_dir_all(merge_dir)?;
    let mut seen: BTreeMap<PathBuf, (S, ArtifactFileFingerprint)> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (source, inventory) in sources.iter().zip(inventories) {
        for artifact in inventory {
            let current = make_seen(
                &source.data,
                &artifact.relative,
                &artifact.artifact_path,
                artifact.fingerprint.clone(),
            );
            let decision = match seen.get(&artifact.relative) {
                Some((_, previous)) if previous == &artifact.fingerprint => continue,
                Some((previous, _)) => decide_conflict(&artifact.relative, previous, &current),
                None => {
                    copy_merge_file(&artifact.artifact_path, &merge_dir.join(&artifact.relative))?;
                    seen.insert(artifact.relative, (current, artifact.fingerprint));
                    continue;
                }
            };
            match decision {
                ComposeMergeDecision::KeepExisting => {}
                ComposeMergeDecision::UseCurrent => {
                    copy_merge_file(&artifact.artifact_path, &merge_dir.join(&artifact.relative))?;
                    seen.insert(artifact.relative, (current, artifact.fingerprint));
                }
                ComposeMergeDecision::RecordConflict {
                    conflict,
                    use_current,
                } => {
                    conflicts.push(conflict);
                    if use_current {
                        copy_merge_file(
                            &artifact.artifact_path,
                            &merge_dir.join(&artifact.relative),
                        )?;
                        seen.insert(artifact.relative, (current, artifact.fingerprint));
                    }
                }
            }
        }
    }
    Ok(conflicts)
}

fn reject_artifact_hierarchy_collisions(inventories: &[Vec<MergeableRunArtifact>]) -> Result<()> {
    let paths = inventories
        .iter()
        .flat_map(|inventory| inventory.iter().map(|artifact| artifact.relative.clone()))
        .collect::<BTreeSet<_>>();
    for path in &paths {
        let mut ancestor = path.parent();
        while let Some(candidate) = ancestor {
            if candidate.as_os_str().is_empty() {
                break;
            }
            if paths.contains(candidate) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "plan artifact hierarchy collision: {} is an ancestor of {}",
                    candidate.display(),
                    path.display()
                ))));
            }
            ancestor = candidate.parent();
        }
    }
    Ok(())
}

/// Compose several already-promoted result trees into one merge dir. Sub-results
/// are independent (no dependency edges), so this is fail-on-conflict: two roots
/// touching the same relative path with different content yields a conflict. The
/// first writer wins on disk; conflicts are reported so the campaign can fail.
#[cfg(test)]
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
        |label, _relative, _file, _fingerprint| label.clone(),
        |relative, previous, current| ComposeMergeDecision::RecordConflict {
            conflict: ComposeConflict {
                path: relative.to_path_buf(),
                first_label: previous.clone(),
                second_label: current.clone(),
            },
            use_current: false,
        },
    )?;
    Ok(ComposeResult { conflicts })
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
        hash: artifact_fingerprint_label(&file.hash),
        depends_on,
    }
}

fn artifact_fingerprint_label(fingerprint: &ArtifactFileFingerprint) -> String {
    let kind = match fingerprint.kind {
        ArtifactFileKind::RegularFile => "regular",
        ArtifactFileKind::ExecutableFile => "executable",
        ArtifactFileKind::SymbolicLink => "symlink",
    };
    format!("{kind}:{}:{}", fingerprint.size, fingerprint.hash)
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
    let path = context.proof_dir.join("repair-request.json");
    if context.proof_dir.join("repair-run.json").is_file() && path.is_file() {
        return Ok(path);
    }
    let worker_specs = plan
        .tasks
        .iter()
        .map(|task| {
            let worker_spec = if task.worker_spec.is_absolute() {
                task.worker_spec.clone()
            } else {
                paths.plan_dir(&plan.plan_id).join(&task.worker_spec)
            };
            (task.task_id.clone(), worker_spec.display().to_string())
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
    #[serde(skip)]
    planner_spend_usd: f64,
    #[serde(skip)]
    planner_wall_seconds: f64,
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
    let repair_plan_path = context.proof_dir.join("repair-plan.json");
    let repair_plan =
        if context.proof_dir.join("repair-run.json").is_file() && repair_plan_path.is_file() {
            serde_json::from_slice(&fs::read(&repair_plan_path)?)?
        } else {
            invoke_merge_repair_planner(
                paths,
                plan,
                options.provider,
                options.mode,
                &request_path,
                options.quiet,
            )
            .await?
        };
    validate_merge_repair_plan(&repair_plan, &merge.unresolved_conflicts(), options.mode)?;
    if !repair_plan_path.is_file() {
        fs::write(&repair_plan_path, serde_json::to_vec_pretty(&repair_plan)?)?;
    }
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
    let sandbox_backend = commands::graph_job::current_driver_sandbox_backend(paths)?
        .filter(|backend| *backend != deadreckon_sandbox::SandboxBackend::None)
        .unwrap_or(deadreckon_sandbox::SandboxBackend::Auto);
    let request =
        ProviderRequest::enforceably_read_only_with_backend(prompt, 8192, cwd, sandbox_backend);
    let started_at = std::time::Instant::now();
    let response =
        maybe_with_cli_wait_status(!quiet, "planning merge repair", router.complete(&request))
            .await?;
    let mut plan = parse_merge_repair_response(&response.content)?;
    plan.planner_spend_usd = response.spend.cost_usd;
    plan.planner_wall_seconds = started_at.elapsed().as_secs_f64();
    Ok(plan)
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
        remove_artifact_path(&dest)?;
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
    let request_path = context.proof_dir.join("repair-request.json");
    let repair_plan_path = context.proof_dir.join("repair-plan.json");
    let repair_request_sha256 = deadreckon_core::flight::sha256_file(&request_path)?;
    let repair_plan_sha256 = deadreckon_core::flight::sha256_file(&repair_plan_path)?;
    let repair_id = deadreckon_core::flight::sha256_text(&format!(
        "{}\0{}\0{}",
        plan.plan_id, repair_request_sha256, repair_plan_sha256
    ));
    let repair_round = 1;
    let inherited_sandbox = commands::graph_job::current_driver_sandbox_backend(paths)?
        .unwrap_or(deadreckon_sandbox::SandboxBackend::Auto);
    if inherited_sandbox == deadreckon_sandbox::SandboxBackend::None {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair child refused an uncontained parent sandbox".to_string(),
        )));
    }
    let remaining_budget = commands::graph_job::current_driver_remaining_repair_budget(
        paths,
        plan,
        repair_plan.planner_spend_usd,
        repair_plan.planner_wall_seconds,
    )?;
    if let Some((existing, library)) = recover_existing_merge_repair_child(
        paths,
        plan,
        context,
        &repair_id,
        repair_round,
        &repair_request_sha256,
        &repair_plan_sha256,
        inherited_sandbox,
        remaining_budget.map(|(_, wall)| wall),
    )
    .await?
    {
        copy_repair_library_to_working(&context.working_dir, &library)?;
        return Ok(existing);
    }
    let repair_run_id = Uuid::new_v4().simple().to_string();
    let repair_goal = format!(
        "{}\n\nRoot goal: {}\nPlan: {}\nRepair request: {}\nRepair plan: {}\n\nResolve only the merge conflict paths named in the repair plan unless a build/test update is strictly required to make the repaired artifact coherent. Preserve completed child behavior and report files changed.",
        repair_plan
            .repair_goal
            .as_deref()
            .unwrap_or("Resolve orchestration merge conflicts."),
        plan.root_goal,
        plan.plan_id,
        request_path.display(),
        repair_plan_path.display()
    );
    let merge_working = context.working_dir.clone();
    let mut argv = vec![
        "run".to_string(),
        repair_goal,
        "--from".to_string(),
        merge_working.display().to_string(),
        "--yes".to_string(),
        "--no-confirm".to_string(),
        "--no-hints".to_string(),
        "--no-docs".to_string(),
        "--sandbox".to_string(),
        inherited_sandbox.to_string(),
    ];
    if let Some((remaining_spend, remaining_wall)) = remaining_budget {
        argv.extend([
            "--max-spend".to_string(),
            remaining_spend.to_string(),
            "--max-wall-seconds".to_string(),
            remaining_wall.to_string(),
        ]);
    }
    if provider == "smoke" || provider.starts_with("smoke:") {
        argv.push("--smoke".to_string());
    } else {
        argv.extend(["--provider".to_string(), provider.to_string()]);
    }
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&merge_working)
        .env("DEADRECKON_HOME", paths.home())
        .env("DEADRECKON_HINTS", "0")
        .env("DEADRECKON_SCOPE_ROOT", &repair_scope)
        .args(&argv);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let durable_parent_expected = commands::graph_job::resolve_plan_owner(paths, plan)?.is_some();
    if durable_parent_expected && commands::graph_job::current_parent_job_id().is_none() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Job-owned merge repair cannot fall back to an unguarded child launch".to_string(),
        )));
    }
    let prepared = if let Some(parent_job_id) =
        commands::graph_job::current_parent_job_id().map(str::to_string)
    {
        let canonical_proof = fs::canonicalize(&context.proof_dir)?;
        Some(commands::graph_job::prepare_delegated_invocation(
            paths,
            commands::graph_job::DelegatedAction::MergeRepair {
                root_artifact_id: parent_job_id,
                repair_id: repair_id.clone(),
                repair_round,
                run_id: repair_run_id.clone(),
                proof_dir: canonical_proof,
                repair_request_sha256: repair_request_sha256.clone(),
                repair_plan_sha256: repair_plan_sha256.clone(),
            },
            &argv,
            &merge_working,
            &repair_scope,
            None,
        )?)
    } else {
        None
    };
    let launch_authority = merge_repair_launch_record(
        plan,
        context,
        &repair_id,
        repair_round,
        &repair_run_id,
        &repair_request_sha256,
        &repair_plan_sha256,
        inherited_sandbox,
        prepared.as_ref().map(|prepared| prepared.capability_id()),
        repair_plan.planner_spend_usd,
        repair_plan.planner_wall_seconds,
    )?;
    let authority_path = context.proof_dir.join("repair-run.json");
    let child = if let Some(prepared) = prepared.as_ref() {
        commands::graph_job::spawn_merge_repair_delegated(
            paths,
            command,
            prepared,
            &authority_path,
            launch_authority,
        )?
    } else {
        commands::job::write_json_synced(&authority_path, &launch_authority)?;
        command.spawn()?
    };
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
    write_merge_repair_run_record(
        paths,
        plan,
        context,
        &run_id,
        status,
        &repair_id,
        repair_round,
        &repair_request_sha256,
        &repair_plan_sha256,
        inherited_sandbox,
        repair_plan.planner_spend_usd,
        repair_plan.planner_wall_seconds,
    )?;
    if !output.status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "repair child failed: {stdout}{stderr}"
        ))));
    }
    let library = validate_merge_repair_child(
        paths,
        plan,
        context,
        &run_id,
        &repair_id,
        repair_round,
        &repair_request_sha256,
        &repair_plan_sha256,
        inherited_sandbox,
    )?;
    copy_repair_library_to_working(&context.working_dir, &library)?;
    Ok(run_id)
}

#[allow(clippy::too_many_arguments)]
fn merge_repair_launch_record(
    plan: &Plan,
    context: &MergeRepairContext,
    repair_id: &str,
    repair_round: u32,
    run_id: &str,
    repair_request_sha256: &str,
    repair_plan_sha256: &str,
    sandbox_requested: deadreckon_sandbox::SandboxBackend,
    capability_id: Option<&str>,
    planner_spend_usd: f64,
    planner_wall_seconds: f64,
) -> Result<Value> {
    let path = context.proof_dir.join("repair-run.json");
    if path.exists() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair launch already has durable authority; refusing a duplicate child"
                .to_string(),
        )));
    }
    Ok(json!({
        "schema_version": 2,
        "plan_id": &plan.plan_id,
        "root_artifact_id": commands::graph_job::current_parent_job_id()
            .unwrap_or(&plan.plan_id),
        "repair_id": repair_id,
        "repair_round": repair_round,
        "repair_request_sha256": repair_request_sha256,
        "repair_plan_sha256": repair_plan_sha256,
        "capability_id": capability_id,
        "run_id": run_id,
        "status": "launch_prepared",
        "sandbox_requested": sandbox_requested.to_string(),
        "planner_spend_usd": planner_spend_usd,
        "planner_wall_seconds": planner_wall_seconds,
        "source": &context.working_dir,
        "created_at": Utc::now(),
        "updated_at": Utc::now(),
    }))
}

#[allow(clippy::too_many_arguments)]
async fn recover_existing_merge_repair_child(
    paths: &DeadreckonPaths,
    plan: &Plan,
    context: &MergeRepairContext,
    repair_id: &str,
    repair_round: u32,
    repair_request_sha256: &str,
    repair_plan_sha256: &str,
    sandbox_requested: deadreckon_sandbox::SandboxBackend,
    remaining_wall_seconds: Option<f64>,
) -> Result<Option<(String, PathBuf)>> {
    let path = context.proof_dir.join("repair-run.json");
    if commands::graph_job::current_parent_job_id().is_some() {
        commands::graph_job::restore_merge_repair_projection_if_needed(paths, repair_id, &path)?;
    }
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(_) => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "persisted merge repair authority is not a regular file".to_string(),
            )));
        }
        Err(error) => return Err(error.into()),
    }
    let wait_seconds = remaining_wall_seconds.unwrap_or(120.0).clamp(0.25, 120.0);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs_f64(wait_seconds);
    loop {
        let record: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if record.get("repair_id").and_then(Value::as_str) != Some(repair_id)
            || record.get("repair_round").and_then(Value::as_u64) != Some(u64::from(repair_round))
            || record.get("repair_request_sha256").and_then(Value::as_str)
                != Some(repair_request_sha256)
            || record.get("repair_plan_sha256").and_then(Value::as_str) != Some(repair_plan_sha256)
            || record.get("sandbox_requested").and_then(Value::as_str)
                != Some(sandbox_requested.to_string().as_str())
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "persisted merge repair authority changed immutable request identity".to_string(),
            )));
        }
        let run_id = record
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|run_id| !run_id.trim().is_empty())
            .map(str::to_string);
        if let Some(run_id) = run_id.as_deref()
            && let Ok(library) = validate_merge_repair_child(
                paths,
                plan,
                context,
                run_id,
                repair_id,
                repair_round,
                repair_request_sha256,
                repair_plan_sha256,
                sandbox_requested,
            )
        {
            return Ok(Some((run_id.to_string(), library)));
        }
        let process: deadreckon_core::SupervisedProcessRecord =
            serde_json::from_value(record.get("process").cloned().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "persisted merge repair authority has no supervised child identity".to_string(),
                ))
            })?)?;
        match process.identity() {
            deadreckon_core::SupervisedProcessIdentity::Current => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(
                        "timed out adopting the exact supervised merge repair child".to_string(),
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            deadreckon_core::SupervisedProcessIdentity::Exited
            | deadreckon_core::SupervisedProcessIdentity::DifferentBoot => {
                let Some(run_id) = run_id else {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(
                        "supervised merge repair child exited before linking a Run; refusing a duplicate launch"
                            .to_string(),
                    )));
                };
                let library = validate_merge_repair_child(
                    paths,
                    plan,
                    context,
                    &run_id,
                    repair_id,
                    repair_round,
                    repair_request_sha256,
                    repair_plan_sha256,
                    sandbox_requested,
                )?;
                return Ok(Some((run_id, library)));
            }
            deadreckon_core::SupervisedProcessIdentity::Reused
            | deadreckon_core::SupervisedProcessIdentity::Unverifiable => {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "supervised merge repair child identity is conflicting or unverifiable"
                        .to_string(),
                )));
            }
        }
    }
}

fn write_merge_repair_run_record(
    paths: &DeadreckonPaths,
    plan: &Plan,
    context: &MergeRepairContext,
    run_id: &str,
    status: &str,
    repair_id: &str,
    repair_round: u32,
    repair_request_sha256: &str,
    repair_plan_sha256: &str,
    sandbox_requested: deadreckon_sandbox::SandboxBackend,
    planner_spend_usd: f64,
    planner_wall_seconds: f64,
) -> Result<()> {
    let path = context.proof_dir.join("repair-run.json");
    let state = load_run(paths, run_id).ok();
    let mut value: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let object = value.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "merge repair launch authority is not an object".to_string(),
        ))
    })?;
    if object.get("plan_id").and_then(Value::as_str) != Some(plan.plan_id.as_str())
        || object.get("repair_id").and_then(Value::as_str) != Some(repair_id)
        || object.get("repair_round").and_then(Value::as_u64) != Some(u64::from(repair_round))
        || object.get("repair_request_sha256").and_then(Value::as_str)
            != Some(repair_request_sha256)
        || object.get("repair_plan_sha256").and_then(Value::as_str) != Some(repair_plan_sha256)
        || object.get("sandbox_requested").and_then(Value::as_str)
            != Some(sandbox_requested.to_string().as_str())
        || object
            .get("run_id")
            .and_then(Value::as_str)
            .is_some_and(|existing| existing != run_id)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair completion crossed its durable launch authority".to_string(),
        )));
    }
    object.insert("run_id".to_string(), Value::String(run_id.to_string()));
    object.insert(
        "scope".to_string(),
        serde_json::to_value(state.as_ref().map(|state| state.scope.clone()))?,
    );
    object.insert("status".to_string(), Value::String(status.to_string()));
    object.insert(
        "planner_spend_usd".to_string(),
        serde_json::to_value(planner_spend_usd)?,
    );
    object.insert(
        "planner_wall_seconds".to_string(),
        serde_json::to_value(planner_wall_seconds)?,
    );
    object.insert("updated_at".to_string(), serde_json::to_value(Utc::now())?);
    commands::job::replace_json_synced(&path, &value)
}

#[allow(clippy::too_many_arguments)]
fn validate_merge_repair_child(
    paths: &DeadreckonPaths,
    plan: &Plan,
    context: &MergeRepairContext,
    run_id: &str,
    repair_id: &str,
    repair_round: u32,
    repair_request_sha256: &str,
    repair_plan_sha256: &str,
    sandbox_requested: deadreckon_sandbox::SandboxBackend,
) -> Result<PathBuf> {
    if sandbox_requested == deadreckon_sandbox::SandboxBackend::None
        || deadreckon_core::flight::sha256_file(&context.proof_dir.join("repair-request.json"))?
            != repair_request_sha256
        || deadreckon_core::flight::sha256_file(&context.proof_dir.join("repair-plan.json"))?
            != repair_plan_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair proof changed before parent consumption".to_string(),
        )));
    }
    let state = load_run(paths, run_id)?;
    let expected_job_id = commands::graph_job::current_parent_job_id();
    if state.sandbox != sandbox_requested.to_string()
        || expected_job_id.is_some_and(|job_id| {
            state.ownership.as_ref()
                != Some(&deadreckon_core::RunOwnership::merge_repair(
                    job_id,
                    job_id,
                    repair_id,
                    repair_round,
                    run_id,
                    fs::canonicalize(&context.proof_dir)
                        .unwrap_or_else(|_| context.proof_dir.clone()),
                    repair_request_sha256,
                    repair_plan_sha256,
                ))
        })
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "repair Run {} is not owned by its exact durable parent request",
            run_prefix(run_id)
        ))));
    }
    let marker = validate_acceptance_marker(&state)?;
    if !marker.is_native_gate_proof() || !marker.contained || marker.sandbox_backend == "none" {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "repair Run {} has no signed contained native gate proof",
            run_prefix(run_id)
        ))));
    }
    let library = paths.library_dir(&state.scope, &state.run_id);
    if !library.is_dir() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("repair run {} has no promoted library", run_prefix(run_id)),
            &format!("deadreckon attach {}", run_prefix(run_id)),
        )));
    }
    let result_tree_sha256 =
        deadreckon_core::flight::build_deliverable_file_index(&library)?.tree_hash();
    let path = context.proof_dir.join("repair-run.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let object = record.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "merge repair record is not an object".to_string(),
        ))
    })?;
    if object.get("run_id").and_then(Value::as_str) != Some(run_id)
        || object.get("repair_id").and_then(Value::as_str) != Some(repair_id)
        || object.get("repair_round").and_then(Value::as_u64) != Some(u64::from(repair_round))
        || object.get("repair_request_sha256").and_then(Value::as_str)
            != Some(repair_request_sha256)
        || object.get("repair_plan_sha256").and_then(Value::as_str) != Some(repair_plan_sha256)
        || object.get("plan_id").and_then(Value::as_str) != Some(plan.plan_id.as_str())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair record crosses immutable request identities".to_string(),
        )));
    }
    object.insert(
        "acceptance_marker_sha256".to_string(),
        Value::String(deadreckon_core::flight::sha256_file(
            &deadreckon_core::marker_path_for_run_root(&state.run_root),
        )?),
    );
    object.insert(
        "result_tree_sha256".to_string(),
        Value::String(result_tree_sha256),
    );
    object.insert("trusted".to_string(), Value::Bool(true));
    object.insert(
        "validated_at".to_string(),
        serde_json::to_value(Utc::now())?,
    );
    commands::job::replace_json_synced(&path, &record)?;
    Ok(library)
}

fn copy_repair_library_to_working(working_dir: &Path, library: &Path) -> Result<()> {
    let library_files = build_deliverable_file_index(library)?.files;
    remove_if_exists(working_dir)?;
    fs::create_dir_all(working_dir)?;
    for (relative, _) in library_files {
        if skip_plan_merge_file(&relative) {
            continue;
        }
        copy_merge_file(&library.join(&relative), &working_dir.join(relative))?;
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
    !deadreckon_core::is_deliverable_workspace_path(relative)
        || relative == Path::new("manifest.json")
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

fn copy_merge_file(source: &Path, dest: &Path) -> Result<()> {
    copy_artifact_path(source, dest).map_err(CliError::from)
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
    let run_options = RunOptions {
        // The merged run IS the delivered result of the operator's goal, so it
        // carries that goal verbatim. Prefixing an internal label ("merge
        // orchestration plan …") leaked into the status goal line and, via
        // task_key, into the suggested export dir (./merge-orchestration-plan).
        goal: plan.root_goal.clone(),
        cwd,
        sandbox: "none".to_string(),
        provider: None,
        skill_name: "default-coding".to_string(),
        max_spend_usd: None,
        max_wall_seconds: None,
        run_id: None,
        codebase: None,
    };
    let mut state = if let Some(owner) = commands::graph_job::resolve_plan_owner(paths, plan)? {
        deadreckon_core::create_owned_run(
            paths,
            run_options,
            deadreckon_core::RunOwnership::plan_result(owner.job.job_id.as_ref(), &plan.plan_id),
        )?
    } else {
        create_run(paths, run_options)?
    };
    remove_if_exists(&state.working_dir)?;
    copy_deliverable_tree(&paths.merge_working(&plan.plan_id), &state.working_dir)?;
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
    let merged_files = build_deliverable_file_index(merged_source)?.files;
    clear_plan_result_deliverables(worktree_path)?;
    for (relative, _) in merged_files {
        if skip_plan_apply_file(&relative) {
            continue;
        }
        copy_merge_file(
            &merged_source.join(&relative),
            &worktree_path.join(relative),
        )?;
    }
    materialize_plan_docs_to_working(paths, plan, worktree_path, None)?;

    stage_plan_result_changes(worktree_path)?;
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

fn clear_plan_result_deliverables(worktree_path: &Path) -> Result<()> {
    for relative in build_deliverable_file_index(worktree_path)?.files.keys() {
        remove_artifact_path(&worktree_path.join(relative))?;
    }
    Ok(())
}

fn stage_plan_result_changes(worktree_path: &Path) -> Result<()> {
    let mut paths = BTreeSet::new();
    paths.extend(git_nul_paths(
        worktree_path,
        &["diff", "--name-only", "--no-renames", "-z"],
    )?);
    paths.extend(git_nul_paths(
        worktree_path,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?);

    for relative in paths
        .iter()
        .filter(|relative| is_plan_committable_path(relative))
    {
        let pathspec = format!(":(literal){}", path_to_str(relative)?);
        git_status(worktree_path, &["add", "-A", "--", &pathspec])?;
    }

    let inadmissible = git_nul_paths(
        worktree_path,
        &["diff", "--cached", "--name-only", "--no-renames", "-z"],
    )?
    .into_iter()
    .filter(|relative| !is_plan_committable_path(relative))
    .collect::<Vec<_>>();
    if !inadmissible.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "plan result staging contains non-deliverable paths: {}",
                inadmissible
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "remove the staged provider/runtime artifacts and retry",
        )));
    }
    Ok(())
}

fn git_nul_paths(cwd: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    let output = deadreckon_core::git::run_git(cwd, args)?;
    if !output.status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path).map(PathBuf::from).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "git reported a path that is not valid UTF-8".to_string(),
                ))
            })
        })
        .collect()
}

fn is_plan_committable_path(relative: &Path) -> bool {
    deadreckon_core::is_deliverable_workspace_path(relative)
}

#[cfg(test)]
mod plan_artifact_boundary_tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{
        clear_plan_result_deliverables, compose_roots, copy_repair_library_to_working,
        git_nul_paths, git_status, stage_plan_result_changes,
    };

    fn write_file(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, body).expect("write");
    }

    #[test]
    fn plan_composition_ignores_provider_and_runtime_artifacts() {
        let temp = TempDir::new().expect("tempdir");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        write_file(&first, "src/first.rs", "first\n");
        write_file(&second, "src/second.rs", "second\n");
        write_file(&first, ".specstory/history/session.md", "private first\n");
        write_file(&second, ".specstory/history/session.md", "private second\n");
        write_file(&first, "target/debug/output", "runtime first\n");
        write_file(&second, "target/debug/output", "runtime second\n");
        write_file(&first, ".deadreckon/codebase.json", "{}\n");
        write_file(&second, ".deadreckon/codebase.json", "{}\n");

        let merge = temp.path().join("merge");
        let result = compose_roots(
            &[("first".to_string(), first), ("second".to_string(), second)],
            &merge,
        )
        .expect("compose");

        assert!(result.conflicts.is_empty());
        assert!(merge.join("src/first.rs").is_file());
        assert!(merge.join("src/second.rs").is_file());
        assert!(!merge.join(".specstory").exists());
        assert!(!merge.join("target").exists());
        assert!(!merge.join(".deadreckon").exists());
    }

    #[cfg(unix)]
    #[test]
    fn plan_composition_preserves_executable_and_symlink_identity() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = TempDir::new().expect("tempdir");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        write_file(&first, "bin/run", "#!/bin/sh\n");
        fs::set_permissions(first.join("bin/run"), fs::Permissions::from_mode(0o755))
            .expect("executable mode");
        symlink("run", first.join("bin/current")).expect("source symlink");
        write_file(&second, "config/settings.toml", "enabled = true\n");

        let merge = temp.path().join("merge");
        let result = compose_roots(
            &[("first".to_string(), first), ("second".to_string(), second)],
            &merge,
        )
        .expect("compose");

        assert!(result.conflicts.is_empty());
        assert_ne!(
            fs::metadata(merge.join("bin/run"))
                .expect("merged executable")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert!(
            fs::symlink_metadata(merge.join("bin/current"))
                .expect("merged symlink")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(merge.join("bin/current")).expect("raw symlink target"),
            Path::new("run")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plan_composition_treats_mode_and_kind_changes_as_conflicts() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = TempDir::new().expect("tempdir");
        let regular = temp.path().join("regular");
        let executable = temp.path().join("executable");
        write_file(&regular, "bin/run", "same bytes\n");
        write_file(&executable, "bin/run", "same bytes\n");
        fs::set_permissions(regular.join("bin/run"), fs::Permissions::from_mode(0o644))
            .expect("regular mode");
        fs::set_permissions(
            executable.join("bin/run"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("executable mode");

        let mode_result = compose_roots(
            &[
                ("regular".to_string(), regular),
                ("executable".to_string(), executable),
            ],
            &temp.path().join("mode-merge"),
        )
        .expect("mode compose");
        assert_eq!(mode_result.conflicts.len(), 1);
        assert_eq!(mode_result.conflicts[0].path, Path::new("bin/run"));

        let file_root = temp.path().join("file-root");
        let link_root = temp.path().join("link-root");
        write_file(&file_root, "current", "release\n");
        fs::create_dir_all(&link_root).expect("link root");
        symlink("release", link_root.join("current")).expect("kind symlink");

        let kind_result = compose_roots(
            &[
                ("file".to_string(), file_root),
                ("link".to_string(), link_root),
            ],
            &temp.path().join("kind-merge"),
        )
        .expect("kind compose");
        assert_eq!(kind_result.conflicts.len(), 1);
        assert_eq!(kind_result.conflicts[0].path, Path::new("current"));
    }

    #[test]
    fn plan_composition_rejects_hierarchy_collisions_before_copying() {
        let temp = TempDir::new().expect("tempdir");
        let ancestor = temp.path().join("ancestor");
        let descendant = temp.path().join("descendant");
        write_file(&ancestor, "config", "single file\n");
        write_file(&descendant, "config/settings.toml", "nested file\n");
        let merge = temp.path().join("merge");
        write_file(&merge, "sentinel.txt", "must remain\n");

        let error = compose_roots(
            &[
                ("ancestor".to_string(), ancestor),
                ("descendant".to_string(), descendant),
            ],
            &merge,
        )
        .expect_err("hierarchy collision must fail");

        assert!(
            error
                .to_string()
                .contains("plan artifact hierarchy collision"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(merge.join("sentinel.txt")).expect("sentinel"),
            "must remain\n"
        );
        assert!(!merge.join("config").exists());
    }

    #[cfg(unix)]
    #[test]
    fn repair_library_copy_preserves_identity_and_omits_private_artifacts() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = TempDir::new().expect("tempdir");
        let library = temp.path().join("library");
        let working = temp.path().join("working");
        write_file(&library, "bin/run", "#!/bin/sh\n");
        fs::set_permissions(library.join("bin/run"), fs::Permissions::from_mode(0o755))
            .expect("executable mode");
        symlink("run", library.join("bin/current")).expect("library symlink");
        write_file(
            &library,
            ".specstory/history/session.md",
            "provider private\n",
        );

        copy_repair_library_to_working(&working, &library).expect("repair copy");

        assert_ne!(
            fs::metadata(working.join("bin/run"))
                .expect("copied executable")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert!(
            fs::symlink_metadata(working.join("bin/current"))
                .expect("copied symlink")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(working.join("bin/current")).expect("raw symlink target"),
            Path::new("run")
        );
        assert!(!working.join(".specstory").exists());
    }

    #[test]
    fn plan_apply_preserves_private_base_files_and_stages_only_result_artifacts() {
        let temp = TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        git_status(&repo, &["init", "-b", "main"]).expect("git init");
        git_status(
            &repo,
            &["config", "user.email", "deadreckon@example.invalid"],
        )
        .expect("git email");
        git_status(&repo, &["config", "user.name", "deadreckon"]).expect("git name");
        write_file(&repo, "old.txt", "old result\n");
        write_file(&repo, ".specstory/history/session.md", "private base\n");
        write_file(&repo, "target/debug/output", "runtime base\n");
        write_file(&repo, ".deadreckon/codebase.json", "{}\n");
        git_status(&repo, &["add", "-A"]).expect("baseline add");
        git_status(&repo, &["commit", "-m", "baseline"]).expect("baseline commit");

        clear_plan_result_deliverables(&repo).expect("clear deliverables");
        assert!(!repo.join("old.txt").exists());
        assert!(repo.join(".specstory/history/session.md").is_file());
        assert!(repo.join("target/debug/output").is_file());
        assert!(repo.join(".deadreckon/codebase.json").is_file());

        write_file(&repo, "result.txt", "new result\n");
        write_file(&repo, ".specstory/history/session.md", "private mutation\n");
        write_file(
            &repo,
            ".deadreckon/docs/PLAN-NARRATIVE.md",
            "approved lifecycle doc\n",
        );
        write_file(
            &repo,
            ".deadreckon/docs/.specstory/secret.md",
            "not an approved lifecycle doc\n",
        );

        stage_plan_result_changes(&repo).expect("stage result");

        let staged = git_nul_paths(
            &repo,
            &["diff", "--cached", "--name-only", "--no-renames", "-z"],
        )
        .expect("staged")
        .into_iter()
        .collect::<BTreeSet<PathBuf>>();
        assert_eq!(
            staged,
            BTreeSet::from([PathBuf::from("old.txt"), PathBuf::from("result.txt"),])
        );
        let unstaged =
            git_nul_paths(&repo, &["diff", "--name-only", "--no-renames", "-z"]).expect("unstaged");
        assert!(
            unstaged
                .iter()
                .any(|path| path == Path::new(".specstory/history/session.md"))
        );
        assert!(!staged.iter().any(|path| {
            path.starts_with(".specstory")
                || path.starts_with(".deadreckon")
                || path.starts_with("target")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn plan_result_cleanup_removes_symlink_deliverables_without_following() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let worktree = temp.path().join("worktree");
        let outside = temp.path().join("outside.txt");
        fs::create_dir_all(&worktree).expect("worktree");
        fs::write(&outside, "operator-owned\n").expect("outside");
        symlink(&outside, worktree.join("result-link")).expect("deliverable symlink");
        write_file(
            &worktree,
            ".specstory/history/session.md",
            "private evidence\n",
        );

        clear_plan_result_deliverables(&worktree).expect("clear result");

        assert!(
            fs::symlink_metadata(worktree.join("result-link")).is_err(),
            "deliverable symlink should be removed"
        );
        assert_eq!(
            fs::read_to_string(&outside).expect("outside survives"),
            "operator-owned\n"
        );
        assert!(worktree.join(".specstory/history/session.md").is_file());
    }
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
    let id = run_prefix(&plan.plan_id);
    let finish = format!("deadreckon finish {id}");
    let mut secondary = Vec::new();
    if plan_apply_git_root(plan).ok().flatten().is_some() {
        secondary.push(format!("deadreckon apply {id}"));
    }
    secondary.push(format!(
        "deadreckon export {} --dest ./{}",
        id,
        dest_slug(&deadreckon_core::paths::task_key(&plan.root_goal))
    ));
    secondary.push(format!("deadreckon show {id}"));
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    println!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "plan",
            Some(&id),
            ExplanationPanel::new(
                "DeadReckon merged the child artifacts into a promoted result run.",
                "The plan now has a completed result; finish is the canonical next command for landing or exporting it.",
                vec![
                    ("plan".to_string(), id.clone()),
                    ("result run".to_string(), run_prefix(&merged_run.run_id)),
                    (
                        "artifact library".to_string(),
                        library_dir.display().to_string(),
                    ),
                    ("status".to_string(), plan_status_label(plan.status).to_string()),
                    (
                        "tasks".to_string(),
                        format!("{completed}/{} completed", plan.tasks.len()),
                    ),
                ],
            ),
            vec![("Recommended", finish.as_str())],
            secondary
                .iter()
                .map(|command| ("Secondary", command.as_str())),
        )
        .render_plain(!completion_hints_enabled(no_hints))
    );
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
}

fn print_plan_summary(paths: &DeadreckonPaths, plan: &Plan, show_hints: bool) {
    print!(
        "{}",
        commands::plan::plan_verdict_surface(paths, plan).render_plain(!show_hints)
    );
    println!();
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
    print_spine_plain_lines(crate::tui::spine::spine_plain_lines(
        &crate::tui::spine::spine_for_plan(paths, plan, Utc::now()),
    ));
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
            println!("    child actions");
            println!(
                "      {}",
                ui_command(format!("deadreckon attach {child_ref}"))
            );
            println!(
                "      {}",
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

/// Humanize a wall-clock duration for budget displays: `42s`, `23m`, `1h 5m`.
fn human_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if total >= 60 {
        let minutes = total / 60;
        let secs = total % 60;
        if secs == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {secs}s")
        }
    } else {
        format!("{total}s")
    }
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

/// Pick a provider with a menu that already knows the machine: detected
/// subscription CLIs lead (probed for login state), API routes show whether a
/// key is already in the environment, and a free-input escape hatch keeps
/// every route reachable. Probe-before-ask: the detection is in the hints,
/// the best candidate is preselected.
fn prompt_provider() -> Result<String> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let mut choices = provider_picker_choices(&registry);
    choices.push(prompt::SelectChoice::with_detail(
        "other",
        "Type another provider route",
        "for example a providers.d descriptor id",
    ));
    let selection = prompt::select_one(&prompt::SelectPrompt {
        title: "Choose a provider".to_string(),
        help: Some(
            "detected agent CLIs first; everything stays changeable via deadreckon config provider"
                .to_string(),
        ),
        choices,
        default_index: 0,
    })?;
    if selection.id == "other" {
        let typed = prompt::open("provider route: ", None)?;
        if typed.trim().is_empty() {
            return Ok(auto_subscription_cli_provider(&registry)
                .unwrap_or_else(|| "anthropic".to_string()));
        }
        return Ok(typed.trim().to_string());
    }
    Ok(selection.id)
}

/// The probe-before-ask provider menu shared by init's picker and the launch
/// rescue: detected subscription CLIs first (live login-state hints), then
/// API routes annotated with whether their key is already exported.
pub(crate) fn provider_picker_choices(registry: &ProviderRegistry) -> Vec<prompt::SelectChoice> {
    let mut choices = Vec::new();
    for descriptor in registry.iter() {
        if descriptor.kind != deadreckon_providers::registry::DescriptorKind::Cli
            || !descriptor.subscription
            || !setup::provider_route_is_auto_selectable(&descriptor.id)
        {
            continue;
        }
        let Some(binary) = descriptor.default_binary.as_deref() else {
            continue;
        };
        if !command_exists(binary) {
            continue;
        }
        let detail = match descriptor
            .auth_probe
            .as_ref()
            .map(|probe| deadreckon_providers::probe_cli_auth(binary, probe))
        {
            Some(deadreckon_providers::CliAuthStatus::LoggedIn) => {
                "installed, logged in".to_string()
            }
            Some(deadreckon_providers::CliAuthStatus::NotLoggedIn { .. }) => {
                format!("installed, not logged in (run `{binary} login` before starting)")
            }
            _ => "installed".to_string(),
        };
        choices.push(prompt::SelectChoice::with_detail(
            &descriptor.id,
            format!("{} ({})", descriptor.display_name, descriptor.id),
            detail,
        ));
    }
    for (id, label, env_key) in [
        ("anthropic", "Anthropic API", "ANTHROPIC_API_KEY"),
        ("openai", "OpenAI API", "OPENAI_API_KEY"),
    ] {
        let detail = if std::env::var_os(env_key).is_some() {
            format!("{env_key} found in environment")
        } else {
            format!("needs an API key ({env_key} or typed next)")
        };
        choices.push(prompt::SelectChoice::with_detail(
            id,
            format!("{label} ({id})"),
            detail,
        ));
    }
    choices.push(prompt::SelectChoice::with_detail(
        "openai-compatible",
        "OpenAI-compatible endpoint (openai-compatible)",
        "any local or hosted compatible API (base URL + key)",
    ));
    choices
}

enum NonGitChoice {
    Init,
    Copy,
    Cancel,
}

fn prompt_non_git_mode() -> Result<NonGitChoice> {
    let selection = prompt::select_one(&prompt::SelectPrompt {
        title: "No git repo here".to_string(),
        help: Some(
            "DeadReckon needs either a git worktree or a disposable copy before it lets an agent edit files."
                .to_string(),
        ),
        choices: vec![
            prompt::SelectChoice::with_detail(
                "init",
                "Initialize git and use worktree mode (recommended)",
                "adds .git here, then runs the agent in an isolated deadreckon worktree",
            ),
            prompt::SelectChoice::with_detail(
                "copy",
                "Use copy mode",
                "leaves this folder alone; the agent works under ~/.deadreckon/runstate/",
            ),
            prompt::SelectChoice::new("cancel", "Cancel"),
        ],
        default_index: 0,
    })?;
    Ok(match selection.id.as_str() {
        "init" => NonGitChoice::Init,
        "copy" => NonGitChoice::Copy,
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
    let mut printed = false;
    loop {
        tokio::select! {
            result = &mut future => {
                if printed {
                    finish_cli_wait_status();
                } else {
                    clear_cli_wait_status();
                }
                return result;
            }
            _ = interval.tick() => {
                tick = tick.wrapping_add(1);
                printed = true;
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
    show_secondary_hints: bool,
) {
    print!(
        "{}",
        render_exit_summary_card(state, outcome, plain, show_secondary_hints)
    );
}

fn render_exit_summary_card(
    state: &deadreckon_core::PipelineState,
    outcome: &RunLoopOutcome,
    plain: bool,
    show_secondary_hints: bool,
) -> String {
    let input = exit_summary_input(state, outcome);
    let mut card = build_exit_summary_card(&input);
    if !show_secondary_hints {
        card.hints.clear();
    }
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
        .find(|(label, _)| matches!(label.as_str(), "finish" | "apply" | "export" | "undo"))
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

fn run_is_subscription_only(state: &deadreckon_core::PipelineState) -> bool {
    let Ok(summary) = deadreckon_core::state::spend_summary(state) else {
        return false;
    };
    let turns = summary.turns.max(state.turn as usize);
    turns > 0
        && summary.subscription_turns == summary.turns
        && summary.total_usd == 0.0
        && (summary.any_subscription_turn || !provider_is_metered(state))
}

/// Render CLI token usage for a run summary, or `None` when no tokens were
/// recorded (older CLI turns that reported 0/0 before Semaphore, or non-CLI
/// runs). Shown on subscription surfaces where dollars stay $0.
fn cli_token_usage_label(input_tokens: u64, output_tokens: u64) -> Option<String> {
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    Some(format!("{input_tokens} in / {output_tokens} out tokens"))
}

#[cfg(test)]
mod semaphore_friendliness_tests {
    use super::cli_token_usage_label;

    #[test]
    fn show_renders_cli_token_usage_for_both_providers() {
        // codex-like and claude-like real token counts both render on the
        // subscription surface where dollars stay $0.
        assert_eq!(
            cli_token_usage_label(100, 40).as_deref(),
            Some("100 in / 40 out tokens")
        );
        assert_eq!(
            cli_token_usage_label(2, 4).as_deref(),
            Some("2 in / 4 out tokens")
        );
        // Pre-Semaphore 0/0 turns render nothing rather than a misleading zero.
        assert!(cli_token_usage_label(0, 0).is_none());
    }

    #[test]
    fn show_and_report_render_descriptor_provider_tokens() {
        // Descriptor providers use the same spend summary renderer as the
        // bespoke CLI mirrors: Pi reports both sides, while Copilot 1.0.45
        // currently exposes output tokens only.
        assert_eq!(
            cli_token_usage_label(403, 25).as_deref(),
            Some("403 in / 25 out tokens")
        );
        assert_eq!(
            cli_token_usage_label(0, 26).as_deref(),
            Some("0 in / 26 out tokens")
        );
    }
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
        // Subscription runs are billed in time, not dollars: lead with the
        // wall-time budget instead of a $0.00 the harness knows is false. Since
        // Semaphore, CLI turns report real tokens, so show them when present.
        let provider = state.provider.as_deref().unwrap_or("subscription CLI");
        let mut label = format!(
            "not metered (subscription) · {} of {provider} · {} turns",
            human_duration(wall_seconds),
            turns
        );
        if let Some(tokens) = cli_token_usage_label(summary.input_tokens, summary.output_tokens) {
            label.push_str(" · ");
            label.push_str(&tokens);
        }
        return label;
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
                    if let Some(worktree) =
                        codebase.and_then(|record| record.worktree_path.as_ref())
                    {
                        hints.push((
                            "inspect".to_string(),
                            format!("cd {} && git status", worktree.display()),
                        ));
                    }
                    hints.push((
                        "finish".to_string(),
                        format!("deadreckon finish {prefix} --autostash --cleanup"),
                    ));
                    hints.push(("apply".to_string(), format!("deadreckon apply {prefix}")));
                    hints.push((
                        "cleanup".to_string(),
                        format!("deadreckon apply {prefix} --autostash --cleanup"),
                    ));
                    hints.push((
                        "cleanup".to_string(),
                        format!("deadreckon cleanup {prefix}"),
                    ));
                    hints.push((
                        "docs".to_string(),
                        format!("deadreckon doc {prefix} --kind decisions"),
                    ));
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

fn finish_cli_wait_status() {
    let _ = ui::clear_current_line(ui::Stream::Stderr);
    eprintln!();
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
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\ndoc_provider = \"{provider}\"\ndoc_skill = \"run-narrator\"\ndoc_subskills = [\"narrator-overview\", \"narrator-phases\", \"narrator-as-built\", \"narrator-decisions\"]\ndoc_polish_token_budget = 16384\nmax_spend = {max_spend}\ncli_max_wall_seconds = 36000\nprevent_sleep = \"auto\"\nplain = false\nsandbox = \"{sandbox}\"\n\n"
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

fn toml_path_string_equals(root: &toml::Value, key: &str, expected: &str) -> bool {
    get_toml_path(root, key).and_then(toml::Value::as_str) == Some(expected)
}

fn remove_toml_path(root: &mut toml::Value, key: &str) -> bool {
    let parts = key.split('.').collect::<Vec<_>>();
    let Some((last, parents)) = parts.split_last() else {
        return false;
    };
    let mut cursor = root;
    for part in parents {
        let Some(next) = cursor.as_table_mut().and_then(|table| table.get_mut(*part)) else {
            return false;
        };
        cursor = next;
    }
    cursor
        .as_table_mut()
        .and_then(|table| table.remove(*last))
        .is_some()
}

fn remove_toml_path_if_string(root: &mut toml::Value, key: &str, expected: &str) -> bool {
    if !toml_path_string_equals(root, key, expected) {
        return false;
    }
    remove_toml_path(root, key)
}

fn remove_provider_from_fallback(root: &mut toml::Value, provider: &str) -> usize {
    let Some(fallback) = get_toml_path_mut(root, "fallback").and_then(toml::Value::as_array_mut)
    else {
        return 0;
    };
    let before = fallback.len();
    fallback.retain(|item| item.as_str() != Some(provider));
    let removed = before.saturating_sub(fallback.len());
    if fallback.is_empty() {
        remove_toml_path(root, "fallback");
    }
    removed
}

fn remove_provider_table(root: &mut toml::Value, provider: &str) -> bool {
    let Some(root_table) = root.as_table_mut() else {
        return false;
    };
    let Some(providers) = root_table
        .get_mut("providers")
        .and_then(toml::Value::as_table_mut)
    else {
        return false;
    };
    let removed = providers.remove(provider).is_some();
    if providers.is_empty() {
        root_table.remove("providers");
    }
    removed
}

fn get_toml_path_mut<'a>(root: &'a mut toml::Value, key: &str) -> Option<&'a mut toml::Value> {
    let mut cursor = root;
    for part in key.split('.') {
        cursor = cursor.as_table_mut()?.get_mut(part)?;
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

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(CliError::from),
        Ok(_) => fs::remove_file(path).map_err(CliError::from),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn absolute_dest(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = lexical_normalize_path(&absolute);
    resolve_nearest_existing_ancestor(&normalized)
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::RootDir
            | std::path::Component::Prefix(_)
            | std::path::Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn resolve_nearest_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "destination {} has no resolvable ancestor",
                        path.display()
                    )))
                })?;
            }
            Err(source) => return Err(CliError::Io(source)),
        }
    }

    let unresolved = path.strip_prefix(existing).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "destination {} could not be resolved safely",
            path.display()
        )))
    })?;
    let resolved = fs::canonicalize(existing).map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "destination alias {} could not be resolved safely: {source}",
            existing.display()
        )))
    })?;
    Ok(resolved.join(unresolved))
}

fn resolve_external_dest(
    paths: &DeadreckonPaths,
    dest: Option<PathBuf>,
    verb: &str,
) -> Result<Option<PathBuf>> {
    dest.map(|dest| {
        let dest = absolute_dest(dest)?;
        refuse_dest_inside_home(paths, &dest, verb)?;
        Ok(dest)
    })
    .transpose()
}

fn refuse_dest_inside_home(paths: &DeadreckonPaths, dest: &Path, verb: &str) -> Result<()> {
    let home = fs::canonicalize(paths.home()).map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "DeadReckon home {} could not be resolved safely: {source}",
            paths.home().display()
        )))
    })?;
    if dest.starts_with(&home) || home.starts_with(dest) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "refusing to {verb} back into runstate (pick a path outside ~/.deadreckon/)"
        ))));
    }
    Ok(())
}

/// First non-empty line of a failed run's stored failure reason, for
/// bubbling into the plan- and campaign-level refusal surfaces that today
/// only report their own layer's status.
fn child_failure_reason(paths: &DeadreckonPaths, run_id: &str) -> Option<String> {
    let entry = deadreckon_core::state::list_runs(paths, None)
        .ok()?
        .into_iter()
        .find(|run| run.run_id == run_id)?;
    let state = deadreckon_core::state::load_state(&entry.state_path).ok()?;
    let reason = state.failure_reason.or(state.pause_reason)?;
    let line = reason.lines().find(|line| !line.trim().is_empty())?.trim();
    Some(if line.chars().count() > 200 {
        let mut clipped: String = line.chars().take(200).collect();
        clipped.push('…');
        clipped
    } else {
        line.to_string()
    })
}

/// A failure reason naming a provider session/usage/rate limit is resumable
/// once the limit resets — surface that instead of a generic failure.
fn provider_quota_note(reason: &str) -> Option<String> {
    let lower = reason.to_lowercase();
    let quota = ["session limit", "usage limit", "rate limit", "quota"]
        .iter()
        .any(|marker| lower.contains(marker));
    if !quota {
        return None;
    }
    let resets = reason
        .lines()
        .flat_map(|line| line.split('\u{b7}'))
        .map(str::trim)
        .find(|part| part.to_lowercase().starts_with("resets"));
    Some(match resets {
        Some(resets) => format!("provider limit hit; {resets}"),
        None => "provider limit hit; resumable after it resets".to_string(),
    })
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

fn current_scope() -> Result<String> {
    let cwd = std::env::current_dir()?;
    workspace_scope(&cwd).map_err(CliError::from)
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

fn codebase_mode_status(paths: &DeadreckonPaths, state: &deadreckon_core::PipelineState) -> String {
    read_run_codebase_record(paths, state)
        .map(|record| record.mode.to_string())
        .unwrap_or_else(|_| "-".to_string())
}

fn read_run_codebase_record(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<CodebaseRecord> {
    if paths.job_json(&state.run_id).is_file() {
        return deadreckon_core::read_trusted_codebase_record(&state.run_root)
            .map_err(CliError::from);
    }
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
    // Shakedown P7: `kill` probed campaign, run, plan, chain and silently missed
    // plan children, so `kill <plan>:task-0` -- an id `attach` accepts and
    // prints -- came back as "not found".
    let resolved = commands::reference::resolve_ref(
        &paths,
        commands::reference::RefQuery {
            reference: Some(&run_id),
            all_scopes: false,
            verb: "kill",
        },
    )?;
    let mut state = match resolved {
        commands::reference::ResolvedRef::Job(job) => {
            return commands::job::cancel_job(&paths, &job, force);
        }
        commands::reference::ResolvedRef::Campaign {
            dir: campaign_dir,
            campaign,
        } => {
            let mut campaign = *campaign;
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
            print!(
                "{}",
                commands::campaign::campaign_verdict_surface(Some(&paths), &campaign, None)
                    .render_plain(false)
            );
            return Ok(());
        }
        commands::reference::ResolvedRef::Plan(plan) => {
            if let Some(owner) = commands::graph_job::resolve_plan_owner(&paths, &plan)? {
                let view = deadreckon_core::JobView::load(&paths, owner.job.job_id.as_ref())?;
                return commands::job::cancel_job(&paths, &view, force);
            }
            return kill_plan_command(&paths, &plan.plan_id, force);
        }
        commands::reference::ResolvedRef::Chain(chain) => {
            return commands::chain::chain_kill_command(&paths, &chain.chain_id, force);
        }
        commands::reference::ResolvedRef::Run(state) => {
            if let Some(owner) = commands::graph_job::resolve_run_owner(&paths, &state)? {
                let view = deadreckon_core::JobView::load(&paths, owner.job.job_id.as_ref())?;
                return commands::job::cancel_job(&paths, &view, force);
            }
            *state
        }
        commands::reference::ResolvedRef::PlanChild { state, .. } => {
            if let Some(owner) = commands::graph_job::resolve_run_owner(&paths, &state)? {
                let view = deadreckon_core::JobView::load(&paths, owner.job.job_id.as_ref())?;
                return commands::job::cancel_job(&paths, &view, force);
            }
            *state
        }
    };
    kill_loaded_run(&paths, &mut state, force)?;
    print_exit_summary_card(&state, &RunLoopOutcome::Killed, plain, true);
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
    let id = run_prefix(plan_id);
    let mut evidence = vec![
        ("plan".to_string(), id.clone()),
        ("status".to_string(), "killed".to_string()),
        ("processes signalled".to_string(), killed.to_string()),
        (
            "events".to_string(),
            paths.plan_events(plan_id).display().to_string(),
        ),
    ];
    if force {
        evidence.push(("signal".to_string(), "escalated".to_string()));
    }
    let primary = format!("deadreckon show {id} --why-failed");
    let secondary = format!("deadreckon attach {id}");
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Killed,
            "plan",
            Some(&id),
            ExplanationPanel::new(
                "DeadReckon stopped the plan coordinator and any known live child work.",
                "The plan is no longer advancing; inspect the failure record before cleanup or relaunch.",
                evidence,
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .render_plain(false)
    );
    Ok(())
}

#[cfg(test)]
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
        RunEventKind::RunCompleted {
            status: "killed".to_string(),
        },
    )?;
    if !force && app_server_turn_active(state) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4500);
        while app_server_turn_active(state) && std::time::Instant::now() < deadline {
            // The runtime's cancel-marker watcher maps this request to
            // turn/interrupt and clears active_turn_id after Codex answers.
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
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

fn app_server_turn_active(state: &deadreckon_core::PipelineState) -> bool {
    if state.provider.as_deref() != Some("cli:codex-server") {
        return false;
    }
    let path = state.run_root.join("provider-session.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(session) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    session.get("route").and_then(serde_json::Value::as_str) == Some("cli:codex-server")
        && session
            .get("active_turn_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|turn| !turn.is_empty())
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
    let state = commands::reference::resolve_run_like(&paths, Some(&run_id), "resume")?;
    commands::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "resume")?;
    let _ = (from_turn, max_wall_seconds, no_docs, plain);
    Err(retired_public_resume_error(&state))
}

fn retired_public_resume_error(state: &deadreckon_core::PipelineState) -> CliError {
    let source = if state.working_dir.is_dir() {
        format!(
            "--from {}",
            quote_shell_command_argument(&state.working_dir.display().to_string())
        )
    } else {
        "--fresh".to_string()
    };
    let replacement = format!(
        "deadreckon start {} --mode run {source} --yes",
        quote_shell_command_argument(&state.goal)
    );
    CliError::Core(deadreckon_core::user_error(
        &format!(
            "public resume is retired for legacy Run {}; no Run state was changed and no provider was started",
            run_prefix(&state.run_id)
        ),
        &replacement,
    ))
}

fn quote_shell_command_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

async fn trusted_supervisor_resume_command(job_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let _authority = commands::supervisor::require_guarded_driver_launch(&paths, &job_id)?;
    let job = deadreckon_core::load_job(&paths, &job_id)?;
    if job.job_id.as_ref() != job_id || job.shape != deadreckon_protocol::JobShape::Single {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "trusted resume requires the exact single-leaf job identity".to_string(),
        )));
    }
    let state = deadreckon_core::load_run(&paths, &job_id)?;
    resume_loaded_command(&paths, state, None, None, true, true).await
}

async fn resume_loaded_command(
    paths: &DeadreckonPaths,
    state: deadreckon_core::PipelineState,
    from_turn: Option<u32>,
    max_wall_seconds: Option<f64>,
    no_docs: bool,
    plain: bool,
) -> Result<()> {
    resume_loaded_command_with_mode(
        paths,
        state,
        from_turn,
        max_wall_seconds,
        no_docs,
        plain,
        None,
        None,
    )
    .await
}

pub(crate) async fn resume_parent_repair_command(
    paths: &DeadreckonPaths,
    state: deadreckon_core::PipelineState,
    model: Option<&str>,
    candidate: deadreckon_runtime::ParentRepairCandidateContext,
) -> Result<()> {
    resume_loaded_command_with_mode(paths, state, None, None, true, true, model, Some(candidate))
        .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_loaded_command_with_mode(
    paths: &DeadreckonPaths,
    mut state: deadreckon_core::PipelineState,
    from_turn: Option<u32>,
    max_wall_seconds: Option<f64>,
    no_docs: bool,
    plain: bool,
    model: Option<&str>,
    parent_repair: Option<deadreckon_runtime::ParentRepairCandidateContext>,
) -> Result<()> {
    if state.status == RunStatus::Completed {
        let id = run_prefix(&state.run_id);
        let primary = format!("deadreckon show {id}");
        print!(
            "{}",
            VerdictSurface::must_new(
                VerdictKind::Noop,
                "run",
                Some(&id),
                ExplanationPanel::new(
                    "DeadReckon did not resume the run because it is already completed.",
                    "There is no incomplete provider turn to continue; inspect the completed record before starting follow-up work.",
                    vec![
                        ("run".to_string(), id.clone()),
                        ("status".to_string(), "completed".to_string()),
                        ("state".to_string(), state.run_root.display().to_string()),
                    ],
                ),
                vec![("Recommended", primary.as_str())],
                Vec::<(&str, &str)>::new(),
            )
            .render_plain(!completion_hints_enabled(false))
        );
        return Ok(());
    }
    let mut lock = acquire_lock(
        paths,
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
    let router = provider_router_for_run_with_catalog_seam(
        paths,
        &state,
        backend,
        provider.as_deref(),
        model,
        false,
    )
    .await?;
    let selected_route = router.selected_route_info();
    let defaults = config_defaults(paths)?;
    let doc_provider_selection = doc_provider_selection_from_setup(&doc_provider_setup_selection(
        paths,
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
    let config = RunLoopConfig {
        provider,
        max_spend_usd,
        max_wall_seconds,
        sandbox_backend: backend,
        no_seams: false,
        max_turns: 12,
        from_turn,
        event_sender: None,
        cancellation_token: None,
        narrate: None,
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
    };
    let outcome = if let Some(candidate) = parent_repair {
        with_cli_wait_status(
            &wait_label,
            deadreckon_runtime::run_parent_repair_turn_loop(&mut state, &router, config, candidate),
        )
        .await?
    } else {
        with_cli_wait_status(&wait_label, run_turn_loop(&mut state, &router, config)).await?
    };
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;
    print_exit_summary_card(&state, &outcome, plain, true);
    commands::lifecycle::fire_lifecycle_notification(paths, &state, &outcome).await;
    Ok(())
}

/// Put the last committed thing back, whatever kind of thing it was.
///
/// A run undoes to a turn snapshot; a chain unwinds its last applied step.
/// These were two commands (`undo --run` and `chain undo <id> --step`) with
/// two id spaces for one intent. The resolver already knew both kinds — only
/// this dispatch was missing.
fn undo_command(run: Option<&str>, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    // One resolution, one dispatch by kind — the same shape every other
    // id-taking verb uses.
    let resolved = commands::reference::resolve_ref(
        &paths,
        commands::reference::RefQuery {
            reference: run,
            all_scopes: false,
            verb: "undo",
        },
    )?;
    let state = match resolved {
        commands::reference::ResolvedRef::Chain(chain) => {
            return commands::chain::chain_undo_command(&paths, &chain.chain_id, turn, false);
        }
        commands::reference::ResolvedRef::Run(state) => *state,
        commands::reference::ResolvedRef::PlanChild { state, .. } => *state,
        other => {
            return Err(commands::reference::refusal_for(
                other.kind(),
                "undo",
                &commands::reference::resolved_id(&other),
            ));
        }
    };
    commands::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "undo")?;
    let target_turn = turn.unwrap_or_else(|| state.turn.saturating_sub(1));
    let restore_state = undo_restore_state(&paths, &state)?;
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
    let id = run_prefix(&state.run_id);
    let primary = format!("deadreckon show {id}");
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "undo",
            Some(&id),
            ExplanationPanel::new(
                format!("DeadReckon restored the run workspace to snapshot turn {target_turn}."),
                "The snapshot restore completed; inspect the run state before resuming or making another recovery move.",
                vec![
                    ("run".to_string(), id.clone()),
                    ("turn".to_string(), target_turn.to_string()),
                    (
                        "snapshot".to_string(),
                        restore_state
                            .run_root
                            .join("snapshots")
                            .join(format!("turn-{target_turn}"))
                            .display()
                            .to_string(),
                    ),
                    (
                        "workspace".to_string(),
                        restore_state.working_dir.display().to_string(),
                    ),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .render_plain(!completion_hints_enabled(false))
    );
    Ok(())
}

fn undo_restore_state(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    let record = match read_run_codebase_record(paths, state) {
        Ok(record) => record,
        Err(error) if paths.job_json(&state.run_id).is_file() => return Err(error),
        Err(_) => return Ok(state.clone()),
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
    let state = commands::reference::resolve_run_like(&paths, Some(run_id), "rewind")?;
    commands::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "rewind")?;
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

    let surface = rewind_verdict_surface(&state, mode, &resolved, &preview_dir, &files);

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(json!({
                "run_id": state.run_id,
                "mode": rewind_mode_label(mode),
                "target": resolved.target,
                "checkpoint_id": resolved.checkpoint_id,
                "preview_dir": preview_dir,
                "files": files,
            })))?
        );
        return Ok(());
    }

    print!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn rewind_verdict_surface(
    state: &deadreckon_core::PipelineState,
    mode: RewindMode,
    resolved: &ResolvedRewindTarget,
    preview_dir: &Path,
    files: &[PathBuf],
) -> VerdictSurface {
    let id = run_prefix(&state.run_id);
    let target = format!("{:?} {}", resolved.target.kind, resolved.target.id);
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        ("checkpoint".to_string(), resolved.checkpoint_id.clone()),
        ("target".to_string(), target),
        ("changed files".to_string(), files.len().to_string()),
        ("preview".to_string(), preview_dir.display().to_string()),
    ];
    for (index, path) in files.iter().take(5).enumerate() {
        evidence.push((format!("file {}", index + 1), path.display().to_string()));
    }
    if files.len() > 5 {
        evidence.push((
            "additional files".to_string(),
            (files.len() - 5).to_string(),
        ));
    }

    let (kind, what, why, primary, secondary) = match mode {
        RewindMode::Preview => (
            VerdictKind::Preview,
            format!(
                "DeadReckon materialized checkpoint {} into a preview directory without changing the run workspace.",
                resolved.checkpoint_id
            ),
            "This is a preview because the hash-guarded apply step has not run yet.".to_string(),
            format!(
                "deadreckon rewind {id} --to-checkpoint {} --apply",
                resolved.checkpoint_id
            ),
            format!("deadreckon show {id} --flight"),
        ),
        RewindMode::Apply => (
            VerdictKind::Completed,
            format!(
                "DeadReckon rewound the run workspace to checkpoint {}.",
                resolved.checkpoint_id
            ),
            "The checkpoint passed the hash guard and the changed files were restored.".to_string(),
            format!("deadreckon show {id}"),
            format!(
                "deadreckon rewind {id} --to-checkpoint {} --preview",
                resolved.checkpoint_id
            ),
        ),
    };

    VerdictSurface::must_new(
        kind,
        "rewind",
        Some(&id),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
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

fn render_why_failed(report: &WhyFailedReport) {
    print!("{}", why_failed_surface(report).render_plain(false));
}

fn why_failed_surface(report: &WhyFailedReport) -> VerdictSurface {
    let primary = report
        .try_lines
        .first()
        .cloned()
        .unwrap_or_else(|| format!("deadreckon show {} --why-failed", report.id));
    let secondary = report
        .try_lines
        .iter()
        .skip(1)
        .filter(|line| line.as_str() != primary)
        .map(|line| ("Secondary", line.as_str()))
        .collect::<Vec<_>>();
    let reason = report
        .reason
        .as_deref()
        .unwrap_or("no explicit failure reason was recorded");
    VerdictSurface::must_new(
        VerdictKind::Failed,
        report.kind,
        Some(&report.id),
        ExplanationPanel::new(
            format!(
                "DeadReckon inspected {} {} and found failure evidence.",
                report.kind, report.id
            ),
            format!("The stored status is {} and {reason}.", report.status),
            why_failed_evidence(report),
        ),
        vec![("Recommended", primary.as_str())],
        secondary,
    )
}

fn why_failed_evidence(report: &WhyFailedReport) -> Vec<(String, String)> {
    let mut evidence = vec![("status".to_string(), report.status.clone())];
    if let Some(reason) = report.reason.as_deref() {
        evidence.push(("reason".to_string(), reason.to_string()));
    }
    evidence.extend(
        report
            .evidence
            .iter()
            .enumerate()
            .map(|(index, line)| (format!("evidence {}", index + 1), line.clone())),
    );
    evidence
}

fn render_no_failures(kind: &'static str, id: &str, status: impl Into<String>) {
    let primary = format!("deadreckon show {id}");
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Noop,
            kind,
            Some(id),
            ExplanationPanel::new(
                format!("DeadReckon inspected {kind} {id} and found no failure evidence."),
                "The stored state is already successful, so --why-failed has no failure root cause to report.",
                vec![("status", status.into())],
            ),
            vec![("Recommended", primary.as_str())],
            Vec::<(&str, &str)>::new(),
        )
        .render_plain(false)
    );
}

fn show_plan_why_failed(paths: &DeadreckonPaths, plan: &Plan) {
    if plan.status == PlanStatus::Merged
        && plan
            .tasks
            .iter()
            .all(|task| task.status == PlanTaskStatus::Completed)
    {
        render_no_failures(
            "plan",
            &run_prefix(&plan.plan_id),
            plan_status_label(plan.status),
        );
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
    render_why_failed(&WhyFailedReport {
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
        render_no_failures(
            "run",
            &run_prefix(&state.run_id),
            run_status_label(state.status),
        );
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
    render_why_failed(&WhyFailedReport {
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
        let surface = manifest
            .is_none()
            .then(|| flight_unavailable_surface(state, turn, file_filter.as_deref()));
        let next_actions = surface
            .as_ref()
            .map(|surface| vec![surface.primary_action.command.clone()])
            .unwrap_or_default();
        let value = json!({
            "kind": "flight",
            "id": &state.run_id,
            "available": manifest.is_some(),
            "turn": turn,
            "file": file_filter,
            "manifest": manifest,
            "events": filtered_events,
            "checkpoints": filtered_checkpoints,
            "next_actions": next_actions,
            "try_lines": Vec::<String>::new(),
        });
        let value = surface
            .as_ref()
            .map(|surface| surface.add_to_json(value.clone()))
            .unwrap_or(value);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let Some(manifest) = manifest else {
        print!(
            "{}",
            flight_unavailable_surface(state, turn, file_filter.as_deref()).render_plain(false)
        );
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

fn flight_unavailable_surface(
    state: &deadreckon_core::PipelineState,
    turn: Option<u32>,
    file: Option<&Path>,
) -> VerdictSurface {
    let id = run_prefix(&state.run_id);
    let primary = format!("deadreckon show {id}");
    let mut evidence = vec![
        ("run".to_string(), id.clone()),
        ("available".to_string(), "false".to_string()),
        (
            "manifest".to_string(),
            state
                .run_root
                .join(FLIGHT_MANIFEST_JSON)
                .display()
                .to_string(),
        ),
        (
            "state".to_string(),
            state.state_path().display().to_string(),
        ),
    ];
    if let Some(turn) = turn {
        evidence.push(("turn".to_string(), turn.to_string()));
    }
    if let Some(file) = file {
        evidence.push(("file".to_string(), file.display().to_string()));
    }
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "flight",
        Some(&id),
        ExplanationPanel::new(
            format!("DeadReckon found no flight recorder data for run {id}."),
            "The command was read-only and the run has no flight manifest to inspect.",
            evidence,
        ),
        vec![("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
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

#[derive(Clone, Copy)]
struct ShowCommandArgs<'a> {
    run_id: &'a str,
    turn: Option<u32>,
    diff: bool,
    raw: Option<&'a str>,
    why_failed: bool,
    json_output: bool,
    flight: bool,
    file: Option<&'a Path>,
}

fn show_command(args: ShowCommandArgs<'_>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    // Shakedown P6: the hand-rolled cascade (campaign, plan child, run, plan --
    // and no chain at all) becomes one resolution. Each branch body below is the
    // one that was already here; the characterization goldens are the guard that
    // this stayed a move rather than a rewrite.
    let resolved = commands::reference::resolve_ref(
        &paths,
        commands::reference::RefQuery {
            reference: Some(args.run_id),
            all_scopes: false,
            verb: "show",
        },
    )?;
    let mut child_context: Option<PlanChildSelection> = None;
    let state = match resolved {
        commands::reference::ResolvedRef::Job(job) => {
            return commands::job::print_job_status(&job, args.json_output);
        }
        commands::reference::ResolvedRef::Run(state) => *state,
        commands::reference::ResolvedRef::PlanChild { selection, state } => {
            child_context = Some(selection);
            *state
        }
        commands::reference::ResolvedRef::Chain(chain) => {
            // `show` has never resolved a chain; the shared resolver hands it
            // one, and `chain show` is the renderer that already exists.
            return commands::chain::chain_show_command(
                &paths,
                &chain.chain_id,
                args.why_failed,
                args.json_output,
            );
        }
        commands::reference::ResolvedRef::Campaign {
            dir: campaign_dir,
            campaign,
        } => {
            let campaign = *campaign;
            let rollup = deadreckon_core::campaign::read_campaign_rollup(&campaign_dir).ok();
            let report = if args.why_failed {
                commands::campaign::campaign_why_failed_report(&paths, &campaign, rollup.as_ref())
            } else {
                commands::campaign::campaign_attach_summary(
                    Some(&paths),
                    &campaign,
                    rollup.as_ref(),
                )
            };
            if args.json_output {
                let surface = commands::campaign::campaign_verdict_surface(
                    Some(&paths),
                    &campaign,
                    rollup.as_ref(),
                );
                let value = surface.add_to_json(json!({
                    "campaign_id": campaign.campaign_id,
                    "status": commands::campaign::campaign_status_text(campaign.status),
                    "n": campaign.n,
                    "merged_run_id": campaign.merged_run_id,
                    "next_actions": [surface.primary_action.command.clone()],
                    "rollup": rollup
                        .as_ref()
                        .map(|r| commands::campaign::rollup_verdict_text(r.rollup_verdict)),
                }));
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            } else {
                print!("{report}");
            }
            return Ok(());
        }
        commands::reference::ResolvedRef::Plan(plan) => {
            let plan = *plan;
            if args.json_output {
                let value = commands::plan::plan_verdict_surface(&paths, &plan)
                            .add_to_json(json!({
                                "kind": "plan",
                                "id": &plan.plan_id,
                                "status": plan_status_label(plan.status),
                                "next_actions": commands::plan::plan_next_actions_with_context(&paths, &plan),
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
                            }));
                println!("{}", serde_json::to_string_pretty(&value)?);
                return Ok(());
            }
            if args.why_failed {
                show_plan_why_failed(&paths, &plan);
                return Ok(());
            }
            print_plan_summary(&paths, &plan, true);
            println!("{}", serde_json::to_string_pretty(&plan)?);
            return Ok(());
        }
    };
    if args.flight || args.file.is_some() {
        return show_flight(&state, args.turn, args.file, args.json_output);
    }
    if let Some(raw) = args.raw {
        return show_raw_artifact(&state, raw);
    }
    if args.diff {
        return show_run_diff(&state, args.json_output);
    }
    if let Some(turn) = args.turn {
        return show_run_turn(&state, turn, args.json_output);
    }
    let plan_wrapper_context = plan_wrapper_context_from_run(&paths, &state)?;
    if args.json_output {
        let status = run_status_label(state.status);
        let run_view = deadreckon_core::RunView::from_state(&state).ok();
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
                "run_view": run_view,
                "plan_child": child_context,
                "plan_result": plan_result,
            }))?
        );
        return Ok(());
    }
    if args.why_failed {
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
            if args
                .turn
                .is_some_and(|turn| record.prompt_id != format!("turn-{turn}"))
            {
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
            if args.turn.is_some_and(|turn| record.turn != turn) {
                continue;
            }
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
    }
    Ok(())
}

fn show_run_diff(state: &deadreckon_core::PipelineState, json_output: bool) -> Result<()> {
    let view = deadreckon_core::RunView::from_state(state)?;
    if state.turn == 0
        || view.missing.iter().any(|artifact| match artifact {
            deadreckon_core::Artifact::Snapshot(turn) => *turn == 0 || *turn == state.turn,
            _ => false,
        })
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no snapshots available for run diff",
            &format!("deadreckon show {}", run_prefix(&state.run_id)),
        )));
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&view.changed)?);
        return Ok(());
    }
    print!("{}", render_diff_summary(&view.changed));
    Ok(())
}

fn show_run_turn(
    state: &deadreckon_core::PipelineState,
    turn: u32,
    json_output: bool,
) -> Result<()> {
    let view = deadreckon_core::RunView::from_state(state)?;
    let Some(turn_view) = view.turns.iter().find(|candidate| candidate.n == turn) else {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("no snapshot for turn {turn}"),
            &format!("deadreckon show {}", run_prefix(&state.run_id)),
        )));
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(turn_view)?);
        return Ok(());
    }
    println!("turn {}", turn_view.n);
    println!("did {}", turn_view.did);
    println!("diff:");
    print!("{}", render_diff_summary(&turn_view.diff));
    if let Some(exchange) = turn_view.exchange_ref.as_ref() {
        println!("exchange {}#{}", exchange.path.display(), exchange.index);
        println!("{}", exchange.preview);
    } else {
        println!("exchange none");
    }
    if turn_view.sandbox_events.is_empty() {
        println!("sandbox events none");
    } else {
        println!("sandbox events:");
        for event in &turn_view.sandbox_events {
            println!("- {} {}", event.kind, event.summary);
        }
    }
    if let Some(check) = turn_view.check.as_ref() {
        println!("check {} {}/{}", check.status, check.index, check.total);
    }
    Ok(())
}

fn render_diff_summary(diff: &deadreckon_core::DiffSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "source diff: {} file(s), +{} -{}\n",
        diff.files_changed, diff.added, diff.removed
    ));
    if diff.files.is_empty() {
        out.push_str("(no source changes)\n");
        return out;
    }
    for file in &diff.files {
        out.push_str(&format!(
            "\n{:?} {} (+{} -{})\n",
            file.status,
            file.path.display(),
            file.added,
            file.removed
        ));
        if let Some(unified) = file.unified_diff.as_deref() {
            out.push_str(unified);
            if !unified.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str("(binary or unreadable diff)\n");
        }
    }
    out
}

fn show_raw_artifact(state: &deadreckon_core::PipelineState, raw: &str) -> Result<()> {
    let path = raw_artifact_path(state, raw)?;
    let bytes = fs::read(&path)?;
    io::stdout().write_all(&bytes)?;
    Ok(())
}

fn raw_artifact_path(state: &deadreckon_core::PipelineState, raw: &str) -> Result<PathBuf> {
    let name = raw.trim().to_ascii_lowercase();
    let name = name.trim_start_matches("./");
    let secret_names = ["gate-nonce", "gate/nonce", "nonce"];
    if secret_names.contains(&name) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "gate secret is not dumpable",
            &format!("deadreckon verdict {}", run_prefix(&state.run_id)),
        )));
    }
    let path = match name {
        "state" | "state.json" => state.state_path(),
        "history" | "history.json" => state.run_root.join("history.json"),
        "spend" | "spend.jsonl" => state.run_root.join("spend.jsonl"),
        "traces" | "trace" | "traces.jsonl" => state.run_root.join("traces.jsonl"),
        "events" | "events.jsonl" => state.run_root.join(RUN_EVENTS_JSONL),
        "provenance" | "provenance.jsonl" => state.run_root.join("provenance.jsonl"),
        "acceptance" | "acceptance.yaml" => acceptance_spec_path_for_run_root(&state.run_root),
        "sandbox" | "sandbox.toml" => state.run_root.join("sandbox.toml"),
        "launch-plan" | "launch-plan.json" => state.run_root.join("launch-plan.json"),
        "seams" | "seams.json" => state.run_root.join("seams.json"),
        "marker" | "acceptance-marker" | "turn-acceptance" | "turn-acceptance.json" => {
            marker_path_for_run_root(&state.run_root)
        }
        "acceptance-progress" | "acceptance-progress.jsonl" => {
            acceptance_progress_path_for_run_root(&state.run_root)
        }
        "acceptance-tamper" | "acceptance-tamper.json" => {
            deadreckon_core::tamper::acceptance_tamper_path_for_run_root(&state.run_root)
        }
        "narrative" | "run-narrative" | "run-narrative.md" => {
            doc_path_for_kind(&state.working_dir, DocKind::Narrative)
                .ok_or_else(|| DeadreckonError::NotFound("RUN-NARRATIVE.md".to_string()))?
        }
        "decisions" | "run-decisions" | "run-decisions.md" => {
            doc_path_for_kind(&state.working_dir, DocKind::Decisions)
                .ok_or_else(|| DeadreckonError::NotFound("RUN-DECISIONS.md".to_string()))?
        }
        "as-built" | "run-as-built" | "run-as-built.md" => {
            doc_path_for_kind(&state.working_dir, DocKind::AsBuilt)
                .ok_or_else(|| DeadreckonError::NotFound("RUN-AS-BUILT.md".to_string()))?
        }
        "flight" | "flight-events" | "flight-events.jsonl" => {
            state.run_root.join(FLIGHT_EVENTS_JSONL)
        }
        // Semaphore: the per-run conversation record (codex thread / claude
        // session id + resume-failure counter).
        "provider-session" | "provider-session.json" => {
            state.run_root.join("provider-session.json")
        }
        _ => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("unknown artifact {raw}"),
                &format!("deadreckon show {} --json", run_prefix(&state.run_id)),
            )));
        }
    };
    if !path.exists() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "{} for run {}",
            path.display(),
            run_prefix(&state.run_id)
        ))));
    }
    Ok(path)
}

fn read_parent_marker(root: &Path) -> Result<Option<commands::lifecycle::ParentMarker>> {
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

/// `status` is the orientation verb, so it orients across every kind.
///
/// It used to resolve runs only. Handed a plan id that `list` had just printed
/// it answered "not found: run <id>" and pointed back at `list`, which
/// recommended `status latest`, which refused identically — a closed loop
/// between the two most-used commands. It now answers for whatever the
/// reference names, delegating to the same renderers `show` uses so the two
/// cannot describe the same object differently.
fn status_command(run_id: Option<&str>, all: bool, plain: bool, json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let resolved = commands::reference::resolve_ref(
        &paths,
        commands::reference::RefQuery {
            reference: run_id,
            all_scopes: all,
            verb: "status",
        },
    )?;
    let state = match resolved {
        commands::reference::ResolvedRef::Job(job) => {
            return commands::job::print_job_status(&job, json_output);
        }
        commands::reference::ResolvedRef::Run(state) => *state,
        commands::reference::ResolvedRef::PlanChild { state, .. } => *state,
        commands::reference::ResolvedRef::Plan(plan) => {
            return status_plan(&paths, &plan, json_output);
        }
        commands::reference::ResolvedRef::Chain(chain) => {
            return commands::chain::chain_show_command(
                &paths,
                &chain.chain_id,
                false,
                json_output,
            );
        }
        commands::reference::ResolvedRef::Campaign { campaign, dir } => {
            return status_campaign(&paths, &campaign, &dir, json_output);
        }
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
                "billing": if run_is_subscription_only(&state) {
                    "subscription: cost is not metered, time is the budget"
                } else {
                    "metered"
                },
            }))?
        );
        return Ok(());
    }
    print_status_report(&state, plain);
    print_lifecycle_hints(&state);
    Ok(())
}

/// Plan orientation reuses `show`'s renderers verbatim, so `status <plan>` and
/// `show <plan>` cannot disagree about the same plan.
fn status_plan(paths: &DeadreckonPaths, plan: &Plan, json_output: bool) -> Result<()> {
    if json_output {
        let value = commands::plan::plan_verdict_surface(paths, plan).add_to_json(json!({
            "kind": "plan",
            "id": &plan.plan_id,
            "status": plan_status_label(plan.status),
            "next_actions": commands::plan::plan_next_actions_with_context(paths, plan),
            "try_lines": Vec::<String>::new(),
            "paths": commands::plan::plan_paths_json(plan),
            "plan": plan,
        }));
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print_plan_summary(paths, plan, true);
    Ok(())
}

fn status_campaign(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    dir: &Path,
    json_output: bool,
) -> Result<()> {
    let rollup = deadreckon_core::campaign::read_campaign_rollup(dir).ok();
    if json_output {
        let surface =
            commands::campaign::campaign_verdict_surface(Some(paths), campaign, rollup.as_ref());
        let value = surface.add_to_json(json!({
            "kind": "campaign",
            "id": &campaign.campaign_id,
            "status": commands::campaign::campaign_status_text(campaign.status),
            "next_actions": [surface.primary_action.command.clone()],
            "try_lines": Vec::<String>::new(),
        }));
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print!(
        "{}",
        commands::campaign::campaign_attach_summary(Some(paths), campaign, rollup.as_ref())
    );
    Ok(())
}

/// A merge run is the synthetic run that lands an orchestration's result — it
/// does no provider work itself, so its own spend/wall/turns are ~0 and read as
/// broken in `status`. When the shown run is a plan's merged run, roll up the
/// child runs' real metrics so the operator sees the orchestration's true cost.
struct MergeRunRollup {
    plan_prefix: String,
    children: usize,
    total_usd: f64,
    wall_seconds: f64,
    turns: usize,
    any_estimated: bool,
    partial: bool,
}

fn merge_run_rollup(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Option<MergeRunRollup> {
    // A merged plan run is created with no provider (see create_merged_plan_run).
    // Gate the plan scan on that so ordinary runs skip it on every status call.
    if state.provider.is_some() {
        return None;
    }
    let plan_entry = commands::inspection::list_plan_entries(paths, None)
        .ok()?
        .into_iter()
        .find(|plan| plan.merged_run_id.as_deref() == Some(state.run_id.as_str()))?;
    let plan = deadreckon_core::plan::load_plan(paths, &plan_entry.plan_id).ok()?;
    let mut rollup = MergeRunRollup {
        plan_prefix: run_prefix(&plan.plan_id),
        children: 0,
        total_usd: 0.0,
        wall_seconds: 0.0,
        turns: 0,
        any_estimated: false,
        partial: false,
    };
    let mut loaded = 0usize;
    for task in &plan.tasks {
        let (Some(run_id), Some(scope)) =
            (task.child_run_id.as_deref(), task.child_scope.as_deref())
        else {
            continue;
        };
        rollup.children += 1;
        let path = paths.run_root(scope, run_id).join("state.json");
        let Ok(child) = deadreckon_core::state::load_state(&path) else {
            continue;
        };
        if let Ok(summary) = deadreckon_core::state::spend_summary(&child) {
            rollup.total_usd += summary.total_usd;
            rollup.wall_seconds += summary.wall_seconds;
            rollup.turns += summary.turns;
            rollup.any_estimated |= summary.any_estimated_turn;
            loaded += 1;
        }
    }
    if rollup.children == 0 {
        return None;
    }
    rollup.partial = loaded < rollup.children;
    Some(rollup)
}

/// Shorten a task-key slug for a suggested/default export directory, trimming
/// back to a hyphen boundary so it reads as whole words (`./build-a-comprehensive`)
/// instead of a mid-word cut (`./build-a-comprehensive-fi`). task_key is ASCII.
fn dest_slug(task_key: &str) -> String {
    const MAX: usize = 24;
    if task_key.len() <= MAX {
        return task_key.trim_end_matches('-').to_string();
    }
    let head = &task_key[..MAX];
    let trimmed = match head.rfind('-') {
        Some(idx) if idx >= 8 => &head[..idx],
        _ => head,
    };
    trimmed.trim_end_matches('-').to_string()
}

fn merge_rollup_label(rollup: &MergeRunRollup) -> String {
    let estimate = if rollup.any_estimated { "~" } else { "" };
    let partial = if rollup.partial { " (partial)" } else { "" };
    format!(
        "{estimate}${:.6} · wall {:.1}s · {} turns across {} children{partial}",
        rollup.total_usd, rollup.wall_seconds, rollup.turns, rollup.children
    )
}

#[cfg(test)]
mod orchestration_status_tests {
    use super::{MergeRunRollup, dest_slug, merge_rollup_label};

    #[test]
    fn dest_slug_trims_to_a_word_boundary() {
        // The reported ugly case truncated mid-word at 24 chars.
        assert_eq!(
            dest_slug("build-a-comprehensive-financial-net-worth"),
            "build-a-comprehensive"
        );
        // Short slugs pass through, sans trailing dash.
        assert_eq!(dest_slug("financial-dashboard"), "financial-dashboard");
        assert_eq!(dest_slug("networth-"), "networth");
    }

    #[test]
    fn merge_rollup_label_reports_child_totals() {
        let label = merge_rollup_label(&MergeRunRollup {
            plan_prefix: "6119e8b2".to_string(),
            children: 4,
            total_usd: 0.42,
            wall_seconds: 312.5,
            turns: 58,
            any_estimated: false,
            partial: false,
        });
        assert!(label.contains("wall 312.5s"));
        assert!(label.contains("58 turns across 4 children"));
        assert!(!label.contains("partial"));
    }

    #[test]
    fn merge_rollup_label_marks_partial_and_estimated() {
        let label = merge_rollup_label(&MergeRunRollup {
            plan_prefix: "abc".to_string(),
            children: 3,
            total_usd: 1.0,
            wall_seconds: 10.0,
            turns: 5,
            any_estimated: true,
            partial: true,
        });
        assert!(label.starts_with('~'));
        assert!(label.contains("(partial)"));
    }
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
    let state_line = format!(
        "{} -> {next_action}",
        ui::render_status(ui::Stream::Stdout, run_status_label(state.status))
    );
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
    if run_is_subscription_only(state) {
        rows.insert(
            rows.len() - 1,
            (
                "billing".to_string(),
                "subscription: cost is not metered, time is the budget".to_string(),
            ),
        );
    }
    if let Some(rollup) = merge_run_rollup(&paths, state) {
        let insert_at = rows.len() - 1;
        rows.insert(
            insert_at,
            (
                "merge".to_string(),
                format!(
                    "plan {} · {} children (this run only lands the result)",
                    rollup.plan_prefix, rollup.children
                ),
            ),
        );
        rows.insert(
            insert_at + 1,
            ("rollup".to_string(), merge_rollup_label(&rollup)),
        );
    }
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
    let mut health: Vec<(&str, String)> = vec![
        ("action", next_action),
        ("stale", if stale { "yes" } else { "no" }.to_string()),
        ("pids", supervised.len().to_string()),
    ];
    if let Some(reason) = state.pause_reason.as_deref() {
        health.push(("paused", one_line(reason, 100)));
    }
    if let Some(reason) = state.failure_reason.as_deref() {
        health.push((
            "failure",
            ui::render(
                ui::Stream::Stdout,
                ui::Tone::Negative,
                one_line(reason, 100),
            ),
        ));
    }
    let gate = acceptance_status_value(state);
    health.push(("gate", ui::color_status_word(&gate)));
    let docs_status = docs_status_for_state(state);
    health.push(("docs", ui::color_status_word(&docs_status.to_string())));
    if docs_status == DocsStatus::Failed
        && let Ok(Some(record)) = deadreckon_runtime::read_polish_record(state)
    {
        let detail = record.error.as_deref().unwrap_or(record.status.as_str());
        health.push((
            "docs",
            format!(
                "polish failed ({}); fallback docs are still available",
                one_line(detail, 72)
            ),
        ));
    }
    print!("{}", kv_block_string("  ", &health));

    println!();
    println!("{}", ui_heading("library"));
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let manifest_present = library_dir.join("manifest.json").exists();
    let artifact_count =
        commands::inspection::library_entries(&paths, Some(state.scope.clone()), false)
            .map(|entries| entries.len())
            .unwrap_or(0);
    let current = if manifest_present {
        library_dir.display().to_string()
    } else {
        "not promoted".to_string()
    };
    let exported = commands::inspection::materialized_count_label(
        commands::inspection::materialized_marker_count(&library_dir),
    );
    print!(
        "{}",
        library_status_block(artifact_count, &current, &exported)
    );

    println!();
    println!("{}", ui_heading("disk"));
    let disk: Vec<(&str, String)> = match free_kb(paths.home()) {
        Some(kb) => {
            let mb = kb / 1024;
            let mut rows = vec![(
                "home",
                format!("{} MB free in {}", mb, paths.home().display()),
            )];
            if mb > 10_240 {
                rows.push(("tip", ui_command("deadreckon cleanup --completed")));
            } else {
                rows.push((
                    "warning",
                    "low disk; clean old worktrees and artifacts soon".to_string(),
                ));
            }
            rows
        }
        None => vec![("home", "unavailable; run deadreckon doctor".to_string())],
    };
    print!("{}", kv_block_string("  ", &disk));
}

/// The shared columnar-table primitive: lowercase `headers` over `rows`, each
/// column sized to its widest cell by display width and padded with `pad_visible`
/// (ANSI-aware), 2-space gutters, last column unpadded. Use this instead of
/// per-call-site const widths and `{:<N}` format-pads so colored cells align the
/// same as plain ones.
pub(crate) fn columns(headers: &[&str], rows: &[Vec<String>]) -> String {
    let col_count = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| ui::display_width(h)).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(col_count) {
            widths[index] = widths[index].max(ui::display_width(cell));
        }
    }
    let mut out = String::new();
    let mut push_row = |cells: &[&str]| {
        for (index, cell) in cells.iter().enumerate().take(col_count) {
            if index > 0 {
                out.push_str("  ");
            }
            if index + 1 == col_count {
                out.push_str(cell);
            } else {
                out.push_str(&ui::pad_visible(cell, widths[index]));
            }
        }
        out.push('\n');
    };
    push_row(headers);
    for row in rows {
        let cells = row.iter().map(String::as_str).collect::<Vec<_>>();
        push_row(&cells);
    }
    out
}

/// The status report's `library` section, extracted as a pure function so its
/// kv-block alignment is testable. `scope artifacts` is the widest key, so every
/// colon lines up beneath it.
fn library_status_block(artifact_count: usize, current: &str, exported: &str) -> String {
    kv_block_string(
        "  ",
        &[
            ("scope artifacts", artifact_count.to_string()),
            ("current", current.to_string()),
            ("exported", exported.to_string()),
        ],
    )
}

#[cfg(test)]
mod uniform_surface_kv_tests {
    use super::{columns, kv_block_string, library_status_block};

    #[test]
    fn columns_pads_by_display_width_with_color() {
        let rows_plain = vec![
            vec!["a".to_string(), "ok".to_string()],
            vec!["bbb".to_string(), "no".to_string()],
        ];
        let esc = char::from(27);
        let rows_colored = vec![
            vec![format!("{esc}[1;35ma{esc}[0m"), "ok".to_string()],
            vec![format!("{esc}[1;35mbbb{esc}[0m"), "no".to_string()],
        ];
        let plain = columns(&["id", "status"], &rows_plain);
        let colored = columns(&["id", "status"], &rows_colored);
        // Padding is by display width, so stripping ANSI from the colored table
        // yields exactly the plain table: color adds no visible columns.
        assert_eq!(crate::ui::strip_ansi(&colored), plain, "{colored}");
        assert!(plain.starts_with("id"), "lowercase header: {plain}");
    }

    #[test]
    fn kv_block_aligns_dynamic_key_column() {
        let block = kv_block_string("", &[("a", "1"), ("longkey", "2"), ("mid", "3")]);
        let colon_cols: Vec<usize> = block.lines().map(|line| line.find(':').unwrap()).collect();
        assert!(!colon_cols.is_empty());
        assert!(
            colon_cols.iter().all(|&c| c == colon_cols[0]),
            "colons must align under the widest key: {colon_cols:?}\n{block}"
        );
        // "longkey" (7) drives the column; the colon follows the padded key.
        assert_eq!(colon_cols[0], 7, "{block}");
    }

    #[test]
    fn status_report_uses_kv_block_no_manual_colons() {
        // The library section (which had the misaligned `scope artifacts:` line)
        // now renders through kv_block, so every colon aligns under the widest key.
        let block = library_status_block(3, "/lib/path", "2 exported");
        let colon_cols: Vec<usize> = block.lines().map(|line| line.find(':').unwrap()).collect();
        assert!(
            colon_cols.iter().all(|&c| c == colon_cols[0]),
            "library section colons must align: {colon_cols:?}\n{block}"
        );
        // indent (2) + "scope artifacts" (15) places every colon at column 17.
        assert_eq!(colon_cols[0], 17, "{block}");
        assert!(block.contains("scope artifacts: 3"), "{block}");
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
    print_spine_plain_lines(crate::tui::spine::spine_plain_lines(
        &crate::tui::spine::spine_for_run(state, Utc::now()),
    ));
    print_run_locations(state);
    print_chain_context_for_working(&state.working_dir);
}

fn print_spine_plain_lines(lines: Vec<String>) {
    println!("{}", ui_heading("Status spine"));
    for line in lines {
        println!("{line}");
    }
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
    print!(
        "{}",
        run_lifecycle_surface(state)
            .render_plain(false)
            .trim_end_matches('\n')
    );
    println!();
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
    let task_prefix = dest_slug(&state.task_key);
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

fn run_lifecycle_surface(state: &deadreckon_core::PipelineState) -> VerdictSurface {
    let id = run_prefix(&state.run_id);
    let (primary, secondary) = lifecycle_actions(state);
    let (kind, what, why) = match state.status {
        RunStatus::Completed => (
            VerdictKind::Completed,
            "The run has completed and DeadReckon has recorded its result state.",
            "The recommended command is the safest next lifecycle action for inspecting, landing, or exporting this result.",
        ),
        RunStatus::Failed => (
            VerdictKind::Failed,
            "The run ended before producing a verified result.",
            "Failure inspection is the safest next step before resuming or applying any work.",
        ),
        RunStatus::Killed => (
            VerdictKind::Killed,
            "The run was stopped before normal completion.",
            "DeadReckon cannot treat a killed run as complete; inspect or resume it before cleanup.",
        ),
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => (
            VerdictKind::Paused,
            "The run has state on disk but has not reached a terminal completed result.",
            "Attaching is the safest next command because it shows the live or resumable run state before any mutation.",
        ),
    };
    VerdictSurface::must_new(
        kind,
        "run",
        Some(&id),
        ExplanationPanel::new(
            what,
            why,
            vec![
                ("run".to_string(), id.clone()),
                (
                    "status".to_string(),
                    run_status_label(state.status).to_string(),
                ),
                (
                    "state".to_string(),
                    state.state_path().display().to_string(),
                ),
                (
                    "working".to_string(),
                    state.working_dir.display().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.command.as_str())],
        secondary
            .iter()
            .map(|action| ("Secondary", action.command.as_str())),
    )
}

async fn complete_run_actions(
    state: &deadreckon_core::PipelineState,
    allow_prompt: bool,
    print_hints: bool,
) -> Result<()> {
    if print_hints {
        print_lifecycle_hints(state);
    }
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
        let action = if prompt::is_interactive() {
            completion_action_select(state)?
        } else {
            // `x` is the abandon key everywhere (the attach TUI reserves `b`
            // for "back"); `b` stays accepted here for muscle memory but is
            // not advertised, so the documented keymap is one keymap.
            let prompt_text = if is_worktree_run(state) {
                "completed action [a apply, x abandon, d docs, s show, q quit]: "
            } else {
                "completed action [m export, e extend, d docs, s show, q quit]: "
            };
            completion_action_from_input(&prompt::open(prompt_text, None)?)
        };
        match action {
            Some(CompletionAction::Materialize) => prompt_materialize_action(&paths, state)?,
            Some(CompletionAction::Extend) => prompt_extend_action(state).await?,
            Some(CompletionAction::Apply) => commands::lifecycle::apply_command(
                state.run_id.clone(),
                "squash".to_string(),
                None,
                false,
                false,
                false,
                None,
                false,
            )?,
            Some(CompletionAction::Abandon) => {
                commands::lifecycle::abandon_command(state.run_id.clone(), false, false)?
            }
            Some(CompletionAction::Docs) => {
                Box::pin(commands::doc::doc_command(commands::doc::DocCommandArgs {
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
            Some(CompletionAction::Show) => show_command(ShowCommandArgs {
                run_id: &state.run_id,
                turn: None,
                diff: false,
                raw: None,
                why_failed: false,
                json_output: false,
                flight: false,
                file: None,
            })?,
            Some(CompletionAction::Quit) | None => break,
        }
    }
    Ok(())
}

/// The interactive completion menu: same actions as the lettered line REPL,
/// with each consequence spelled out. Esc maps to Quit via the cancel id.
fn completion_action_select(
    state: &deadreckon_core::PipelineState,
) -> Result<Option<CompletionAction>> {
    let mut choices = if is_worktree_run(state) {
        vec![
            prompt::SelectChoice::with_detail(
                "apply",
                "Apply",
                "squash-apply the run's changes to your branch",
            ),
            prompt::SelectChoice::with_detail(
                "abandon",
                "Abandon",
                "remove the worktree and temporary branch",
            ),
        ]
    } else {
        vec![
            prompt::SelectChoice::with_detail(
                "export",
                "Export",
                "copy the artifact to a directory you choose",
            ),
            prompt::SelectChoice::with_detail(
                "extend",
                "Extend",
                "start a follow-up run from this artifact",
            ),
        ]
    };
    choices.push(prompt::SelectChoice::with_detail(
        "docs",
        "Docs",
        "read RUN-NARRATIVE.md in the terminal",
    ));
    choices.push(prompt::SelectChoice::with_detail(
        "show",
        "Show",
        "inspect state, spend, and lineage",
    ));
    choices.push(prompt::SelectChoice::new("cancel", "Quit"));
    let selection = prompt::select_one(&prompt::SelectPrompt {
        title: "completed run — what next?".to_string(),
        help: None,
        choices,
        default_index: 0,
    })?;
    Ok(match selection.id.as_str() {
        "apply" => Some(CompletionAction::Apply),
        "abandon" => Some(CompletionAction::Abandon),
        "export" => Some(CompletionAction::Materialize),
        "extend" => Some(CompletionAction::Extend),
        "docs" => Some(CompletionAction::Docs),
        "show" => Some(CompletionAction::Show),
        _ => Some(CompletionAction::Quit),
    })
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

    /// Actions that change or discard the run's result and therefore require an
    /// in-TUI confirmation keystroke before they fire.
    pub(crate) fn is_destructive(self) -> bool {
        matches!(self, Self::Apply | Self::Abandon)
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
        "x" | "b" | "abandon" => Some(CompletionAction::Abandon),
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
    let default_dest = commands::lifecycle::default_materialize_dest(state);
    let answer = prompt::open(&format!("export dest [{}]: ", default_dest.display()), None)?;
    let dest = if answer.trim().is_empty() {
        default_dest
    } else {
        PathBuf::from(answer.trim())
    };
    let dest = absolute_dest(dest)?;
    refuse_dest_inside_home(paths, &dest, "export")?;
    let force = if dest.exists() && !commands::lifecycle::path_is_empty_dir(&dest)? {
        prompt::confirm("destination is not empty; overwrite?", false)?
    } else {
        false
    };
    let materialized =
        commands::lifecycle::materialize_completed_run(paths, state, Some(dest), force, false)?;
    commands::lifecycle::print_materialized(&materialized);
    Ok(())
}

async fn prompt_extend_action(state: &deadreckon_core::PipelineState) -> Result<()> {
    let goal = prompt::open("follow-up goal: ", None)?;
    if goal.trim().is_empty() {
        println!("extend skipped; follow-up goal was empty");
        return Ok(());
    }
    Box::pin(commands::lifecycle::extend_command(ExtendCommandArgs {
        parent_run_id: state.run_id.clone(),
        new_goal: goal,
        dest: None,
        acceptance: None,
        yes: true,
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
        narrate: false,
        no_narrate: false,
        narrator_model: None,
    }))
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

#[derive(Debug, Default, Clone)]
struct AttachLiveInventory {
    file_count: usize,
    total_bytes: u64,
    files: Vec<LiveFile>,
}

const ATTACH_LIVE_FILE_DISPLAY_LIMIT: usize = 240;
const ATTACH_JSONL_TAIL_ROW_LIMIT: usize = 4096;

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
        self.trim_rows();
        self.partial_bytes = bytes.len().saturating_sub(complete_len);
        self.offset = self.offset.saturating_add(complete_len as u64);
        self.signature = Some(AttachJsonlSignature {
            len: self.offset,
            modified,
        });
        Ok(&self.rows)
    }

    fn trim_rows(&mut self) {
        let overflow = self.rows.len().saturating_sub(ATTACH_JSONL_TAIL_ROW_LIMIT);
        if overflow > 0 {
            self.rows.drain(0..overflow);
        }
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

const ATTACH_LIVE_INVENTORY_FRESHNESS: Duration = Duration::from_secs(1);

#[derive(Default)]
struct AttachLiveInventoryCacheEntry {
    inventory: AttachLiveInventory,
    walked_at: Option<Instant>,
}

fn attach_live_inventory(root: &Path) -> AttachLiveInventory {
    // The attach draw path calls this every idle tick (~4-5 fps), and each call
    // recursively walks + stats + sorts the entire working_dir. Cache the result
    // per working_dir path and reuse it while it is fresh; the walk is equivalent
    // between refreshes because the display already truncates to a fixed count.
    thread_local! {
        static ATTACH_LIVE_INVENTORY_CACHE: RefCell<BTreeMap<PathBuf, AttachLiveInventoryCacheEntry>> =
            const { RefCell::new(BTreeMap::new()) };
    }
    let now = Instant::now();
    let cached = ATTACH_LIVE_INVENTORY_CACHE.with(|cache| {
        cache.borrow().get(root).and_then(|entry| {
            entry.walked_at.and_then(|walked_at| {
                (now.duration_since(walked_at) < ATTACH_LIVE_INVENTORY_FRESHNESS)
                    .then(|| entry.inventory.clone())
            })
        })
    });
    if let Some(inventory) = cached {
        return inventory;
    }

    let inventory = attach_live_inventory_walk(root);
    ATTACH_LIVE_INVENTORY_CACHE.with(|cache| {
        cache.borrow_mut().insert(
            root.to_path_buf(),
            AttachLiveInventoryCacheEntry {
                inventory: inventory.clone(),
                walked_at: Some(now),
            },
        );
    });
    inventory
}

fn attach_live_inventory_walk(root: &Path) -> AttachLiveInventory {
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

const LIVE_PID_COMMAND_FRESHNESS: Duration = Duration::from_secs(1);

fn live_pid(pid: u32) -> LivePid {
    // `pid_is_alive` is a non-forking liveness check and runs every tick so that
    // process death stays prompt. Only the `ps`-derived command detail (which
    // forks a subprocess) is throttled: refresh it at most once per
    // LIVE_PID_COMMAND_FRESHNESS per pid and reuse the cached string otherwise.
    let alive = deadreckon_core::pid_is_alive(pid);
    let command = if alive {
        live_pid_command(pid)
    } else {
        "not running".to_string()
    };
    LivePid {
        pid,
        alive,
        command,
    }
}

fn live_pid_command(pid: u32) -> String {
    thread_local! {
        static LIVE_PID_COMMAND_CACHE: RefCell<std::collections::HashMap<u32, (String, Instant)>> =
            RefCell::new(std::collections::HashMap::new());
    }
    let now = Instant::now();
    let cached = LIVE_PID_COMMAND_CACHE.with(|cache| {
        cache.borrow().get(&pid).and_then(|(command, fetched_at)| {
            (now.duration_since(*fetched_at) < LIVE_PID_COMMAND_FRESHNESS).then(|| command.clone())
        })
    });
    if let Some(command) = cached {
        return command;
    }

    let command = std::process::Command::new("ps")
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
        .unwrap_or_else(|| "alive".to_string());
    LIVE_PID_COMMAND_CACHE.with(|cache| {
        cache.borrow_mut().insert(pid, (command.clone(), now));
    });
    command
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
    let mut provider_request = ProviderRequest::enforceably_read_only(
        prompt.prompt,
        2_000,
        cwd.unwrap_or_else(|| paths.home().to_path_buf()),
    );
    provider_request.output_path = Some(output_path);
    provider_request.cancellation_token = cancellation_token;
    let response = match router.complete(&provider_request).await {
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
        RunEventKind::TurnStarted { turn } => {
            format!("turn {turn} started")
        }
        RunEventKind::ToolCallStarted {
            turn,
            tool_call_id,
            tool_name,
            ..
        } => format!("turn {turn} {tool_name} {tool_call_id} started"),
        RunEventKind::ToolCallResult {
            turn,
            tool_call_id,
            status,
            preview,
        } => format!("turn {turn} {tool_call_id} {status} {preview}"),
        RunEventKind::TokenUsageDelta {
            turn,
            input_tokens,
            output_tokens,
        } => format!("turn {turn} tokens +{}", input_tokens + output_tokens),
        RunEventKind::SpendDelta {
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
        RunEventKind::DocsCheckpoint { turn, path, status } => {
            format!("turn {turn} docs {status} {}", path.display())
        }
        RunEventKind::RunCompleted { status } => {
            format!("run {status}")
        }
        RunEventKind::RunPromoted { library_dir } => {
            format!("promoted {}", library_dir.display())
        }
        RunEventKind::Error { turn, message } => {
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

fn compact_whitespace(value: &str) -> String {
    value
        .split_whitespace()
        .fold(String::new(), |mut out, word| {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
            out
        })
}

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = compact_whitespace(value);
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

/// The last successfully-parsing row of a JSONL file, read from the end.
/// Equivalent to `read_jsonl(path)?.into_iter().last()` but without reading or
/// parsing the whole file — used on the per-frame attach draw path where only
/// the latest row matters.
fn read_last_jsonl<T: DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    // A JSONL row is small; the final 64 KiB always contains the last complete
    // line. Scanning from the end finds the most recent line that parses (a
    // partial trailing write is skipped, matching read_jsonl's filter_map).
    let block = len.min(64 * 1024);
    file.seek(SeekFrom::Start(len - block)).ok()?;
    let mut bytes = Vec::with_capacity(block as usize);
    file.read_to_end(&mut bytes).ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .find_map(|line| serde_json::from_str::<T>(line).ok())
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
