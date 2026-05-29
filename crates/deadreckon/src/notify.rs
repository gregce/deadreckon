use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Utc};
use deadreckon_core::{DeadreckonPaths, PipelineState};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const NOTIFY_JSONL: &str = "notify.jsonl";
const NOTIFY_SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub type NotifyFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTransition {
    Accepted,
    Paused,
    Failed,
}

impl NotifyTransition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Paused => "paused",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "accepted" | "done" | "completed" => Some(Self::Accepted),
            "paused" | "paused-at-cap" | "paused_at_cap" => Some(Self::Paused),
            "failed" | "failure" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn all() -> BTreeSet<Self> {
        [Self::Accepted, Self::Paused, Self::Failed]
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyConfig {
    pub enabled: bool,
    pub on: BTreeSet<NotifyTransition>,
    pub native: bool,
    pub command: Option<String>,
    pub webhook: Option<String>,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            on: NotifyTransition::all(),
            native: true,
            command: None,
            webhook: None,
        }
    }
}

impl NotifyConfig {
    pub fn from_root(root: &toml::Value) -> Self {
        let Some(notify) = root.get("notify") else {
            return Self::default();
        };
        let mut config = Self::default();
        if let Some(enabled) = notify.get("enabled").and_then(toml::Value::as_bool) {
            config.enabled = enabled;
        }
        if let Some(native) = notify.get("native").and_then(toml::Value::as_bool) {
            config.native = native;
        }
        if let Some(on) = notify.get("on") {
            let transitions = notify_transitions(on);
            if !transitions.is_empty() {
                config.on = transitions;
            }
        }
        config.command = notify
            .get("command")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        config.webhook = notify
            .get("webhook")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        config
    }

    pub fn enabled_for(&self, transition: NotifyTransition) -> bool {
        self.enabled && self.on.contains(&transition)
    }
}

fn notify_transitions(value: &toml::Value) -> BTreeSet<NotifyTransition> {
    if let Some(single) = value.as_str() {
        return NotifyTransition::parse(single).into_iter().collect();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(NotifyTransition::parse)
        .collect()
}

pub fn load_notify_config(paths: &DeadreckonPaths) -> deadreckon_core::Result<NotifyConfig> {
    let root = match std::fs::read_to_string(paths.config_path()) {
        Ok(raw) => toml::from_str(&raw)
            .map_err(|err| deadreckon_core::DeadreckonError::InvalidInput(err.to_string()))?,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(Default::default())
        }
        Err(source) => {
            return Err(deadreckon_core::DeadreckonError::Io {
                path: paths.config_path(),
                source,
            });
        }
    };
    Ok(NotifyConfig::from_root(&root))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyContext {
    pub transition: NotifyTransition,
    pub run_id: String,
    pub verdict: String,
    pub spend: String,
    pub narrative_path: Option<PathBuf>,
}

impl NotifyContext {
    pub fn env_vars(&self) -> Vec<(&'static str, String)> {
        let mut vars = vec![
            (
                "DEADRECKON_NOTIFY_TRANSITION",
                self.transition.as_str().to_string(),
            ),
            ("DEADRECKON_NOTIFY_RUN_ID", self.run_id.clone()),
            ("DEADRECKON_NOTIFY_VERDICT", self.verdict.clone()),
            ("DEADRECKON_NOTIFY_SPEND", self.spend.clone()),
        ];
        if let Some(path) = self.narrative_path.as_ref() {
            vars.push(("DEADRECKON_NOTIFY_NARRATIVE", path.display().to_string()));
        }
        vars
    }

    pub fn webhook_body(&self) -> serde_json::Value {
        json!({
            "transition": self.transition.as_str(),
            "run_id": self.run_id,
            "verdict": self.verdict,
            "spend": self.spend,
            "narrative_path": self.narrative_path.as_ref().map(|path| path.display().to_string()),
        })
    }

    fn title(&self) -> String {
        format!("deadreckon {}", self.transition.as_str())
    }

    fn body(&self) -> String {
        format!("{} - {}", self.verdict, self.spend)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyAttempt {
    pub ts: DateTime<Utc>,
    pub transition: NotifyTransition,
    pub channel: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub trait NotifyChannel: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, context: &'a NotifyContext) -> NotifyFuture<'a>;
}

pub struct CommandNotifyChannel {
    command: String,
}

impl CommandNotifyChannel {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

impl NotifyChannel for CommandNotifyChannel {
    fn name(&self) -> &'static str {
        "command"
    }

    fn send<'a>(&'a self, context: &'a NotifyContext) -> NotifyFuture<'a> {
        Box::pin(async move {
            let command = self.command.clone();
            let env_vars = context.env_vars();
            tokio::task::spawn_blocking(move || {
                let mut child = Command::new("sh");
                child.arg("-c").arg(command);
                child.stdout(Stdio::null()).stderr(Stdio::null());
                child.env_clear();
                if let Some(path) = std::env::var_os("PATH") {
                    child.env("PATH", path);
                }
                for (key, value) in env_vars {
                    child.env(key, value);
                }
                let status = run_with_timeout(child, NOTIFY_SEND_TIMEOUT)?;
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("command exited with {status}"))
                }
            })
            .await
            .map_err(|err| err.to_string())?
        })
    }
}

pub struct NativeNotifyChannel;

impl NotifyChannel for NativeNotifyChannel {
    fn name(&self) -> &'static str {
        "native"
    }

    fn send<'a>(&'a self, context: &'a NotifyContext) -> NotifyFuture<'a> {
        Box::pin(async move {
            let title = context.title();
            let body = context.body();
            tokio::task::spawn_blocking(move || send_native(&title, &body))
                .await
                .map_err(|err| err.to_string())?
        })
    }
}

