//! Native provider adapters for agentctl.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use agentctl_core::dsl::{ReasoningEffort, SecretReference};
use agentctl_core::provider::{
    ContentBlock, ContinuationState, FinishReason, Message, ModelProvider, ProviderError,
    ProviderRequest, ProviderResponse, ToolCall, Usage,
};
use agentctl_core::secret::{SecretSourceResolver, SecretValue};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_PROVIDER_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HttpProviderConfig {
    pub endpoint: String,
    pub credential: SecretReference,
    pub resolved_credential: Option<SecretValue>,
    pub credential_resolver: Option<Arc<dyn SecretSourceResolver>>,
    pub organization: Option<String>,
    pub project: Option<String>,
    pub api_version: Option<String>,
    pub headers: BTreeMap<String, SecretValue>,
}

impl HttpProviderConfig {
    #[must_use]
    pub fn openai(credential_env: impl Into<String>) -> Self {
        Self {
            endpoint: "https://api.openai.com/v1/responses".to_owned(),
            credential: SecretReference::environment(credential_env),
            resolved_credential: None,
            credential_resolver: None,
            organization: None,
            project: None,
            api_version: None,
            headers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn anthropic(credential_env: impl Into<String>) -> Self {
        Self {
            endpoint: "https://api.anthropic.com/v1/messages".to_owned(),
            credential: SecretReference::environment(credential_env),
            resolved_credential: None,
            credential_resolver: None,
            organization: None,
            project: None,
            api_version: None,
            headers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn google(credential_env: impl Into<String>) -> Self {
        Self {
            endpoint: "https://generativelanguage.googleapis.com/v1beta/models".to_owned(),
            credential: SecretReference::environment(credential_env),
            resolved_credential: None,
            credential_resolver: None,
            organization: None,
            project: None,
            api_version: None,
            headers: BTreeMap::new(),
        }
    }
}

fn secure_client() -> Result<Client, ProviderError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("agentctl/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| ProviderError::Malformed(error.to_string()))
}

#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    config: HttpProviderConfig,
    azure: bool,
}

impl OpenAiProvider {
    pub fn new(config: HttpProviderConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            client: secure_client()?,
            config,
            azure: false,
        })
    }

    pub fn azure(config: HttpProviderConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            client: secure_client()?,
            config,
            azure: true,
        })
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        if self.azure { "azure_openai" } else { "openai" }
    }

    async fn complete(
        &self,
        request: &ProviderRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let credential = load_credential(&self.config, cancellation).await?;
        let endpoint = if self.azure {
            let separator = if self.config.endpoint.contains('?') {
                '&'
            } else {
                '?'
            };
            format!(
                "{}{separator}api-version={}",
                self.config.endpoint.trim_end_matches('/'),
                self.config.api_version.as_deref().unwrap_or("v1")
            )
        } else {
            self.config.endpoint.clone()
        };
        let mut http = self.client.post(endpoint).json(&openai_request(request)?);
        for (name, value) in &self.config.headers {
            http = http.header(name, value.expose());
        }
        http = if self.azure {
            http.header("api-key", credential.expose())
        } else {
            http.bearer_auth(credential.expose())
        };
        if let Some(organization) = &self.config.organization {
            http = http.header("OpenAI-Organization", organization);
        }
        if let Some(project) = &self.config.project {
            http = http.header("OpenAI-Project", project);
        }
        let secrets = configured_secrets(&credential, &self.config.headers);
        let response = send(http, cancellation, &secrets).await?;
        parse_openai(response)
    }
}

