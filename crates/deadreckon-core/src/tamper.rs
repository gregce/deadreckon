use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::artifacts::ProvenanceRecord;
use crate::artifacts::inventory_files;
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::gate::ACCEPTANCE_SPEC;
use crate::gate::AcceptanceCheck;

pub const ACCEPTANCE_TAMPER: &str = "acceptance-tamper.json";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TouchedChange {
    Modified,
    Deleted,
    Created,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CoverageClassification {
    Test,
    Target,
    Build,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceTamperVerdict {
    Clean,
    Caveat,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuppressionFinding {
    pub check_kind: String,
    pub command: String,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckCoverage {
    pub path: PathBuf,
    pub by_check: String,
    pub classification: CoverageClassification,
    pub directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoveredFileTouch {
    pub path: String,
    pub change: TouchedChange,
    pub by_check: String,
    pub classification: CoverageClassification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptanceTamper {
    pub schema_version: u32,
    pub run_id: String,
    pub evaluated_at: chrono::DateTime<Utc>,
    pub verdict: AcceptanceTamperVerdict,
    pub spec_modified: bool,
    pub lint_findings: Vec<SuppressionFinding>,
    pub covered_files_touched: Vec<CoveredFileTouch>,
    pub caveats: Vec<String>,
    pub refusal_reasons: Vec<String>,
}

pub fn acceptance_tamper_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_TAMPER)
}

pub fn read_acceptance_tamper_for_run_root(run_root: &Path) -> Result<Option<AcceptanceTamper>> {
    let path = acceptance_tamper_path_for_run_root(run_root);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&raw).with_json_path(&path).map(Some)
}

pub fn write_acceptance_tamper(run_root: &Path, tamper: &AcceptanceTamper) -> Result<()> {
    let path = acceptance_tamper_path_for_run_root(run_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(tamper).map_err(|source| DeadreckonError::Json {
        path: path.clone(),
        source,
    })?;
    fs::write(&path, bytes).with_path(&path)
}

pub fn evaluate(
    run_id: &str,
    run_root: &Path,
    working_dir: &Path,
    checks: &[AcceptanceCheck],
) -> Result<AcceptanceTamper> {
    let touched = touched_files(run_root, working_dir)?;
    let spec_modified = spec_modified(run_root, working_dir)?;
    let mut coverage = check_coverage(checks, working_dir)?;
    if checks
        .iter()
        .any(|check| matches!(check, AcceptanceCheck::CargoTest { .. }))
        && let Some(snapshot) = baseline_snapshot_dir(run_root)?
    {
        for path in rust_test_files(&snapshot)? {
            coverage.push(CheckCoverage {
                path,
                by_check: "cargo_test".to_string(),
                classification: CoverageClassification::Test,
                directory: false,
            });
        }
    }
    // Cross-language analogue: a shell test-runner check covers the conventional
    // test files in the earliest snapshot, so deleting a JS/Py/Go/etc. test
    // (gone from the post-run working dir) still refuses like a deleted Rust test.
    if checks.iter().any(|check| {
        matches!(check, AcceptanceCheck::Shell { command, .. } if shell_program_is_test_runner(command))
    }) && let Some(snapshot) = baseline_snapshot_dir(run_root)?
    {
        for path in conventional_test_files(&snapshot)? {
            coverage.push(CheckCoverage {
                path,
                by_check: "shell".to_string(),
                classification: CoverageClassification::Test,
                directory: false,
            });
        }
    }
    coverage.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.by_check.cmp(&right.by_check))
            .then(left.classification.cmp(&right.classification))
    });
    coverage.dedup_by(|left, right| {
        left.path == right.path
            && left.by_check == right.by_check
            && left.classification == right.classification
            && left.directory == right.directory
    });
    let covered_files_touched = covered_touches(&coverage, &touched);
    let lint_findings = lint_checks(checks);
    let (verdict, caveats, refusal_reasons) =
        classify(spec_modified, &lint_findings, &covered_files_touched);
    Ok(AcceptanceTamper {
        schema_version: 1,
        run_id: run_id.to_string(),
        evaluated_at: Utc::now(),
        verdict,
        spec_modified,
        lint_findings,
        covered_files_touched,
        caveats,
        refusal_reasons,
    })
}

pub fn touched_files(
    run_root: &Path,
    working_dir: &Path,
) -> Result<BTreeMap<PathBuf, TouchedChange>> {
    let first_snapshot = baseline_snapshot_dir(run_root)?;
    let first_inventory = match first_snapshot.as_deref() {
        Some(snapshot) => relative_inventory(snapshot)?,
        None => BTreeSet::new(),
    };
    let final_inventory = relative_inventory(working_dir)?;
    let mut touched = BTreeMap::new();
    for path in provenance_files(run_root)? {
        let Some(relative) = working_relative_path(working_dir, &path) else {
            continue;
        };
        if ignored_relative(&relative) {
            continue;
        }
        let change = if !final_inventory.contains(&relative) {
            TouchedChange::Deleted
        } else if first_inventory.contains(&relative) {
            TouchedChange::Modified
        } else {
            TouchedChange::Created
        };
        touched.insert(relative, change);
    }
    for deleted in first_inventory.difference(&final_inventory) {
        if !ignored_relative(deleted) {
            touched.insert(deleted.clone(), TouchedChange::Deleted);
        }
    }
    Ok(touched)
}

pub fn check_coverage(
    checks: &[AcceptanceCheck],
    working_dir: &Path,
) -> Result<Vec<CheckCoverage>> {
    let mut coverage = Vec::new();
    for check in checks {
        match check {
            AcceptanceCheck::FileExists { path, .. } => {
                if let Some(path) = rendered_relative(working_dir, working_dir, path) {
                    coverage.push(CheckCoverage {
                        path,
                        by_check: "file_exists".to_string(),
                        classification: CoverageClassification::Target,
                        directory: false,
                    });
                }
            }
            AcceptanceCheck::ContentMatch { path, .. } => {
                if let Some(path) = rendered_relative(working_dir, working_dir, path) {
                    coverage.push(CheckCoverage {
                        path,
                        by_check: "content_match".to_string(),
                        classification: CoverageClassification::Target,
                        directory: false,
                    });
                }
            }
            AcceptanceCheck::BuildSuccess { cwd, .. } => {
                if let Some(path) = rendered_relative(working_dir, working_dir, cwd) {
                    coverage.push(CheckCoverage {
                        path,
                        by_check: "build_success".to_string(),
                        classification: CoverageClassification::Build,
                        directory: true,
                    });
                }
            }
            AcceptanceCheck::CargoTest { .. } => {
                for path in rust_test_files(working_dir)? {
                    coverage.push(CheckCoverage {
                        path,
                        by_check: "cargo_test".to_string(),
                        classification: CoverageClassification::Test,
                        directory: false,
                    });
                }
            }
            AcceptanceCheck::Shell { command, cwd, .. } => {
                let cwd_path = cwd
                    .as_deref()
                    .map(|cwd| rendered_path(working_dir, working_dir, cwd))
                    .unwrap_or_else(|| working_dir.to_path_buf());
                for path in command_existing_paths(command, working_dir, &cwd_path) {
                    let classification = if is_rust_test_file(working_dir, &path) {
                        CoverageClassification::Test
                    } else {
                        CoverageClassification::Unknown
                    };
                    coverage.push(CheckCoverage {
                        path,
                        by_check: "shell".to_string(),
                        classification,
                        directory: false,
                    });
                }
                // A command whose program is a known cross-language test runner
                // covers the ecosystem's conventional test files as Test — so
                // deleting a JS/Py/Go/etc. test refuses like a deleted Rust test.
                if shell_program_is_test_runner(command) {
                    for path in conventional_test_files(working_dir)? {
                        coverage.push(CheckCoverage {
                            path,
                            by_check: "shell".to_string(),
                            classification: CoverageClassification::Test,
                            directory: false,
                        });
                    }
                }
            }
        }
    }
    coverage.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.by_check.cmp(&right.by_check))
            .then(left.classification.cmp(&right.classification))
    });
    coverage.dedup_by(|left, right| {
        left.path == right.path
            && left.by_check == right.by_check
            && left.classification == right.classification
            && left.directory == right.directory
    });
    Ok(coverage)
}

