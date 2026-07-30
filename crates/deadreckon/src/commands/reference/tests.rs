//! P1 depth tests for the single reference resolver.
//!
//! These pin the resolution *spec*, not an implementation: probe precedence,
//! the prefix rule, and the two distinct ambiguity refusals. The cross-verb
//! journey tests that use this resolver live in `tests/coherence.rs`.

use deadreckon_core::campaign::{Campaign, CampaignStatus, write_campaign};
use deadreckon_core::paths::workspace_scope;
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, OnFail, Plan, PlanMode,
    PlanProviders, PlanRole, PlanTask, RunOptions, append_job_event, create_run, save_chain,
    save_plan, write_job,
};
use deadreckon_protocol::{
    Job, JobEvent, JobEventKind, JobEventSequence, JobId, JobPolicy, JobSchemaVersion, JobShape,
    SemanticJudgeMode,
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
            root_planner_accounting: None,
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

    fn job(&self, job_id: &str, updated_at: DateTime<Utc>) -> JobView {
        let job_id = JobId(job_id.to_string());
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: self.scope(),
            goal: "durable job goal".to_string(),
            shape: JobShape::Single,
            created_at: updated_at - chrono::Duration::seconds(1),
            source_cwd: self.cwd.clone(),
            launch_plan_sha256: "launch".to_string(),
            authority_sha256: "authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 2,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
            },
        };
        write_job(&self.paths, &job).expect("write job");
        for (index, kind) in [JobEventKind::Created, JobEventKind::Queued]
            .into_iter()
            .enumerate()
        {
            append_job_event(
                &self.paths,
                &JobEvent {
                    schema_version: JobSchemaVersion::CURRENT,
                    job_id: job_id.clone(),
                    sequence: JobEventSequence::new(index as u64 + 1).expect("sequence"),
                    event_id: format!("job-event-{index}-{job_id}"),
                    causation_id: format!("job-fixture-{job_id}"),
                    timestamp: updated_at,
                    lease_epoch: 0,
                    kind,
                    detail: Value::Null,
                },
            )
            .expect("job event");
        }
        JobView::load(&self.paths, job_id.as_ref()).expect("job view")
    }

    fn query<'a>(&self, reference: &'a str) -> RefQuery<'a> {
        RefQuery {
            reference: Some(reference),
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
fn job_root_outranks_same_id_backing_run_plan_and_campaign() {
    let fx = fixture();
    let job = fx.job("abababababababababababababababab", Utc::now());
    fx.run(job.job.job_id.as_ref(), "backing run");
    fx.plan(job.job.job_id.as_ref(), None);
    fx.campaign(job.job.job_id.as_ref());

    let resolved = resolve_ref(&fx.paths, fx.query(job.job.job_id.as_ref())).expect("job");

    assert_eq!(resolved.kind(), RefKind::Job);
    assert_eq!(resolved_id(&resolved), job.job.job_id.as_ref());
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
fn accepts_narrows_which_kinds_a_verb_will_take() {
    let fx = fixture();
    let plan = fx.plan("5555555555555555555555555555eeee", None);

    let run_only = RefQuery {
        reference: Some("5555"),
        all_scopes: true,
        verb: "verdict",
    };
    resolve_ref(&fx.paths, run_only).expect_err("a run-only verb must not resolve a plan");

    // Only the verb name differs, which is the point: acceptance is data in
    // VERB_REF_SPECS, not something a call site restates.
    let resolved = resolve_ref(
        &fx.paths,
        RefQuery {
            verb: "status",
            ..run_only
        },
    )
    .expect("status accepts plans");
    assert_eq!(resolved_id(&resolved), plan.plan_id);
}

/// One verb, both kinds. `undo` on a run restores a turn snapshot; on a chain
/// it unwinds an applied step. Those were two commands with two id spaces
/// (`undo --run` and `chain undo <id> --step`) for one intent.
#[test]
fn undo_accepts_a_chain_as_well_as_a_run() {
    let fx = fixture();
    let chain = fx.chain("7777777777777777777777777777aaaa");

    let resolved = resolve_ref(
        &fx.paths,
        RefQuery {
            reference: Some("7777"),
            all_scopes: true,
            verb: "undo",
        },
    )
    .expect("undo accepts a chain");

    assert_eq!(resolved_id(&resolved), chain.chain_id);
}

/// Undo still refuses kinds that have nothing to put back, so widening it did
/// not turn it into a catch-all.
#[test]
fn undo_still_refuses_a_plan() {
    let fx = fixture();
    fx.plan("8888888888888888888888888888bbbb", None);

    resolve_ref(
        &fx.paths,
        RefQuery {
            reference: Some("8888"),
            all_scopes: true,
            verb: "undo",
        },
    )
    .expect_err("a plan has no committed step of its own to undo");
}

#[test]
fn ref_kinds_all_contains_every_kind() {
    for kind in [
        RefKind::Job,
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
fn latest_is_the_first_listed_job() {
    let fx = fixture();
    fx.job(
        "1111111111111111111111111111aaaa",
        Utc::now() - chrono::Duration::minutes(1),
    );
    let newest = fx.job("2222222222222222222222222222bbbb", Utc::now());

    let listed = super::super::job::list_jobs(&fx.paths, Some(&fx.scope())).expect("jobs");
    let first_listed = listed.iter().rev().next().expect("first listed job");
    let latest =
        resolve_latest_in_scope(&fx.paths, RefKinds::ALL, Some(&fx.scope())).expect("latest job");

    assert_eq!(first_listed.job.job_id, newest.job.job_id);
    assert_eq!(latest.kind(), RefKind::Job);
    assert_eq!(resolved_id(&latest), first_listed.job.job_id.as_ref());
}

#[test]
fn every_listed_job_has_a_non_looping_five_command_journey() {
    let fx = fixture();
    let job = fx.job("3333333333333333333333333333cccc", Utc::now());
    let listed = super::super::job::list_jobs(&fx.paths, Some(&fx.scope())).expect("list");
    assert_eq!(listed.len(), 1, "start creates one row in list");

    for verb in ["attach", "status", "finish", "kill"] {
        let resolved = resolve_ref(
            &fx.paths,
            RefQuery {
                reference: Some(job.job.job_id.as_ref()),
                all_scopes: false,
                verb,
            },
        )
        .unwrap_or_else(|error| panic!("{verb} must accept the listed job: {error}"));
        assert_eq!(resolved.kind(), RefKind::Job, "{verb}");
        assert_ne!(
            redirect_verb_for(RefKind::Job, verb),
            "list",
            "{verb} must not send the listed id back to list"
        );
    }
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

// ---------------------------------------------------------------------------
// P2 — one `latest`
//
// Before Shakedown `latest` meant two things: `main.rs`'s `latest_run` was
// scope-bound and took the head of `list_runs` order, while `verdict.rs`'s
// private `resolve_latest_run` ignored scope entirely and sorted by
// `updated_at`. These pin the single rule both collapse into.
// ---------------------------------------------------------------------------

impl Fixture {
    /// Move a run's clock so ordering assertions do not race the wall clock.
    fn touch_run(&self, state: &mut PipelineState, updated_at: DateTime<Utc>) {
        state.updated_at = updated_at;
        deadreckon_core::save_state(state).expect("save state");
    }

    fn scope(&self) -> String {
        workspace_scope(&self.cwd).expect("scope")
    }
}

/// A second workspace, so scope-bound behavior has something to exclude.
fn other_cwd() -> TempDir {
    tempfile::tempdir().expect("other cwd")
}

#[test]
fn latest_and_last_are_the_same_reference() {
    let fx = fixture();
    let state = fx.run("1111111111111111111111111111aaaa", "only run");

    let by_latest =
        resolve_latest_in_scope(&fx.paths, RefKinds::RUN, Some(&fx.scope())).expect("latest");
    let by_word = resolve_ref(
        &fx.paths,
        RefQuery {
            reference: Some("last"),
            all_scopes: true,
            verb: "status",
        },
    )
    .expect("last");

    assert_eq!(resolved_id(&by_latest), state.run_id);
    assert_eq!(resolved_id(&by_word), state.run_id);
}

#[test]
fn latest_is_scope_bound_by_default() {
    let fx = fixture();
    let mine = fx.run("1111111111111111111111111111aaaa", "in scope");

    // A newer run in a different workspace must not win a scoped `latest`.
    let elsewhere = other_cwd();
    let mut theirs = create_run(
        &fx.paths,
        RunOptions {
            goal: "out of scope".to_string(),
            cwd: elsewhere.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: Some("2222222222222222222222222222bbbb".to_string()),
            codebase: None,
        },
    )
    .expect("other run");
    fx.touch_run(&mut theirs, Utc::now() + chrono::Duration::hours(1));

    let resolved =
        resolve_latest_in_scope(&fx.paths, RefKinds::RUN, Some(&fx.scope())).expect("latest");

    assert_eq!(resolved_id(&resolved), mine.run_id);
}

#[test]
fn latest_all_widens_to_every_scope() {
    let fx = fixture();
    let mut mine = fx.run("1111111111111111111111111111aaaa", "in scope");
    fx.touch_run(&mut mine, Utc::now() - chrono::Duration::hours(1));

    let elsewhere = other_cwd();
    let mut theirs = create_run(
        &fx.paths,
        RunOptions {
            goal: "out of scope".to_string(),
            cwd: elsewhere.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: Some("2222222222222222222222222222bbbb".to_string()),
            codebase: None,
        },
    )
    .expect("other run");
    fx.touch_run(&mut theirs, Utc::now());

    let resolved = resolve_latest_in_scope(&fx.paths, RefKinds::RUN, None).expect("latest --all");

    assert_eq!(resolved_id(&resolved), theirs.run_id);
}

#[test]
fn latest_orders_by_updated_at_across_kinds() {
    let fx = fixture();
    let mut run = fx.run("1111111111111111111111111111aaaa", "the run");
    let plan = fx.plan("2222222222222222222222222222bbbb", None);

    // The plan was just written, so an older run must lose to it.
    fx.touch_run(&mut run, Utc::now() - chrono::Duration::hours(2));
    let newest = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, None).expect("latest");
    assert_eq!(resolved_id(&newest), plan.plan_id, "plan is newer");

    // Move the run's clock ahead and the same call must flip to the run.
    fx.touch_run(&mut run, Utc::now() + chrono::Duration::hours(2));
    let newest = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, None).expect("latest");
    assert_eq!(resolved_id(&newest), run.run_id, "run is now newer");
}

#[test]
fn latest_resolves_to_a_plan_when_the_scope_has_no_runs() {
    let fx = fixture();
    // The reproduction's shape: plans exist, no runs. `status` refused here.
    let plan = fx.plan("2222222222222222222222222222bbbb", None);

    let resolved = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, None).expect("latest");

    assert_eq!(resolved.kind(), RefKind::Plan);
    assert_eq!(resolved_id(&resolved), plan.plan_id);
}

#[test]
fn latest_in_empty_scope_names_other_scopes_when_they_have_work() {
    let fx = fixture();
    let elsewhere = other_cwd();
    create_run(
        &fx.paths,
        RunOptions {
            goal: "somewhere else".to_string(),
            cwd: elsewhere.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: Some("2222222222222222222222222222bbbb".to_string()),
            codebase: None,
        },
    )
    .expect("other run");

    let error = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, Some(&fx.scope()))
        .expect_err("empty scope");
    let text = err_text(&error);

    assert!(text.contains("nothing in this project yet"), "{text}");
    assert!(text.contains("deadreckon list --all"), "{text}");
}

#[test]
fn latest_in_a_wholly_empty_home_points_at_start() {
    let fx = fixture();

    let error =
        resolve_latest_in_scope(&fx.paths, RefKinds::ALL, Some(&fx.scope())).expect_err("empty");
    let text = err_text(&error);

    assert!(text.contains("nothing in this project yet"), "{text}");
    assert!(text.contains("deadreckon start"), "{text}");
    assert!(
        !text.contains("--all"),
        "nothing anywhere to widen to: {text}"
    );
}

#[test]
fn latest_respects_accepts() {
    let fx = fixture();
    let mut run = fx.run("1111111111111111111111111111aaaa", "the run");
    let plan = fx.plan("2222222222222222222222222222bbbb", None);
    fx.touch_run(&mut run, Utc::now() - chrono::Duration::hours(2));

    // Plan is newer, but a run-only verb must still land on the run.
    let resolved = resolve_latest_in_scope(&fx.paths, RefKinds::RUN, None).expect("latest run");
    assert_eq!(resolved_id(&resolved), run.run_id);

    let resolved = resolve_latest_in_scope(&fx.paths, RefKinds::PLAN, None).expect("latest plan");
    assert_eq!(resolved_id(&resolved), plan.plan_id);
}

#[test]
fn report_accepts_durable_jobs() {
    let fx = fixture();
    let job = fx.job(
        "3333333333333333333333333333cccc",
        Utc::now() - chrono::Duration::minutes(1),
    );

    assert!(accepts_for("report").contains(RefKind::Job));
    let resolved = resolve_ref(
        &fx.paths,
        RefQuery {
            reference: Some(job.job.job_id.as_ref()),
            all_scopes: true,
            verb: "report",
        },
    )
    .expect("report resolves a durable job");
    assert_eq!(resolved.kind(), RefKind::Job);
}

#[test]
fn cleanup_accepts_durable_jobs_with_same_id_backing_runs() {
    assert!(accepts_for("cleanup").contains(RefKind::Job));
}

#[test]
fn campaigns_are_not_candidates_for_a_scope_bound_latest() {
    let fx = fixture();
    // A campaign carries no scope of its own, so it cannot be attributed to
    // this project; `--all` is the only way to reach it via `latest`.
    fx.campaign("4444444444444444444444444444dddd");

    let error = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, Some(&fx.scope()))
        .expect_err("no scoped candidate");
    assert!(err_text(&error).contains("nothing in this project yet"));

    let resolved = resolve_latest_in_scope(&fx.paths, RefKinds::ALL, None).expect("latest --all");
    assert_eq!(resolved.kind(), RefKind::Campaign);
}

