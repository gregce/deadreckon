//! The single reference resolver.
//!
//! Before Shakedown every id-taking verb hand-rolled its own cascade and no two
//! covered the same kinds in the same order: `show` probed campaign, plan-child,
//! run, plan and missed chains; `kill` probed campaign, run, plan, chain and
//! missed plan children; `status` and `verdict` saw runs only. An operator could
//! take an id printed by `list`, hand it to `status`, and be told it did not
//! exist. This module is the one place that answers "what does this reference
//! name?", so a verb can only disagree with another verb by declaring different
//! `accepts` — which is data, and testable.

// P1 lands the resolver and its depth tests; P4-P7 rewire the verbs onto it and
// P8 deletes the old per-verb cascades. Until the first verb calls it, the
// non-test build sees this whole API as unreachable. Remove this allowance in
// P8, where every call site is expected to route through here.
#![allow(dead_code)]

use std::path::PathBuf;

use deadreckon_core::campaign::Campaign;
use deadreckon_core::{Chain, DeadreckonPaths, PipelineState, Plan, PlanTask};

use super::super::*;

/// What a reference turned out to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefKind {
    Run,
    PlanChild,
    Plan,
    Chain,
    Campaign,
}

impl RefKind {
    /// The operator-facing word. Refusals say "is a plan, not a run" — never a
    /// Rust type name.
    pub(crate) const fn noun(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::PlanChild => "plan child",
            Self::Plan => "plan",
            Self::Chain => "chain",
            Self::Campaign => "campaign",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Run => 1 << 0,
            Self::PlanChild => 1 << 1,
            Self::Plan => 1 << 2,
            Self::Chain => 1 << 3,
            Self::Campaign => 1 << 4,
        }
    }
}

/// The set of kinds a calling verb can handle. Five flags and one crate
/// boundary do not earn a `bitflags` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefKinds(u8);

impl RefKinds {
    pub(crate) const RUN: Self = Self(RefKind::Run.bit());
    pub(crate) const PLAN_CHILD: Self = Self(RefKind::PlanChild.bit());
    pub(crate) const PLAN: Self = Self(RefKind::Plan.bit());
    pub(crate) const CHAIN: Self = Self(RefKind::Chain.bit());
    pub(crate) const CAMPAIGN: Self = Self(RefKind::Campaign.bit());
    /// Every kind. `status`, `show` and `attach` orient across all of them.
    pub(crate) const ALL: Self = Self(
        RefKind::Run.bit()
            | RefKind::PlanChild.bit()
            | RefKind::Plan.bit()
            | RefKind::Chain.bit()
            | RefKind::Campaign.bit(),
    );

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn contains(self, kind: RefKind) -> bool {
        self.0 & kind.bit() != 0
    }
}

/// One resolved reference. Boxed payloads keep the enum small; `PipelineState`
/// alone is large enough that clippy's `large_enum_variant` fires otherwise.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedRef {
    Run(Box<PipelineState>),
    PlanChild {
        selection: PlanChildSelection,
        state: Box<PipelineState>,
    },
    Plan(Box<Plan>),
    Chain(Box<Chain>),
    Campaign {
        dir: PathBuf,
        campaign: Box<Campaign>,
    },
}

impl ResolvedRef {
    pub(crate) const fn kind(&self) -> RefKind {
        match self {
            Self::Run(_) => RefKind::Run,
            Self::PlanChild { .. } => RefKind::PlanChild,
            Self::Plan(_) => RefKind::Plan,
            Self::Chain(_) => RefKind::Chain,
            Self::Campaign { .. } => RefKind::Campaign,
        }
    }
}

/// One resolution request. `verb` appears only in refusal text.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RefQuery<'a> {
    pub(crate) reference: Option<&'a str>,
    pub(crate) accepts: RefKinds,
    pub(crate) all_scopes: bool,
    /// Names the refusing verb in the P3 refusal table.
    pub(crate) verb: &'static str,
}

