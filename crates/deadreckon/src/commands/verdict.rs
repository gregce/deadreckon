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
    let id = args.run_id.as_deref().unwrap_or("latest");
    let state = load_cli_run(&paths, id)?;
    let report = VerdictReport {
        schema: 1,
        run_id: state.run_id,
        taken_at: chrono::Utc::now().to_rfc3339(),
        state: compute_verdict(false, false, false),
        had_signed_marker: false,
        marker_valid: false,
        checks: Vec::new(),
        changed_files: ChangedFiles::default(),
        source: VerdictSource::Native,
    };
    println!("{} {}", report.state.label(), report.run_id);
    Ok(())
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

impl VerdictState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            VerdictState::Verified => "VERIFIED",
            VerdictState::Regressed => "REGRESSED",
            VerdictState::Unverified => "UNVERIFIED",
        }
    }
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
}