fn openai_request(request: &ProviderRequest) -> Result<Value, ProviderError> {
    let mut body = Map::from_iter([
        ("model".to_owned(), Value::String(request.model.clone())),
        (
            "instructions".to_owned(),
            Value::String(request.instructions.clone()),
        ),
        (
            "max_output_tokens".to_owned(),
            Value::from(request.max_output_tokens),
        ),
        (
            "store".to_owned(),
            Value::Bool(
                request
                    .provider_options
                    .get("store")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            ),
        ),
        ("input".to_owned(), Value::Array(openai_input(request)?)),
    ]);
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_owned(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "type": "function",
                            "name": tool.id,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                            "strict": true,
                        })
                    })
                    .collect(),
            ),
        );
        body.insert(
            "parallel_tool_calls".to_owned(),
            Value::Bool(
                request
                    .provider_options
                    .get("parallelToolCalls")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            ),
        );
    }
    if let Some(reasoning) = &request.reasoning {
        let mut value = serde_json::json!({"effort": reasoning_effort(&reasoning.effort)});
        if let Some(mode) = &reasoning.mode {
            value["mode"] = Value::String(mode.clone());
        }
        if let Some(context) = request
            .provider_options
            .get("reasoningContext")
            .and_then(Value::as_str)
        {
            value["context"] = Value::String(context.to_owned());
        }
        body.insert("reasoning".to_owned(), value);
    }
    if let Some(schema) = &request.structured_output {
        body.insert(
            "text".to_owned(),
            serde_json::json!({
                "format": {
                    "type": "json_schema",
                    "name": "agentctl_output",
                    "schema": schema,
                    "strict": true
                }
            }),
        );
    }
    if let Some(ContinuationState::OpenaiPreviousResponse(id)) = &request.continuation {
        body.insert("previous_response_id".to_owned(), Value::String(id.clone()));
    }
    if let Some(key) = &request.prompt_cache_key {
        body.insert("prompt_cache_key".to_owned(), Value::String(key.clone()));
        let mode = request
            .provider_options
            .get("promptCacheMode")
            .and_then(Value::as_str)
            .unwrap_or("implicit");
        let ttl = request
            .provider_options
            .get("promptCacheTtl")
            .and_then(Value::as_str)
            .unwrap_or("30m");
        body.insert(
            "prompt_cache_options".to_owned(),
            serde_json::json!({"mode": mode, "ttl": ttl}),
        );
    }
    if let Some(identifier) = request.safety_identifier.as_deref().or_else(|| {
        request
            .provider_options
            .get("safetyIdentifier")
            .and_then(Value::as_str)
    }) {
        body.insert(
            "safety_identifier".to_owned(),
            Value::String(identifier.to_owned()),
        );
    }
    Ok(Value::Object(body))
}

fn openai_input(request: &ProviderRequest) -> Result<Vec<Value>, ProviderError> {
    let only_latest_tool_results = matches!(
        request.continuation,
        Some(ContinuationState::OpenaiPreviousResponse(_))
    );
    let messages = if only_latest_tool_results {
        request.messages.last().into_iter().collect::<Vec<_>>()
    } else {
        request.messages.iter().collect::<Vec<_>>()
    };
    let mut output = Vec::new();
    for message in messages {
        match message {
            Message::User(blocks) => {
                let mut text = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::Text { text: value } => text.push(serde_json::json!({
                            "type": "input_text",
                            "text": value
                        })),
                        ContentBlock::ToolResult { id, output: value, .. } => output.push(
                            serde_json::json!({
                                "type": "function_call_output",
                                "call_id": id,
                                "output": serde_json::to_string(value).map_err(|error| ProviderError::Malformed(error.to_string()))?
                            }),
                        ),
                        ContentBlock::ToolCall { .. } | ContentBlock::OpaqueReasoning { .. } => {}
                    }
                }
                if !text.is_empty() {
                    output.push(serde_json::json!({"role": "user", "content": text}));
                }
            }
            Message::Assistant(blocks) if !only_latest_tool_results => {
                let mut text = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::Text { text: value } => text.push(serde_json::json!({
                            "type": "output_text",
                            "text": value
                        })),
                        ContentBlock::ToolCall {
                            id, name, input, ..
                        } => output.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": serde_json::to_string(input).map_err(|error| ProviderError::Malformed(error.to_string()))?
                        })),
                        ContentBlock::OpaqueReasoning { value } => output.push(value.clone()),
                        ContentBlock::ToolResult { .. } => {}
                    }
                }
                if !text.is_empty() {
                    output.push(serde_json::json!({"role": "assistant", "content": text}));
                }
            }
            Message::Assistant(_) => {}
        }
    }
    Ok(output)
}

fn parse_openai(value: Value) -> Result<ProviderResponse, ProviderError> {
    let response_id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut assistant_content = Vec::new();
    let mut refusal = false;
    for item in value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for content in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match content.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            let value = content.get("text").and_then(Value::as_str).unwrap_or("");
                            text.push_str(value);
                            assistant_content.push(ContentBlock::Text {
                                text: value.to_owned(),
                            });
                        }
                        Some("refusal") => refusal = true,
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                let id = required_field(item, "call_id")?;
                let name = required_field(item, "name")?;
                let input: Value = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Malformed("function call missing arguments".to_owned())
                    })
                    .and_then(|raw| {
                        serde_json::from_str(raw).map_err(|error| {
                            ProviderError::Malformed(format!("function arguments: {error}"))
                        })
                    })?;
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                assistant_content.push(ContentBlock::ToolCall {
                    id,
                    name,
                    input,
                    provider_metadata: None,
                });
            }
            Some("reasoning") => assistant_content.push(ContentBlock::OpaqueReasoning {
                value: item.clone(),
            }),
            _ => {}
        }
    }
    let finish_reason = if refusal {
        FinishReason::Refusal
    } else if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else if value.get("status").and_then(Value::as_str) == Some("incomplete") {
        FinishReason::MaxTokens
    } else {
        FinishReason::Complete
    };
    let usage = value.get("usage").cloned().unwrap_or(Value::Null);
    Ok(ProviderResponse {
        continuation: response_id
            .clone()
            .map(ContinuationState::OpenaiPreviousResponse),
        response_id,
        text,
        tool_calls,
        assistant_content,
        usage: Usage {
            input_tokens: number(&usage, "input_tokens"),
            output_tokens: number(&usage, "output_tokens"),
            reasoning_tokens: nested_number(&usage, &["output_tokens_details", "reasoning_tokens"]),
            cache_read_tokens: nested_number(&usage, &["input_tokens_details", "cached_tokens"]),
            cache_write_tokens: nested_number(
                &usage,
                &["input_tokens_details", "cache_write_tokens"],
            )
            .max(number(&usage, "cache_write_tokens")),
            cost_microusd: None,
        },
        finish_reason,
    })
}