/// A plan child, named as `<plan-ref>:<task>` or `<plan-ref>/<task>`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlanChildSelection {
    pub(crate) plan_id: String,
    pub(crate) task_id: String,
    pub(crate) run_id: String,
}

/// Resolve an operator-supplied reference.
///
/// Plan-child refs are checked first because their syntax is disjoint — they
/// contain `:` or `/`, which a bare id never does — not because they outrank
/// other kinds. Every other accepted kind is then probed and *all* matches are
/// collected: a prefix matching both a run and a plan is a refusal naming both,
/// never a silent win for whichever kind happened to be probed first. Guessing
/// is the failure mode this module exists to remove.
pub(crate) fn resolve_ref(paths: &DeadreckonPaths, query: RefQuery<'_>) -> Result<ResolvedRef> {
    let Some(reference) = query.reference else {
        return resolve_latest(paths, query);
    };
    if matches!(reference, "latest" | "last") {
        return resolve_latest(paths, query);
    }

    if query.accepts.contains(RefKind::PlanChild)
        && let Some(selection) = resolve_plan_child_ref(paths, reference)?
    {
        let state = load_run(paths, &selection.run_id)?;
        return Ok(ResolvedRef::PlanChild {
            selection,
            state: Box::new(state),
        });
    }

    let mut matches = Vec::new();
    if query.accepts.contains(RefKind::Run)
        && let Some(state) = probe_run(paths, reference)?
    {
        matches.push(ResolvedRef::Run(Box::new(state)));
    }
    if query.accepts.contains(RefKind::Plan)
        && let Some(plan) = probe_plan(paths, reference)?
    {
        matches.push(ResolvedRef::Plan(Box::new(plan)));
    }
    if query.accepts.contains(RefKind::Chain)
        && let Some(chain) = probe_chain(paths, reference, query.all_scopes)?
    {
        matches.push(ResolvedRef::Chain(Box::new(chain)));
    }
    if query.accepts.contains(RefKind::Campaign)
        && let Some((dir, campaign)) = super::campaign::resolve_campaign(paths, reference)?
    {
        matches.push(ResolvedRef::Campaign {
            dir,
            campaign: Box::new(campaign),
        });
    }

    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(unresolved_reference(paths, reference, query)),
        _ => Err(ambiguous_across_kinds(reference, &matches)),
    }
}

/// P1 keeps today's `latest` semantics verbatim; P2 replaces this with the one
/// scope-bound, `updated_at`-ordered rule that every verb shares.
fn resolve_latest(paths: &DeadreckonPaths, query: RefQuery<'_>) -> Result<ResolvedRef> {
    let state = latest_run(paths, query.all_scopes)?;
    Ok(ResolvedRef::Run(Box::new(state)))
}

/// A missing run is "no match"; an ambiguous prefix is the loader's own message,
/// passed through so the operator sees the candidate ids.
fn probe_run(paths: &DeadreckonPaths, reference: &str) -> Result<Option<PipelineState>> {
    match load_run(paths, reference) {
        Ok(state) => Ok(Some(state)),
        Err(DeadreckonError::NotFound(_)) => Ok(None),
        Err(source) => Err(CliError::from(source)),
    }
}

fn probe_plan(paths: &DeadreckonPaths, reference: &str) -> Result<Option<Plan>> {
    let mut ids = plan_ids_matching(paths, reference)?;
    match ids.len() {
        1 => Ok(Some(load_plan(paths, &ids.remove(0))?)),
        0 => Ok(None),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous plan id prefix {reference}; matches {}",
                ids.join(", ")
            ),
            "use a longer plan id prefix",
        ))),
    }
}

