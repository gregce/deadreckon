#![allow(clippy::expect_used)]

#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use deadreckon_core::{
    DeadreckonPaths, JobView, PipelineState, RunOwnership, load_run, read_supervised_process,
    save_state, validate_acceptance_marker,
};
#[cfg(target_os = "macos")]
use deadreckon_protocol::{JobEventKind, JobOutcome, StopReason};
#[cfg(target_os = "macos")]
use serde_json::{Value, json};
#[cfg(target_os = "macos")]
use tempfile::TempDir;

#[cfg(target_os = "macos")]
mod common;

#[cfg(target_os = "macos")]
use common::SupervisorServiceFixture;

/// Depth test for the HIGH merge-repair trust-boundary gap.
///
/// Run this test while developing the fix:
///
/// cargo test -p deadreckon --test watchkeeper_repair_child_ownership \
///   graph_final_repair_child_must_be_trusted_before_parent_consumes_it -- --exact
///
/// Consuming the repair is allowed once it is both owned and contained, but an
/// untrusted repair must leave the parent without a result or receipt.
#[cfg(target_os = "macos")]
#[test]
fn graph_final_repair_child_must_be_trusted_before_parent_consumes_it() {
    let fixture = RepairFixture::new(RepairShape::Graph);
    let observation = fixture.run();

    assert_repair_trust_invariant(&observation);
    assert_trusted_repair_launched(&observation);
}

/// The Campaign driver reaches the same shared repair-child launcher after its
/// sub-orchestrator results conflict. It must enforce the identical ownership,
/// containment, consumption, and receipt invariant.
#[cfg(target_os = "macos")]
#[test]
fn durable_campaign_final_repair_child_must_be_trusted_before_parent_consumes_it() {
    let fixture = RepairFixture::new(RepairShape::Campaign);
    let observation = fixture.run();

    assert_repair_trust_invariant(&observation);
    assert_trusted_repair_launched(&observation);
}

/// Crash after the exact trusted repair is durable but before it is copied into
/// the parent result. Even when that repair consumed the final approved spend,
/// restart must adopt it before refusing any new work. A second repair launch
/// would cross both the immutable request identity and the approved budget.
#[cfg(target_os = "macos")]
#[test]
fn graph_and_campaign_adopt_final_budget_repair_after_driver_crash() {
    for shape in [RepairShape::Graph, RepairShape::Campaign] {
        let fixture = RepairFixture::new_with_repair_crash(shape);
        let output = match shape {
            RepairShape::Graph => fixture.start_graph(),
            RepairShape::Campaign => fixture.start_campaign(),
        };
        assert!(
            output.status.success(),
            "{shape:?} dispatch failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let job_id = only_directory_name(&fixture.paths.jobs_dir());
        let proof_dir = fixture.paths.merge_proofs(&job_id);
        wait_for_path(
            &proof_dir.join(".test-failpoint-after_trusted_discovery_before_copy"),
            Duration::from_secs(120),
            &fixture.paths,
            &job_id,
        );
        let repair_record: Value = serde_json::from_slice(
            &fs::read(proof_dir.join("repair-run.json")).expect("repair authority projection"),
        )
        .expect("repair authority JSON");
        let repair_run_id = repair_record["run_id"]
            .as_str()
            .expect("repair Run ID")
            .to_string();
        let job = deadreckon_core::load_job(&fixture.paths, &job_id).expect("parent Job");
        let mut repair = load_run(&fixture.paths, &repair_run_id).expect("repair Run");
        repair.total_spend_usd = job.policy.max_spend_usd;
        save_state(&repair).expect("persist final approved repair spend");

        let driver =
            read_supervised_process(&fixture.paths.job_dir(&job_id).join("supervised-child.json"))
                .expect("supervised driver process");
        signal_pid(driver.pid, nix::sys::signal::Signal::SIGKILL);

        let view = wait_for_terminal_job(&fixture.paths, &job_id);
        assert_eq!(view.projection.outcome, Some(JobOutcome::BudgetExhausted));
        assert_eq!(view.projection.stop_reason, Some(StopReason::SpendCap));
        let result = fixture
            .result_run(&job_id)
            .expect("trusted repair must be composed before bounded stop");
        assert_eq!(
            result_file(&fixture.paths, &result, "README.md").as_deref(),
            Some(shape.expected_repair_body())
        );

        let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&job_id))
            .expect("Job history");
        let repair_ids = history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::RepairChildAuthorityChanged)
            .filter_map(|event| event.detail.get("run_id").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            repair_ids,
            std::collections::BTreeSet::from([repair_run_id.as_str()])
        );
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
enum RepairShape {
    Graph,
    Campaign,
}

