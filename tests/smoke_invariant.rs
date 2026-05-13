    use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

static SMOKE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn smoke_baseline_matches_pre_rider_head() {
    assert_smoke_baseline_matches();
}

#[test]
fn smoke_baseline_holds_after_print_refactor() {
    assert_smoke_baseline_matches();
}

fn assert_smoke_baseline_matches() {
    let _guard = SMOKE_LOCK.lock().expect("smoke invariant lock");
    let root = workspace_root();
    let output = Command::new("make")
        .arg("smoke")
        .current_dir(&root)
        .output()
        .expect("run make smoke");
    assert!(
        output.status.success(),
        "make smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let narrative = final_narrative_path(&root.join(".deadreckon-smoke"))
        .expect("find final RUN-NARRATIVE.md from make smoke");
    let content = fs::read_to_string(&narrative).expect("read final RUN-NARRATIVE.md");
    let actual = sha256_hex(normalize_narrative(&content).as_bytes());
    let expected = fs::read_to_string(root.join("tests/.smoke-baseline"))
        .expect("read smoke baseline")
        .split_whitespace()
        .next()
        .expect("smoke baseline hash")
        .to_string();
    assert_eq!(
        expected,
        actual,
        "normalized smoke RUN-NARRATIVE.md changed at {}",
        narrative.display()
    );
}

fn final_narrative_path(home: &Path) -> Option<PathBuf> {
    let mut paths = Vec::new();
    collect_narratives(home, &mut paths);
    paths.sort();
    paths.into_iter().find(|path| {
        let display = path.to_string_lossy();
        display.contains("/library/")
            && display.ends_with("/docs/RUN-NARRATIVE.md")
            && !display.contains("/.deadreckon/")
    })
}

fn collect_narratives(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_narratives(&path, out);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("RUN-NARRATIVE.md") {
            out.push(path);
        }
    }
}

fn normalize_narrative(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.starts_with("**Date:** ") {
                "**Date:** <normalized>".to_string()
            } else if line.starts_with("**Last updated:** ") {
                "**Last updated:** <normalized>".to_string()
            } else if line.starts_with("**Run ID:** ") {
                "**Run ID:** `<normalized>`".to_string()
            } else if line.starts_with("**Owner:** ") {
                "**Owner:** <normalized>".to_string()
            } else if line.starts_with("- Tool: bash (") && line.ends_with("ms)") {
                "- Tool: bash (<duration>)".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("crates").is_dir() && dir.join("Cargo.toml").is_file() {
            return dir;
        }
        assert!(dir.pop(), "could not locate workspace root");
    }
}
