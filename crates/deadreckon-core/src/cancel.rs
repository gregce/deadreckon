use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::state::PipelineState;

pub const CANCEL_MARKER: &str = "cancel.marker";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelMarker {
    pub run_id: String,
    pub requested_at: DateTime<Utc>,
    pub reason: String,
}

pub fn cancel_marker_path(state: &PipelineState) -> PathBuf {
    cancel_marker_path_for_run_root(&state.run_root)
}

pub fn cancel_marker_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(CANCEL_MARKER)
}

pub fn write_cancel_marker(state: &PipelineState, reason: impl Into<String>) -> Result<PathBuf> {
    let path = cancel_marker_path(state);
    let marker = CancelMarker {
        run_id: state.run_id.clone(),
        requested_at: Utc::now(),
        reason: reason.into(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&marker).with_json_path(&path)?,
    )
    .with_path(&path)?;
    Ok(path)
}

pub fn clear_cancel_marker(state: &PipelineState) -> Result<()> {
    let path = cancel_marker_path(state);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

pub fn cancel_marker_present(state: &PipelineState) -> bool {
    cancel_marker_path(state).exists()
}
