use super::super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartSelectedMode {
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
pub(crate) enum StartSelectionSource {
    ExplicitFlag,
    GoalShape,
    // `Heuristic` is gone: it existed only for the keyword matcher that used
    // to choose the shape before any classifier ran.
    InteractiveChoice,
    Default,
}

impl StartSelectionSource {
    fn label(self) -> &'static str {
        match self {
            Self::ExplicitFlag => "explicit_flag",
            Self::GoalShape => "goal_shape",
            Self::InteractiveChoice => "interactive_choice",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GoalShape {
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
pub(crate) enum GoalShapeSource {
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

// Not Eq: pieces carry an optional budget_usd.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoalShapeRecommendation {
    pub(crate) schema_version: u8,
    pub(crate) goal: String,
    pub(crate) shape: GoalShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) n: Option<u8>,
    pub(crate) rationale: String,
    pub(crate) source: GoalShapeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    /// The graph the classifier drew, carried through to plan creation.
    ///
    /// Previously the classifier's decomposition was discarded after the
    /// shape and `n` were read, and `build_full_plan_tasks` asked a *second*
    /// planner for the child graph. That second call was the only one that
    /// could express dependencies, so the shape decision — the expensive,
    /// irreversible one — was made blind to the structure that followed it.
    /// Keeping the nodes here makes one classifier's answer the plan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pieces: Vec<commands::course::CoursePiece>,
    /// When the classifier wants results to land: at the end, or as each node
    /// passes its gate.
    #[serde(default)]
    pub(crate) apply: deadreckon_core::plan::ApplyWhen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartProviderSource {
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
pub(crate) enum StartDoneCriteriaSource {
    Project,
    Generated,
    Manual,
    /// Polyglot detection found a real test contract — zero questions asked.
    Detected,
    /// The operator answered the one launch question in English.
    Asked,
    Missing,
}

impl StartDoneCriteriaSource {
    fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Generated => "generated",
            Self::Manual => "manual",
            Self::Detected => "detected",
            Self::Asked => "asked",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartSourceMode {
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
pub(crate) enum StartDoneAction {
    Existing,
    ManualText {
        text: String,
        overwrite_existing: bool,
    },
    DefaultGate,
    Missing,
}

pub(crate) trait StartPrompter {
    fn select_one(&mut self, prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice>;
    fn confirm(&mut self, question: &str, default_yes: bool) -> Result<bool>;
    fn input(&mut self, message: &str, default: Option<&str>) -> Result<String>;
}

pub(crate) struct TerminalStartPrompter;

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
pub(crate) struct StartPromptEligibility {
    pub(crate) stdin_is_tty: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
    pub(crate) quiet: bool,
    pub(crate) yes: bool,
}

impl StartPromptEligibility {
    pub(crate) fn from_args(args: &StartCommandArgs, stdin_is_tty: bool) -> Self {
        Self {
            stdin_is_tty,
            json: args.json,
            plain: args.plain,
            quiet: args.quiet,
            yes: args.yes,
        }
    }

    pub(crate) fn allows_prompts(self) -> bool {
        self.stdin_is_tty && !self.json && !self.plain && !self.quiet && !self.yes
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StartLaunchInput<'a> {
    pub(crate) goal: &'a str,
    pub(crate) requested_mode: crate::cli::CliStartMode,
    pub(crate) stdin_is_tty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartSourceProvenance {
    CurrentCleanWorktree,
    ExplicitCopy,
    ExplicitFresh,
    InitGit,
}

impl StartSourceProvenance {
    fn label(self) -> &'static str {
        match self {
            Self::CurrentCleanWorktree => "current-clean-worktree",
            Self::ExplicitCopy => "explicit-copy",
            Self::ExplicitFresh => "explicit-fresh",
            Self::InitGit => "init-git",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedStartSource {
    pub(crate) mode: commands::job::DurableSourceMode,
    pub(crate) requested_from: Option<PathBuf>,
    pub(crate) inspection_root: PathBuf,
    pub(crate) contract_write_root: PathBuf,
    pub(crate) allow_dirty: bool,
    pub(crate) provenance: StartSourceProvenance,
}

impl ResolvedStartSource {
    fn durable_source(&self) -> commands::job::DurableSource {
        commands::job::DurableSource {
            mode: self.mode,
            from: self.requested_from.clone(),
            allow_dirty: self.allow_dirty,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StartLaunchDecision {
    pub(crate) goal: String,
    pub(crate) selected_mode: StartSelectedMode,
    pub(crate) selection_source: StartSelectionSource,
    pub(crate) reason: String,
    pub(crate) provider_source: StartProviderSource,
    pub(crate) provider_route: Option<String>,
    pub(crate) provider_label: String,
    pub(crate) model: Option<String>,
    pub(crate) child_count: Option<u8>,
    pub(crate) planner_provider_route: Option<String>,
    pub(crate) planner_model: Option<String>,
    pub(crate) child_provider_route: Option<String>,
    pub(crate) child_model: Option<String>,
    pub(crate) child_provider_overrides: Vec<String>,
    pub(crate) child_model_overrides: Vec<String>,
    pub(crate) coder_provider_route: Option<String>,
    pub(crate) coder_model: Option<String>,
    pub(crate) reviewer_provider_route: Option<String>,
    pub(crate) reviewer_model: Option<String>,
    pub(crate) done_criteria_source: StartDoneCriteriaSource,
    pub(crate) done_action: StartDoneAction,
    pub(crate) done_criteria_label: String,
    pub(crate) done_contract: Option<commands::acceptance::CompiledContract>,
    pub(crate) done_divergence: Option<commands::acceptance::ContractDivergence>,
    pub(crate) source_mode: StartSourceMode,
    pub(crate) source_mode_label: String,
    pub(crate) source_fresh: bool,
    pub(crate) source_worktree: bool,
    pub(crate) source_from: Option<PathBuf>,
    pub(crate) source_init_git: bool,
    pub(crate) source_allow_dirty: bool,
    pub(crate) resolved_source: Option<ResolvedStartSource>,
    pub(crate) base_run_id: Option<String>,
    pub(crate) base_run_label: Option<String>,
    pub(crate) history_action_label: Option<String>,
    pub(crate) history_next_actions: Vec<String>,
    pub(crate) goal_shape: Option<GoalShapeRecommendation>,
    pub(crate) requires_confirmation: bool,
    pub(crate) confirmed_by_start_picker: bool,
    pub(crate) try_lines: Vec<String>,
    pub(crate) recovery: Option<StartRecovery>,
}

#[derive(Debug, Clone)]
pub(crate) struct StartRecovery {
    pub(crate) message: String,
    pub(crate) try_lines: Vec<String>,
}

pub(crate) fn start_launch_decision(input: StartLaunchInput<'_>) -> StartLaunchDecision {
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
        model: None,
        provider_label: StartProviderSource::Missing.label().to_string(),
        child_count: None,
        planner_provider_route: None,
        planner_model: None,
        child_provider_route: None,
        child_model: None,
        child_provider_overrides: Vec::new(),
        child_model_overrides: Vec::new(),
        coder_provider_route: None,
        coder_model: None,
        reviewer_provider_route: None,
        reviewer_model: None,
        done_criteria_source: StartDoneCriteriaSource::Missing,
        done_action: StartDoneAction::Missing,
        done_criteria_label: StartDoneCriteriaSource::Missing.label().to_string(),
        done_contract: None,
        done_divergence: None,
        source_mode: StartSourceMode::Missing,
        source_mode_label: StartSourceMode::Missing.label().to_string(),
        source_fresh: false,
        source_worktree: false,
        source_from: None,
        source_init_git: false,
        source_allow_dirty: false,
        resolved_source: None,
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

/// The conservative floor for auto mode.
///
/// This used to keyword-match the goal first — "audit", "harden", "validate"
/// latched Review; parallel-sounding words latched FullPlan — and whatever it
/// picked won outright, because `apply_goal_shape_recommendation` then
/// returned early for Review and discarded both the signal ladder and the
/// provider classifier. A word in the goal could double the bill with no
/// visible decision. Shape now comes from one classifier; this only supplies
/// the starting point it refines.
fn start_auto_mode_decision(
    _goal: &str,
    stdin_is_tty: bool,
) -> (StartSelectedMode, StartSelectionSource, String) {
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

/// Auto mode always classifies.
///
/// This used to require an interactive terminal, `--preview`, or `--json`, so
/// `deadreckon start "goal" --yes --plain` — the unattended path this tool
/// exists for — never classified at all and always fell to the conservative
/// floor. The deterministic ladder is free and instant, so there is no reason
/// to withhold it from a scripted launch; only the provider call is gated,
/// and that gating lives in `start_goal_shape_provider_route`.
fn start_goal_shape_should_classify(
    args: &StartCommandArgs,
    _eligibility: StartPromptEligibility,
) -> bool {
    matches!(args.mode, crate::cli::CliStartMode::Auto)
}

/// Whether to spend a provider call refining the ladder's answer.
///
/// The ladder runs either way. This only decides whether a bounded, read-only
/// planning call is worth making: it is when the operator is watching or
/// asking for a preview, and when they explicitly named a provider for an
/// unattended launch.
fn start_should_ask_provider_for_shape(
    args: &StartCommandArgs,
    eligibility: StartPromptEligibility,
) -> bool {
    eligibility.allows_prompts() || args.preview || args.json || args.provider.is_some()
}

fn start_goal_shape_provider_route(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    args: &StartCommandArgs,
) -> Option<String> {
    goal_shape_provider_route(paths, defaults, args.provider.as_deref())
}

pub(crate) fn goal_shape_provider_route(
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

/// Classify the goal's launch shape. Superseded internals (C-P5): the old
/// text-only classifier is gone — a SignalBundle grounds one bounded planner
/// call whose draft is clamped against the deterministic ladder, and the
/// ladder itself is the provider-free floor. The `GoalShapeRecommendation`
/// output shape is unchanged so preview records and campaign seeding keep
/// working until dispatch consumes the plan file directly (C-P9).
pub(crate) async fn classify_goal_shape_for_start(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    provider: Option<&str>,
    sandbox_backend: deadreckon_sandbox::SandboxBackend,
    plain: bool,
) -> Result<GoalShapeRecommendation> {
    let defaults_ceiling = config_defaults(paths).ok().and_then(|d| d.max_spend);
    let signals = commands::course::collect_signal_bundle(paths, cwd, goal, defaults_ceiling);
    let ladder = commands::course::ladder_decision(&signals);
    if let Some(provider) = provider
        && provider != "smoke"
        && !provider.starts_with("smoke:")
        && let Some(resolved) = provider_course_plan(
            paths,
            cwd,
            goal,
            provider,
            sandbox_backend,
            plain,
            &signals,
            &ladder,
        )
        .await?
    {
        let (shape, n, pieces, apply, resolution) = resolved;
        return Ok(GoalShapeRecommendation {
            schema_version: 1,
            goal: goal.to_string(),
            shape: course_shape_to_goal_shape(shape),
            n,
            rationale: resolution.rationale,
            source: GoalShapeSource::Provider,
            provider: Some(provider.to_string()),
            pieces,
            apply,
        });
    }
    Ok(ladder_goal_shape_recommendation(goal, &ladder))
}

#[allow(clippy::too_many_arguments)]
async fn provider_course_plan(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    provider: &str,
    sandbox_backend: deadreckon_sandbox::SandboxBackend,
    plain: bool,
    signals: &commands::course::SignalBundle,
    ladder: &commands::course::LadderDecision,
) -> Result<Option<commands::course::ResolvedCoursePlan>> {
    let router = match ProviderRouter::from_config_path(&paths.config_path(), Some(provider)) {
        Ok(router) => router,
        Err(error) => {
            eprintln!(
                "{}",
                ui_warn(format!(
                    "course planner unavailable via {provider}; using the deterministic course: {error}"
                ))
            );
            return Ok(None);
        }
    };
    let token = CancellationToken::new();
    let attempt_id = Uuid::new_v4().simple().to_string();
    let pid_file = std::env::temp_dir().join(format!("deadreckon-course-{attempt_id}.pid"));
    let output_path = std::env::temp_dir().join(format!("deadreckon-course-{attempt_id}.out"));
    let mut request = ProviderRequest::enforceably_read_only_with_backend(
        commands::course::course_planner_prompt(goal, signals),
        512,
        cwd,
        sandbox_backend,
    );
    request.pid_file = Some(pid_file.clone());
    request.output_path = Some(output_path.clone());
    request.cancellation_token = Some(token.clone());
    let completion =
        maybe_with_cli_wait_status(!plain, "plotting the course", router.complete(&request));
    tokio::pin!(completion);
    let response = tokio::select! {
        response = &mut completion => response,
        () = tokio::time::sleep(course_planner_timeout(provider)) => {
            token.cancel();
            let cleanup = tokio::time::timeout(
                course_planner_cleanup_timeout(),
                &mut completion,
            ).await;
            require_course_planner_cleanup_proven(
                cleanup.is_ok(),
                &pid_file,
                provider,
                "timed out",
            )?;
            remove_course_planner_outputs(&output_path);
            eprintln!(
                "{}",
                ui_warn(format!(
                    "course planner timed out via {provider}; using the deterministic course"
                ))
            );
            return Ok(None);
        }
    };
    require_course_planner_cleanup_proven(true, &pid_file, provider, "returned")?;
    remove_course_planner_outputs(&output_path);
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            eprintln!(
                "{}",
                ui_warn(format!(
                    "course planner unavailable via {provider}; using the deterministic course: {error}"
                ))
            );
            return Ok(None);
        }
    };
    Ok(commands::course::resolve_provider_course_plan(
        &response.content,
        signals,
        ladder,
        commands::course::SHAPE_CONFIDENCE_FLOOR_DEFAULT,
    ))
}

fn remove_course_planner_outputs(output_path: &Path) {
    let _ = fs::remove_file(output_path);
    let _ = fs::remove_file(output_path.with_extension("last.txt"));
}

fn require_course_planner_cleanup_proven(
    cleanup_completed: bool,
    pid_file: &Path,
    provider: &str,
    phase: &str,
) -> Result<()> {
    if cleanup_completed && !pid_file.exists() {
        return Ok(());
    }
    let authority = if pid_file.exists() {
        format!("process authority remains at {}", pid_file.display())
    } else {
        format!(
            "the cleanup wait did not resolve, so provider exit cannot be proven (PID checkpoint path: {})",
            pid_file.display()
        )
    };
    Err(CliError::Core(deadreckon_core::user_error(
        &format!(
            "course planner {phase} via {provider}, but provider cleanup was not proven: {authority}"
        ),
        "stop the recorded provider process, then rerun deadreckon start",
    )))
}

/// The shape planner is a bounded read-only phase, but subscription CLIs can
/// cold-start or compact for tens of seconds. Give both local and HTTP routes
/// realistic room, then cancel and prove cleanup before falling back.
pub(crate) fn course_planner_timeout(provider: &str) -> Duration {
    if provider.starts_with("cli:") {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(30)
    }
}

fn course_planner_cleanup_timeout() -> Duration {
    Duration::from_secs(30)
}

fn course_shape_to_goal_shape(shape: commands::course::CourseShape) -> GoalShape {
    match shape {
        // Chain-extend rides the single-run recommendation until dispatch
        // consumes the plan file directly (C-P9); the rationale carries the
        // continuation story.
        commands::course::CourseShape::Single | commands::course::CourseShape::ChainExtend => {
            GoalShape::Single
        }
        commands::course::CourseShape::Plan => GoalShape::Orchestrate,
        commands::course::CourseShape::Campaign => GoalShape::Campaign,
    }
}

/// Convert a ladder decision into the recommendation shape the preview and
/// campaign seeding consume — the provider-free classification floor.
/// Campaign is never a deterministic outcome (Course doctrine: deterministic
/// campaign selection is a spend hazard).
pub(crate) fn ladder_goal_shape_recommendation(
    goal: &str,
    ladder: &commands::course::LadderDecision,
) -> GoalShapeRecommendation {
    let resolution = commands::course::ladder_resolution(ladder);
    GoalShapeRecommendation {
        schema_version: 1,
        goal: goal.to_string(),
        shape: course_shape_to_goal_shape(ladder.shape),
        n: ladder.n,
        rationale: resolution.rationale,
        source: GoalShapeSource::Fallback,
        provider: None,
        // The ladder decides shape and n from measured signals; it does not
        // author node goals, so plan creation falls back to its own planner.
        pieces: Vec::new(),
        // The ladder does ask for incremental landing, but only when the goal
        // spells out an order ("migrate, then update, then delete"). Ordering
        // is a structural fact in the text, not a spend gamble — and it is the
        // one shape `start` previously could not reach at all.
        apply: ladder.apply,
    }
}

fn goal_shape_to_start_mode(shape: GoalShape) -> StartSelectedMode {
    match shape {
        GoalShape::Single => StartSelectedMode::Run,
        GoalShape::Orchestrate => StartSelectedMode::FullPlan,
        GoalShape::Campaign => StartSelectedMode::Campaign,
    }
}

pub(crate) fn apply_goal_shape_recommendation(
    decision: &mut StartLaunchDecision,
    recommendation: GoalShapeRecommendation,
) {
    // An explicit --mode review is the operator's call and still wins; what no
    // longer wins is a keyword match, which used to reach here as Review and
    // silently discard this recommendation.
    if matches!(decision.selected_mode, StartSelectedMode::Review)
        && decision.selection_source == StartSelectionSource::ExplicitFlag
    {
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

pub(crate) fn write_goal_shape_preview_record(
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

pub(crate) fn maybe_prompt_start_mode(
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

fn start_detected_cli_provider_ids(paths: &DeadreckonPaths) -> Result<Vec<String>> {
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let mut ids = Vec::new();
    for descriptor in registry.iter() {
        if descriptor.kind == DescriptorKind::Cli
            && descriptor.subscription
            && setup::provider_route_is_auto_selectable(&descriptor.id)
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

pub(crate) fn add_start_history_actions(
    decision: &mut StartLaunchDecision,
    run: Option<&RunListEntry>,
) {
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
    decision.history_action_label = Some(format!("follow-up available from {prefix}"));
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartExecutionSelection {
    provider: String,
    model: Option<String>,
    label: String,
    detail: String,
}

fn start_available_provider_routes(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    current: Option<&str>,
) -> Result<Vec<String>> {
    Ok(start_provider_picker_choices(paths, defaults, current)?
        .into_iter()
        .filter_map(|choice| choice.id.strip_prefix("route:").map(ToString::to_string))
        .collect())
}

fn descriptor_id_for_provider_kind(kind: &ProviderKind) -> &str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAi => "openai",
        ProviderKind::OpenAiCompatible => "openai-compatible",
        ProviderKind::CliClaudeCode => "cli:claude-code",
        ProviderKind::CliCodex => "cli:codex",
        ProviderKind::ScriptedSmoke => "smoke",
        ProviderKind::Generic(id) => id,
    }
}

fn start_descriptor_for_route<'a>(
    route: &str,
    registry: &'a ProviderRegistry,
    config: &deadreckon_providers::ProviderConfigFile,
) -> Option<&'a deadreckon_providers::registry::ProviderDescriptor> {
    registry.get(route).or_else(|| {
        let kind = config.providers.get(route)?.kind.as_ref()?;
        registry.get(descriptor_id_for_provider_kind(kind))
    })
}

fn configured_start_model_for_route(
    route: &str,
    defaults: &ConfigDefaults,
    config: &deadreckon_providers::ProviderConfigFile,
) -> Option<String> {
    config
        .providers
        .get(route)
        .and_then(|entry| entry.model.clone())
        .or_else(|| {
            (defaults.provider.as_deref() == Some(route))
                .then(|| defaults.model.clone())
                .flatten()
        })
}

fn start_execution_selections(
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    current_provider: Option<&str>,
    current_model: Option<&str>,
) -> Result<(Vec<StartExecutionSelection>, usize)> {
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let config = read_config(&paths.config_path())?;
    let user_home = std::env::home_dir().unwrap_or_else(|| paths.home().to_path_buf());
    let routes = start_available_provider_routes(paths, defaults, current_provider)?;
    let mut selections = Vec::new();
    let mut default_index = 0;

    for route in routes {
        let descriptor = start_descriptor_for_route(&route, &registry, &config);
        let display_name = descriptor
            .map(|item| item.display_name.as_str())
            .unwrap_or(route.as_str());
        let configured_model = configured_start_model_for_route(&route, defaults, &config);
        let mut models = descriptor
            .map(|item| deadreckon_providers::resolve_model_catalog(item, &user_home))
            .map(|catalog| (catalog.models, catalog.source.label().to_string()))
            .unwrap_or_else(|| (Vec::new(), "configured route".to_string()));

        let pinned_model = configured_model.as_deref().or_else(|| {
            (current_provider == Some(route.as_str()))
                .then_some(current_model)
                .flatten()
        });
        if let Some(model) = pinned_model
            && !models
                .0
                .iter()
                .any(|entry| entry.id == model || entry.aliases.iter().any(|alias| alias == model))
        {
            models.0.insert(
                0,
                deadreckon_providers::ModelEntry {
                    id: model.to_string(),
                    context_window: None,
                    input_per_million: None,
                    output_per_million: None,
                    aliases: Vec::new(),
                    recommended: false,
                },
            );
        }
        if models.0.is_empty() {
            models.0.push(deadreckon_providers::ModelEntry {
                id: "provider default".to_string(),
                context_window: None,
                input_per_million: None,
                output_per_million: None,
                aliases: Vec::new(),
                recommended: true,
            });
        }

        for entry in models.0 {
            let model = (entry.id != "provider default").then(|| entry.id.clone());
            let mut details = vec![route.clone(), models.1.clone()];
            if let Some(window) = entry.context_window {
                details.push(format!("context {}k", window / 1000));
            }
            if configured_model.as_deref() == Some(entry.id.as_str()) {
                details.push("configured".to_string());
            } else if entry.recommended {
                details.push("recommended".to_string());
            }
            let index = selections.len();
            let is_current_model = current_model == model.as_deref()
                || (current_model.is_none() && configured_model.as_deref() == model.as_deref());
            if current_provider == Some(route.as_str()) && is_current_model {
                default_index = index;
            }
            selections.push(StartExecutionSelection {
                provider: route.clone(),
                model,
                label: format!("{display_name} · {}", entry.id),
                detail: details.join(" · "),
            });
        }
    }
    Ok((selections, default_index))
}

struct StartExecutionPrompt<'a> {
    role: setup::SetupProviderRoleRef,
    title: String,
    help: String,
    current_provider: Option<&'a str>,
    current_model: Option<&'a str>,
}

fn prompt_start_execution_selection(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    prompt: StartExecutionPrompt<'_>,
    prompter: &mut dyn StartPrompter,
) -> Result<Option<StartExecutionSelection>> {
    let (selections, default_index) = start_execution_selections(
        paths,
        defaults,
        prompt.current_provider,
        prompt.current_model,
    )?;
    let mut choices = selections
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            prompt::SelectChoice::with_detail(
                format!("execution:{index}"),
                &selection.label,
                &selection.detail,
            )
        })
        .collect::<Vec<_>>();
    choices.push(start_prompt_choice(
        "typed",
        "Type another provider and model",
        "advanced: PROVIDER [MODEL]",
    ));
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: prompt.title,
        help: Some(prompt.help),
        choices,
        default_index: default_index.min(selections.len().saturating_sub(1)),
    })?;
    if let Some(raw) = choice.id.strip_prefix("execution:") {
        let index = raw.parse::<usize>().map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                "execution-team selection was invalid",
                "run deadreckon start again",
            ))
        })?;
        return Ok(selections.get(index).cloned());
    }
    if choice.id == "typed" {
        let answer = prompter.input("provider and optional model: ", None)?;
        let mut parts = answer.split_whitespace();
        let Some(route) = parts.next() else {
            set_start_recovery(
                decision,
                "provider/model selection was empty",
                vec!["deadreckon providers list --models".to_string()],
            );
            return Ok(None);
        };
        let route = resolve_explicit_start_provider(paths, defaults, prompt.role, route)?;
        let model = parts.collect::<Vec<_>>().join(" ");
        let model = (!model.is_empty() && model != "provider default").then_some(model);
        return Ok(Some(StartExecutionSelection {
            label: format!(
                "{route} · {}",
                model.as_deref().unwrap_or("provider default")
            ),
            detail: "typed for this launch".to_string(),
            provider: route,
            model,
        }));
    }
    set_start_recovery(
        decision,
        "guided start cancelled before choosing the execution team",
        vec![format!(
            "deadreckon start \"{}\" --provider <provider> --model <model>",
            shell_display_quote(&decision.goal)
        )],
    );
    Ok(None)
}

fn apply_primary_execution_selection(
    decision: &mut StartLaunchDecision,
    selection: &StartExecutionSelection,
) {
    decision.provider_source = StartProviderSource::Interactive;
    decision.provider_route = Some(selection.provider.clone());
    decision.model = selection.model.clone();
    decision.provider_label = format!(
        "{} / {} ({})",
        selection.provider,
        selection.model.as_deref().unwrap_or("provider default"),
        decision.provider_source.label()
    );
}

pub(crate) fn prompt_start_primary_execution(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let current_provider = decision.provider_route.clone();
    let current_model = decision.model.clone();
    if let Some(selection) = prompt_start_execution_selection(
        decision,
        paths,
        defaults,
        StartExecutionPrompt {
            role: setup::SetupProviderRoleRef::PrimaryRun,
            title: "Choose execution team".to_string(),
            help: "Choose one provider and model for this run. Defaults are not changed."
                .to_string(),
            current_provider: current_provider.as_deref(),
            current_model: current_model.as_deref(),
        },
        prompter,
    )? {
        apply_primary_execution_selection(decision, &selection);
    }
    Ok(())
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

fn prompt_start_role_execution(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    prompt: &StartRoleExecutionPrompt<'_>,
    prompter: &mut dyn StartPrompter,
) -> Result<Option<StartExecutionSelection>> {
    let title = format!("Choose {}", prompt.label);
    let help = format!(
        "Choose the {} provider and model for this launch. Defaults are not changed.",
        prompt.label
    );
    prompt_start_execution_selection(
        decision,
        paths,
        defaults,
        StartExecutionPrompt {
            role: prompt.role,
            title,
            help,
            current_provider: prompt.current_provider,
            current_model: prompt.current_model,
        },
        prompter,
    )
}

struct StartRoleExecutionPrompt<'a> {
    role: setup::SetupProviderRoleRef,
    label: String,
    current_provider: Option<&'a str>,
    current_model: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartTeamChoice {
    Uniform(StartExecutionSelection),
    Customize,
    Cancel,
}

fn start_team_role_label(mode: StartSelectedMode, count: Option<u8>) -> String {
    match mode {
        StartSelectedMode::Review => "the implementor and reviewer".to_string(),
        StartSelectedMode::FullPlan => {
            format!("the planner and {} implementors", count.unwrap_or(3))
        }
        StartSelectedMode::Campaign => {
            format!("the planner and {} sub-orchestrators", count.unwrap_or(3))
        }
        StartSelectedMode::Run | StartSelectedMode::Extend => "this run".to_string(),
    }
}

fn prompt_start_execution_team(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    prompter: &mut dyn StartPrompter,
) -> Result<StartTeamChoice> {
    let current_provider = decision.provider_route.clone();
    let current_model = decision.model.clone();
    let (selections, default_index) = start_execution_selections(
        paths,
        defaults,
        current_provider.as_deref(),
        current_model.as_deref(),
    )?;
    let roles = start_team_role_label(decision.selected_mode, decision.child_count);
    let mut choices = selections
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            prompt::SelectChoice::with_detail(
                format!("team:{index}"),
                format!("Use {} for {roles}", selection.label),
                selection.detail.clone(),
            )
        })
        .collect::<Vec<_>>();
    choices.push(start_prompt_choice(
        "customize",
        "Customize execution team",
        "choose provider and model per role; individual child overrides are advanced",
    ));
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Configure execution team".to_string(),
        help: Some(
            "One choice applies a provider/model pair to every required role; customize only when roles should differ."
                .to_string(),
        ),
        choices,
        default_index: default_index.min(selections.len().saturating_sub(1)),
    })?;
    if let Some(raw) = choice.id.strip_prefix("team:") {
        let index = raw.parse::<usize>().map_err(|_| {
            CliError::Core(deadreckon_core::user_error(
                "execution-team selection was invalid",
                "run deadreckon start again",
            ))
        })?;
        return selections
            .get(index)
            .cloned()
            .map(StartTeamChoice::Uniform)
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "execution-team selection was out of range",
                    "run deadreckon start again",
                ))
            });
    }
    Ok(match choice.id.as_str() {
        "customize" => StartTeamChoice::Customize,
        _ => StartTeamChoice::Cancel,
    })
}

fn apply_uniform_start_team(
    decision: &mut StartLaunchDecision,
    selection: StartExecutionSelection,
) {
    apply_primary_execution_selection(decision, &selection);
    match decision.selected_mode {
        StartSelectedMode::Review => {
            decision.coder_provider_route = Some(selection.provider.clone());
            decision.coder_model = selection.model.clone();
            decision.reviewer_provider_route = Some(selection.provider);
            decision.reviewer_model = selection.model;
        }
        StartSelectedMode::FullPlan | StartSelectedMode::Campaign => {
            decision.planner_provider_route = Some(selection.provider.clone());
            decision.planner_model = selection.model.clone();
            decision.child_provider_route = Some(selection.provider);
            decision.child_model = selection.model;
        }
        StartSelectedMode::Run | StartSelectedMode::Extend => {}
    }
}

fn prompt_start_child_count(
    decision: &mut StartLaunchDecision,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let recommended = commands::orchestrate::recommend_child_count_for_goal(
        &decision.goal,
        CliPlanMode::FullPlan,
    );
    let current = decision.child_count.unwrap_or(recommended).clamp(2, 6);
    let mut choices = (2..=6)
        .map(|n| {
            let label = if n == recommended {
                format!("{n} implementors (recommended)")
            } else {
                format!("{n} implementors")
            };
            let detail = if n == current {
                "current team size"
            } else {
                "full-plan team size"
            };
            start_prompt_choice(format!("n:{n}"), label, detail)
        })
        .collect::<Vec<_>>();
    choices.push(prompt::SelectChoice::new("cancel", "Cancel"));
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose implementor count".to_string(),
        help: Some(
            "Pick how many isolated implementation workers the planner should create.".to_string(),
        ),
        choices,
        default_index: usize::from(current.saturating_sub(2)),
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

fn prompt_start_child_execution_overrides(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    defaults: &ConfigDefaults,
    n: u8,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Customize individual implementors".to_string(),
        help: Some(format!(
            "Optional provider/model overrides. Implementor indexes are 0 through {}.",
            n.saturating_sub(1)
        )),
        choices: vec![
            start_prompt_choice(
                "none",
                "No individual overrides",
                "all implementors use the team default",
            ),
            start_prompt_choice(
                "customize",
                "Customize implementors",
                "choose an index, provider, and model",
            ),
            prompt::SelectChoice::new("cancel", "Cancel"),
        ],
        default_index: 0,
    })?;
    match choice.id.as_str() {
        "none" => {
            decision.child_provider_overrides.clear();
            decision.child_model_overrides.clear();
            Ok(())
        }
        "customize" => {
            decision.child_provider_overrides.clear();
            decision.child_model_overrides.clear();
            loop {
                let index_choices = (0..n)
                    .map(|index| {
                        prompt::SelectChoice::new(
                            format!("index:{index}"),
                            format!("Implementor {}", usize::from(index) + 1),
                        )
                    })
                    .chain(std::iter::once(prompt::SelectChoice::new(
                        "cancel", "Cancel",
                    )))
                    .collect::<Vec<_>>();
                let index_choice = prompter.select_one(prompt::SelectPrompt {
                    title: "Choose implementor".to_string(),
                    help: Some(
                        "Select the implementor whose provider/model should differ.".to_string(),
                    ),
                    choices: index_choices,
                    default_index: 0,
                })?;
                let Some(raw_index) = index_choice.id.strip_prefix("index:") else {
                    set_start_recovery(
                        decision,
                        "guided start cancelled while customizing implementors",
                        vec![format!(
                            "deadreckon start \"{}\" --mode full-plan --yes",
                            shell_display_quote(&decision.goal)
                        )],
                    );
                    return Ok(());
                };
                let index = raw_index.parse::<u8>().map_err(|_| {
                    CliError::Core(deadreckon_core::user_error(
                        "implementor index was invalid",
                        "run deadreckon start again",
                    ))
                })?;
                let current_provider = decision.child_provider_route.clone();
                let current_model = decision.child_model.clone();
                let Some(selection) = prompt_start_role_execution(
                    decision,
                    paths,
                    defaults,
                    &StartRoleExecutionPrompt {
                        role: setup::SetupProviderRoleRef::ChildOverride(usize::from(index)),
                        label: format!("implementor {} provider and model", usize::from(index) + 1),
                        current_provider: current_provider.as_deref(),
                        current_model: current_model.as_deref(),
                    },
                    prompter,
                )?
                else {
                    return Ok(());
                };
                decision
                    .child_provider_overrides
                    .retain(|value| !value.starts_with(&format!("{index}=")));
                decision
                    .child_provider_overrides
                    .push(format!("{index}={}", selection.provider));
                decision
                    .child_model_overrides
                    .retain(|value| !value.starts_with(&format!("{index}=")));
                if let Some(model) = selection.model {
                    decision
                        .child_model_overrides
                        .push(format!("{index}={model}"));
                }
                if !prompter.confirm("Customize another implementor?", false)? {
                    break;
                }
            }
            Ok(())
        }
        _ => {
            set_start_recovery(
                decision,
                "guided start cancelled before choosing implementor overrides",
                vec![format!(
                    "deadreckon start \"{}\" --mode full-plan --yes",
                    shell_display_quote(&decision.goal)
                )],
            );
            Ok(())
        }
    }
}

fn start_execution_team_flags_present(args: &StartCommandArgs) -> bool {
    args.provider.is_some()
        || args.model.is_some()
        || args.planner_provider.is_some()
        || args.planner_model.is_some()
        || !args.child_provider.is_empty()
        || !args.child_model.is_empty()
        || args.coder_provider.is_some()
        || args.coder_model.is_some()
        || args.reviewer_provider.is_some()
        || args.reviewer_model.is_some()
}

fn explicit_role_model(
    role_provider: Option<&str>,
    role_model: Option<&String>,
    decision: &StartLaunchDecision,
) -> Option<String> {
    role_model.cloned().or_else(|| {
        let primary = decision.provider_route.as_deref()?;
        (role_provider.unwrap_or(primary) == primary)
            .then(|| decision.model.clone())
            .flatten()
    })
}

pub(crate) fn resolve_start_orchestration_options(
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
            let recommended = commands::orchestrate::recommend_child_count_for_goal(
                &decision.goal,
                CliPlanMode::FullPlan,
            );
            if let Some(n) = args.children {
                validate_task_count(usize::from(n)).map_err(CliError::Core)?;
                decision.child_count = Some(n);
            } else if decision.child_count.is_none() {
                decision.child_count = Some(recommended);
            }

            if matches!(decision.selected_mode, StartSelectedMode::Campaign)
                && (!args.child_provider.is_empty() || !args.child_model.is_empty())
            {
                set_start_recovery(
                    decision,
                    "per-child provider/model overrides are only supported by start --mode full-plan",
                    vec![format!(
                        "deadreckon campaign \"{}\" --provider <provider> --model <model>",
                        shell_display_quote(&decision.goal)
                    )],
                );
                return Ok(());
            }

            if !start_execution_team_flags_present(args)
                && let Some(prompter) = prompter.as_mut()
            {
                match prompt_start_execution_team(decision, paths, defaults, &mut **prompter)? {
                    StartTeamChoice::Uniform(selection) => {
                        apply_uniform_start_team(decision, selection);
                    }
                    StartTeamChoice::Customize => {
                        prompt_start_child_count(decision, &mut **prompter)?;
                        if decision.recovery.is_some() {
                            return Ok(());
                        }
                        let current_provider = decision.provider_route.clone();
                        let current_model = decision.model.clone();
                        let Some(planner) = prompt_start_role_execution(
                            decision,
                            paths,
                            defaults,
                            &StartRoleExecutionPrompt {
                                role: setup::SetupProviderRoleRef::Planner,
                                label: "planner provider and model".to_string(),
                                current_provider: current_provider.as_deref(),
                                current_model: current_model.as_deref(),
                            },
                            &mut **prompter,
                        )?
                        else {
                            return Ok(());
                        };
                        decision.planner_provider_route = Some(planner.provider);
                        decision.planner_model = planner.model;

                        let Some(implementor) = prompt_start_role_execution(
                            decision,
                            paths,
                            defaults,
                            &StartRoleExecutionPrompt {
                                role: setup::SetupProviderRoleRef::DefaultChild,
                                label: "implementor provider and model".to_string(),
                                current_provider: current_provider.as_deref(),
                                current_model: current_model.as_deref(),
                            },
                            &mut **prompter,
                        )?
                        else {
                            return Ok(());
                        };
                        decision.child_provider_route = Some(implementor.provider.clone());
                        decision.child_model = implementor.model.clone();
                        apply_primary_execution_selection(decision, &implementor);

                        if matches!(decision.selected_mode, StartSelectedMode::FullPlan) {
                            prompt_start_child_execution_overrides(
                                decision,
                                paths,
                                defaults,
                                decision.child_count.unwrap_or(recommended),
                                &mut **prompter,
                            )?;
                        }
                    }
                    StartTeamChoice::Cancel => {
                        set_start_recovery(
                            decision,
                            "guided start cancelled before choosing the execution team",
                            vec![format!(
                                "deadreckon start \"{}\" --mode full-plan --provider <provider> --model <model>",
                                shell_display_quote(&decision.goal)
                            )],
                        );
                        return Ok(());
                    }
                }
            } else {
                if let Some(route) = args.planner_provider.as_deref() {
                    decision.planner_provider_route = Some(resolve_explicit_start_provider(
                        paths,
                        defaults,
                        setup::SetupProviderRoleRef::Planner,
                        route,
                    )?);
                }
                decision.child_provider_route = decision.provider_route.clone();
                decision.planner_model = explicit_role_model(
                    decision.planner_provider_route.as_deref(),
                    args.planner_model.as_ref(),
                    decision,
                );
                decision.child_model = decision.model.clone();
            }

            if matches!(decision.selected_mode, StartSelectedMode::FullPlan) {
                let n = decision.child_count.unwrap_or(recommended);
                if !args.child_provider.is_empty() {
                    commands::plan::parse_child_provider_overrides(&args.child_provider, n)?;
                    decision.child_provider_overrides = args.child_provider.clone();
                }
                if !args.child_model.is_empty() {
                    commands::plan::parse_child_model_overrides(&args.child_model, n)?;
                    decision.child_model_overrides = args.child_model.clone();
                }
            }
        }
        StartSelectedMode::Review => {
            if args.children.is_some()
                || args.planner_provider.is_some()
                || args.planner_model.is_some()
                || !args.child_provider.is_empty()
                || !args.child_model.is_empty()
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

            if !start_execution_team_flags_present(args)
                && let Some(prompter) = prompter.as_mut()
            {
                match prompt_start_execution_team(decision, paths, defaults, &mut **prompter)? {
                    StartTeamChoice::Uniform(selection) => {
                        apply_uniform_start_team(decision, selection);
                    }
                    StartTeamChoice::Customize => {
                        let current_provider = decision.provider_route.clone();
                        let current_model = decision.model.clone();
                        let Some(coder) = prompt_start_role_execution(
                            decision,
                            paths,
                            defaults,
                            &StartRoleExecutionPrompt {
                                role: setup::SetupProviderRoleRef::Coder,
                                label: "implementor provider and model".to_string(),
                                current_provider: current_provider.as_deref(),
                                current_model: current_model.as_deref(),
                            },
                            &mut **prompter,
                        )?
                        else {
                            return Ok(());
                        };
                        decision.coder_provider_route = Some(coder.provider.clone());
                        decision.coder_model = coder.model.clone();
                        apply_primary_execution_selection(decision, &coder);

                        let Some(reviewer) = prompt_start_role_execution(
                            decision,
                            paths,
                            defaults,
                            &StartRoleExecutionPrompt {
                                role: setup::SetupProviderRoleRef::Reviewer,
                                label: "reviewer provider and model".to_string(),
                                current_provider: current_provider.as_deref(),
                                current_model: current_model.as_deref(),
                            },
                            &mut **prompter,
                        )?
                        else {
                            return Ok(());
                        };
                        decision.reviewer_provider_route = Some(reviewer.provider);
                        decision.reviewer_model = reviewer.model;
                    }
                    StartTeamChoice::Cancel => {
                        set_start_recovery(
                            decision,
                            "guided start cancelled before choosing the execution team",
                            vec![format!(
                                "deadreckon start \"{}\" --mode review --provider <provider> --model <model>",
                                shell_display_quote(&decision.goal)
                            )],
                        );
                        return Ok(());
                    }
                }
            } else {
                if let Some(route) = args.coder_provider.as_deref() {
                    decision.coder_provider_route = Some(resolve_explicit_start_provider(
                        paths,
                        defaults,
                        setup::SetupProviderRoleRef::Coder,
                        route,
                    )?);
                } else {
                    decision.coder_provider_route = decision.provider_route.clone();
                }
                if let Some(route) = args.reviewer_provider.as_deref() {
                    decision.reviewer_provider_route = Some(resolve_explicit_start_provider(
                        paths,
                        defaults,
                        setup::SetupProviderRoleRef::Reviewer,
                        route,
                    )?);
                } else {
                    decision.reviewer_provider_route = decision.provider_route.clone();
                }
                decision.coder_model = explicit_role_model(
                    decision.coder_provider_route.as_deref(),
                    args.coder_model.as_ref(),
                    decision,
                );
                decision.reviewer_model = explicit_role_model(
                    decision.reviewer_provider_route.as_deref(),
                    args.reviewer_model.as_ref(),
                    decision,
                );
            }
        }
        StartSelectedMode::Extend | StartSelectedMode::Run => {}
    }
    Ok(())
}