#[derive(Clone)]
pub struct AnthropicProvider {
    client: Client,
    config: HttpProviderConfig,
}

impl AnthropicProvider {
    pub fn new(config: HttpProviderConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            client: secure_client()?,
            config,
        })
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(
        &self,
        request: &ProviderRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let credential = load_credential(&self.config, cancellation).await?;
        let http = self
            .client
            .post(&self.config.endpoint)
            .json(&anthropic_request(request)?);
        let http = self
            .config
            .headers
            .iter()
            .fold(http, |request, (name, value)| {
                request.header(name, value.expose())
            });
        let http = http
            .header("x-api-key", credential.expose())
            .header("anthropic-version", ANTHROPIC_VERSION);
        let secrets = configured_secrets(&credential, &self.config.headers);
        let response = send(http, cancellation, &secrets).await?;
        parse_anthropic(response, request)
    }
}

fn anthropic_request(request: &ProviderRequest) -> Result<Value, ProviderError> {
    let mut body = Map::from_iter([
        ("model".to_owned(), Value::String(request.model.clone())),
        (
            "max_tokens".to_owned(),
            Value::from(request.max_output_tokens),
        ),
        (
            "system".to_owned(),
            Value::String(request.instructions.clone()),
        ),
        (
            "messages".to_owned(),
            Value::Array(anthropic_messages(&request.messages)?),
        ),
    ]);
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_owned(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "name": tool.id,
                            "description": tool.description,
                            "input_schema": tool.input_schema,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(reasoning) = &request.reasoning {
        body.insert(
            "output_config".to_owned(),
            serde_json::json!({"effort": reasoning_effort(&reasoning.effort)}),
        );
    }
    if let Some(schema) = &request.structured_output {
        body.entry("output_config".to_owned())
            .or_insert_with(|| Value::Object(Map::new()))["format"] = serde_json::json!({
            "type": "json_schema",
            "schema": schema,
        });
    }
    Ok(Value::Object(body))
}

fn anthropic_messages(messages: &[Message]) -> Result<Vec<Value>, ProviderError> {
    messages
        .iter()
        .map(|message| {
            let (role, blocks) = match message {
                Message::User(blocks) => ("user", blocks),
                Message::Assistant(blocks) => ("assistant", blocks),
            };
            let content: Vec<Value> = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => {
                        Some(serde_json::json!({"type": "text", "text": text}))
                    }
                    ContentBlock::ToolCall {
                        id, name, input, ..
                    } => Some(serde_json::json!({
                        "type": "tool_use", "id": id, "name": name, "input": input
                    })),
                    ContentBlock::ToolResult {
                        id,
                        output,
                        is_error,
                    } => Some(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": serde_json::to_string(output).ok()?,
                        "is_error": is_error,
                    })),
                    ContentBlock::OpaqueReasoning { value } => Some(value.clone()),
                })
                .collect();
            Some(serde_json::json!({"role": role, "content": content}))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ProviderError::Malformed("tool result could not be serialized".to_owned()))
}

fn parse_anthropic(
    value: Value,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderError> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut assistant_content = Vec::new();
    for block in value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block.get("text").and_then(Value::as_str).unwrap_or("");
                text.push_str(value);
                assistant_content.push(ContentBlock::Text {
                    text: value.to_owned(),
                });
            }
            Some("tool_use") => {
                let call = ToolCall {
                    id: required_field(block, "id")?,
                    name: required_field(block, "name")?,
                    input: block.get("input").cloned().unwrap_or(Value::Null),
                };
                assistant_content.push(ContentBlock::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                    provider_metadata: None,
                });
                tool_calls.push(call);
            }
            Some("thinking" | "redacted_thinking") => {
                assistant_content.push(ContentBlock::OpaqueReasoning {
                    value: block.clone(),
                });
            }
            _ => {}
        }
    }
    let finish_reason = match value.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use" | "pause_turn") => FinishReason::ToolCalls,
        Some("max_tokens") => FinishReason::MaxTokens,
        Some("refusal") => FinishReason::Refusal,
        _ => FinishReason::Complete,
    };
    let mut history = request.messages.clone();
    history.push(Message::Assistant(assistant_content.clone()));
    let usage = value.get("usage").cloned().unwrap_or(Value::Null);
    Ok(ProviderResponse {
        response_id: value
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        text,
        tool_calls,
        assistant_content,
        continuation: Some(ContinuationState::Conversation(history)),
        usage: Usage {
            input_tokens: number(&usage, "input_tokens"),
            output_tokens: number(&usage, "output_tokens"),
            reasoning_tokens: 0,
            cache_read_tokens: number(&usage, "cache_read_input_tokens"),
            cache_write_tokens: number(&usage, "cache_creation_input_tokens"),
            cost_microusd: None,
        },
        finish_reason,
    })
}