pub fn lint_suppressions(check_kind: &str, command: &str) -> Vec<SuppressionFinding> {
    [
        ("|| exit 0", r"(?i)\|\|\s*exit\s+0\b"),
        ("|| true", r"(?i)\|\|\s*true\b"),
        ("; true", r"(?i);\s*true\b"),
        ("&& true", r"(?i)&&\s*true\b"),
        ("| true", r"(?i)(^|[^|])\|\s*true\b"),
        ("|| :", r"(?i)\|\|\s*:(\s|$|[;&|])"),
        ("--no-verify", r"(?i)(^|\s)--no-verify(\s|$)"),
        ("--exit-zero", r"(?i)(^|\s)--exit-zero(\s|$)"),
        ("--passWithNoTests", r"(?i)(^|\s)--passwithnotests(\s|$)"),
        ("trailing exit 0", r"(?i)(^|[;&|]|\s)exit\s+0\s*$"),
    ]
    .into_iter()
    .filter_map(|(pattern, regex)| {
        let Ok(regex) = Regex::new(regex) else {
            return None;
        };
        regex.is_match(command).then(|| SuppressionFinding {
            check_kind: check_kind.to_string(),
            command: command.to_string(),
            pattern: pattern.to_string(),
        })
    })
    .collect()
}

pub fn lint_checks(checks: &[AcceptanceCheck]) -> Vec<SuppressionFinding> {
    let mut findings = Vec::new();
    for check in checks {
        match check {
            AcceptanceCheck::Shell { command, .. } => {
                findings.extend(lint_suppressions("shell", command));
            }
            AcceptanceCheck::CargoTest { args, .. } if !args.is_empty() => {
                findings.extend(lint_suppressions("cargo_test", &args.join(" ")));
            }
            AcceptanceCheck::BuildSuccess { .. }
            | AcceptanceCheck::CargoTest { .. }
            | AcceptanceCheck::FileExists { .. }
            | AcceptanceCheck::ContentMatch { .. } => {}
        }
    }
    findings
}

