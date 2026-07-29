use std::collections::{BTreeMap, VecDeque};
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use which::which;

use deadreckon_core::NetworkCapability;
use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, build_command,
};

use crate::cli_codex::CliCodexProvider;
use crate::cli_contract::ProviderSession;
use crate::cli_contract::add_caveat;
use crate::types::CapabilityPosture;
use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate,
};

const KNOWN_NOTIFICATIONS: &[&str] = &[
    "turn/started",
    "turn/completed",
    "item/started",
    "item/completed",
    "item/agentMessage/delta",
    "thread/tokenUsage/updated",
];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcNotification {
    pub(crate) method: String,
    pub(crate) params: Value,
}

#[derive(Debug)]
enum Incoming {
    Response {
        id: Value,
        result: Value,
    },
    Error {
        id: Value,
        code: i64,
        message: String,
        data: Option<Value>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification(RpcNotification),
}

#[derive(Debug)]
enum PendingReply {
    Response(Value),
    Error {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

/// Minimal newline-delimited JSON-RPC client for the Codex app-server wire.
/// Codex intentionally omits the `jsonrpc` field. Responses may arrive out of
/// order, and server requests can arrive while a client request is pending.
pub(crate) struct RpcClient<R, W> {
    provider: String,
    reader: Lines<R>,
    writer: W,
    next_id: u64,
    pending: BTreeMap<String, PendingReply>,
    notifications: VecDeque<RpcNotification>,
    unknown_notifications: Vec<String>,
}

impl<R, W> RpcClient<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub(crate) fn new(provider: impl Into<String>, reader: R, writer: W) -> Self {
        Self {
            provider: provider.into(),
            reader: reader.lines(),
            writer,
            next_id: 1,
            pending: BTreeMap::new(),
            notifications: VecDeque::new(),
            unknown_notifications: Vec::new(),
        }
    }

    pub(crate) fn unknown_notifications(&self) -> &[String] {
        &self.unknown_notifications
    }

    pub(crate) async fn request<F>(
        &mut self,
        method: &str,
        params: Value,
        handler: &mut F,
    ) -> Result<Value>
    where
        F: FnMut(&str, &Value) -> Result<Value>,
    {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_value(&json!({"id": id, "method": method, "params": params}))
            .await?;
        let key = id.to_string();
        loop {
            if let Some(reply) = self.pending.remove(&key) {
                return self.resolve_reply(method, reply);
            }
            let incoming = self.read_incoming().await?;
            match incoming {
                Incoming::Response { id, result } => {
                    let response_key = id_key(&id);
                    if response_key == key {
                        return Ok(result);
                    }
                    self.pending
                        .insert(response_key, PendingReply::Response(result));
                }
                Incoming::Error {
                    id,
                    code,
                    message,
                    data,
                } => {
                    let response_key = id_key(&id);
                    let reply = PendingReply::Error {
                        code,
                        message,
                        data,
                    };
                    if response_key == key {
                        return self.resolve_reply(method, reply);
                    }
                    self.pending.insert(response_key, reply);
                }
                Incoming::Request { id, method, params } => {
                    self.answer_server_request(id, &method, &params, handler)
                        .await?;
                }
                Incoming::Notification(notification) => {
                    self.notifications.push_back(notification);
                }
            }
        }
    }

    pub(crate) async fn notification(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut message = json!({"method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_value(&message).await
    }

    async fn next_notification_with_timeout<F>(
        &mut self,
        timeout: Duration,
        handler: &mut F,
    ) -> Result<Option<RpcNotification>>
    where
        F: FnMut(&str, &Value) -> Result<Value>,
    {
        if let Some(notification) = self.notifications.pop_front() {
            return Ok(Some(notification));
        }
        loop {
            let incoming = match tokio::time::timeout(timeout, self.read_incoming()).await {
                Ok(incoming) => incoming?,
                Err(_) => return Ok(None),
            };
            match incoming {
                Incoming::Notification(notification) => return Ok(Some(notification)),
                Incoming::Request { id, method, params } => {
                    self.answer_server_request(id, &method, &params, handler)
                        .await?;
                }
                Incoming::Response { id, result } => {
                    self.pending
                        .insert(id_key(&id), PendingReply::Response(result));
                }
                Incoming::Error {
                    id,
                    code,
                    message,
                    data,
                } => {
                    self.pending.insert(
                        id_key(&id),
                        PendingReply::Error {
                            code,
                            message,
                            data,
                        },
                    );
                }
            }
        }
    }

    async fn answer_server_request<F>(
        &mut self,
        id: Value,
        method: &str,
        params: &Value,
        handler: &mut F,
    ) -> Result<()>
    where
        F: FnMut(&str, &Value) -> Result<Value>,
    {
        match handler(method, params) {
            Ok(result) => self.write_value(&json!({"id": id, "result": result})).await,
            Err(err) => {
                self.write_value(&json!({
                    "id": id,
                    "error": {"code": -32603, "message": err.to_string()}
                }))
                .await
            }
        }
    }

    async fn read_incoming(&mut self) -> Result<Incoming> {
        let line = self
            .reader
            .next_line()
            .await
            .map_err(|source| ProviderError::Io {
                path: format!("{} app-server stdout", self.provider),
                source,
            })?
            .ok_or_else(|| ProviderError::Cli {
                provider: self.provider.clone(),
                detail: "app-server closed its JSON-RPC stream".to_string(),
            })?;
        let value: Value = serde_json::from_str(&line).map_err(|source| ProviderError::Cli {
            provider: self.provider.clone(),
            detail: format!("invalid app-server JSON-RPC line: {source}"),
        })?;
        let object = value.as_object().ok_or_else(|| ProviderError::Cli {
            provider: self.provider.clone(),
            detail: "app-server JSON-RPC message was not an object".to_string(),
        })?;
        let id = object.get("id").cloned();
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(id), Some(method)) = (id.clone(), method.clone()) {
            return Ok(Incoming::Request {
                id,
                method,
                params: object.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        if let (Some(id), Some(result)) = (id.clone(), object.get("result").cloned()) {
            return Ok(Incoming::Response { id, result });
        }
        if let (Some(id), Some(error)) = (id, object.get("error")) {
            return Ok(Incoming::Error {
                id,
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("app-server request failed")
                    .to_string(),
                data: error.get("data").cloned(),
            });
        }
        if let Some(method) = method {
            if !KNOWN_NOTIFICATIONS.contains(&method.as_str()) {
                self.unknown_notifications.push(method.clone());
            }
            return Ok(Incoming::Notification(RpcNotification {
                method,
                params: object.get("params").cloned().unwrap_or(Value::Null),
            }));
        }
        Err(ProviderError::Cli {
            provider: self.provider.clone(),
            detail: "unrecognized app-server JSON-RPC message".to_string(),
        })
    }

    async fn write_value(&mut self, value: &Value) -> Result<()> {
        let mut payload = serde_json::to_vec(value).map_err(|source| ProviderError::Cli {
            provider: self.provider.clone(),
            detail: format!("could not encode app-server request: {source}"),
        })?;
        payload.push(b'\n');
        self.writer
            .write_all(&payload)
            .await
            .map_err(|source| ProviderError::Io {
                path: format!("{} app-server stdin", self.provider),
                source,
            })?;
        self.writer
            .flush()
            .await
            .map_err(|source| ProviderError::Io {
                path: format!("{} app-server stdin", self.provider),
                source,
            })
    }

    fn resolve_reply(&self, method: &str, reply: PendingReply) -> Result<Value> {
        match reply {
            PendingReply::Response(value) => Ok(value),
            PendingReply::Error {
                code,
                message,
                data,
            } => Err(ProviderError::Cli {
                provider: self.provider.clone(),
                detail: format!(
                    "app-server {method} failed ({code}): {message}{}",
                    data.map(|value| format!("; {value}")).unwrap_or_default()
                ),
            }),
        }
    }
}

fn id_key(id: &Value) -> String {
    match id {
        Value::String(value) => value.clone(),
        _ => id.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApprovalRequest {
    CommandExecution { command: Option<String> },
    FileChange { paths: Vec<PathBuf> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionOutcome {
    Allow,
    Deny,
}

impl DecisionOutcome {
    fn trace_label(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    fn wire_label(self) -> &'static str {
        match self {
            Self::Allow => "accept",
            Self::Deny => "decline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Decision {
    outcome: DecisionOutcome,
    reason: &'static str,
    capability: &'static str,
}

impl Decision {
    fn allow(reason: &'static str, capability: &'static str) -> Self {
        Self {
            outcome: DecisionOutcome::Allow,
            reason,
            capability,
        }
    }

    fn deny(reason: &'static str, capability: &'static str) -> Self {
        Self {
            outcome: DecisionOutcome::Deny,
            reason,
            capability,
        }
    }
}

/// Deterministically answer one app-server approval from the run's existing
/// capability posture. This function deliberately has no I/O: callers are
/// responsible for building the posture and recording the returned decision.
fn answer_approval(posture: &CapabilityPosture, request: &ApprovalRequest) -> Decision {
    if posture.workspace_access == WorkspaceAccess::ReadOnly {
        return match request {
            ApprovalRequest::CommandExecution { .. } => {
                Decision::deny("read-only provider cannot approve commands", "read-only")
            }
            ApprovalRequest::FileChange { .. } => Decision::deny(
                "read-only provider cannot approve file changes",
                "read-only",
            ),
        };
    }
    match request {
        ApprovalRequest::CommandExecution { command: None } => {
            Decision::deny("command could not be determined", "commandExecution")
        }
        ApprovalRequest::CommandExecution {
            command: Some(command),
        } if install_intent(command) => {
            if posture.install {
                Decision::allow("global install allowed by run capabilities", "install")
            } else {
                Decision::deny("global install denied by run capabilities", "install")
            }
        }
        ApprovalRequest::CommandExecution {
            command: Some(command),
        } if network_intent(command) => match posture.network {
            NetworkCapability::Deny => {
                Decision::deny("network denied by run capabilities", "network")
            }
            NetworkCapability::Full => {
                Decision::allow("network allowed by run capabilities", "network")
            }
            NetworkCapability::Allowlist => {
                let Some(host) = extract_network_host(command) else {
                    return Decision::deny("network host could not be determined", "network");
                };
                if posture
                    .network_allowlist
                    .iter()
                    .any(|allowed| host_matches(allowed, &host))
                {
                    Decision::allow("network host is in run allowlist", "network")
                } else {
                    Decision::deny("network host is not in run allowlist", "network")
                }
            }
        },
        ApprovalRequest::CommandExecution { command: Some(_) } => Decision::allow(
            "command stays within the workspace-write sandbox",
            "workspace-write",
        ),
        ApprovalRequest::FileChange { paths } => {
            let mut roots = vec![normalize_path(&posture.working_dir)];
            roots.extend(posture.additional_write_roots.iter().map(|path| {
                if path.is_absolute() {
                    normalize_path(path)
                } else {
                    normalize_path(&posture.working_dir.join(path))
                }
            }));
            let allowed = !paths.is_empty()
                && paths.iter().all(|path| {
                    let path = if path.is_absolute() {
                        normalize_path(path)
                    } else {
                        normalize_path(&posture.working_dir.join(path))
                    };
                    roots.iter().any(|root| path.starts_with(root))
                });
            if allowed {
                Decision::allow("file change stays within writable roots", "filesystem")
            } else {
                Decision::deny("file change outside writable roots", "filesystem")
            }
        }
    }
}

fn install_intent(command: &str) -> bool {
    let tokens = command_tokens(command);
    tokens.windows(3).any(|window| {
        matches!(window[0].as_str(), "npm" | "pnpm" | "yarn")
            && matches!(window[1].as_str(), "i" | "install" | "add")
            && matches!(window[2].as_str(), "-g" | "--global")
    }) || tokens.windows(2).any(|window| {
        (matches!(window[0].as_str(), "brew" | "apt" | "apt-get") && window[1] == "install")
            || (matches!(window[0].as_str(), "npm" | "pnpm" | "yarn")
                && matches!(window[1].as_str(), "-g" | "--global"))
    })
}

fn network_intent(command: &str) -> bool {
    let tokens = command_tokens(command);
    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "curl" | "wget" | "npm" | "npx" | "pnpm" | "yarn" | "pip" | "pip3" | "cargo"
        )
    }) {
        return true;
    }
    tokens.windows(2).any(|window| {
        window[0] == "git"
            && matches!(
                window[1].as_str(),
                "clone" | "fetch" | "pull" | "push" | "remote" | "ls-remote"
            )
    })
}

fn command_tokens(command: &str) -> Vec<String> {
    command
        .split_ascii_whitespace()
        .map(|token| {
            let token = token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
            });
            Path::new(token)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(token)
                .to_ascii_lowercase()
        })
        .collect()
}

fn extract_network_host(command: &str) -> Option<String> {
    for raw in command.split_ascii_whitespace() {
        let token = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
            )
        });
        if let Some((_, rest)) = token.split_once("://") {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = host.split(':').next().unwrap_or(host);
            if !host.is_empty() {
                return Some(host.to_ascii_lowercase());
            }
        }
        if let Some((_, rest)) = token.split_once('@') {
            let host = rest.split([':', '/', '?', '#']).next().unwrap_or(rest);
            if host.contains('.') {
                return Some(host.to_ascii_lowercase());
            }
        }
        let candidate = token
            .trim_start_matches("--registry=")
            .split(['/', '?', '#', ':'])
            .next()
            .unwrap_or(token);
        if candidate.contains('.')
            && candidate
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        {
            return Some(candidate.to_ascii_lowercase());
        }
    }
    None
}

fn host_matches(allowed: &str, host: &str) -> bool {
    let allowed = allowed.trim().to_ascii_lowercase();
    allowed == "*"
        || allowed == host
        || allowed
            .strip_prefix("*.")
            .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

struct ApprovalSession {
    posture: CapabilityPosture,
    traces: Vec<Value>,
}

impl ApprovalSession {
    fn new(posture: CapabilityPosture) -> Self {
        Self {
            posture,
            traces: Vec::new(),
        }
    }

    fn handle(&mut self, method: &str, params: &Value) -> Result<Value> {
        let request = match method {
            "item/commandExecution/requestApproval" => ApprovalRequest::CommandExecution {
                command: params
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            "item/fileChange/requestApproval" => {
                let paths = params
                    .get("grantRoot")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .into_iter()
                    .collect::<Vec<_>>();
                ApprovalRequest::FileChange {
                    paths: if paths.is_empty() {
                        vec![self.posture.working_dir.clone()]
                    } else {
                        paths
                    },
                }
            }
            _ => {
                return Err(ProviderError::Cli {
                    provider: "cli:codex-server".to_string(),
                    detail: format!("unsupported app-server request: {method}"),
                });
            }
        };
        let decision = answer_approval(&self.posture, &request);
        self.traces
            .push(approval_trace(&request, &decision, &self.posture));
        Ok(json!({"decision": decision.outcome.wire_label()}))
    }
}

fn approval_trace(
    request: &ApprovalRequest,
    decision: &Decision,
    posture: &CapabilityPosture,
) -> Value {
    let (kind, subject_key, subject) = match request {
        ApprovalRequest::CommandExecution { command } => (
            "commandExecution",
            "command",
            command.clone().unwrap_or_else(|| "<unknown>".to_string()),
        ),
        ApprovalRequest::FileChange { paths } => (
            "fileChange",
            "path",
            paths
                .first()
                .unwrap_or(&posture.working_dir)
                .display()
                .to_string(),
        ),
    };
    let mut trace = json!({
        "kind": kind,
        "decision": decision.outcome.trace_label(),
        "reason": decision.reason,
        "capability": decision.capability,
    });
    trace[subject_key] = Value::String(subject);
    trace
}

fn default_capability_posture(cwd: &Path) -> CapabilityPosture {
    CapabilityPosture {
        network: NetworkCapability::Deny,
        network_allowlist: Vec::new(),
        deploy: false,
        install: false,
        working_dir: cwd.to_path_buf(),
        additional_write_roots: Vec::new(),
        workspace_access: WorkspaceAccess::ReadWrite,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAppServerSpec {
    pub(crate) provider: String,
    pub(crate) binary: String,
    pub(crate) extra_args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) pid_file: Option<PathBuf>,
    pub(crate) sandbox_backend: Option<SandboxBackend>,
    pub(crate) workspace_access: WorkspaceAccess,
}

type ChildRpcClient = RpcClient<BufReader<ChildStdout>, ChildStdin>;

pub(crate) struct CodexAppServer {
    child: Child,
    rpc: ChildRpcClient,
    cwd: PathBuf,
    thread_id: Option<String>,
    pid_file: Option<PathBuf>,
}

struct AppServerLaunch {
    program: PathBuf,
    args: Vec<std::ffi::OsString>,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ServerTurn {
    pub(crate) turn_id: String,
    pub(crate) content: String,
    pub(crate) usage: ProviderUsage,
    pub(crate) unknown_notifications: Vec<String>,
    pub(crate) approvals: Vec<Value>,
}

impl CodexAppServer {
    async fn spawn(spec: CodexAppServerSpec) -> Result<Self> {
        let launch = app_server_launch_command(&spec)?;
        let mut command = Command::new(launch.program);
        command
            .args(launch.args)
            .envs(launch.env)
            .current_dir(launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|source| ProviderError::Cli {
            provider: spec.provider.clone(),
            detail: format!("could not start codex app-server: {source}"),
        })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| ProviderError::Cli {
            provider: spec.provider.clone(),
            detail: "codex app-server stdin was unavailable".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Cli {
            provider: spec.provider.clone(),
            detail: "codex app-server stdout was unavailable".to_string(),
        })?;
        if let (Some(pid), Some(pid_file)) = (pid, spec.pid_file.as_ref()) {
            if let Some(parent) = pid_file.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ProviderError::Io {
                        path: parent.display().to_string(),
                        source,
                    })?;
            }
            tokio::fs::write(pid_file, format!("{pid}\n"))
                .await
                .map_err(|source| ProviderError::Io {
                    path: pid_file.display().to_string(),
                    source,
                })?;
        }
        let mut server = Self {
            child,
            rpc: RpcClient::new(spec.provider, BufReader::new(stdout), stdin),
            cwd: spec.cwd,
            thread_id: None,
            pid_file: spec.pid_file,
        };
        server.initialize().await?;
        Ok(server)
    }

    async fn initialize(&mut self) -> Result<()> {
        self.rpc
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "deadreckon",
                        "title": "DeadReckon",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"experimentalApi": false}
                }),
                &mut |method, _| {
                    Err(ProviderError::Cli {
                        provider: "cli:codex-server".to_string(),
                        detail: format!("unexpected server request during initialize: {method}"),
                    })
                },
            )
            .await?;
        self.rpc.notification("initialized", None).await
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub(crate) async fn ensure_thread(
        &mut self,
        session_dir: Option<&Path>,
        model: Option<&str>,
        workspace_access: WorkspaceAccess,
    ) -> Result<String> {
        const ROUTE: &str = "cli:codex-server";
        if workspace_access == WorkspaceAccess::ReadWrite
            && let Some(thread_id) = self.thread_id.as_ref()
        {
            return Ok(thread_id.clone());
        }
        let prior = (workspace_access == WorkspaceAccess::ReadWrite)
            .then(|| session_dir.and_then(|dir| ProviderSession::read(dir, ROUTE)))
            .flatten();
        let sandbox = app_server_sandbox_mode(workspace_access);
        let (method, mut params) = match prior.as_ref() {
            Some(session) => (
                "thread/resume",
                json!({
                    "threadId": session.conversation_id,
                    "cwd": self.cwd.display().to_string(),
                    "approvalPolicy": "on-request",
                    "approvalsReviewer": "user",
                    "sandbox": sandbox
                }),
            ),
            None => (
                "thread/start",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "approvalPolicy": "on-request",
                    "approvalsReviewer": "user",
                    "sandbox": sandbox
                }),
            ),
        };
        if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
            params["model"] = Value::String(model.to_string());
        }
        let result = self
            .rpc
            .request(method, params, &mut |request_method, _| {
                Err(ProviderError::Cli {
                    provider: ROUTE.to_string(),
                    detail: format!("unexpected server request before a turn: {request_method}"),
                })
            })
            .await?;
        let thread_id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ProviderError::Cli {
                provider: ROUTE.to_string(),
                detail: format!("app-server {method} response omitted thread.id"),
            })?
            .to_string();
        let now = chrono::Utc::now();
        let mut session = prior
            .filter(|session| session.conversation_id == thread_id)
            .unwrap_or_else(|| ProviderSession::new(ROUTE, &thread_id, now));
        session.touch(now);
        session.route = Some(ROUTE.to_string());
        session.server_pid = self.pid();
        if workspace_access == WorkspaceAccess::ReadWrite
            && let Some(session_dir) = session_dir
        {
            session.write(session_dir)?;
        }
        if workspace_access == WorkspaceAccess::ReadWrite {
            self.thread_id = Some(thread_id.clone());
        }
        Ok(thread_id)
    }

    pub(crate) async fn run_turn(
        &mut self,
        session_dir: Option<&Path>,
        thread_id: &str,
        prompt: &str,
        output_schema: Option<&Value>,
        capability_posture: CapabilityPosture,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ServerTurn> {
        const ROUTE: &str = "cli:codex-server";
        let mut approval_session = ApprovalSession::new(capability_posture);
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt, "textElements": []}]
        });
        if let Some(schema) = output_schema {
            params["outputSchema"] = schema.clone();
        }
        let result = self
            .rpc
            .request("turn/start", params, &mut |method, params| {
                approval_session.handle(method, params)
            })
            .await?;
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ProviderError::Cli {
                provider: ROUTE.to_string(),
                detail: "app-server turn/start response omitted turn.id".to_string(),
            })?
            .to_string();
        update_active_turn(session_dir, Some(&turn_id))?;

        let outcome = if let Some(token) = cancellation_token {
            tokio::select! {
                biased;
                () = token.cancelled() => {
                    let interrupted = self
                        .interrupt_turn(thread_id, &turn_id, &mut approval_session)
                        .await;
                    if interrupted.is_err() {
                        self.kill_child().await;
                    }
                    Err(ProviderError::Cli {
                        provider: ROUTE.to_string(),
                        detail: "request cancelled after app-server interrupt".to_string(),
                    })
                }
                result = self.read_turn(
                    session_dir,
                    thread_id,
                    &turn_id,
                    &mut approval_session,
                ) => result,
            }
        } else {
            self.read_turn(session_dir, thread_id, &turn_id, &mut approval_session)
                .await
        };
        let cleared = update_active_turn(session_dir, None);
        match (outcome, cleared) {
            (Ok(mut turn), Ok(())) => {
                turn.approvals = approval_session.traces;
                Ok(turn)
            }
            (Err(err), _) => Err(err),
            (Ok(_), Err(err)) => Err(err),
        }
    }

    async fn interrupt_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        approval_session: &mut ApprovalSession,
    ) -> Result<()> {
        const ROUTE: &str = "cli:codex-server";
        tokio::time::timeout(Duration::from_secs(5), async {
            self.rpc
                .request(
                    "turn/interrupt",
                    json!({"threadId": thread_id, "turnId": turn_id}),
                    &mut |method, params| approval_session.handle(method, params),
                )
                .await?;
            loop {
                let Some(notification) = self
                    .rpc
                    .next_notification_with_timeout(
                        Duration::from_millis(100),
                        &mut |method, params| approval_session.handle(method, params),
                    )
                    .await?
                else {
                    continue;
                };
                if notification.method == "turn/completed"
                    && notification
                        .params
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        == Some(turn_id)
                {
                    return Ok(());
                }
            }
        })
        .await
        .map_err(|_| ProviderError::Cli {
            provider: ROUTE.to_string(),
            detail: "app-server turn/interrupt grace period expired".to_string(),
        })?
    }

    async fn kill_child(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await;
        if let Some(pid_file) = self.pid_file.as_ref() {
            let _ = tokio::fs::remove_file(pid_file).await;
        }
    }

    async fn read_turn(
        &mut self,
        session_dir: Option<&Path>,
        thread_id: &str,
        turn_id: &str,
        approval_session: &mut ApprovalSession,
    ) -> Result<ServerTurn> {
        const ROUTE: &str = "cli:codex-server";
        let mut usage = ProviderUsage {
            input_tokens: 0,
            output_tokens: 0,
        };
        let mut content = None;
        self.deliver_pending_steers(session_dir, thread_id, turn_id, approval_session)
            .await?;
        loop {
            let Some(notification) = self
                .rpc
                .next_notification_with_timeout(
                    Duration::from_millis(100),
                    &mut |method, params| approval_session.handle(method, params),
                )
                .await?
            else {
                self.deliver_pending_steers(session_dir, thread_id, turn_id, approval_session)
                    .await?;
                continue;
            };
            match notification.method.as_str() {
                "thread/tokenUsage/updated" => {
                    if notification
                        .params
                        .pointer("/threadId")
                        .and_then(Value::as_str)
                        == Some(thread_id)
                    {
                        usage.input_tokens = notification
                            .params
                            .pointer("/tokenUsage/last/inputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(usage.input_tokens);
                        usage.output_tokens = notification
                            .params
                            .pointer("/tokenUsage/last/outputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(usage.output_tokens);
                    }
                }
                "item/completed" => {
                    let item = notification.params.pointer("/item");
                    if notification
                        .params
                        .pointer("/turnId")
                        .and_then(Value::as_str)
                        == Some(turn_id)
                        && item
                            .and_then(|item| item.get("type"))
                            .and_then(Value::as_str)
                            == Some("agentMessage")
                        && let Some(text) = item
                            .and_then(|item| item.get("text"))
                            .and_then(Value::as_str)
                    {
                        content = Some(text.to_string());
                    }
                }
                "turn/completed" => {
                    let turn = notification.params.pointer("/turn");
                    if turn.and_then(|turn| turn.get("id")).and_then(Value::as_str) != Some(turn_id)
                    {
                        continue;
                    }
                    let status = turn
                        .and_then(|turn| turn.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("failed");
                    if status != "completed" {
                        let detail = turn
                            .and_then(|turn| turn.pointer("/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("app-server turn did not complete");
                        return Err(ProviderError::Cli {
                            provider: ROUTE.to_string(),
                            detail: format!("app-server turn {status}: {detail}"),
                        });
                    }
                    let content =
                        content
                            .filter(|text| !text.trim().is_empty())
                            .ok_or_else(|| ProviderError::Cli {
                                provider: ROUTE.to_string(),
                                detail: "app-server completed without a final agent message"
                                    .to_string(),
                            })?;
                    return Ok(ServerTurn {
                        turn_id: turn_id.to_string(),
                        content,
                        usage,
                        unknown_notifications: self.rpc.unknown_notifications().to_vec(),
                        approvals: Vec::new(),
                    });
                }
                _ => {}
            }
        }
    }

    async fn deliver_pending_steers(
        &mut self,
        session_dir: Option<&Path>,
        thread_id: &str,
        turn_id: &str,
        approval_session: &mut ApprovalSession,
    ) -> Result<()> {
        let Some(run_root) = session_dir else {
            return Ok(());
        };
        let pending = deadreckon_core::steer_inbox::pending_steers(run_root)
            .map_err(|error| steer_inbox_error(&error))?;
        for entry in pending {
            let result = self
                .rpc
                .request(
                    "turn/steer",
                    json!({
                        "threadId": thread_id,
                        "input": [{
                            "type": "text",
                            "text": entry.text,
                            "textElements": []
                        }],
                        "expectedTurnId": turn_id
                    }),
                    &mut |method, params| approval_session.handle(method, params),
                )
                .await;
            let response = match result {
                Ok(response) => response,
                Err(error) if stale_steer_precondition(&error) => break,
                Err(error) => return Err(error),
            };
            let delivered_turn_id = response
                .pointer("/turnId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| ProviderError::Cli {
                    provider: "cli:codex-server".to_string(),
                    detail: "app-server turn/steer response omitted turnId".to_string(),
                })?;
            if delivered_turn_id != turn_id {
                return Err(ProviderError::Cli {
                    provider: "cli:codex-server".to_string(),
                    detail: format!(
                        "app-server turn/steer returned turn {delivered_turn_id}, expected {turn_id}"
                    ),
                });
            }
            deadreckon_core::steer_inbox::mark_steer_delivered(
                run_root,
                &entry.identity(),
                delivered_turn_id,
            )
            .map_err(|error| steer_inbox_error(&error))?;
        }
        Ok(())
    }
}

fn app_server_launch_command(spec: &CodexAppServerSpec) -> Result<AppServerLaunch> {
    let mut args = vec![std::ffi::OsString::from("app-server")];
    args.extend(spec.extra_args.iter().map(std::ffi::OsString::from));
    let Some(backend) = spec.sandbox_backend else {
        return Ok(AppServerLaunch {
            program: PathBuf::from(&spec.binary),
            args,
            env: BTreeMap::new(),
            cwd: spec.cwd.clone(),
        });
    };

    let program = which(&spec.binary).unwrap_or_else(|_| PathBuf::from(&spec.binary));
    let mut read_allowlist = vec![program.clone()];
    let mut write_allowlist = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        read_allowlist.push(home.join(".codex"));
        write_allowlist.push(home.join(".codex"));
    }
    let policy = ToolSandboxPolicy::cli_provider(spec.cwd.clone(), read_allowlist, write_allowlist);
    let mut env = BTreeMap::new();
    if let Some(path) = std::env::var_os("PATH") {
        env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }
    if let Some(home) = std::env::var_os("HOME") {
        env.insert("HOME".to_string(), home.to_string_lossy().into_owned());
    }
    let command = build_command(&SandboxSpec {
        backend,
        cwd: spec.cwd.clone(),
        program: program.into_os_string(),
        args,
        stdin: None,
        env,
        allow_network: policy.allow_network,
        pid_file: None,
        cancellation_token: None,
        profile_dir: spec
            .pid_file
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|root| root.join("sandbox").join("app-server")),
        read_allowlist: policy.read_allowlist,
        write_allowlist: policy.write_allowlist,
        read_denylist: Vec::new(),
        write_denylist: Vec::new(),
        network_allowlist: policy.network_allowlist,
        workspace_access: spec.workspace_access,
    })
    .map_err(|source| ProviderError::Cli {
        provider: spec.provider.clone(),
        detail: format!("could not protect codex app-server: {source}"),
    })?;
    Ok(AppServerLaunch {
        program: PathBuf::from(command.program),
        args: command.args,
        env: command.env,
        cwd: command.cwd,
    })
}

fn app_server_sandbox_mode(workspace_access: WorkspaceAccess) -> &'static str {
    match workspace_access {
        WorkspaceAccess::ReadWrite => "workspace-write",
        WorkspaceAccess::ReadOnly => "read-only",
    }
}

