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
fn dist_plan_packages_only_production_binaries() {
    let dist = dist_config();
    let binaries = dist
        .get("binaries")
        .and_then(Value::as_table)
        .expect("binaries table");
    let expected = ["deadreckon", "dr-capture", "dr-gate"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    for target in DIST_TARGETS {
        let actual = string_array(binaries, target)
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            expected, actual,
            "{target} must package the three production binaries and exclude the internal characterization harness"
        );
    }
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
    bin.install "deadreckon", "dr-gate", "dr-capture"
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
        patched.contains(r#""install_source" => "brew:gregce/tap/deadreckon""#),
        "{patched}"
    );
    assert!(patched.contains("write_deadreckon_receipt!"), "{patched}");
    for binary in [
        "deadreckon",
        "dr-gate",
        "dr-capture",
        "dr-gate-evaluator-aarch64-unknown-linux-musl",
        "dr-gate-evaluator-x86_64-unknown-linux-musl",
    ] {
        assert!(patched.contains(&format!("\"{binary}\"")), "{patched}");
    }
}

#[test]
fn homebrew_formula_pins_release_sha256() {
    let dist = dist_config();
    assert_eq!(
        Some("gregce/homebrew-tap"),
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
        workflow.contains("repository: gregce/homebrew-tap"),
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
    bin.install "deadreckon", "dr-gate", "dr-capture"
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
fn homebrew_formula_installs_complete_runtime_on_every_supported_platform() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let formula = temp.path().join("deadreckon.rb");
    let install_blocks = [
        ("OS.mac? && Hardware::CPU.arm?", "aarch64-apple-darwin"),
        ("OS.mac? && Hardware::CPU.intel?", "x86_64-apple-darwin"),
        (
            "OS.linux? && Hardware::CPU.arm?",
            "aarch64-unknown-linux-gnu",
        ),
        (
            "OS.linux? && Hardware::CPU.intel?",
            "x86_64-unknown-linux-gnu",
        ),
    ];
    let mut fixture = String::from(
        "class Deadreckon < Formula\n  version \"9.8.7\"\n\n  def install_binary_aliases!\n  end\n\n  def install\n",
    );
    for (condition, _) in install_blocks {
        fixture.push_str(&format!(
            "    if {condition}\n      bin.install \"deadreckon\", \"dr-gate\", \"dr-capture\"\n    end\n"
        ));
    }
    fixture.push_str("    install_binary_aliases!\n  end\nend\n");
    fs::write(&formula, fixture).expect("formula");

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
    for evaluator in [
        "dr-gate-evaluator-aarch64-unknown-linux-musl",
        "dr-gate-evaluator-x86_64-unknown-linux-musl",
    ] {
        assert_eq!(
            4,
            patched.matches(&format!("\"{evaluator}\"")).count(),
            "{patched}"
        );
    }
    for (condition, _) in install_blocks {
        let block = patched
            .split(&format!("if {condition}"))
            .nth(1)
            .expect("platform install block")
            .split("end")
            .next()
            .expect("platform block end");
        for binary in [
            "deadreckon",
            "dr-gate",
            "dr-capture",
            "dr-gate-evaluator-aarch64-unknown-linux-musl",
            "dr-gate-evaluator-x86_64-unknown-linux-musl",
        ] {
            assert!(block.contains(&format!("\"{binary}\"")), "{block}");
        }
    }
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
        Some(false),
        stable.get("publish_npm").and_then(JsonValue::as_bool),
        "npm publishing is consciously deferred until trusted publishing is configured"
    );
    assert_eq!(
        Some(false),
        stable
            .get("requires_windows_signing")
            .and_then(JsonValue::as_bool),
        "Windows Authenticode is consciously deferred until a certificate exists"
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
    ] {
        assert!(
            stderr.contains(required),
            "{required} missing from {stderr}"
        );
    }
    for deferred in [
        "npm trusted publishing or NPM_TOKEN",
        "WINDOWS_CERT_PFX",
        "WINDOWS_CERT_PWD",
    ] {
        assert!(
            !stderr.contains(deferred),
            "{deferred} should be deferred from the narrowed v0.8.0 lane: {stderr}"
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
fn stable_validate_requires_changelog_section_and_rc_does_not() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let changelog = temp.path().join("CHANGELOG.md");
    fs::write(&changelog, "# Changelog\n\n## Some Other Release\n").expect("fixture changelog");
    let changelog_arg = changelog.to_str().expect("utf8 path");

    let stable = release_trust([
        "validate",
        "--ref",
        "refs/tags/v0.1.0",
        "--repo",
        "gregce/deadreckon",
        "--changelog",
        changelog_arg,
    ]);
    assert!(
        !stable.status.success(),
        "stable lane must gate on the changelog"
    );
    let stderr = String::from_utf8_lossy(&stable.stderr);
    assert!(stderr.contains("CHANGELOG.md"), "{stderr}");

    let workspace = workspace_version_string();
    let rc_version = if workspace.contains("-rc.") {
        workspace.clone()
    } else {
        format!("{workspace}-rc.999")
    };
    let rc_tag = format!("refs/tags/v{rc_version}");
    let rc = release_trust([
        "validate",
        "--ref",
        rc_tag.as_str(),
        "--repo",
        "gregce/deadreckon",
        "--changelog",
        changelog_arg,
    ]);
    let rc_stderr = String::from_utf8_lossy(&rc.stderr);
    if rc_version == workspace {
        assert!(
            rc.status.success(),
            "rc lane must not require a changelog section: {rc_stderr}"
        );
    } else {
        assert!(
            !rc_stderr.contains("CHANGELOG.md"),
            "rc lane must not require a changelog section: {rc_stderr}"
        );
    }
}

#[test]
fn stable_validate_requires_npm_wrapper_version_match() {
    // Use a version that will never be the shipped one, so the npm-wrapper
    // mismatch gate fires regardless of the workspace's current version (a real
    // tag matching the bumped version would make validate succeed).
    let stable = release_trust([
        "validate",
        "--ref",
        "refs/tags/v9.9.9",
        "--repo",
        "gregce/deadreckon",
        "--skip-changelog",
    ]);
    assert!(!stable.status.success());
    let stderr = String::from_utf8_lossy(&stable.stderr);
    assert!(stderr.contains("npm wrapper version"), "{stderr}");

    let rc = release_trust([
        "validate",
        "--ref",
        "refs/tags/v9.9.9-rc.1",
        "--repo",
        "gregce/deadreckon",
        "--skip-changelog",
    ]);
    let rc_stderr = String::from_utf8_lossy(&rc.stderr);
    assert!(
        !rc_stderr.contains("npm wrapper version"),
        "the npm wrapper gate must fire on the stable lane only: {rc_stderr}"
    );
}

#[test]
fn installer_artifact_checksum_verification_is_documented_or_embedded() {
    let install = fs::read_to_string(workspace_root().join("release/install.sh"))
        .expect("release/install.sh");
    assert!(
        install.contains("SHA256SUMS") && install.contains("checksum verification failed"),
        "the wrapper installer must verify artifacts against SHA256SUMS and die on mismatch"
    );

    let dist = dist_config();
    let embedded = dist
        .get("checksum")
        .and_then(Value::as_str)
        .is_some_and(|algo| algo == "sha256");
    let candidates = fs::read_to_string(workspace_root().join("docs/V1-CANDIDATES.md"))
        .expect("docs/V1-CANDIDATES.md");
    let documented_fallback = candidates.contains("embedded checksum");
    assert!(
        embedded && documented_fallback,
        "pin the checksum algorithm in dist-workspace.toml and document the \
         inner-installer embedded-checksum upgrade path in V1-CANDIDATES"
    );
}

#[test]
fn preflight_real_script_refuses_under_ci_env() {
    let script = workspace_root().join("release/preflight-real.sh");
    let output = Command::new("sh")
        .arg(&script)
        .env("CI", "1")
        .current_dir(workspace_root())
        .output()
        .expect("run preflight-real.sh");
    assert!(
        !output.status.success(),
        "preflight-real.sh must refuse to run under CI"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("real providers") && stderr.contains("CI"),
        "{stderr}"
    );
}

#[test]
fn preflight_real_proves_execution_routes_against_a_frozen_falsifiable_contract() {
    let script = fs::read_to_string(workspace_root().join("release/preflight-real.sh"))
        .expect("release/preflight-real.sh");
    for required in [
        "write_fixture_contract",
        "commit_fixture_contract",
        "deadreckon_bin\" def-done show",
        "deadreckon_bin\" def-done check",
        "preflight contract passed before provider work",
        "output=$(sh purpose.sh); test",
        "wait_for_verified_job",
        "deadreckon_bin\" run \"$goal\"",
        "deadreckon_bin\" finish \"$job_id\"",
        "verified receipt delivered",
        "wait_for_provider_pid",
        "assert_job_cancelled",
        "assert_process_reaped",
        "--escalate",
        "cancel/reap",
    ] {
        assert!(
            script.contains(required),
            "missing {required} from {script}"
        );
    }
    assert!(
        !script.contains("deadreckon_bin\" def-done \\\n"),
        "real execution-route proof must not depend on a separate LLM-authored fixture contract"
    );
    assert!(
        !script.contains("python3 -c"),
        "the hardened macOS gate sandbox must not depend on xcrun-backed Python discovery"
    );
    assert!(
        !script.contains("deadreckon_bin\" start"),
        "an isolated provider preflight must not claim the machine-restart service used by ordinary start"
    );
    assert!(
        !script.contains("deadreckon_bin\" resume"),
        "public resume is retired for Job-owned runs; recovery belongs to supervisor acceptance"
    );
}

#[test]
fn known_good_providers_schema_round_trips() {
    let fixture = serde_json::json!({
        "schema_version": 1,
        "recorded_at": "2026-06-10T00:00:00Z",
        "providers": [
            {
                "route": "cli:claude-code",
                "binary_version": "2.1.172 (Claude Code)",
                "proof": "start -> 2+ real turns -> gate signed -> apply -> kill/resume",
                "run_id": "abc123",
                "operator": "greg"
            }
        ]
    });
    let text = serde_json::to_string_pretty(&fixture).expect("serialize");
    let parsed: JsonValue = serde_json::from_str(&text).expect("parse");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["providers"][0]["route"], "cli:claude-code");

    if let Ok(committed) =
        fs::read_to_string(workspace_root().join("release/known-good-providers.json"))
    {
        let value: JsonValue = serde_json::from_str(&committed).expect("committed file parses");
        assert_eq!(
            value["schema_version"], 1,
            "release/known-good-providers.json must stay on schema_version 1"
        );
        assert!(value["providers"].is_array(), "{value}");
    }
}

fn workspace_version_string() -> String {
    let manifest = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("Cargo.toml");
    let value: toml::Table = manifest.parse().expect("workspace toml");
    value["workspace"]["package"]["version"]
        .as_str()
        .expect("workspace version")
        .to_string()
}

#[test]
fn release_manifest_covers_artifacts_and_checksums() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    fs::create_dir_all(&distrib).expect("distrib dir");
    // Real archives: verify-manifest fails closed on unreadable or
    // "./"-prefixed archives, so placeholder bytes are not valid fixtures.
    for stem in [
        "deadreckon-aarch64-apple-darwin",
        "deadreckon-x86_64-unknown-linux-gnu",
    ] {
        let payload = temp.path().join("payload").join(stem);
        fs::create_dir_all(&payload).expect("payload");
        fs::write(payload.join("deadreckon"), stem).expect("binary");
        let output = Command::new("tar")
            .args([
                "-cJf",
                distrib
                    .join(format!("{stem}.tar.xz"))
                    .to_str()
                    .expect("utf8 path"),
                "-C",
                temp.path().join("payload").to_str().expect("utf8 path"),
                stem,
            ])
            .output()
            .expect("tar");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::create_dir_all(distrib.join("trust")).expect("trust dir");
    fs::write(
        distrib.join("release-archive-members.json"),
        r#"{"schema_version":1,"evaluators":[],"archives":[]}"#,
    )
    .expect("archive member manifest");
    fs::write(
        distrib.join("trust/macos-aarch64-apple-darwin.json"),
        r#"{"target":"aarch64-apple-darwin","signed":true,"signature_kind":"Developer ID Application","notarized":true}"#,
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
    let unsigned_linux = artifacts
        .iter()
        .find(|artifact| {
            artifact.get("name").and_then(JsonValue::as_str)
                == Some("deadreckon-x86_64-unknown-linux-gnu.tar.xz")
        })
        .expect("unsigned Linux archive");
    assert_eq!(Some(false), unsigned_linux["signed"].as_bool());
    assert_eq!(Some(false), unsigned_linux["notarized"].as_bool());
    assert!(unsigned_linux["signature_kind"].is_null());
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
    assert!(
        artifacts.iter().any(|artifact| {
            artifact.get("name").and_then(JsonValue::as_str) == Some("release-archive-members.json")
        }),
        "{manifest:#?}"
    );
}

#[test]
fn nested_ci_trust_status_survives_flat_public_verification() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    let nested_distrib = distrib.join("target/distrib");
    let payload_name = "deadreckon-aarch64-apple-darwin";
    let payload = temp.path().join("payload").join(payload_name);
    fs::create_dir_all(&payload).expect("payload");
    fs::create_dir_all(nested_distrib.join("trust")).expect("nested trust");
    fs::write(payload.join("deadreckon"), "native binary").expect("binary");
    let archive = nested_distrib.join(format!("{payload_name}.tar.xz"));
    let output = Command::new("tar")
        .args(["-cJf"])
        .arg(&archive)
        .arg("-C")
        .arg(temp.path().join("payload"))
        .arg(payload_name)
        .output()
        .expect("tar");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let trust_name = "macos-aarch64-apple-darwin.json";
    let trust = r#"{
  "target": "aarch64-apple-darwin",
  "signed": true,
  "signature_kind": "Developer ID Application",
  "notarized": true
}
"#;
    fs::write(nested_distrib.join("trust").join(trust_name), trust).expect("nested trust status");
    let windows_trust_name = "windows-x86_64-pc-windows-msvc.json";
    let windows_trust = r#"{"target":"x86_64-pc-windows-msvc","signed":true,"notarized":false}"#;
    fs::write(
        nested_distrib.join("trust").join(windows_trust_name),
        windows_trust,
    )
    .expect("nested Windows trust status");
    let duplicate_trust = distrib.join("dist-global/target/distrib/trust");
    fs::create_dir_all(&duplicate_trust).expect("duplicate trust directory");
    fs::write(duplicate_trust.join(trust_name), trust).expect("identical duplicate trust status");

    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--out",
        distrib.join("SHA256SUMS").to_str().expect("checksums path"),
    ]);
    assert_release_trust_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--ref",
        "refs/tags/v0.8.0",
        "--repo",
        "gregce/deadreckon",
        "--commit",
        "abc123",
        "--out",
        distrib
            .join("release-manifest.json")
            .to_str()
            .expect("manifest path"),
    ]);

    let manifest = read_json(&distrib.join("release-manifest.json"));
    let archive_record = manifest["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .find(|artifact| artifact["name"] == format!("{payload_name}.tar.xz"))
        .expect("macOS archive record");
    assert_eq!(Some(true), archive_record["signed"].as_bool());
    assert_eq!(Some(true), archive_record["notarized"].as_bool());
    assert_eq!(
        Some("Developer ID Application"),
        archive_record["signature_kind"].as_str()
    );
    assert!(
        manifest["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .any(|artifact| {
                artifact["name"] == windows_trust_name && artifact["kind"] == "trust-status"
            }),
        "{manifest:#?}"
    );

    let public = temp.path().join("public-v0.8.0");
    fs::create_dir_all(&public).expect("public download");
    fs::copy(&archive, public.join(format!("{payload_name}.tar.xz"))).expect("flatten archive");
    fs::copy(
        nested_distrib.join("trust").join(trust_name),
        public.join(trust_name),
    )
    .expect("flatten trust status");
    fs::copy(
        nested_distrib.join("trust").join(windows_trust_name),
        public.join(windows_trust_name),
    )
    .expect("flatten Windows trust status");
    fs::copy(distrib.join("SHA256SUMS"), public.join("SHA256SUMS")).expect("flatten checksums");
    fs::copy(
        distrib.join("release-manifest.json"),
        public.join("release-manifest.json"),
    )
    .expect("flatten manifest");

    assert_release_trust_success([
        "verify-manifest",
        "--dir",
        public.to_str().expect("public path"),
        "--manifest",
        public
            .join("release-manifest.json")
            .to_str()
            .expect("public manifest"),
        "--checksums",
        public
            .join("SHA256SUMS")
            .to_str()
            .expect("public checksums"),
    ]);

    let correct_manifest =
        fs::read(public.join("release-manifest.json")).expect("correct manifest");
    for (field, false_value) in [
        ("signed", JsonValue::Bool(false)),
        ("notarized", JsonValue::Bool(false)),
        ("signature_kind", JsonValue::Null),
    ] {
        let mut false_manifest: JsonValue =
            serde_json::from_slice(&correct_manifest).expect("parse correct manifest");
        let false_archive = false_manifest["artifacts"]
            .as_array_mut()
            .expect("artifacts")
            .iter_mut()
            .find(|artifact| artifact["name"] == format!("{payload_name}.tar.xz"))
            .expect("macOS archive record");
        false_archive[field] = false_value;
        fs::write(
            public.join("release-manifest.json"),
            serde_json::to_vec_pretty(&false_manifest).expect("encode false manifest"),
        )
        .expect("write false manifest");

        let output = release_trust([
            "verify-manifest",
            "--dir",
            public.to_str().expect("public path"),
            "--manifest",
            public
                .join("release-manifest.json")
                .to_str()
                .expect("public manifest"),
            "--checksums",
            public
                .join("SHA256SUMS")
                .to_str()
                .expect("public checksums"),
        ]);
        assert!(!output.status.success(), "false {field} must fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("target aarch64-apple-darwin"), "{stderr}");
        assert!(stderr.contains(&format!("field {field}")), "{stderr}");
        assert!(stderr.contains("checksummed trust"), "{stderr}");
    }
    fs::write(public.join("release-manifest.json"), correct_manifest).expect("restore manifest");
}

