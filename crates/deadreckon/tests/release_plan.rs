#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use toml::Value;

const DIST_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];

#[test]
fn dist_plan_lists_all_five_targets() {
    let dist = dist_config();
    let actual = string_array(&dist, "targets")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected = DIST_TARGETS
        .iter()
        .map(|target| (*target).to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, actual);
    assert_dist_plan_json_if_installed();
}

#[test]
fn dist_plan_pins_linux_glibc_2_28() {
    let dist = dist_config();
    let glibc = dist
        .get("min-glibc-version")
        .and_then(Value::as_table)
        .expect("min-glibc-version table");
    for target in ["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"] {
        assert_eq!(
            Some("2.28"),
            glibc.get(target).and_then(Value::as_str),
            "{target} must pin glibc 2.28"
        );
    }
    assert_dist_plan_json_if_installed();
}

#[test]
fn dist_plan_excludes_bundled_npm_installer() {
    let dist = dist_config();
    let installers = string_array(&dist, "installers")
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ["powershell".to_string(), "shell".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        installers
    );
    assert!(
        dist.get("npm-package").is_none(),
        "P8 owns the npm wrapper; dist's bundled npm installer must stay off"
    );
    assert_dist_plan_json_if_installed();
}

#[test]
fn release_workflow_runs_dist_plan_on_every_push() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(workflow.contains("push:"), "{workflow}");
    assert!(
        workflow.contains("dist plan --output-format=json"),
        "{workflow}"
    );
    assert!(
        workflow.contains("cargo-dist/releases/download/v0.31.0/cargo-dist-installer.sh"),
        "{workflow}"
    );
}

#[test]
fn release_workflow_codesigns_only_on_apple_targets() {
    let workflow = release_workflow();
    let codesign_step = workflow
        .split("- name: Codesign and notarize macOS binary")
        .nth(1)
        .expect("codesign step");
    assert!(
        codesign_step.contains("if: contains(matrix.target, 'apple-darwin')"),
        "{codesign_step}"
    );
    assert!(codesign_step.contains("codesign --sign"), "{codesign_step}");
    assert!(
        codesign_step.contains("xcrun notarytool submit"),
        "{codesign_step}"
    );
}

#[test]
fn release_workflow_skips_codesign_without_cert_secret() {
    let workflow = release_workflow();
    let codesign_step = workflow
        .split("- name: Codesign and notarize macOS binary")
        .nth(1)
        .expect("codesign step");
    assert!(codesign_step.contains("APPLE_CERT_P12"), "{codesign_step}");
    assert!(
        codesign_step.contains("skipping macOS codesign/notarization"),
        "{codesign_step}"
    );
    assert!(codesign_step.contains("exit 0"), "{codesign_step}");
}

#[test]
fn release_doc_lists_all_five_apple_secrets() {
    let doc =
        fs::read_to_string(workspace_root().join("docs/RELEASE.md")).expect("read docs/RELEASE.md");
    for secret in [
        "APPLE_CERT_P12",
        "APPLE_CERT_PWD",
        "APPLE_ID",
        "APPLE_TEAM_ID",
        "APPLE_APP_PWD",
    ] {
        assert!(
            doc.contains(secret),
            "{secret} missing from docs/RELEASE.md"
        );
    }
}

fn dist_config() -> toml::value::Table {
    let path = workspace_root().join("dist-workspace.toml");
    let text = fs::read_to_string(&path).expect("read dist-workspace.toml");
    text.parse::<toml::Table>()
        .expect("parse dist-workspace.toml")
        .get("dist")
        .and_then(Value::as_table)
        .expect("[dist] table")
        .clone()
}

fn string_array(table: &toml::value::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} must be an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{key} must contain strings"))
                .to_string()
        })
        .collect()
}

fn release_workflow() -> String {
    fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow")
}

fn assert_dist_plan_json_if_installed() {
    let available = Command::new("cargo")
        .args(["dist", "--version"])
        .current_dir(workspace_root())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !available {
        return;
    }

    let output = Command::new("cargo")
        .args(["dist", "plan", "--output-format=json"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo dist plan");
    assert!(
        output.status.success(),
        "cargo dist plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("dist plan json");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
