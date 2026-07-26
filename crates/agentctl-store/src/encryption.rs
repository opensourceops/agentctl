use std::sync::Arc;

use aes_gcm::aead::array::Array;
use aes_gcm::aead::{Aead, Generate, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::StoreError;

pub const ENCRYPTION_FORMAT_VERSION: u32 = 1;
pub const ENCRYPTION_ALGORITHM: &str = "AES-256-GCM";
pub(crate) const ENVELOPE_PREFIX: &str = "agentctl.encrypted.v1:";
const KEY_CHECK_VALUE: &str = "agentctl-state-key-check-v1";
pub const KEY_CHECK_CONTEXT: &str = "state_encryption.key_check";

#[derive(Debug, Clone, Copy)]
pub(crate) struct SensitiveColumn {
    pub table: &'static str,
    pub column: &'static str,
}

impl SensitiveColumn {
    pub fn context(self) -> String {
        format!("{}.{}", self.table, self.column)
    }
}

pub(crate) const SENSITIVE_COLUMNS: &[SensitiveColumn] = &[
    SensitiveColumn {
        table: "runs",
        column: "workflow_json",
    },
    SensitiveColumn {
        table: "runs",
        column: "plan_json",
    },
    SensitiveColumn {
        table: "runs",
        column: "inputs_json",
    },
    SensitiveColumn {
        table: "runs",
        column: "working_memory_json",
    },
    SensitiveColumn {
        table: "runs",
        column: "output_json",
    },
    SensitiveColumn {
        table: "runs",
        column: "repair_reason",
    },
    SensitiveColumn {
        table: "runs",
        column: "retry_reason",
    },
    SensitiveColumn {
        table: "task_states",
        column: "output_json",
    },
    SensitiveColumn {
        table: "task_states",
        column: "error",
    },
    SensitiveColumn {
        table: "task_states",
        column: "state_delta_json",
    },
    SensitiveColumn {
        table: "task_states",
        column: "reuse_decision_json",
    },
    SensitiveColumn {
        table: "task_states",
        column: "execution_memory_json",
    },
    SensitiveColumn {
        table: "effects",
        column: "input_json",
    },
    SensitiveColumn {
        table: "effects",
        column: "expected_effect",
    },
    SensitiveColumn {
        table: "effects",
        column: "result_json",
    },
    SensitiveColumn {
        table: "effects",
        column: "error",
    },
    SensitiveColumn {
        table: "approvals",
        column: "redacted_input_json",
    },
    SensitiveColumn {
        table: "approvals",
        column: "expected_effect",
    },
    SensitiveColumn {
        table: "approvals",
        column: "reason",
    },
    SensitiveColumn {
        table: "approvals",
        column: "resolution_reason",
    },
    SensitiveColumn {
        table: "checkpoints",
        column: "state_json",
    },
    SensitiveColumn {
        table: "audit_events",
        column: "payload_json",
    },
    SensitiveColumn {
        table: "provider_sessions",
        column: "continuation_json",
    },
    SensitiveColumn {
        table: "stream_events",
        column: "payload_json",
    },
    SensitiveColumn {
        table: "protocol_sessions",
        column: "state_json",
    },
    SensitiveColumn {
        table: "protocol_calls",
        column: "state_json",
    },
    SensitiveColumn {
        table: "long_term_memory",
        column: "value_json",
    },
    SensitiveColumn {
        table: "trace_events",
        column: "event_json",
    },
    SensitiveColumn {
        table: "run_upgrades",
        column: "analysis_json",
    },
    SensitiveColumn {
        table: "run_upgrades",
        column: "upgraded_tasks_json",
    },
    SensitiveColumn {
        table: "effect_reconciliations",
        column: "reason",
    },
    SensitiveColumn {
        table: "effect_reconciliations",
        column: "evidence_json",
    },
    SensitiveColumn {
        table: "effect_reconciliations",
        column: "result_json",
    },
    SensitiveColumn {
        table: "effect_reconciliations",
        column: "result_schema_json",
    },
    SensitiveColumn {
        table: "effect_reconciliations",
        column: "authorization_json",
    },
];

pub trait StateKeyResolver: Send + Sync {
    /// Resolve a key reference to exactly 32 raw bytes.
    ///
    /// Implementations must not persist or log the returned value.
    fn resolve(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, StoreError>;
}

#[derive(Debug, Default)]
pub struct EnvironmentKeyResolver;

impl StateKeyResolver for EnvironmentKeyResolver {
    fn resolve(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        validate_environment_reference(reference)?;
        let encoded = Zeroizing::new(std::env::var(reference).map_err(|_| {
            StoreError::Encryption(format!(
                "state-encryption key environment reference `{reference}` is unavailable"
            ))
        })?);
        let decoded = STANDARD.decode(encoded.as_bytes()).map_err(|_| {
            StoreError::Encryption(format!(
                "state-encryption key from `{reference}` must be base64"
            ))
        })?;
        validate_key_bytes(reference, decoded)
    }
}

#[derive(Clone)]
pub(crate) enum StateProtection {
    Plaintext,
    Encrypted(EncryptionCodec),
}

impl StateProtection {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Encrypted(_))
    }

    pub fn protect(&self, plaintext: &str, context: &str) -> Result<String, StoreError> {
        match self {
            Self::Plaintext => {
                if is_encrypted_value(plaintext) {
                    return Err(StoreError::Encryption(format!(
                        "encrypted value for `{context}` has no configured state key"
                    )));
                }
                Ok(plaintext.to_owned())
            }
            Self::Encrypted(codec) => codec.encrypt(plaintext, context),
        }
    }

    pub fn expose(&self, stored: &str, context: &str) -> Result<String, StoreError> {
        match self {
            Self::Plaintext => {
                if is_encrypted_value(stored) {
                    return Err(StoreError::Encryption(format!(
                        "encrypted value for `{context}` has no configured state key"
                    )));
                }
                Ok(stored.to_owned())
            }
            Self::Encrypted(codec) => {
                if !is_encrypted_value(stored) {
                    return Err(StoreError::Encryption(format!(
                        "plaintext value found in protected field `{context}`"
                    )));
                }
                codec.decrypt(stored, context)
            }
        }
    }
}