fn steer_inbox_error(error: &deadreckon_core::DeadreckonError) -> ProviderError {
    ProviderError::Cli {
        provider: "cli:codex-server".to_string(),
        detail: format!("steer inbox error: {error}"),
    }
}

fn stale_steer_precondition(error: &ProviderError) -> bool {
    let ProviderError::Cli { detail, .. } = error else {
        return false;
    };
    detail.contains("app-server turn/steer failed (-32600)")
        && (detail.contains("no active turn to steer")
            || detail.contains("expected active turn id"))
}

fn update_active_turn(session_dir: Option<&Path>, turn_id: Option<&str>) -> Result<()> {
    const ROUTE: &str = "cli:codex-server";
    let Some(session_dir) = session_dir else {
        return Ok(());
    };
    let Some(mut session) = ProviderSession::read(session_dir, ROUTE) else {
        return Ok(());
    };
    session.active_turn_id = turn_id.map(str::to_string);
    session.touch(chrono::Utc::now());
    session.write(session_dir)
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(pid_file) = self.pid_file.as_ref() {
            let _ = std::fs::remove_file(pid_file);
        }
    }
}

pub(crate) enum ServerStart {
    Ready(Box<CodexAppServer>),
    Degraded { trace: Value },
}

pub(crate) async fn start_server_or_degrade(spec: CodexAppServerSpec) -> ServerStart {
    match CodexAppServer::spawn(spec).await {
        Ok(server) => ServerStart::Ready(Box::new(server)),
        Err(err) => {
            let mut trace = json!({"route": "cli:codex-server"});
            add_caveat(
                &mut trace,
                "provider.route.degraded",
                &format!("codex app-server initialize failed; using cli:codex exec: {err}"),
            );
            ServerStart::Degraded { trace }
        }
    }
}

