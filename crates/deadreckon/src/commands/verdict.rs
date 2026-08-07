//! `deadreckon verdict` — a non-authoritative "did it actually work?" report.
//!
//! Verdict re-verifies a run NOW through the gate's contained, keyless
//! evaluator in a disposable workspace, reads (never overwrites) the original
//! signed marker, and renders one of three honest states with evidence and a
//! single next action. It NEVER mutates run state, advances a phase, or
//! promotes — the result is a sidecar audit file only.

use super::super::*;
use deadreckon_core::gate::AcceptanceCheckResult;
use serde::Serialize;

/// Parsed `deadreckon verdict` arguments.
pub(crate) struct VerdictArgs {
    pub(crate) run_id: Option<String>,
    pub(crate) all: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) receipt: bool,
    pub(crate) rerun_checks: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
    pub(crate) quiet: bool,
}

/// Re-verify a run (or compare several with `--all`) and report the verdict.
///
/// P1 wires the verb and resolves the run; live re-evaluation, marker reading,
/// `VerdictSurface` rendering, `--json`, `--all`, and the sidecar cache land in
/// P2–P10. For now it prints a placeholder so the command is reachable.
///
/// A durable Job reference maps onto the job's current attempt run. Its
/// DEFAULT behavior is the read-only receipt audit — never a check re-run —
/// because re-executing checks against a job-owned run root is exactly what
/// the driver fence exists to prevent. `--rerun-checks` opts into the normal
/// re-run path, which still passes through the fence and therefore refuses
/// publicly while the job's driver owns the run.
pub(crate) async fn verdict_command(args: VerdictArgs) -> Result<()> {
    let _ = args.plain; // applied via ui::set_plain_output at the dispatch site
    let paths = DeadreckonPaths::discover();
    if args.all {
        return verdict_all_command(&paths, args.limit.unwrap_or(10), args.json).await;
    }
    let resolved = crate::commands::reference::resolve_ref(
        &paths,
        crate::commands::reference::RefQuery {
            reference: args.run_id.as_deref(),
            all_scopes: false,
            verb: "verdict",
        },
    )?;
    let state = match resolved {
        crate::commands::reference::ResolvedRef::Job(view) => {
            if !args.rerun_checks {
                return verdict_job_receipt_audit(&paths, &view, &args);
            }
            let state = current_attempt_state(&paths, &view)?;
            // A Single-shape job's attempt run IS the job (run_id == job_id):
            // it carries no stamped ownership and no plan/campaign reference,
            // so the run-level fence below cannot see its owner. The
            // artifact-level fence (the same one steer uses) closes that gap
            // with the identical typed "belongs to durable Job" refusal.
            if paths.job_json(&state.run_id).is_file() {
                let owner = deadreckon_core::load_job(&paths, &state.run_id)?;
                super::graph_job::require_current_driver_for_job_artifact(
                    &paths,
                    &state.run_id,
                    owner.shape,
                    "verdict",
                )?;
            }
            state
        }
        crate::commands::reference::ResolvedRef::Run(state) => *state,
        crate::commands::reference::ResolvedRef::PlanChild { state, .. } => *state,
        // The resolver already refuses kinds outside verdict's acceptance
        // matrix; this arm only restates that refusal for exhaustiveness.
        other => {
            return Err(crate::commands::reference::refusal_for(
                other.kind(),
                "verdict",
                &crate::commands::reference::resolved_id(&other),
            ));
        }
    };
    super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "verdict")?;
    let report = evaluate_verdict_report(&state).await?;
    // The sidecar is an additive audit artifact; a read-only filesystem must not
    // turn a verdict into a hard failure, so a write error is swallowed.
    let cached = cache_verdict_report(&state, &report).ok();
    let surface = render_verdict_surface(&report, &state);
    if args.json {
        // The per-digest receipt audit is inspection only: it performs the
        // same reads as the strict validator (promotion still runs the strict
        // fail-closed path) and never mutates run state.
        let receipt_audit = args
            .receipt
            .then(|| deadreckon_core::audit_completion_receipt(&paths, &state));
        let envelope = verdict_json(
            &report,
            &state,
            &surface.primary_action.command,
            cached.as_deref(),
            receipt_audit.as_ref(),
        );
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        println!("{}", surface.render_plain(args.quiet));
    }
    Ok(())
}

/// The job's current attempt run: `projection.json` `child_run_ids`, newest
/// last — the same rule the fleet rollup and the mac app's Chartroom use. A
/// job with no linked attempt falls back to its own run id, which is how a
/// Single-shape job names its one attempt (supervisor.rs stamps run_id =
/// job_id). `show`'s diff surfaces (G10) share this resolution so a Job ref
/// and a run ref can never disagree about which attempt is "current".
pub(crate) fn current_attempt_state(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
) -> Result<deadreckon_core::PipelineState> {
    try_current_attempt_state(paths, view)?.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!(
                "job {} has no attempt run to verify yet",
                run_prefix(view.job.job_id.as_ref())
            ),
            &format!("deadreckon status {}", run_prefix(view.job.job_id.as_ref())),
        ))
    })
}

/// The resolution rule of [`current_attempt_state`] with the no-attempt case
/// surfaced as `Ok(None)` instead of a verdict-worded refusal, so other verbs
/// (follow) can attach their own wording to the same rule instead of
/// borrowing verdict's.
pub(crate) fn try_current_attempt_state(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
) -> Result<Option<deadreckon_core::PipelineState>> {
    let run_id = view
        .projection
        .child_run_ids
        .last()
        .map(String::as_str)
        .unwrap_or(view.job.job_id.as_ref());
    match deadreckon_core::load_run(paths, run_id) {
        Ok(state) => Ok(Some(state)),
        Err(deadreckon_core::DeadreckonError::NotFound(_)) => Ok(None),
        Err(error) => Err(CliError::Core(error)),
    }
}

