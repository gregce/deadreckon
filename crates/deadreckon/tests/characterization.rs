#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use deadreckon_core::DeadreckonPaths;
use regex::Regex;
use tempfile::TempDir;

/// Golden output embeds absolute paths, and the *length* of the raw path
/// decides kv wrap points, path truncation points, and even the smoke
/// provider's prompt-length-derived token counts. Build every test workspace
/// at one fixed canonical path length on every platform (`/tmp` is
/// `/private/tmp` on macOS, `/tmp` on Linux — pad to a shared prefix) so the
/// goldens are byte-stable across operating systems.
fn fixed_length_tempdir() -> TempDir {
    let base = Path::new("/tmp").canonicalize().expect("canonical /tmp");
    const PREFIX_LEN: usize = 24;
    let pad_len = PREFIX_LEN
        .checked_sub(base.as_os_str().len() + 1)
        .expect("/tmp canonical prefix too long for the fixed-length test root");
    let root = base.join("d".repeat(pad_len.max(1)));
    fs::create_dir_all(&root).expect("padded test root");
    tempfile::Builder::new()
        .prefix("c")
        .tempdir_in(root)
        .expect("tempdir")
}

#[test]
fn plan_draft_stdout_matches_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "characterize plan output",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--plain",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    assert_capture_matches_golden("plan-draft.golden", &temp, &output);
}

#[test]
fn plan_quiet_stdout_matches_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "plan",
            "characterize quiet plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--quiet",
            "--plain",
        ])
        .output()
        .expect("plan quiet");

    assert_success(&output);
    assert_capture_matches_golden("plan-quiet.golden", &temp, &output);
}

#[test]
fn orchestrate_preview_json_matches_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "start",
            "characterize orchestration",
            "--mode",
            "full-plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
            "--json",
            "--plain",
        ])
        .output()
        .expect("start preview json");

    assert_success(&output);
    assert_capture_matches_golden("orchestrate-preview-json.golden", &temp, &output);
}

#[test]
fn chain_status_table_matches_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let draft = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--draft",
            "first step",
            "second step",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--plain",
        ])
        .output()
        .expect("chain draft");
    assert_success(&draft);

    let status = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "status", "--plain"])
        .output()
        .expect("chain status");

    assert_success(&status);
    assert_capture_matches_golden("chain-status-table.golden", &temp, &status);
}

#[test]
fn characterization_goldens_unchanged_after_split() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = characterization_deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "attach characterize",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--fresh",
            "--no-docs",
            "--plain",
        ])
        .output()
        .expect("smoke run");
    assert_success(&run);
    let run_prefix = started_run_prefix(&stdout(&run));

    let attach = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &run_prefix, "--plain"])
        .output()
        .expect("attach");

    assert_success(&attach);
    assert_capture_matches_golden("attach-off-tty-frame.golden", &temp, &attach);
}

#[test]
fn attach_characterization_goldens_unchanged() {
    assert_attach_characterization_golden();
}

#[test]
fn attach_goldens_unchanged_after_reader_rewire() {
    assert_attach_characterization_golden();
}

fn assert_attach_characterization_golden() {
    // Logbook P9: attach's projections are backed by the shared RunView model;
    // the pinned plain-attach frame must stay byte-identical to the golden
    // recorded before the rewire. Same command, same golden — this test names
    // the rider contract so the pin survives future refactors by name.
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = characterization_deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "attach characterize",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--fresh",
            "--no-docs",
            "--plain",
        ])
        .output()
        .expect("smoke run");
    assert_success(&run);
    let run_prefix = started_run_prefix(&stdout(&run));

    let attach = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &run_prefix, "--plain"])
        .output()
        .expect("attach");

    assert_success(&attach);
    assert_capture_matches_golden("attach-off-tty-frame.golden", &temp, &attach);
}

// Logbook P4/P6/P7 parity guards: `show`, `verdict`, and `doc` are projections
// of the shared RunView model; these goldens pin their default output so a
// future RunView change cannot silently drift what the operator reads.

#[test]
fn show_default_output_matches_characterization_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = smoke_run(&paths, &repo);
    let run_prefix = started_run_prefix(&stdout(&run));

    let show = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", &run_prefix])
        .output()
        .expect("show");

    assert_success(&show);
    assert_capture_matches_golden("show-default-run.golden", &temp, &show);
}

#[test]
fn verdict_default_output_matches_characterization_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = smoke_run(&paths, &repo);
    let run_prefix = started_run_prefix(&stdout(&run));

    let verdict = deadreckon(&paths)
        .current_dir(&repo)
        .args(["verdict", &run_prefix])
        .output()
        .expect("verdict");

    assert_success(&verdict);
    assert_capture_matches_golden("verdict-default-run.golden", &temp, &verdict);
}