fn send_native(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title {}",
            osascript_string(body),
            osascript_string(title)
        );
        let mut command = Command::new("osascript");
        command.arg("-e").arg(script);
        let status = run_with_timeout(command, NOTIFY_SEND_TIMEOUT)?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("osascript exited with {status}"))
        };
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("notify-send");
        command.arg(title).arg(body);
        let status = run_with_timeout(command, NOTIFY_SEND_TIMEOUT)?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("notify-send exited with {status}"))
        };
    }
    #[allow(unreachable_code)]
    Err("native notifications unsupported on this platform".to_string())
}

fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let started = std::time::Instant::now();
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    loop {
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "notification command timed out after {:.0}s",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn osascript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub struct WebhookNotifyChannel {
    url: String,
    client: reqwest::Client,
}

impl WebhookNotifyChannel {
    pub fn new(url: impl Into<String>) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self {
            url: url.into(),
            client,
        })
    }
}

impl NotifyChannel for WebhookNotifyChannel {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn send<'a>(&'a self, context: &'a NotifyContext) -> NotifyFuture<'a> {
        Box::pin(async move {
            let response = self
                .client
                .post(&self.url)
                .json(&context.webhook_body())
                .send()
                .await
                .map_err(|err| err.to_string())?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!("webhook returned {}", response.status()))
            }
        })
    }
}

pub fn channels_for_config(config: &NotifyConfig) -> Vec<Box<dyn NotifyChannel>> {
    let mut channels: Vec<Box<dyn NotifyChannel>> = Vec::new();
    if config.native {
        channels.push(Box::new(NativeNotifyChannel));
    }
    if let Some(command) = config.command.as_ref() {
        channels.push(Box::new(CommandNotifyChannel::new(command.clone())));
    }
    if let Some(webhook) = config.webhook.as_ref()
        && let Ok(channel) = WebhookNotifyChannel::new(webhook.clone())
    {
        channels.push(Box::new(channel));
    }
    channels
}

pub async fn notify_run(
    state: &PipelineState,
    config: &NotifyConfig,
    context: &NotifyContext,
    channels: &[Box<dyn NotifyChannel>],
) -> Vec<NotifyAttempt> {
    if !config.enabled_for(context.transition) {
        return Vec::new();
    }
    let mut attempts = Vec::new();
    for channel in channels {
        let result = channel.send(context).await;
        let attempt = NotifyAttempt {
            ts: Utc::now(),
            transition: context.transition,
            channel: channel.name().to_string(),
            ok: result.is_ok(),
            detail: result.err(),
        };
        let _ = append_notify_attempt(state, &attempt);
        attempts.push(attempt);
    }
    attempts
}

pub fn append_notify_attempt(
    state: &PipelineState,
    attempt: &NotifyAttempt,
) -> deadreckon_core::Result<()> {
    deadreckon_core::state::append_json_line(&notify_jsonl_path(state), attempt)
}

pub fn notify_jsonl_path(state: &PipelineState) -> PathBuf {
    state.run_root.join(NOTIFY_JSONL)
}