/// Read-only inspection of a durable Job: audit the recorded proofs of its
/// current attempt run without re-running checks.
///
/// DRIVER-FENCE CARVE-OUT (G7 follow-up). This path deliberately does NOT call
/// `require_current_driver_for_job_owned_run`, and that is sound only because
/// it performs reads and nothing else:
///   - the signed acceptance marker is read and its signature validated
///     (`legacy_signature_fact` — a pure read, never a rewrite);
///   - the completion receipt is audited fact by fact
///     (`audit_completion_receipt` — reads exactly what the strict validator
///     reads; promotion still runs the strict fail-closed path);
///   - the verdict sidecar is SKIPPED, not written best-effort: the run root
///     belongs to the job's driver, so inspection must leave it byte-identical
///     (`paths.cache` reads `null` in the envelope to say so).
/// Every path that re-runs checks or writes run-root artifacts — including
/// `verdict <job> --rerun-checks`, which maps onto this same attempt run —
/// still goes through the fence and keeps its typed public refusal.
fn verdict_job_receipt_audit(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    args: &VerdictArgs,
) -> Result<()> {
    let state = current_attempt_state(paths, view)?;
    let (had_signed_marker, marker_valid) = legacy_signature_fact(&state);
    let audit = deadreckon_core::audit_completion_receipt(paths, &state);
    // The same decision rule as `compute_verdict`, with "every receipt audit
    // fact passes NOW" standing in for the check re-run: this path re-validates
    // recorded proofs (signatures, digests), it does not re-execute checks.
    let status = compute_verdict(had_signed_marker, marker_valid, audit.passed());
    let surface = render_job_audit_surface(view, status, &audit);
    if args.json {
        let envelope = verdict_job_audit_json(
            view,
            &state,
            status,
            had_signed_marker,
            marker_valid,
            &audit,
            &surface.primary_action.command,
        );
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        println!("{}", surface.render_plain(args.quiet));
    }
    Ok(())
}

/// The machine envelope for a Job-reference verdict: the run envelope's shape
/// plus `job_id`, `mode:"receipt_audit"`, and `checks_rerun:false`. `checks`
/// is always empty (nothing was re-executed) and `paths.cache` is always
/// `null` (no sidecar is written into the driver-owned run root).
fn verdict_job_audit_json(
    view: &deadreckon_core::JobView,
    state: &deadreckon_core::PipelineState,
    status: VerdictState,
    had_signed_marker: bool,
    marker_valid: bool,
    audit: &deadreckon_core::ReceiptAudit,
    primary_command: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "verdict",
        "id": state.run_id,
        "job_id": view.job.job_id.as_ref(),
        "mode": "receipt_audit",
        "checks_rerun": false,
        "status": status,
        "checks": Vec::<serde_json::Value>::new(),
        "had_signed_marker": had_signed_marker,
        "marker_valid": marker_valid,
        "receipt_audit": { "facts": &audit.facts },
        "next_actions": [primary_command],
        "try_lines": Vec::<String>::new(),
        "paths": {
            "run_root": state.run_root,
            "marker": deadreckon_core::gate::marker_path_for_run_root(&state.run_root),
            "cache": serde_json::Value::Null,
        },
    })
}

/// Render the Job-reference audit as a `VerdictSurface`: one label over the
/// receipt facts, and one next action. The wording says "recorded proofs",
/// never "re-running checks" — nothing was re-executed on this path.
fn render_job_audit_surface(
    view: &deadreckon_core::JobView,
    status: VerdictState,
    audit: &deadreckon_core::ReceiptAudit,
) -> VerdictSurface {
    let passed = audit.facts.iter().filter(|fact| fact.pass).count();
    let total = audit.facts.len();
    let kind = match status {
        VerdictState::Verified => VerdictKind::Verified,
        VerdictState::Regressed => VerdictKind::Failed,
        VerdictState::Unverified => VerdictKind::Noop,
    };
    let what_happened = match status {
        VerdictState::Verified => {
            "This job's recorded proofs — signed marker and completion receipt — validate now."
        }
        VerdictState::Regressed => {
            "This job's recorded proofs no longer validate — a signature or digest broke."
        }
        VerdictState::Unverified => {
            "This job's current attempt has no signed marker; its proofs were audited read-only."
        }
    };
    let why_this_verdict = match status {
        VerdictState::Verified => "Valid signed marker, and every receipt audit fact passes now.",
        VerdictState::Regressed => "The signed marker or a receipt audit fact fails validation.",
        VerdictState::Unverified => {
            "Checks were NOT re-run: a job's run root belongs to its driver."
        }
    };
    let mut evidence = vec![(
        "receipt facts".to_string(),
        format!("{passed}/{total} passed"),
    )];
    for fact in audit.facts.iter().take(6) {
        let mark = if fact.pass { "pass" } else { "FAIL" };
        evidence.push((format!("fact · {mark}"), fact.name.clone()));
    }

    let short = run_prefix(view.job.job_id.as_ref());
    let command = match status {
        VerdictState::Verified => format!("deadreckon finish {short}"),
        _ => format!("deadreckon status {short}"),
    };
    let explanation = ExplanationPanel::new(what_happened, why_this_verdict, evidence);
    VerdictSurface::must_new(
        kind,
        "job",
        Some(view.job.job_id.as_ref()),
        explanation,
        [("Recommended", command)],
        [
            ("Inspect", format!("deadreckon show {short}")),
            ("Report", format!("deadreckon report {short}")),
        ],
    )
}

/// A one-row verdict summary for the `--all` comparison.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerdictSummary {
    pub(crate) run_id: String,
    pub(crate) goal: String,
    pub(crate) state: VerdictState,
    pub(crate) checks_passed: usize,
    pub(crate) checks_total: usize,
    pub(crate) spend_usd: f64,
}

/// Re-verify the most recent `limit` runs (across scopes) into compact summaries
/// so "several at once" collapses to one screen. Newest first.
async fn evaluate_verdict_all_summaries(
    paths: &DeadreckonPaths,
    limit: usize,
) -> Result<Vec<VerdictSummary>> {
    let mut runs = deadreckon_core::list_runs(paths, None)?;
    runs.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    runs.truncate(limit);
    let mut summaries = Vec::new();
    for entry in runs {
        let state = deadreckon_core::load_run(paths, &entry.run_id)?;
        super::graph_job::require_current_driver_for_job_owned_run(paths, &state, "verdict")?;
        let report = evaluate_verdict_report(&state).await?;
        let checks_passed = report.checks.iter().filter(|check| check.passed).count();
        summaries.push(VerdictSummary {
            run_id: entry.run_id,
            goal: entry.goal,
            state: report.state,
            checks_passed,
            checks_total: report.checks.len(),
            spend_usd: state.total_spend_usd,
        });
    }
    Ok(summaries)
}