#[test]
fn doc_default_output_matches_characterization_golden() {
    let temp = fixed_length_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let run = smoke_run(&paths, &repo);
    let run_prefix = started_run_prefix(&stdout(&run));

    // The smoke run generates no docs (--no-docs); write a deterministic
    // narrative into the promoted artifact so `doc` has real content to
    // project through RunView.why.
    let working = single_library_working_dir(&paths);
    let docs = working.join("docs");
    fs::create_dir_all(&docs).expect("docs dir");
    fs::write(
        docs.join("RUN-NARRATIVE.md"),
        "# Run narrative\n\ncharacterized narrative body\n",
    )
    .expect("narrative doc");

    let doc = deadreckon(&paths)
        .current_dir(&repo)
        .args(["doc", &run_prefix])
        .output()
        .expect("doc");

    assert_success(&doc);
    assert_capture_matches_golden("doc-default-run.golden", &temp, &doc);
}

fn smoke_run(paths: &DeadreckonPaths, repo: &Path) -> Output {
    let run = characterization_deadreckon(paths)
        .current_dir(repo)
        .args([
            "run",
            "attach characterize",
            "--smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--fresh",
            "--no-docs",
            "--plain",
        ])
        .output()
        .expect("smoke run");
    assert_success(&run);
    run
}

fn single_library_working_dir(paths: &DeadreckonPaths) -> PathBuf {
    let library = paths.home().join("library");
    let scope = fs::read_dir(&library)
        .expect("library dir")
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.path().is_dir())
        .expect("one library scope");
    fs::read_dir(scope.path())
        .expect("scope dir")
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.path().is_dir())
        .expect("one library artifact")
        .path()
}

#[test]
fn error_footers_match_canonical_goldens() {
    let temp = fixed_length_tempdir();
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain).expect("plain dir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let depth = deadreckon(&paths)
        .current_dir(&plain)
        .env("DEADRECKON_CAMPAIGN_DEPTH", "1")
        .args([
            "campaign",
            "nested campaign",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
            "--plain",
        ])
        .output()
        .expect("campaign depth refusal");
    assert_status_code(&depth, 1);

    let worktree = deadreckon(&paths)
        .current_dir(&plain)
        .args([
            "run",
            "plain dir refusal",
            "--provider",
            "smoke",
            "--preview",
            "--plain",
            "--worktree",
        ])
        .output()
        .expect("plain worktree refusal");
    assert_status_code(&worktree, 1);

    let actual = format!(
        "case: campaign-depth\n{}\ncase: non-git-worktree\n{}",
        normalized_capture(&temp, &depth),
        normalized_capture(&temp, &worktree)
    );
    assert_text_matches_golden("error-footers.golden", &actual);
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .env("DEADRECKON_HOME", paths.home())
        .env("NO_COLOR", "1")
        .env("COLUMNS", "120")
        .env("RUST_BACKTRACE", "0")
        .env_remove("DEADRECKON_CAMPAIGN_DEPTH")
        .env_remove("DEADRECKON_CAMPAIGN_ROOT")
        .env_remove("DEADRECKON_CAMPAIGN_ANCESTOR_TASK_KEYS")
        .env_remove("DEADRECKON_CAMPAIGN_ANCESTOR_SCOPES")
        .env_remove("DEADRECKON_CAMPAIGN_SUB_RESULT");
    command
}

fn characterization_deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon-characterization"));
    command
        .env("DEADRECKON_HOME", paths.home())
        .env("NO_COLOR", "1")
        .env("COLUMNS", "120")
        .env("RUST_BACKTRACE", "0")
        // The characterization binary launches nested Cargo smoke checks.
        // Keep those checks inside the fixed test workspace even when the
        // outer verifier uses a shared target directory for its own build.
        .env_remove("CARGO_TARGET_DIR");
    command
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo dir");
    run_git(&repo, ["init", "--initial-branch", "main"]);
    run_git(
        &repo,
        ["config", "user.email", "deadreckon@example.invalid"],
    );
    run_git(&repo, ["config", "user.name", "deadreckon"]);
    fs::write(repo.join("README.md"), "hello\n").expect("readme");
    run_git(&repo, ["add", "README.md"]);
    run_git(&repo, ["commit", "-m", "initial"]);
    repo
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git");
    assert_success(&output);
}

fn assert_capture_matches_golden(name: &str, temp: &TempDir, output: &Output) {
    let actual = normalized_capture(temp, output);
    assert_text_matches_golden(name, &actual);
}

