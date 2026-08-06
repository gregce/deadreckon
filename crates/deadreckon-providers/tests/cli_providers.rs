use std::fs;

use deadreckon_providers::{
    ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRequest, ProviderRouter,
    WorkspaceAccess,
};
use deadreckon_sandbox::SandboxBackend;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cli_claude_code_provider_runs_fake_binary_and_captures_output() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_binary(&binary, "claude-output");
    let output_path = temp.path().join("turns/turn-1/claude.out");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: Some(output_path.clone()),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("claude-output"));
    assert!(response.spend.subscription);
    assert_eq!(response.trace["kind"], "cli_subagent");
    assert!(
        fs::read_to_string(output_path)
            .expect("out")
            .contains("claude-output")
    );
}

#[tokio::test]
async fn enforceably_read_only_request_denies_a_hostile_cli_workspace_write() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let target = workspace.join("planner-wrote.txt");
    let binary = temp.path().join("hostile-planner");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\n: > '{}'\nprintf 'read-only planner response\\n'\n",
            target.display()
        ),
    )
    .expect("hostile planner");
    chmod_exec(&binary);
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: vec!["--dangerously-skip-permissions".to_string()],
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let result = router
        .complete(&ProviderRequest::enforceably_read_only(
            "inspect without writing",
            128,
            &workspace,
        ))
        .await;

    assert!(!target.exists(), "hostile planner changed the workspace");
    match result {
        Ok(response) => {
            assert!(response.content.contains("read-only planner response"));
            assert_eq!(response.trace["workspace_access"], "read-only");
        }
        Err(error) => {
            let detail = error.to_string();
            assert!(
                detail.contains("read-only")
                    || detail.contains("sandbox")
                    || detail.contains("Operation not permitted")
                    || detail.contains("Read-only file system"),
                "{detail}"
            );
        }
    }
}

#[tokio::test]
async fn enforceably_read_only_request_remains_usable_with_an_operational_sandbox() {
    let Ok((backend, _)) = deadreckon_sandbox::resolve_backend(SandboxBackend::Auto) else {
        return;
    };
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let binary = temp.path().join("benign-planner");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'read-only planner response\\n'\n",
    )
    .expect("benign planner");
    chmod_exec(&binary);
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest::enforceably_read_only_with_backend(
            "inspect without writing",
            128,
            &workspace,
            backend,
        ))
        .await
        .expect("operational read-only planner");

    assert!(response.content.contains("read-only planner response"));
    assert_eq!(response.trace["workspace_access"], "read-only");
}

#[tokio::test]
async fn cli_provider_cancellation_stops_non_sandbox_process() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("slow-claude");
    fs::write(
        &binary,
        "#!/bin/sh\nsleep 30\nprintf 'should-not-finish\\n'\n",
    )
    .expect("write fake binary");
    chmod_exec(&binary);
    let pid_file = temp.path().join("provider.pid");
    let token = CancellationToken::new();
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let request = ProviderRequest {
        prompt: "make notes".to_string(),
        max_output_tokens: 128,
        cwd: Some(temp.path().to_path_buf()),
        output_path: None,
        sandbox_backend: None,
        workspace_access: WorkspaceAccess::ReadWrite,
        pid_file: Some(pid_file.clone()),
        cancellation_token: Some(token.clone()),
        session_dir: None,
        output_schema: None,
        capability_posture: None,
    };
    let completion = router.complete(&request);
    tokio::pin!(completion);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    token.cancel();
    let err = completion.await.expect_err("cancelled completion");

    assert!(err.to_string().contains("request cancelled"));
    assert!(!pid_file.exists());
}

#[tokio::test]
async fn cli_codex_provider_uses_exec_verb() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_binary(&binary, "codex-output");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:codex".to_string()]),
            providers: [(
                "cli:codex".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliCodex),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:codex".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("codex-output"));
    assert!(response.content.contains(
        "args:--ask-for-approval never exec --skip-git-repo-check --sandbox workspace-write -- make notes"
    ));
    assert!(response.spend.subscription);
}

#[tokio::test]
async fn cli_codex_provider_delimits_option_like_prompt_payload() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_binary(&binary, "codex-output");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:codex".to_string()]),
            providers: [(
                "cli:codex".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliCodex),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:codex".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "---\nname: narrator-overview\n---\nwrite docs".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response
            .content
            .contains(" -- ---\nname: narrator-overview")
    );
    assert_eq!(
        response.trace["args"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap(),
        "---\nname: narrator-overview\n---\nwrite docs"
    );
}

#[tokio::test]
async fn cli_codex_model_override_passes_model_flag() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_binary(&binary, "codex-output");
    let router = ProviderRouter::from_config_with_model(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:codex".to_string()]),
            providers: [(
                "cli:codex".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliCodex),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        Some("cli:codex"),
        Some("gpt-5.1-codex"),
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("exec --model gpt-5.1-codex"));
    assert_eq!(response.model, "gpt-5.1-codex");
    assert_eq!(
        router.selected_route_info().expect("route").model,
        "gpt-5.1-codex"
    );
}

#[tokio::test]
async fn cli_claude_model_config_passes_model_flag() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_binary(&binary, "claude-output");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("sonnet".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("--model sonnet"));
    assert_eq!(response.model, "sonnet");
}

#[tokio::test]
async fn generic_cli_provider_runs_descriptor_template() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let providers_dir = home.join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers dir");
    let binary = temp.path().join("fake-local-test");
    write_fake_binary(&binary, "local-test-output");
    fs::write(
        providers_dir.join("local-test.toml"),
        format!(
            r#"
id = "cli:local-test"
display_name = "Local Test CLI"
kind = "cli"
default_binary = "{}"
subscription = true
sandbox_writes = ["~/.local-test"]

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "--sandbox", "{{sandbox}}", "{{prompt}}"]
model_arg = "--model"

[install_hint]
url = "https://example.invalid/local-test"
try_lines = ["install local-test"]
"#,
            binary.display()
        ),
    )
    .expect("write provider descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:local-test\"\n").expect("write config");

    let router =
        ProviderRouter::from_config_path_with_model(&config_path, None, Some("local-model"))
            .expect("router");
    assert_eq!(
        router
            .selected_route_info()
            .expect("selected route")
            .executable
            .as_deref(),
        Some(binary.to_str().expect("binary path")),
        "diagnostics must observe the exact executable runtime constructed"
    );
    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("local-test-output"));
    assert!(
        response
            .content
            .contains("args:run --sandbox workspace-write --model local-model make notes"),
        "{}",
        response.content
    );
    assert_eq!(response.model, "local-model");
    assert_eq!(response.trace["kind"], "cli_subagent");
    assert!(
        response.trace["sandbox_write_allowlist"]
            .as_array()
            .expect("sandbox writes")
            .iter()
            .any(|path| path
                .as_str()
                .is_some_and(|path| path.ends_with(".local-test"))),
        "{}",
        response.trace
    );
    assert!(response.spend.subscription);
}