#[cfg(test)]
pub(crate) fn verdict_all_summaries(
    paths: &DeadreckonPaths,
    limit: usize,
) -> Result<Vec<VerdictSummary>> {
    let mut runs = deadreckon_core::list_runs(paths, None)?;
    runs.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    runs.truncate(limit);
    let mut summaries = Vec::new();
    for entry in runs {
        let state = deadreckon_core::load_run(paths, &entry.run_id)?;
        let report = build_verdict_report(&state);
        let checks_passed = report.checks.iter().filter(|check| check.passed).count();
        summaries.push(VerdictSummary {
            run_id: entry.run_id,
            goal: entry.goal,
            state: report.state,
            checks_passed,
            checks_total: report.checks.len(),
            spend_usd: state.total_spend_usd,
        });
    }
    Ok(summaries)
}

async fn verdict_all_command(paths: &DeadreckonPaths, limit: usize, json: bool) -> Result<()> {
    let summaries = evaluate_verdict_all_summaries(paths, limit).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }
    if summaries.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no runs to verify",
            "deadreckon start \"<goal>\"",
        )));
    }
    println!(
        "{:<10} {:<11} {:>7} {:>9}  goal",
        "run", "verdict", "checks", "spend"
    );
    for summary in &summaries {
        let state = match summary.state {
            VerdictState::Verified => "VERIFIED",
            VerdictState::Regressed => "REGRESSED",
            VerdictState::Unverified => "UNVERIFIED",
        };
        println!(
            "{:<10} {:<11} {:>3}/{:<3} {:>8.2}  {}",
            run_prefix(&summary.run_id),
            state,
            summary.checks_passed,
            summary.checks_total,
            summary.spend_usd,
            summary.goal.chars().take(48).collect::<String>(),
        );
    }
    Ok(())
}

/// The machine envelope for `verdict --json`, matching the inspection shape
/// (`kind`/`id`/`status`/`next_actions`/`paths` + the verdict payload).
pub(crate) fn verdict_json(
    report: &VerdictReport,
    state: &deadreckon_core::PipelineState,
    primary_command: &str,
    cached: Option<&std::path::Path>,
    receipt_audit: Option<&deadreckon_core::ReceiptAudit>,
) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "kind": "verdict",
        "id": report.run_id,
        "status": report.state,
        "checks": report.checks,
        "changed_files": report.changed_files,
        "source": report.source,
        "had_signed_marker": report.had_signed_marker,
        "marker_valid": report.marker_valid,
        "next_actions": [primary_command],
        "try_lines": Vec::<String>::new(),
        "paths": {
            "run_root": state.run_root,
            "marker": deadreckon_core::gate::marker_path_for_run_root(&state.run_root),
            "cache": cached,
        },
    });
    if let Some(audit) = receipt_audit {
        envelope["receipt_audit"] = serde_json::json!({ "facts": &audit.facts });
    }
    envelope
}

/// Render the verdict as a `VerdictSurface` — one label, an Explanation/Evidence
/// panel (per-check pass/fail, changed-file summary, provenance line), and the
/// single mapped next action. Verified→pass kind, Regressed→fail, Unverified→noop.
pub(crate) fn render_verdict_surface(
    report: &VerdictReport,
    state: &deadreckon_core::PipelineState,
) -> VerdictSurface {
    let passed = report.checks.iter().filter(|check| check.passed).count();
    let total = report.checks.len();
    let all_pass = report
        .checks
        .iter()
        .all(|check| !check.must_pass || check.passed);
    let working_dir_gone = report.checks.is_empty() && !state.working_dir.is_dir();

    let kind = match report.state {
        VerdictState::Verified => VerdictKind::Verified,
        VerdictState::Regressed => VerdictKind::Failed,
        VerdictState::Unverified => VerdictKind::Noop,
    };
    let what_happened = match report.state {
        VerdictState::Verified => "Re-running this run's acceptance checks still passes.",
        VerdictState::Regressed => {
            "This run's acceptance checks no longer pass — the work silently broke."
        }
        VerdictState::Unverified => {
            "This run has no signed deadreckon marker; its checks were re-run fresh."
        }
    };
    let why_this_verdict = match report.state {
        VerdictState::Verified => "Valid signed marker, and every must-pass check passes now.",
        VerdictState::Regressed if report.had_signed_marker && !report.marker_valid => {
            "The signed marker no longer validates (forged or tampered)."
        }
        VerdictState::Regressed => "A must-pass check fails when re-run now.",
        VerdictState::Unverified if working_dir_gone => {
            "Working dir unavailable; nothing could be re-verified."
        }
        VerdictState::Unverified if all_pass => {
            "Checks pass now — verified now, not gated at build time."
        }
        VerdictState::Unverified => "Checks fail when re-run now.",
    };

    let provenance = if report.had_signed_marker {
        format!(
            "deadreckon-gated ({})",
            if report.marker_valid {
                "valid"
            } else {
                "invalid"
            }
        )
    } else {
        "not natively gated (verified now)".to_string()
    };
    let mut evidence = vec![
        (
            "checks".to_string(),
            if working_dir_gone {
                "working dir unavailable".to_string()
            } else {
                format!("{passed}/{total} passed")
            },
        ),
        (
            "changed files".to_string(),
            format!(
                "+{} ~{} -{}",
                report.changed_files.added,
                report.changed_files.modified,
                report.changed_files.deleted
            ),
        ),
        ("provenance".to_string(), provenance),
    ];
    for check in report.checks.iter().take(6) {
        let mark = if check.passed { "pass" } else { "FAIL" };
        let label = check.command.clone().unwrap_or_else(|| check.kind.clone());
        evidence.push((format!("check · {mark}"), label));
    }

    let short = run_prefix(&report.run_id);
    let command = match report.state {
        VerdictState::Verified => format!("deadreckon finish {short}"),
        VerdictState::Regressed => format!("deadreckon resume {short}"),
        VerdictState::Unverified if all_pass && !working_dir_gone => {
            format!("deadreckon finish {short}")
        }
        VerdictState::Unverified => format!("deadreckon resume {short}"),
    };
    let explanation = ExplanationPanel::new(what_happened, why_this_verdict, evidence);
    VerdictSurface::must_new(
        kind,
        "run",
        Some(&report.run_id),
        explanation,
        [("Recommended", command)],
        [
            ("Inspect", format!("deadreckon show {short}")),
            ("Compare", "deadreckon verdict --all".to_string()),
        ],
    )
}

