//! Versioned SQLite persistence for agentctl.

pub mod artifact;
pub mod encryption;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use agentctl_core::dsl::ResourceBudgetDefinition;
use agentctl_core::effect::{EffectRecord, EffectRequest, EffectStatus};
use agentctl_core::memory::{
    MAX_EMBEDDING_DIMENSIONS, MEMORY_ENTRY_FORMAT_VERSION, MIN_EMBEDDING_DIMENSIONS, MemoryEntry,
    MemoryQuery, MemorySearchMode, memory_tokens,
};
use agentctl_core::state::{RunState, TaskState};
use agentctl_core::{CompiledPlan, PLAN_FORMAT_VERSION};
use artifact::{ArtifactStore, ArtifactStoreError, ArtifactVerification, LocalArtifactStore};
use chrono::{DateTime, Utc};
use encryption::{
    ENVELOPE_PREFIX, EncryptionCodec, EnvironmentKeyResolver, SENSITIVE_COLUMNS,
    SharedStateProtection, StateKeyResolver, StateProtection, is_encrypted_value,
    validate_envelope,
};
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const DATABASE_SCHEMA_VERSION: u32 = 15;
pub const RUNTIME_STATE_VERSION: u32 = 1;
pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;
pub const AUDIT_EVENT_VERSION: u32 = 1;
pub const STREAM_EVENT_FORMAT_VERSION: u32 = 1;
const ARTIFACT_INGEST_LEASE_MINUTES: i64 = 60;
const MAX_MEMORY_SEARCH_CANDIDATES: usize = 10_000;

const MIGRATION_1: &str = r#"
CREATE TABLE runs (
  run_id TEXT PRIMARY KEY,
  runtime_state_version INTEGER NOT NULL,
  workflow_digest TEXT NOT NULL,
  workflow_schema_version TEXT NOT NULL,
  plan_digest TEXT NOT NULL,
  plan_format_version INTEGER NOT NULL,
  workflow_json TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  inputs_json TEXT NOT NULL,
  working_memory_json TEXT NOT NULL,
  output_json TEXT,
  state TEXT NOT NULL,
  mode TEXT NOT NULL,
  parent_run_id TEXT REFERENCES runs(run_id),
  cancellation_requested INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE task_states (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  state TEXT NOT NULL,
  attempt INTEGER NOT NULL DEFAULT 0,
  output_json TEXT,
  error TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id)
);
CREATE TABLE effects (
  effect_id TEXT PRIMARY KEY,
  format_version INTEGER NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  task_attempt INTEGER NOT NULL,
  ordinal INTEGER NOT NULL,
  operation TEXT NOT NULL,
  effect_class TEXT NOT NULL,
  risk TEXT NOT NULL,
  idempotency TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  input_digest TEXT NOT NULL,
  input_json TEXT NOT NULL,
  expected_effect TEXT NOT NULL,
  trace_id TEXT NOT NULL,
  status TEXT NOT NULL,
  effect_attempt INTEGER NOT NULL,
  requested_at TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  result_json TEXT,
  error TEXT,
  confirmed INTEGER NOT NULL DEFAULT 0,
  UNIQUE (run_id, task_id, task_attempt, ordinal)
);
CREATE TABLE approvals (
  approval_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  effect_id TEXT NOT NULL REFERENCES effects(effect_id),
  task_id TEXT NOT NULL,
  agent TEXT,
  tool TEXT NOT NULL,
  capability TEXT NOT NULL,
  risk TEXT NOT NULL,
  redacted_input_json TEXT NOT NULL,
  expected_effect TEXT NOT NULL,
  reason TEXT NOT NULL,
  trace_id TEXT NOT NULL,
  status TEXT NOT NULL,
  requested_at TEXT NOT NULL,
  resolved_at TEXT,
  resolved_by TEXT,
  resolution_reason TEXT
);
CREATE TABLE checkpoints (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL,
  format_version INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  checksum TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, sequence)
);
CREATE TABLE audit_events (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL,
  event_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  task_id TEXT,
  trace_id TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, sequence)
);
"#;

const MIGRATION_2: &str = r#"
CREATE TABLE provider_sessions (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  continuation_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id)
);
CREATE TABLE tool_calls (
  call_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  effect_id TEXT REFERENCES effects(effect_id),
  tool_id TEXT NOT NULL,
  input_digest TEXT NOT NULL,
  output_digest TEXT,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  completed_at TEXT
);
CREATE TABLE long_term_memory (
  namespace TEXT NOT NULL,
  memory_key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  expires_at TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (namespace, memory_key)
);
CREATE INDEX idx_runs_state_updated ON runs(state, updated_at);
CREATE INDEX idx_effects_run_status ON effects(run_id, status);
CREATE INDEX idx_approvals_run_status ON approvals(run_id, status);
CREATE INDEX idx_audit_run_created ON audit_events(run_id, created_at);
"#;

const MIGRATION_3: &str = r#"
ALTER TABLE runs ADD COLUMN base_path TEXT;
CREATE TABLE trace_events (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL,
  trace_id TEXT NOT NULL,
  event_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, sequence)
);
CREATE INDEX idx_trace_run_created ON trace_events(run_id, created_at);
"#;

const MIGRATION_4: &str = r#"
CREATE TABLE tool_calls_v4 (
  call_id TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  effect_id TEXT REFERENCES effects(effect_id),
  tool_id TEXT NOT NULL,
  input_digest TEXT NOT NULL,
  output_digest TEXT,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (run_id, call_id)
);
INSERT INTO tool_calls_v4
  (call_id, run_id, task_id, effect_id, tool_id, input_digest, output_digest, status, created_at, completed_at)
SELECT call_id, run_id, task_id, effect_id, tool_id, input_digest, output_digest, status, created_at, completed_at
FROM tool_calls;
DROP TABLE tool_calls;
ALTER TABLE tool_calls_v4 RENAME TO tool_calls;
"#;

const MIGRATION_5: &str = r#"
ALTER TABLE runs ADD COLUMN source_run_id TEXT;
ALTER TABLE runs ADD COLUMN source_workflow_digest TEXT;
ALTER TABLE runs ADD COLUMN repair_roots_json TEXT;
ALTER TABLE runs ADD COLUMN repair_reason TEXT;
ALTER TABLE runs ADD COLUMN repair_format_version INTEGER;

ALTER TABLE task_states ADD COLUMN disposition TEXT NOT NULL DEFAULT 'executed';
ALTER TABLE task_states ADD COLUMN metadata_version INTEGER;
ALTER TABLE task_states ADD COLUMN source_run_id TEXT;
ALTER TABLE task_states ADD COLUMN source_task_id TEXT;
ALTER TABLE task_states ADD COLUMN source_attempt INTEGER;
ALTER TABLE task_states ADD COLUMN definition_fingerprint TEXT;
ALTER TABLE task_states ADD COLUMN input_digest TEXT;
ALTER TABLE task_states ADD COLUMN output_contract_fingerprint TEXT;
ALTER TABLE task_states ADD COLUMN output_digest TEXT;
ALTER TABLE task_states ADD COLUMN state_delta_json TEXT;
ALTER TABLE task_states ADD COLUMN state_delta_digest TEXT;
ALTER TABLE task_states ADD COLUMN artifact_manifest_json TEXT;
ALTER TABLE task_states ADD COLUMN reuse_decision_json TEXT;

CREATE INDEX idx_runs_source_run ON runs(source_run_id);
CREATE INDEX idx_tasks_disposition ON task_states(run_id, disposition);
"#;

const MIGRATION_6: &str = r#"
CREATE TABLE artifact_blobs (
  digest TEXT PRIMARY KEY,
  algorithm TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  relative_path TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  last_verified_at TEXT
);
CREATE TABLE artifact_refs (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  logical_path TEXT NOT NULL,
  logical_name TEXT NOT NULL,
  media_type TEXT NOT NULL,
  digest TEXT NOT NULL REFERENCES artifact_blobs(digest),
  source_run_id TEXT,
  source_task_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id, logical_path),
  FOREIGN KEY (run_id, task_id) REFERENCES task_states(run_id, task_id) ON DELETE CASCADE
);
CREATE INDEX idx_artifact_refs_digest ON artifact_refs(digest);
CREATE INDEX idx_artifact_refs_run_task ON artifact_refs(run_id, task_id);
CREATE TABLE artifact_ingests (
  run_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  logical_path TEXT NOT NULL,
  digest TEXT NOT NULL REFERENCES artifact_blobs(digest) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id, logical_path),
  FOREIGN KEY (run_id, task_id) REFERENCES task_states(run_id, task_id) ON DELETE CASCADE
);
CREATE INDEX idx_artifact_ingests_expiry ON artifact_ingests(expires_at);
"#;

const MIGRATION_7: &str = r#"
CREATE TABLE run_upgrades (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  upgrade_id TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  analysis_json TEXT NOT NULL,
  upgraded_tasks_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, upgrade_id)
);
CREATE INDEX idx_run_upgrades_created ON run_upgrades(run_id, created_at);
"#;

const MIGRATION_8: &str = r#"
CREATE TABLE effect_reconciliations (
  reconciliation_id TEXT PRIMARY KEY,
  effect_id TEXT NOT NULL REFERENCES effects(effect_id),
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  format_version INTEGER NOT NULL,
  status TEXT NOT NULL,
  actor TEXT NOT NULL,
  reason TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  result_json TEXT,
  result_schema_json TEXT,
  authorization_json TEXT NOT NULL,
  compensation_effect_id TEXT REFERENCES effects(effect_id),
  supersedes_id TEXT REFERENCES effect_reconciliations(reconciliation_id),
  trace_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_effect_reconciliations_effect_created
  ON effect_reconciliations(effect_id, created_at, reconciliation_id);
CREATE INDEX idx_effect_reconciliations_run_created
  ON effect_reconciliations(run_id, created_at, reconciliation_id);
"#;

const MIGRATION_9: &str = r#"
ALTER TABLE runs ADD COLUMN retry_roots_json TEXT;
ALTER TABLE runs ADD COLUMN retry_reason TEXT;
ALTER TABLE runs ADD COLUMN retry_format_version INTEGER;
ALTER TABLE runs ADD COLUMN retry_failed_only INTEGER NOT NULL DEFAULT 0;
"#;

const MIGRATION_10: &str = r#"
CREATE TABLE state_encryption (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  format_version INTEGER NOT NULL,
  key_id TEXT NOT NULL,
  key_reference TEXT NOT NULL,
  key_check TEXT NOT NULL,
  maintenance INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);
"#;

const MIGRATION_11: &str = r#"
ALTER TABLE task_states ADD COLUMN execution_memory_json TEXT;
"#;

const MIGRATION_12: &str = r#"
CREATE TABLE stream_events (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  task_attempt INTEGER NOT NULL,
  sequence INTEGER NOT NULL,
  format_version INTEGER NOT NULL,
  effect_id TEXT,
  event_type TEXT NOT NULL,
  provider_sequence INTEGER,
  payload_json TEXT NOT NULL,
  truncated INTEGER NOT NULL DEFAULT 0,
  source_run_id TEXT,
  source_sequence INTEGER,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id, task_attempt, sequence),
  FOREIGN KEY (run_id, task_id) REFERENCES task_states(run_id, task_id) ON DELETE CASCADE
);
CREATE INDEX idx_stream_events_run
  ON stream_events(run_id, task_id, task_attempt, sequence);
"#;

const MIGRATION_13: &str = r#"
CREATE TABLE protocol_sessions (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  effect_id TEXT NOT NULL,
  protocol TEXT NOT NULL,
  remote TEXT NOT NULL,
  generation INTEGER NOT NULL,
  status TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  source_run_id TEXT,
  source_task_id TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, task_id, protocol),
  FOREIGN KEY (run_id, task_id) REFERENCES task_states(run_id, task_id) ON DELETE CASCADE
);
CREATE TABLE protocol_calls (
  effect_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  task_attempt INTEGER NOT NULL,
  protocol TEXT NOT NULL,
  operation TEXT NOT NULL,
  call_identity TEXT NOT NULL,
  generation INTEGER NOT NULL,
  idempotency TEXT NOT NULL,
  status TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  source_run_id TEXT,
  source_effect_id TEXT,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (run_id, task_id) REFERENCES task_states(run_id, task_id) ON DELETE CASCADE
);
CREATE INDEX idx_protocol_sessions_run
  ON protocol_sessions(run_id, task_id, protocol);
CREATE INDEX idx_protocol_calls_run
  ON protocol_calls(run_id, task_id, task_attempt, effect_id);
"#;

const MIGRATION_14: &str = r#"
ALTER TABLE long_term_memory ADD COLUMN format_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE long_term_memory ADD COLUMN embedding_provider TEXT;
ALTER TABLE long_term_memory ADD COLUMN embedding_dimensions INTEGER;
ALTER TABLE long_term_memory ADD COLUMN embedding_json TEXT;
ALTER TABLE long_term_memory ADD COLUMN created_at TEXT;
UPDATE long_term_memory SET created_at = updated_at WHERE created_at IS NULL;
CREATE INDEX idx_long_term_memory_search
  ON long_term_memory(namespace, expires_at, memory_key);
"#;

