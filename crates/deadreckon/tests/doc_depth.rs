#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;

use deadreckon_core::{
    DeadreckonPaths, RunOptions, TurnDocInput, append_turn_doc, as_built_path,
    capture_response_full, capture_response_summary, changed_doc_files, diff_samples_markdown,
    is_documentable_path, narrative_path, read_turn_records, rewrite_templated_docs,
    snapshot_working, source_layout,
};
use tempfile::TempDir;

mod common;

use common::repo_tempdir;

#[test]
fn incremental_jsonl_carries_full_response_up_to_50kb() {
    let long = "a".repeat(70 * 1024);
    let captured = capture_response_full(&long);
    assert_eq!(captured.len(), 50 * 1024);
}

#[test]
fn response_summary_ends_on_word_boundary() {
    let text = format!("{} {}", "alpha ".repeat(80), "tailword");
    let summary = capture_response_summary(&text);
    assert!(summary.len() <= 280);
    assert!(!summary.ends_with("tailwo"));
}

#[test]
fn incremental_jsonl_carries_diff_samples_per_file() {
    let (_temp, state) = fresh_state("diff sample");
    fs::write(state.working_dir.join("README.md"), "one\ntwo\nthree\n").expect("base");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(
        state.working_dir.join("README.md"),
        "one\nchanged\nthree\nnew\n",
    )
    .expect("changed");

    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("README.md")],
            outcome: "updated README".to_string(),
            response_text: "updated README".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let file = &records[0].files[0];
    assert_eq!(file.path, "README.md");
    assert!(file.adds >= 1);
    assert!(file.dels >= 1);
    assert!(file.largest_hunk_excerpt.contains("@@"));
    assert!(diff_samples_markdown(&records).contains("+changed"));
}

#[test]
fn binary_files_marked_is_binary_no_excerpt() {
    let (_temp, state) = fresh_state("binary sample");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(state.working_dir.join("asset.bin"), [0, 159, 146, 150]).expect("binary");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("asset.bin")],
            outcome: "wrote binary".to_string(),
            response_text: "wrote binary".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    assert!(records[0].files[0].is_binary);
    assert!(records[0].files[0].largest_hunk_excerpt.is_empty());
}

#[test]
fn generated_artifacts_are_omitted_from_doc_inputs() {
    let (_temp, state) = fresh_state("generated docs");
    for dir in ["src", "docs", "node_modules/pkg", ".next/server", "dist"] {
        fs::create_dir_all(state.working_dir.join(dir)).expect("dir");
    }
    for (path, body) in [
        ("src/main.js", "console.log('ok');\n"),
        ("docs/guide.md", "# Guide\n"),
        ("package-lock.json", "{}\n"),
        ("node_modules/pkg/index.js", "module.exports = 1;\n"),
        (".next/server/app.js", "generated();\n"),
        ("dist/bundle.js", "generated();\n"),
    ] {
        fs::write(state.working_dir.join(path), body).expect("file");
    }

    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "cli_subagent".to_string(),
            latency_ms: Some(1),
            files: [
                "src/main.js",
                "docs/guide.md",
                "package-lock.json",
                "node_modules/pkg/index.js",
                ".next/server/app.js",
                "dist/bundle.js",
            ]
            .into_iter()
            .map(|path| state.working_dir.join(path))
            .collect(),
            outcome: "built app".to_string(),
            response_text: "built app".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");

    let records = read_turn_records(&state.working_dir).expect("records");
    assert!(is_documentable_path("docs/guide.md"));
    let mut paths = records[0].file_paths();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "docs/guide.md".to_string(),
            "package-lock.json".to_string(),
            "src/main.js".to_string()
        ]
    );
    assert_eq!(
        changed_doc_files(&state).expect("changed"),
        vec![
            "docs/guide.md".to_string(),
            "package-lock.json".to_string(),
            "src/main.js".to_string()
        ]
    );
    let diff_samples = diff_samples_markdown(&records);
    assert!(diff_samples.contains("src/main.js"));
    assert!(!diff_samples.contains("node_modules"));
    assert!(!diff_samples.contains(".next"));
    assert!(!diff_samples.contains("dist/bundle.js"));
    assert!(is_documentable_path("src/main.js"));
    assert!(!is_documentable_path("node_modules/pkg/index.js"));
    assert!(!is_documentable_path(".next/server/app.js"));
}