/// The one question `start` may ask (C-P8), and only when no contract was
/// found or detected: "How will you know it worked?". A one-line answer is
/// compiled through the existing def-done flow. Pressing Enter asks that same
/// flow to draft practical, goal-derived criteria; an unknown project must not
/// silently arm the directory-exists fallback that strict completion rejects.
pub(crate) fn prompt_start_done_criteria(
    decision: &mut StartLaunchDecision,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let text = prompter.input(
        "How will you know it worked? (one line; Enter = draft from goal) ",
        None,
    )?;
    let text = text.trim();
    if text.is_empty() {
        let request = format!(
            "Draft practical acceptance checks that independently prove this goal: {}",
            decision.goal
        );
        decision.done_criteria_source = StartDoneCriteriaSource::Asked;
        decision.done_action = StartDoneAction::ManualText {
            text: request,
            overwrite_existing: false,
        };
        decision.done_criteria_label = "draft goal-derived done contract before launch".to_string();
        return Ok(());
    }
    decision.done_criteria_source = StartDoneCriteriaSource::Asked;
    decision.done_action = StartDoneAction::ManualText {
        text: text.to_string(),
        overwrite_existing: false,
    };
    decision.done_criteria_label = format!("asked at launch: {text}");
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

fn apply_done_criteria_selection(
    decision: &mut StartLaunchDecision,
    source: StartDoneCriteriaSource,
    action: StartDoneAction,
    label: String,
    selection: &setup::DoneCriteriaSelection,
) -> Result<()> {
    decision.done_criteria_source = source;
    decision.done_action = action;
    decision.done_criteria_label = label;
    decision.done_contract = if selection.path.as_ref().is_some_and(|path| path.exists()) {
        commands::acceptance::compiled_contract_for_selection(selection)?
    } else {
        None
    };
    decision.done_divergence = decision
        .done_contract
        .as_ref()
        .map(|contract| commands::acceptance::reconcile(&decision.goal, contract));
    Ok(())
}

fn contract_review_try_lines(decision: &StartLaunchDecision) -> Vec<String> {
    vec![
        "deadreckon acceptance refine \"add a check that builds and runs the app\"".to_string(),
        format!(
            "deadreckon start \"{}\" --review-done",
            shell_display_quote(&decision.goal)
        ),
    ]
}

fn print_start_contract_divergence(decision: &StartLaunchDecision) {
    let Some(contract) = decision.done_contract.as_ref() else {
        return;
    };
    let divergence = decision.done_divergence.as_ref();
    if divergence.is_some_and(commands::acceptance::ContractDivergence::clean) {
        return;
    }
    println!("{}", ui_heading("done contract divergence"));
    commands::acceptance::print_compiled_contract(contract, divergence);
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

pub(crate) fn prompt_start_existing_done_criteria(
    decision: &mut StartLaunchDecision,
    cwd: &Path,
    selection: &setup::DoneCriteriaSelection,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    loop {
        let compiled = commands::acceptance::compiled_contract_for_selection(selection)
            .ok()
            .flatten();
        let divergence = compiled
            .as_ref()
            .map(|contract| commands::acceptance::reconcile(&decision.goal, contract));
        if let Some(contract) = compiled.as_ref() {
            println!("{}", ui_heading(format!("Review {NOUN_DONE_CONTRACT}")));
            commands::acceptance::print_compiled_contract(contract, divergence.as_ref());
        }
        let choice = prompter.select_one(prompt::SelectPrompt {
            title: format!("Review {NOUN_DONE_CONTRACT}"),
            help: Some(format!(
                "Current {NOUN_DONE_CONTRACT}: {}. You can accept, re-prompt, edit, check, or cancel before launch.",
                done_criteria_prompt_detail(selection)
            )),
            choices: vec![
                start_prompt_choice(
                    "keep",
                    format!("Accept current {NOUN_DONE_CONTRACT}"),
                    "uses the compiled checks shown above",
                ),
                start_prompt_choice(
                    "view",
                    "View compiled checks",
                    "prints real checks, behavior/falsifiability labels, and divergence",
                ),
                start_prompt_choice(
                    "check",
                    "Check current contract now",
                    "dry-runs the configured checks against this working tree",
                ),
                start_prompt_choice(
                    "update",
                    "Re-prompt compiler before launch",
                    "uses your note plus the run goal to compile a replacement contract",
                ),
                start_prompt_choice(
                    "edit",
                    "Edit contract files",
                    "prints the YAML and Markdown paths for manual editing, then returns here",
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
                apply_done_criteria_selection(
                    decision,
                    StartDoneCriteriaSource::Project,
                    StartDoneAction::Existing,
                    selection.full_label(),
                    selection,
                )?;
                return Ok(());
            }
            "view" => {
                print_start_done_criteria_summary(selection);
                if let Some(contract) = compiled.as_ref() {
                    commands::acceptance::print_compiled_contract(contract, divergence.as_ref());
                }
            }
            "check" => check_start_done_criteria(cwd, selection)?,
            "update" => {
                let text = prompter.input("updated definition of done: ", None)?;
                if text.trim().is_empty() {
                    set_start_recovery(
                        decision,
                        format!("empty {NOUN_DONE_CONTRACT} was not saved"),
                        vec!["deadreckon acceptance draft \"<criteria>\"".to_string()],
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
            "edit" => {
                println!("{}", ui_heading(format!("Edit {NOUN_DONE_CONTRACT}")));
                if let Some(path) = selection.path.as_ref() {
                    println!("  yaml: {}", path.display());
                }
                if let Some(path) = selection.companion_doc.as_ref() {
                    println!("  notes: {}", path.display());
                }
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

pub(crate) fn start_launch_preview_facts(decision: &StartLaunchDecision) -> LaunchPreviewFacts<'_> {
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
        model: decision.model.clone(),
        roles: start_provider_role_summary(decision),
        base: decision.base_run_label.clone(),
        history: decision.history_action_label.clone(),
        done: &decision.done_criteria_label,
        workspace: &decision.source_mode_label,
        watch: "deadreckon attach <id>".to_string(),
        stop: "deadreckon kill <id>".to_string(),
        finish: "deadreckon finish <id>".to_string(),
        override_command,
    }
}

pub(crate) fn start_provider_role_summary(decision: &StartLaunchDecision) -> Option<String> {
    let role = |provider: &str, model: Option<&str>| {
        format!("{provider}/{}", model.unwrap_or("provider default"))
    };
    match decision.selected_mode {
        StartSelectedMode::Extend | StartSelectedMode::Run => None,
        StartSelectedMode::Review => {
            let route = decision.provider_route.as_deref()?;
            let coder = decision.coder_provider_route.as_deref().unwrap_or(route);
            let reviewer = decision.reviewer_provider_route.as_deref().unwrap_or(route);
            Some(format!(
                "implementor={}, reviewer={}",
                role(
                    coder,
                    start_role_model(decision, coder, decision.coder_model.as_deref())
                ),
                role(
                    reviewer,
                    start_role_model(decision, reviewer, decision.reviewer_model.as_deref())
                )
            ))
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
            let mut summary = format!(
                "implementors={n}, planner={}, implementor={}",
                role(
                    planner,
                    start_role_model(decision, planner, decision.planner_model.as_deref())
                ),
                role(
                    child,
                    start_role_model(decision, child, decision.child_model.as_deref())
                )
            );
            if !decision.child_provider_overrides.is_empty() {
                summary.push_str(", overrides=");
                summary.push_str(&decision.child_provider_overrides.join(","));
            }
            if !decision.child_model_overrides.is_empty() {
                summary.push_str(", model-overrides=");
                summary.push_str(&decision.child_model_overrides.join(","));
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
            Some(format!(
                "subs={n}, planner={}, implementor={}",
                role(
                    planner,
                    start_role_model(decision, planner, decision.planner_model.as_deref())
                ),
                role(
                    child,
                    start_role_model(decision, child, decision.child_model.as_deref())
                )
            ))
        }
    }
}

fn start_role_model<'a>(
    decision: &'a StartLaunchDecision,
    provider: &str,
    role_model: Option<&'a str>,
) -> Option<&'a str> {
    role_model.or_else(|| {
        (decision.provider_route.as_deref() == Some(provider))
            .then_some(decision.model.as_deref())
            .flatten()
    })
}

fn resolve_start_setup(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    prompter: Option<&mut dyn StartPrompter>,
    stdin_is_tty: bool,
) -> Result<()> {
    if let Some(prompter) = prompter {
        resolve_start_source_setup(decision, args, Some(&mut *prompter), stdin_is_tty)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_provider_and_done_setup(decision, args, Some(prompter))
    } else {
        resolve_start_source_setup(decision, args, None, stdin_is_tty)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_provider_and_done_setup(decision, args, None)
    }
}

fn resolve_start_source_setup(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    prompter: Option<&mut dyn StartPrompter>,
    stdin_is_tty: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let cwd = std::env::current_dir()?;
    validate_start_source_compatibility(decision, args, &cwd);
    if decision.recovery.is_some() {
        return Ok(());
    }
    if decision.resolved_source.is_none()
        && !matches!(
            decision.selected_mode,
            StartSelectedMode::Extend | StartSelectedMode::Campaign
        )
    {
        resolve_start_source_mode(
            decision,
            &paths,
            &cwd,
            prompter,
            StartSourceModeRequest {
                fresh: args.fresh,
                worktree: args.worktree,
                from: args.from.as_deref(),
                allow_dirty: args.allow_dirty,
                stdin_is_tty,
            },
        )?;
    }
    Ok(())
}

fn resolve_start_provider_and_done_setup(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    prompter: Option<&mut dyn StartPrompter>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let cwd = std::env::current_dir()?;
    let (write_root, inspect_root) = decision.resolved_source.as_ref().map_or_else(
        || (cwd.clone(), cwd.clone()),
        |source| {
            (
                source.contract_write_root.clone(),
                source.inspection_root.clone(),
            )
        },
    );
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
        resolve_start_done_criteria_with_roots(
            decision,
            &write_root,
            &inspect_root,
            Some(prompter),
            args.yes,
            !args.json && !args.quiet,
        )
    } else {
        resolve_start_provider(decision, args, &paths, &defaults, None)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_orchestration_options(decision, args, &paths, &defaults, None)?;
        if decision.recovery.is_some() {
            return Ok(());
        }
        resolve_start_done_criteria_with_roots(
            decision,
            &write_root,
            &inspect_root,
            None,
            args.yes,
            !args.json && !args.quiet,
        )
    }
}

fn validate_start_source_compatibility(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    cwd: &Path,
) {
    if start_source_flags_present(args)
        && matches!(
            decision.selected_mode,
            StartSelectedMode::Extend | StartSelectedMode::Campaign
        )
    {
        set_start_recovery(
            decision,
            format!(
                "{} launches do not accept a replacement source",
                decision.selected_mode.label()
            ),
            vec![format!(
                "deadreckon start \"{}\" --mode {}",
                shell_display_quote(&decision.goal),
                decision.selected_mode.label()
            )],
        );
        return;
    }
    if args.allow_dirty
        && args.from.is_none()
        && matches!(
            decision.selected_mode,
            StartSelectedMode::Review | StartSelectedMode::FullPlan
        )
    {
        set_start_recovery(
            decision,
            "Graph Jobs freeze dirty and untracked inputs through an explicit copy source",
            vec![format!(
                "deadreckon start \"{}\" --mode {} --from {}",
                shell_display_quote(&decision.goal),
                decision.selected_mode.label(),
                cwd.display()
            )],
        );
    }
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
            if matches!(
                decision.selected_mode,
                StartSelectedMode::Run | StartSelectedMode::Extend
            ) {
                prompt_start_primary_execution(decision, paths, defaults, &mut **prompter)?;
            }
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
    } else {
        decision.provider_label = format!("{provider} ({})", decision.provider_source.label());
    }
    if args.model.is_none()
        && matches!(
            decision.selected_mode,
            StartSelectedMode::Run | StartSelectedMode::Extend
        )
        && let Some(prompter) = prompter.as_mut()
    {
        prompt_start_primary_execution(decision, paths, defaults, &mut **prompter)?;
    }
    Ok(())
}

fn detected_start_provider_label(provider: &str) -> String {
    format!("{provider} (detected) - run deadreckon config provider {provider} to make permanent")
}

#[cfg(test)]
pub(crate) fn resolve_start_done_criteria(
    decision: &mut StartLaunchDecision,
    cwd: &Path,
    prompter: Option<&mut dyn StartPrompter>,
    yes: bool,
    emit_human_output: bool,
) -> Result<()> {
    resolve_start_done_criteria_with_roots(decision, cwd, cwd, prompter, yes, emit_human_output)
}

fn resolve_start_done_criteria_with_roots(
    decision: &mut StartLaunchDecision,
    write_root: &Path,
    inspect_root: &Path,
    prompter: Option<&mut dyn StartPrompter>,
    yes: bool,
    emit_human_output: bool,
) -> Result<()> {
    let source = commands::acceptance::resolve_acceptance_source(write_root, None)?;
    if source.is_some() {
        let selection = commands::acceptance::done_criteria_selection(&source)?;
        if let Some(prompter) = prompter {
            prompt_start_existing_done_criteria(decision, write_root, &selection, prompter)?;
            return Ok(());
        }
        apply_done_criteria_selection(
            decision,
            StartDoneCriteriaSource::Project,
            StartDoneAction::Existing,
            selection.full_label(),
            &selection,
        )?;
        if decision
            .done_divergence
            .as_ref()
            .is_some_and(commands::acceptance::ContractDivergence::strong)
            && yes
        {
            set_start_recovery(
                decision,
                "done contract does not cover the run goal strongly enough for --yes",
                contract_review_try_lines(decision),
            );
        } else if emit_human_output {
            print_start_contract_divergence(decision);
        }
        return Ok(());
    }

    // C-P8: the one-question flow. A detected contract means everything
    // about "done" is already known — zero questions, interactive or not.
    let contract = commands::course::contract_signal(inspect_root);
    if contract.detected {
        decision.done_criteria_source = StartDoneCriteriaSource::Detected;
        decision.done_action = StartDoneAction::DefaultGate;
        decision.done_criteria_label = format!(
            "{} [detected]",
            contract.command.as_deref().unwrap_or("detected contract")
        );
        return Ok(());
    }

    if let Some(prompter) = prompter {
        prompt_start_done_criteria(decision, prompter)?;
        return Ok(());
    }

    if yes {
        // `--yes` may approve a resolved launch, but it cannot invent and
        // approve a definition of done. Unknown projects fail closed before a
        // durable Job is created instead of starting with an unsealable gate.
        decision.done_criteria_source = StartDoneCriteriaSource::Missing;
        decision.done_action = StartDoneAction::Missing;
        decision.done_criteria_label = format!("missing {NOUN_DONE_CONTRACT}");
        set_start_recovery(
            decision,
            "unknown project has no durable done contract; --yes cannot approve the directory-exists fallback",
            vec![format!(
                "deadreckon def-done \"what should count as done\" && deadreckon start \"{}\" --yes",
                shell_display_quote(&decision.goal)
            )],
        );
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
                    set_resolved_start_source(
                        decision,
                        cwd,
                        commands::job::DurableSourceMode::InitGit,
                        None,
                        cwd,
                        false,
                        StartSourceProvenance::InitGit,
                    )?;
                    return Ok(());
                }
                StartNonGitChoice::Copy => {
                    flags.from = Some(cwd.to_path_buf());
                }
                StartNonGitChoice::Fresh => {
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
                    allowed_dirty_paths: Vec::new(),
                },
            );
            match first {
                Ok(_) => {
                    decision.source_mode = StartSourceMode::Worktree;
                    decision.source_mode_label = format!("worktree from {}", source_path.display());
                    decision.source_worktree = flags.worktree;
                    decision.source_allow_dirty = request.allow_dirty;
                    set_resolved_start_source(
                        decision,
                        cwd,
                        commands::job::DurableSourceMode::Worktree,
                        None,
                        &source_path,
                        request.allow_dirty,
                        StartSourceProvenance::CurrentCleanWorktree,
                    )?;
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
                                        allowed_dirty_paths: Vec::new(),
                                    },
                                )?;
                                decision.source_mode = StartSourceMode::Worktree;
                                decision.source_mode_label = format!(
                                    "worktree from {} with dirty files",
                                    source_path.display()
                                );
                                decision.source_worktree = flags.worktree;
                                decision.source_allow_dirty = true;
                                set_resolved_start_source(
                                    decision,
                                    cwd,
                                    commands::job::DurableSourceMode::Worktree,
                                    None,
                                    &source_path,
                                    true,
                                    StartSourceProvenance::CurrentCleanWorktree,
                                )?;
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
            decision.source_from = Some(source_path.clone());
            set_resolved_start_source(
                decision,
                cwd,
                commands::job::DurableSourceMode::Copy,
                Some(source_path.clone()),
                &source_path,
                false,
                StartSourceProvenance::ExplicitCopy,
            )?;
        }
        ResolvedMode::Fresh => {
            decision.source_mode = StartSourceMode::Fresh;
            decision.source_mode_label = "fresh".to_string();
            decision.source_fresh = true;
            set_resolved_start_source(
                decision,
                cwd,
                commands::job::DurableSourceMode::Fresh,
                None,
                cwd,
                false,
                StartSourceProvenance::ExplicitFresh,
            )?;
        }
        ResolvedMode::InPlace { source_path } => {
            decision.source_mode = StartSourceMode::Copy;
            decision.source_mode_label = format!("in-place from {}", source_path.display());
            decision.source_from = Some(source_path.clone());
            set_resolved_start_source(
                decision,
                cwd,
                commands::job::DurableSourceMode::Copy,
                Some(source_path.clone()),
                &source_path,
                false,
                StartSourceProvenance::ExplicitCopy,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn set_resolved_start_source(
    decision: &mut StartLaunchDecision,
    contract_write_root: &Path,
    mode: commands::job::DurableSourceMode,
    requested_from: Option<PathBuf>,
    inspection_root: &Path,
    allow_dirty: bool,
    provenance: StartSourceProvenance,
) -> Result<()> {
    let contract_write_root = contract_write_root.canonicalize()?;
    let inspection_root = inspection_root.canonicalize().map_err(|error| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "source directory cannot be resolved: {} ({error})",
                inspection_root.display()
            ),
            "deadreckon start \"<goal>\" --mode full-plan --from <existing-dir>",
        ))
    })?;
    if !inspection_root.is_dir() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("source is not a directory: {}", inspection_root.display()),
            "deadreckon start \"<goal>\" --mode full-plan --from <existing-dir>",
        )));
    }
    fs::read_dir(&inspection_root).map_err(|error| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "source directory cannot be read: {} ({error})",
                inspection_root.display()
            ),
            "deadreckon start \"<goal>\" --mode full-plan --from <readable-dir>",
        ))
    })?;
    let resolved = ResolvedStartSource {
        mode,
        requested_from,
        inspection_root: inspection_root.clone(),
        contract_write_root,
        allow_dirty,
        provenance,
    };
    if let Some(existing) = decision.resolved_source.as_ref() {
        if existing == &resolved {
            return Ok(());
        }
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "start source was already resolved and cannot be reinterpreted".to_string(),
        )));
    }
    decision.source_mode_label = match mode {
        commands::job::DurableSourceMode::Copy => {
            format!("approved copy from {}", inspection_root.display())
        }
        commands::job::DurableSourceMode::Fresh => "fresh approved source".to_string(),
        commands::job::DurableSourceMode::InitGit => {
            format!("git init from {}", inspection_root.display())
        }
        commands::job::DurableSourceMode::Worktree if allow_dirty => {
            format!(
                "worktree from {} with dirty files",
                inspection_root.display()
            )
        }
        commands::job::DurableSourceMode::Worktree => {
            format!("worktree from {}", inspection_root.display())
        }
    };
    decision.resolved_source = Some(resolved);
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
            .first()
            .cloned()
            .unwrap_or_else(|| "deadreckon doctor".to_string()),
    }
}

