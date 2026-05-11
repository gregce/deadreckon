use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::state::PipelineState;

pub const ACCEPTANCE_MARKER: &str = "turn-acceptance.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,
    pub produced_by: String,
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
}

pub fn marker_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn validate_acceptance_marker(state: &PipelineState) -> Result<AcceptanceMarker> {
    // AS-BUILT §8/§17: completion is accepted only from an external marker
    // written by a binary runner and bound to this run_id.
    let path = marker_path(state);
    let raw = std::fs::read(&path).with_path(&path)?;
    let marker: AcceptanceMarker = serde_json::from_slice(&raw).with_json_path(&path)?;
    if marker.schema_version != 1 {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported acceptance marker schema {}",
            marker.schema_version
        )));
    }
    if marker.run_id != state.run_id {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance marker run_id {} does not match {}",
            marker.run_id, state.run_id
        )));
    }
    if marker.status != "pass" || marker.produced_by != "dr-gate" {
        return Err(DeadreckonError::InvalidInput(
            "acceptance marker was not produced by dr-gate with pass status".to_string(),
        ));
    }
    Ok(marker)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::{ACCEPTANCE_MARKER, AcceptanceMarker, validate_acceptance_marker};

    #[test]
    fn rejects_agent_written_marker_with_wrong_run_id() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
            },
        )
        .expect("run");
        let proofs = state.run_root.join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let marker = AcceptanceMarker {
            schema_version: 1,
            run_id: "wrong-run".to_string(),
            status: "pass".to_string(),
            produced_by: "agent".to_string(),
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject");
        assert!(err.to_string().contains("does not match"));
    }
}
