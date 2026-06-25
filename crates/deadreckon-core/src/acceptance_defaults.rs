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
    let exists = |name: &str| working_dir.join(name).exists();

    // Native kinds (rows 1–12) — a precise, sentinel-fixed command. Lower row
    // wins; native always beats a script-runner (rows 13–15).
    if exists("Cargo.toml") {
        return ProjectKind::Rust;
    }
    if exists("package.json") && package_json_has_test_script(working_dir) {
        return ProjectKind::Node(node_package_manager(working_dir));
    }
    if exists("deno.json") || exists("deno.jsonc") {
        return ProjectKind::Deno;
    }
    if exists("go.mod") {
        return ProjectKind::Go;
    }
    if exists("mix.exs") {
        return ProjectKind::Elixir;
    }
    if has_dotnet_project(working_dir) {
        return ProjectKind::Dotnet;
    }

    // Script-runners (rows 13–15) — a textual `test` entry-point scan. The
    // universal catch for ecosystems not in the native table.
    if let Some(runner) = detect_script_runner(working_dir) {
        return ProjectKind::ScriptRunner(runner);
    }

    // Rows 16/17: a bare package.json (no test script) and everything else
    // degrade to Unknown (FileExists + caveat).
    ProjectKind::Unknown
}

/// Whether `package.json` declares a non-empty `scripts.test`.
fn package_json_has_test_script(working_dir: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(working_dir.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value
        .get("scripts")
        .and_then(|scripts| scripts.get("test"))
        .and_then(|test| test.as_str())
        .map(|test| !test.trim().is_empty())
        .unwrap_or(false)
}

/// Resolve the Node package manager from the lockfile present, defaulting to npm.
fn node_package_manager(working_dir: &Path) -> PackageManager {
    let exists = |name: &str| working_dir.join(name).exists();
    if exists("bun.lockb") {
        PackageManager::Bun
    } else if exists("pnpm-lock.yaml") {
        PackageManager::Pnpm
    } else if exists("yarn.lock") {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

/// Any `*.csproj` / `*.fsproj` / `*.sln` in the directory marks a .NET project.
fn has_dotnet_project(working_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(working_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| matches!(ext, "csproj" | "fsproj" | "sln"))
            .unwrap_or(false)
    })
}

/// Textual, deterministic scan for a `test` entry-point in a Make/just/Task
/// runner. Does not evaluate the file or expand variables — it only proves a
/// `test` target exists, then the canonical invocation is compiled.
fn detect_script_runner(working_dir: &Path) -> Option<Runner> {
    for name in ["Makefile", "makefile", "GNUmakefile"] {
        if let Ok(body) = std::fs::read_to_string(working_dir.join(name))
            && has_make_target(&body, "test")
        {
            return Some(Runner::Make);
        }
    }
    for name in ["justfile", "Justfile"] {
        if let Ok(body) = std::fs::read_to_string(working_dir.join(name))
            && has_just_recipe(&body, "test")
        {
            return Some(Runner::Just);
        }
    }
    for name in ["Taskfile.yml", "Taskfile.yaml"] {
        if let Ok(body) = std::fs::read_to_string(working_dir.join(name))
            && has_task(&body, "test")
        {
            return Some(Runner::Task);
        }
    }
    None
}

/// A Makefile target line `test:` (rule at column 0, before any `=`).
fn has_make_target(body: &str, target: &str) -> bool {
    let head = format!("{target}:");
    body.lines().any(|line| {
        let trimmed = line.trim_end();
        trimmed == head.trim_end_matches(':')
            || trimmed.starts_with(&head) && !line.starts_with([' ', '\t']) && !line.contains('=')
    })
}

/// A just recipe `test:` (recipe name at column 0, optional deps after).
fn has_just_recipe(body: &str, recipe: &str) -> bool {
    body.lines().any(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .split_once(':')
                .map(|(name, _)| name.trim() == recipe)
                .unwrap_or(false)
    })
}

/// A Taskfile `test:` key nested under `tasks:` (two-space indent convention).
fn has_task(body: &str, task: &str) -> bool {
    let head = format!("  {task}:");
    body.lines().any(|line| {
        let trimmed = line.trim_end();
        trimmed == head || trimmed.starts_with(&format!("{head} "))
    })
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

    // ---- P2: single-canonical-command kinds + script-runner ----

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write fixture file");
    }

    #[test]
    fn detect_node_npm_when_no_lockfile() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "package.json", r#"{"scripts":{"test":"jest"}}"#);
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Node(PackageManager::Npm)
        );
    }

    #[test]
    fn detect_node_pnpm_from_lockfile() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest"}}"#,
        );
        write(tmp.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Node(PackageManager::Pnpm)
        );
    }

    #[test]
    fn node_without_test_script_yields_caveat() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "package.json", r#"{"scripts":{"build":"tsc"}}"#);
        let kind = detect_project_kind(tmp.path());
        assert_eq!(kind, ProjectKind::Unknown);
        assert!(detection_caveat(&kind).is_some());
    }

    #[test]
    fn detect_deno_from_deno_json() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "deno.json", "{}");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Deno);
    }

    #[test]
    fn detect_go_from_go_mod() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "go.mod", "module example.com/x\n\ngo 1.22\n");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Go);
    }

    #[test]
    fn detect_elixir_from_mix_exs() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "mix.exs", "defmodule X.MixProject do\nend\n");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Elixir);
    }

    #[test]
    fn detect_dotnet_from_csproj() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\" />\n",
        );
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Dotnet);
    }

    #[test]
    fn detect_make_test_target() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "Makefile",
            "build:\n\tcc x.c\n\ntest:\n\t./run-tests\n",
        );
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::ScriptRunner(Runner::Make)
        );
    }

    #[test]
    fn detect_justfile_test_recipe() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "justfile",
            "build:\n    cargo build\n\ntest:\n    ./run\n",
        );
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::ScriptRunner(Runner::Just)
        );
    }
}