pub(crate) fn start_done_materialization_request(
    decision: &StartLaunchDecision,
) -> Option<(String, bool)> {
    match decision.done_action.clone() {
        StartDoneAction::ManualText {
            text,
            overwrite_existing,
        } => Some((text, overwrite_existing)),
        StartDoneAction::Existing | StartDoneAction::DefaultGate | StartDoneAction::Missing => None,
    }
}

fn start_done_authoring_selection(
    decision: &StartLaunchDecision,
) -> (Option<String>, Option<String>) {
    let (provider, role_model) = match decision.selected_mode {
        StartSelectedMode::FullPlan | StartSelectedMode::Campaign => (
            decision
                .planner_provider_route
                .as_deref()
                .or(decision.provider_route.as_deref()),
            decision.planner_model.as_deref(),
        ),
        StartSelectedMode::Review => (
            decision
                .coder_provider_route
                .as_deref()
                .or(decision.provider_route.as_deref()),
            decision.coder_model.as_deref(),
        ),
        StartSelectedMode::Run | StartSelectedMode::Extend => (
            decision.provider_route.as_deref(),
            decision.model.as_deref(),
        ),
    };
    let model = provider
        .and_then(|provider| start_role_model(decision, provider, role_model))
        .map(str::to_string);
    (provider.map(str::to_string), model)
}

