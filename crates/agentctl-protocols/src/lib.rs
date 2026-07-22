//! Stable MCP and A2A protocol clients for agentctl.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentctl_core::dsl::ActionKind;
use agentctl_runtime::{ExternalActionHandler, RuntimeError};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const A2A_PROTOCOL_VERSION: &str = "1.0";
const MAX_PROTOCOL_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("protocol request was cancelled")]
    Cancelled,
    #[error("protocol request timed out after {0:?}")]
    Timeout(Duration),
    #[error("protocol transport failed: {0}")]
    Transport(String),
    #[error("protocol returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("protocol response is malformed: {0}")]
    Malformed(String),
    #[error("protocol version `{found}` is unsupported; expected `{expected}`")]
    Version {
        found: String,
        expected: &'static str,
    },
    #[error("remote session expired")]
    SessionExpired,
    #[error("remote operation failed ({code}): {message}")]
    Remote { code: i64, message: String },
    #[error("remote operation is unsupported: {0}")]
    Unsupported(String),
    #[error("remote task `{0}` did not finish before the polling bound")]
    PollLimit(String),
}

#[derive(Debug, Clone)]
pub struct ProtocolHttpConfig {
    pub url: Url,
    pub headers: BTreeMap<String, String>,
    pub timeout: Duration,
}

fn client() -> Result<Client, ProtocolError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("agentctl/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| ProtocolError::Transport(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub annotations: Option<Value>,
}

pub struct McpClient {
    client: Client,
    config: ProtocolHttpConfig,
    session_id: Mutex<Option<String>>,
    initialized: AtomicBool,
    next_id: AtomicU64,
}

impl McpClient {
    pub fn new(config: ProtocolHttpConfig) -> Result<Self, ProtocolError> {
        Ok(Self {
            client: client()?,
            config,
            session_id: Mutex::new(None),
            initialized: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn initialize(&self, cancellation: &CancellationToken) -> Result<(), ProtocolError> {
        let result = self
            .rpc(
                "initialize",
                serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "agentctl", "version": env!("CARGO_PKG_VERSION")}
                }),
                false,
                cancellation,
            )
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProtocolError::Malformed("initialize result omits protocolVersion".to_owned())
            })?;
        if version != MCP_PROTOCOL_VERSION {
            return Err(ProtocolError::Version {
                found: version.to_owned(),
                expected: MCP_PROTOCOL_VERSION,
            });
        }
        self.notification(
            "notifications/initialized",
            serde_json::json!({}),
            cancellation,
        )
        .await?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn list_tools(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<McpTool>, ProtocolError> {
        let result = self
            .rpc("tools/list", serde_json::json!({}), true, cancellation)
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| ProtocolError::Malformed("tools/list result omits tools".to_owned()))?;
        tools
            .iter()
            .map(|tool| {
                Ok(McpTool {
                    name: string_field(tool, "name")?,
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    input_schema: tool.get("inputSchema").cloned().ok_or_else(|| {
                        ProtocolError::Malformed("MCP tool omits inputSchema".to_owned())
                    })?,
                    output_schema: tool.get("outputSchema").cloned(),
                    annotations: tool.get("annotations").cloned(),
                })
            })
            .collect()
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        if !self.initialized.load(Ordering::Acquire) {
            self.initialize(cancellation).await?;
        }
        let result = self
            .rpc(
                "tools/call",
                serde_json::json!({"name": name, "arguments": arguments}),
                true,
                cancellation,
            )
            .await?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(ProtocolError::Remote {
                code: -1,
                message: summarize_remote(&result),
            });
        }
        if let Some(structured) = result.get("structuredContent") {
            return Ok(structured.clone());
        }
        Ok(result.get("content").cloned().unwrap_or(Value::Null))
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        require_session: bool,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut request = self.request().json(&body);
        if require_session {
            let session = self
                .session_id
                .lock()
                .map_err(|_| ProtocolError::Transport("MCP session lock was poisoned".to_owned()))?
                .clone();
            request = request.header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
            if let Some(session) = session {
                request = request.header("Mcp-Session-Id", session);
            }
        }
        let response =
            execute_request(request, self.config.timeout, cancellation, Some((id, self))).await?;
        if method == "initialize"
            && let Some(session) = response
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|value| value.to_str().ok())
        {
            *self.session_id.lock().map_err(|_| {
                ProtocolError::Transport("MCP session lock was poisoned".to_owned())
            })? = Some(session.to_owned());
        }
        let value = response_value(response, self.config.timeout, cancellation).await?;
        json_rpc_result(value)
    }

    async fn notification(
        &self,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<(), ProtocolError> {
        let body = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut request = self
            .request()
            .json(&body)
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
        if let Some(session) = self
            .session_id
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP session lock was poisoned".to_owned()))?
            .clone()
        {
            request = request.header("Mcp-Session-Id", session);
        }
        let response = execute_request(request, self.config.timeout, cancellation, None).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(http_error(response).await)
        }
    }

    fn request(&self) -> reqwest::RequestBuilder {
        self.config.headers.iter().fold(
            self.client
                .post(self.config.url.clone())
                .header("Accept", "application/json, text/event-stream")
                .header("Origin", "agentctl://local"),
            |request, (name, value)| request.header(name, value),
        )
    }

    async fn cancel_request(&self, id: u64) {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": id}
        });
        let mut request = self
            .request()
            .json(&body)
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
        if let Ok(session) = self.session_id.lock()
            && let Some(session) = session.clone()
        {
            request = request.header("Mcp-Session-Id", session);
        }
        let _ = tokio::time::timeout(self.config.timeout, request.send()).await;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    pub url: String,
    pub protocol_binding: String,
    pub protocol_version: String,
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub supported_interfaces: Vec<AgentInterface>,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub skills: Vec<Value>,
    #[serde(default)]
    pub security_schemes: Value,
    #[serde(default)]
    pub security: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum A2aResponse {
    Message(Value),
    Task(Value),
}