#[tokio::test]
async fn contractless_descriptor_behavior_is_byte_identical() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-contractless");
    write_fake_binary(&binary, "legacy-output");
    fs::write(
        home.join("providers.d/contractless.toml"),
        format!(
            r#"
id = "cli:contractless"
display_name = "Contractless"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]
"#,
            binary.display()
        ),
    )
    .expect("descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:contractless\"\n").expect("config");

    let response = ProviderRouter::from_config_path(&config_path, None)
        .expect("router")
        .complete(&ProviderRequest {
            prompt: "unchanged".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "legacy-output\nargs:run unchanged\n");
    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 0);
    assert_eq!(
        response.trace["args"],
        serde_json::json!(["run", "unchanged"])
    );
    assert!(
        response.trace.get("contract").is_none(),
        "{}",
        response.trace
    );
    assert!(
        response.trace.get("flight_rows").is_none(),
        "{}",
        response.trace
    );
    let mut keys = response
        .trace
        .as_object()
        .expect("trace object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "args",
            "binary",
            "descriptor",
            "duration_ms",
            "exit_code",
            "kind",
            "pid",
            "sandbox_backend",
            "sandbox_warning",
            "sandbox_write_allowlist",
            "stdout_path",
        ]
    );
}

#[allow(clippy::expect_used)]
fn write_generic_contract_binary(path: &std::path::Path, stdout: &str) {
    let script = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"--help\" ]; then printf '%s\\n' 'Options: --structured'; exit 0; fi\n\
cat <<'OUTPUT'\n{stdout}\nOUTPUT\n",
    );
    fs::write(path, script).expect("contract binary");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn generic_contract_router(temp: &TempDir, stdout: &str) -> ProviderRouter {
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-contract-cli");
    write_generic_contract_binary(&binary, stdout);
    fs::write(
        home.join("providers.d/contract-cli.toml"),
        format!(
            r#"
id = "cli:contract-cli"
display_name = "Contract CLI"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]

[contract]
stream_args = ["--structured"]
dialect = "json-lines"
conversation_id_path = "/session_id"
usage_input_path = "/usage/input"
usage_output_path = "/usage/output"
answer_path = "/answer"
error_flag_path = "/is_error"
probe_substring = "--structured"
"#,
            binary.display()
        ),
    )
    .expect("descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:contract-cli\"\n").expect("config");
    ProviderRouter::from_config_path(&config_path, None).expect("router")
}

#[tokio::test]
async fn contract_provider_reports_real_usage_and_answer() {
    let temp = TempDir::new().expect("tempdir");
    let router = generic_contract_router(
        &temp,
        r#"{"session_id":"session-1","usage":{"input":41,"output":9},"answer":"extracted answer","is_error":false}"#,
    );
    let response = router
        .complete(&ProviderRequest {
            prompt: "work".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "extracted answer");
    assert_eq!(response.usage.input_tokens, 41);
    assert_eq!(response.usage.output_tokens, 9);
    assert_eq!(response.trace["contract"]["active"], true);
    assert_eq!(
        response.trace["args"],
        serde_json::json!(["run", "--structured", "work"])
    );
}

#[tokio::test]
async fn contract_probe_can_target_a_subcommand_capability_surface() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("subcommand-probe-home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-subcommand-probe-cli");
    fs::write(
        &binary,
        "#!/bin/sh\n\
if [ \"$1\" = \"--help\" ]; then printf '%s\\n' 'top-level options'; exit 0; fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"--help\" ]; then printf '%s\\n' 'run options: --structured'; exit 0; fi\n\
printf '%s\\n' '{\"answer\":\"subcommand probe active\"}'\n",
    )
    .expect("probe binary");
    chmod_exec(&binary);
    fs::write(
        home.join("providers.d/subcommand-probe.toml"),
        format!(
            r#"
id = "cli:subcommand-probe"
display_name = "Subcommand Probe CLI"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]

[contract]
stream_args = ["--structured"]
dialect = "json-lines"
answer_path = "/answer"
probe_args = ["run", "--help"]
probe_substring = "--structured"
"#,
            binary.display()
        ),
    )
    .expect("descriptor");
    let config_path = home.join("config.toml");
    fs::write(
        &config_path,
        "default_provider = \"cli:subcommand-probe\"\n",
    )
    .expect("config");
    let router = ProviderRouter::from_config_path(&config_path, None).expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "work".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "subcommand probe active");
    assert_eq!(response.trace["contract"]["active"], true);
    assert_eq!(
        response.trace["args"],
        serde_json::json!(["run", "--structured", "work"])
    );
}

#[tokio::test]
async fn unparseable_output_degrades_with_caveat_generic() {
    let temp = TempDir::new().expect("tempdir");
    let router = generic_contract_router(&temp, "plain fallback response");
    let response = router
        .complete(&ProviderRequest {
            prompt: "work".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "plain fallback response\n");
    assert_eq!(response.usage.input_tokens, 0);
    let caveats = response.trace["caveats"].as_array().expect("caveats");
    assert!(
        caveats
            .iter()
            .any(|caveat| caveat["code"] == "provider.contract.degraded")
    );
}

#[allow(clippy::expect_used)]
fn generic_resume_router(temp: &TempDir) -> ProviderRouter {
    let home = temp.path().join("resume-home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-resume-cli");
    write_generic_contract_binary(
        &binary,
        r#"{"session_id":"resume-session-1","usage":{"input":2,"output":3},"answer":"resumed","is_error":false}"#,
    );
    fs::write(
        home.join("providers.d/resume-cli.toml"),
        format!(
            r#"
id = "cli:resume-cli"
display_name = "Resume CLI"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]

[contract]
stream_args = ["--structured"]
dialect = "json-lines"
conversation_id_path = "/session_id"
usage_input_path = "/usage/input"
usage_output_path = "/usage/output"
answer_path = "/answer"
error_flag_path = "/is_error"
resume_args = ["--session", "{{conversation_id}}"]
probe_substring = "--structured"
"#,
            binary.display()
        ),
    )
    .expect("descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:resume-cli\"\n").expect("config");
    ProviderRouter::from_config_path(&config_path, None).expect("router")
}

