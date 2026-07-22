use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::dsl::{ApprovalRequirement, EffectClass, Idempotency, Risk, SecretReference};
use crate::effect::ActionResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolContract {
    pub id: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capability: String,
    pub risk: Risk,
    pub effect_class: EffectClass,
    pub idempotency: Idempotency,
    pub retry_safe: bool,
    pub timeout_seconds: u64,
    pub secret_requirements: Vec<SecretReference>,
    pub network_requirements: Vec<String>,
    pub approval: ApprovalRequirement,
    pub observability: Value,
    pub compensation: Option<String>,
}

impl ToolContract {
    pub fn validate_input(&self, input: &Value) -> Result<(), ToolContractError> {
        validate_schema(&self.input_schema, input, "input")
    }

    pub fn validate_output(&self, output: &Value) -> Result<(), ToolContractError> {
        validate_schema(&self.output_schema, output, "output")
    }
}

fn validate_schema(
    schema: &Value,
    instance: &Value,
    direction: &str,
) -> Result<(), ToolContractError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| ToolContractError::InvalidSchema(error.to_string()))?;
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ToolContractError::Validation {
            direction: direction.to_owned(),
            errors,
        })
    }
}

#[derive(Debug, Error)]
pub enum ToolContractError {
    #[error("invalid JSON Schema: {0}")]
    InvalidSchema(String),
    #[error("tool {direction} failed schema validation: {errors:?}")]
    Validation {
        direction: String,
        errors: Vec<String>,
    },
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("tool execution was cancelled")]
    Cancelled,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn contract(&self) -> &ToolContract;
    async fn execute(
        &self,
        input: Value,
        cancellation: &CancellationToken,
    ) -> Result<ActionResult, ToolContractError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{ApprovalRequirement, EffectClass, Idempotency, Risk};

    fn contract() -> ToolContract {
        ToolContract {
            id: "example.echo".to_owned(),
            description: "Echo text".to_owned(),
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
            compensation: None,
        }
    }

    #[test]
    fn rejects_malformed_input_and_output() {
        let contract = contract();
        assert!(
            contract
                .validate_input(&serde_json::json!({"text": "ok"}))
                .is_ok()
        );
        assert!(
            contract
                .validate_input(&serde_json::json!({"text": 4}))
                .is_err()
        );
        assert!(
            contract
                .validate_input(&serde_json::json!({"text": "ok", "extra": true}))
                .is_err()
        );
        assert!(
            contract
                .validate_output(&Value::String("wrong".to_owned()))
                .is_err()
        );
    }
}