#[test]
fn release_manifest_rejects_conflicting_duplicate_target_trust() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    let first = distrib.join("target/distrib/trust");
    let second = distrib.join("dist-global/target/distrib/trust");
    fs::create_dir_all(&first).expect("first trust directory");
    fs::create_dir_all(&second).expect("second trust directory");
    let name = "macos-aarch64-apple-darwin.json";
    fs::write(
        first.join(name),
        r#"{"target":"aarch64-apple-darwin","signed":true,"notarized":true}"#,
    )
    .expect("first trust status");
    fs::write(
        second.join(name),
        r#"{"target":"aarch64-apple-darwin","signed":true,"notarized":false}"#,
    )
    .expect("conflicting trust status");

    let output = release_trust([
        "manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--ref",
        "refs/tags/v0.8.0",
        "--repo",
        "gregce/deadreckon",
        "--commit",
        "abc123",
        "--out",
        distrib
            .join("release-manifest.json")
            .to_str()
            .expect("manifest path"),
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("conflicting trust status for target aarch64-apple-darwin"),
        "{stderr}"
    );
    assert!(!distrib.join("release-manifest.json").exists());
}

#[test]
fn verify_manifest_requires_trust_when_target_policy_requires_signing() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    let payload_name = "deadreckon-aarch64-apple-darwin";
    let payload = temp.path().join("payload").join(payload_name);
    fs::create_dir_all(&distrib).expect("distrib");
    fs::create_dir_all(&payload).expect("payload");
    fs::write(payload.join("deadreckon"), "native binary").expect("binary");
    let archive = distrib.join(format!("{payload_name}.tar.xz"));
    let output = Command::new("tar")
        .args(["-cJf"])
        .arg(&archive)
        .arg("-C")
        .arg(temp.path().join("payload"))
        .arg(payload_name)
        .output()
        .expect("tar");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sums = distrib.join("SHA256SUMS");
    let manifest_path = distrib.join("release-manifest.json");
    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--out",
        sums.to_str().expect("checksums path"),
    ]);
    assert_release_trust_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--ref",
        "refs/tags/v0.8.0",
        "--repo",
        "gregce/deadreckon",
        "--commit",
        "abc123",
        "--out",
        manifest_path.to_str().expect("manifest path"),
    ]);

    let manifest = read_json(&manifest_path);
    let archive_record = manifest["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .find(|artifact| artifact["name"] == format!("{payload_name}.tar.xz"))
        .expect("macOS archive record");
    assert_eq!(Some(false), archive_record["signed"].as_bool());
    assert_eq!(Some(false), archive_record["notarized"].as_bool());
    assert!(archive_record["signature_kind"].is_null());

    let output = release_trust([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--manifest",
        manifest_path.to_str().expect("manifest path"),
        "--checksums",
        sums.to_str().expect("checksums path"),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "target aarch64-apple-darwin requires signing but has no checksummed trust status"
        ),
        "{stderr}"
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
fn release_plan_is_read_only_and_only_final_publisher_mutates_github_release() {
    let workflow = release_workflow();
    let dist_plan = workflow
        .split_once("  dist-plan:")
        .and_then(|(_, tail)| tail.split_once("\n  release-verify:"))
        .map(|(job, _)| job)
        .expect("dist-plan job");
    assert!(
        dist_plan.contains("dist plan --output-format=json"),
        "{dist_plan}"
    );
    for forbidden in [
        "dist host",
        "--steps=create",
        "gh release",
        "contents: write",
        "GH_TOKEN",
    ] {
        assert!(
            !dist_plan.contains(forbidden),
            "dist-plan must be read-only; found {forbidden}:\n{dist_plan}"
        );
    }

    let publisher = workflow
        .split_once("  publish-github-release:")
        .and_then(|(_, tail)| tail.split_once("\n  publish-homebrew-formula:"))
        .map(|(job, _)| job)
        .expect("GitHub Release publisher job");
    assert!(
        publisher.contains("permissions:\n      contents: write"),
        "{publisher}"
    );
    assert!(
        publisher.contains("- release-trust-artifacts")
            && publisher.contains("- attest-release-artifacts"),
        "publisher must wait for trust and attestation: {publisher}"
    );
    for mutation in ["gh release create", "gh release edit", "gh release upload"] {
        assert!(
            publisher.contains(mutation),
            "{mutation} missing: {publisher}"
        );
        assert_eq!(
            1,
            workflow.matches(mutation).count(),
            "{mutation} must exist only in the final publisher"
        );
    }
    assert!(
        !workflow.contains("dist host") && !workflow.contains("--steps=create"),
        "cargo-dist must never own GitHub Release creation: {workflow}"
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
fn release_verification_mirrors_working_ci_test_prerequisites() {
    let workflow = release_workflow();
    let verify_job = workflow
        .split_once("  release-verify:")
        .and_then(|(_, tail)| tail.split_once("\n  build-evaluator-sidecars:"))
        .map(|(job, _)| job)
        .expect("release-verify job");

    assert!(
        verify_job.contains("sudo apt-get install -y -qq bubblewrap expect"),
        "release verification must install the same sandbox and prompt prerequisites as CI:\n{verify_job}"
    );

    let characterization_build = verify_job
        .find("cargo build -p deadreckon --features internal-characterization --bin deadreckon-characterization --locked")
        .expect("release verification must build the gated characterization binary");
    let workspace_tests = verify_job
        .find("cargo test --workspace --locked --no-fail-fast")
        .expect("release verification must run workspace tests");
    assert!(
        characterization_build < workspace_tests,
        "the characterization binary must exist before workspace tests spawn it:\n{verify_job}"
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
    let assembly_step = workflow
        .find("- name: Assemble complete target archive")
        .expect("complete archive assembly step");
    let codesign_step = workflow
        .find("- name: Sign and verify packaged macOS artifacts")
        .expect("packaged macOS signing step");
    assert!(
        build_step < assembly_step && assembly_step < codesign_step,
        "musl evaluators must be assembled after dist build and before macOS signing"
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
    let signer =
        fs::read_to_string(workspace_root().join("release/trust/sign-macos-artifacts.mjs"))
            .expect("macOS signer");
    assert!(
        signer.contains("\"codesign\"") && signer.contains("\"--verify\""),
        "{signer}"
    );
    assert!(
        signer.contains("\"xcrun\"")
            && signer.contains("\"notarytool\"")
            && signer.contains("\"submit\""),
        "{signer}"
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
        "node release/evaluator-sidecars.mjs manifest",
        "node release/evaluator-sidecars.mjs verify-manifest",
        "node release/trust/release-trust.mjs sbom",
        "node release/trust/release-trust.mjs checksums",
        "node release/trust/release-trust.mjs manifest",
        "node release/trust/release-trust.mjs verify-manifest",
        "uses: actions/attest@v4",
        "id-token: write",
        "attestations: write",
        "artifact-metadata: write",
        "release-manifest.json",
        "release-archive-members.json",
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
    for binary in ["deadreckon.exe", "dr-gate.exe", "dr-capture.exe"] {
        assert!(signer.contains(binary), "{binary} missing from {signer}");
    }
}

#[test]
fn release_workflow_builds_static_evaluators_and_assembles_them_before_trust() {
    let workflow = release_workflow();
    for needle in [
        "build-evaluator-sidecars:",
        "aarch64-unknown-linux-musl",
        "x86_64-unknown-linux-musl",
        "--bin dr-gate",
        "release/evaluator-sidecars.mjs verify-sidecars",
        "release/evaluator-sidecars.mjs assemble",
        "release/evaluator-sidecars.mjs refresh-checksum",
        "release/evaluator-sidecars.mjs verify-archive",
        "release/evaluator-sidecars.mjs patch-installers",
        "release/evaluator-sidecars.mjs verify-installers",
    ] {
        assert!(workflow.contains(needle), "{needle} missing from workflow");
    }

    let assemble = workflow
        .find("release/evaluator-sidecars.mjs assemble")
        .expect("archive assembly");
    let mac_sign = workflow
        .find("release/trust/sign-macos-artifacts.mjs")
        .expect("mac signing");
    let checksums = workflow
        .find("release/trust/release-trust.mjs checksums")
        .expect("release checksums");
    assert!(assemble < mac_sign && mac_sign < checksums, "{workflow}");

    let mac_signer =
        fs::read_to_string(workspace_root().join("release/trust/sign-macos-artifacts.mjs"))
            .expect("mac signer");
    for binary in ["deadreckon", "dr-gate", "dr-capture"] {
        assert!(
            mac_signer.contains(&format!("\"{binary}\"")),
            "{mac_signer}"
        );
    }
    assert!(
        mac_signer.contains("payloadRoot(extractDir)"),
        "notarization must cover the complete assembled payload"
    );
}

#[test]
fn evaluator_sidecar_tool_assembles_and_manifests_static_linux_helpers() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    let sidecars = temp.path().join("sidecars");
    let target = "aarch64-apple-darwin";
    let payload = temp
        .path()
        .join("payload")
        .join(format!("deadreckon-{target}"));
    fs::create_dir_all(&distrib).expect("distrib");
    fs::create_dir_all(&sidecars).expect("sidecars");
    fs::create_dir_all(&payload).expect("payload");

    for helper in ["deadreckon", "dr-gate", "dr-capture"] {
        let identity = if helper == "dr-capture" {
            ""
        } else {
            FAKE_GATE_BUNDLE
        };
        fs::write(payload.join(helper), format!("{helper} native{identity}"))
            .expect("native helper");
    }
    write_fake_static_elf_sized(
        &sidecars.join("dr-gate-evaluator-aarch64-unknown-linux-musl"),
        0xb7,
        false,
        2 * 1024 * 1024,
    );
    write_fake_static_elf_sized(
        &sidecars.join("dr-gate-evaluator-x86_64-unknown-linux-musl"),
        0x3e,
        false,
        3 * 1024 * 1024,
    );

    let archive = distrib.join(format!("deadreckon-{target}.tar.xz"));
    let output = Command::new("tar")
        .args([
            "-cJf",
            archive.to_str().expect("archive path"),
            "-C",
            temp.path().join("payload").to_str().expect("payload path"),
            &format!("deadreckon-{target}"),
        ])
        .output()
        .expect("create archive");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        format!("{}.sha256", archive.display()),
        format!(
            "{} *{}\n",
            "0".repeat(64),
            archive.file_name().unwrap().to_string_lossy()
        ),
    )
    .expect("checksum sibling");

    assert_evaluator_tool_success([
        "assemble",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--target",
        target,
        "--sidecars-dir",
        sidecars.to_str().expect("sidecars path"),
    ]);
    let stale = evaluator_tool([
        "verify-archive",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--target",
        target,
    ]);
    assert!(
        !stale.status.success(),
        "final archive verification must reject a stale cargo-dist checksum sibling"
    );
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("is stale"),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    assert_evaluator_tool_success([
        "refresh-checksum",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--target",
        target,
    ]);
    assert_evaluator_tool_success([
        "verify-archive",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--target",
        target,
    ]);

    let member_manifest = distrib.join("release-archive-members.json");
    assert_evaluator_tool_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--target",
        target,
        "--out",
        member_manifest.to_str().expect("manifest path"),
    ]);
    assert_evaluator_tool_success([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--manifest",
        member_manifest.to_str().expect("manifest path"),
    ]);

    let manifest = read_json(&member_manifest);
    assert_eq!(Some(1), manifest["schema_version"].as_u64());
    assert_eq!(Some(1), manifest["archives"].as_array().map(Vec::len));
    assert_eq!(Some(2), manifest["evaluators"].as_array().map(Vec::len));
    let members = manifest["archives"][0]["members"]
        .as_array()
        .expect("archive members");
    for required in [
        "deadreckon",
        "dr-gate",
        "dr-capture",
        "dr-gate-evaluator-aarch64-unknown-linux-musl",
        "dr-gate-evaluator-x86_64-unknown-linux-musl",
    ] {
        assert!(
            members
                .iter()
                .any(|member| member["name"].as_str() == Some(required)),
            "{required} missing from {manifest:#}"
        );
    }
    let checksum =
        fs::read_to_string(format!("{}.sha256", archive.display())).expect("refreshed checksum");
    assert!(
        checksum.ends_with(&format!(" *deadreckon-{target}.tar.xz\n")),
        "{checksum}"
    );
}

#[test]
fn evaluator_sidecar_tool_rejects_dynamically_linked_evaluator() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    write_fake_static_elf(
        &temp
            .path()
            .join("dr-gate-evaluator-aarch64-unknown-linux-musl"),
        0xb7,
        true,
    );
    write_fake_static_elf(
        &temp
            .path()
            .join("dr-gate-evaluator-x86_64-unknown-linux-musl"),
        0x3e,
        false,
    );
    let output = evaluator_tool([
        "verify-sidecars",
        "--sidecars-dir",
        temp.path().to_str().expect("temp path"),
    ]);
    assert!(
        !output.status.success(),
        "dynamic evaluator must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("PT_INTERP"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn evaluator_sidecar_tool_rejects_same_protocol_mixed_build_bundles() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let arm = temp
        .path()
        .join("dr-gate-evaluator-aarch64-unknown-linux-musl");
    let intel = temp
        .path()
        .join("dr-gate-evaluator-x86_64-unknown-linux-musl");
    write_fake_static_elf(&arm, 0xb7, false);
    write_fake_static_elf(&intel, 0x3e, false);
    let mut stale = fs::read(&intel).expect("fake evaluator");
    let identity = b"1111111111111111111111111111111111111111111111111111111111111111";
    let offset = stale
        .windows(identity.len())
        .position(|window| window == identity)
        .expect("fake build identity");
    stale[offset..offset + identity.len()].fill(b'2');
    fs::write(&intel, stale).expect("mixed evaluator bundle");

    let output = evaluator_tool([
        "verify-sidecars",
        "--sidecars-dir",
        temp.path().to_str().expect("temp path"),
    ]);
    assert!(
        !output.status.success(),
        "mixed build bundle must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("mix incompatible"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn evaluator_sidecar_tool_rehearses_all_five_release_archives() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    let sidecars = temp.path().join("sidecars");
    let payloads = temp.path().join("payloads");
    fs::create_dir_all(&distrib).expect("distrib");
    fs::create_dir_all(&sidecars).expect("sidecars");
    fs::create_dir_all(&payloads).expect("payloads");
    write_fake_static_elf(
        &sidecars.join("dr-gate-evaluator-aarch64-unknown-linux-musl"),
        0xb7,
        false,
    );
    write_fake_static_elf(
        &sidecars.join("dr-gate-evaluator-x86_64-unknown-linux-musl"),
        0x3e,
        false,
    );

    for target in DIST_TARGETS {
        let payload_name = format!("deadreckon-{target}");
        let payload = payloads.join(&payload_name);
        fs::create_dir_all(&payload).expect("payload");
        let extension = if target.ends_with("windows-msvc") {
            ".exe"
        } else {
            ""
        };
        for helper in [
            format!("deadreckon{extension}"),
            format!("dr-gate{extension}"),
            format!("dr-capture{extension}"),
        ] {
            let identity = if helper.starts_with("dr-capture") {
                ""
            } else {
                FAKE_GATE_BUNDLE
            };
            fs::write(
                payload.join(&helper),
                format!("{helper} for {target}{identity}"),
            )
            .expect("native helper");
        }

        let archive = if target.ends_with("windows-msvc") {
            let archive = distrib.join(format!("deadreckon-{target}.zip"));
            let output = Command::new("zip")
                .args(["-X", "-q"])
                .arg(&archive)
                .args(["deadreckon.exe", "dr-gate.exe", "dr-capture.exe"])
                .current_dir(&payload)
                .output()
                .expect("create Windows zip");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            archive
        } else {
            let archive = distrib.join(format!("deadreckon-{target}.tar.xz"));
            let output = Command::new("tar")
                .args(["-cJf"])
                .arg(&archive)
                .arg("-C")
                .arg(&payloads)
                .arg(&payload_name)
                .output()
                .expect("create target tarball");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            archive
        };
        fs::write(
            format!("{}.sha256", archive.display()),
            format!(
                "{} *{}\n",
                "0".repeat(64),
                archive.file_name().unwrap().to_string_lossy()
            ),
        )
        .expect("stale cargo-dist checksum");

        assert_evaluator_tool_success([
            "assemble",
            "--dir",
            distrib.to_str().expect("distrib path"),
            "--target",
            target,
            "--sidecars-dir",
            sidecars.to_str().expect("sidecars path"),
        ]);
        assert_evaluator_tool_success([
            "refresh-checksum",
            "--dir",
            distrib.to_str().expect("distrib path"),
            "--target",
            target,
        ]);
        assert_evaluator_tool_success([
            "verify-archive",
            "--dir",
            distrib.to_str().expect("distrib path"),
            "--target",
            target,
        ]);
    }

    let manifest_path = distrib.join("release-archive-members.json");
    assert_evaluator_tool_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--out",
        manifest_path.to_str().expect("manifest path"),
    ]);
    assert_evaluator_tool_success([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("distrib path"),
        "--manifest",
        manifest_path.to_str().expect("manifest path"),
    ]);

    let manifest = read_json(&manifest_path);
    assert_eq!(Some(5), manifest["archives"].as_array().map(Vec::len));
    assert_eq!(Some(2), manifest["evaluators"].as_array().map(Vec::len));
    for archive in manifest["archives"].as_array().expect("archive inventory") {
        let members = archive["members"].as_array().expect("member inventory");
        assert_eq!(
            2,
            members
                .iter()
                .filter(|member| member["role"] == "sandbox-evaluator")
                .count(),
            "{archive:#}"
        );
        assert_eq!(
            3,
            members
                .iter()
                .filter(|member| member["role"] == "host-helper")
                .count(),
            "{archive:#}"
        );
    }
}

#[test]
fn evaluator_sidecar_tool_uses_dotnet_zip_apis_on_windows() {
    let tool = fs::read_to_string(workspace_root().join("release/evaluator-sidecars.mjs"))
        .expect("evaluator sidecar tool");
    for required in [
        "listZipArchiveOnWindows(archive)",
        "extractZipArchiveOnWindows(archive, destination)",
        "extractZipArchiveMemberOnWindows(archive, member)",
        "createZipArchiveOnWindows(staged, sourceDir)",
        "Add-Type -AssemblyName System.IO.Compression",
        "System.IO.Compression.ZipFile",
        "System.IO.Compression.ZipFileExtensions",
        "ZipArchiveMode]::Create",
        "$entry.LastWriteTime=$timestamp",
        "Get-ChildItem -LiteralPath $root -Recurse -File",
        "$source=$item.OpenRead()",
        "Expand-Archive -LiteralPath",
    ] {
        assert!(tool.contains(required), "missing {required} from {tool}");
    }
    assert!(
        !tool.contains("process.platform === \"win32\"\n      ? spawnSync(\"tar\""),
        "Windows ZIP handling must not depend on whichever tar Git Bash places first on PATH"
    );
    assert!(
        !tool.contains(".zip.entries.json"),
        "Windows ZIP assembly must keep paths as native FileInfo objects instead of hydrating them through JSON"
    );
    assert!(
        !tool.contains("Compress-Archive -LiteralPath"),
        "Windows ZIP repacking must tolerate reproducible pre-1980 source timestamps"
    );
}

#[test]
fn generated_installers_are_patched_for_native_helpers_and_both_evaluators() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let shell = temp.path().join("deadreckon-installer.sh");
    let powershell = temp.path().join("deadreckon-installer.ps1");
    let mut shell_fixture = String::from("case \"$_archive\" in\n");
    for target in DIST_TARGETS {
        let windows = target.ends_with("windows-msvc");
        let suffix = if windows { ".zip" } else { ".tar.xz" };
        let extension = if windows { ".exe" } else { "" };
        shell_fixture.push_str(&format!(
            "  \"deadreckon-{target}{suffix}\")\n    _bins=\"deadreckon{extension} dr-gate{extension} dr-capture{extension}\"\n    _bins_js_array='\"deadreckon{extension}\",\"dr-gate{extension}\",\"dr-capture{extension}\"'\n    ;;\n"
        ));
    }
    shell_fixture.push_str("esac\n");
    fs::write(&shell, shell_fixture).expect("shell installer");
    fs::write(
        &powershell,
        r#"$platform = @{
  "artifact_name" = "deadreckon-x86_64-pc-windows-msvc.zip"
  "bins" = @("deadreckon.exe", "dr-gate.exe", "dr-capture.exe")
}
"#,
    )
    .expect("PowerShell installer");

    assert_evaluator_tool_success([
        "patch-installers",
        "--dir",
        temp.path().to_str().expect("temp path"),
    ]);
    assert_evaluator_tool_success([
        "verify-installers",
        "--dir",
        temp.path().to_str().expect("temp path"),
    ]);

    for installer in [&shell, &powershell] {
        let text = fs::read_to_string(installer).expect("patched installer");
        for evaluator in [
            "dr-gate-evaluator-aarch64-unknown-linux-musl",
            "dr-gate-evaluator-x86_64-unknown-linux-musl",
        ] {
            assert!(text.contains(evaluator), "{}: {text}", installer.display());
        }
    }
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
fn release_runbook_contains_stable_operator_checklist() {
    let doc =
        fs::read_to_string(workspace_root().join("docs/RELEASE.md")).expect("read docs/RELEASE.md");
    assert!(
        doc.contains("Stable operator checklist"),
        "docs/RELEASE.md needs the stable operator checklist section"
    );
    for item in [
        "gregce/homebrew-tap",
        "HOMEBREW_TAP_TOKEN",
        "NPM_TOKEN",
        "WINDOWS_CERT_PFX",
        "WINDOWS_CERT_PWD",
        "npm/deadreckon/package.json",
        "preflight-real.sh",
        "Windows smoke",
    ] {
        assert!(doc.contains(item), "{item} missing from docs/RELEASE.md");
    }
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

#[test]
fn checksums_record_flat_basenames_and_collapse_nested_duplicates() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    fs::create_dir_all(distrib.join("target/distrib")).expect("nested dir");
    fs::write(distrib.join("deadreckon-installer.sh"), b"#!/bin/sh\n").expect("installer");
    // Build intermediates (extracted per-target dirs with loose binaries)
    // are not published assets and must never appear in SHA256SUMS — their
    // basenames collide across targets with different content.
    let loose = distrib.join("deadreckon-aarch64-apple-darwin");
    fs::create_dir_all(&loose).expect("loose dir");
    fs::write(loose.join("deadreckon"), b"mac binary").expect("loose binary");
    // The CI global-artifact layout nests some files twice with identical
    // content; SHA256SUMS must record one flat basename per asset so users
    // can run `shasum -a 256 -c SHA256SUMS` next to downloaded files.
    fs::write(
        distrib.join("target/distrib/deadreckon-installer.sh"),
        b"#!/bin/sh\n",
    )
    .expect("nested duplicate");
    let sums = distrib.join("SHA256SUMS");

    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        sums.to_str().expect("utf8 path"),
    ]);

    let raw = fs::read_to_string(&sums).expect("sums");
    assert_eq!(raw.lines().count(), 1, "{raw}");
    let line = raw.lines().next().expect("entry");
    assert!(
        line.ends_with("  deadreckon-installer.sh"),
        "entry must be a flat basename: {line}"
    );
    assert!(
        !line.contains('/'),
        "no path segments in SHA256SUMS: {line}"
    );
}