// ---------------------------------------------------------------------------
// P3 — refusals that go somewhere
//
// The defect this slice exists to remove is not "a verb refused". It is "a verb
// refused and sent the operator to a command that refuses too". These pin the
// refusal *quality*: every cell of the table has a message and a `try:`, and no
// `try:` points back at the verb that produced it.
// ---------------------------------------------------------------------------

#[test]
fn every_refusal_pair_in_the_table_has_a_message_and_a_try() {
    for spec in VERB_REF_SPECS {
        for kind in ALL_REF_KINDS {
            if spec.accepts.contains(kind) {
                continue;
            }
            let error = refusal_for(kind, spec.verb, "0c11f68e");
            let text = err_text(&error);

            assert!(
                text.contains("0c11f68e"),
                "{} x {}: refusal must name the reference: {text}",
                spec.verb,
                kind.noun()
            );
            assert!(
                text.contains("try:"),
                "{} x {}: refusal must carry a try line: {text}",
                spec.verb,
                kind.noun()
            );
            assert!(
                text.contains("deadreckon "),
                "{} x {}: the try line must be a command: {text}",
                spec.verb,
                kind.noun()
            );
        }
    }
}

#[test]
fn refusal_try_target_is_never_the_originating_verb() {
    for spec in VERB_REF_SPECS {
        for kind in ALL_REF_KINDS {
            if spec.accepts.contains(kind) {
                continue;
            }
            let text = err_text(&refusal_for(kind, spec.verb, "0c11f68e"));
            let try_line = text
                .lines()
                .find(|line| line.contains("try:"))
                .unwrap_or_default()
                .to_string();

            assert!(
                !try_line.contains(&format!("deadreckon {} ", spec.verb)),
                "{} x {}: refusal sends the operator back to itself: {try_line}",
                spec.verb,
                kind.noun()
            );
        }
    }
}