fn recover_start_done_authoring(
    decision: &mut StartLaunchDecision,
    request: &str,
    overwrite_existing: bool,
    error: &CliError,
    prompter: &mut dyn StartPrompter,
) -> Result<bool> {
    eprintln!(
        "{}",
        ui_warn(format!("done contract authoring stopped: {error}"))
    );
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Recover done contract authoring".to_string(),
        help: Some(
            "Your source and execution-team choices are preserved. Retry, revise only the done wording, or stop before launch."
                .to_string(),
        ),
        choices: vec![
            start_prompt_choice(
                "retry",
                "Retry the same execution team",
                "keeps the selected provider, model, source, and launch shape",
            ),
            start_prompt_choice(
                "revise",
                "Revise the definition of done",
                "keeps every other start choice and recompiles the contract",
            ),
            prompt::SelectChoice::new("cancel", "Stop before launch"),
        ],
        default_index: 0,
    })?;
    match choice.id.as_str() {
        "retry" => Ok(true),
        "revise" => {
            let revised = prompter.input("revised definition of done: ", Some(request))?;
            if revised.trim().is_empty() {
                set_start_recovery(
                    decision,
                    "empty done criteria were not saved",
                    vec![format!(
                        "deadreckon start \"{}\"",
                        shell_display_quote(&decision.goal)
                    )],
                );
                return Ok(false);
            }
            decision.done_action = StartDoneAction::ManualText {
                text: revised.trim().to_string(),
                overwrite_existing,
            };
            Ok(true)
        }
        _ => {
            set_start_recovery(
                decision,
                "guided start stopped after done contract authoring failed",
                vec![
                    format!(
                        "deadreckon start \"{}\"",
                        shell_display_quote(&decision.goal)
                    ),
                    "deadreckon doctor".to_string(),
                    "deadreckon update --check".to_string(),
                ],
            );
            Ok(false)
        }
    }
}

async fn materialize_start_done_criteria(
    decision: &mut StartLaunchDecision,
    mut prompter: Option<&mut dyn StartPrompter>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (write_root, inspect_root) = decision.resolved_source.as_ref().map_or_else(
        || (cwd.clone(), cwd.clone()),
        |source| {
            (
                source.contract_write_root.clone(),
                source.inspection_root.clone(),
            )
        },
    );
    let (authoring_provider, authoring_model) = start_done_authoring_selection(decision);
    loop {
        let Some((request, overwrite_existing)) = start_done_materialization_request(decision)
        else {
            return Ok(());
        };
        let context = commands::acceptance::AcceptanceAuthoringContext {
            write_root: &write_root,
            inspect_root: &inspect_root,
            goal: Some(&decision.goal),
        };
        let authoring_result = if prompter.is_some() {
            commands::acceptance::acceptance_agent_command_with_explicit_review(
                context,
                commands::acceptance::AcceptanceAgentMode::Draft,
                vec![request.clone()],
                authoring_provider.clone(),
                authoring_model.clone(),
                overwrite_existing,
            )
            .await
        } else {
            commands::acceptance::acceptance_agent_command_with_context(
                context,
                commands::acceptance::AcceptanceAgentMode::Draft,
                vec![request.clone()],
                authoring_provider.clone(),
                authoring_model.clone(),
                overwrite_existing,
            )
            .await
        };
        if let Err(error) = authoring_result {
            let Some(active_prompter) = prompter.as_mut() else {
                return Err(error);
            };
            if recover_start_done_authoring(
                decision,
                &request,
                overwrite_existing,
                &error,
                &mut **active_prompter,
            )? {
                continue;
            }
            return Ok(());
        }
        let Some(source) = commands::acceptance::mark_generated_done_criteria(
            commands::acceptance::resolve_acceptance_source(&write_root, None)?,
        ) else {
            return Ok(());
        };
        let selection = commands::acceptance::done_criteria_selection(&Some(source))?;
        apply_done_criteria_selection(
            decision,
            StartDoneCriteriaSource::Generated,
            StartDoneAction::Existing,
            selection.full_label(),
            &selection,
        )?;
        // A contract the drafter just invented is one the operator has never
        // seen — pause for accept / re-prompt / edit before anything launches.
        // Choosing "update" sets a fresh ManualText action, so the loop
        // re-drafts and re-reviews (the re-prompt). Non-interactive paths
        // (--yes, --quiet, replay) keep zero-questions but still surface
        // divergence via the printed compiled contract.
        let Some(active_prompter) = prompter.as_mut() else {
            return Ok(());
        };
        prompt_start_existing_done_criteria(
            decision,
            &write_root,
            &selection,
            &mut **active_prompter,
        )?;
        if decision.recovery.is_some() {
            return Ok(());
        }
    }
}

