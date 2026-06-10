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
    GenerateFromGoal,
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
    pub(crate) child_count: Option<u8>,
    pub(crate) planner_provider_route: Option<String>,
    pub(crate) child_provider_route: Option<String>,
    pub(crate) child_provider_overrides: Vec<String>,
    pub(crate) coder_provider_route: Option<String>,
    pub(crate) reviewer_provider_route: Option<String>,
    pub(crate) done_criteria_source: StartDoneCriteriaSource,
    pub(crate) done_action: StartDoneAction,
    pub(crate) done_criteria_label: String,
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

pub(crate) async fn classify_goal_shape_for_start(
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

pub(crate) fn parse_provider_goal_shape(
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

pub(crate) fn fallback_goal_shape_recommendation(goal: &str) -> GoalShapeRecommendation {
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

pub(crate) fn prompt_start_done_criteria(
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

pub(crate) fn prompt_start_existing_done_criteria(
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
            .first()
            .cloned()
            .unwrap_or_else(|| "deadreckon doctor".to_string()),
    }
}

pub(crate) fn start_done_materialization_request(
    decision: &StartLaunchDecision,
) -> Option<(String, bool)> {
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
    Ok(VerdictSurface::try_new(
        kind,
        "start",
        None,
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary,
    )
    .expect("start preview verdict surface must be valid"))
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
pub(crate) fn resolve_start_goal(provided: Option<String>, allows_prompts: bool) -> Result<String> {
    match start_goal_plan(provided.as_deref(), allows_prompts) {
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

pub(crate) async fn start_command(args: StartCommandArgs) -> Result<()> {
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
                no_seams: args.no_seams,
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
            let result = super::lifecycle::extend_command(ExtendCommandArgs {
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
                planner_model: None,
                model: args.model.clone(),
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
        VerdictSurface::try_new(
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
        .expect("start lifecycle verdict surface must be valid")
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
