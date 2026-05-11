use std::path::{Path, PathBuf};

use deadreckon_core::gate::{evaluate_acceptance, write_acceptance_marker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut run_id = None;
    let mut run_root = None;
    let mut working_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => run_id = args.next(),
            "--run-root" => run_root = args.next().map(PathBuf::from),
            "--working-dir" => working_dir = args.next().map(PathBuf::from),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let run_id = run_id.ok_or("--run is required")?;
    let working_dir = working_dir.ok_or("--working-dir is required")?;
    let run_root = match run_root {
        Some(run_root) => run_root,
        None => infer_run_root(&working_dir)?,
    };
    let results = evaluate_acceptance(&run_root, &working_dir)?;
    write_acceptance_marker(&run_root, run_id, working_dir, results.len())?;
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