#[derive(Clone)]
pub struct GoogleProvider {
    client: Client,
    config: HttpProviderConfig,
}

impl GoogleProvider {
    pub fn new(config: HttpProviderConfig) -> Result<Self, ProviderError> {
        Ok(Self {
            client: secure_client()?,
            config,
        })
    }
}

#[async_trait]
impl ModelProvider for GoogleProvider {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn complete(
        &self,
        request: &ProviderRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let credential = load_credential(&self.config, cancellation).await?;
        let endpoint = format!(
            "{}/{}:generateContent",
            self.config.endpoint.trim_end_matches('/'),
            request.model
        );
        let http = self.client.post(endpoint).json(&google_request(request)?);
        let http = self
            .config
            .headers
            .iter()
            .fold(http, |request, (name, value)| {
                request.header(name, value.expose())
            });
        let http = http.header("x-goog-api-key", credential.expose());
        let secrets = configured_secrets(&credential, &self.config.headers);
        let response = send(http, cancellation, &secrets).await?;
        parse_google(response, request)
    }
}

fn google_request(request: &ProviderRequest) -> Result<Value, ProviderError> {
    let mut body = Map::from_iter([
        (
            "systemInstruction".to_owned(),
            serde_json::json!({"parts": [{"text": request.instructions}]}),
        ),
        (
            "contents".to_owned(),
            Value::Array(google_contents(&request.messages)?),
        ),
        (
            "generationConfig".to_owned(),
            serde_json::json!({"maxOutputTokens": request.max_output_tokens}),
        ),
    ]);
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_owned(),
            serde_json::json!([{
                "functionDeclarations": request.tools.iter().map(|tool| serde_json::json!({
                    "name": tool.id,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    "response": tool.output_schema,
                })).collect::<Vec<_>>()
            }]),
        );
    }
    if let Some(schema) = &request.structured_output {
        body["generationConfig"]["responseMimeType"] = Value::String("application/json".to_owned());
        body["generationConfig"]["responseJsonSchema"] = schema.clone();
    }
    if let Some(reasoning) = &request.reasoning {
        body["generationConfig"]["thinkingConfig"] = serde_json::json!({
            "thinkingLevel": reasoning_effort(&reasoning.effort).to_ascii_uppercase()
        });
    }
    Ok(Value::Object(body))
}

fn google_contents(messages: &[Message]) -> Result<Vec<Value>, ProviderError> {
    let tool_names = messages
        .iter()
        .flat_map(|message| match message {
            Message::User(blocks) | Message::Assistant(blocks) => blocks,
        })
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    messages
        .iter()
        .map(|message| {
            let (role, blocks) = match message {
                Message::User(blocks) => ("user", blocks),
                Message::Assistant(blocks) => ("model", blocks),
            };
            let parts = blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => Ok(serde_json::json!({"text": text})),
                    ContentBlock::ToolCall {
                        id,
                        name,
                        input,
                        provider_metadata,
                    } => {
                        let mut part = serde_json::json!({
                            "functionCall": {"id": id, "name": name, "args": input}
                        });
                        if let Some(signature) = provider_metadata
                            .as_ref()
                            .and_then(|metadata| metadata.get("thoughtSignature"))
                        {
                            part["thoughtSignature"] = signature.clone();
                        }
                        Ok(part)
                    }
                    ContentBlock::ToolResult { id, output, .. } => tool_names.get(id).map_or_else(
                        || {
                            Err(ProviderError::Malformed(format!(
                                "Gemini tool result `{id}` has no matching function name"
                            )))
                        },
                        |name| {
                            Ok(serde_json::json!({
                                "functionResponse": {"id": id, "name": name, "response": output}
                            }))
                        },
                    ),
                    ContentBlock::OpaqueReasoning { value } => Ok(value.clone()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::json!({"role": role, "parts": parts}))
        })
        .collect()
}