pub(crate) struct CliCodexServerProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
    model_arg: Option<String>,
    fallback: CliCodexProvider,
    server: Mutex<Option<Box<CodexAppServer>>>,
    degraded: AtomicBool,
}

impl CliCodexServerProvider {
    pub(crate) fn new(name: impl Into<String>, entry: ProviderEntry) -> Self {
        let name = name.into();
        let binary = entry.binary.clone().unwrap_or_else(|| "codex".to_string());
        let (model, model_arg) = server_model(entry.model.clone());
        let mut fallback_entry = entry.clone();
        fallback_entry.model = model_arg.clone();
        fallback_entry.extra_args.clear();
        Self {
            name,
            binary,
            extra_args: entry.extra_args,
            model,
            model_arg,
            fallback: CliCodexProvider::new("cli:codex", fallback_entry),
            server: Mutex::new(None),
            degraded: AtomicBool::new(false),
        }
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let started = Instant::now();
        let cwd = request
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        if self.degraded.load(Ordering::Acquire) {
            return self
                .run_fallback(
                    request,
                    json!({
                        "route": self.name,
                        "reason": "app-server route was degraded earlier in this run"
                    }),
                )
                .await;
        }
        let mut server_slot = self.server.lock().await;
        if server_slot.is_none() {
            match start_server_or_degrade(CodexAppServerSpec {
                provider: self.name.clone(),
                binary: self.binary.clone(),
                extra_args: self.extra_args.clone(),
                cwd: cwd.clone(),
                pid_file: request.pid_file.clone(),
                sandbox_backend: request.sandbox_backend,
                workspace_access: request.workspace_access,
            })
            .await
            {
                ServerStart::Ready(server) => *server_slot = Some(server),
                ServerStart::Degraded { trace } => {
                    self.degraded.store(true, Ordering::Release);
                    drop(server_slot);
                    return self.run_fallback(request, trace).await;
                }
            }
        }
        let server_result = async {
            let server = server_slot.as_mut().ok_or_else(|| ProviderError::Cli {
                provider: self.name.clone(),
                detail: "app-server was not retained after startup".to_string(),
            })?;
            let session_dir = (request.workspace_access == WorkspaceAccess::ReadWrite)
                .then_some(request.session_dir.as_deref())
                .flatten();
            let thread_id = server
                .ensure_thread(
                    session_dir,
                    self.model_arg.as_deref(),
                    request.workspace_access,
                )
                .await?;
            let pid = server.pid();
            let mut capability_posture = request
                .capability_posture
                .clone()
                .unwrap_or_else(|| default_capability_posture(&cwd));
            capability_posture.workspace_access = request.workspace_access;
            let turn = server
                .run_turn(
                    session_dir,
                    &thread_id,
                    &request.prompt,
                    request.output_schema.as_ref(),
                    capability_posture,
                    request.cancellation_token.as_ref(),
                )
                .await?;
            Ok::<_, ProviderError>((thread_id, pid, turn))
        }
        .await;
        let (thread_id, pid, turn) = match server_result {
            Ok(result) => result,
            Err(error)
                if request
                    .cancellation_token
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled) =>
            {
                return Err(error);
            }
            Err(error) => {
                self.degraded.store(true, Ordering::Release);
                server_slot.take();
                drop(server_slot);
                return self
                    .run_fallback(
                        request,
                        json!({
                            "route": self.name,
                            "reason": "app-server request failed structurally",
                            "error": error.to_string(),
                        }),
                    )
                    .await;
            }
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        if let Some(path) = request.output_path.as_ref() {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ProviderError::Io {
                        path: parent.display().to_string(),
                        source,
                    })?;
            }
            tokio::fs::write(path, &turn.content)
                .await
                .map_err(|source| ProviderError::Io {
                    path: path.display().to_string(),
                    source,
                })?;
        }
        let spend = self
            .estimate_spend(turn.usage.clone())
            .with_wall_time(started.elapsed().as_secs_f64());
        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content: turn.content,
            usage: turn.usage,
            spend,
            trace: json!({
                "kind": "cli_subagent",
                "route": self.name,
                "transport": "app-server-stdio-jsonl",
                "binary": self.binary,
                "pid": pid,
                "workspace_access": request.workspace_access.as_str(),
                "thread_id": thread_id,
                "turn_id": turn.turn_id,
                "duration_ms": duration_ms,
                "stdout_path": request.output_path,
                "unknown_notifications": turn.unknown_notifications,
                "approvals": turn.approvals,
                "flight_rows": [],
            }),
        })
    }

    async fn run_fallback(
        &self,
        request: &ProviderRequest,
        degraded_trace: Value,
    ) -> Result<ProviderResponse> {
        let mut response = self.fallback.run(request).await?;
        response.provider = self.name.clone();
        response.trace["requested_route"] = Value::String(self.name.clone());
        response.trace["degraded_from"] = degraded_trace;
        let pending_steers = request
            .session_dir
            .as_deref()
            .and_then(|run_root| deadreckon_core::steer_inbox::pending_steers(run_root).ok())
            .map_or(0, |pending| pending.len());
        response.trace["pending_steers"] = json!(pending_steers);
        let pending_notice = if pending_steers == 0 {
            String::new()
        } else {
            format!("; {pending_steers} pending steers remain undelivered")
        };
        add_caveat(
            &mut response.trace,
            "provider.route.degraded",
            &format!("codex app-server was unavailable; used cli:codex exec{pending_notice}"),
        );
        Ok(response)
    }
}

