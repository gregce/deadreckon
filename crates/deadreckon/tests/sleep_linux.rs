#![allow(clippy::expect_used)]

#[cfg(target_os = "linux")]
use deadreckon::sleep::{SleepMode, preview_with_binary_lookup};
use deadreckon::sleep::{SleepPrefs, is_trusted_linux_ready_path, metadata_path, preview};
use tempfile::TempDir;

#[cfg(target_os = "linux")]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn prevent_sleep_trusted_path_validator_rejects_outside_tmp() {
    let temp = TempDir::new().expect("temp");
    let bad = temp.path().join("not-deadreckon").join("reexec-ready");
    assert!(!is_trusted_linux_ready_path(&bad));
    assert!(!is_trusted_linux_ready_path(
        &temp.path().join("deadreckon-sleep-x").join("ready")
    ));
}

#[test]
fn prevent_sleep_linux_refuses_untrusted_ready_path() {
    let temp = TempDir::new().expect("temp");
    let untrusted = temp.path().join("deadreckon-sleep-test").join("ready");

    assert!(!is_trusted_linux_ready_path(&untrusted));
}

#[test]
fn prevent_sleep_auto_skips_when_non_tty() {
    let preview = preview(SleepPrefs::Auto, false);
    assert_eq!(preview.label(), "none (non-tty)");
}

#[test]
fn prevent_sleep_metadata_path_lives_under_working_deadreckon() {
    let temp = TempDir::new().expect("temp");
    assert_eq!(
        metadata_path(temp.path()),
        temp.path()
            .join(".deadreckon")
            .join("sleep-prevention.json")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn prevent_sleep_linux_writes_ready_file_when_under_inhibitor() {
    use deadreckon::sleep::{SleepPrevention, arm_with_tty};

    let _guard = ENV_LOCK.lock().expect("env lock");
    let ready_dir =
        std::env::temp_dir().join(format!("deadreckon-sleep-test-{}", std::process::id()));
    let ready_path = ready_dir.join("reexec-ready");
    std::fs::create_dir_all(&ready_dir).expect("ready dir");
    let temp = TempDir::new().expect("temp");
    // SAFETY: this integration test serializes process environment mutation
    // with ENV_LOCK and restores both variables before returning.
    unsafe {
        std::env::set_var("DEADRECKON_SLEEP_INHIBITED", "1");
        std::env::set_var("DEADRECKON_SLEEP_REEXEC_READY_PATH", &ready_path);
    }

    let prevention = arm_with_tty(SleepPrefs::On, temp.path(), true).expect("arm");

    // SAFETY: see the set_var block above.
    unsafe {
        std::env::remove_var("DEADRECKON_SLEEP_INHIBITED");
        std::env::remove_var("DEADRECKON_SLEEP_REEXEC_READY_PATH");
    }
    assert!(matches!(prevention, SleepPrevention::Active { .. }));
    assert_eq!(
        std::fs::read_to_string(&ready_path).expect("ready"),
        "ready\n"
    );
    let _ = std::fs::remove_dir_all(&ready_dir);
}

#[cfg(target_os = "linux")]
#[test]
fn prevent_sleep_linux_falls_back_when_systemd_inhibit_missing() {
    let preview = preview_with_binary_lookup(SleepPrefs::On, true, |_| None);

    assert_eq!(preview.mode, SleepMode::Unsupported);
}

#[cfg(target_os = "linux")]
#[test]
fn prevent_sleep_linux_timeout_after_five_seconds_does_not_hang_run() {
    let preview = preview_with_binary_lookup(SleepPrefs::On, true, |_| {
        Some(std::path::PathBuf::from("/usr/bin/systemd-inhibit"))
    });

    assert!(matches!(
        preview.mode,
        SleepMode::SystemdInhibit | SleepMode::Unsupported
    ));
}

#[cfg(windows)]
#[test]
fn prevent_sleep_windows_skipped_with_unsupported_reason() {
    use deadreckon::sleep::{SkipReason, SleepPrevention, arm_with_tty};

    let temp = TempDir::new().expect("temp");
    let prevention = arm_with_tty(SleepPrefs::On, temp.path(), true).expect("arm");

    assert!(matches!(
        prevention,
        SleepPrevention::Skipped {
            reason: SkipReason::Unsupported
        }
    ));
}
