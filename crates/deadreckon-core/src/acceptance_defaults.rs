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
    if python_project(working_dir) && python_has_visible_tests(working_dir) {
        return ProjectKind::Python;
    }
    if exists("mix.exs") {
        return ProjectKind::Elixir;
    }
    if has_dotnet_project(working_dir) {
        return ProjectKind::Dotnet;
    }
    // JVM: Maven wins over Gradle (lower row); Gradle prefers the wrapper.
    if exists("pom.xml") {
        return ProjectKind::Jvm(BuildTool::Maven);
    }
    if exists("build.gradle") || exists("build.gradle.kts") {
        return ProjectKind::Jvm(if exists("gradlew") {
            BuildTool::GradleWrapper
        } else {
            BuildTool::Gradle
        });
    }
    if exists("Gemfile") || exists("Rakefile") || working_dir.join("spec").is_dir() {
        return ProjectKind::Ruby(ruby_runner(working_dir));
    }
    if exists("composer.json") && composer_has_test_script(working_dir) {
        return ProjectKind::Php(PhpRunner::Composer);
    }
    if exists("phpunit.xml") || exists("phpunit.xml.dist") {
        return ProjectKind::Php(PhpRunner::Phpunit);
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

/// A Python project sentinel: `pyproject.toml`, `setup.py`, or `setup.cfg`.
fn python_project(working_dir: &Path) -> bool {
    let exists = |name: &str| working_dir.join(name).exists();
    exists("pyproject.toml") || exists("setup.py") || exists("setup.cfg")
}

/// Whether the tree has visible tests: a `tests/` dir, or any top-level
/// `test_*.py` / `*_test.py`. A bare project with no tests degrades to Unknown —
/// a green "0 tests" is hollow.
fn python_has_visible_tests(working_dir: &Path) -> bool {
    if working_dir.join("tests").is_dir() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(working_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .map(is_python_test_file)
            .unwrap_or(false)
    })
}

