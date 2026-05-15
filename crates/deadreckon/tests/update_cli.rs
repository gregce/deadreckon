#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use chrono::{Duration, Utc};
use deadreckon_core::DeadreckonPaths;
use deadreckon_core::install_receipt::{
    Channel, INSTALL_RECEIPT_VERSION, Receipt, receipt_path, write_receipt,
};
use deadreckon_core::update_cache::{Cache, read_cache, write_cache};
use tempfile::TempDir;

#[test]
fn update_check_prints_channel_from_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--check"])
        .env("DEADRECKON_UPDATE_TEST_LATEST_VERSION", "0.2.3")
        .output()
        .expect("update check");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("channel: npm"), "{out}");
    assert!(out.contains("current: 0.1.0"), "{out}");
    assert!(out.contains("latest: 0.2.3"), "{out}");
}

#[test]
fn update_check_exits_zero_when_no_network() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Brew)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--check"])
        .env("DEADRECKON_UPDATE_TEST_OFFLINE", "1")
        .output()
        .expect("update check");

    assert_success(&output);
    assert!(stdout(&output).contains("channel: brew"));
}

#[test]
fn update_check_refreshes_cache_when_stale() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Cargo)).expect("receipt");
    write_cache(
        &paths,
        &Cache {
            checked_at: Utc::now() - Duration::hours(25),
            latest_version: "0.1.0".to_string(),
            current_version: "0.1.0".to_string(),
            release_url: "https://github.com/gdc/deadreckon/releases/tag/v0.1.0".to_string(),
            update_available: false,
        },
    )
    .expect("cache");

    let output = deadreckon(&paths)
        .args(["update", "--check"])
        .env("DEADRECKON_UPDATE_TEST_LATEST_VERSION", "0.2.3")
        .output()
        .expect("update check");

    assert_success(&output);
    let cache = read_cache(&paths)
        .expect("read cache")
        .expect("cache present");
    assert_eq!(cache.latest_version, "0.2.3");
    assert!(cache.update_available);
}

#[test]
fn update_check_does_not_write_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);

    let output = deadreckon(&paths)
        .args(["update", "--check"])
        .env("DEADRECKON_UPDATE_TEST_LATEST_VERSION", "0.2.3")
        .output()
        .expect("update check");

    assert_success(&output);
    assert!(!receipt_path(&paths).exists());
}

#[test]
fn update_npm_prints_bun_update_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    assert!(stdout(&output).contains("try: bun update -g deadreckon"));
}

#[test]
fn update_brew_prints_brew_upgrade_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Brew)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    assert!(stdout(&output).contains("try: brew upgrade gdc/tap/deadreckon"));
}

#[test]
fn update_cargo_prints_binstall_or_install_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Cargo)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    assert!(stdout(&output).contains("try: cargo binstall --force deadreckon"));
}

#[test]
fn update_source_refuses_with_cargo_install_path() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Source)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("update: channel = source"), "{err}");
    assert!(
        err.contains("try: cargo install --path crates/deadreckon"),
        "{err}"
    );
}

fn paths(temp: &TempDir) -> DeadreckonPaths {
    DeadreckonPaths::from_home(temp.path().join("home"))
}

fn receipt(channel: Channel) -> Receipt {
    Receipt {
        channel,
        channel_version: "0.1.0".to_string(),
        binary_path: PathBuf::from("/usr/local/bin/deadreckon"),
        installed_at: Utc::now(),
        install_source: Some("test".to_string()),
        platform_package: (channel == Channel::Npm).then(|| "deadreckon-darwin-arm64".to_string()),
        receipt_version: INSTALL_RECEIPT_VERSION,
    }
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