#[tokio::test]
async fn descriptor_resume_substitutes_conversation_id() {
    let temp = TempDir::new().expect("tempdir");
    let router = generic_resume_router(&temp);
    let session_dir = temp.path().join("run");
    let request = || ProviderRequest {
        prompt: "work".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        session_dir: Some(session_dir.clone()),
        ..Default::default()
    };
    router.complete(&request()).await.expect("first turn");
    let second = router.complete(&request()).await.expect("second turn");

    assert_eq!(second.trace["contract"]["resumed"], true);
    assert_eq!(
        second.trace["args"],
        serde_json::json!([
            "run",
            "--structured",
            "--session",
            "resume-session-1",
            "work"
        ])
    );
    let session: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("provider-session.json")).expect("session file"),
    )
    .expect("session json");
    assert_eq!(session["provider"], "cli:resume-cli");
    assert_eq!(session["conversation_id"], "resume-session-1");
}

#[tokio::test]
async fn no_resume_args_means_fresh_turns_without_caveat() {
    let temp = TempDir::new().expect("tempdir");
    let router = generic_contract_router(
        &temp,
        r#"{"session_id":"fresh-only","usage":{"input":1,"output":1},"answer":"fresh","is_error":false}"#,
    );
    let session_dir = temp.path().join("run");
    let request = || ProviderRequest {
        prompt: "work".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        session_dir: Some(session_dir.clone()),
        ..Default::default()
    };
    router.complete(&request()).await.expect("first turn");
    let second = router.complete(&request()).await.expect("second turn");

    assert_eq!(second.trace["contract"]["resume"], false);
    assert_eq!(second.trace["contract"]["resumed"], false);
    assert!(!session_dir.join("provider-session.json").exists());
    assert!(second.trace["caveats"].as_array().is_none_or(|caveats| {
        caveats
            .iter()
            .all(|caveat| caveat["code"] != "provider.session.reset")
    }));
}

const PI_PENNANT_FIXTURE: &str = include_str!("fixtures/pennant/pi-simple.jsonl");
const PI_ERROR_FIXTURE: &str = include_str!("fixtures/pennant/pi-insufficient-balance.jsonl");

#[allow(clippy::expect_used)]
fn write_pi_pennant_output_binary(path: &std::path::Path, output: &str) {
    let script = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"--help\" ]; then printf '%s\\n' 'Options: --mode --session'; exit 0; fi\n\
cat <<'JSONL'\n{output}\nJSONL\n"
    );
    fs::write(path, script).expect("pi fixture binary");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn write_pi_pennant_binary(path: &std::path::Path) {
    write_pi_pennant_output_binary(path, PI_PENNANT_FIXTURE);
}

#[allow(clippy::expect_used)]
fn pi_pennant_router(binary: &std::path::Path) -> ProviderRouter {
    ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:pi".to_string()]),
            providers: [(
                "cli:pi".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("pi router")
}

#[tokio::test]
async fn pi_fixture_yields_usage_answer_and_session() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-pi-pennant");
    write_pi_pennant_binary(&binary);
    let session_dir = temp.path().join("run");
    let response = pi_pennant_router(&binary)
        .complete(&ProviderRequest {
            prompt: "fixture".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            session_dir: Some(session_dir.clone()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(
        response.content, "PI_FIXTURE_OK",
        "trace: {}",
        response.trace
    );
    assert_eq!(response.usage.input_tokens, 403);
    assert_eq!(response.usage.output_tokens, 25);
    let reported_cost = response.trace["contract"]["reported_cost_usd"]
        .as_f64()
        .expect("reported cost");
    assert!((reported_cost - 0.000197055).abs() < f64::EPSILON);
    let session: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("provider-session.json")).expect("session file"),
    )
    .expect("session json");
    assert_eq!(
        session["conversation_id"],
        "019f6c15-c7c8-7936-b475-1637f9d25191"
    );
}

#[tokio::test]
async fn pi_zero_exit_error_event_is_a_provider_failure() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-pi-error");
    write_pi_pennant_output_binary(&binary, PI_ERROR_FIXTURE);
    let error = pi_pennant_router(&binary)
        .complete(&ProviderRequest {
            prompt: "fixture".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            session_dir: Some(temp.path().join("run")),
            ..Default::default()
        })
        .await
        .expect_err("Pi error events must not become successful worker turns");

    let message = error.to_string();
    assert!(
        message.contains("provider contract reported an error"),
        "{message}"
    );
    assert!(message.contains("402 Insufficient Balance"), "{message}");
}

#[tokio::test]
async fn pi_response_content_is_answer_not_json_blob() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-pi-pennant");
    write_pi_pennant_binary(&binary);
    let response = pi_pennant_router(&binary)
        .complete(&ProviderRequest {
            prompt: "fixture".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "PI_FIXTURE_OK");
    assert!(!response.content.contains("\"type\""));
    let args = response.trace["args"].as_array().expect("args");
    assert_eq!(args.iter().filter(|arg| *arg == "--mode").count(), 1);
    assert_eq!(args.iter().filter(|arg| *arg == "--print").count(), 1);
}

const COPILOT_PENNANT_FIXTURE: &str = include_str!("fixtures/pennant/copilot-simple.jsonl");
const COPILOT_TOOL_FIXTURE: &str = include_str!("fixtures/pennant/copilot-tool.jsonl");

#[allow(clippy::expect_used)]
fn write_copilot_pennant_binary(path: &std::path::Path) {
    let script = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"--help\" ]; then printf '%s\\n' 'Options: --output-format --resume'; exit 0; fi\n\
cat <<'JSONL'\n{COPILOT_PENNANT_FIXTURE}\nJSONL\n"
    );
    fs::write(path, script).expect("copilot fixture binary");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn copilot_pennant_router(binary: &std::path::Path) -> ProviderRouter {
    ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:copilot".to_string()]),
            providers: [(
                "cli:copilot".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("copilot router")
}

#[tokio::test]
async fn copilot_fixture_yields_usage_and_answer() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-copilot-pennant");
    write_copilot_pennant_binary(&binary);
    let session_dir = temp.path().join("run");
    let response = copilot_pennant_router(&binary)
        .complete(&ProviderRequest {
            prompt: "fixture".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            session_dir: Some(session_dir.clone()),
            ..Default::default()
        })
        .await
        .expect("completion");

    assert_eq!(response.content, "COPILOT_FIXTURE_OK");
    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 26);
    assert_eq!(response.trace["contract"]["dialect"], "json-lines");
    assert_eq!(
        response.trace["contract"]["missing_fields"],
        serde_json::json!([])
    );
    let session: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("provider-session.json")).expect("session file"),
    )
    .expect("session json");
    assert_eq!(
        session["conversation_id"],
        "c35073fd-b43b-4b04-8dea-f47047d0bbb0"
    );
}