pub struct A2aClient {
    client: Client,
    card_config: ProtocolHttpConfig,
    interface: Mutex<Option<AgentInterface>>,
    next_id: AtomicU64,
    max_polls: usize,
    poll_interval: Duration,
}

impl A2aClient {
    pub fn new(config: ProtocolHttpConfig) -> Result<Self, ProtocolError> {
        Ok(Self {
            client: client()?,
            card_config: config,
            interface: Mutex::new(None),
            next_id: AtomicU64::new(1),
            max_polls: 100,
            poll_interval: Duration::from_millis(100),
        })
    }

    #[must_use]
    pub fn with_poll_bounds(mut self, max_polls: usize, poll_interval: Duration) -> Self {
        self.max_polls = max_polls;
        self.poll_interval = poll_interval;
        self
    }

    pub async fn discover(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AgentCard, ProtocolError> {
        let request = self.card_config.headers.iter().fold(
            self.client.get(self.card_config.url.clone()),
            |request, (name, value)| request.header(name, value),
        );
        let response =
            execute_request(request, self.card_config.timeout, cancellation, None).await?;
        let card: AgentCard =
            response_json(response, self.card_config.timeout, cancellation).await?;
        if card.name.trim().is_empty() || card.supported_interfaces.is_empty() {
            return Err(ProtocolError::Malformed(
                "Agent Card requires a name and supportedInterfaces".to_owned(),
            ));
        }
        let interface = card
            .supported_interfaces
            .iter()
            .find(|interface| {
                interface.protocol_binding.eq_ignore_ascii_case("JSONRPC")
                    && interface.protocol_version == A2A_PROTOCOL_VERSION
            })
            .cloned()
            .ok_or_else(|| ProtocolError::Version {
                found: card
                    .supported_interfaces
                    .iter()
                    .map(|interface| interface.protocol_version.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                expected: A2A_PROTOCOL_VERSION,
            })?;
        let interface_url = Url::parse(&interface.url).map_err(|error| {
            ProtocolError::Malformed(format!("Agent Card interface URL: {error}"))
        })?;
        if !same_origin(&self.card_config.url, &interface_url) {
            return Err(ProtocolError::Unsupported(
                "Agent Card interface must share the configured card origin".to_owned(),
            ));
        }
        *self.interface.lock().map_err(|_| {
            ProtocolError::Transport("A2A interface lock was poisoned".to_owned())
        })? = Some(interface);
        Ok(card)
    }

    pub async fn send_message(
        &self,
        message_id: &str,
        text: &str,
        context_id: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<A2aResponse, ProtocolError> {
        let mut message = serde_json::json!({
            "messageId": message_id,
            "role": "user",
            "parts": [{"text": text}]
        });
        if let Some(context_id) = context_id {
            message["contextId"] = Value::String(context_id.to_owned());
        }
        let result = self
            .rpc(
                "SendMessage",
                serde_json::json!({"message": message}),
                cancellation,
            )
            .await?;
        if let Some(task) = result.get("task") {
            Ok(A2aResponse::Task(task.clone()))
        } else if let Some(message) = result.get("message") {
            Ok(A2aResponse::Message(message.clone()))
        } else if result.get("id").is_some() && result.get("status").is_some() {
            Ok(A2aResponse::Task(result))
        } else {
            Ok(A2aResponse::Message(result))
        }
    }

    pub async fn wait_for_task(
        &self,
        task: Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let task_id = string_field(&task, "id")?;
        let mut current = task;
        for _ in 0..self.max_polls {
            match task_state(&current) {
                Some("completed") => return Ok(current),
                Some("failed" | "rejected" | "canceled") => {
                    return Err(ProtocolError::Remote {
                        code: -1,
                        message: format!(
                            "A2A task `{task_id}` ended in {}",
                            task_state(&current).unwrap_or("unknown")
                        ),
                    });
                }
                Some("input_required" | "auth_required") => return Ok(current),
                _ => {}
            }
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                () = cancellation.cancelled() => {
                    let _ = self.cancel_task(&task_id, &CancellationToken::new()).await;
                    return Err(ProtocolError::Cancelled);
                }
            }
            current = self
                .rpc("GetTask", serde_json::json!({"id": task_id}), cancellation)
                .await?;
            if let Some(inner) = current.get("task") {
                current = inner.clone();
            }
        }
        Err(ProtocolError::PollLimit(task_id))
    }

    pub async fn cancel_task(
        &self,
        task_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        self.rpc(
            "CancelTask",
            serde_json::json!({"id": task_id}),
            cancellation,
        )
        .await
    }

    pub async fn send_streaming_message(
        &self,
        message_id: &str,
        text: &str,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ProtocolError> {
        let interface = self.selected_interface()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "SendStreamingMessage",
            "params": {"message": {"messageId": message_id, "role": "user", "parts": [{"text": text}]}}
        });
        let request = self.request(&interface)?.json(&body);
        let response =
            execute_request(request, self.card_config.timeout, cancellation, None).await?;
        let values = response_values(response, self.card_config.timeout, cancellation).await?;
        values.into_iter().map(json_rpc_result).collect()
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let interface = self.selected_interface()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = execute_request(
            self.request(&interface)?.json(&body),
            self.card_config.timeout,
            cancellation,
            None,
        )
        .await?;
        json_rpc_result(response_value(response, self.card_config.timeout, cancellation).await?)
    }

    fn selected_interface(&self) -> Result<AgentInterface, ProtocolError> {
        self.interface
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A interface lock was poisoned".to_owned()))?
            .clone()
            .ok_or_else(|| ProtocolError::Malformed("A2A discovery has not run".to_owned()))
    }

    fn request(
        &self,
        interface: &AgentInterface,
    ) -> Result<reqwest::RequestBuilder, ProtocolError> {
        let url = Url::parse(&interface.url)
            .map_err(|error| ProtocolError::Malformed(format!("A2A interface URL: {error}")))?;
        Ok(self.card_config.headers.iter().fold(
            self.client
                .post(url)
                .header("A2A-Version", A2A_PROTOCOL_VERSION)
                .header("Content-Type", "application/json"),
            |request, (name, value)| request.header(name, value),
        ))
    }
}

pub struct ProtocolActionHandler {
    mcp: BTreeMap<String, Arc<McpClient>>,
    a2a: BTreeMap<String, Arc<A2aClient>>,
}

impl ProtocolActionHandler {
    #[must_use]
    pub fn new(
        mcp: BTreeMap<String, Arc<McpClient>>,
        a2a: BTreeMap<String, Arc<A2aClient>>,
    ) -> Self {
        Self { mcp, a2a }
    }
}

#[async_trait]
impl ExternalActionHandler for ProtocolActionHandler {
    async fn execute(
        &self,
        kind: ActionKind,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, RuntimeError> {
        match kind {
            ActionKind::McpCall => {
                let server = string_field(input, "server")
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                let tool = string_field(input, "tool")
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                let arguments = input
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                self.mcp
                    .get(&server)
                    .ok_or_else(|| {
                        RuntimeError::InvalidState(format!("unknown MCP server `{server}`"))
                    })?
                    .call_tool(&tool, arguments, cancellation)
                    .await
                    .map_err(map_effect_error)
            }
            ActionKind::A2aDelegate => {
                let peer = string_field(input, "peer")
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                let message_id = string_field(input, "messageId")
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                let text = string_field(input, "message")
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                let client = self.a2a.get(&peer).ok_or_else(|| {
                    RuntimeError::InvalidState(format!("unknown A2A peer `{peer}`"))
                })?;
                if client.selected_interface().is_err() {
                    client
                        .discover(cancellation)
                        .await
                        .map_err(map_effect_error)?;
                }
                let response = client
                    .send_message(&message_id, &text, None, cancellation)
                    .await
                    .map_err(map_effect_error)?;
                match response {
                    A2aResponse::Message(message) => Ok(message),
                    A2aResponse::Task(task) => client
                        .wait_for_task(task, cancellation)
                        .await
                        .map_err(map_effect_error),
                }
            }
            _ => Err(RuntimeError::InvalidState(
                "protocol handler received a non-protocol action".to_owned(),
            )),
        }
    }
}

fn map_effect_error(error: ProtocolError) -> RuntimeError {
    match error {
        ProtocolError::Cancelled => RuntimeError::Cancelled,
        error @ (ProtocolError::Timeout(_)
        | ProtocolError::Transport(_)
        | ProtocolError::Malformed(_)
        | ProtocolError::SessionExpired
        | ProtocolError::PollLimit(_)) => RuntimeError::ExternalEffectUncertain(error.to_string()),
        error => RuntimeError::InvalidState(error.to_string()),
    }
}

async fn execute_request(
    request: reqwest::RequestBuilder,
    timeout: Duration,
    cancellation: &CancellationToken,
    mcp_cancel: Option<(u64, &McpClient)>,
) -> Result<Response, ProtocolError> {
    tokio::select! {
        result = tokio::time::timeout(timeout, request.send()) => {
            result
                .map_err(|_| ProtocolError::Timeout(timeout))?
                .map_err(|error| ProtocolError::Transport(error.to_string()))
                .and_then(|response| {
                    if response.status() == StatusCode::NOT_FOUND && mcp_cancel.is_some() {
                        Err(ProtocolError::SessionExpired)
                    } else {
                        Ok(response)
                    }
                })
        }
        () = cancellation.cancelled() => {
            if let Some((id, client)) = mcp_cancel {
                client.cancel_request(id).await;
            }
            Err(ProtocolError::Cancelled)
        }
    }
}

async fn response_json<T: for<'de> Deserialize<'de>>(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<T, ProtocolError> {
    if !response.status().is_success() {
        return Err(http_error(response).await);
    }
    let bytes = bounded_response(response, timeout, cancellation).await?;
    serde_json::from_slice(&bytes).map_err(|error| ProtocolError::Malformed(error.to_string()))
}

async fn response_value(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<Value, ProtocolError> {
    response_values(response, timeout, cancellation)
        .await?
        .into_iter()
        .last()
        .ok_or_else(|| ProtocolError::Malformed("empty protocol response".to_owned()))
}

async fn response_values(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<Vec<Value>, ProtocolError> {
    if !response.status().is_success() {
        return Err(http_error(response).await);
    }
    let is_sse = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_sse {
        let bytes = bounded_response(response, timeout, cancellation).await?;
        let text = String::from_utf8(bytes)
            .map_err(|error| ProtocolError::Malformed(format!("SSE encoding: {error}")))?;
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|data| {
                serde_json::from_str(data.trim())
                    .map_err(|error| ProtocolError::Malformed(format!("SSE data: {error}")))
            })
            .collect()
    } else {
        let bytes = bounded_response(response, timeout, cancellation).await?;
        serde_json::from_slice(&bytes)
            .map(|value| vec![value])
            .map_err(|error| ProtocolError::Malformed(error.to_string()))
    }
}

async fn bounded_response(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ProtocolError> {
    let collect = async move {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ProtocolError::Transport(error.to_string()))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_PROTOCOL_RESPONSE_BYTES {
                return Err(ProtocolError::Malformed(format!(
                    "response exceeds {MAX_PROTOCOL_RESPONSE_BYTES} bytes"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! {
        result = tokio::time::timeout(timeout, collect) => {
            result.map_err(|_| ProtocolError::Timeout(timeout))?
        }
        () = cancellation.cancelled() => Err(ProtocolError::Cancelled),
    }
}

async fn http_error(response: Response) -> ProtocolError {
    let status = response.status().as_u16();
    ProtocolError::Http {
        status,
        message: "remote protocol request failed; body omitted".to_owned(),
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn json_rpc_result(value: Value) -> Result<Value, ProtocolError> {
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(ProtocolError::Malformed(
            "response is not JSON-RPC 2.0".to_owned(),
        ));
    }
    if let Some(error) = value.get("error") {
        return Err(ProtocolError::Remote {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-1),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("remote error")
                .to_owned(),
        });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| ProtocolError::Malformed("JSON-RPC response omits result".to_owned()))
}

fn string_field(value: &Value, field: &str) -> Result<String, ProtocolError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ProtocolError::Malformed(format!("missing string field `{field}`")))
}

fn summarize_remote(value: &Value) -> String {
    serde_json::to_string(value)
        .map(|mut value| {
            value.truncate(512);
            value
        })
        .unwrap_or_else(|_| "remote tool failed".to_owned())
}

fn task_state(task: &Value) -> Option<&str> {
    task.pointer("/status/state").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn mcp_negotiates_session_lists_and_calls_tools() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer fixture"))
            .and(body_partial_json(serde_json::json!({"method": "initialize"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "session-1")
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "result": {"protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {}, "serverInfo": {"name": "mock", "version": "1"}}
                    })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(
                serde_json::json!({"method": "notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("mcp-session-id", "session-1"))
            .and(header("mcp-protocol-version", MCP_PROTOCOL_VERSION))
            .and(body_partial_json(serde_json::json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": {"tools": [{"name": "echo", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(
                serde_json::json!({"method": "tools/call"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 3,
                "result": {"structuredContent": {"echo": "ok"}, "isError": false}
            })))
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            headers: BTreeMap::from([("authorization".to_owned(), "Bearer fixture".to_owned())]),
            timeout: Duration::from_secs(2),
        })
        .expect("client");
        let cancellation = CancellationToken::new();
        client.initialize(&cancellation).await.expect("initialize");
        let tools = client.list_tools(&cancellation).await.expect("tools");
        assert_eq!(tools[0].name, "echo");
        // Annotations are retained as untrusted data; they never become policy decisions here.
        assert!(tools[0].annotations.is_some());
        let result = client
            .call_tool("echo", serde_json::json!({"text": "ok"}), &cancellation)
            .await
            .expect("call");
        assert_eq!(result, serde_json::json!({"echo": "ok"}));
    }

    #[tokio::test]
    async fn mcp_rejects_version_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"protocolVersion": "2099-01-01"}
            })))
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig {
            url: Url::parse(&server.uri()).expect("url"),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(2),
        })
        .expect("client");
        assert!(matches!(
            client.initialize(&CancellationToken::new()).await,
            Err(ProtocolError::Version { .. })
        ));
    }

    #[tokio::test]
    async fn a2a_discovers_v1_and_polls_to_artifact() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "mock-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": true},
                "skills": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(header("a2a-version", A2A_PROTOCOL_VERSION))
            .and(body_partial_json(
                serde_json::json!({"method": "SendMessage"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"task": {"id": "task-1", "status": {"state": "working"}}}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(serde_json::json!({"method": "GetTask"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": {"id": "task-1", "status": {"state": "completed"}, "artifacts": [{"artifactId": "a1", "parts": [{"text": "done"}]}]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(serde_json::json!({
                "method": "SendStreamingMessage"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"message\":{\"messageId\":\"stream-1\"}}}\n\n",
                    "text/event-stream",
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(
                serde_json::json!({"method": "CancelTask"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 4,
                "result": {"id": "task-1", "status": {"state": "canceled"}}
            })))
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/.well-known/agent-card.json", server.uri())).expect("url"),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(2),
        })
        .expect("client")
        .with_poll_bounds(2, Duration::from_millis(1));
        let cancellation = CancellationToken::new();
        client.discover(&cancellation).await.expect("discover");
        let response = client
            .send_message("message-1", "do work", None, &cancellation)
            .await
            .expect("send");
        let A2aResponse::Task(task) = response else {
            panic!("expected task");
        };
        let completed = client
            .wait_for_task(task, &cancellation)
            .await
            .expect("poll");
        assert_eq!(task_state(&completed), Some("completed"));
        assert_eq!(
            completed.pointer("/artifacts/0/artifactId"),
            Some(&Value::String("a1".to_owned()))
        );
        let stream = client
            .send_streaming_message("message-2", "stream", &cancellation)
            .await
            .expect("stream");
        assert_eq!(stream.len(), 1);
        let cancelled = client
            .cancel_task("task-1", &cancellation)
            .await
            .expect("cancel");
        assert_eq!(task_state(&cancelled), Some("canceled"));
    }

    #[tokio::test]
    async fn protocol_timeout_is_bounded() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&server.uri()).expect("url"),
            headers: BTreeMap::new(),
            timeout: Duration::from_millis(10),
        })
        .expect("client");
        assert!(matches!(
            client.discover(&CancellationToken::new()).await,
            Err(ProtocolError::Timeout(_))
        ));
    }

    #[tokio::test]
    async fn a2a_rejects_cross_origin_interface_and_honors_cancellation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cross-origin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "malicious",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": "http://127.0.0.1:9/a2a",
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }]
            })))
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/cross-origin", server.uri())).expect("url"),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(2),
        })
        .expect("client");
        assert!(matches!(
            client.discover(&CancellationToken::new()).await,
            Err(ProtocolError::Unsupported(_))
        ));

        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let slow = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/slow", server.uri())).expect("url"),
            headers: BTreeMap::new(),
            timeout: Duration::from_secs(2),
        })
        .expect("client");
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            signal.cancel();
        });
        assert!(matches!(
            slow.discover(&cancellation).await,
            Err(ProtocolError::Cancelled)
        ));
    }

    #[test]
    fn ambiguous_protocol_failures_are_not_classified_as_definitive() {
        assert!(matches!(
            map_effect_error(ProtocolError::Timeout(Duration::from_secs(1))),
            RuntimeError::ExternalEffectUncertain(_)
        ));
        assert!(matches!(
            map_effect_error(ProtocolError::Transport("closed".to_owned())),
            RuntimeError::ExternalEffectUncertain(_)
        ));
        assert!(matches!(
            map_effect_error(ProtocolError::Cancelled),
            RuntimeError::Cancelled
        ));
    }
}
