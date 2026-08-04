use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::{DeadreckonError, is_retryable_io_kind};
use deadreckon_providers::ProviderError;
use deadreckon_sandbox::SandboxError;
use serde_json::Value;

const CRATE_MANIFESTS: &[&str] = &[
    "crates/deadreckon/Cargo.toml",
    "crates/deadreckon-core/Cargo.toml",
    "crates/deadreckon-protocol/Cargo.toml",
    "crates/deadreckon-providers/Cargo.toml",
    "crates/deadreckon-runtime/Cargo.toml",
    "crates/deadreckon-sandbox/Cargo.toml",
];

const LIB_CRATES: &[(&str, &str)] = &[
    ("deadreckon-core", "crates/deadreckon-core/src/lib.rs"),
    (
        "deadreckon-protocol",
        "crates/deadreckon-protocol/src/lib.rs",
    ),
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
    // The dedicated rustfmt commit is repo archaeology; a shallow clone
    // (CI checkouts default to depth 1) cannot see it.
    let shallow = git_stdout(&root, &["rev-parse", "--is-shallow-repository"]);
    if shallow.trim() == "true" {
        eprintln!("skipping format-commit archaeology: shallow clone");
        return;
    }
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
    if std::env::var_os("DEADRECKON_RELEASE_SIZE_CHECK").is_none() {
        eprintln!("skipping binary size check outside `make size-check`");
        return;
    }
    let root = workspace_root();
    // Binary size is format- and platform-specific (Mach-O vs ELF), so each
    // OS pins its own baseline; an OS without one skips.
    let baseline_path = root.join(format!("tests/.size-baseline-{}", std::env::consts::OS));
    let Ok(raw) = fs::read_to_string(&baseline_path) else {
        eprintln!(
            "skipping binary size check: no baseline at {}",
            baseline_path.display()
        );
        return;
    };
    let baseline = raw.trim().parse::<u64>().expect("parse size baseline");
    let binary = root.join("target/release/deadreckon");
    let actual = fs::metadata(&binary)
        .expect("`make size-check` must build the current release binary first")
        .len();
    let allowed = baseline + (baseline / 20);
    assert!(
        actual <= allowed,
        "release binary grew too much: actual={actual}, baseline={baseline}, allowed={allowed}"
    );
}

#[test]
fn verify_checks_size_only_after_building_the_release_binary() {
    let makefile = fs::read_to_string(workspace_root().join("Makefile")).expect("read Makefile");
    assert!(makefile.contains("size-check: build"));
    assert!(makefile.contains("DEADRECKON_RELEASE_SIZE_CHECK=1 cargo test"));
    let verify = makefile
        .split_once("verify:\n")
        .and_then(|(_, tail)| tail.split_once("\nverify-timed:"))
        .map(|(body, _)| body)
        .expect("verify recipe");
    assert!(verify.contains("$(MAKE) test"));
    assert!(verify.contains("$(MAKE) size-check"));
    assert!(
        verify.find("$(MAKE) test") < verify.find("$(MAKE) size-check"),
        "size check must inspect the final release artifact after ordinary tests"
    );
}

#[test]
fn response_only_provider_routes_require_an_enforceable_read_only_boundary() {
    let root = workspace_root();
    for relative in [
        "crates/deadreckon/src/narrator.rs",
        "crates/deadreckon/src/commands/doctor.rs",
        "crates/deadreckon/src/commands/learning.rs",
    ] {
        let source = fs::read_to_string(root.join(relative)).expect("read response-only route");
        assert!(
            source.contains("ProviderRequest::enforceably_read_only("),
            "{relative} does not construct its provider request through the read-only boundary"
        );
        assert!(
            !source.contains("sandbox_backend: None")
                && !source.contains("WorkspaceAccess::ReadWrite"),
            "{relative} can still spawn a response-only provider without containment"
        );
    }

    let main = fs::read_to_string(root.join("crates/deadreckon/src/main.rs")).expect("read main.rs");
    for (start, end, route) in [
        (
            "async fn refresh_plan_docs(",
            "fn manifest_from_plan_doc_input(",
            "plan document polish",
        ),
        (
            "async fn refresh_narrative_projection_with_provider(",
            "fn narrative_refresh_notice(",
            "attach narrative refresh",
        ),
    ] {
        let body = main
            .split_once(start)
            .and_then(|(_, tail)| tail.split_once(end))
            .map(|(body, _)| body)
            .expect("response-only provider function");
        assert!(
            body.contains("ProviderRequest::enforceably_read_only("),
            "{route} does not construct its provider request through the read-only boundary"
        );
        assert!(
            !body.contains("sandbox_backend: None")
                && !body.contains("WorkspaceAccess::ReadWrite")
                && !body.contains("SandboxBackend::None"),
            "{route} can still spawn its provider without containment"
        );
    }
}

