//! Versioned SQLite persistence for agentctl.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agentctl_core::effect::{EffectRecord, EffectRequest, EffectStatus};
use agentctl_core::state::{RunState, TaskState};
use agentctl_core::{CompiledPlan, PLAN_FORMAT_VERSION};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const DATABASE_SCHEMA_VERSION: u32 = 5;
pub const RUNTIME_STATE_VERSION: u32 = 1;
pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;
pub const AUDIT_EVENT_VERSION: u32 = 1;

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

#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
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
    pub tool_calls: i64,
    pub trace_events: i64,
    pub long_term_memory: i64,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
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
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn open_memory() -> Result<Self, StoreError> {
        let mut connection = Connection::open_in_memory()?;
        configure(&connection)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.connection
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0)
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
        base_path: &Path,
        now: DateTime<Utc>,
        trace_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO runs (run_id, runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, state, mode, parent_run_id, base_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            params![
                run_id,
                RUNTIME_STATE_VERSION,
                plan.workflow_digest,
                workflow_schema_version,
                plan.plan_digest,
                plan.format_version,
                encode(workflow)?,
                encode(plan)?,
                encode(inputs)?,
                encode(working_memory)?,
                encode_enum(RunState::Running)?,
                encode_enum(mode)?,
                parent_run_id,
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
        append_audit_tx(
            &transaction,
            run_id,
            "run.created",
            None,
            trace_id,
            &serde_json::json!({"mode": mode, "planDigest": plan.plan_digest}),
            now,
        )?;
        checkpoint_tx(&transaction, run_id, now)?;
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
                encode(workflow)?,
                encode(plan)?,
                encode(inputs)?,
                encode(working_memory)?,
                encode_enum(RunState::Running)?,
                encode_enum(RunMode::Repair)?,
                source_run_id,
                source_workflow_digest,
                encode(repair_roots)?,
                reason,
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
                        encode(&task.output)?,
                        encode_enum(TaskDisposition::Reused)?,
                        task.metadata.execution.metadata_version,
                        task.source_run_id,
                        task.source_task_id,
                        task.source_attempt,
                        task.metadata.execution.definition_fingerprint,
                        task.metadata.execution.input_digest,
                        task.metadata.execution.output_contract_fingerprint,
                        task.metadata.output_digest,
                        encode(&task.metadata.state_delta)?,
                        task.metadata.state_delta_digest,
                        encode(&task.metadata.artifact_manifest)?,
                        encode(&task.reuse_decision)?,
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
        )?;
        checkpoint_tx(&transaction, run_id, now)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_run(&self, run_id: &str) -> Result<RunRecord, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT runtime_state_version, workflow_digest, workflow_schema_version, plan_digest, plan_format_version, workflow_json, plan_json, inputs_json, working_memory_json, output_json, state, mode, parent_run_id, cancellation_requested, created_at, updated_at, base_path, source_run_id, source_workflow_digest, repair_roots_json, repair_reason, repair_format_version FROM runs WHERE run_id = ?1",
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
                    workflow: decode(&row.5, "workflow_json")?,
                    plan: decode(&row.6, "plan_json")?,
                    inputs: decode(&row.7, "inputs_json")?,
                    working_memory: decode(&row.8, "working_memory_json")?,
                    output: row.9.map(|value| decode(&value, "output_json")).transpose()?,
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
                    repair_reason: row.20,
                    repair_format_version: row.21,
                    base_path: row.16,
                    cancellation_requested: row.13,
                    created_at: parse_time(&row.14, "created_at")?,
                    updated_at: parse_time(&row.15, "updated_at")?,
                })
            })
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
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_tasks(&self, run_id: &str) -> Result<Vec<TaskRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT task_id, position, state, attempt, output_json, error, updated_at, disposition, metadata_version, source_run_id, source_task_id, source_attempt, definition_fingerprint, input_digest, output_contract_fingerprint, output_digest, state_delta_json, state_delta_digest, artifact_manifest_json, reuse_decision_json FROM task_states WHERE run_id = ?1 ORDER BY position",
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
                    .map(|value| decode(&value, "task.output"))
                    .transpose()?,
                error: row.5,
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
                    .map(|value| decode(&value, "task.state_delta"))
                    .transpose()?,
                state_delta_digest: row.17,
                artifact_manifest: row
                    .18
                    .map(|value| decode(&value, "task.artifact_manifest"))
                    .transpose()?
                    .unwrap_or_default(),
                reuse_decision: row
                    .19
                    .map(|value| decode(&value, "task.reuse_decision"))
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
            "UPDATE task_states SET state = ?3, output_json = COALESCE(?4, output_json), error = ?5, attempt = attempt + ?7, updated_at = ?6 WHERE run_id = ?1 AND task_id = ?2",
            params![run_id, task_id, encode_enum(next)?, output.map(encode).transpose()?, error, now.to_rfc3339(), i64::from(current == TaskState::Ready && next == TaskState::Running)],
        )?;
        if let Some(memory) = working_memory {
            transaction.execute(
                "UPDATE runs SET working_memory_json = ?2, updated_at = ?3 WHERE run_id = ?1",
                params![run_id, encode(memory)?, now.to_rfc3339()],
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
            &serde_json::json!({"from": current, "to": next, "error": error}),
            now,
        )?;
        checkpoint_tx(&transaction, run_id, now)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_task_execution_metadata(
        &self,
        run_id: &str,
        task_id: &str,
        metadata: &TaskExecutionMetadata,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let changed = self.connection.lock().execute(
            "UPDATE task_states SET metadata_version = ?3, definition_fingerprint = ?4, input_digest = ?5, output_contract_fingerprint = ?6, updated_at = ?7 WHERE run_id = ?1 AND task_id = ?2 AND state = ?8",
            params![
                run_id,
                task_id,
                metadata.metadata_version,
                metadata.definition_fingerprint,
                metadata.input_digest,
                metadata.output_contract_fingerprint,
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
            .transition(TaskState::Succeeded)
            .map_err(|transition| StoreError::InvalidTransition(transition.to_string()))?;
        transaction.execute(
            "UPDATE task_states SET state = ?3, output_json = ?4, error = NULL, disposition = ?5, metadata_version = ?6, definition_fingerprint = ?7, input_digest = ?8, output_contract_fingerprint = ?9, output_digest = ?10, state_delta_json = ?11, state_delta_digest = ?12, artifact_manifest_json = ?13, updated_at = ?14 WHERE run_id = ?1 AND task_id = ?2",
            params![
                run_id,
                task_id,
                encode_enum(TaskState::Succeeded)?,
                encode(output)?,
                encode_enum(TaskDisposition::Executed)?,
                metadata.execution.metadata_version,
                metadata.execution.definition_fingerprint,
                metadata.execution.input_digest,
                metadata.execution.output_contract_fingerprint,
                metadata.output_digest,
                encode(&metadata.state_delta)?,
                metadata.state_delta_digest,
                encode(&metadata.artifact_manifest)?,
                now.to_rfc3339(),
            ],
        )?;
        if let Some(memory) = working_memory {
            transaction.execute(
                "UPDATE runs SET working_memory_json = ?2, updated_at = ?3 WHERE run_id = ?1",
                params![run_id, encode(memory)?, now.to_rfc3339()],
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
                "to": TaskState::Succeeded,
                "disposition": TaskDisposition::Executed,
                "outputDigest": metadata.output_digest,
                "stateDeltaDigest": metadata.state_delta_digest,
            }),
            now,
        )?;
        checkpoint_tx(&transaction, run_id, now)?;
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
                source.state_delta.as_ref().map(encode).transpose()?,
                source.state_delta_digest,
                encode(&source.artifact_manifest)?,
                encode(&serde_json::json!({
                    "recordedFromRunId": source.run_id,
                    "sourceDisposition": source.disposition,
                    "sourceProvenance": source.reuse_decision,
                }))?,
                now.to_rfc3339(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::TaskNotFound {
                run_id: replay_run_id.to_owned(),
                task_id: source.task_id.clone(),
            });
        }
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
        )?;
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
            params![run_id, encode_enum(state)?, output.map(encode).transpose()?, now.to_rfc3339()],
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
        )?;
        checkpoint_tx(&transaction, run_id, now)?;
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
                encode(&request.input)?,
                request.expected_effect,
                request.trace_id,
                encode_enum(EffectStatus::Requested)?,
                now.to_rfc3339(),
            ],
        )?;
        append_audit_tx(
            &transaction,
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
            }),
            now,
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
            Ok(output) => (EffectStatus::Succeeded, Some(encode(output)?), None, true),
            Err(error) => (EffectStatus::Failed, None, Some(error), false),
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
                error,
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
                        input: decode(&row.11, "effect.input")?,
                        expected_effect: row.12,
                        trace_id: row.13,
                    },
                    status: decode_enum(&row.14, "effect.status")?,
                    attempt_number: row.15,
                    requested_at: parse_time(&row.16, "effect.requested_at")?,
                    started_at: row.17.map(|value| parse_time(&value, "effect.started_at")).transpose()?,
                    completed_at: row.18.map(|value| parse_time(&value, "effect.completed_at")).transpose()?,
                    result: row.19.map(|value| decode(&value, "effect.result")).transpose()?,
                    error: row.20,
                    confirmed: row.21,
                })
            })
    }

    pub fn unresolved_effects(&self, run_id: &str) -> Result<Vec<String>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT effect_id FROM effects WHERE run_id = ?1 AND status IN ('started', 'uncertain') ORDER BY rowid",
        )?;
        statement
            .query_map([run_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn reconcile_effect_not_applied(
        &self,
        effect_id: &str,
        actor: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_id: Option<String> = transaction
            .query_row(
                "SELECT run_id FROM effects WHERE effect_id = ?1 AND status IN (?2, ?3)",
                params![
                    effect_id,
                    encode_enum(EffectStatus::Started)?,
                    encode_enum(EffectStatus::Uncertain)?,
                ],
                |row| row.get(0),
            )
            .optional()?;
        let Some(run_id) = run_id else {
            return Err(StoreError::Incompatible(format!(
                "effect `{effect_id}` is missing or is not uncertain"
            )));
        };
        transaction.execute(
            "UPDATE effects SET status = ?2, error = ?3, completed_at = ?4, confirmed = 0 WHERE effect_id = ?1",
            params![
                effect_id,
                encode_enum(EffectStatus::Failed)?,
                format!("operator reconciled as not applied: {reason}"),
                now.to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "UPDATE tool_calls SET status = 'failed', completed_at = COALESCE(completed_at, ?2) WHERE run_id = ?1 AND effect_id = ?3 AND status IN ('started', 'uncertain')",
            params![run_id, now.to_rfc3339(), effect_id],
        )?;
        append_audit_tx(
            &transaction,
            &run_id,
            "effect.reconciled_not_applied",
            None,
            "operator-reconciliation",
            &serde_json::json!({
                "effectId": effect_id,
                "actor": actor,
                "reason": reason,
                "outcome": "not_applied",
            }),
            now,
        )?;
        transaction.commit()?;
        Ok(())
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

    pub fn create_approval(&self, request: &ApprovalRequest) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO approvals (approval_id, run_id, effect_id, task_id, agent, tool, capability, risk, redacted_input_json, expected_effect, reason, trace_id, status, requested_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13)",
            params![request.approval_id, request.run_id, request.effect_id, request.task_id, request.agent, request.tool, request.capability, request.risk, encode(&request.redacted_input)?, request.expected_effect, request.reason, request.trace_id, request.requested_at.to_rfc3339()],
        )?;
        transaction.execute(
            "UPDATE effects SET status = ?2 WHERE effect_id = ?1",
            params![
                request.effect_id,
                encode_enum(EffectStatus::WaitingForApproval)?
            ],
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
            params![approval_id, status, now.to_rfc3339(), actor, reason],
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
                    redacted_input: decode(&row.7, "approval.input")?,
                    expected_effect: row.8,
                    reason: row.9,
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
                    payload: decode(&row.4, "audit.payload")?,
                    created_at: parse_time(&row.5, "audit.created_at")?,
                })
            })
            .collect()
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
                    state: decode(&row.2, "checkpoint.state")?,
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
                    continuation: decode(&row.3, "provider_session.continuation")?,
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
            params![run_id, sequence, trace_id, encode(event)?, now.to_rfc3339()],
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
                    event: decode(&row.2, "trace.event")?,
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
            "INSERT INTO long_term_memory (namespace, memory_key, value_json, expires_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(namespace, memory_key) DO UPDATE SET value_json = excluded.value_json, expires_at = excluded.expires_at, updated_at = excluded.updated_at",
            params![namespace, key, encode(value)?, expires_at.map(|value| value.to_rfc3339()), now.to_rfc3339()],
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
            params![run_id, task_id, provider, encode(continuation)?, now.to_rfc3339()],
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
                Some(encode(output)?),
                None,
                true,
                "succeeded",
            ),
            Err(error) => (EffectStatus::Failed, None, Some(error), false, "failed"),
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
            params![effect_id, encode_enum(EffectStatus::Uncertain)?, error, now.to_rfc3339(), encode_enum(EffectStatus::Started)?],
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
        let value: Option<String> = self.connection.lock().query_row(
            "SELECT value_json FROM long_term_memory WHERE namespace = ?1 AND memory_key = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
            params![namespace, key, now.to_rfc3339()],
            |row| row.get(0),
        ).optional()?;
        value
            .map(|value| decode(&value, "long_term_memory.value"))
            .transpose()
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
                "tool_calls",
                "trace_events",
                "long_term_memory",
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
            tool_calls: count("tool_calls")?,
            trace_events: count("trace_events")?,
            long_term_memory: count("long_term_memory")?,
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
    Ok(())
}

