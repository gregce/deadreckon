use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("CARGO_MANIFEST_DIR is not set"))?,
    );
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("deadreckon crate must live below the workspace root"))?;

    let mut inputs = vec![workspace.join("Cargo.toml"), workspace.join("Cargo.lock")];
    collect_gate_build_inputs(&workspace.join("crates"), &mut inputs)?;
    inputs.sort();

    let mut digest = Sha256::new();
    for path in inputs {
        let relative = path.strip_prefix(workspace)?;
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = relative.to_string_lossy().replace('\\', "/");
        digest.update(relative.as_bytes());
        digest.update([0]);
        let bytes = fs::read(&path)?;
        let normalized = String::from_utf8(bytes)?.replace("\r\n", "\n");
        digest.update(normalized.as_bytes());
        digest.update([0]);
    }

    let digest = digest.finalize();
    println!(
        "cargo:rustc-env=DEADRECKON_BUNDLE_BUILD_ID=deadreckon-bundle-build-id-sha256:{digest:x}"
    );
    Ok(())
}

fn collect_gate_build_inputs(root: &Path, inputs: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut crate_dirs = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    crate_dirs.sort();
    for crate_dir in crate_dirs {
        for manifest_input in [crate_dir.join("Cargo.toml"), crate_dir.join("build.rs")] {
            if manifest_input.is_file() {
                inputs.push(manifest_input);
            }
        }
        let source = crate_dir.join("src");
        if source.is_dir() {
            collect_rust_sources(&source, inputs)?;
        }
    }
    Ok(())
}

fn collect_rust_sources(root: &Path, inputs: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                inputs.push(path);
            }
        }
    }
    Ok(())
}