#[test]
fn stoa_cli_artifact_patterns_are_omitted_from_doc_paths() {
    for path in [
        ".github/workflows/ci.yml",
        ".storybook/main.ts",
        "lib/app.js",
        "src/main.ts",
        "src/components/App.tsx",
        "package-lock.json",
        "Cargo.lock",
        "go.mod",
        ".ruby-version",
        "README.md",
    ] {
        assert!(
            is_documentable_path(path),
            "{path} should remain documentable"
        );
    }

    for path in [
        ".stoa/state.json",
        ".specstory/history/session.md",
        ".claude/settings.json",
        "app/.astro/types.d.ts",
        ".output/server/index.mjs",
        ".docusaurus/routes.js",
        ".vuepress/dist/index.html",
        ".remix/build/index.js",
        "docs/_build/html/index.html",
        "site/.vitepress/cache/deps.js",
        ".gradle/caches/modules-2/files.bin",
        "android/.kotlin/sessions/state",
        "target/debug/app",
        ".venv/lib/python/site-packages/pkg.py",
        "__pypackages__/3.12/lib/pkg.py",
        ".tox/py312/log.txt",
        ".nox/tests/tmp",
        ".hypothesis/examples/db",
        ".pybuilder/target/classes",
        ".pixi/envs/default/python",
        ".pyre/types.json",
        ".ipynb_checkpoints/notebook-checkpoint.ipynb",
        "coverage/lcov.info",
        "htmlcov/index.html",
        "node_modules/pkg/index.js",
        "jspm_packages/pkg/index.js",
        ".pnpm-store/v3/files/index",
        ".yarn/cache/pkg.zip",
        ".nyc_output/out.json",
        ".serverless/app.zip",
        ".firebase/hosting.cache",
        "src/main.css.map",
        "foo/bar.tsbuildinfo",
        ".env.local",
        "Thumbs.db",
        "CMakeFiles/progress.marks",
        "cmake-build-debug/compile_commands.json",
        "_deps/zlib-src/CMakeLists.txt",
        "vendor/bundle/ruby/gems/rake.rb",
        "ios/Flutter/ephemeral/file",
        "ios/Flutter/App.framework/App",
        ".dart_tool/package_config.json",
        ".pub-cache/hosted/pub.dev/pkg",
        "_build/dev/lib/app/ebin/app.beam",
        "deps/phoenix/mix.exs",
        ".elixir_ls/build/ref",
        "_site/index.html",
        ".terraform/providers/registry.terraform.io/hashicorp/aws",
        "terraform.tfstate",
        ".ansible/tmp/playbook",
        ".pulumi/history/checkpoint.json",
        "packer_cache/template.iso",
        ".vagrant/machines/default",
        "cdk.out/tree.json",
        ".cdk.staging/asset.hash",
        "fastlane/test_output/report.xml",
        "src/generated_plugin_registrant.dart",
        "build-android/output.apk",
        "app.dSYM/Contents/Info.plist",
    ] {
        assert!(!is_documentable_path(path), "{path} should be omitted");
    }
}

#[test]
fn incremental_jsonl_carries_bash_stdout_and_stderr() {
    let (_temp, state) = fresh_state("stdio capture");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "bash".to_string(),
            latency_ms: Some(1),
            files: Vec::new(),
            outcome: "ran tests".to_string(),
            response_text: "ran tests".to_string(),
            tool_stdout: Some("test stdout".to_string()),
            tool_stderr: Some("warning stderr".to_string()),
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    assert_eq!(records[0].tool_stdout.as_deref(), Some("test stdout"));
    assert_eq!(records[0].tool_stderr.as_deref(), Some("warning stderr"));
}

#[test]
fn diff_sample_picks_largest_hunk_with_at_header() {
    let (_temp, state) = fresh_state("largest hunk");
    fs::write(state.working_dir.join("src.rs"), "one\ntwo\nthree\n").expect("base");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(
        state.working_dir.join("src.rs"),
        "one\ntwo changed\nthree changed\nfour added\n",
    )
    .expect("changed");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("src.rs")],
            outcome: "changed largest hunk".to_string(),
            response_text: "changed largest hunk".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let hunk = &records[0].files[0].largest_hunk_excerpt;
    assert!(hunk.starts_with("@@"));
    assert!(hunk.contains("+two changed"));
    assert!(hunk.lines().count() <= 6);
}

#[test]
fn narrative_heading_uses_full_goal_no_truncation() {
    let goal = "make it possible to create and add a gallery of artwork and browse it with a deliberately long title";
    let (_temp, state) = fresh_state(goal);
    rewrite_templated_docs(&state, "templated only").expect("docs");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.starts_with(&format!("# {goal}")));
}

