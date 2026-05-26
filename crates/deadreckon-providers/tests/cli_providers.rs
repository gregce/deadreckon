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
