//! Stable MCP and A2A protocol clients for agentctl.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentctl_core::dsl::{ActionKind, Idempotency, SecretReference};
use agentctl_core::network::{HttpTransportSecurity, custom_ca_pem_is_valid};
use agentctl_core::secret::{SecretSourceResolver, SecretValue};
use agentctl_runtime::{
    ExternalActionContext, ExternalActionHandler, ExternalEventSink, ExternalStreamEvent,
    RuntimeError,
};
use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
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
    #[error("protocol authentication refresh failed: {0}")]
    Authentication(String),
    #[error("remote schema changed for `{tool}`: expected `{expected}`, found `{found}`")]
    SchemaChanged {
        tool: String,
        expected: String,
        found: String,
    },
    #[error("bounded reconnect was exhausted for {0}")]
    ReconnectLimit(String),
    #[error("remote effect cannot continue safely: {0}")]
    ContinuationUnavailable(String),
    #[error("remote operation failed ({code}): {message}")]
    Remote { code: i64, message: String },
    #[error("remote operation is unsupported: {0}")]
    Unsupported(String),
    #[error("remote task `{0}` did not finish before the polling bound")]
    PollLimit(String),
}

#[derive(Clone)]
pub struct ProtocolHttpConfig {
    pub url: Url,
    pub headers: BTreeMap<String, SecretValue>,
    pub header_references: BTreeMap<String, SecretReference>,
    pub header_resolver: Option<Arc<dyn SecretSourceResolver>>,
    pub timeout: Duration,
    pub transport: HttpTransportSecurity,
}

impl ProtocolHttpConfig {
    #[must_use]
    pub fn fixed(url: Url, headers: BTreeMap<String, SecretValue>, timeout: Duration) -> Self {
        Self {
            url,
            headers,
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout,
            transport: HttpTransportSecurity::default(),
        }
    }
}