pub(crate) type SharedStateProtection = Arc<RwLock<StateProtection>>;

#[derive(Clone)]
pub(crate) struct EncryptionCodec {
    key_id: String,
    key: Zeroizing<Vec<u8>>,
}

impl EncryptionCodec {
    pub fn resolve(
        key_id: &str,
        key_reference: &str,
        resolver: &dyn StateKeyResolver,
    ) -> Result<Self, StoreError> {
        validate_key_id(key_id)?;
        let key = resolver.resolve(key_reference)?;
        if key.len() != 32 {
            return Err(StoreError::Encryption(format!(
                "state-encryption key from `{key_reference}` must decode to exactly 32 bytes"
            )));
        }
        Ok(Self {
            key_id: key_id.to_owned(),
            key,
        })
    }

    #[cfg(test)]
    pub fn from_bytes(key_id: &str, key: Vec<u8>) -> Result<Self, StoreError> {
        validate_key_id(key_id)?;
        let key = validate_key_bytes("test key", key)?;
        Ok(Self {
            key_id: key_id.to_owned(),
            key,
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn encrypt(&self, plaintext: &str, context: &str) -> Result<String, StoreError> {
        let cipher = Aes256Gcm::new_from_slice(self.key.as_slice())
            .map_err(|_| StoreError::Encryption("invalid state-encryption key".to_owned()))?;
        let nonce = Nonce::generate();
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| StoreError::Encryption("state encryption failed".to_owned()))?;
        let envelope = EncryptionEnvelope {
            version: ENCRYPTION_FORMAT_VERSION,
            algorithm: ENCRYPTION_ALGORITHM.to_owned(),
            key_id: self.key_id.clone(),
            nonce: STANDARD.encode(nonce.as_slice()),
            ciphertext: STANDARD.encode(ciphertext),
        };
        Ok(format!(
            "{ENVELOPE_PREFIX}{}:{}",
            self.key_id,
            serde_json::to_string(&envelope)?
        ))
    }

    pub fn decrypt(&self, stored: &str, context: &str) -> Result<String, StoreError> {
        let envelope = parse_envelope(stored, context)?;
        if envelope.key_id != self.key_id {
            return Err(StoreError::Encryption(format!(
                "protected field `{context}` requires key ID `{}`, configured key ID is `{}`",
                envelope.key_id, self.key_id
            )));
        }
        let nonce = STANDARD.decode(envelope.nonce.as_bytes()).map_err(|_| {
            StoreError::Encryption(format!("protected field `{context}` has an invalid nonce"))
        })?;
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| {
            StoreError::Encryption(format!(
                "protected field `{context}` has an invalid nonce length"
            ))
        })?;
        let ciphertext = STANDARD
            .decode(envelope.ciphertext.as_bytes())
            .map_err(|_| {
                StoreError::Encryption(format!(
                    "protected field `{context}` has invalid ciphertext"
                ))
            })?;
        let cipher = Aes256Gcm::new_from_slice(self.key.as_slice())
            .map_err(|_| StoreError::Encryption("invalid state-encryption key".to_owned()))?;
        let nonce = Array(nonce);
        let plaintext = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| {
                StoreError::Encryption(format!(
                    "authentication failed for protected field `{context}`"
                ))
            })?;
        String::from_utf8(plaintext).map_err(|_| {
            StoreError::Encryption(format!("protected field `{context}` did not contain UTF-8"))
        })
    }

