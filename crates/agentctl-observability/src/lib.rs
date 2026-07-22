//! Optional, redacted observability contracts for agentctl.

use std::sync::Mutex;

use agentctl_core::policy::redact;
use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use opentelemetry::trace::{Span, Tracer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TRACE_EVENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Run,
    Task,
    Attempt,
    AgentTurn,
    ProviderRequest,
    ModelResponse,
    ToolCall,
    Effect,
    Approval,
    McpRequest,
    A2aDelegation,
    Retry,
    Checkpoint,
    StateTransition,
    Database,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePhase {
    Started,
    Completed,
    Failed,
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    pub version: u32,
    pub kind: SpanKind,
    pub phase: TracePhase,
    pub name: String,
    pub trace_id: String,
    pub run_id: String,
    pub task_id: Option<String>,
    pub effect_id: Option<String>,
    pub attributes: Value,
    pub timestamp: DateTime<Utc>,
}

impl TraceEvent {
    #[must_use]
    pub fn new(
        kind: SpanKind,
        phase: TracePhase,
        name: impl Into<String>,
        trace_id: impl Into<String>,
        run_id: impl Into<String>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            version: TRACE_EVENT_VERSION,
            kind,
            phase,
            name: name.into(),
            trace_id: trace_id.into(),
            run_id: run_id.into(),
            task_id: None,
            effect_id: None,
            attributes: Value::Null,
            timestamp,
        }
    }

    #[must_use]
    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    #[must_use]
    pub fn effect(mut self, effect_id: impl Into<String>) -> Self {
        self.effect_id = Some(effect_id.into());
        self
    }

    #[must_use]
    pub fn attributes(mut self, attributes: Value, secrets: &[String]) -> Self {
        self.attributes = redact(&attributes, secrets);
        self
    }
}

pub trait TraceSink: Send + Sync {
    fn record(&self, event: &TraceEvent);
}

#[derive(Debug, Default)]
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn record(&self, _event: &TraceEvent) {}
}

/// Emits OpenTelemetry spans through the process-global tracer provider.
#[derive(Debug, Default)]
pub struct OpenTelemetrySink;

impl TraceSink for OpenTelemetrySink {
    fn record(&self, event: &TraceEvent) {
        let tracer = opentelemetry::global::tracer("agentctl");
        let mut span = tracer.start(event.name.clone());
        span.set_attribute(KeyValue::new("agentctl.trace_id", event.trace_id.clone()));
        span.set_attribute(KeyValue::new("agentctl.run_id", event.run_id.clone()));
        span.set_attribute(KeyValue::new("agentctl.kind", format!("{:?}", event.kind)));
        span.set_attribute(KeyValue::new(
            "agentctl.phase",
            format!("{:?}", event.phase),
        ));
        if let Some(task_id) = &event.task_id {
            span.set_attribute(KeyValue::new("agentctl.task_id", task_id.clone()));
        }
        if let Some(effect_id) = &event.effect_id {
            span.set_attribute(KeyValue::new("agentctl.effect_id", effect_id.clone()));
        }
        span.end();
    }
}

#[derive(Debug, Default)]
pub struct BufferedTraceSink {
    events: Mutex<Vec<TraceEvent>>,
}

impl BufferedTraceSink {
    #[must_use]
    pub fn events(&self) -> Vec<TraceEvent> {
        self.events
            .lock()
            .map_or_else(|_| Vec::new(), |events| events.clone())
    }
}

impl TraceSink for BufferedTraceSink {
    fn record(&self, event: &TraceEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_attributes_are_redacted_before_reaching_sink() {
        let event = TraceEvent::new(
            SpanKind::ProviderRequest,
            TracePhase::Started,
            "provider.request",
            "trace",
            "run",
            Utc::now(),
        )
        .attributes(
            serde_json::json!({"authorization": "Bearer key", "text": "contains key"}),
            &["key".to_owned()],
        );
        let serialized = serde_json::to_string(&event).expect("serialize");
        assert!(!serialized.contains("key"));
        assert!(serialized.contains("[REDACTED]"));
    }
}
