#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use deadreckon_core::DeadreckonPaths;
use deadreckon_core::RunStatus;
use deadreckon_core::list_runs;
use deadreckon_core::load_run;

#[path = "../src/friendliness_contract.rs"]
mod friendliness_contract;

use friendliness_contract::FRIENDLINESS_CLAUSES;
use friendliness_contract::FRIENDLINESS_CONTRACT;

mod common;

use common::{assert_success, deadreckon, stdout, workspace_tempdir as repo_tempdir};

#[test]
fn friendliness_contract_table_covers_every_top_level_verb() {
    let cli_verbs = top_level_cli_verbs();
    let contract_verbs = FRIENDLINESS_CONTRACT
        .iter()
        .map(|entry| entry.verb.to_string())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        cli_verbs, contract_verbs,
        "friendliness contract must cover every top-level clap verb"
    );
}

#[test]
fn audit_doc_lists_a_row_per_verb_and_clause() {
    let audit = fs::read_to_string(workspace_root().join("docs/FRIENDLINESS-AUDIT.md"))
        .expect("read friendliness audit");
    let rows = parse_audit_rows(&audit);

    for entry in FRIENDLINESS_CONTRACT {
        for (index, clause) in FRIENDLINESS_CLAUSES.iter().enumerate() {
            let key = (entry.verb.to_string(), clause.label().to_string());
            let status = rows.get(&key).unwrap_or_else(|| {
                panic!("missing audit row for {} / {}", entry.verb, clause.label())
            });
            assert_eq!(
                *status,
                entry.marks[index].as_str(),
                "audit row status drifted for {} / {}",
                entry.verb,
                clause.label()
            );
        }
    }

    assert_eq!(
        FRIENDLINESS_CONTRACT.len() * FRIENDLINESS_CLAUSES.len(),
        rows.len(),
        "audit must contain exactly one row per verb x clause"
    );
}

#[test]
fn try_runs_keyless_and_signs_a_gate() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let workspace = temp.path().join("caller-checkout");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("untouched.txt"), "do not touch").expect("sentinel");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&workspace)
        .arg("try")
        .output()
        .expect("try");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(workspace.join("untouched.txt")).expect("sentinel"),
        "do not touch"
    );
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.provider.as_deref(), Some("smoke"));
    assert!(state.run_root.join("proofs/turn-acceptance.json").is_file());
    assert!(state.cwd.starts_with(paths.home().join("try")));
}

#[test]
fn try_prints_proof_block_with_narrative_and_lineage() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .arg("try")
        .output()
        .expect("try");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("gate: SIGNED by dr-gate"), "{stdout}");
    assert!(stdout.contains("proof:  "), "{stdout}");
    assert!(stdout.contains("/proofs/turn-acceptance.json"), "{stdout}");
    assert!(stdout.contains("story:  "), "{stdout}");
    assert!(stdout.contains("/docs/RUN-NARRATIVE.md"), "{stdout}");
    assert!(stdout.contains("lineage: "), "{stdout}");
    assert!(stdout.contains("turn "), "{stdout}");
    assert!(stdout.contains("smoke"), "{stdout}");
    assert!(
        stdout.contains("deadreckon start \"build the real thing\""),
        "{stdout}"
    );
}

fn parse_audit_rows(audit: &str) -> BTreeMap<(String, String), String> {
    let mut rows = BTreeMap::new();
    for line in audit.lines() {
        if !line.starts_with("| `") {
            continue;
        }
        let cells = line.split('|').skip(1).map(str::trim).collect::<Vec<_>>();
        if cells.len() < 4 {
            continue;
        }
        let verb = cells[0].trim_matches('`').to_string();
        let clause = cells[1].to_string();
        let status = cells[2].to_string();
        assert!(
            matches!(status.as_str(), "pass" | "fail" | "n-a"),
            "unexpected audit status {status:?} in {line}"
        );
        assert!(
            rows.insert((verb.clone(), clause.clone()), status)
                .is_none(),
            "duplicate audit row for {verb} / {clause}"
        );
    }
    rows
}

fn top_level_cli_verbs() -> BTreeSet<String> {
    let source = include_str!("../src/cli.rs");
    let start = source
        .find("pub(crate) enum Commands")
        .expect("Commands enum");
    let mut depth = 0usize;
    let mut in_commands = false;
    let mut pending_name = None::<String>;
    let mut verbs = BTreeSet::new();

    for line in source[start..].lines() {
        let trimmed = line.trim();
        if !in_commands && trimmed.starts_with("pub(crate) enum Commands") {
            in_commands = true;
        }
        if !in_commands {
            continue;
        }

        if depth == 1 {
            if let Some(name) = command_name_attr(trimmed) {
                pending_name = Some(name);
            }
            if let Some(variant) = top_level_variant(trimmed) {
                let name = pending_name.take().unwrap_or_else(|| kebab_case(variant));
                verbs.insert(name);
            }
        }

        let opens = line.chars().filter(|ch| *ch == '{').count();
        let closes = line.chars().filter(|ch| *ch == '}').count();
        depth = depth + opens - closes;
        if in_commands && depth == 0 && trimmed == "}" {
            break;
        }
    }

    verbs
}

fn command_name_attr(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("name = \"")?;
    let (name, _) = rest.split_once('"')?;
    Some(name.to_string())
}

fn top_level_variant(line: &str) -> Option<&str> {
    let first = line.chars().next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    let end = line
        .find(|ch: char| ch == '{' || ch == '(' || ch == ',' || ch.is_whitespace())
        .unwrap_or(line.len());
    Some(&line[..end])
}

fn kebab_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn workspace_root() -> std::path::PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !dir.join("Cargo.toml").is_file() || !dir.join("crates").is_dir() {
        assert!(dir.pop(), "workspace root not found");
    }
    dir
}
