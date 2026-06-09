#![allow(clippy::expect_used)]

//! Guard: no developer-machine path may be compiled into the product or
//! documented as the way to use it. The release-trust closure rider added this
//! discipline for the test suite; this extends it to the shipped surface:
//! crate sources (including fixtures and goldens), installers, the npm
//! wrapper, the Makefile, and user-facing docs.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: &str = concat!("/Users/", "gdc");

const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "toml", "sh", "js", "mjs", "json", "md", "golden", "sql", "yml", "yaml", "txt", "cast",
];

#[test]
fn no_developer_machine_paths_in_shipped_surface() {
    let root = workspace_root();
    let mut findings = Vec::new();
    for dir in ["crates", "release", "npm"] {
        scan_tree(&root.join(dir), &mut findings);
    }
    for file in ["Makefile", "HOWTO.md", "README.md", "dist-workspace.toml"] {
        scan_file(&root.join(file), &mut findings);
    }
    assert!(
        findings.is_empty(),
        "developer-machine path {FORBIDDEN} found in shipped surface; \
         derive from $HOME, DEADRECKON_HOME, or source_root() instead:\n{}",
        findings.join("\n")
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn scan_tree(dir: &Path, findings: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            scan_tree(&path, findings);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext))
            || name.ends_with(".sh")
            || !name.contains('.')
        {
            scan_file(&path, findings);
        }
    }
}

fn scan_file(path: &Path, findings: &mut Vec<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    for (idx, line) in contents.lines().enumerate() {
        if line.contains(FORBIDDEN) {
            findings.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
        }
    }
}