#[test]
fn refusal_try_target_accepts_the_kind_it_was_given() {
    // The invariant in one assertion: the verb a refusal names must itself
    // accept the kind that was refused. Without this, "try: deadreckon show"
    // is just a different dead end.
    for spec in VERB_REF_SPECS {
        for kind in ALL_REF_KINDS {
            if spec.accepts.contains(kind) {
                continue;
            }
            let redirect = redirect_verb_for(kind, spec.verb);
            let Some(target) = VERB_REF_SPECS
                .iter()
                .find(|candidate| candidate.verb == redirect)
            else {
                // Sub-commands like `chain resume` are not top-level rows.
                continue;
            };
            assert!(
                target.accepts.contains(kind),
                "{} refuses a {} and names `{}`, which does not accept one either",
                spec.verb,
                kind.noun(),
                redirect
            );
        }
    }
}

#[test]
fn refusal_names_the_operator_noun_not_the_rust_type() {
    let text = err_text(&refusal_for(RefKind::Plan, "status", "0c11f68e"));

    assert!(text.contains("plan"), "{text}");
    for rust_name in ["Plan", "PipelineState", "ResolvedRef", "RefKind"] {
        assert!(
            !text.contains(rust_name),
            "operator text leaked the type name {rust_name}: {text}"
        );
    }
}

