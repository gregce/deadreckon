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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    DefaultGate,
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
            Self::DefaultGate => "default",
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
    pub(crate) child_provider_route: Option<String>,
    pub(crate) child_provider_overrides: Vec<String>,
    pub(crate) coder_provider_route: Option<String>,
    pub(crate) reviewer_provider_route: Option<String>,
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
        child_provider_route: None,
        child_provider_overrides: Vec::new(),
        coder_provider_route: None,
        reviewer_provider_route: None,
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
    plain: bool,
) -> GoalShapeRecommendation {
    let defaults_ceiling = config_defaults(paths).ok().and_then(|d| d.max_spend);
    let signals = commands::course::collect_signal_bundle(paths, cwd, goal, defaults_ceiling);
    let ladder = commands::course::ladder_decision(&signals);
    if let Some(provider) = provider
        && provider != "smoke"
        && !provider.starts_with("smoke:")
        && let Some(resolved) =
            provider_course_plan(paths, cwd, goal, provider, plain, &signals, &ladder).await
    {
        let (shape, n, _pieces, resolution) = resolved;
        return GoalShapeRecommendation {
            schema_version: 1,
            goal: goal.to_string(),
            shape: course_shape_to_goal_shape(shape),
            n,
            rationale: resolution.rationale,
            source: GoalShapeSource::Provider,
            provider: Some(provider.to_string()),
        };
    }
    ladder_goal_shape_recommendation(goal, &ladder)
}

async fn provider_course_plan(
    paths: &DeadreckonPaths,
    cwd: &Path,
    goal: &str,
    provider: &str,
    plain: bool,
    signals: &commands::course::SignalBundle,
    ladder: &commands::course::LadderDecision,
) -> Option<commands::course::ResolvedCoursePlan> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(provider)).ok()?;
    let request = ProviderRequest {
        prompt: commands::course::course_planner_prompt(goal, signals),
        max_output_tokens: 512,
        cwd: Some(cwd.to_path_buf()),
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    let response = tokio::time::timeout(
        course_planner_timeout(provider),
        maybe_with_cli_wait_status(!plain, "plotting the course", router.complete(&request)),
    )
    .await
    .ok()?
    .ok()?;
    commands::course::resolve_provider_course_plan(
        &response.content,
        signals,
        ladder,
        commands::course::SHAPE_CONFIDENCE_FLOOR_DEFAULT,
    )
}

/// CLI subagent providers cold-start in ~10-15s; the 5s ceiling that suits
/// HTTP routes guarantees a silent ladder fallback for them. The planner is
/// one bounded read-only call either way — give CLIs room to answer.
pub(crate) fn course_planner_timeout(provider: &str) -> Duration {
    if provider.starts_with("cli:") {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(5)
    }
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
    // One keyword list, owned by the course ladder (rule 2.5), so the
    // auto-mode heuristic and the deterministic floor can never drift.
    commands::course::goal_names_parallel_workstreams(lower_goal)
}

fn start_goal_contains_word(lower_goal: &str, needle: &str) -> bool {
    lower_goal
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == needle)
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
    prompt_start_model(decision, paths, &provider, prompter)?;
    Ok(())
}

/// After an interactive provider choice, offer the provider's model catalog.
/// Enter keeps the default (configured defaults.model when it names this
/// provider's entry, else the descriptor's recommended entry); choosing
/// "provider default" launches with no model override at all. Skipped when
/// the catalog has at most one entry or the model was already pinned.
pub(crate) fn prompt_start_model(
    decision: &mut StartLaunchDecision,
    paths: &DeadreckonPaths,
    provider: &str,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    if decision.model.is_some() {
        return Ok(());
    }
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let Some(descriptor) = registry.get(provider) else {
        return Ok(());
    };
    if descriptor.model_catalog.len() < 2 {
        return Ok(());
    }
    let configured = config_defaults(paths)?.model;
    let mut default_index = 0;
    let choices = descriptor
        .model_catalog
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if configured.as_deref() == Some(entry.id.as_str())
                || (configured.is_none() && entry.recommended && default_index == 0)
            {
                default_index = index;
            }
            let mut details = Vec::new();
            if let Some(window) = entry.context_window {
                details.push(format!("context {}k", window / 1000));
            }
            if let (Some(input), Some(output)) = (entry.input_per_million, entry.output_per_million)
                && (input > 0.0 || output > 0.0)
            {
                details.push(format!("${input}/{output} per M"));
            }
            if entry.recommended {
                details.push("recommended".to_string());
            }
            prompt::SelectChoice::with_detail(&entry.id, &entry.id, details.join(" · "))
        })
        .collect::<Vec<_>>();
    let choice = prompter.select_one(prompt::SelectPrompt {
        title: "Choose model".to_string(),
        help: Some("Enter keeps the default; this launch only".to_string()),
        choices,
        default_index,
    })?;
    decision.model = (choice.id != "provider default").then(|| choice.id.clone());
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