async fn evaluate_verdict_report(state: &deadreckon_core::PipelineState) -> Result<VerdictReport> {
    let rerun = rerun_acceptance_contained(state).await?;
    Ok(assemble_verdict_report(state, rerun))
}

/// Build the full verdict from independently obtained acceptance evidence,
/// read (never overwrite) the signed marker, and combine through
/// `compute_verdict`.
fn assemble_verdict_report(
    state: &deadreckon_core::PipelineState,
    rerun: RerunResult,
) -> VerdictReport {
    if let Ok(view) = deadreckon_core::RunView::from_state(state) {
        return build_verdict_report_from_view(state, &view, rerun);
    }
    let (had_signed_marker, marker_valid) = legacy_signature_fact(state);
    VerdictReport {
        schema: 1,
        run_id: state.run_id.clone(),
        taken_at: chrono::Utc::now().to_rfc3339(),
        state: compute_verdict(had_signed_marker, marker_valid, rerun.all_must_pass),
        had_signed_marker,
        marker_valid,
        checks: rerun.checks,
        changed_files: changed_file_counts(state),
        source: if state.run_root.join("import.json").exists() {
            VerdictSource::Imported
        } else {
            VerdictSource::Native
        },
    }
}

#[cfg(test)]
pub(crate) fn build_verdict_report(state: &deadreckon_core::PipelineState) -> VerdictReport {
    assemble_verdict_report(state, rerun_acceptance(state))
}

fn build_verdict_report_from_view(
    state: &deadreckon_core::PipelineState,
    view: &deadreckon_core::RunView,
    rerun: RerunResult,
) -> VerdictReport {
    let had_signed_marker = view.signature.marker_path.is_some();
    let marker_valid = matches!(
        view.signature.status,
        deadreckon_core::SignatureStatus::Valid
    );
    VerdictReport {
        schema: 1,
        run_id: state.run_id.clone(),
        taken_at: chrono::Utc::now().to_rfc3339(),
        state: compute_verdict(had_signed_marker, marker_valid, rerun.all_must_pass),
        had_signed_marker,
        marker_valid,
        checks: rerun.checks,
        changed_files: changed_file_counts_from_view(view),
        source: if state.run_root.join("import.json").exists() {
            VerdictSource::Imported
        } else {
            VerdictSource::Native
        },
    }
}

fn legacy_signature_fact(state: &deadreckon_core::PipelineState) -> (bool, bool) {
    let marker_path = deadreckon_core::gate::marker_path_for_run_root(&state.run_root);
    let had_signed_marker = marker_path.exists();
    // A marker that no longer validates (signature mismatch, tampered tamper
    // bytes, wrong run_id) is treated as not-valid → Regressed, never Verified.
    let marker_valid =
        had_signed_marker && deadreckon_core::gate::validate_acceptance_marker(state).is_ok();
    (had_signed_marker, marker_valid)
}

fn changed_file_counts_from_view(view: &deadreckon_core::RunView) -> ChangedFiles {
    let mut counts = ChangedFiles::default();
    for file in &view.changed.files {
        match file.status {
            deadreckon_core::FileDeltaStatus::Added => counts.added += 1,
            deadreckon_core::FileDeltaStatus::Modified => counts.modified += 1,
            deadreckon_core::FileDeltaStatus::Removed => counts.deleted += 1,
        }
    }
    counts
}

/// Cache the verdict report as a sidecar at `<run_root>/proofs/verdict-<ts>.json`.
///
/// This is the ONLY write `verdict` performs, and it is an additive audit
/// artifact — never the PipelineState, never the marker, never a progress row.
/// The cache is never read back as authority: each invocation re-verifies live,
/// so a stale sidecar can never mask a regression. Returns the written path.
pub(crate) fn cache_verdict_report(
    state: &deadreckon_core::PipelineState,
    report: &VerdictReport,
) -> std::io::Result<std::path::PathBuf> {
    let proofs = state.run_root.join("proofs");
    std::fs::create_dir_all(&proofs)?;
    let stamp = report
        .taken_at
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let path = proofs.join(format!("verdict-{stamp}.json"));
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(report)?),
    )?;
    Ok(path)
}

/// Added/modified/deleted counts since the run's earliest snapshot, via the same
/// `tamper::touched_files` diff the gate uses. Empty when there is no snapshot.
fn changed_file_counts(state: &deadreckon_core::PipelineState) -> ChangedFiles {
    let touched = deadreckon_core::tamper::touched_files(&state.run_root, &state.working_dir)
        .unwrap_or_default();
    let mut counts = ChangedFiles::default();
    for change in touched.values() {
        match change {
            deadreckon_core::tamper::TouchedChange::Created => counts.added += 1,
            deadreckon_core::tamper::TouchedChange::Modified => counts.modified += 1,
            deadreckon_core::tamper::TouchedChange::Deleted => counts.deleted += 1,
        }
    }
    counts
}

/// The result of re-running a run's acceptance checks NOW.
pub(crate) struct RerunResult {
    pub(crate) checks: Vec<AcceptanceCheckResult>,
    pub(crate) all_must_pass: bool,
}

/// Re-run the approved checks through keyless `dr-gate evaluate` in a
/// disposable, contained workspace. A missing working dir (exported/cleaned)
/// yields no checks; a missing sandbox fails closed before any check executes.
async fn rerun_acceptance_contained(state: &deadreckon_core::PipelineState) -> Result<RerunResult> {
    if !state.working_dir.is_dir() {
        return Ok(RerunResult {
            checks: Vec::new(),
            all_must_pass: false,
        });
    }
    let evaluation = deadreckon_runtime::run_contained_verdict_evaluation(state, None).await?;
    let checks = evaluation.results;
    let all_must_pass = checks.iter().all(|check| !check.must_pass || check.passed);
    Ok(RerunResult {
        checks,
        all_must_pass,
    })
}