#[test]
fn a_plan_given_to_a_run_only_verb_is_refused_by_kind_not_by_absence() {
    let fx = fixture();
    let plan = fx.plan("0c11f68e00000000000000000000aaaa", None);

    let error = resolve_ref(
        &fx.paths,
        RefQuery {
            reference: Some("0c11f68e"),
            all_scopes: true,
            verb: "verdict",
        },
    )
    .expect_err("a run-only verb cannot take a plan");
    let text = err_text(&error);

    // "not found: run 0c11f68e" was the old answer, and it was false -- the id
    // exists, it is simply a plan.
    assert!(!text.contains("not found"), "{text}");
    assert!(text.contains("is a plan"), "{text}");
    assert!(
        text.contains(&format!("deadreckon show {}", plan.plan_id)),
        "{text}"
    );
}

#[test]
fn steering_a_plan_points_at_attach_and_resuming_one_points_at_fork() {
    let steer = err_text(&refusal_for(RefKind::Plan, "steer", "0c11f68e"));
    assert!(steer.contains("deadreckon attach 0c11f68e"), "{steer}");

    let resume = err_text(&refusal_for(RefKind::Plan, "resume", "0c11f68e"));
    assert!(resume.contains("deadreckon fork 0c11f68e"), "{resume}");

    let chain = err_text(&refusal_for(RefKind::Chain, "resume", "0c11f68e"));
    assert!(
        chain.contains("deadreckon chain resume 0c11f68e"),
        "{chain}"
    );
}