#[test]
fn phase_paragraph_combines_response_summaries() {
    let (_temp, state) = fresh_state("phase summaries");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("a.txt")],
            outcome: "wrote file".to_string(),
            response_text:
                "Created the file with a useful implementation summary.\n\nMore details follow."
                    .to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("Created the file with a useful implementation summary."));
}

#[test]
fn per_file_adds_dels_appear_in_phase_body() {
    let (_temp, state) = fresh_state("phase adds dels");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(state.working_dir.join("a.txt"), "one\ntwo\n").expect("write");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("a.txt")],
            outcome: "wrote file".to_string(),
            response_text: "wrote file".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("| `a.txt` | +2 / -0 |"));
}

#[test]
fn largest_hunk_excerpt_inlined_in_phase_body() {
    let (_temp, state) = fresh_state("phase hunk");
    fs::write(state.working_dir.join("a.txt"), "old\n").expect("base");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(state.working_dir.join("a.txt"), "new\n").expect("changed");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("a.txt")],
            outcome: "changed file".to_string(),
            response_text: "changed file".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("@@"));
    assert!(narrative.contains("+new"));
}

#[test]
fn open_threads_extracted_from_todo_phrases() {
    let (_temp, state) = fresh_state("open threads");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "cli_subagent".to_string(),
            latency_ms: Some(1),
            files: Vec::new(),
            outcome: "left follow-up".to_string(),
            response_text: "Implemented the baseline.\nTODO: add browser coverage as follow-up."
                .to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("turn 1: TODO: add browser coverage as follow-up."));
}

#[test]
fn outcome_text_never_truncated_at_200_chars() {
    let (_temp, state) = fresh_state("long outcome");
    let outcome = format!("{}TAIL", "long outcome ".repeat(30));
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "cli_subagent".to_string(),
            latency_ms: Some(1),
            files: Vec::new(),
            outcome: outcome.clone(),
            response_text: outcome,
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("TAIL"));
}

#[test]
fn component_table_omits_unmapped_project_files() {
    let (_temp, state) = fresh_state("component inference");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![
                state.working_dir.join("crates/app/src/lib.rs"),
                state.working_dir.join("misc.dat"),
            ],
            outcome: "changed crate".to_string(),
            response_text: "changed crate".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Crate app (Rust)"));
    assert!(!as_built.contains("Project files"));
}

#[test]
fn crates_path_maps_to_crate_layer_with_name() {
    let (_temp, state) = fresh_state("crate layer");
    append_component_turn(&state, "crates/app/src/lib.rs");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Crate app (Rust)"));
    assert!(as_built.contains("`crates/app/src/lib.rs:1`"));
}

#[test]
fn frontend_components_path_maps_to_frontend_component() {
    let (_temp, state) = fresh_state("frontend component");
    append_component_turn(&state, "src/components/gallery/Card.tsx");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Frontend component (gallery)"));
}

#[test]
fn tests_path_maps_to_tests_layer() {
    let (_temp, state) = fresh_state("test layer");
    append_component_turn(&state, "tests/gallery.test.ts");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Tests"));
}

#[test]
fn docs_path_maps_to_documentation_layer() {
    let (_temp, state) = fresh_state("docs layer");
    append_component_turn(&state, "docs/gallery.md");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Documentation"));
}

#[test]
fn unmapped_path_omitted_not_emitted_as_project_files() {
    let (_temp, state) = fresh_state("unmapped layer");
    append_component_turn(&state, "misc.dat");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(!as_built.contains("Project files"));
    assert!(!as_built.contains("misc.dat:"));
}

#[test]
fn topology_emitted_only_when_three_or_more_top_dirs() {
    let (_temp, state) = fresh_state("topology");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![
                state.working_dir.join("crates/app/src/lib.rs"),
                state.working_dir.join("skills/x/SKILL.md"),
                state.working_dir.join("docs/guide.md"),
            ],
            outcome: "changed dirs".to_string(),
            response_text: "changed dirs".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let layout = source_layout(&records, &state.working_dir);
    assert!(layout.contains("+-----------+"));
    assert!(layout.contains("| crates/"));
}