/// The one question `start` may ask (C-P8), and only when no contract was
/// found or detected: "How will you know it worked?". A one-line answer is
/// compiled through the existing def-done flow; pressing Enter accepts the
/// default gate with its caveat. No menus, no second question.
pub(crate) fn prompt_start_done_criteria(
    decision: &mut StartLaunchDecision,
    prompter: &mut dyn StartPrompter,
) -> Result<()> {
    let text = prompter.input(
        "How will you know it worked? (one line; Enter = default gate) ",
        None,
    )?;
    let text = text.trim();
    if text.is_empty() {
        decision.done_criteria_source = StartDoneCriteriaSource::DefaultGate;
        decision.done_action = StartDoneAction::DefaultGate;
        decision.done_criteria_label = "default dr-gate behavior".to_string();
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
        resolve_start_done_criteria(decision, &cwd, Some(&mut *prompter), args.yes)?;
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
        resolve_start_done_criteria(decision, &cwd, None, args.yes)?;
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

pub(crate) fn resolve_start_done_criteria(
    decision: &mut StartLaunchDecision,
    cwd: &Path,
    prompter: Option<&mut dyn StartPrompter>,
    yes: bool,
) -> Result<()> {
    let source = commands::acceptance::resolve_acceptance_source(cwd, None)?;
    if source.is_some() {
        let selection = commands::acceptance::done_criteria_selection(&source)?;
        if let Some(prompter) = prompter {
            prompt_start_existing_done_criteria(decision, cwd, &selection, prompter)?;
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
        } else {
            print_start_contract_divergence(decision);
        }
        return Ok(());
    }

    // C-P8: the one-question flow. A detected contract means everything
    // about "done" is already known — zero questions, interactive or not.
    let contract = commands::course::contract_signal(cwd);
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
        // Explicit consent to proceed: skip the question, carry the caveat
        // (Polyglot doctrine — the gate will surface it, never silent green).
        let caveat = contract
            .caveat
            .unwrap_or_else(|| "no test contract detected".to_string());
        decision.done_criteria_source = StartDoneCriteriaSource::DefaultGate;
        decision.done_action = StartDoneAction::DefaultGate;
        decision.done_criteria_label = format!("default gate - caveat: {caveat}");
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

async fn materialize_start_done_criteria(decision: &mut StartLaunchDecision) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Some((request, overwrite_existing)) = start_done_materialization_request(decision) else {
        return Ok(());
    };
    commands::acceptance::acceptance_agent_command_in_dir(
        &cwd,
        commands::acceptance::AcceptanceAgentMode::Draft,
        vec![request],
        Some(&decision.goal),
        decision.provider_route.clone(),
        None,
        overwrite_existing,
    )
    .await?;
    if let Some(source) = commands::acceptance::mark_generated_done_criteria(
        commands::acceptance::resolve_acceptance_source(&cwd, None)?,
    ) {
        let selection = commands::acceptance::done_criteria_selection(&Some(source))?;
        apply_done_criteria_selection(
            decision,
            StartDoneCriteriaSource::Generated,
            StartDoneAction::Existing,
            selection.full_label(),
            &selection,
        )?;
    }
    Ok(())
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
    let seams = read_seams_config(&paths.config_path(), args.no_seams)?;
    rows.push(("seams".to_string(), seam_preview_label(&seams)));
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
async fn start_replay_command(args: StartCommandArgs, plan_path: &Path) -> Result<()> {
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
    decision.provider_route = plan.providers.coder.clone();
    decision.planner_provider_route = plan.providers.planner.clone();
    decision.coder_provider_route = plan.providers.coder.clone();
    decision.reviewer_provider_route = plan.providers.reviewer.clone();
    decision.confirmed_by_start_picker = args.yes;
    let mut replay_args = args;
    replay_args.goal = plan.goal.clone();
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
    materialize_start_done_criteria(&mut decision).await?;
    dispatch_start_command(replay_args, &decision, plan).await
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
    let force_contract_review = args.review_done
        || config_defaults(&paths)
            .ok()
            .and_then(|defaults| defaults.start_confirm_contract)
            .unwrap_or(false);
    apply_forced_contract_review_guard(&mut decision, eligibility, force_contract_review);
    if args.json && !args.yes {
        let surface = start_preview_surface(&decision, &args, &paths)?;
        let mut next_actions = vec![surface.primary_action.command.clone()];
        next_actions.extend(start_preview_secondary_actions(&decision));
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
            "goal": decision.goal,
            "selected_mode": decision.selected_mode.label(),
            "selection_source": decision.selection_source.label(),
            "reason": decision.reason,
            "provider": decision.provider_label,
            "provider_source": decision.provider_source.label(),
            "done_criteria": decision.done_criteria_label,
            "done_criteria_source": decision.done_criteria_source.label(),
            "done_contract": start_done_contract_json(&decision),
            "source_mode": decision.source_mode.label(),
            "goal_shape": &decision.goal_shape,
            "requires_confirmation": decision.requires_confirmation,
            "will_start": false,
            "history_actions": decision.history_next_actions,
            "next_actions": next_actions,
            "try_lines": decision.try_lines
        }));
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    if args.preview {
        if !args.quiet {
            print_start_preview_surface(&decision, &args, &paths)?;
        }
        return Ok(());
    }
    if decision.recovery.is_none() && eligibility.allows_prompts() {
        prompt_start_launch_confirmation(&mut decision, &args, &paths, &mut terminal_prompter)?;
    }
    if !args.quiet && decision.recovery.is_none() {
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
    materialize_start_done_criteria(&mut decision).await?;
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
    let launch_plan = commands::course::launch_plan_from_decision(&decision, ceiling, accepted_by);
    if args.json && args.yes {
        // C-P10: launch JSON parity — dispatch quietly, then emit one
        // machine envelope carrying the plan and what actually launched.
        let before_runs = start_run_ids(&paths)?;
        let before_plans = start_plan_ids(&paths)?;
        let goal = decision.goal.clone();
        let mode_label = decision.selected_mode.label().to_string();
        let mut quiet_args = args;
        quiet_args.quiet = true;
        dispatch_start_command(quiet_args, &decision, launch_plan.clone()).await?;
        let mut dispatched_ids: Vec<String> = Vec::new();
        if let Some(run) = newest_start_run(&paths, &before_runs, &goal)? {
            dispatched_ids.push(run.run_id);
        }
        if let Some(plan_entry) = newest_start_plan(&paths, &before_plans, &goal)? {
            dispatched_ids.push(plan_entry.plan_id);
        }
        let next_actions: Vec<String> = dispatched_ids
            .first()
            .map(|id| {
                vec![
                    format!("deadreckon attach {id}"),
                    format!("deadreckon status {id}"),
                ]
            })
            .unwrap_or_default();
        let envelope = json!({
            "kind": "launch",
            "goal": goal,
            "mode": mode_label,
            "plan": &launch_plan,
            "done_contract": start_done_contract_json(&decision),
            "dispatched": { "mode": mode_label, "ids": dispatched_ids },
            "next_actions": next_actions,
        });
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }
    dispatch_start_command(args, &decision, launch_plan).await
}

#[derive(Clone, Copy)]
struct StartAttachFlags {
    json: bool,
    quiet: bool,
    preview: bool,
}

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
) -> Result<()> {
    let args_snapshot = StartAttachFlags {
        json: args.json,
        quiet: args.quiet,
        preview: args.preview,
    };
    match decision.selected_mode {
        StartSelectedMode::Run => {
            let paths = DeadreckonPaths::discover();
            let before = start_run_ids(&paths)?;
            let goal = args.goal.clone();
            let quiet = args.quiet;
            let auto_confirm = args.yes || args.quiet || decision.confirmed_by_start_picker;
            let result = commands::run::run_command_with_launch_plan(
                RunCommandArgs {
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
                    no_seams: args.no_seams,
                    plain: args.plain,
                    prevent_sleep: None,
                    quiet: args.quiet,
                    max_spend: args.max_spend,
                    max_wall_seconds: None,
                    sandbox: None,
                    provider: decision.provider_route.clone(),
                    model: decision.model.clone().or_else(|| args.model.clone()),
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
                    narrate: false,
                    no_narrate: false,
                    narrator_model: None,
                    infer_contract: false,
                },
                launch_plan,
            )
            .await;
            if result.is_ok()
                && !quiet
                && let Some(run) = newest_start_run(&paths, &before, &goal)?
            {
                print_start_lifecycle_footer("run", &run.run_id);
                maybe_start_attach(&run.run_id, &args_snapshot).await;
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
            let result = super::lifecycle::extend_command(ExtendCommandArgs {
                parent_run_id,
                new_goal: args.goal,
                dest: None,
                max_context_turns: None,
                no_context: false,
                max_spend: args.max_spend,
                max_wall_seconds: None,
                provider: decision.provider_route.clone(),
                model: decision.model.clone().or_else(|| args.model.clone()),
                sandbox: None,
                no_docs: false,
                doc_skill: None,
                post_actions: !args.quiet,
                narrate: false,
                no_narrate: false,
                narrator_model: None,
            })
            .await;
            if result.is_ok()
                && let Some(run) = newest_start_run(&paths, &before, &goal)?
            {
                if let Some(root) = run.state_path.parent() {
                    commands::course::save_launch_plan_best_effort(root, &launch_plan);
                }
                if !quiet {
                    print_start_lifecycle_footer("run", &run.run_id);
                    maybe_start_attach(&run.run_id, &args_snapshot).await;
                }
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
                planner_model: None,
                model: args.model.clone(),
                max_spend: args.max_spend,
                max_wall_seconds: None,
                sandbox: None,
                preview: false,
                yes: args.yes || args.quiet || decision.confirmed_by_start_picker,
                no_hints: args.quiet,
                quiet: args.quiet,
                plain: args.plain,
                narrate: false,
                no_narrate: false,
                narrator_model: None,
            })
            .await;
            if result.is_ok()
                && let Some(plan) = newest_start_plan(&paths, &before, &goal)?
            {
                commands::course::save_launch_plan_best_effort(
                    &paths.plan_dir(&plan.plan_id),
                    &launch_plan,
                );
                if !quiet {
                    print_start_lifecycle_footer("campaign", &plan.plan_id);
                    maybe_start_attach(&plan.plan_id, &args_snapshot).await;
                }
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
                        max_spend: args.max_spend,
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
                        planner_model: None,
                        model: args.model.clone(),
                        child_model: Vec::new(),
                        coder_model: None,
                        reviewer_model: None,
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
                    completion_surface: false,
                    narrate: false,
                    narrator_model: None,
                },
            )
            .await;
            if result.is_ok()
                && let Some(plan) = newest_start_plan(&paths, &before, &goal)?
            {
                commands::course::save_launch_plan_best_effort(
                    &paths.plan_dir(&plan.plan_id),
                    &launch_plan,
                );
                if !quiet {
                    print_start_lifecycle_footer("plan", &plan.plan_id);
                    maybe_start_attach(&plan.plan_id, &args_snapshot).await;
                }
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
    Ok(super::inspection::list_plan_entries(paths, None)?
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
) -> Result<Option<super::inspection::PlanListEntry>> {
    let mut plans = super::inspection::list_plan_entries(paths, None)?
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
    print!(
        "{}",
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "start",
            Some(&id),
            ExplanationPanel::new(
                format!("DeadReckon launched the guided start path as a {kind}."),
                "The id now exists; attach is the safest first command for observing the launched work before applying, finishing, or stopping it.",
                vec![
                    ("target".to_string(), kind.to_string()),
                    ("id".to_string(), id.clone()),
                ],
            ),
            vec![("Recommended", attach.as_str())],
            vec![
                ("Secondary", status.as_str()),
                ("Secondary", kill.as_str()),
                ("Secondary", finish.as_str()),
            ],
        )
        .render_plain(!completion_hints_enabled(false))
    );
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

        resolve_start_done_criteria(&mut decision, dir.path(), None, true).expect("resolve");

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

        resolve_start_done_criteria(&mut decision, dir.path(), None, true).expect("resolve");

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
