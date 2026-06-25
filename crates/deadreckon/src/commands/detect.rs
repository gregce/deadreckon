//! Friendliness surfaces for the polyglot done-contract: the `deadreckon detect`
//! report, the run preflight contract-preview line, the no-test-contract caveat,
//! and the refuse-with-`try:` footer for a detected-but-unrunnable tree.

use std::path::Path;

use deadreckon_core::acceptance_defaults::{
    ContractSource, ProjectKind, default_command_for, detect_project_kind, detection_caveat,
    kind_label,
};

/// The `deadreckon detect` report: the resolved project kind and the default
/// done-contract command (or the caveat when none can be compiled).
pub(crate) fn detect_report(working_dir: &Path) -> String {
    let kind = detect_project_kind(working_dir);
    let mut lines = vec![format!("project kind: {}", kind_label(&kind))];
    match default_command_for(&kind, working_dir) {
        Some(command) => lines.push(format!("done contract: {command}")),
        None => lines.push(format!(
            "done contract: none — {}",
            detection_caveat(&kind).unwrap_or_else(|| "no test contract detected".to_string())
        )),
    }
    if let Some((reason, try_hint)) = unrunnable_refusal(working_dir, "run") {
        lines.push(format!("note: {reason}"));
        lines.push(format!("try:  {try_hint}"));
    }
    lines.join("\n")
}

/// One line for the run preflight preview: the resolved done-contract command and
/// where it came from (detected / operator / inferred).
pub(crate) fn contract_preview_line(working_dir: &Path, source: ContractSource) -> String {
    let kind = detect_project_kind(working_dir);
    let source_label = match source {
        ContractSource::Detected => "detected",
        ContractSource::Operator => "operator",
        ContractSource::Inferred => "inferred",
    };
    match default_command_for(&kind, working_dir) {
        Some(command) => format!("done contract: {command}  ({source_label})"),
        None => format!(
            "done contract: {}  ({source_label})",
            detection_caveat(&kind).unwrap_or_else(|| "no test contract detected".to_string())
        ),
    }
}

/// The no-test-contract caveat for a tree, surfaced into the run verdict so an
/// Unknown-kind run reads "PASSED with caveat" rather than a bare green.
pub(crate) fn contract_caveat(working_dir: &Path) -> Option<String> {
    detection_caveat(&detect_project_kind(working_dir))
}

/// A detected-but-unrunnable tree refuses with a `try:` footer rather than
/// silently running a hollow gate: we know it's a JS/Python project but can't
/// resolve a real test command. Returns `(reason, try_hint)`. A truly unknown
/// tree (no project signals) returns `None` — it proceeds with the caveat.
pub(crate) fn unrunnable_refusal(working_dir: &Path, verb: &str) -> Option<(String, String)> {
    if !matches!(detect_project_kind(working_dir), ProjectKind::Unknown) {
        return None;
    }
    if working_dir.join("package.json").exists() {
        return Some((
            "package.json has no test script".to_string(),
            format!(
                "deadreckon {verb} <goal> --acceptance ./acceptance.yaml  (or --infer-contract)"
            ),
        ));
    }
    let python = ["pyproject.toml", "setup.py", "setup.cfg"]
        .iter()
        .any(|name| working_dir.join(name).exists());
    if python {
        return Some((
            "pyproject present but no tests are visible".to_string(),
            "add tests/ or pass --acceptance ./acceptance.yaml".to_string(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write fixture");
    }

    #[test]
    fn detect_command_reports_project_kind_and_default_command() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "go.mod", "module example.com/x\n");
        let report = detect_report(tmp.path());
        assert!(report.contains("project kind: go"), "{report}");
        assert!(report.contains("done contract: go test ./..."), "{report}");
    }

    #[test]
    fn node_without_test_script_refuses_with_try_acceptance_or_infer_line() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "package.json", r#"{"scripts":{"build":"tsc"}}"#);
        let (reason, try_hint) = unrunnable_refusal(tmp.path(), "run").expect("refusal");
        assert!(reason.contains("no test script"), "{reason}");
        assert!(try_hint.contains("--acceptance"), "{try_hint}");
        assert!(try_hint.contains("--infer-contract"), "{try_hint}");
    }

    #[test]
    fn preflight_preview_lists_detected_done_contract_and_source() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "go.mod", "module example.com/x\n");
        let line = contract_preview_line(tmp.path(), ContractSource::Detected);
        assert!(line.contains("go test ./..."), "{line}");
        assert!(line.contains("detected"), "{line}");
    }

    #[test]
    fn unknown_kind_run_surfaces_no_test_contract_caveat() {
        let tmp = TempDir::new().expect("tempdir");
        let caveat = contract_caveat(tmp.path()).expect("caveat for unknown tree");
        assert!(caveat.contains("no test contract"), "{caveat}");
        // A truly unknown tree (no project signals) is a caveat, not a refusal.
        assert!(unrunnable_refusal(tmp.path(), "run").is_none());
    }
}
