use std::fs;

use deadreckon_providers::{
    ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRequest, ProviderRouter,
};
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
        })
        .await
        .expect("completion");

    assert!(response.content.contains("codex-output"));
    assert!(response.content.contains("args:exec make notes"));
    assert!(response.spend.subscription);
}

fn write_fake_binary(path: &std::path::Path, label: &str) {
    fs::write(
        path,
        format!("#!/bin/sh\nprintf '{label}\\nargs:%s\\n' \"$*\"\n"),
    )
    .expect("write fake binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}