/// Unit-test fixture evaluator. Production verdicts never call this host path.
#[cfg(test)]
pub(crate) fn rerun_acceptance(state: &deadreckon_core::PipelineState) -> RerunResult {
    if !state.working_dir.is_dir() {
        return RerunResult {
            checks: Vec::new(),
            all_must_pass: false,
        };
    }
    let checks =
        deadreckon_core::gate::evaluate_acceptance_checks(&state.run_root, &state.working_dir)
            .unwrap_or_default();
    let all_must_pass = checks.iter().all(|check| !check.must_pass || check.passed);
    RerunResult {
        checks,
        all_must_pass,
    }
}

/// The three honest post-run states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictState {
    /// Valid signed marker AND re-running its checks now still passes.
    Verified,
    /// A marker existed (valid or stale) but re-running its checks now fails a
    /// must-pass check, or the marker no longer validates — the work silently
    /// broke. The load-bearing new signal.
    Regressed,
    /// No signed marker (imported / paused / failed run): verdict ran the
    /// declared checks fresh — verified now, not at build time.
    Unverified,
}

/// Whether the run was gated natively by deadreckon or imported from another tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictSource {
    Native,
    /// A run produced by `deadreckon import` (no native marker) — its `import.json`
    /// is present. Such a run is always `Unverified` and labeled not-natively-gated.
    Imported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub(crate) struct ChangedFiles {
    pub(crate) added: usize,
    pub(crate) modified: usize,
    pub(crate) deleted: usize,
}

/// The cached audit record written to `<run_root>/proofs/verdict-<ts>.json`. It
/// is never read back as authority — each `verdict` invocation re-verifies live.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerdictReport {
    pub(crate) schema: u32,
    pub(crate) run_id: String,
    pub(crate) taken_at: String,
    pub(crate) state: VerdictState,
    pub(crate) had_signed_marker: bool,
    pub(crate) marker_valid: bool,
    pub(crate) checks: Vec<AcceptanceCheckResult>,
    pub(crate) changed_files: ChangedFiles,
    pub(crate) source: VerdictSource,
}

