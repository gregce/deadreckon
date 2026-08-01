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

use std::path::PathBuf;

use deadreckon_core::campaign::Campaign;
use deadreckon_core::{Chain, DeadreckonPaths, JobView, PipelineState, Plan, PlanTask};

use super::super::*;

/// What a reference turned out to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefKind {
    Job,
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
            Self::Job => "job",
            Self::Run => "run",
            Self::PlanChild => "plan child",
            Self::Plan => "plan",
            Self::Chain => "chain",
            Self::Campaign => "campaign",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Job => 1 << 0,
            Self::Run => 1 << 1,
            Self::PlanChild => 1 << 2,
            Self::Plan => 1 << 3,
            Self::Chain => 1 << 4,
            Self::Campaign => 1 << 5,
        }
    }
}

/// The set of kinds a calling verb can handle. Five flags and one crate
/// boundary do not earn a `bitflags` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefKinds(u8);

impl RefKinds {
    pub(crate) const JOB: Self = Self(RefKind::Job.bit());
    pub(crate) const RUN: Self = Self(RefKind::Run.bit());
    pub(crate) const PLAN_CHILD: Self = Self(RefKind::PlanChild.bit());
    pub(crate) const PLAN: Self = Self(RefKind::Plan.bit());
    pub(crate) const CHAIN: Self = Self(RefKind::Chain.bit());
    pub(crate) const CAMPAIGN: Self = Self(RefKind::Campaign.bit());
    /// Every kind. `status`, `show` and `attach` orient across all of them.
    /// Built from the five constants rather than the bits directly, so adding a
    /// kind without adding it here is a compile-visible omission.
    pub(crate) const ALL: Self = Self::JOB
        .union(Self::RUN)
        .union(Self::PLAN_CHILD)
        .union(Self::PLAN)
        .union(Self::CHAIN)
        .union(Self::CAMPAIGN);

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
    Job(Box<JobView>),
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
            Self::Job(_) => RefKind::Job,
            Self::Run(_) => RefKind::Run,
            Self::PlanChild { .. } => RefKind::PlanChild,
            Self::Plan(_) => RefKind::Plan,
            Self::Chain(_) => RefKind::Chain,
            Self::Campaign { .. } => RefKind::Campaign,
        }
    }
}

/// Every kind, in the order refusals and ambiguity messages name them. Used by
/// the refusal-table tests to iterate the matrix exhaustively; the resolver
/// itself matches on `RefKind` and needs no list.
#[cfg(test)]
pub(crate) const ALL_REF_KINDS: [RefKind; 6] = [
    RefKind::Job,
    RefKind::Run,
    RefKind::PlanChild,
    RefKind::Plan,
    RefKind::Chain,
    RefKind::Campaign,
];

/// What one verb can be handed. This table is the acceptance matrix: a verb can
/// only disagree with another verb by differing here, which makes the
/// disagreement reviewable instead of buried in a hand-rolled cascade.
pub(crate) struct VerbRefSpec {
    pub(crate) verb: &'static str,
    pub(crate) accepts: RefKinds,
}

const RUN_LIKE: RefKinds = RefKinds::RUN.union(RefKinds::PLAN_CHILD);