#[test]
fn topology_arrows_derived_from_grep_cross_refs() {
    let (_temp, state) = fresh_state("topology refs");
    fs::create_dir_all(state.working_dir.join("crates/app")).expect("crates");
    fs::create_dir_all(state.working_dir.join("skills/x")).expect("skills");
    fs::create_dir_all(state.working_dir.join("docs")).expect("docs");
    fs::write(
        state.working_dir.join("crates/app/lib.rs"),
        "see skills/x/SKILL.md",
    )
    .expect("xref");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![
                state.working_dir.join("crates/app/lib.rs"),
                state.working_dir.join("skills/x/SKILL.md"),
                state.working_dir.join("docs/guide.md"),
            ],
            outcome: "changed topology".to_string(),
            response_text: "changed topology".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let layout = source_layout(&records, &state.working_dir);
    assert!(layout.contains("crates/ -> skills/"));
}

#[test]
fn four_subskill_files_present_in_repo_skills_dir() {
    for skill in [
        "narrator-overview",
        "narrator-phases",
        "narrator-as-built",
        "narrator-decisions",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills")
            .join(skill)
            .join("SKILL.md");
        assert!(path.exists(), "{} missing", path.display());
        let raw = fs::read_to_string(&path).expect("skill");
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("output: json"));
        assert!(raw.contains("inputs:"));
    }
}

#[test]
fn each_subskill_has_required_frontmatter_fields() {
    for skill in [
        "narrator-overview",
        "narrator-phases",
        "narrator-as-built",
        "narrator-decisions",
    ] {
        let raw = repo_skill(skill);
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("name:"));
        assert!(raw.contains("description:"));
        assert!(raw.contains("output: json"));
    }
}

#[test]
fn narrator_overview_prompt_asks_for_reading_order_and_why_now() {
    let overview = repo_skill("narrator-overview");
    assert!(overview.contains("reading_order"));
    assert!(overview.contains("why_now"));
    assert!(overview.contains("practical project handoff"));
    assert!(overview.contains("generated/vendor/cache/build artifacts"));
}

#[test]
fn narrator_phases_prompt_requires_per_phase_paragraph_and_diff_quote() {
    let phases = repo_skill("narrator-phases");
    assert!(phases.contains("prose paragraph per phase"));
    assert!(phases.contains("largest diff hunk"));
    assert!(phases.contains("documentable changed file"));
    assert!(phases.contains("node_modules/"));
}

#[test]
fn narrator_as_built_prompt_forbids_project_files_layer() {
    let as_built = repo_skill("narrator-as-built");
    assert!(as_built.contains("Project files"));
}

#[test]
fn narrator_as_built_prompt_requires_load_bearing_and_seams() {
    let as_built = repo_skill("narrator-as-built");
    assert!(as_built.contains("load-bearing"));
    assert!(as_built.contains("seams"));
    assert!(as_built.contains("Exclude generated"));
}

#[test]
fn narrator_decisions_prompt_filters_false_positive_candidates() {
    let decisions = repo_skill("narrator-decisions");
    assert!(decisions.contains("false positives"));
    assert!(decisions.contains("decision_candidate"));
}

#[test]
fn narrator_decisions_prompt_outputs_interpretation_sections() {
    let decisions = repo_skill("narrator-decisions");
    assert!(decisions.contains("design_decisions"));
    assert!(decisions.contains("deviations"));
    assert!(decisions.contains("tradeoffs"));
    assert!(decisions.contains("open_questions"));
    assert!(decisions.contains("implementation-notes.html"));
}

#[test]
fn legacy_run_narrator_skill_still_present() {
    assert!(repo_skill_path("run-narrator").exists());
    let skill = repo_skill("run-narrator");
    assert!(skill.contains("useful handoff"));
    assert!(skill.contains("Every documentable changed file"));
}

#[test]
fn narrator_subskill_prompts_require_doc_depth_contracts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    let overview =
        fs::read_to_string(root.join("narrator-overview/SKILL.md")).expect("overview skill");
    assert!(overview.contains("reading_order"));
    assert!(overview.contains("why_now"));
    assert!(overview.contains("practical project handoff"));

    let phases = fs::read_to_string(root.join("narrator-phases/SKILL.md")).expect("phases skill");
    assert!(phases.contains("prose paragraph per phase"));
    assert!(phases.contains("largest diff hunk"));
    assert!(phases.contains("documentable changed file"));

    let as_built =
        fs::read_to_string(root.join("narrator-as-built/SKILL.md")).expect("as-built skill");
    assert!(as_built.contains("Project files"));
    assert!(as_built.contains("load-bearing"));
    assert!(as_built.contains("seams"));
    assert!(as_built.contains("Exclude generated"));

    let decisions =
        fs::read_to_string(root.join("narrator-decisions/SKILL.md")).expect("decisions skill");
    assert!(decisions.contains("false positives"));
    assert!(decisions.contains("decisions"));
}

