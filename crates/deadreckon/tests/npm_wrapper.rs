#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

const PLATFORM_PACKAGES: &[PlatformPackage] = &[
    PlatformPackage {
        name: "deadreckon-darwin-arm64",
        target: "aarch64-apple-darwin",
        os: "darwin",
        cpu: "arm64",
        binary: "deadreckon",
    },
    PlatformPackage {
        name: "deadreckon-darwin-x64",
        target: "x86_64-apple-darwin",
        os: "darwin",
        cpu: "x64",
        binary: "deadreckon",
    },
    PlatformPackage {
        name: "deadreckon-linux-arm64",
        target: "aarch64-unknown-linux-gnu",
        os: "linux",
        cpu: "arm64",
        binary: "deadreckon",
    },
    PlatformPackage {
        name: "deadreckon-linux-x64",
        target: "x86_64-unknown-linux-gnu",
        os: "linux",
        cpu: "x64",
        binary: "deadreckon",
    },
    PlatformPackage {
        name: "deadreckon-win32-x64",
        target: "x86_64-pc-windows-msvc",
        os: "win32",
        cpu: "x64",
        binary: "deadreckon.exe",
    },
];
const EVALUATOR_SIDECARS: &[&str] = &[
    "dr-gate-evaluator-aarch64-unknown-linux-musl",
    "dr-gate-evaluator-x86_64-unknown-linux-musl",
];

#[derive(Clone, Copy)]
struct PlatformPackage {
    name: &'static str,
    target: &'static str,
    os: &'static str,
    cpu: &'static str,
    binary: &'static str,
}

#[test]
fn npm_wrapper_optional_deps_match_five_platforms() {
    let package = read_json(&workspace_root().join("npm/deadreckon/package.json"));
    let wrapper_version = package
        .get("version")
        .and_then(Value::as_str)
        .expect("wrapper version");
    let deps = package
        .get("optionalDependencies")
        .and_then(Value::as_object)
        .expect("optionalDependencies object");
    let actual = deps.keys().cloned().collect::<BTreeSet<_>>();
    let expected = PLATFORM_PACKAGES
        .iter()
        .map(|package| package.name.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, actual);
    for package in PLATFORM_PACKAGES {
        assert_eq!(
            Some(wrapper_version),
            deps.get(package.name).and_then(Value::as_str),
            "{} version should match wrapper",
            package.name
        );
        let template = fs::read_to_string(
            workspace_root()
                .join("npm")
                .join(package.name)
                .join("package.json.template"),
        )
        .expect("package template");
        let package_json: Value =
            serde_json::from_str(&template.replace("__VERSION__", wrapper_version))
                .expect("package template json");
        assert_eq!(
            Some(wrapper_version),
            package_json.get("version").and_then(Value::as_str),
            "{} template version placeholder",
            package.name
        );
    }
}

#[test]
fn npm_wrapper_postinstall_writes_receipt_no_network() {
    let Some(package) = current_platform_package() else {
        return;
    };
    let temp = TempDir::new().expect("tempdir");
    let package_root = temp.path().join("deadreckon");
    copy_dir(&workspace_root().join("npm/deadreckon"), &package_root);
    write_fake_platform_package(&package_root, package);

    let home = temp.path().join("home");
    let output = Command::new("node")
        .arg(package_root.join("scripts/postinstall.js"))
        .env("DEADRECKON_HOME", &home)
        .env("DEADRECKON_PLATFORM_PACKAGE", package.name)
        .current_dir(&package_root)
        .output()
        .expect("run node postinstall");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let receipt = read_json(&home.join("install-receipt.json"));
    assert_eq!(Some("npm"), receipt.get("channel").and_then(Value::as_str));
    assert_eq!(
        Some(package.name),
        receipt.get("platform_package").and_then(Value::as_str)
    );
    assert_eq!(
        Some(1),
        receipt.get("receipt_version").and_then(Value::as_u64)
    );
    let postinstall =
        fs::read_to_string(workspace_root().join("npm/deadreckon/scripts/postinstall.js"))
            .expect("postinstall script");
    for forbidden in [
        "https://",
        "http://",
        "fetch(",
        "curl ",
        "wget ",
        "child_process",
        "spawn",
    ] {
        assert!(
            !postinstall.contains(forbidden),
            "postinstall must not perform network/process work: {forbidden}"
        );
    }
}