    pub fn key_check(&self) -> Result<String, StoreError> {
        self.encrypt(KEY_CHECK_VALUE, KEY_CHECK_CONTEXT)
    }

    pub fn verify_key_check(&self, stored: &str) -> Result<(), StoreError> {
        let value = self.decrypt(stored, KEY_CHECK_CONTEXT)?;
        if value != KEY_CHECK_VALUE {
            return Err(StoreError::Encryption(
                "state-encryption key check is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptionEnvelope {
    version: u32,
    algorithm: String,
    key_id: String,
    nonce: String,
    ciphertext: String,
}

pub(crate) fn is_encrypted_value(value: &str) -> bool {
    value.starts_with(ENVELOPE_PREFIX)
}

pub(crate) fn validate_envelope(value: &str, context: &str) -> Result<(), StoreError> {
    parse_envelope(value, context).map(|_| ())
}

fn parse_envelope(value: &str, context: &str) -> Result<EncryptionEnvelope, StoreError> {
    let raw = value.strip_prefix(ENVELOPE_PREFIX).ok_or_else(|| {
        StoreError::Encryption(format!("protected field `{context}` is not encrypted"))
    })?;
    let (prefix_key_id, raw) = raw.split_once(':').ok_or_else(|| {
        StoreError::Encryption(format!(
            "protected field `{context}` has a malformed encryption envelope"
        ))
    })?;
    let envelope: EncryptionEnvelope = serde_json::from_str(raw).map_err(|_| {
        StoreError::Encryption(format!(
            "protected field `{context}` has a malformed encryption envelope"
        ))
    })?;
    if envelope.version != ENCRYPTION_FORMAT_VERSION {
        return Err(StoreError::Encryption(format!(
            "protected field `{context}` uses unsupported encryption format {}",
            envelope.version
        )));
    }
    if envelope.algorithm != ENCRYPTION_ALGORITHM {
        return Err(StoreError::Encryption(format!(
            "protected field `{context}` uses unsupported algorithm `{}`",
            envelope.algorithm
        )));
    }
    validate_key_id(&envelope.key_id)?;
    if prefix_key_id != envelope.key_id {
        return Err(StoreError::Encryption(format!(
            "protected field `{context}` has inconsistent key metadata"
        )));
    }
    Ok(envelope)
}

fn validate_key_bytes(reference: &str, bytes: Vec<u8>) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    if bytes.len() != 32 {
        return Err(StoreError::Encryption(format!(
            "state-encryption key from `{reference}` must decode to exactly 32 bytes"
        )));
    }
    Ok(Zeroizing::new(bytes))
}

fn validate_key_id(key_id: &str) -> Result<(), StoreError> {
    if key_id.is_empty()
        || key_id.len() > 128
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StoreError::Encryption(
            "state-encryption key ID must contain 1-128 ASCII letters, digits, `.`, `_`, or `-`"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_environment_reference(reference: &str) -> Result<(), StoreError> {
    let mut bytes = reference.bytes();
    let first = bytes.next();
    if first.is_none_or(|byte| !(byte.is_ascii_uppercase() || byte == b'_'))
        || !bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(StoreError::Encryption(
            "state-encryption key environment reference must match [A-Z_][A-Z0-9_]*".to_owned(),
        ));
    }
    Ok(())
}