#[cfg(target_os = "macos")]
impl RepairShape {
    fn root_goal(self) -> &'static str {
        match self {
            Self::Graph => "Force final durable Graph repair containment.",
            Self::Campaign => "Force final durable Campaign repair containment.",
        }
    }

    fn expected_repair_body(self) -> &'static str {
        match self {
            Self::Graph => "# graph repair child sentinel\n",
            Self::Campaign => "# campaign repair child sentinel\n",
        }
    }
}

#[cfg(target_os = "macos")]
struct RepairFixture {
    _supervisor: SupervisorServiceFixture,
    _temp: TempDir,
    shape: RepairShape,
    paths: DeadreckonPaths,
    workspace: PathBuf,
    provider_id: String,
}

#[cfg(target_os = "macos")]
impl RepairFixture {
    fn new(shape: RepairShape) -> Self {
        Self::new_with_supervisor_env(shape, &[])
    }

    fn new_with_repair_crash(shape: RepairShape) -> Self {
        Self::new_with_supervisor_env(
            shape,
            &[
                ("DEADRECKON_TEST_MERGE_REPAIR_FAILPOINTS", "1"),
                (
                    "DEADRECKON_TEST_MERGE_REPAIR_FAILPOINT",
                    "after_trusted_discovery_before_copy",
                ),
                ("DEADRECKON_TEST_MERGE_REPAIR_FAILPOINT_PAUSE", "1"),
            ],
        )
    }

    fn new_with_supervisor_env(shape: RepairShape, env: &[(&str, &str)]) -> Self {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let provider_id = "cli:repair-ownership-fixture".to_string();

        fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
        fs::create_dir_all(paths.home()).expect("DeadReckon home");
        fs::write(workspace.join("README.md"), "# baseline\n").expect("baseline README");
        fs::write(
            workspace.join(".deadreckon/acceptance.yaml"),
            concat!(
                "name: repair ownership depth test\n",
                "checks:\n",
                "  - kind: file_exists\n",
                "    path: \"{working_dir}/README.md\"\n",
            ),
        )
        .expect("acceptance");
        init_git_repo(&workspace);
        write_repair_fixture_provider(&paths, temp.path(), &provider_id);
        let supervisor = SupervisorServiceFixture::configured_with_env(&paths, env);

        Self {
            _supervisor: supervisor,
            _temp: temp,
            shape,
            paths,
            workspace,
            provider_id,
        }
    }

