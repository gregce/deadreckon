#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use deadreckon_core::DeadreckonPaths;
use deadreckon_core::install_receipt::{
    Channel, INSTALL_RECEIPT_VERSION, Receipt, read_receipt, receipt_path, write_receipt,
};
use deadreckon_core::update_cache::{Cache, read_cache, write_cache};
use tempfile::TempDir;

mod common;

use common::{assert_success, deadreckon, stderr, stdout};

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
    assert!(out.contains("preview update npm"), "{out}");
    assert!(out.contains("channel: npm"), "{out}");
    assert!(out.contains("current: 0.1.0"), "{out}");
    assert!(out.contains("latest: 0.2.3"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\nbun update -g deadreckon"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
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
    let out = stdout(&output);
    assert!(out.contains("no-op update brew"), "{out}");
    assert!(out.contains("channel: brew"), "{out}");
    assert!(out.contains("current: 0.1.0"), "{out}");
    assert!(out.contains("latest: 0.1.0"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(out.contains("Recommended\ndeadreckon doctor"), "{out}");
    assert!(!out.contains("try:"), "{out}");
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
fn update_writes_receipt_when_missing_on_first_run() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert!(!output.status.success());
    let detected = read_receipt(&paths)
        .expect("read receipt")
        .expect("detected receipt");
    assert_eq!(detected.channel, Channel::Source);
    assert_eq!(detected.receipt_version, INSTALL_RECEIPT_VERSION);
}

#[test]
fn update_npm_prints_bun_update_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("blocked update npm"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\nbun update -g deadreckon"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn update_brew_prints_brew_upgrade_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Brew)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("blocked update brew"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\nbrew upgrade gdc/tap/deadreckon"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn update_cargo_prints_binstall_or_install_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Cargo)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("blocked update cargo"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\ncargo binstall --force deadreckon"),
        "{out}"
    );
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn update_source_refuses_with_cargo_install_path() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Source)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("blocked update source"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ncargo install --path crates/deadreckon"),
        "{err}"
    );
    assert!(err.contains("channel: source"), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
}

#[test]
fn update_shell_writes_backup_before_swap() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    assert_eq!(fs::read(&binary).expect("binary"), b"new binary");
    let backup = newest_backup(&paths);
    assert_eq!(
        fs::read(backup.join("deadreckon")).expect("backup binary"),
        b"old binary"
    );
    assert!(backup.join("receipt.json").exists());
}

#[test]
fn update_shell_prunes_backups_to_three() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");
    for index in 0..4 {
        fs::create_dir_all(
            paths
                .home()
                .join("update-backups")
                .join(format!("2000010100000{index}")),
        )
        .expect("old backup");
    }

    let output = deadreckon(&paths)
        .args(["update", "--yes"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    let backups = backup_dirs(&paths);
    assert_eq!(backups.len(), 3, "{backups:?}");
}

#[test]
fn update_shell_swap_failure_preserves_binary() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    write_file(&binary, b"old binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_FAIL", "1")
        .output()
        .expect("update");

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert_eq!(fs::read(&binary).expect("binary"), b"old binary");
    let err = stderr(&output);
    assert!(err.contains("failed update shell"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(err.contains("Recommended\ncp "), "{err}");
    assert!(err.contains("backup:"), "{err}");
    assert!(err.contains("updated:"), "{err}");
    assert!(err.contains("source: test requested swap failure"), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
    assert!(newest_backup(&paths).join("deadreckon").exists());
}

#[test]
fn update_shell_previews_before_swap() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes"])
        .env("DEADRECKON_UPDATE_TEST_LATEST_VERSION", "0.2.3")
        .env(
            "DEADRECKON_UPDATE_TEST_ARCHIVE_URL",
            "https://github.com/gdc/deadreckon/releases/download/v0.2.3/deadreckon.tar.xz",
        )
        .env("DEADRECKON_UPDATE_TEST_SHA256", "abc123")
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("completed update shell"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(out.contains("target: 0.2.3"), "{out}");
    assert!(
        out.contains(
            "archive: https://github.com/gdc/deadreckon/releases/download/v0.2.3/deadreckon.tar.xz"
        ),
        "{out}"
    );
    assert!(out.contains("sha256: abc123"), "{out}");
    assert!(out.contains("backup:"), "{out}");
}

#[test]
fn update_shell_requires_yes_under_non_tty() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    write_file(&binary, b"old binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths).arg("update").output().expect("update");

    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("blocked update shell"), "{err}");
    assert!(err.contains("Explanation\n"), "{err}");
    assert!(err.contains("Evidence\n"), "{err}");
    assert_eq!(err.matches("\nRecommended\n").count(), 1, "{err}");
    assert!(
        err.contains("Recommended\ndeadreckon update --yes"),
        "{err}"
    );
    assert!(err.contains("channel: shell"), "{err}");
    assert!(err.contains("updated:"), "{err}");
    assert!(!err.contains("try:"), "{err}");
    assert!(!err.contains("hint:"), "{err}");
    assert!(!paths.home().join("update-backups").exists());
}

#[test]
fn update_success_prints_doctor_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("completed update shell"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(out.contains("Recommended\ndeadreckon doctor"), "{out}");
    assert!(!out.contains("try:"), "{out}");
}

#[test]
fn update_quiet_suppresses_lifecycle_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes", "--quiet"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    assert!(!stdout(&output).contains("deadreckon doctor"));
}

#[test]
fn update_plain_strips_color() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let binary = temp.path().join("bin/deadreckon");
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&binary, b"old binary");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &shell_receipt(&binary)).expect("receipt");

    let output = deadreckon(&paths)
        .args(["update", "--yes", "--plain"])
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(!out.contains("\u{1b}["), "{out}");
}