fn prompt_start_launch_confirmation(
    decision: &mut StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    if args.preview || args.yes || args.quiet {
        return Ok(());
    }
    print_start_preview_surface(decision, args, paths)?;
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

fn start_launch_preview_rows(
    decision: &StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
) -> Result<Vec<(String, String)>> {
    let mut rows = launch_preview_rows(&start_launch_preview_facts(decision));
    if start_done_materialization_request(decision).is_some()
        && let Some(source) = decision.resolved_source.as_ref()
    {
        let defaults = config_defaults(paths)?;
        let (authoring_provider, authoring_model) = start_done_authoring_selection(decision);
        rows.push((
            "done inspect".to_string(),
            source.inspection_root.display().to_string(),
        ));
        rows.push((
            "done writer".to_string(),
            source
                .contract_write_root
                .join(".deadreckon")
                .display()
                .to_string(),
        ));
        rows.push((
            "done provider".to_string(),
            commands::acceptance::done_authoring_route_label(
                paths,
                &defaults,
                authoring_provider.as_deref(),
                authoring_model.as_deref(),
            )
            .unwrap_or_else(|| "unavailable (structured text required)".to_string()),
        ));
        rows.push((
            "done limit".to_string(),
            format!(
                "{}s total",
                commands::acceptance::done_authoring_wall_seconds(&defaults)
            ),
        ));
    }
    let seams = read_seams_config(&paths.config_path(), args.no_seams)?;
    rows.push(("seams".to_string(), seam_preview_label(&seams)));
    rows.push((
        "deadline".to_string(),
        args.deadline
            .as_ref()
            .map(DateTime::to_rfc3339)
            .unwrap_or_else(|| "none".to_string()),
    ));
    Ok(rows)
}

fn start_preview_primary_action(decision: &StartLaunchDecision) -> String {
    if decision.recovery.is_some() {
        return decision
            .try_lines
            .first()
            .cloned()
            .unwrap_or_else(|| "deadreckon doctor".to_string());
    }
    match decision.selected_mode {
        StartSelectedMode::Extend => decision
            .base_run_id
            .as_ref()
            .map(|run_id| {
                format!(
                    "deadreckon extend {} \"{}\"",
                    run_prefix(run_id),
                    shell_display_quote(&decision.goal)
                )
            })
            .unwrap_or_else(|| "deadreckon list".to_string()),
        StartSelectedMode::Campaign => format!(
            "deadreckon campaign \"{}\" --n {} --yes",
            shell_display_quote(&decision.goal),
            decision.child_count.unwrap_or(3)
        ),
        StartSelectedMode::Run | StartSelectedMode::Review | StartSelectedMode::FullPlan => {
            format!(
                "deadreckon start \"{}\" --mode {} --yes",
                shell_display_quote(&decision.goal),
                decision.selected_mode.label()
            )
        }
    }
}

fn start_done_contract_json(decision: &StartLaunchDecision) -> serde_json::Value {
    json!({
        "checks": decision.done_contract.as_ref().map(|contract| &contract.checks),
        "divergence": decision.done_divergence,
    })
}

fn start_source_json(decision: &StartLaunchDecision) -> serde_json::Value {
    decision.resolved_source.as_ref().map_or_else(
        || serde_json::Value::Null,
        |source| {
            json!({
                "mode": source.mode,
                "requested_from": &source.requested_from,
                "inspection_root": &source.inspection_root,
                "contract_write_root": &source.contract_write_root,
                "allow_dirty": source.allow_dirty,
                "provenance": source.provenance.label(),
                "label": &decision.source_mode_label,
            })
        },
    )
}

fn start_done_authoring_json(
    decision: &StartLaunchDecision,
    paths: &DeadreckonPaths,
) -> Result<serde_json::Value> {
    let Some(source) = decision.resolved_source.as_ref() else {
        return Ok(serde_json::Value::Null);
    };
    let defaults = config_defaults(paths)?;
    let (authoring_provider, authoring_model) = start_done_authoring_selection(decision);
    Ok(json!({
        "needed": start_done_materialization_request(decision).is_some(),
        "inspect_root": &source.inspection_root,
        "write_root": source.contract_write_root.join(".deadreckon"),
        "provider": commands::acceptance::done_authoring_route_label(
            paths,
            &defaults,
            authoring_provider.as_deref(),
            authoring_model.as_deref(),
        ),
        "wall_seconds": commands::acceptance::done_authoring_wall_seconds(&defaults),
        "posture": "structured_text",
    }))
}

fn apply_forced_contract_review_guard(
    decision: &mut StartLaunchDecision,
    eligibility: StartPromptEligibility,
    force_contract_review: bool,
) {
    if force_contract_review && decision.recovery.is_none() && !eligibility.allows_prompts() {
        let try_line = format!(
            "deadreckon start \"{}\" --review-done",
            shell_display_quote(&decision.goal)
        );
        set_start_recovery(
            decision,
            "done contract review needs an interactive terminal",
            vec![try_line],
        );
    }
}

fn start_preview_secondary_actions(decision: &StartLaunchDecision) -> Vec<String> {
    if decision.recovery.is_some() {
        return decision.try_lines.iter().skip(1).cloned().collect();
    }
    vec![
        "deadreckon attach <id>".to_string(),
        "deadreckon kill <id>".to_string(),
        "deadreckon finish <id>".to_string(),
    ]
}

fn start_preview_surface(
    decision: &StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
) -> Result<VerdictSurface> {
    let rows = start_launch_preview_rows(decision, args, paths)?;
    let mut evidence = rows
        .into_iter()
        .filter(|(key, _)| !matches!(key.as_str(), "watch" | "stop" | "finish"))
        .collect::<Vec<_>>();
    evidence.push(("will start".to_string(), "false".to_string()));
    let primary = start_preview_primary_action(decision);
    let secondary = start_preview_secondary_actions(decision);
    let (kind, what, why) = if let Some(recovery) = decision.recovery.as_ref() {
        (
            VerdictKind::Blocked,
            recovery.message.clone(),
            "The selected path cannot start until the recommended recovery command resolves the missing setup.".to_string(),
        )
    } else {
        (
            VerdictKind::Preview,
            "DeadReckon classified the goal and is ready to launch, but no run, plan, or campaign id exists yet.".to_string(),
            "This is a preview; post-launch commands become real only after the launch creates an id.".to_string(),
        )
    };
    let secondary_label = if decision.recovery.is_some() {
        "Secondary"
    } else {
        "After start"
    };
    let secondary = secondary
        .iter()
        .map(|command| (secondary_label, command.as_str()))
        .collect::<Vec<_>>();
    Ok(VerdictSurface::must_new(
        kind,
        "start",
        None,
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary,
    ))
}

fn print_start_preview_surface(
    decision: &StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
) -> Result<()> {
    print!(
        "{}",
        start_preview_surface(decision, args, paths)?
            .render_plain(!completion_hints_enabled(false))
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartGoalPlan {
    Provided(String),
    Prompt,
    Notice(String),
}

/// Decide how to obtain the start goal: use a provided non-empty goal, prompt
/// interactively when prompts are allowed (a TTY without --yes/--json/--plain/
/// --quiet), or emit a notice when prompts are suppressed.
pub(crate) fn start_goal_plan(provided: Option<&str>, allows_prompts: bool) -> StartGoalPlan {
    match provided.map(str::trim) {
        Some(goal) if !goal.is_empty() => StartGoalPlan::Provided(goal.to_string()),
        _ if allows_prompts => StartGoalPlan::Prompt,
        _ => StartGoalPlan::Notice(
            "no goal given and prompts are off; pass a goal: deadreckon start \"<goal>\""
                .to_string(),
        ),
    }
}

/// Resolve the start goal, prompting interactively for it when none was given and
/// prompts are allowed. Returns a friendly error (after a one-line notice) when
/// no goal is available and prompts are suppressed.
pub(crate) fn resolve_start_goal(provided: Option<&str>, allows_prompts: bool) -> Result<String> {
    match start_goal_plan(provided, allows_prompts) {
        StartGoalPlan::Provided(goal) => Ok(goal),
        StartGoalPlan::Prompt => {
            let entered = crate::prompt::open("goal: ", None)?;
            let goal = entered.trim().to_string();
            if goal.is_empty() {
                return Err(start_goal_required_error());
            }
            Ok(goal)
        }
        StartGoalPlan::Notice(message) => {
            let _ = ui::writeln(ui::Stream::Stderr, ui::Tone::Note, &message);
            Err(start_goal_required_error())
        }
    }
}

fn start_service_preflight_error(
    preflight: &commands::supervisor_service::SupervisorServicePreflight,
) -> CliError {
    let reason = preflight
        .reason()
        .unwrap_or("machine-restart supervisor readiness could not be proven");
    let recovery = if preflight.setup_is_supported() {
        "deadreckon setup --supervisor"
    } else {
        "deadreckon supervisor status"
    };
    CliError::Core(deadreckon_core::user_error(
        &format!("durable start requires a restart-capable supervisor service; {reason}"),
        recovery,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartSupervisorAdmission {
    Proceed,
    OfferSetup,
    Refuse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartLaunchIntent {
    Inspect,
    Launch,
}

fn start_launch_intent(args: &StartCommandArgs) -> StartLaunchIntent {
    if args.preview || (args.json && !args.yes) {
        StartLaunchIntent::Inspect
    } else {
        StartLaunchIntent::Launch
    }
}

fn emit_start_read_only_result(
    decision: &StartLaunchDecision,
    args: &StartCommandArgs,
    paths: &DeadreckonPaths,
) -> Result<bool> {
    if args.json && !args.yes {
        let surface = start_preview_surface(decision, args, paths)?;
        let mut next_actions = vec![surface.primary_action.command.clone()];
        next_actions.extend(start_preview_secondary_actions(decision));
        if decision.recovery.is_none()
            && !matches!(decision.selected_mode, StartSelectedMode::Extend)
        {
            for action in &decision.history_next_actions {
                if !next_actions.iter().any(|existing| existing == action) {
                    next_actions.push(action.clone());
                }
            }
        }
        let payload = surface.add_to_json(json!({
            "kind": "start",
            "goal": &decision.goal,
            "selected_mode": decision.selected_mode.label(),
            "selection_source": decision.selection_source.label(),
            "reason": &decision.reason,
            "provider": &decision.provider_label,
            "provider_source": decision.provider_source.label(),
            "done_criteria": &decision.done_criteria_label,
            "done_criteria_source": decision.done_criteria_source.label(),
            "done_contract": start_done_contract_json(decision),
            "source_mode": decision.source_mode.label(),
            "source": start_source_json(decision),
            "done_authoring": start_done_authoring_json(decision, paths)?,
            "goal_shape": &decision.goal_shape,
            "requires_confirmation": decision.requires_confirmation,
            "will_start": false,
            "history_actions": &decision.history_next_actions,
            "next_actions": next_actions,
            "try_lines": &decision.try_lines
        }));
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(true);
    }
    if args.preview {
        if !args.quiet {
            print_start_preview_surface(decision, args, paths)?;
        }
        return Ok(true);
    }
    Ok(false)
}

fn start_supervisor_admission(
    preflight: &commands::supervisor_service::SupervisorServicePreflight,
    prompts_allowed: bool,
) -> StartSupervisorAdmission {
    if preflight.is_ready() {
        StartSupervisorAdmission::Proceed
    } else if preflight.setup_is_supported() && prompts_allowed {
        StartSupervisorAdmission::OfferSetup
    } else {
        StartSupervisorAdmission::Refuse
    }
}

/// The ordinary `start` promise includes recovery after a machine restart.
/// Preview remains read-only, while a real TTY launch may perform the explicit
/// one-time install/start only after operator confirmation. Non-interactive
/// launches fail closed and point at the same explicit lifecycle commands.
fn require_start_supervisor_service(mut prompter: Option<&mut dyn StartPrompter>) -> Result<()> {
    let preflight =
        commands::supervisor_service::supervisor_service_preflight().map_err(|error| {
            CliError::Core(deadreckon_core::user_error(
                &format!(
                    "durable start could not prove restart-capable supervisor readiness: {error}"
                ),
                "deadreckon supervisor status",
            ))
        })?;
    match start_supervisor_admission(&preflight, prompter.is_some()) {
        StartSupervisorAdmission::Proceed => return Ok(()),
        StartSupervisorAdmission::Refuse => {
            return Err(start_service_preflight_error(&preflight));
        }
        StartSupervisorAdmission::OfferSetup => {}
    }
    let prior_instance = preflight.instance().cloned();

    let reason = preflight.reason().unwrap_or("setup is required");
    let question =
        format!("{reason}. Install/update and start the per-user DeadReckon supervisor now?");
    let Some(prompter) = prompter.as_mut() else {
        return Err(start_service_preflight_error(&preflight));
    };
    if !prompter.confirm(&question, true)? {
        return Err(start_service_preflight_error(&preflight));
    }

    commands::providers::reconcile_receipt_for_current_binary(&DeadreckonPaths::discover(), false)?;
    commands::supervisor_service::supervisor_service_install_command()?;
    commands::supervisor_service::supervisor_service_start_command()?;

    let deadline =
        std::time::Instant::now() + commands::supervisor_service::supervisor_readiness_timeout();
    loop {
        match commands::supervisor_service::supervisor_service_preflight() {
            Ok(ready)
                if ready.is_ready()
                    && commands::supervisor_service::supervisor_instance_is_fresh_successor(
                        prior_instance.as_ref(),
                        ready.instance(),
                    ) =>
            {
                return Ok(());
            }
            Ok(ready) if ready.is_ready() && std::time::Instant::now() >= deadline => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "the supervisor service manager became ready, but it did not publish a fresh live instance after restart",
                    "deadreckon supervisor status",
                )));
            }
            Ok(not_ready) if std::time::Instant::now() >= deadline => {
                return Err(start_service_preflight_error(&not_ready));
            }
            Err(error) if std::time::Instant::now() >= deadline => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "the supervisor service was started, but readiness could not be proven: {error}"
                    ),
                    "deadreckon supervisor status",
                )));
            }
            Ok(_) | Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

fn start_goal_required_error() -> CliError {
    CliError::Core(deadreckon_core::user_error(
        "start goal required",
        "deadreckon start \"<goal>\"",
    ))
}

/// Replay a saved launch plan (C-P10): validate, re-clamp against the
/// current budget flag, stamp the resolution as a replay, and dispatch the
/// identical shape. The plan file is the decision — no classification, no
/// planning, at most the standard launch confirmation.
async fn start_replay_command(mut args: StartCommandArgs, plan_path: &Path) -> Result<()> {
    let mut plan = commands::course::load_launch_plan(plan_path)?;
    if plan.shape == commands::course::CourseShape::ChainExtend {
        return Err(CliError::Core(deadreckon_core::user_error(
            "a chain-extend plan cannot be replayed (it needs its parent run)",
            "deadreckon start \"<goal>\"",
        )));
    }
    if let (Some(cap), Some(ceiling)) = (args.max_spend, plan.budget.ceiling_usd)
        && ceiling > cap
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("launch plan budget ${ceiling:.2} exceeds --max-spend ${cap:.2}"),
            &format!(
                "deadreckon start --plan {} --max-spend {ceiling:.0}",
                plan_path.display()
            ),
        )));
    }
    if args.max_spend.is_some() {
        plan.budget.ceiling_usd = args.max_spend;
    }
    if args.deadline.is_some() {
        plan.budget.deadline = args.deadline;
    } else {
        args.deadline = plan.budget.deadline;
    }
    plan.resolution.source = commands::course::ResolutionSource::Replay;
    plan.accepted_by = Some("replay".to_string());

    let mut decision = start_launch_decision(StartLaunchInput {
        goal: &plan.goal.clone(),
        requested_mode: match plan.shape {
            commands::course::CourseShape::Single | commands::course::CourseShape::ChainExtend => {
                crate::cli::CliStartMode::Run
            }
            commands::course::CourseShape::Plan => crate::cli::CliStartMode::FullPlan,
            commands::course::CourseShape::Campaign => crate::cli::CliStartMode::Auto,
        },
        stdin_is_tty: io::stdin().is_terminal(),
    });
    if plan.shape == commands::course::CourseShape::Campaign {
        decision.selected_mode = StartSelectedMode::Campaign;
        decision.selection_source = StartSelectionSource::ExplicitFlag;
    }
    decision.reason = format!("replayed launch plan from {}", plan_path.display());
    decision.child_count = plan.n;
    decision.provider_route = plan
        .providers
        .default_child
        .clone()
        .or_else(|| plan.providers.coder.clone());
    decision.model = plan
        .providers
        .default_child_model
        .clone()
        .or_else(|| plan.providers.coder_model.clone());
    decision.planner_provider_route = plan.providers.planner.clone();
    decision.planner_model = plan.providers.planner_model.clone();
    decision.child_provider_route = plan.providers.default_child.clone();
    decision.child_model = plan.providers.default_child_model.clone();
    decision.child_provider_overrides = plan
        .providers
        .children
        .iter()
        .map(|(index, provider)| format!("{index}={provider}"))
        .collect();
    decision.child_model_overrides = plan
        .providers
        .child_models
        .iter()
        .map(|(index, model)| format!("{index}={model}"))
        .collect();
    decision.coder_provider_route = plan.providers.coder.clone();
    decision.coder_model = plan.providers.coder_model.clone();
    decision.reviewer_provider_route = plan.providers.reviewer.clone();
    decision.reviewer_model = plan.providers.reviewer_model.clone();
    decision.confirmed_by_start_picker = args.yes;
    let mut replay_args = args;
    replay_args.goal = plan.goal.clone();
    replay_args.provider = decision.provider_route.clone();
    replay_args.model = decision.model.clone();
    replay_args.planner_provider = decision.planner_provider_route.clone();
    replay_args.planner_model = decision.planner_model.clone();
    replay_args.child_provider = decision.child_provider_overrides.clone();
    replay_args.child_model = decision.child_model_overrides.clone();
    replay_args.coder_provider = decision.coder_provider_route.clone();
    replay_args.coder_model = decision.coder_model.clone();
    replay_args.reviewer_provider = decision.reviewer_provider_route.clone();
    replay_args.reviewer_model = decision.reviewer_model.clone();
    if plan.shape == commands::course::CourseShape::Single {
        // A Single launch plan stores its execution provider in the historical
        // `coder` slot. It is already projected into the primary provider/model
        // above; retaining the role-specific fields would make setup mistake a
        // Single replay for review orchestration.
        replay_args.planner_provider = None;
        replay_args.planner_model = None;
        replay_args.child_provider.clear();
        replay_args.child_model.clear();
        replay_args.coder_provider = None;
        replay_args.coder_model = None;
        replay_args.reviewer_provider = None;
        replay_args.reviewer_model = None;
    }
    // Replays never prompt for setup; the plan is the decision. The accept
    // matrix still applies at dispatch (campaign guardrails included).
    resolve_start_setup(&mut decision, &replay_args, None, false)?;
    decision.child_count = plan.n;
    if let Some(recovery) = decision.recovery.as_ref() {
        let mut try_lines = recovery.try_lines.clone();
        try_lines.truncate(1);
        return Err(CliError::Core(deadreckon_core::user_error(
            &recovery.message,
            try_lines
                .first()
                .map(String::as_str)
                .unwrap_or("deadreckon try"),
        )));
    }
    let paths = DeadreckonPaths::discover();
    if emit_start_read_only_result(&decision, &replay_args, &paths)? {
        return Ok(());
    }
    let eligibility = StartPromptEligibility::from_args(&replay_args, io::stdin().is_terminal());
    let mut terminal_prompter = TerminalStartPrompter;
    let prompter = eligibility
        .allows_prompts()
        .then_some(&mut terminal_prompter as &mut dyn StartPrompter);
    materialize_start_done_criteria(&mut decision, None).await?;
    require_start_supervisor_service(prompter)?;
    dispatch_start_command(replay_args, &decision, plan)
        .await
        .map(|_| ())
}

