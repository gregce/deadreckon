#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value as JsonValue;
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
        [
            "homebrew".to_string(),
            "powershell".to_string(),
            "shell".to_string()
        ]
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
fn homebrew_formula_writes_install_receipt() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let formula = temp.path().join("deadreckon.rb");
    fs::write(
        &formula,
        r#"class Deadreckon < Formula
  version "9.8.7"
  url "https://github.com/gregce/deadreckon/releases/download/v9.8.7/deadreckon.tar.xz"
  sha256 "abc123"

  def install_binary_aliases!
  end

  def install
    bin.install "deadreckon"
    install_binary_aliases!
  end
end
"#,
    )
    .expect("write formula");
    let output = Command::new("node")
        .arg(workspace_root().join("release/homebrew/patch-formula.mjs"))
        .arg(&formula)
        .output()
        .expect("patch formula");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let patched = fs::read_to_string(&formula).expect("patched formula");
    assert!(patched.contains("install-receipt.json"), "{patched}");
    assert!(patched.contains(r#""channel" => "brew""#), "{patched}");
    assert!(
        patched.contains(r#""install_source" => "brew:gdc/tap/deadreckon""#),
        "{patched}"
    );
    assert!(patched.contains("write_deadreckon_receipt!"), "{patched}");
}

#[test]
fn homebrew_formula_pins_release_sha256() {
    let dist = dist_config();
    assert_eq!(
        Some("gdc/homebrew-tap"),
        dist.get("tap").and_then(Value::as_str)
    );
    let publish_jobs = string_array(&dist, "publish-jobs");
    assert_eq!(vec!["homebrew".to_string()], publish_jobs);

    let workflow = release_workflow();
    assert!(
        workflow.contains("node release/homebrew/patch-formula.mjs target/distrib"),
        "{workflow}"
    );
    assert!(
        workflow.contains("repository: gdc/homebrew-tap"),
        "{workflow}"
    );
    assert!(workflow.contains("HOMEBREW_TAP_TOKEN"), "{workflow}");

    let temp = tempfile::TempDir::new().expect("tempdir");
    let formula = temp.path().join("deadreckon.rb");
    fs::write(
        &formula,
        r#"class Deadreckon < Formula
  version "9.8.7"
  url "https://github.com/gregce/deadreckon/releases/download/v9.8.7/deadreckon.tar.xz"
  sha256 "abc123"

  def install_binary_aliases!
  end

  def install
    bin.install "deadreckon"
    install_binary_aliases!
  end
end
"#,
    )
    .expect("write formula");
    let output = Command::new("node")
        .arg(workspace_root().join("release/homebrew/patch-formula.mjs"))
        .arg(&formula)
        .output()
        .expect("patch formula");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let template = fs::read_to_string(&formula).expect("patched formula");
    assert!(
        template.contains(r#"sha256 "abc123""#),
        "formula patching must preserve cargo-dist release archive sha256s"
    );
}

#[test]
fn release_lane_classifies_branch_rc_and_stable_tags() {
    let branch = release_trust_json([
        "lane",
        "--ref",
        "refs/heads/main",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert_eq!(
        Some("branch"),
        branch.get("lane").and_then(JsonValue::as_str)
    );
    assert_eq!(
        Some(false),
        branch.get("build_artifacts").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(false),
        branch.get("publishes").and_then(JsonValue::as_bool)
    );

    let rc = release_trust_json([
        "lane",
        "--ref",
        "refs/tags/v1.2.3-rc.4",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert_eq!(Some("rc"), rc.get("lane").and_then(JsonValue::as_str));
    assert_eq!(
        Some("v1.2.3-rc.4"),
        rc.get("tag").and_then(JsonValue::as_str)
    );
    assert_eq!(
        Some("1.2.3-rc.4"),
        rc.get("version").and_then(JsonValue::as_str)
    );
    assert_eq!(
        Some(true),
        rc.get("build_artifacts").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(true),
        rc.get("publish_github_release")
            .and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(false),
        rc.get("publish_homebrew").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(false),
        rc.get("publish_npm").and_then(JsonValue::as_bool)
    );

    let stable = release_trust_json([
        "lane",
        "--ref",
        "refs/tags/v1.2.3",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert_eq!(
        Some("stable"),
        stable.get("lane").and_then(JsonValue::as_str)
    );
    assert_eq!(
        Some(true),
        stable.get("publish_homebrew").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(true),
        stable.get("publish_npm").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(true),
        stable
            .get("requires_macos_signing")
            .and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(true),
        stable
            .get("requires_attestation")
            .and_then(JsonValue::as_bool)
    );
}

#[test]
fn official_release_requires_trust_material() {
    let output = release_trust([
        "preflight",
        "--ref",
        "refs/tags/v1.2.3",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert!(
        !output.status.success(),
        "preflight must fail without official trust material"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in [
        "APPLE_CERT_P12",
        "APPLE_CERT_PWD",
        "APPLE_ID",
        "APPLE_TEAM_ID",
        "APPLE_APP_PWD",
        "HOMEBREW_TAP_TOKEN",
        "npm trusted publishing or NPM_TOKEN",
        "WINDOWS_CERT_PFX",
        "WINDOWS_CERT_PWD",
    ] {
        assert!(
            stderr.contains(required),
            "{required} missing from {stderr}"
        );
    }
}

#[test]
fn fork_release_plan_does_not_require_private_secrets() {
    let output = release_trust([
        "preflight",
        "--ref",
        "refs/tags/v1.2.3",
        "--repo",
        "someone/deadreckon",
    ]);
    assert!(
        output.status.success(),
        "fork tags are non-publishing dry-runs\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: JsonValue = serde_json::from_slice(&output.stdout).expect("preflight json");
    assert_eq!(
        Some(false),
        value.get("official_repo").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(false),
        value.get("publishes").and_then(JsonValue::as_bool)
    );
}

#[test]
fn invalid_tag_never_publishes() {
    let value = release_trust_json([
        "lane",
        "--ref",
        "refs/tags/release-1.2.3",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert_eq!(
        Some("invalid_tag"),
        value.get("lane").and_then(JsonValue::as_str)
    );
    assert_eq!(
        Some(false),
        value.get("build_artifacts").and_then(JsonValue::as_bool)
    );
    assert_eq!(
        Some(false),
        value.get("publishes").and_then(JsonValue::as_bool)
    );

    let output = release_trust([
        "validate",
        "--ref",
        "refs/tags/release-1.2.3",
        "--repo",
        "gregce/deadreckon",
    ]);
    assert!(
        !output.status.success(),
        "invalid tags must fail validation"
    );
}

#[test]
fn stable_tag_must_match_workspace_version() {
    let output = release_trust([
        "validate",
        "--ref",
        "refs/tags/v99.99.99",
        "--repo",
        "gregce/deadreckon",
        "--skip-changelog",
    ]);
    assert!(
        !output.status.success(),
        "stable tag must match workspace version"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("workspace version"), "{stderr}");
}

#[test]
fn stable_tag_requires_changelog_entry() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let changelog = temp.path().join("CHANGELOG.md");
    fs::write(&changelog, "# Changelog\n\n## Not This Release\n").expect("fixture changelog");
    let output = release_trust([
        "validate",
        "--ref",
        "refs/tags/v0.1.0",
        "--repo",
        "gregce/deadreckon",
        "--changelog",
        changelog.to_str().expect("utf8 path"),
    ]);
    assert!(
        !output.status.success(),
        "stable tag must have a changelog section"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CHANGELOG.md"), "{stderr}");
}

#[test]
fn release_manifest_covers_artifacts_and_checksums() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    fs::create_dir_all(&distrib).expect("distrib dir");
    fs::write(
        distrib.join("deadreckon-aarch64-apple-darwin.tar.xz"),
        b"mac archive",
    )
    .expect("mac artifact");
    fs::write(
        distrib.join("deadreckon-x86_64-unknown-linux-gnu.tar.xz"),
        b"linux archive",
    )
    .expect("linux artifact");
    fs::create_dir_all(distrib.join("trust")).expect("trust dir");
    fs::write(
        distrib.join("trust/macos-aarch64-apple-darwin.json"),
        r#"{"target":"aarch64-apple-darwin","signed":true,"notarized":true}"#,
    )
    .expect("mac trust");

    assert_release_trust_success([
        "sbom",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        distrib
            .join("release.spdx.json")
            .to_str()
            .expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        distrib.join("SHA256SUMS").to_str().expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--ref",
        "refs/tags/v1.2.3-rc.1",
        "--repo",
        "gregce/deadreckon",
        "--commit",
        "abc123",
        "--out",
        distrib
            .join("release-manifest.json")
            .to_str()
            .expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--manifest",
        distrib
            .join("release-manifest.json")
            .to_str()
            .expect("utf8 path"),
        "--checksums",
        distrib.join("SHA256SUMS").to_str().expect("utf8 path"),
    ]);

    let manifest = read_json(&distrib.join("release-manifest.json"));
    assert_eq!(
        Some(1),
        manifest.get("schema_version").and_then(JsonValue::as_u64)
    );
    assert_eq!(Some("rc"), manifest.get("lane").and_then(JsonValue::as_str));
    let artifacts = manifest
        .get("artifacts")
        .and_then(JsonValue::as_array)
        .expect("artifacts array");
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.get("name").and_then(JsonValue::as_str)
                == Some("deadreckon-aarch64-apple-darwin.tar.xz")
                && artifact.get("signed").and_then(JsonValue::as_bool) == Some(true)
                && artifact.get("notarized").and_then(JsonValue::as_bool) == Some(true)),
        "{manifest:#?}"
    );
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.get("name").and_then(JsonValue::as_str) == Some("SHA256SUMS")),
        "{manifest:#?}"
    );
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.get("name").and_then(JsonValue::as_str)
                == Some("release.spdx.json")),
        "{manifest:#?}"
    );
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
fn release_workflow_verification_matches_release_trust_contract() {
    let workflow = release_workflow();
    let verify_step = workflow
        .split("- name: Full release verification")
        .nth(1)
        .expect("release verification step");
    assert!(verify_step.contains("cargo fmt --check"), "{verify_step}");
    assert!(
        verify_step.contains("cargo test --workspace"),
        "{verify_step}"
    );
    assert!(
        !verify_step.contains("cargo clippy --workspace"),
        "release trust verification must not import unrelated workspace clippy debt"
    );
}

#[test]
fn release_workflow_permissions_are_lane_scoped() {
    let workflow = release_workflow();
    let top_permissions = workflow
        .split("on:")
        .next()
        .expect("workflow header permissions");
    assert!(top_permissions.contains("permissions:\n  contents: read"));
    assert!(
        !top_permissions.contains("contents: write"),
        "non-publishing jobs should not inherit contents write"
    );
    assert!(
        !top_permissions.contains("id-token: write"),
        "non-attestation jobs should not inherit OIDC minting"
    );

    let attestation_job = workflow
        .split("attest-release-artifacts:")
        .nth(1)
        .expect("attestation job");
    assert!(
        attestation_job.contains("id-token: write"),
        "{attestation_job}"
    );
    assert!(
        attestation_job.contains("attestations: write"),
        "{attestation_job}"
    );
    assert!(
        attestation_job.contains("artifact-metadata: write"),
        "{attestation_job}"
    );

    let github_release_job = workflow
        .split("publish-github-release:")
        .nth(1)
        .expect("github release job");
    assert!(
        github_release_job.contains("permissions:\n      contents: write"),
        "{github_release_job}"
    );
}

#[test]
fn release_workflow_codesigns_packaged_apple_artifacts_after_dist_build() {
    let workflow = release_workflow();
    let build_step = workflow
        .find("- name: Build target artifacts")
        .expect("build target artifacts step");
    let codesign_step = workflow
        .find("- name: Sign and verify packaged macOS artifacts")
        .expect("packaged macOS signing step");
    assert!(
        build_step < codesign_step,
        "macOS signing must happen after dist build so the uploaded archive is proven"
    );
    let codesign_step = workflow
        .split("- name: Sign and verify packaged macOS artifacts")
        .nth(1)
        .expect("codesign step body");
    assert!(
        codesign_step.contains("if: contains(matrix.target, 'apple-darwin')"),
        "{codesign_step}"
    );
    assert!(
        codesign_step.contains("release/trust/sign-macos-artifacts.mjs"),
        "{codesign_step}"
    );
    assert!(
        codesign_step.contains("codesign --verify --verbose"),
        "{codesign_step}"
    );
    assert!(
        codesign_step.contains("xcrun notarytool submit"),
        "{codesign_step}"
    );
}

#[test]
fn release_workflow_fails_closed_for_official_missing_macos_signing() {
    let workflow = release_workflow();
    assert!(
        workflow.contains("node release/trust/release-trust.mjs preflight"),
        "{workflow}"
    );
    assert!(workflow.contains("APPLE_CERT_P12"), "{workflow}");
    assert!(
        !workflow.contains("skipping macOS codesign/notarization"),
        "official release signing must fail closed, not warn-and-skip"
    );
    assert!(
        workflow.contains("requires_macos_signing"),
        "workflow should use release lane metadata for signing policy"
    );
}

#[test]
fn release_workflow_generates_trust_artifacts_and_attestations() {
    let workflow = release_workflow();
    for needle in [
        "release-trust-artifacts:",
        "node release/trust/release-trust.mjs sbom",
        "node release/trust/release-trust.mjs checksums",
        "node release/trust/release-trust.mjs manifest",
        "node release/trust/release-trust.mjs verify-manifest",
        "uses: actions/attest@v4",
        "id-token: write",
        "attestations: write",
        "artifact-metadata: write",
        "release-manifest.json",
        "SHA256SUMS",
        "release.spdx.json",
        "gh release upload",
        "gh release edit",
        "gh attestation verify <artifact> --repo gregce/deadreckon",
    ] {
        assert!(workflow.contains(needle), "{needle} missing from workflow");
    }
    assert!(
        workflow.contains("pattern: dist-local-*"),
        "trust bundle must not hash dist-plan artifacts"
    );
    assert!(
        workflow.contains("name: dist-global"),
        "trust bundle should download global release artifacts explicitly"
    );
}

#[test]
fn stable_windows_artifact_requires_signing_or_is_withheld() {
    let workflow = release_workflow();
    assert!(
        workflow.contains("requires_windows_signing"),
        "stable Windows signing policy must be lane-aware"
    );
    assert!(
        workflow.contains("Windows stable artifacts require a signing provider"),
        "{workflow}"
    );
    assert!(workflow.contains("WINDOWS_CERT_PFX"), "{workflow}");
    assert!(workflow.contains("WINDOWS_CERT_PWD"), "{workflow}");
    assert!(
        workflow.contains("release/trust/sign-windows-artifacts.mjs"),
        "{workflow}"
    );
    let signer =
        fs::read_to_string(workspace_root().join("release/trust/sign-windows-artifacts.mjs"))
            .expect("windows signer script");
    assert!(signer.contains("signtool"), "{signer}");
}

#[test]
fn rc_release_does_not_publish_stable_package_managers() {
    let workflow = release_workflow();
    let homebrew = workflow
        .split("publish-homebrew-formula:")
        .nth(1)
        .expect("homebrew job");
    assert!(
        homebrew.contains("needs.release-policy.outputs.publish_homebrew == 'true'"),
        "{homebrew}"
    );
    let npm = workflow.split("publish-npm:").last().expect("npm job");
    assert!(
        npm.contains("needs.release-policy.outputs.publish_npm == 'true'"),
        "{npm}"
    );
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

fn read_json(path: &Path) -> JsonValue {
    serde_json::from_slice(
        &fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display())),
    )
    .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn release_trust<const N: usize>(args: [&str; N]) -> std::process::Output {
    Command::new("node")
        .arg(workspace_root().join("release/trust/release-trust.mjs"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("run release-trust")
}

fn release_trust_json<const N: usize>(args: [&str; N]) -> JsonValue {
    let output = release_trust(args);
    assert!(
        output.status.success(),
        "release-trust failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("release-trust json")
}

fn assert_release_trust_success<const N: usize>(args: [&str; N]) {
    let output = release_trust(args);
    assert!(
        output.status.success(),
        "release-trust failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