pub(crate) const VERB_REF_SPECS: &[VerbRefSpec] = &[
    // Orientation and inspection see everything.
    VerbRefSpec {
        verb: "status",
        accepts: RefKinds::ALL,
    },
    VerbRefSpec {
        verb: "show",
        accepts: RefKinds::ALL,
    },
    VerbRefSpec {
        verb: "attach",
        accepts: RefKinds::ALL,
    },
    VerbRefSpec {
        verb: "kill",
        accepts: RefKinds::ALL,
    },
    // Finish promotes work; a campaign is inspected, not promoted directly.
    VerbRefSpec {
        verb: "finish",
        accepts: RefKinds::JOB
            .union(RUN_LIKE)
            .union(RefKinds::PLAN)
            .union(RefKinds::CHAIN),
    },
    // export/apply accept a plan because they map it onto its merged result run.
    VerbRefSpec {
        verb: "export",
        accepts: RefKinds::JOB.union(RUN_LIKE).union(RefKinds::PLAN),
    },
    VerbRefSpec {
        verb: "apply",
        accepts: RUN_LIKE.union(RefKinds::PLAN),
    },
    VerbRefSpec {
        verb: "abandon",
        accepts: RUN_LIKE,
    },
    VerbRefSpec {
        verb: "cleanup",
        accepts: RefKinds::JOB.union(RUN_LIKE),
    },
    // A verdict describes one gated run, so plans and chains redirect.
    VerbRefSpec {
        verb: "verdict",
        accepts: RUN_LIKE,
    },
    VerbRefSpec {
        verb: "report",
        accepts: RefKinds::JOB.union(RUN_LIKE),
    },
    VerbRefSpec {
        verb: "resume",
        accepts: RUN_LIKE,
    },
    VerbRefSpec {
        verb: "steer",
        accepts: RUN_LIKE,
    },
    // Undo reverses the last committed step of whatever it is handed. For a
    // run that is a snapshot restore; for a chain it is unwinding an applied
    // step; for a durable Job it is the one receipt-bound applied delivery.
    VerbRefSpec {
        verb: "undo",
        accepts: RefKinds::JOB.union(RUN_LIKE).union(RefKinds::CHAIN),
    },
    VerbRefSpec {
        verb: "rewind",
        accepts: RUN_LIKE,
    },
    VerbRefSpec {
        verb: "extend",
        accepts: RUN_LIKE,
    },
    VerbRefSpec {
        verb: "doc",
        accepts: RUN_LIKE.union(RefKinds::PLAN),
    },
    VerbRefSpec {
        verb: "merge",
        accepts: RefKinds::PLAN,
    },
];

/// The verb a refusal should name. `show` is the default because it accepts
/// every kind, so it is always a real answer rather than another refusal.
pub(crate) fn redirect_verb_for(kind: RefKind, verb: &str) -> &'static str {
    match (verb, kind) {
        // Steering targets one executing run; watching is the nearest thing a
        // plan or campaign can offer.
        ("steer", _) => "attach",
        // A pending plan is advanced by forking it, not by resuming a run.
        ("resume" | "extend", RefKind::Plan) => "fork",
        ("resume", RefKind::Chain) => "chain resume",
        ("extend", RefKind::Chain) => "chain",
        _ => "show",
    }
}

/// The one place a "wrong kind" refusal is written.
///
/// Every message names the reference, the kind it actually is, and one command
/// that accepts that kind. `deadreckon list` is deliberately absent: an id that
/// came from `list` must never be sent back to `list`.
pub(crate) fn refusal_for(kind: RefKind, verb: &str, reference: &str) -> CliError {
    let noun = kind.noun();
    let redirect = redirect_verb_for(kind, verb);
    let message = match (verb, kind) {
        ("verdict", RefKind::Plan) => {
            format!("{reference} is a plan; verdicts describe gated runs")
        }
        ("verdict", RefKind::Chain) => {
            format!("{reference} is a chain; verdicts describe gated runs")
        }
        ("steer", _) => {
            format!("{reference} is a {noun}; steering targets one executing run")
        }
        _ => format!("{reference} is a {noun}, not a run"),
    };
    CliError::Core(deadreckon_core::user_error(
        &message,
        &format!("deadreckon {redirect} {reference}"),
    ))
}

/// One resolution request.
///
/// There is deliberately no `accepts` field. A verb's accepted kinds come from
/// `VERB_REF_SPECS` via its name, so the acceptance matrix the tests iterate is
/// the same one the code obeys. Carrying `accepts` at the call site would be a
/// second source of truth that could drift from the table -- the same shape of
/// defect this slice exists to remove.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RefQuery<'a> {
    pub(crate) reference: Option<&'a str>,
    pub(crate) all_scopes: bool,
    pub(crate) verb: &'static str,
}

impl RefQuery<'_> {
    fn accepts(&self) -> RefKinds {
        accepts_for(self.verb)
    }
}

