use std::collections::{BTreeMap, VecDeque};

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::{ProviderError, Result};

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
    reader: R,
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
            reader,
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

    pub(crate) async fn next_notification<F>(&mut self, handler: &mut F) -> Result<RpcNotification>
    where
        F: FnMut(&str, &Value) -> Result<Value>,
    {
        if let Some(notification) = self.notifications.pop_front() {
            return Ok(notification);
        }
        loop {
            match self.read_incoming().await? {
                Incoming::Notification(notification) => return Ok(notification),
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
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|source| ProviderError::Io {
                path: format!("{} app-server stdout", self.provider),
                source,
            })?;
        if read == 0 {
            return Err(ProviderError::Cli {
                provider: self.provider.clone(),
                detail: "app-server closed its JSON-RPC stream".to_string(),
            });
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::RpcClient;

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
}
