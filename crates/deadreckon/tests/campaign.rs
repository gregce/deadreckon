use deadreckon_core::DeadreckonPaths;
use tempfile::TempDir;

mod common;

use common::{deadreckon, stderr, stdout};

fn workdir(temp: &TempDir) -> std::path::PathBuf {
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).expect("workdir");
    work
}

#[test]
fn campaign_preview_writes_campaign_json_and_stops() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "build a thing",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
        ])
        .output()
        .expect("run campaign --preview");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );

    // A campaign.json was written, and the run stopped before any merge.
    let plans = paths.home().join("plans");
    let mut campaign_json = None;
    for entry in std::fs::read_dir(&plans).expect("plans dir") {
        let dir = entry.expect("entry").path();
        let candidate = dir.join("campaign.json");
        if candidate.is_file() {
            campaign_json = Some((dir, candidate));
        }
    }
    let (dir, candidate) =
        campaign_json.unwrap_or_else(|| panic!("campaign.json not written: {}", stdout(&output)));
    let body = std::fs::read_to_string(&candidate).expect("read campaign.json");
    assert!(body.contains("\"n\": 2"), "{body}");
    assert!(body.contains("\"status\": \"pending\""), "{body}");
    // Preview stops before fork: no merge-working dir.
    assert!(!dir.join("merge-working").exists());
}

#[test]
fn campaign_preflight_shows_depth_cap_and_tree_budget() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "build a thing",
            "--n",
            "2",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--max-spend",
            "10",
            "--preview",
        ])
        .output()
        .expect("run campaign preflight");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(out.contains("depth cap 2"), "{out}");
    assert!(out.contains("tree budget $10"), "{out}");
}

#[test]
fn campaign_without_n_uses_recommended_count() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    let output = deadreckon(&paths)
        .current_dir(&work)
        .args([
            "campaign",
            "rebuild billing, notifications, and admin",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--preview",
        ])
        .output()
        .expect("run campaign --preview");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );

    let plans = paths.home().join("plans");
    let campaign_json = std::fs::read_dir(&plans)
        .expect("plans dir")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("campaign.json"))
        .find(|path| path.is_file())
        .expect("campaign.json");
    let body = std::fs::read_to_string(campaign_json).expect("read campaign.json");
    assert!(body.contains("\"n\": 3"), "{body}");
    assert!(stdout(&output).contains("campaign: 3 sub-orchestrators"));
}

#[test]
fn campaign_rejects_n_outside_range_at_cli() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let work = workdir(&temp);

    for n in ["1", "7"] {
        let output = deadreckon(&paths)
            .current_dir(&work)
            .args([
                "campaign",
                "build a thing",
                "--n",
                n,
                "--planner-provider",
                "smoke",
                "--preview",
            ])
            .output()
            .expect("run campaign bad n");
        assert!(
            !output.status.success(),
            "--n {n} should be rejected: {}",
            stdout(&output)
        );
    }
}