/// What this verb can be handed, from the one table. An unlisted verb accepts
/// everything, which is the safe direction: it can only make a refusal less
/// likely, never send an operator somewhere wrong. `every_verb_used_in_source_is_listed`
/// keeps the list honest.
pub(crate) fn accepts_for(verb: &str) -> RefKinds {
    VERB_REF_SPECS
        .iter()
        .find(|spec| spec.verb == verb)
        .map(|spec| spec.accepts)
        .unwrap_or(RefKinds::ALL)
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

    // Every kind is probed regardless of what the verb accepts. Probing only the
    // accepted kinds is what produced "not found: run 0c11f68e" for an id that
    // existed and was simply a plan -- a false statement, and the far end of the
    // status/list loop. Identify first, then decide whether this verb can take it.
    if let Some(selection) = resolve_plan_child_ref(paths, reference)? {
        if !query.accepts().contains(RefKind::PlanChild) {
            return Err(refusal_for(RefKind::PlanChild, query.verb, reference));
        }
        let state = load_run(paths, &selection.run_id)?;
        return Ok(ResolvedRef::PlanChild {
            selection,
            state: Box::new(state),
        });
    }

    let mut matches = Vec::new();
    let job = probe_job(paths, reference)?;
    let canonical_job_id = job
        .as_ref()
        .map(|view| view.job.job_id.as_ref().to_string());
    if let Some(job) = job {
        matches.push(ResolvedRef::Job(Box::new(job)));
    }
    if let Some(state) = probe_run(paths, reference)?
        && canonical_job_id.as_deref() != Some(state.run_id.as_str())
    {
        matches.push(ResolvedRef::Run(Box::new(state)));
    }
    if let Some(plan) = probe_plan(paths, reference)?
        && canonical_job_id.as_deref() != Some(plan.plan_id.as_str())
    {
        matches.push(ResolvedRef::Plan(Box::new(plan)));
    }
    if let Some(chain) = probe_chain(paths, reference, query.all_scopes)? {
        matches.push(ResolvedRef::Chain(Box::new(chain)));
    }
    if let Some((dir, campaign)) = super::campaign::resolve_campaign(paths, reference)?
        && canonical_job_id.as_deref() != Some(campaign.campaign_id.as_str())
    {
        matches.push(ResolvedRef::Campaign {
            dir,
            campaign: Box::new(campaign),
        });
    }

    match matches.len() {
        1 => {
            let resolved = matches.remove(0);
            if query.accepts().contains(resolved.kind()) {
                Ok(resolved)
            } else {
                Err(refusal_for(
                    resolved.kind(),
                    query.verb,
                    &resolved_id(&resolved),
                ))
            }
        }
        0 => Err(unresolved_reference(paths, reference, query)),
        // Ambiguity is judged across every kind, not just the accepted ones: the
        // operator typed a prefix that names two things, and narrowing by verb
        // would resolve it by guessing which one they meant.
        _ => Err(ambiguous_across_kinds(reference, &matches)),
    }
}

fn resolve_latest(paths: &DeadreckonPaths, query: RefQuery<'_>) -> Result<ResolvedRef> {
    let scope = if query.all_scopes {
        None
    } else {
        Some(current_scope()?)
    };
    resolve_latest_in_scope(paths, query.accepts(), scope.as_deref())
}

/// Resolve a reference for a verb that only operates on a single gated run.
///
/// Shakedown P7: these verbs used to call the run loader directly, so a plan or
/// chain id -- an id `list` had just printed -- came back as "not found", which
/// was false. The shared resolver identifies the kind and the refusal table
/// names a verb that accepts it.
pub(crate) fn resolve_run_like(
    paths: &DeadreckonPaths,
    reference: Option<&str>,
    verb: &'static str,
) -> Result<PipelineState> {
    let resolved = resolve_ref(
        paths,
        RefQuery {
            reference,
            all_scopes: false,
            verb,
        },
    )?;
    match resolved {
        ResolvedRef::Run(state) => Ok(*state),
        ResolvedRef::PlanChild { state, .. } => Ok(*state),
        other => Err(refusal_for(other.kind(), verb, &resolved_id(&other))),
    }
}

