//! Deterministic, total, no-network detection of a project's default test
//! contract. When a run has no operator `acceptance.yaml`, the gate compiles a
//! real default from this module instead of the old `cargo test`-or-`FileExists`
//! fallback, so "VERIFIED" means a real test set ran — in any language.
//!
//! The detector is pure over the filesystem: it reads file presence and (for a
//! few ambiguous kinds) file contents, never executes a subprocess, and always
//! returns a `ProjectKind` (it is total — `Unknown` is a value, not an error).
//! `default_checks_for` compiles each kind into real [`AcceptanceCheck`]s
//! (`Shell` for non-Rust), which reuse the existing evaluation/signing/tamper
//! path. See the Polyglot rider for the full detection table.

use std::path::Path;

use crate::gate::AcceptanceCheck;

/// The detected project kind. `detect_project_kind` is total — it always
/// returns one of these, never errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectKind {
    Rust,
    Node(PackageManager),
    Deno,
    Go,
    Python,
    Elixir,
    Dotnet,
    Jvm(BuildTool),
    Ruby(RubyRunner),
    Php(PhpRunner),
    ScriptRunner(Runner),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTool {
    Maven,
    Gradle,
    GradleWrapper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RubyRunner {
    Rspec,
    Minitest,
    Rake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhpRunner {
    Composer,
    Phpunit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Make,
    Just,
    Task,
}

/// Where a compiled contract came from. Computed in-memory and threaded into
/// preview/verdict text; the durable record is the generated spec's provenance
/// comment, not a persisted struct field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractSource {
    Detected,
    Inferred,
    Operator,
}

/// Resolve the project kind from `working_dir` by sentinel files (first match
/// wins). Pure, total, no network, no subprocess.
pub fn detect_project_kind(working_dir: &Path) -> ProjectKind {
    // P1: Rust + Unknown only; later phases fill in the table.
    if working_dir.join("Cargo.toml").exists() {
        return ProjectKind::Rust;
    }
    ProjectKind::Unknown
}

/// Compile a kind into the real default [`AcceptanceCheck`] set. Rust keeps the
/// existing `CargoTest`; everything else (until P4) falls back to the historical
/// `FileExists {working_dir}` placeholder.
pub fn default_checks_for(kind: &ProjectKind, _working_dir: &Path) -> Vec<AcceptanceCheck> {
    match kind {
        ProjectKind::Rust => vec![AcceptanceCheck::CargoTest {
            args: Vec::new(),
            must_pass: true,
        }],
        _ => vec![AcceptanceCheck::FileExists {
            path: "{working_dir}".to_string(),
            must_pass: true,
        }],
    }
}

/// A caveat string for kinds that cannot prove a real test ran (only `Unknown`
/// and degraded cases). `None` means the compiled contract is a genuine test.
pub fn detection_caveat(kind: &ProjectKind) -> Option<String> {
    match kind {
        ProjectKind::Unknown => Some("no test contract detected".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_rust_from_cargo_toml() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n")
            .expect("write Cargo.toml");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Rust);
    }

    #[test]
    fn unknown_kind_when_no_sentinels() {
        let tmp = TempDir::new().expect("tempdir");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Unknown);
    }
}
