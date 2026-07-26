use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::dsl::ReasoningDefinition;
use crate::tool::ToolContract;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequest {
    pub model: String,
    pub instructions: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolContract>,
    pub max_output_tokens: u32,
    pub reasoning: Option<ReasoningDefinition>,
    pub structured_output: Option<Value>,
    pub continuation: Option<ContinuationState>,
    pub prompt_cache_key: Option<String>,
    pub safety_identifier: Option<String>,
    #[serde(default)]
    pub provider_options: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "role", content = "content")]
pub enum Message {
    User(Vec<ContentBlock>),
    Assistant(Vec<ContentBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<Value>,
    },
    ToolResult {
        id: String,
        output: Value,
        is_error: bool,
    },
    OpaqueReasoning {
        value: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResponse {
    pub response_id: Option<String>,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub assistant_content: Vec<ContentBlock>,
    pub continuation: Option<ContinuationState>,
    pub usage: Usage,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStreamEvent {
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_sequence: Option<u64>,
    pub payload: Value,
}

#[async_trait]
pub trait ProviderStreamSink: Send + Sync {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ContinuationState {
    OpenaiPreviousResponse(String),
    Conversation(Vec<Message>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_microusd: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Complete,
    ToolCalls,
    MaxTokens,
    Refusal,
    Cancelled,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider authentication is unavailable: {0}")]
    Authentication(String),
    #[error("provider capability is unsupported: {0}")]
    Unsupported(String),
    #[error("provider request timed out")]
    Timeout,
    #[error("provider request was cancelled")]
    Cancelled,
    #[error("provider returned HTTP {status}: {message} (request id: {request_id})")]
    Http {
        status: u16,
        message: String,
        request_id: String,
        retryable: bool,
    },
    #[error("provider response was malformed: {0}")]
    Malformed(String),
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn complete(
        &self,
        request: &ProviderRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError>;

    async fn complete_streaming(
        &self,
        _request: &ProviderRequest,
        _sink: &dyn ProviderStreamSink,
        _cancellation: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::Unsupported(format!(
            "{} does not support streaming",
            self.name()
        )))
    }
}