pub(crate) async fn start_command(args: StartCommandArgs) -> Result<()> {
    if let Some(plan_path) = args.plan.clone() {
        return start_replay_command(args, &plan_path).await;
    }
    let stdin_is_tty = io::stdin().is_terminal();
    let paths = DeadreckonPaths::discover();
    let cwd = std::env::current_dir()?;
    let latest_extendable_run = start_latest_extendable_run(&paths, &cwd)?;
    let mut decision = start_launch_decision(StartLaunchInput {
        goal: &args.goal,
        requested_mode: args.mode,
        stdin_is_tty,
    });
    decision.model = args.model.clone();
    add_start_history_actions(&mut decision, latest_extendable_run.as_ref());
    let eligibility = StartPromptEligibility::from_args(&args, stdin_is_tty);
    let mut terminal_prompter = TerminalStartPrompter;

    // Soundings: source policy is an admission decision, not a dispatch
    // afterthought. Resolve it once before any provider process, contract
    // authoring, service-install prompt, or final launch confirmation.
    let source_prompter = eligibility
        .allows_prompts()
        .then_some(&mut terminal_prompter as &mut dyn StartPrompter);
    resolve_start_source_setup(&mut decision, &args, source_prompter, stdin_is_tty)?;

    if decision.recovery.is_none() && start_goal_shape_should_classify(&args, eligibility) {
        let inspection_root = decision
            .resolved_source
            .as_ref()
            .map(|source| source.inspection_root.as_path())
            .unwrap_or(cwd.as_path());
        let defaults = config_defaults(&paths)?;
        // A source-bearing auto launch first takes the deterministic ladder.
        // That lets an unsupported Campaign/source combination refuse without
        // ever starting a provider merely to discover that it cannot launch.
        let provider = (!start_source_flags_present(&args)
            && start_should_ask_provider_for_shape(&args, eligibility))
        .then(|| start_goal_shape_provider_route(&paths, &defaults, &args))
        .flatten();
        let planner_sandbox = defaults
            .sandbox
            .as_deref()
            .unwrap_or("auto")
            .parse::<deadreckon_sandbox::SandboxBackend>()?;
        let recommendation = classify_goal_shape_for_start(
            &paths,
            inspection_root,
            &args.goal,
            provider.as_deref(),
            planner_sandbox,
            args.plain,
        )
        .await?;
        apply_goal_shape_recommendation(&mut decision, recommendation.clone());
        validate_start_source_compatibility(&mut decision, &args, &cwd);
        if decision.recovery.is_none() {
            let scope = workspace_scope(&cwd)?;
            write_goal_shape_preview_record(&paths, &scope, &recommendation)?;
        }
    }
    if decision.recovery.is_none() && eligibility.allows_prompts() {
        maybe_prompt_start_mode(
            &mut decision,
            &args,
            latest_extendable_run.as_ref(),
            &mut terminal_prompter,
        )?;
        if decision.recovery.is_none() {
            validate_start_source_compatibility(&mut decision, &args, &cwd);
        }
        if decision.recovery.is_none() {
            resolve_start_provider_and_done_setup(
                &mut decision,
                &args,
                Some(&mut terminal_prompter),
            )?;
        }
    } else if decision.recovery.is_none() {
        resolve_start_provider_and_done_setup(&mut decision, &args, None)?;
    }
    let force_contract_review = args.review_done
        || config_defaults(&paths)
            .ok()
            .and_then(|defaults| defaults.start_confirm_contract)
            .unwrap_or(false);
    apply_forced_contract_review_guard(&mut decision, eligibility, force_contract_review);
    if emit_start_read_only_result(&decision, &args, &paths)? {
        return Ok(());
    }
    if decision.recovery.is_some() {
        let surface = start_preview_surface(&decision, &args, &paths)?
            .render_plain(!completion_hints_enabled(false));
        return Err(CliError::Surface { code: 1, surface });
    }
    let review_prompter: Option<&mut dyn StartPrompter> = eligibility
        .allows_prompts()
        .then_some(&mut terminal_prompter as &mut dyn StartPrompter);
    materialize_start_done_criteria(&mut decision, review_prompter).await?;
    if let Some(recovery) = decision.recovery.as_ref() {
        return Err(start_recovery_error(recovery));
    }
    if decision.recovery.is_none() && eligibility.allows_prompts() {
        prompt_start_launch_confirmation(&mut decision, &args, &paths, &mut terminal_prompter)?;
    }
    // Service installation is the first durable machine mutation. Perform it
    // only after provider-backed contract authoring and the operator's final
    // launch confirmation have both succeeded.
    if decision.recovery.is_none() && start_launch_intent(&args) == StartLaunchIntent::Launch {
        let service_prompter = eligibility
            .allows_prompts()
            .then_some(&mut terminal_prompter as &mut dyn StartPrompter);
        require_start_supervisor_service(service_prompter)?;
    }
    if !args.quiet && !args.json && decision.recovery.is_none() {
        println!("{}", ui_heading("guided start"));
        let seam_label =
            seam_preview_label(&read_seams_config(&paths.config_path(), args.no_seams)?);
        let mode = decision.selected_mode.label();
        let suggestion = decision.goal_shape.as_ref().map(|recommendation| {
            format!(
                "{} via {}: {}",
                recommendation.shape.label(),
                recommendation.source.label(),
                recommendation.rationale
            )
        });
        let deadline_label = args
            .deadline
            .as_ref()
            .map(DateTime::to_rfc3339)
            .unwrap_or_else(|| "none".to_string());
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
            ("seams", seam_label.as_str()),
            ("deadline", deadline_label.as_str()),
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
    if decision.recovery.is_some() {
        let surface = start_preview_surface(&decision, &args, &paths)?
            .render_plain(!completion_hints_enabled(false));
        return Err(CliError::Surface { code: 1, surface });
    }
    // C-P9: the decision becomes the durable artifact before anything runs.
    let accepted_by = if decision.confirmed_by_start_picker {
        "operator"
    } else if args.yes {
        "yes-flag-guardrail"
    } else {
        "operator"
    };
    let ceiling = args
        .max_spend
        .or_else(|| config_defaults(&paths).ok().and_then(|d| d.max_spend));
    let mut launch_plan =
        commands::course::launch_plan_from_decision(&decision, ceiling, accepted_by);
    launch_plan.budget.deadline = args.deadline;
    if args.json && args.yes {
        // C-P10: launch JSON parity — dispatch quietly, then emit one
        // machine envelope carrying the plan and what actually launched.
        let goal = decision.goal.clone();
        let mode_label = decision.selected_mode.label().to_string();
        let mut quiet_args = args;
        quiet_args.quiet = true;
        let dispatched = dispatch_start_command(quiet_args, &decision, launch_plan.clone()).await?;
        pause_before_json_launch_envelope_for_test();
        let dispatched_ids = dispatched.ids;
        let next_actions: Vec<String> = dispatched_ids
            .first()
            .map(|id| {
                vec![
                    format!("deadreckon attach {id}"),
                    format!("deadreckon status {id}"),
                ]
            })
            .unwrap_or_default();
        let (process_durability, machine_restart_durability) =
            commands::supervisor_service::guided_durability_labels();
        let envelope = json!({
            "kind": "launch",
            "goal": goal,
            "mode": mode_label,
            "plan": &launch_plan,
            "done_contract": start_done_contract_json(&decision),
            "dispatched": { "mode": mode_label, "ids": dispatched_ids },
            "durability": {
                "process": process_durability,
                "machine_restart": machine_restart_durability,
            },
            "next_actions": next_actions,
        });
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }
    dispatch_start_command(args, &decision, launch_plan)
        .await
        .map(|_| ())
}

#[derive(Clone, Copy)]
struct StartAttachFlags {
    json: bool,
    quiet: bool,
    preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartDispatch {
    ids: Vec<String>,
}

impl StartDispatch {
    fn job(job_id: &deadreckon_protocol::JobId) -> Self {
        Self {
            ids: vec![job_id.to_string()],
        }
    }
}

#[cfg(debug_assertions)]
fn pause_before_json_launch_envelope_for_test() {
    let Some(milliseconds) = std::env::var_os("DEADRECKON_TEST_START_ENVELOPE_DELAY_MS")
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u64>().ok()))
    else {
        return;
    };
    std::thread::sleep(std::time::Duration::from_millis(milliseconds.min(5_000)));
}

#[cfg(not(debug_assertions))]
fn pause_before_json_launch_envelope_for_test() {}

/// C-P13: start-then-watch. After a successful interactive launch, drop
/// into attach when `[defaults] start_attach = true`. Best-effort — a
/// failed attach must not turn a successful launch into an error.
async fn maybe_start_attach(id: &str, flags: &StartAttachFlags) {
    let paths = DeadreckonPaths::discover();
    let enabled = config_defaults(&paths)
        .ok()
        .and_then(|defaults| defaults.start_attach)
        .unwrap_or(false);
    let tty = io::stdout().is_terminal() && io::stdin().is_terminal();
    if !commands::course::should_auto_attach(enabled, tty, flags.json, flags.quiet, flags.preview) {
        return;
    }
    let _ = commands::attach::attach_command(commands::attach::AttachCommandArgs {
        run_id: id.to_string(),
        no_hints: false,
        plain: false,
        json: false,
        why: false,
        view: crate::narrative::AttachViewMode::Activity,
        visual: crate::narrative::NarrativeVisualMode::Architecture,
        narrative_provider: None,
        narrative_max_spend: None,
    })
    .await;
}

async fn dispatch_start_command(
    args: StartCommandArgs,
    decision: &StartLaunchDecision,
    launch_plan: commands::course::LaunchPlan,
) -> Result<StartDispatch> {
    let args_snapshot = StartAttachFlags {
        json: args.json,
        quiet: args.quiet,
        preview: args.preview,
    };
    match decision.selected_mode {
        StartSelectedMode::Run => {
            let paths = DeadreckonPaths::discover();
            let cwd = std::env::current_dir()?;
            let defaults = config_defaults(&paths)?;
            let max_spend_usd = args.max_spend.or(defaults.max_spend).unwrap_or(10.0);
            let max_wall_seconds = commands::job::checked_job_wall_seconds(
                defaults.cli_max_wall_seconds.unwrap_or(36_000.0),
            )?;
            let sandbox_requested = defaults.sandbox.unwrap_or_else(|| "auto".to_string());
            let source = decision
                .resolved_source
                .as_ref()
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "durable start is missing its resolved source".to_string(),
                    ))
                })?
                .durable_source();
            let accepted_by = if launch_plan.accepted_by.as_deref() == Some("yes-flag-guardrail") {
                deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
            } else {
                deadreckon_protocol::AuthorityAcceptedBy::Operator
            };
            let job = commands::job::create_job(commands::job::CreateJob {
                paths: &paths,
                source_cwd: &cwd,
                scope: workspace_scope(&cwd)?,
                launch_plan,
                shape: deadreckon_protocol::JobShape::Single,
                driver: None,
                contract_source: decision
                    .done_contract
                    .as_ref()
                    .map(|contract| contract.source_path.as_path()),
                source,
                max_spend_usd,
                max_wall_seconds,
                max_attempts: 3,
                deadline: args.deadline,
                sandbox_requested,
                accepted_by,
            })?;
            commands::job::launch_detached_supervisor(&paths, &job.job_id)?;
            if !args.quiet {
                print_start_lifecycle_footer(
                    "job",
                    job.job_id.as_ref(),
                    StartLaunchState::InFlight,
                );
            }
            if !args_snapshot.json {
                maybe_start_attach(job.job_id.as_ref(), &args_snapshot).await;
            }
            Ok(StartDispatch::job(&job.job_id))
        }
        StartSelectedMode::Extend => {
            let extension = guided_extension_args(&args, decision)?;
            let job_id = commands::lifecycle::extend_command_with_launch_plan(
                extension,
                Some(launch_plan),
                true,
            )
            .await?
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "approved guided follow-up ended before Job creation".to_string(),
                ))
            })?;
            if !args.quiet {
                print_start_lifecycle_footer("job", job_id.as_ref(), StartLaunchState::InFlight);
            }
            if !args_snapshot.json {
                maybe_start_attach(job_id.as_ref(), &args_snapshot).await;
            }
            Ok(StartDispatch::job(&job_id))
        }
        StartSelectedMode::Campaign => {
            dispatch_advanced_start_job(
                args,
                decision,
                launch_plan,
                args_snapshot,
                commands::graph_job::DriverKind::Campaign,
            )
            .await
        }
        StartSelectedMode::Review | StartSelectedMode::FullPlan => {
            let kind = match decision.selected_mode {
                StartSelectedMode::Review => commands::graph_job::DriverKind::Review,
                StartSelectedMode::FullPlan => commands::graph_job::DriverKind::FullPlan,
                StartSelectedMode::Run
                | StartSelectedMode::Extend
                | StartSelectedMode::Campaign => unreachable!("handled above"),
            };
            dispatch_advanced_start_job(args, decision, launch_plan, args_snapshot, kind).await
        }
    }
}

fn guided_extension_args(
    args: &StartCommandArgs,
    decision: &StartLaunchDecision,
) -> Result<crate::cli::ExtendCommandArgs> {
    if start_source_flags_present(args) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "a guided follow-up starts from its frozen parent artifact",
            "omit --fresh, --worktree, --from, and --allow-dirty",
        )));
    }
    let parent_run_id = decision.base_run_id.clone().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "guided follow-up is missing its selected parent run".to_string(),
        ))
    })?;
    Ok(crate::cli::ExtendCommandArgs {
        parent_run_id,
        new_goal: decision.goal.clone(),
        dest: None,
        acceptance: decision
            .done_contract
            .as_ref()
            .map(|contract| contract.source_path.clone()),
        yes: true,
        max_context_turns: None,
        no_context: false,
        max_spend: args.max_spend,
        max_wall_seconds: None,
        deadline: args.deadline,
        provider: decision
            .provider_route
            .clone()
            .or_else(|| args.provider.clone()),
        model: decision.model.clone().or_else(|| args.model.clone()),
        sandbox: None,
        no_docs: false,
        doc_skill: None,
        post_actions: false,
        narrate: false,
        no_narrate: false,
        narrator_model: None,
    })
}

async fn dispatch_advanced_start_job(
    args: StartCommandArgs,
    decision: &StartLaunchDecision,
    launch_plan: commands::course::LaunchPlan,
    attach_flags: StartAttachFlags,
    kind: commands::graph_job::DriverKind,
) -> Result<StartDispatch> {
    let paths = DeadreckonPaths::discover();
    let cwd = std::env::current_dir()?;
    let defaults = config_defaults(&paths)?;
    let max_spend_usd = args.max_spend.or(defaults.max_spend).unwrap_or(10.0);
    let max_wall_seconds =
        commands::job::checked_job_wall_seconds(defaults.cli_max_wall_seconds.unwrap_or(36_000.0))?;
    let sandbox_requested = defaults.sandbox.unwrap_or_else(|| "auto".to_string());
    let provider_route = decision.provider_route.clone();
    let graph = matches!(
        kind,
        commands::graph_job::DriverKind::Review | commands::graph_job::DriverKind::FullPlan
    );
    let review = kind == commands::graph_job::DriverKind::Review;
    let driver = commands::graph_job::DriverSpec {
        kind,
        child_count: decision.child_count,
        apply: decision
            .goal_shape
            .as_ref()
            .map(|shape| shape.apply)
            .unwrap_or_default(),
        planner_provider: (!review)
            .then(|| {
                decision
                    .planner_provider_route
                    .clone()
                    .or_else(|| provider_route.clone())
            })
            .flatten(),
        child_provider: (!review)
            .then(|| {
                decision
                    .child_provider_route
                    .clone()
                    .or_else(|| provider_route.clone())
            })
            .flatten(),
        child_provider_overrides: decision.child_provider_overrides.clone(),
        coder_provider: review
            .then(|| {
                decision
                    .coder_provider_route
                    .clone()
                    .or_else(|| provider_route.clone())
            })
            .flatten(),
        reviewer_provider: review
            .then(|| {
                decision
                    .reviewer_provider_route
                    .clone()
                    .or_else(|| provider_route.clone())
            })
            .flatten(),
        planner_model: (!review).then(|| decision.planner_model.clone()).flatten(),
        child_model: (!review).then(|| decision.child_model.clone()).flatten(),
        child_model_overrides: decision.child_model_overrides.clone(),
        coder_model: review.then(|| decision.coder_model.clone()).flatten(),
        reviewer_model: review.then(|| decision.reviewer_model.clone()).flatten(),
        model: None,
        source_init_git: decision.resolved_source.as_ref().is_some_and(|source| {
            matches!(
                source.mode,
                commands::job::DurableSourceMode::Copy
                    | commands::job::DurableSourceMode::Fresh
                    | commands::job::DurableSourceMode::InitGit
            )
        }),
    };
    let accepted_by = if launch_plan.accepted_by.as_deref() == Some("yes-flag-guardrail") {
        deadreckon_protocol::AuthorityAcceptedBy::YesFlagGuardrail
    } else {
        deadreckon_protocol::AuthorityAcceptedBy::Operator
    };
    let source = if graph {
        decision
            .resolved_source
            .as_ref()
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "durable Graph start is missing its resolved source".to_string(),
                ))
            })?
            .durable_source()
    } else {
        commands::job::DurableSource {
            mode: commands::job::DurableSourceMode::Worktree,
            from: None,
            allow_dirty: false,
        }
    };
    let job = commands::job::create_job(commands::job::CreateJob {
        paths: &paths,
        source_cwd: &cwd,
        scope: workspace_scope(&cwd)?,
        launch_plan,
        shape: if graph {
            deadreckon_protocol::JobShape::Graph
        } else {
            deadreckon_protocol::JobShape::LegacyCampaign
        },
        driver: Some(driver),
        contract_source: decision
            .done_contract
            .as_ref()
            .map(|contract| contract.source_path.as_path()),
        source,
        max_spend_usd,
        max_wall_seconds,
        max_attempts: 3,
        deadline: args.deadline,
        sandbox_requested,
        accepted_by,
    })?;
    commands::job::launch_detached_supervisor(&paths, &job.job_id)?;
    if !args.quiet {
        print_start_lifecycle_footer("job", job.job_id.as_ref(), StartLaunchState::InFlight);
    }
    if !attach_flags.json {
        maybe_start_attach(job.job_id.as_ref(), &attach_flags).await;
    }
    Ok(StartDispatch::job(&job.job_id))
}

