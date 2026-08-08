#![cfg(any(target_os = "macos", target_os = "linux"))]
#![allow(clippy::expect_used)]

//! `deadreckon supervisor` — the SUPERVISOR lifecycle machine surface
//! (FULL-DRIVE gap B5b).
//!
//! `install`/`start`/`stop --json` are G1 envelopes over the same
//! `VerdictSurface` facts the prose renders, refusals (including unmanaged
//! service units) are typed `{kind:"error"}` envelopes that never force the
//! conflicting unit aside, and `status --json` serializes the two-source
//! health truth — service-manager state AND the per-home instance
//! checkpoint — as one typed verdict word instead of refusing in prose.
//! Everything rides the hermetic `SupervisorServiceFixture`: no test can
//! touch the developer's real service manager.

use std::fs;
use std::process::Output;

use deadreckon_core::DeadreckonPaths;
use serde_json::Value;

mod common;

use common::{SupervisorServiceFixture, assert_success, repo_tempdir, stderr, stdout};

fn parse_stdout(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "expected one JSON object on stdout ({error})\nstdout:\n{}\nstderr:\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

fn status_json(service: &SupervisorServiceFixture) -> Value {
    let output = service
        .deadreckon()
        .args(["supervisor", "status", "--json"])
        .output()
        .expect("supervisor status --json");
    assert_success(&output);
    parse_stdout(&output)
}

#[test]
fn supervisor_lifecycle_json_envelopes_round_trip_on_the_hermetic_manager() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);
    let unit_path = service.managed_unit_path().display().to_string();

    let install = service
        .deadreckon()
        .args(["supervisor", "install", "--json"])
        .output()
        .expect("supervisor install --json");
    assert_success(&install);
    let value = parse_stdout(&install);
    assert_eq!(value["kind"], "supervisor", "{value}");
    assert_eq!(value["id"], "install", "{value}");
    assert_eq!(value["action"], "install", "{value}");
    assert_eq!(value["result"], "installed", "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    assert_eq!(value["unit_path"], unit_path.as_str(), "{value}");
    assert_eq!(
        value["deadreckon_home"],
        paths.home().display().to_string().as_str(),
        "{value}"
    );
    assert_eq!(value["next_actions"][0], "deadreckon supervisor start");
    assert_eq!(value["verdict"]["kind"], "completed", "{value}");
    assert_eq!(value["try_lines"], serde_json::json!([]));
    assert!(service.managed_unit_path().is_file());

    // Idempotent re-install is an honest no-op, not a second mutation claim.
    let reinstall = service
        .deadreckon()
        .args(["supervisor", "install", "--json"])
        .output()
        .expect("supervisor re-install --json");
    assert_success(&reinstall);
    let value = parse_stdout(&reinstall);
    assert_eq!(value["result"], "already-installed", "{value}");
    assert_eq!(value["status"], "no-op", "{value}");

    let start = service
        .deadreckon()
        .args(["supervisor", "start", "--json"])
        .output()
        .expect("supervisor start --json");
    assert_success(&start);
    let value = parse_stdout(&start);
    assert_eq!(value["kind"], "supervisor", "{value}");
    assert_eq!(value["action"], "start", "{value}");
    assert_eq!(value["result"], "started", "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    // The post-action state is observed from the (fixture) service manager,
    // which reports the unit loaded/active.
    assert_eq!(value["service_state"], "running", "{value}");

    let stop = service
        .deadreckon()
        .args(["supervisor", "stop", "--json"])
        .output()
        .expect("supervisor stop --json");
    assert_success(&stop);
    let value = parse_stdout(&stop);
    assert_eq!(value["action"], "stop", "{value}");
    assert_eq!(value["result"], "stopped", "{value}");
    assert_eq!(value["status"], "completed", "{value}");
    assert_eq!(value["unit_path"], unit_path.as_str(), "{value}");
    assert!(
        service.managed_unit_path().is_file(),
        "stop must retain the managed unit"
    );
}

#[test]
fn supervisor_stop_without_unit_is_a_noop_and_start_refuses_with_an_envelope() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);

    let stop = service
        .deadreckon()
        .args(["supervisor", "stop", "--json"])
        .output()
        .expect("supervisor stop --json without a unit");
    assert_success(&stop);
    let value = parse_stdout(&stop);
    assert_eq!(value["action"], "stop", "{value}");
    assert_eq!(value["result"], "already-stopped", "{value}");
    assert_eq!(value["status"], "no-op", "{value}");
    assert_eq!(value["service_state"], "not_installed", "{value}");

    let start = service
        .deadreckon()
        .args(["supervisor", "start", "--json"])
        .output()
        .expect("supervisor start --json without a unit");
    assert!(!start.status.success(), "{}", stdout(&start));
    let value = parse_stdout(&start);
    assert_eq!(value["kind"], "error", "{value}");
    assert_eq!(value["verb"], "supervisor", "{value}");
    assert_eq!(
        value["code"],
        i64::from(start.status.code().expect("exit code")),
        "{value}"
    );
    let message = value["message"].as_str().unwrap_or_default();
    assert!(message.contains("not installed"), "{value}");
    assert!(message.contains("supervisor install"), "{value}");
}