#[test]
fn every_ambiguity_refusal_carries_a_runnable_try() {
    // Caught in P5: routing runs through `probe_run` dropped the `try:` the old
    // verdict path wrapped around the loader's ambiguity error, leaving a
    // refusal with no way forward. "use a longer prefix" is advice; a refusal in
    // this slice names something the operator can run.
    let fx = fixture();
    fx.run("abcd1111111111111111111111111111", "first");
    fx.run("abcd2222222222222222222222222222", "second");
    fx.plan("beef1111111111111111111111111111", None);
    fx.plan("beef2222222222222222222222222222", None);
    fx.chain("f00d1111111111111111111111111111");
    fx.chain("f00d2222222222222222222222222222");

    for prefix in ["abcd", "beef", "f00d"] {
        let text = err_text(&resolve_ref(&fx.paths, fx.query(prefix)).expect_err("ambiguous"));
        assert!(text.contains("ambiguous"), "{prefix}: {text}");
        assert!(text.contains("try:"), "{prefix}: no way forward: {text}");
        assert!(text.contains("deadreckon "), "{prefix}: {text}");
    }
}

#[test]
fn every_verb_used_in_source_is_listed_in_the_acceptance_table() {
    // `RefQuery` derives `accepts` from the verb name, so a verb absent from the
    // table silently gets `ALL`. That is the safe direction -- it can only make a
    // refusal less likely, never send an operator somewhere wrong -- but it would
    // also mean the matrix the tests iterate is not the one the code obeys. This
    // keeps them the same list.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let callers = [
        "resolve_run_like(",
        "refusal_for_reference(",
        "try_resolve_run(",
    ];
    let mut missing = Vec::new();
    for file in rust_files(&src) {
        if file.to_string_lossy().contains("commands/reference") {
            continue;
        }
        let text = fs::read_to_string(&file).expect("source");
        let lines = text.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            // A `verb:` field, but only inside a RefQuery literal.
            if line.contains("verb: \"")
                && lines[index.saturating_sub(6)..index]
                    .iter()
                    .any(|prior| prior.contains("RefQuery {"))
                && let Some(verb) = quoted_after(line, "verb: \"")
                && !VERB_REF_SPECS.iter().any(|spec| spec.verb == verb)
            {
                missing.push(format!("{}: {verb}", file.display()));
            }
            // A verb name passed positionally to a resolver helper.
            for caller in callers {
                if let Some(rest) = line.split_once(caller)
                    && let Some(verb) = last_quoted(rest.1)
                    && !VERB_REF_SPECS.iter().any(|spec| spec.verb == verb)
                {
                    missing.push(format!("{}: {verb}", file.display()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these verbs resolve references but are absent from VERB_REF_SPECS:\n{}",
        missing.join("\n")
    );
}

/// The verb is the last argument these helpers take, so a leading string
/// argument (a run id in a test fixture, say) must not be mistaken for it.
fn last_quoted(rest: &str) -> Option<String> {
    let mut parts = rest.split('"');
    let mut last = None;
    // Quoted segments sit at odd indexes once split on the quote character.
    for (index, part) in parts.by_ref().enumerate() {
        if index % 2 == 1 {
            last = Some(part.to_string());
        }
    }
    last.filter(|value| {
        !value.is_empty() && value.chars().all(|c| c.is_ascii_lowercase() || c == '-')
    })
}

fn quoted_after(line: &str, marker: &str) -> Option<String> {
    let (_, rest) = line.split_once(marker)?;
    let (value, _) = rest.split_once('"')?;
    (!value.is_empty() && value.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
        .then(|| value.to_string())
}

fn rust_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}
