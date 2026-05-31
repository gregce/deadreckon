use std::cell::Cell;

use crate::commands::learning::{SelfImprovePrAdapter, open_self_improve_pr_if_eligible};
use deadreckon_core::learning::{AutoPrDecision, LearningRisk};
use tempfile::TempDir;

use super::*;

struct FakePrAdapter {
    called: Cell<bool>,
}

impl SelfImprovePrAdapter for FakePrAdapter {
    fn open_pr(&self, _candidate: &LearningCandidate, _dry_run: &PrDryRun) -> Result<String> {
        self.called.set(true);
        Ok("https://github.com/example/deadreckon/pull/1".to_string())
    }
}

#[test]
fn open_pr_adapter_not_called_when_evidence_gate_refuses() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let candidate = test_learning_candidate(&paths);
    let dry_run = PrDryRun {
        title: "Self-improve: test".to_string(),
        body: "## Summary\n\nbody".to_string(),
        body_path: temp.path().join("body.md"),
        branch: candidate.branch.clone(),
        decision: AutoPrDecision {
            eligible: false,
            reasons: vec!["focused verification did not pass".to_string()],
        },
    };
    let adapter = FakePrAdapter {
        called: Cell::new(false),
    };

    let err = open_self_improve_pr_if_eligible(&paths, "prop-test", &candidate, &dry_run, &adapter)
        .expect_err("refuse");

    assert!(err.to_string().contains("PR gate failed"));
    assert!(!adapter.called.get());
}

#[test]
fn open_pr_records_pr_events_row_with_url() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let candidate = test_learning_candidate(&paths);
    let dry_run = PrDryRun {
        title: "Self-improve: test".to_string(),
        body: "## Summary\n\nbody".to_string(),
        body_path: temp.path().join("body.md"),
        branch: candidate.branch.clone(),
        decision: AutoPrDecision {
            eligible: true,
            reasons: Vec::new(),
        },
    };
    let adapter = FakePrAdapter {
        called: Cell::new(false),
    };

    let url = open_self_improve_pr_if_eligible(&paths, "prop-test", &candidate, &dry_run, &adapter)
        .expect("open");

    assert_eq!(url, "https://github.com/example/deadreckon/pull/1");
    assert!(adapter.called.get());
    let events = read_jsonl::<LearningPrEvent>(&paths.learning_pr_events_path()).expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].pr_url.as_deref(), Some(url.as_str()));
    assert_eq!(events[0].mode, "open");
}

#[test]
fn open_pr_adapter_receives_fixed_body_sections_and_evaluated_head() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let candidate = test_learning_candidate(&paths);
    let dry_run = PrDryRun {
        title: "Self-improve: test".to_string(),
        body: [
            "## Summary",
            "## Stimulus and Proposal",
            "## Evidence Packet",
            "## Verification",
            "## Risk Classification",
            "## Rollback",
            "## Files Changed",
        ]
        .join("\n\n"),
        body_path: temp.path().join("body.md"),
        branch: candidate.branch.clone(),
        decision: AutoPrDecision {
            eligible: true,
            reasons: Vec::new(),
        },
    };
    let adapter = SectionCheckingPrAdapter;

    let url = open_self_improve_pr_if_eligible(&paths, "prop-test", &candidate, &dry_run, &adapter)
        .expect("open");

    assert_eq!(url, "https://github.com/example/deadreckon/pull/sections");
}

fn test_learning_candidate(paths: &DeadreckonPaths) -> LearningCandidate {
    LearningCandidate {
        version: 1,
        candidate_id: "cand-test".to_string(),
        proposal_id: "prop-test".to_string(),
        branch: "deadreckon/self/cand-test".to_string(),
        base_commit: "base".to_string(),
        head_commit: "head".to_string(),
        run_id: "run-test".to_string(),
        worktree: paths.learning_candidate_dir("cand-test").join("worktree"),
        diff: LearningCandidateDiff {
            files: 1,
            insertions: 1,
            deletions: 0,
            changed_files: vec!["crates/deadreckon/src/main.rs".to_string()],
        },
        risk: LearningRisk {
            class: "low".to_string(),
            reasons: Vec::new(),
        },
        status: "verified".to_string(),
        evidence_packet: "evidence.json".to_string(),
    }
}

struct SectionCheckingPrAdapter;

impl SelfImprovePrAdapter for SectionCheckingPrAdapter {
    fn open_pr(&self, candidate: &LearningCandidate, dry_run: &PrDryRun) -> Result<String> {
        assert_eq!(dry_run.branch, candidate.branch);
        for section in [
            "## Summary",
            "## Stimulus and Proposal",
            "## Evidence Packet",
            "## Verification",
            "## Risk Classification",
            "## Rollback",
            "## Files Changed",
        ] {
            assert!(dry_run.body.contains(section), "{section}");
        }
        assert_ne!(candidate.base_commit, candidate.head_commit);
        Ok("https://github.com/example/deadreckon/pull/sections".to_string())
    }
}