fn parse_google(
    value: Value,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderError> {
    let candidate = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .ok_or_else(|| ProviderError::Malformed("Gemini response has no candidate".to_owned()))?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut assistant_content = Vec::new();
    let response_scope = value
        .get("responseId")
        .and_then(Value::as_str)
        .unwrap_or("response");
    for part in candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
            assistant_content.push(ContentBlock::Text {
                text: value.to_owned(),
            });
        }
        if let Some(call) = part.get("functionCall") {
            let tool_call = ToolCall {
                id: call.get("id").and_then(Value::as_str).map_or_else(
                    || format!("gemini-{response_scope}-call-{}", tool_calls.len()),
                    ToOwned::to_owned,
                ),
                name: required_field(call, "name")?,
                input: call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            };
            assistant_content.push(ContentBlock::ToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                input: tool_call.input.clone(),
                provider_metadata: part
                    .get("thoughtSignature")
                    .map(|signature| serde_json::json!({"thoughtSignature": signature})),
            });
            tool_calls.push(tool_call);
        }
    }
    let finish_reason = if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else {
        match candidate.get("finishReason").and_then(Value::as_str) {
            Some("MAX_TOKENS") => FinishReason::MaxTokens,
            Some("SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT") => FinishReason::Refusal,
            _ => FinishReason::Complete,
        }
    };
    let mut history = request.messages.clone();
    history.push(Message::Assistant(assistant_content.clone()));
    let usage = value.get("usageMetadata").cloned().unwrap_or(Value::Null);
    Ok(ProviderResponse {
        response_id: value
            .get("responseId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        text,
        tool_calls,
        assistant_content,
        continuation: Some(ContinuationState::Conversation(history)),
        usage: Usage {
            input_tokens: number(&usage, "promptTokenCount"),
            output_tokens: number(&usage, "candidatesTokenCount"),
            reasoning_tokens: number(&usage, "thoughtsTokenCount"),
            cache_read_tokens: number(&usage, "cachedContentTokenCount"),
            cache_write_tokens: 0,
            cost_microusd: None,
        },
        finish_reason,
    })
}

#[derive(Default)]
pub struct FakeProvider {
    script: Mutex<VecDeque<ProviderResponse>>,
    calls: Mutex<u64>,
}

impl FakeProvider {
    #[must_use]
    pub fn scripted(responses: impl IntoIterator<Item = ProviderResponse>) -> Self {
        Self {
            script: Mutex::new(responses.into_iter().collect()),
            calls: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for FakeProvider {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn complete(
        &self,
        request: &ProviderRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let delay_ms = request
            .provider_options
            .get("delayMs")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if delay_ms > 0 {
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                () = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            }
        }
        let call_number = {
            let mut calls = self.calls.lock().map_err(|_| {
                ProviderError::Malformed("fake provider call counter was poisoned".to_owned())
            })?;
            *calls = calls.saturating_add(1);
            *calls
        };
        if call_number
            <= request
                .provider_options
                .get("failFirst")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        {
            return Err(ProviderError::Http {
                status: 503,
                message: "scripted transient failure".to_owned(),
                request_id: format!("fake-{call_number}"),
                retryable: true,
            });
        }
        let scripted = self
            .script
            .lock()
            .map_err(|_| ProviderError::Malformed("fake provider lock was poisoned".to_owned()))?
            .pop_front();
        if let Some(response) = scripted {
            return Ok(response);
        }
        let has_tool_result = request.messages.iter().any(|message| match message {
            Message::User(blocks) | Message::Assistant(blocks) => blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. })),
        });
        if request.provider_options.contains_key("toolInput")
            && !has_tool_result
            && let Some(tool) = request.tools.first()
        {
            let input = request
                .provider_options
                .get("toolInput")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let call = ToolCall {
                id: "fake-call-1".to_owned(),
                name: tool.id.clone(),
                input: input.clone(),
            };
            return Ok(ProviderResponse {
                response_id: Some(format!("fake-response-{call_number}")),
                text: String::new(),
                tool_calls: vec![call],
                assistant_content: vec![ContentBlock::ToolCall {
                    id: "fake-call-1".to_owned(),
                    name: tool.id.clone(),
                    input,
                    provider_metadata: None,
                }],
                continuation: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..Usage::default()
                },
                finish_reason: FinishReason::ToolCalls,
            });
        }
        let text = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(blocks) => blocks.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                Message::Assistant(_) => None,
            })
            .unwrap_or_default();
        let final_text = request
            .provider_options
            .get("finalText")
            .and_then(Value::as_str)
            .map_or_else(|| format!("fake: {text}"), ToOwned::to_owned);
        Ok(ProviderResponse {
            response_id: Some("fake-response".to_owned()),
            text: final_text.clone(),
            tool_calls: Vec::new(),
            assistant_content: vec![ContentBlock::Text { text: final_text }],
            continuation: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                ..Usage::default()
            },
            finish_reason: FinishReason::Complete,
        })
    }
}

