#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon::cards::exit_summary::{
    BranchDiffSummary, ExitSummaryInput, OutcomeKind, build_exit_summary_card,
};
use deadreckon::ui_card::{CardOptions, Tone, render_card};
use deadreckon_core::DeadreckonPaths;
use deadreckon_core::glossary::{
    DR_GATE_DESCRIPTION, NOUN_DONE_CONTRACT, NOUN_VERIFIED_RUN, PHRASE_VERIFIED_BY_DR_GATE,
    VERDICT_VERIFIED,
};
use tempfile::TempDir;

#[test]
fn plain_flag_strips_color_and_box_drawing_globally() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "plain preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
            "--plain",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(err.contains("+"), "{err}");
}

#[test]
fn no_color_env_implies_plain_for_cards() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("NO_COLOR", "1")
        .args([
            "run",
            "no color preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(err.contains("+"), "{err}");
}

#[test]
fn width_below_forty_uses_single_column_ascii_fallback() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("COLUMNS", "32")
        .args([
            "run",
            "narrow preview",
            "--smoke",
            "--preview",
            "--max-spend",
            "1",
        ])
        .output()
        .expect("preview");

    assert_success(&output);
    let err = stderr(&output);
    assert!(err.contains("deadreckon run preview"), "{err}");
    assert!(!err.contains("\u{1b}["), "{err:?}");
    assert!(!err.contains("╭"), "{err}");
    assert!(!err.contains("+"), "{err}");
}

#[test]
fn guarantee_noun_is_consistent_across_surfaces() {
    assert_eq!(NOUN_VERIFIED_RUN, "verified run");
    assert_eq!(PHRASE_VERIFIED_BY_DR_GATE, "verified by dr-gate");
    assert_eq!(DR_GATE_DESCRIPTION, "the process that verifies the run");
    assert_eq!(NOUN_DONE_CONTRACT, "done contract");
    assert_eq!(VERDICT_VERIFIED, "VERIFIED");

    let rendered = render_card(
        &build_exit_summary_card(&exit_input()),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(140),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("* VERIFIED"), "{rendered}");
    assert!(rendered.contains(NOUN_VERIFIED_RUN), "{rendered}");
    assert!(rendered.contains(PHRASE_VERIFIED_BY_DR_GATE), "{rendered}");
    assert!(!rendered.contains("* completed run"), "{rendered}");

    let main = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("main");
    assert!(main.contains("NOUN_VERIFIED_RUN"), "{main}");
    assert!(main.contains("NOUN_DONE_CONTRACT"), "{main}");
    assert!(
        !main.contains("\"verified run\""),
        "user-facing verified-run copy must use the glossary constant"
    );
}

#[test]
fn every_refusal_carries_a_try_line() {
    struct RefusalCase {
        name: &'static str,
        args: Vec<&'static str>,
        cwd: RefusalCwd,
        empty_path: bool,
    }

    #[derive(Clone, Copy)]
    enum RefusalCwd {
        PlainDir,
        GitRepo,
    }

    let cases = [
        RefusalCase {
            name: "run without source mode",
            args: vec!["run", "refuse", "--smoke", "--max-spend", "1", "--yes"],
            cwd: RefusalCwd::PlainDir,
            empty_path: false,
        },
        RefusalCase {
            name: "start without provider",
            args: vec!["start", "build the app", "--plain"],
            cwd: RefusalCwd::GitRepo,
            empty_path: true,
        },
        RefusalCase {
            name: "def-done add without criterion",
            args: vec!["def-done", "add"],
            cwd: RefusalCwd::GitRepo,
            empty_path: false,
        },
        RefusalCase {
            name: "def-done check missing spec",
            args: vec!["def-done", "check", "--spec", "missing.yaml"],
            cwd: RefusalCwd::GitRepo,
            empty_path: false,
        },
    ];

    for case in cases {
        let temp = repo_tempdir();
        let cwd = match case.cwd {
            RefusalCwd::PlainDir => {
                let dir = temp.path().join("not-git");
                fs::create_dir_all(&dir).expect("plain dir");
                dir
            }
            RefusalCwd::GitRepo => clean_git_repo(&temp),
        };
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut command = deadreckon(&paths);
        command.current_dir(&cwd).args(case.args);
        let empty_bin;
        if case.empty_path {
            empty_bin = temp.path().join("empty-bin");
            fs::create_dir_all(&empty_bin).expect("empty bin");
            command.env("PATH", &empty_bin);
        }

        let output = command.output().unwrap_or_else(|err| panic!("{}", err));
        assert!(
            !output.status.success(),
            "{name}\n{}{}",
            stdout(&output),
            stderr(&output),
            name = case.name
        );
        let err = stderr(&output);
        let last = err
            .lines()
            .filter(|line| !line.trim().is_empty())
            .last()
            .expect("last stderr line");
        assert!(last.contains("try:"), "{}\n{err}", case.name);
    }
}

fn exit_input() -> ExitSummaryInput {
    ExitSummaryInput {
        run_id: "abc123456789".to_string(),
        goal: "ship it".to_string(),
        provider: "cli:codex".to_string(),
        branch: Some("dr/ship-it-abc12345".to_string()),
        outcome: OutcomeKind::Completed,
        turns: 3,
        input_tokens: 100,
        output_tokens: 50,
        spend_usd: 0.0,
        approximate_spend: true,
        spend_label: "not metered (subscription)".to_string(),
        wall_seconds: 12.5,
        diff: Some(BranchDiffSummary {
            lines_added: 42,
            lines_deleted: 3,
            files_added: 1,
            files_updated: 2,
            files_deleted: 0,
        }),
        gate: "passed by dr-gate (2 checks)".to_string(),
        gate_tone: Tone::Neutral,
        tests_modified: Some(false),
        gate_caveats: Vec::new(),
        working_dir: PathBuf::from("/tmp/work"),
        proof_path: PathBuf::from("/tmp/run/proofs/turn-acceptance.json"),
        proof_block: None,
        hints: vec![("apply".to_string(), "deadreckon apply abc12345".to_string())],
    }
}

fn repo_tempdir() -> TempDir {
    TempDir::new().expect("tempdir")
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    git(
        &repo,
        &["config", "user.email", "deadreckon@example.invalid"],
    )
    .expect("email");
    git(&repo, &["config", "user.name", "deadreckon"]).expect("name");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    assert!(
        output.status.success(),
        "git {:?}\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