    fn run(&self) -> RepairObservation {
        let output = match self.shape {
            RepairShape::Graph => self.start_graph(),
            RepairShape::Campaign => self.start_campaign(),
        };
        assert!(
            output.status.success(),
            "{:?} dispatch failed\nstdout:\n{}\nstderr:\n{}",
            self.shape,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let job_id = only_directory_name(&self.paths.jobs_dir());
        let view = wait_for_terminal_job(&self.paths, &job_id);
        let proof_dir = self.paths.merge_proofs(&job_id);
        let repair_path = proof_dir.join("repair-run.json");
        let (repair_run_id, repair_owner, repair_sandbox_requested, repair_containment) =
            match fs::read(&repair_path) {
                Ok(raw) => {
                    let repair_record: Value =
                        serde_json::from_slice(&raw).expect("repair Run record JSON");
                    let repair_run_id = repair_record["run_id"]
                        .as_str()
                        .expect("repair Run ID")
                        .to_string();
                    let repair = deadreckon_core::load_run(&self.paths, &repair_run_id)
                        .expect("repair Run remains inspectable");
                    let marker =
                        validate_acceptance_marker(&repair).expect("repair acceptance marker");
                    (
                        Some(repair_run_id),
                        repair.ownership,
                        Some(repair.sandbox),
                        Some((marker.contained, marker.sandbox_backend)),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // A safe implementation may refuse before launching any
                    // repair child. That is the desired fail-closed branch.
                    (None, None, None, None)
                }
                Err(error) => panic!("could not inspect {}: {error}", repair_path.display()),
            };
        let result = self.result_run(&job_id);
        let expected = self.shape.expected_repair_body();
        let parent_consumed_repair = result.as_ref().is_some_and(|state| {
            result_file(&self.paths, state, "README.md").is_some_and(|body| body == expected)
        });

        RepairObservation {
            shape: self.shape,
            job_id: job_id.clone(),
            repair_run_id,
            repair_owner,
            repair_sandbox_requested,
            repair_containment,
            parent_consumed_repair,
            parent_receipt_exists: self.paths.job_receipt(&job_id).is_file(),
            job_view: format!("{view:#?}"),
            driver_stderr: driver_stderr(&self.paths, &job_id),
        }
    }

    fn start_graph(&self) -> std::process::Output {
        let launch_plan = self._temp.path().join("graph-launch-plan.json");
        fs::write(
            &launch_plan,
            serde_json::to_vec_pretty(&json!({
                "schema": 1,
                "created_at": "2026-07-30T00:00:00Z",
                "goal": self.shape.root_goal(),
                "shape": "plan",
                "n": 2,
                "pieces": [
                    {
                        "id": "graph-zero",
                        "goal": "Write the Graph task-zero conflict fixture.",
                        "provider": self.provider_id
                    },
                    {
                        "id": "graph-one",
                        "goal": "Write the Graph task-one conflict fixture.",
                        "provider": self.provider_id
                    }
                ],
                "providers": {
                    "planner": self.provider_id,
                    "coder": self.provider_id
                },
                "budget": {
                    "ceiling_usd": 2.0,
                    "wall_seconds": 120
                },
                "contract": {
                    "source": "operator"
                },
                "signals": {},
                "resolution": {
                    "source": "operator",
                    "confidence": 1.0,
                    "rationale": "repair ownership depth fixture"
                }
            }))
            .expect("Graph launch plan JSON"),
        )
        .expect("Graph launch plan");

        self._supervisor
            .deadreckon()
            .current_dir(&self.workspace)
            .args([
                "start",
                "--plan",
                launch_plan.to_str().expect("launch plan path"),
                "--yes",
                "--quiet",
                "--plain",
                "--json",
            ])
            .output()
            .expect("durable Graph dispatch")
    }

    fn start_campaign(&self) -> std::process::Output {
        self._supervisor
            .deadreckon()
            .current_dir(&self.workspace)
            .args([
                "campaign",
                self.shape.root_goal(),
                "--n",
                "2",
                "--planner-provider",
                &self.provider_id,
                "--provider",
                &self.provider_id,
                "--max-spend",
                "4",
                "--max-wall-seconds",
                "180",
                "--sandbox",
                "sandbox-exec",
                "--acceptance",
                self.workspace
                    .join(".deadreckon/acceptance.yaml")
                    .to_str()
                    .expect("acceptance path"),
                "--yes",
                "--quiet",
                "--plain",
            ])
            .output()
            .expect("durable Campaign dispatch")
    }

    fn result_run(&self, job_id: &str) -> Option<PipelineState> {
        let result_id = match self.shape {
            RepairShape::Graph => {
                deadreckon_core::load_plan(&self.paths, job_id)
                    .ok()?
                    .merged_run_id?
            }
            RepairShape::Campaign => {
                deadreckon_core::campaign::read_campaign(&self.paths.plan_dir(job_id))
                    .ok()?
                    .merged_run_id?
            }
        };
        deadreckon_core::load_run(&self.paths, &result_id).ok()
    }
}

#[cfg(target_os = "macos")]
struct RepairObservation {
    shape: RepairShape,
    job_id: String,
    repair_run_id: Option<String>,
    repair_owner: Option<RunOwnership>,
    repair_sandbox_requested: Option<String>,
    repair_containment: Option<(bool, String)>,
    parent_consumed_repair: bool,
    parent_receipt_exists: bool,
    job_view: String,
    driver_stderr: String,
}

#[cfg(target_os = "macos")]
fn assert_repair_trust_invariant(observation: &RepairObservation) {
    let owned_by_parent = observation
        .repair_owner
        .as_ref()
        .is_some_and(|owner| owner.job_id == observation.job_id);
    let contained = observation
        .repair_sandbox_requested
        .as_deref()
        .is_some_and(|requested| requested != "none")
        && observation
            .repair_containment
            .as_ref()
            .is_some_and(|(contained, backend)| *contained && backend != "none");
    let trusted_repair = owned_by_parent && contained;
    let failed_closed = !observation.parent_consumed_repair && !observation.parent_receipt_exists;

    assert!(
        trusted_repair || failed_closed,
        concat!(
            "{shape:?} trusted parent consumed an untrusted repair child\n",
            "parent Job: {job_id}\n",
            "repair Run: {repair_run_id:?}\n",
            "owned by parent: {owned_by_parent}\n",
            "ownership: {ownership:#?}\n",
            "sandbox requested: {sandbox_requested:?}\n",
            "gate containment: {repair_containment:?}\n",
            "parent consumed repair: {parent_consumed}\n",
            "parent receipt exists: {receipt_exists}\n",
            "desired: repair is parent-owned and contained, or parent fails closed ",
            "without consuming the repair and without issuing a receipt\n",
            "Job:\n{job_view}\n",
            "Driver stderr:\n{driver_stderr}"
        ),
        shape = observation.shape,
        job_id = observation.job_id,
        repair_run_id = observation.repair_run_id,
        owned_by_parent = owned_by_parent,
        ownership = observation.repair_owner,
        sandbox_requested = observation.repair_sandbox_requested,
        repair_containment = observation.repair_containment,
        parent_consumed = observation.parent_consumed_repair,
        receipt_exists = observation.parent_receipt_exists,
        job_view = observation.job_view,
        driver_stderr = observation.driver_stderr,
    );
}

#[cfg(target_os = "macos")]
fn assert_trusted_repair_launched(observation: &RepairObservation) {
    let ownership = observation
        .repair_owner
        .as_ref()
        .expect("durable repair child must retain parent ownership");
    assert_eq!(ownership.job_id, observation.job_id);
    assert!(
        observation
            .repair_sandbox_requested
            .as_deref()
            .is_some_and(|requested| requested != "none")
    );
    assert!(
        observation
            .repair_containment
            .as_ref()
            .is_some_and(|(contained, backend)| *contained && backend != "none")
    );
    assert!(
        observation.parent_consumed_repair,
        "trusted repair child should be composed into the parent result\n{}",
        observation.driver_stderr
    );
}

#[cfg(target_os = "macos")]
fn write_repair_fixture_provider(paths: &DeadreckonPaths, root: &Path, provider_id: &str) {
    let provider_root = root.join("providers");
    let descriptor_root = paths.home().join("providers.d");
    fs::create_dir_all(&provider_root).expect("provider root");
    fs::create_dir_all(&descriptor_root).expect("provider descriptors");
    let binary = provider_root.join("repair-ownership-fixture.sh");
    fs::write(
        &binary,
        r#"#!/bin/sh
prompt=$1

write_notes() {
  cat > implementation-notes.html <<'HTML'
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>Repair ownership fixture.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>None.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
HTML
}

case "$prompt" in
  *"You are an independent completion judge"*)
    printf '%s\n' '{"decision":"uncertain","summary":"The fixture leaves semantic completion for operator review.","goal_coverage":[{"claim":"repair trust lineage","status":"unclear","evidence":["authority"]}],"missing":["trusted repair ownership evidence"]}'
    ;;
  *"read-only planning agent"*"Force final durable Campaign"*)
    printf '%s\n' '{"tasks":[{"subject":"Build campaign lane zero","goal":"campaign-lane-zero","active_form":"Building lane zero","depends_on":[]},{"subject":"Build campaign lane one","goal":"campaign-lane-one","active_form":"Building lane one","depends_on":[]}]}'
    ;;
  *"read-only planning agent"*"campaign-lane-zero"*)
    printf '%s\n' '{"tasks":[{"subject":"Write lane zero A","goal":"campaign-lane-zero child A","active_form":"Writing lane zero A","depends_on":[]},{"subject":"Write lane zero B","goal":"campaign-lane-zero child B","active_form":"Writing lane zero B","depends_on":[]}]}'
    ;;
  *"read-only planning agent"*"campaign-lane-one"*)
    printf '%s\n' '{"tasks":[{"subject":"Write lane one A","goal":"campaign-lane-one child A","active_form":"Writing lane one A","depends_on":[]},{"subject":"Write lane one B","goal":"campaign-lane-one child B","active_form":"Writing lane one B","depends_on":[]}]}'
    ;;
  *"read-only merge repair planner"*"Force final durable Campaign"*)
    printf '%s\n' '{"decision":"spawn_repair_child","rationale":"force the Campaign ownership seam","actions":[{"path":"README.md","action":"repair_child","preserve":["both campaign lanes"]}],"repair_goal":"Write the final Campaign repair sentinel."}'
    ;;
  *"read-only merge repair planner"*)
    printf '%s\n' '{"decision":"spawn_repair_child","rationale":"force the Graph ownership seam","actions":[{"path":"README.md","action":"repair_child","preserve":["both graph tasks"]}],"repair_goal":"Write the final Graph repair sentinel."}'
    ;;
  *"Task: task-0"*"Force final durable Graph"*)
    printf '%s\n' '# graph task zero' > README.md
    write_notes
    printf '%s\n' 'graph task zero complete'
    ;;
  *"Task: task-1"*"Force final durable Graph"*)
    printf '%s\n' '# graph task one' > README.md
    write_notes
    printf '%s\n' 'graph task one complete'
    ;;
  *"campaign-lane-zero"*)
    printf '%s\n' '# campaign lane zero' > README.md
    write_notes
    printf '%s\n' 'campaign lane zero complete'
    ;;
  *"campaign-lane-one"*)
    printf '%s\n' '# campaign lane one' > README.md
    write_notes
    printf '%s\n' 'campaign lane one complete'
    ;;
  *"Root goal: Force final durable Campaign"*)
    printf '%s\n' '# campaign repair child sentinel' > README.md
    write_notes
    printf '%s\n' 'campaign repair child complete'
    ;;
  *"Root goal: Force final durable Graph"*)
    printf '%s\n' '# graph repair child sentinel' > README.md
    write_notes
    printf '%s\n' 'graph repair child complete'
    ;;
  *)
    write_notes
    printf '%s\n' 'fixture completion'
    ;;
