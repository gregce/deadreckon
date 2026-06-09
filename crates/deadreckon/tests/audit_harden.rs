#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::path::Path;

const AUDIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/AUDIT-2026-05-11.md"
);
// The needs report is a source research doc outside this repository; resolve
// it relative to the running user's home so the cross-reference runs on the
// author's machine and skips cleanly everywhere else (e.g. CI).
fn report_path() -> std::path::PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join("stoa/docs/research/2026-05-10-unmet-needs/REPORT.md")
}

#[test]
fn audit_doc_lists_all_25_needs_with_status() {
    let audit = read_audit();
    let rows = audit_rows(&audit);

    assert_eq!(rows.len(), 25, "audit rows:\n{rows:#?}");

    // REPORT.md is a source research doc that lives outside this repository, so
    // the cross-reference can only run on a machine that has it (e.g. the
    // author's). Skip it cleanly elsewhere, such as in CI.
    let Some(needs) = report_need_titles() else {
        eprintln!(
            "skipping external REPORT.md cross-reference: {} not present",
            report_path().display()
        );
        return;
    };
    for (idx, need) in needs.iter().enumerate() {
        let row = rows
            .iter()
            .find(|row| row.columns[0] == (idx + 1).to_string())
            .unwrap_or_else(|| panic!("missing row {}", idx + 1));
        assert_eq!(row.columns[1], *need);
        assert!(!row.columns[2].trim().is_empty(), "missing status: {row:?}");
    }
}

#[test]
fn audit_doc_status_values_are_closed_enum() {
    let audit = read_audit();
    for row in audit_rows(&audit) {
        assert!(
            matches!(
                row.columns[2].as_str(),
                "Resolved" | "Partial" | "Unmet" | "V1"
            ),
            "invalid status in row: {row:?}"
        );
    }
}

#[test]
fn audit_doc_evidence_paths_resolve_under_repo() {
    let audit = read_audit();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");
    for row in audit_rows(&audit) {
        let evidence = &row.columns[3];
        let path = evidence
            .split(['`', ' ', '('])
            .find(|part| {
                part.starts_with("crates/")
                    || part.starts_with("docs/")
                    || part.starts_with("README.md")
                    || part.starts_with("HOWTO.md")
                    || part.starts_with("CHANGELOG.md")
                    || part.starts_with("demo.cast")
            })
            .unwrap_or_else(|| panic!("missing evidence path in row: {row:?}"))
            .trim_end_matches([',', ')', ';']);
        let path = path.split(':').next().expect("path before line");
        assert!(
            repo.join(path).exists(),
            "evidence path does not exist: {path} in row {row:?}"
        );
    }
}

#[test]
fn audit_doc_closures_map_lists_p2_through_p10() {
    let audit = read_audit();
    for phase in 2..=10 {
        let needle = format!("P{phase} ");
        assert!(audit.contains(&needle), "missing closure phase {needle}");
    }
}

#[derive(Debug)]
struct AuditRow {
    columns: Vec<String>,
}

fn read_audit() -> String {
    std::fs::read_to_string(AUDIT).expect("docs/AUDIT-2026-05-11.md should exist")
}

fn audit_rows(audit: &str) -> Vec<AuditRow> {
    audit
        .lines()
        .filter(|line| {
            line.starts_with('|')
                && !line.starts_with("|---")
                && !line.contains("Need (verbatim title)")
        })
        .filter_map(|line| {
            let columns = line
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect::<Vec<_>>();
            columns
                .first()
                .and_then(|cell| cell.parse::<usize>().ok())
                .map(|_| AuditRow { columns })
        })
        .collect()
}

fn report_need_titles() -> Option<Vec<String>> {
    let contents = std::fs::read_to_string(report_path()).ok()?;
    Some(
        contents
            .lines()
            .filter_map(|line| line.strip_prefix("## Need: "))
            .map(ToString::to_string)
            .collect(),
    )
}