#[test]
fn ci_and_release_verification_run_the_fresh_size_gate() {
    let root = workspace_root();
    for relative in [".github/workflows/ci.yml", ".github/workflows/release.yml"] {
        let workflow = fs::read_to_string(root.join(relative)).expect("read workflow");
        assert!(
            workflow.contains("make size-check"),
            "{relative} must measure a freshly built release binary"
        );
    }
}

#[test]
fn internal_crates_listed_in_workspace_dependencies() {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    for (name, path) in [
        ("deadreckon-core", "crates/deadreckon-core"),
        ("deadreckon-protocol", "crates/deadreckon-protocol"),
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
    let lib_rs =
        fs::read_to_string(root.join("crates/deadreckon/src/lib.rs")).expect("read lib.rs");
    assert!(!lib_rs.contains("clippy::print_stdout"));
    assert!(!lib_rs.contains("clippy::print_stderr"));
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
fn is_retryable_io_kind_behaves_identically_across_crates() {
    let cases = [
        (std::io::ErrorKind::Interrupted, true),
        (std::io::ErrorKind::WouldBlock, true),
        (std::io::ErrorKind::TimedOut, true),
        (std::io::ErrorKind::ConnectionReset, true),
        (std::io::ErrorKind::ConnectionAborted, true),
        (std::io::ErrorKind::BrokenPipe, true),
        (std::io::ErrorKind::NotFound, false),
        (std::io::ErrorKind::PermissionDenied, false),
        (std::io::ErrorKind::Other, false),
    ];
    for (kind, expected) in cases {
        assert_eq!(is_retryable_io_kind(kind), expected, "{kind:?}");

        let core_error = DeadreckonError::Io {
            path: "path".into(),
            source: std::io::Error::new(kind, "core"),
        };
        assert_eq!(core_error.is_retryable(), expected, "core {kind:?}");
        assert_eq!(core_error.is_fatal(), !expected, "core {kind:?}");

        let provider_error = ProviderError::Io {
            path: "path".to_string(),
            source: std::io::Error::new(kind, "provider"),
        };
        assert_eq!(
            provider_error.is_retryable(),
            expected,
            "provider {kind:?}"
        );
        assert_eq!(provider_error.is_fatal(), !expected, "provider {kind:?}");

        let sandbox_error = SandboxError::Io(std::io::Error::new(kind, "sandbox"));
        assert_eq!(sandbox_error.is_retryable(), expected, "sandbox {kind:?}");
        assert_eq!(sandbox_error.is_fatal(), !expected, "sandbox {kind:?}");
    }
}

#[test]
fn provider_error_no_route_is_fatal() {
    let error = ProviderError::NoRoute("none".to_string());
    assert!(!error.is_retryable());
    assert!(error.is_fatal());
}

#[test]
fn provider_error_http_retryability_is_explicit_per_construction_site() {
    // The former V1 candidate ("Http has no status field, so it's always
    // fatal") is implemented: each construction site tags transience, and
    // 408/429/5xx/transport failures get one bounded retry in the turn loop.
    let transient = ProviderError::Http {
        provider: "openai".to_string(),
        detail: "HTTP 429: slow down".to_string(),
        retryable: true,
    };
    assert!(transient.is_retryable());
    assert!(!transient.is_fatal());
    let auth = ProviderError::Http {
        provider: "openai".to_string(),
        detail: "HTTP 401: bad key".to_string(),
        retryable: false,
    };
    assert!(!auth.is_retryable());
    assert!(auth.is_fatal());
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
            heartbeat_age_seconds: 5,
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
            retryable: false,
        },
        ProviderError::Cli {
            provider: "cli".to_string(),
            detail: "detail".to_string(),
        },
        ProviderError::Cancelled {
            provider: "cli".to_string(),
            detail: "cancelled".to_string(),
        },
        ProviderError::CleanupIncomplete {
            provider: "cli".to_string(),
            authority: Some("provider.pid".into()),
            detail: "retained authority".to_string(),
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
