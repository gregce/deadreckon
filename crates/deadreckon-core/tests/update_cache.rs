#![allow(clippy::expect_used)]

use chrono::{Duration, Utc};
use deadreckon_core::DeadreckonPaths;
use deadreckon_core::update_cache::{Cache, read_cache, write_cache};
use tempfile::TempDir;

#[test]
fn update_cache_is_stale_after_24h() {
    let now = Utc::now();
    let cache = Cache {
        checked_at: now - Duration::hours(25),
        latest_version: "0.2.3".to_string(),
        current_version: "0.2.0".to_string(),
        release_url: "https://github.com/gregce/deadreckon/releases/tag/v0.2.3".to_string(),
        update_available: true,
    };

    assert!(cache.is_stale(now));
}

#[test]
fn update_cache_round_trips_release_url() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cache = Cache {
        checked_at: Utc::now(),
        latest_version: "0.2.3".to_string(),
        current_version: "0.2.0".to_string(),
        release_url: "https://github.com/gregce/deadreckon/releases/tag/v0.2.3".to_string(),
        update_available: true,
    };

    write_cache(&paths, &cache).expect("write cache");
    let reloaded = read_cache(&paths)
        .expect("read cache")
        .expect("cache present");

    assert_eq!(reloaded.release_url, cache.release_url);
    assert_eq!(reloaded.update_available, cache.update_available);
}