async fn send(
    request: reqwest::RequestBuilder,
    cancellation: &CancellationToken,
    secrets: &[&str],
) -> Result<Value, ProviderError> {
    let response = tokio::select! {
        response = request.send() => response.map_err(normalize_transport)?,
        () = cancellation.cancelled() => return Err(ProviderError::Cancelled),
    };
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .or_else(|| response.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unavailable");
    let mut request_id = redact_text(request_id, secrets);
    request_id.truncate(512);
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(normalize_transport)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(ProviderError::Malformed(format!(
                "response exceeds {MAX_PROVIDER_RESPONSE_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut body: Value = serde_json::from_slice(&bytes)
        .map_err(|error| ProviderError::Malformed(error.to_string()))?;
    redact_value(&mut body, secrets);
    if status.is_success() {
        Ok(body)
    } else {
        let message = body
            .pointer("/error/message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("provider request failed");
        let mut safe = redact_text(message, secrets);
        safe.truncate(512);
        Err(ProviderError::Http {
            status: status.as_u16(),
            message: safe,
            request_id,
            retryable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
        })
    }
}

fn configured_secrets<'a>(
    credential: &'a SecretValue,
    headers: &'a BTreeMap<String, SecretValue>,
) -> Vec<&'a str> {
    std::iter::once(credential.expose())
        .chain(headers.values().map(SecretValue::expose))
        .filter(|secret| !secret.is_empty())
        .collect()
}

fn redact_value(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::String(text) => *text = redact_text(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_value(value, secrets);
            }
        }
        Value::Object(values) => {
            let entries = std::mem::take(values);
            for (name, mut value) in entries {
                redact_value(&mut value, secrets);
                values.insert(redact_text(&name, secrets), value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_text(value: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
}

fn normalize_transport(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Http {
            status: 0,
            message: error.to_string(),
            request_id: "unavailable".to_owned(),
            retryable: false,
        }
    }
}

async fn load_credential(
    config: &HttpProviderConfig,
    cancellation: &CancellationToken,
) -> Result<SecretValue, ProviderError> {
    if let Some(credential) = &config.resolved_credential {
        return Ok(credential.clone());
    }
    if let Some(resolver) = &config.credential_resolver {
        return resolver
            .resolve_secret(&config.credential, cancellation)
            .await
            .map_err(ProviderError::Authentication);
    }
    #[cfg(test)]
    if matches!(
        &config.credential,
        SecretReference::Environment { env } if env == "AGENTCTL_PROVIDER_TEST_KEY"
    ) {
        return Ok(SecretValue::from("test-key"));
    }
    match &config.credential {
        SecretReference::Environment { env } => std::env::var(env)
            .map(SecretValue::new)
            .map_err(|_| ProviderError::Authentication(env.clone())),
        reference => Err(ProviderError::Authentication(format!(
            "{} was not resolved by the runtime",
            reference.source_description()
        ))),
    }
}

fn required_field(value: &Value, field: &str) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ProviderError::Malformed(format!("missing string field `{field}`")))
}

fn number(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn nested_number(value: &Value, path: &[&str]) -> u64 {
    path.iter()
        .try_fold(value, |current, field| current.get(*field))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

const fn reasoning_effort(effort: &ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        ReasoningEffort::Max => "max",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentctl_core::dsl::{
        ApprovalRequirement, EffectClass, Idempotency, ReasoningDefinition, ReasoningEffort, Risk,
    };
    use agentctl_core::tool::ToolContract;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request() -> ProviderRequest {
        ProviderRequest {
            model: "test-model".to_owned(),
            instructions: "Be concise.".to_owned(),
            messages: vec![Message::User(vec![ContentBlock::Text {
                text: "hello".to_owned(),
            }])],
            tools: vec![ToolContract {
                id: "echo".to_owned(),
                description: "Echo input".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                    "additionalProperties": false
                }),
                output_schema: serde_json::json!({"type": "object"}),
                capability: "observe".to_owned(),
                risk: Risk::Low,
                effect_class: EffectClass::Pure,
                idempotency: Idempotency::Pure,
                retry_safe: true,
                timeout_seconds: 5,
                secret_requirements: Vec::new(),
                network_requirements: Vec::new(),
                approval: ApprovalRequirement::Never,
                observability: Value::Null,
            }],
            max_output_tokens: 64,
            reasoning: None,
            structured_output: None,
            continuation: None,
            prompt_cache_key: Some("cache-key".to_owned()),
            safety_identifier: Some("user-hash".to_owned()),
            provider_options: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn fake_provider_is_deterministic() {
        let response = FakeProvider::default()
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("fake response");
        assert_eq!(response.text, "fake: hello");
        assert_eq!(response.usage.input_tokens, 1);
    }

    #[test]
    fn openai_maps_explicit_gpt56_options_without_silent_fallback() {
        let mut request = request();
        request.structured_output = Some(serde_json::json!({
            "type": "object",
            "properties": {"verdict": {"type": "string"}},
            "required": ["verdict"],
            "additionalProperties": false
        }));
        request.reasoning = Some(ReasoningDefinition {
            effort: ReasoningEffort::Max,
            mode: Some("pro".to_owned()),
        });
        request.provider_options = BTreeMap::from([
            ("store".to_owned(), Value::Bool(false)),
            (
                "reasoningContext".to_owned(),
                Value::String("all_turns".to_owned()),
            ),
            (
                "promptCacheMode".to_owned(),
                Value::String("explicit".to_owned()),
            ),
            ("promptCacheTtl".to_owned(), Value::String("30m".to_owned())),
            ("parallelToolCalls".to_owned(), Value::Bool(false)),
        ]);
        let body = openai_request(&request).expect("request mapping");
        assert_eq!(body["store"], false);
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["reasoning"]["mode"], "pro");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        assert_eq!(body["prompt_cache_options"]["mode"], "explicit");
        assert_eq!(body["prompt_cache_options"]["ttl"], "30m");
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["text"]["format"]["strict"], true);
    }

    #[test]
    fn openai_preserves_multiple_function_call_ids() {
        let response = parse_openai(serde_json::json!({
            "id": "resp_tools",
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call-a", "name": "echo", "arguments": "{\"text\":\"a\"}"},
                {"type": "function_call", "call_id": "call-b", "name": "echo", "arguments": "{\"text\":\"b\"}"}
            ]
        }))
        .expect("valid multiple function calls");
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].id, "call-a");
        assert_eq!(response.tool_calls[1].id, "call-b");
        assert_eq!(
            response.continuation,
            Some(ContinuationState::OpenaiPreviousResponse(
                "resp_tools".to_owned()
            ))
        );
    }

    #[tokio::test]
    async fn openai_maps_responses_api_and_usage() {
        let server = MockServer::start().await;
        let expected = openai_request(&request()).expect("request mapping");
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_1",
                "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "input_tokens_details": {"cached_tokens": 3},
                    "output_tokens_details": {"reasoning_tokens": 2},
                    "cache_write_tokens": 5
                }
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/responses", server.uri());
        let response = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        assert_eq!(response.text, "ok");
        assert_eq!(response.usage.cache_read_tokens, 3);
        assert_eq!(response.usage.cache_write_tokens, 5);
        assert_eq!(response.usage.reasoning_tokens, 2);
    }

    #[tokio::test]
    async fn anthropic_maps_native_tool_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_1",
                "content": [{"type": "tool_use", "id": "toolu_1", "name": "echo", "input": {"text": "hello"}}],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 8, "output_tokens": 3, "cache_read_input_tokens": 2, "cache_creation_input_tokens": 1}
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::anthropic("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/messages", server.uri());
        let response = AnthropicProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert_eq!(response.tool_calls[0].id, "toolu_1");
        assert_eq!(response.usage.cache_write_tokens, 1);
    }

    #[tokio::test]
    async fn google_maps_native_generate_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/models/test-model:generateContent"))
            .and(header("x-goog-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "responseId": "gemini_1",
                "candidates": [{"content": {"parts": [{"text": "ok"}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 2, "thoughtsTokenCount": 1, "cachedContentTokenCount": 4}
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::google("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/models", server.uri());
        let response = GoogleProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        assert_eq!(response.text, "ok");
        assert_eq!(response.usage.reasoning_tokens, 1);
        assert_eq!(response.usage.cache_read_tokens, 4);
    }

    #[test]
    fn google_preserves_function_identity_and_thought_signature_on_continuation() {
        let response = parse_google(
            serde_json::json!({
                "responseId": "gemini_tools",
                "candidates": [{
                    "content": {"parts": [{
                        "functionCall": {"id": "call-7", "name": "echo", "args": {"text": "hello"}},
                        "thoughtSignature": "encrypted-signature"
                    }]},
                    "finishReason": "STOP"
                }]
            }),
            &request(),
        )
        .expect("Gemini tool response");
        let mut messages = request().messages;
        messages.push(Message::Assistant(response.assistant_content));
        messages.push(Message::User(vec![ContentBlock::ToolResult {
            id: "call-7".to_owned(),
            output: serde_json::json!({"text": "hello"}),
            is_error: false,
        }]));

        let contents = google_contents(&messages).expect("Gemini continuation");
        assert_eq!(
            contents[1].pointer("/parts/0/thoughtSignature"),
            Some(&Value::String("encrypted-signature".to_owned()))
        );
        assert_eq!(
            contents[2].pointer("/parts/0/functionResponse/id"),
            Some(&Value::String("call-7".to_owned()))
        );
        assert_eq!(
            contents[2].pointer("/parts/0/functionResponse/name"),
            Some(&Value::String("echo".to_owned()))
        );
    }

    #[test]
    fn anthropic_preserves_thinking_blocks_on_continuation() {
        let response = parse_anthropic(
            serde_json::json!({
                "id": "msg-thinking",
                "content": [
                    {"type": "thinking", "thinking": "hidden", "signature": "signed"},
                    {"type": "tool_use", "id": "toolu-1", "name": "echo", "input": {"text": "hello"}}
                ],
                "stop_reason": "tool_use"
            }),
            &request(),
        )
        .expect("Anthropic tool response");
        let message = Message::Assistant(response.assistant_content);
        let mapped = anthropic_messages(&[message]).expect("Anthropic continuation");
        assert_eq!(
            mapped[0].pointer("/content/0/type"),
            Some(&Value::String("thinking".to_owned()))
        );
        assert_eq!(
            mapped[0].pointer("/content/0/signature"),
            Some(&Value::String("signed".to_owned()))
        );
    }

    #[tokio::test]
    async fn azure_uses_api_key_and_v1_responses_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/openai/v1/responses"))
            .and(header("api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_azure",
                "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "azure-ok"}]}],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })))
            .mount(&server)
            .await;
        let config = HttpProviderConfig {
            endpoint: format!("{}/openai/v1/responses", server.uri()),
            credential: SecretReference::environment("AGENTCTL_PROVIDER_TEST_KEY"),
            resolved_credential: None,
            credential_resolver: None,
            organization: None,
            project: None,
            api_version: Some("v1".to_owned()),
            headers: BTreeMap::new(),
        };
        let response = OpenAiProvider::azure(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        assert_eq!(response.text, "azure-ok");
    }

    #[tokio::test]
    async fn provider_authentication_errors_are_redacted_and_not_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("x-request-id", "request-header-secret")
                    .set_body_json(serde_json::json!({
                        "error": {"message": "invalid credentials test-key and header-secret"}
                    })),
            )
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/responses", server.uri());
        config
            .headers
            .insert("x-custom-auth".to_owned(), "header-secret".into());
        let error = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect_err("authentication failure");
        match error {
            ProviderError::Http {
                status,
                message,
                request_id,
                retryable,
            } => {
                assert_eq!(status, 401);
                assert_eq!(message, "invalid credentials [REDACTED] and [REDACTED]");
                assert_eq!(request_id, "request-[REDACTED]");
                assert!(!retryable);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn provider_success_payloads_cannot_echo_configured_header_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("x-custom-auth", "header-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_header-secret",
                "status": "completed",
                "output": [
                    {"type": "reasoning", "header-secret": "test-key"},
                    {"type": "message", "content": [{"type": "output_text", "text": "echo header-secret and test-key"}]}
                ]
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/responses", server.uri());
        config
            .headers
            .insert("x-custom-auth".to_owned(), "header-secret".into());
        let response = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        let serialized = serde_json::to_string(&response).expect("serialized response");
        assert!(!serialized.contains("header-secret"));
        assert!(!serialized.contains("test-key"));
        assert_eq!(response.text, "echo [REDACTED] and [REDACTED]");
    }

    #[tokio::test]
    async fn runtime_resolved_non_environment_credential_is_used_and_redacted() {
        #[derive(Debug)]
        struct FixtureSecretResolver;

        #[async_trait]
        impl SecretSourceResolver for FixtureSecretResolver {
            async fn resolve_secret(
                &self,
                _reference: &SecretReference,
                _cancellation: &CancellationToken,
            ) -> Result<SecretValue, String> {
                Ok(SecretValue::from("resolved-file-secret"))
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer resolved-file-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_resolved-file-secret",
                "status": "completed",
                "output": [
                    {"type": "message", "content": [{"type": "output_text", "text": "resolved-file-secret"}]}
                ]
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("unused");
        config.endpoint = format!("{}/v1/responses", server.uri());
        config.credential = SecretReference::File {
            file: "/run/secrets/openai".to_owned(),
        };
        config.credential_resolver = Some(Arc::new(FixtureSecretResolver));
        let response = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect("response");
        assert_eq!(response.text, "[REDACTED]");
        assert!(
            !serde_json::to_string(&response)
                .expect("response json")
                .contains("resolved-file-secret")
        );
    }

    #[tokio::test]
    async fn rate_limits_are_explicitly_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": {"message": "rate limited"}
            })))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/responses", server.uri());
        let error = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect_err("rate limit");
        assert!(matches!(
            error,
            ProviderError::Http {
                status: 429,
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn malformed_success_responses_fail_explicitly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("not-json", "text/plain"))
            .mount(&server)
            .await;
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = format!("{}/v1/responses", server.uri());
        let error = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &CancellationToken::new())
            .await
            .expect_err("malformed response");
        assert!(matches!(error, ProviderError::Malformed(_)));
    }

    #[tokio::test]
    async fn cancellation_is_normalized() {
        let token = CancellationToken::new();
        token.cancel();
        let mut config = HttpProviderConfig::openai("AGENTCTL_PROVIDER_TEST_KEY");
        config.endpoint = "http://127.0.0.1:9/v1/responses".to_owned();
        let result = OpenAiProvider::new(config)
            .expect("provider")
            .complete(&request(), &token)
            .await;
        assert!(matches!(result, Err(ProviderError::Cancelled)));
    }
}