impl Provider for CliCodexServerProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Generic("cli:codex-server".to_string())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn has_credential(&self) -> bool {
        which(&self.binary).is_ok() || PathBuf::from(&self.binary).exists()
    }

    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate {
        SpendEstimate {
            provider: self.name.clone(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: 0.0,
            subscription: true,
            wall_time_seconds: None,
        }
    }

    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a> {
        Box::pin(async move { self.run(request).await })
    }
}

fn server_model(model: Option<String>) -> (String, Option<String>) {
    match model {
        Some(model)
            if model.trim().is_empty()
                || model == "provider default"
                || model == "cli:codex-server" =>
        {
            ("provider default".to_string(), None)
        }
        Some(model) => (model.clone(), Some(model)),
        None => ("provider default".to_string(), None),
    }
}

trait WithWallTime {
    fn with_wall_time(self, seconds: f64) -> Self;
}

impl WithWallTime for SpendEstimate {
    fn with_wall_time(mut self, seconds: f64) -> Self {
        self.wall_time_seconds = Some(seconds);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::json;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::sync::CancellationToken;

    use super::{
        ApprovalRequest, CapabilityPosture, CodexAppServerSpec, DecisionOutcome, RpcClient,
        ServerStart, WorkspaceAccess, answer_approval, app_server_launch_command,
        app_server_sandbox_mode, default_capability_posture, start_server_or_degrade,
    };
    use crate::cli_contract::ProviderSession;
    use crate::{
        ProviderConfigFile, ProviderEntry, ProviderRequest, ProviderRouter, ProviderUsage,
    };
    use deadreckon_core::{DeadreckonPaths, NetworkCapability};
    use deadreckon_sandbox::SandboxBackend;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rudder/fake-codex-app-server.sh")
    }