#[test]
fn npm_wrapper_bin_resolves_to_platform_package() {
    if cfg!(windows) {
        return;
    }
    let Some(package) = current_platform_package() else {
        return;
    };
    let temp = TempDir::new().expect("tempdir");
    let package_root = temp.path().join("deadreckon");
    copy_dir(&workspace_root().join("npm/deadreckon"), &package_root);
    write_fake_platform_package(&package_root, package);

    let marker = temp.path().join("argv.txt");
    let output = Command::new("node")
        .arg(package_root.join("bin/deadreckon.js"))
        .arg("alpha")
        .arg("beta")
        .env("DEADRECKON_PLATFORM_PACKAGE", package.name)
        .env("DEADRECKON_TEST_MARKER", &marker)
        .current_dir(&package_root)
        .output()
        .expect("run wrapper bin");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!("alpha\nbeta\n", fs::read_to_string(marker).expect("marker"));
}

#[test]
fn npm_platform_package_contains_native_helpers_and_both_evaluators() {
    let prepared = prepare_fake_npm_release();
    for package in PLATFORM_PACKAGES {
        let bin_dir = prepared.path().join("npm").join(package.name).join("bin");
        let entries = fs::read_dir(&bin_dir)
            .expect("read bin dir")
            .map(|entry| {
                entry
                    .expect("bin entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        let expected = package_files(*package)
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(expected, entries, "{}", bin_dir.display());

        let package_json = read_json(
            &prepared
                .path()
                .join("npm")
                .join(package.name)
                .join("package.json"),
        );
        let declared = package_json
            .get("files")
            .and_then(Value::as_array)
            .expect("files array")
            .iter()
            .filter_map(Value::as_str)
            .map(|entry| entry.trim_start_matches("bin/").to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(expected, declared, "{} package files", package.name);
    }
}

#[test]
fn npm_platform_package_json_pins_os_and_cpu() {
    let prepared = prepare_fake_npm_release();
    for package in PLATFORM_PACKAGES {
        let package_json = read_json(
            &prepared
                .path()
                .join("npm")
                .join(package.name)
                .join("package.json"),
        );
        assert_eq!(
            Some(package.name),
            package_json.get("name").and_then(Value::as_str)
        );
        assert_eq!(
            Some("9.8.7"),
            package_json.get("version").and_then(Value::as_str)
        );
        assert_eq!(
            Some(&json!([package.os])),
            package_json.get("os"),
            "{}",
            package.name
        );
        assert_eq!(
            Some(&json!([package.cpu])),
            package_json.get("cpu"),
            "{}",
            package.name
        );
    }
}

#[test]
fn npm_platform_packages_pack_complete_runtime_from_release_archives() {
    let prepared = prepare_fake_npm_release_from_archives();
    for package in PLATFORM_PACKAGES {
        let package_dir = prepared.path().join("npm").join(package.name);
        let output = Command::new("npm")
            .args(["pack", "--dry-run", "--json"])
            .arg(&package_dir)
            .current_dir(prepared.path())
            .output()
            .expect("npm pack dry run");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let packed: Value = serde_json::from_slice(&output.stdout).expect("npm pack json");
        let files = packed[0]["files"]
            .as_array()
            .expect("packed files")
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect::<BTreeSet<_>>();
        let mut expected = package_files(*package)
            .into_iter()
            .map(|name| format!("bin/{name}"))
            .collect::<BTreeSet<_>>();
        expected.insert("package.json".to_string());
        let actual = files
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(expected, actual, "{} npm tarball", package.name);
    }
}

#[test]
fn npm_publish_workflow_downloads_release_artifacts_and_publishes_wrapper_last() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/publish-npm.yml"))
        .expect("publish-npm workflow");
    assert!(workflow.contains("id-token: write"), "{workflow}");
    assert!(
        workflow.contains("required: false"),
        "NPM_TOKEN must not be the only official publishing path"
    );
    assert!(
        workflow.contains("Validate stable npm release lane"),
        "{workflow}"
    );
    assert!(
        workflow.contains("release/trust/release-trust.mjs validate"),
        "{workflow}"
    );
    assert!(
        workflow.contains("release/trust/release-trust.mjs lane"),
        "{workflow}"
    );
    assert!(
        workflow.contains("npm publishing is allowed only for official stable release tags"),
        "{workflow}"
    );
    assert!(workflow.contains("gh release download"), "{workflow}");
    assert!(
        workflow.contains("node npm/scripts/prepare-release.mjs"),
        "{workflow}"
    );
    let validate = workflow
        .find("Validate stable npm release lane")
        .expect("stable release validation step");
    let download = workflow
        .find("Download GitHub release artifacts")
        .expect("download release artifacts step");
    let platform_publish = workflow
        .find("Publish platform packages")
        .expect("platform publish step");
    let wrapper_publish = workflow
        .find("Publish wrapper package")
        .expect("wrapper publish step");
    assert!(
        validate < download,
        "npm stable-lane validation must run before artifact download"
    );
    assert!(
        validate < platform_publish,
        "npm stable-lane validation must run before package publishing"
    );
    assert!(
        platform_publish < wrapper_publish,
        "wrapper must publish after platform packages"
    );
    assert!(
        workflow.contains("npm publish \"$package_dir\" --access public --provenance"),
        "{workflow}"
    );
    assert!(
        workflow.contains("npm publish npm/deadreckon --access public --provenance"),
        "{workflow}"
    );
}

fn prepare_fake_npm_release() -> TempDir {
    let temp = TempDir::new().expect("tempdir");
    copy_dir(&workspace_root().join("npm"), &temp.path().join("npm"));
    let artifacts = temp.path().join("artifacts");
    for package in PLATFORM_PACKAGES {
        let dir = artifacts.join(package.target);
        fs::create_dir_all(&dir).expect("artifact dir");
        for file in package_files(*package) {
            fs::write(dir.join(file), format!("{file} for {}\n", package.target))
                .expect("artifact binary");
        }
    }
    let output = Command::new("node")
        .arg(temp.path().join("npm/scripts/prepare-release.mjs"))
        .args(["--tag", "v9.8.7", "--artifacts"])
        .arg(&artifacts)
        .output()
        .expect("run prepare-release");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    temp
}

fn prepare_fake_npm_release_from_archives() -> TempDir {
    let temp = TempDir::new().expect("tempdir");
    copy_dir(&workspace_root().join("npm"), &temp.path().join("npm"));
    let artifacts = temp.path().join("artifacts");
    let payloads = temp.path().join("payloads");
    fs::create_dir_all(&artifacts).expect("artifacts");
    fs::create_dir_all(&payloads).expect("payloads");
    for package in PLATFORM_PACKAGES {
        let payload_name = format!("deadreckon-{}", package.target);
        let payload = payloads.join(&payload_name);
        fs::create_dir_all(&payload).expect("archive payload");
        for file in package_files(*package) {
            fs::write(
                payload.join(file),
                format!("{file} for {}\n", package.target),
            )
            .expect("archive member");
        }
        let output = if package.target.ends_with("windows-msvc") {
            Command::new("zip")
                .args(["-X", "-q", "-r"])
                .arg(artifacts.join(format!("{payload_name}.zip")))
                .arg(&payload_name)
                .current_dir(&payloads)
                .output()
                .expect("create npm Windows archive")
        } else {
            Command::new("tar")
                .args(["-cJf"])
                .arg(artifacts.join(format!("{payload_name}.tar.xz")))
                .arg("-C")
                .arg(&payloads)
                .arg(&payload_name)
                .output()
                .expect("create npm target archive")
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new("node")
        .arg(temp.path().join("npm/scripts/prepare-release.mjs"))
        .args(["--tag", "v9.8.7", "--artifacts"])
        .arg(&artifacts)
        .output()
        .expect("prepare npm release from archives");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    temp
}

fn package_files(package: PlatformPackage) -> Vec<&'static str> {
    let windows = package.target.ends_with("windows-msvc");
    let mut files = vec![
        package.binary,
        if windows { "dr-gate.exe" } else { "dr-gate" },
        if windows {
            "dr-capture.exe"
        } else {
            "dr-capture"
        },
    ];
    files.extend(EVALUATOR_SIDECARS.iter().copied());
    files
}

fn write_fake_platform_package(wrapper_root: &Path, package: PlatformPackage) {
    let package_root = wrapper_root.join("node_modules").join(package.name);
    let bin_dir = package_root.join("bin");
    fs::create_dir_all(&bin_dir).expect("fake bin dir");
    fs::write(
        package_root.join("package.json"),
        serde_json::to_vec_pretty(&json!({
            "name": package.name,
            "version": "0.1.0",
            "bin": {
                "deadreckon": format!("bin/{}", package.binary),
            },
        }))
        .expect("fake package json"),
    )
    .expect("write fake package json");
    let binary = bin_dir.join(package.binary);
    fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$DEADRECKON_TEST_MARKER\"\n",
    )
    .expect("fake binary");
    make_executable(&binary);
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
}

fn current_platform_package() -> Option<PlatformPackage> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win32",
        _ => return None,
    };
    let cpu = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    PLATFORM_PACKAGES
        .iter()
        .copied()
        .find(|package| package.os == os && package.cpu == cpu)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display())),
    )
    .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|err| panic!("create {}: {err}", dst.display()));
    for entry in fs::read_dir(src).unwrap_or_else(|err| panic!("read {}: {err}", src.display())) {
        let entry = entry.expect("dir entry");
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path)
                .unwrap_or_else(|err| panic!("copy {}: {err}", src_path.display()));
        }
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