/// Resolve to a run when the reference names one, without refusing when it
/// names something else.
///
/// `finish`, `export`, `apply` and `doc` have a fallback richer than the
/// acceptance matrix: a plan reference maps onto that plan's merged result run,
/// or its doc target. That is a real feature, not a guessing cascade, so it must
/// still get its turn. `Err` is reserved for a reference that resolves to
/// nothing at all, or ambiguously.
pub(crate) fn try_resolve_run(
    paths: &DeadreckonPaths,
    reference: &str,
    verb: &'static str,
) -> Result<Option<PipelineState>> {
    match resolve_ref(
        paths,
        RefQuery {
            reference: Some(reference),
            all_scopes: false,
            verb,
        },
    )? {
        ResolvedRef::Run(state) => Ok(Some(*state)),
        ResolvedRef::PlanChild { state, .. } => Ok(Some(*state)),
        _ => Ok(None),
    }
}

/// Explain a reference this verb could not use, by naming what it actually is.
///
/// For verbs whose fallback is richer than the acceptance matrix -- `finish` and
/// `doc` map a plan reference onto that plan's merged result run, which is a
/// real feature, not a guessing cascade -- the fallback stays. Only its final
/// "not found" becomes a kind-aware refusal, so a chain id stops being reported
/// as a missing run.
pub(crate) fn refusal_for_reference(
    paths: &DeadreckonPaths,
    reference: &str,
    verb: &'static str,
) -> CliError {
    match resolve_ref(
        paths,
        RefQuery {
            reference: Some(reference),
            all_scopes: false,
            verb,
        },
    ) {
        Ok(resolved) => refusal_for(resolved.kind(), verb, &resolved_id(&resolved)),
        // Genuinely unresolvable, or ambiguous. The resolver's own message is
        // always at least as good as anything the caller could supply, so there
        // is no fallback to override it -- one fewer place that can invent a
        // worse refusal.
        Err(error) => error,
    }
}

/// One candidate for `latest`, reduced to the only two things the ranking needs.
struct LatestCandidate {
    kind: RefKind,
    id: String,
    updated_at: DateTime<Utc>,
}

/// The single meaning of `latest`: the most recently updated item, among the
/// kinds the calling verb accepts, attributable to `scope` (or to anywhere when
/// `scope` is `None`).
///
/// Scope is taken from the same fields `list` uses — `PipelineState::scope` for
/// runs, `Plan::parent_scope` for plans, `Chain::scope` for chains — so `latest`
/// and the top row of `list` cannot disagree. Campaigns carry no scope of their
/// own and are therefore candidates only under `--all`; they remain resolvable
/// by explicit id in every scope.
///
/// The ordering key is last-write time with a status-timestamp fallback, matching
/// `list_plan_entries` rather than introducing a second notion of "recent".
///
/// Taking `scope` as a parameter instead of reading the process cwd is what makes
/// the rule unit-testable; `resolve_latest` above is the one place that consults
/// `current_scope`.
pub(crate) fn resolve_latest_in_scope(
    paths: &DeadreckonPaths,
    accepts: RefKinds,
    scope: Option<&str>,
) -> Result<ResolvedRef> {
    let candidates = latest_candidates(paths, accepts, scope)?;
    let Some(newest) = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.updated_at)
    else {
        return Err(empty_latest(paths, accepts, scope));
    };
    match newest.kind {
        RefKind::Job => Ok(ResolvedRef::Job(Box::new(JobView::load(
            paths, &newest.id,
        )?))),
        RefKind::Run => Ok(ResolvedRef::Run(Box::new(load_run(paths, &newest.id)?))),
        RefKind::Plan => Ok(ResolvedRef::Plan(Box::new(load_plan(paths, &newest.id)?))),
        RefKind::Chain => Ok(ResolvedRef::Chain(Box::new(deadreckon_core::load_chain(
            paths, &newest.id,
        )?))),
        RefKind::Campaign => match super::campaign::resolve_campaign(paths, &newest.id)? {
            Some((dir, campaign)) => Ok(ResolvedRef::Campaign {
                dir,
                campaign: Box::new(campaign),
            }),
            None => Err(CliError::Core(deadreckon_core::user_error(
                &format!("campaign {} disappeared while resolving latest", newest.id),
                "deadreckon list",
            ))),
        },
        // Plan children are addressed by an explicit `<plan>:<task>` reference;
        // there is no "latest child" to rank across plans.
        RefKind::PlanChild => Err(CliError::Core(deadreckon_core::user_error(
            "latest does not name a plan child",
            "deadreckon show <plan-id>",
        ))),
    }
}

