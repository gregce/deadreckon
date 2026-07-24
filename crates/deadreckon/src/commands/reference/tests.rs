//! P1 depth tests for the single reference resolver.
//!
//! These pin the resolution *spec*, not an implementation: probe precedence,
//! the prefix rule, and the two distinct ambiguity refusals. The cross-verb
//! journey tests that use this resolver live in `tests/coherence.rs`.

use deadreckon_core::campaign::{Campaign, CampaignStatus, write_campaign};
use deadreckon_core::paths::workspace_scope;
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, OnFail, Plan, PlanMode,
    PlanProviders, PlanRole, PlanTask, RunOptions, create_run, save_chain, save_plan,
};
use tempfile::TempDir;

use super::*;

struct Fixture {
    _home: TempDir,
    _cwd: TempDir,
    paths: DeadreckonPaths,
    cwd: PathBuf,
}

fn fixture() -> Fixture {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let paths = DeadreckonPaths::from_home(home.path());
    let cwd_path = cwd.path().to_path_buf();
    Fixture {
        _home: home,
        _cwd: cwd,
        paths,
        cwd: cwd_path,
    }
}

impl Fixture {
    fn run(&self, run_id: &str, goal: &str) -> PipelineState {
        create_run(
            &self.paths,
            RunOptions {
                goal: goal.to_string(),
                cwd: self.cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: Some(run_id.to_string()),
                codebase: None,
            },
        )
        .expect("run")
    }

    fn plan(&self, plan_id: &str, child_run_id: Option<&str>) -> Plan {
        // A plan is only valid with two or more children, indexed from zero.
        let mut first = PlanTask::new(0, "first", "do the first thing", PlanRole::Coder, None);
        first.child_run_id = child_run_id.map(str::to_string);
        let second = PlanTask::new(1, "second", "do the second thing", PlanRole::Coder, None);
        let mut plan = Plan::new(
            "plan root goal",
            PlanMode::FullPlan,
            vec![first, second],
            PlanProviders::default(),
            None,
            "0.0.0",
        )
        .expect("plan");
        plan.plan_id = plan_id.to_string();
        save_plan(&self.paths, &plan).expect("save plan");
        plan
    }

    fn chain(&self, chain_id: &str) -> Chain {
        let mut chain = Chain::new(ChainNewOptions {
            root_goal: "chain root goal".to_string(),
            goals: vec!["first step".to_string(), "second step".to_string()],
            scope: workspace_scope(&self.cwd).expect("scope"),
            base_branch: "main".to_string(),
            base_sha: "0123456789abcdef".to_string(),
            cwd: self.cwd.clone(),
            provider: None,
            model: None,
            sandbox: "none".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Manual,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 1,
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            deadreckon_version: "0.0.0".to_string(),
        })
        .expect("chain");
        chain.chain_id = chain_id.to_string();
        save_chain(&self.paths, &chain).expect("save chain");
        chain
    }

    fn campaign(&self, campaign_id: &str) -> Campaign {
        let dir = self.paths.plans_dir().join(campaign_id);
        fs::create_dir_all(&dir).expect("campaign dir");
        let campaign = Campaign {
            schema_version: 1,
            campaign_id: campaign_id.to_string(),
            root_goal: "campaign root goal".to_string(),
            n: 1,
            depth: 1,
            providers: PlanProviders::default(),
            sub_goals: Vec::new(),
            tree_budget_usd: None,
            tree_wall_seconds: None,
            status: CampaignStatus::Pending,
            merged_run_id: None,
            created_at: Utc::now(),
            forked_at: None,
            merged_at: None,
            deadreckon_version: "0.0.0".to_string(),
        };
        write_campaign(&dir, &campaign).expect("write campaign");
        campaign
    }

    fn query<'a>(&self, reference: &'a str) -> RefQuery<'a> {
        RefQuery {
            reference: Some(reference),
            accepts: RefKinds::ALL,
            all_scopes: true,
            verb: "status",
        }
    }
}

fn err_text(error: &CliError) -> String {
    error.to_string()
}

#[test]
fn probe_order_prefers_plan_child_over_run_prefix() {
    let fx = fixture();
    // A run whose id is the plan id + a task-looking suffix would win a bare
    // prefix race; the plan-child syntax must be recognized first.
    let child = fx.run("aaaa1111bbbb2222cccc3333dddd4444", "child work");
    fx.plan("aaaa1111000000000000000000000000", Some(&child.run_id));

    let resolved = resolve_ref(
        &fx.paths,
        fx.query("aaaa1111000000000000000000000000:task-0"),
    )
    .expect("resolves");

    assert_eq!(resolved.kind(), RefKind::PlanChild);
    match resolved {
        ResolvedRef::PlanChild { state, .. } => assert_eq!(state.run_id, child.run_id),
        other => panic!("expected a plan child, got {:?}", other.kind()),
    }
}

#[test]
fn prefix_matching_both_a_run_and_a_plan_is_refused_as_ambiguous() {
    let fx = fixture();
    fx.run("ddddeeee111122223333444455556666", "run work");
    fx.plan("ddddeeee999988887777666655554444", None);

    let error = resolve_ref(&fx.paths, fx.query("ddddeeee")).expect_err("ambiguous");
    let text = err_text(&error);

    assert!(
        text.contains("run ddddeeee111122223333444455556666"),
        "{text}"
    );
    assert!(
        text.contains("plan ddddeeee999988887777666655554444"),
        "{text}"
    );
}