fn client(config: &ProtocolHttpConfig) -> Result<Client, ProtocolError> {
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("agentctl/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(config.transport.connect_timeout);
    if !config.transport.allow_proxy {
        builder = builder.no_proxy();
    }
    if let Some(host) = &config.transport.resolved_host
        && !config.transport.resolved_addresses.is_empty()
    {
        builder = builder.resolve_to_addrs(host, &config.transport.resolved_addresses);
    }
    if let Some(pem) = &config.transport.custom_ca_pem {
        if !custom_ca_pem_is_valid(pem.expose()) {
            return Err(ProtocolError::Transport(
                "network custom CA PEM is invalid".to_owned(),
            ));
        }
        let certificate = reqwest::Certificate::from_pem(pem.expose().as_bytes())
            .map_err(|_| ProtocolError::Transport("network custom CA PEM is invalid".to_owned()))?;
        builder = builder.add_root_certificate(certificate);
    }
    builder
        .build()
        .map_err(|error| ProtocolError::Transport(error.to_string()))
}

struct NoopExternalEvents;

#[async_trait]
impl ExternalEventSink for NoopExternalEvents {
    async fn emit(&self, _event: ExternalStreamEvent) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub annotations: Option<Value>,
    pub execution: Option<Value>,
}

pub struct McpClient {
    client: Client,
    config: ProtocolHttpConfig,
    headers: Mutex<BTreeMap<String, SecretValue>>,
    session_id: Mutex<Option<String>>,
    initialized: AtomicBool,
    generation: AtomicU64,
    tools: Mutex<BTreeMap<String, McpTool>>,
    initialize_lock: AsyncMutex<()>,
    next_id: AtomicU64,
}

impl McpClient {
    pub fn new(config: ProtocolHttpConfig) -> Result<Self, ProtocolError> {
        Ok(Self {
            client: client(&config)?,
            headers: Mutex::new(config.headers.clone()),
            config,
            session_id: Mutex::new(None),
            initialized: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            tools: Mutex::new(BTreeMap::new()),
            initialize_lock: AsyncMutex::new(()),
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn initialize(&self, cancellation: &CancellationToken) -> Result<(), ProtocolError> {
        self.ensure_initialized(cancellation).await
    }

    async fn ensure_initialized(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), ProtocolError> {
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.initialize_lock.lock().await;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut last_error = None;
        for _ in 0..=1 {
            match self.initialize_once(cancellation).await {
                Ok(()) => return Ok(()),
                Err(error @ (ProtocolError::Transport(_) | ProtocolError::Timeout(_))) => {
                    last_error = Some(error);
                    self.reset_session()?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(ProtocolError::ReconnectLimit(
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "MCP initialization".to_owned()),
        ))
    }

    async fn initialize_once(&self, cancellation: &CancellationToken) -> Result<(), ProtocolError> {
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
        let tools = self.list_tools_once(cancellation).await?;
        *self
            .tools
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP tool lock was poisoned".to_owned()))? =
            tools
                .into_iter()
                .map(|tool| (tool.name.clone(), tool))
                .collect();
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn list_tools(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<McpTool>, ProtocolError> {
        self.ensure_initialized(cancellation).await?;
        Ok(self
            .tools
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP tool lock was poisoned".to_owned()))?
            .values()
            .cloned()
            .collect())
    }

    async fn list_tools_once(
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
                    execution: tool.get("execution").cloned(),
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
        self.call_tool_with_events(
            name,
            arguments,
            Idempotency::Unknown,
            None,
            &NoopExternalEvents,
            cancellation,
        )
        .await
    }

    pub async fn call_tool_with_events(
        &self,
        name: &str,
        arguments: Value,
        idempotency: Idempotency,
        idempotency_key: Option<&str>,
        events: &dyn ExternalEventSink,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        self.ensure_initialized(cancellation).await?;
        let expected_schema = self.tool_schema_digest(name)?;
        let mut reconnects = 0_u8;
        let mut call_params = serde_json::json!({"name": name, "arguments": arguments.clone()});
        if let Some(idempotency_key) = idempotency_key {
            call_params["_meta"] = serde_json::json!({
                "agentctl.dev/idempotency-key": idempotency_key,
            });
        }
        let result = loop {
            let result = self
                .rpc_with_events(
                    "tools/call",
                    call_params.clone(),
                    true,
                    Some(events),
                    cancellation,
                )
                .await;
            match result {
                Ok(result) => break result,
                Err(error) if reconnects == 0 && reconnect_safe(&error, idempotency) => {
                    reconnects = reconnects.saturating_add(1);
                    self.reset_session()?;
                    self.ensure_initialized(cancellation).await?;
                    let found = self.tool_schema_digest(name)?;
                    if found != expected_schema {
                        return Err(ProtocolError::SchemaChanged {
                            tool: name.to_owned(),
                            expected: expected_schema,
                            found,
                        });
                    }
                    events
                        .emit(ExternalStreamEvent {
                            event_type: "mcp.reconnected".to_owned(),
                            remote_sequence: None,
                            payload: serde_json::json!({
                                "generation": self.generation(),
                                "tool": name,
                            }),
                        })
                        .await
                        .map_err(|error| ProtocolError::Transport(error.to_string()))?;
                }
                Err(error) => return Err(error),
            }
        };
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

    #[must_use]
    pub fn generation(&self) -> u32 {
        u32::try_from(self.generation.load(Ordering::Acquire)).unwrap_or(u32::MAX)
    }

    pub fn session_state(&self) -> Result<Value, ProtocolError> {
        let session_id = self
            .session_id
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP session lock was poisoned".to_owned()))?
            .clone();
        let tools = self
            .tools
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP tool lock was poisoned".to_owned()))?;
        let catalog = tools
            .iter()
            .map(|(name, tool)| {
                Ok(serde_json::json!({
                    "name": name,
                    "schemaDigest": mcp_tool_digest(tool)?,
                    "execution": tool.execution,
                }))
            })
            .collect::<Result<Vec<_>, ProtocolError>>()?;
        Ok(serde_json::json!({
            "sessionId": session_id,
            "generation": self.generation(),
            "catalog": catalog,
            "catalogDigest": canonical_digest(&serde_json::to_value(
                tools.values().collect::<Vec<_>>()
            ).map_err(|error| ProtocolError::Malformed(error.to_string()))?)?,
        }))
    }

    fn tool_schema_digest(&self, name: &str) -> Result<String, ProtocolError> {
        let tools = self
            .tools
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP tool lock was poisoned".to_owned()))?;
        let tool = tools.get(name).ok_or_else(|| {
            ProtocolError::Unsupported(format!("MCP server does not expose tool `{name}`"))
        })?;
        mcp_tool_digest(tool)
    }

    fn reset_session(&self) -> Result<(), ProtocolError> {
        self.initialized.store(false, Ordering::Release);
        *self
            .session_id
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP session lock was poisoned".to_owned()))? =
            None;
        self.tools
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP tool lock was poisoned".to_owned()))?
            .clear();
        Ok(())
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        require_session: bool,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        self.rpc_with_events(method, params, require_session, None, cancellation)
            .await
    }

    async fn rpc_with_events(
        &self,
        method: &str,
        params: Value,
        require_session: bool,
        events: Option<&dyn ExternalEventSink>,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut response = None;
        for auth_attempt in 0..=1 {
            let mut request = self.request()?.json(&body);
            if require_session {
                let session = self
                    .session_id
                    .lock()
                    .map_err(|_| {
                        ProtocolError::Transport("MCP session lock was poisoned".to_owned())
                    })?
                    .clone();
                request = request.header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
                if let Some(session) = session {
                    request = request.header("Mcp-Session-Id", session);
                }
            }
            let received =
                execute_request(request, self.config.timeout, cancellation, Some((id, self)))
                    .await?;
            if received.status() == StatusCode::NOT_FOUND && require_session {
                return Err(ProtocolError::SessionExpired);
            }
            if received.status() == StatusCode::UNAUTHORIZED && auth_attempt == 0 {
                self.refresh_headers(cancellation).await?;
                continue;
            }
            response = Some(received);
            break;
        }
        let response = response.ok_or_else(|| {
            ProtocolError::Authentication("credential refresh did not authorize request".to_owned())
        })?;
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
        let headers = self.current_headers()?;
        let (values, streamed) = response_values_with_events(
            response,
            self.config.timeout,
            cancellation,
            &headers,
            events,
            "mcp.stream",
            self.config.transport.max_response_bytes,
        )
        .await?;
        let mut result = None;
        for value in values {
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                result = Some(value);
            } else if !streamed && let Some(events) = events {
                let method = value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                events
                    .emit(ExternalStreamEvent {
                        event_type: format!("mcp.{method}"),
                        remote_sequence: None,
                        payload: value,
                    })
                    .await
                    .map_err(|error| ProtocolError::Transport(error.to_string()))?;
            }
        }
        json_rpc_result(result.ok_or_else(|| {
            ProtocolError::Malformed(format!("MCP response omits request ID {id}"))
        })?)
    }

    async fn notification(
        &self,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<(), ProtocolError> {
        let body = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params});
        for auth_attempt in 0..=1 {
            let mut request = self
                .request()?
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
            let response =
                execute_request(request, self.config.timeout, cancellation, None).await?;
            if response.status() == StatusCode::UNAUTHORIZED && auth_attempt == 0 {
                self.refresh_headers(cancellation).await?;
                continue;
            }
            return if response.status().is_success() {
                Ok(())
            } else {
                Err(http_error(response).await)
            };
        }
        Err(ProtocolError::Authentication(
            "credential refresh did not authorize MCP notification".to_owned(),
        ))
    }

    fn request(&self) -> Result<reqwest::RequestBuilder, ProtocolError> {
        Ok(self.current_headers()?.iter().fold(
            self.client
                .post(self.config.url.clone())
                .header("Accept", "application/json, text/event-stream")
                .header("Origin", "agentctl://local"),
            |request, (name, value)| request.header(name, value.expose()),
        ))
    }

    fn current_headers(&self) -> Result<BTreeMap<String, SecretValue>, ProtocolError> {
        self.headers
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP header lock was poisoned".to_owned()))
            .map(|headers| headers.clone())
    }

    async fn refresh_headers(&self, cancellation: &CancellationToken) -> Result<(), ProtocolError> {
        let resolver = self.config.header_resolver.as_ref().ok_or_else(|| {
            ProtocolError::Authentication("no header resolver is configured".to_owned())
        })?;
        if self.config.header_references.is_empty() {
            return Err(ProtocolError::Authentication(
                "no refreshable headers are configured".to_owned(),
            ));
        }
        let mut refreshed = BTreeMap::new();
        for (name, reference) in &self.config.header_references {
            let value = resolver
                .resolve_secret(reference, cancellation)
                .await
                .map_err(ProtocolError::Authentication)?;
            refreshed.insert(name.clone(), value);
        }
        *self
            .headers
            .lock()
            .map_err(|_| ProtocolError::Transport("MCP header lock was poisoned".to_owned()))? =
            refreshed;
        Ok(())
    }

    async fn cancel_request(&self, id: u64) {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": id}
        });
        let Ok(request) = self.request() else {
            return;
        };
        let mut request = request
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
    headers: Mutex<BTreeMap<String, SecretValue>>,
    card: Mutex<Option<AgentCard>>,
    interface: Mutex<Option<AgentInterface>>,
    generation: AtomicU64,
    discover_lock: AsyncMutex<()>,
    next_id: AtomicU64,
    max_polls: usize,
    poll_interval: Duration,
}

impl A2aClient {
    pub fn new(config: ProtocolHttpConfig) -> Result<Self, ProtocolError> {
        Ok(Self {
            client: client(&config)?,
            headers: Mutex::new(config.headers.clone()),
            card_config: config,
            card: Mutex::new(None),
            interface: Mutex::new(None),
            generation: AtomicU64::new(0),
            discover_lock: AsyncMutex::new(()),
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
        let _guard = self.discover_lock.lock().await;
        let mut response = None;
        for auth_attempt in 0..=1 {
            let request = self.current_headers()?.iter().fold(
                self.client
                    .get(self.card_config.url.clone())
                    .header("A2A-Version", A2A_PROTOCOL_VERSION),
                |request, (name, value)| request.header(name, value.expose()),
            );
            let received =
                execute_request(request, self.card_config.timeout, cancellation, None).await?;
            if received.status() == StatusCode::UNAUTHORIZED && auth_attempt == 0 {
                self.refresh_headers(cancellation).await?;
                continue;
            }
            response = Some(received);
            break;
        }
        let response = response.ok_or_else(|| {
            ProtocolError::Authentication("credential refresh did not authorize Agent Card".into())
        })?;
        let headers = self.current_headers()?;
        let card: AgentCard = response_json(
            response,
            self.card_config.timeout,
            cancellation,
            &headers,
            self.card_config.transport.max_response_bytes,
        )
        .await?;
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
        *self
            .card
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A card lock was poisoned".to_owned()))? =
            Some(card.clone());
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(card)
    }

    async fn ensure_discovered(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), ProtocolError> {
        if self.selected_interface().is_ok() {
            return Ok(());
        }
        self.discover(cancellation).await.map(|_| ())
    }

    #[must_use]
    pub fn generation(&self) -> u32 {
        u32::try_from(self.generation.load(Ordering::Acquire)).unwrap_or(u32::MAX)
    }

    pub fn session_state(&self) -> Result<Value, ProtocolError> {
        let card = self
            .card
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A card lock was poisoned".to_owned()))?
            .clone();
        let interface = self.selected_interface()?;
        Ok(serde_json::json!({
            "generation": self.generation(),
            "cardDigest": card
                .as_ref()
                .map(|card| serde_json::to_value(card)
                    .map_err(|error| ProtocolError::Malformed(error.to_string()))
                    .and_then(|card| canonical_digest(&card)))
                .transpose()?,
            "interface": interface,
            "capabilities": card.map(|card| card.capabilities),
        }))
    }

    fn supports_streaming(&self) -> Result<bool, ProtocolError> {
        Ok(self
            .card
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A card lock was poisoned".to_owned()))?
            .as_ref()
            .and_then(|card| card.capabilities.get("streaming"))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    pub async fn send_message(
        &self,
        message_id: &str,
        text: &str,
        context_id: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<A2aResponse, ProtocolError> {
        self.ensure_discovered(cancellation).await?;
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
        self.wait_for_task_inner(task, None, cancellation).await
    }

    pub async fn wait_for_task_with_events(
        &self,
        task: Value,
        events: &dyn ExternalEventSink,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        self.wait_for_task_inner(task, Some(events), cancellation)
            .await
    }

    async fn wait_for_task_inner(
        &self,
        task: Value,
        events: Option<&dyn ExternalEventSink>,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let task_id = string_field(&task, "id")?;
        let mut current = task;
        let mut refreshes = 0_u8;
        if let Some(events) = events
            && self.supports_streaming()?
        {
            match self.subscribe_to_task(&task_id, events, cancellation).await {
                Ok(Some(task)) => current = task,
                Ok(None)
                | Err(ProtocolError::Transport(_))
                | Err(ProtocolError::Timeout(_))
                | Err(ProtocolError::Malformed(_))
                | Err(ProtocolError::Http { status: 404, .. })
                | Err(ProtocolError::Unsupported(_)) => {}
                Err(error) => return Err(error),
            }
        }
        for _ in 0..self.max_polls {
            if let Some(events) = events {
                events
                    .emit(ExternalStreamEvent {
                        event_type: "a2a.task.status".to_owned(),
                        remote_sequence: None,
                        payload: current.clone(),
                    })
                    .await
                    .map_err(|error| ProtocolError::Transport(error.to_string()))?;
            }
            match normalized_task_state(&current) {
                Some("completed") => return Ok(current),
                Some("failed" | "rejected" | "canceled") => {
                    return Err(ProtocolError::Remote {
                        code: -1,
                        message: format!(
                            "A2A task `{task_id}` ended in {}",
                            normalized_task_state(&current).unwrap_or("unknown")
                        ),
                    });
                }
                Some("input_required" | "auth_required") => {
                    return Err(ProtocolError::ContinuationUnavailable(format!(
                        "A2A task `{task_id}` requires external input or authentication"
                    )));
                }
                _ => {}
            }
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                () = cancellation.cancelled() => {
                    let _ = self.cancel_task(&task_id, &CancellationToken::new()).await;
                    return Err(ProtocolError::Cancelled);
                }
            }
            current = match self
                .rpc("GetTask", serde_json::json!({"id": task_id}), cancellation)
                .await
            {
                Ok(task) => task,
                Err(error @ (ProtocolError::Transport(_) | ProtocolError::Timeout(_)))
                    if refreshes == 0 =>
                {
                    refreshes = refreshes.saturating_add(1);
                    self.discover(cancellation).await?;
                    if let Some(events) = events {
                        events
                            .emit(ExternalStreamEvent {
                                event_type: "a2a.card.refreshed".to_owned(),
                                remote_sequence: None,
                                payload: self.session_state()?,
                            })
                            .await
                            .map_err(|error| ProtocolError::Transport(error.to_string()))?;
                    }
                    let _ = error;
                    self.rpc("GetTask", serde_json::json!({"id": task_id}), cancellation)
                        .await?
                }
                Err(error) => return Err(error),
            };
            if let Some(inner) = current.get("task") {
                current = inner.clone();
            }
        }
        match normalized_task_state(&current) {
            Some("completed") => Ok(current),
            Some("input_required" | "auth_required") => {
                Err(ProtocolError::ContinuationUnavailable(format!(
                    "A2A task `{task_id}` requires external input or authentication"
                )))
            }
            Some("failed" | "rejected" | "canceled") => Err(ProtocolError::Remote {
                code: -1,
                message: format!(
                    "A2A task `{task_id}` ended in {}",
                    normalized_task_state(&current).unwrap_or("unknown")
                ),
            }),
            _ => Err(ProtocolError::PollLimit(task_id)),
        }
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

    async fn subscribe_to_task(
        &self,
        task_id: &str,
        events: &dyn ExternalEventSink,
        cancellation: &CancellationToken,
    ) -> Result<Option<Value>, ProtocolError> {
        let values = self
            .rpc_values_with_events(
                "SubscribeToTask",
                serde_json::json!({"id": task_id}),
                Some(events),
                "a2a.stream",
                cancellation,
            )
            .await?;
        let mut current = None;
        for value in values {
            let event = if value.get("jsonrpc").is_some() {
                json_rpc_result(value)?
            } else {
                value
            };
            if let Some(task) = event.get("task") {
                current = Some(task.clone());
            } else if event.get("id").and_then(Value::as_str) == Some(task_id)
                && event.get("status").is_some()
            {
                current = Some(event);
            }
        }
        Ok(current)
    }

    pub async fn send_streaming_message(
        &self,
        message_id: &str,
        text: &str,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ProtocolError> {
        let (_, response) = self
            .rpc_response(
                "SendStreamingMessage",
                serde_json::json!({
                    "message": {"messageId": message_id, "role": "user", "parts": [{"text": text}]}
                }),
                cancellation,
            )
            .await?;
        let headers = self.current_headers()?;
        let values = response_values(
            response,
            self.card_config.timeout,
            cancellation,
            &headers,
            self.card_config.transport.max_response_bytes,
        )
        .await?;
        values.into_iter().map(json_rpc_result).collect()
    }

    async fn rpc_values_with_events(
        &self,
        method: &str,
        params: Value,
        events: Option<&dyn ExternalEventSink>,
        event_type: &str,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ProtocolError> {
        let (_, response) = self.rpc_response(method, params, cancellation).await?;
        let headers = self.current_headers()?;
        response_values_with_events(
            response,
            self.card_config.timeout,
            cancellation,
            &headers,
            events,
            event_type,
            self.card_config.transport.max_response_bytes,
        )
        .await
        .map(|(values, _)| values)
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProtocolError> {
        let (_, response) = self.rpc_response(method, params, cancellation).await?;
        let headers = self.current_headers()?;
        json_rpc_result(
            response_value(
                response,
                self.card_config.timeout,
                cancellation,
                &headers,
                self.card_config.transport.max_response_bytes,
            )
            .await?,
        )
    }

    async fn rpc_response(
        &self,
        method: &str,
        params: Value,
        cancellation: &CancellationToken,
    ) -> Result<(u64, Response), ProtocolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        for auth_attempt in 0..=1 {
            let interface = self.selected_interface()?;
            let response = execute_request(
                self.request(&interface)?.json(&body),
                self.card_config.timeout,
                cancellation,
                None,
            )
            .await?;
            if response.status() == StatusCode::UNAUTHORIZED && auth_attempt == 0 {
                self.refresh_headers(cancellation).await?;
                continue;
            }
            return Ok((id, response));
        }
        Err(ProtocolError::Authentication(
            "credential refresh did not authorize A2A request".to_owned(),
        ))
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
        Ok(self.current_headers()?.iter().fold(
            self.client
                .post(url)
                .header("A2A-Version", A2A_PROTOCOL_VERSION)
                .header("Content-Type", "application/json"),
            |request, (name, value)| request.header(name, value.expose()),
        ))
    }

    async fn fetch_artifact_url(
        &self,
        artifact_url: &str,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, ProtocolError> {
        let interface = self.selected_interface()?;
        let interface_url = Url::parse(&interface.url)
            .map_err(|error| ProtocolError::Malformed(format!("A2A interface URL: {error}")))?;
        let url = Url::parse(artifact_url)
            .map_err(|error| ProtocolError::Malformed(format!("A2A artifact URL: {error}")))?;
        if !same_origin(&interface_url, &url) {
            return Err(ProtocolError::Unsupported(
                "A2A artifact URL must share the selected interface origin".to_owned(),
            ));
        }
        for auth_attempt in 0..=1 {
            let request = self
                .current_headers()?
                .iter()
                .fold(self.client.get(url.clone()), |request, (name, value)| {
                    request.header(name, value.expose())
                });
            let response =
                execute_request(request, self.card_config.timeout, cancellation, None).await?;
            if response.status() == StatusCode::UNAUTHORIZED && auth_attempt == 0 {
                self.refresh_headers(cancellation).await?;
                continue;
            }
            if !response.status().is_success() {
                return Err(http_error(response).await);
            }
            return bounded_response(
                response,
                self.card_config.timeout,
                cancellation,
                self.card_config.transport.max_response_bytes,
            )
            .await;
        }
        Err(ProtocolError::Authentication(
            "credential refresh did not authorize A2A artifact retrieval".to_owned(),
        ))
    }

    fn current_headers(&self) -> Result<BTreeMap<String, SecretValue>, ProtocolError> {
        self.headers
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A header lock was poisoned".to_owned()))
            .map(|headers| headers.clone())
    }

    async fn refresh_headers(&self, cancellation: &CancellationToken) -> Result<(), ProtocolError> {
        let resolver = self.card_config.header_resolver.as_ref().ok_or_else(|| {
            ProtocolError::Authentication("no header resolver is configured".to_owned())
        })?;
        if self.card_config.header_references.is_empty() {
            return Err(ProtocolError::Authentication(
                "no refreshable headers are configured".to_owned(),
            ));
        }
        let mut refreshed = BTreeMap::new();
        for (name, reference) in &self.card_config.header_references {
            let value = resolver
                .resolve_secret(reference, cancellation)
                .await
                .map_err(ProtocolError::Authentication)?;
            refreshed.insert(name.clone(), value);
        }
        *self
            .headers
            .lock()
            .map_err(|_| ProtocolError::Transport("A2A header lock was poisoned".to_owned()))? =
            refreshed;
        Ok(())
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
        context: &ExternalActionContext,
        kind: ActionKind,
        input: &Value,
        events: &dyn ExternalEventSink,
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
                let client = self.mcp.get(&server).ok_or_else(|| {
                    RuntimeError::InvalidState(format!("unknown MCP server `{server}`"))
                })?;
                client
                    .initialize(cancellation)
                    .await
                    .map_err(map_pre_dispatch_error)?;
                let initial_generation = client.generation();
                context
                    .store
                    .put_protocol_session(
                        &context.run_id,
                        &context.task_id,
                        &context.effect_id,
                        "mcp",
                        client.config.url.as_str(),
                        initial_generation,
                        "initialized",
                        &client.session_state().map_err(map_effect_error)?,
                        Utc::now(),
                    )
                    .map_err(RuntimeError::from)?;
                let state = serde_json::json!({
                    "server": server,
                    "tool": tool,
                    "argumentsDigest": canonical_digest(&arguments).map_err(map_effect_error)?,
                });
                self.put_call(
                    context,
                    "mcp",
                    &tool,
                    "prepared",
                    &state,
                    initial_generation,
                )?;
                let result = client
                    .call_tool_with_events(
                        &tool,
                        arguments,
                        context.idempotency,
                        (context.idempotency == Idempotency::Keyed)
                            .then_some(context.effect_id.as_str()),
                        events,
                        cancellation,
                    )
                    .await
                    .map_err(|error| {
                        let status = if ambiguous_protocol_error(&error) {
                            "uncertain"
                        } else {
                            "failed"
                        };
                        let _ = self.put_call(
                            context,
                            "mcp",
                            &tool,
                            status,
                            &serde_json::json!({
                                "server": server,
                                "tool": tool,
                                "error": error.to_string(),
                            }),
                            client.generation(),
                        );
                        map_effect_error(error)
                    })?;
                let generation = client.generation();
                context
                    .store
                    .put_protocol_session(
                        &context.run_id,
                        &context.task_id,
                        &context.effect_id,
                        "mcp",
                        client.config.url.as_str(),
                        generation,
                        if generation > initial_generation {
                            "reinitialized"
                        } else {
                            "initialized"
                        },
                        &client.session_state().map_err(map_effect_error)?,
                        Utc::now(),
                    )
                    .map_err(RuntimeError::from)?;
                self.put_call(
                    context,
                    "mcp",
                    &tool,
                    "succeeded",
                    &serde_json::json!({
                        "server": server,
                        "tool": tool,
                        "resultDigest": canonical_digest(&result).map_err(map_effect_error)?,
                    }),
                    generation,
                )?;
                Ok(result)
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
                client
                    .ensure_discovered(cancellation)
                    .await
                    .map_err(map_pre_dispatch_error)?;
                context
                    .store
                    .put_protocol_session(
                        &context.run_id,
                        &context.task_id,
                        &context.effect_id,
                        "a2a",
                        client.card_config.url.as_str(),
                        client.generation(),
                        "discovered",
                        &client.session_state().map_err(map_effect_error)?,
                        Utc::now(),
                    )
                    .map_err(RuntimeError::from)?;
                self.put_call(
                    context,
                    "a2a",
                    &message_id,
                    "submitting",
                    &serde_json::json!({
                        "peer": peer,
                        "messageId": message_id,
                        "messageDigest": canonical_digest(&Value::String(text.clone()))
                            .map_err(map_effect_error)?,
                        "submissionAmbiguous": false,
                    }),
                    client.generation(),
                )?;
                let response = client
                    .send_message(&message_id, &text, None, cancellation)
                    .await
                    .map_err(|error| {
                        let status = if ambiguous_protocol_error(&error) {
                            "uncertain"
                        } else {
                            "failed"
                        };
                        let _ = self.put_call(
                            context,
                            "a2a",
                            &message_id,
                            status,
                            &serde_json::json!({
                                "peer": peer,
                                "messageId": message_id,
                                "submissionAmbiguous": ambiguous_protocol_error(&error),
                                "error": error.to_string(),
                            }),
                            client.generation(),
                        );
                        map_effect_error(error)
                    })?;
                match response {
                    A2aResponse::Message(message) => {
                        self.put_call(
                            context,
                            "a2a",
                            &message_id,
                            "succeeded",
                            &serde_json::json!({
                                "peer": peer,
                                "messageId": message_id,
                                "response": "message",
                            }),
                            client.generation(),
                        )?;
                        Ok(message)
                    }
                    A2aResponse::Task(task) => {
                        let remote_task_id = string_field(&task, "id").map_err(map_effect_error)?;
                        self.put_call(
                            context,
                            "a2a",
                            &message_id,
                            "polling",
                            &serde_json::json!({
                                "peer": peer,
                                "messageId": message_id,
                                "remoteTaskId": remote_task_id,
                                "task": task,
                                "submissionAmbiguous": false,
                            }),
                            client.generation(),
                        )?;
                        let completed = client
                            .wait_for_task_with_events(task.clone(), events, cancellation)
                            .await
                            .map_err(|error| {
                                let status = if matches!(error, ProtocolError::Cancelled) {
                                    "cancellation_requested"
                                } else if ambiguous_protocol_error(&error) {
                                    "uncertain"
                                } else {
                                    "failed"
                                };
                                let _ = self.put_call(
                                    context,
                                    "a2a",
                                    &message_id,
                                    status,
                                    &serde_json::json!({
                                        "peer": peer,
                                        "messageId": message_id,
                                        "remoteTaskId": remote_task_id,
                                        "task": task,
                                        "submissionAmbiguous": false,
                                        "error": error.to_string(),
                                    }),
                                    client.generation(),
                                );
                                map_effect_error(error)
                            })?;
                        self.ingest_a2a_artifacts(context, client, &completed, cancellation)
                            .await?;
                        context.store.put_protocol_session(
                            &context.run_id,
                            &context.task_id,
                            &context.effect_id,
                            "a2a",
                            client.card_config.url.as_str(),
                            client.generation(),
                            "completed",
                            &client.session_state().map_err(map_effect_error)?,
                            Utc::now(),
                        )?;
                        self.put_call(
                            context,
                            "a2a",
                            &message_id,
                            "succeeded",
                            &serde_json::json!({
                                "peer": peer,
                                "messageId": message_id,
                                "remoteTaskId": remote_task_id,
                                "task": completed.clone(),
                                "submissionAmbiguous": false,
                            }),
                            client.generation(),
                        )?;
                        Ok(completed)
                    }
                }
            }
            _ => Err(RuntimeError::InvalidState(
                "protocol handler received a non-protocol action".to_owned(),
            )),
        }
    }

    async fn continue_effect(
        &self,
        context: &ExternalActionContext,
        kind: ActionKind,
        _input: &Value,
        events: &dyn ExternalEventSink,
        cancellation: &CancellationToken,
    ) -> Result<Value, RuntimeError> {
        if kind != ActionKind::A2aDelegate {
            return Err(RuntimeError::InvalidState(
                "MCP calls without a durable remote task ID require explicit effect reconciliation"
                    .to_owned(),
            ));
        }
        let call = context
            .store
            .protocol_call(&context.effect_id)?
            .ok_or_else(|| {
                RuntimeError::InvalidState(format!(
                    "effect `{}` has no durable protocol call",
                    context.effect_id
                ))
            })?;
        if call
            .state
            .get("submissionAmbiguous")
            .and_then(Value::as_bool)
            == Some(true)
        {
            return Err(RuntimeError::InvalidState(
                "A2A submission response was ambiguous; refusing to resubmit without a remote task ID"
                    .to_owned(),
            ));
        }
        let peer = string_field(&call.state, "peer").map_err(map_effect_error)?;
        let task = call.state.get("task").cloned().ok_or_else(|| {
            RuntimeError::InvalidState(
                "A2A protocol call has no persisted remote task state".to_owned(),
            )
        })?;
        let client = self
            .a2a
            .get(&peer)
            .ok_or_else(|| RuntimeError::InvalidState(format!("unknown A2A peer `{peer}`")))?;
        client
            .ensure_discovered(cancellation)
            .await
            .map_err(map_pre_dispatch_error)?;
        context.store.put_protocol_session(
            &context.run_id,
            &context.task_id,
            &context.effect_id,
            "a2a",
            client.card_config.url.as_str(),
            client.generation(),
            "observing",
            &client.session_state().map_err(map_effect_error)?,
            Utc::now(),
        )?;
        let completed = client
            .wait_for_task_with_events(task, events, cancellation)
            .await
            .map_err(map_effect_error)?;
        self.ingest_a2a_artifacts(context, client, &completed, cancellation)
            .await?;
        context.store.put_protocol_session(
            &context.run_id,
            &context.task_id,
            &context.effect_id,
            "a2a",
            client.card_config.url.as_str(),
            client.generation(),
            "completed",
            &client.session_state().map_err(map_effect_error)?,
            Utc::now(),
        )?;
        self.put_call(
            context,
            "a2a",
            &call.call_identity,
            "succeeded",
            &serde_json::json!({
                "peer": peer,
                "messageId": call.call_identity,
                "remoteTaskId": call.state.get("remoteTaskId"),
                "task": completed.clone(),
                "submissionAmbiguous": false,
            }),
            client.generation(),
        )?;
        Ok(completed)
    }
}

impl ProtocolActionHandler {
    fn put_call(
        &self,
        context: &ExternalActionContext,
        protocol: &str,
        call_identity: &str,
        status: &str,
        state: &Value,
        generation: u32,
    ) -> Result<(), RuntimeError> {
        let idempotency = serde_json::to_value(context.idempotency)?
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        context
            .store
            .put_protocol_call(
                &context.effect_id,
                &context.run_id,
                &context.task_id,
                context.task_attempt,
                protocol,
                &context.operation,
                call_identity,
                generation,
                &idempotency,
                status,
                state,
                Utc::now(),
            )
            .map_err(RuntimeError::from)
    }

    async fn ingest_a2a_artifacts(
        &self,
        context: &ExternalActionContext,
        client: &A2aClient,
        task: &Value,
        cancellation: &CancellationToken,
    ) -> Result<(), RuntimeError> {
        let Some(artifacts) = task.get("artifacts").and_then(Value::as_array) else {
            return Ok(());
        };
        for artifact in artifacts {
            let artifact_id = safe_artifact_name(
                artifact
                    .get("artifactId")
                    .and_then(Value::as_str)
                    .unwrap_or("artifact"),
            );
            let parts = artifact
                .get("parts")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RuntimeError::InvalidState(format!("A2A artifact `{artifact_id}` omits parts"))
                })?;
            for (index, part) in parts.iter().enumerate() {
                let content_fields = ["text", "raw", "data", "url"]
                    .into_iter()
                    .filter(|field| part.get(*field).is_some())
                    .count();
                if content_fields != 1 {
                    return Err(RuntimeError::InvalidState(format!(
                        "A2A artifact `{artifact_id}` part {index} must contain exactly one of text, raw, data, or url"
                    )));
                }
                let filename = part
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(safe_artifact_name)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        let extension = artifact_part_extension(part);
                        format!("{artifact_id}-{index}.{extension}")
                    });
                let logical_path = format!("a2a/{artifact_id}/{filename}");
                let bytes = if let Some(text) = part.get("text").and_then(Value::as_str) {
                    text.as_bytes().to_vec()
                } else if let Some(raw) = part.get("raw").and_then(Value::as_str) {
                    base64::engine::general_purpose::STANDARD
                        .decode(raw)
                        .map_err(|error| {
                            RuntimeError::InvalidState(format!(
                                "A2A artifact `{logical_path}` has invalid base64: {error}"
                            ))
                        })?
                } else if let Some(data) = part.get("data") {
                    serde_json::to_vec(data)?
                } else if let Some(url) = part.get("url").and_then(Value::as_str) {
                    client
                        .fetch_artifact_url(url, cancellation)
                        .await
                        .map_err(map_effect_error)?
                } else {
                    return Err(RuntimeError::InvalidState(format!(
                        "A2A artifact `{logical_path}` has no supported part content"
                    )));
                };
                context.store.ingest_artifact_bytes(
                    &context.run_id,
                    &context.task_id,
                    &bytes,
                    &logical_path,
                    16 * 1024 * 1024,
                    Utc::now(),
                )?;
            }
        }
        Ok(())
    }
}

fn safe_artifact_name(value: &str) -> String {
    let name = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("artifact")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        "artifact".to_owned()
    } else {
        name
    }
}

fn artifact_part_extension(part: &Value) -> &'static str {
    match part.get("mediaType").and_then(Value::as_str) {
        Some("application/json") => "json",
        Some("text/markdown") => "md",
        Some("text/csv") => "csv",
        Some("application/pdf") => "pdf",
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/svg+xml") => "svg",
        Some(value) if value.starts_with("text/") => "txt",
        _ if part.get("data").is_some() => "json",
        _ if part.get("text").is_some() => "txt",
        _ => "bin",
    }
}

fn ambiguous_protocol_error(error: &ProtocolError) -> bool {
    matches!(
        error,
        ProtocolError::Timeout(_)
            | ProtocolError::Transport(_)
            | ProtocolError::Malformed(_)
            | ProtocolError::SessionExpired
            | ProtocolError::PollLimit(_)
            | ProtocolError::ReconnectLimit(_)
            | ProtocolError::ContinuationUnavailable(_)
    )
}

fn map_pre_dispatch_error(error: ProtocolError) -> RuntimeError {
    match error {
        ProtocolError::Cancelled => RuntimeError::Cancelled,
        error => RuntimeError::InvalidState(error.to_string()),
    }
}

fn map_effect_error(error: ProtocolError) -> RuntimeError {
    match error {
        ProtocolError::Cancelled => RuntimeError::Cancelled,
        error if ambiguous_protocol_error(&error) => {
            RuntimeError::ExternalEffectUncertain(error.to_string())
        }
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
    headers: &BTreeMap<String, SecretValue>,
    max_response_bytes: usize,
) -> Result<T, ProtocolError> {
    if !response.status().is_success() {
        return Err(http_error(response).await);
    }
    let bytes = bounded_response(response, timeout, cancellation, max_response_bytes).await?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| ProtocolError::Malformed(error.to_string()))?;
    redact_header_secrets(&mut value, headers);
    serde_json::from_value(value).map_err(|error| ProtocolError::Malformed(error.to_string()))
}

async fn response_value(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
    headers: &BTreeMap<String, SecretValue>,
    max_response_bytes: usize,
) -> Result<Value, ProtocolError> {
    response_values(response, timeout, cancellation, headers, max_response_bytes)
        .await?
        .into_iter()
        .last()
        .ok_or_else(|| ProtocolError::Malformed("empty protocol response".to_owned()))
}

async fn response_values(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
    headers: &BTreeMap<String, SecretValue>,
    max_response_bytes: usize,
) -> Result<Vec<Value>, ProtocolError> {
    response_values_with_events(
        response,
        timeout,
        cancellation,
        headers,
        None,
        "",
        max_response_bytes,
    )
    .await
    .map(|(values, _)| values)
}

async fn response_values_with_events(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
    headers: &BTreeMap<String, SecretValue>,
    events: Option<&dyn ExternalEventSink>,
    event_type: &str,
    max_response_bytes: usize,
) -> Result<(Vec<Value>, bool), ProtocolError> {
    if !response.status().is_success() {
        return Err(http_error(response).await);
    }
    let is_sse = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_sse {
        let collect = async move {
            let mut stream = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut total = 0_usize;
            let mut values = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| ProtocolError::Transport(error.to_string()))?;
                total = total.saturating_add(chunk.len());
                let limit = MAX_PROTOCOL_RESPONSE_BYTES.min(max_response_bytes);
                if total > limit {
                    return Err(ProtocolError::Malformed(format!(
                        "response exceeds {limit} bytes"
                    )));
                }
                buffer.extend_from_slice(&chunk);
                while let Some(frame) = take_sse_frame(&mut buffer) {
                    let Some(mut value) = parse_sse_frame(&frame)? else {
                        continue;
                    };
                    redact_header_secrets(&mut value, headers);
                    if let Some(events) = events {
                        events
                            .emit(ExternalStreamEvent {
                                event_type: event_type.to_owned(),
                                remote_sequence: None,
                                payload: value.clone(),
                            })
                            .await
                            .map_err(|error| ProtocolError::Transport(error.to_string()))?;
                    }
                    values.push(value);
                }
            }
            if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
                return Err(ProtocolError::Malformed(
                    "SSE transport ended with a fragmented event".to_owned(),
                ));
            }
            Ok(values)
        };
        tokio::select! {
            result = tokio::time::timeout(timeout, collect) => {
                result
                    .map_err(|_| ProtocolError::Timeout(timeout))?
                    .map(|values| (values, true))
            }
            () = cancellation.cancelled() => Err(ProtocolError::Cancelled),
        }
    } else {
        let bytes = bounded_response(response, timeout, cancellation, max_response_bytes).await?;
        let mut value = serde_json::from_slice(&bytes)
            .map_err(|error| ProtocolError::Malformed(error.to_string()))?;
        redact_header_secrets(&mut value, headers);
        Ok((vec![value], false))
    }
}

fn take_sse_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let (position, delimiter) = match (lf, crlf) {
        (Some(lf), Some(crlf)) if lf <= crlf => (lf, 2),
        (Some(_), Some(crlf)) => (crlf, 4),
        (Some(lf), None) => (lf, 2),
        (None, Some(crlf)) => (crlf, 4),
        (None, None) => return None,
    };
    let frame = buffer[..position].to_vec();
    buffer.drain(..position + delimiter);
    Some(frame)
}

fn parse_sse_frame(frame: &[u8]) -> Result<Option<Value>, ProtocolError> {
    let text = std::str::from_utf8(frame)
        .map_err(|error| ProtocolError::Malformed(format!("SSE encoding: {error}")))?;
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| ProtocolError::Malformed(format!("SSE data: {error}")))
}

fn redact_header_secrets(value: &mut Value, headers: &BTreeMap<String, SecretValue>) {
    match value {
        Value::String(text) => {
            for secret in headers.values().filter(|secret| !secret.is_empty()) {
                *text = text.replace(secret.expose(), "[REDACTED]");
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_header_secrets(value, headers);
            }
        }
        Value::Object(values) => {
            let entries = std::mem::take(values);
            for (name, mut value) in entries {
                redact_header_secrets(&mut value, headers);
                let name = headers
                    .values()
                    .filter(|secret| !secret.is_empty())
                    .fold(name, |name, secret| {
                        name.replace(secret.expose(), "[REDACTED]")
                    });
                values.insert(name, value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

async fn bounded_response(
    response: Response,
    timeout: Duration,
    cancellation: &CancellationToken,
    max_response_bytes: usize,
) -> Result<Vec<u8>, ProtocolError> {
    let collect = async move {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ProtocolError::Transport(error.to_string()))?;
            let limit = MAX_PROTOCOL_RESPONSE_BYTES.min(max_response_bytes);
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(ProtocolError::Malformed(format!(
                    "response exceeds {limit} bytes"
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

fn reconnect_safe(error: &ProtocolError, idempotency: Idempotency) -> bool {
    matches!(
        idempotency,
        Idempotency::Pure | Idempotency::Idempotent | Idempotency::Keyed
    ) && matches!(
        error,
        ProtocolError::SessionExpired
            | ProtocolError::Timeout(_)
            | ProtocolError::Transport(_)
            | ProtocolError::Malformed(_)
    )
}

fn mcp_tool_digest(tool: &McpTool) -> Result<String, ProtocolError> {
    canonical_digest(&serde_json::json!({
        "name": tool.name,
        "inputSchema": tool.input_schema,
        "outputSchema": tool.output_schema,
        "execution": tool.execution,
    }))
}

fn canonical_digest(value: &Value) -> Result<String, ProtocolError> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| ProtocolError::Malformed(error.to_string()))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn task_state(task: &Value) -> Option<&str> {
    task.pointer("/status/state").and_then(Value::as_str)
}

fn normalized_task_state(task: &Value) -> Option<&'static str> {
    match task_state(task)? {
        "completed" | "TASK_STATE_COMPLETED" => Some("completed"),
        "failed" | "TASK_STATE_FAILED" => Some("failed"),
        "rejected" | "TASK_STATE_REJECTED" => Some("rejected"),
        "canceled" | "cancelled" | "TASK_STATE_CANCELED" => Some("canceled"),
        "input_required" | "TASK_STATE_INPUT_REQUIRED" => Some("input_required"),
        "auth_required" | "TASK_STATE_AUTH_REQUIRED" => Some("auth_required"),
        "working" | "submitted" | "TASK_STATE_WORKING" | "TASK_STATE_SUBMITTED" => Some("working"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    #[derive(Default)]
    struct CapturingEvents {
        events: Mutex<Vec<ExternalStreamEvent>>,
    }

    #[async_trait]
    impl ExternalEventSink for CapturingEvents {
        async fn emit(&self, event: ExternalStreamEvent) -> Result<(), RuntimeError> {
            self.events.lock().expect("events").push(event);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RestartingMcp {
        initialize_count: Arc<AtomicUsize>,
        call_count: Arc<AtomicUsize>,
        change_schema: bool,
    }

    #[derive(Clone, Default)]
    struct ContinuingA2a {
        sends: Arc<AtomicUsize>,
        observations: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct RefreshingCard {
        discoveries: Arc<AtomicUsize>,
        origin: String,
    }

    impl Respond for RefreshingCard {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let discovery = self.discoveries.fetch_add(1, Ordering::SeqCst) + 1;
            let interface = if discovery == 1 {
                "a2a-before"
            } else {
                "a2a-after"
            };
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "refreshing-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/{interface}", self.origin),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            }))
        }
    }

    impl Respond for ContinuingA2a {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = request.body_json().expect("JSON-RPC request");
            let method = body.get("method").and_then(Value::as_str).unwrap_or("");
            let id = body.get("id").and_then(Value::as_u64).unwrap_or(0);
            match method {
                "SendMessage" => {
                    self.sends.fetch_add(1, Ordering::SeqCst);
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "task": {
                                "id": "remote-task-1",
                                "status": {"state": "working"}
                            }
                        }
                    }))
                }
                "GetTask" => {
                    let observation = self.observations.fetch_add(1, Ordering::SeqCst) + 1;
                    let task = if observation == 1 {
                        serde_json::json!({
                            "id": "remote-task-1",
                            "status": {"state": "working"}
                        })
                    } else {
                        serde_json::json!({
                            "id": "remote-task-1",
                            "status": {"state": "completed"},
                            "artifacts": [{
                                "artifactId": "report",
                                "parts": [{
                                    "text": "completed without resubmission",
                                    "filename": "report.txt",
                                    "mediaType": "text/plain"
                                }]
                            }]
                        })
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": task
                    }))
                }
                _ => ResponseTemplate::new(400),
            }
        }
    }

    impl RestartingMcp {
        fn new(change_schema: bool) -> Self {
            Self {
                initialize_count: Arc::new(AtomicUsize::new(0)),
                call_count: Arc::new(AtomicUsize::new(0)),
                change_schema,
            }
        }
    }

    impl Respond for RestartingMcp {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = request.body_json().expect("JSON-RPC request");
            let method = body.get("method").and_then(Value::as_str).unwrap_or("");
            let id = body.get("id").and_then(Value::as_u64).unwrap_or(0);
            match method {
                "initialize" => {
                    let generation = self.initialize_count.fetch_add(1, Ordering::SeqCst) + 1;
                    ResponseTemplate::new(200)
                        .insert_header("Mcp-Session-Id", format!("session-{generation}"))
                        .set_body_json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": MCP_PROTOCOL_VERSION,
                                "capabilities": {},
                                "serverInfo": {"name": "restart", "version": "1"}
                            }
                        }))
                }
                "notifications/initialized" => ResponseTemplate::new(202),
                "tools/list" => {
                    let changed =
                        self.change_schema && self.initialize_count.load(Ordering::SeqCst) > 1;
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [{
                                "name": "mutate",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "value": {"type": if changed {"number"} else {"string"}}
                                    }
                                }
                            }]
                        }
                    }))
                }
                "tools/call" => {
                    let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if call == 1 {
                        ResponseTemplate::new(404)
                    } else {
                        ResponseTemplate::new(200).set_body_json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "structuredContent": {"ok": true},
                                "isError": false
                            }
                        }))
                    }
                }
                _ => ResponseTemplate::new(400),
            }
        }
    }

    #[derive(Debug)]
    struct RefreshedSecret;

    #[async_trait]
    impl SecretSourceResolver for RefreshedSecret {
        async fn resolve_secret(
            &self,
            _reference: &SecretReference,
            _cancellation: &CancellationToken,
        ) -> Result<SecretValue, String> {
            Ok(SecretValue::from("Bearer fresh"))
        }
    }

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
            .and(header("authorization", "Bearer fixture"))
            .and(body_partial_json(
                serde_json::json!({"method": "tools/call"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 3,
                "result": {"structuredContent": {"Bearer fixture": "Bearer fixture"}, "isError": false}
            })))
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            headers: BTreeMap::from([("authorization".to_owned(), "Bearer fixture".into())]),
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
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
        assert_eq!(result, serde_json::json!({"[REDACTED]": "[REDACTED]"}));
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
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
        })
        .expect("client");
        assert!(matches!(
            client.initialize(&CancellationToken::new()).await,
            Err(ProtocolError::Version { .. })
        ));
    }

    #[tokio::test]
    async fn mcp_reconnects_once_only_when_declared_safe_and_schema_is_stable() {
        let server = MockServer::start().await;
        let responder = RestartingMcp::new(false);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(responder.clone())
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        ))
        .expect("client");
        let events = CapturingEvents::default();
        let result = client
            .call_tool_with_events(
                "mutate",
                serde_json::json!({"value": "safe"}),
                Idempotency::Idempotent,
                None,
                &events,
                &CancellationToken::new(),
            )
            .await
            .expect("safe reconnect");
        assert_eq!(result, serde_json::json!({"ok": true}));
        assert_eq!(client.generation(), 2);
        assert_eq!(responder.initialize_count.load(Ordering::SeqCst), 2);
        assert_eq!(responder.call_count.load(Ordering::SeqCst), 2);
        assert!(
            events
                .events
                .lock()
                .expect("events")
                .iter()
                .any(|event| event.event_type == "mcp.reconnected")
        );

        let server = MockServer::start().await;
        let responder = RestartingMcp::new(false);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(responder.clone())
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        ))
        .expect("client");
        assert!(matches!(
            client
                .call_tool_with_events(
                    "mutate",
                    serde_json::json!({"value": "unsafe"}),
                    Idempotency::Unknown,
                    None,
                    &CapturingEvents::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(ProtocolError::SessionExpired)
        ));
        assert_eq!(responder.initialize_count.load(Ordering::SeqCst), 1);
        assert_eq!(responder.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mcp_reconnect_refuses_a_changed_tool_schema_before_redispatch() {
        let server = MockServer::start().await;
        let responder = RestartingMcp::new(true);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(responder.clone())
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        ))
        .expect("client");
        assert!(matches!(
            client
                .call_tool_with_events(
                    "mutate",
                    serde_json::json!({"value": "safe"}),
                    Idempotency::Idempotent,
                    None,
                    &CapturingEvents::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(ProtocolError::SchemaChanged { .. })
        ));
        assert_eq!(responder.initialize_count.load(Ordering::SeqCst), 2);
        assert_eq!(responder.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mcp_refreshes_runtime_credentials_once_on_notification_auth_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer stale"))
            .and(body_partial_json(
                serde_json::json!({"method": "initialize"}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "session-1")
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "protocolVersion": MCP_PROTOCOL_VERSION,
                            "capabilities": {},
                            "serverInfo": {"name": "auth", "version": "1"}
                        }
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer stale"))
            .and(body_partial_json(
                serde_json::json!({"method": "notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer fresh"))
            .and(body_partial_json(
                serde_json::json!({"method": "notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", "Bearer fresh"))
            .and(body_partial_json(
                serde_json::json!({"method": "tools/list"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "tools": [{"name": "echo", "inputSchema": {"type": "object"}}]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = McpClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/mcp", server.uri())).expect("url"),
            headers: BTreeMap::from([(
                "authorization".to_owned(),
                SecretValue::from("Bearer stale"),
            )]),
            header_references: BTreeMap::from([(
                "authorization".to_owned(),
                SecretReference::File {
                    file: "/run/secrets/mcp".to_owned(),
                },
            )]),
            header_resolver: Some(Arc::new(RefreshedSecret)),
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
        })
        .expect("client");
        client
            .initialize(&CancellationToken::new())
            .await
            .expect("credential refresh");
        assert_eq!(
            client
                .list_tools(&CancellationToken::new())
                .await
                .unwrap()
                .len(),
            1
        );
        server.verify().await;
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
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
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
    async fn a2a_handler_continues_a_known_task_without_sending_a_second_message() {
        use agentctl_core::compile;
        use agentctl_core::dsl::API_VERSION;
        use agentctl_store::{RunMode, SqliteStore};
        use tempfile::tempdir;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "continuation-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })))
            .mount(&server)
            .await;
        let responder = ContinuingA2a::default();
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(responder.clone())
            .mount(&server)
            .await;
        let client = Arc::new(
            A2aClient::new(ProtocolHttpConfig::fixed(
                Url::parse(&format!("{}/.well-known/agent-card.json", server.uri())).expect("url"),
                BTreeMap::new(),
                Duration::from_secs(2),
            ))
            .expect("client")
            .with_poll_bounds(1, Duration::from_millis(1)),
        );
        let handler =
            ProtocolActionHandler::new(BTreeMap::new(), BTreeMap::from([("local".into(), client)]));
        let workflow = agentctl_core::dsl::parse_workflow(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: protocol-continuation }
spec:
  policy: { approval: never }
  actions:
    delegate: { kind: a2a.delegate }
  tasks:
    - id: delegate
      uses: action:delegate
"#,
            "fixture.yaml",
        )
        .expect("workflow")
        .workflow;
        let plan = compile(&workflow, "fixture.yaml").expect("plan");
        let store = SqliteStore::open_memory().expect("store");
        let directory = tempdir().expect("directory");
        store
            .create_run(
                "run-1",
                API_VERSION,
                &serde_json::to_value(&workflow).expect("workflow value"),
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                RunMode::Execute,
                None,
                None,
                directory.path(),
                Utc::now(),
                "trace-1",
            )
            .expect("run");
        let context = ExternalActionContext {
            run_id: "run-1".to_owned(),
            task_id: "delegate".to_owned(),
            task_attempt: 1,
            effect_id: "effect-1".to_owned(),
            operation: "a2a.delegate".to_owned(),
            idempotency: Idempotency::AtMostOnce,
            store: store.clone(),
        };
        let input = serde_json::json!({
            "peer": "local",
            "messageId": "message-1",
            "message": "perform durable work"
        });
        assert!(matches!(
            handler
                .execute(
                    &context,
                    ActionKind::A2aDelegate,
                    &input,
                    &CapturingEvents::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(RuntimeError::ExternalEffectUncertain(_))
        ));
        let interrupted = store
            .protocol_call("effect-1")
            .expect("call")
            .expect("protocol call");
        assert_eq!(interrupted.status, "uncertain");
        assert_eq!(
            interrupted.state["remoteTaskId"],
            Value::String("remote-task-1".to_owned())
        );
        let completed = handler
            .continue_effect(
                &context,
                ActionKind::A2aDelegate,
                &input,
                &CapturingEvents::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("continue known task");
        assert_eq!(task_state(&completed), Some("completed"));
        assert_eq!(responder.sends.load(Ordering::SeqCst), 1);
        assert_eq!(responder.observations.load(Ordering::SeqCst), 2);
        assert_eq!(
            store
                .protocol_call("effect-1")
                .expect("call")
                .expect("protocol call")
                .status,
            "succeeded"
        );
        let artifacts = store
            .pending_artifacts("run-1", "delegate")
            .expect("artifacts");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].path, "a2a/report/report.txt");
    }

    #[tokio::test]
    async fn a2a_handler_never_resubmits_an_ambiguous_send() {
        use agentctl_core::compile;
        use agentctl_core::dsl::API_VERSION;
        use agentctl_store::{RunMode, SqliteStore};
        use tempfile::tempdir;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "ambiguous-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(
                serde_json::json!({"method": "SendMessage"}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "task": {
                                "id": "remote-task-unknown",
                                "status": {"state": "working"}
                            }
                        }
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = Arc::new(
            A2aClient::new(ProtocolHttpConfig::fixed(
                Url::parse(&format!("{}/.well-known/agent-card.json", server.uri())).expect("url"),
                BTreeMap::new(),
                Duration::from_millis(10),
            ))
            .expect("client"),
        );
        let handler =
            ProtocolActionHandler::new(BTreeMap::new(), BTreeMap::from([("local".into(), client)]));
        let workflow = agentctl_core::dsl::parse_workflow(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: ambiguous-a2a }
spec:
  policy: { approval: never }
  actions:
    delegate: { kind: a2a.delegate }
  tasks:
    - id: delegate
      uses: action:delegate
"#,
            "fixture.yaml",
        )
        .expect("workflow")
        .workflow;
        let plan = compile(&workflow, "fixture.yaml").expect("plan");
        let store = SqliteStore::open_memory().expect("store");
        let directory = tempdir().expect("directory");
        store
            .create_run(
                "run-ambiguous",
                API_VERSION,
                &serde_json::to_value(&workflow).expect("workflow value"),
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                RunMode::Execute,
                None,
                None,
                directory.path(),
                Utc::now(),
                "trace-ambiguous",
            )
            .expect("run");
        let context = ExternalActionContext {
            run_id: "run-ambiguous".to_owned(),
            task_id: "delegate".to_owned(),
            task_attempt: 1,
            effect_id: "effect-ambiguous".to_owned(),
            operation: "a2a.delegate".to_owned(),
            idempotency: Idempotency::AtMostOnce,
            store: store.clone(),
        };
        let input = serde_json::json!({
            "peer": "local",
            "messageId": "message-ambiguous",
            "message": "perform durable work"
        });
        assert!(matches!(
            handler
                .execute(
                    &context,
                    ActionKind::A2aDelegate,
                    &input,
                    &CapturingEvents::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(RuntimeError::ExternalEffectUncertain(_))
        ));
        let call = store
            .protocol_call("effect-ambiguous")
            .expect("call")
            .expect("protocol call");
        assert_eq!(call.status, "uncertain");
        assert_eq!(call.state["submissionAmbiguous"], Value::Bool(true));
        assert!(call.state.get("remoteTaskId").is_none());
        assert!(matches!(
            handler
                .continue_effect(
                    &context,
                    ActionKind::A2aDelegate,
                    &input,
                    &CapturingEvents::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(RuntimeError::InvalidState(_))
        ));
        server.verify().await;
    }

    #[tokio::test]
    async fn a2a_refreshes_the_card_once_and_resumes_observation_on_same_origin() {
        let server = MockServer::start().await;
        let card = RefreshingCard {
            discoveries: Arc::new(AtomicUsize::new(0)),
            origin: server.uri(),
        };
        Mock::given(method("GET"))
            .and(path("/agent-card.json"))
            .respond_with(card.clone())
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a-before"))
            .and(body_partial_json(serde_json::json!({"method": "GetTask"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "id": "remote-task-1",
                            "status": {"state": "working"}
                        }
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a-after"))
            .and(body_partial_json(serde_json::json!({"method": "GetTask"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "id": "remote-task-1",
                    "status": {"state": "completed"}
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/agent-card.json", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_millis(20),
        ))
        .expect("client")
        .with_poll_bounds(1, Duration::from_millis(1));
        client
            .discover(&CancellationToken::new())
            .await
            .expect("discover");
        let events = CapturingEvents::default();
        let completed = client
            .wait_for_task_with_events(
                serde_json::json!({
                    "id": "remote-task-1",
                    "status": {"state": "working"}
                }),
                &events,
                &CancellationToken::new(),
            )
            .await
            .expect("resume after card refresh");
        assert_eq!(task_state(&completed), Some("completed"));
        assert_eq!(client.generation(), 2);
        assert_eq!(card.discoveries.load(Ordering::SeqCst), 2);
        assert!(
            events
                .events
                .lock()
                .expect("events")
                .iter()
                .any(|event| event.event_type == "a2a.card.refreshed")
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn a2a_refreshes_runtime_credentials_once_on_rpc_auth_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/agent-card.json"))
            .and(header("authorization", "Bearer stale"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "auth-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(header("authorization", "Bearer stale"))
            .and(body_partial_json(serde_json::json!({"method": "GetTask"})))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(header("authorization", "Bearer fresh"))
            .and(body_partial_json(serde_json::json!({"method": "GetTask"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "id": "remote-task-1",
                    "status": {"state": "completed"}
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&format!("{}/agent-card.json", server.uri())).expect("url"),
            headers: BTreeMap::from([(
                "authorization".to_owned(),
                SecretValue::from("Bearer stale"),
            )]),
            header_references: BTreeMap::from([(
                "authorization".to_owned(),
                SecretReference::File {
                    file: "/run/secrets/a2a".to_owned(),
                },
            )]),
            header_resolver: Some(Arc::new(RefreshedSecret)),
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
        })
        .expect("client")
        .with_poll_bounds(1, Duration::from_millis(1));
        client
            .discover(&CancellationToken::new())
            .await
            .expect("discover");
        let completed = client
            .wait_for_task(
                serde_json::json!({
                    "id": "remote-task-1",
                    "status": {"state": "working"}
                }),
                &CancellationToken::new(),
            )
            .await
            .expect("refresh RPC credential");
        assert_eq!(task_state(&completed), Some("completed"));
        server.verify().await;
    }

    #[tokio::test]
    async fn a2a_cancellation_attempts_remote_task_cancellation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "cancel-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(
                serde_json::json!({"method": "CancelTask"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "id": "remote-task-1",
                    "status": {"state": "canceled"}
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/agent-card.json", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        ))
        .expect("client")
        .with_poll_bounds(2, Duration::from_secs(1));
        client
            .discover(&CancellationToken::new())
            .await
            .expect("discover");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            client
                .wait_for_task(
                    serde_json::json!({
                        "id": "remote-task-1",
                        "status": {"state": "working"}
                    }),
                    &cancellation,
                )
                .await,
            Err(ProtocolError::Cancelled)
        ));
        server.verify().await;
    }

    #[test]
    fn fragmented_sse_frames_are_buffered_until_the_complete_boundary() {
        let chunks = [
            b"data: {\"jsonrpc\":\"2.0\",\"id\":1,".as_slice(),
            b"\"result\":{\"value\":\"one\"}}\r\n".as_slice(),
            b"\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2".as_slice(),
            b",\"result\":{\"value\":\"two\"}}\n\n".as_slice(),
        ];
        let mut buffer = Vec::new();
        let mut values = Vec::new();
        for chunk in chunks {
            buffer.extend_from_slice(chunk);
            while let Some(frame) = take_sse_frame(&mut buffer) {
                if let Some(value) = parse_sse_frame(&frame).expect("frame") {
                    values.push(value);
                }
            }
        }
        assert!(buffer.is_empty());
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["result"]["value"], "one");
        assert_eq!(values[1]["result"]["value"], "two");
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
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_millis(10),
            transport: HttpTransportSecurity::default(),
        })
        .expect("client");
        assert!(matches!(
            client.discover(&CancellationToken::new()).await,
            Err(ProtocolError::Timeout(_))
        ));
    }

    #[tokio::test]
    async fn protocol_transport_pins_dns_and_bounds_responses() {
        let server = MockServer::start().await;
        let origin = format!("http://agentctl.test:{}", server.address().port());
        Mock::given(method("GET"))
            .and(path("/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "pinned-agent",
                "description": "fixture",
                "supportedInterfaces": [{
                    "url": format!("{origin}/a2a"),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })))
            .mount(&server)
            .await;
        let mut pinned = ProtocolHttpConfig::fixed(
            Url::parse(&format!("{origin}/agent-card.json")).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        );
        pinned.transport.resolved_host = Some("agentctl.test".to_owned());
        pinned.transport.resolved_addresses = vec![*server.address()];
        A2aClient::new(pinned)
            .expect("client")
            .discover(&CancellationToken::new())
            .await
            .expect("pinned discovery");

        Mock::given(method("GET"))
            .and(path("/oversized-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "oversized-agent",
                "description": "x".repeat(1024),
                "supportedInterfaces": [{
                    "url": format!("{}/a2a", server.uri()),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": A2A_PROTOCOL_VERSION
                }]
            })))
            .mount(&server)
            .await;
        let mut oversized = ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/oversized-card.json", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        );
        oversized.transport.max_response_bytes = 128;
        assert!(matches!(
            A2aClient::new(oversized)
                .expect("client")
                .discover(&CancellationToken::new())
                .await,
            Err(ProtocolError::Malformed(message))
                if message.contains("response exceeds 128 bytes")
        ));
    }

    #[tokio::test]
    async fn protocol_transport_rejects_redirects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/agent-card.json", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/agent-card.json"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let client = A2aClient::new(ProtocolHttpConfig::fixed(
            Url::parse(&format!("{}/redirect", server.uri())).expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        ))
        .expect("client");
        assert!(matches!(
            client.discover(&CancellationToken::new()).await,
            Err(ProtocolError::Http { status: 302, .. })
        ));
        server.verify().await;
    }

    #[test]
    fn protocol_transport_rejects_an_invalid_custom_ca() {
        let mut config = ProtocolHttpConfig::fixed(
            Url::parse("https://agentctl.test/a2a").expect("url"),
            BTreeMap::new(),
            Duration::from_secs(2),
        );
        config.transport.custom_ca_pem = Some(SecretValue::from("not a PEM certificate"));
        assert!(matches!(
            A2aClient::new(config),
            Err(ProtocolError::Transport(message))
                if message == "network custom CA PEM is invalid"
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
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
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
            header_references: BTreeMap::new(),
            header_resolver: None,
            timeout: Duration::from_secs(2),
            transport: HttpTransportSecurity::default(),
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