fn start_source_flags_present(args: &StartCommandArgs) -> bool {
    args.fresh || args.worktree || args.from.is_some() || args.allow_dirty
}

fn start_orchestration_flags_present(args: &StartCommandArgs) -> bool {
    args.children.is_some()
        || args.planner_provider.is_some()
        || args.planner_model.is_some()
        || !args.child_provider.is_empty()
        || !args.child_model.is_empty()
        || args.coder_provider.is_some()
        || args.coder_model.is_some()
        || args.reviewer_provider.is_some()
        || args.reviewer_model.is_some()
}

/// Whether the work a guided `start` launched is still running (recommend
/// attach to observe) or has already finished (recommend finish to land it).
/// A synchronous full-plan/run returns to the footer already terminal, so a
/// fixed "attach to observe" recommendation pointed at nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
enum StartLaunchState {
    InFlight,
    Completed,
}

#[cfg(test)]
fn run_launch_state(status: RunStatus) -> StartLaunchState {
    match status {
        RunStatus::Completed => StartLaunchState::Completed,
        _ => StartLaunchState::InFlight,
    }
}

#[cfg(test)]
fn plan_launch_state_from(
    status: deadreckon_core::PlanStatus,
    completed_children: usize,
    total_children: usize,
) -> StartLaunchState {
    let all_children_done = total_children > 0 && completed_children == total_children;
    match status {
        deadreckon_core::PlanStatus::Merged => StartLaunchState::Completed,
        deadreckon_core::PlanStatus::Forked if all_children_done => StartLaunchState::Completed,
        _ => StartLaunchState::InFlight,
    }
}

/// Pure (summary, detail, recommended-verb) for the footer — unit-tested so the
/// recommendation tracks the launch state instead of always saying "attach".
fn start_footer_content(
    kind: &str,
    launch_state: StartLaunchState,
) -> (String, &'static str, &'static str) {
    match launch_state {
        StartLaunchState::Completed => (
            format!("DeadReckon finished the guided start {kind}; all work is complete."),
            "finish lands the result — apply it to your git source, or export with --dest; attach still replays what happened.",
            "finish",
        ),
        StartLaunchState::InFlight => (
            format!("DeadReckon launched the guided start path as a {kind}."),
            "The id now exists; attach is the safest first command for observing the launched work before applying, finishing, or stopping it.",
            "attach",
        ),
    }
}

fn print_start_lifecycle_footer(kind: &str, id: &str, launch_state: StartLaunchState) {
    let id = run_prefix(id);
    let attach = format!("deadreckon attach {id}");
    let status = format!("deadreckon status {id}");
    let kill = format!("deadreckon kill {id}");
    let finish = format!("deadreckon finish {id}");
    let (summary, detail, recommended_verb) = start_footer_content(kind, launch_state);
    let (recommended, secondary): (&str, Vec<(&str, &str)>) = match launch_state {
        StartLaunchState::Completed => (
            finish.as_str(),
            vec![
                ("Secondary", status.as_str()),
                ("Secondary", attach.as_str()),
            ],
        ),
        StartLaunchState::InFlight => (
            attach.as_str(),
            vec![
                ("Secondary", status.as_str()),
                ("Secondary", kill.as_str()),
                ("Secondary", finish.as_str()),
            ],
        ),
    };
    debug_assert!(recommended.contains(recommended_verb));
    let (process_durability, machine_restart_durability) =
        commands::supervisor_service::guided_durability_labels();
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "start",
            Some(&id),
            ExplanationPanel::new(
                summary,
                detail,
                vec![
                    ("target".to_string(), kind.to_string()),
                    ("id".to_string(), id.clone()),
                    (
                        "process durability".to_string(),
                        process_durability.to_string(),
                    ),
                    ("machine restart".to_string(), machine_restart_durability,),
                ],
            ),
            vec![("Recommended", recommended)],
            secondary,
        )
        .render_plain(!completion_hints_enabled(false))
    );
}

#[cfg(test)]
mod execution_team_tests {
    use super::*;

    fn decision(mode: StartSelectedMode) -> StartLaunchDecision {
        let mut decision = start_launch_decision(StartLaunchInput {
            goal: "build the app",
            requested_mode: crate::cli::CliStartMode::Auto,
            stdin_is_tty: true,
        });
        decision.selected_mode = mode;
        decision
    }

    #[test]
    fn every_custom_provider_uses_its_own_model_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("deadreckon"));
        let providers = paths.home().join("providers.d");
        fs::create_dir_all(&providers).expect("providers");
        fs::write(
            providers.join("custom.toml"),
            r#"
id = "cli:custom"
display_name = "Custom CLI"
kind = "cli"
default_binary = "sh"

[exec_template]
args_template = ["{prompt}"]

[[model_catalog]]
id = "custom-large"
recommended = true

[[model_catalog]]
id = "custom-small"
"#,
        )
        .expect("descriptor");
        let defaults = ConfigDefaults {
            provider: Some("cli:custom".to_string()),
            ..ConfigDefaults::default()
        };

        let (selections, _) =
            start_execution_selections(&paths, &defaults, Some("cli:custom"), Some("custom-small"))
                .expect("selections");
        let custom = selections
            .iter()
            .filter(|selection| selection.provider == "cli:custom")
            .map(|selection| selection.model.as_deref())
            .collect::<Vec<_>>();

        assert!(custom.contains(&Some("custom-large")), "{custom:?}");
        assert!(custom.contains(&Some("custom-small")), "{custom:?}");
        assert!(!custom.contains(&Some("gpt-5.1-codex")), "{custom:?}");
    }

    #[test]
    fn uniform_team_applies_one_provider_model_pair_to_every_required_role() {
        let mut decision = decision(StartSelectedMode::FullPlan);
        decision.child_count = Some(4);

        apply_uniform_start_team(
            &mut decision,
            StartExecutionSelection {
                provider: "cli:codex".to_string(),
                model: Some("gpt-team".to_string()),
                label: "Codex · gpt-team".to_string(),
                detail: "test".to_string(),
            },
        );

        assert_eq!(
            decision.planner_provider_route.as_deref(),
            Some("cli:codex")
        );
        assert_eq!(decision.child_provider_route.as_deref(), Some("cli:codex"));
        assert_eq!(decision.planner_model.as_deref(), Some("gpt-team"));
        assert_eq!(decision.child_model.as_deref(), Some("gpt-team"));
    }

    #[test]
    fn a_different_role_provider_does_not_inherit_the_primary_model() {
        let mut decision = decision(StartSelectedMode::Review);
        decision.provider_route = Some("cli:codex".to_string());
        decision.model = Some("gpt-team".to_string());
        decision.coder_provider_route = Some("cli:codex".to_string());
        decision.reviewer_provider_route = Some("cli:claude-code".to_string());

        assert_eq!(
            explicit_role_model(Some("cli:codex"), None, &decision).as_deref(),
            Some("gpt-team")
        );
        assert_eq!(
            explicit_role_model(Some("cli:claude-code"), None, &decision),
            None
        );
        assert_eq!(
            start_provider_role_summary(&decision).as_deref(),
            Some("implementor=cli:codex/gpt-team, reviewer=cli:claude-code/provider default")
        );
    }

    #[test]
    fn accepted_launch_plan_records_the_exact_execution_team() {
        let mut decision = decision(StartSelectedMode::FullPlan);
        decision.provider_route = Some("cli:codex".to_string());
        decision.model = Some("worker-default".to_string());
        decision.child_count = Some(2);
        decision.planner_provider_route = Some("cli:claude-code".to_string());
        decision.planner_model = Some("opus".to_string());
        decision.child_provider_route = Some("cli:codex".to_string());
        decision.child_model = Some("gpt-worker".to_string());
        decision.child_provider_overrides = vec!["1=cli:claude-code".to_string()];
        decision.child_model_overrides = vec!["1=sonnet".to_string()];

        let plan = commands::course::launch_plan_from_decision(&decision, Some(10.0), "operator");

        assert_eq!(plan.providers.planner.as_deref(), Some("cli:claude-code"));
        assert_eq!(plan.providers.planner_model.as_deref(), Some("opus"));
        assert_eq!(plan.providers.default_child.as_deref(), Some("cli:codex"));
        assert_eq!(
            plan.providers.default_child_model.as_deref(),
            Some("gpt-worker")
        );
        assert_eq!(
            plan.providers.children.get(&1).map(String::as_str),
            Some("cli:claude-code")
        );
        assert_eq!(
            plan.providers.child_models.get(&1).map(String::as_str),
            Some("sonnet")
        );
    }

    #[test]
    fn done_authoring_preview_uses_the_selected_planner_not_the_doc_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        fs::create_dir_all(paths.home()).expect("home");
        fs::write(
            paths.config_path(),
            r#"
default_provider = "cli:codex"

[defaults]
provider = "cli:codex"
doc_provider = "cli:claude-code"
"#,
        )
        .expect("config");

        let mut decision = decision(StartSelectedMode::FullPlan);
        decision.provider_route = Some("cli:codex".to_string());
        decision.model = Some("worker-model".to_string());
        decision.planner_provider_route = Some("cli:codex".to_string());
        decision.planner_model = Some("selected-planner-model".to_string());
        decision.done_action = StartDoneAction::ManualText {
            text: "the app launches".to_string(),
            overwrite_existing: false,
        };
        decision.resolved_source = Some(ResolvedStartSource {
            mode: commands::job::DurableSourceMode::Worktree,
            requested_from: None,
            inspection_root: temp.path().to_path_buf(),
            contract_write_root: temp.path().to_path_buf(),
            allow_dirty: false,
            provenance: StartSourceProvenance::CurrentCleanWorktree,
        });

        let authoring = start_done_authoring_json(&decision, &paths).expect("authoring preview");
        assert_eq!(
            authoring["provider"],
            "cli:codex / selected-planner-model (structured text)"
        );
    }

    #[test]
    fn done_authoring_selection_follows_the_lead_role_for_each_launch_shape() {
        let mut full_plan = decision(StartSelectedMode::FullPlan);
        full_plan.provider_route = Some("cli:codex".to_string());
        full_plan.model = Some("worker-model".to_string());
        full_plan.planner_provider_route = Some("cli:claude-code".to_string());
        full_plan.planner_model = Some("planner-model".to_string());
        assert_eq!(
            start_done_authoring_selection(&full_plan),
            (
                Some("cli:claude-code".to_string()),
                Some("planner-model".to_string())
            )
        );

        let mut review = decision(StartSelectedMode::Review);
        review.provider_route = Some("cli:codex".to_string());
        review.model = Some("primary-model".to_string());
        review.coder_provider_route = Some("cli:claude-code".to_string());
        review.coder_model = Some("coder-model".to_string());
        assert_eq!(
            start_done_authoring_selection(&review),
            (
                Some("cli:claude-code".to_string()),
                Some("coder-model".to_string())
            )
        );

        let mut run = decision(StartSelectedMode::Run);
        run.provider_route = Some("cli:codex".to_string());
        run.model = Some("run-model".to_string());
        assert_eq!(
            start_done_authoring_selection(&run),
            (Some("cli:codex".to_string()), Some("run-model".to_string()))
        );
    }
}

#[cfg(test)]
mod start_footer_tests {
    use super::{
        ResolvedStartSource, StartLaunchInput, StartLaunchIntent, StartLaunchState,
        StartSelectedMode, StartSourceProvenance, StartSupervisorAdmission, guided_extension_args,
        plan_launch_state_from, run_launch_state, set_resolved_start_source, start_footer_content,
        start_launch_decision, start_launch_intent, start_launch_preview_facts, start_source_json,
        start_supervisor_admission,
    };
    use crate::cli::{CliStartMode, StartCommandArgs};
    use crate::commands::supervisor_service::SupervisorServicePreflight;
    use deadreckon_core::{PlanStatus, RunStatus};

    fn start_args() -> StartCommandArgs {
        StartCommandArgs {
            goal: "test goal".to_string(),
            mode: CliStartMode::Auto,
            plan: None,
            max_spend: Some(3.0),
            deadline: None,
            provider: None,
            model: None,
            children: None,
            planner_provider: None,
            planner_model: None,
            child_provider: Vec::new(),
            child_model: Vec::new(),
            coder_provider: None,
            coder_model: None,
            reviewer_provider: None,
            reviewer_model: None,
            preview: false,
            review_done: false,
            yes: true,
            no_seams: false,
            fresh: false,
            worktree: false,
            from: None,
            allow_dirty: false,
            plain: true,
            quiet: true,
            json: false,
        }
    }

    #[test]
    fn completed_launch_recommends_finish_not_attach() {
        let (summary, detail, verb) = start_footer_content("plan", StartLaunchState::Completed);
        assert_eq!(verb, "finish");
        assert!(summary.contains("finished"));
        assert!(detail.contains("finish lands the result"));
        assert!(!detail.starts_with("The id now exists"));
    }

    #[test]
    fn in_flight_launch_recommends_attach() {
        let (_summary, _detail, verb) = start_footer_content("run", StartLaunchState::InFlight);
        assert_eq!(verb, "attach");
    }

    #[test]
    fn ordinary_start_fails_closed_without_restart_capable_service() {
        assert_eq!(
            start_supervisor_admission(&SupervisorServicePreflight::ready_for_test(), false),
            StartSupervisorAdmission::Proceed
        );
        let setup = SupervisorServicePreflight::SetupRequired {
            reason: "service stopped".to_string(),
            last_instance: None,
        };
        assert_eq!(
            start_supervisor_admission(&setup, true),
            StartSupervisorAdmission::OfferSetup
        );
        assert_eq!(
            start_supervisor_admission(&setup, false),
            StartSupervisorAdmission::Refuse
        );
        let unsupported = SupervisorServicePreflight::Refused {
            reason: "unsupported".to_string(),
        };
        assert_eq!(
            start_supervisor_admission(&unsupported, true),
            StartSupervisorAdmission::Refuse
        );
    }

    #[test]
    fn preview_and_unconfirmed_json_are_read_only_launch_intents() {
        let mut args = start_args();
        args.preview = true;
        assert_eq!(start_launch_intent(&args), StartLaunchIntent::Inspect);

        args.preview = false;
        args.json = true;
        args.yes = false;
        assert_eq!(start_launch_intent(&args), StartLaunchIntent::Inspect);

        args.yes = true;
        assert_eq!(start_launch_intent(&args), StartLaunchIntent::Launch);
    }

    #[test]
    fn json_plain_and_card_project_identical_source_truth() {
        let canonical = std::path::PathBuf::from("/canonical/Cloudwing");
        let mut decision = start_launch_decision(StartLaunchInput {
            goal: "continue Cloudwing",
            requested_mode: CliStartMode::FullPlan,
            stdin_is_tty: false,
        });
        decision.source_mode = super::StartSourceMode::Copy;
        decision.source_mode_label = format!("approved copy from {}", canonical.display());
        decision.resolved_source = Some(ResolvedStartSource {
            mode: crate::commands::job::DurableSourceMode::Copy,
            requested_from: Some(canonical.clone()),
            inspection_root: canonical.clone(),
            contract_write_root: std::path::PathBuf::from("/launch"),
            allow_dirty: false,
            provenance: StartSourceProvenance::ExplicitCopy,
        });

        let card = start_launch_preview_facts(&decision).workspace.to_string();
        let json = start_source_json(&decision);
        let plain = decision.source_mode_label.clone();
        for projection in [
            card,
            plain,
            json["label"].as_str().expect("json label").to_string(),
        ] {
            assert!(projection.contains("/canonical/Cloudwing"), "{projection}");
        }
        assert_eq!(
            json["inspection_root"],
            serde_json::Value::String("/canonical/Cloudwing".to_string())
        );
    }

    #[test]
    fn start_resolves_source_once_before_done_authoring() {
        let temp = tempfile::tempdir().expect("tempdir");
        let launch = temp.path().join("launch");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        for path in [&launch, &first, &second] {
            std::fs::create_dir_all(path).expect("source directory");
        }
        let mut decision = start_launch_decision(StartLaunchInput {
            goal: "continue Cloudwing",
            requested_mode: CliStartMode::FullPlan,
            stdin_is_tty: false,
        });
        set_resolved_start_source(
            &mut decision,
            &launch,
            crate::commands::job::DurableSourceMode::Copy,
            Some(first.clone()),
            &first,
            false,
            StartSourceProvenance::ExplicitCopy,
        )
        .expect("first resolution");
        let error = set_resolved_start_source(
            &mut decision,
            &launch,
            crate::commands::job::DurableSourceMode::Copy,
            Some(second),
            &temp.path().join("second"),
            false,
            StartSourceProvenance::ExplicitCopy,
        )
        .expect_err("source reinterpretation must fail");

        assert!(error.to_string().contains("already resolved"), "{error}");
        assert_eq!(
            decision
                .resolved_source
                .as_ref()
                .expect("resolved source")
                .inspection_root,
            first.canonicalize().expect("canonical first")
        );
    }

    #[test]
    fn preview_and_dispatch_share_the_same_resolved_source() {
        let canonical = std::path::PathBuf::from("/canonical/Cloudwing");
        let resolved = ResolvedStartSource {
            mode: crate::commands::job::DurableSourceMode::Copy,
            requested_from: Some(canonical.clone()),
            inspection_root: canonical.clone(),
            contract_write_root: std::path::PathBuf::from("/launch"),
            allow_dirty: false,
            provenance: StartSourceProvenance::ExplicitCopy,
        };
        let durable = resolved.durable_source();
        assert_eq!(durable.mode, crate::commands::job::DurableSourceMode::Copy);
        assert_eq!(durable.from.as_deref(), Some(canonical.as_path()));
        assert_eq!(resolved.inspection_root, canonical);
    }