#[test]
fn update_shell_rejects_swap_on_non_shell_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    let replacement = temp.path().join("replacement/deadreckon");
    write_file(&replacement, b"new binary");
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");

    let output = deadreckon(&paths)
        .arg("update")
        .env("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT", &replacement)
        .output()
        .expect("update");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("blocked update npm"), "{out}");
    assert_eq!(out.matches("\nRecommended\n").count(), 1, "{out}");
    assert!(
        out.contains("Recommended\nbun update -g deadreckon"),
        "{out}"
    );
    assert!(!paths.home().join("update-backups").exists());
}

#[test]
fn startup_check_skips_under_non_tty() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");
    write_update_available_cache(&paths, "0.2.3");

    let output = deadreckon(&paths)
        .args(["list", "--plain"])
        .output()
        .expect("list");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("deadreckon 0.2.3 is available"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn startup_check_skips_under_env_disable() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");
    write_update_available_cache(&paths, "0.2.3");

    let output = deadreckon(&paths)
        .args(["list", "--plain"])
        .env("DEADRECKON_UPDATE_TEST_TTY", "1")
        .env("DEADRECKON_UPDATE_CHECK", "0")
        .output()
        .expect("list");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("deadreckon 0.2.3 is available"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn startup_check_skips_for_source_channel() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Source)).expect("receipt");
    write_update_available_cache(&paths, "0.2.3");

    let output = deadreckon(&paths)
        .args(["list", "--plain"])
        .env("DEADRECKON_UPDATE_TEST_TTY", "1")
        .output()
        .expect("list");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("deadreckon 0.2.3 is available"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn startup_check_skips_for_update_subcommand() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");
    write_update_available_cache(&paths, "0.2.3");

    let output = deadreckon(&paths)
        .args(["update", "--check"])
        .env("DEADRECKON_UPDATE_TEST_TTY", "1")
        .output()
        .expect("update");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("deadreckon 0.2.3 is available"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn startup_check_does_not_block_subcommand_exit() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");

    let started = Instant::now();
    let output = deadreckon(&paths)
        .args(["list", "--plain"])
        .env("DEADRECKON_UPDATE_TEST_TTY", "1")
        .env("DEADRECKON_UPDATE_TEST_FETCH_DELAY_MS", "2000")
        .env("DEADRECKON_UPDATE_TEST_LATEST_VERSION", "0.2.3")
        .output()
        .expect("list");

    assert_success(&output);
    assert!(
        started.elapsed() < StdDuration::from_millis(750),
        "startup check blocked for {:?}",
        started.elapsed()
    );
}

#[test]
fn startup_check_prints_hint_when_cache_stale() {
    let temp = TempDir::new().expect("tempdir");
    let paths = paths(&temp);
    write_receipt(&paths, &receipt(Channel::Npm)).expect("receipt");
    write_update_available_cache(&paths, "0.2.3");

    let output = deadreckon(&paths)
        .args(["list", "--plain"])
        .env("DEADRECKON_UPDATE_TEST_TTY", "1")
        .output()
        .expect("list");

    assert_success(&output);
    assert!(
        stderr(&output).contains("deadreckon 0.2.3 is available. Run `deadreckon update`."),
        "{}",
        stderr(&output)
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

fn shell_receipt(binary_path: &std::path::Path) -> Receipt {
    let mut receipt = receipt(Channel::Shell);
    receipt.binary_path = binary_path.to_path_buf();
    receipt.install_source = Some("https://github.com/gdc/deadreckon/releases".to_string());
    receipt
}

fn write_update_available_cache(paths: &DeadreckonPaths, latest_version: &str) {
    write_cache(
        paths,
        &Cache {
            checked_at: Utc::now(),
            latest_version: latest_version.to_string(),
            current_version: "0.1.0".to_string(),
            release_url: format!(
                "https://github.com/gdc/deadreckon/releases/tag/v{latest_version}"
            ),
            update_available: true,
        },
    )
    .expect("cache");
}

fn write_file(path: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, bytes).expect("write file");
}

fn newest_backup(paths: &DeadreckonPaths) -> PathBuf {
    backup_dirs(paths)
        .pop()
        .expect("at least one update backup")
}

fn backup_dirs(paths: &DeadreckonPaths) -> Vec<PathBuf> {
    let mut backups = fs::read_dir(paths.home().join("update-backups"))
        .expect("backup dir")
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            path.is_dir().then_some(path)
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups
}