fn probe_chain(paths: &DeadreckonPaths, reference: &str, all: bool) -> Result<Option<Chain>> {
    let scope = if all { None } else { Some(current_scope()?) };
    let chains = super::chain::list_chain_records(paths, scope)?;
    let mut matches = chains
        .into_iter()
        .filter(|chain| chain.chain_id.starts_with(reference))
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(Some(matches.remove(0))),
        0 => Ok(None),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous chain id prefix {reference}; matches {}",
                matches
                    .iter()
                    .map(|chain| chain.chain_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "use a longer chain id prefix",
        ))),
    }
}

/// Nothing matched. An empty home is a different problem from a typo, so it gets
/// a different `try:` — pointing a first-time operator at `list` shows them an
/// empty table and no way forward.
fn unresolved_reference(paths: &DeadreckonPaths, reference: &str, query: RefQuery<'_>) -> CliError {
    let has_any_state = deadreckon_core::list_runs(paths, None)
        .map(|runs| !runs.is_empty())
        .unwrap_or(false)
        || plan_ids_matching(paths, "")
            .map(|plans| !plans.is_empty())
            .unwrap_or(false);
    if has_any_state {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "no {} matches {reference}",
                accepted_nouns(query.accepts).join(", ")
            ),
            "deadreckon list",
        ))
    } else {
        CliError::Core(deadreckon_core::user_error(
            "no runs or plans yet",
            "deadreckon start \"<goal>\"",
        ))
    }
}

fn ambiguous_across_kinds(reference: &str, matches: &[ResolvedRef]) -> CliError {
    let described = matches
        .iter()
        .map(|resolved| format!("{} {}", resolved.kind().noun(), resolved_id(resolved)))
        .collect::<Vec<_>>()
        .join(" and ");
    let longer = matches
        .first()
        .map(|resolved| format!("deadreckon show {}", resolved_id(resolved)))
        .unwrap_or_else(|| "deadreckon list".to_string());
    CliError::Core(deadreckon_core::user_error(
        &format!("{reference} matches {described}"),
        &longer,
    ))
}

pub(crate) fn resolved_id(resolved: &ResolvedRef) -> String {
    match resolved {
        ResolvedRef::Run(state) => state.run_id.clone(),
        ResolvedRef::PlanChild { selection, .. } => {
            format!("{}:{}", selection.plan_id, selection.task_id)
        }
        ResolvedRef::Plan(plan) => plan.plan_id.clone(),
        ResolvedRef::Chain(chain) => chain.chain_id.clone(),
        ResolvedRef::Campaign { campaign, .. } => campaign.campaign_id.clone(),
    }
}

fn accepted_nouns(accepts: RefKinds) -> Vec<&'static str> {
    [
        RefKind::Run,
        RefKind::Plan,
        RefKind::Chain,
        RefKind::Campaign,
    ]
    .into_iter()
    .filter(|kind| accepts.contains(*kind))
    .map(RefKind::noun)
    .collect()
}

/// Plan ids whose directory holds a `plan.json` and which start with `prefix`.
/// An empty prefix lists every plan.
pub(crate) fn plan_ids_matching(paths: &DeadreckonPaths, prefix: &str) -> Result<Vec<String>> {
    let plans_dir = paths.plans_dir();
    if !plans_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut plans = fs::read_dir(&plans_dir)
        .map_err(|source| DeadreckonError::Io {
            path: plans_dir.clone(),
            source,
        })?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join("plan.json").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|plan_id| plan_id.starts_with(prefix))
        .collect::<Vec<_>>();
    plans.sort();
    Ok(plans)
}

pub(crate) fn resolve_plan_id(paths: &DeadreckonPaths, id: &str) -> Result<String> {
    if matches!(id, "latest" | "last") {
        let mut plans = plan_ids_matching(paths, "")?;
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
    let matches = plan_ids_matching(paths, id)?;
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

pub(crate) fn resolve_plan_child_ref(
    paths: &DeadreckonPaths,
    id: &str,
) -> Result<Option<PlanChildSelection>> {
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

pub(crate) fn resolve_plan_child_task<'a>(plan: &'a Plan, child_ref: &str) -> Option<&'a PlanTask> {
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
mod tests;
