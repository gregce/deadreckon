#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

use deadreckon::notify::{
    CommandNotifyChannel, NotifyAttempt, NotifyChannel, NotifyConfig, NotifyContext, NotifyFuture,
    NotifyTransition, notify_jsonl_path, notify_run,
};
use deadreckon_core::glossary::NOUN_VERIFIED_RUN;
use deadreckon_core::{DeadreckonPaths, RunOptions, create_run};
use tempfile::TempDir;

#[test]
fn notify_config_parses_and_defaults_off() {
    let empty = toml::Value::Table(Default::default());
    let config = NotifyConfig::from_root(&empty);

    assert!(!config.enabled);
    assert!(config.native);
    assert!(config.command.is_none());
    assert!(config.webhook.is_none());
    assert!(config.on.contains(&NotifyTransition::Accepted));
    assert!(config.on.contains(&NotifyTransition::Paused));
    assert!(config.on.contains(&NotifyTransition::Failed));

    let root: toml::Value = toml::from_str(
        r#"
        [notify]
        enabled = true
        on = ["accepted", "failed"]
        native = false
        command = "echo notify"
        webhook = "https://example.test/deadreckon"
        "#,
    )
    .expect("toml");
    let config = NotifyConfig::from_root(&root);

    assert!(config.enabled_for(NotifyTransition::Accepted));
    assert!(config.enabled_for(NotifyTransition::Failed));
    assert!(!config.enabled_for(NotifyTransition::Paused));
    assert!(!config.native);
    assert_eq!(config.command.as_deref(), Some("echo notify"));
    assert_eq!(
        config.webhook.as_deref(),
        Some("https://example.test/deadreckon")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_channel_receives_redacted_context() {
    let temp = TempDir::new().expect("tempdir");
    let out = temp.path().join("notify-env.txt");
    let command = format!(
        "env | sort | grep '^DEADRECKON_NOTIFY_' > {}",
        shell_quote(&out)
    );
    let context = context();
    let channel = CommandNotifyChannel::new(command);

    channel.send(&context).await.expect("command notify");

    let env = fs::read_to_string(&out).expect("env output");
    assert!(
        env.contains("DEADRECKON_NOTIFY_TRANSITION=accepted"),
        "{env}"
    );
    assert!(env.contains("DEADRECKON_NOTIFY_RUN_ID=run-abc123"), "{env}");
    assert!(
        env.contains(&format!("DEADRECKON_NOTIFY_VERDICT={NOUN_VERIFIED_RUN}")),
        "{env}"
    );
    assert!(
        env.contains("DEADRECKON_NOTIFY_SPEND=not metered (subscription)"),
        "{env}"
    );
    assert!(
        env.contains("DEADRECKON_NOTIFY_NARRATIVE=/tmp/RUN-NARRATIVE.md"),
        "{env}"
    );
    assert!(!env.contains("ship the secret feature"), "{env}");
    assert!(!env.contains("sk-test-secret"), "{env}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_records_attempt_to_jsonl() {
    let temp = TempDir::new().expect("tempdir");
    let state = state(&temp);
    let config = NotifyConfig {
        enabled: true,
        native: false,
        command: None,
        webhook: None,
        on: NotifyTransition::all(),
    };
    let channels: Vec<Box<dyn NotifyChannel>> = vec![Box::new(FakeChannel)];
    let attempts = notify_run(&state, &config, &context(), &channels).await;

    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].ok);
    assert_eq!(attempts[0].channel, "fake");
    let raw = fs::read_to_string(notify_jsonl_path(&state)).expect("notify jsonl");
    let records = raw
        .lines()
        .map(|line| serde_json::from_str::<NotifyAttempt>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].transition, NotifyTransition::Accepted);
    assert_eq!(records[0].channel, "fake");
    assert!(records[0].ok);

    let disabled = NotifyConfig::default();
    let attempts = notify_run(&state, &disabled, &context(), &channels).await;
    assert!(attempts.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_command_notification_records_evidence_not_try_footer() {
    let temp = TempDir::new().expect("tempdir");
    let state = state(&temp);
    let config = NotifyConfig {
        enabled: true,
        native: false,
        command: Some("exit 2".to_string()),
        webhook: None,
        on: NotifyTransition::all(),
    };
    let channels: Vec<Box<dyn NotifyChannel>> = vec![Box::new(CommandNotifyChannel::new("exit 2"))];

    let attempts = notify_run(&state, &config, &context(), &channels).await;

    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].ok, "{attempts:?}");
    let detail = attempts[0].detail.as_deref().expect("detail");
    assert!(
        detail.contains("configure notify.command with a working command and re-run"),
        "{detail}"
    );
    assert!(!detail.contains("try:"), "{detail}");
}

struct FakeChannel;

impl NotifyChannel for FakeChannel {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn send<'a>(&'a self, _context: &'a NotifyContext) -> NotifyFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn context() -> NotifyContext {
    NotifyContext {
        transition: NotifyTransition::Accepted,
        run_id: "run-abc123".to_string(),
        verdict: NOUN_VERIFIED_RUN.to_string(),
        spend: "not metered (subscription)".to_string(),
        narrative_path: Some("/tmp/RUN-NARRATIVE.md".into()),
    }
}

fn state(temp: &TempDir) -> deadreckon_core::PipelineState {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let state = create_run(
        &paths,
        RunOptions {
            goal: "ship the secret feature sk-test-secret".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
