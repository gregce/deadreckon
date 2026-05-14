use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::DeadreckonError;
use deadreckon_providers::ProviderError;
use deadreckon_sandbox::SandboxError;
use serde_json::Value;

const CRATE_MANIFESTS: &[&str] = &[
    "crates/deadreckon/Cargo.toml",
    "crates/deadreckon-core/Cargo.toml",
    "crates/deadreckon-providers/Cargo.toml",
    "crates/deadreckon-runtime/Cargo.toml",
    "crates/deadreckon-sandbox/Cargo.toml",
];

const LIB_CRATES: &[(&str, &str)] = &[
    ("deadreckon-core", "crates/deadreckon-core/src/lib.rs"),
    (
        "deadreckon-providers",
        "crates/deadreckon-providers/src/lib.rs",
    ),
    (
        "deadreckon-runtime",
        "crates/deadreckon-runtime/src/lib.rs",
    ),
    (
        "deadreckon-sandbox",
        "crates/deadreckon-sandbox/src/lib.rs",
    ),
];

#[test]
fn every_crate_inherits_workspace_lints() {
    let root = workspace_root();
    for manifest in CRATE_MANIFESTS {
        let text = fs::read_to_string(root.join(manifest)).expect("read crate Cargo.toml");
        assert!(
            text.contains("[lints]\nworkspace = true"),
            "{manifest} must inherit [workspace.lints]"
        );
    }
}

#[test]
fn clippy_toml_allows_unwrap_in_tests() {
    let text = fs::read_to_string(workspace_root().join("clippy.toml")).expect("read clippy.toml");
    assert!(text.contains("allow-unwrap-in-tests = true"));
    assert!(text.contains("allow-expect-in-tests = true"));
    assert!(text.contains("allow-dbg-in-tests = true"));
    assert!(text.contains("large-error-threshold = 256"));
}

#[test]
fn lint_table_denies_unwrap_used() {
    assert_lint_level("unwrap_used", "deny");
}

#[test]
fn lint_table_denies_expect_used() {
    assert_lint_level("expect_used", "deny");
}

#[test]
fn lint_table_denies_await_holding_lock() {
    assert_lint_level("await_holding_lock", "deny");
}