#[test]
fn copilot_document_dialect_parses_single_json() {
    // The rider's pre-probe name is retained as an audit trail. Copilot 1.0.45
    // actually emits a sequence of standalone JSON documents even with
    // `--stream off`, so the honest descriptor dialect is JSON Lines.
    assert!(serde_json::from_str::<serde_json::Value>(COPILOT_PENNANT_FIXTURE).is_err());
    let documents = COPILOT_PENNANT_FIXTURE
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()
        .expect("each Copilot line is one JSON document");
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0]["data"]["content"], "COPILOT_FIXTURE_OK");
    assert_eq!(
        documents[1]["sessionId"],
        "c35073fd-b43b-4b04-8dea-f47047d0bbb0"
    );
}

#[tokio::test]
async fn copilot_tool_fixture_yields_one_terminal_flight_row() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-copilot-tool-pennant");
    let script = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"--help\" ]; then printf '%s\\n' 'Options: --output-format --resume'; exit 0; fi\n\
cat <<'JSONL'\n{COPILOT_TOOL_FIXTURE}\n{COPILOT_PENNANT_FIXTURE}\nJSONL\n"
    );
    fs::write(&binary, script).expect("copilot tool fixture binary");
    chmod_exec(&binary);

    let response = copilot_pennant_router(&binary)
        .complete(&ProviderRequest {
            prompt: "fixture".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");

    let rows = response.trace["flight_rows"]
        .as_array()
        .expect("flight rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "call_cqEGNV54zsmiZPCpaRIlA40u");
    assert_eq!(rows[0]["tool_name"], "bash");
    assert_eq!(rows[0]["status"], "tool.execution_complete");
    assert!(
        rows[0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("COPILOT_TOOL_OK"))
    );
}

#[tokio::test]
async fn generic_cli_provider_preserves_codex_prompt_delimiter() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let providers_dir = home.join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers dir");
    let binary = temp.path().join("fake-generic-codex");
    write_fake_binary(&binary, "generic-codex-output");
    fs::write(
        providers_dir.join("generic-codex.toml"),
        format!(
            r#"
id = "cli:generic-codex"
display_name = "Generic Codex"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["exec", "--", "{{prompt}}"]
"#,
            binary.display()
        ),
    )
    .expect("write provider descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:generic-codex\"\n").expect("write config");

    let router = ProviderRouter::from_config_path(&config_path, None).expect("router");
    let prompt = "---\nname: narrator-overview\n---\nwrite docs";
    let response = router
        .complete(&ProviderRequest {
            prompt: prompt.to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response
            .content
            .contains("args:exec -- ---\nname: narrator-overview"),
        "{}",
        response.content
    );
    let args = response.trace["args"].as_array().expect("args");
    assert_eq!(args[1], "--");
    assert_eq!(args.last().and_then(|arg| arg.as_str()), Some(prompt));
}

#[tokio::test]
async fn generic_cli_provider_passes_model_arg_from_descriptor() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-model-cli");
    write_fake_binary(&binary, "model-cli-output");
    fs::write(
        home.join("providers.d/model-cli.toml"),
        format!(
            r#"
id = "cli:model-cli"
display_name = "Model CLI"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]
model_arg = "--model"
"#,
            binary.display()
        ),
    )
    .expect("write provider descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:model-cli\"\n").expect("write config");

    let router =
        ProviderRouter::from_config_path_with_model(&config_path, None, Some("fast-model"))
            .expect("router");
    let response = router
        .complete(&ProviderRequest {
            prompt: "ship it".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response
            .content
            .contains("args:run --model fast-model ship it")
    );
    assert_eq!(response.model, "fast-model");
}

#[tokio::test]
async fn generic_cli_provider_places_model_before_prompt_value_flag() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-prompt-flag-cli");
    write_fake_binary(&binary, "prompt-flag-output");
    fs::write(
        home.join("providers.d/prompt-flag.toml"),
        format!(
            r#"
id = "cli:prompt-flag"
display_name = "Prompt Flag CLI"
kind = "cli"
default_binary = "{}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["-p", "{{prompt}}"]
model_arg = "--model"
"#,
            binary.display()
        ),
    )
    .expect("write provider descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:prompt-flag\"\n").expect("write config");

    let router =
        ProviderRouter::from_config_path_with_model(&config_path, None, Some("flag-model"))
            .expect("router");
    let response = router
        .complete(&ProviderRequest {
            prompt: "ship it".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response
            .content
            .contains("args:--model flag-model -p ship it")
    );
}

#[tokio::test]
async fn generic_cli_provider_runs_builtin_copilot_descriptor() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-copilot");
    write_fake_binary(&binary, "copilot-output");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:copilot".to_string()]),
            providers: [(
                "cli:copilot".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("copilot-output"));
    assert!(
        response.content.contains(
            "args:-p make notes --output-format json --stream off --no-color --allow-all"
        )
    );
    assert_eq!(response.trace["kind"], "cli_subagent");
    assert_eq!(response.trace["descriptor"], "cli:copilot");
    assert!(response.spend.subscription);
}

#[tokio::test]
async fn generic_cli_provider_passes_copilot_model_arg() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-copilot");
    write_fake_binary(&binary, "copilot-model-output");
    let router = ProviderRouter::from_config_with_model(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:copilot".to_string()]),
            providers: [(
                "cli:copilot".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        Some("cli:copilot"),
        Some("gpt-5.1"),
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "ship it".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains(
        "args:--model gpt-5.1 -p ship it --output-format json --stream off --no-color --allow-all"
    ));
    assert_eq!(response.model, "gpt-5.1");
}

#[tokio::test]
async fn generic_cli_provider_runs_builtin_pi_descriptor() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-pi");
    write_fake_binary(&binary, "pi-output");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:pi".to_string()]),
            providers: [(
                "cli:pi".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("pi-output"));
    assert!(
        response
            .content
            .contains("args:--mode json --print make notes")
    );
    assert_eq!(response.trace["kind"], "cli_subagent");
    assert_eq!(response.trace["descriptor"], "cli:pi");
    assert!(response.spend.subscription);
}

#[tokio::test]
async fn generic_cli_provider_passes_pi_model_arg() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-pi");
    write_fake_binary(&binary, "pi-model-output");
    let router = ProviderRouter::from_config_with_model(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:pi".to_string()]),
            providers: [(
                "cli:pi".to_string(),
                ProviderEntry {
                    kind: None,
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: None,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        Some("cli:pi"),
        Some("google/gemini-2.5-pro"),
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "ship it".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response
            .content
            .contains("args:--mode json --print --model google/gemini-2.5-pro ship it")
    );
    assert_eq!(response.model, "google/gemini-2.5-pro");
}

#[tokio::test]
async fn generic_cli_provider_uses_descriptor_sandbox_writes() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(home.join("providers.d")).expect("providers dir");
    let binary = temp.path().join("fake-sandbox-cli");
    write_fake_binary(&binary, "sandbox-cli-output");
    fs::write(
        home.join("providers.d/sandbox-cli.toml"),
        format!(
            r#"
id = "cli:sandbox-cli"
display_name = "Sandbox CLI"
kind = "cli"
default_binary = "{}"
subscription = true
sandbox_writes = ["~/.sandbox-cli"]

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{{prompt}}"]
"#,
            binary.display()
        ),
    )
    .expect("write provider descriptor");
    let config_path = home.join("config.toml");
    fs::write(&config_path, "default_provider = \"cli:sandbox-cli\"\n").expect("write config");

    let router = ProviderRouter::from_config_path(&config_path, None).expect("router");
    let response = router
        .complete(&ProviderRequest {
            prompt: "ship it".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(
        response.trace["sandbox_write_allowlist"]
            .as_array()
            .expect("sandbox writes")
            .iter()
            .any(|path| path
                .as_str()
                .is_some_and(|path| path.ends_with(".sandbox-cli"))),
        "{}",
        response.trace
    );
}

#[tokio::test]
async fn cli_provider_runs_inside_requested_sandbox_backend() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_binary(&binary, "sandboxed-claude-output");
    let pid_file = temp.path().join("child-pids/provider.pid");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: Some(SandboxBackend::None),
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: Some(pid_file.clone()),
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect("completion");

    assert!(response.content.contains("sandboxed-claude-output"));
    assert_eq!(response.trace["sandbox_backend"], "none");
    assert!(response.trace["pid"].as_u64().is_some());
    assert!(!pid_file.exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cli_provider_resolves_user_path_binary_inside_sandbox_exec() {
    const HELPER_ENV: &str = "DEADRECKON_TEST_PATH_HELPER";
    const TEMP_ENV: &str = "DEADRECKON_TEST_PATH_TEMP";

    if which::which("sandbox-exec").is_err() {
        return;
    }
    if std::env::var_os(HELPER_ENV).is_some() {
        let temp_path =
            std::path::PathBuf::from(std::env::var_os(TEMP_ENV).expect("helper temp path"));
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: None,
                fallback: Some(vec!["cli:codex".to_string()]),
                providers: [(
                    "cli:codex".to_string(),
                    ProviderEntry {
                        kind: Some(ProviderKind::CliCodex),
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: Some("cli:codex".to_string()),
                        input_cost_per_million: Some(0.0),
                        output_cost_per_million: Some(0.0),
                        binary: Some("fake-codex".to_string()),
                        extra_args: Vec::new(),
                    },
                )]
                .into_iter()
                .collect(),
            },
            None,
        )
        .expect("router");

        let response = router
            .complete(&ProviderRequest {
                prompt: "make notes".to_string(),
                max_output_tokens: 128,
                cwd: Some(temp_path),
                output_path: None,
                sandbox_backend: Some(SandboxBackend::SandboxExec),
                workspace_access: WorkspaceAccess::ReadWrite,
                pid_file: None,
                cancellation_token: None,
                session_dir: None,
                output_schema: None,
                capability_posture: None,
            })
            .await
            .expect("sandboxed completion");

        assert!(response.content.contains("path-resolved-codex-output"));
        assert!(response.content.contains("--sandbox danger-full-access"));
        assert_eq!(response.trace["sandbox_backend"], "sandbox-exec");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let bin_dir = temp.path().join("user-bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    let binary = bin_dir.join("fake-codex");
    write_fake_binary(&binary, "path-resolved-codex-output");
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());

    let output = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .args([
            "--exact",
            "cli_provider_resolves_user_path_binary_inside_sandbox_exec",
            "--nocapture",
        ])
        .env(HELPER_ENV, "1")
        .env(TEMP_ENV, temp.path())
        .env("PATH", new_path)
        .output()
        .expect("spawn path helper");

    assert!(
        output.status.success(),
        "helper failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn cli_provider_errors_on_nonzero_exit_after_capturing_output() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex-fail");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'partial stdout\\n'\nprintf 'failure stderr\\n' >&2\nexit 7\n",
    )
    .expect("write fake binary");
    chmod_exec(&binary);
    let output_path = temp.path().join("turns/turn-1/codex.out");
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:codex".to_string()]),
            providers: [(
                "cli:codex".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliCodex),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:codex".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let err = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: Some(output_path.clone()),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await
        .expect_err("nonzero exit should fail");

    assert!(err.to_string().contains("exited with Some(7)"));
    let captured = fs::read_to_string(output_path).expect("captured output");
    assert!(captured.contains("partial stdout"));
    assert!(captured.contains("failure stderr"));
}

#[allow(clippy::expect_used)]
fn write_fake_binary(path: &std::path::Path, label: &str) {
    fs::write(
        path,
        format!("#!/bin/sh\nprintf '{label}\\nargs:%s\\n' \"$*\"\n"),
    )
    .expect("write fake binary");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn chmod_exec(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

// ---------------------------------------------------------------------------
// Semaphore: capability-capable fake CLIs. `--help` prints the real flags so
// the probe lights up; a real run replays canned JSONL (and, for codex, writes
// the -o last-message file).
// ---------------------------------------------------------------------------

const FAKE_CODEX_HELP: &str = "\
Run Codex non-interactively
Commands:
  resume  Resume a previous session by id
Options:
      --output-schema <FILE>
      --ephemeral
      --ignore-user-config
      --ignore-rules
      --strict-config
      --disable <FEATURE>
      --json
  -o, --output-last-message <FILE>
";

const FAKE_CODEX_FEATURES: &str = "\
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
";

#[allow(clippy::expect_used)]
fn write_fake_codex(path: &std::path::Path, jsonl: &str, answer: &str) {
    let script = format!(
        "#!/bin/sh\n\
if printf '%s\\n' \"$*\" | grep -q -- '--version'; then echo 'codex-cli fake-1'; exit 0; fi\n\
if [ \"$*\" = \"features list\" ]; then\n\
cat <<'FEATURES'\n{features}\nFEATURES\n  exit 0\nfi\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"--help\" ]; then\n\
cat <<'HELP'\n{help}\nHELP\n    exit 0\n  fi\n\
done\n\
prev=\"\"; out=\"\"\n\
for a in \"$@\"; do\n\
  if [ \"$prev\" = \"-o\" ] || [ \"$prev\" = \"--output-last-message\" ]; then out=\"$a\"; fi\n\
  prev=\"$a\"\n\
done\n\
cat <<'JSONL'\n{jsonl}\nJSONL\n\
if [ -n \"$out\" ]; then\ncat > \"$out\" <<'ANSWER'\n{answer}\nANSWER\nfi\n\
exit 0\n",
        help = FAKE_CODEX_HELP,
        features = FAKE_CODEX_FEATURES,
        jsonl = jsonl,
        answer = answer,
    );
    fs::write(path, script).expect("write fake codex");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn codex_router(binary: &std::path::Path) -> ProviderRouter {
    ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:codex".to_string()]),
            providers: [(
                "cli:codex".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliCodex),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:codex".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router")
}

#[tokio::test]
async fn codex_turn_reports_real_token_usage() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    let jsonl = "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n\
                 {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":10,\"output_tokens\":40,\"reasoning_output_tokens\":0}}";
    write_fake_codex(&binary, jsonl, "{\"action\":\"done\"}");
    let out = temp.path().join("turns/turn-1/codex.out");
    let response = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            output_path: Some(out),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.output_tokens, 40);
    assert_eq!(response.spend.input_tokens, 100);
    assert!(response.spend.subscription);
    assert_eq!(response.spend.cost_usd, 0.0);
}

#[tokio::test]
async fn codex_response_content_is_last_message_not_stdout_noise() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    let jsonl = "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n\
                 {\"type\":\"item.completed\",\"item\":{\"id\":\"item_0\",\"type\":\"agent_message\",\"text\":\"noise\"}}\n\
                 {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"cached_input_tokens\":0,\"output_tokens\":1}}";
    write_fake_codex(
        &binary,
        jsonl,
        "{\"action\":\"finish\",\"summary\":\"clean\"}",
    );
    let out = temp.path().join("turns/turn-1/codex.out");
    let response = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            output_path: Some(out),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    assert_eq!(
        response.content,
        "{\"action\":\"finish\",\"summary\":\"clean\"}"
    );
    assert!(!response.content.contains("thread.started"));
}

#[tokio::test]
async fn codex_unparseable_stdout_degrades_with_caveat() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    // Capability-capable binary, but the run prints non-JSON and writes no
    // meaningful last-message: the driver degrades to raw stdout with a caveat.
    write_fake_codex(&binary, "this is not json\njust plain text", "");
    let out = temp.path().join("turns/turn-1/codex.out");
    let response = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            output_path: Some(out),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    assert!(response.content.contains("this is not json"));
    let caveats = response.trace["caveats"].as_array().expect("caveats");
    assert!(
        caveats
            .iter()
            .any(|c| c["code"] == "provider.contract.degraded")
    );
}

const FAKE_CLAUDE_HELP: &str = "\
Claude Code
  --json-schema <schema>   JSON Schema for structured output
  --output-format <format> choices: text, json, stream-json
  -r, --resume [sessionId] Resume a conversation
  -p, --print
";

#[allow(clippy::expect_used)]
fn write_fake_claude(path: &std::path::Path, jsonl: &str) {
    let script = format!(
        "#!/bin/sh\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"--help\" ]; then\n\
cat <<'HELP'\n{help}\nHELP\n    exit 0\n  fi\n\
done\n\
cat <<'JSONL'\n{jsonl}\nJSONL\n\
exit 0\n",
        help = FAKE_CLAUDE_HELP,
        jsonl = jsonl,
    );
    fs::write(path, script).expect("write fake claude");
    chmod_exec(path);
}

#[allow(clippy::expect_used)]
fn claude_router(binary: &std::path::Path) -> ProviderRouter {
    ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["cli:claude-code".to_string()]),
            providers: [(
                "cli:claude-code".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::CliClaudeCode),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some("cli:claude-code".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router")
}

const CLAUDE_SIMPLE_FIXTURE: &str = include_str!("fixtures/semaphore/claude-simple.jsonl");

#[tokio::test]
async fn claude_turn_reports_real_token_usage() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_claude(&binary, CLAUDE_SIMPLE_FIXTURE);
    let response = claude_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    assert_eq!(response.usage.input_tokens, 2);
    assert_eq!(response.usage.output_tokens, 4);
    assert_eq!(response.spend.input_tokens, 2);
}

#[tokio::test]
async fn claude_reported_cost_lands_in_trace_detail_not_spend() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_claude(&binary, CLAUDE_SIMPLE_FIXTURE);
    let response = claude_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    // Reported cost is trace detail only.
    assert_eq!(response.trace["contract"]["reported_cost_usd"], 0.131228);
    // Spend stays subscription/$0.
    assert_eq!(response.spend.cost_usd, 0.0);
    assert!(response.spend.subscription);
}

#[tokio::test]
async fn claude_response_content_is_result_text() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    write_fake_claude(&binary, CLAUDE_SIMPLE_FIXTURE);
    let response = claude_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect("completion");
    assert_eq!(response.content, "pong");
    assert!(!response.content.contains("\"type\":\"system\""));
}

#[tokio::test]
async fn claude_is_error_result_maps_to_provider_error() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    let jsonl = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n\
                 {\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"boom\",\"session_id\":\"s-1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}";
    write_fake_claude(&binary, jsonl);
    let err = claude_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .await
        .expect_err("is_error maps to provider error");
    assert!(err.to_string().contains("error result"));
}

// ---------------------------------------------------------------------------
// P7 — per-run resume.
// ---------------------------------------------------------------------------

const CODEX_TURN_JSONL: &str = "{\"type\":\"thread.started\",\"thread_id\":\"t-run1\"}\n\
{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":5,\"cached_input_tokens\":0,\"output_tokens\":2}}";

#[tokio::test]
async fn codex_second_turn_resumes_persisted_thread() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex(&binary, CODEX_TURN_JSONL, "{\"action\":\"done\"}");
    let session_dir = temp.path().join("run");
    let req = |turn: u32| ProviderRequest {
        prompt: "hi".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        output_path: Some(temp.path().join(format!("turns/turn-{turn}/codex.out"))),
        session_dir: Some(session_dir.clone()),
        ..Default::default()
    };
    let router = codex_router(&binary);
    let first = router.complete(&req(1)).await.expect("turn 1");
    assert_eq!(first.trace["contract"]["resumed"], false);
    let second = router.complete(&req(2)).await.expect("turn 2");
    assert_eq!(second.trace["contract"]["resumed"], true);
    let args = second.trace["args"].as_array().expect("args");
    assert!(args.iter().any(|a| a == "resume"));
    assert!(args.iter().any(|a| a == "t-run1"));
}

#[tokio::test]
async fn claude_second_turn_resumes_persisted_session() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-claude");
    let jsonl = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-run1\"}\n\
{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s-run1\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}";
    write_fake_claude(&binary, jsonl);
    let session_dir = temp.path().join("run");
    let req = || ProviderRequest {
        prompt: "hi".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        session_dir: Some(session_dir.clone()),
        ..Default::default()
    };
    let router = claude_router(&binary);
    router.complete(&req()).await.expect("turn 1");
    let second = router.complete(&req()).await.expect("turn 2");
    assert_eq!(second.trace["contract"]["resumed"], true);
    let args = second.trace["args"].as_array().expect("args");
    assert!(args.iter().any(|a| a == "--resume"));
    assert!(args.iter().any(|a| a == "s-run1"));
}