pub fn classify(
    spec_modified: bool,
    lint_findings: &[SuppressionFinding],
    covered_files_touched: &[CoveredFileTouch],
) -> (AcceptanceTamperVerdict, Vec<String>, Vec<String>) {
    let mut refusal_reasons = Vec::new();
    if spec_modified {
        refusal_reasons.push("agent modified acceptance.yaml this run".to_string());
    }
    for finding in lint_findings {
        refusal_reasons.push(format!(
            "suppression pattern '{}' in {} check",
            finding.pattern, finding.check_kind
        ));
    }
    for touch in covered_files_touched {
        if touch.change == TouchedChange::Deleted
            && matches!(
                touch.classification,
                CoverageClassification::Test | CoverageClassification::Target
            )
        {
            refusal_reasons.push(format!(
                "agent deleted {} file {} this run",
                classification_label(touch.classification),
                touch.path
            ));
        }
    }
    if !refusal_reasons.is_empty() {
        return (AcceptanceTamperVerdict::Refuse, Vec::new(), refusal_reasons);
    }
    let caveats = covered_files_touched
        .iter()
        .filter(|touch| {
            matches!(
                touch.classification,
                CoverageClassification::Test | CoverageClassification::Target
            )
        })
        .map(|touch| {
            format!(
                "agent {} {} file {} this run",
                change_verb(touch.change),
                classification_label(touch.classification),
                touch.path
            )
        })
        .collect::<Vec<_>>();
    if !caveats.is_empty() {
        (AcceptanceTamperVerdict::Caveat, caveats, Vec::new())
    } else {
        (AcceptanceTamperVerdict::Clean, Vec::new(), Vec::new())
    }
}

fn covered_touches(
    coverage: &[CheckCoverage],
    touched: &BTreeMap<PathBuf, TouchedChange>,
) -> Vec<CoveredFileTouch> {
    let mut out = BTreeMap::<(PathBuf, String, CoverageClassification), CoveredFileTouch>::new();
    for item in coverage {
        for (path, change) in touched {
            if coverage_matches(item, path) {
                out.insert(
                    (path.clone(), item.by_check.clone(), item.classification),
                    CoveredFileTouch {
                        path: path.to_string_lossy().to_string(),
                        change: *change,
                        by_check: item.by_check.clone(),
                        classification: item.classification,
                    },
                );
            }
        }
    }
    out.into_values().collect()
}

fn coverage_matches(coverage: &CheckCoverage, touched: &Path) -> bool {
    if coverage.directory {
        coverage.path.as_os_str().is_empty()
            || coverage.path == Path::new(".")
            || touched == coverage.path
            || touched.starts_with(&coverage.path)
    } else {
        touched == coverage.path
    }
}

fn provenance_files(run_root: &Path) -> Result<Vec<PathBuf>> {
    let path = run_root.join("provenance.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    let mut files = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let record: ProvenanceRecord = serde_json::from_str(line).with_json_path(&path)?;
        files.extend(record.files);
    }
    Ok(files)
}

fn spec_modified(run_root: &Path, working_dir: &Path) -> Result<bool> {
    let run_spec = run_root.join(ACCEPTANCE_SPEC);
    let project_spec = working_dir.join(".deadreckon").join(ACCEPTANCE_SPEC);
    Ok(provenance_files(run_root)?.into_iter().any(|path| {
        path == run_spec
            || path == project_spec
            || path.ends_with(ACCEPTANCE_SPEC)
                && path
                    .components()
                    .any(|component| component.as_os_str() == ".deadreckon")
    }))
}

/// The baseline file a retry writes: `gate/baseline` names the snapshot dir
/// of the node's FIRST attempt. Without it, attempt N's baseline is attempt
/// N-1's finished tree, so a test file edited in attempt 1 to game the gate
/// looks pre-existing to every later attempt — the tamper detector loses
/// vision exactly across the boundary self-healing introduced.
pub const TAMPER_BASELINE_FILE: &str = "gate/baseline";

pub fn tamper_baseline_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(TAMPER_BASELINE_FILE)
}