    fn approval_posture(network: NetworkCapability) -> CapabilityPosture {
        CapabilityPosture {
            network,
            network_allowlist: Vec::new(),
            deploy: false,
            install: false,
            working_dir: PathBuf::from("/workspace"),
            additional_write_roots: Vec::new(),
            workspace_access: WorkspaceAccess::ReadWrite,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_server_requested_backend_crosses_the_same_outer_boundary() {
        if which::which("sandbox-exec").is_err() {
            return;
        }
        let temp = TempDir::new().expect("tempdir");
        let launch = app_server_launch_command(&CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: None,
            sandbox_backend: Some(SandboxBackend::SandboxExec),
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .expect("launch");

        assert_eq!(launch.program, PathBuf::from("sandbox-exec"));
        let profile = launch
            .args
            .windows(2)
            .find(|parts| parts[0] == "-p")
            .map(|parts| parts[1].to_string_lossy().into_owned())
            .expect("seatbelt profile");
        let key_store = DeadreckonPaths::discover().home().join("gate-keys");
        assert!(profile.contains(&format!(
            "(deny file-read* (literal \"{}\"))",
            key_store.display()
        )));
    }

    #[test]
    fn network_deny_capability_denies_curl_command() {
        let request = ApprovalRequest::CommandExecution {
            command: Some("curl https://api.example.com/v1".to_string()),
        };

        let decision = answer_approval(&approval_posture(NetworkCapability::Deny), &request);

        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert_eq!(decision.reason, "network denied by run capabilities");
        assert_eq!(decision.capability, "network");
    }

    #[test]
    fn allowlisted_host_command_is_approved() {
        let mut posture = approval_posture(NetworkCapability::Allowlist);
        posture.network_allowlist = vec!["api.example.com".to_string()];
        let request = ApprovalRequest::CommandExecution {
            command: Some("wget https://api.example.com/v1".to_string()),
        };

        let decision = answer_approval(&posture, &request);

        assert_eq!(decision.outcome, DecisionOutcome::Allow);
        assert_eq!(decision.capability, "network");
    }

    #[test]
    fn file_change_outside_workspace_is_denied() {
        let request = ApprovalRequest::FileChange {
            paths: vec![PathBuf::from("/outside/secrets.txt")],
        };

        let decision = answer_approval(&approval_posture(NetworkCapability::Deny), &request);

        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert_eq!(decision.reason, "file change outside writable roots");
        assert_eq!(decision.capability, "filesystem");
    }

    #[test]
    fn read_only_server_uses_read_only_sandbox_and_denies_approvals() {
        assert_eq!(
            app_server_sandbox_mode(WorkspaceAccess::ReadOnly),
            "read-only"
        );
        let mut posture = approval_posture(NetworkCapability::Full);
        posture.workspace_access = WorkspaceAccess::ReadOnly;

        let command = answer_approval(
            &posture,
            &ApprovalRequest::CommandExecution {
                command: Some("git status".to_string()),
            },
        );
        let change = answer_approval(
            &posture,
            &ApprovalRequest::FileChange {
                paths: vec![PathBuf::from("/workspace/result.rs")],
            },
        );

        assert_eq!(command.outcome, DecisionOutcome::Deny);
        assert_eq!(change.outcome, DecisionOutcome::Deny);
        assert_eq!(command.capability, "read-only");
        assert_eq!(change.capability, "read-only");
    }

    #[tokio::test]
    async fn rpc_client_correlates_responses_by_id() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut client = RpcClient::new("test", BufReader::new(client_read), client_write);

        server_write
            .write_all(b"{\"id\":2,\"result\":{\"value\":\"second\"}}\n{\"id\":1,\"result\":{\"value\":\"first\"}}\n")
            .await
            .expect("responses");
        let first = client
            .request("first", json!({}), &mut |_, _| Ok(json!({})))
            .await
            .expect("first response");
        let second = client
            .request("second", json!({}), &mut |_, _| Ok(json!({})))
            .await
            .expect("second response");

        assert_eq!(first["value"], "first");
        assert_eq!(second["value"], "second");
        drop(server_read);
    }

    #[tokio::test]
    async fn rpc_client_routes_server_requests_to_handler() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut client = RpcClient::new("test", BufReader::new(client_read), client_write);

        server_write
            .write_all(b"{\"id\":\"approval-1\",\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"command\":\"pwd\"}}\n{\"id\":1,\"result\":{\"ok\":true}}\n")
            .await
            .expect("messages");
        let mut handled = false;
        client
            .request("turn/start", json!({}), &mut |method, params| {
                handled = true;
                assert_eq!(method, "item/commandExecution/requestApproval");
                assert_eq!(params["command"], "pwd");
                Ok(json!({"decision": "accept"}))
            })
            .await
            .expect("turn response");

        assert!(handled);
        let mut lines = BufReader::new(server_read).lines();
        let _request = lines.next_line().await.expect("request").expect("line");
        let approval = lines.next_line().await.expect("approval").expect("line");
        let approval: serde_json::Value = serde_json::from_str(&approval).expect("json");
        assert_eq!(approval["id"], "approval-1");
        assert_eq!(approval["result"]["decision"], "accept");
    }

    #[tokio::test]
    async fn unknown_notification_is_recorded_not_fatal() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (_server_read, mut server_write) = tokio::io::split(server_io);
        let mut client = RpcClient::new("test", BufReader::new(client_read), client_write);

        server_write
            .write_all(b"{\"method\":\"future/notification\",\"params\":{\"value\":1}}\n{\"id\":1,\"result\":{\"ok\":true}}\n")
            .await
            .expect("messages");
        let response = client
            .request("initialize", json!({}), &mut |_, _| Ok(json!({})))
            .await
            .expect("response");

        assert_eq!(response["ok"], true);
        assert_eq!(client.unknown_notifications(), ["future/notification"]);
    }