#[tokio::test]
async fn distinct_runs_never_share_a_conversation() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex(&binary, CODEX_TURN_JSONL, "{\"action\":\"done\"}");
    let router = codex_router(&binary);
    let mk = |run: &str| ProviderRequest {
        prompt: "hi".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        output_path: Some(temp.path().join(format!("{run}/codex.out"))),
        session_dir: Some(temp.path().join(run)),
        ..Default::default()
    };
    let a = router.complete(&mk("run-a")).await.expect("run a");
    let b = router.complete(&mk("run-b")).await.expect("run b");
    // Each run's first turn is fresh — a distinct run never resumes another's.
    assert_eq!(a.trace["contract"]["resumed"], false);
    assert_eq!(b.trace["contract"]["resumed"], false);
}

#[tokio::test]
async fn semantic_judge_has_no_worker_session_or_write_capability() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex(&binary, CODEX_TURN_JSONL, "{\"decision\":\"achieved\"}");
    let session_dir = temp.path().join("worker-run");
    let router = codex_router(&binary);

    router
        .complete(&ProviderRequest {
            prompt: "worker turn".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            session_dir: Some(session_dir.clone()),
            ..Default::default()
        })
        .await
        .expect("worker turn");

    let judgment = router
        .complete(&ProviderRequest {
            prompt: "judge this evidence".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            session_dir: Some(session_dir),
            workspace_access: WorkspaceAccess::ReadOnly,
            ..Default::default()
        })
        .await
        .expect("read-only judgment");

    let args = judgment.trace["args"].as_array().expect("args");
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--sandbox" && pair[1] == "read-only")
    );
    assert!(!args.iter().any(|arg| arg == "resume"));
    assert_eq!(judgment.trace["contract"]["resumed"], false);
    assert_eq!(judgment.trace["workspace_access"], "read-only");
}

