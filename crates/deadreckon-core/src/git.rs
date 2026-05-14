use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::error::{DeadreckonError, Result};

const COMMIT_FAMILY_VERBS: &[&str] = &[
    "commit",
    "merge",
    "cherry-pick",
    "rebase",
    "tag",
    "am",
    "revert",
];

pub fn run_git(cwd: &Path, args: &[&str]) -> Result<Output> {
    git_command(cwd, args).output().map_err(git_io)
}

pub fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output> {
    let mut child = git_command(cwd, args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(git_io)?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(input)
            .map_err(|source| DeadreckonError::Io {
                path: cwd.to_path_buf(),
                source,
            })?;
    }
    child.wait_with_output().map_err(git_io)
}

pub fn git_command(cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    for arg in hardened_git_prefix(args) {
        command.arg(arg);
    }
    command.arg("-C").arg(cwd).args(args);
    command
}

pub fn hardened_git_prefix(args: &[&str]) -> Vec<&'static str> {
    let Some(verb) = first_git_verb(args) else {
        return Vec::new();
    };
    if COMMIT_FAMILY_VERBS.contains(&verb) {
        vec![
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "gpg.format=",
        ]
    } else {
        Vec::new()
    }
}

pub fn hardened_git_argv(cwd: &Path, args: &[&str]) -> Vec<OsString> {
    let mut argv = vec![OsString::from("git")];
    argv.extend(hardened_git_prefix(args).into_iter().map(OsString::from));
    argv.push(OsString::from("-C"));
    argv.push(cwd.as_os_str().to_os_string());
    argv.extend(args.iter().map(OsString::from));
    argv
}

fn first_git_verb<'a>(args: &'a [&str]) -> Option<&'a str> {
    args.iter().copied().find(|arg| !arg.starts_with('-'))
}

fn git_io(source: std::io::Error) -> DeadreckonError {
    DeadreckonError::Io {
        path: PathBuf::from("git"),
        source,
    }
}

#[allow(dead_code)]
fn _is_git_binary(binary: &OsStr) -> bool {
    binary == OsStr::new("git")
}