esac
"#,
    )
    .expect("provider executable");
    let mut permissions = fs::metadata(&binary)
        .expect("provider metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).expect("provider chmod");

    fs::write(
        descriptor_root.join("repair-ownership-fixture.toml"),
        format!(
            r#"
id = "{provider_id}"
display_name = "Repair Ownership Fixture"
kind = "cli"
default_binary = "{binary}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{{prompt}}"]
"#,
            binary = binary.display()
        ),
    )
    .expect("provider descriptor");
    fs::write(
        paths.config_path(),
        format!(
            r#"
default_provider = "{provider_id}"
fallback = ["{provider_id}"]

[defaults]
sandbox = "sandbox-exec"
cli_max_wall_seconds = 180

[providers."{provider_id}"]
binary = "{binary}"
"#,
            binary = binary.display()
        ),
    )
    .expect("config");
}

#[cfg(target_os = "macos")]
fn init_git_repo(workspace: &Path) {
    for args in [
        &["init", "--initial-branch=main"][..],
        &["config", "user.email", "watchkeeper@example.invalid"][..],
        &["config", "user.name", "Watchkeeper Repair Test"][..],
        &["add", "-A"][..],
        &["commit", "-m", "repair ownership fixture"][..],
    ] {
        let output = Command::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(target_os = "macos")]
fn wait_for_terminal_job(paths: &DeadreckonPaths, job_id: &str) -> JobView {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            return view;
        }
        assert!(
            Instant::now() < deadline,
            "Job {job_id} did not terminate\nDriver stderr:\n{}",
            driver_stderr(paths, job_id)
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_path(path: &Path, timeout: Duration, paths: &DeadreckonPaths, job_id: &str) {
    let deadline = Instant::now() + timeout;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "{} was not created before timeout\nDriver stderr:\n{}",
            path.display(),
            driver_stderr(paths, job_id)
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn signal_pid(pid: u32, signal: nix::sys::signal::Signal) {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(pid).expect("driver PID")),
        Some(signal),
    )
    .expect("signal driver process");
}

#[cfg(target_os = "macos")]
fn only_directory_name(path: &Path) -> String {
    let mut names = fs::read_dir(path)
        .expect("directory")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names.len(), 1, "expected one directory under {path:?}");
    names.remove(0)
}

#[cfg(target_os = "macos")]
fn result_file(paths: &DeadreckonPaths, state: &PipelineState, relative: &str) -> Option<String> {
    let library = paths.library_dir(&state.scope, &state.run_id);
    let root = if library.is_dir() {
        library
    } else {
        state.working_dir.clone()
    };
    fs::read_to_string(root.join(relative)).ok()
}

#[cfg(target_os = "macos")]
fn driver_stderr(paths: &DeadreckonPaths, job_id: &str) -> String {
    [
        paths.job_dir(job_id).join("supervisor.err"),
        paths.job_dir(job_id).join("supervisor-stderr.log"),
    ]
    .into_iter()
    .filter_map(|path| fs::read_to_string(path).ok())
    .collect::<Vec<_>>()
    .join("\n")
}