    #[tokio::test]
    async fn server_child_pid_is_supervised_and_killed_on_drop() {
        let temp = TempDir::new().expect("tempdir");
        let pid_file = temp.path().join("child-pids/codex-app-server.pid");
        let started = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(pid_file.clone()),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await;
        let ServerStart::Ready(server) = started else {
            panic!("server should start")
        };
        let pid = server.pid().expect("pid");
        assert_eq!(
            std::fs::read_to_string(&pid_file).expect("pid file").trim(),
            pid.to_string()
        );
        assert!(deadreckon_core::pid_is_alive(pid));

        drop(server);
        for _ in 0..40 {
            if !deadreckon_core::pid_is_alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!deadreckon_core::pid_is_alive(pid));
        assert!(!pid_file.exists());
    }

    #[tokio::test]
    async fn handshake_failure_degrades_route_with_trace() {
        let temp = TempDir::new().expect("tempdir");
        let started = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["handshake-failure".to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await;
        let ServerStart::Degraded { trace } = started else {
            panic!("handshake failure should degrade")
        };
        assert_eq!(trace["caveats"][0]["code"], "provider.route.degraded");
        assert!(
            trace["caveats"][0]["message"]
                .as_str()
                .expect("message")
                .contains("initialize")
        );
    }

    #[tokio::test]
    async fn server_route_persists_thread_and_resumes_it() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        let spec = || CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        };