#[test]
fn as_built_documents_nonblocking_attach_contract() {
    let as_built = repo_file("docs/AS-BUILT-ARCHITECTURE.md");

    assert!(as_built.contains("Data source and responsiveness contract"));
    assert!(as_built.contains("The render path is pure"));
    assert!(as_built.contains("background jobs"));
    assert!(as_built.contains("AttachJsonlTail"));
    assert!(as_built.contains("PlanEventBus"));
    assert!(as_built.contains("chain-events.jsonl"));
    assert!(as_built.contains("stale narrative snapshots survive redraw"));
}

#[test]
fn changelog_mentions_tui_responsiveness_alpha_slice() {
    let changelog = repo_file("CHANGELOG.md");

    assert!(changelog.contains("## TUI Responsiveness (alpha) - 2026-05-26"));
    assert!(changelog.contains("background job"));
    assert!(changelog.contains("incremental chain activity tailing"));
    assert!(changelog.contains("known limits"));
    assert!(changelog.contains("no attach daemon"));
}

#[test]
fn v1_candidates_records_out_of_scope_attach_daemon() {
    let candidates = repo_file("docs/V1-CANDIDATES.md");

    assert!(candidates.contains("Attach responsiveness platform"));
    assert!(candidates.contains("long-lived attach daemon"));
    assert!(candidates.contains("shared broadcaster"));
    assert!(candidates.contains("diagnostic dashboard"));
}

#[test]
fn as_built_documents_rudder_connection_inbox_approvals_and_degrade_rules() {
    let as_built = repo_file("docs/AS-BUILT-ARCHITECTURE.md");

    assert!(
        as_built.contains("## 51. Rudder: Steering the Running Child"),
        "missing Rudder section"
    );
    for required in [
        "cli:codex-server",
        "codex app-server",
        "provider-session.json",
        "steer-inbox.jsonl",
        "at-least-once",
        "turn/steer",
        "expectedTurnId",
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "provider.approval",
        "turn/interrupt",
        "provider.route.degraded",
    ] {
        assert!(as_built.contains(required), "missing {required}");
    }
    assert!(as_built.contains("§47"));
    assert!(as_built.contains("§50"));
    assert!(as_built.contains(":steer <instruction>"));
}

#[test]
fn changelog_records_rudder_stable_with_every_implementation_phase_sha() {
    let changelog = repo_file("CHANGELOG.md");

    assert!(changelog.contains("## Rudder (stable) — steer the running child — 2026-07-16"));
    assert!(changelog.contains("durable at-least-once inbox"));
    assert!(changelog.contains("interrupt-before-kill"));
    assert!(changelog.contains("danger-full-access inversion"));
    assert!(changelog.contains("server loss degrades to the exec route"));
    for (phase, sha) in [
        ("P1", "731c907"),
        ("P2", "e4df6b9"),
        ("P3", "0a5bc0e"),
        ("P4", "be439d2"),
        ("P5", "ab8d0b3"),
        ("P6", "f86fdff"),
        ("P7", "14f6402"),
        ("P8", "a8319d1"),
        ("P9", "0ca9328"),
        ("P10", "efc22c3"),
    ] {
        assert!(
            changelog.contains(&format!("- {phase} (`{sha}`):")),
            "missing {phase} {sha}"
        );
    }
}

#[test]
fn v1_candidates_record_rudder_boundaries() {
    let candidates = repo_file("docs/V1-CANDIDATES.md");
    let lowercase = candidates.to_lowercase();

    assert!(candidates.contains("Rudder follow-ups"));
    assert!(candidates.contains("shared app-server daemon and Unix socket"));
    assert!(lowercase.contains("cross-run server reuse"));
    assert!(candidates.contains("thread/fork"));
    assert!(candidates.contains("rewind"));
    assert!(candidates.contains("steering for `cli:claude-code`"));
}

fn append_component_turn(state: &deadreckon_core::PipelineState, file: &str) {
    append_turn_doc(
        state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join(file)],
            outcome: "changed component".to_string(),
            response_text: "changed component".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
}

fn repo_skill_path(skill: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills")
        .join(skill)
        .join("SKILL.md")
}

fn repo_skill(skill: &str) -> String {
    fs::read_to_string(repo_skill_path(skill)).expect("skill")
}

fn repo_file(path: &str) -> String {
    fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .expect(path)
}

fn fresh_state(goal: &str) -> (TempDir, deadreckon_core::PipelineState) {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = deadreckon_core::create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("create run");
    (temp, state)
}