#[tokio::test]
async fn read_only_codex_judge_without_session_uses_schema_and_clean_answer() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    let answer = serde_json::json!({
        "decision": "achieved",
        "summary": "the requested behavior is present",
        "goal_coverage": [{
            "claim": "greeting is exact",
            "status": "met",
            "evidence": ["source-diff", "deterministic-gate"]
        }],
        "missing": []
    })
    .to_string();
    let jsonl = format!(
        "{}\n{}\n{}",
        r#"{"type":"thread.started","thread_id":"judge-1"}"#,
        serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "agent_message",
                "text": answer
            }
        }),
        r#"{"type":"turn.completed","usage":{"input_tokens":12,"cached_input_tokens":0,"output_tokens":8}}"#
    );
    write_fake_codex(&binary, &jsonl, "unused last-message");
    let judge_workspace = temp.path().join("judge-workspace");
    fs::create_dir_all(&judge_workspace).expect("judge workspace");
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["decision", "summary", "goal_coverage", "missing"],
        "properties": {
            "decision": {"type": "string"},
            "summary": {"type": "string"},
            "goal_coverage": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["claim", "status", "evidence"],
                    "properties": {
                        "claim": {"type": "string"},
                        "status": {"type": "string"},
                        "evidence": {"type": "array", "items": {"type": "string"}}
                    }
                }
            },
            "missing": {"type": "array", "items": {"type": "string"}}
        }
    });

    let judgment = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "judge this evidence".to_string(),
            cwd: Some(judge_workspace.clone()),
            output_path: None,
            session_dir: None,
            output_schema: Some(schema),
            workspace_access: WorkspaceAccess::ReadOnly,
            ..Default::default()
        })
        .await
        .expect("read-only judgment");

    let parsed: serde_json::Value =
        serde_json::from_str(&judgment.content).expect("clean semantic JSON");
    assert_eq!(parsed["decision"], "achieved");
    assert!(!judgment.content.contains("thread.started"));
    let args = judgment.trace["args"].as_array().expect("args");
    assert!(args.iter().any(|arg| arg == "--output-schema"));
    assert!(!args.iter().any(|arg| arg == "-o"));
    assert!(!args.iter().any(|arg| arg == "resume"));
    assert!(
        fs::read_dir(&judge_workspace)
            .expect("judge workspace")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with("provider-output-schema-"))
    );
}