        let ServerStart::Ready(mut first) = start_server_or_degrade(spec()).await else {
            panic!("first server")
        };
        let first_id = first
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("start thread");
        assert_eq!(first_id, "thread-fixture");
        drop(first);

        let session = ProviderSession::read(temp.path(), "cli:codex-server").expect("session");
        assert_eq!(session.route.as_deref(), Some("cli:codex-server"));
        assert_eq!(session.conversation_id, "thread-fixture");
        assert!(session.server_pid.is_some());

        let ServerStart::Ready(mut second) = start_server_or_degrade(spec()).await else {
            panic!("second server")
        };
        let resumed_id = second
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("resume thread");
        assert_eq!(resumed_id, "thread-fixture");
        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert!(calls.contains("thread/start"));
        assert!(calls.contains("thread/resume"));
    }

    #[tokio::test]
    async fn read_only_server_request_never_resumes_worker_thread() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };

        server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("worker thread");
        server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadOnly)
            .await
            .expect("judge thread");

        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert_eq!(calls.matches("thread/start").count(), 2);
        assert!(!calls.contains("thread/resume"));
    }

    #[tokio::test]
    async fn server_turn_completes_with_real_usage() {
        let temp = TempDir::new().expect("tempdir");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");
        let turn = server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "finish the task",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect("turn");

        assert_eq!(turn.turn_id, "turn-fixture");
        assert_eq!(turn.content, "fixture answer");
        assert_eq!(turn.usage.input_tokens, 321);
        assert_eq!(turn.usage.output_tokens, 45);
        let session = ProviderSession::read(temp.path(), "cli:codex-server").expect("session");
        assert_eq!(session.active_turn_id, None);
    }

    #[tokio::test]
    async fn turn_failed_surfaces_provider_error() {
        let temp = TempDir::new().expect("tempdir");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["turn-failed".to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");

        let error = server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "fail this turn",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect_err("failed turn");

        assert!(error.to_string().contains("fixture turn failed"));
    }

    #[tokio::test]
    async fn pending_steer_delivers_with_expected_turn_id() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        deadreckon_core::steer_inbox::append_steer(
            temp.path(),
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "prefer the smaller patch",
        )
        .expect("append steer");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");

        server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "finish the task",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect("turn");

        let calls = std::fs::read_to_string(log).expect("rpc log");
        let steer = calls
            .lines()
            .find(|line| line.contains("\"method\":\"turn/steer\""))
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("steer json"))
            .expect("turn/steer request");
        assert_eq!(steer["params"]["threadId"], "thread-fixture");
        assert_eq!(steer["params"]["expectedTurnId"], "turn-fixture");
        assert_eq!(
            steer["params"]["input"][0]["text"],
            "prefer the smaller patch"
        );
        let entries = deadreckon_core::steer_inbox::read_steer_inbox(temp.path()).expect("inbox");
        assert_eq!(
            entries[0].status,
            deadreckon_core::steer_inbox::SteerStatus::Delivered
        );
        assert_eq!(
            entries[0].delivered_turn_id.as_deref(),
            Some("turn-fixture")
        );
    }

    #[tokio::test]
    async fn stale_turn_precondition_retries_not_drops() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        deadreckon_core::steer_inbox::append_steer(
            temp.path(),
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "keep this instruction pending",
        )
        .expect("append steer");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["stale-steer-once".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");

        server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "first turn",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect("first turn");
        assert_eq!(
            deadreckon_core::steer_inbox::pending_steers(temp.path())
                .expect("pending after stale")
                .len(),
            1
        );

        server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "second turn",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect("second turn");
        assert!(
            deadreckon_core::steer_inbox::pending_steers(temp.path())
                .expect("pending after retry")
                .is_empty()
        );
        let entries = deadreckon_core::steer_inbox::read_steer_inbox(temp.path()).expect("inbox");
        assert_eq!(
            entries[0].delivered_turn_id.as_deref(),
            Some("turn-stale-2")
        );
        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert_eq!(calls.matches("\"method\":\"turn/steer\"").count(), 2);
    }

    #[tokio::test]
    async fn duplicate_delivery_is_harmless() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        deadreckon_core::steer_inbox::append_steer(
            temp.path(),
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "send this once",
        )
        .expect("append steer");
        let inbox = temp.path().join("steer-inbox.jsonl");
        let row = std::fs::read_to_string(&inbox).expect("inbox row");
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&inbox)
            .expect("open inbox")
            .write_all(row.as_bytes())
            .expect("duplicate physical row");

        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["normal".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");

        server
            .run_turn(
                Some(temp.path()),
                &thread_id,
                "finish",
                None,
                default_capability_posture(temp.path()),
                None,
            )
            .await
            .expect("turn");

        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert_eq!(calls.matches("\"method\":\"turn/steer\"").count(), 1);
        assert!(
            deadreckon_core::steer_inbox::pending_steers(temp.path())
                .expect("pending")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn steer_appended_mid_turn_is_polled_and_delivered() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["wait-for-steer".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");
        let run_root = temp.path().to_path_buf();

        let (turn, appended) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                server.run_turn(
                    Some(&run_root),
                    &thread_id,
                    "keep working",
                    None,
                    default_capability_posture(&run_root),
                    None,
                ),
                async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    deadreckon_core::steer_inbox::append_steer(
                        &run_root,
                        deadreckon_core::steer_inbox::SteerSource::Cli,
                        "stop refactoring and ship",
                    )
                }
            )
        })
        .await
        .expect("mid-turn steer timeout");

        appended.expect("append steer");
        turn.expect("turn");
        assert!(
            deadreckon_core::steer_inbox::pending_steers(temp.path())
                .expect("pending")
                .is_empty()
        );
        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert!(calls.contains("stop refactoring and ship"), "{calls}");
    }

    #[tokio::test]
    async fn explicit_server_provider_route_round_trips() {
        let temp = TempDir::new().expect("tempdir");
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("cli:codex-server".to_string()),
                fallback: None,
                providers: BTreeMap::from([(
                    "cli:codex-server".to_string(),
                    ProviderEntry {
                        kind: None,
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: None,
                        input_cost_per_million: None,
                        output_cost_per_million: None,
                        binary: Some(fixture().display().to_string()),
                        extra_args: vec!["normal".to_string()],
                    },
                )]),
            },
            None,
        )
        .expect("router");

        let response = router
            .complete(&ProviderRequest {
                prompt: "finish the task".to_string(),
                cwd: Some(temp.path().to_path_buf()),
                output_path: Some(temp.path().join("turns/turn-1/codex-server.out")),
                pid_file: Some(temp.path().join("child-pids/provider-turn-1.pid")),
                session_dir: Some(temp.path().to_path_buf()),
                ..ProviderRequest::default()
            })
            .await
            .expect("response");

        assert_eq!(response.provider, "cli:codex-server");
        assert_eq!(response.content, "fixture answer");
        assert_eq!(
            response.usage,
            ProviderUsage {
                input_tokens: 321,
                output_tokens: 45
            }
        );
        assert_eq!(response.trace["transport"], "app-server-stdio-jsonl");
    }

    #[tokio::test]
    async fn server_approval_is_answered_and_returned_for_trace() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("cli:codex-server".to_string()),
                fallback: None,
                providers: BTreeMap::from([(
                    "cli:codex-server".to_string(),
                    ProviderEntry {
                        kind: None,
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: None,
                        input_cost_per_million: None,
                        output_cost_per_million: None,
                        binary: Some(fixture().display().to_string()),
                        extra_args: vec!["approval-command".to_string(), log.display().to_string()],
                    },
                )]),
            },
            None,
        )
        .expect("router");
        let mut posture = approval_posture(NetworkCapability::Deny);
        posture.working_dir = temp.path().to_path_buf();

        let response = router
            .complete(&ProviderRequest {
                prompt: "try the network".to_string(),
                cwd: Some(temp.path().to_path_buf()),
                session_dir: Some(temp.path().to_path_buf()),
                capability_posture: Some(posture),
                ..ProviderRequest::default()
            })
            .await
            .expect("response");

        assert_eq!(response.trace["approvals"][0]["decision"], "deny");
        assert_eq!(
            response.trace["approvals"][0]["reason"],
            "network denied by run capabilities"
        );
        let reply = std::fs::read_to_string(log).expect("approval reply");
        assert!(reply.contains("\"decision\":\"decline\""), "{reply}");
    }

    #[tokio::test]
    async fn kill_sends_interrupt_before_process_kill() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        let ServerStart::Ready(mut server) = start_server_or_degrade(CodexAppServerSpec {
            provider: "cli:codex-server".to_string(),
            binary: fixture().display().to_string(),
            extra_args: vec!["wait-for-interrupt".to_string(), log.display().to_string()],
            cwd: temp.path().to_path_buf(),
            pid_file: Some(temp.path().join("child-pids/codex-app-server.pid")),
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
        })
        .await
        else {
            panic!("server")
        };
        let thread_id = server
            .ensure_thread(Some(temp.path()), None, WorkspaceAccess::ReadWrite)
            .await
            .expect("thread");
        let pid = server.pid().expect("server pid");
        let token = CancellationToken::new();
        let cancel = token.clone();

        let result = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                server.run_turn(
                    Some(temp.path()),
                    &thread_id,
                    "keep working",
                    None,
                    default_capability_posture(temp.path()),
                    Some(&token),
                ),
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    cancel.cancel();
                }
            )
        })
        .await
        .expect("cancellation timeout")
        .0;
        assert!(
            result
                .expect_err("cancelled turn")
                .to_string()
                .contains("cancelled")
        );

        let calls = std::fs::read_to_string(&log).expect("rpc log");
        assert!(calls.contains("turn/interrupt"), "{calls}");
        assert!(deadreckon_core::pid_is_alive(pid));

        drop(server);
        for _ in 0..40 {
            if !deadreckon_core::pid_is_alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!deadreckon_core::pid_is_alive(pid));
    }

    #[tokio::test]
    async fn server_death_degrades_to_exec_route_with_caveat() {
        let temp = TempDir::new().expect("tempdir");
        let log = temp.path().join("rpc.log");
        deadreckon_core::steer_inbox::append_steer(
            temp.path(),
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "keep this durable",
        )
        .expect("pending steer");
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("cli:codex-server".to_string()),
                fallback: None,
                providers: BTreeMap::from([(
                    "cli:codex-server".to_string(),
                    ProviderEntry {
                        kind: None,
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: None,
                        input_cost_per_million: None,
                        output_cost_per_million: None,
                        binary: Some(fixture().display().to_string()),
                        extra_args: vec!["die-mid-turn".to_string(), log.display().to_string()],
                    },
                )]),
            },
            None,
        )
        .expect("router");

        let response = router
            .complete(&ProviderRequest {
                prompt: "finish despite server death".to_string(),
                cwd: Some(temp.path().to_path_buf()),
                session_dir: Some(temp.path().to_path_buf()),
                ..ProviderRequest::default()
            })
            .await
            .expect("exec fallback response");

        assert!(response.content.contains("exec fallback completed"));
        assert_eq!(response.trace["pending_steers"], 1);
        assert!(response.trace["caveats"].as_array().is_some_and(|caveats| {
            caveats
                .iter()
                .any(|caveat| caveat["code"] == "provider.route.degraded")
        }));
        assert!(response.trace["caveats"].as_array().is_some_and(|caveats| {
            caveats.iter().any(|caveat| {
                caveat["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("1 pending steers remain undelivered"))
            })
        }));
        assert_eq!(
            deadreckon_core::steer_inbox::pending_steers(temp.path())
                .expect("pending after degrade")
                .len(),
            1
        );

        let second = router
            .complete(&ProviderRequest {
                prompt: "use the degraded route".to_string(),
                cwd: Some(temp.path().to_path_buf()),
                session_dir: Some(temp.path().to_path_buf()),
                ..ProviderRequest::default()
            })
            .await
            .expect("next turn stays on exec fallback");
        assert!(second.content.contains("exec fallback completed"));
        let calls = std::fs::read_to_string(log).expect("rpc log");
        assert_eq!(calls.matches("initialize").count(), 1, "{calls}");
    }

    #[test]
    fn semaphore_session_schema_still_readable() {
        let legacy = json!({
            "schema": 1,
            "provider": "cli:codex",
            "conversation_id": "thread-old",
            "created_at": "2026-07-11T18:00:00Z",
            "last_turn_at": "2026-07-11T18:04:12Z",
            "resume_failures": 0
        });
        let session: ProviderSession = serde_json::from_value(legacy).expect("legacy session");
        assert_eq!(session.conversation_id, "thread-old");
        assert_eq!(session.route, None);
        assert_eq!(session.server_pid, None);
        assert_eq!(session.active_turn_id, None);
    }
}
