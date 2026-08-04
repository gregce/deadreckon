#![cfg(unix)]
#![allow(clippy::expect_used, clippy::needless_pass_by_value)]

use std::fs;
use std::io::Read as _;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::DeadreckonPaths;
use serde_json::json;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stdout};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Exercise the complete user-facing authoring path against the compatibility
/// boundaries enforced by current Codex CLI releases. This deliberately lives
/// above the provider unit tests: both the draft and critic schemas must cross
/// the real `def-done` command without an unsupported feature flag or a schema
/// shape that Codex would reject.
#[test]
fn def_done_accepts_current_codex_feature_and_schema_contract() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let workspace = temp.path().join("app");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(workspace.join("app.py"), "print('hello from app')\n").expect("application");

    let fixture_dir = temp.path().join("fixtures");
    fs::create_dir_all(&fixture_dir).expect("fixtures");
    let draft = fixture_dir.join("draft.json");
    let critic = fixture_dir.join("critic.json");
    fs::write(
        &draft,
        json!({
            "acceptance_yaml": concat!(
                "name: adversarial codex authoring\n",
                "checks:\n",
                "  - kind: shell\n",
                "    command: >-\n",
                "      test \"$(python3 app.py)\" = \"hello from app\"\n",
                "    cwd: \"{working_dir}\"\n"
            ),
            "acceptance_md": "# Done criteria\n\nRunning the application prints `hello from app`.\n",
            "files": []
        })
        .to_string(),
    )
    .expect("draft fixture");
    fs::write(
        &critic,
        json!({
            "stub_would_pass": false,
            "uncovered_goal_clauses": [],
            "weak_check_indices": [],
            "verdict": "pass"
        })
        .to_string(),
    )
    .expect("critic fixture");

    let fake_codex = temp.path().join("fake-codex");
    fs::write(&fake_codex, FAKE_CODEX).expect("fake Codex CLI");
    make_executable(&fake_codex);
    write_codex_config(&paths, &fake_codex);

    let invocation_count = temp.path().join("main-invocations");
    let schema_log = temp.path().join("validated-schemas.log");
    let argv_log = temp.path().join("main-argv.log");
    let mut command = deadreckon(&paths);
    command
        .current_dir(&workspace)
        .env("DEADRECKON_AUTH_PROBE", "0")
        .env("FAKE_CODEX_DRAFT", &draft)
        .env("FAKE_CODEX_CRITIC", &critic)
        .env("FAKE_CODEX_COUNT", &invocation_count)
        .env("FAKE_CODEX_SCHEMA_LOG", &schema_log)
        .env("FAKE_CODEX_ARGV_LOG", &argv_log)
        .args([
            "def-done",
            "running the Python app prints hello from app",
            "--provider",
            "cli:codex",
        ]);

    let output = output_with_timeout(command, COMMAND_TIMEOUT);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("completed def-done"), "{out}");
    assert!(
        workspace.join(".deadreckon/acceptance.yaml").is_file(),
        "the real authoring surface must persist the compiled draft"
    );
    assert_eq!(
        fs::read_to_string(&invocation_count)
            .expect("main invocation count")
            .trim(),
        "2",
        "the fake must serve exactly one draft and one critic turn"
    );
    assert_eq!(
        fs::read_to_string(&schema_log)
            .expect("schema validation log")
            .lines()
            .count(),
        2,
        "both controller-owned output schemas must reach and pass the Codex-compatible validator"
    );
    let argv = fs::read_to_string(&argv_log).expect("main argv log");
    assert!(
        !argv.contains("--disable\nweb_search_request\n"),
        "a removed/deprecated Codex feature must not be passed back to --disable:\n{argv}"
    );
}

fn write_codex_config(paths: &DeadreckonPaths, binary: &std::path::Path) {
    fs::create_dir_all(paths.home()).expect("DeadReckon home");
    fs::write(
        paths.config_path(),
        format!(
            r#"
fallback = ["cli:codex"]

[defaults]
provider = "cli:codex"
done_contract_max_wall_seconds = 30

[providers."cli:codex"]
kind = "cli-codex"
binary = "{}"
model = "gpt-test"
"#,
            toml_string(&binary.display().to_string())
        ),
    )
    .expect("provider config");
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path).expect("fake CLI metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod fake CLI");
}

fn output_with_timeout(mut command: Command, timeout: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start DeadReckon");
    let mut child_stdout = child.stdout.take().expect("DeadReckon stdout");
    let mut child_stderr = child.stderr.take().expect("DeadReckon stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        child_stdout.read_to_end(&mut bytes).expect("read stdout");
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        child_stderr.read_to_end(&mut bytes).expect("read stderr");
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll DeadReckon") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("def-done exceeded the {timeout:?} integration-test bound");
        }
        thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: stdout_reader.join().expect("stdout reader"),
        stderr: stderr_reader.join().expect("stderr reader"),
    }
}

