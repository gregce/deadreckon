use super::super::*;
use deadreckon_protocol::{RunEvent, TraceRecord};

pub(crate) fn list_command(
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
    let jobs = super::job::list_jobs(&paths, effective_scope.as_deref())?;
    let job_ids = jobs
        .iter()
        .map(|view| view.job.job_id.as_ref().to_string())
        .collect::<BTreeSet<_>>();
    let runs = list_runs(&paths, effective_scope.as_deref())?
        .into_iter()
        .filter(|run| !job_ids.contains(&run.run_id))
        .collect::<Vec<_>>();
    let plans = list_plan_entries(&paths, effective_scope.as_deref())?
        .into_iter()
        .filter(|plan| !job_ids.contains(&plan.plan_id))
        .collect::<Vec<_>>();
    let chains = super::chain::list_chain_records(&paths, effective_scope.clone())?;
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
        let chains = chains
            .iter()
            .map(|chain| {
                json!({
                    "chain_id": &chain.chain_id,
                    "scope": &chain.scope,
                    "goal": &chain.root_goal,
                    "status": deadreckon_core::chain_status_label(chain.status),
                    "updated_at": chain_activity_at(&paths, chain),
                    "steps": {
                        "applied": chain
                            .steps
                            .iter()
                            .filter(|step| step.status == ChainStepStatus::Applied)
                            .count(),
                        "total": chain.steps.len(),
                    },
                })
            })
            .collect::<Vec<_>>();
        let jobs = jobs
            .iter()
            .map(|view| {
                json!({
                    "job_id": view.job.job_id,
                    "scope": view.job.scope,
                    "goal": view.job.goal,
                    "status": super::job::job_status_label(view),
                    "updated_at": view.projection.updated_at.unwrap_or(view.job.created_at),
                    "attempts": view.projection.attempt_count,
                    "outcome": view.projection.outcome,
                    "stop_reason": view.projection.stop_reason,
                })
            })
            .collect::<Vec<_>>();
        let empty = runs.is_empty() && plans.is_empty() && chains.is_empty() && jobs.is_empty();
        let empty_surface = empty.then(|| empty_list_surface(effective_scope.as_deref(), all));
        let next_actions = if let Some(surface) = empty_surface.as_ref() {
            let mut actions = vec![surface.primary_action.command.clone()];
            actions.extend(
                surface
                    .secondary_actions
                    .iter()
                    .map(|action| action.command.clone()),
            );
            actions
        } else {
            vec![
                "deadreckon status latest".to_string(),
                "deadreckon attach <id>".to_string(),
                "deadreckon show <id>".to_string(),
            ]
        };
        let mut value = json!({
            "kind": "list",
            "id": effective_scope.as_deref().unwrap_or("all-scopes"),
            "status": "ok",
            "next_actions": next_actions,
            "try_lines": Vec::<String>::new(),
            "paths": {
                "home": paths.home(),
            },
            "runs": runs,
            "jobs": jobs,
            "plans": plans,
            "chains": chains,
        });
        if let Some(surface) = empty_surface.as_ref() {
            value = surface.add_to_json(value);
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    // Shakedown P9: children of a listed plan fold under it. One plan with six
    // children used to render as seven peer rows, each repeating the same root
    // goal. A child whose parent is not in this listing stays top-level, so
    // nothing disappears from the inventory.
    let child_index = plan_child_index(&paths, &plans);
    let listed_plans = plans
        .iter()
        .map(|plan| plan.plan_id.clone())
        .collect::<BTreeSet<_>>();
    let mut folded: BTreeMap<String, Vec<RunListEntry>> = BTreeMap::new();
    let mut top_level_runs = Vec::new();
    for run in runs {
        match child_index.get(&run.run_id) {
            Some((plan_id, _)) if listed_plans.contains(plan_id) => {
                folded.entry(plan_id.clone()).or_default().push(run);
            }
            _ => top_level_runs.push(run),
        }
    }
    let mut entries = top_level_runs
        .into_iter()
        .map(ListEntry::Run)
        .chain(jobs.into_iter().map(|view| ListEntry::Job(Box::new(view))))
        .chain(plans.into_iter().map(ListEntry::Plan))
        .chain(chains.into_iter().map(|chain| {
            let activity = chain_activity_at(&paths, &chain);
            ListEntry::Chain {
                chain: Box::new(chain),
                activity,
            }
        }))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| Reverse(entry.updated_at()));
    if entries.is_empty() {
        print!(
            "{}",
            empty_list_surface(effective_scope.as_deref(), all)
                .render_plain(!crate::completion_hints_enabled(false))
        );
        return Ok(());
    }
    let header = list_header();
    let mut output = String::new();
    output.push_str(&ui_heading(header));
    output.push('\n');
    let goal_width = list_goal_width();
    for entry in entries {
        match entry {
            ListEntry::Job(view) => {
                let status = super::job::job_status_label(&view);
                let action = super::job::job_primary_action(&view, "attach");
                append_list_row(
                    &mut output,
                    &ListRow {
                        id: run_prefix(view.job.job_id.as_ref()),
                        status,
                        age: relative_age(
                            view.projection.updated_at.unwrap_or(view.job.created_at),
                        ),
                        scope: view.job.scope,
                        kind: "job".to_string(),
                        mode: format!("{:?}", view.job.shape).to_ascii_lowercase(),
                        action: action.to_string(),
                        goal: view.job.goal,
                        goal_width,
                        orchestration: false,
                        child: false,
                    },
                );
            }
            ListEntry::Run(run) => {
                let state = load_state(&run.state_path).ok();
                let mode = state
                    .as_ref()
                    .map(|state| codebase_mode_status(&paths, state))
                    .unwrap_or_else(|| "-".to_string());
                let action = state
                    .as_ref()
                    .map(|state| next_action_label(&paths, state))
                    .unwrap_or_else(|| "-".to_string());
                append_list_row(
                    &mut output,
                    &ListRow {
                        id: run_prefix(&run.run_id),
                        status: run_status_label(run.status).to_string(),
                        age: relative_age(run.updated_at),
                        scope: run.scope.clone(),
                        kind: "run".to_string(),
                        mode,
                        action,
                        goal: run.goal.clone(),
                        goal_width,
                        orchestration: false,
                        child: false,
                    },
                );
            }
            ListEntry::Chain { chain, activity } => {
                let applied = chain
                    .steps
                    .iter()
                    .filter(|step| step.status == ChainStepStatus::Applied)
                    .count();
                append_list_row(
                    &mut output,
                    &ListRow {
                        id: run_prefix(&chain.chain_id),
                        status: deadreckon_core::chain_status_label(chain.status).to_string(),
                        age: relative_age(activity),
                        scope: chain.scope.clone(),
                        kind: "chain".to_string(),
                        // A chain is a sequential graph that lands each step;
                        // progress belongs in the action column, matching how
                        // plan rows read ("attach 2/3").
                        mode: "sequential".to_string(),
                        action: chain_action_label(chain.status, applied, chain.steps.len()),
                        goal: chain.root_goal.clone(),
                        goal_width,
                        orchestration: true,
                        child: false,
                    },
                );
            }
            ListEntry::Plan(plan) => {
                append_list_row(
                    &mut output,
                    &ListRow {
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
                        child: false,
                    },
                );
                for child in folded.get(&plan.plan_id).into_iter().flatten() {
                    let state = load_state(&child.state_path).ok();
                    let subject = child_index
                        .get(&child.run_id)
                        .map(|(_, subject)| subject.clone())
                        .unwrap_or_else(|| child.goal.clone());
                    append_list_row(
                        &mut output,
                        &ListRow {
                            id: run_prefix(&child.run_id),
                            status: run_status_label(child.status).to_string(),
                            age: relative_age(child.updated_at),
                            scope: child.scope.clone(),
                            kind: "child".to_string(),
                            mode: state
                                .as_ref()
                                .map(|state| codebase_mode_status(&paths, state))
                                .unwrap_or_else(|| "-".to_string()),
                            action: state
                                .as_ref()
                                .map(|state| next_action_label(&paths, state))
                                .unwrap_or_else(|| "-".to_string()),
                            goal: subject,
                            goal_width,
                            orchestration: false,
                            child: true,
                        },
                    );
                }
            }
        }
    }
    append_list_action_footer(&mut output);
    print!("{output}");
    Ok(())
}

