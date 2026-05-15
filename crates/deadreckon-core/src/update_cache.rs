use std::fs;
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{IoContext, JsonContext};
use crate::{DeadreckonPaths, Result};

pub const UPDATE_CACHE_FILE: &str = "update-check.json";
pub const UPDATE_CACHE_TTL_HOURS: i64 = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    pub checked_at: DateTime<Utc>,
    pub latest_version: String,
    pub current_version: String,
    pub release_url: String,
    #[serde(rename = "is_stale")]
    pub update_available: bool,
}

impl Cache {
    #[must_use]
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        now.signed_duration_since(self.checked_at) >= Duration::hours(UPDATE_CACHE_TTL_HOURS)
    }
}

#[must_use]
pub fn cache_path(paths: &DeadreckonPaths) -> PathBuf {
    paths.home().join(UPDATE_CACHE_FILE)
}

pub fn read_cache(paths: &DeadreckonPaths) -> Result<Option<Cache>> {
    let path = cache_path(paths);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(crate::DeadreckonError::Io { path, source }),
    };
    serde_json::from_slice(&raw).with_json_path(path).map(Some)
}

pub fn write_cache(paths: &DeadreckonPaths, cache: &Cache) -> Result<()> {
    let path = cache_path(paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    let raw = serde_json::to_vec_pretty(cache).with_json_path(&path)?;
    fs::write(&path, raw).with_path(path)
}