/// The verdict decision — the single source of truth for what state a run is in.
///
/// - A run with a valid marker whose checks still all-must-pass is `Verified`.
/// - A run that HAD a marker but whose marker no longer validates, or whose
///   checks now fail a must-pass, is `Regressed` (never silently `Verified`).
/// - A run with no signed marker is `Unverified` — the fresh check results are
///   carried in the report, but the verdict never claims native gating.
pub(crate) fn compute_verdict(
    had_marker: bool,
    marker_valid: bool,
    rerun_all_must_pass: bool,
) -> VerdictState {
    if !had_marker {
        return VerdictState::Unverified;
    }
    if marker_valid && rerun_all_must_pass {
        VerdictState::Verified
    } else {
        VerdictState::Regressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_verdict_marker_valid_and_rerun_passes_is_verified() {
        assert_eq!(compute_verdict(true, true, true), VerdictState::Verified);
    }

    #[test]
    fn compute_verdict_marker_present_rerun_fails_is_regressed() {
        // A signed marker whose checks now fail is a silent regression.
        assert_eq!(compute_verdict(true, true, false), VerdictState::Regressed);
        // A marker that no longer validates is also Regressed, never Verified.
        assert_eq!(compute_verdict(true, false, true), VerdictState::Regressed);
    }

    #[test]
    fn compute_verdict_no_marker_is_unverified() {
        // No marker → Unverified regardless of the fresh rerun outcome.
        assert_eq!(
            compute_verdict(false, false, true),
            VerdictState::Unverified
        );
        assert_eq!(
            compute_verdict(false, false, false),
            VerdictState::Unverified
        );
    }

    // ---- V-P2: run resolution ----

    use deadreckon_core::paths::DeadreckonPaths;
    use deadreckon_core::state::{RunOptions, create_run, save_state};
    use tempfile::TempDir;

    fn fixture_run(paths: &DeadreckonPaths, run_id: &str, repo: &std::path::Path) {
        create_run(
            paths,
            RunOptions {
                goal: format!("goal {run_id}"),
                cwd: repo.to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some(run_id.to_string()),
                codebase: None,
            },
        )
        .expect("create_run");
    }

    #[test]
    fn verdict_resolves_latest_when_no_id() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "only-run-0001", &repo);

        // Shakedown P2: `latest` is scope-bound for every verb, so this asserts
        // the shared rule against the fixture's own scope. It used to assert
        // verdict's private all-scopes search, which is the divergence the
        // slice removes -- `status` and `verdict` could land on different runs
        // from the same directory.
        let scope = deadreckon_core::paths::workspace_scope(&repo).expect("scope");
        let resolved = crate::commands::reference::resolve_latest_in_scope(
            &paths,
            crate::commands::reference::RefKinds::RUN,
            Some(&scope),
        )
        .expect("latest");

        match resolved {
            crate::commands::reference::ResolvedRef::Run(state) => {
                assert_eq!(state.run_id, "only-run-0001");
            }
            other => panic!("expected a run, got {}", other.kind().noun()),
        }
    }

    #[test]
    fn verdict_latest_outside_the_scope_refuses_with_a_command_that_works() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "only-run-0001", &repo);

        // A scope with no work of its own, while another scope has some. The
        // refusal must name a command that actually leads somewhere -- this is
        // the half of the status/list loop that pointed back at an empty table.
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        let scope = deadreckon_core::paths::workspace_scope(&elsewhere).expect("scope");

        let err = crate::commands::reference::resolve_latest_in_scope(
            &paths,
            crate::commands::reference::RefKinds::RUN,
            Some(&scope),
        )
        .expect_err("no run in this scope");
        let message = err.to_string();

        assert!(message.contains("nothing in this project yet"), "{message}");
        assert!(message.contains("deadreckon list --all"), "{message}");
    }

    #[test]
    fn verdict_unknown_id_refuses_with_try_list() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "real-run-0001", &repo);

        let err = crate::commands::reference::resolve_run_like(&paths, Some("nope"), "verdict")
            .expect_err("refuse");
        assert!(err.to_string().contains("deadreckon list"), "{err}");
    }

    #[test]
    fn verdict_ambiguous_prefix_refuses() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "ambig-run-0001", &repo);
        fixture_run(&paths, "ambig-run-0002", &repo);

        let err = crate::commands::reference::resolve_run_like(&paths, Some("ambig"), "verdict")
            .expect_err("refuse");
        let message = err.to_string();
        assert!(message.contains("ambiguous"), "{message}");
        assert!(message.contains("deadreckon list"), "{message}");
    }

    // ---- V-P3: live re-evaluation ----

    #[test]
    fn verdict_reruns_compiled_checks_without_mutating_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "rerun-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "rerun-run-0001").expect("load");
        let status_before = state.status;

        let rerun = rerun_acceptance(&state);

        assert!(
            !rerun.checks.is_empty(),
            "re-running yields per-check results"
        );

        // The run state on disk is unchanged — verdict is read-only.
        let reloaded = deadreckon_core::load_run(&paths, "rerun-run-0001").expect("reload");
        assert_eq!(reloaded.status, status_before);
    }

    #[test]
    fn verdict_missing_working_dir_yields_unverified_detail() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "gone-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "gone-run-0001").expect("load");
        std::fs::remove_dir_all(&state.working_dir).expect("remove working dir");

        let rerun = rerun_acceptance(&state);

        assert!(rerun.checks.is_empty());
        // No marker + nothing re-verifiable → Unverified.
        assert_eq!(
            compute_verdict(false, false, rerun.all_must_pass),
            VerdictState::Unverified
        );
    }

    // ---- V-P4: marker read + verdict computation ----

    fn sign_marker(state: &deadreckon_core::PipelineState) {
        // Real signed marker over genuine (passing) results — the gate path.
        deadreckon_core::gate::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("write signed marker");
    }

    #[test]
    fn valid_marker_passing_rerun_is_verified() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "verified-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "verified-run-0001").expect("load");
        sign_marker(&state);

        let report = build_verdict_report(&state);
        assert!(report.had_signed_marker && report.marker_valid);
        assert_eq!(report.state, VerdictState::Verified);
    }

    #[test]
    fn tampered_marker_is_regressed_not_verified() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "tampered-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "tampered-run-0001").expect("load");
        sign_marker(&state);

        // Forge the signature: the marker file still exists but no longer validates.
        let marker_path = deadreckon_core::gate::marker_path_for_run_root(&state.run_root);
        let mut marker: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker_path).expect("read marker"))
                .expect("parse");
        marker["signature"] = serde_json::Value::String("forged".to_string());
        std::fs::write(&marker_path, marker.to_string()).expect("write tampered");

        let report = build_verdict_report(&state);
        assert!(report.had_signed_marker && !report.marker_valid);
        assert_eq!(report.state, VerdictState::Regressed);
    }

    #[test]
    fn imported_run_without_marker_is_unverified() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "imported-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "imported-run-0001").expect("load");
        // No marker written (as if imported from another tool).

        let report = build_verdict_report(&state);
        assert!(!report.had_signed_marker);
        assert_eq!(report.state, VerdictState::Unverified);
    }

    #[test]
    fn imported_run_verdict_is_unverified_imported_source() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "imported-src-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "imported-src-0001").expect("load");
        // A run produced by `deadreckon import` is marked by import.json in its run_root.
        std::fs::write(state.run_root.join("import.json"), "{}\n").expect("import.json");

        let report = build_verdict_report(&state);
        assert_eq!(report.source, VerdictSource::Imported);
        assert_eq!(report.state, VerdictState::Unverified);
        assert!(!report.had_signed_marker);
    }

    // ---- V-P5: changed-file summary ----

    use chrono::Utc;
    use deadreckon_core::artifacts::{ProvenanceRecord, append_provenance, snapshot_working};

    #[test]
    fn verdict_reports_changed_file_counts() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "changed-run-0001", &repo);
        let mut state = deadreckon_core::load_run(&paths, "changed-run-0001").expect("load");

        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(state.working_dir.join("new.txt"), "hello\n").expect("create");
        state.turn = 1;
        save_state(&state).expect("save");
        snapshot_working(&state, 1).expect("snapshot 1");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "t".to_string(),
                session_id: "s".to_string(),
                files: vec![state.working_dir.join("new.txt")],
            },
        )
        .expect("provenance");

        let report = build_verdict_report(&state);
        assert!(
            report.changed_files.added >= 1,
            "{:?}",
            report.changed_files
        );
    }

    #[test]
    fn verdict_derives_signature_and_tamper_from_run_view() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "view-sig-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "view-sig-run-0001").expect("load");
        sign_marker(&state);

        let view = deadreckon_core::RunView::from_state(&state).expect("view");
        let report = build_verdict_report_from_view(&state, &view, rerun_acceptance(&state));

        assert!(report.had_signed_marker);
        assert_eq!(report.marker_valid, view.proof.marker_valid);
    }

    #[test]
    fn verdict_and_show_report_identical_signature_fact() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "view-parity-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "view-parity-run-0001").expect("load");
        sign_marker(&state);

        let view = deadreckon_core::RunView::from_state(&state).expect("view");
        let report = build_verdict_report(&state);

        assert_eq!(
            report.had_signed_marker,
            view.signature.marker_path.is_some()
        );
        assert_eq!(
            report.marker_valid,
            matches!(
                view.signature.status,
                deadreckon_core::SignatureStatus::Valid
            )
        );
    }

    #[test]
    fn verdict_changed_files_empty_when_no_snapshot() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "nosnap-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "nosnap-run-0001").expect("load");

        let report = build_verdict_report(&state);
        assert_eq!(report.changed_files, ChangedFiles::default());
    }

    // ---- V-P6: single-run render via VerdictSurface ----

    fn report_for(state: VerdictState) -> VerdictReport {
        VerdictReport {
            schema: 1,
            run_id: "render-run-0001".to_string(),
            taken_at: "2026-06-27T00:00:00Z".to_string(),
            state,
            had_signed_marker: matches!(state, VerdictState::Verified | VerdictState::Regressed),
            marker_valid: matches!(state, VerdictState::Verified),
            checks: vec![AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: matches!(state, VerdictState::Verified | VerdictState::Unverified),
                must_pass: true,
                detail: "ran".to_string(),
                command: Some("go test ./...".to_string()),
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
            changed_files: ChangedFiles {
                added: 1,
                modified: 2,
                deleted: 0,
            },
            source: VerdictSource::Native,
        }
    }

    fn dummy_state() -> deadreckon_core::PipelineState {
        // A working dir that exists so the render does not treat it as gone.
        let temp = Box::leak(Box::new(TempDir::new().expect("tempdir")));
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        create_run(
            &paths,
            RunOptions {
                goal: "g".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("render-state-0001".to_string()),
                codebase: None,
            },
        )
        .expect("run")
    }

    #[test]
    fn verdict_render_uses_one_primary_action() {
        let surface = render_verdict_surface(&report_for(VerdictState::Verified), &dummy_state());
        assert!(!surface.primary_action.command.is_empty());
        // VerdictSurface enforces exactly one primary action by construction.
        let rendered = surface.render_plain(false);
        assert!(
            rendered.contains("VERIFIED") || rendered.contains("verified"),
            "{rendered}"
        );
    }

    #[test]
    fn verified_render_recommends_finish() {
        let surface = render_verdict_surface(&report_for(VerdictState::Verified), &dummy_state());
        assert!(surface.primary_action.command.contains("deadreckon finish"));
    }

    #[test]
    fn regressed_render_recommends_resume() {
        let surface = render_verdict_surface(&report_for(VerdictState::Regressed), &dummy_state());
        assert!(surface.primary_action.command.contains("deadreckon resume"));
    }

    // ---- V-P7: --json parity ----

    #[test]
    fn verdict_json_envelope_shape_is_stable() {
        let report = report_for(VerdictState::Verified);
        let envelope = verdict_json(
            &report,
            &dummy_state(),
            "deadreckon finish render",
            None,
            None,
        );
        for key in [
            "kind",
            "id",
            "status",
            "checks",
            "changed_files",
            "source",
            "next_actions",
            "paths",
        ] {
            assert!(envelope.get(key).is_some(), "missing {key}: {envelope}");
        }
        assert_eq!(envelope["kind"], "verdict");
        assert_eq!(envelope["status"], "verified");
    }

    #[test]
    fn verdict_json_includes_per_check_results() {
        let report = report_for(VerdictState::Verified);
        let envelope = verdict_json(
            &report,
            &dummy_state(),
            "deadreckon finish render",
            None,
            None,
        );
        let checks = envelope["checks"].as_array().expect("checks array");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0]["command"], "go test ./...");
        assert_eq!(checks[0]["passed"], true);
    }

    #[test]
    fn verdict_json_omits_receipt_audit_without_the_flag() {
        let report = report_for(VerdictState::Verified);
        let envelope = verdict_json(
            &report,
            &dummy_state(),
            "deadreckon finish render",
            None,
            None,
        );
        assert!(envelope.get("receipt_audit").is_none());
    }

    #[test]
    fn verdict_json_receipt_audit_lists_per_digest_facts() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "audit-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "audit-run-0001").expect("load");

        // A run with no sealed receipt yields one failed receipt_document fact;
        // the envelope carries it verbatim under receipt_audit.facts.
        let audit = deadreckon_core::audit_completion_receipt(&paths, &state);
        let report = build_verdict_report(&state);
        let envelope = verdict_json(
            &report,
            &state,
            "deadreckon show audit-run-0001",
            None,
            Some(&audit),
        );

        let facts = envelope["receipt_audit"]["facts"]
            .as_array()
            .expect("facts array");
        assert_eq!(facts.len(), 1, "{facts:?}");
        assert_eq!(facts[0]["name"], "receipt_document");
        assert_eq!(facts[0]["pass"], false);
        assert!(
            facts[0]["detail"].as_str().expect("detail").len() > 0,
            "failed facts carry the strict error message"
        );
    }

    // ---- V-P8: --all comparison ----

    #[test]
    fn verdict_all_lists_recent_runs_with_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "all-run-0001", &repo);
        fixture_run(&paths, "all-run-0002", &repo);

        let summaries = verdict_all_summaries(&paths, 10).expect("summaries");
        assert_eq!(summaries.len(), 2);
        // No markers → every run is Unverified.
        assert!(
            summaries
                .iter()
                .all(|s| s.state == VerdictState::Unverified)
        );
    }

    #[test]
    fn verdict_all_json_is_array() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "alljson-run-0001", &repo);

        let summaries = verdict_all_summaries(&paths, 10).expect("summaries");
        let value = serde_json::to_value(&summaries).expect("json");
        assert!(value.is_array());
        assert_eq!(value.as_array().unwrap().len(), 1);
    }

    #[test]
    fn verdict_all_respects_limit() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "lim-run-0001", &repo);
        fixture_run(&paths, "lim-run-0002", &repo);
        fixture_run(&paths, "lim-run-0003", &repo);

        let summaries = verdict_all_summaries(&paths, 2).expect("summaries");
        assert_eq!(summaries.len(), 2);
    }

    // ---- V-P10: cache + output policy ----

    #[test]
    fn verdict_caches_report_sidecar() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "cache-run-0001", &repo);
        let state = deadreckon_core::load_run(&paths, "cache-run-0001").expect("load");

        let report = build_verdict_report(&state);
        let path = cache_verdict_report(&state, &report).expect("cache");

        assert!(path.exists(), "sidecar must be written: {path:?}");
        assert_eq!(
            path.parent().and_then(|p| p.file_name()),
            Some("proofs".as_ref())
        );
        let body = std::fs::read_to_string(&path).expect("read sidecar");
        let value: serde_json::Value = serde_json::from_str(&body).expect("sidecar json");
        assert_eq!(value["run_id"], "cache-run-0001");
        assert!(value.get("state").is_some());
    }

    #[test]
    fn verdict_never_mutates_run_status() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "nomut-run-0001", &repo);
        let before = deadreckon_core::load_run(&paths, "nomut-run-0001").expect("load");
        let status_before = before.status;

        let report = build_verdict_report(&before);
        cache_verdict_report(&before, &report).expect("cache");

        let after = deadreckon_core::load_run(&paths, "nomut-run-0001").expect("reload");
        assert_eq!(
            status_before, after.status,
            "verdict must never advance or mutate run status"
        );
    }

    // ---- G7 follow-up (APP-3): JOB refs map to the current attempt and
    // ---- default to the read-only receipt audit ----

    fn job_view_for(
        job_id: &str,
        scope: &str,
        source_cwd: &std::path::Path,
        child_run_ids: Vec<String>,
    ) -> deadreckon_core::JobView {
        use deadreckon_protocol::{
            Job, JobId, JobPhase, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode,
        };
        let id = JobId(job_id.to_string());
        deadreckon_core::JobView {
            projection: deadreckon_core::JobProjection {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: id.clone(),
                phase: JobPhase::Running,
                outcome: None,
                stop_reason: None,
                last_sequence: 1,
                current_lease_epoch: 0,
                attempt_count: child_run_ids.len() as u32,
                child_run_ids,
                delivery: None,
                last_gate_attempt: None,
                updated_at: Some(chrono::Utc::now()),
                caveats: Vec::new(),
            },
            job: Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: id,
                scope: scope.to_string(),
                goal: "job audit fixture".to_string(),
                shape: JobShape::Single,
                created_at: chrono::Utc::now(),
                source_cwd: source_cwd.to_path_buf(),
                launch_plan_sha256: "fixture-launch-plan".to_string(),
                authority_sha256: "fixture-authority".to_string(),
                policy: JobPolicy {
                    max_spend_usd: 1.0,
                    max_wall_seconds: 60,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: SemanticJudgeMode::Required,
                    execution: None,
                },
            },
            attempts: Vec::new(),
            missing_attempts: Vec::new(),
            verified_receipt_error: None,
        }
    }

    #[test]
    fn job_ref_maps_to_the_newest_linked_attempt() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "attempt-run-0001", &repo);
        fixture_run(&paths, "attempt-run-0002", &repo);
        let view = job_view_for(
            "job-mapping-0001",
            "scope",
            &repo,
            vec![
                "attempt-run-0001".to_string(),
                "attempt-run-0002".to_string(),
            ],
        );

        // `child_run_ids` is newest-last (the same rule the fleet rollup and
        // the mac app's Chartroom use), so the current attempt is the last.
        let state = current_attempt_state(&paths, &view).expect("current attempt");
        assert_eq!(state.run_id, "attempt-run-0002");
    }

    #[test]
    fn single_job_without_linked_children_falls_back_to_its_own_run() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        // A Single-shape job's attempt run IS the job id (supervisor.rs stamps
        // run_id = job_id), so an empty child list still names one run.
        fixture_run(&paths, "single-job-run-0001", &repo);
        let view = job_view_for("single-job-run-0001", "scope", &repo, Vec::new());

        let state = current_attempt_state(&paths, &view).expect("backing run");
        assert_eq!(state.run_id, "single-job-run-0001");
    }

    #[test]
    fn job_without_any_attempt_run_refuses_toward_status() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        let view = job_view_for("job-no-attempt-01", "scope", &repo, Vec::new());

        let error = current_attempt_state(&paths, &view).expect_err("no attempt run");
        let message = error.to_string();
        assert!(
            message.contains("has no attempt run to verify yet"),
            "{message}"
        );
        assert!(message.contains("deadreckon status"), "{message}");
    }

    #[test]
    fn job_audit_envelope_is_inspection_only() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "job-audit-run-0001", &repo);
        let view = job_view_for(
            "job-audit-job-0001",
            "scope",
            &repo,
            vec!["job-audit-run-0001".to_string()],
        );
        let state = current_attempt_state(&paths, &view).expect("attempt");
        let (had_signed_marker, marker_valid) = legacy_signature_fact(&state);
        let audit = deadreckon_core::audit_completion_receipt(&paths, &state);
        let status = compute_verdict(had_signed_marker, marker_valid, audit.passed());
        let surface = render_job_audit_surface(&view, status, &audit);

        let envelope = verdict_job_audit_json(
            &view,
            &state,
            status,
            had_signed_marker,
            marker_valid,
            &audit,
            &surface.primary_action.command,
        );

        assert_eq!(envelope["kind"], "verdict");
        assert_eq!(envelope["id"], "job-audit-run-0001");
        assert_eq!(envelope["job_id"], "job-audit-job-0001");
        assert_eq!(envelope["mode"], "receipt_audit");
        // Nothing is re-executed and nothing is written: checks stay empty and
        // the sidecar slot reads null — the run root belongs to the driver.
        assert_eq!(envelope["checks_rerun"], false);
        assert_eq!(envelope["checks"], serde_json::json!([]));
        assert_eq!(envelope["paths"]["cache"], serde_json::Value::Null);
        // No marker and no receipt → honest Unverified, never Verified.
        assert_eq!(envelope["status"], "unverified");
        let facts = envelope["receipt_audit"]["facts"]
            .as_array()
            .expect("facts array");
        assert_eq!(facts[0]["name"], "receipt_document");
        assert_eq!(facts[0]["pass"], false);
    }

    #[test]
    fn job_audit_surface_speaks_of_proofs_not_rerun_checks() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "job-surface-run-0001", &repo);
        let view = job_view_for(
            "job-surface-job-0001",
            "scope",
            &repo,
            vec!["job-surface-run-0001".to_string()],
        );
        let state = current_attempt_state(&paths, &view).expect("attempt");
        let audit = deadreckon_core::audit_completion_receipt(&paths, &state);
        let surface = render_job_audit_surface(&view, VerdictState::Unverified, &audit);

        let rendered = surface.render_plain(false);
        assert!(rendered.contains("NOT re-run"), "{rendered}");
        assert!(
            surface.primary_action.command.contains("deadreckon status"),
            "{}",
            surface.primary_action.command
        );
    }

    #[test]
    fn verdict_quiet_suppresses_secondary_actions() {
        let surface = render_verdict_surface(&report_for(VerdictState::Verified), &dummy_state());
        let loud = surface.render_plain(false);
        let quiet = surface.render_plain(true);
        assert!(
            loud.contains("deadreckon show"),
            "non-quiet render should carry the secondary inspect hint:\n{loud}"
        );
        assert!(
            !quiet.contains("deadreckon show"),
            "--quiet must suppress secondary actions:\n{quiet}"
        );
    }
}