#[test]
fn chain_id_resolves_when_no_run_or_plan_matches() {
    let fx = fixture();
    fx.run("1111111111111111111111111111aaaa", "run work");
    fx.plan("2222222222222222222222222222bbbb", None);
    let chain = fx.chain("3333333333333333333333333333cccc");

    let resolved = resolve_ref(&fx.paths, fx.query("3333")).expect("resolves");

    assert_eq!(resolved.kind(), RefKind::Chain);
    assert_eq!(resolved_id(&resolved), chain.chain_id);
}

#[test]
fn campaign_id_resolves_when_no_other_kind_matches() {
    let fx = fixture();
    fx.run("1111111111111111111111111111aaaa", "run work");
    let campaign = fx.campaign("4444444444444444444444444444dddd");

    let resolved = resolve_ref(&fx.paths, fx.query("4444")).expect("resolves");

    assert_eq!(resolved.kind(), RefKind::Campaign);
    assert_eq!(resolved_id(&resolved), campaign.campaign_id);
}

#[test]
fn ambiguous_prefix_within_runs_passes_through_loader_message() {
    let fx = fixture();
    fx.run("abcd1111111111111111111111111111", "first");
    fx.run("abcd2222222222222222222222222222", "second");

    let error = resolve_ref(&fx.paths, fx.query("abcd")).expect_err("ambiguous");
    let text = err_text(&error);

    assert!(text.contains("ambiguous run id prefix abcd"), "{text}");
    assert!(text.contains("abcd1111111111111111111111111111"), "{text}");
    assert!(text.contains("abcd2222222222222222222222222222"), "{text}");
}

#[test]
fn ambiguous_prefix_across_run_and_plan_names_both_full_ids() {
    let fx = fixture();
    fx.run("beef1111111111111111111111111111", "run work");
    fx.plan("beef2222222222222222222222222222", None);

    let error = resolve_ref(&fx.paths, fx.query("beef")).expect_err("ambiguous");
    let text = err_text(&error);

    // Both full ids, and the word that tells the operator which is which.
    assert!(
        text.contains("run beef1111111111111111111111111111"),
        "{text}"
    );
    assert!(
        text.contains("plan beef2222222222222222222222222222"),
        "{text}"
    );
    assert!(text.contains("matches"), "{text}");
}

#[test]
fn unknown_reference_with_no_state_refuses_with_start_not_list() {
    let fx = fixture();

    let error = resolve_ref(&fx.paths, fx.query("nothinghere")).expect_err("unresolved");
    let text = err_text(&error);

    assert!(text.contains("no runs or plans yet"), "{text}");
    assert!(text.contains("deadreckon start"), "{text}");
    assert!(
        !text.contains("deadreckon list"),
        "an empty home must not be sent to an empty list: {text}"
    );
}

#[test]
fn unknown_reference_with_existing_state_refuses_with_list() {
    let fx = fixture();
    fx.run("cafe1111111111111111111111111111", "run work");

    let error = resolve_ref(&fx.paths, fx.query("nothinghere")).expect_err("unresolved");
    let text = err_text(&error);

    assert!(text.contains("deadreckon list"), "{text}");
}

#[test]
fn accepts_narrows_which_kinds_are_probed() {
    let fx = fixture();
    let plan = fx.plan("5555555555555555555555555555eeee", None);

    let run_only = RefQuery {
        reference: Some("5555"),
        accepts: RefKinds::RUN,
        all_scopes: true,
        verb: "verdict",
    };
    resolve_ref(&fx.paths, run_only).expect_err("a run-only verb must not resolve a plan");

    let resolved = resolve_ref(
        &fx.paths,
        RefQuery {
            accepts: RefKinds::RUN.union(RefKinds::PLAN),
            ..run_only
        },
    )
    .expect("resolves once plans are accepted");
    assert_eq!(resolved_id(&resolved), plan.plan_id);
}

#[test]
fn ref_kinds_all_contains_every_kind() {
    for kind in [
        RefKind::Run,
        RefKind::PlanChild,
        RefKind::Plan,
        RefKind::Chain,
        RefKind::Campaign,
    ] {
        assert!(RefKinds::ALL.contains(kind), "{} missing", kind.noun());
    }
    assert!(!RefKinds::RUN.contains(RefKind::Plan));
}

#[test]
fn plan_ids_matching_with_empty_prefix_lists_every_plan() {
    let fx = fixture();
    fx.plan("aaaa000000000000000000000000000a", None);
    fx.plan("bbbb000000000000000000000000000b", None);

    let all = plan_ids_matching(&fx.paths, "").expect("plans");

    assert_eq!(all.len(), 2, "{all:?}");
}

#[test]
fn plan_ids_matching_ignores_directories_without_plan_json() {
    let fx = fixture();
    fx.campaign("cccc000000000000000000000000000c");

    let all = plan_ids_matching(&fx.paths, "").expect("plans");

    assert!(
        all.is_empty(),
        "a campaign dir is not a plan for prefix purposes: {all:?}"
    );
}
