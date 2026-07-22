use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::EFFECT_FORMAT_VERSION;
use crate::dsl::{EffectClass, Idempotency, Risk};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Requested,
    WaitingForApproval,
    Started,
    Succeeded,
    Failed,
    Cancelled,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectRequest {
    pub format_version: u32,
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub attempt: u16,
    pub ordinal: u16,
    pub operation: String,
    pub effect_class: EffectClass,
    pub risk: Risk,
    pub idempotency: Idempotency,
    pub idempotency_key: String,
    pub input_digest: String,
    pub input: Value,
    pub expected_effect: String,
    pub trace_id: String,
}

impl EffectRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: &str,
        task_id: &str,
        attempt: u16,
        ordinal: u16,
        operation: &str,
        effect_class: EffectClass,
        risk: Risk,
        idempotency: Idempotency,
        input: Value,
        expected_effect: &str,
        trace_id: &str,
    ) -> Self {
        let serialized = serde_json::to_vec(&input).unwrap_or_default();
        let input_digest = hex::encode(Sha256::digest(serialized));
        let identity =
            format!("{run_id}\0{task_id}\0{attempt}\0{ordinal}\0{operation}\0{input_digest}");
        let id = hex::encode(Sha256::digest(identity.as_bytes()));
        Self {
            format_version: EFFECT_FORMAT_VERSION,
            idempotency_key: id.clone(),
            id,
            run_id: run_id.to_owned(),
            task_id: task_id.to_owned(),
            attempt,
            ordinal,
            operation: operation.to_owned(),
            effect_class,
            risk,
            idempotency,
            input_digest,
            input,
            expected_effect: expected_effect.to_owned(),
            trace_id: trace_id.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectRecord {
    pub request: EffectRequest,
    pub status: EffectStatus,
    pub attempt_number: u16,
    pub requested_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub status: ChangeStatus,
    pub changed: bool,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub diff: Option<String>,
    pub output: Value,
    pub predictability: crate::compiler::PlanPredictability,
}

impl ActionResult {
    #[must_use]
    pub fn unchanged(output: Value) -> Self {
        Self {
            status: ChangeStatus::Unchanged,
            changed: false,
            before: None,
            after: None,
            diff: None,
            output,
            predictability: crate::compiler::PlanPredictability::FullyPredictable,
        }
    }

    #[must_use]
    pub fn changed(output: Value) -> Self {
        Self {
            status: ChangeStatus::Changed,
            changed: true,
            before: None,
            after: None,
            diff: None,
            output,
            predictability: crate::compiler::PlanPredictability::FullyPredictable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Changed,
    Unchanged,
    Skipped,
    Failed,
}