fn assert_text_matches_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if std::env::var_os("DEADRECKON_UPDATE_GOLDENS").is_some() {
        fs::write(&path, actual).expect("update golden");
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "missing golden {}: {err}\n--- actual ---\n{}",
            path.display(),
            actual
        )
    });
    assert_eq!(
        expected,
        actual,
        "golden mismatch for {}\n--- actual ---\n{}",
        path.display(),
        actual
    );
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/characterization")
        .join(name)
}

fn normalized_capture(temp: &TempDir, output: &Output) -> String {
    format!(
        "status: {}\n--- stdout ---\n{}--- stderr ---\n{}",
        output.status.code().unwrap_or(-1),
        normalize_text(temp, &stdout(output)),
        normalize_text(temp, &stderr(output))
    )
}

fn normalize_text(temp: &TempDir, text: &str) -> String {
    let mut normalized = text.to_string();
    for path in temp_path_variants(temp.path()) {
        normalized = normalized.replace(&path, "<TMP>");
    }
    // The deadreckon source checkout leaks into output that embeds skill
    // paths (e.g. `show`'s state dump); normalize it so goldens survive
    // being recorded on one machine and checked on another.
    if let Some(src_root) = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(|root| root.display().to_string())
    {
        normalized = normalized.replace(&src_root, "<SRC>");
    }
    for (pattern, replacement) in [
        // Duration fields must normalize before the hex rules: a float's
        // digit run is also valid hex and would otherwise be half-eaten.
        (
            r#""([a-z_]*wall_seconds)": [0-9.]+"#,
            r#""$1": "<DURATION>""#,
        ),
        // Build-dir contents are never part of the contract; rustc's
        // incremental artifact names vary per run and per toolchain.
        (r#""[^"]*/working/target/[^"]*""#, r#""<BUILD_ARTIFACT>""#),
        (r#""latency_ms": \d+"#, r#""latency_ms": "<LATENCY>""#),
        // Captured tool output embeds cargo's timing and lock chatter.
        (r"(?: *Blocking waiting for file lock on [a-z ]+\\n)+", ""),
        (r"target\(s\) in [0-9.]+s", "target(s) in <DURATION>"),
        (r"([0-9a-f]{8,})(\.\.\.)", "<HEX>$2"),
        (r"\b[0-9a-f]{32}\b", "<ID32>"),
        (r"\b[0-9a-f]{12,40}\b", "<HEX>"),
        (r"\b[0-9a-f]{8}\b", "<ID8>"),
        (r"\b[0-9a-f]{7}\b", "<SHA7>"),
        (
            r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d+ UTC",
            "<TIMESTAMP>",
        ),
        (r"\d{4}-\d{2}-\d{2}T[0-9:.]+Z", "<TIMESTAMP>"),
        (r"wall [0-9]+(?:\.[0-9]+)?s", "wall <DURATION>"),
        (
            r"attach-characterize-[0-9a-f]{4}",
            "attach-characterize-<SLUG>",
        ),
    ] {
        normalized = Regex::new(pattern)
            .expect("BUG: characterization regex must compile")
            .replace_all(&normalized, replacement)
            .into_owned();
    }
    collapse_adjacent_build_artifact_lines(&normalized)
}

/// The number of rustc incremental artifacts varies with the toolchain's
/// codegen-unit split, so after tokenizing them, fold adjacent identical
/// `<BUILD_ARTIFACT>` lines into one — the golden pins "build output was
/// listed", not how many object files this rustc emitted.
fn collapse_adjacent_build_artifact_lines(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.contains("<BUILD_ARTIFACT>") && out.last().is_some_and(|previous| *previous == line)
        {
            continue;
        }
        out.push(line);
    }
    let mut collapsed = out.join("\n");
    if text.ends_with('\n') {
        collapsed.push('\n');
    }
    collapsed
}

fn temp_path_variants(path: &Path) -> Vec<String> {
    let mut variants = vec![path.display().to_string()];
    if let Ok(canonical) = path.canonicalize() {
        variants.push(canonical.display().to_string());
    }
    let mut private_variants = Vec::new();
    for variant in &variants {
        if let Some(stripped) = variant.strip_prefix("/private") {
            private_variants.push(stripped.to_string());
        } else if variant.starts_with("/var/") || variant.starts_with("/tmp/") {
            private_variants.push(format!("/private{variant}"));
        }
    }
    variants.extend(private_variants);
    variants.sort_by_key(|variant| std::cmp::Reverse(variant.len()));
    variants.dedup();
    variants
}

fn started_run_prefix(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("started run "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("missing started run line:\n{stdout}"))
        .to_string()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_status_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