    #[test]
    fn invalid_source_refuses_before_provider_confirmation_or_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut decision = start_launch_decision(StartLaunchInput {
            goal: "continue Cloudwing",
            requested_mode: CliStartMode::FullPlan,
            stdin_is_tty: false,
        });
        let missing = temp.path().join("missing");
        let error = set_resolved_start_source(
            &mut decision,
            temp.path(),
            crate::commands::job::DurableSourceMode::Copy,
            Some(missing.clone()),
            &missing,
            false,
            StartSourceProvenance::ExplicitCopy,
        )
        .expect_err("missing source must refuse");

        assert!(error.to_string().contains("cannot be resolved"), "{error}");
        assert!(decision.provider_route.is_none());
        assert!(decision.resolved_source.is_none());
        assert!(!temp.path().join(".deadreckon/acceptance.yaml").exists());
        assert!(!decision.requires_confirmation);
    }

    #[test]
    fn guided_history_continuation_builds_parent_bound_durable_request() {
        let mut decision = start_launch_decision(StartLaunchInput {
            goal: "add a durable follow-up",
            requested_mode: CliStartMode::Auto,
            stdin_is_tty: true,
        });
        decision.selected_mode = StartSelectedMode::Extend;
        decision.base_run_id = Some("1234567890abcdef".to_string());
        decision.provider_route = Some("cli:codex".to_string());
        let mut args = start_args();
        args.goal = decision.goal.clone();
        let deadline = chrono::Utc::now() + chrono::TimeDelta::hours(2);
        args.deadline = Some(deadline);
        let extension = guided_extension_args(&args, &decision).expect("guided extension request");

        assert_eq!(extension.parent_run_id, "1234567890abcdef");
        assert_eq!(extension.new_goal, "add a durable follow-up");
        assert_eq!(extension.provider.as_deref(), Some("cli:codex"));
        assert_eq!(extension.max_spend, Some(3.0));
        assert_eq!(extension.deadline, Some(deadline));
        assert!(extension.yes);
        assert!(extension.dest.is_none());
    }

    #[test]
    fn completed_run_maps_to_completed_state() {
        assert_eq!(
            run_launch_state(RunStatus::Completed),
            StartLaunchState::Completed
        );
        assert_eq!(
            run_launch_state(RunStatus::Executing),
            StartLaunchState::InFlight
        );
    }

    #[test]
    fn forked_plan_with_all_children_done_is_completed() {
        // The reported case: a full-plan that ran all 4 children to completion
        // returned to the footer as Forked 4/4 — the next action is finish.
        assert_eq!(
            plan_launch_state_from(PlanStatus::Forked, 4, 4),
            StartLaunchState::Completed
        );
        assert_eq!(
            plan_launch_state_from(PlanStatus::Forked, 2, 4),
            StartLaunchState::InFlight
        );
        assert_eq!(
            plan_launch_state_from(PlanStatus::Merged, 0, 0),
            StartLaunchState::Completed
        );
        assert_eq!(
            plan_launch_state_from(PlanStatus::Pending, 0, 4),
            StartLaunchState::InFlight
        );
    }
}

#[cfg(test)]
mod start_goal_tests {
    use super::{StartGoalPlan, start_goal_plan};

    #[test]
    fn start_without_goal_prompts_when_tty() {
        assert!(matches!(start_goal_plan(None, true), StartGoalPlan::Prompt));
        // An all-whitespace goal is treated as missing and still prompts.
        assert!(matches!(
            start_goal_plan(Some("   "), true),
            StartGoalPlan::Prompt
        ));
        assert!(matches!(
            start_goal_plan(Some("build the app"), true),
            StartGoalPlan::Provided(goal) if goal == "build the app"
        ));
    }

    #[test]
    fn start_without_goal_prints_notice_when_prompts_suppressed() {
        assert!(matches!(
            start_goal_plan(None, false),
            StartGoalPlan::Notice(_)
        ));
        // A provided goal is still used even when prompts are suppressed.
        assert!(matches!(
            start_goal_plan(Some("ship it"), false),
            StartGoalPlan::Provided(_)
        ));
    }
}

#[cfg(test)]
mod course_planner_cleanup_tests {
    use super::{
        course_planner_cleanup_timeout, course_planner_timeout,
        require_course_planner_cleanup_proven,
    };

    #[test]
    fn goal_shape_planner_budgets_cover_cli_and_http_cold_starts() {
        assert_eq!(course_planner_timeout("cli:codex").as_secs(), 120);
        assert_eq!(course_planner_timeout("openai").as_secs(), 30);
        assert_eq!(course_planner_cleanup_timeout().as_secs(), 30);
    }

    #[test]
    fn unresolved_goal_shape_provider_cleanup_stops_admission_and_retains_authority() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("course-provider.pid");
        std::fs::write(&pid_file, "4242\n").expect("pid authority");

        let error =
            require_course_planner_cleanup_proven(true, &pid_file, "cli:fixture", "returned")
                .expect_err(
                    "a returned provider with residual authority must stop start admission",
                );
        let rendered = error.to_string();
        assert!(rendered.contains("cleanup was not proven"), "{rendered}");
        assert!(rendered.contains("rerun deadreckon start"), "{rendered}");
        assert!(
            pid_file.exists(),
            "unresolved process authority must remain for operator cleanup"
        );
    }

    #[test]
    fn cleanup_timeout_stops_admission_even_when_no_pid_checkpoint_was_observed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("course-provider.pid");

        let error =
            require_course_planner_cleanup_proven(false, &pid_file, "cli:fixture", "timed out")
                .expect_err("an unresolved cleanup wait must stop start admission");
        let rendered = error.to_string();
        assert!(rendered.contains("cleanup was not proven"), "{rendered}");
        assert!(rendered.contains("exit cannot be proven"), "{rendered}");
        assert!(!rendered.contains("authority remains"), "{rendered}");
    }

    #[test]
    fn proven_goal_shape_provider_cleanup_allows_deterministic_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        require_course_planner_cleanup_proven(
            true,
            &temp.path().join("absent.pid"),
            "cli:fixture",
            "timed out",
        )
        .expect("completed cleanup without residual authority");
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn contract(raw: &str) -> commands::acceptance::CompiledContract {
        commands::acceptance::compile_contract(raw, Some("# Done\n")).expect("contract")
    }

    fn decision(goal: &str) -> StartLaunchDecision {
        start_launch_decision(StartLaunchInput {
            goal,
            requested_mode: crate::cli::CliStartMode::Auto,
            stdin_is_tty: false,
        })
    }

    struct PanickingPrompter;

    impl StartPrompter for PanickingPrompter {
        fn select_one(&mut self, _prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice> {
            panic!("review must only fire after a fresh draft");
        }
        fn confirm(&mut self, _question: &str, _default_yes: bool) -> Result<bool> {
            panic!("review must only fire after a fresh draft");
        }
        fn input(&mut self, _message: &str, _default: Option<&str>) -> Result<String> {
            panic!("review must only fire after a fresh draft");
        }
    }

    struct RecoveryPrompter {
        choice: &'static str,
        input: &'static str,
        offered: Vec<String>,
    }

    impl StartPrompter for RecoveryPrompter {
        fn select_one(&mut self, prompt: prompt::SelectPrompt) -> Result<prompt::SelectChoice> {
            self.offered = prompt
                .choices
                .iter()
                .map(|choice| choice.id.clone())
                .collect();
            Ok(prompt
                .choices
                .into_iter()
                .find(|choice| choice.id == self.choice)
                .expect("recovery choice"))
        }

        fn confirm(&mut self, _question: &str, _default_yes: bool) -> Result<bool> {
            panic!("recovery does not confirm")
        }

        fn input(&mut self, _message: &str, _default: Option<&str>) -> Result<String> {
            Ok(self.input.to_string())
        }
    }

    #[tokio::test]
    async fn materialize_without_manual_text_never_prompts() {
        // The post-draft review pauses only when the drafter actually ran:
        // an Existing/DefaultGate/Missing action must pass straight through
        // even with an interactive prompter available (zero-questions doctrine
        // for previously accepted contracts).
        let mut prompter = PanickingPrompter;
        for action in [
            StartDoneAction::Existing,
            StartDoneAction::DefaultGate,
            StartDoneAction::Missing,
        ] {
            let mut d = decision("build a realtime dashboard");
            d.done_action = action;
            materialize_start_done_criteria(&mut d, Some(&mut prompter))
                .await
                .expect("materialize");
        }
    }

    #[test]
    fn failed_done_authoring_can_revise_without_losing_the_execution_team() {
        let mut d = decision("build a realtime dashboard");
        d.provider_route = Some("cli:codex".to_string());
        d.model = Some("gpt-5.6-sol".to_string());
        let mut prompter = RecoveryPrompter {
            choice: "revise",
            input: "the dashboard passes its network smoke test",
            offered: Vec::new(),
        };
        let error = CliError::Core(deadreckon_core::user_error(
            "provider failed",
            "deadreckon doctor",
        ));

        assert!(
            recover_start_done_authoring(&mut d, "old done wording", false, &error, &mut prompter)
                .expect("recovery")
        );
        assert_eq!(prompter.offered, ["retry", "revise", "cancel"]);
        assert_eq!(d.provider_route.as_deref(), Some("cli:codex"));
        assert_eq!(d.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            d.done_action,
            StartDoneAction::ManualText {
                text: "the dashboard passes its network smoke test".to_string(),
                overwrite_existing: false,
            }
        );
    }

    #[test]
    fn fresh_draft_review_cancel_maps_to_recovery_error() {
        // The caller turns a review cancellation into a launch-stopping error;
        // pin the message so cancel can never silently fall through to dispatch.
        let mut d = decision("build a realtime dashboard");
        set_start_recovery(
            &mut d,
            format!("guided start cancelled before accepting the {NOUN_DONE_CONTRACT}"),
            vec!["deadreckon def-done show".to_string()],
        );
        let recovery = d.recovery.as_ref().expect("recovery");
        let err = start_recovery_error(recovery);
        let text = format!("{err:?}");
        assert!(text.contains("cancelled before accepting"), "{text}");
    }

    #[test]
    fn divergence_lines_carry_a_refine_remedy() {
        let weak = contract(
            r#"
name: weak
checks:
  - kind: content_match
    path: "{working_dir}/src/main.js"
    pattern: "scalable"
"#,
        );
        let divergence =
            commands::acceptance::reconcile("build a scalable realtime slack clone", &weak);
        let lines = commands::acceptance::render_compiled_contract_lines(&weak, Some(&divergence))
            .join("\n");

        assert!(!divergence.clean(), "{divergence:?}");
        assert!(
            lines.contains("try: deadreckon def-done refine"),
            "divergence must name the command that closes the gap\n{lines}"
        );
    }

    #[test]
    fn start_draft_passes_goal_into_acceptance_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let decision = decision("build a realtime dashboard");
        let prompt = commands::acceptance::acceptance_agent_prompt(
            commands::acceptance::AcceptanceAgentMode::Draft,
            "compile done criteria",
            Some(&decision.goal),
            dir.path(),
            None,
            None,
        )
        .expect("prompt");

        assert!(
            prompt.contains("Run goal:\nbuild a realtime dashboard"),
            "{prompt}"
        );
    }

    #[test]
    fn reconcile_reports_uncovered_realtime_clause() {
        let contract = contract(
            r#"
name: weak
checks:
  - kind: shell
    command: "npm run build"
    cwd: "{working_dir}"
"#,
        );
        let divergence = commands::acceptance::reconcile("build a realtime dashboard", &contract);

        assert!(
            divergence
                .uncovered
                .iter()
                .any(|clause| clause.contains("realtime")),
            "{divergence:#?}"
        );
    }

    #[test]
    fn reconcile_clean_when_every_clause_has_a_check() {
        let contract = contract(
            r#"
name: covered
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/realtime-dashboard.mjs"
    cwd: "{working_dir}"
"#,
        );
        let divergence = commands::acceptance::reconcile("build realtime dashboard", &contract);

        assert!(divergence.uncovered.is_empty(), "{divergence:#?}");
    }

    #[test]
    fn review_renders_real_checks_not_just_count() {
        let contract = contract(
            r#"
name: behavior
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/check.mjs"
    cwd: "{working_dir}"
"#,
        );
        let lines = commands::acceptance::render_compiled_contract_lines(&contract, None);
        let joined = lines.join("\n");

        assert!(joined.contains("runs shell: npm run build"), "{joined}");
        assert!(!joined.contains("(1 checks)"), "{joined}");
    }

    #[test]
    fn reprompt_recompiles_and_reshows_until_accept() {
        let weak = contract(
            r#"
name: weak
checks:
  - kind: content_match
    path: "{working_dir}/src/main.js"
    pattern: "realtime"
"#,
        );
        let fixed = contract(
            r#"
name: fixed
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/realtime.mjs"
    cwd: "{working_dir}"
"#,
        );
        let weak_lines = commands::acceptance::render_compiled_contract_lines(
            &weak,
            Some(&commands::acceptance::reconcile(
                "build realtime app",
                &weak,
            )),
        )
        .join("\n");
        let fixed_lines = commands::acceptance::render_compiled_contract_lines(
            &fixed,
            Some(&commands::acceptance::reconcile(
                "build realtime app",
                &fixed,
            )),
        )
        .join("\n");

        assert!(weak_lines.contains("weak check"), "{weak_lines}");
        assert!(fixed_lines.contains("runs shell"), "{fixed_lines}");
        assert_ne!(weak_lines, fixed_lines);
    }

    #[test]
    fn edit_and_check_reuse_existing_paths() {
        let selection = setup::DoneCriteriaSelection::project(
            PathBuf::from(".deadreckon/acceptance.yaml"),
            Some(PathBuf::from(".deadreckon/acceptance.md")),
            Some(1),
        );
        let lines = done_criteria_inspection_try_lines(&selection);

        assert!(
            lines
                .iter()
                .any(|line| line.contains("def-done show --spec"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("def-done check --spec"))
        );
    }

    #[test]
    fn yes_launch_still_surfaces_divergence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let acceptance_dir = dir.path().join(".deadreckon");
        fs::create_dir_all(&acceptance_dir).expect("mkdir");
        fs::write(
            acceptance_dir.join("acceptance.yaml"),
            r#"
name: weak
checks:
  - kind: content_match
    path: "{working_dir}/src/main.js"
    pattern: "offline"
"#,
        )
        .expect("write yaml");
        fs::write(acceptance_dir.join("acceptance.md"), "# Done\n").expect("write md");
        let mut decision = decision("build a realtime dashboard");

        resolve_start_done_criteria(&mut decision, dir.path(), None, true, true).expect("resolve");

        assert!(decision.done_divergence.is_some(), "{decision:#?}");
    }

    #[test]
    fn strong_divergence_refuses_under_yes_with_try() {
        let dir = tempfile::tempdir().expect("tempdir");
        let acceptance_dir = dir.path().join(".deadreckon");
        fs::create_dir_all(&acceptance_dir).expect("mkdir");
        fs::write(
            acceptance_dir.join("acceptance.yaml"),
            r#"
name: weak
checks:
  - kind: content_match
    path: "{working_dir}/src/main.js"
    pattern: "offline"
"#,
        )
        .expect("write yaml");
        let mut decision = decision("build a realtime dashboard");

        resolve_start_done_criteria(&mut decision, dir.path(), None, true, true).expect("resolve");

        assert!(decision.recovery.is_some(), "{decision:#?}");
        assert!(
            decision
                .try_lines
                .iter()
                .any(|line| line.contains("--review-done")),
            "{decision:#?}"
        );
    }

    #[test]
    fn review_done_flag_forces_loop_non_interactively() {
        let mut decision = decision("build the app");
        let eligibility = StartPromptEligibility {
            stdin_is_tty: false,
            json: false,
            plain: false,
            quiet: false,
            yes: true,
        };

        apply_forced_contract_review_guard(&mut decision, eligibility, true);

        assert!(decision.recovery.is_some(), "{decision:#?}");
        assert!(
            decision
                .try_lines
                .iter()
                .any(|line| line.contains("--review-done")),
            "{decision:#?}"
        );
    }

    #[test]
    fn plain_contract_review_prints_check_lines() {
        let contract = contract(
            r#"
name: behavior
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/check.mjs"
    cwd: "{working_dir}"
"#,
        );
        let lines = commands::acceptance::render_compiled_contract_lines(&contract, None);

        assert!(lines.iter().any(|line| line.starts_with("1. runs shell:")));
    }

    #[test]
    fn every_contract_refusal_emits_a_try_line() {
        let decision = decision("build the app");
        let lines = contract_review_try_lines(&decision);

        assert!(!lines.is_empty());
        assert!(lines.iter().all(|line| line.starts_with("deadreckon ")));
    }

    #[test]
    fn start_json_emits_compiled_checks_and_divergence() {
        let mut decision = decision("build realtime dashboard");
        let contract = contract(
            r#"
name: behavior
checks:
  - kind: shell
    command: "npm run build && node .deadreckon/acceptance/realtime-dashboard.mjs"
    cwd: "{working_dir}"
"#,
        );
        decision.done_contract = Some(contract.clone());
        decision.done_divergence = Some(commands::acceptance::reconcile(&decision.goal, &contract));

        let value = start_done_contract_json(&decision);

        assert!(value.get("checks").is_some(), "{value}");
        assert!(value.get("divergence").is_some(), "{value}");
    }
}
