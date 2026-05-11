use std::path::{Path, PathBuf};

use chrono::Utc;
use deadreckon_core::gate::{ACCEPTANCE_MARKER, AcceptanceMarker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut run_id = None;
    let mut working_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => run_id = args.next(),
            "--working-dir" => working_dir = args.next().map(PathBuf::from),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let run_id = run_id.ok_or("--run is required")?;
    let working_dir = working_dir.ok_or("--working-dir is required")?;
    if !working_dir.is_dir() {
        return Err(format!(
            "working directory does not exist: {}",
            working_dir.display()
        )
        .into());
    }
    if working_dir.join("Cargo.toml").exists() {
        let status = std::process::Command::new("cargo")
            .arg("test")
            .current_dir(&working_dir)
            .status()?;
        if !status.success() {
            return Err("cargo test failed in working directory".into());
        }
    }
    let run_root = infer_run_root(&working_dir)?;
    let proofs = run_root.join("proofs");
    std::fs::create_dir_all(&proofs)?;
    let marker = AcceptanceMarker {
        schema_version: 1,
        run_id,
        status: "pass".to_string(),
        produced_by: "dr-gate".to_string(),
        checked_at: Utc::now(),
        working_dir,
    };
    std::fs::write(
        proofs.join(ACCEPTANCE_MARKER),
        serde_json::to_vec_pretty(&marker)?,
    )?;
    Ok(())
}

fn infer_run_root(working_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if working_dir.file_name().and_then(|name| name.to_str()) != Some("working") {
        return Err("working directory must be <run-root>/working".into());
    }
    working_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "working directory has no parent".into())
}
