use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, Result};
use crate::gate::validate_acceptance_marker;
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, save_state};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub scope: String,
    pub goal: String,
    pub promoted_at: DateTime<Utc>,
    pub source_working_dir: PathBuf,
    pub provenance_hash: String,
}

pub fn promote_completed_run(
    paths: &DeadreckonPaths,
    state: &mut PipelineState,
) -> Result<PathBuf> {
    validate_acceptance_marker(state)?;
    recover_promotion(paths, state)?;
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if library_dir.exists() && library_dir.join("manifest.json").exists() {
        state.working_dir = library_dir.clone();
        state.promoted_library_dir = Some(library_dir.clone());
        save_state(state)?;
        return Ok(library_dir);
    }

    let parent = library_dir.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "library path has no parent: {}",
            library_dir.display()
        ))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let staging = parent.join(format!(".{}.promoting", state.run_id));
    if staging.exists() {
        fs::remove_dir_all(&staging).with_path(&staging)?;
    }
    if !state.working_dir.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "working directory {}",
            state.working_dir.display()
        )));
    }
    fs::rename(&state.working_dir, &staging).with_path(&state.working_dir)?;
    write_manifest(state, &staging, state.working_dir.clone())?;
    fs::rename(&staging, &library_dir).with_path(&staging)?;
    state.working_dir = library_dir.clone();
    state.promoted_library_dir = Some(library_dir.clone());
    save_state(state)?;
    Ok(library_dir)
}

pub fn recover_promotion(paths: &DeadreckonPaths, state: &mut PipelineState) -> Result<()> {
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let parent = library_dir.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "library path has no parent: {}",
            library_dir.display()
        ))
    })?;
    let staging = parent.join(format!(".{}.promoting", state.run_id));
    if staging.exists() && !library_dir.exists() {
        write_manifest(state, &staging, state.run_root.join("working"))?;
        fs::rename(&staging, &library_dir).with_path(&staging)?;
    } else if staging.exists() {
        fs::remove_dir_all(&staging).with_path(&staging)?;
    }
    if library_dir.exists() && !library_dir.join("manifest.json").exists() {
        write_manifest(state, &library_dir, state.run_root.join("working"))?;
    }
    Ok(())
}

fn write_manifest(
    state: &PipelineState,
    library_dir: &Path,
    source_working_dir: PathBuf,
) -> Result<()> {
    fs::create_dir_all(library_dir).with_path(library_dir)?;
    let manifest = PromotionManifest {
        schema_version: 1,
        run_id: state.run_id.clone(),
        scope: state.scope.clone(),
        goal: state.goal.clone(),
        promoted_at: Utc::now(),
        source_working_dir,
        provenance_hash: provenance_hash(&state.run_root.join("provenance.jsonl"))?,
    };
    let path = library_dir.join("manifest.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )
    .with_path(path)
}

fn provenance_hash(path: &Path) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    match fs::read(path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => 0_u8.hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::gate::write_acceptance_marker;
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::promote_completed_run;

    #[test]
    fn promotion_atomic_under_crash() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "promote".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("write");
        write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("marker");
        let library = paths.library_dir(&state.scope, &state.run_id);
        fs::create_dir_all(&library).expect("library");
        fs::write(library.join("notes.md"), "dead reckoning").expect("orphan");

        let promoted = promote_completed_run(&paths, &mut state).expect("promote");
        assert_eq!(promoted, library);
        assert!(promoted.join("manifest.json").exists());
    }
}