#[test]
fn checksums_fail_closed_on_conflicting_duplicate_basenames() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    fs::create_dir_all(distrib.join("nested")).expect("nested dir");
    fs::write(distrib.join("deadreckon.rb"), b"formula one").expect("formula");
    fs::write(distrib.join("nested/deadreckon.rb"), b"formula two").expect("conflict");

    let output = release_trust([
        "checksums",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        distrib.join("SHA256SUMS").to_str().expect("utf8 path"),
    ]);

    assert!(
        !output.status.success(),
        "conflicting duplicate basenames must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("conflicting contents"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn verify_manifest_rejects_dot_slash_prefixed_archive_members() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let distrib = temp.path().join("target/distrib");
    fs::create_dir_all(&distrib).expect("distrib dir");
    let payload = temp.path().join("payload/deadreckon-aarch64-apple-darwin");
    fs::create_dir_all(&payload).expect("payload");
    fs::write(payload.join("deadreckon"), b"binary").expect("binary");

    let run_tar = |args: &[&str]| {
        let output = Command::new("tar")
            .args(args)
            .current_dir(temp.path())
            .output()
            .expect("tar");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let archive = distrib.join("deadreckon-aarch64-apple-darwin.tar.xz");
    let archive_str = archive.to_str().expect("utf8 path").to_string();
    // The broken rc.7 shape: members prefixed with "./" from `tar -C dir .`.
    run_tar(&["-cJf", &archive_str, "-C", "payload", "."]);

    let manifest = distrib.join("release-manifest.json");
    let sums = distrib.join("SHA256SUMS");
    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        sums.to_str().expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--ref",
        "refs/tags/v9.9.9-rc.1",
        "--repo",
        "someone/deadreckon",
        "--commit",
        "0000000000000000000000000000000000000000",
        "--out",
        manifest.to_str().expect("utf8 path"),
    ]);

    let broken = release_trust([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--manifest",
        manifest.to_str().expect("utf8 path"),
        "--checksums",
        sums.to_str().expect("utf8 path"),
    ]);
    assert!(
        !broken.status.success(),
        "'./'-prefixed members must fail verify-manifest"
    );
    assert!(
        String::from_utf8_lossy(&broken.stderr).contains("'./'-prefixed member"),
        "{}",
        String::from_utf8_lossy(&broken.stderr)
    );

    // The fixed shape — explicit top-level names — passes the same gate.
    fs::remove_file(&archive).expect("remove broken archive");
    run_tar(&[
        "-cJf",
        &archive_str,
        "-C",
        "payload",
        "deadreckon-aarch64-apple-darwin",
    ]);
    assert_release_trust_success([
        "checksums",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--out",
        sums.to_str().expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--ref",
        "refs/tags/v9.9.9-rc.1",
        "--repo",
        "someone/deadreckon",
        "--commit",
        "0000000000000000000000000000000000000000",
        "--out",
        manifest.to_str().expect("utf8 path"),
    ]);
    assert_release_trust_success([
        "verify-manifest",
        "--dir",
        distrib.to_str().expect("utf8 path"),
        "--manifest",
        manifest.to_str().expect("utf8 path"),
        "--checksums",
        sums.to_str().expect("utf8 path"),
    ]);
}

