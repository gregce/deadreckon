use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, JsonContext, Result};
use crate::state::append_json_line;

pub const STEER_INBOX_JSONL: &str = "steer-inbox.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerSource {
    Cli,
    Tui,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SteerIdentity {
    pub ts: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerInboxEntry {
    pub ts: DateTime<Utc>,
    pub source: SteerSource,
    pub text: String,
    pub status: SteerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<DateTime<Utc>>,
}

impl SteerInboxEntry {
    pub fn identity(&self) -> SteerIdentity {
        SteerIdentity {
            ts: self.ts,
            text: self.text.clone(),
        }
    }
}

pub fn steer_inbox_path(run_root: &Path) -> PathBuf {
    run_root.join(STEER_INBOX_JSONL)
}

pub fn append_steer(
    run_root: &Path,
    source: SteerSource,
    text: impl Into<String>,
) -> Result<SteerInboxEntry> {
    let text = text.into();
    if text.trim().is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "steer text must not be empty".to_string(),
        ));
    }
    let entry = SteerInboxEntry {
        ts: Utc::now(),
        source,
        text,
        status: SteerStatus::Pending,
        delivered_turn_id: None,
        delivered_at: None,
    };
    append_json_line(&steer_inbox_path(run_root), &entry)?;
    Ok(entry)
}

pub fn read_steer_inbox(run_root: &Path) -> Result<Vec<SteerInboxEntry>> {
    let path = steer_inbox_path(run_root);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(DeadreckonError::Io { path, source }),
    };
    let mut effective = BTreeMap::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let entry: SteerInboxEntry = serde_json::from_str(line).with_json_path(&path)?;
        effective.insert(entry.identity(), entry);
    }
    Ok(effective.into_values().collect())
}

pub fn pending_steers(run_root: &Path) -> Result<Vec<SteerInboxEntry>> {
    Ok(read_steer_inbox(run_root)?
        .into_iter()
        .filter(|entry| entry.status == SteerStatus::Pending)
        .collect())
}

pub fn mark_steer_delivered(
    run_root: &Path,
    identity: &SteerIdentity,
    turn_id: &str,
) -> Result<SteerInboxEntry> {
    if turn_id.trim().is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "delivered turn id must not be empty".to_string(),
        ));
    }
    let mut entry = read_steer_inbox(run_root)?
        .into_iter()
        .find(|entry| entry.identity() == *identity)
        .ok_or_else(|| {
            DeadreckonError::NotFound(format!(
                "steer inbox entry at {} with text {:?}",
                identity.ts, identity.text
            ))
        })?;
    if entry.status == SteerStatus::Delivered {
        return Ok(entry);
    }
    entry.status = SteerStatus::Delivered;
    entry.delivered_turn_id = Some(turn_id.to_string());
    entry.delivered_at = Some(Utc::now());
    append_json_line(&steer_inbox_path(run_root), &entry)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{
        SteerSource, SteerStatus, append_steer, mark_steer_delivered, pending_steers,
        read_steer_inbox,
    };

    #[test]
    fn steer_inbox_appends_and_marks_delivered() {
        let temp = TempDir::new().expect("tempdir");
        let first =
            append_steer(temp.path(), SteerSource::Cli, "prefer sqlite").expect("append first");
        let second =
            append_steer(temp.path(), SteerSource::Tui, "ship the fix").expect("append second");

        mark_steer_delivered(temp.path(), &first.identity(), "turn_3").expect("mark delivered");

        let entries = read_steer_inbox(temp.path()).expect("read inbox");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, SteerStatus::Delivered);
        assert_eq!(entries[0].delivered_turn_id.as_deref(), Some("turn_3"));
        assert_eq!(entries[1], second);
        assert_eq!(pending_steers(temp.path()).expect("pending"), vec![second]);
        let rows = std::fs::read_to_string(temp.path().join("steer-inbox.jsonl")).expect("ledger");
        assert_eq!(rows.lines().count(), 3);
    }

    #[test]
    fn pending_lines_survive_process_restart() {
        let temp = TempDir::new().expect("tempdir");
        let appended = append_steer(temp.path(), SteerSource::Cli, "keep the public API stable")
            .expect("append");

        let reconstructed = read_steer_inbox(temp.path()).expect("read after restart");

        assert_eq!(reconstructed, vec![appended]);
        assert_eq!(reconstructed[0].status, SteerStatus::Pending);
    }
}
