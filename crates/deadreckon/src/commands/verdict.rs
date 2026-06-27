//! `deadreckon verdict` — a read-only "did it actually work?" report for any run.
//!
//! Verdict re-verifies a run NOW by re-running its acceptance checks through the
//! same engine the gate uses (`evaluate_acceptance_checks`), reads (never
//! overwrites) the original signed marker, and renders one of three honest
//! states with evidence and a single next action. It NEVER mutates run state,
//! advances a phase, or promotes — the result is a sidecar audit file only.

use super::super::*;
use deadreckon_core::gate::AcceptanceCheckResult;
use serde::Serialize;

/// Parsed `deadreckon verdict` arguments.
pub(crate) struct VerdictArgs {
    pub(crate) run_id: Option<String>,
    pub(crate) all: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) json: bool,
    pub(crate) plain: bool,
    pub(crate) quiet: bool,
}

/// Re-verify a run (or compare several with `--all`) and report the verdict.
///
/// P1 wires the verb and resolves the run; live re-evaluation, marker reading,
/// `VerdictSurface` rendering, `--json`, `--all`, and the sidecar cache land in
/// P2–P10. For now it prints a placeholder so the command is reachable.
pub(crate) async fn verdict_command(args: VerdictArgs) -> Result<()> {
    let _ = (args.all, args.limit, args.json, args.plain, args.quiet);
    let paths = DeadreckonPaths::discover();
    let state = resolve_verdict_run(&paths, args.run_id.as_deref())?;
    let report = build_verdict_report(&state);
    let surface = render_verdict_surface(&report, &state);
    println!("{}", surface.render_plain(args.quiet));
    Ok(())
}

/// Resolve the run to verify: an explicit id/prefix, or the most recent run
/// across all scopes when no id is given. Not-found and ambiguous-prefix become
/// `try:`-footer refusals so a typo reads as guidance, not a stack trace.
pub(crate) fn resolve_verdict_run(
    paths: &DeadreckonPaths,
    id_arg: Option<&str>,
) -> Result<deadreckon_core::PipelineState> {
    match id_arg {
        None | Some("latest") | Some("last") => resolve_latest_run(paths),
        Some(id) => deadreckon_core::load_run(paths, id).map_err(|err| {
            let message = err.to_string();
            let refusal = if message.contains("ambiguous") {
                message
            } else {
                format!("unknown run {id}")
            };
            CliError::Core(deadreckon_core::user_error(&refusal, "deadreckon list"))
        }),
    }
}

/// The most recently updated run across every scope (so `verdict` works from any
/// directory), or a refusal when there are no runs at all.
fn resolve_latest_run(paths: &DeadreckonPaths) -> Result<deadreckon_core::PipelineState> {
    let mut runs = deadreckon_core::list_runs(paths, None)?;
    runs.sort_by_key(|entry| entry.updated_at);
    match runs.last() {
        Some(entry) => Ok(deadreckon_core::load_run(paths, &entry.run_id)?),
        None => Err(CliError::Core(deadreckon_core::user_error(
            "no runs to verify",
            "deadreckon start \"<goal>\"",
        ))),
    }
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
        Vec::<(String, String)>::new(),
    )
}

/// Build the full verdict for a run: re-run its checks, read (never overwrite)
/// the signed marker, and combine through `compute_verdict`. Read-only.
pub(crate) fn build_verdict_report(state: &deadreckon_core::PipelineState) -> VerdictReport {
    let rerun = rerun_acceptance(state);
    let marker_path = deadreckon_core::gate::marker_path_for_run_root(&state.run_root);
    let had_signed_marker = marker_path.exists();
    // A marker that no longer validates (signature mismatch, tampered tamper
    // bytes, wrong run_id) is treated as not-valid → Regressed, never Verified.
    let marker_valid =
        had_signed_marker && deadreckon_core::gate::validate_acceptance_marker(state).is_ok();
    VerdictReport {
        schema: 1,
        run_id: state.run_id.clone(),
        taken_at: chrono::Utc::now().to_rfc3339(),
        state: compute_verdict(had_signed_marker, marker_valid, rerun.all_must_pass),
        had_signed_marker,
        marker_valid,
        checks: rerun.checks,
        changed_files: changed_file_counts(state),
        source: VerdictSource::Native,
    }
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

/// The result of re-running a run's acceptance checks NOW. Read-only: it runs
/// the same checks the gate uses but writes no spec, no progress, and no state.
pub(crate) struct RerunResult {
    pub(crate) checks: Vec<AcceptanceCheckResult>,
    pub(crate) all_must_pass: bool,
}

/// Re-run the run's acceptance checks against its recorded working dir via the
/// same `evaluate_acceptance_checks` path dr-gate uses (no early-exit, full
/// per-check results). A missing working dir (exported/cleaned) yields no checks
/// (a caller treats empty + missing dir as "working dir unavailable").
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
    // Constructed in P9 (imported-run integration); reserved here so the report
    // schema is stable from P1.
    #[allow(dead_code)]
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
    use deadreckon_core::state::{RunOptions, create_run};
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

        let state = resolve_verdict_run(&paths, None).expect("latest");
        assert_eq!(state.run_id, "only-run-0001");
    }

    #[test]
    fn verdict_unknown_id_refuses_with_try_list() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        fixture_run(&paths, "real-run-0001", &repo);

        let err = resolve_verdict_run(&paths, Some("nope")).expect_err("refuse");
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

        let err = resolve_verdict_run(&paths, Some("ambig")).expect_err("refuse");
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
        let state = deadreckon_core::load_run(&paths, "changed-run-0001").expect("load");

        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(state.working_dir.join("new.txt"), "hello\n").expect("create");
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
}