const FAKE_CODEX: &str = r#"#!/bin/sh
set -eu

if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
  printf '%s\n' 'codex-cli adversarial-0.8.1'
  exit 0
fi

if [ "$#" -eq 2 ] && [ "$1" = "exec" ] && [ "$2" = "--help" ]; then
  printf '%s\n' 'resume --json --output-last-message --output-schema --ephemeral --ignore-user-config --ignore-rules --strict-config --disable'
  exit 0
fi

if [ "$#" -eq 2 ] && [ "$1" = "features" ] && [ "$2" = "list" ]; then
  cat <<'FEATURES'
apps stable true
browser_use stable true
browser_use_external stable true
browser_use_full_cdp_access stable true
code_mode stable true
code_mode_host stable true
computer_use stable true
enable_mcp_apps stable true
in_app_browser stable true
multi_agent stable true
plugins stable true
shell_tool stable true
standalone_web_search stable true
unified_exec stable true
web_search_request deprecated false
FEATURES
  exit 0
fi

schema=''
last_message=''
printf '%s\n' '--- invocation ---' >> "$FAKE_CODEX_ARGV_LOG"
while [ "$#" -gt 0 ]; do
  printf '%s\n' "$1" >> "$FAKE_CODEX_ARGV_LOG"
  case "$1" in
    --disable)
      [ "$#" -ge 2 ] || exit 61
      if [ "$2" = "web_search_request" ]; then
        printf '%s\n' 'unsupported feature web_search_request was passed to --disable' >&2
        exit 62
      fi
      printf '%s\n' "$2" >> "$FAKE_CODEX_ARGV_LOG"
      shift 2
      ;;
    --output-schema)
      [ "$#" -ge 2 ] || exit 63
      schema=$2
      printf '%s\n' "$2" >> "$FAKE_CODEX_ARGV_LOG"
      shift 2
      ;;
    -o|--output-last-message)
      [ "$#" -ge 2 ] || exit 64
      last_message=$2
      printf '%s\n' "$2" >> "$FAKE_CODEX_ARGV_LOG"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

[ -n "$schema" ] || { printf '%s\n' 'missing --output-schema' >&2; exit 65; }
[ -n "$last_message" ] || { printf '%s\n' 'missing output-last-message path' >&2; exit 66; }

python3 - "$schema" <<'PY'
import json
import sys

schema_path = sys.argv[1]
with open(schema_path, encoding="utf-8") as handle:
    root = json.load(handle)

def reject(message, path):
    raise SystemExit(f"Codex-compatible schema rejection at {path}: {message}")

def walk(node, path="$"):
    if not isinstance(node, dict):
        reject("schema node is not an object", path)
    object_shaped = (
        node.get("type") == "object"
        or "properties" in node
        or "additionalProperties" in node
        or "required" in node
    )
    if object_shaped:
        additional = node.get("additionalProperties")
        if isinstance(additional, dict):
            reject("dynamic additionalProperties maps are unsupported", path)
        if additional is not False:
            reject("additionalProperties must be false", path)
        properties = node.get("properties")
        required = node.get("required")
        if not isinstance(properties, dict):
            reject("properties must be a fixed object", path)
        if not isinstance(required, list):
            reject("required must be an array", path)
        if len(required) != len(set(required)) or set(required) != set(properties):
            reject("required must exactly match properties", path)
        for name, child in properties.items():
            walk(child, f"{path}.properties.{name}")
    if node.get("type") == "array":
        if "items" not in node:
            reject("array schema is missing items", path)
        walk(node["items"], f"{path}.items")
    for keyword in ("anyOf", "oneOf", "allOf"):
        if keyword in node:
            branches = node[keyword]
            if not isinstance(branches, list):
                reject(f"{keyword} must be an array", path)
            for index, branch in enumerate(branches):
                walk(branch, f"{path}.{keyword}[{index}]")
    if "$defs" in node:
        definitions = node["$defs"]
        if not isinstance(definitions, dict):
            reject("$defs must be an object", path)
        for name, definition in definitions.items():
            walk(definition, f"{path}.$defs.{name}")

walk(root)
PY
printf '%s\n' "$schema" >> "$FAKE_CODEX_SCHEMA_LOG"

count=0
if [ -f "$FAKE_CODEX_COUNT" ]; then
  count=$(cat "$FAKE_CODEX_COUNT")
fi
count=$((count + 1))
printf '%s\n' "$count" > "$FAKE_CODEX_COUNT"
case "$count" in
  1) fixture=$FAKE_CODEX_DRAFT ;;
  2) fixture=$FAKE_CODEX_CRITIC ;;
  *) printf '%s\n' "unexpected main invocation $count" >&2; exit 67 ;;
esac
cp "$fixture" "$last_message"

printf '%s\n' '{"type":"thread.started","thread_id":"done-authoring-adversarial"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"structured response written"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#;