pub fn write_tamper_baseline(run_root: &Path, snapshot: &Path) -> Result<()> {
    let path = tamper_baseline_path_for_run_root(run_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::write(&path, snapshot.display().to_string()).with_path(&path)
}

/// The snapshot this run's changes are judged against: the recorded baseline
/// when one exists, else the run's own earliest snapshot. A recorded baseline
/// whose directory has vanished is a hard error, not a silent fallback — a
/// tamper check that quietly weakens itself is the defect this file exists
/// to catch in others.
fn baseline_snapshot_dir(run_root: &Path) -> Result<Option<PathBuf>> {
    let recorded = tamper_baseline_path_for_run_root(run_root);
    match fs::read_to_string(&recorded) {
        Ok(content) => {
            let target = PathBuf::from(content.trim());
            if target.is_dir() {
                Ok(Some(target))
            } else {
                Err(DeadreckonError::InvalidInput(format!(
                    "tamper baseline {} no longer exists; refusing to judge against a weaker one",
                    target.display()
                )))
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            earliest_snapshot_dir(run_root)
        }
        Err(source) => Err(DeadreckonError::Io {
            path: recorded,
            source,
        }),
    }
}

pub fn earliest_snapshot_dir(run_root: &Path) -> Result<Option<PathBuf>> {
    let snapshots = run_root.join("snapshots");
    if !snapshots.is_dir() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&snapshots).with_path(&snapshots)? {
        let entry = entry.map_err(|source| DeadreckonError::Io {
            path: snapshots.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(turn) = name
            .strip_prefix("turn-")
            .and_then(|suffix| suffix.parse::<u32>().ok())
        else {
            continue;
        };
        candidates.push((turn, path));
    }
    candidates.sort_by_key(|(turn, _)| *turn);
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

fn relative_inventory(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    for path in inventory_files(root)? {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = normalize_relative(relative);
        if !relative.as_os_str().is_empty() && !ignored_relative(&relative) {
            files.insert(relative);
        }
    }
    Ok(files)
}

fn rust_test_files(working_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in inventory_files(working_dir)? {
        let Some(relative) = working_relative_path(working_dir, &path) else {
            continue;
        };
        if ignored_relative(&relative) {
            continue;
        }
        if is_rust_test_file(working_dir, &relative) {
            files.push(relative);
        }
    }
    files.sort();
    Ok(files)
}

/// Whether a shell command's program is a known cross-language test runner.
/// Deterministic, LLM-free — conventions are stable and this is security-critical.
pub fn shell_program_is_test_runner(command: &str) -> bool {
    let raw: Vec<&str> = command.split_whitespace().collect();
    // Strip a leading `bundle exec` wrapper.
    let tokens: &[&str] = if raw.len() >= 2 && raw[0] == "bundle" && raw[1] == "exec" {
        &raw[2..]
    } else {
        &raw[..]
    };
    let Some(first) = tokens.first() else {
        return false;
    };
    let program = program_basename(first);
    let sub = tokens.get(1).copied().unwrap_or("");
    match program {
        // Runners that are tests by name, whatever the args.
        "pytest" | "rspec" | "phpunit" | "jest" | "vitest" => true,
        // Tools whose `test` subcommand is the test runner.
        "go" | "npm" | "pnpm" | "yarn" | "bun" | "deno" | "mix" | "dotnet" | "gradle"
        | "gradlew" | "make" | "just" | "task" | "composer" => sub == "test",
        // rake (bare) or `rake test`.
        "rake" => tokens.len() == 1 || tokens.contains(&"test"),
        // maven: `mvn … test`.
        "mvn" => tokens.contains(&"test"),
        // `python -m pytest`.
        "python" | "python3" => tokens.windows(2).any(|w| w == ["-m", "pytest"]),
        _ => false,
    }
}

/// The program basename of a command token, stripping a `./` or directory prefix
/// (`./gradlew` → `gradlew`, `vendor/bin/phpunit` → `phpunit`).
fn program_basename(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// Files matching cross-language test conventions: anything under a `tests/`,
/// `test/`, or `spec/` directory, plus conventional test filenames per ecosystem.
fn conventional_test_files(working_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in inventory_files(working_dir)? {
        let Some(relative) = working_relative_path(working_dir, &path) else {
            continue;
        };
        if ignored_relative(&relative) {
            continue;
        }
        if is_conventional_test_file(&relative) {
            files.push(relative);
        }
    }
    files.sort();
    Ok(files)
}

/// Whether a relative path matches a cross-language test convention.
fn is_conventional_test_file(relative: &Path) -> bool {
    if relative.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("tests" | "test" | "spec")
        )
    }) {
        return true;
    }
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with("_test.go")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("Test.java")
        || name.ends_with("Test.kt")
        || name.ends_with("_test.exs")
        || name.ends_with("Test.cs")
        || name.ends_with("Tests.cs")
        || name.ends_with("_test.py")
        || (name.starts_with("test_") && name.ends_with(".py"))
}

fn is_rust_test_file(working_dir: &Path, relative: &Path) -> bool {
    if relative.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return false;
    }
    if relative
        .components()
        .any(|component| component.as_os_str() == "tests")
    {
        return true;
    }
    if relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_test.rs"))
    {
        return true;
    }
    let path = working_dir.join(relative);
    let body = fs::read_to_string(&path).unwrap_or_default();
    body.contains("#[test]") || body.contains("#[cfg(test)]")
}

fn command_existing_paths(command: &str, working_dir: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for token in command.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ';' | ',' | '&' | '|'
            )
        });
        if token.is_empty() || token.starts_with('-') {
            continue;
        }
        let candidate = rendered_path(working_dir, cwd, token);
        if candidate.exists()
            && let Some(relative) = working_relative_path(working_dir, &candidate)
            && !ignored_relative(&relative)
        {
            paths.insert(relative);
        }
    }
    paths.into_iter().collect()
}