fn empty_list_surface(scope: Option<&str>, all: bool) -> VerdictSurface {
    let subject = if all || scope.is_none() {
        "all-scopes"
    } else {
        "current-project"
    };
    let scope_label = match scope {
        Some(scope) if !all => format!("current project ({scope})"),
        _ => "all scopes".to_string(),
    };
    let primary = if all || scope.is_none() {
        "deadreckon start \"goal\""
    } else {
        "deadreckon list --all"
    };
    let secondary = if all || scope.is_none() {
        vec![("Secondary", "deadreckon init")]
    } else {
        vec![
            ("Secondary", "deadreckon start \"goal\""),
            ("Secondary", "deadreckon run \"goal\""),
        ]
    };
    VerdictSurface::must_new(
        VerdictKind::Noop,
        "list",
        Some(subject),
        ExplanationPanel::new(
            format!("List found no runs or plans for {scope_label}."),
            "This is a no-op because the selected run inventory scope has no durable run or orchestration records.",
            vec![
                ("scope".to_string(), scope_label),
                ("inventory".to_string(), "runs and plans".to_string()),
                (
                    "state".to_string(),
                    "no matching runstate entries".to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary)],
        secondary,
    )
}

fn append_list_action_footer(output: &mut String) {
    output.push('\n');
    output.push_str(&ui_heading("Recommended"));
    output.push('\n');
    output.push_str(&ui_command("deadreckon status latest"));
    output.push('\n');
    output.push('\n');
    output.push_str(&ui_heading("Secondary"));
    output.push('\n');
    output.push_str(&ui_command("deadreckon list --all"));
    output.push('\n');
    output.push_str(&ui_command("deadreckon attach <id>"));
    output.push('\n');
    output.push_str(&ui_command("deadreckon show <id>"));
    output.push('\n');
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
    pub(crate) scope: String,
    kind: String,
    mode: String,
    action: String,
    pub(crate) goal: String,
    goal_width: usize,
    orchestration: bool,
    /// A run folded under its parent plan: indented, so the inventory reads as
    /// one row per plan rather than N peers repeating the same root goal.
    child: bool,
}

fn list_header() -> String {
    format!(
        "{}  {}  {}  {}  {}  {}  {}  goal",
        pad_plain("id", LIST_ID_WIDTH),
        pad_plain("status", LIST_STATUS_WIDTH),
        pad_plain("age", LIST_AGE_WIDTH),
        pad_plain("scope", LIST_SCOPE_WIDTH),
        pad_plain("kind", LIST_KIND_WIDTH),
        pad_plain("mode", LIST_MODE_WIDTH),
        pad_plain("action", LIST_ACTION_WIDTH)
    )
}

/// Shakedown P9: which runs are children of which listed plan, and the subject
/// each was given.
///
/// A child run's goal is the launch prompt it was handed -- "This is one
/// full-plan child run in a larger plan. Root goal: ..." -- which is plumbing,
/// not inventory. The plan task's `subject` is the operator-meaningful name for
/// that piece of work, so folded children show it instead of a string-surgery
/// attempt on the prompt.
fn plan_child_index(
    paths: &DeadreckonPaths,
    plans: &[PlanListEntry],
) -> BTreeMap<String, (String, String)> {
    let mut index = BTreeMap::new();
    for entry in plans {
        let Ok(plan) = load_plan(paths, &entry.plan_id) else {
            continue;
        };
        for task in &plan.tasks {
            if let Some(child_run_id) = task.child_run_id.as_ref() {
                index.insert(
                    child_run_id.clone(),
                    (plan.plan_id.clone(), task.subject.clone()),
                );
            }
        }
    }
    index
}

fn append_list_row(output: &mut String, row: &ListRow) {
    let first_prefix = format!(
        "{}  {}  {}  {}  {}  {}  {}  ",
        pad_rendered(&row.id, LIST_ID_WIDTH, Some(ui_id)),
        pad_rendered(&row.status, LIST_STATUS_WIDTH, Some(ui_status)),
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
    // A folded child is indented as a whole line. Putting the marker inside the
    // id cell would overflow its fixed width and truncate the id itself.
    let indent = if row.child { "  " } else { "" };
    for (index, line) in goal_lines.iter().enumerate() {
        output.push_str(indent);
        if index == 0 {
            output.push_str(&first_prefix);
            output.push_str(line);
        } else {
            output.push_str(&continuation_prefix);
            output.push_str(line);
        }
        output.push('\n');
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
    let padding = width.saturating_sub(crate::ui::display_width(&plain));
    format!("{plain}{}", " ".repeat(padding))
}

fn pad_rendered(value: &str, width: usize, render: Option<fn(String) -> String>) -> String {
    let plain = truncate_text(value, width);
    let padding = width.saturating_sub(crate::ui::display_width(&plain));
    let rendered = render.map_or_else(|| plain.clone(), |render| render(plain.clone()));
    format!("{rendered}{}", " ".repeat(padding))
}

fn wrap_list_goal(value: &str, width: usize) -> Vec<String> {
    let compact = compact_whitespace(value);
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
    crate::ui::wrap_words(value, width)
}

#[derive(Debug)]
enum ListEntry {
    Job(Box<deadreckon_core::JobView>),
    Run(RunListEntry),
    Plan(PlanListEntry),
    /// Chains were invisible here until now: `deadreckon status <chain-id>`
    /// resolved one while `deadreckon list` omitted it entirely, so the only
    /// way to find a chain was to already know it existed and type a second,
    /// differently-formatted command.
    /// Boxed: a Chain carries its full step vector, dwarfing the other
    /// variants inline (the same reason ResolvedRef boxes its payloads).
    /// `activity` is when the chain last changed, not when it started — a
    /// chain paused for days must not sort as if nothing happened since its
    /// first step.
    Chain {
        chain: Box<Chain>,
        activity: DateTime<Utc>,
    },
}

impl ListEntry {
    fn updated_at(&self) -> DateTime<Utc> {
        match self {
            Self::Job(view) => view.projection.updated_at.unwrap_or(view.job.created_at),
            Self::Run(run) => run.updated_at,
            Self::Plan(plan) => plan.updated_at,
            Self::Chain { activity, .. } => *activity,
        }
    }
}

/// When this chain last changed. Every lifecycle transition rewrites
/// chain.json, so its mtime is the honest activity stamp; the lifecycle
/// fields are the fallback when the filesystem will not say.
fn chain_activity_at(paths: &DeadreckonPaths, chain: &Chain) -> DateTime<Utc> {
    std::fs::metadata(paths.chain_json(&chain.chain_id))
        .and_then(|meta| meta.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| {
            chain
                .completed_at
                .or(chain.started_at)
                .unwrap_or(chain.created_at)
        })
}

#[derive(Debug)]
pub(crate) struct PlanListEntry {
    pub(crate) plan_id: String,
    pub(crate) merged_run_id: Option<String>,
    pub(crate) scope: String,
    pub(crate) goal: String,
    pub(crate) status: PlanStatus,
    pub(crate) mode: PlanMode,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) plan_path: PathBuf,
    pub(crate) completed_children: usize,
    pub(crate) total_children: usize,
}

pub(crate) fn list_plan_entries(
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

/// The one thing to do next with a chain, in the same vocabulary the run and
/// plan rows use.
fn chain_action_label(
    status: deadreckon_core::ChainStatus,
    applied: usize,
    total: usize,
) -> String {
    match status {
        ChainStatus::Pending => "run".to_string(),
        ChainStatus::Running => format!("attach {applied}/{total}"),
        ChainStatus::Paused => format!("resume {applied}/{total}"),
        ChainStatus::Completed => "finish".to_string(),
        ChainStatus::Failed => "show failure".to_string(),
        ChainStatus::Killed | ChainStatus::Undone => "show".to_string(),
    }
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
            let compiled = Regex::new(&pattern)
                .map_err(|err| invalid_history_regex_error(&pattern, &err.to_string()))?;
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

fn invalid_history_regex_error(pattern: &str, parser_error: &str) -> CliError {
    let primary = format!(
        "deadreckon history grep {} --regex",
        command_literal(pattern)
    );
    let secondary = format!("deadreckon history grep {}", command_literal(pattern));
    CliError::Surface {
        code: 2,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "history grep",
            Some("--regex"),
            ExplanationPanel::new(
                format!("History grep could not compile regex pattern {pattern:?}."),
                "The command stopped before scanning history because --regex requires a valid Rust regular expression.",
                vec![
                    ("pattern".to_string(), pattern.to_string()),
                    ("parser error".to_string(), parser_error.to_string()),
                    ("mode".to_string(), "regex".to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .render_plain(!crate::completion_hints_enabled(false)),
    }
}

fn invalid_history_limit_error(pattern: &str, value: usize) -> CliError {
    let primary = format!(
        "deadreckon history grep {} --limit 20",
        command_literal(pattern)
    );
    let secondary = format!("deadreckon history grep {}", command_literal(pattern));
    CliError::Surface {
        code: 2,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "history grep",
            Some("--limit"),
            ExplanationPanel::new(
                format!("History grep cannot run with --limit {value}."),
                "The command stopped before scanning history because --limit must allow at least one matching line to be printed.",
                vec![
                    ("filter".to_string(), "--limit".to_string()),
                    ("value".to_string(), value.to_string()),
                    ("minimum".to_string(), "1".to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .render_plain(!crate::completion_hints_enabled(false)),
    }
}

pub(crate) fn history_command(command: HistoryCommand) -> Result<()> {
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
        return Err(invalid_history_limit_error(&pattern, limit));
    }
    let paths = DeadreckonPaths::discover();
    let matcher = HistoryMatcher::new(pattern.clone(), regex)?;
    let cutoff = parse_history_since(&pattern, since)?;
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
                ui_muted(event.timestamp.to_rfc3339()),
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
                ui_muted(timestamp.to_rfc3339()),
                run.scope,
                one_line(line.trim(), 220)
            );
            printed += 1;
        }
    }
    if total_matches == 0 {
        print_history_grep_no_matches_surface(
            &pattern,
            kind,
            regex,
            plan.as_deref(),
            effective_scope.as_deref(),
            all,
        );
    } else if total_matches > printed {
        println!(
            "{}",
            ui_muted(format!("... ({} more)", total_matches - printed))
        );
    }
    Ok(())
}

fn print_history_grep_no_matches_surface(
    pattern: &str,
    kind: HistoryKind,
    regex: bool,
    plan: Option<&str>,
    scope: Option<&str>,
    all: bool,
) {
    let primary = format!("deadreckon history grep {} --all", command_literal(pattern));
    let search_scope = if let Some(plan) = plan {
        format!("plan {plan}")
    } else if all {
        "all scopes".to_string()
    } else if let Some(scope) = scope {
        format!("scope {scope}")
    } else {
        "current scope".to_string()
    };
    let surface = VerdictSurface::must_new(
        VerdictKind::Noop,
        "history grep",
        Some(pattern),
        ExplanationPanel::new(
            format!("History grep scanned {search_scope} and found no matches for {pattern:?}."),
            "This is a no-op because no matching history ledger lines were found; broadening the search to all scopes is the safest next check.",
            vec![
                ("pattern".to_string(), pattern.to_string()),
                ("scope".to_string(), search_scope),
                ("kind".to_string(), history_kind_label(kind).to_string()),
                (
                    "matcher".to_string(),
                    if regex { "regex" } else { "substring" }.to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon show <run-id>")],
    );
    print!(
        "{}",
        surface.render_plain(!crate::completion_hints_enabled(false))
    );
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
        HistoryKind::Events => RUN_EVENTS_JSONL,
    }
}

fn history_kind_label(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Trace => "trace",
        HistoryKind::Provenance => "provenance",
        HistoryKind::Events => "events",
    }
}

fn parse_history_since(pattern: &str, value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.len() < 2 {
        return Err(invalid_history_since_error(pattern, value));
    }
    let (amount, unit) = value.split_at(value.len() - 1);
    let amount = amount
        .parse::<i64>()
        .map_err(|_| invalid_history_since_error(pattern, value))?;
    if amount < 0 {
        return Err(invalid_history_since_error(pattern, value));
    }
    let duration = match unit {
        "d" => ChronoDuration::days(amount),
        "h" => ChronoDuration::hours(amount),
        "m" => ChronoDuration::minutes(amount),
        _ => return Err(invalid_history_since_error(pattern, value)),
    };
    Ok(Some(Utc::now() - duration))
}

fn invalid_history_since_error(pattern: &str, value: &str) -> CliError {
    let primary = format!(
        "deadreckon history grep {} --since 7d",
        command_literal(pattern)
    );
    let secondary = format!("deadreckon history grep {} --all", command_literal(pattern));
    CliError::Surface {
        code: 2,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "history grep",
            Some("--since"),
            ExplanationPanel::new(
                format!("History grep could not parse --since duration {value:?}."),
                "The command stopped before scanning history because --since must be a positive duration ending in d, h, or m.",
                vec![
                    ("filter".to_string(), "--since".to_string()),
                    ("value".to_string(), value.to_string()),
                    ("accepted duration".to_string(), "7d, 24h, or 30m".to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", secondary.as_str())],
        )
        .render_plain(!crate::completion_hints_enabled(false)),
    }
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
        HistoryKind::Events => serde_json::from_str::<RunEvent>(line)
            .ok()
            .map(|record| record.timestamp),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LibraryEntry {
    pub(crate) manifest: PromotionManifest,
    pub(crate) path: PathBuf,
    pub(crate) materialized_count: usize,
}

pub(crate) fn library_command(command: LibraryCommand) -> Result<()> {
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
                print_library_search_no_matches_surface(&query, all);
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

fn print_library_search_no_matches_surface(query: &str, all: bool) {
    let primary = if all {
        "deadreckon library list --all".to_string()
    } else {
        format!("deadreckon library search {} --all", command_literal(query))
    };
    let scope = if all { "all scopes" } else { "current scope" };
    let surface = VerdictSurface::must_new(
        VerdictKind::Noop,
        "library search",
        Some(query),
        ExplanationPanel::new(
            format!("Library search scanned {scope} and found no artifact matching {query:?}."),
            "This is a no-op because no promoted artifact metadata or run documentation matched; widening to all scopes is the safest next check.",
            vec![
                ("query".to_string(), query.to_string()),
                ("scope".to_string(), scope.to_string()),
                (
                    "source".to_string(),
                    "promoted artifact metadata and docs".to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon library list --all")],
    );
    print!(
        "{}",
        surface.render_plain(!crate::completion_hints_enabled(false))
    );
}

fn command_literal(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '_' | '-' | '.' | '/' | ':' | '@' | '%' | '+' | '=' | ','
                )
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
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
            return Err(invalid_library_date_error(label, &value));
        };
        return Ok(Some(date_time.and_utc()));
    }
    Err(invalid_library_date_error(label, &value))
}

fn invalid_library_date_error(label: &str, value: &str) -> CliError {
    let primary = format!("deadreckon library list {label} 2026-05-11");
    CliError::Surface {
        code: 2,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "library list",
            Some(label),
            ExplanationPanel::new(
                format!("Library list could not parse {label} date {value:?}."),
                "The command stopped before reading artifacts because date filters must be RFC3339 timestamps or YYYY-MM-DD dates.",
                vec![
                    ("filter".to_string(), label.to_string()),
                    ("value".to_string(), value.to_string()),
                    ("accepted format".to_string(), "YYYY-MM-DD or RFC3339".to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", "deadreckon library list --all")],
        )
        .render_plain(!crate::completion_hints_enabled(false)),
    }
}

pub(crate) fn library_entries(
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

pub(crate) fn materialized_marker_count(library_dir: &Path) -> usize {
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
    let subject = match scope {
        Some(scope) => scope.to_string(),
        None if all => "all-scopes".to_string(),
        None => "current-project".to_string(),
    };
    let scope_label = match scope {
        Some(scope) => format!("scope {scope}"),
        None if all => "all scopes".to_string(),
        None => "current project".to_string(),
    };
    let surface = VerdictSurface::must_new(
        VerdictKind::Noop,
        "library list",
        Some(&subject),
        ExplanationPanel::new(
            format!("Library list found no promoted artifacts for {scope_label}."),
            "This is a no-op because there are no completed fresh/copy run artifacts in the selected library scope.",
            vec![
                ("scope".to_string(), scope_label),
                ("library".to_string(), "promoted run artifacts".to_string()),
                (
                    "promotion".to_string(),
                    "completed fresh/copy runs promote automatically".to_string(),
                ),
            ],
        ),
        vec![("Recommended", "deadreckon library list --all")],
        vec![
            ("Secondary", "deadreckon run \"goal\""),
            ("Secondary", "deadreckon finish <run-id>"),
        ],
    );
    print!(
        "{}",
        surface.render_plain(!crate::completion_hints_enabled(false))
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

    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            let manifest = &entry.manifest;
            vec![
                ui_id(run_prefix(&manifest.run_id)),
                relative_age(manifest.promoted_at),
                truncate_text(&manifest.scope, 26),
                materialized_count_label(entry.materialized_count),
                truncate_text(&one_line(&manifest.goal, 88), 88),
            ]
        })
        .collect();
    let headers = [
        ui_heading("run"),
        ui_heading("age"),
        ui_heading("scope"),
        ui_heading("exported"),
        ui_heading("goal"),
    ];
    print!(
        "{}",
        crate::columns(
            &headers.iter().map(String::as_str).collect::<Vec<_>>(),
            &rows
        )
    );
    print_library_table_action_footer();
}

fn print_library_table_action_footer() {
    println!();
    println!("{}", ui_heading("Recommended"));
    println!("{}", ui_command("deadreckon library show <run-id>"));
    println!();
    println!("{}", ui_heading("Secondary"));
    println!("{}", ui_command("deadreckon export <run-id> --dest <path>"));
}

pub(crate) fn materialized_count_label(count: usize) -> String {
    match count {
        0 => "no".to_string(),
        1 => "yes (1)".to_string(),
        count => format!("yes ({count})"),
    }
}

fn print_library_entry(entry: &LibraryEntry) {
    let manifest = &entry.manifest;
    println!("{}", ui_heading("library artifact"));
    println!("{}        {}", ui_muted("run:"), ui_id(&manifest.run_id));
    println!("{}      {}", ui_muted("scope:"), manifest.scope);
    println!("{}   {}", ui_muted("promoted:"), manifest.promoted_at);
    println!(
        "{}   {}",
        ui_muted("exported:"),
        materialized_count_label(entry.materialized_count)
    );
    println!("{}       {}", ui_muted("path:"), entry.path.display());
    println!(
        "{}     {}",
        ui_muted("source:"),
        manifest.source_working_dir.display()
    );
    println!(
        "{} {}",
        ui_muted("provenance:"),
        ui_id(&manifest.provenance_hash)
    );
    println!("{}       {}", ui_muted("goal:"), manifest.goal);
    println!();
    println!("{}", ui_heading("Recommended"));
    println!(
        "{}",
        ui_command(format!(
            "deadreckon export {}",
            run_prefix(&manifest.run_id)
        ))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_library_use_shared_columns() {
        // The run list header is lowercase (migrated off UPPERCASE column names).
        let header = list_header();
        assert_eq!(
            header,
            header.to_lowercase(),
            "list header must be lowercase: {header}"
        );
        assert!(header.contains("id") && header.contains("goal"), "{header}");
        // The library table renders through the shared columns helper, which
        // emits lowercase headers.
        let table = crate::columns(
            &["run", "age", "scope"],
            &[vec!["x".to_string(), "1m".to_string(), "s".to_string()]],
        );
        assert!(table.starts_with("run"), "{table}");
    }

    #[test]
    fn no_hints_suppresses_inspection_doc_chain_hints() {
        // empty_list_surface carries secondary "next step" actions. The inspection,
        // doc, and chain completion surfaces all render through the same
        // VerdictSurface::render_plain, so suppressing here covers the shared
        // discipline: no_hints drops the Secondary block.
        let surface = empty_list_surface(None, true);
        assert!(
            !surface.secondary_actions.is_empty(),
            "empty list surface should carry secondary hints to suppress"
        );
        let shown = surface.render_plain(false);
        let hidden = surface.render_plain(true);
        assert!(shown.contains("\nSecondary\n"), "{shown}");
        assert!(
            !hidden.contains("\nSecondary\n"),
            "no_hints must drop the secondary actions\n{hidden}"
        );
    }
}
