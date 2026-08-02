use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

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
    let mut command = git_command(cwd, args);
    run_git_command_with_input(cwd, &mut command, input)
}

/// Run a preconfigured Git command while communicating over all three pipes.
///
/// The input writer must run concurrently with stdout/stderr collection. Git
/// commands such as `check-attr --stdin` emit output while they consume input;
/// writing all input first can fill both pipe directions and deadlock.
pub fn run_git_command_with_input(
    cwd: &Path,
    command: &mut Command,
    input: &[u8],
) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| git_io_at(cwd, source))?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        DeadreckonError::InvalidInput("Git command did not expose its input pipe".to_string())
    })?;

    let (write_result, output_result) = thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        // `wait_with_output` drains stdout and stderr concurrently on supported
        // platforms. Starting it before joining the writer keeps every pipe
        // moving even when input and output both exceed the OS pipe capacity.
        let output = child.wait_with_output();
        (writer.join(), output)
    });
    let write_result = write_result.map_err(|_| {
        DeadreckonError::InvalidInput("Git input writer thread panicked".to_string())
    })?;
    write_result.map_err(|source| git_io_at(cwd, source))?;
    output_result.map_err(|source| git_io_at(cwd, source))
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

fn git_io_at(path: &Path, source: std::io::Error) -> DeadreckonError {
    DeadreckonError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::TempDir;

    use super::{run_git, run_git_with_input};

    #[test]
    fn git_with_input_captures_stdout_for_callers_that_parse_it() {
        let temp = TempDir::new().expect("tempdir");
        let output = run_git_with_input(temp.path(), &["hash-object", "--stdin"], b"fixture\n")
            .expect("hash stdin");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ee8c1ee49b4799bbd170233915a897c19e3b55e1"
        );
    }

    #[test]
    fn git_with_input_drains_large_bidirectional_output_without_deadlock() {
        let temp = TempDir::new().expect("tempdir");
        let init = run_git(temp.path(), &["init", "-q"]).expect("git init");
        assert!(init.status.success());

        let mut input = Vec::new();
        for index in 0..5_000_u32 {
            writeln!(input, "src/{index:05}-long-check-attribute-path.rs").expect("path input");
        }
        // `check-attr --stdin` expects newline-delimited input unless `-z` is
        // supplied. Its three-field output is comfortably larger than a 64 KiB
        // pipe, as is the input itself.
        assert!(input.len() > 64 * 1024);
        let output = run_git_with_input(temp.path(), &["check-attr", "--stdin", "filter"], &input)
            .expect("large check-attr communication");

        assert!(output.status.success());
        assert!(output.stdout.len() > 64 * 1024);
        assert_eq!(
            output.stdout.iter().filter(|byte| **byte == b'\n').count(),
            5_000
        );
    }
}