#[allow(clippy::expect_used)]
fn write_fake_codex_vanishing(path: &std::path::Path) {
    let script = format!(
        "#!/bin/sh\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"--help\" ]; then\ncat <<'HELP'\n{help}\nHELP\n  exit 0; fi\n\
done\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"resume\" ]; then echo 'error: session not found' >&2; exit 1; fi\n\
done\n\
prev=\"\"; out=\"\"\n\
for a in \"$@\"; do\n\
  if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi\n\
  prev=\"$a\"\n\
done\n\
cat <<'JSONL'\n{jsonl}\nJSONL\n\
if [ -n \"$out\" ]; then printf '%s' '{{\"action\":\"done\"}}' > \"$out\"; fi\n\
exit 0\n",
        help = FAKE_CODEX_HELP,
        jsonl = CODEX_TURN_JSONL,
    );
    fs::write(path, script).expect("write vanishing codex");
    chmod_exec(path);
}

#[tokio::test]
async fn vanished_conversation_retries_fresh_once_with_reset_trace() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex_vanishing(&binary);
    let session_dir = temp.path().join("run");
    let req = |turn: u32| ProviderRequest {
        prompt: "hi".to_string(),
        cwd: Some(temp.path().to_path_buf()),
        output_path: Some(temp.path().join(format!("turns/turn-{turn}/codex.out"))),
        session_dir: Some(session_dir.clone()),
        ..Default::default()
    };
    let router = codex_router(&binary);
    // Turn 1 writes a session (t-run1).
    router.complete(&req(1)).await.expect("turn 1");
    // Turn 2 tries to resume t-run1; the fake reports it vanished; the driver
    // retries once fresh and records a reset caveat.
    let second = router.complete(&req(2)).await.expect("turn 2 recovers");
    assert_eq!(second.trace["contract"]["reset"], true);
    let caveats = second.trace["caveats"].as_array().expect("caveats");
    assert!(
        caveats
            .iter()
            .any(|c| c["code"] == "provider.session.reset")
    );
    // The final (fresh) attempt did not carry a resume verb.
    let args = second.trace["args"].as_array().expect("args");
    assert!(!args.iter().any(|a| a == "resume"));
}