fn latest_candidates(
    paths: &DeadreckonPaths,
    accepts: RefKinds,
    scope: Option<&str>,
) -> Result<Vec<LatestCandidate>> {
    let mut candidates = Vec::new();
    let jobs = super::job::list_jobs(paths, scope)?;
    let job_ids = jobs
        .iter()
        .map(|view| view.job.job_id.as_ref().to_string())
        .collect::<BTreeSet<_>>();
    if accepts.contains(RefKind::Job) {
        for view in jobs {
            candidates.push(LatestCandidate {
                kind: RefKind::Job,
                id: view.job.job_id.as_ref().to_string(),
                updated_at: view.projection.updated_at.unwrap_or(view.job.created_at),
            });
        }
    }
    if accepts.contains(RefKind::Run) {
        for run in deadreckon_core::list_runs(paths, scope)? {
            if job_ids.contains(&run.run_id) {
                continue;
            }
            candidates.push(LatestCandidate {
                kind: RefKind::Run,
                id: run.run_id,
                updated_at: run.updated_at,
            });
        }
    }
    if accepts.contains(RefKind::Plan) {
        for plan in super::inspection::list_plan_entries(paths, scope)? {
            if job_ids.contains(&plan.plan_id) {
                continue;
            }
            candidates.push(LatestCandidate {
                kind: RefKind::Plan,
                id: plan.plan_id,
                updated_at: plan.updated_at,
            });
        }
    }
    if accepts.contains(RefKind::Chain) {
        for chain in super::chain::list_chain_records(paths, scope.map(str::to_string))? {
            let updated_at = file_mtime(&deadreckon_core::chain_json_path(paths, &chain.chain_id))
                .unwrap_or_else(|| {
                    chain
                        .completed_at
                        .or(chain.started_at)
                        .unwrap_or(chain.created_at)
                });
            candidates.push(LatestCandidate {
                kind: RefKind::Chain,
                id: chain.chain_id,
                updated_at,
            });
        }
    }
    // A campaign has no scope field, so it can only be ranked when the caller has
    // not asked for one. Narrowing to a scope it cannot claim would be a guess.
    if accepts.contains(RefKind::Campaign) && scope.is_none() {
        for (dir, campaign) in all_campaigns(paths)? {
            if job_ids.contains(&campaign.campaign_id) {
                continue;
            }
            let updated_at =
                file_mtime(&deadreckon_core::campaign::campaign_path_for_plan_dir(&dir))
                    .unwrap_or_else(|| {
                        campaign
                            .merged_at
                            .or(campaign.forked_at)
                            .unwrap_or(campaign.created_at)
                    });
            candidates.push(LatestCandidate {
                kind: RefKind::Campaign,
                id: campaign.campaign_id,
                updated_at,
            });
        }
    }
    Ok(candidates)
}

fn all_campaigns(paths: &DeadreckonPaths) -> Result<Vec<(PathBuf, Campaign)>> {
    let plans = paths.plans_dir();
    if !plans.is_dir() {
        return Ok(Vec::new());
    }
    let mut campaigns = Vec::new();
    for entry in fs::read_dir(&plans).map_err(|source| DeadreckonError::Io {
        path: plans.clone(),
        source,
    })? {
        let dir = entry
            .map_err(|source| DeadreckonError::Io {
                path: plans.clone(),
                source,
            })?
            .path();
        if !deadreckon_core::campaign::campaign_path_for_plan_dir(&dir).is_file() {
            continue;
        }
        let campaign = deadreckon_core::campaign::read_campaign(&dir)?;
        campaigns.push((dir, campaign));
    }
    Ok(campaigns)
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .ok()
}

