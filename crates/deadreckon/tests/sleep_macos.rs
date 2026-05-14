#![allow(clippy::expect_used)]

#[cfg(target_os = "macos")]
mod macos {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use deadreckon::sleep::{
        SkipReason, SleepMetadata, SleepMode, SleepPrefs, SleepPrevention, arm_with_tty,
        metadata_path, preview_with_binary_lookup,
    };
    use tempfile::TempDir;

    #[test]
    fn prevent_sleep_macos_spawns_caffeinate_child_when_tty() {
        require_caffeinate();
        let temp = TempDir::new().expect("temp");
        let prevention = arm_with_tty(SleepPrefs::On, temp.path(), true).expect("arm");
        let metadata = read_metadata(temp.path());

        assert!(matches!(prevention, SleepPrevention::Active { .. }));
        assert_eq!(metadata.mode, SleepMode::Caffeinate);
        let pid = metadata.pid.expect("pid");
        assert!(deadreckon_core::pid_is_alive(pid), "pid={pid}");
        drop(prevention);
    }

    #[test]
    fn prevent_sleep_macos_drop_reaps_caffeinate_within_500ms() {
        require_caffeinate();
        let temp = TempDir::new().expect("temp");
        let prevention = arm_with_tty(SleepPrefs::On, temp.path(), true).expect("arm");
        let pid = read_metadata(temp.path()).pid.expect("pid");

        drop(prevention);

        let deadline = Instant::now() + Duration::from_millis(750);
        while Instant::now() < deadline && deadreckon_core::pid_is_alive(pid) {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!deadreckon_core::pid_is_alive(pid), "pid={pid}");
    }

    #[test]
    fn prevent_sleep_macos_writes_and_removes_metadata_file() {
        require_caffeinate();
        let temp = TempDir::new().expect("temp");
        let path = metadata_path(temp.path());
        {
            let prevention = arm_with_tty(SleepPrefs::On, temp.path(), true).expect("arm");
            assert!(path.is_file(), "{}", path.display());
            drop(prevention);
        }
        assert!(!path.exists(), "{}", path.display());
    }

    #[test]
    fn prevent_sleep_off_does_not_spawn_caffeinate() {
        let temp = TempDir::new().expect("temp");
        let prevention = arm_with_tty(SleepPrefs::Off, temp.path(), true).expect("arm");

        assert!(matches!(
            prevention,
            SleepPrevention::Skipped {
                reason: SkipReason::UserDisabled
            }
        ));
        assert!(!metadata_path(temp.path()).exists());
    }

    #[test]
    fn prevent_sleep_auto_skips_when_non_tty() {
        let temp = TempDir::new().expect("temp");
        let prevention = arm_with_tty(SleepPrefs::Auto, temp.path(), false).expect("arm");

        assert!(matches!(
            prevention,
            SleepPrevention::Skipped {
                reason: SkipReason::NonTty
            }
        ));
        assert!(!metadata_path(temp.path()).exists());
    }

    #[test]
    fn prevent_sleep_macos_handles_missing_binary_with_unavailable_skip() {
        let preview = preview_with_binary_lookup(SleepPrefs::On, true, |_| None);

        assert_eq!(preview.mode, SleepMode::Unsupported);
        assert_eq!(preview.skip_reason, Some(SkipReason::UnavailableBinary));
    }

    fn require_caffeinate() {
        assert!(
            Path::new("/usr/bin/caffeinate").is_file(),
            "macOS caffeinate is required for this depth test"
        );
    }

    fn read_metadata(working_dir: &Path) -> SleepMetadata {
        let raw = fs::read_to_string(metadata_path(working_dir)).expect("metadata");
        serde_json::from_str(&raw).expect("metadata json")
    }
}
