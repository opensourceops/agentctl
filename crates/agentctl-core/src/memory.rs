use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MEMORY_ENTRY_FORMAT_VERSION: u32 = 1;
pub const MIN_EMBEDDING_DIMENSIONS: u16 = 8;
pub const MAX_EMBEDDING_DIMENSIONS: u16 = 4096;
pub const MAX_MEMORY_RESULTS: u16 = 100;
pub const MAX_MEMORY_ENTRY_BYTES: usize = 1024 * 1024;
pub const MAX_MEMORY_QUERY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum MemoryContent {
    Text {
        text: String,
    },
    Json {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryEntry {
    pub format_version: u32,
    pub content: MemoryContent,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl MemoryEntry {
    #[must_use]
    pub fn text(text: impl Into<String>, metadata: BTreeMap<String, Value>) -> Self {
        Self {
            format_version: MEMORY_ENTRY_FORMAT_VERSION,
            content: MemoryContent::Text { text: text.into() },
            metadata,
        }
    }

    #[must_use]
    pub fn json(value: Value, text: Option<String>, metadata: BTreeMap<String, Value>) -> Self {
        Self {
            format_version: MEMORY_ENTRY_FORMAT_VERSION,
            content: MemoryContent::Json { value, text },
            metadata,
        }
    }

    #[must_use]
    pub fn from_legacy(value: Value) -> Self {
        let text = match &value {
            Value::String(value) => Some(value.clone()),
            Value::Null => None,
            value => serde_json::to_string(value).ok(),
        };
        Self::json(value, text, BTreeMap::new())
    }

    #[must_use]
    pub fn value(&self) -> Value {
        match &self.content {
            MemoryContent::Text { text } => Value::String(text.clone()),
            MemoryContent::Json { value, .. } => value.clone(),
        }
    }

    #[must_use]
    pub fn searchable_text(&self) -> Option<&str> {
        match &self.content {
            MemoryContent::Text { text } => Some(text),
            MemoryContent::Json { text, .. } => text.as_deref(),
        }
    }

    pub fn validate(&self) -> Result<(), MemoryContractError> {
        if self.format_version != MEMORY_ENTRY_FORMAT_VERSION {
            return Err(MemoryContractError::FormatVersion(self.format_version));
        }
        if self.searchable_text().is_some_and(str::is_empty) {
            return Err(MemoryContractError::EmptyText);
        }
        if self.metadata.keys().any(|key| key.is_empty()) {
            return Err(MemoryContractError::EmptyMetadataKey);
        }
        if serde_json::to_vec(self).map_or(true, |value| value.len() > MAX_MEMORY_ENTRY_BYTES) {
            return Err(MemoryContractError::EntryTooLarge);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySearchMode {
    #[default]
    Text,
    Vector,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryQuery {
    pub namespace: String,
    pub text: String,
    #[serde(default)]
    pub mode: MemorySearchMode,
    #[serde(default = "default_memory_limit")]
    pub limit: u16,
    #[serde(default)]
    pub filters: BTreeMap<String, Value>,
}

impl MemoryQuery {
    pub fn validate(&self) -> Result<(), MemoryContractError> {
        if self.namespace.is_empty() {
            return Err(MemoryContractError::EmptyNamespace);
        }
        if self.text.is_empty() {
            return Err(MemoryContractError::EmptyQuery);
        }
        if self.limit == 0 || self.limit > MAX_MEMORY_RESULTS {
            return Err(MemoryContractError::InvalidLimit(self.limit));
        }
        if self.filters.keys().any(|key| key.is_empty()) {
            return Err(MemoryContractError::EmptyMetadataKey);
        }
        if self.text.len() > MAX_MEMORY_QUERY_BYTES {
            return Err(MemoryContractError::QueryTooLarge);
        }
        Ok(())
    }
}

const fn default_memory_limit() -> u16 {
    10
}

#[derive(Debug, Error)]
pub enum MemoryContractError {
    #[error("unsupported memory entry format version {0}")]
    FormatVersion(u32),
    #[error("memory text must not be empty")]
    EmptyText,
    #[error("memory metadata keys must not be empty")]
    EmptyMetadataKey,
    #[error("memory entry exceeds the {MAX_MEMORY_ENTRY_BYTES}-byte limit")]
    EntryTooLarge,
    #[error("memory namespace must not be empty")]
    EmptyNamespace,
    #[error("memory search query must not be empty")]
    EmptyQuery,
    #[error("memory search query exceeds the {MAX_MEMORY_QUERY_BYTES}-byte limit")]
    QueryTooLarge,
    #[error("memory result limit {0} must be between 1 and {MAX_MEMORY_RESULTS}")]
    InvalidLimit(u16),
    #[error(
        "embedding dimensions {0} must be between {MIN_EMBEDDING_DIMENSIONS} and {MAX_EMBEDDING_DIMENSIONS}"
    )]
    InvalidDimensions(u16),
}

#[must_use]
pub fn memory_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub fn local_hash_embedding(text: &str, dimensions: u16) -> Result<Vec<f32>, MemoryContractError> {
    if !(MIN_EMBEDDING_DIMENSIONS..=MAX_EMBEDDING_DIMENSIONS).contains(&dimensions) {
        return Err(MemoryContractError::InvalidDimensions(dimensions));
    }
    let mut vector = vec![0.0_f32; usize::from(dimensions)];
    for token in memory_tokens(text) {
        let digest = Sha256::digest(token.as_bytes());
        let index =
            usize::from(u16::from_be_bytes([digest[0], digest[1]])) % usize::from(dimensions);
        let direction = if digest[2] & 1 == 0 { 1.0 } else { -1.0 };
        vector[index] += direction;
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value = (f64::from(*value) / norm) as f32;
        }
    }
    Ok(vector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_embeddings_are_deterministic_and_bounded() {
        let first = local_hash_embedding("Alpha alpha beta", 64).expect("embedding");
        let second = local_hash_embedding("Alpha alpha beta", 64).expect("embedding");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.iter().all(|value| value.is_finite()));
        assert!(local_hash_embedding("text", 7).is_err());
    }

    #[test]
    fn legacy_values_become_typed_entries() {
        let entry = MemoryEntry::from_legacy(Value::String("hello".to_owned()));
        assert_eq!(entry.value(), Value::String("hello".to_owned()));
        assert_eq!(entry.searchable_text(), Some("hello"));
        entry.validate().expect("entry");
    }
}