#[test]
fn clippy_runs_clean_under_deny_warnings() {
    if !recursive_verify_enabled() {
        return;
    }
    let output = Command::new("cargo")
        .args(["clippy", "--workspace", "--", "-D", "warnings"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo clippy --workspace -- -D warnings");
    assert!(
        output.status.success(),
        "clippy must be clean under -D warnings\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rustfmt_toml_pins_imports_granularity_item() {
    let text = fs::read_to_string(workspace_root().join("rustfmt.toml")).expect("read rustfmt.toml");
    assert!(text.contains("edition = \"2024\""));
    assert!(text.contains("imports_granularity = \"Item\""));
    assert!(text.contains("group_imports = \"StdExternalCrate\""));
    assert!(text.contains("reorder_imports = true"));
}

#[test]
fn rustfmt_check_clean() {
    if !recursive_verify_enabled() {
        return;
    }
    let output = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo fmt --check");
    assert!(
        output.status.success(),
        "rustfmt must be clean\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_commit_touches_only_whitespace_and_imports() {
    let root = workspace_root();
    let commit = git_stdout(
        &root,
        &[
            "log",
            "--format=%H",
            "--grep=^style: apply rustfmt with imports_granularity=Item$",
            "-1",
        ],
    );
    assert!(!commit.trim().is_empty(), "missing dedicated rustfmt commit");
    let files = git_stdout(
        &root,
        &["show", "--name-only", "--format=", commit.trim()],
    );
    let files = files
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert!(
        files
            .iter()
            .all(|path| *path == "rustfmt.toml" || path.ends_with(".rs")),
        "format commit touched non-format files: {files:?}"
    );
    for file in files.iter().filter(|path| path.ends_with(".rs")) {
        assert_rs_identifier_set_unchanged(&root, commit.trim(), file);
    }
}

#[test]
fn release_profile_pins_lto_fat() {
    assert_cargo_toml_contains("lto = \"fat\"");
}

#[test]
fn release_profile_pins_codegen_units_one() {
    assert_cargo_toml_contains("codegen-units = 1");
}

#[test]
fn release_profile_keeps_panic_unwind() {
    assert_cargo_toml_contains("panic = \"unwind\"");
}

#[test]
fn release_binary_size_within_baseline_slack() {
    let root = workspace_root();
    let baseline = fs::read_to_string(root.join("tests/.size-baseline"))
        .expect("read size baseline")
        .trim()
        .parse::<u64>()
        .expect("parse size baseline");
    let binary = root.join("target/release/deadreckon");
    if !binary.exists() {
        let output = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(&root)
            .output()
            .expect("build release binary");
        assert!(
            output.status.success(),
            "cargo build --release failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let actual = fs::metadata(&binary).expect("release binary metadata").len();
    let allowed = baseline + (baseline / 20);
    assert!(
        actual <= allowed,
        "release binary grew too much: actual={actual}, baseline={baseline}, allowed={allowed}"
    );
}

#[test]
fn internal_crates_listed_in_workspace_dependencies() {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    for (name, path) in [
        ("deadreckon-core", "crates/deadreckon-core"),
        ("deadreckon-providers", "crates/deadreckon-providers"),
        ("deadreckon-runtime", "crates/deadreckon-runtime"),
        ("deadreckon-sandbox", "crates/deadreckon-sandbox"),
    ] {
        let needle = format!("{name} = {{ path = \"{path}\" }}");
        assert!(text.contains(&needle), "missing workspace dep `{needle}`");
    }
}

#[test]
fn no_crate_uses_relative_path_for_internal_dep() {
    let root = workspace_root();
    for manifest in CRATE_MANIFESTS {
        let text = fs::read_to_string(root.join(manifest)).expect("read crate Cargo.toml");
        for line in text.lines() {
            assert!(
                !(line.contains("deadreckon-") && line.contains("path = \"../")),
                "{manifest} still uses a relative internal dependency: {line}"
            );
        }
    }
}

#[test]
fn cargo_metadata_resolves_same_dag() {
    let root = workspace_root();
    let expected = fs::read_to_string(root.join("tests/.metadata-dag-baseline"))
        .expect("read metadata DAG baseline");
    let actual = internal_metadata_dag(&root).join("\n") + "\n";
    assert_eq!(expected, actual, "internal cargo metadata DAG changed");
}

#[test]
fn library_crate_lib_rs_denies_print_stdout() {
    for (_, rel) in LIB_CRATES {
        assert!(
            lib_rs_text(rel).contains("#![deny(clippy::print_stdout)]"),
            "{rel} must deny print_stdout"
        );
    }
}

#[test]
fn library_crate_lib_rs_denies_print_stderr() {
    for (_, rel) in LIB_CRATES {
        assert!(
            lib_rs_text(rel).contains("#![deny(clippy::print_stderr)]"),
            "{rel} must deny print_stderr"
        );
    }
}

#[test]
fn binary_crate_does_not_inherit_print_deny() {
    let root = workspace_root();
    let main_rs =
        fs::read_to_string(root.join("crates/deadreckon/src/main.rs")).expect("read main.rs");
    assert!(!main_rs.contains("clippy::print_stdout"));
    assert!(!main_rs.contains("clippy::print_stderr"));
    assert!(!root.join("crates/deadreckon/src/lib.rs").exists());
}

#[test]
fn core_lib_rs_module_declarations_grouped() {
    assert_lib_rs_module_declarations_grouped("crates/deadreckon-core/src/lib.rs");
}

#[test]
fn core_lib_rs_pub_use_paths_sorted() {
    assert_lib_rs_pub_use_paths_sorted("crates/deadreckon-core/src/lib.rs");
}

#[test]
fn core_lib_rs_contains_no_impl_block() {
    let text = lib_rs_text("crates/deadreckon-core/src/lib.rs");
    assert!(!text.contains("\nimpl "), "core lib.rs must not contain impl blocks");
}

#[test]
fn core_lib_rs_contains_no_fn_definition() {
    let text = lib_rs_text("crates/deadreckon-core/src/lib.rs");
    assert!(
        !text.contains("\nfn "),
        "core lib.rs must not contain function definitions"
    );
}

#[test]
fn providers_lib_rs_module_declarations_grouped() {
    assert_lib_rs_module_declarations_grouped("crates/deadreckon-providers/src/lib.rs");
}

#[test]
fn runtime_lib_rs_module_declarations_grouped() {
    assert_lib_rs_module_declarations_grouped("crates/deadreckon-runtime/src/lib.rs");
}

#[test]
fn sandbox_lib_rs_module_declarations_grouped() {
    assert_lib_rs_module_declarations_grouped("crates/deadreckon-sandbox/src/lib.rs");
}

#[test]
fn every_library_lib_rs_pub_use_set_unchanged_from_p1() {
    if !recursive_verify_enabled() {
        return;
    }
    let root = workspace_root();
    let output = Command::new("cargo")
        .args(["test", "-p", "deadreckon", "--test", "public_surface"])
        .current_dir(&root)
        .output()
        .expect("run public surface test");
    assert!(
        output.status.success(),
        "public surface changed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn recursive_verify_enabled() -> bool {
    std::env::var_os("DEADRECKON_RECURSIVE_VERIFY").is_some()
}

#[test]
fn deadreckon_error_every_variant_is_retryable_or_fatal() {
    assert_taxonomy(deadreckon_error_variants());
}

#[test]
fn provider_error_every_variant_is_retryable_or_fatal() {
    assert_taxonomy(provider_error_variants());
}

#[test]
fn sandbox_error_every_variant_is_retryable_or_fatal() {
    assert_taxonomy(sandbox_error_variants());
}

#[test]
fn deadreckon_error_io_interrupted_is_retryable() {
    let error = DeadreckonError::Io {
        path: "path".into(),
        source: std::io::Error::new(std::io::ErrorKind::Interrupted, "interrupted"),
    };
    assert!(error.is_retryable());
    assert!(!error.is_fatal());
}

#[test]
fn provider_error_no_route_is_fatal() {
    let error = ProviderError::NoRoute("none".to_string());
    assert!(!error.is_retryable());
    assert!(error.is_fatal());
}

#[test]
fn provider_error_http_is_fatal_with_v1_followup_noted() {
    let error = ProviderError::Http {
        provider: "openai".to_string(),
        detail: "429".to_string(),
    };
    assert!(!error.is_retryable());
    assert!(error.is_fatal());
    let notes =
        fs::read_to_string(workspace_root().join("docs/V1-CANDIDATES.md")).expect("read V1 notes");
    assert!(
        notes.contains("ProviderError::Http") && notes.contains("status"),
        "V1 notes must record ProviderError::Http needs a status field"
    );
}

#[test]
fn runtime_error_taxonomy_present() {
    let error = DeadreckonError::InvalidInput("runtime uses core errors".to_string());
    assert!(error.is_fatal());
    assert!(!error.is_retryable());
}

fn assert_lint_level(lint: &str, level: &str) {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    let needle = format!("{lint} = \"{level}\"");
    assert!(text.contains(&needle), "missing `{needle}`");
}

trait Taxonomy {
    fn is_retryable(&self) -> bool;
    fn is_fatal(&self) -> bool;
}

impl Taxonomy for DeadreckonError {
    fn is_retryable(&self) -> bool {
        DeadreckonError::is_retryable(self)
    }

    fn is_fatal(&self) -> bool {
        DeadreckonError::is_fatal(self)
    }
}

impl Taxonomy for ProviderError {
    fn is_retryable(&self) -> bool {
        ProviderError::is_retryable(self)
    }

    fn is_fatal(&self) -> bool {
        ProviderError::is_fatal(self)
    }
}

impl Taxonomy for SandboxError {
    fn is_retryable(&self) -> bool {
        SandboxError::is_retryable(self)
    }

    fn is_fatal(&self) -> bool {
        SandboxError::is_fatal(self)
    }
}

fn assert_taxonomy<T: Taxonomy>(errors: Vec<T>) {
    for error in errors {
        assert!(
            error.is_retryable() || error.is_fatal(),
            "error variant must be retryable or fatal"
        );
        assert!(
            !(error.is_retryable() && error.is_fatal()),
            "error variant cannot be both retryable and fatal"
        );
    }
}

fn deadreckon_error_variants() -> Vec<DeadreckonError> {
    vec![
        DeadreckonError::Io {
            path: "path".into(),
            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
        },
        DeadreckonError::Json {
            path: "path".into(),
            source: serde_json::from_str::<Value>("{").expect_err("invalid json"),
        },
        DeadreckonError::InvalidInput("bad".to_string()),
        DeadreckonError::NotFound("missing".to_string()),
        DeadreckonError::LockHeld {
            task_key: "task".to_string(),
            run_id: "run".to_string(),
            phase: "phase".to_string(),
        },
    ]
}

fn provider_error_variants() -> Vec<ProviderError> {
    vec![
        ProviderError::Io {
            path: "path".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
        },
        ProviderError::Toml {
            path: "path".to_string(),
            source: toml::from_str::<toml::Value>("=").expect_err("invalid toml"),
        },
        ProviderError::MissingCredential("missing".to_string()),
        ProviderError::NoRoute("none".to_string()),
        ProviderError::Http {
            provider: "openai".to_string(),
            detail: "detail".to_string(),
        },
        ProviderError::Cli {
            provider: "cli".to_string(),
            detail: "detail".to_string(),
        },
        ProviderError::InvalidConfig("bad".to_string()),
    ]
}

fn sandbox_error_variants() -> Vec<SandboxError> {
    vec![
        SandboxError::InvalidBackend("bad".to_string()),
        SandboxError::Unavailable("sandbox".to_string()),
        SandboxError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        )),
        SandboxError::Cancelled,
    ]
}

fn lib_rs_text(rel: &str) -> String {
    fs::read_to_string(workspace_root().join(rel)).expect("read library lib.rs")
}

fn assert_lib_rs_module_declarations_grouped(rel: &str) {
    let text = lib_rs_text(rel);
    let kinds = registry_kinds(&text);
    let mut seen_pub_mod = false;
    let mut seen_pub_use = false;
    for kind in kinds {
        match kind {
            RegistryKind::PrivateMod => {
                assert!(!seen_pub_mod, "{rel} has private mod after pub mod");
                assert!(!seen_pub_use, "{rel} has private mod after pub use");
            }
            RegistryKind::PubMod => {
                seen_pub_mod = true;
                assert!(!seen_pub_use, "{rel} has pub mod after pub use");
            }
            RegistryKind::PubUse => seen_pub_use = true,
        }
    }
    assert_sorted_group(rel, "mod", registry_lines(&text, RegistryKind::PrivateMod));
    assert_sorted_group(rel, "pub mod", registry_lines(&text, RegistryKind::PubMod));
    assert_sorted_group(rel, "pub use", registry_lines(&text, RegistryKind::PubUse));
}

fn assert_lib_rs_pub_use_paths_sorted(rel: &str) {
    let text = lib_rs_text(rel);
    let paths = collect_pub_use_statements(&text);
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(sorted, paths, "{rel} pub use statements are not sorted");
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RegistryKind {
    PrivateMod,
    PubMod,
    PubUse,
}

fn registry_kinds(text: &str) -> Vec<RegistryKind> {
    text.lines().filter_map(registry_kind).collect()
}

fn registry_lines(text: &str, wanted: RegistryKind) -> Vec<String> {
    text.lines()
        .filter(|line| registry_kind(line) == Some(wanted))
        .map(|line| line.trim().to_string())
        .collect()
}

fn registry_kind(line: &str) -> Option<RegistryKind> {
    let line = line.trim();
    if line.starts_with("mod tests") {
        None
    } else if line.starts_with("pub use ") {
        Some(RegistryKind::PubUse)
    } else if line.starts_with("pub mod ") {
        Some(RegistryKind::PubMod)
    } else if line.starts_with("mod ") {
        Some(RegistryKind::PrivateMod)
    } else {
        None
    }
}

fn assert_sorted_group(rel: &str, name: &str, lines: Vec<String>) {
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(sorted, lines, "{rel} {name} declarations are not sorted");
}

fn collect_pub_use_statements(text: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = Vec::new();
    let mut in_statement = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !in_statement && trimmed.starts_with("pub use ") {
            current.push(trimmed.to_string());
            in_statement = true;
        } else if in_statement {
            current.push(trimmed.to_string());
        }
        if in_statement && trimmed.ends_with(';') {
            statements.push(current.join(" "));
            current.clear();
            in_statement = false;
        }
    }
    statements
}

fn assert_cargo_toml_contains(needle: &str) {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    assert!(text.contains(needle), "missing `{needle}`");
}

fn assert_rs_identifier_set_unchanged(root: &PathBuf, commit: &str, file: &str) {
    let before = git_stdout(root, &["show", &format!("{commit}^:{file}")]);
    let after = git_stdout(root, &["show", &format!("{commit}:{file}")]);
    assert_eq!(
        identifier_tokens(&before),
        identifier_tokens(&after),
        "format commit changed Rust identifiers in {file}"
    );
}

fn identifier_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.sort();
    tokens
}

fn git_stdout(root: &PathBuf, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
}

fn internal_metadata_dag(root: &PathBuf) -> Vec<String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let mut edges = Vec::new();
    for package in metadata["packages"].as_array().expect("packages array") {
        let name = package["name"].as_str().expect("package name");
        if !name.starts_with("deadreckon") {
            continue;
        }
        for dependency in package["dependencies"]
            .as_array()
            .expect("dependencies array")
        {
            let dep_name = dependency["name"].as_str().expect("dependency name");
            if dep_name.starts_with("deadreckon") {
                let kind = dependency["kind"].as_str().unwrap_or("normal");
                edges.push(format!("{name} -> {dep_name} [{kind}]"));
            }
        }
    }
    edges.sort();
    edges
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("crates").is_dir() && dir.join("Cargo.toml").is_file() {
            return dir;
        }
        assert!(dir.pop(), "could not locate workspace root");
    }
}