fn probe_job(paths: &DeadreckonPaths, reference: &str) -> Result<Option<JobView>> {
    let mut ids = job_ids_matching(paths, reference)?;
    match ids.len() {
        1 => Ok(Some(JobView::load(paths, &ids.remove(0))?)),
        0 => Ok(None),
        _ => Err(ambiguous_within_kind(&format!(
            "ambiguous job id prefix {reference}; matches {}",
            ids.join(", ")
        ))),
    }
}

fn job_ids_matching(paths: &DeadreckonPaths, prefix: &str) -> Result<Vec<String>> {
    let jobs_dir = paths.jobs_dir();
    if !jobs_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut jobs = fs::read_dir(&jobs_dir)
        .map_err(|source| DeadreckonError::Io {
            path: jobs_dir.clone(),
            source,
        })?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join(deadreckon_core::job::JOB_JSON).is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|job_id| job_id.starts_with(prefix))
        .collect::<Vec<_>>();
    jobs.sort();
    Ok(jobs)
}

/// An empty scope and an empty machine are different problems. Sending someone
/// with no work anywhere to `list` shows them an empty table; sending someone
/// whose work lives in another project to `start` would have them redo it.
fn empty_latest(paths: &DeadreckonPaths, accepts: RefKinds, scope: Option<&str>) -> CliError {
    let elsewhere = scope.is_some()
        && latest_candidates(paths, accepts, None)
            .map(|candidates| !candidates.is_empty())
            .unwrap_or(false);
    if elsewhere {
        CliError::Core(deadreckon_core::user_error(
            "nothing in this project yet; other projects have work",
            "deadreckon list --all",
        ))
    } else {
        CliError::Core(deadreckon_core::user_error(
            "nothing in this project yet",
            "deadreckon start \"<goal>\"",
        ))
    }
}

/// A missing run is "no match"; an ambiguous prefix is the loader's own message,
/// passed through so the operator sees the candidate ids.
fn probe_run(paths: &DeadreckonPaths, reference: &str) -> Result<Option<PipelineState>> {
    match load_run(paths, reference) {
        Ok(state) => Ok(Some(state)),
        Err(DeadreckonError::NotFound(_)) => Ok(None),
        // The loader's ambiguity text names the candidate ids, which is the
        // useful part; `list` is a legal `try:` here because an ambiguous prefix
        // is a typo, not an id `list` handed the operator.
        Err(source) => Err(ambiguous_within_kind(&source.to_string())),
    }
}

/// Every ambiguity refusal carries the same way forward. Guidance like "use a
/// longer prefix" is advice, not a command, and this slice holds refusals to
/// naming something the operator can run.
fn ambiguous_within_kind(message: &str) -> CliError {
    CliError::Core(deadreckon_core::user_error(message, "deadreckon list"))
}

fn probe_plan(paths: &DeadreckonPaths, reference: &str) -> Result<Option<Plan>> {
    let mut ids = plan_ids_matching(paths, reference)?;
    match ids.len() {
        1 => Ok(Some(load_plan(paths, &ids.remove(0))?)),
        0 => Ok(None),
        _ => Err(ambiguous_within_kind(&format!(
            "ambiguous plan id prefix {reference}; matches {}",
            ids.join(", ")
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
        _ => Err(ambiguous_within_kind(&format!(
            "ambiguous chain id prefix {reference}; matches {}",
            matches
                .iter()
                .map(|chain| chain.chain_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Nothing matched. An empty home is a different problem from a typo, so it gets
/// a different `try:` — pointing a first-time operator at `list` shows them an
/// empty table and no way forward.
fn unresolved_reference(paths: &DeadreckonPaths, reference: &str, query: RefQuery<'_>) -> CliError {
    let accepts = query.accepts();
    let has_any_state = deadreckon_core::list_runs(paths, None)
        .map(|runs| !runs.is_empty())
        .unwrap_or(false)
        || job_ids_matching(paths, "")
            .map(|jobs| !jobs.is_empty())
            .unwrap_or(false)
        || plan_ids_matching(paths, "")
            .map(|plans| !plans.is_empty())
            .unwrap_or(false);
    if has_any_state {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "no {} matches {reference}",
                accepted_nouns(accepts).join(", ")
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
        ResolvedRef::Job(view) => view.job.job_id.as_ref().to_string(),
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
        RefKind::Job,
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
