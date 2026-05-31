use std::ffi::OsString;
use std::fs;

use tempfile::TempDir;

use super::*;

fn joined_paths(paths: &[&std::path::Path]) -> OsString {
    std::env::join_paths(paths).expect("join paths")
}

#[test]
fn command_exists_resolves_path_and_bare_name() {
    let temp = TempDir::new().expect("tempdir");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin dir");
    fs::write(bin.join("deadreckon-p8-tool"), b"#!/bin/sh\n").expect("tool");

    assert!(command_exists_in_paths(
        "deadreckon-p8-tool",
        Some(joined_paths(&[bin.as_path()]))
    ));
    assert!(!command_exists_in_paths(
        "missing-deadreckon-p8-tool",
        Some(joined_paths(&[bin.as_path()]))
    ));
    assert!(!command_exists_in_paths("deadreckon-p8-tool", None));
}

#[test]
fn start_command_exists_explicit_path_branch_preserved() {
    let temp = TempDir::new().expect("tempdir");
    let bin = temp.path().join("bin");
    let explicit_dir = temp.path().join("explicit");
    fs::create_dir_all(&bin).expect("bin dir");
    fs::create_dir_all(&explicit_dir).expect("explicit dir");
    fs::write(bin.join("deadreckon-p8-tool"), b"#!/bin/sh\n").expect("path tool");

    let explicit_tool = explicit_dir.join("deadreckon-p8-tool");
    fs::write(&explicit_tool, b"#!/bin/sh\n").expect("explicit tool");
    assert!(command_exists_in_paths(
        explicit_tool.to_str().expect("utf8 path"),
        None
    ));

    let missing_explicit = explicit_dir.join("missing-deadreckon-p8-tool");
    assert!(!command_exists_in_paths(
        missing_explicit.to_str().expect("utf8 path"),
        Some(joined_paths(&[bin.as_path()]))
    ));
}