fn checkpoint_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let run_state: (String, String, Option<String>, bool) = transaction.query_row(
        "SELECT state, working_memory_json, output_json, cancellation_requested FROM runs WHERE run_id = ?1",
        [run_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let mut statement = transaction.prepare(
        "SELECT task_id, state, attempt, output_json, error FROM task_states WHERE run_id = ?1 ORDER BY position",
    )?;
    let tasks: Vec<Value> = statement
        .query_map([run_id], |row| {
            Ok(serde_json::json!({
                "taskId": row.get::<_, String>(0)?,
                "state": row.get::<_, String>(1)?,
                "attempt": row.get::<_, u16>(2)?,
                "output": row.get::<_, Option<String>>(3)?.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
                "error": row.get::<_, Option<String>>(4)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    let state = serde_json::json!({
        "runId": run_id,
        "state": run_state.0,
        "workingMemory": decode::<Value>(&run_state.1, "working_memory")?,
        "output": run_state.2.map(|raw| decode::<Value>(&raw, "output")).transpose()?,
        "cancellationRequested": run_state.3,
        "tasks": tasks,
    });
    let state_json = encode(&state)?;
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

fn append_audit_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    event_type: &str,
    task_id: Option<&str>,
    trace_id: &str,
    payload: &Value,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM audit_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO audit_events (run_id, sequence, event_version, event_type, task_id, trace_id, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![run_id, sequence, AUDIT_EVENT_VERSION, event_type, task_id, trace_id, encode(payload)?, now.to_rfc3339()],
    )?;
    Ok(())
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(StoreError::from)
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
                Path::new("."),
                Utc::now(),
                "trace",
            )
            .expect("create run");
    }

    fn create_version_four_database(path: &Path) {
        let connection = Connection::open(path).expect("raw connection");
        connection.execute_batch(MIGRATION_1).expect("v1 schema");
        connection.execute_batch(MIGRATION_2).expect("v2 schema");
        connection.execute_batch(MIGRATION_3).expect("v3 schema");
        connection.execute_batch(MIGRATION_4).expect("v4 schema");
        connection
            .pragma_update(None, "user_version", 4)
            .expect("v4 marker");
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
        let connection = Connection::open(&path).expect("raw connection");
        connection.execute_batch(MIGRATION_1).expect("v1 schema");
        connection
            .pragma_update(None, "user_version", 1)
            .expect("v1 marker");
        drop(connection);

        let store = SqliteStore::open(&path).expect("upgrade");
        assert_eq!(store.schema_version(), DATABASE_SCHEMA_VERSION);
        assert_eq!(store.stats().expect("stats").long_term_memory, 0);
    }

    #[test]
    fn upgrades_the_pre_repair_schema_and_creates_repair_records() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        create_version_four_database(&path);

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
    fn interrupted_repair_migration_can_restart_cleanly() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("runtime.db");
        create_version_four_database(&path);
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
}