#[test]
fn supervisor_lifecycle_refuses_unmanaged_units_with_typed_envelopes_and_never_forces() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);
    let unit_path = service.managed_unit_path();
    fs::create_dir_all(unit_path.parent().expect("unit parent")).expect("unit dir");
    let foreign_unit = "# hand-authored supervisor unit owned by the operator\n";
    fs::write(&unit_path, foreign_unit).expect("unmanaged unit");

    for verb in ["install", "start", "status", "stop"] {
        let output = service
            .deadreckon()
            .args(["supervisor", verb, "--json"])
            .output()
            .expect("supervisor verb against an unmanaged unit");
        assert!(!output.status.success(), "{verb}:\n{}", stdout(&output));
        let value = parse_stdout(&output);
        assert_eq!(value["kind"], "error", "{verb}: {value}");
        assert_eq!(value["verb"], "supervisor", "{verb}: {value}");
        let message = value["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("unmanaged service unit"),
            "{verb}: {value}"
        );
        assert!(
            message.contains(&unit_path.display().to_string()),
            "{verb}: {value}"
        );
    }
    assert_eq!(
        fs::read_to_string(&unit_path).expect("unit after refusals"),
        foreign_unit,
        "refusals must never rewrite an unmanaged unit"
    );
}

#[test]
fn supervisor_status_json_reports_two_source_verdicts_without_refusing() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::unconfigured(&paths);

    // Nothing installed, nothing recorded: total severity, one honest word.
    let report = status_json(&service);
    assert_eq!(report["schema_version"], 4, "{report}");
    assert_eq!(report["installed"], "not_installed", "{report}");
    assert_eq!(report["service"], "not_installed", "{report}");
    assert_eq!(report["home_checkpoint"], "absent", "{report}");
    assert_eq!(report["verdict"], "down", "{report}");

    // Install the unit without starting a supervisor for this home. The
    // fixture manager reports the unit loaded/active, so the two sources now
    // disagree: running per the manager, no instance checkpoint in this
    // home. This exact state refused in prose before the typed verdict.
    let install = service
        .deadreckon()
        .args(["supervisor", "install"])
        .output()
        .expect("supervisor install");
    assert_success(&install);
    let report = status_json(&service);
    assert_eq!(report["service"], "running", "{report}");
    assert_eq!(report["home_checkpoint"], "absent", "{report}");
    assert_eq!(report["verdict"], "foreign_home", "{report}");
    assert!(
        report["verdict_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("checkpoint is absent"),
        "{report}"
    );
}

#[test]
fn supervisor_status_json_is_healthy_live_and_degrades_on_stale_checkpoint_identity() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let service = SupervisorServiceFixture::configured(&paths);

    let report = status_json(&service);
    assert_eq!(report["installed"], "current", "{report}");
    assert_eq!(report["service"], "running", "{report}");
    assert_eq!(report["home_checkpoint"], "present", "{report}");
    assert_eq!(report["verdict"], "healthy", "{report}");
    assert_eq!(report["verdict_reason"], Value::Null, "{report}");

    // Prose stays prose: the human surface is unchanged by the machine flag.
    let prose = service
        .deadreckon()
        .args(["supervisor", "status"])
        .output()
        .expect("supervisor status prose");
    assert_success(&prose);
    assert!(
        stdout(&prose).starts_with("supervisor service:"),
        "{}",
        stdout(&prose)
    );

    // Same home, different executable bytes recorded: stale identity is an
    // observable degraded state with the mismatch fact verbatim.
    let checkpoint_path = paths.home().join("supervisor/instance.json");
    let mut checkpoint: Value =
        serde_json::from_slice(&fs::read(&checkpoint_path).expect("checkpoint bytes"))
            .expect("checkpoint JSON");
    checkpoint["binary_sha256"] = Value::String("different-executable-sha256".to_string());
    fs::write(
        &checkpoint_path,
        serde_json::to_vec_pretty(&checkpoint).expect("stale checkpoint JSON"),
    )
    .expect("write stale checkpoint");
    let report = status_json(&service);
    assert_eq!(report["service"], "running", "{report}");
    assert_eq!(report["home_checkpoint"], "stale", "{report}");
    assert_eq!(report["verdict"], "degraded", "{report}");
    assert!(
        report["verdict_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("executable digest"),
        "{report}"
    );

    // A checkpoint recorded for another state home is the foreign-home word.
    checkpoint["deadreckon_home"] =
        Value::String(temp.path().join("another-state-home").display().to_string());
    fs::write(
        &checkpoint_path,
        serde_json::to_vec_pretty(&checkpoint).expect("foreign checkpoint JSON"),
    )
    .expect("write foreign checkpoint");
    let report = status_json(&service);
    assert_eq!(report["home_checkpoint"], "stale", "{report}");
    assert_eq!(report["verdict"], "foreign_home", "{report}");
    assert!(
        report["verdict_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("does not match the current DEADRECKON_HOME"),
        "{report}"
    );
    assert_eq!(
        report["checkpoint"]["deadreckon_home"], checkpoint["deadreckon_home"],
        "the checkpoint evidence must ride along verbatim: {report}"
    );
}