// ---------------------------------------------------------------------------
// P9 — schema-constrained output (codex --output-schema).
// ---------------------------------------------------------------------------

// A capability-capable codex whose help LACKS --output-schema (has --json).
const FAKE_CODEX_HELP_NO_SCHEMA: &str = "\
Run Codex non-interactively
Commands:
  resume  Resume a previous session by id
Options:
      --json
  -o, --output-last-message <FILE>
";

#[allow(clippy::expect_used)]
fn write_fake_codex_no_schema(path: &std::path::Path, jsonl: &str) {
    let script = format!(
        "#!/bin/sh\n\
for a in \"$@\"; do\n\
  if [ \"$a\" = \"--help\" ]; then\ncat <<'HELP'\n{help}\nHELP\n  exit 0; fi\n\
done\n\
cat <<'JSONL'\n{jsonl}\nJSONL\n\
exit 0\n",
        help = FAKE_CODEX_HELP_NO_SCHEMA,
        jsonl = jsonl,
    );
    fs::write(path, script).expect("write fake codex no-schema");
    chmod_exec(path);
}

#[tokio::test]
async fn output_schema_passes_schema_file_to_codex() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex(&binary, CODEX_TURN_JSONL, "{\"action\":\"done\"}");
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action"],
        "properties": {"action": {"type": "string"}}
    });
    let response = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            output_path: Some(temp.path().join("turns/turn-1/codex.out")),
            session_dir: None,
            output_schema: Some(schema),
            workspace_access: WorkspaceAccess::ReadOnly,
            sandbox_backend: None,
            ..Default::default()
        })
        .await
        .expect("completion");
    let args = response.trace["args"].as_array().expect("args");
    assert!(args.iter().any(|a| a == "--output-schema"));
    assert!(fs::read_dir(temp.path()).expect("tempdir").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("provider-output-schema-")
    }));
}

#[tokio::test]
async fn schema_incapable_provider_fails_closed() {
    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("fake-codex");
    write_fake_codex_no_schema(&binary, CODEX_TURN_JSONL);
    let error = codex_router(&binary)
        .complete(&ProviderRequest {
            prompt: "hi".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            output_path: Some(temp.path().join("turns/turn-1/codex.out")),
            session_dir: Some(temp.path().join("run")),
            output_schema: Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": [],
                "properties": {}
            })),
            workspace_access: WorkspaceAccess::ReadOnly,
            sandbox_backend: None,
            ..Default::default()
        })
        .await
        .expect_err("schema-only posture must fail closed");
    assert!(error.to_string().contains("schema-only"), "{error}");
}
