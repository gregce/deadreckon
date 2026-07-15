use std::fs;

use deadreckon_providers::{
    ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRequest, ProviderRouter,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
        pid_file: Some(pid_file.clone()),
        cancellation_token: Some(token.clone()),
        session_dir: None,
        output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
    let response = router
        .complete(&ProviderRequest {
            prompt: "make notes".to_string(),
            max_output_tokens: 128,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
            pid_file: Some(pid_file.clone()),
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
                pid_file: None,
                cancellation_token: None,
                session_dir: None,
                output_schema: None,
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
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
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
      --json
  -o, --output-last-message <FILE>
";

#[allow(clippy::expect_used)]
fn write_fake_codex(path: &std::path::Path, jsonl: &str, answer: &str) {
    let script = format!(
        "#!/bin/sh\n\
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