fn write_fake_static_elf(path: &Path, machine: u16, with_interp: bool) {
    write_fake_static_elf_sized(path, machine, with_interp, 64 + 56);
}

const FAKE_GATE_BUNDLE: &str = concat!(
    " deadreckon-gate-evaluator-protocol-v1 ",
    "deadreckon-bundle-build-id-sha256:",
    "1111111111111111111111111111111111111111111111111111111111111111"
);

fn write_fake_static_elf_sized(path: &Path, machine: u16, with_interp: bool, size: usize) {
    let mut bytes = vec![0_u8; size.max(64 + 56)];
    bytes[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    bytes[4] = 2; // ELFCLASS64
    bytes[5] = 1; // little endian
    bytes[6] = 1; // ELF version
    bytes[16..18].copy_from_slice(&3_u16.to_le_bytes()); // ET_DYN/static PIE
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64_u64.to_le_bytes()); // e_phoff
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes()); // e_ehsize
    bytes[54..56].copy_from_slice(&56_u16.to_le_bytes()); // e_phentsize
    bytes[56..58].copy_from_slice(&1_u16.to_le_bytes()); // e_phnum
    bytes[64..68].copy_from_slice(&(if with_interp { 3_u32 } else { 1_u32 }).to_le_bytes());
    bytes.extend_from_slice(FAKE_GATE_BUNDLE.as_bytes());
    fs::write(path, bytes).expect("write fake ELF");
}

fn evaluator_tool<const N: usize>(args: [&str; N]) -> std::process::Output {
    Command::new("node")
        .arg(workspace_root().join("release/evaluator-sidecars.mjs"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("run evaluator-sidecars")
}

fn assert_evaluator_tool_success<const N: usize>(args: [&str; N]) {
    let output = evaluator_tool(args);
    assert!(
        output.status.success(),
        "evaluator-sidecars failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