fn rendered_relative(working_dir: &Path, base: &Path, value: &str) -> Option<PathBuf> {
    working_relative_path(working_dir, &rendered_path(working_dir, base, value))
}

fn rendered_path(working_dir: &Path, base: &Path, value: &str) -> PathBuf {
    let rendered = value.replace("{working_dir}", &working_dir.to_string_lossy());
    let path = PathBuf::from(rendered);
    let absolute = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    lexical_normalize(&absolute)
}

fn working_relative_path(working_dir: &Path, path: &Path) -> Option<PathBuf> {
    let normalized_working = lexical_normalize(working_dir);
    let normalized_path = lexical_normalize(path);
    let relative = if normalized_path.is_absolute() {
        normalized_path
            .strip_prefix(&normalized_working)
            .ok()?
            .to_path_buf()
    } else {
        normalized_path
    };
    Some(normalize_relative(&relative))
}

fn normalize_relative(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}

fn ignored_relative(path: &Path) -> bool {
    !crate::artifact_policy::is_deliverable_workspace_path(path)
}

fn change_verb(change: TouchedChange) -> &'static str {
    match change {
        TouchedChange::Modified => "modified",
        TouchedChange::Deleted => "deleted",
        TouchedChange::Created => "created",
    }
}

fn classification_label(classification: CoverageClassification) -> &'static str {
    match classification {
        CoverageClassification::Test => "test",
        CoverageClassification::Target => "target",
        CoverageClassification::Build => "build",
        CoverageClassification::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::Utc;
    use tempfile::TempDir;

    use crate::artifacts::{ProvenanceRecord, append_provenance, snapshot_working};
    use crate::gate::AcceptanceCheck;
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::{AcceptanceTamperVerdict, CoverageClassification, CoveredFileTouch, TouchedChange};

    fn fixture_run(goal: &str) -> (TempDir, crate::state::PipelineState) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: goal.to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        (temp, state)
    }

    fn record_files(state: &crate::state::PipelineState, files: Vec<PathBuf>) {
        append_provenance(
            state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "prompt".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files,
            },
        )
        .expect("provenance");
    }

    /// The cross-attempt blind spot self-healing introduced: a retry resumes
    /// the failed attempt's tree, so its own snapshot-0 already contains
    /// whatever attempt 1 did — including a deleted test. Judged against its
    /// own snapshot the deletion is invisible; judged against the FIRST
    /// attempt's snapshot it refuses, exactly as it would have in one run.
    #[test]
    fn a_recorded_baseline_keeps_tamper_vision_across_attempts() {
        // Attempt 1: the honest starting tree, with the gate's test present.
        let (_temp_first, first) = fixture_run("attempt one");
        std::fs::create_dir_all(first.working_dir.join("tests")).expect("tests");
        std::fs::write(
            first.working_dir.join("tests/gate_test.rs"),
            "#[test]\nfn gate() {}\n",
        )
        .expect("test file");
        snapshot_working(&first, 0).expect("first snapshot");
        let first_snapshot = super::earliest_snapshot_dir(&first.run_root)
            .expect("snapshot lookup")
            .expect("snapshot exists");

        // The retry: its tree is attempt 1's aftermath — the test is gone —
        // and its own snapshot-0 faithfully records that already-tainted tree.
        let (_temp_retry, retry) = fixture_run("attempt two");
        std::fs::create_dir_all(retry.working_dir.join("src")).expect("src");
        std::fs::write(retry.working_dir.join("src/lib.rs"), "pub fn f() {}\n").expect("lib");
        snapshot_working(&retry, 0).expect("retry snapshot");
        record_files(&retry, vec![retry.working_dir.join("src/lib.rs")]);
        let checks = vec![AcceptanceCheck::CargoTest {
            args: Vec::new(),
            must_pass: true,
        }];

        let blind = super::evaluate("retry", &retry.run_root, &retry.working_dir, &checks)
            .expect("evaluate without baseline");
        assert_ne!(
            blind.verdict,
            AcceptanceTamperVerdict::Refuse,
            "against its own snapshot the deletion is invisible: {blind:?}"
        );

        super::write_tamper_baseline(&retry.run_root, &first_snapshot).expect("baseline");
        let sighted = super::evaluate("retry", &retry.run_root, &retry.working_dir, &checks)
            .expect("evaluate with baseline");
        assert_eq!(
            sighted.verdict,
            AcceptanceTamperVerdict::Refuse,
            "against attempt 1's snapshot the deleted test refuses: {sighted:?}"
        );
    }

    /// A recorded baseline whose directory has vanished is a hard error, not
    /// a silent fallback to a weaker comparison.
    #[test]
    fn a_vanished_baseline_refuses_rather_than_weakening() {
        let (_temp, state) = fixture_run("vanished baseline");
        snapshot_working(&state, 0).expect("snapshot");
        super::write_tamper_baseline(&state.run_root, &state.run_root.join("gone"))
            .expect("baseline");

        let error = super::evaluate("run", &state.run_root, &state.working_dir, &[])
            .expect_err("must refuse");

        assert!(error.to_string().contains("tamper baseline"), "{error}");
    }

    #[test]
    fn touched_files_unions_provenance_and_detects_snapshot_deletions() {
        let (_temp, state) = fixture_run("touched files");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn old() {}\n").expect("lib");
        std::fs::write(state.working_dir.join("obsolete.txt"), "remove me\n").expect("obsolete");
        snapshot_working(&state, 0).expect("snapshot");

        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn new() {}\n").expect("edit");
        std::fs::write(state.working_dir.join("new.txt"), "created\n").expect("new");
        std::fs::remove_file(state.working_dir.join("obsolete.txt")).expect("delete");
        record_files(
            &state,
            vec![
                state.working_dir.join("src/lib.rs"),
                state.working_dir.join("new.txt"),
            ],
        );

        let touched = super::touched_files(&state.run_root, &state.working_dir).expect("touched");

        assert_eq!(
            touched.get(&PathBuf::from("src/lib.rs")),
            Some(&TouchedChange::Modified)
        );
        assert_eq!(
            touched.get(&PathBuf::from("new.txt")),
            Some(&TouchedChange::Created)
        );
        assert_eq!(
            touched.get(&PathBuf::from("obsolete.txt")),
            Some(&TouchedChange::Deleted)
        );
    }

    #[test]
    fn touched_files_excludes_lifecycle_and_provider_evidence_subtrees() {
        let (_temp, state) = fixture_run("private paths excluded");
        std::fs::create_dir_all(state.working_dir.join(".deadreckon")).expect("dir");
        std::fs::create_dir_all(state.working_dir.join(".specstory/history")).expect("private dir");
        std::fs::write(state.working_dir.join(".deadreckon/RUN.md"), "generated\n").expect("doc");
        std::fs::write(
            state.working_dir.join(".specstory/history/session.md"),
            "private\n",
        )
        .expect("private");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::remove_file(state.working_dir.join(".deadreckon/RUN.md")).expect("delete");
        std::fs::remove_file(state.working_dir.join(".specstory/history/session.md"))
            .expect("delete private");
        record_files(
            &state,
            vec![
                state.working_dir.join(".deadreckon/RUN.md"),
                state.working_dir.join(".specstory/history/session.md"),
            ],
        );

        let touched = super::touched_files(&state.run_root, &state.working_dir).expect("touched");

        assert!(touched.is_empty(), "{touched:?}");
    }

    #[test]
    fn check_coverage_classifies_test_target_build_unknown() {
        let (_temp, state) = fixture_run("coverage");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("tests/auth_test.rs"),
            "#[test]\nfn auth() {}\n",
        )
        .expect("test");
        std::fs::write(
            state.working_dir.join("src/lib.rs"),
            "#[cfg(test)]\nmod tests {}\n",
        )
        .expect("lib");
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(state.working_dir.join("script.sh"), "exit 0\n").expect("script");
        let checks = vec![
            AcceptanceCheck::CargoTest {
                args: Vec::new(),
                must_pass: true,
            },
            AcceptanceCheck::FileExists {
                path: "{working_dir}/README.md".to_string(),
                must_pass: true,
            },
            AcceptanceCheck::BuildSuccess {
                cwd: "{working_dir}/src".to_string(),
                must_pass: true,
            },
            AcceptanceCheck::Shell {
                command: "sh script.sh".to_string(),
                cwd: None,
                must_pass: true,
            },
        ];

        let coverage = super::check_coverage(&checks, &state.working_dir).expect("coverage");
        let by_path = coverage
            .iter()
            .map(|item| (item.path.clone(), item.classification))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            by_path.get(&PathBuf::from("tests/auth_test.rs")),
            Some(&CoverageClassification::Test)
        );
        assert_eq!(
            by_path.get(&PathBuf::from("src/lib.rs")),
            Some(&CoverageClassification::Test)
        );
        assert_eq!(
            by_path.get(&PathBuf::from("README.md")),
            Some(&CoverageClassification::Target)
        );
        assert_eq!(
            by_path.get(&PathBuf::from("src")),
            Some(&CoverageClassification::Build)
        );
        assert_eq!(
            by_path.get(&PathBuf::from("script.sh")),
            Some(&CoverageClassification::Unknown)
        );
    }

    // ---- P7: shell test-runner commands classify conventional tests as Test ----

    fn covers_as_test(working_dir: &std::path::Path, command: &str, test_rel: &str) -> bool {
        let checks = vec![AcceptanceCheck::Shell {
            command: command.to_string(),
            cwd: None,
            must_pass: true,
        }];
        let coverage = super::check_coverage(&checks, working_dir).expect("coverage");
        coverage.iter().any(|item| {
            item.path == std::path::Path::new(test_rel)
                && item.classification == CoverageClassification::Test
        })
    }

    #[test]
    fn npm_test_shell_check_classifies_as_test_coverage() {
        let (_temp, state) = fixture_run("npm");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::write(
            state.working_dir.join("src/auth.test.js"),
            "test('x',()=>{})\n",
        )
        .expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "npm test",
            "src/auth.test.js"
        ));
    }

    #[test]
    fn pytest_shell_check_maps_tests_dir_coverage() {
        let (_temp, state) = fixture_run("pytest");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("tests/test_auth.py"),
            "def test_x():\n    pass\n",
        )
        .expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "python -m pytest -q",
            "tests/test_auth.py"
        ));
    }

    #[test]
    fn go_test_shell_check_maps_go_test_files() {
        let (_temp, state) = fixture_run("go");
        std::fs::write(state.working_dir.join("auth_test.go"), "package x\n").expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "go test ./...",
            "auth_test.go"
        ));
    }

    #[test]
    fn mix_test_shell_check_classifies_as_test_coverage() {
        let (_temp, state) = fixture_run("mix");
        std::fs::create_dir_all(state.working_dir.join("test")).expect("test dir");
        std::fs::write(
            state.working_dir.join("test/auth_test.exs"),
            "defmodule X do\nend\n",
        )
        .expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "mix test",
            "test/auth_test.exs"
        ));
    }

    #[test]
    fn dotnet_test_shell_check_maps_test_files() {
        let (_temp, state) = fixture_run("dotnet");
        std::fs::write(
            state.working_dir.join("AuthTests.cs"),
            "class AuthTests {}\n",
        )
        .expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "dotnet test",
            "AuthTests.cs"
        ));
    }

    #[test]
    fn make_test_shell_check_classifies_as_test_coverage() {
        let (_temp, state) = fixture_run("make");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(state.working_dir.join("tests/smoke.sh"), "echo ok\n").expect("test");
        assert!(covers_as_test(
            &state.working_dir,
            "make test",
            "tests/smoke.sh"
        ));
    }

    #[test]
    fn cargo_test_coverage_matches_test_dirs_and_cfg_test_files() {
        let (_temp, state) = fixture_run("cargo coverage");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(state.working_dir.join("tests/auth.rs"), "fn helper() {}\n")
            .expect("integration");
        std::fs::write(
            state.working_dir.join("src/unit_test.rs"),
            "fn helper() {}\n",
        )
        .expect("unit name");
        std::fs::write(
            state.working_dir.join("src/lib.rs"),
            "#[cfg(test)]\nmod tests {}\n",
        )
        .expect("cfg");
        let checks = vec![AcceptanceCheck::CargoTest {
            args: Vec::new(),
            must_pass: true,
        }];

        let coverage = super::check_coverage(&checks, &state.working_dir).expect("coverage");
        let paths = coverage
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();

        assert!(paths.contains(&PathBuf::from("tests/auth.rs")), "{paths:?}");
        assert!(
            paths.contains(&PathBuf::from("src/unit_test.rs")),
            "{paths:?}"
        );
        assert!(paths.contains(&PathBuf::from("src/lib.rs")), "{paths:?}");
    }

    #[test]
    fn spec_modified_yields_refuse() {
        let (verdict, _caveats, reasons) = super::classify(true, &[], &[]);

        assert_eq!(verdict, AcceptanceTamperVerdict::Refuse);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("acceptance.yaml"))
        );
    }

    #[test]
    fn suppression_finding_yields_refuse() {
        let findings = super::lint_suppressions("shell", "cargo test || true");
        let (verdict, _caveats, reasons) = super::classify(false, &findings, &[]);

        assert_eq!(verdict, AcceptanceTamperVerdict::Refuse);
        assert!(reasons.iter().any(|reason| reason.contains("|| true")));
    }

    #[test]
    fn modified_test_file_yields_caveat() {
        let touches = vec![CoveredFileTouch {
            path: "tests/auth_test.rs".to_string(),
            change: TouchedChange::Modified,
            by_check: "cargo_test".to_string(),
            classification: CoverageClassification::Test,
        }];

        let (verdict, caveats, reasons) = super::classify(false, &[], &touches);

        assert_eq!(verdict, AcceptanceTamperVerdict::Caveat);
        assert!(reasons.is_empty());
        assert!(
            caveats
                .iter()
                .any(|caveat| caveat.contains("tests/auth_test.rs"))
        );
    }

    #[test]
    fn modified_production_code_only_stays_clean() {
        let (_temp, state) = fixture_run("production clean");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn old() {}\n").expect("lib");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("tests/auth_test.rs"),
            "#[test]\nfn auth() {}\n",
        )
        .expect("test");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn new() {}\n").expect("edit");
        record_files(&state, vec![state.working_dir.join("src/lib.rs")]);
        let checks = vec![AcceptanceCheck::CargoTest {
            args: Vec::new(),
            must_pass: true,
        }];

        let tamper = super::evaluate(&state.run_id, &state.run_root, &state.working_dir, &checks)
            .expect("tamper");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Clean);
        assert!(tamper.covered_files_touched.is_empty());
    }

    // ---- P8: deletion + suppression refuse cross-language ----

    #[test]
    fn deleting_covered_jest_test_refuses() {
        let (_temp, state) = fixture_run("jest delete");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::write(
            state.working_dir.join("src/auth.test.js"),
            "test('x',()=>{})\n",
        )
        .expect("test");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::remove_file(state.working_dir.join("src/auth.test.js")).expect("delete");
        record_files(&state, vec![state.working_dir.join("src/auth.test.js")]);
        let checks = vec![AcceptanceCheck::Shell {
            command: "npm test".to_string(),
            cwd: None,
            must_pass: true,
        }];

        let tamper = super::evaluate(&state.run_id, &state.run_root, &state.working_dir, &checks)
            .expect("tamper");
        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Refuse);
        assert!(
            tamper
                .refusal_reasons
                .iter()
                .any(|r| r.contains("auth.test.js"))
        );
    }

    #[test]
    fn deleting_covered_pytest_test_refuses() {
        let (_temp, state) = fixture_run("pytest delete");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("tests/test_auth.py"),
            "def test_x():\n    pass\n",
        )
        .expect("test");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::remove_file(state.working_dir.join("tests/test_auth.py")).expect("delete");
        record_files(&state, vec![state.working_dir.join("tests/test_auth.py")]);
        let checks = vec![AcceptanceCheck::Shell {
            command: "python -m pytest -q".to_string(),
            cwd: None,
            must_pass: true,
        }];

        let tamper = super::evaluate(&state.run_id, &state.run_root, &state.working_dir, &checks)
            .expect("tamper");
        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Refuse);
    }

    #[test]
    fn pass_with_no_tests_flag_refuses() {
        let findings = super::lint_suppressions("shell", "jest --passWithNoTests");
        assert!(
            !findings.is_empty(),
            "--passWithNoTests is a suppression evasion"
        );
        let (verdict, _, reasons) = super::classify(false, &findings, &[]);
        assert_eq!(verdict, AcceptanceTamperVerdict::Refuse);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("passWithNoTests") || r.contains("suppression"))
        );

        // A trailing `exit 0` that masks a real test failure also refuses.
        assert!(!super::lint_suppressions("shell", "pytest -q ; exit 0").is_empty());
    }

    #[test]
    fn build_or_unknown_touch_only_stays_clean_but_is_recorded() {
        let (_temp, state) = fixture_run("build clean");
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn old() {}\n").expect("lib");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(state.working_dir.join("src/lib.rs"), "pub fn new() {}\n").expect("edit");
        record_files(&state, vec![state.working_dir.join("src/lib.rs")]);
        let checks = vec![AcceptanceCheck::BuildSuccess {
            cwd: "{working_dir}/src".to_string(),
            must_pass: true,
        }];

        let tamper = super::evaluate(&state.run_id, &state.run_root, &state.working_dir, &checks)
            .expect("tamper");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Clean);
        assert_eq!(tamper.covered_files_touched.len(), 1);
        assert_eq!(
            tamper.covered_files_touched[0].classification,
            CoverageClassification::Build
        );
    }
}
