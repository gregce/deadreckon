use std::path::{Path, PathBuf};

use deadreckon_core::gate::{
    evaluate_acceptance_checks_with_progress, write_acceptance_marker_with_results,
};

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
    let results = evaluate_acceptance_checks_with_progress(&run_root, &working_dir)?;
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        eprintln!("acceptance failed");
        for result in &results {
            let mark = if result.passed { "PASS" } else { "FAIL" };
            let required = if result.must_pass {
                "required"
            } else {
                "optional"
            };
            eprintln!("{mark} {required} {}: {}", result.kind, result.detail);
            if !result.passed {
                if let Some(stderr) = result.stderr.as_deref() {
                    eprintln!("stderr: {}", one_line(stderr, 500));
                }
                if let Some(stdout) = result.stdout.as_deref() {
                    eprintln!("stdout: {}", one_line(stdout, 500));
                }
            }
        }
        return Err(format!("required check failed: {}", failed.detail).into());
    }
    write_acceptance_marker_with_results(&run_root, run_id, working_dir, results)?;
    Ok(())
}

fn one_line(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() <= limit {
        compact
    } else {
        let clipped = compact.chars().take(limit).collect::<String>();
        format!("{clipped}...")
    }
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
