use std::fs;

use deadreckon_providers::{
    ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRequest, ProviderRouter,
};
use deadreckon_sandbox::SandboxBackend;
use tempfile::TempDir;

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
    if which::which("sandbox-exec").is_err() {
        return;
    }
    let temp = TempDir::new().expect("tempdir");
    let bin_dir = temp.path().join("user-bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    let binary = bin_dir.join("fake-codex");
    write_fake_binary(&binary, "path-resolved-codex-output");
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
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
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: Some(SandboxBackend::SandboxExec),
            pid_file: None,
            cancellation_token: None,
        })
        .await
        .expect("sandboxed completion");
    unsafe {
        std::env::set_var("PATH", old_path);
    }

    assert!(response.content.contains("path-resolved-codex-output"));
    assert!(response.content.contains("--sandbox danger-full-access"));
    assert_eq!(response.trace["sandbox_backend"], "sandbox-exec");
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

fn write_fake_binary(path: &std::path::Path, label: &str) {
    fs::write(
        path,
        format!("#!/bin/sh\nprintf '{label}\\nargs:%s\\n' \"$*\"\n"),
    )
    .expect("write fake binary");
    chmod_exec(path);
}

fn chmod_exec(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}