fn is_python_test_file(name: &str) -> bool {
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

/// Ruby sub-runner: `spec/` + `rspec` in `Gemfile.lock` → Rspec; else Rake.
/// (Substring scan of `Gemfile.lock`; no bundler crate.)
fn ruby_runner(working_dir: &Path) -> RubyRunner {
    let has_rspec = working_dir.join("spec").is_dir()
        && std::fs::read_to_string(working_dir.join("Gemfile.lock"))
            .map(|lock| lock.contains("rspec"))
            .unwrap_or(false);
    if has_rspec {
        RubyRunner::Rspec
    } else {
        RubyRunner::Rake
    }
}

/// Whether `composer.json` declares a non-empty `scripts.test`.
fn composer_has_test_script(working_dir: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(working_dir.join("composer.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value
        .get("scripts")
        .and_then(|scripts| scripts.get("test"))
        .map(|test| !test.is_null())
        .unwrap_or(false)
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
/// existing `CargoTest`; every other native kind compiles to a single
/// `Shell { command, cwd: Some(working_dir), must_pass: true }` running that
/// ecosystem's canonical test command — which reuses the existing evaluation,
/// signing, and tamper path. `Unknown` keeps the historical `FileExists`.
pub fn default_checks_for(kind: &ProjectKind, working_dir: &Path) -> Vec<AcceptanceCheck> {
    if let ProjectKind::Rust = kind {
        return vec![AcceptanceCheck::CargoTest {
            args: Vec::new(),
            must_pass: true,
        }];
    }
    let Some(command) = test_command_for(kind, working_dir) else {
        return vec![AcceptanceCheck::FileExists {
            path: "{working_dir}".to_string(),
            must_pass: true,
        }];
    };
    vec![AcceptanceCheck::Shell {
        command,
        cwd: Some(working_dir.display().to_string()),
        must_pass: true,
    }]
}

/// The canonical test command for a kind, or `None` for `Unknown`/`Rust`.
fn test_command_for(kind: &ProjectKind, working_dir: &Path) -> Option<String> {
    let command = match kind {
        ProjectKind::Rust | ProjectKind::Unknown => return None,
        ProjectKind::Node(pm) => format!("{} test", package_manager_program(*pm)),
        ProjectKind::Deno => "deno test -A".to_string(),
        ProjectKind::Go => "go test ./...".to_string(),
        ProjectKind::Python => "python -m pytest -q".to_string(),
        ProjectKind::Elixir => "mix test".to_string(),
        ProjectKind::Dotnet => "dotnet test".to_string(),
        ProjectKind::Jvm(BuildTool::Maven) => "mvn -q test".to_string(),
        ProjectKind::Jvm(BuildTool::Gradle) => "gradle test".to_string(),
        ProjectKind::Jvm(BuildTool::GradleWrapper) => "./gradlew test".to_string(),
        ProjectKind::Ruby(runner) => ruby_command(*runner, working_dir),
        ProjectKind::Php(PhpRunner::Composer) => "composer test".to_string(),
        ProjectKind::Php(PhpRunner::Phpunit) => {
            if working_dir.join("vendor/bin/phpunit").exists() {
                "vendor/bin/phpunit".to_string()
            } else {
                "phpunit".to_string()
            }
        }
        ProjectKind::ScriptRunner(Runner::Make) => "make test".to_string(),
        ProjectKind::ScriptRunner(Runner::Just) => "just test".to_string(),
        ProjectKind::ScriptRunner(Runner::Task) => "task test".to_string(),
    };
    Some(command)
}

fn package_manager_program(pm: PackageManager) -> &'static str {
    match pm {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Yarn => "yarn",
        PackageManager::Bun => "bun",
    }
}

/// Ruby command: rspec when resolved; otherwise `bundle exec rake test` when the
/// Rakefile declares a `test` task, else `bundle exec rake`.
fn ruby_command(runner: RubyRunner, working_dir: &Path) -> String {
    match runner {
        RubyRunner::Rspec => "bundle exec rspec".to_string(),
        RubyRunner::Minitest => "bundle exec rake test".to_string(),
        RubyRunner::Rake => {
            let has_test_task = std::fs::read_to_string(working_dir.join("Rakefile"))
                .map(|body| body.contains(":test") || body.contains("\"test\""))
                .unwrap_or(false);
            if has_test_task {
                "bundle exec rake test".to_string()
            } else {
                "bundle exec rake".to_string()
            }
        }
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

    // ---- P3: resolved kinds (Python/JVM/Ruby/PHP) + precedence ----

    fn mkdir(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(name)).expect("mkdir fixture");
    }

    #[test]
    fn detect_python_requires_visible_tests() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "pyproject.toml", "[project]\nname = \"x\"\n");
        write(
            tmp.path(),
            "test_app.py",
            "def test_ok():\n    assert True\n",
        );
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Python);
    }

    #[test]
    fn bare_pyproject_no_tests_is_unknown() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "pyproject.toml", "[project]\nname = \"x\"\n");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Unknown);
    }

    #[test]
    fn detect_maven_from_pom_xml() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "pom.xml", "<project></project>\n");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Jvm(BuildTool::Maven)
        );
    }

    #[test]
    fn gradle_uses_wrapper_when_gradlew_present() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "build.gradle", "plugins { id 'java' }\n");
        write(tmp.path(), "gradlew", "#!/bin/sh\n");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Jvm(BuildTool::GradleWrapper)
        );
    }

    #[test]
    fn ruby_prefers_rspec_when_spec_dir_and_rspec_in_lockfile() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "Gemfile", "source 'https://rubygems.org'\n");
        write(
            tmp.path(),
            "Gemfile.lock",
            "GEM\n  specs:\n    rspec (3.12.0)\n",
        );
        mkdir(tmp.path(), "spec");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Ruby(RubyRunner::Rspec)
        );
    }

    #[test]
    fn ruby_falls_back_to_rake_test_task() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "Gemfile", "source 'https://rubygems.org'\n");
        write(tmp.path(), "Rakefile", "task :test do\nend\n");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Ruby(RubyRunner::Rake)
        );
    }

    #[test]
    fn php_prefers_composer_test_script() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "composer.json",
            r#"{"scripts":{"test":"phpunit"}}"#,
        );
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Php(PhpRunner::Composer)
        );
    }

    #[test]
    fn php_uses_phpunit_when_no_composer_script() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "phpunit.xml", "<phpunit></phpunit>\n");
        assert_eq!(
            detect_project_kind(tmp.path()),
            ProjectKind::Php(PhpRunner::Phpunit)
        );
    }

    #[test]
    fn native_kind_wins_over_makefile_test_target() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "go.mod", "module example.com/x\n");
        write(tmp.path(), "Makefile", "test:\n\t./run-tests\n");
        assert_eq!(detect_project_kind(tmp.path()), ProjectKind::Go);
    }

    // ---- P4: default_checks_for compiles real Shell checks ----

    fn shell_of(checks: &[AcceptanceCheck]) -> (String, Option<String>, bool) {
        match checks.first().expect("one check") {
            AcceptanceCheck::Shell {
                command,
                cwd,
                must_pass,
            } => (command.clone(), cwd.clone(), *must_pass),
            other => panic!("expected Shell, got {other:?}"),
        }
    }

    #[test]
    fn node_pnpm_compiles_pnpm_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Node(PackageManager::Pnpm), tmp.path());
        let (command, _, must_pass) = shell_of(&checks);
        assert_eq!(command, "pnpm test");
        assert!(must_pass);
    }

    #[test]
    fn python_compiles_pytest_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Python, tmp.path());
        assert_eq!(shell_of(&checks).0, "python -m pytest -q");
    }

    #[test]
    fn go_compiles_go_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Go, tmp.path());
        assert_eq!(shell_of(&checks).0, "go test ./...");
    }

    #[test]
    fn elixir_compiles_mix_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Elixir, tmp.path());
        assert_eq!(shell_of(&checks).0, "mix test");
    }

    #[test]
    fn dotnet_compiles_dotnet_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Dotnet, tmp.path());
        assert_eq!(shell_of(&checks).0, "dotnet test");
    }

    #[test]
    fn gradle_wrapper_compiles_gradlew_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Jvm(BuildTool::GradleWrapper), tmp.path());
        assert_eq!(shell_of(&checks).0, "./gradlew test");
    }

    #[test]
    fn script_runner_compiles_make_test_shell_check() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::ScriptRunner(Runner::Make), tmp.path());
        assert_eq!(shell_of(&checks).0, "make test");
    }

    #[test]
    fn compiled_check_sets_cwd_to_working_dir() {
        let tmp = TempDir::new().expect("tempdir");
        let checks = default_checks_for(&ProjectKind::Go, tmp.path());
        let (_, cwd, _) = shell_of(&checks);
        assert_eq!(
            cwd.as_deref(),
            Some(tmp.path().display().to_string().as_str())
        );
    }
}