const MIGRATION_15: &str = r#"
CREATE TABLE run_budgets (
  run_id TEXT PRIMARY KEY REFERENCES runs(run_id) ON DELETE CASCADE,
  format_version INTEGER NOT NULL,
  limits_json TEXT NOT NULL,
  pricing_version TEXT,
  usage_json TEXT NOT NULL,
  reserved_json TEXT NOT NULL,
  exceeded_json TEXT,
  planned_tasks INTEGER NOT NULL,
  planned_expansion_items INTEGER NOT NULL,
  planned_loop_iterations INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE budget_reservations (
  run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
  reservation_id TEXT NOT NULL,
  task_id TEXT,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  reserved_json TEXT NOT NULL,
  actual_json TEXT,
  source TEXT,
  created_at TEXT NOT NULL,
  reconciled_at TEXT,
  PRIMARY KEY (run_id, reservation_id)
);
CREATE INDEX idx_budget_reservations_run
  ON budget_reservations(run_id, status, created_at);
INSERT INTO run_budgets (
  run_id,
  format_version,
  limits_json,
  usage_json,
  reserved_json,
  planned_tasks,
  planned_expansion_items,
  planned_loop_iterations,
  updated_at
)
SELECT
  runs.run_id,
  1,
  '{}',
  '{"providerRequests":0,"turns":0,"toolCalls":0,"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"processOutputBytes":0,"artifactBytes":0,"costMicrousd":0,"unpricedProviderRequests":0}',
  '{"providerRequests":0,"turns":0,"toolCalls":0,"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"processOutputBytes":0,"artifactBytes":0,"costMicrousd":0,"unpricedProviderRequests":0}',
  (SELECT COUNT(*) FROM task_states WHERE task_states.run_id = runs.run_id),
  0,
  0,
  runs.updated_at
FROM runs;
"#;

#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    artifact_store: Arc<LocalArtifactStore>,
    artifact_lock: Arc<Mutex<()>>,
    protection: SharedStateProtection,
    key_resolver: Arc<dyn StateKeyResolver>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("database schema {found} is newer than supported schema {supported}")]
    UnknownSchema { found: u32, supported: u32 },
    #[error("durable state is incompatible: {0}")]
    Incompatible(String),
    #[error("durable state is corrupt: {0}")]
    Corrupt(String),
    #[error("state encryption error: {0}")]
    Encryption(String),
    #[error("run `{0}` was not found")]
    RunNotFound(String),
    #[error("task `{task_id}` was not found in run `{run_id}`")]
    TaskNotFound { run_id: String, task_id: String },
    #[error("invalid task transition: {0}")]
    InvalidTransition(String),
    #[error("approval `{0}` was not found or already resolved")]
    ApprovalNotPending(String),
    #[error("effect `{0}` was not found")]
    EffectNotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Artifact(#[from] ArtifactStoreError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: String,
    pub workflow_digest: String,
    pub workflow_schema_version: String,
    pub plan_digest: String,
    pub workflow: Value,
    pub plan: CompiledPlan,
    pub inputs: Value,
    pub working_memory: Value,
    pub output: Option<Value>,
    pub state: RunState,
    pub mode: RunMode,
    pub parent_run_id: Option<String>,
    pub source_run_id: Option<String>,
    pub source_workflow_digest: Option<String>,
    pub repair_roots: Vec<String>,
    pub repair_reason: Option<String>,
    pub repair_format_version: Option<u32>,
    pub retry_roots: Vec<String>,
    pub retry_reason: Option<String>,
    pub retry_format_version: Option<u32>,
    pub retry_failed_only: bool,
    pub base_path: Option<String>,
    pub cancellation_requested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Execute,
    Check,
    Replay,
    Fork,
    Repair,
    Retry,
    Compensation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDisposition {
    Executed,
    Reused,
    Recorded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub path: String,
    pub digest: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub logical_name: String,
    #[serde(default)]
    pub store_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactBlobRecord {
    pub digest: String,
    pub algorithm: String,
    pub size_bytes: u64,
    pub relative_path: String,
    pub created_at: DateTime<Utc>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub reference_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactReference {
    pub run_id: String,
    pub task_id: String,
    pub logical_path: String,
    pub logical_name: String,
    pub media_type: String,
    pub digest: String,
    pub source_run_id: Option<String>,
    pub source_task_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactGcReport {
    pub considered: u64,
    pub removed: Vec<String>,
    pub reclaimed_bytes: u64,
    pub temporary_files_considered: u64,
    pub temporary_files_removed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub run_id: String,
    pub task_id: String,
    pub position: i64,
    pub state: TaskState,
    pub attempt: u16,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub disposition: TaskDisposition,
    pub metadata_version: Option<u32>,
    pub source_run_id: Option<String>,
    pub source_task_id: Option<String>,
    pub source_attempt: Option<u16>,
    pub definition_fingerprint: Option<String>,
    pub input_digest: Option<String>,
    pub output_contract_fingerprint: Option<String>,
    pub output_digest: Option<String>,
    pub state_delta: Option<Value>,
    pub state_delta_digest: Option<String>,
    pub artifact_manifest: Vec<ArtifactRecord>,
    pub reuse_decision: Option<Value>,
    #[serde(skip)]
    pub execution_memory: Option<Value>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecutionMetadata {
    pub metadata_version: u32,
    pub definition_fingerprint: String,
    pub input_digest: String,
    pub output_contract_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCompletionMetadata {
    pub execution: TaskExecutionMetadata,
    pub output_digest: String,
    pub state_delta: Value,
    pub state_delta_digest: String,
    pub artifact_manifest: Vec<ArtifactRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskBatchOutcome {
    Succeeded {
        output: Value,
        metadata: Box<TaskCompletionMetadata>,
    },
    Failed {
        error: String,
    },
    RetryScheduled {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskBatchResult {
    pub task_id: String,
    pub outcome: TaskBatchOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReusedTaskMaterialization {
    pub task_id: String,
    pub source_run_id: String,
    pub source_task_id: String,
    pub source_attempt: u16,
    pub output: Value,
    pub metadata: TaskCompletionMetadata,
    pub reuse_decision: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyTaskUpgrade {
    pub task_id: String,
    pub metadata: TaskCompletionMetadata,
    pub provenance: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Applied,
    NotApplied,
    Compensated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReconciliationRequest {
    pub reconciliation_id: String,
    pub effect_id: String,
    pub status: ReconciliationStatus,
    pub actor: String,
    pub reason: String,
    pub evidence: Value,
    pub result: Option<Value>,
    pub result_schema: Option<Value>,
    pub authorization: Value,
    pub compensation_effect_id: Option<String>,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReconciliationRecord {
    pub reconciliation_id: String,
    pub effect_id: String,
    pub run_id: String,
    pub format_version: u32,
    pub status: ReconciliationStatus,
    pub actor: String,
    pub reason: String,
    pub evidence: Value,
    pub result: Option<Value>,
    pub result_schema: Option<Value>,
    pub authorization: Value,
    pub compensation_effect_id: Option<String>,
    pub supersedes_id: Option<String>,
    pub trace_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub run_id: String,
    pub effect_id: String,
    pub task_id: String,
    pub agent: Option<String>,
    pub tool: String,
    pub capability: String,
    pub risk: String,
    pub redacted_input: Value,
    pub expected_effect: String,
    pub reason: String,
    pub trace_id: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalResolution {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub sequence: i64,
    pub event_type: String,
    pub task_id: Option<String>,
    pub trace_id: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRecord {
    pub sequence: i64,
    pub format_version: u32,
    pub state: Value,
    pub checksum: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSessionRecord {
    pub task_id: String,
    pub provider: String,
    pub format_version: u32,
    pub continuation: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEventRecord {
    pub run_id: String,
    pub task_id: String,
    pub task_attempt: u16,
    pub sequence: i64,
    pub effect_id: Option<String>,
    pub event_type: String,
    pub provider_sequence: Option<i64>,
    pub payload: Value,
    pub truncated: bool,
    pub source_run_id: Option<String>,
    pub source_sequence: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolSessionRecord {
    pub run_id: String,
    pub task_id: String,
    pub effect_id: String,
    pub protocol: String,
    pub remote: String,
    pub generation: u32,
    pub status: String,
    pub format_version: u32,
    pub state: Value,
    pub source_run_id: Option<String>,
    pub source_task_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolCallRecord {
    pub effect_id: String,
    pub run_id: String,
    pub task_id: String,
    pub task_attempt: u16,
    pub protocol: String,
    pub operation: String,
    pub call_identity: String,
    pub generation: u32,
    pub idempotency: String,
    pub status: String,
    pub format_version: u32,
    pub state: Value,
    pub source_run_id: Option<String>,
    pub source_effect_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub call_id: String,
    pub task_id: String,
    pub effect_id: String,
    pub tool_id: String,
    pub input_digest: String,
    pub output_digest: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRecord {
    pub sequence: i64,
    pub trace_id: String,
    pub event: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub namespace: String,
    pub key: String,
    pub entry: MemoryEntry,
    pub embedding_provider: Option<String>,
    pub embedding_dimensions: Option<u16>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchResult {
    pub record: MemoryRecord,
    pub score_millionths: u32,
    pub text_score_millionths: u32,
    pub vector_score_millionths: u32,
}

pub const BUDGET_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BudgetCounters {
    pub provider_requests: u64,
    pub turns: u64,
    pub tool_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub wall_time_seconds: u64,
    pub process_output_bytes: u64,
    pub artifact_bytes: u64,
    pub cost_microusd: u64,
    pub unpriced_provider_requests: u64,
}

impl BudgetCounters {
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    fn saturating_add_assign(&mut self, other: &Self) {
        self.provider_requests = self
            .provider_requests
            .saturating_add(other.provider_requests);
        self.turns = self.turns.saturating_add(other.turns);
        self.tool_calls = self.tool_calls.saturating_add(other.tool_calls);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.wall_time_seconds = self
            .wall_time_seconds
            .saturating_add(other.wall_time_seconds);
        self.process_output_bytes = self
            .process_output_bytes
            .saturating_add(other.process_output_bytes);
        self.artifact_bytes = self.artifact_bytes.saturating_add(other.artifact_bytes);
        self.cost_microusd = self.cost_microusd.saturating_add(other.cost_microusd);
        self.unpriced_provider_requests = self
            .unpriced_provider_requests
            .saturating_add(other.unpriced_provider_requests);
    }

    fn saturating_sub_assign(&mut self, other: &Self) {
        self.provider_requests = self
            .provider_requests
            .saturating_sub(other.provider_requests);
        self.turns = self.turns.saturating_sub(other.turns);
        self.tool_calls = self.tool_calls.saturating_sub(other.tool_calls);
        self.input_tokens = self.input_tokens.saturating_sub(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_sub(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_sub(other.reasoning_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_sub(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_sub(other.cache_write_tokens);
        self.wall_time_seconds = self
            .wall_time_seconds
            .saturating_sub(other.wall_time_seconds);
        self.process_output_bytes = self
            .process_output_bytes
            .saturating_sub(other.process_output_bytes);
        self.artifact_bytes = self.artifact_bytes.saturating_sub(other.artifact_bytes);
        self.cost_microusd = self.cost_microusd.saturating_sub(other.cost_microusd);
        self.unpriced_provider_requests = self
            .unpriced_provider_requests
            .saturating_sub(other.unpriced_provider_requests);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetExceeded {
    pub dimension: String,
    pub limit: u64,
    pub attempted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetSnapshot {
    pub format_version: u32,
    pub limits: ResourceBudgetDefinition,
    pub pricing_version: Option<String>,
    pub usage: BudgetCounters,
    pub reserved: BudgetCounters,
    pub exceeded: Option<BudgetExceeded>,
    pub planned_tasks: u64,
    pub planned_expansion_items: u64,
    pub planned_loop_iterations: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetReservationDecision {
    Allowed(BudgetSnapshot),
    Denied {
        exceeded: BudgetExceeded,
        snapshot: BudgetSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStats {
    pub schema_version: u32,
    pub runs: i64,
    pub tasks: i64,
    pub effects: i64,
    pub approvals: i64,
    pub checkpoints: i64,
    pub audit_events: i64,
    pub provider_sessions: i64,
    pub stream_events: i64,
    pub protocol_sessions: i64,
    pub protocol_calls: i64,
    pub tool_calls: i64,
    pub trace_events: i64,
    pub long_term_memory: i64,
    pub artifact_blobs: i64,
    pub artifact_references: i64,
    pub artifact_ingests: i64,
    pub run_upgrades: i64,
    pub effect_reconciliations: i64,
    pub run_budgets: i64,
    pub budget_reservations: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionInventory {
    pub enabled: bool,
    pub key_id: Option<String>,
    pub key_reference: Option<String>,
    pub protected_values: u64,
    pub encrypted_values: u64,
    pub plaintext_values: u64,
    pub invalid_envelopes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionMigrationReport {
    pub operation: String,
    pub dry_run: bool,
    pub key_id: String,
    pub key_reference: String,
    pub values_scanned: u64,
    pub values_rewritten: u64,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with_key_resolver(path, Arc::new(EnvironmentKeyResolver))
    }

    pub fn open_with_key_resolver(
        path: &Path,
        key_resolver: Arc<dyn StateKeyResolver>,
    ) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        configure(&connection)?;
        migrate(&mut { connection })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, permissions)?;
        }
        let connection = Connection::open(path)?;
        configure(&connection)?;
        let state_root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let artifact_store = LocalArtifactStore::open(state_root.join("artifacts"))?;
        let _artifact_file_lock = artifact_store.lock_exclusive()?;
        recover_artifact_quarantine(&connection, &artifact_store)?;
        let protection = load_state_protection(&connection, key_resolver.as_ref())?;
        let inventory = encryption_inventory(&connection, &protection)?;
        if protection.is_enabled()
            && (inventory.plaintext_values != 0 || inventory.invalid_envelopes != 0)
        {
            return Err(StoreError::Encryption(format!(
                "encrypted database contains {} plaintext and {} invalid protected value(s)",
                inventory.plaintext_values, inventory.invalid_envelopes
            )));
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            artifact_store: Arc::new(artifact_store),
            artifact_lock: Arc::new(Mutex::new(())),
            protection: Arc::new(RwLock::new(protection)),
            key_resolver,
        })
    }

    pub fn open_memory() -> Result<Self, StoreError> {
        Self::open_memory_with_key_resolver(Arc::new(EnvironmentKeyResolver))
    }

    pub fn open_memory_with_key_resolver(
        key_resolver: Arc<dyn StateKeyResolver>,
    ) -> Result<Self, StoreError> {
        let mut connection = Connection::open_in_memory()?;
        configure(&connection)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            artifact_store: Arc::new(LocalArtifactStore::temporary()?),
            artifact_lock: Arc::new(Mutex::new(())),
            protection: Arc::new(RwLock::new(StateProtection::Plaintext)),
            key_resolver,
        })
    }

    #[must_use]
    pub fn artifact_root(&self) -> &Path {
        self.artifact_store.root()
    }

    pub fn ingest_artifact(
        &self,
        run_id: &str,
        task_id: &str,
        source: &Path,
        logical_path: &str,
        max_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, StoreError> {
        let _guard = self.artifact_lock.lock();
        let _file_lock = self.artifact_store.lock_exclusive()?;
        let logical_name = Path::new(logical_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                StoreError::Incompatible(format!(
                    "artifact logical path `{logical_path}` has no valid file name"
                ))
            })?;
        let media_type = media_type_for_path(Path::new(logical_path));
        let blob = self.artifact_store.ingest(source, max_bytes)?;
        let size_bytes = i64::try_from(blob.size_bytes).map_err(|_| {
            StoreError::Incompatible(format!(
                "artifact `{logical_path}` size exceeds SQLite integer range"
            ))
        })?;
        let expires_at = now + chrono::Duration::minutes(ARTIFACT_INGEST_LEASE_MINUTES);
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO artifact_blobs (digest, algorithm, size_bytes, relative_path, created_at) VALUES (?1, 'sha256', ?2, ?3, ?4) ON CONFLICT(digest) DO NOTHING",
            params![
                blob.digest,
                size_bytes,
                blob.relative_path,
                now.to_rfc3339(),
            ],
        )?;
        let stored: (i64, String) = transaction.query_row(
            "SELECT size_bytes, relative_path FROM artifact_blobs WHERE digest = ?1",
            [&blob.digest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if sqlite_u64(stored.0, "artifact_blob.size_bytes")? != blob.size_bytes
            || stored.1 != blob.relative_path
        {
            return Err(StoreError::Corrupt(format!(
                "artifact metadata for `{}` conflicts with its existing blob record",
                blob.digest
            )));
        }
        transaction.execute(
            "INSERT INTO artifact_ingests (run_id, task_id, logical_path, digest, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(run_id, task_id, logical_path) DO UPDATE SET digest = excluded.digest, expires_at = excluded.expires_at, created_at = excluded.created_at",
            params![
                run_id,
                task_id,
                logical_path,
                blob.digest,
                expires_at.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(ArtifactRecord {
            path: logical_path.to_owned(),
            digest: blob.digest,
            size_bytes: blob.size_bytes,
            media_type: media_type.to_owned(),
            logical_name: logical_name.to_owned(),
            store_path: blob.relative_path,
        })
    }

    pub fn ingest_artifact_bytes(
        &self,
        run_id: &str,
        task_id: &str,
        bytes: &[u8],
        logical_path: &str,
        max_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, StoreError> {
        use std::io::Write as _;

        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(StoreError::Incompatible(format!(
                "artifact `{logical_path}` exceeds the configured limit of {max_bytes} bytes"
            )));
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.artifact_store.root().join("tmp"))?;
        temporary.write_all(bytes)?;
        temporary.as_file_mut().sync_all()?;
        self.ingest_artifact(
            run_id,
            task_id,
            temporary.path(),
            logical_path,
            max_bytes,
            now,
        )
    }

    pub fn pending_artifacts(
        &self,
        run_id: &str,
        task_id: &str,
    ) -> Result<Vec<ArtifactRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT i.logical_path, b.digest, b.size_bytes, b.relative_path
             FROM artifact_ingests i
             JOIN artifact_blobs b ON b.digest = i.digest
             WHERE i.run_id = ?1 AND i.task_id = ?2
             ORDER BY i.logical_path",
        )?;
        statement
            .query_map(params![run_id, task_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                let logical_name = Path::new(&row.0)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        StoreError::Corrupt(format!(
                            "artifact logical path `{}` has no valid file name",
                            row.0
                        ))
                    })?;
                Ok(ArtifactRecord {
                    path: row.0.clone(),
                    digest: row.1,
                    size_bytes: sqlite_u64(row.2, "artifact_ingest.size_bytes")?,
                    media_type: media_type_for_path(Path::new(&row.0)).to_owned(),
                    logical_name: logical_name.to_owned(),
                    store_path: row.3,
                })
            })
            .collect()
    }

    pub fn budget_snapshot(&self, run_id: &str) -> Result<BudgetSnapshot, StoreError> {
        let connection = self.connection.lock();
        load_budget_snapshot(&connection, run_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reserve_budget(
        &self,
        run_id: &str,
        reservation_id: &str,
        task_id: Option<&str>,
        kind: &str,
        requested: &BudgetCounters,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<BudgetReservationDecision, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT status FROM budget_reservations
                 WHERE run_id = ?1 AND reservation_id = ?2",
                params![run_id, reservation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if existing.is_some() {
            let snapshot = load_budget_snapshot(&transaction, run_id)?;
            transaction.commit()?;
            return Ok(BudgetReservationDecision::Allowed(snapshot));
        }

        let mut snapshot = load_budget_snapshot(&transaction, run_id)?;
        if let Some(exceeded) = budget_exceeded(
            &snapshot.limits,
            &snapshot.usage,
            &snapshot.reserved,
            requested,
        ) {
            snapshot.exceeded = Some(exceeded.clone());
            snapshot.updated_at = now;
            transaction.execute(
                "UPDATE run_budgets
                 SET exceeded_json = ?1, updated_at = ?2
                 WHERE run_id = ?3",
                params![encode(&exceeded)?, now.to_rfc3339(), run_id],
            )?;
            append_audit_tx(
                &transaction,
                run_id,
                "budget.exceeded",
                task_id,
                trace_id,
                &serde_json::json!({
                    "reservationId": reservation_id,
                    "kind": kind,
                    "requested": requested,
                    "exceeded": exceeded,
                    "usage": snapshot.usage,
                    "reserved": snapshot.reserved,
                }),
                now,
                &self.protection,
            )?;
            checkpoint_tx(&transaction, run_id, now, &self.protection)?;
            transaction.commit()?;
            return Ok(BudgetReservationDecision::Denied { exceeded, snapshot });
        }

        snapshot.reserved.saturating_add_assign(requested);
        snapshot.updated_at = now;
        transaction.execute(
            "INSERT INTO budget_reservations
             (run_id, reservation_id, task_id, kind, status, reserved_json, created_at)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)",
            params![
                run_id,
                reservation_id,
                task_id,
                kind,
                encode(requested)?,
                now.to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "UPDATE run_budgets
             SET reserved_json = ?1, updated_at = ?2
             WHERE run_id = ?3",
            params![encode(&snapshot.reserved)?, now.to_rfc3339(), run_id],
        )?;
        append_audit_tx(
            &transaction,
            run_id,
            "budget.reserved",
            task_id,
            trace_id,
            &serde_json::json!({
                "reservationId": reservation_id,
                "kind": kind,
                "requested": requested,
                "reserved": snapshot.reserved,
            }),
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(BudgetReservationDecision::Allowed(snapshot))
    }

    pub fn reconcile_budget(
        &self,
        run_id: &str,
        reservation_id: &str,
        actual: &BudgetCounters,
        source: &str,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<BudgetSnapshot, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reservation = transaction
            .query_row(
                "SELECT task_id, status, reserved_json
                 FROM budget_reservations
                 WHERE run_id = ?1 AND reservation_id = ?2",
                params![run_id, reservation_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::Incompatible(format!(
                    "budget reservation `{reservation_id}` was not found in run `{run_id}`"
                ))
            })?;
        if reservation.1 == "reconciled" {
            let snapshot = load_budget_snapshot(&transaction, run_id)?;
            transaction.commit()?;
            return Ok(snapshot);
        }
        if reservation.1 != "active" {
            return Err(StoreError::Corrupt(format!(
                "budget reservation `{reservation_id}` has invalid status `{}`",
                reservation.1
            )));
        }
        let reserved: BudgetCounters = decode(&reservation.2, "budget_reservations.reserved_json")?;
        let mut snapshot = load_budget_snapshot(&transaction, run_id)?;
        snapshot.reserved.saturating_sub_assign(&reserved);
        snapshot.usage.saturating_add_assign(actual);
        snapshot.exceeded = budget_exceeded(
            &snapshot.limits,
            &snapshot.usage,
            &BudgetCounters::default(),
            &BudgetCounters::default(),
        )
        .or(snapshot.exceeded);
        snapshot.updated_at = now;
        transaction.execute(
            "UPDATE budget_reservations
             SET status = 'reconciled', actual_json = ?1, source = ?2, reconciled_at = ?3
             WHERE run_id = ?4 AND reservation_id = ?5",
            params![
                encode(actual)?,
                source,
                now.to_rfc3339(),
                run_id,
                reservation_id
            ],
        )?;
        transaction.execute(
            "UPDATE run_budgets
             SET usage_json = ?1, reserved_json = ?2, exceeded_json = ?3, updated_at = ?4
             WHERE run_id = ?5",
            params![
                encode(&snapshot.usage)?,
                encode(&snapshot.reserved)?,
                snapshot.exceeded.as_ref().map(encode).transpose()?,
                now.to_rfc3339(),
                run_id
            ],
        )?;
        append_audit_tx(
            &transaction,
            run_id,
            if snapshot.exceeded.is_some() {
                "budget.reconciled_exceeded"
            } else {
                "budget.reconciled"
            },
            reservation.0.as_deref(),
            trace_id,
            &serde_json::json!({
                "reservationId": reservation_id,
                "source": source,
                "reserved": reserved,
                "actual": actual,
                "usage": snapshot.usage,
                "exceeded": snapshot.exceeded,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(snapshot)
    }

    pub fn record_wall_time(
        &self,
        run_id: &str,
        elapsed_seconds: u64,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<BudgetSnapshot, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut snapshot = load_budget_snapshot(&transaction, run_id)?;
        snapshot.usage.wall_time_seconds = snapshot.usage.wall_time_seconds.max(elapsed_seconds);
        snapshot.exceeded = budget_exceeded(
            &snapshot.limits,
            &snapshot.usage,
            &snapshot.reserved,
            &BudgetCounters::default(),
        )
        .or(snapshot.exceeded);
        snapshot.updated_at = now;
        transaction.execute(
            "UPDATE run_budgets
             SET usage_json = ?1, exceeded_json = ?2, updated_at = ?3
             WHERE run_id = ?4",
            params![
                encode(&snapshot.usage)?,
                snapshot.exceeded.as_ref().map(encode).transpose()?,
                now.to_rfc3339(),
                run_id
            ],
        )?;
        append_audit_tx(
            &transaction,
            run_id,
            if snapshot.exceeded.is_some() {
                "budget.wall_time_exceeded"
            } else {
                "budget.wall_time_observed"
            },
            None,
            trace_id,
            &serde_json::json!({
                "elapsedSeconds": elapsed_seconds,
                "exceeded": snapshot.exceeded,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(snapshot)
    }

    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.connection
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0)
    }

    pub fn encryption_inventory(&self) -> Result<EncryptionInventory, StoreError> {
        let connection = self.connection.lock();
        encryption_inventory(&connection, &self.protection.read())
    }

    pub fn enable_encryption(
        &self,
        key_id: &str,
        key_reference: &str,
        dry_run: bool,
        now: DateTime<Utc>,
    ) -> Result<EncryptionMigrationReport, StoreError> {
        if self.protection.read().is_enabled() {
            return Err(StoreError::Encryption(
                "state encryption is already enabled; use key rotation".to_owned(),
            ));
        }
        let codec = EncryptionCodec::resolve(key_id, key_reference, self.key_resolver.as_ref())?;
        let mut connection = self.connection.lock();
        let inventory = encryption_inventory(&connection, &StateProtection::Plaintext)?;
        if inventory.encrypted_values != 0 || inventory.invalid_envelopes != 0 {
            return Err(StoreError::Encryption(
                "unencrypted database contains an unexpected encryption envelope".to_owned(),
            ));
        }
        let report = EncryptionMigrationReport {
            operation: "enable".to_owned(),
            dry_run,
            key_id: key_id.to_owned(),
            key_reference: key_reference.to_owned(),
            values_scanned: inventory.protected_values,
            values_rewritten: if dry_run {
                0
            } else {
                inventory.plaintext_values
            },
        };
        if dry_run {
            return Ok(report);
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        rewrite_sensitive_values(
            &transaction,
            &StateProtection::Plaintext,
            &StateProtection::Encrypted(codec.clone()),
        )?;
        transaction.execute(
            "INSERT INTO state_encryption (singleton, format_version, key_id, key_reference, key_check, updated_at) VALUES (1, 1, ?1, ?2, ?3, ?4)",
            params![
                key_id,
                key_reference,
                codec.key_check()?,
                now.to_rfc3339()
            ],
        )?;
        transaction.commit()?;
        *self.protection.write() = StateProtection::Encrypted(codec);
        Ok(report)
    }

    pub fn rotate_encryption_key(
        &self,
        key_id: &str,
        key_reference: &str,
        dry_run: bool,
        now: DateTime<Utc>,
    ) -> Result<EncryptionMigrationReport, StoreError> {
        let current = self.protection.read().clone();
        let StateProtection::Encrypted(current_codec) = &current else {
            return Err(StoreError::Encryption(
                "state encryption is not enabled".to_owned(),
            ));
        };
        if current_codec.key_id() == key_id {
            return Err(StoreError::Encryption(
                "rotation requires a different key ID".to_owned(),
            ));
        }
        let next = EncryptionCodec::resolve(key_id, key_reference, self.key_resolver.as_ref())?;
        let mut connection = self.connection.lock();
        let inventory = encryption_inventory(&connection, &current)?;
        if inventory.plaintext_values != 0 || inventory.invalid_envelopes != 0 {
            return Err(StoreError::Encryption(
                "encrypted database is not in a fully protected state".to_owned(),
            ));
        }
        let report = EncryptionMigrationReport {
            operation: "rotate".to_owned(),
            dry_run,
            key_id: key_id.to_owned(),
            key_reference: key_reference.to_owned(),
            values_scanned: inventory.protected_values,
            values_rewritten: if dry_run {
                0
            } else {
                inventory.encrypted_values
            },
        };
        if dry_run {
            return Ok(report);
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE state_encryption SET format_version = 1, key_id = ?1, key_reference = ?2, key_check = ?3, maintenance = 1, updated_at = ?4 WHERE singleton = 1",
            params![
                key_id,
                key_reference,
                next.key_check()?,
                now.to_rfc3339()
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Encryption(
                "state-encryption configuration disappeared during rotation".to_owned(),
            ));
        }
        rewrite_sensitive_values(
            &transaction,
            &current,
            &StateProtection::Encrypted(next.clone()),
        )?;
        let changed = transaction.execute(
            "UPDATE state_encryption SET maintenance = 0 WHERE singleton = 1 AND maintenance = 1",
            [],
        )?;
        if changed != 1 {
            return Err(StoreError::Encryption(
                "state-encryption configuration disappeared during rotation".to_owned(),
            ));
        }
        transaction.commit()?;
        *self.protection.write() = StateProtection::Encrypted(next);
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_run(
        &self,
        run_id: &str,
        workflow_schema_version: &str,
        workflow: &Value,
        plan: &CompiledPlan,
        inputs: &Value,
        working_memory: &Value,
        mode: RunMode,
        parent_run_id: Option<&str>,
        source_run_id: Option<&str>,
        base_path: &Path,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO runs (run_id, runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, state, mode, parent_run_id, source_run_id, base_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
            params![
                run_id,
                RUNTIME_STATE_VERSION,
                plan.workflow_digest,
                workflow_schema_version,
                plan.plan_digest,
                plan.format_version,
                encode_protected(&self.protection, workflow, "runs.workflow_json")?,
                encode_protected(&self.protection, plan, "runs.plan_json")?,
                encode_protected(&self.protection, inputs, "runs.inputs_json")?,
                encode_protected(
                    &self.protection,
                    working_memory,
                    "runs.working_memory_json"
                )?,
                encode_enum(RunState::Running)?,
                encode_enum(mode)?,
                parent_run_id,
                source_run_id,
                base_path.display().to_string(),
                now.to_rfc3339(),
            ],
        )?;
        for (position, task_id) in plan.order.iter().enumerate() {
            let position = i64::try_from(position).map_err(|_| {
                StoreError::Incompatible("task position exceeds SQLite integer range".to_owned())
            })?;
            transaction.execute(
                "INSERT INTO task_states (run_id, task_id, position, state, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, task_id, position, encode_enum(TaskState::Pending)?, now.to_rfc3339()],
            )?;
        }
        initialize_budget_tx(&transaction, run_id, plan, now)?;
        append_audit_tx(
            &transaction,
            run_id,
            "run.created",
            None,
            trace_id,
            &serde_json::json!({
                "mode": mode,
                "planDigest": plan.plan_digest,
                "sourceRunId": source_run_id,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_repair_run(
        &self,
        run_id: &str,
        source_run_id: &str,
        source_workflow_digest: &str,
        workflow_schema_version: &str,
        workflow: &Value,
        plan: &CompiledPlan,
        inputs: &Value,
        working_memory: &Value,
        repair_roots: &[String],
        reason: Option<&str>,
        reused_tasks: &[ReusedTaskMaterialization],
        task_decisions: &Value,
        base_path: &Path,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let reused = reused_tasks
            .iter()
            .map(|task| (task.task_id.as_str(), task))
            .collect::<BTreeMap<_, _>>();
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        for task in reused_tasks {
            verify_artifact_manifest(
                self.artifact_store.as_ref(),
                &task.metadata.artifact_manifest,
            )?;
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO runs (run_id, runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, state, mode, source_run_id, source_workflow_digest, repair_roots_json, repair_reason, repair_format_version, base_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, ?17, ?18, ?18)",
            params![
                run_id,
                RUNTIME_STATE_VERSION,
                plan.workflow_digest,
                workflow_schema_version,
                plan.plan_digest,
                plan.format_version,
                encode_protected(&self.protection, workflow, "runs.workflow_json")?,
                encode_protected(&self.protection, plan, "runs.plan_json")?,
                encode_protected(&self.protection, inputs, "runs.inputs_json")?,
                encode_protected(
                    &self.protection,
                    working_memory,
                    "runs.working_memory_json"
                )?,
                encode_enum(RunState::Running)?,
                encode_enum(RunMode::Repair)?,
                source_run_id,
                source_workflow_digest,
                encode(repair_roots)?,
                reason
                    .map(|value| protect_text(&self.protection, value, "runs.repair_reason"))
                    .transpose()?,
                base_path.display().to_string(),
                now.to_rfc3339(),
            ],
        )?;
        for (position, task_id) in plan.order.iter().enumerate() {
            let position = i64::try_from(position).map_err(|_| {
                StoreError::Incompatible("task position exceeds SQLite integer range".to_owned())
            })?;
            if let Some(task) = reused.get(task_id.as_str()) {
                transaction.execute(
                    "INSERT INTO task_states (run_id, task_id, position, state, attempt, output_json, disposition, metadata_version, source_run_id, source_task_id, source_attempt, definition_fingerprint, input_digest, output_contract_fingerprint, output_digest, state_delta_json, state_delta_digest, artifact_manifest_json, reuse_decision_json, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                    params![
                        run_id,
                        task_id,
                        position,
                        encode_enum(TaskState::Succeeded)?,
                        encode_protected(
                            &self.protection,
                            &task.output,
                            "task_states.output_json"
                        )?,
                        encode_enum(TaskDisposition::Reused)?,
                        task.metadata.execution.metadata_version,
                        task.source_run_id,
                        task.source_task_id,
                        task.source_attempt,
                        task.metadata.execution.definition_fingerprint,
                        task.metadata.execution.input_digest,
                        task.metadata.execution.output_contract_fingerprint,
                        task.metadata.output_digest,
                        encode_protected(
                            &self.protection,
                            &task.metadata.state_delta,
                            "task_states.state_delta_json"
                        )?,
                        task.metadata.state_delta_digest,
                        encode(&task.metadata.artifact_manifest)?,
                        encode_protected(
                            &self.protection,
                            &task.reuse_decision,
                            "task_states.reuse_decision_json"
                        )?,
                        now.to_rfc3339(),
                    ],
                )?;
                append_audit_tx(
                    &transaction,
                    run_id,
                    "repair.task_reused",
                    Some(task_id),
                    trace_id,
                    &serde_json::json!({
                        "sourceRunId": task.source_run_id,
                        "sourceTaskId": task.source_task_id,
                        "sourceAttempt": task.source_attempt,
                        "outputDigest": task.metadata.output_digest,
                        "decision": task.reuse_decision,
                    }),
                    now,
                    &self.protection,
                )?;
                record_artifact_references_tx(
                    &transaction,
                    run_id,
                    task_id,
                    &task.metadata.artifact_manifest,
                    Some(&task.source_run_id),
                    Some(&task.source_task_id),
                    now,
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO task_states (run_id, task_id, position, state, disposition, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        run_id,
                        task_id,
                        position,
                        encode_enum(TaskState::Pending)?,
                        encode_enum(TaskDisposition::Executed)?,
                        now.to_rfc3339(),
                    ],
                )?;
            }
        }
        copy_protocol_records_for_tasks_tx(
            &transaction,
            run_id,
            source_run_id,
            reused.keys().copied(),
            now,
        )?;
        initialize_budget_tx(&transaction, run_id, plan, now)?;
        append_audit_tx(
            &transaction,
            run_id,
            "repair.created",
            None,
            trace_id,
            &serde_json::json!({
                "sourceRunId": source_run_id,
                "sourceWorkflowDigest": source_workflow_digest,
                "targetWorkflowDigest": plan.workflow_digest,
                "repairRoots": repair_roots,
                "reason": reason,
                "reusedTasks": reused_tasks.iter().map(|task| task.task_id.as_str()).collect::<Vec<_>>(),
                "taskDecisions": task_decisions,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_retry_run(
        &self,
        run_id: &str,
        source_run_id: &str,
        source_workflow_digest: &str,
        workflow_schema_version: &str,
        workflow: &Value,
        plan: &CompiledPlan,
        inputs: &Value,
        working_memory: &Value,
        retry_roots: &[String],
        failed_only: bool,
        reason: Option<&str>,
        reused_tasks: &[ReusedTaskMaterialization],
        task_decisions: &Value,
        base_path: &Path,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let reused = reused_tasks
            .iter()
            .map(|task| (task.task_id.as_str(), task))
            .collect::<BTreeMap<_, _>>();
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        for task in reused_tasks {
            verify_artifact_manifest(
                self.artifact_store.as_ref(),
                &task.metadata.artifact_manifest,
            )?;
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO runs (run_id, runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, state, mode, source_run_id, source_workflow_digest, retry_roots_json, retry_reason, retry_format_version, retry_failed_only, base_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, ?17, ?18, ?19, ?19)",
            params![
                run_id,
                RUNTIME_STATE_VERSION,
                plan.workflow_digest,
                workflow_schema_version,
                plan.plan_digest,
                plan.format_version,
                encode_protected(&self.protection, workflow, "runs.workflow_json")?,
                encode_protected(&self.protection, plan, "runs.plan_json")?,
                encode_protected(&self.protection, inputs, "runs.inputs_json")?,
                encode_protected(
                    &self.protection,
                    working_memory,
                    "runs.working_memory_json"
                )?,
                encode_enum(RunState::Running)?,
                encode_enum(RunMode::Retry)?,
                source_run_id,
                source_workflow_digest,
                encode(retry_roots)?,
                reason
                    .map(|value| protect_text(&self.protection, value, "runs.retry_reason"))
                    .transpose()?,
                failed_only,
                base_path.display().to_string(),
                now.to_rfc3339(),
            ],
        )?;
        for (position, task_id) in plan.order.iter().enumerate() {
            let position = i64::try_from(position).map_err(|_| {
                StoreError::Incompatible("task position exceeds SQLite integer range".to_owned())
            })?;
            if let Some(task) = reused.get(task_id.as_str()) {
                transaction.execute(
                    "INSERT INTO task_states (run_id, task_id, position, state, attempt, output_json, disposition, metadata_version, source_run_id, source_task_id, source_attempt, definition_fingerprint, input_digest, output_contract_fingerprint, output_digest, state_delta_json, state_delta_digest, artifact_manifest_json, reuse_decision_json, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                    params![
                        run_id,
                        task_id,
                        position,
                        encode_enum(TaskState::Succeeded)?,
                        encode_protected(
                            &self.protection,
                            &task.output,
                            "task_states.output_json"
                        )?,
                        encode_enum(TaskDisposition::Reused)?,
                        task.metadata.execution.metadata_version,
                        task.source_run_id,
                        task.source_task_id,
                        task.source_attempt,
                        task.metadata.execution.definition_fingerprint,
                        task.metadata.execution.input_digest,
                        task.metadata.execution.output_contract_fingerprint,
                        task.metadata.output_digest,
                        encode_protected(
                            &self.protection,
                            &task.metadata.state_delta,
                            "task_states.state_delta_json"
                        )?,
                        task.metadata.state_delta_digest,
                        encode(&task.metadata.artifact_manifest)?,
                        encode_protected(
                            &self.protection,
                            &task.reuse_decision,
                            "task_states.reuse_decision_json"
                        )?,
                        now.to_rfc3339(),
                    ],
                )?;
                append_audit_tx(
                    &transaction,
                    run_id,
                    "retry.task_reused",
                    Some(task_id),
                    trace_id,
                    &serde_json::json!({
                        "sourceRunId": task.source_run_id,
                        "sourceTaskId": task.source_task_id,
                        "sourceAttempt": task.source_attempt,
                        "outputDigest": task.metadata.output_digest,
                        "decision": task.reuse_decision,
                    }),
                    now,
                    &self.protection,
                )?;
                record_artifact_references_tx(
                    &transaction,
                    run_id,
                    task_id,
                    &task.metadata.artifact_manifest,
                    Some(&task.source_run_id),
                    Some(&task.source_task_id),
                    now,
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO task_states (run_id, task_id, position, state, disposition, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        run_id,
                        task_id,
                        position,
                        encode_enum(TaskState::Pending)?,
                        encode_enum(TaskDisposition::Executed)?,
                        now.to_rfc3339(),
                    ],
                )?;
            }
        }
        copy_protocol_records_for_tasks_tx(
            &transaction,
            run_id,
            source_run_id,
            reused.keys().copied(),
            now,
        )?;
        initialize_budget_tx(&transaction, run_id, plan, now)?;
        append_audit_tx(
            &transaction,
            run_id,
            "retry.created",
            None,
            trace_id,
            &serde_json::json!({
                "sourceRunId": source_run_id,
                "sourceWorkflowDigest": source_workflow_digest,
                "workflowDigest": plan.workflow_digest,
                "retryRoots": retry_roots,
                "failedOnly": failed_only,
                "reason": reason,
                "reusedTasks": reused_tasks.iter().map(|task| task.task_id.as_str()).collect::<Vec<_>>(),
                "taskDecisions": task_decisions,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_run(&self, run_id: &str) -> Result<RunRecord, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, output_json, state, mode, parent_run_id, cancellation_requested, created_at, updated_at, base_path, source_run_id, source_workflow_digest, repair_roots_json, repair_reason, repair_format_version, retry_roots_json, retry_reason, retry_format_version, retry_failed_only FROM runs WHERE run_id = ?1",
                [run_id],
                |row| {
                    let state_version: u32 = row.get(0)?;
                    let plan_version: u32 = row.get(4)?;
                    Ok((
                        state_version,
                        plan_version,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, bool>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, String>(15)?,
                        row.get::<_, Option<String>>(16)?,
                        row.get::<_, Option<String>>(17)?,
                        row.get::<_, Option<String>>(18)?,
                        row.get::<_, Option<String>>(19)?,
                        row.get::<_, Option<String>>(20)?,
                        row.get::<_, Option<u32>>(21)?,
                        row.get::<_, Option<String>>(22)?,
                        row.get::<_, Option<String>>(23)?,
                        row.get::<_, Option<u32>>(24)?,
                        row.get::<_, bool>(25)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound(run_id.to_owned()))
            .and_then(|row| {
                if row.0 != RUNTIME_STATE_VERSION {
                    return Err(StoreError::Incompatible(format!(
                        "run state version {} is not supported",
                        row.0
                    )));
                }
                if row.1 != PLAN_FORMAT_VERSION {
                    return Err(StoreError::Incompatible(format!(
                        "plan format version {} is not supported",
                        row.1
                    )));
                }
                Ok(RunRecord {
                    run_id: run_id.to_owned(),
                    workflow_digest: row.2,
                    workflow_schema_version: row.3,
                    plan_digest: row.4,
                    workflow: decode_protected(
                        &self.protection,
                        &row.5,
                        "runs.workflow_json",
                    )?,
                    plan: decode_protected(&self.protection, &row.6, "runs.plan_json")?,
                    inputs: decode_protected(&self.protection, &row.7, "runs.inputs_json")?,
                    working_memory: decode_protected(
                        &self.protection,
                        &row.8,
                        "runs.working_memory_json",
                    )?,
                    output: row
                        .9
                        .map(|value| {
                            decode_protected(&self.protection, &value, "runs.output_json")
                        })
                        .transpose()?,
                    state: decode_enum(&row.10, "run.state")?,
                    mode: decode_enum(&row.11, "run.mode")?,
                    parent_run_id: row.12,
                    source_run_id: row.17,
                    source_workflow_digest: row.18,
                    repair_roots: row
                        .19
                        .map(|value| decode(&value, "run.repair_roots"))
                        .transpose()?
                        .unwrap_or_default(),
                    repair_reason: row
                        .20
                        .map(|value| {
                            expose_text(&self.protection, &value, "runs.repair_reason")
                        })
                        .transpose()?,
                    repair_format_version: row.21,
                    retry_roots: row
                        .22
                        .map(|value| decode(&value, "run.retry_roots"))
                        .transpose()?
                        .unwrap_or_default(),
                    retry_reason: row
                        .23
                        .map(|value| expose_text(&self.protection, &value, "runs.retry_reason"))
                        .transpose()?,
                    retry_format_version: row.24,
                    retry_failed_only: row.25,
                    base_path: row.16,
                    cancellation_requested: row.13,
                    created_at: parse_time(&row.14, "created_at")?,
                    updated_at: parse_time(&row.15, "updated_at")?,
                })
            })
    }

    pub fn compensation_runs(
        &self,
        source_run_id: &str,
    ) -> Result<Vec<(String, RunState)>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT run_id, state FROM runs
             WHERE source_run_id = ?1 AND mode = ?2
             ORDER BY created_at, run_id",
        )?;
        statement
            .query_map(
                params![source_run_id, encode_enum(RunMode::Compensation)?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .map(|row| {
                let (run_id, state) = row?;
                Ok((run_id, decode_enum(&state, "run.state")?))
            })
            .collect()
    }

    pub fn record_replay_effects_reused(
        &self,
        replay_run_id: &str,
        source_run_id: &str,
        effects: &[EffectRecord],
        tool_calls: &[ToolCallRecord],
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let effects = effects
            .iter()
            .map(|effect| {
                serde_json::json!({
                    "effectId": effect.request.id,
                    "taskId": effect.request.task_id,
                    "effectClass": effect.request.effect_class,
                    "status": effect.status,
                    "confirmed": effect.confirmed,
                })
            })
            .collect::<Vec<_>>();
        let tool_calls = tool_calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "callId": call.call_id,
                    "effectId": call.effect_id,
                    "taskId": call.task_id,
                    "toolId": call.tool_id,
                    "status": call.status,
                })
            })
            .collect::<Vec<_>>();
        append_audit_tx(
            &transaction,
            replay_run_id,
            "replay.effects_reused",
            None,
            trace_id,
            &serde_json::json!({
                "sourceRunId": source_run_id,
                "effects": effects,
                "toolCalls": tool_calls,
            }),
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_tasks(&self, run_id: &str) -> Result<Vec<TaskRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT task_id, position, state, attempt, output_json, error, updated_at, disposition, metadata_version, source_run_id, source_task_id, source_attempt, definition_fingerprint, input_digest, output_contract_fingerprint, output_digest, state_delta_json, state_delta_digest, artifact_manifest_json, reuse_decision_json, execution_memory_json FROM task_states WHERE run_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u16>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<u32>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<u16>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<String>>(18)?,
                row.get::<_, Option<String>>(19)?,
                row.get::<_, Option<String>>(20)?,
            ))
        })?;
        rows.map(|row| {
            let row = row?;
            Ok(TaskRecord {
                run_id: run_id.to_owned(),
                task_id: row.0,
                position: row.1,
                state: decode_enum(&row.2, "task.state")?,
                attempt: row.3,
                output: row
                    .4
                    .map(|value| {
                        decode_protected(&self.protection, &value, "task_states.output_json")
                    })
                    .transpose()?,
                error: row
                    .5
                    .map(|value| expose_text(&self.protection, &value, "task_states.error"))
                    .transpose()?,
                disposition: decode_enum(&row.7, "task.disposition")?,
                metadata_version: row.8,
                source_run_id: row.9,
                source_task_id: row.10,
                source_attempt: row.11,
                definition_fingerprint: row.12,
                input_digest: row.13,
                output_contract_fingerprint: row.14,
                output_digest: row.15,
                state_delta: row
                    .16
                    .map(|value| {
                        decode_protected(&self.protection, &value, "task_states.state_delta_json")
                    })
                    .transpose()?,
                state_delta_digest: row.17,
                artifact_manifest: row
                    .18
                    .map(|value| decode(&value, "task.artifact_manifest"))
                    .transpose()?
                    .unwrap_or_default(),
                reuse_decision: row
                    .19
                    .map(|value| {
                        decode_protected(
                            &self.protection,
                            &value,
                            "task_states.reuse_decision_json",
                        )
                    })
                    .transpose()?,
                execution_memory: row
                    .20
                    .map(|value| {
                        decode_protected(
                            &self.protection,
                            &value,
                            "task_states.execution_memory_json",
                        )
                    })
                    .transpose()?,
                updated_at: parse_time(&row.6, "task.updated_at")?,
            })
        })
        .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn transition_task(
        &self,
        run_id: &str,
        task_id: &str,
        next: TaskState,
        output: Option<&Value>,
        error: Option<&str>,
        working_memory: Option<&Value>,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: String = transaction
            .query_row(
                "SELECT state FROM task_states WHERE run_id = ?1 AND task_id = ?2",
                params![run_id, task_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::TaskNotFound {
                run_id: run_id.to_owned(),
                task_id: task_id.to_owned(),
            })?;
        let current: TaskState = decode_enum(&current, "task.state")?;
        current
            .transition(next)
            .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
        transaction.execute(
            "UPDATE task_states SET state = ?3, output_json = COALESCE(?4, output_json), error = ?5, attempt = attempt + ?7, execution_memory_json = CASE WHEN ?8 = 1 THEN NULL ELSE execution_memory_json END, updated_at = ?6 WHERE run_id = ?1 AND task_id = ?2",
            params![
                run_id,
                task_id,
                encode_enum(next)?,
                output
                    .map(|value| encode_protected(
                        &self.protection,
                        value,
                        "task_states.output_json"
                    ))
                    .transpose()?,
                error
                    .map(|value| protect_text(&self.protection, value, "task_states.error"))
                    .transpose()?,
                now.to_rfc3339(),
                i64::from(current == TaskState::Ready && next == TaskState::Running),
                i64::from(current == TaskState::RetryScheduled && next == TaskState::Ready),
            ],
        )?;
        if let Some(memory) = working_memory {
            transaction.execute(
                "UPDATE runs SET working_memory_json = ?2, updated_at = ?3 WHERE run_id = ?1",
                params![
                    run_id,
                    encode_protected(&self.protection, memory, "runs.working_memory_json")?,
                    now.to_rfc3339()
                ],
            )?;
        } else {
            transaction.execute(
                "UPDATE runs SET updated_at = ?2 WHERE run_id = ?1",
                params![run_id, now.to_rfc3339()],
            )?;
        }
        append_audit_tx(
            &transaction,
            run_id,
            "task.transition",
            Some(task_id),
            trace_id,
            &serde_json::json!({
                "from": current,
                "to": next,
                "error": error,
                "decision": output,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_task_execution_metadata(
        &self,
        run_id: &str,
        task_id: &str,
        metadata: &TaskExecutionMetadata,
        execution_memory: &Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let changed = self.connection.lock().execute(
            "UPDATE task_states SET metadata_version = ?3, definition_fingerprint = ?4, input_digest = ?5, output_contract_fingerprint = ?6, execution_memory_json = ?7, updated_at = ?8 WHERE run_id = ?1 AND task_id = ?2 AND state = ?9",
            params![
                run_id,
                task_id,
                metadata.metadata_version,
                metadata.definition_fingerprint,
                metadata.input_digest,
                metadata.output_contract_fingerprint,
                encode_protected(
                    &self.protection,
                    execution_memory,
                    "task_states.execution_memory_json"
                )?,
                now.to_rfc3339(),
                encode_enum(TaskState::Running)?,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidTransition(format!(
                "task `{task_id}` in run `{run_id}` is not running"
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_task(
        &self,
        run_id: &str,
        task_id: &str,
        output: &Value,
        working_memory: Option<&Value>,
        metadata: &TaskCompletionMetadata,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        self.commit_task_batch(
            run_id,
            &[TaskBatchResult {
                task_id: task_id.to_owned(),
                outcome: TaskBatchOutcome::Succeeded {
                    output: output.clone(),
                    metadata: Box::new(metadata.clone()),
                },
            }],
            working_memory,
            false,
            now,
            trace_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_task_batch(
        &self,
        run_id: &str,
        results: &[TaskBatchResult],
        working_memory: Option<&Value>,
        fail_run: bool,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        for result in results {
            if let TaskBatchOutcome::Succeeded { metadata, .. } = &result.outcome {
                verify_artifact_manifest(
                    self.artifact_store.as_ref(),
                    &metadata.artifact_manifest,
                )?;
            }
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for result in results {
            let current: String = transaction
                .query_row(
                    "SELECT state FROM task_states WHERE run_id = ?1 AND task_id = ?2",
                    params![run_id, result.task_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::TaskNotFound {
                    run_id: run_id.to_owned(),
                    task_id: result.task_id.clone(),
                })?;
            let current: TaskState = decode_enum(&current, "task.state")?;
            let next = match &result.outcome {
                TaskBatchOutcome::Succeeded { .. } => TaskState::Succeeded,
                TaskBatchOutcome::Failed { .. } => TaskState::Failed,
                TaskBatchOutcome::RetryScheduled { .. } => TaskState::RetryScheduled,
            };
            current
                .transition(next)
                .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
            match &result.outcome {
                TaskBatchOutcome::Succeeded { output, metadata } => {
                    transaction.execute(
                        "UPDATE task_states SET state = ?3, output_json = ?4, error = NULL, disposition = ?5, metadata_version = ?6, definition_fingerprint = ?7, input_digest = ?8, output_contract_fingerprint = ?9, output_digest = ?10, state_delta_json = ?11, state_delta_digest = ?12, artifact_manifest_json = ?13, updated_at = ?14 WHERE run_id = ?1 AND task_id = ?2",
                        params![
                            run_id,
                            result.task_id,
                            encode_enum(TaskState::Succeeded)?,
                            encode_protected(
                                &self.protection,
                                output,
                                "task_states.output_json"
                            )?,
                            encode_enum(TaskDisposition::Executed)?,
                            metadata.execution.metadata_version,
                            metadata.execution.definition_fingerprint,
                            metadata.execution.input_digest,
                            metadata.execution.output_contract_fingerprint,
                            metadata.output_digest,
                            encode_protected(
                                &self.protection,
                                &metadata.state_delta,
                                "task_states.state_delta_json"
                            )?,
                            metadata.state_delta_digest,
                            encode(&metadata.artifact_manifest)?,
                            now.to_rfc3339(),
                        ],
                    )?;
                    record_artifact_references_tx(
                        &transaction,
                        run_id,
                        &result.task_id,
                        &metadata.artifact_manifest,
                        None,
                        None,
                        now,
                    )?;
                    transaction.execute(
                        "DELETE FROM artifact_ingests WHERE run_id = ?1 AND task_id = ?2",
                        params![run_id, result.task_id],
                    )?;
                    append_audit_tx(
                        &transaction,
                        run_id,
                        "task.transition",
                        Some(&result.task_id),
                        trace_id,
                        &serde_json::json!({
                            "from": current,
                            "to": TaskState::Succeeded,
                            "disposition": TaskDisposition::Executed,
                            "outputDigest": metadata.output_digest,
                            "stateDeltaDigest": metadata.state_delta_digest,
                        }),
                        now,
                        &self.protection,
                    )?;
                }
                TaskBatchOutcome::Failed { error } | TaskBatchOutcome::RetryScheduled { error } => {
                    transaction.execute(
                        "UPDATE task_states SET state = ?3, error = ?4, updated_at = ?5 WHERE run_id = ?1 AND task_id = ?2",
                        params![
                            run_id,
                            result.task_id,
                            encode_enum(next)?,
                            protect_text(&self.protection, error, "task_states.error")?,
                            now.to_rfc3339(),
                        ],
                    )?;
                    append_audit_tx(
                        &transaction,
                        run_id,
                        "task.transition",
                        Some(&result.task_id),
                        trace_id,
                        &serde_json::json!({"from": current, "to": next, "error": error}),
                        now,
                        &self.protection,
                    )?;
                }
            }
        }

        if fail_run {
            let mut statement = transaction.prepare(
                "SELECT task_id, state FROM task_states WHERE run_id = ?1 ORDER BY position",
            )?;
            let remaining = statement
                .query_map([run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            for (task_id, encoded) in remaining {
                let current: TaskState = decode_enum(&encoded, "task.state")?;
                if current.is_terminal() {
                    continue;
                }
                current
                    .transition(TaskState::Cancelled)
                    .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
                transaction.execute(
                    "UPDATE task_states SET state = ?3, error = ?4, updated_at = ?5 WHERE run_id = ?1 AND task_id = ?2",
                    params![
                        run_id,
                        task_id,
                        encode_enum(TaskState::Cancelled)?,
                        protect_text(
                            &self.protection,
                            "cancelled after a stop-on-failure task failed",
                            "task_states.error"
                        )?,
                        now.to_rfc3339(),
                    ],
                )?;
                append_audit_tx(
                    &transaction,
                    run_id,
                    "task.transition",
                    Some(&task_id),
                    trace_id,
                    &serde_json::json!({
                        "from": current,
                        "to": TaskState::Cancelled,
                        "error": "cancelled after a stop-on-failure task failed",
                    }),
                    now,
                    &self.protection,
                )?;
            }
            transaction.execute(
                "UPDATE effects SET status = ?2, error = ?3, completed_at = ?4, confirmed = 0 WHERE run_id = ?1 AND status IN (?5, ?6)",
                params![
                    run_id,
                    encode_enum(EffectStatus::Cancelled)?,
                    protect_text(
                        &self.protection,
                        "cancelled after a stop-on-failure task failed",
                        "effects.error"
                    )?,
                    now.to_rfc3339(),
                    encode_enum(EffectStatus::Requested)?,
                    encode_enum(EffectStatus::WaitingForApproval)?,
                ],
            )?;
            transaction.execute(
                "UPDATE approvals SET status = 'cancelled', resolved_at = ?2, resolved_by = 'runtime', resolution_reason = ?3 WHERE run_id = ?1 AND status = 'pending'",
                params![
                    run_id,
                    now.to_rfc3339(),
                    protect_text(
                        &self.protection,
                        "run stopped after task failure",
                        "approvals.resolution_reason"
                    )?,
                ],
            )?;
        }

        let current_run_state = if fail_run {
            let encoded: String = transaction
                .query_row(
                    "SELECT state FROM runs WHERE run_id = ?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::RunNotFound(run_id.to_owned()))?;
            let current: RunState = decode_enum(&encoded, "run.state")?;
            current
                .transition(RunState::Failed)
                .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
            Some(current)
        } else {
            None
        };
        match (working_memory, fail_run) {
            (Some(memory), true) => {
                transaction.execute(
                    "UPDATE runs SET working_memory_json = ?2, state = ?3, updated_at = ?4 WHERE run_id = ?1",
                    params![
                        run_id,
                        encode_protected(
                            &self.protection,
                            memory,
                            "runs.working_memory_json"
                        )?,
                        encode_enum(RunState::Failed)?,
                        now.to_rfc3339()
                    ],
                )?;
            }
            (Some(memory), false) => {
                transaction.execute(
                    "UPDATE runs SET working_memory_json = ?2, updated_at = ?3 WHERE run_id = ?1",
                    params![
                        run_id,
                        encode_protected(&self.protection, memory, "runs.working_memory_json")?,
                        now.to_rfc3339()
                    ],
                )?;
            }
            (None, true) => {
                transaction.execute(
                    "UPDATE runs SET state = ?2, updated_at = ?3 WHERE run_id = ?1",
                    params![run_id, encode_enum(RunState::Failed)?, now.to_rfc3339()],
                )?;
            }
            (None, false) => {
                transaction.execute(
                    "UPDATE runs SET updated_at = ?2 WHERE run_id = ?1",
                    params![run_id, now.to_rfc3339()],
                )?;
            }
        }
        if let Some(current) = current_run_state {
            append_audit_tx(
                &transaction,
                run_id,
                "run.state",
                None,
                trace_id,
                &serde_json::json!({"from": current, "to": RunState::Failed}),
                now,
                &self.protection,
            )?;
        }
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_replayed_task_metadata(
        &self,
        replay_run_id: &str,
        source: &TaskRecord,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        verify_artifact_manifest(self.artifact_store.as_ref(), &source.artifact_manifest)?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE task_states SET disposition = ?3, metadata_version = ?4, source_run_id = ?5, source_task_id = ?6, source_attempt = ?7, definition_fingerprint = ?8, input_digest = ?9, output_contract_fingerprint = ?10, output_digest = ?11, state_delta_json = ?12, state_delta_digest = ?13, artifact_manifest_json = ?14, reuse_decision_json = ?15, updated_at = ?16 WHERE run_id = ?1 AND task_id = ?2",
            params![
                replay_run_id,
                source.task_id,
                encode_enum(TaskDisposition::Recorded)?,
                source.metadata_version,
                source.run_id,
                source.task_id,
                source.attempt,
                source.definition_fingerprint,
                source.input_digest,
                source.output_contract_fingerprint,
                source.output_digest,
                source
                    .state_delta
                    .as_ref()
                    .map(|value| encode_protected(
                        &self.protection,
                        value,
                        "task_states.state_delta_json"
                    ))
                    .transpose()?,
                source.state_delta_digest,
                encode(&source.artifact_manifest)?,
                encode_protected(
                    &self.protection,
                    &serde_json::json!({
                        "recordedFromRunId": source.run_id,
                        "sourceDisposition": source.disposition,
                        "sourceProvenance": source.reuse_decision,
                    }),
                    "task_states.reuse_decision_json"
                )?,
                now.to_rfc3339(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::TaskNotFound {
                run_id: replay_run_id.to_owned(),
                task_id: source.task_id.clone(),
            });
        }
        record_artifact_references_tx(
            &transaction,
            replay_run_id,
            &source.task_id,
            &source.artifact_manifest,
            Some(&source.run_id),
            Some(&source.task_id),
            now,
        )?;
        append_audit_tx(
            &transaction,
            replay_run_id,
            "replay.task_recorded",
            Some(&source.task_id),
            trace_id,
            &serde_json::json!({
                "sourceRunId": source.run_id,
                "sourceTaskId": source.task_id,
                "sourceDisposition": source.disposition,
                "outputDigest": source.output_digest,
            }),
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn apply_legacy_run_upgrade(
        &self,
        upgrade_id: &str,
        run_id: &str,
        analysis: &Value,
        updates: &[LegacyTaskUpgrade],
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        for update in updates {
            verify_artifact_manifest(
                self.artifact_store.as_ref(),
                &update.metadata.artifact_manifest,
            )?;
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_state: String = transaction
            .query_row(
                "SELECT state FROM runs WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound(run_id.to_owned()))?;
        let run_state: RunState = decode_enum(&run_state, "run.state")?;
        if !run_state.is_terminal() {
            return Err(StoreError::Incompatible(format!(
                "legacy run upgrade requires a terminal run, found {run_state:?}"
            )));
        }
        for update in updates {
            let changed = transaction.execute(
                "UPDATE task_states SET metadata_version = ?3, definition_fingerprint = ?4, input_digest = ?5, output_contract_fingerprint = ?6, output_digest = ?7, state_delta_json = ?8, state_delta_digest = ?9, artifact_manifest_json = ?10, reuse_decision_json = ?11, updated_at = ?12 WHERE run_id = ?1 AND task_id = ?2 AND state = ?13 AND metadata_version IS NULL",
                params![
                    run_id,
                    update.task_id,
                    update.metadata.execution.metadata_version,
                    update.metadata.execution.definition_fingerprint,
                    update.metadata.execution.input_digest,
                    update.metadata.execution.output_contract_fingerprint,
                    update.metadata.output_digest,
                    encode_protected(
                        &self.protection,
                        &update.metadata.state_delta,
                        "task_states.state_delta_json"
                    )?,
                    update.metadata.state_delta_digest,
                    encode(&update.metadata.artifact_manifest)?,
                    encode_protected(
                        &self.protection,
                        &serde_json::json!({"legacyUpgrade": update.provenance}),
                        "task_states.reuse_decision_json"
                    )?,
                    now.to_rfc3339(),
                    encode_enum(TaskState::Succeeded)?,
                ],
            )?;
            if changed != 1 {
                return Err(StoreError::Incompatible(format!(
                    "legacy task `{}` is missing, not successful, or already upgraded",
                    update.task_id
                )));
            }
            record_artifact_references_tx(
                &transaction,
                run_id,
                &update.task_id,
                &update.metadata.artifact_manifest,
                None,
                None,
                now,
            )?;
            transaction.execute(
                "DELETE FROM artifact_ingests WHERE run_id = ?1 AND task_id = ?2",
                params![run_id, update.task_id],
            )?;
        }
        let upgraded_tasks = updates
            .iter()
            .map(|update| update.task_id.as_str())
            .collect::<Vec<_>>();
        transaction.execute(
            "INSERT INTO run_upgrades (run_id, upgrade_id, format_version, analysis_json, upgraded_tasks_json, created_at) VALUES (?1, ?2, 1, ?3, ?4, ?5)",
            params![
                run_id,
                upgrade_id,
                encode_protected(&self.protection, analysis, "run_upgrades.analysis_json")?,
                encode_protected(
                    &self.protection,
                    &upgraded_tasks,
                    "run_upgrades.upgraded_tasks_json"
                )?,
                now.to_rfc3339(),
            ],
        )?;
        append_audit_tx(
            &transaction,
            run_id,
            "run.legacy_upgraded",
            None,
            trace_id,
            &serde_json::json!({
                "upgradeId": upgrade_id,
                "formatVersion": 1,
                "upgradedTasks": upgraded_tasks,
                "analysis": analysis,
            }),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_run_state(
        &self,
        run_id: &str,
        state: RunState,
        output: Option<&Value>,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: String = transaction
            .query_row(
                "SELECT state FROM runs WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound(run_id.to_owned()))?;
        let current: RunState = decode_enum(&current, "run.state")?;
        current
            .transition(state)
            .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
        let changed = transaction.execute(
            "UPDATE runs SET state = ?2, output_json = COALESCE(?3, output_json), updated_at = ?4 WHERE run_id = ?1",
            params![
                run_id,
                encode_enum(state)?,
                output
                    .map(|value| encode_protected(
                        &self.protection,
                        value,
                        "runs.output_json"
                    ))
                    .transpose()?,
                now.to_rfc3339()
            ],
        )?;
        debug_assert_eq!(changed, 1);
        append_audit_tx(
            &transaction,
            run_id,
            "run.state",
            None,
            trace_id,
            &serde_json::json!({"from": current, "to": state}),
            now,
            &self.protection,
        )?;
        checkpoint_tx(&transaction, run_id, now, &self.protection)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_effect_request(
        &self,
        request: &EffectRequest,
        now: DateTime<Utc>,
    ) -> Result<EffectRecord, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_effect_request_tx(
            &transaction,
            request,
            EffectStatus::Requested,
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(EffectRecord {
            request: request.clone(),
            status: EffectStatus::Requested,
            attempt_number: 1,
            requested_at: now,
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            confirmed: false,
        })
    }

    pub fn mark_effect_started(
        &self,
        effect_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let changed = self.connection.lock().execute(
            "UPDATE effects SET status = ?2, started_at = ?3 WHERE effect_id = ?1 AND status = ?4",
            params![
                effect_id,
                encode_enum(EffectStatus::Started)?,
                now.to_rfc3339(),
                encode_enum(EffectStatus::Requested)?
            ],
        )?;
        if changed == 0 {
            Err(StoreError::EffectNotFound(effect_id.to_owned()))
        } else {
            Ok(())
        }
    }

    pub fn complete_effect(
        &self,
        effect_id: &str,
        result: Result<&Value, &str>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let (status, output, error, confirmed) = match result {
            Ok(output) => (
                EffectStatus::Succeeded,
                Some(encode_protected(
                    &self.protection,
                    output,
                    "effects.result_json",
                )?),
                None,
                true,
            ),
            Err(error) => (
                EffectStatus::Failed,
                None,
                Some(protect_text(&self.protection, error, "effects.error")?),
                false,
            ),
        };
        let changed = self.connection.lock().execute(
            "UPDATE effects SET status = ?2, result_json = ?3, error = ?4, confirmed = ?5, completed_at = ?6 WHERE effect_id = ?1 AND status = ?7",
            params![effect_id, encode_enum(status)?, output, error, confirmed, now.to_rfc3339(), encode_enum(EffectStatus::Started)?],
        )?;
        if changed == 0 {
            Err(StoreError::EffectNotFound(effect_id.to_owned()))
        } else {
            Ok(())
        }
    }

    pub fn mark_effect_uncertain(
        &self,
        effect_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let changed = self.connection.lock().execute(
            "UPDATE effects SET status = ?2, error = ?3, completed_at = ?4, confirmed = 0 WHERE effect_id = ?1 AND status = ?5",
            params![
                effect_id,
                encode_enum(EffectStatus::Uncertain)?,
                protect_text(&self.protection, error, "effects.error")?,
                now.to_rfc3339(),
                encode_enum(EffectStatus::Started)?
            ],
        )?;
        if changed == 0 {
            Err(StoreError::EffectNotFound(effect_id.to_owned()))
        } else {
            Ok(())
        }
    }

    pub fn load_effect(&self, effect_id: &str) -> Result<EffectRecord, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT format_version, run_id, task_id, task_attempt, ordinal, operation, effect_class, risk, idempotency, idempotency_key, input_digest, input_json, expected_effect, trace_id, status, effect_attempt, requested_at, started_at, completed_at, result_json, error, confirmed FROM effects WHERE effect_id = ?1",
                [effect_id],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                        row.get::<_, u16>(3)?, row.get::<_, u16>(4)?, row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?, row.get::<_, String>(7)?, row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?, row.get::<_, String>(10)?, row.get::<_, String>(11)?,
                        row.get::<_, String>(12)?, row.get::<_, String>(13)?, row.get::<_, String>(14)?,
                        row.get::<_, u16>(15)?, row.get::<_, String>(16)?, row.get::<_, Option<String>>(17)?,
                        row.get::<_, Option<String>>(18)?, row.get::<_, Option<String>>(19)?,
                        row.get::<_, Option<String>>(20)?, row.get::<_, bool>(21)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::EffectNotFound(effect_id.to_owned()))
            .and_then(|row| {
                if row.0 != agentctl_core::EFFECT_FORMAT_VERSION {
                    return Err(StoreError::Incompatible(format!("effect format version {}", row.0)));
                }
                Ok(EffectRecord {
                    request: EffectRequest {
                        format_version: row.0,
                        id: effect_id.to_owned(),
                        run_id: row.1,
                        task_id: row.2,
                        attempt: row.3,
                        ordinal: row.4,
                        operation: row.5,
                        effect_class: decode_enum(&row.6, "effect.effect_class")?,
                        risk: decode_enum(&row.7, "effect.risk")?,
                        idempotency: decode_enum(&row.8, "effect.idempotency")?,
                        idempotency_key: row.9,
                        input_digest: row.10,
                        input: decode_protected(
                            &self.protection,
                            &row.11,
                            "effects.input_json",
                        )?,
                        expected_effect: expose_text(
                            &self.protection,
                            &row.12,
                            "effects.expected_effect",
                        )?,
                        trace_id: row.13,
                    },
                    status: decode_enum(&row.14, "effect.status")?,
                    attempt_number: row.15,
                    requested_at: parse_time(&row.16, "effect.requested_at")?,
                    started_at: row.17.map(|value| parse_time(&value, "effect.started_at")).transpose()?,
                    completed_at: row.18.map(|value| parse_time(&value, "effect.completed_at")).transpose()?,
                    result: row
                        .19
                        .map(|value| {
                            decode_protected(
                                &self.protection,
                                &value,
                                "effects.result_json",
                            )
                        })
                        .transpose()?,
                    error: row
                        .20
                        .map(|value| expose_text(&self.protection, &value, "effects.error"))
                        .transpose()?,
                    confirmed: row.21,
                })
            })
    }

    pub fn unresolved_effects(&self, run_id: &str) -> Result<Vec<String>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT e.effect_id FROM effects e
             WHERE e.run_id = ?1
               AND e.status IN ('started', 'uncertain')
               AND NOT EXISTS (
                 SELECT 1 FROM effect_reconciliations r WHERE r.effect_id = e.effect_id
               )
             ORDER BY e.rowid",
        )?;
        statement
            .query_map([run_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn reconcile_effect(
        &self,
        request: &EffectReconciliationRequest,
        now: DateTime<Utc>,
    ) -> Result<EffectReconciliationRecord, StoreError> {
        if request.actor.trim().is_empty() || request.reason.trim().is_empty() {
            return Err(StoreError::Incompatible(
                "effect reconciliation requires a non-empty actor and reason".to_owned(),
            ));
        }
        if request.status == ReconciliationStatus::Applied && request.result.is_none() {
            return Err(StoreError::Incompatible(
                "an applied reconciliation requires an externally confirmed result".to_owned(),
            ));
        }
        if request.status == ReconciliationStatus::Compensated
            && request.compensation_effect_id.is_none()
        {
            return Err(StoreError::Incompatible(
                "a compensated reconciliation requires a linked compensation effect".to_owned(),
            ));
        }

        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source: (String, String, bool) = transaction
            .query_row(
                "SELECT run_id, status, confirmed FROM effects WHERE effect_id = ?1",
                [&request.effect_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| StoreError::EffectNotFound(request.effect_id.clone()))?;
        let source_status: EffectStatus = decode_enum(&source.1, "effect.status")?;
        let previous = transaction
            .query_row(
                "SELECT reconciliation_id, effect_id, run_id, format_version, status, actor, reason, evidence_json, result_json, result_schema_json, authorization_json, compensation_effect_id, supersedes_id, trace_id, created_at
                 FROM effect_reconciliations
                 WHERE effect_id = ?1
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                [&request.effect_id],
                decode_reconciliation_row,
            )
            .optional()?
            .map(|row| reconciliation_from_row(row, &self.protection))
            .transpose()?;

        if let Some(previous) = &previous {
            let allowed = previous.status == request.status
                || (previous.status == ReconciliationStatus::Applied
                    && request.status == ReconciliationStatus::Compensated);
            if !allowed {
                return Err(StoreError::Incompatible(format!(
                    "effect `{}` is already reconciled as {:?}; contradictory {:?} reconciliation is forbidden",
                    request.effect_id, previous.status, request.status
                )));
            }
        } else {
            let uncertain_source = matches!(
                source_status,
                EffectStatus::Started | EffectStatus::Uncertain
            );
            let confirmed_source = source_status == EffectStatus::Succeeded && source.2;
            let allowed = match request.status {
                ReconciliationStatus::Applied | ReconciliationStatus::NotApplied => {
                    uncertain_source
                }
                ReconciliationStatus::Compensated => confirmed_source,
            };
            if !allowed {
                return Err(StoreError::Incompatible(format!(
                    "effect `{}` in state {:?} cannot be reconciled as {:?}",
                    request.effect_id, source_status, request.status
                )));
            }
        }

        if let Some(compensation_effect_id) = &request.compensation_effect_id {
            if compensation_effect_id == &request.effect_id {
                return Err(StoreError::Incompatible(
                    "an effect cannot compensate itself".to_owned(),
                ));
            }
            let compensation: (String, String, bool, Option<String>, String) = transaction
                .query_row(
                    "SELECT e.run_id, e.status, e.confirmed, r.source_run_id, r.mode
                     FROM effects e
                     JOIN runs r ON r.run_id = e.run_id
                     WHERE e.effect_id = ?1",
                    [compensation_effect_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| StoreError::EffectNotFound(compensation_effect_id.clone()))?;
            let compensation_mode: RunMode = decode_enum(&compensation.4, "compensation_run.mode")?;
            let linked_compensation_run = compensation_mode == RunMode::Compensation
                && compensation.3.as_deref() == Some(source.0.as_str());
            if compensation.0 != source.0 && !linked_compensation_run {
                return Err(StoreError::Incompatible(
                    "compensation effect must belong to the same run or a source-linked compensation run"
                        .to_owned(),
                ));
            }
            let compensation_status: EffectStatus =
                decode_enum(&compensation.1, "compensation_effect.status")?;
            let reconciled_applied = transaction
                .query_row(
                    "SELECT status FROM effect_reconciliations WHERE effect_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                    [compensation_effect_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|status| decode_enum::<ReconciliationStatus>(&status, "compensation_reconciliation.status"))
                .transpose()?
                == Some(ReconciliationStatus::Applied);
            if !(compensation_status == EffectStatus::Succeeded && compensation.2
                || reconciled_applied)
            {
                return Err(StoreError::Incompatible(format!(
                    "compensation effect `{compensation_effect_id}` is not confirmed applied"
                )));
            }
        }

        let supersedes_id = previous
            .as_ref()
            .map(|record| record.reconciliation_id.as_str());
        transaction.execute(
            "INSERT INTO effect_reconciliations (reconciliation_id, effect_id, run_id, format_version, status, actor, reason, evidence_json, result_json, result_schema_json, authorization_json, compensation_effect_id, supersedes_id, trace_id, created_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                request.reconciliation_id,
                request.effect_id,
                source.0,
                encode_enum(request.status)?,
                request.actor,
                protect_text(
                    &self.protection,
                    &request.reason,
                    "effect_reconciliations.reason"
                )?,
                encode_protected(
                    &self.protection,
                    &request.evidence,
                    "effect_reconciliations.evidence_json"
                )?,
                request
                    .result
                    .as_ref()
                    .map(|value| encode_protected(
                        &self.protection,
                        value,
                        "effect_reconciliations.result_json"
                    ))
                    .transpose()?,
                request
                    .result_schema
                    .as_ref()
                    .map(|value| encode_protected(
                        &self.protection,
                        value,
                        "effect_reconciliations.result_schema_json"
                    ))
                    .transpose()?,
                encode_protected(
                    &self.protection,
                    &request.authorization,
                    "effect_reconciliations.authorization_json"
                )?,
                request.compensation_effect_id,
                supersedes_id,
                request.trace_id,
                now.to_rfc3339(),
            ],
        )?;
        let payload = serde_json::json!({
            "reconciliationId": request.reconciliation_id,
            "effectId": request.effect_id,
            "status": request.status,
            "actor": request.actor,
            "reason": request.reason,
            "hasEvidence": !request.evidence.is_null(),
            "hasResult": request.result.is_some(),
            "compensationEffectId": request.compensation_effect_id,
            "supersedesId": supersedes_id,
        });
        append_audit_tx(
            &transaction,
            &source.0,
            "effect.reconciled",
            None,
            &request.trace_id,
            &payload,
            now,
            &self.protection,
        )?;
        append_trace_tx(
            &transaction,
            &source.0,
            &request.trace_id,
            &serde_json::json!({
                "spanKind": "effect",
                "phase": "completed",
                "name": "effect.reconcile",
                "runId": source.0,
                "traceId": request.trace_id,
                "effectId": request.effect_id,
                "timestamp": now,
                "attributes": payload,
            }),
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(EffectReconciliationRecord {
            reconciliation_id: request.reconciliation_id.clone(),
            effect_id: request.effect_id.clone(),
            run_id: source.0,
            format_version: 1,
            status: request.status,
            actor: request.actor.clone(),
            reason: request.reason.clone(),
            evidence: request.evidence.clone(),
            result: request.result.clone(),
            result_schema: request.result_schema.clone(),
            authorization: request.authorization.clone(),
            compensation_effect_id: request.compensation_effect_id.clone(),
            supersedes_id: supersedes_id.map(ToOwned::to_owned),
            trace_id: request.trace_id.clone(),
            created_at: now,
        })
    }

    pub fn reconcile_effect_not_applied(
        &self,
        effect_id: &str,
        actor: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let identity = format!("{effect_id}\0{actor}\0{reason}\0{}", now.to_rfc3339());
        self.reconcile_effect(
            &EffectReconciliationRequest {
                reconciliation_id: format!(
                    "reconciliation-{}",
                    hex::encode(Sha256::digest(identity.as_bytes()))
                ),
                effect_id: effect_id.to_owned(),
                status: ReconciliationStatus::NotApplied,
                actor: actor.to_owned(),
                reason: reason.to_owned(),
                evidence: serde_json::json!({"source": "legacy-api"}),
                result: None,
                result_schema: None,
                authorization: serde_json::json!({"kind": "explicit-operator-action"}),
                compensation_effect_id: None,
                trace_id: "operator-reconciliation".to_owned(),
            },
            now,
        )
        .map(|_| ())
    }

    pub fn latest_effect_reconciliation(
        &self,
        effect_id: &str,
    ) -> Result<Option<EffectReconciliationRecord>, StoreError> {
        self.connection
            .lock()
            .query_row(
                "SELECT reconciliation_id, effect_id, run_id, format_version, status, actor, reason, evidence_json, result_json, result_schema_json, authorization_json, compensation_effect_id, supersedes_id, trace_id, created_at
                 FROM effect_reconciliations
                 WHERE effect_id = ?1
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                [effect_id],
                decode_reconciliation_row,
            )
            .optional()?
            .map(|row| reconciliation_from_row(row, &self.protection))
            .transpose()
    }

    pub fn effect_reconciliations(
        &self,
        effect_id: &str,
    ) -> Result<Vec<EffectReconciliationRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT reconciliation_id, effect_id, run_id, format_version, status, actor, reason, evidence_json, result_json, result_schema_json, authorization_json, compensation_effect_id, supersedes_id, trace_id, created_at
             FROM effect_reconciliations
             WHERE effect_id = ?1
             ORDER BY created_at, rowid",
        )?;
        statement
            .query_map([effect_id], decode_reconciliation_row)?
            .map(|row| reconciliation_from_row(row?, &self.protection))
            .collect()
    }

    pub fn run_effect_reconciliations(
        &self,
        run_id: &str,
    ) -> Result<Vec<EffectReconciliationRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT reconciliation_id, effect_id, run_id, format_version, status, actor, reason, evidence_json, result_json, result_schema_json, authorization_json, compensation_effect_id, supersedes_id, trace_id, created_at
             FROM effect_reconciliations
             WHERE run_id = ?1
             ORDER BY created_at, rowid",
        )?;
        statement
            .query_map([run_id], decode_reconciliation_row)?
            .map(|row| reconciliation_from_row(row?, &self.protection))
            .collect()
    }

    pub fn latest_effect_for_task(
        &self,
        run_id: &str,
        task_id: &str,
    ) -> Result<Option<EffectRecord>, StoreError> {
        let effect_id: Option<String> = self
            .connection
            .lock()
            .query_row(
                "SELECT effect_id FROM effects WHERE run_id = ?1 AND task_id = ?2 ORDER BY rowid DESC LIMIT 1",
                params![run_id, task_id],
                |row| row.get(0),
            )
            .optional()?;
        effect_id.map(|id| self.load_effect(&id)).transpose()
    }

    pub fn create_approval(
        &self,
        effect: &EffectRequest,
        request: &ApprovalRequest,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: String = transaction
            .query_row(
                "SELECT state FROM task_states WHERE run_id = ?1 AND task_id = ?2",
                params![request.run_id, request.task_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::TaskNotFound {
                run_id: request.run_id.clone(),
                task_id: request.task_id.clone(),
            })?;
        let current: TaskState = decode_enum(&current, "task.state")?;
        current
            .transition(TaskState::WaitingForApproval)
            .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
        insert_effect_request_tx(
            &transaction,
            effect,
            EffectStatus::WaitingForApproval,
            request.requested_at,
            &self.protection,
        )?;
        transaction.execute(
            "INSERT INTO approvals (approval_id, run_id, effect_id, task_id, agent, tool, capability, risk, redacted_input_json, expected_effect, reason, trace_id, status, requested_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13)",
            params![
                request.approval_id,
                request.run_id,
                request.effect_id,
                request.task_id,
                request.agent,
                request.tool,
                request.capability,
                request.risk,
                encode_protected(
                    &self.protection,
                    &request.redacted_input,
                    "approvals.redacted_input_json"
                )?,
                protect_text(
                    &self.protection,
                    &request.expected_effect,
                    "approvals.expected_effect"
                )?,
                protect_text(&self.protection, &request.reason, "approvals.reason")?,
                request.trace_id,
                request.requested_at.to_rfc3339()
            ],
        )?;
        transaction.execute(
            "UPDATE task_states SET state = ?3, error = NULL, updated_at = ?4 WHERE run_id = ?1 AND task_id = ?2",
            params![
                request.run_id,
                request.task_id,
                encode_enum(TaskState::WaitingForApproval)?,
                request.requested_at.to_rfc3339(),
            ],
        )?;
        append_audit_tx(
            &transaction,
            &request.run_id,
            "task.transition",
            Some(&request.task_id),
            &request.trace_id,
            &serde_json::json!({
                "from": current,
                "to": TaskState::WaitingForApproval,
            }),
            request.requested_at,
            &self.protection,
        )?;
        checkpoint_tx(
            &transaction,
            &request.run_id,
            request.requested_at,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        resolution: ApprovalResolution,
        actor: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let effect_id: Option<String> = transaction
            .query_row(
                "SELECT effect_id FROM approvals WHERE approval_id = ?1 AND status = 'pending'",
                [approval_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(effect_id) = effect_id else {
            return Err(StoreError::ApprovalNotPending(approval_id.to_owned()));
        };
        let status = match resolution {
            ApprovalResolution::Approved => "approved",
            ApprovalResolution::Rejected => "rejected",
        };
        transaction.execute(
            "UPDATE approvals SET status = ?2, resolved_at = ?3, resolved_by = ?4, resolution_reason = ?5 WHERE approval_id = ?1",
            params![
                approval_id,
                status,
                now.to_rfc3339(),
                actor,
                protect_text(
                    &self.protection,
                    reason,
                    "approvals.resolution_reason"
                )?
            ],
        )?;
        let effect_status = match resolution {
            ApprovalResolution::Approved => EffectStatus::Requested,
            ApprovalResolution::Rejected => EffectStatus::Cancelled,
        };
        transaction.execute(
            "UPDATE effects SET status = ?2 WHERE effect_id = ?1",
            params![effect_id, encode_enum(effect_status)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_approvals(&self, run_id: &str) -> Result<Vec<ApprovalRequest>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT approval_id, effect_id, task_id, agent, tool, capability, risk, redacted_input_json, expected_effect, reason, trace_id, requested_at FROM approvals WHERE run_id = ?1 AND status = 'pending' ORDER BY requested_at, approval_id",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                Ok(ApprovalRequest {
                    approval_id: row.0,
                    run_id: run_id.to_owned(),
                    effect_id: row.1,
                    task_id: row.2,
                    agent: row.3,
                    tool: row.4,
                    capability: row.5,
                    risk: row.6,
                    redacted_input: decode_protected(
                        &self.protection,
                        &row.7,
                        "approvals.redacted_input_json",
                    )?,
                    expected_effect: expose_text(
                        &self.protection,
                        &row.8,
                        "approvals.expected_effect",
                    )?,
                    reason: expose_text(&self.protection, &row.9, "approvals.reason")?,
                    trace_id: row.10,
                    requested_at: parse_time(&row.11, "approval.requested_at")?,
                })
            })
            .collect()
    }

    pub fn request_cancellation(
        &self,
        run_id: &str,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE runs SET cancellation_requested = 1, updated_at = ?2 WHERE run_id = ?1 AND state IN ('running', 'paused')",
            params![run_id, now.to_rfc3339()],
        )?;
        if changed == 0 {
            return Err(StoreError::RunNotFound(run_id.to_owned()));
        }
        append_audit_tx(
            &transaction,
            run_id,
            "run.cancellation_requested",
            None,
            trace_id,
            &Value::Null,
            now,
            &self.protection,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn audit_events(&self, run_id: &str) -> Result<Vec<AuditEvent>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT sequence, event_type, task_id, trace_id, payload_json, created_at, event_version FROM audit_events WHERE run_id = ?1 ORDER BY sequence",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.6 != AUDIT_EVENT_VERSION {
                    return Err(StoreError::Incompatible(format!(
                        "audit event version {}",
                        row.6
                    )));
                }
                Ok(AuditEvent {
                    sequence: row.0,
                    event_type: row.1,
                    task_id: row.2,
                    trace_id: row.3,
                    payload: decode_protected(
                        &self.protection,
                        &row.4,
                        "audit_events.payload_json",
                    )?,
                    created_at: parse_time(&row.5, "audit.created_at")?,
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_stream_event(
        &self,
        run_id: &str,
        task_id: &str,
        task_attempt: u16,
        effect_id: Option<&str>,
        event_type: &str,
        provider_sequence: Option<i64>,
        payload: &Value,
        truncated: bool,
        now: DateTime<Utc>,
    ) -> Result<StreamEventRecord, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sequence: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM stream_events
             WHERE run_id = ?1 AND task_id = ?2 AND task_attempt = ?3",
            params![run_id, task_id, task_attempt],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO stream_events
             (run_id, task_id, task_attempt, sequence, format_version, effect_id, event_type, provider_sequence, payload_json, truncated, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run_id,
                task_id,
                task_attempt,
                sequence,
                STREAM_EVENT_FORMAT_VERSION,
                effect_id,
                event_type,
                provider_sequence,
                encode_protected(&self.protection, payload, "stream_events.payload_json")?,
                truncated,
                now.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(StreamEventRecord {
            run_id: run_id.to_owned(),
            task_id: task_id.to_owned(),
            task_attempt,
            sequence,
            effect_id: effect_id.map(ToOwned::to_owned),
            event_type: event_type.to_owned(),
            provider_sequence,
            payload: payload.clone(),
            truncated,
            source_run_id: None,
            source_sequence: None,
            created_at: now,
        })
    }

    pub fn stream_events(&self, run_id: &str) -> Result<Vec<StreamEventRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT s.task_id, s.task_attempt, s.sequence, s.format_version,
                    s.effect_id, s.event_type, s.provider_sequence, s.payload_json,
                    s.truncated, s.source_run_id, s.source_sequence, s.created_at
             FROM stream_events s
             JOIN task_states t ON t.run_id = s.run_id AND t.task_id = s.task_id
             WHERE s.run_id = ?1
             ORDER BY t.position, s.task_attempt, s.sequence",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u16>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, String>(11)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.3 != STREAM_EVENT_FORMAT_VERSION {
                    return Err(StoreError::Incompatible(format!(
                        "stream event format version {}",
                        row.3
                    )));
                }
                Ok(StreamEventRecord {
                    run_id: run_id.to_owned(),
                    task_id: row.0,
                    task_attempt: row.1,
                    sequence: row.2,
                    effect_id: row.4,
                    event_type: row.5,
                    provider_sequence: row.6,
                    payload: decode_protected(
                        &self.protection,
                        &row.7,
                        "stream_events.payload_json",
                    )?,
                    truncated: row.8,
                    source_run_id: row.9,
                    source_sequence: row.10,
                    created_at: parse_time(&row.11, "stream_event.created_at")?,
                })
            })
            .collect()
    }

    pub fn stream_event_count(
        &self,
        run_id: &str,
        task_id: &str,
        task_attempt: u16,
    ) -> Result<usize, StoreError> {
        let count = self.connection.lock().query_row(
            "SELECT COUNT(*) FROM stream_events
             WHERE run_id = ?1 AND task_id = ?2 AND task_attempt = ?3",
            params![run_id, task_id, task_attempt],
            |row| row.get::<_, i64>(0),
        )?;
        usize::try_from(count)
            .map_err(|_| StoreError::Corrupt("stream event count is invalid".to_owned()))
    }

    pub fn copy_stream_events_for_replay(
        &self,
        replay_run_id: &str,
        source_run_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<StreamEventRecord>, StoreError> {
        let source = self.stream_events(source_run_id)?;
        let attempts = self
            .list_tasks(replay_run_id)?
            .into_iter()
            .map(|task| (task.task_id, task.attempt))
            .collect::<BTreeMap<_, _>>();
        let mut copied = Vec::with_capacity(source.len());
        for event in source {
            let task_attempt =
                attempts
                    .get(&event.task_id)
                    .copied()
                    .ok_or_else(|| StoreError::TaskNotFound {
                        run_id: replay_run_id.to_owned(),
                        task_id: event.task_id.clone(),
                    })?;
            let mut record = self.record_stream_event(
                replay_run_id,
                &event.task_id,
                task_attempt,
                None,
                &event.event_type,
                event.provider_sequence,
                &event.payload,
                event.truncated,
                now,
            )?;
            self.connection.lock().execute(
                "UPDATE stream_events SET source_run_id = ?5, source_sequence = ?6
                 WHERE run_id = ?1 AND task_id = ?2 AND task_attempt = ?3 AND sequence = ?4",
                params![
                    replay_run_id,
                    record.task_id,
                    record.task_attempt,
                    record.sequence,
                    source_run_id,
                    event.sequence,
                ],
            )?;
            record.source_run_id = Some(source_run_id.to_owned());
            record.source_sequence = Some(event.sequence);
            copied.push(record);
        }
        Ok(copied)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_protocol_session(
        &self,
        run_id: &str,
        task_id: &str,
        effect_id: &str,
        protocol: &str,
        remote: &str,
        generation: u32,
        status: &str,
        state: &Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.connection.lock().execute(
            "INSERT INTO protocol_sessions
             (run_id, task_id, effect_id, protocol, remote, generation, status, format_version, state_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)
             ON CONFLICT(run_id, task_id, protocol) DO UPDATE SET
               effect_id = excluded.effect_id,
               remote = excluded.remote,
               generation = excluded.generation,
               status = excluded.status,
               format_version = excluded.format_version,
               state_json = excluded.state_json,
               source_run_id = NULL,
               source_task_id = NULL,
               updated_at = excluded.updated_at",
            params![
                run_id,
                task_id,
                effect_id,
                protocol,
                remote,
                generation,
                status,
                encode_protected(
                    &self.protection,
                    state,
                    "protocol_sessions.state_json"
                )?,
                now.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_protocol_call(
        &self,
        effect_id: &str,
        run_id: &str,
        task_id: &str,
        task_attempt: u16,
        protocol: &str,
        operation: &str,
        call_identity: &str,
        generation: u32,
        idempotency: &str,
        status: &str,
        state: &Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        let identity = connection
            .query_row(
                "SELECT run_id, task_id, task_attempt, protocol, operation, call_identity, idempotency
                 FROM protocol_calls WHERE effect_id = ?1",
                [effect_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u16>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        if identity.as_ref().is_some_and(|identity| {
            identity.0 != run_id
                || identity.1 != task_id
                || identity.2 != task_attempt
                || identity.3 != protocol
                || identity.4 != operation
                || identity.5 != call_identity
                || identity.6 != idempotency
        }) {
            return Err(StoreError::Incompatible(format!(
                "protocol call identity for effect `{effect_id}` changed"
            )));
        }
        connection.execute(
            "INSERT INTO protocol_calls
             (effect_id, run_id, task_id, task_attempt, protocol, operation, call_identity, generation, idempotency, status, format_version, state_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?12)
             ON CONFLICT(effect_id) DO UPDATE SET
               generation = excluded.generation,
               status = excluded.status,
               format_version = excluded.format_version,
               state_json = excluded.state_json,
               source_run_id = NULL,
               source_effect_id = NULL,
               updated_at = excluded.updated_at",
            params![
                effect_id,
                run_id,
                task_id,
                task_attempt,
                protocol,
                operation,
                call_identity,
                generation,
                idempotency,
                status,
                encode_protected(&self.protection, state, "protocol_calls.state_json")?,
                now.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn protocol_sessions(
        &self,
        run_id: &str,
    ) -> Result<Vec<ProtocolSessionRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT task_id, effect_id, protocol, remote, generation, status, format_version,
                    state_json, source_run_id, source_task_id, updated_at
             FROM protocol_sessions WHERE run_id = ?1 ORDER BY task_id, protocol",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.6 != 1 {
                    return Err(StoreError::Incompatible(format!(
                        "protocol session format version {}",
                        row.6
                    )));
                }
                Ok(ProtocolSessionRecord {
                    run_id: run_id.to_owned(),
                    task_id: row.0,
                    effect_id: row.1,
                    protocol: row.2,
                    remote: row.3,
                    generation: row.4,
                    status: row.5,
                    format_version: row.6,
                    state: decode_protected(
                        &self.protection,
                        &row.7,
                        "protocol_sessions.state_json",
                    )?,
                    source_run_id: row.8,
                    source_task_id: row.9,
                    updated_at: parse_time(&row.10, "protocol_session.updated_at")?,
                })
            })
            .collect()
    }

    pub fn protocol_calls(&self, run_id: &str) -> Result<Vec<ProtocolCallRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT effect_id, task_id, task_attempt, protocol, operation, call_identity,
                    generation, idempotency, status, format_version, state_json,
                    source_run_id, source_effect_id, updated_at
             FROM protocol_calls WHERE run_id = ?1
             ORDER BY task_id, task_attempt, effect_id",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u16>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, String>(13)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.9 != 1 {
                    return Err(StoreError::Incompatible(format!(
                        "protocol call format version {}",
                        row.9
                    )));
                }
                Ok(ProtocolCallRecord {
                    effect_id: row.0,
                    run_id: run_id.to_owned(),
                    task_id: row.1,
                    task_attempt: row.2,
                    protocol: row.3,
                    operation: row.4,
                    call_identity: row.5,
                    generation: row.6,
                    idempotency: row.7,
                    status: row.8,
                    format_version: row.9,
                    state: decode_protected(
                        &self.protection,
                        &row.10,
                        "protocol_calls.state_json",
                    )?,
                    source_run_id: row.11,
                    source_effect_id: row.12,
                    updated_at: parse_time(&row.13, "protocol_call.updated_at")?,
                })
            })
            .collect()
    }

    pub fn protocol_call(&self, effect_id: &str) -> Result<Option<ProtocolCallRecord>, StoreError> {
        let run_id = self
            .connection
            .lock()
            .query_row(
                "SELECT run_id FROM protocol_calls WHERE effect_id = ?1",
                [effect_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(run_id
            .map(|run_id| self.protocol_calls(&run_id))
            .transpose()?
            .and_then(|calls| calls.into_iter().find(|call| call.effect_id == effect_id)))
    }

    pub fn copy_protocol_records_for_replay(
        &self,
        replay_run_id: &str,
        source_run_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.copy_protocol_records(replay_run_id, source_run_id, None, now)
    }

    pub fn copy_protocol_records_for_materialization(
        &self,
        target_run_id: &str,
        source_run_id: &str,
        task_ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let task_ids = task_ids.iter().cloned().collect::<BTreeSet<_>>();
        self.copy_protocol_records(target_run_id, source_run_id, Some(&task_ids), now)
    }

    fn copy_protocol_records(
        &self,
        target_run_id: &str,
        source_run_id: &str,
        task_ids: Option<&BTreeSet<String>>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let sessions = self
            .protocol_sessions(source_run_id)?
            .into_iter()
            .filter(|session| task_ids.is_none_or(|ids| ids.contains(&session.task_id)))
            .collect::<Vec<_>>();
        let calls = self
            .protocol_calls(source_run_id)?
            .into_iter()
            .filter(|call| task_ids.is_none_or(|ids| ids.contains(&call.task_id)))
            .collect::<Vec<_>>();
        let attempts = self
            .list_tasks(target_run_id)?
            .into_iter()
            .map(|task| (task.task_id, task.attempt))
            .collect::<BTreeMap<_, _>>();
        let connection = self.connection.lock();
        let transaction = connection.unchecked_transaction()?;
        for session in sessions {
            transaction.execute(
                "INSERT INTO protocol_sessions
                 (run_id, task_id, effect_id, protocol, remote, generation, status, format_version,
                  state_json, source_run_id, source_task_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'recorded', 1, ?7, ?8, ?9, ?10)",
                params![
                    target_run_id,
                    session.task_id,
                    format!("recorded:{target_run_id}:{}", session.effect_id),
                    session.protocol,
                    session.remote,
                    session.generation,
                    encode_protected(
                        &self.protection,
                        &session.state,
                        "protocol_sessions.state_json"
                    )?,
                    source_run_id,
                    session.task_id,
                    now.to_rfc3339(),
                ],
            )?;
        }
        for call in calls {
            let attempt =
                attempts
                    .get(&call.task_id)
                    .copied()
                    .ok_or_else(|| StoreError::TaskNotFound {
                        run_id: target_run_id.to_owned(),
                        task_id: call.task_id.clone(),
                    })?;
            transaction.execute(
                "INSERT INTO protocol_calls
                 (effect_id, run_id, task_id, task_attempt, protocol, operation, call_identity,
                  generation, idempotency, status, format_version, state_json, source_run_id,
                  source_effect_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'recorded', 1, ?10, ?11, ?12, ?13)",
                params![
                    format!("recorded:{target_run_id}:{}", call.effect_id),
                    target_run_id,
                    call.task_id,
                    attempt,
                    call.protocol,
                    call.operation,
                    call.call_identity,
                    call.generation,
                    call.idempotency,
                    encode_protected(&self.protection, &call.state, "protocol_calls.state_json")?,
                    source_run_id,
                    call.effect_id,
                    now.to_rfc3339(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn list_effects(&self, run_id: &str) -> Result<Vec<EffectRecord>, StoreError> {
        let ids = {
            let connection = self.connection.lock();
            let mut statement = connection.prepare(
                "SELECT effect_id FROM effects WHERE run_id = ?1 ORDER BY task_id, task_attempt, ordinal",
            )?;
            statement
                .query_map([run_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        ids.into_iter().map(|id| self.load_effect(&id)).collect()
    }

    pub fn checkpoints(&self, run_id: &str) -> Result<Vec<CheckpointRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT sequence, format_version, state_json, checksum, created_at FROM checkpoints WHERE run_id = ?1 ORDER BY sequence",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.1 != CHECKPOINT_FORMAT_VERSION {
                    return Err(StoreError::Incompatible(format!(
                        "checkpoint format version {}",
                        row.1
                    )));
                }
                let actual = hex::encode(Sha256::digest(row.2.as_bytes()));
                if actual != row.3 {
                    return Err(StoreError::Corrupt(format!(
                        "checkpoint {} checksum mismatch",
                        row.0
                    )));
                }
                Ok(CheckpointRecord {
                    sequence: row.0,
                    format_version: row.1,
                    state: decode_protected(&self.protection, &row.2, "checkpoints.state_json")?,
                    checksum: row.3,
                    created_at: parse_time(&row.4, "checkpoint.created_at")?,
                })
            })
            .collect()
    }

    pub fn provider_sessions(
        &self,
        run_id: &str,
    ) -> Result<Vec<ProviderSessionRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT task_id, provider, format_version, continuation_json, updated_at FROM provider_sessions WHERE run_id = ?1 ORDER BY task_id",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                if row.2 != 1 {
                    return Err(StoreError::Incompatible(format!(
                        "provider session format version {}",
                        row.2
                    )));
                }
                Ok(ProviderSessionRecord {
                    task_id: row.0,
                    provider: row.1,
                    format_version: row.2,
                    continuation: decode_protected(
                        &self.protection,
                        &row.3,
                        "provider_sessions.continuation_json",
                    )?,
                    updated_at: parse_time(&row.4, "provider_session.updated_at")?,
                })
            })
            .collect()
    }

    pub fn tool_calls(&self, run_id: &str) -> Result<Vec<ToolCallRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT call_id, task_id, effect_id, tool_id, input_digest, output_digest, status, created_at, completed_at FROM tool_calls WHERE run_id = ?1 ORDER BY created_at, call_id",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                Ok(ToolCallRecord {
                    call_id: row.0,
                    task_id: row.1,
                    effect_id: row.2,
                    tool_id: row.3,
                    input_digest: row.4,
                    output_digest: row.5,
                    status: row.6,
                    created_at: parse_time(&row.7, "tool_call.created_at")?,
                    completed_at: row
                        .8
                        .map(|value| parse_time(&value, "tool_call.completed_at"))
                        .transpose()?,
                })
            })
            .collect()
    }

    pub fn record_trace_event(
        &self,
        run_id: &str,
        trace_id: &str,
        event: &Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        let sequence: i64 = connection.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM trace_events WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )?;
        connection.execute(
            "INSERT INTO trace_events (run_id, sequence, trace_id, event_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                sequence,
                trace_id,
                encode_protected(&self.protection, event, "trace_events.event_json")?,
                now.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn trace_events(&self, run_id: &str) -> Result<Vec<TraceRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT sequence, trace_id, event_json, created_at FROM trace_events WHERE run_id = ?1 ORDER BY sequence",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                Ok(TraceRecord {
                    sequence: row.0,
                    trace_id: row.1,
                    event: decode_protected(&self.protection, &row.2, "trace_events.event_json")?,
                    created_at: parse_time(&row.3, "trace.created_at")?,
                })
            })
            .collect()
    }

    pub fn put_long_term_memory(
        &self,
        namespace: &str,
        key: &str,
        value: &Value,
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.connection.lock().execute(
            "INSERT INTO long_term_memory (namespace, memory_key, value_json, expires_at, updated_at, format_version, embedding_provider, embedding_dimensions, embedding_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, NULL, NULL, ?5)
             ON CONFLICT(namespace, memory_key) DO UPDATE SET
               value_json = excluded.value_json,
               expires_at = excluded.expires_at,
               updated_at = excluded.updated_at,
               format_version = 0,
               embedding_provider = NULL,
               embedding_dimensions = NULL,
               embedding_json = NULL",
            params![
                namespace,
                key,
                encode_protected(&self.protection, value, "long_term_memory.value_json")?,
                expires_at.map(|value| value.to_rfc3339()),
                now.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_memory_entry(
        &self,
        namespace: &str,
        key: &str,
        entry: &MemoryEntry,
        embedding_provider: Option<&str>,
        embedding: Option<&[f32]>,
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if namespace.is_empty() || key.is_empty() {
            return Err(StoreError::Incompatible(
                "memory namespace and key must not be empty".to_owned(),
            ));
        }
        entry
            .validate()
            .map_err(|error| StoreError::Incompatible(error.to_string()))?;
        if embedding_provider.is_some() != embedding.is_some() {
            return Err(StoreError::Incompatible(
                "memory embedding provider and vector must be supplied together".to_owned(),
            ));
        }
        let embedding_dimensions = embedding
            .map(|vector| {
                if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
                    return Err(StoreError::Incompatible(
                        "memory embedding must be non-empty and finite".to_owned(),
                    ));
                }
                let dimensions = u16::try_from(vector.len()).map_err(|_| {
                    StoreError::Incompatible(
                        "memory embedding dimensions exceed the supported range".to_owned(),
                    )
                })?;
                if !(MIN_EMBEDDING_DIMENSIONS..=MAX_EMBEDDING_DIMENSIONS).contains(&dimensions) {
                    return Err(StoreError::Incompatible(format!(
                        "memory embedding dimensions must be between {MIN_EMBEDDING_DIMENSIONS} and {MAX_EMBEDDING_DIMENSIONS}"
                    )));
                }
                Ok(dimensions)
            })
            .transpose()?;
        let embedding_json = embedding
            .map(|vector| {
                encode_protected(&self.protection, &vector, "long_term_memory.embedding_json")
            })
            .transpose()?;
        self.connection.lock().execute(
            "INSERT INTO long_term_memory (namespace, memory_key, value_json, expires_at, updated_at, format_version, embedding_provider, embedding_dimensions, embedding_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?5)
             ON CONFLICT(namespace, memory_key) DO UPDATE SET
               value_json = excluded.value_json,
               expires_at = excluded.expires_at,
               updated_at = excluded.updated_at,
               format_version = excluded.format_version,
               embedding_provider = excluded.embedding_provider,
               embedding_dimensions = excluded.embedding_dimensions,
               embedding_json = excluded.embedding_json",
            params![
                namespace,
                key,
                encode_protected(&self.protection, entry, "long_term_memory.value_json")?,
                expires_at.map(|value| value.to_rfc3339()),
                now.to_rfc3339(),
                MEMORY_ENTRY_FORMAT_VERSION,
                embedding_provider,
                embedding_dimensions,
                embedding_json,
            ],
        )?;
        Ok(())
    }

    pub fn put_provider_session(
        &self,
        run_id: &str,
        task_id: &str,
        provider: &str,
        continuation: &Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.connection.lock().execute(
            "INSERT INTO provider_sessions (run_id, task_id, provider, format_version, continuation_json, updated_at) VALUES (?1, ?2, ?3, 1, ?4, ?5) ON CONFLICT(run_id, task_id) DO UPDATE SET provider = excluded.provider, format_version = excluded.format_version, continuation_json = excluded.continuation_json, updated_at = excluded.updated_at",
            params![
                run_id,
                task_id,
                provider,
                encode_protected(
                    &self.protection,
                    continuation,
                    "provider_sessions.continuation_json"
                )?,
                now.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_tool_call(
        &self,
        call_id: &str,
        run_id: &str,
        task_id: &str,
        effect_id: &str,
        tool_id: &str,
        input_digest: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.connection.lock().execute(
            "INSERT INTO tool_calls (call_id, run_id, task_id, effect_id, tool_id, input_digest, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'started', ?7)",
            params![call_id, run_id, task_id, effect_id, tool_id, input_digest, now.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn complete_tool_effect(
        &self,
        effect_id: &str,
        run_id: &str,
        call_id: &str,
        result: Result<&Value, &str>,
        output_digest: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let (effect_status, output, error, confirmed, call_status) = match result {
            Ok(output) => (
                EffectStatus::Succeeded,
                Some(encode_protected(
                    &self.protection,
                    output,
                    "effects.result_json",
                )?),
                None,
                true,
                "succeeded",
            ),
            Err(error) => (
                EffectStatus::Failed,
                None,
                Some(protect_text(&self.protection, error, "effects.error")?),
                false,
                "failed",
            ),
        };
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let effect_changed = transaction.execute(
            "UPDATE effects SET status = ?2, result_json = ?3, error = ?4, confirmed = ?5, completed_at = ?6 WHERE effect_id = ?1 AND status = ?7",
            params![effect_id, encode_enum(effect_status)?, output, error, confirmed, now.to_rfc3339(), encode_enum(EffectStatus::Started)?],
        )?;
        if effect_changed != 1 {
            return Err(StoreError::EffectNotFound(effect_id.to_owned()));
        }
        let call_changed = transaction.execute(
            "UPDATE tool_calls SET output_digest = ?3, status = ?4, completed_at = ?5 WHERE run_id = ?1 AND call_id = ?2 AND status = 'started'",
            params![run_id, call_id, output_digest, call_status, now.to_rfc3339()],
        )?;
        if call_changed != 1 {
            return Err(StoreError::Incompatible(format!(
                "tool call `{call_id}` in run `{run_id}` is missing or terminal"
            )));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn mark_tool_effect_uncertain(
        &self,
        effect_id: &str,
        run_id: &str,
        call_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let effect_changed = transaction.execute(
            "UPDATE effects SET status = ?2, error = ?3, completed_at = ?4, confirmed = 0 WHERE effect_id = ?1 AND status = ?5",
            params![
                effect_id,
                encode_enum(EffectStatus::Uncertain)?,
                protect_text(&self.protection, error, "effects.error")?,
                now.to_rfc3339(),
                encode_enum(EffectStatus::Started)?
            ],
        )?;
        if effect_changed != 1 {
            return Err(StoreError::EffectNotFound(effect_id.to_owned()));
        }
        let call_changed = transaction.execute(
            "UPDATE tool_calls SET status = 'uncertain', completed_at = ?3 WHERE run_id = ?1 AND call_id = ?2 AND status = 'started'",
            params![run_id, call_id, now.to_rfc3339()],
        )?;
        if call_changed != 1 {
            return Err(StoreError::Incompatible(format!(
                "tool call `{call_id}` in run `{run_id}` is missing or terminal"
            )));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get_long_term_memory(
        &self,
        namespace: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Value>, StoreError> {
        Ok(self
            .get_memory_entry(namespace, key, now)?
            .map(|record| record.entry.value()))
    }

    pub fn get_memory_entry(
        &self,
        namespace: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<MemoryRecord>, StoreError> {
        let raw = self
            .connection
            .lock()
            .query_row(
                "SELECT format_version, value_json, embedding_provider, embedding_dimensions, created_at, updated_at, expires_at
                 FROM long_term_memory
                 WHERE namespace = ?1 AND memory_key = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
                params![namespace, key, now.to_rfc3339()],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?;
        raw.map(|raw| {
            let updated_at = parse_time(&raw.5, "memory.updated_at")?;
            let entry = decode_memory_entry(&self.protection, raw.0, &raw.1)?;
            Ok(MemoryRecord {
                namespace: namespace.to_owned(),
                key: key.to_owned(),
                entry,
                embedding_provider: raw.2,
                embedding_dimensions: raw
                    .3
                    .map(|value| sqlite_u16(value, "memory.embedding_dimensions"))
                    .transpose()?,
                created_at: raw
                    .4
                    .as_deref()
                    .map(|value| parse_time(value, "memory.created_at"))
                    .transpose()?
                    .unwrap_or(updated_at),
                updated_at,
                expires_at: raw
                    .6
                    .as_deref()
                    .map(|value| parse_time(value, "memory.expires_at"))
                    .transpose()?,
            })
        })
        .transpose()
    }

    pub fn list_memory_entries(
        &self,
        namespace: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<MemoryRecord>, StoreError> {
        let keys = {
            let connection = self.connection.lock();
            let mut statement = connection.prepare(
                "SELECT memory_key
                 FROM long_term_memory
                 WHERE namespace = ?1 AND (expires_at IS NULL OR expires_at > ?2)
                 ORDER BY memory_key
                 LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![
                        namespace,
                        now.to_rfc3339(),
                        i64::try_from(MAX_MEMORY_SEARCH_CANDIDATES + 1).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        if keys.len() > MAX_MEMORY_SEARCH_CANDIDATES {
            return Err(StoreError::Incompatible(format!(
                "memory listing exceeds the bounded candidate limit of {MAX_MEMORY_SEARCH_CANDIDATES}"
            )));
        }
        keys.into_iter()
            .map(|key| {
                self.get_memory_entry(namespace, &key, now)?.ok_or_else(|| {
                    StoreError::Corrupt(format!(
                        "memory `{namespace}/{key}` disappeared during listing"
                    ))
                })
            })
            .collect()
    }

    pub fn search_memory(
        &self,
        query: &MemoryQuery,
        query_embedding: Option<&[f32]>,
        now: DateTime<Utc>,
    ) -> Result<Vec<MemorySearchResult>, StoreError> {
        query
            .validate()
            .map_err(|error| StoreError::Incompatible(error.to_string()))?;
        if matches!(
            query.mode,
            MemorySearchMode::Vector | MemorySearchMode::Hybrid
        ) && query_embedding.is_none()
        {
            return Err(StoreError::Incompatible(
                "vector and hybrid memory search require a query embedding".to_owned(),
            ));
        }
        if query_embedding.is_some_and(|embedding| {
            embedding.is_empty() || embedding.iter().any(|v| !v.is_finite())
        }) {
            return Err(StoreError::Incompatible(
                "query embedding must be non-empty and finite".to_owned(),
            ));
        }
        if query_embedding.is_some_and(|embedding| {
            u16::try_from(embedding.len()).map_or(true, |dimensions| {
                !(MIN_EMBEDDING_DIMENSIONS..=MAX_EMBEDDING_DIMENSIONS).contains(&dimensions)
            })
        }) {
            return Err(StoreError::Incompatible(format!(
                "query embedding dimensions must be between {MIN_EMBEDDING_DIMENSIONS} and {MAX_EMBEDDING_DIMENSIONS}"
            )));
        }
        let raw = {
            let connection = self.connection.lock();
            let mut statement = connection.prepare(
                "SELECT memory_key, format_version, value_json, embedding_provider, embedding_dimensions, embedding_json, created_at, updated_at, expires_at
                 FROM long_term_memory
                 WHERE namespace = ?1 AND (expires_at IS NULL OR expires_at > ?2)
                 ORDER BY memory_key
                 LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![
                        query.namespace,
                        now.to_rfc3339(),
                        i64::try_from(MAX_MEMORY_SEARCH_CANDIDATES + 1).unwrap_or(i64::MAX)
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u32>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        if raw.len() > MAX_MEMORY_SEARCH_CANDIDATES {
            return Err(StoreError::Incompatible(format!(
                "memory search exceeds the bounded candidate limit of {MAX_MEMORY_SEARCH_CANDIDATES}"
            )));
        }
        let query_tokens = memory_tokens(&query.text);
        let mut results = Vec::new();
        for raw in raw {
            let entry = decode_memory_entry(&self.protection, raw.1, &raw.2)?;
            if !metadata_matches(&entry.metadata, &query.filters) {
                continue;
            }
            let text_score = text_score(&query_tokens, entry.searchable_text().unwrap_or_default());
            let embedding = raw
                .5
                .as_deref()
                .map(|stored| {
                    decode_protected::<Vec<f32>>(
                        &self.protection,
                        stored,
                        "long_term_memory.embedding_json",
                    )
                })
                .transpose()?;
            match (raw.4, embedding.as_deref()) {
                (Some(dimensions), Some(vector))
                    if usize::try_from(dimensions).ok() != Some(vector.len()) =>
                {
                    return Err(StoreError::Corrupt(format!(
                        "memory `{}/{}` embedding dimensions do not match the stored vector",
                        query.namespace, raw.0
                    )));
                }
                (None, Some(_)) | (Some(_), None) => {
                    return Err(StoreError::Corrupt(format!(
                        "memory `{}/{}` has incomplete embedding metadata",
                        query.namespace, raw.0
                    )));
                }
                _ => {}
            }
            if let (Some(query_embedding), Some(candidate)) =
                (query_embedding, embedding.as_deref())
                && query_embedding.len() != candidate.len()
            {
                return Err(StoreError::Incompatible(format!(
                    "memory `{}/{}` uses {} embedding dimensions but the query uses {}; rebuild the namespace index",
                    query.namespace,
                    raw.0,
                    candidate.len(),
                    query_embedding.len()
                )));
            }
            let vector_score = match (query_embedding, embedding.as_deref()) {
                (Some(query), Some(candidate)) => vector_score(query, candidate),
                _ => 0,
            };
            let score = match query.mode {
                MemorySearchMode::Text => text_score,
                MemorySearchMode::Vector => vector_score,
                MemorySearchMode::Hybrid => {
                    u32::try_from((u64::from(text_score) + u64::from(vector_score)) / 2)
                        .unwrap_or(u32::MAX)
                }
            };
            if score == 0 {
                continue;
            }
            let updated_at = parse_time(&raw.7, "memory.updated_at")?;
            results.push(MemorySearchResult {
                record: MemoryRecord {
                    namespace: query.namespace.clone(),
                    key: raw.0,
                    entry,
                    embedding_provider: raw.3,
                    embedding_dimensions: raw
                        .4
                        .map(|value| sqlite_u16(value, "memory.embedding_dimensions"))
                        .transpose()?,
                    created_at: raw
                        .6
                        .as_deref()
                        .map(|value| parse_time(value, "memory.created_at"))
                        .transpose()?
                        .unwrap_or(updated_at),
                    updated_at,
                    expires_at: raw
                        .8
                        .as_deref()
                        .map(|value| parse_time(value, "memory.expires_at"))
                        .transpose()?,
                },
                score_millionths: score,
                text_score_millionths: text_score,
                vector_score_millionths: vector_score,
            });
        }
        results.sort_by(|left, right| {
            right
                .score_millionths
                .cmp(&left.score_millionths)
                .then_with(|| left.record.key.cmp(&right.record.key))
        });
        results.truncate(usize::from(query.limit));
        Ok(results)
    }

    pub fn artifact_references(
        &self,
        run_id: Option<&str>,
        task_id: Option<&str>,
    ) -> Result<Vec<ArtifactReference>, StoreError> {
        let connection = self.connection.lock();
        let mut references = Vec::new();
        match (run_id, task_id) {
            (Some(run_id), Some(task_id)) => {
                let mut statement = connection.prepare(
                    "SELECT run_id, task_id, logical_path, logical_name, media_type, digest, source_run_id, source_task_id, created_at FROM artifact_refs WHERE run_id = ?1 AND task_id = ?2 ORDER BY logical_path",
                )?;
                let rows =
                    statement.query_map(params![run_id, task_id], decode_artifact_ref_row)?;
                for row in rows {
                    references.push(artifact_reference_from_row(row?)?);
                }
            }
            (Some(run_id), None) => {
                let mut statement = connection.prepare(
                    "SELECT run_id, task_id, logical_path, logical_name, media_type, digest, source_run_id, source_task_id, created_at FROM artifact_refs WHERE run_id = ?1 ORDER BY task_id, logical_path",
                )?;
                let rows = statement.query_map([run_id], decode_artifact_ref_row)?;
                for row in rows {
                    references.push(artifact_reference_from_row(row?)?);
                }
            }
            (None, None) => {
                let mut statement = connection.prepare(
                    "SELECT run_id, task_id, logical_path, logical_name, media_type, digest, source_run_id, source_task_id, created_at FROM artifact_refs ORDER BY run_id, task_id, logical_path",
                )?;
                let rows = statement.query_map([], decode_artifact_ref_row)?;
                for row in rows {
                    references.push(artifact_reference_from_row(row?)?);
                }
            }
            (None, Some(_)) => {
                return Err(StoreError::Incompatible(
                    "artifact task filter requires a run filter".to_owned(),
                ));
            }
        }
        Ok(references)
    }

    pub fn artifact_blobs(&self) -> Result<Vec<ArtifactBlobRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT b.digest, b.algorithm, b.size_bytes, b.relative_path, b.created_at, b.last_verified_at, COUNT(r.digest) FROM artifact_blobs b LEFT JOIN artifact_refs r ON r.digest = b.digest GROUP BY b.digest ORDER BY b.created_at, b.digest",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                Ok(ArtifactBlobRecord {
                    digest: row.0,
                    algorithm: row.1,
                    size_bytes: sqlite_u64(row.2, "artifact_blob.size_bytes")?,
                    relative_path: row.3,
                    created_at: parse_time(&row.4, "artifact_blob.created_at")?,
                    last_verified_at: row
                        .5
                        .map(|value| parse_time(&value, "artifact_blob.last_verified_at"))
                        .transpose()?,
                    reference_count: sqlite_u64(row.6, "artifact_blob.reference_count")?,
                })
            })
            .collect()
    }

    pub fn artifact_blob(&self, digest: &str) -> Result<ArtifactBlobRecord, StoreError> {
        let row = self
            .connection
            .lock()
            .query_row(
                "SELECT b.digest, b.algorithm, b.size_bytes, b.relative_path, b.created_at, b.last_verified_at, (SELECT COUNT(*) FROM artifact_refs r WHERE r.digest = b.digest) FROM artifact_blobs b WHERE b.digest = ?1",
                [digest],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::Incompatible(format!("artifact blob `{digest}` was not found"))
            })?;
        Ok(ArtifactBlobRecord {
            digest: row.0,
            algorithm: row.1,
            size_bytes: sqlite_u64(row.2, "artifact_blob.size_bytes")?,
            relative_path: row.3,
            created_at: parse_time(&row.4, "artifact_blob.created_at")?,
            last_verified_at: row
                .5
                .map(|value| parse_time(&value, "artifact_blob.last_verified_at"))
                .transpose()?,
            reference_count: sqlite_u64(row.6, "artifact_blob.reference_count")?,
        })
    }

    pub fn verify_artifact(
        &self,
        digest: &str,
        now: DateTime<Utc>,
    ) -> Result<ArtifactVerification, StoreError> {
        let blob = self.artifact_blob(digest)?;
        let _guard = self.artifact_lock.lock();
        let _file_lock = self.artifact_store.lock_exclusive()?;
        let verification = self.artifact_store.verify(digest, blob.size_bytes)?;
        self.connection.lock().execute(
            "UPDATE artifact_blobs SET last_verified_at = ?2 WHERE digest = ?1",
            params![digest, now.to_rfc3339()],
        )?;
        Ok(verification)
    }

    pub fn verify_artifact_record(
        &self,
        artifact: &ArtifactRecord,
    ) -> Result<ArtifactVerification, StoreError> {
        if artifact.store_path.is_empty() {
            return Err(StoreError::Incompatible(format!(
                "artifact `{}` has no content-addressed blob reference",
                artifact.path
            )));
        }
        let blob = self.artifact_blob(&artifact.digest)?;
        if blob.size_bytes != artifact.size_bytes || blob.relative_path != artifact.store_path {
            return Err(StoreError::Corrupt(format!(
                "artifact manifest for `{}` disagrees with blob metadata `{}`",
                artifact.path, artifact.digest
            )));
        }
        let _guard = self.artifact_lock.lock();
        let _file_lock = self.artifact_store.lock_exclusive()?;
        self.artifact_store
            .verify(&artifact.digest, artifact.size_bytes)
            .map_err(StoreError::from)
    }

    pub fn export_artifact(
        &self,
        digest: &str,
        destination: &Path,
        overwrite: bool,
    ) -> Result<(), StoreError> {
        let blob = self.artifact_blob(digest)?;
        let _guard = self.artifact_lock.lock();
        let _file_lock = self.artifact_store.lock_exclusive()?;
        self.artifact_store
            .export(digest, blob.size_bytes, destination, overwrite)?;
        Ok(())
    }

    pub fn garbage_collect_artifacts(
        &self,
        before: DateTime<Utc>,
        dry_run: bool,
    ) -> Result<ArtifactGcReport, StoreError> {
        let _artifact_guard = self.artifact_lock.lock();
        let _artifact_file_lock = self.artifact_store.lock_exclusive()?;
        let stored_blobs = self.artifact_store.stored_blobs()?;
        let temporary_files = self
            .artifact_store
            .stale_temporary_files(SystemTime::from(before))?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let tracked_candidates = {
            let mut statement = transaction.prepare(
                "SELECT b.digest, b.size_bytes FROM artifact_blobs b LEFT JOIN artifact_refs r ON r.digest = b.digest WHERE r.digest IS NULL AND b.created_at < ?1 AND NOT EXISTS (SELECT 1 FROM artifact_ingests i WHERE i.digest = b.digest AND i.expires_at > ?2) ORDER BY b.created_at, b.digest",
            )?;
            statement
                .query_map(
                    params![before.to_rfc3339(), Utc::now().to_rfc3339()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut candidates = tracked_candidates
            .into_iter()
            .map(|(digest, size)| {
                sqlite_u64(size, "artifact_gc.size_bytes").map(|size| (digest, (size, true)))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let metadata_digests = {
            let mut statement = transaction.prepare("SELECT digest FROM artifact_blobs")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for blob in stored_blobs {
            if blob.modified_at < SystemTime::from(before)
                && !metadata_digests.contains(&blob.digest)
            {
                candidates.insert(blob.digest, (blob.size_bytes, false));
            }
        }
        let considered = u64::try_from(candidates.len()).unwrap_or(u64::MAX);
        let reclaimed_bytes = candidates
            .values()
            .map(|candidate| candidate.0)
            .chain(temporary_files.iter().map(|file| file.1))
            .fold(0_u64, u64::saturating_add);
        let temporary_files_considered = u64::try_from(temporary_files.len()).unwrap_or(u64::MAX);
        if dry_run {
            transaction.rollback()?;
            return Ok(ArtifactGcReport {
                considered,
                removed: Vec::new(),
                reclaimed_bytes,
                temporary_files_considered,
                temporary_files_removed: 0,
            });
        }

        let mut staged = Vec::new();
        for (digest, (_, tracked)) in &candidates {
            match self.artifact_store.stage_remove(digest) {
                Ok(Some(path)) => staged.push((digest.clone(), path)),
                Ok(None) => {}
                Err(error) => {
                    for (staged_digest, path) in &staged {
                        let _ = self.artifact_store.restore_staged(staged_digest, path);
                    }
                    return Err(error.into());
                }
            }
            if *tracked {
                let changed = match transaction.execute(
                    "DELETE FROM artifact_blobs WHERE digest = ?1 AND NOT EXISTS (SELECT 1 FROM artifact_refs WHERE digest = ?1) AND NOT EXISTS (SELECT 1 FROM artifact_ingests WHERE digest = ?1 AND expires_at > ?2)",
                    params![digest, Utc::now().to_rfc3339()],
                ) {
                    Ok(changed) => changed,
                    Err(error) => {
                        for (staged_digest, path) in &staged {
                            let _ = self.artifact_store.restore_staged(staged_digest, path);
                        }
                        return Err(error.into());
                    }
                };
                if changed != 1 {
                    for (staged_digest, path) in &staged {
                        let _ = self.artifact_store.restore_staged(staged_digest, path);
                    }
                    return Err(StoreError::Incompatible(format!(
                        "artifact `{digest}` became reachable during garbage collection"
                    )));
                }
            }
        }
        if let Err(error) = transaction.execute(
            "DELETE FROM artifact_ingests WHERE expires_at <= ?1",
            [Utc::now().to_rfc3339()],
        ) {
            for (digest, path) in &staged {
                let _ = self.artifact_store.restore_staged(digest, path);
            }
            return Err(error.into());
        }
        if let Err(error) = transaction.commit() {
            for (digest, path) in &staged {
                let _ = self.artifact_store.restore_staged(digest, path);
            }
            return Err(error.into());
        }
        for (_, path) in &staged {
            self.artifact_store.finish_staged(path)?;
        }
        for (path, _) in &temporary_files {
            std::fs::remove_file(path)?;
        }
        Ok(ArtifactGcReport {
            considered,
            removed: candidates.keys().cloned().collect(),
            reclaimed_bytes,
            temporary_files_considered,
            temporary_files_removed: temporary_files_considered,
        })
    }

    pub fn garbage_collect(&self, before: DateTime<Utc>) -> Result<usize, StoreError> {
        let connection = self.connection.lock();
        let expired = connection.execute(
            "DELETE FROM long_term_memory WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            [before.to_rfc3339()],
        )?;
        let runs = connection.execute(
            "DELETE FROM runs WHERE state IN ('succeeded', 'failed', 'cancelled') AND updated_at < ?1",
            [before.to_rfc3339()],
        )?;
        Ok(expired + runs)
    }

    pub fn checkpoint_count(&self, run_id: &str) -> Result<i64, StoreError> {
        self.connection
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn stats(&self) -> Result<DatabaseStats, StoreError> {
        let connection = self.connection.lock();
        let count = |table: &str| -> Result<i64, StoreError> {
            let allowed = [
                "runs",
                "task_states",
                "effects",
                "approvals",
                "checkpoints",
                "audit_events",
                "provider_sessions",
                "stream_events",
                "protocol_sessions",
                "protocol_calls",
                "tool_calls",
                "trace_events",
                "long_term_memory",
                "artifact_blobs",
                "artifact_refs",
                "artifact_ingests",
                "run_upgrades",
                "effect_reconciliations",
                "run_budgets",
                "budget_reservations",
            ];
            if !allowed.contains(&table) {
                return Err(StoreError::Incompatible(
                    "invalid statistics table".to_owned(),
                ));
            }
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .map_err(StoreError::from)
        };
        let schema_version =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(DatabaseStats {
            schema_version,
            runs: count("runs")?,
            tasks: count("task_states")?,
            effects: count("effects")?,
            approvals: count("approvals")?,
            checkpoints: count("checkpoints")?,
            audit_events: count("audit_events")?,
            provider_sessions: count("provider_sessions")?,
            stream_events: count("stream_events")?,
            protocol_sessions: count("protocol_sessions")?,
            protocol_calls: count("protocol_calls")?,
            tool_calls: count("tool_calls")?,
            trace_events: count("trace_events")?,
            long_term_memory: count("long_term_memory")?,
            artifact_blobs: count("artifact_blobs")?,
            artifact_references: count("artifact_refs")?,
            artifact_ingests: count("artifact_ingests")?,
            run_upgrades: count("run_upgrades")?,
            effect_reconciliations: count("effect_reconciliations")?,
            run_budgets: count("run_budgets")?,
            budget_reservations: count("budget_reservations")?,
        })
    }

    #[cfg(test)]
    fn connection(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.connection.lock()
    }
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

fn load_state_protection(
    connection: &Connection,
    resolver: &dyn StateKeyResolver,
) -> Result<StateProtection, StoreError> {
    let config = connection
        .query_row(
            "SELECT format_version, key_id, key_reference, key_check, maintenance FROM state_encryption WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((format_version, key_id, key_reference, key_check, maintenance)) = config else {
        return Ok(StateProtection::Plaintext);
    };
    if maintenance {
        return Err(StoreError::Encryption(
            "state-encryption maintenance transaction is incomplete".to_owned(),
        ));
    }
    if format_version != encryption::ENCRYPTION_FORMAT_VERSION {
        return Err(StoreError::Encryption(format!(
            "unsupported state-encryption configuration version {format_version}"
        )));
    }
    let codec = EncryptionCodec::resolve(&key_id, &key_reference, resolver)?;
    codec.verify_key_check(&key_check)?;
    Ok(StateProtection::Encrypted(codec))
}

fn encryption_inventory(
    connection: &Connection,
    protection: &StateProtection,
) -> Result<EncryptionInventory, StoreError> {
    let config = connection
        .query_row(
            "SELECT key_id, key_reference FROM state_encryption WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let mut protected_values = 0_u64;
    let mut encrypted_values = 0_u64;
    let mut plaintext_values = 0_u64;
    let mut invalid_envelopes = 0_u64;
    for column in SENSITIVE_COLUMNS {
        let context = column.context();
        let sql = format!(
            "SELECT {} FROM {} WHERE {} IS NOT NULL",
            column.column, column.table, column.column
        );
        let mut statement = connection.prepare(&sql)?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for value in values {
            protected_values = protected_values.saturating_add(1);
            if is_encrypted_value(&value) {
                encrypted_values = encrypted_values.saturating_add(1);
                let valid = if protection.is_enabled() {
                    protection.expose(&value, &context).map(|_| ())
                } else {
                    validate_envelope(&value, &context)
                };
                if valid.is_err() {
                    invalid_envelopes = invalid_envelopes.saturating_add(1);
                }
            } else {
                plaintext_values = plaintext_values.saturating_add(1);
            }
        }
    }
    Ok(EncryptionInventory {
        enabled: config.is_some(),
        key_id: config.as_ref().map(|value| value.0.clone()),
        key_reference: config.map(|value| value.1),
        protected_values,
        encrypted_values,
        plaintext_values,
        invalid_envelopes,
    })
}

fn rewrite_sensitive_values(
    transaction: &Transaction<'_>,
    current: &StateProtection,
    next: &StateProtection,
) -> Result<(), StoreError> {
    for column in SENSITIVE_COLUMNS {
        let context = column.context();
        let select = format!(
            "SELECT rowid, {} FROM {} WHERE {} IS NOT NULL",
            column.column, column.table, column.column
        );
        let values = {
            let mut statement = transaction.prepare(&select)?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (row_id, stored) in values {
            let plaintext = current.expose(&stored, &context)?;
            let protected = next.protect(&plaintext, &context)?;
            if column.table == "checkpoints" && column.column == "state_json" {
                let checksum = hex::encode(Sha256::digest(protected.as_bytes()));
                transaction.execute(
                    "UPDATE checkpoints SET state_json = ?1, checksum = ?2 WHERE rowid = ?3",
                    params![protected, checksum, row_id],
                )?;
            } else {
                let update = format!(
                    "UPDATE {} SET {} = ?1 WHERE rowid = ?2",
                    column.table, column.column
                );
                transaction.execute(&update, params![protected, row_id])?;
            }
        }
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let current: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > DATABASE_SCHEMA_VERSION {
        return Err(StoreError::UnknownSchema {
            found: current,
            supported: DATABASE_SCHEMA_VERSION,
        });
    }
    let migrations = [
        (1_u32, MIGRATION_1),
        (2_u32, MIGRATION_2),
        (3_u32, MIGRATION_3),
        (4_u32, MIGRATION_4),
        (5_u32, MIGRATION_5),
        (6_u32, MIGRATION_6),
        (7_u32, MIGRATION_7),
        (8_u32, MIGRATION_8),
        (9_u32, MIGRATION_9),
        (10_u32, MIGRATION_10),
        (11_u32, MIGRATION_11),
        (12_u32, MIGRATION_12),
        (13_u32, MIGRATION_13),
        (14_u32, MIGRATION_14),
        (15_u32, MIGRATION_15),
    ];
    for (version, sql) in migrations
        .into_iter()
        .filter(|(version, _)| *version > current)
    {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    install_encryption_triggers(connection)?;
    Ok(())
}

fn install_encryption_triggers(connection: &Connection) -> Result<(), StoreError> {
    for column in SENSITIVE_COLUMNS {
        let insert_name = format!(
            "enforce_encryption_{}_{}_insert",
            column.table, column.column
        );
        let update_name = format!(
            "enforce_encryption_{}_{}_update",
            column.table, column.column
        );
        let expected_prefix = format!(
            "'{ENVELOPE_PREFIX}' || (SELECT key_id FROM state_encryption WHERE singleton = 1) || ':'"
        );
        let condition = format!(
            "(SELECT COUNT(*) FROM state_encryption WHERE singleton = 1 AND maintenance = 0) = 1
             AND NEW.{column} IS NOT NULL
             AND substr(NEW.{column}, 1, length({expected_prefix})) != {expected_prefix}",
            column = column.column
        );
        connection.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS {insert_name}
             BEFORE INSERT ON {table}
             WHEN {condition}
             BEGIN
               SELECT RAISE(ABORT, 'protected field requires the current state-encryption key');
             END;
             CREATE TRIGGER IF NOT EXISTS {update_name}
             BEFORE UPDATE OF {column} ON {table}
             WHEN {condition}
             BEGIN
               SELECT RAISE(ABORT, 'protected field requires the current state-encryption key');
             END;",
            table = column.table,
            column = column.column,
        ))?;
    }
    Ok(())
}

fn initialize_budget_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    plan: &CompiledPlan,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO run_budgets
         (run_id, format_version, limits_json, pricing_version, usage_json, reserved_json,
          planned_tasks, planned_expansion_items, planned_loop_iterations, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9)",
        params![
            run_id,
            BUDGET_FORMAT_VERSION,
            encode(&plan.budget.limits)?,
            plan.budget
                .pricing
                .as_ref()
                .map(|pricing| pricing.version.as_str()),
            encode(&BudgetCounters::default())?,
            sqlite_i64(plan.budget.planned_tasks, "budget.planned_tasks")?,
            sqlite_i64(
                plan.budget.planned_expansion_items,
                "budget.planned_expansion_items"
            )?,
            sqlite_i64(
                plan.budget.planned_loop_iterations,
                "budget.planned_loop_iterations"
            )?,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn load_budget_snapshot(
    connection: &Connection,
    run_id: &str,
) -> Result<BudgetSnapshot, StoreError> {
    connection
        .query_row(
            "SELECT format_version, limits_json, pricing_version, usage_json, reserved_json,
                    exceeded_json, planned_tasks, planned_expansion_items,
                    planned_loop_iterations, updated_at
             FROM run_budgets WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::RunNotFound(run_id.to_owned()))
        .and_then(|row| {
            if row.0 != BUDGET_FORMAT_VERSION {
                return Err(StoreError::Incompatible(format!(
                    "budget format version {} is not supported",
                    row.0
                )));
            }
            Ok(BudgetSnapshot {
                format_version: row.0,
                limits: decode(&row.1, "run_budgets.limits_json")?,
                pricing_version: row.2,
                usage: decode(&row.3, "run_budgets.usage_json")?,
                reserved: decode(&row.4, "run_budgets.reserved_json")?,
                exceeded: row
                    .5
                    .map(|value| decode(&value, "run_budgets.exceeded_json"))
                    .transpose()?,
                planned_tasks: sqlite_u64(row.6, "run_budgets.planned_tasks")?,
                planned_expansion_items: sqlite_u64(row.7, "run_budgets.planned_expansion_items")?,
                planned_loop_iterations: sqlite_u64(row.8, "run_budgets.planned_loop_iterations")?,
                updated_at: parse_time(&row.9, "run_budgets.updated_at")?,
            })
        })
}

fn budget_exceeded(
    limits: &ResourceBudgetDefinition,
    usage: &BudgetCounters,
    reserved: &BudgetCounters,
    requested: &BudgetCounters,
) -> Option<BudgetExceeded> {
    let candidate =
        |used: u64, held: u64, next: u64| used.saturating_add(held).saturating_add(next);
    let checks = [
        (
            "providerRequests",
            limits.max_provider_requests,
            candidate(
                usage.provider_requests,
                reserved.provider_requests,
                requested.provider_requests,
            ),
        ),
        (
            "turns",
            limits.max_turns,
            candidate(usage.turns, reserved.turns, requested.turns),
        ),
        (
            "toolCalls",
            limits.max_tool_calls,
            candidate(usage.tool_calls, reserved.tool_calls, requested.tool_calls),
        ),
        (
            "inputTokens",
            limits.max_input_tokens,
            candidate(
                usage.input_tokens,
                reserved.input_tokens,
                requested.input_tokens,
            ),
        ),
        (
            "outputTokens",
            limits.max_output_tokens,
            candidate(
                usage.output_tokens,
                reserved.output_tokens,
                requested.output_tokens,
            ),
        ),
        (
            "totalTokens",
            limits.max_total_tokens,
            candidate(
                usage.total_tokens(),
                reserved.total_tokens(),
                requested.total_tokens(),
            ),
        ),
        (
            "wallTimeSeconds",
            limits.max_wall_time_seconds,
            candidate(
                usage.wall_time_seconds,
                reserved.wall_time_seconds,
                requested.wall_time_seconds,
            ),
        ),
        (
            "processOutputBytes",
            limits.max_process_output_bytes,
            candidate(
                usage.process_output_bytes,
                reserved.process_output_bytes,
                requested.process_output_bytes,
            ),
        ),
        (
            "artifactBytes",
            limits.max_artifact_bytes,
            candidate(
                usage.artifact_bytes,
                reserved.artifact_bytes,
                requested.artifact_bytes,
            ),
        ),
        (
            "costMicrousd",
            limits.max_cost_microusd,
            candidate(
                usage.cost_microusd,
                reserved.cost_microusd,
                requested.cost_microusd,
            ),
        ),
    ];
    checks
        .into_iter()
        .find_map(|(dimension, limit, attempted)| {
            limit
                .filter(|limit| attempted > *limit)
                .map(|limit| BudgetExceeded {
                    dimension: dimension.to_owned(),
                    limit,
                    attempted,
                })
        })
}

fn checkpoint_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    now: DateTime<Utc>,
    protection: &SharedStateProtection,
) -> Result<(), StoreError> {
    let run_state: (String, String, Option<String>, bool) = transaction.query_row(
        "SELECT state, working_memory_json, output_json, cancellation_requested FROM runs WHERE run_id = ?1",
        [run_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let mut statement = transaction.prepare(
        "SELECT task_id, state, attempt, output_json, error FROM task_states WHERE run_id = ?1 ORDER BY position",
    )?;
    let task_rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u16>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let tasks = task_rows
        .into_iter()
        .map(|row| {
            Ok(serde_json::json!({
                "taskId": row.0,
                "state": row.1,
                "attempt": row.2,
                "output": row.3
                    .map(|raw| decode_protected::<Value>(
                        protection,
                        &raw,
                        "task_states.output_json"
                    ))
                    .transpose()?,
                "error": row.4
                    .map(|raw| expose_text(protection, &raw, "task_states.error"))
                    .transpose()?,
            }))
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    let budget = load_budget_snapshot(transaction, run_id)?;
    let state = serde_json::json!({
        "runId": run_id,
        "state": run_state.0,
        "workingMemory": decode_protected::<Value>(
            protection,
            &run_state.1,
            "runs.working_memory_json"
        )?,
        "output": run_state.2
            .map(|raw| decode_protected::<Value>(protection, &raw, "runs.output_json"))
            .transpose()?,
        "cancellationRequested": run_state.3,
        "tasks": tasks,
        "budget": budget,
    });
    let state_json = encode_protected(protection, &state, "checkpoints.state_json")?;
    let checksum = hex::encode(Sha256::digest(state_json.as_bytes()));
    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM checkpoints WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO checkpoints (run_id, sequence, format_version, state_json, checksum, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run_id, sequence, CHECKPOINT_FORMAT_VERSION, state_json, checksum, now.to_rfc3339()],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_audit_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    event_type: &str,
    task_id: Option<&str>,
    trace_id: &str,
    payload: &Value,
    now: DateTime<Utc>,
    protection: &SharedStateProtection,
) -> Result<(), StoreError> {
    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM audit_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO audit_events (run_id, sequence, event_version, event_type, task_id, trace_id, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            run_id,
            sequence,
            AUDIT_EVENT_VERSION,
            event_type,
            task_id,
            trace_id,
            encode_protected(protection, payload, "audit_events.payload_json")?,
            now.to_rfc3339()
        ],
    )?;
    Ok(())
}

fn insert_effect_request_tx(
    transaction: &Transaction<'_>,
    request: &EffectRequest,
    status: EffectStatus,
    now: DateTime<Utc>,
    protection: &SharedStateProtection,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO effects (effect_id, format_version, run_id, task_id, task_attempt, ordinal, operation, effect_class, risk, idempotency, idempotency_key, input_digest, input_json, expected_effect, trace_id, status, effect_attempt, requested_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, ?17)",
        params![
            request.id,
            request.format_version,
            request.run_id,
            request.task_id,
            request.attempt,
            request.ordinal,
            request.operation,
            encode_enum(request.effect_class)?,
            encode_enum(request.risk)?,
            encode_enum(request.idempotency)?,
            request.idempotency_key,
            request.input_digest,
            encode_protected(protection, &request.input, "effects.input_json")?,
            protect_text(
                protection,
                &request.expected_effect,
                "effects.expected_effect"
            )?,
            request.trace_id,
            encode_enum(status)?,
            now.to_rfc3339(),
        ],
    )?;
    append_audit_tx(
        transaction,
        &request.run_id,
        "effect.requested",
        Some(&request.task_id),
        &request.trace_id,
        &serde_json::json!({
            "effectId": request.id,
            "operation": request.operation,
            "inputDigest": request.input_digest,
            "effectClass": request.effect_class,
            "risk": request.risk,
            "status": status,
        }),
        now,
        protection,
    )
}

fn append_trace_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    trace_id: &str,
    event: &Value,
    now: DateTime<Utc>,
    protection: &SharedStateProtection,
) -> Result<(), StoreError> {
    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM trace_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO trace_events (run_id, sequence, trace_id, event_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            run_id,
            sequence,
            trace_id,
            encode_protected(protection, event, "trace_events.event_json")?,
            now.to_rfc3339()
        ],
    )?;
    Ok(())
}

type ReconciliationRow = (
    String,
    String,
    String,
    u32,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn decode_reconciliation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReconciliationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

fn reconciliation_from_row(
    row: ReconciliationRow,
    protection: &SharedStateProtection,
) -> Result<EffectReconciliationRecord, StoreError> {
    if row.3 != 1 {
        return Err(StoreError::Incompatible(format!(
            "effect reconciliation format version {}",
            row.3
        )));
    }
    Ok(EffectReconciliationRecord {
        reconciliation_id: row.0,
        effect_id: row.1,
        run_id: row.2,
        format_version: row.3,
        status: decode_enum(&row.4, "effect_reconciliation.status")?,
        actor: row.5,
        reason: expose_text(protection, &row.6, "effect_reconciliations.reason")?,
        evidence: decode_protected(protection, &row.7, "effect_reconciliations.evidence_json")?,
        result: row
            .8
            .map(|value| decode_protected(protection, &value, "effect_reconciliations.result_json"))
            .transpose()?,
        result_schema: row
            .9
            .map(|value| {
                decode_protected(
                    protection,
                    &value,
                    "effect_reconciliations.result_schema_json",
                )
            })
            .transpose()?,
        authorization: decode_protected(
            protection,
            &row.10,
            "effect_reconciliations.authorization_json",
        )?,
        compensation_effect_id: row.11,
        supersedes_id: row.12,
        trace_id: row.13,
        created_at: parse_time(&row.14, "effect_reconciliation.created_at")?,
    })
}

type ArtifactReferenceRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

fn decode_artifact_ref_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactReferenceRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn artifact_reference_from_row(row: ArtifactReferenceRow) -> Result<ArtifactReference, StoreError> {
    Ok(ArtifactReference {
        run_id: row.0,
        task_id: row.1,
        logical_path: row.2,
        logical_name: row.3,
        media_type: row.4,
        digest: row.5,
        source_run_id: row.6,
        source_task_id: row.7,
        created_at: parse_time(&row.8, "artifact_reference.created_at")?,
    })
}

fn verify_artifact_manifest(
    artifact_store: &dyn ArtifactStore,
    artifacts: &[ArtifactRecord],
) -> Result<(), StoreError> {
    for artifact in artifacts {
        if artifact.store_path.is_empty() {
            return Err(StoreError::Incompatible(format!(
                "artifact `{}` has not been imported into the content-addressed store",
                artifact.path
            )));
        }
        artifact_store.verify(&artifact.digest, artifact.size_bytes)?;
    }
    Ok(())
}

fn copy_protocol_records_for_tasks_tx<'a>(
    transaction: &Transaction<'_>,
    target_run_id: &str,
    source_run_id: &str,
    task_ids: impl Iterator<Item = &'a str>,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let updated_at = now.to_rfc3339();
    for task_id in task_ids {
        transaction.execute(
            "INSERT INTO protocol_sessions
             (run_id, task_id, effect_id, protocol, remote, generation, status, format_version,
              state_json, source_run_id, source_task_id, updated_at)
             SELECT ?1, task_id, 'recorded:' || ?1 || ':' || effect_id, protocol, remote,
                    generation, 'recorded', format_version, state_json, ?2, task_id, ?4
             FROM protocol_sessions
             WHERE run_id = ?2 AND task_id = ?3",
            params![target_run_id, source_run_id, task_id, updated_at],
        )?;
        transaction.execute(
            "INSERT INTO protocol_calls
             (effect_id, run_id, task_id, task_attempt, protocol, operation, call_identity,
              generation, idempotency, status, format_version, state_json, source_run_id,
              source_effect_id, updated_at)
             SELECT 'recorded:' || ?1 || ':' || effect_id, ?1, task_id,
                    (SELECT attempt FROM task_states
                     WHERE run_id = ?1 AND task_id = protocol_calls.task_id),
                    protocol, operation, call_identity, generation, idempotency, 'recorded',
                    format_version, state_json, ?2, effect_id, ?4
             FROM protocol_calls
             WHERE run_id = ?2 AND task_id = ?3",
            params![target_run_id, source_run_id, task_id, updated_at],
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_artifact_references_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    task_id: &str,
    artifacts: &[ArtifactRecord],
    source_run_id: Option<&str>,
    source_task_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    for artifact in artifacts {
        if artifact.store_path.is_empty() {
            continue;
        }
        let size_bytes = i64::try_from(artifact.size_bytes).map_err(|_| {
            StoreError::Incompatible(format!(
                "artifact `{}` size exceeds SQLite integer range",
                artifact.path
            ))
        })?;
        transaction.execute(
            "INSERT INTO artifact_blobs (digest, algorithm, size_bytes, relative_path, created_at) VALUES (?1, 'sha256', ?2, ?3, ?4) ON CONFLICT(digest) DO NOTHING",
            params![
                artifact.digest,
                size_bytes,
                artifact.store_path,
                now.to_rfc3339(),
            ],
        )?;
        let stored: (i64, String) = transaction.query_row(
            "SELECT size_bytes, relative_path FROM artifact_blobs WHERE digest = ?1",
            [&artifact.digest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if sqlite_u64(stored.0, "artifact_blob.size_bytes")? != artifact.size_bytes
            || stored.1 != artifact.store_path
        {
            return Err(StoreError::Corrupt(format!(
                "artifact metadata for `{}` conflicts with its existing blob record",
                artifact.digest
            )));
        }
        transaction.execute(
            "INSERT INTO artifact_refs (run_id, task_id, logical_path, logical_name, media_type, digest, source_run_id, source_task_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                task_id,
                artifact.path,
                artifact.logical_name,
                artifact.media_type,
                artifact.digest,
                source_run_id,
                source_task_id,
                now.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn recover_artifact_quarantine(
    connection: &Connection,
    artifact_store: &LocalArtifactStore,
) -> Result<(), StoreError> {
    for (digest, path) in artifact_store.quarantined()? {
        let retained: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM artifact_blobs WHERE digest = ?1)",
            [&digest],
            |row| row.get(0),
        )?;
        if retained {
            artifact_store.restore_staged(&digest, &path)?;
        } else {
            artifact_store.finish_staged(&path)?;
        }
    }
    Ok(())
}

fn media_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => "application/json",
        Some("yaml" | "yml") => "application/yaml",
        Some("md" | "txt" | "log") => "text/plain",
        Some("html" | "htm") => "text/html",
        Some("csv") => "text/csv",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn decode_memory_entry(
    protection: &SharedStateProtection,
    format_version: u32,
    stored: &str,
) -> Result<MemoryEntry, StoreError> {
    let value: Value = decode_protected(protection, stored, "long_term_memory.value_json")?;
    let entry = match format_version {
        0 => MemoryEntry::from_legacy(value),
        MEMORY_ENTRY_FORMAT_VERSION => serde_json::from_value(value)?,
        version => {
            return Err(StoreError::Corrupt(format!(
                "unsupported long-term memory format version {version}"
            )));
        }
    };
    entry
        .validate()
        .map_err(|error| StoreError::Corrupt(error.to_string()))?;
    Ok(entry)
}

fn metadata_matches(metadata: &BTreeMap<String, Value>, filters: &BTreeMap<String, Value>) -> bool {
    filters
        .iter()
        .all(|(key, expected)| metadata.get(key) == Some(expected))
}

fn text_score(query_tokens: &[String], candidate: &str) -> u32 {
    let query = query_tokens.iter().collect::<BTreeSet<_>>();
    if query.is_empty() {
        return 0;
    }
    let candidate = memory_tokens(candidate)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let matches = query
        .iter()
        .filter(|token| candidate.contains(token.as_str()))
        .count();
    u32::try_from(
        u64::try_from(matches)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000)
            / u64::try_from(query.len()).unwrap_or(u64::MAX),
    )
    .unwrap_or(1_000_000)
}

fn vector_score(query: &[f32], candidate: &[f32]) -> u32 {
    if query.len() != candidate.len() || query.is_empty() {
        return 0;
    }
    let mut dot = 0.0_f64;
    let mut query_norm = 0.0_f64;
    let mut candidate_norm = 0.0_f64;
    for (left, right) in query.iter().zip(candidate) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        dot += left * right;
        query_norm += left * left;
        candidate_norm += right * right;
    }
    if query_norm == 0.0 || candidate_norm == 0.0 {
        return 0;
    }
    let cosine = (dot / (query_norm.sqrt() * candidate_norm.sqrt())).clamp(0.0, 1.0);
    (cosine * 1_000_000.0).round() as u32
}

fn sqlite_u16(value: i64, field: &str) -> Result<u16, StoreError> {
    u16::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("{field} is outside the u16 range: {value}")))
}

fn sqlite_u64(value: i64, field: &str) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("{field} cannot be negative: {value}")))
}

fn sqlite_i64(value: u64, field: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Incompatible(format!("{field} exceeds the SQLite integer range")))
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(StoreError::from)
}

fn encode_protected<T: Serialize + ?Sized>(
    protection: &SharedStateProtection,
    value: &T,
    context: &str,
) -> Result<String, StoreError> {
    protection.read().protect(&encode(value)?, context)
}

fn decode_protected<T: DeserializeOwned>(
    protection: &SharedStateProtection,
    value: &str,
    context: &str,
) -> Result<T, StoreError> {
    let plaintext = protection.read().expose(value, context)?;
    decode(&plaintext, context)
}

fn protect_text(
    protection: &SharedStateProtection,
    value: &str,
    context: &str,
) -> Result<String, StoreError> {
    protection.read().protect(value, context)
}

fn expose_text(
    protection: &SharedStateProtection,
    value: &str,
    context: &str,
) -> Result<String, StoreError> {
    protection.read().expose(value, context)
}

fn encode_enum<T: Serialize>(value: T) -> Result<String, StoreError> {
    let value = serde_json::to_value(value)?;
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| StoreError::Corrupt("enum did not serialize as a string".to_owned()))
}

fn decode<T: DeserializeOwned>(value: &str, field: &str) -> Result<T, StoreError> {
    serde_json::from_str(value).map_err(|error| StoreError::Corrupt(format!("{field}: {error}")))
}

fn decode_enum<T: DeserializeOwned>(value: &str, field: &str) -> Result<T, StoreError> {
    decode(&format!("\"{value}\""), field)
}

fn parse_time(value: &str, field: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| StoreError::Corrupt(format!("{field}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentctl_core::compile;
    use agentctl_core::dsl::{API_VERSION, EffectClass, Idempotency, Risk, parse_workflow};
    use agentctl_core::effect::EffectRequest;
    use tempfile::tempdir;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct FixedKeyResolver {
        keys: BTreeMap<String, Vec<u8>>,
    }

    impl FixedKeyResolver {
        fn with(reference: &str, byte: u8) -> Self {
            Self {
                keys: BTreeMap::from([(reference.to_owned(), vec![byte; 32])]),
            }
        }

        fn and(mut self, reference: &str, byte: u8) -> Self {
            self.keys.insert(reference.to_owned(), vec![byte; 32]);
            self
        }
    }

    impl StateKeyResolver for FixedKeyResolver {
        fn resolve(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, StoreError> {
            self.keys
                .get(reference)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| {
                    StoreError::Encryption(format!(
                        "fixed key reference `{reference}` is unavailable"
                    ))
                })
        }
    }

    fn fixture() -> (Value, CompiledPlan) {
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: store }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: one, uses: "action:assign", with: { value: 1 } }
"#;
        let workflow = parse_workflow(source, "fixture.yaml")
            .expect("parse")
            .workflow;
        let plan = compile(&workflow, "fixture.yaml").expect("compile");
        (serde_json::to_value(workflow).expect("json"), plan)
    }

    fn create(store: &SqliteStore, run_id: &str) {
        let (workflow, plan) = fixture();
        store
            .create_run(
                run_id,
                API_VERSION,
                &workflow,
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                RunMode::Execute,
                None,
                None,
                Path::new("."),
                Utc::now(),
                "trace",
            )
            .expect("create run");
    }

    fn begin_task(store: &SqliteStore, run_id: &str) {
        store
            .transition_task(
                run_id,
                "one",
                TaskState::Ready,
                None,
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("ready");
        store
            .transition_task(
                run_id,
                "one",
                TaskState::Running,
                None,
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("running");
    }

    #[test]
    fn budget_reservations_are_atomic_idempotent_and_reconcile_actual_usage() {
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: budget-store }
spec:
  runtime:
    maxConcurrency: 2
    budgets:
      maxProviderRequests: 1
      maxOutputTokens: 5
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: one, uses: "action:assign", with: { value: 1 } }
"#;
        let workflow = parse_workflow(source, "budget-store.yaml")
            .expect("parse")
            .workflow;
        let plan = compile(&workflow, "budget-store.yaml").expect("compile");
        let temporary = tempdir().expect("tempdir");
        let store = SqliteStore::open(&temporary.path().join("state.db")).expect("store");
        store
            .create_run(
                "budget-run",
                API_VERSION,
                &serde_json::to_value(&workflow).expect("workflow json"),
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                RunMode::Execute,
                None,
                None,
                Path::new("."),
                Utc::now(),
                "trace",
            )
            .expect("create run");
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles = ["reservation-a", "reservation-b"].map(|reservation_id| {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                (
                    reservation_id,
                    store
                        .reserve_budget(
                            "budget-run",
                            reservation_id,
                            Some("one"),
                            "provider",
                            &BudgetCounters {
                                provider_requests: 1,
                                output_tokens: 5,
                                ..BudgetCounters::default()
                            },
                            Utc::now(),
                            "trace",
                        )
                        .expect("reservation"),
                )
            })
        });
        barrier.wait();
        let decisions = handles.map(|handle| handle.join().expect("reservation thread"));
        let allowed = decisions
            .iter()
            .find_map(|(reservation_id, decision)| {
                matches!(decision, BudgetReservationDecision::Allowed(_)).then_some(*reservation_id)
            })
            .expect("one allowed reservation");
        assert_eq!(
            decisions
                .iter()
                .filter(|(_, decision)| {
                    matches!(decision, BudgetReservationDecision::Denied { .. })
                })
                .count(),
            1
        );
        assert!(matches!(
            store
                .reserve_budget(
                    "budget-run",
                    allowed,
                    Some("one"),
                    "provider",
                    &BudgetCounters {
                        provider_requests: 1,
                        output_tokens: 5,
                        ..BudgetCounters::default()
                    },
                    Utc::now(),
                    "trace",
                )
                .expect("idempotent reserve"),
            BudgetReservationDecision::Allowed(_)
        ));
        let snapshot = store
            .reconcile_budget(
                "budget-run",
                allowed,
                &BudgetCounters {
                    provider_requests: 1,
                    output_tokens: 5,
                    ..BudgetCounters::default()
                },
                "test",
                Utc::now(),
                "trace",
            )
            .expect("reconcile");
        assert_eq!(snapshot.usage.provider_requests, 1);
        assert_eq!(snapshot.usage.output_tokens, 5);
        assert_eq!(snapshot.reserved, BudgetCounters::default());
        assert_eq!(
            snapshot.exceeded,
            Some(BudgetExceeded {
                dimension: "providerRequests".to_owned(),
                limit: 1,
                attempted: 2,
            }),
            "a concurrent reconciliation must not clear the persisted denial"
        );
        let snapshot = store
            .record_wall_time("budget-run", 1, Utc::now(), "trace")
            .expect("record wall time");
        assert_eq!(
            snapshot
                .exceeded
                .as_ref()
                .map(|exceeded| exceeded.dimension.as_str()),
            Some("providerRequests"),
            "later observations must not clear the terminal budget cause"
        );
        assert_eq!(
            store
                .reconcile_budget(
                    "budget-run",
                    allowed,
                    &BudgetCounters {
                        provider_requests: 99,
                        output_tokens: 99,
                        ..BudgetCounters::default()
                    },
                    "duplicate",
                    Utc::now(),
                    "trace",
                )
                .expect("idempotent reconcile"),
            snapshot
        );
    }

    #[test]
    fn budget_reconciliation_records_actual_overrun() {
        let temporary = tempdir().expect("tempdir");
        let store = SqliteStore::open(&temporary.path().join("state.db")).expect("store");
        create(&store, "budget-overrun");
        store
            .connection()
            .execute(
                "UPDATE run_budgets SET limits_json = ?1 WHERE run_id = ?2",
                params![
                    encode(&ResourceBudgetDefinition {
                        max_output_tokens: Some(5),
                        ..ResourceBudgetDefinition::default()
                    })
                    .expect("limits"),
                    "budget-overrun"
                ],
            )
            .expect("set limit");
        assert!(matches!(
            store
                .reserve_budget(
                    "budget-overrun",
                    "provider",
                    Some("one"),
                    "provider",
                    &BudgetCounters {
                        output_tokens: 4,
                        ..BudgetCounters::default()
                    },
                    Utc::now(),
                    "trace",
                )
                .expect("reserve"),
            BudgetReservationDecision::Allowed(_)
        ));
        let snapshot = store
            .reconcile_budget(
                "budget-overrun",
                "provider",
                &BudgetCounters {
                    output_tokens: 6,
                    ..BudgetCounters::default()
                },
                "provider",
                Utc::now(),
                "trace",
            )
            .expect("reconcile");
        assert_eq!(
            snapshot.exceeded,
            Some(BudgetExceeded {
                dimension: "outputTokens".to_owned(),
                limit: 5,
                attempted: 6,
            })
        );
        assert_eq!(
            store
                .checkpoints("budget-overrun")
                .expect("checkpoints")
                .last()
                .expect("checkpoint")
                .state["budget"]["usage"]["outputTokens"],
            6
        );
    }

    #[test]
    fn every_dynamic_budget_dimension_is_inclusive_and_bounded() {
        let base = BudgetCounters {
            provider_requests: 1,
            turns: 1,
            tool_calls: 1,
            input_tokens: 1,
            output_tokens: 1,
            wall_time_seconds: 1,
            process_output_bytes: 1,
            artifact_bytes: 1,
            cost_microusd: 1,
            ..BudgetCounters::default()
        };
        let limits = ResourceBudgetDefinition {
            max_provider_requests: Some(1),
            max_turns: Some(1),
            max_tool_calls: Some(1),
            max_input_tokens: Some(1),
            max_output_tokens: Some(1),
            max_total_tokens: Some(2),
            max_wall_time_seconds: Some(1),
            max_process_output_bytes: Some(1),
            max_artifact_bytes: Some(1),
            max_cost_microusd: Some(1),
            ..ResourceBudgetDefinition::default()
        };
        assert_eq!(
            budget_exceeded(
                &limits,
                &base,
                &BudgetCounters::default(),
                &BudgetCounters::default()
            ),
            None,
            "exact equality is allowed"
        );
        let dimensions = [
            (
                "providerRequests",
                BudgetCounters {
                    provider_requests: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "turns",
                BudgetCounters {
                    turns: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "toolCalls",
                BudgetCounters {
                    tool_calls: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "inputTokens",
                BudgetCounters {
                    input_tokens: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "outputTokens",
                BudgetCounters {
                    output_tokens: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "wallTimeSeconds",
                BudgetCounters {
                    wall_time_seconds: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "processOutputBytes",
                BudgetCounters {
                    process_output_bytes: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "artifactBytes",
                BudgetCounters {
                    artifact_bytes: 1,
                    ..BudgetCounters::default()
                },
            ),
            (
                "costMicrousd",
                BudgetCounters {
                    cost_microusd: 1,
                    ..BudgetCounters::default()
                },
            ),
        ];
        for (expected, requested) in dimensions {
            assert_eq!(
                budget_exceeded(&limits, &base, &BudgetCounters::default(), &requested)
                    .expect("exceeded")
                    .dimension,
                expected
            );
        }
        assert_eq!(
            budget_exceeded(
                &limits,
                &BudgetCounters {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..BudgetCounters::default()
                },
                &BudgetCounters::default(),
                &BudgetCounters {
                    input_tokens: 1,
                    ..BudgetCounters::default()
                }
            )
            .expect("total token budget")
            .dimension,
            "inputTokens"
        );
        let total_only = ResourceBudgetDefinition {
            max_total_tokens: Some(2),
            ..ResourceBudgetDefinition::default()
        };
        assert_eq!(
            budget_exceeded(
                &total_only,
                &BudgetCounters {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..BudgetCounters::default()
                },
                &BudgetCounters::default(),
                &BudgetCounters {
                    input_tokens: 1,
                    ..BudgetCounters::default()
                }
            )
            .expect("total token budget")
            .dimension,
            "totalTokens"
        );
    }

    fn complete_with_artifact(store: &SqliteStore, run_id: &str, artifact: ArtifactRecord) {
        store
            .complete_task(
                run_id,
                "one",
                &serde_json::json!({"ok": true}),
                None,
                &TaskCompletionMetadata {
                    execution: TaskExecutionMetadata {
                        metadata_version: 1,
                        definition_fingerprint: "definition".to_owned(),
                        input_digest: "input".to_owned(),
                        output_contract_fingerprint: "contract".to_owned(),
                    },
                    output_digest: "output".to_owned(),
                    state_delta: serde_json::json!({}),
                    state_delta_digest: "state".to_owned(),
                    artifact_manifest: vec![artifact],
                },
                Utc::now(),
                "trace",
            )
            .expect("complete task");
    }

    fn create_version_database(path: &Path, version: u32) {
        let connection = Connection::open(path).expect("raw connection");
        for (index, migration) in [
            MIGRATION_1,
            MIGRATION_2,
            MIGRATION_3,
            MIGRATION_4,
            MIGRATION_5,
            MIGRATION_6,
            MIGRATION_7,
            MIGRATION_8,
            MIGRATION_9,
            MIGRATION_10,
            MIGRATION_11,
            MIGRATION_12,
            MIGRATION_13,
            MIGRATION_14,
        ]
        .into_iter()
        .enumerate()
        .take(usize::try_from(version).expect("version"))
        {
            connection
                .execute_batch(migration)
                .unwrap_or_else(|error| panic!("v{} schema: {error}", index + 1));
        }
        connection
            .pragma_update(None, "user_version", version)
            .expect("version marker");
    }

    #[test]
    fn fresh_database_migrates_and_permissions_are_private() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        let store = SqliteStore::open(&path).expect("open");
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn authenticated_envelope_binds_key_and_field_context() {
        let codec = EncryptionCodec::from_bytes("key-2026", vec![7; 32]).expect("encryption codec");
        let protected = codec
            .encrypt(r#"{"secret":"value"}"#, "runs.inputs_json")
            .expect("encrypt");
        assert!(protected.starts_with("agentctl.encrypted.v1:key-2026:"));
        assert!(!protected.contains(r#""secret":"value""#));
        assert_eq!(
            codec
                .decrypt(&protected, "runs.inputs_json")
                .expect("decrypt"),
            r#"{"secret":"value"}"#
        );
        assert!(codec.decrypt(&protected, "runs.output_json").is_err());
        let mut tampered = protected;
        tampered.push('x');
        assert!(codec.decrypt(&tampered, "runs.inputs_json").is_err());
    }

    #[test]
    fn encryption_inventory_migration_rotation_and_fail_closed_reads_are_transactional() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        let resolver = Arc::new(
            FixedKeyResolver::with("AGENTCTL_TEST_OLD_KEY", 1).and("AGENTCTL_TEST_NEW_KEY", 2),
        );
        let store =
            SqliteStore::open_with_key_resolver(&path, resolver.clone()).expect("open store");
        let marker = "protected-marker-value";
        let (mut workflow, plan) = fixture();
        workflow["testSensitiveValue"] = Value::String(marker.to_owned());
        store
            .create_run(
                "encrypted-run",
                API_VERSION,
                &workflow,
                &plan,
                &serde_json::json!({"secret": marker}),
                &serde_json::json!({"working": marker}),
                RunMode::Execute,
                None,
                None,
                directory.path(),
                Utc::now(),
                "trace-encryption",
            )
            .expect("create run");
        let effect = EffectRequest::new(
            "encrypted-run",
            "one",
            1,
            1,
            "test.sensitive",
            EffectClass::Observe,
            Risk::Low,
            Idempotency::Idempotent,
            serde_json::json!({"secret": marker}),
            marker,
            "trace-encryption",
        );
        store
            .record_effect_request(&effect, Utc::now())
            .expect("effect request");
        store
            .put_provider_session(
                "encrypted-run",
                "one",
                "fake",
                &serde_json::json!({"opaque": marker}),
                Utc::now(),
            )
            .expect("provider session");
        store
            .put_protocol_session(
                "encrypted-run",
                "one",
                &effect.id,
                "a2a",
                "https://agent.invalid",
                1,
                "polling",
                &serde_json::json!({"remoteTask": marker}),
                Utc::now(),
            )
            .expect("protocol session");
        store
            .put_protocol_call(
                &effect.id,
                "encrypted-run",
                "one",
                1,
                "a2a",
                "a2a.delegate",
                "message-1",
                1,
                "at_most_once",
                "polling",
                &serde_json::json!({"remoteTask": marker}),
                Utc::now(),
            )
            .expect("protocol call");
        let memory_entry = MemoryEntry::json(
            serde_json::json!({"value": marker}),
            Some(marker.to_owned()),
            BTreeMap::from([("classification".to_owned(), serde_json::json!("secret"))]),
        );
        let memory_embedding =
            agentctl_core::memory::local_hash_embedding(marker, 64).expect("memory embedding");
        store
            .put_memory_entry(
                "test",
                "secret",
                &memory_entry,
                Some("local_hash"),
                Some(&memory_embedding),
                None,
                Utc::now(),
            )
            .expect("memory");
        store
            .record_trace_event(
                "encrypted-run",
                "trace-encryption",
                &serde_json::json!({"detail": marker}),
                Utc::now(),
            )
            .expect("trace");

        let before = store.encryption_inventory().expect("inventory");
        assert!(!before.enabled);
        assert!(before.plaintext_values > 0);
        let dry_run = store
            .enable_encryption("keyXv1", "AGENTCTL_TEST_OLD_KEY", true, Utc::now())
            .expect("dry run");
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.values_rewritten, 0);
        assert!(!store.encryption_inventory().expect("inventory").enabled);

        let enabled = store
            .enable_encryption("keyXv1", "AGENTCTL_TEST_OLD_KEY", false, Utc::now())
            .expect("enable encryption");
        assert!(enabled.values_rewritten > 0);
        let inventory = store.encryption_inventory().expect("encrypted inventory");
        assert!(inventory.enabled);
        assert_eq!(inventory.key_id.as_deref(), Some("keyXv1"));
        assert_eq!(inventory.plaintext_values, 0);
        assert_eq!(inventory.invalid_envelopes, 0);
        assert_eq!(inventory.protected_values, inventory.encrypted_values);
        assert!(
            !serde_json::to_string(&inventory)
                .expect("inventory json")
                .contains(marker)
        );
        assert_eq!(
            store.load_run("encrypted-run").expect("run").inputs["secret"],
            marker
        );
        assert_eq!(
            store.load_effect(&effect.id).expect("effect").request.input["secret"],
            marker
        );
        assert_eq!(
            store
                .protocol_call(&effect.id)
                .expect("protocol call")
                .expect("protocol call record")
                .state["remoteTask"],
            marker
        );
        assert_eq!(
            store
                .get_long_term_memory("test", "secret", Utc::now())
                .expect("memory")
                .expect("present")["value"],
            marker
        );
        assert!(
            !store
                .checkpoints("encrypted-run")
                .expect("checkpoints")
                .is_empty()
        );

        let stale_ciphertext = {
            let connection = Connection::open(&path).expect("raw connection");
            for column in SENSITIVE_COLUMNS {
                let sql = format!(
                    "SELECT {} FROM {} WHERE {} IS NOT NULL",
                    column.column, column.table, column.column
                );
                let mut statement = connection.prepare(&sql).expect("prepare");
                let values = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .expect("query")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("values");
                for value in values {
                    assert!(is_encrypted_value(&value), "{}", column.context());
                    assert!(!value.contains(marker), "{}", column.context());
                }
            }
            assert!(
                connection
                    .execute(
                        "UPDATE runs SET inputs_json = 'plaintext' WHERE run_id = 'encrypted-run'",
                        [],
                    )
                    .is_err()
            );
            connection
                .query_row(
                    "SELECT inputs_json FROM runs WHERE run_id = 'encrypted-run'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("stale ciphertext")
        };

        let wrong = Arc::new(
            FixedKeyResolver::with("AGENTCTL_TEST_OLD_KEY", 9).and("AGENTCTL_TEST_NEW_KEY", 2),
        );
        assert!(SqliteStore::open_with_key_resolver(&path, wrong).is_err());

        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_test_rotation
                 BEFORE UPDATE OF inputs_json ON runs
                 WHEN (SELECT maintenance FROM state_encryption WHERE singleton = 1) = 1
                 BEGIN
                   SELECT RAISE(ABORT, 'injected rotation failure');
                 END;",
            )
            .expect("failure trigger");
        assert!(
            store
                .rotate_encryption_key("key_v1", "AGENTCTL_TEST_NEW_KEY", false, Utc::now(),)
                .is_err()
        );
        assert_eq!(
            store
                .encryption_inventory()
                .expect("post-rollback inventory")
                .key_id
                .as_deref(),
            Some("keyXv1")
        );
        assert_eq!(
            store
                .load_run("encrypted-run")
                .expect("rollback run")
                .inputs["secret"],
            marker
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_test_rotation")
            .expect("drop trigger");

        let rotation_plan = store
            .rotate_encryption_key("key_v1", "AGENTCTL_TEST_NEW_KEY", true, Utc::now())
            .expect("rotation plan");
        assert!(rotation_plan.dry_run);
        let rotated = store
            .rotate_encryption_key("key_v1", "AGENTCTL_TEST_NEW_KEY", false, Utc::now())
            .expect("rotate");
        assert!(rotated.values_rewritten > 0);
        assert_eq!(
            store
                .encryption_inventory()
                .expect("rotated inventory")
                .key_id
                .as_deref(),
            Some("key_v1")
        );
        assert!(
            Connection::open(&path)
                .expect("raw connection")
                .execute(
                    "UPDATE runs SET inputs_json = ?1 WHERE run_id = 'encrypted-run'",
                    [stale_ciphertext],
                )
                .is_err()
        );
        let new_only = Arc::new(FixedKeyResolver::with("AGENTCTL_TEST_NEW_KEY", 2));
        let reopened =
            SqliteStore::open_with_key_resolver(&path, new_only).expect("open with new key");
        assert_eq!(
            reopened
                .load_run("encrypted-run")
                .expect("rotated run")
                .inputs["secret"],
            marker
        );
        assert!(
            !reopened
                .checkpoints("encrypted-run")
                .expect("rotated checkpoints")
                .is_empty()
        );

        let old_for_new = Arc::new(FixedKeyResolver::with("AGENTCTL_TEST_NEW_KEY", 1));
        assert!(SqliteStore::open_with_key_resolver(&path, old_for_new).is_err());
        {
            let connection = Connection::open(&path).expect("raw connection");
            let mut stored: String = connection
                .query_row(
                    "SELECT inputs_json FROM runs WHERE run_id = 'encrypted-run'",
                    [],
                    |row| row.get(0),
                )
                .expect("stored input");
            stored.push('x');
            connection
                .execute(
                    "UPDATE runs SET inputs_json = ?1 WHERE run_id = 'encrypted-run'",
                    [stored],
                )
                .expect("tamper");
        }
        assert!(reopened.load_run("encrypted-run").is_err());
    }

    #[test]
    fn unknown_future_schema_fails_explicitly() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        let connection = Connection::open(&path).expect("open raw");
        connection
            .pragma_update(None, "user_version", 999)
            .expect("set version");
        drop(connection);
        assert!(matches!(
            SqliteStore::open(&path),
            Err(StoreError::UnknownSchema { found: 999, .. })
        ));
    }

    #[test]
    fn transition_and_checkpoint_commit_together() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        store
            .transition_task(
                "run",
                "one",
                TaskState::Ready,
                None,
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("ready");
        store
            .transition_task(
                "run",
                "one",
                TaskState::Running,
                None,
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("running");
        let output = serde_json::json!({"value": 1});
        store
            .transition_task(
                "run",
                "one",
                TaskState::Succeeded,
                Some(&output),
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("succeeded");
        let tasks = store.list_tasks("run").expect("tasks");
        assert_eq!(tasks[0].state, TaskState::Succeeded);
        assert_eq!(tasks[0].attempt, 1);
        assert_eq!(store.checkpoint_count("run").expect("count"), 4);
    }

    #[test]
    fn parallel_batch_commit_rolls_back_every_task_and_memory_on_failure() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        begin_task(&store, "run");
        let metadata = TaskCompletionMetadata {
            execution: TaskExecutionMetadata {
                metadata_version: 1,
                definition_fingerprint: "definition".to_owned(),
                input_digest: "input".to_owned(),
                output_contract_fingerprint: "contract".to_owned(),
            },
            output_digest: "output".to_owned(),
            state_delta: serde_json::json!({
                "formatVersion": 1,
                "set": {"value": "changed"},
                "remove": [],
            }),
            state_delta_digest: "delta".to_owned(),
            artifact_manifest: Vec::new(),
        };
        let result = store.commit_task_batch(
            "run",
            &[
                TaskBatchResult {
                    task_id: "one".to_owned(),
                    outcome: TaskBatchOutcome::Succeeded {
                        output: serde_json::json!({"ok": true}),
                        metadata: Box::new(metadata.clone()),
                    },
                },
                TaskBatchResult {
                    task_id: "missing".to_owned(),
                    outcome: TaskBatchOutcome::Succeeded {
                        output: serde_json::json!({"ok": true}),
                        metadata: Box::new(metadata),
                    },
                },
            ],
            Some(&serde_json::json!({"value": "changed"})),
            false,
            Utc::now(),
            "trace",
        );
        assert!(matches!(result, Err(StoreError::TaskNotFound { .. })));
        assert_eq!(
            store.list_tasks("run").expect("tasks")[0].state,
            TaskState::Running
        );
        assert_eq!(
            store.load_run("run").expect("run").working_memory,
            serde_json::json!({})
        );
    }

    #[test]
    fn corrupt_rows_fail_without_panicking() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        store
            .connection()
            .execute(
                "UPDATE runs SET plan_json = 'not-json' WHERE run_id = 'run'",
                [],
            )
            .expect("corrupt");
        assert!(matches!(store.load_run("run"), Err(StoreError::Corrupt(_))));
    }

    #[test]
    fn effect_identity_is_durable_and_started_effect_is_not_repeatable() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        let request = EffectRequest::new(
            "run",
            "one",
            1,
            1,
            "builtin.write",
            EffectClass::WorkspaceMutate,
            Risk::Medium,
            Idempotency::Idempotent,
            serde_json::json!({"path": "out.txt"}),
            "write out.txt",
            "trace",
        );
        store
            .record_effect_request(&request, Utc::now())
            .expect("record");
        store
            .mark_effect_started(&request.id, Utc::now())
            .expect("start");
        assert_eq!(
            store.unresolved_effects("run").expect("unresolved"),
            [request.id]
        );
    }

    #[test]
    fn reconciliation_is_immutable_audited_traced_and_rejects_contradictions() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        let effect = EffectRequest::new(
            "run",
            "one",
            1,
            1,
            "external.publish",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "publish record",
            "trace",
        );
        let now = Utc::now();
        store
            .record_effect_request(&effect, now)
            .expect("record effect");
        store.mark_effect_started(&effect.id, now).expect("start");
        store
            .mark_effect_uncertain(&effect.id, "unknown", now)
            .expect("uncertain");
        let applied = EffectReconciliationRequest {
            reconciliation_id: "reconciliation-applied-1".to_owned(),
            effect_id: effect.id.clone(),
            status: ReconciliationStatus::Applied,
            actor: "operator".to_owned(),
            reason: "external record exists".to_owned(),
            evidence: serde_json::json!({"externalId": "record-1"}),
            result: Some(serde_json::json!({"externalId": "record-1"})),
            result_schema: Some(serde_json::json!({"type": "object"})),
            authorization: serde_json::json!({"kind": "manual"}),
            compensation_effect_id: None,
            trace_id: "trace-reconcile".to_owned(),
        };
        let first = store
            .reconcile_effect(&applied, now)
            .expect("applied reconciliation");
        assert_eq!(first.status, ReconciliationStatus::Applied);
        assert!(
            store
                .unresolved_effects("run")
                .expect("resolved")
                .is_empty()
        );
        let source = store.load_effect(&effect.id).expect("immutable source");
        assert_eq!(source.status, EffectStatus::Uncertain);
        assert!(!source.confirmed);

        let mut superseding = applied.clone();
        superseding.reconciliation_id = "reconciliation-applied-2".to_owned();
        superseding.reason = "external record independently verified".to_owned();
        let second = store
            .reconcile_effect(&superseding, now + chrono::Duration::seconds(1))
            .expect("same-outcome supersession");
        assert_eq!(
            second.supersedes_id.as_deref(),
            Some("reconciliation-applied-1")
        );
        let mut contradiction = superseding;
        contradiction.reconciliation_id = "reconciliation-contradiction".to_owned();
        contradiction.status = ReconciliationStatus::NotApplied;
        contradiction.result = None;
        assert!(matches!(
            store.reconcile_effect(&contradiction, now + chrono::Duration::seconds(2)),
            Err(StoreError::Incompatible(_))
        ));
        let compensation = EffectRequest::new(
            "run",
            "one",
            1,
            2,
            "external.delete",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Idempotent,
            serde_json::json!({"externalId": "record-1"}),
            "delete record",
            "trace",
        );
        store
            .record_effect_request(&compensation, now)
            .expect("compensation");
        store
            .mark_effect_started(&compensation.id, now)
            .expect("start compensation");
        store
            .complete_effect(
                &compensation.id,
                Ok(&serde_json::json!({"deleted": true})),
                now,
            )
            .expect("complete compensation");
        let mut compensated = applied;
        compensated.reconciliation_id = "reconciliation-compensated".to_owned();
        compensated.status = ReconciliationStatus::Compensated;
        compensated.result = None;
        compensated.compensation_effect_id = Some(compensation.id);
        let compensated = store
            .reconcile_effect(&compensated, now + chrono::Duration::seconds(3))
            .expect("applied to compensated");
        assert_eq!(
            compensated.supersedes_id.as_deref(),
            Some("reconciliation-applied-2")
        );
        assert_eq!(
            store
                .effect_reconciliations(&effect.id)
                .expect("history")
                .len(),
            3
        );
        assert!(
            store
                .audit_events("run")
                .expect("audit")
                .iter()
                .any(|event| event.event_type == "effect.reconciled")
        );
        assert!(
            store
                .trace_events("run")
                .expect("traces")
                .iter()
                .any(|event| event.event["name"] == "effect.reconcile")
        );
    }

    #[test]
    fn reconciliation_compensation_requires_a_confirmed_same_run_effect() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        let now = Utc::now();
        let original = EffectRequest::new(
            "run",
            "one",
            1,
            1,
            "external.publish",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::AtMostOnce,
            serde_json::json!({"record": "x"}),
            "publish record",
            "trace",
        );
        store
            .record_effect_request(&original, now)
            .expect("original");
        store
            .mark_effect_started(&original.id, now)
            .expect("start original");
        store
            .complete_effect(
                &original.id,
                Ok(&serde_json::json!({"externalId": "record-1"})),
                now,
            )
            .expect("complete original");
        let compensation = EffectRequest::new(
            "run",
            "one",
            1,
            2,
            "external.delete",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Idempotent,
            serde_json::json!({"externalId": "record-1"}),
            "delete record",
            "trace",
        );
        store
            .record_effect_request(&compensation, now)
            .expect("compensation");
        store
            .mark_effect_started(&compensation.id, now)
            .expect("start compensation");
        let request = EffectReconciliationRequest {
            reconciliation_id: "reconciliation-compensated".to_owned(),
            effect_id: original.id.clone(),
            status: ReconciliationStatus::Compensated,
            actor: "operator".to_owned(),
            reason: "record was deleted".to_owned(),
            evidence: serde_json::json!({"externalId": "record-1", "deleted": true}),
            result: None,
            result_schema: None,
            authorization: serde_json::json!({"kind": "manual"}),
            compensation_effect_id: Some(compensation.id.clone()),
            trace_id: "trace-reconcile".to_owned(),
        };
        assert!(matches!(
            store.reconcile_effect(&request, now),
            Err(StoreError::Incompatible(_))
        ));
        store
            .complete_effect(
                &compensation.id,
                Ok(&serde_json::json!({"deleted": true})),
                now,
            )
            .expect("complete compensation");
        let record = store.reconcile_effect(&request, now).expect("compensated");
        assert_eq!(record.status, ReconciliationStatus::Compensated);
        assert_eq!(
            record.compensation_effect_id.as_deref(),
            Some(compensation.id.as_str())
        );
    }

    #[test]
    fn not_applied_reconciliation_can_only_supersede_the_same_outcome() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        let now = Utc::now();
        let effect = EffectRequest::new(
            "run",
            "one",
            1,
            1,
            "external.publish",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "publish",
            "trace",
        );
        store.record_effect_request(&effect, now).expect("effect");
        store.mark_effect_started(&effect.id, now).expect("start");
        store
            .mark_effect_uncertain(&effect.id, "unknown", now)
            .expect("uncertain");
        let request = EffectReconciliationRequest {
            reconciliation_id: "not-applied-1".to_owned(),
            effect_id: effect.id.clone(),
            status: ReconciliationStatus::NotApplied,
            actor: "operator".to_owned(),
            reason: "no remote record".to_owned(),
            evidence: serde_json::json!({"query": "empty"}),
            result: None,
            result_schema: None,
            authorization: serde_json::json!({"kind": "manual"}),
            compensation_effect_id: None,
            trace_id: "trace-reconcile".to_owned(),
        };
        store.reconcile_effect(&request, now).expect("not applied");
        let mut superseding = request.clone();
        superseding.reconciliation_id = "not-applied-2".to_owned();
        assert_eq!(
            store
                .reconcile_effect(&superseding, now + chrono::Duration::seconds(1))
                .expect("supersede")
                .supersedes_id
                .as_deref(),
            Some("not-applied-1")
        );
        superseding.reconciliation_id = "contradictory-applied".to_owned();
        superseding.status = ReconciliationStatus::Applied;
        superseding.result = Some(serde_json::json!({"record": "x"}));
        assert!(matches!(
            store.reconcile_effect(&superseding, now + chrono::Duration::seconds(2)),
            Err(StoreError::Incompatible(_))
        ));
    }

    #[test]
    fn tool_effect_and_call_completion_commit_atomically() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        let request = EffectRequest::new(
            "run",
            "one",
            1,
            1,
            "tool.echo",
            EffectClass::Pure,
            Risk::Low,
            Idempotency::Pure,
            serde_json::json!({"text": "hello"}),
            "execute echo",
            "trace",
        );
        let now = Utc::now();
        store
            .record_effect_request(&request, now)
            .expect("record effect");
        store
            .mark_effect_started(&request.id, now)
            .expect("start effect");
        store
            .start_tool_call(
                "call-1",
                "run",
                "one",
                &request.id,
                "echo",
                &request.input_digest,
                now,
            )
            .expect("start call");
        let output = serde_json::json!({"text": "hello"});

        assert!(
            store
                .complete_tool_effect(
                    &request.id,
                    "run",
                    "missing-call",
                    Ok(&output),
                    Some("digest"),
                    now,
                )
                .is_err()
        );
        assert_eq!(
            store.load_effect(&request.id).expect("effect").status,
            EffectStatus::Started
        );
        assert_eq!(store.tool_calls("run").expect("calls")[0].status, "started");

        store
            .complete_tool_effect(
                &request.id,
                "run",
                "call-1",
                Ok(&output),
                Some("digest"),
                now,
            )
            .expect("complete atomically");
        let effect = store.load_effect(&request.id).expect("effect");
        assert_eq!(effect.status, EffectStatus::Succeeded);
        assert!(effect.confirmed);
        assert_eq!(
            store.tool_calls("run").expect("calls")[0].status,
            "succeeded"
        );
    }

    #[test]
    fn audit_sequence_continues_across_resume() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "run");
        store
            .transition_task(
                "run",
                "one",
                TaskState::Ready,
                None,
                None,
                None,
                Utc::now(),
                "trace",
            )
            .expect("ready");
        let events = store.audit_events("run").expect("events");
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn upgrades_a_version_one_database_transactionally() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        create_version_database(&path, 1);

        let store = SqliteStore::open(&path).expect("upgrade");
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
        assert_eq!(store.stats().expect("stats").long_term_memory, 0);
    }

    #[test]
    fn upgrades_every_retained_database_schema_fixture() {
        for version in 1..DATABASE_SCHEMA_VERSION {
            let directory = tempdir().expect("temp dir");
            let path = directory.path().join(format!("runtime-v{version}.db"));
            create_version_database(&path, version);
            let store = SqliteStore::open(&path)
                .unwrap_or_else(|error| panic!("upgrade schema {version}: {error}"));
            assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
            assert_eq!(store.stats().expect("stats").run_upgrades, 0);
        }
    }

    #[test]
    fn migration_fourteen_preserves_legacy_memory_as_typed_entries() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime-v13.db");
        create_version_database(&path, 13);
        let now = Utc::now();
        Connection::open(&path)
            .expect("legacy connection")
            .execute(
                "INSERT INTO long_term_memory (namespace, memory_key, value_json, expires_at, updated_at)
                 VALUES ('legacy', 'greeting', '\"hello\"', NULL, ?1)",
                [now.to_rfc3339()],
            )
            .expect("legacy memory");

        let store = SqliteStore::open(&path).expect("migrate");
        let record = store
            .get_memory_entry("legacy", "greeting", now)
            .expect("read")
            .expect("record");
        assert_eq!(record.entry.value(), serde_json::json!("hello"));
        assert_eq!(record.entry.searchable_text(), Some("hello"));
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
    }

    #[test]
    fn migration_fifteen_initializes_unlimited_ledgers_for_retained_runs() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime-v14.db");
        create_version_database(&path, 14);
        let now = Utc::now().to_rfc3339();
        let connection = Connection::open(&path).expect("legacy connection");
        connection
            .execute(
                "INSERT INTO runs
                 (run_id, runtime_state_version, workflow_digest, workflow_schema_version,
                  plan_digest, plan_format_version, workflow_json, plan_json, inputs_json,
                  working_memory_json, state, mode, cancellation_requested, created_at, updated_at)
                 VALUES ('retained', 1, 'workflow', 'agentctl.dev/v1alpha1', 'plan', 1,
                         '{}', '{}', '{}', '{}', 'running', 'execute', 0, ?1, ?1)",
                [&now],
            )
            .expect("legacy run");
        connection
            .execute(
                "INSERT INTO task_states
                 (run_id, task_id, position, state, updated_at)
                 VALUES ('retained', 'one', 0, 'pending', ?1)",
                [&now],
            )
            .expect("legacy task");
        drop(connection);

        let store = SqliteStore::open(&path).expect("migrate");
        let budget = store.budget_snapshot("retained").expect("budget");
        assert!(budget.limits.is_empty());
        assert_eq!(budget.planned_tasks, 1);
        assert_eq!(budget.usage, BudgetCounters::default());
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
    }

    #[test]
    fn upgrades_the_pre_repair_schema_and_creates_repair_records() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        create_version_database(&path, 4);

        let store = SqliteStore::open(&path).expect("upgrade");
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
        create(&store, "source");
        let (workflow, plan) = fixture();
        store
            .create_repair_run(
                "repair",
                "source",
                &plan.workflow_digest,
                API_VERSION,
                &workflow,
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                &["one".to_owned()],
                Some("migration test"),
                &[],
                &serde_json::json!([]),
                Path::new("."),
                Utc::now(),
                "trace-repair",
            )
            .expect("create repair");

        let repair = store.load_run("repair").expect("repair run");
        assert_eq!(repair.mode, RunMode::Repair);
        assert_eq!(repair.source_run_id.as_deref(), Some("source"));
        assert_eq!(repair.repair_roots, ["one"]);
        assert_eq!(
            store.list_tasks("repair").expect("repair tasks")[0].disposition,
            TaskDisposition::Executed
        );
    }

    #[test]
    fn creates_retry_lineage_separately_from_repair_metadata() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "source");
        let (workflow, plan) = fixture();
        store
            .create_retry_run(
                "retry",
                "source",
                &plan.workflow_digest,
                API_VERSION,
                &workflow,
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                &["one".to_owned()],
                true,
                Some("retry failed tasks"),
                &[],
                &serde_json::json!([]),
                Path::new("."),
                Utc::now(),
                "trace-retry",
            )
            .expect("create retry");

        let retry = store.load_run("retry").expect("retry run");
        assert_eq!(retry.mode, RunMode::Retry);
        assert_eq!(retry.source_run_id.as_deref(), Some("source"));
        assert_eq!(retry.retry_roots, ["one"]);
        assert_eq!(retry.retry_reason.as_deref(), Some("retry failed tasks"));
        assert_eq!(retry.retry_format_version, Some(1));
        assert!(retry.retry_failed_only);
        assert!(retry.repair_roots.is_empty());
        assert_eq!(
            store.list_tasks("retry").expect("tasks")[0].disposition,
            TaskDisposition::Executed
        );
        assert!(
            store
                .audit_events("retry")
                .expect("audit")
                .iter()
                .any(|event| event.event_type == "retry.created")
        );
    }

    #[test]
    fn protocol_lineage_can_be_recorded_into_multiple_replays() {
        let store = SqliteStore::open_memory().expect("store");
        create(&store, "source");
        store
            .put_protocol_session(
                "source",
                "one",
                "effect-source",
                "a2a",
                "https://agent.invalid",
                1,
                "completed",
                &serde_json::json!({"remoteTaskId": "task-1"}),
                Utc::now(),
            )
            .expect("session");
        store
            .put_protocol_call(
                "effect-source",
                "source",
                "one",
                1,
                "a2a",
                "a2a.delegate",
                "message-1",
                1,
                "at_most_once",
                "succeeded",
                &serde_json::json!({"remoteTaskId": "task-1"}),
                Utc::now(),
            )
            .expect("call");
        let (workflow, plan) = fixture();
        for replay_run_id in ["replay-one", "replay-two"] {
            store
                .create_run(
                    replay_run_id,
                    API_VERSION,
                    &workflow,
                    &plan,
                    &serde_json::json!({}),
                    &serde_json::json!({}),
                    RunMode::Replay,
                    Some("source"),
                    Some("source"),
                    Path::new("."),
                    Utc::now(),
                    "trace-replay",
                )
                .expect("replay run");
            store
                .copy_protocol_records_for_replay(replay_run_id, "source", Utc::now())
                .expect("copy protocol lineage");
            let calls = store.protocol_calls(replay_run_id).expect("calls");
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].status, "recorded");
            assert_eq!(calls[0].source_run_id.as_deref(), Some("source"));
            assert_eq!(calls[0].source_effect_id.as_deref(), Some("effect-source"));
            assert!(calls[0].effect_id.contains(replay_run_id));
        }
    }

    #[test]
    fn interrupted_repair_migration_can_restart_cleanly() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        create_version_database(&path, 4);
        let mut connection = Connection::open(&path).expect("raw connection");
        let transaction = connection.transaction().expect("migration transaction");
        transaction
            .execute_batch("ALTER TABLE runs ADD COLUMN source_run_id TEXT;")
            .expect("partial migration");
        transaction.rollback().expect("simulate interruption");
        drop(connection);

        let store = SqliteStore::open(&path).expect("restart migration");
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
        create(&store, "run");
        assert_eq!(store.list_tasks("run").expect("tasks").len(), 1);
    }

    #[test]
    fn repair_creation_rolls_back_every_row_on_materialization_failure() {
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, mut plan) = fixture();
        plan.order.push("one".to_owned());
        let result = store.create_repair_run(
            "repair",
            "source",
            &plan.workflow_digest,
            API_VERSION,
            &workflow,
            &plan,
            &serde_json::json!({}),
            &serde_json::json!({}),
            &["one".to_owned()],
            None,
            &[],
            &serde_json::json!([]),
            Path::new("."),
            Utc::now(),
            "trace",
        );

        assert!(matches!(result, Err(StoreError::Sqlite(_))));
        assert!(matches!(
            store.load_run("repair"),
            Err(StoreError::RunNotFound(_))
        ));
        assert_eq!(store.stats().expect("stats").runs, 0);
    }

    #[test]
    fn legacy_run_upgrade_rolls_back_task_mutations_on_failure() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("runtime.db");
        let artifact_path = directory.path().join("legacy.txt");
        std::fs::write(&artifact_path, b"legacy artifact").expect("artifact");
        let store = SqliteStore::open(&database).expect("store");
        create(&store, "legacy");
        let now = Utc::now();
        begin_task(&store, "legacy");
        store
            .transition_task(
                "legacy",
                "one",
                TaskState::Succeeded,
                Some(&serde_json::json!({"ok": true})),
                None,
                None,
                now,
                "trace",
            )
            .expect("legacy success");
        store
            .update_run_state(
                "legacy",
                RunState::Succeeded,
                Some(&serde_json::json!({"ok": true})),
                now,
                "trace",
            )
            .expect("terminal");
        let artifact = store
            .ingest_artifact("legacy", "one", &artifact_path, "legacy.txt", 1024, now)
            .expect("ingest");
        let metadata = TaskCompletionMetadata {
            execution: TaskExecutionMetadata {
                metadata_version: 1,
                definition_fingerprint: "definition".to_owned(),
                input_digest: "input".to_owned(),
                output_contract_fingerprint: "contract".to_owned(),
            },
            output_digest: "output".to_owned(),
            state_delta: serde_json::json!({"set": {}, "remove": []}),
            state_delta_digest: "state".to_owned(),
            artifact_manifest: vec![artifact],
        };
        let result = store.apply_legacy_run_upgrade(
            "upgrade",
            "legacy",
            &serde_json::json!({"dryRun": false}),
            &[
                LegacyTaskUpgrade {
                    task_id: "one".to_owned(),
                    metadata: metadata.clone(),
                    provenance: serde_json::json!({"confidence": "proven"}),
                },
                LegacyTaskUpgrade {
                    task_id: "missing".to_owned(),
                    metadata,
                    provenance: serde_json::json!({"confidence": "proven"}),
                },
            ],
            now,
            "trace",
        );

        assert!(matches!(result, Err(StoreError::Incompatible(_))));
        assert_eq!(
            store.list_tasks("legacy").expect("tasks")[0].metadata_version,
            None
        );
        let stats = store.stats().expect("stats");
        assert_eq!(stats.run_upgrades, 0);
        assert_eq!(stats.artifact_references, 0);
        assert_eq!(stats.artifact_ingests, 1);
    }

    #[test]
    fn concurrent_readers_and_bounded_lock_wait_succeed() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        let store = SqliteStore::open(&path).expect("store");
        create(&store, "run");
        let reader_path = path.clone();
        let reader = std::thread::spawn(move || {
            let store = SqliteStore::open(&reader_path).expect("reader store");
            store.load_run("run").expect("concurrent read").run_id
        });
        assert_eq!(reader.join().expect("reader thread"), "run");

        let writer_path = path.clone();
        let blocker = Connection::open(&path).expect("blocker");
        blocker.execute_batch("BEGIN IMMEDIATE").expect("lock");
        let writer = std::thread::spawn(move || {
            let store = SqliteStore::open(&writer_path).expect("writer store");
            store.put_long_term_memory("test", "key", &serde_json::json!(true), None, Utc::now())
        });
        std::thread::sleep(Duration::from_millis(25));
        blocker.execute_batch("ROLLBACK").expect("unlock");
        writer.join().expect("writer thread").expect("bounded wait");
    }

    #[test]
    fn garbage_collection_removes_only_expired_memory() {
        let store = SqliteStore::open_memory().expect("store");
        let now = Utc::now();
        store
            .put_long_term_memory(
                "test",
                "expired",
                &serde_json::json!(1),
                Some(now - chrono::Duration::seconds(1)),
                now,
            )
            .expect("expired memory");
        store
            .put_long_term_memory(
                "test",
                "live",
                &serde_json::json!(2),
                Some(now + chrono::Duration::days(1)),
                now,
            )
            .expect("live memory");
        assert_eq!(store.garbage_collect(now).expect("gc"), 1);
        assert_eq!(
            store
                .get_long_term_memory("test", "live", now)
                .expect("read"),
            Some(serde_json::json!(2))
        );
    }

    #[test]
    fn typed_memory_search_is_stable_filtered_and_retention_aware() {
        let store = SqliteStore::open_memory().expect("store");
        let now = Utc::now();
        let first = MemoryEntry::text(
            "Rust durable workflow repair",
            BTreeMap::from([
                ("project".to_owned(), serde_json::json!("agentctl")),
                ("kind".to_owned(), serde_json::json!("guide")),
            ]),
        );
        let second = MemoryEntry::text(
            "Python web application",
            BTreeMap::from([("project".to_owned(), serde_json::json!("other"))]),
        );
        let expired = MemoryEntry::text(
            "Rust expired note",
            BTreeMap::from([("project".to_owned(), serde_json::json!("agentctl"))]),
        );
        for (key, entry, expires_at) in [
            ("repair", &first, Some(now + chrono::Duration::days(1))),
            ("web", &second, None),
            (
                "expired",
                &expired,
                Some(now - chrono::Duration::seconds(1)),
            ),
        ] {
            let embedding =
                agentctl_core::memory::local_hash_embedding(entry.searchable_text().unwrap(), 64)
                    .expect("embedding");
            store
                .put_memory_entry(
                    "docs",
                    key,
                    entry,
                    Some("local_hash"),
                    Some(&embedding),
                    expires_at,
                    now,
                )
                .expect("put");
        }
        let query = MemoryQuery {
            namespace: "docs".to_owned(),
            text: "Rust workflow".to_owned(),
            mode: MemorySearchMode::Hybrid,
            limit: 10,
            filters: BTreeMap::from([("project".to_owned(), serde_json::json!("agentctl"))]),
        };
        let embedding =
            agentctl_core::memory::local_hash_embedding(&query.text, 64).expect("query embedding");
        let results = store
            .search_memory(&query, Some(&embedding), now)
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].record.key, "repair");
        assert!(results[0].score_millionths > 0);
        assert_eq!(
            store
                .get_memory_entry("docs", "repair", now)
                .expect("get")
                .expect("record")
                .entry,
            first
        );
        assert!(
            store
                .get_memory_entry("docs", "expired", now)
                .expect("expired")
                .is_none()
        );
        store
            .connection()
            .execute(
                "UPDATE long_term_memory SET embedding_dimensions = 63
                 WHERE namespace = 'docs' AND memory_key = 'repair'",
                [],
            )
            .expect("corrupt dimensions");
        assert!(matches!(
            store.search_memory(&query, Some(&embedding), now),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn artifacts_are_deduplicated_referenced_and_durable_without_workspace_files() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("state").join("runtime.db");
        let source = directory.path().join("workspace").join("report.txt");
        std::fs::create_dir_all(source.parent().expect("source parent")).expect("workspace");
        std::fs::write(&source, b"durable report").expect("source");
        let store = SqliteStore::open(&database).expect("store");

        create(&store, "first");
        begin_task(&store, "first");
        let first = store
            .ingest_artifact("first", "one", &source, "report.txt", 1024, Utc::now())
            .expect("first ingest");
        assert_eq!(store.stats().expect("stats").artifact_ingests, 1);
        complete_with_artifact(&store, "first", first.clone());
        assert_eq!(store.stats().expect("stats").artifact_ingests, 0);

        create(&store, "second");
        begin_task(&store, "second");
        let second = store
            .ingest_artifact("second", "one", &source, "report.json", 1024, Utc::now())
            .expect("deduplicated ingest");
        assert_eq!(first.digest, second.digest);
        complete_with_artifact(&store, "second", second);
        assert_eq!(store.stats().expect("stats").artifact_blobs, 1);
        let references = store.artifact_references(None, None).expect("references");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].media_type, "text/plain");
        assert_eq!(references[1].media_type, "application/json");

        std::fs::remove_file(&source).expect("remove workspace source");
        drop(store);
        let reopened = SqliteStore::open(&database).expect("reopen");
        let verification = reopened
            .verify_artifact(&first.digest, Utc::now())
            .expect("verify without workspace");
        assert!(verification.valid);
        let export = directory.path().join("export").join("report.txt");
        reopened
            .export_artifact(&first.digest, &export, false)
            .expect("export");
        assert_eq!(
            std::fs::read(export).expect("export bytes"),
            b"durable report"
        );
    }

    #[test]
    fn concurrent_ingests_share_one_blob_and_keep_independent_leases() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("state").join("runtime.db");
        let source = directory.path().join("source.bin");
        std::fs::write(&source, b"same bytes").expect("source");
        let store = SqliteStore::open(&database).expect("store");
        create(&store, "left");
        create(&store, "right");
        drop(store);

        let workers = ["left", "right"].map(|run_id| {
            let database = database.clone();
            let source = source.clone();
            std::thread::spawn(move || {
                let store = SqliteStore::open(&database).expect("worker store");
                store
                    .ingest_artifact(
                        run_id,
                        "one",
                        &source,
                        &format!("{run_id}.bin"),
                        1024,
                        Utc::now(),
                    )
                    .expect("concurrent ingest")
            })
        });
        let [left_worker, right_worker] = workers;
        let left = left_worker.join().expect("left worker");
        let right = right_worker.join().expect("right worker");
        assert_eq!(left.digest, right.digest);
        let store = SqliteStore::open(&database).expect("reopen");
        let stats = store.stats().expect("stats");
        assert_eq!(stats.artifact_blobs, 1);
        assert_eq!(stats.artifact_ingests, 2);
    }

    #[test]
    fn artifact_gc_respects_leases_references_orphans_and_partial_files() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("state").join("runtime.db");
        let source = directory.path().join("artifact.bin");
        std::fs::write(&source, b"leased").expect("source");
        let store = SqliteStore::open(&database).expect("store");
        create(&store, "leased");
        let leased = store
            .ingest_artifact("leased", "one", &source, "leased.bin", 1024, Utc::now())
            .expect("leased ingest");
        let future = Utc::now() + chrono::Duration::days(1);
        let protected = store
            .garbage_collect_artifacts(future, false)
            .expect("lease-protected gc");
        assert!(protected.removed.is_empty());
        store
            .verify_artifact(&leased.digest, Utc::now())
            .expect("leased blob remains");

        store
            .connection()
            .execute(
                "UPDATE artifact_ingests SET expires_at = ?1",
                [(Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()],
            )
            .expect("expire lease");
        let removed = store
            .garbage_collect_artifacts(future, false)
            .expect("expired lease gc");
        assert_eq!(removed.removed, [leased.digest]);

        let orphan = store
            .artifact_store
            .ingest(&source, 1024)
            .expect("orphan blob");
        let partial = store.artifact_root().join("tmp").join("interrupted");
        std::fs::write(&partial, b"partial").expect("partial file");
        let preview = store
            .garbage_collect_artifacts(future, true)
            .expect("dry run");
        assert_eq!(preview.considered, 1);
        assert_eq!(preview.temporary_files_considered, 1);
        assert_eq!(preview.temporary_files_removed, 0);
        assert!(partial.exists());
        let collected = store
            .garbage_collect_artifacts(future, false)
            .expect("orphan gc");
        assert_eq!(collected.removed, [orphan.digest]);
        assert_eq!(collected.temporary_files_removed, 1);
        assert!(!partial.exists());
    }

    #[test]
    fn artifact_quarantine_is_recovered_after_interrupted_gc() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("state").join("runtime.db");
        let source = directory.path().join("artifact.bin");
        std::fs::write(&source, b"recoverable").expect("source");
        let store = SqliteStore::open(&database).expect("store");
        create(&store, "run");
        begin_task(&store, "run");
        let artifact = store
            .ingest_artifact("run", "one", &source, "artifact.bin", 1024, Utc::now())
            .expect("ingest");
        complete_with_artifact(&store, "run", artifact.clone());
        let staged = store
            .artifact_store
            .stage_remove(&artifact.digest)
            .expect("stage")
            .expect("staged path");
        assert!(staged.exists());
        drop(store);

        let reopened = SqliteStore::open(&database).expect("recover quarantine");
        reopened
            .verify_artifact(&artifact.digest, Utc::now())
            .expect("restored referenced artifact");
        assert!(!staged.exists());
    }
}
