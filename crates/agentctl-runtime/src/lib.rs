//! Durable deterministic workflow runtime for agentctl.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentctl_core::compiler::{CompiledPlan, PlanPredictability, TaskUse};
use agentctl_core::dsl::{
    API_VERSION, ActionDefinition, ActionKind, ApprovalRequirement, EffectClass, FailureBehavior,
    Idempotency, Risk, ToolDefinition, ToolKind, Workflow,
};
use agentctl_core::effect::{
    ActionResult, ChangeStatus, EffectRecord, EffectRequest, EffectStatus,
};
use agentctl_core::policy::{PolicyContext, PolicyDecision, PolicyEngine, PolicyError, redact};
use agentctl_core::provider::{
    ContentBlock, FinishReason, Message, ModelProvider, ProviderError, ProviderRequest,
    ProviderResponse, Usage,
};
use agentctl_core::state::{RunState, TaskState};
use agentctl_core::template::{EvalContext, TemplateError, evaluate_when, render};
use agentctl_core::tool::{ToolContract, ToolContractError, ToolExecutor};
use agentctl_observability::{NoopTraceSink, SpanKind, TraceEvent, TracePhase, TraceSink};
use agentctl_store::{
    ApprovalRequest, ArtifactRecord, CheckpointRecord, EffectReconciliationRecord,
    EffectReconciliationRequest, LegacyTaskUpgrade, ReconciliationStatus,
    ReusedTaskMaterialization, RunMode, SqliteStore, StoreError, TaskBatchOutcome, TaskBatchResult,
    TaskCompletionMetadata, TaskDisposition, TaskExecutionMetadata, TaskRecord,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

mod process;
pub mod secret;

use process::{ProcessOutputLimits, ProcessRunError, run_bounded_process};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub trait IdGenerator: Send + Sync {
    fn next_id(&self, kind: &str) -> String;
}

#[derive(Debug, Default)]
pub struct UuidGenerator;

impl IdGenerator for UuidGenerator {
    fn next_id(&self, kind: &str) -> String {
        format!("{kind}-{}", Uuid::now_v7())
    }
}

#[async_trait]
pub trait ExternalActionHandler: Send + Sync {
    async fn execute(
        &self,
        kind: ActionKind,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, RuntimeError>;
}

pub trait EffectReconciliationHook: Send + Sync {
    fn validate(
        &self,
        effect: &EffectRecord,
        evidence: &Value,
        result: Option<&Value>,
    ) -> Result<(), String>;
}

const MAX_WORKSPACE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;

pub struct BuiltinToolExecutor {
    contract: ToolContract,
    kind: ToolKind,
    policy: PolicyEngine,
}

impl BuiltinToolExecutor {
    #[must_use]
    pub fn new(id: impl Into<String>, definition: &ToolDefinition, policy: PolicyEngine) -> Self {
        Self {
            contract: ToolContract {
                id: id.into(),
                description: definition.description.clone(),
                input_schema: definition.input_schema.clone(),
                output_schema: definition.output_schema.clone(),
                capability: definition.capability.clone(),
                risk: definition.risk,
                effect_class: definition.effect_class,
                idempotency: definition.idempotency,
                retry_safe: definition.retry_safe,
                timeout_seconds: definition.timeout_seconds,
                secret_requirements: definition.secrets.clone(),
                network_requirements: definition.network.clone(),
                approval: definition.approval,
                observability: Value::Null,
                compensation: definition.compensation.clone(),
            },
            kind: definition.kind,
            policy,
        }
    }
}

#[async_trait]
impl ToolExecutor for BuiltinToolExecutor {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn execute(
        &self,
        input: Value,
        cancellation: &CancellationToken,
    ) -> Result<ActionResult, ToolContractError> {
        if cancellation.is_cancelled() {
            return Err(ToolContractError::Cancelled);
        }
        match self.kind {
            ToolKind::Echo => Ok(ActionResult::unchanged(input)),
            ToolKind::WorkspaceRead => {
                let path = input.get("path").and_then(Value::as_str).ok_or_else(|| {
                    ToolContractError::Execution("workspace read requires string `path`".to_owned())
                })?;
                let resolved = self
                    .policy
                    .resolve_read_path(path)
                    .map_err(|error| ToolContractError::Execution(error.to_string()))?;
                let content = read_bounded_text(&resolved)
                    .await
                    .map_err(|error| ToolContractError::Execution(error.to_string()))?;
                let bytes = content.len();
                Ok(ActionResult::unchanged(serde_json::json!({
                    "path": path,
                    "content": content,
                    "bytes": bytes,
                    "sha256": digest(content.as_bytes()),
                })))
            }
            ToolKind::WorkspaceWrite => {
                let path = input.get("path").and_then(Value::as_str).ok_or_else(|| {
                    ToolContractError::Execution(
                        "workspace write requires string `path`".to_owned(),
                    )
                })?;
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ToolContractError::Execution(
                            "workspace write requires string `content`".to_owned(),
                        )
                    })?;
                let resolved = self
                    .policy
                    .resolve_write_path(path)
                    .map_err(|error| ToolContractError::Execution(error.to_string()))?;
                write_atomic(&resolved, content.as_bytes())
                    .await
                    .map_err(|error| ToolContractError::Execution(error.to_string()))?;
                Ok(ActionResult::changed(serde_json::json!({
                    "path": path,
                    "bytes": content.len(),
                    "sha256": digest(content.as_bytes()),
                })))
            }
        }
    }
}

#[derive(Default)]
pub struct RuntimeRegistry {
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
    tools: BTreeMap<String, Arc<dyn ToolExecutor>>,
    reconciliation_hooks: BTreeMap<String, Arc<dyn EffectReconciliationHook>>,
    external_actions: Option<Arc<dyn ExternalActionHandler>>,
}

impl RuntimeRegistry {
    #[must_use]
    pub fn with_provider(
        mut self,
        name: impl Into<String>,
        provider: Arc<dyn ModelProvider>,
    ) -> Self {
        self.providers.insert(name.into(), provider);
        self
    }

    #[must_use]
    pub fn with_tool(mut self, name: impl Into<String>, tool: Arc<dyn ToolExecutor>) -> Self {
        self.tools.insert(name.into(), tool);
        self
    }

    #[must_use]
    pub fn with_reconciliation_hook(
        mut self,
        operation: impl Into<String>,
        hook: Arc<dyn EffectReconciliationHook>,
    ) -> Self {
        self.reconciliation_hooks.insert(operation.into(), hook);
        self
    }

    #[must_use]
    pub fn with_external_actions(mut self, handler: Arc<dyn ExternalActionHandler>) -> Self {
        self.external_actions = Some(handler);
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    pub check: bool,
    pub diff: bool,
    pub interactive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub run_id: String,
    pub trace_id: String,
    pub state: RunState,
    pub output: Option<Value>,
}

pub const REPAIR_PLAN_VERSION: &str = "agentctl.dev/repair-plan/v1";
pub const TASK_METADATA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedDisposition {
    Reuse,
    Execute,
    Removed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairTaskPlan {
    pub task_id: String,
    pub disposition: PlannedDisposition,
    pub reason: String,
    pub source_state: Option<TaskState>,
    pub source_fingerprint: Option<String>,
    pub target_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairBlock {
    pub task_id: String,
    pub rule: String,
    pub message: String,
    pub source_fingerprint: Option<String>,
    pub target_fingerprint: Option<String>,
    pub suggested_repair_roots: Vec<String>,
    pub full_fork_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshEffectSummary {
    pub provider_tasks: usize,
    pub action_tasks: usize,
    pub declared_effects: usize,
    pub uncertain_source_effects: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPlan {
    pub api_version: String,
    pub compatible: bool,
    pub source_run_id: String,
    pub source_workflow_digest: String,
    pub target_workflow_digest: String,
    pub repair_roots: Vec<String>,
    pub restart_successful: bool,
    pub reused_tasks: Vec<String>,
    pub rerun_tasks: Vec<String>,
    pub new_tasks: Vec<String>,
    pub removed_tasks: Vec<String>,
    pub changed_tasks: Vec<String>,
    pub blocked_reuse: Vec<RepairBlock>,
    pub fresh_effect_summary: FreshEffectSummary,
    pub approval_summary: Vec<String>,
    pub estimated_provider_tasks: usize,
    pub warnings: Vec<String>,
    pub tasks: Vec<RepairTaskPlan>,
    #[serde(skip)]
    materialized_tasks: Vec<ReusedTaskMaterialization>,
    #[serde(skip)]
    reconstructed_memory: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairOutcome {
    pub run_id: String,
    pub source_run_id: String,
    pub trace_id: String,
    pub state: RunState,
    pub reused_tasks: Vec<String>,
    pub executed_tasks: Vec<String>,
    pub artifacts: Vec<ArtifactRecord>,
    pub output: Option<Value>,
}

pub const RETRY_PLAN_VERSION: &str = "agentctl.dev/retry-plan/v1";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPlan {
    pub api_version: String,
    pub compatible: bool,
    pub source_run_id: String,
    pub workflow_digest: String,
    pub failed_only: bool,
    pub retry_roots: Vec<String>,
    pub restart_successful: bool,
    pub reused_tasks: Vec<String>,
    pub rerun_tasks: Vec<String>,
    pub blocked_reuse: Vec<RepairBlock>,
    pub fresh_effect_summary: FreshEffectSummary,
    pub approval_summary: Vec<String>,
    pub estimated_provider_tasks: usize,
    pub warnings: Vec<String>,
    pub tasks: Vec<RepairTaskPlan>,
    #[serde(skip_serializing)]
    repair_plan: RepairPlan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryOutcome {
    pub run_id: String,
    pub source_run_id: String,
    pub trace_id: String,
    pub state: RunState,
    pub failed_only: bool,
    pub retry_roots: Vec<String>,
    pub reused_tasks: Vec<String>,
    pub executed_tasks: Vec<String>,
    pub artifacts: Vec<ArtifactRecord>,
    pub output: Option<Value>,
}

pub const LEGACY_UPGRADE_ANALYSIS_VERSION: &str = "agentctl.dev/legacy-upgrade/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyTaskUpgradeAnalysis {
    pub task_id: String,
    pub state: TaskState,
    pub already_current: bool,
    pub upgradeable: bool,
    pub confidence: String,
    pub reasons: Vec<String>,
    pub provenance: BTreeMap<String, String>,
    pub proposed_metadata: Option<TaskCompletionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRunUpgradeAnalysis {
    pub api_version: String,
    pub run_id: String,
    pub database_schema_version: u32,
    pub terminal: bool,
    pub fully_upgradeable: bool,
    pub already_current: bool,
    pub upgradeable_tasks: Vec<String>,
    pub unavailable_tasks: Vec<String>,
    pub recommended_repair_roots: Vec<String>,
    pub tasks: Vec<LegacyTaskUpgradeAnalysis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRunUpgradeResult {
    pub upgrade_id: String,
    pub run_id: String,
    pub upgraded_tasks: Vec<String>,
    pub analysis_before: LegacyRunUpgradeAnalysis,
    pub analysis_after: LegacyRunUpgradeAnalysis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReconciliationInput {
    pub effect_id: String,
    pub status: ReconciliationStatus,
    pub actor: String,
    pub reason: String,
    pub evidence: Value,
    pub result: Option<Value>,
    pub result_schema: Option<Value>,
    pub compensation_effect_id: Option<String>,
    pub approved: bool,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Tool(#[from] ToolContractError),
    #[error("workflow state is invalid: {0}")]
    InvalidState(String),
    #[error("task `{task}` failed: {message}")]
    Task { task: String, message: String },
    #[error("run `{run_id}` failed in task `{task}` (trace `{trace_id}`): {message}")]
    RunFailed {
        run_id: String,
        trace_id: String,
        task: String,
        message: String,
    },
    #[error(
        "effect `{effect_id}` in run `{run_id}` has an uncertain outcome and will not be repeated automatically (trace `{trace_id}`)"
    )]
    UncertainEffect {
        run_id: String,
        trace_id: String,
        effect_id: String,
    },
    #[error("external effect outcome is uncertain: {0}")]
    ExternalEffectUncertain(String),
    #[error("execution was cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("repair from run `{source_run_id}` is blocked by {count} compatibility rule(s)")]
    RepairBlocked { source_run_id: String, count: usize },
    #[error("retry from run `{source_run_id}` is blocked by {count} compatibility rule(s)")]
    RetryBlocked { source_run_id: String, count: usize },
}

pub struct Runtime {
    store: SqliteStore,
    registry: RuntimeRegistry,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    traces: Arc<dyn TraceSink>,
    base_path: PathBuf,
}

impl Runtime {
    #[must_use]
    pub fn new(store: SqliteStore, base_path: impl Into<PathBuf>) -> Self {
        Self {
            store,
            registry: RuntimeRegistry::default(),
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidGenerator),
            traces: Arc::new(NoopTraceSink),
            base_path: base_path.into(),
        }
    }

    #[must_use]
    pub fn with_registry(mut self, registry: RuntimeRegistry) -> Self {
        self.registry = registry;
        self
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[must_use]
    pub fn with_ids(mut self, ids: Arc<dyn IdGenerator>) -> Self {
        self.ids = ids;
        self
    }

    #[must_use]
    pub fn with_trace_sink(mut self, traces: Arc<dyn TraceSink>) -> Self {
        self.traces = traces;
        self
    }

    pub async fn start(
        &self,
        workflow: &Workflow,
        plan: &CompiledPlan,
        inputs: Value,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        let run_id = self.ids.next_id("run");
        let trace_id = self.ids.next_id("trace");
        let mode = if options.check {
            RunMode::Check
        } else {
            RunMode::Execute
        };
        self.store.create_run(
            &run_id,
            API_VERSION,
            &serde_json::to_value(workflow)?,
            plan,
            &inputs,
            &Value::Object(workflow.spec.memory.working.clone().into_iter().collect()),
            mode,
            None,
            &self.base_path,
            self.clock.now(),
            &trace_id,
        )?;
        self.trace(TraceEvent::new(
            SpanKind::Run,
            TracePhase::Started,
            "run.execute",
            &trace_id,
            &run_id,
            self.clock.now(),
        ))?;
        self.drive(&run_id, &trace_id, options, cancellation).await
    }

    pub async fn resume(
        &self,
        run_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        let trace_id = self.ids.next_id("trace");
        let unresolved = self.store.unresolved_effects(run_id)?;
        if let Some(effect_id) = unresolved.first() {
            return Err(RuntimeError::UncertainEffect {
                run_id: run_id.to_owned(),
                trace_id,
                effect_id: effect_id.clone(),
            });
        }
        let run = self.store.load_run(run_id)?;
        if run.state.is_terminal() {
            return Err(RuntimeError::InvalidState(format!(
                "run `{run_id}` is already terminal ({:?})",
                run.state
            )));
        }
        self.prepare_reconciled_resume(run_id, &trace_id)?;
        let tasks = self.store.list_tasks(run_id)?;
        for task in tasks
            .iter()
            .filter(|task| task.state == TaskState::WaitingForApproval)
        {
            let effect = self
                .store
                .latest_effect_for_task(run_id, &task.task_id)?
                .ok_or_else(|| {
                    RuntimeError::InvalidState(format!(
                        "task `{}` waits for an approval without an effect",
                        task.task_id
                    ))
                })?;
            match effect.status {
                EffectStatus::Requested => self.store.transition_task(
                    run_id,
                    &task.task_id,
                    TaskState::Running,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    &trace_id,
                )?,
                EffectStatus::WaitingForApproval => {
                    return Ok(RunOutcome {
                        run_id: run_id.to_owned(),
                        trace_id,
                        state: RunState::Paused,
                        output: None,
                    });
                }
                EffectStatus::Cancelled => {
                    self.store.transition_task(
                        run_id,
                        &task.task_id,
                        TaskState::Failed,
                        None,
                        Some("approval rejected"),
                        None,
                        self.clock.now(),
                        &trace_id,
                    )?;
                    self.store.update_run_state(
                        run_id,
                        RunState::Failed,
                        None,
                        self.clock.now(),
                        &trace_id,
                    )?;
                    return Ok(RunOutcome {
                        run_id: run_id.to_owned(),
                        trace_id,
                        state: RunState::Failed,
                        output: None,
                    });
                }
                other => {
                    return Err(RuntimeError::InvalidState(format!(
                        "approval task `{}` has effect state {other:?}",
                        task.task_id
                    )));
                }
            }
        }
        if run.state == RunState::Paused {
            self.store.update_run_state(
                run_id,
                RunState::Running,
                None,
                self.clock.now(),
                &trace_id,
            )?;
        }
        self.drive(run_id, &trace_id, options, cancellation).await
    }

    fn prepare_reconciled_resume(&self, run_id: &str, trace_id: &str) -> Result<(), RuntimeError> {
        for effect in self.store.list_effects(run_id)? {
            let Some(reconciliation) = self
                .store
                .latest_effect_reconciliation(&effect.request.id)?
            else {
                continue;
            };
            if !matches!(
                reconciliation.status,
                ReconciliationStatus::NotApplied | ReconciliationStatus::Compensated
            ) {
                continue;
            }
            let Some(task) = self
                .store
                .list_tasks(run_id)?
                .into_iter()
                .find(|task| task.task_id == effect.request.task_id)
            else {
                return Err(RuntimeError::InvalidState(format!(
                    "reconciled effect `{}` has no task row",
                    effect.request.id
                )));
            };
            if task.state == TaskState::WaitingForEffect {
                self.store.transition_task(
                    run_id,
                    &task.task_id,
                    TaskState::Running,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
            }
            if matches!(task.state, TaskState::Running | TaskState::WaitingForEffect) {
                self.store.transition_task(
                    run_id,
                    &task.task_id,
                    TaskState::RetryScheduled,
                    None,
                    Some("operator reconciliation permits a fresh task attempt"),
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                self.store.transition_task(
                    run_id,
                    &task.task_id,
                    TaskState::Ready,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
            }
        }
        Ok(())
    }

    pub async fn replay(&self, source_run_id: &str) -> Result<RunOutcome, RuntimeError> {
        let source = self.store.load_run(source_run_id)?;
        let source_tasks = self.store.list_tasks(source_run_id)?;
        let source_effects = self.store.list_effects(source_run_id)?;
        let source_tool_calls = self.store.tool_calls(source_run_id)?;
        if !source.state.is_terminal() {
            return Err(RuntimeError::InvalidState(format!(
                "source run `{source_run_id}` is not terminal ({:?})",
                source.state
            )));
        }
        let source_tasks = source_tasks
            .into_iter()
            .map(|task| {
                let terminal = match task.state {
                    TaskState::Succeeded => TaskState::Succeeded,
                    TaskState::Failed => TaskState::Failed,
                    TaskState::Skipped => TaskState::Skipped,
                    TaskState::Cancelled => TaskState::Cancelled,
                    _ => {
                        return Err(RuntimeError::InvalidState(format!(
                            "source run has non-terminal task `{}`",
                            task.task_id
                        )));
                    }
                };
                Ok((task, terminal))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (task, _) in &source_tasks {
            verify_artifacts(&self.store, &task.artifact_manifest).map_err(|message| {
                RuntimeError::InvalidState(format!(
                    "recorded replay cannot verify artifacts for task `{}`: {message}",
                    task.task_id
                ))
            })?;
        }
        let replay_id = self.ids.next_id("replay");
        let trace_id = self.ids.next_id("trace");
        self.store.create_run(
            &replay_id,
            &source.workflow_schema_version,
            &source.workflow,
            &source.plan,
            &source.inputs,
            &source.working_memory,
            RunMode::Replay,
            Some(source_run_id),
            Path::new(source.base_path.as_deref().unwrap_or(".")),
            self.clock.now(),
            &trace_id,
        )?;
        self.store.record_replay_effects_reused(
            &replay_id,
            source_run_id,
            &source_effects,
            &source_tool_calls,
            self.clock.now(),
            &trace_id,
        )?;
        for (task, terminal) in source_tasks {
            self.store.transition_task(
                &replay_id,
                &task.task_id,
                TaskState::Ready,
                None,
                None,
                None,
                self.clock.now(),
                &trace_id,
            )?;
            self.store.transition_task(
                &replay_id,
                &task.task_id,
                TaskState::Running,
                None,
                None,
                None,
                self.clock.now(),
                &trace_id,
            )?;
            self.store.transition_task(
                &replay_id,
                &task.task_id,
                terminal,
                task.output.as_ref(),
                task.error.as_deref(),
                None,
                self.clock.now(),
                &trace_id,
            )?;
            self.store.record_replayed_task_metadata(
                &replay_id,
                &task,
                self.clock.now(),
                &trace_id,
            )?;
        }
        self.store.update_run_state(
            &replay_id,
            source.state,
            source.output.as_ref(),
            self.clock.now(),
            &trace_id,
        )?;
        Ok(RunOutcome {
            run_id: replay_id,
            trace_id,
            state: source.state,
            output: source.output,
        })
    }

    pub fn analyze_legacy_run(
        &self,
        run_id: &str,
    ) -> Result<LegacyRunUpgradeAnalysis, RuntimeError> {
        self.analyze_legacy_run_internal(run_id)
            .map(|(analysis, _)| analysis)
    }

    pub fn upgrade_legacy_run(&self, run_id: &str) -> Result<LegacyRunUpgradeResult, RuntimeError> {
        let (analysis_before, mut updates) = self.analyze_legacy_run_internal(run_id)?;
        let source = self.store.load_run(run_id)?;
        let workflow: Workflow = serde_json::from_value(source.workflow)?;
        let base_path = source
            .base_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.base_path.clone());
        let policy = PolicyEngine::new(workflow.spec.policy, &base_path)?;
        let now = self.clock.now();
        for update in &mut updates {
            for artifact in &mut update.metadata.artifact_manifest {
                if artifact.store_path.is_empty() {
                    let resolved = policy.resolve_artifact_path(&artifact.path)?;
                    let ingested = self.store.ingest_artifact(
                        run_id,
                        &update.task_id,
                        &resolved,
                        &artifact.path,
                        MAX_ARTIFACT_BYTES,
                        now,
                    )?;
                    if ingested.digest != artifact.digest
                        || ingested.size_bytes != artifact.size_bytes
                    {
                        return Err(RuntimeError::InvalidState(format!(
                            "legacy artifact `{}` changed after analysis; expected {} bytes and `{}`, found {} bytes and `{}`",
                            artifact.path,
                            artifact.size_bytes,
                            artifact.digest,
                            ingested.size_bytes,
                            ingested.digest
                        )));
                    }
                    *artifact = ingested;
                }
            }
        }
        let upgrade_id = self.ids.next_id("upgrade");
        let trace_id = self.ids.next_id("trace");
        let analysis_value = serde_json::to_value(&analysis_before)?;
        self.store.apply_legacy_run_upgrade(
            &upgrade_id,
            run_id,
            &analysis_value,
            &updates,
            now,
            &trace_id,
        )?;
        let upgraded_tasks = updates
            .iter()
            .map(|update| update.task_id.clone())
            .collect();
        let analysis_after = self.analyze_legacy_run(run_id)?;
        Ok(LegacyRunUpgradeResult {
            upgrade_id,
            run_id: run_id.to_owned(),
            upgraded_tasks,
            analysis_before,
            analysis_after,
        })
    }

    pub fn reconcile_effect(
        &self,
        input: EffectReconciliationInput,
    ) -> Result<EffectReconciliationRecord, RuntimeError> {
        let effect = self.store.load_effect(&input.effect_id)?;
        if input.evidence.is_null() {
            return Err(RuntimeError::InvalidState(
                "effect reconciliation requires evidence".to_owned(),
            ));
        }
        if let (Some(result), Some(schema)) = (&input.result, &input.result_schema) {
            validate_output_contract(schema, result).map_err(|message| {
                RuntimeError::InvalidState(format!(
                    "reconciled result failed the supplied schema: {message}"
                ))
            })?;
        }
        if input.status == ReconciliationStatus::Applied {
            let result = input.result.as_ref().ok_or_else(|| {
                RuntimeError::InvalidState(
                    "an applied reconciliation requires --result-file".to_owned(),
                )
            })?;
            if effect.request.effect_class == EffectClass::Model {
                serde_json::from_value::<ProviderResponse>(result.clone()).map_err(|error| {
                    RuntimeError::InvalidState(format!(
                        "reconciled model result is not a provider response: {error}"
                    ))
                })?;
            }
            if let Some(tool_id) = effect.request.operation.strip_prefix("tool.") {
                let tool = self.registry.tools.get(tool_id).ok_or_else(|| {
                    RuntimeError::InvalidState(format!(
                        "tool-specific reconciliation requires registered tool `{tool_id}`"
                    ))
                })?;
                tool.contract().validate_output(result)?;
            }
        }
        if let Some(hook) = self
            .registry
            .reconciliation_hooks
            .get(&effect.request.operation)
        {
            hook.validate(&effect, &input.evidence, input.result.as_ref())
                .map_err(|message| {
                    RuntimeError::InvalidState(format!(
                        "reconciliation hook for `{}` rejected the evidence: {message}",
                        effect.request.operation
                    ))
                })?;
        }

        let run = self.store.load_run(&effect.request.run_id)?;
        let workflow: Workflow = serde_json::from_value(run.workflow)?;
        let base_path = run
            .base_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.base_path.clone());
        let policy = PolicyEngine::new(workflow.spec.policy, &base_path)?;
        let trace_id = self.ids.next_id("trace");
        let decision = policy.decide(&PolicyContext {
            run_id: effect.request.run_id.clone(),
            trace_id: trace_id.clone(),
            task_id: effect.request.task_id.clone(),
            agent: None,
            tool: effect.request.operation.clone(),
            capability: "effect_reconciliation".to_owned(),
            effect_class: effect.request.effect_class,
            risk: effect.request.risk,
            resource: Some(effect.request.id.clone()),
            provider: (effect.request.effect_class == EffectClass::Model)
                .then(|| effect.request.operation.clone()),
            input: serde_json::json!({
                "status": input.status,
                "hasResult": input.result.is_some(),
                "compensationEffectId": input.compensation_effect_id,
            }),
            interactive: input.approved,
        });
        let authorization = match decision {
            PolicyDecision::Deny { reason } => {
                return Err(RuntimeError::InvalidState(format!(
                    "policy denied effect reconciliation: {reason}"
                )));
            }
            PolicyDecision::RequireApproval { reason } if !input.approved => {
                return Err(RuntimeError::InvalidState(format!(
                    "effect reconciliation requires explicit --approved confirmation: {reason}"
                )));
            }
            PolicyDecision::RequireApproval { reason } => serde_json::json!({
                "kind": "explicit_operator_approval",
                "actor": input.actor,
                "reason": reason,
            }),
            PolicyDecision::Allow { reason } => serde_json::json!({
                "kind": "policy_allow",
                "reason": reason,
                "explicitApproval": input.approved,
            }),
        };
        let request = EffectReconciliationRequest {
            reconciliation_id: self.ids.next_id("reconciliation"),
            effect_id: input.effect_id,
            status: input.status,
            actor: input.actor,
            reason: input.reason,
            evidence: input.evidence,
            result: input.result,
            result_schema: input.result_schema,
            authorization,
            compensation_effect_id: input.compensation_effect_id,
            trace_id,
        };
        self.store
            .reconcile_effect(&request, self.clock.now())
            .map_err(RuntimeError::from)
    }

    fn analyze_legacy_run_internal(
        &self,
        run_id: &str,
    ) -> Result<(LegacyRunUpgradeAnalysis, Vec<LegacyTaskUpgrade>), RuntimeError> {
        let source = self.store.load_run(run_id)?;
        if !source.state.is_terminal() {
            return Err(RuntimeError::InvalidState(format!(
                "legacy run analysis requires a terminal run, found {:?}",
                source.state
            )));
        }
        let workflow: Workflow = serde_json::from_value(source.workflow.clone())?;
        let base_path = source
            .base_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.base_path.clone());
        let policy = PolicyEngine::new(workflow.spec.policy.clone(), &base_path)?;
        let inputs = source
            .inputs
            .as_object()
            .ok_or_else(|| RuntimeError::InvalidState("run inputs must be an object".to_owned()))?;
        let task_records = self
            .store
            .list_tasks(run_id)?
            .into_iter()
            .map(|task| (task.task_id.clone(), task))
            .collect::<BTreeMap<_, _>>();
        let effects = self.store.list_effects(run_id)?;
        let checkpoints = self.store.checkpoints(run_id)?;
        let outputs = task_records
            .iter()
            .filter_map(|(task_id, task)| {
                task.output
                    .as_ref()
                    .map(|output| (task_id.clone(), output.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut tasks = Vec::new();
        let mut updates = Vec::new();
        let mut unavailable = Vec::new();

        for task_id in &source.plan.order {
            let task = task_records.get(task_id).ok_or_else(|| {
                RuntimeError::InvalidState(format!(
                    "compiled legacy task `{task_id}` has no durable task row"
                ))
            })?;
            if task.state != TaskState::Succeeded {
                unavailable.push(task_id.clone());
                tasks.push(LegacyTaskUpgradeAnalysis {
                    task_id: task_id.clone(),
                    state: task.state,
                    already_current: false,
                    upgradeable: false,
                    confidence: "unavailable".to_owned(),
                    reasons: vec![format!(
                        "task is {:?}; only successful task results can be upgraded for reuse",
                        task.state
                    )],
                    provenance: BTreeMap::new(),
                    proposed_metadata: None,
                });
                continue;
            }
            if task.metadata_version == Some(TASK_METADATA_VERSION) {
                tasks.push(LegacyTaskUpgradeAnalysis {
                    task_id: task_id.clone(),
                    state: task.state,
                    already_current: true,
                    upgradeable: false,
                    confidence: "already_current".to_owned(),
                    reasons: Vec::new(),
                    provenance: BTreeMap::new(),
                    proposed_metadata: None,
                });
                continue;
            }
            if task.metadata_version.is_some() {
                unavailable.push(task_id.clone());
                tasks.push(LegacyTaskUpgradeAnalysis {
                    task_id: task_id.clone(),
                    state: task.state,
                    already_current: false,
                    upgradeable: false,
                    confidence: "unsupported".to_owned(),
                    reasons: vec![format!(
                        "task metadata version {:?} is not supported by upgrader version {TASK_METADATA_VERSION}",
                        task.metadata_version
                    )],
                    provenance: BTreeMap::new(),
                    proposed_metadata: None,
                });
                continue;
            }

            let compiled = source.plan.tasks.get(task_id).ok_or_else(|| {
                RuntimeError::InvalidState(format!("compiled task `{task_id}` disappeared"))
            })?;
            let task_effects = effects
                .iter()
                .filter(|effect| effect.request.task_id == *task_id)
                .cloned()
                .collect::<Vec<_>>();
            let (metadata, provenance, reasons) = derive_legacy_task_metadata(
                &workflow,
                compiled,
                task,
                &policy,
                inputs,
                &outputs,
                &task_effects,
                &checkpoints,
            );
            let upgradeable = metadata.is_some();
            if !upgradeable {
                unavailable.push(task_id.clone());
            }
            if let Some(metadata) = &metadata {
                updates.push(LegacyTaskUpgrade {
                    task_id: task_id.clone(),
                    metadata: metadata.clone(),
                    provenance: serde_json::json!({
                        "formatVersion": 1,
                        "confidence": "proven",
                        "fields": provenance,
                    }),
                });
            }
            tasks.push(LegacyTaskUpgradeAnalysis {
                task_id: task_id.clone(),
                state: task.state,
                already_current: false,
                upgradeable,
                confidence: if upgradeable { "proven" } else { "unavailable" }.to_owned(),
                reasons,
                provenance,
                proposed_metadata: metadata,
            });
        }

        let recommended_repair_roots = earliest_safe_repair_roots(&source.plan, &unavailable);
        let upgradeable_tasks = updates
            .iter()
            .map(|update| update.task_id.clone())
            .collect::<Vec<_>>();
        let legacy_successes = tasks
            .iter()
            .filter(|task| task.state == TaskState::Succeeded && !task.already_current)
            .count();
        let unavailable_successes = tasks
            .iter()
            .filter(|task| {
                task.state == TaskState::Succeeded && !task.already_current && !task.upgradeable
            })
            .count();
        let already_current = legacy_successes == 0;
        Ok((
            LegacyRunUpgradeAnalysis {
                api_version: LEGACY_UPGRADE_ANALYSIS_VERSION.to_owned(),
                run_id: run_id.to_owned(),
                database_schema_version: self.store.schema_version(),
                terminal: true,
                fully_upgradeable: unavailable_successes == 0,
                already_current,
                upgradeable_tasks,
                unavailable_tasks: unavailable,
                recommended_repair_roots,
                tasks,
            },
            updates,
        ))
    }

    pub async fn fork(
        &self,
        source_run_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        let source = self.store.load_run(source_run_id)?;
        let workflow: Workflow = serde_json::from_value(source.workflow.clone())?;
        let run_id = self.ids.next_id("fork");
        let trace_id = self.ids.next_id("trace");
        self.store.create_run(
            &run_id,
            &source.workflow_schema_version,
            &source.workflow,
            &source.plan,
            &source.inputs,
            &serde_json::to_value(&workflow.spec.memory.working)?,
            RunMode::Fork,
            Some(source_run_id),
            &self.base_path,
            self.clock.now(),
            &trace_id,
        )?;
        self.drive(&run_id, &trace_id, options, cancellation).await
    }

    pub fn plan_repair(
        &self,
        source_run_id: &str,
        target_workflow: &Workflow,
        target_plan: &CompiledPlan,
        repair_roots: &[String],
        restart_successful: bool,
    ) -> Result<RepairPlan, RuntimeError> {
        let source = self.store.load_run(source_run_id)?;
        if !source.state.is_terminal() {
            return Err(RuntimeError::InvalidState(format!(
                "repair source run `{source_run_id}` is not terminal ({:?})",
                source.state
            )));
        }
        if repair_roots.is_empty() {
            return Err(RuntimeError::InvalidState(
                "repair requires at least one --from task".to_owned(),
            ));
        }
        let source_workflow: Workflow = serde_json::from_value(source.workflow.clone())?;

        let roots = repair_roots.iter().cloned().collect::<BTreeSet<_>>();
        let source_tasks = self
            .store
            .list_tasks(source_run_id)?
            .into_iter()
            .map(|task| (task.task_id.clone(), task))
            .collect::<BTreeMap<_, _>>();
        let source_effects = self.store.list_effects(source_run_id)?;
        let source_ids = source.plan.order.iter().cloned().collect::<BTreeSet<_>>();
        let target_ids = target_plan.order.iter().cloned().collect::<BTreeSet<_>>();
        let new_tasks = target_ids
            .difference(&source_ids)
            .cloned()
            .collect::<Vec<_>>();
        let removed_tasks = source_ids
            .difference(&target_ids)
            .cloned()
            .collect::<Vec<_>>();
        let mut rerun = roots
            .iter()
            .filter(|root| target_plan.tasks.contains_key(*root))
            .cloned()
            .collect::<BTreeSet<_>>();
        loop {
            let mut changed = false;
            for task in target_plan.tasks.values() {
                if task
                    .needs
                    .iter()
                    .any(|dependency| rerun.contains(dependency))
                    && rerun.insert(task.id.clone())
                {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut blocks = Vec::new();
        if source.mode == RunMode::Replay {
            blocks.push(repair_block(
                "$workflow",
                "recorded_replay_has_no_direct_effect_history",
                "a recorded replay cannot be a repair source because its task effects were not dispatched in that run; select the original terminal run".to_owned(),
                None,
                None,
                vec![],
                true,
            ));
        }
        if source_workflow.metadata.name != target_workflow.metadata.name {
            blocks.push(repair_block(
                "$workflow",
                "workflow_identity_mismatch",
                format!(
                    "source workflow `{}` and target workflow `{}` have different identities; use a full fork for unrelated workflows",
                    source_workflow.metadata.name, target_workflow.metadata.name
                ),
                None,
                None,
                vec![],
                true,
            ));
        }
        for root in &roots {
            let Some(_) = target_plan.tasks.get(root) else {
                blocks.push(repair_block(
                    root,
                    "repair_root_missing",
                    format!("repair root `{root}` does not exist in the target workflow"),
                    None,
                    None,
                    vec![],
                    false,
                ));
                continue;
            };
            if source_tasks
                .get(root)
                .is_some_and(|task| task.state == TaskState::Succeeded)
                && !restart_successful
            {
                blocks.push(repair_block(
                    root,
                    "successful_root_requires_acknowledgement",
                    format!(
                        "task `{root}` succeeded in the source run; use --restart-successful to execute it again"
                    ),
                    source_tasks
                        .get(root)
                        .and_then(|task| task.definition_fingerprint.clone()),
                    None,
                    vec![root.clone()],
                    false,
                ));
            }
            if source_tasks.get(root).is_some_and(|task| {
                task.state == TaskState::Succeeded
                    && matches!(
                        task.disposition,
                        TaskDisposition::Reused | TaskDisposition::Recorded
                    )
            }) {
                blocks.push(repair_block(
                    root,
                    "indirect_effect_history",
                    format!(
                        "task `{root}` was materialized from another run and cannot be restarted without its direct effect history; select the originating run or perform a full fork"
                    ),
                    source_tasks
                        .get(root)
                        .and_then(|task| task.definition_fingerprint.clone()),
                    None,
                    vec![],
                    true,
                ));
            }
        }

        for effect in source_effects
            .iter()
            .filter(|effect| rerun.contains(&effect.request.task_id))
        {
            let reconciliation = self
                .store
                .latest_effect_reconciliation(&effect.request.id)?;
            if repair_effect_is_unsafe(effect, reconciliation.as_ref()) {
                let task_id = effect.request.task_id.clone();
                blocks.push(repair_block(
                    &task_id,
                    "unreconciled_effect",
                    format!(
                        "effect `{}` ({:?}, {:?}, {:?}) may be duplicated by repair; reconcile it before retrying",
                        effect.request.id,
                        effect.request.effect_class,
                        effect.request.idempotency,
                        effect.status
                    ),
                    None,
                    None,
                    vec![task_id.clone()],
                    false,
                ));
            }
        }

        let target_policy =
            PolicyEngine::new(target_workflow.spec.policy.clone(), &self.base_path)?;
        let mut memory = Value::Object(
            target_workflow
                .spec
                .memory
                .working
                .clone()
                .into_iter()
                .collect(),
        );
        let inputs = source
            .inputs
            .as_object()
            .ok_or_else(|| RuntimeError::InvalidState("run inputs must be an object".to_owned()))?;
        let mut outputs = BTreeMap::new();
        let mut reused = Vec::new();
        let mut task_plans = Vec::new();
        let mut changed_tasks = BTreeSet::new();
        let mut blocked_task_ids = BTreeSet::new();

        for task_id in &target_plan.order {
            let target_task = target_plan.tasks.get(task_id).ok_or_else(|| {
                RuntimeError::InvalidState(format!("target task `{task_id}` is missing"))
            })?;
            let target_fingerprint =
                task_definition_fingerprint(target_workflow, target_task, &target_policy, None)?;
            let source_task = source_tasks.get(task_id);
            if source_task
                .and_then(|task| task.definition_fingerprint.as_deref())
                .is_some_and(|fingerprint| fingerprint != target_fingerprint)
            {
                changed_tasks.insert(task_id.clone());
            }

            if rerun.contains(task_id) {
                task_plans.push(RepairTaskPlan {
                    task_id: task_id.clone(),
                    disposition: PlannedDisposition::Execute,
                    reason: if roots.contains(task_id) {
                        "selected repair root".to_owned()
                    } else {
                        "transitive descendant of a repair root".to_owned()
                    },
                    source_state: source_task.map(|task| task.state),
                    source_fingerprint: source_task
                        .and_then(|task| task.definition_fingerprint.clone()),
                    target_fingerprint: Some(target_fingerprint),
                });
                continue;
            }

            let Some(source_task) = source_task else {
                let block = repair_block(
                    task_id,
                    "new_task_outside_repair_closure",
                    format!(
                        "new target task `{task_id}` is outside the repair closure and cannot be reused"
                    ),
                    None,
                    Some(target_fingerprint.clone()),
                    vec![task_id.clone()],
                    false,
                );
                blocks.push(block);
                blocked_task_ids.insert(task_id.clone());
                task_plans.push(blocked_task_plan(task_id, None, None, target_fingerprint));
                continue;
            };
            let source_fingerprint = source_task.definition_fingerprint.clone();
            let blocked = |rule: &str,
                           message: String,
                           full_fork_required: bool,
                           blocks: &mut Vec<RepairBlock>,
                           blocked_task_ids: &mut BTreeSet<String>| {
                blocks.push(repair_block(
                    task_id,
                    rule,
                    message,
                    source_fingerprint.clone(),
                    Some(target_fingerprint.clone()),
                    vec![task_id.clone()],
                    full_fork_required,
                ));
                blocked_task_ids.insert(task_id.clone());
            };

            if source_task.state != TaskState::Succeeded {
                blocked(
                    "source_task_not_successful",
                    format!(
                        "task `{task_id}` is {:?} in the source run and has no reusable successful result",
                        source_task.state
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            if source_task.metadata_version != Some(TASK_METADATA_VERSION) {
                blocked(
                    "legacy_task_metadata",
                    format!(
                        "task `{task_id}` predates repair metadata version {TASK_METADATA_VERSION}; run `agentctl runs analyze`/`runs upgrade`, or choose the reported safe repair root"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            let contract = task_output_schema(target_workflow, target_task)
                .unwrap_or_else(|| serde_json::json!({}));
            let contract_fingerprint = versioned_json_digest(&contract)?;
            if source_task.output_contract_fingerprint.as_deref() != Some(&contract_fingerprint) {
                blocked(
                    "output_contract_mismatch",
                    format!("output contract for task `{task_id}` changed"),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            let source_needs = source
                .plan
                .tasks
                .get(task_id)
                .map(|task| task.needs.as_slice())
                .unwrap_or_default();
            if source_needs != target_task.needs {
                blocked(
                    "dependency_set_mismatch",
                    format!(
                        "task `{task_id}` has a different dependency set in the target workflow"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            if source_task.definition_fingerprint.as_deref() != Some(&target_fingerprint) {
                blocked(
                    "definition_fingerprint_mismatch",
                    format!(
                        "task `{task_id}` changed outside the repair closure; choose it as an earlier repair root"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            match unresolved_reuse_effects(&self.store, source_task, &source_effects) {
                Ok(effect_ids) if !effect_ids.is_empty() => {
                    blocked(
                        "unresolved_reused_effect",
                        format!(
                            "task `{task_id}` has unresolved source effect(s) {}; reconcile external reality before reusing its result",
                            effect_ids.join(", ")
                        ),
                        false,
                        &mut blocks,
                        &mut blocked_task_ids,
                    );
                    task_plans.push(blocked_task_plan(
                        task_id,
                        Some(source_task.state),
                        source_fingerprint,
                        target_fingerprint,
                    ));
                    continue;
                }
                Err(message) => {
                    blocked(
                        "reuse_effect_provenance",
                        format!("task `{task_id}` has invalid reused-effect provenance: {message}"),
                        false,
                        &mut blocks,
                        &mut blocked_task_ids,
                    );
                    task_plans.push(blocked_task_plan(
                        task_id,
                        Some(source_task.state),
                        source_fingerprint,
                        target_fingerprint,
                    ));
                    continue;
                }
                Ok(_) => {}
            }
            if matches!(target_task.uses, TaskUse::Agent(_))
                && task_output_schema(target_workflow, target_task).is_none()
                && target_plan
                    .tasks
                    .values()
                    .any(|candidate| candidate.needs.contains(task_id))
            {
                blocked(
                    "missing_output_contract",
                    format!(
                        "reused agent task `{task_id}` feeds another task but has no outputSchema or structuredOutput contract"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            let output = source_task.output.as_ref().ok_or_else(|| {
                RuntimeError::InvalidState(format!(
                    "successful source task `{task_id}` has no output"
                ))
            })?;
            let output_digest = versioned_json_digest(output)?;
            if source_task.output_digest.as_deref() != Some(&output_digest) {
                blocked(
                    "output_digest_mismatch",
                    format!("stored output for task `{task_id}` is corrupt or was modified"),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            if let Err(message) = validate_output_contract(&contract, output) {
                blocked(
                    "output_contract_validation",
                    format!("stored output for task `{task_id}` is invalid: {message}"),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            if let Err(message) = verify_artifacts(&self.store, &source_task.artifact_manifest) {
                blocked(
                    "artifact_integrity",
                    format!("artifact verification failed for task `{task_id}`: {message}"),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            let input_digest = resolved_input_digest(inputs, &memory, &outputs, target_task)?;
            if source_task.input_digest.as_deref() != Some(&input_digest) {
                blocked(
                    "resolved_input_digest_mismatch",
                    format!(
                        "resolved inputs or boundary memory for task `{task_id}` differ from the source run"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            let Some(state_delta) = source_task.state_delta.as_ref() else {
                blocked(
                    "state_delta_missing",
                    format!(
                        "successful source task `{task_id}` has no committed state delta; choose it as an earlier repair root"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            };
            let state_delta_digest = versioned_json_digest(state_delta)?;
            if source_task.state_delta_digest.as_deref() != Some(&state_delta_digest) {
                blocked(
                    "state_delta_digest_mismatch",
                    format!("state delta for task `{task_id}` is corrupt or unsupported"),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            if let Err(error) = apply_state_delta(&mut memory, state_delta) {
                blocked(
                    "state_delta_invalid",
                    format!(
                        "state delta for task `{task_id}` cannot reconstruct boundary memory: {error}"
                    ),
                    false,
                    &mut blocks,
                    &mut blocked_task_ids,
                );
                task_plans.push(blocked_task_plan(
                    task_id,
                    Some(source_task.state),
                    source_fingerprint,
                    target_fingerprint,
                ));
                continue;
            }
            outputs.insert(task_id.clone(), output.clone());
            let source_effect_summary = if source_task.disposition == TaskDisposition::Reused {
                source_task
                    .reuse_decision
                    .as_ref()
                    .and_then(|decision| decision.get("sourceEffects"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]))
            } else {
                Value::Array(
                    source_effects
                        .iter()
                        .filter(|effect| effect.request.task_id == *task_id)
                        .map(|effect| {
                            serde_json::json!({
                                "effectId": effect.request.id,
                                "effectClass": effect.request.effect_class,
                                "idempotency": effect.request.idempotency,
                                "status": effect.status,
                                "confirmed": effect.confirmed,
                            })
                        })
                        .collect(),
                )
            };
            let reuse_decision = serde_json::json!({
                "formatVersion": 1,
                "reason": "all compatibility checks passed",
                "checks": [
                    "source_succeeded",
                    "outside_rerun_closure",
                    "definition_fingerprint",
                    "resolved_input_digest",
                    "dependency_set",
                    "output_contract",
                    "output_digest",
                    "artifact_integrity",
                    "state_delta_digest",
                    "effect_certainty"
                ],
                "sourceWorkflowDigest": source.workflow_digest,
                "targetWorkflowDigest": target_plan.workflow_digest,
                "sourceEffects": source_effect_summary,
            });
            reused.push(ReusedTaskMaterialization {
                task_id: task_id.clone(),
                source_run_id: source_run_id.to_owned(),
                source_task_id: task_id.clone(),
                source_attempt: source_task.attempt,
                output: output.clone(),
                metadata: TaskCompletionMetadata {
                    execution: TaskExecutionMetadata {
                        metadata_version: TASK_METADATA_VERSION,
                        definition_fingerprint: target_fingerprint.clone(),
                        input_digest,
                        output_contract_fingerprint: contract_fingerprint,
                    },
                    output_digest,
                    state_delta: state_delta.clone(),
                    state_delta_digest,
                    artifact_manifest: source_task.artifact_manifest.clone(),
                },
                reuse_decision,
            });
            task_plans.push(RepairTaskPlan {
                task_id: task_id.clone(),
                disposition: PlannedDisposition::Reuse,
                reason: "successful compatible source result".to_owned(),
                source_state: Some(source_task.state),
                source_fingerprint,
                target_fingerprint: Some(target_fingerprint),
            });
        }

        for task_id in target_plan
            .order
            .iter()
            .filter(|task_id| rerun.contains(*task_id))
        {
            let target_task = target_plan.tasks.get(task_id).ok_or_else(|| {
                RuntimeError::InvalidState(format!("target task `{task_id}` is missing"))
            })?;
            if !target_task
                .needs
                .iter()
                .all(|dependency| outputs.contains_key(dependency))
            {
                continue;
            }
            if let Err(error) = resolved_input_digest(inputs, &memory, &outputs, target_task) {
                let source_fingerprint = source_tasks
                    .get(task_id)
                    .and_then(|task| task.definition_fingerprint.clone());
                let target_fingerprint = task_plans
                    .iter()
                    .find(|task| &task.task_id == task_id)
                    .and_then(|task| task.target_fingerprint.clone());
                blocks.push(repair_block(
                    task_id,
                    "target_input_resolution",
                    format!(
                        "target task `{task_id}` cannot consume the reconstructed boundary state: {error}"
                    ),
                    source_fingerprint,
                    target_fingerprint,
                    vec![task_id.clone()],
                    false,
                ));
                blocked_task_ids.insert(task_id.clone());
            }
        }

        for task_id in &removed_tasks {
            task_plans.push(RepairTaskPlan {
                task_id: task_id.clone(),
                disposition: PlannedDisposition::Removed,
                reason: "task is absent from the target workflow".to_owned(),
                source_state: source_tasks.get(task_id).map(|task| task.state),
                source_fingerprint: source_tasks
                    .get(task_id)
                    .and_then(|task| task.definition_fingerprint.clone()),
                target_fingerprint: None,
            });
        }
        for task_plan in &mut task_plans {
            if blocked_task_ids.contains(&task_plan.task_id) {
                task_plan.disposition = PlannedDisposition::Blocked;
            }
        }

        let rerun_tasks = target_plan
            .order
            .iter()
            .filter(|task| rerun.contains(*task))
            .cloned()
            .collect::<Vec<_>>();
        let reused_tasks = target_plan
            .order
            .iter()
            .filter(|task| {
                reused
                    .iter()
                    .any(|materialized| materialized.task_id.as_str() == task.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        let provider_tasks = rerun_tasks
            .iter()
            .filter(|task| {
                target_plan
                    .tasks
                    .get(*task)
                    .is_some_and(|task| matches!(task.uses, TaskUse::Agent(_)))
            })
            .count();
        let action_tasks = rerun_tasks.len().saturating_sub(provider_tasks);
        let declared_effects = target_plan
            .requirements
            .effects
            .iter()
            .filter(|effect| rerun.contains(&effect.task))
            .count();
        let approval_summary = target_plan
            .requirements
            .effects
            .iter()
            .filter(|effect| rerun.contains(&effect.task) && effect.approval_possible)
            .map(|effect| format!("{}:{}", effect.task, effect.operation))
            .collect::<Vec<_>>();
        let mut uncertain_source_effects = 0;
        for effect in &source_effects {
            if rerun.contains(&effect.request.task_id)
                && matches!(
                    effect.status,
                    EffectStatus::Started | EffectStatus::Uncertain
                )
                && self
                    .store
                    .latest_effect_reconciliation(&effect.request.id)?
                    .is_none()
            {
                uncertain_source_effects += 1;
            }
        }
        let compatible = blocks.is_empty();
        Ok(RepairPlan {
            api_version: REPAIR_PLAN_VERSION.to_owned(),
            compatible,
            source_run_id: source_run_id.to_owned(),
            source_workflow_digest: source.workflow_digest,
            target_workflow_digest: target_plan.workflow_digest.clone(),
            repair_roots: roots.into_iter().collect(),
            restart_successful,
            reused_tasks,
            rerun_tasks,
            new_tasks,
            removed_tasks,
            changed_tasks: changed_tasks.into_iter().collect(),
            blocked_reuse: blocks,
            fresh_effect_summary: FreshEffectSummary {
                provider_tasks,
                action_tasks,
                declared_effects,
                uncertain_source_effects,
            },
            approval_summary,
            estimated_provider_tasks: provider_tasks,
            warnings: vec![
                "repair roots and descendants execute with fresh effects".to_owned(),
                "reused tasks dispatch no providers, tools, processes, or network calls".to_owned(),
            ],
            tasks: task_plans,
            materialized_tasks: reused,
            reconstructed_memory: memory,
        })
    }

    pub fn plan_retry(
        &self,
        source_run_id: &str,
        workflow: &Workflow,
        compiled: &CompiledPlan,
        selected_roots: &[String],
        failed_only: bool,
        restart_successful: bool,
    ) -> Result<RetryPlan, RuntimeError> {
        if failed_only && !selected_roots.is_empty() {
            return Err(RuntimeError::InvalidState(
                "retry accepts either --failed or --from, not both".to_owned(),
            ));
        }
        if !failed_only && selected_roots.is_empty() {
            return Err(RuntimeError::InvalidState(
                "retry requires --failed or at least one --from task".to_owned(),
            ));
        }
        let source = self.store.load_run(source_run_id)?;
        if !source.state.is_terminal() {
            return Err(RuntimeError::InvalidState(format!(
                "retry source run `{source_run_id}` is not terminal ({:?})",
                source.state
            )));
        }
        let roots = if failed_only {
            let roots = self
                .store
                .list_tasks(source_run_id)?
                .into_iter()
                .filter(|task| task.state == TaskState::Failed)
                .map(|task| task.task_id)
                .collect::<Vec<_>>();
            if roots.is_empty() {
                return Err(RuntimeError::InvalidState(format!(
                    "source run `{source_run_id}` has no failed tasks"
                )));
            }
            roots
        } else {
            selected_roots.to_vec()
        };
        let mut repair_plan = self.plan_repair(
            source_run_id,
            workflow,
            compiled,
            &roots,
            restart_successful,
        )?;
        if source.workflow_digest != compiled.workflow_digest {
            repair_plan.blocked_reuse.push(repair_block(
                "$workflow",
                "retry_workflow_definition_mismatch",
                format!(
                    "retry requires workflow digest `{}`, found `{}`; use repair for a changed workflow",
                    source.workflow_digest, compiled.workflow_digest
                ),
                Some(source.workflow_digest.clone()),
                Some(compiled.workflow_digest.clone()),
                Vec::new(),
                false,
            ));
            repair_plan.compatible = false;
        }
        let warnings = vec![
            "retry requires the identical workflow definition and preserves the terminal source"
                .to_owned(),
            "retry roots and descendants start fresh task and effect attempts".to_owned(),
            "compatible successful tasks are materialized without dispatch".to_owned(),
        ];
        Ok(RetryPlan {
            api_version: RETRY_PLAN_VERSION.to_owned(),
            compatible: repair_plan.compatible,
            source_run_id: repair_plan.source_run_id.clone(),
            workflow_digest: compiled.workflow_digest.clone(),
            failed_only,
            retry_roots: repair_plan.repair_roots.clone(),
            restart_successful,
            reused_tasks: repair_plan.reused_tasks.clone(),
            rerun_tasks: repair_plan.rerun_tasks.clone(),
            blocked_reuse: repair_plan.blocked_reuse.clone(),
            fresh_effect_summary: repair_plan.fresh_effect_summary.clone(),
            approval_summary: repair_plan.approval_summary.clone(),
            estimated_provider_tasks: repair_plan.estimated_provider_tasks,
            warnings,
            tasks: repair_plan.tasks.clone(),
            repair_plan,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn retry(
        &self,
        workflow: &Workflow,
        compiled: &CompiledPlan,
        plan: RetryPlan,
        reason: Option<&str>,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RetryOutcome, RuntimeError> {
        if !plan.compatible {
            return Err(RuntimeError::RetryBlocked {
                source_run_id: plan.source_run_id,
                count: plan.blocked_reuse.len(),
            });
        }
        if compiled.workflow_digest != plan.workflow_digest {
            return Err(RuntimeError::InvalidState(
                "workflow changed after retry planning; create a new retry plan".to_owned(),
            ));
        }
        let selected_roots = if plan.failed_only {
            Vec::new()
        } else {
            plan.retry_roots.clone()
        };
        let plan = self.plan_retry(
            &plan.source_run_id,
            workflow,
            compiled,
            &selected_roots,
            plan.failed_only,
            plan.restart_successful,
        )?;
        if !plan.compatible {
            return Err(RuntimeError::RetryBlocked {
                source_run_id: plan.source_run_id,
                count: plan.blocked_reuse.len(),
            });
        }
        let source = self.store.load_run(&plan.source_run_id)?;
        let run_id = self.ids.next_id("retry");
        let trace_id = self.ids.next_id("trace");
        self.store.create_retry_run(
            &run_id,
            &plan.source_run_id,
            &source.workflow_digest,
            API_VERSION,
            &serde_json::to_value(workflow)?,
            compiled,
            &source.inputs,
            &plan.repair_plan.reconstructed_memory,
            &plan.retry_roots,
            plan.failed_only,
            reason,
            &plan.repair_plan.materialized_tasks,
            &serde_json::to_value(&plan.tasks)?,
            &self.base_path,
            self.clock.now(),
            &trace_id,
        )?;
        self.trace(
            TraceEvent::new(
                SpanKind::Run,
                TracePhase::Started,
                "run.retry",
                &trace_id,
                &run_id,
                self.clock.now(),
            )
            .attributes(
                serde_json::json!({
                    "sourceRunId": plan.source_run_id,
                    "failedOnly": plan.failed_only,
                    "retryRoots": plan.retry_roots,
                    "reusedTasks": plan.reused_tasks,
                    "executedTasks": plan.rerun_tasks,
                }),
                &[],
            ),
        )?;
        for task in &plan.repair_plan.materialized_tasks {
            self.trace(
                TraceEvent::new(
                    SpanKind::Task,
                    TracePhase::Completed,
                    "task.reused",
                    &trace_id,
                    &run_id,
                    self.clock.now(),
                )
                .task(&task.task_id)
                .attributes(
                    serde_json::json!({
                        "disposition": "reused",
                        "sourceRunId": task.source_run_id,
                        "sourceTaskId": task.source_task_id,
                        "sourceAttempt": task.source_attempt,
                        "outputDigest": task.metadata.output_digest,
                    }),
                    &[],
                ),
            )?;
        }
        let outcome = self
            .drive(&run_id, &trace_id, options, cancellation)
            .await?;
        let artifacts = self
            .store
            .list_tasks(&outcome.run_id)?
            .into_iter()
            .flat_map(|task| task.artifact_manifest)
            .map(|artifact| (artifact.path.clone(), artifact))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
        Ok(RetryOutcome {
            run_id: outcome.run_id,
            source_run_id: source.run_id,
            trace_id: outcome.trace_id,
            state: outcome.state,
            failed_only: plan.failed_only,
            retry_roots: plan.retry_roots,
            reused_tasks: plan.reused_tasks,
            executed_tasks: plan.rerun_tasks,
            artifacts,
            output: outcome.output,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn repair(
        &self,
        target_workflow: &Workflow,
        target_plan: &CompiledPlan,
        plan: RepairPlan,
        reason: Option<&str>,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RepairOutcome, RuntimeError> {
        if !plan.compatible {
            return Err(RuntimeError::RepairBlocked {
                source_run_id: plan.source_run_id,
                count: plan.blocked_reuse.len(),
            });
        }
        if target_plan.workflow_digest != plan.target_workflow_digest {
            return Err(RuntimeError::InvalidState(
                "target workflow changed after repair planning; create a new repair plan"
                    .to_owned(),
            ));
        }
        let plan = self.plan_repair(
            &plan.source_run_id,
            target_workflow,
            target_plan,
            &plan.repair_roots,
            plan.restart_successful,
        )?;
        if !plan.compatible {
            return Err(RuntimeError::RepairBlocked {
                source_run_id: plan.source_run_id,
                count: plan.blocked_reuse.len(),
            });
        }
        let source = self.store.load_run(&plan.source_run_id)?;
        let run_id = self.ids.next_id("repair");
        let trace_id = self.ids.next_id("trace");
        self.store.create_repair_run(
            &run_id,
            &plan.source_run_id,
            &plan.source_workflow_digest,
            API_VERSION,
            &serde_json::to_value(target_workflow)?,
            target_plan,
            &source.inputs,
            &plan.reconstructed_memory,
            &plan.repair_roots,
            reason,
            &plan.materialized_tasks,
            &serde_json::to_value(&plan.tasks)?,
            &self.base_path,
            self.clock.now(),
            &trace_id,
        )?;
        self.trace(
            TraceEvent::new(
                SpanKind::Run,
                TracePhase::Started,
                "run.repair",
                &trace_id,
                &run_id,
                self.clock.now(),
            )
            .attributes(
                serde_json::json!({
                    "sourceRunId": plan.source_run_id,
                    "repairRoots": plan.repair_roots,
                    "reusedTasks": plan.reused_tasks,
                    "executedTasks": plan.rerun_tasks,
                }),
                &[],
            ),
        )?;
        for task in &plan.materialized_tasks {
            self.trace(
                TraceEvent::new(
                    SpanKind::Task,
                    TracePhase::Completed,
                    "task.reused",
                    &trace_id,
                    &run_id,
                    self.clock.now(),
                )
                .task(&task.task_id)
                .attributes(
                    serde_json::json!({
                        "disposition": "reused",
                        "sourceRunId": task.source_run_id,
                        "sourceTaskId": task.source_task_id,
                        "sourceAttempt": task.source_attempt,
                        "outputDigest": task.metadata.output_digest,
                    }),
                    &[],
                ),
            )?;
        }
        let outcome = self
            .drive(&run_id, &trace_id, options, cancellation)
            .await?;
        let artifacts = self
            .store
            .list_tasks(&outcome.run_id)?
            .into_iter()
            .flat_map(|task| task.artifact_manifest)
            .map(|artifact| (artifact.path.clone(), artifact))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
        Ok(RepairOutcome {
            run_id: outcome.run_id,
            source_run_id: source.run_id,
            trace_id: outcome.trace_id,
            state: outcome.state,
            reused_tasks: plan.reused_tasks,
            executed_tasks: plan.rerun_tasks,
            artifacts,
            output: outcome.output,
        })
    }

    async fn drive(
        &self,
        run_id: &str,
        trace_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        let run = self.store.load_run(run_id)?;
        let workflow: Workflow = serde_json::from_value(run.workflow)?;
        if workflow.spec.runtime.max_concurrency == 1 {
            self.drive_sequential(run_id, trace_id, options, cancellation)
                .await
        } else {
            self.drive_parallel(run_id, trace_id, options, cancellation)
                .await
        }
    }

    async fn drive_parallel(
        &self,
        run_id: &str,
        trace_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        loop {
            let run = self.store.load_run(run_id)?;
            if run.cancellation_requested || cancellation.is_cancelled() {
                self.cancel_non_terminal(run_id, trace_id)?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state: RunState::Cancelled,
                    output: None,
                });
            }
            let workflow: Workflow = serde_json::from_value(run.workflow.clone())?;
            let policy = PolicyEngine::new(workflow.spec.policy.clone(), &self.base_path)?;
            let tasks = self.store.list_tasks(run_id)?;
            if tasks.iter().all(|task| task.state.is_terminal()) {
                let failed = tasks.iter().any(|task| task.state == TaskState::Failed);
                let state = if failed {
                    RunState::Failed
                } else if tasks.iter().any(|task| task.state == TaskState::Cancelled) {
                    RunState::Cancelled
                } else {
                    RunState::Succeeded
                };
                let output = collect_outputs(&run, &tasks, &workflow.spec.outputs)?;
                self.store.update_run_state(
                    run_id,
                    state,
                    Some(&output),
                    self.clock.now(),
                    trace_id,
                )?;
                self.trace(
                    TraceEvent::new(
                        SpanKind::Run,
                        if failed {
                            TracePhase::Failed
                        } else {
                            TracePhase::Completed
                        },
                        "run.execute",
                        trace_id,
                        run_id,
                        self.clock.now(),
                    )
                    .attributes(serde_json::json!({"state": state}), &[]),
                )?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state,
                    output: Some(output),
                });
            }

            if let Some(failed) = tasks.iter().find(|record| {
                record.state == TaskState::Failed
                    && run
                        .plan
                        .tasks
                        .get(&record.task_id)
                        .is_some_and(|task| task.failure == FailureBehavior::Stop)
            }) {
                if run.state == RunState::Running {
                    self.store.update_run_state(
                        run_id,
                        RunState::Failed,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                }
                return Err(RuntimeError::RunFailed {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    task: failed.task_id.clone(),
                    message: failed
                        .error
                        .clone()
                        .unwrap_or_else(|| "task failed".to_owned()),
                });
            }

            let context = context_for(&run, &tasks)?;
            let mut prepared_pending = false;
            for task_id in &run.plan.order {
                let Some(record) = tasks.iter().find(|record| &record.task_id == task_id) else {
                    return Err(RuntimeError::InvalidState(format!(
                        "task `{task_id}` missing"
                    )));
                };
                if record.state != TaskState::Pending {
                    continue;
                }
                let task = &run.plan.tasks[task_id];
                let dependencies = task
                    .needs
                    .iter()
                    .filter_map(|needed| {
                        tasks.iter().find(|candidate| &candidate.task_id == needed)
                    })
                    .collect::<Vec<_>>();
                if !dependencies
                    .iter()
                    .all(|dependency| dependency.state.is_terminal())
                {
                    continue;
                }
                if dependencies.iter().any(|dependency| {
                    matches!(
                        dependency.state,
                        TaskState::Failed | TaskState::Cancelled | TaskState::Skipped
                    )
                }) {
                    self.store.transition_task(
                        run_id,
                        task_id,
                        TaskState::Skipped,
                        None,
                        Some("dependency did not succeed"),
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                    prepared_pending = true;
                    continue;
                }
                if let Some(condition) = &task.when
                    && !evaluate_when(condition, &context)?
                {
                    self.store.transition_task(
                        run_id,
                        task_id,
                        TaskState::Skipped,
                        Some(&serde_json::json!({"reason": "when condition was false"})),
                        None,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                    prepared_pending = true;
                    continue;
                }
                self.store.transition_task(
                    run_id,
                    task_id,
                    TaskState::Ready,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                prepared_pending = true;
            }
            if prepared_pending {
                continue;
            }

            let batch = ready_task_batch(&run.plan, &tasks, workflow.spec.runtime.max_concurrency);
            if batch.is_empty() {
                let retrying = tasks
                    .iter()
                    .filter(|task| task.state == TaskState::RetryScheduled)
                    .collect::<Vec<_>>();
                if !retrying.is_empty() {
                    let backoff_ms = retrying
                        .iter()
                        .filter_map(|record| run.plan.tasks.get(&record.task_id))
                        .map(|task| task.retry.backoff_ms)
                        .max()
                        .unwrap_or_default();
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                        () = cancellation.cancelled() => {
                            self.cancel_non_terminal(run_id, trace_id)?;
                            return Ok(RunOutcome {
                                run_id: run_id.to_owned(),
                                trace_id: trace_id.to_owned(),
                                state: RunState::Cancelled,
                                output: None,
                            });
                        },
                    }
                    for record in retrying {
                        self.store.transition_task(
                            run_id,
                            &record.task_id,
                            TaskState::Ready,
                            None,
                            None,
                            None,
                            self.clock.now(),
                            trace_id,
                        )?;
                    }
                    continue;
                }
                if tasks.iter().any(|task| {
                    matches!(
                        task.state,
                        TaskState::WaitingForApproval | TaskState::WaitingForEffect
                    )
                }) {
                    if run.state == RunState::Running {
                        self.store.update_run_state(
                            run_id,
                            RunState::Paused,
                            None,
                            self.clock.now(),
                            trace_id,
                        )?;
                    }
                    return Ok(RunOutcome {
                        run_id: run_id.to_owned(),
                        trace_id: trace_id.to_owned(),
                        state: RunState::Paused,
                        output: None,
                    });
                }
                return Err(RuntimeError::InvalidState(
                    "no runnable task exists and the run is not terminal".to_owned(),
                ));
            }

            for task in &batch {
                let record = tasks
                    .iter()
                    .find(|record| record.task_id == task.id)
                    .ok_or_else(|| {
                        RuntimeError::InvalidState(format!("task `{}` missing", task.id))
                    })?;
                if record.state == TaskState::Ready {
                    self.store.transition_task(
                        run_id,
                        &task.id,
                        TaskState::Running,
                        None,
                        None,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                    self.trace(
                        TraceEvent::new(
                            SpanKind::Task,
                            TracePhase::Started,
                            "task.execute",
                            trace_id,
                            run_id,
                            self.clock.now(),
                        )
                        .task(&task.id)
                        .attributes(
                            serde_json::json!({
                                "scheduler": "stable_parallel_batch",
                                "maxConcurrency": workflow.spec.runtime.max_concurrency,
                            }),
                            &[],
                        ),
                    )?;
                }
            }

            let running = self.store.list_tasks(run_id)?;
            let task_outputs = running
                .iter()
                .filter_map(|record| {
                    record
                        .output
                        .clone()
                        .map(|output| (record.task_id.clone(), output))
                })
                .collect::<BTreeMap<_, _>>();
            let mut prepared = Vec::with_capacity(batch.len());
            for task in batch {
                let record = running
                    .iter()
                    .find(|record| record.task_id == task.id)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::InvalidState(format!("task `{}` missing", task.id))
                    })?;
                let execution_memory = record
                    .execution_memory
                    .clone()
                    .unwrap_or_else(|| run.working_memory.clone());
                let execution_contract = if options.check {
                    serde_json::json!({})
                } else {
                    task_output_schema(&workflow, task).unwrap_or_else(|| serde_json::json!({}))
                };
                let execution_metadata = TaskExecutionMetadata {
                    metadata_version: TASK_METADATA_VERSION,
                    definition_fingerprint: task_definition_fingerprint(
                        &workflow, task, &policy, None,
                    )?,
                    input_digest: resolved_input_digest(
                        run.inputs.as_object().ok_or_else(|| {
                            RuntimeError::InvalidState("run inputs must be an object".to_owned())
                        })?,
                        &execution_memory,
                        &task_outputs,
                        task,
                    )?,
                    output_contract_fingerprint: versioned_json_digest(&execution_contract)?,
                };
                self.store.record_task_execution_metadata(
                    run_id,
                    &task.id,
                    &execution_metadata,
                    &execution_memory,
                    self.clock.now(),
                )?;
                let mut execution_run = run.clone();
                execution_run.working_memory = execution_memory;
                prepared.push(PreparedBatchTask {
                    task,
                    record,
                    run: execution_run,
                    execution_contract,
                    execution_metadata,
                });
            }

            let executions = join_all(prepared.iter().map(|prepared| async {
                self.execute_task(
                    &workflow,
                    &prepared.run,
                    &prepared.record,
                    prepared.task,
                    &policy,
                    trace_id,
                    options,
                    cancellation,
                )
                .await
                .and_then(|execution| {
                    if let TaskExecution::Complete { output, .. } = &execution {
                        validate_output_contract(&prepared.execution_contract, output).map_err(
                            |message| RuntimeError::Task {
                                task: prepared.task.id.clone(),
                                message: format!("task output contract failed: {message}"),
                            },
                        )?;
                    }
                    Ok(execution)
                })
            }))
            .await;

            if cancellation.is_cancelled()
                || executions
                    .iter()
                    .any(|result| matches!(result, Err(RuntimeError::Cancelled)))
            {
                self.cancel_non_terminal(run_id, trace_id)?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state: RunState::Cancelled,
                    output: None,
                });
            }

            let mut committed_memory = run.working_memory.clone();
            let mut has_memory_update = false;
            let mut results = Vec::new();
            let mut paused = false;
            let mut stop_failure = None;
            let mut retrying = Vec::new();
            let all_effects = self.store.list_effects(run_id)?;
            for (prepared, execution) in prepared.iter().zip(executions) {
                match execution {
                    Ok(TaskExecution::Complete { output, memory }) => {
                        let delta = state_delta(&prepared.run.working_memory, memory.as_ref())?;
                        validate_memory_delta(prepared.task, &delta)?;
                        if memory.is_some() {
                            apply_state_delta(&mut committed_memory, &delta)?;
                            has_memory_update = true;
                        }
                        let completion = TaskCompletionMetadata {
                            execution: TaskExecutionMetadata {
                                definition_fingerprint: task_definition_fingerprint(
                                    &workflow,
                                    prepared.task,
                                    &policy,
                                    Some(&all_effects),
                                )?,
                                ..prepared.execution_metadata.clone()
                            },
                            output_digest: versioned_json_digest(&output)?,
                            state_delta_digest: versioned_json_digest(&delta)?,
                            artifact_manifest: collect_artifacts(
                                &self.store,
                                &policy,
                                &all_effects,
                                run_id,
                                &prepared.task.id,
                                self.clock.now(),
                            )?,
                            state_delta: delta,
                        };
                        results.push(TaskBatchResult {
                            task_id: prepared.task.id.clone(),
                            outcome: TaskBatchOutcome::Succeeded {
                                output,
                                metadata: Box::new(completion),
                            },
                        });
                    }
                    Ok(TaskExecution::Paused) => paused = true,
                    Err(error) => {
                        let message = error.to_string();
                        if prepared.record.attempt < prepared.task.retry.max_attempts
                            && retryable_error(&error)
                        {
                            results.push(TaskBatchResult {
                                task_id: prepared.task.id.clone(),
                                outcome: TaskBatchOutcome::RetryScheduled {
                                    error: message.clone(),
                                },
                            });
                            retrying
                                .push((prepared.task.id.clone(), prepared.task.retry.backoff_ms));
                        } else {
                            results.push(TaskBatchResult {
                                task_id: prepared.task.id.clone(),
                                outcome: TaskBatchOutcome::Failed {
                                    error: message.clone(),
                                },
                            });
                            if prepared.task.failure == FailureBehavior::Stop
                                && stop_failure.is_none()
                            {
                                stop_failure = Some((prepared.task.id.clone(), message));
                            }
                        }
                    }
                }
            }

            if !results.is_empty() {
                self.store.commit_task_batch(
                    run_id,
                    &results,
                    has_memory_update.then_some(&committed_memory),
                    stop_failure.is_some(),
                    self.clock.now(),
                    trace_id,
                )?;
            }
            for result in &results {
                match &result.outcome {
                    TaskBatchOutcome::Succeeded { .. } => {
                        self.trace(
                            TraceEvent::new(
                                SpanKind::Task,
                                TracePhase::Completed,
                                "task.execute",
                                trace_id,
                                run_id,
                                self.clock.now(),
                            )
                            .task(&result.task_id),
                        )?;
                    }
                    TaskBatchOutcome::Failed { error } => {
                        self.trace(
                            TraceEvent::new(
                                SpanKind::Task,
                                TracePhase::Failed,
                                "task.execute",
                                trace_id,
                                run_id,
                                self.clock.now(),
                            )
                            .task(&result.task_id)
                            .attributes(serde_json::json!({"error": error}), &[]),
                        )?;
                    }
                    TaskBatchOutcome::RetryScheduled { error } => {
                        self.trace(
                            TraceEvent::new(
                                SpanKind::Retry,
                                TracePhase::Waiting,
                                "task.retry",
                                trace_id,
                                run_id,
                                self.clock.now(),
                            )
                            .task(&result.task_id)
                            .attributes(serde_json::json!({"error": error}), &[]),
                        )?;
                    }
                }
            }

            if let Some((task, message)) = stop_failure {
                return Err(RuntimeError::RunFailed {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    task,
                    message,
                });
            }
            if paused {
                self.store.update_run_state(
                    run_id,
                    RunState::Paused,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state: RunState::Paused,
                    output: None,
                });
            }
            if !retrying.is_empty() {
                let backoff_ms = retrying
                    .iter()
                    .map(|(_, backoff_ms)| *backoff_ms)
                    .max()
                    .unwrap_or_default();
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    () = cancellation.cancelled() => {
                        self.cancel_non_terminal(run_id, trace_id)?;
                        return Ok(RunOutcome {
                            run_id: run_id.to_owned(),
                            trace_id: trace_id.to_owned(),
                            state: RunState::Cancelled,
                            output: None,
                        });
                    },
                }
                for (task_id, _) in retrying {
                    self.store.transition_task(
                        run_id,
                        &task_id,
                        TaskState::Ready,
                        None,
                        None,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                }
            }
        }
    }

    async fn drive_sequential(
        &self,
        run_id: &str,
        trace_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, RuntimeError> {
        loop {
            let run = self.store.load_run(run_id)?;
            if run.cancellation_requested || cancellation.is_cancelled() {
                self.cancel_non_terminal(run_id, trace_id)?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state: RunState::Cancelled,
                    output: None,
                });
            }
            let workflow: Workflow = serde_json::from_value(run.workflow.clone())?;
            let policy = PolicyEngine::new(workflow.spec.policy.clone(), &self.base_path)?;
            let tasks = self.store.list_tasks(run_id)?;
            if tasks.iter().all(|task| task.state.is_terminal()) {
                let failed = tasks.iter().any(|task| task.state == TaskState::Failed);
                let state = if failed {
                    RunState::Failed
                } else {
                    RunState::Succeeded
                };
                let output = collect_outputs(&run, &tasks, &workflow.spec.outputs)?;
                self.store.update_run_state(
                    run_id,
                    state,
                    Some(&output),
                    self.clock.now(),
                    trace_id,
                )?;
                self.trace(
                    TraceEvent::new(
                        SpanKind::Run,
                        if failed {
                            TracePhase::Failed
                        } else {
                            TracePhase::Completed
                        },
                        "run.execute",
                        trace_id,
                        run_id,
                        self.clock.now(),
                    )
                    .attributes(serde_json::json!({"state": state}), &[]),
                )?;
                return Ok(RunOutcome {
                    run_id: run_id.to_owned(),
                    trace_id: trace_id.to_owned(),
                    state,
                    output: Some(output),
                });
            }
            let Some(task) = next_task(&run.plan, &tasks) else {
                return Err(RuntimeError::InvalidState(
                    "no runnable task exists and the run is not terminal".to_owned(),
                ));
            };
            let dependencies: Vec<&TaskRecord> = task
                .needs
                .iter()
                .filter_map(|needed| tasks.iter().find(|candidate| &candidate.task_id == needed))
                .collect();
            if dependencies.iter().any(|dependency| {
                matches!(
                    dependency.state,
                    TaskState::Failed | TaskState::Cancelled | TaskState::Skipped
                )
            }) {
                self.store.transition_task(
                    run_id,
                    &task.id,
                    TaskState::Skipped,
                    None,
                    Some("dependency did not succeed"),
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                continue;
            }
            let ready_state = tasks
                .iter()
                .find(|record| record.task_id == task.id)
                .map(|record| record.state)
                .ok_or_else(|| RuntimeError::InvalidState(format!("task `{}` missing", task.id)))?;
            if ready_state == TaskState::Pending {
                let context = context_for(&run, &tasks)?;
                if let Some(condition) = &task.when
                    && !evaluate_when(condition, &context)?
                {
                    self.store.transition_task(
                        run_id,
                        &task.id,
                        TaskState::Skipped,
                        Some(&serde_json::json!({"reason": "when condition was false"})),
                        None,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                    continue;
                }
                self.store.transition_task(
                    run_id,
                    &task.id,
                    TaskState::Ready,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                continue;
            }
            if ready_state == TaskState::Ready {
                self.store.transition_task(
                    run_id,
                    &task.id,
                    TaskState::Running,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
                self.trace(
                    TraceEvent::new(
                        SpanKind::Task,
                        TracePhase::Started,
                        "task.execute",
                        trace_id,
                        run_id,
                        self.clock.now(),
                    )
                    .task(&task.id),
                )?;
                continue;
            }
            if ready_state != TaskState::Running {
                return Err(RuntimeError::InvalidState(format!(
                    "scheduler selected task `{}` in state {ready_state:?}",
                    task.id
                )));
            }

            let current = self
                .store
                .list_tasks(run_id)?
                .into_iter()
                .find(|record| record.task_id == task.id)
                .ok_or_else(|| RuntimeError::InvalidState(format!("task `{}` missing", task.id)))?;
            let task_outputs = tasks
                .iter()
                .filter_map(|record| {
                    record
                        .output
                        .clone()
                        .map(|output| (record.task_id.clone(), output))
                })
                .collect::<BTreeMap<_, _>>();
            let execution_contract = if options.check {
                serde_json::json!({})
            } else {
                task_output_schema(&workflow, task).unwrap_or_else(|| serde_json::json!({}))
            };
            let execution_metadata = TaskExecutionMetadata {
                metadata_version: TASK_METADATA_VERSION,
                definition_fingerprint: task_definition_fingerprint(
                    &workflow, task, &policy, None,
                )?,
                input_digest: resolved_input_digest(
                    run.inputs.as_object().ok_or_else(|| {
                        RuntimeError::InvalidState("run inputs must be an object".to_owned())
                    })?,
                    &run.working_memory,
                    &task_outputs,
                    task,
                )?,
                output_contract_fingerprint: versioned_json_digest(&execution_contract)?,
            };
            self.store.record_task_execution_metadata(
                run_id,
                &task.id,
                &execution_metadata,
                &run.working_memory,
                self.clock.now(),
            )?;
            let execution = self
                .execute_task(
                    &workflow,
                    &run,
                    &current,
                    task,
                    &policy,
                    trace_id,
                    options,
                    cancellation,
                )
                .await;
            let execution = execution.and_then(|execution| {
                if let TaskExecution::Complete { output, .. } = &execution {
                    validate_output_contract(&execution_contract, output).map_err(|message| {
                        RuntimeError::Task {
                            task: task.id.clone(),
                            message: format!("task output contract failed: {message}"),
                        }
                    })?;
                }
                Ok(execution)
            });
            match execution {
                Ok(TaskExecution::Complete { output, memory }) => {
                    let effects = self.store.list_effects(run_id)?;
                    let delta = state_delta(&run.working_memory, memory.as_ref())?;
                    let completion = TaskCompletionMetadata {
                        execution: TaskExecutionMetadata {
                            definition_fingerprint: task_definition_fingerprint(
                                &workflow,
                                task,
                                &policy,
                                Some(&effects),
                            )?,
                            ..execution_metadata
                        },
                        output_digest: versioned_json_digest(&output)?,
                        state_delta_digest: versioned_json_digest(&delta)?,
                        artifact_manifest: collect_artifacts(
                            &self.store,
                            &policy,
                            &effects,
                            run_id,
                            &task.id,
                            self.clock.now(),
                        )?,
                        state_delta: delta,
                    };
                    self.store.complete_task(
                        run_id,
                        &task.id,
                        &output,
                        memory.as_ref(),
                        &completion,
                        self.clock.now(),
                        trace_id,
                    )?;
                    self.trace(
                        TraceEvent::new(
                            SpanKind::Task,
                            TracePhase::Completed,
                            "task.execute",
                            trace_id,
                            run_id,
                            self.clock.now(),
                        )
                        .task(&task.id),
                    )?;
                }
                Ok(TaskExecution::Paused) => {
                    self.store.update_run_state(
                        run_id,
                        RunState::Paused,
                        None,
                        self.clock.now(),
                        trace_id,
                    )?;
                    self.trace(
                        TraceEvent::new(
                            SpanKind::Approval,
                            TracePhase::Waiting,
                            "approval.waiting",
                            trace_id,
                            run_id,
                            self.clock.now(),
                        )
                        .task(&task.id),
                    )?;
                    return Ok(RunOutcome {
                        run_id: run_id.to_owned(),
                        trace_id: trace_id.to_owned(),
                        state: RunState::Paused,
                        output: None,
                    });
                }
                Err(error) => {
                    if matches!(error, RuntimeError::Cancelled) {
                        self.cancel_non_terminal(run_id, trace_id)?;
                        return Ok(RunOutcome {
                            run_id: run_id.to_owned(),
                            trace_id: trace_id.to_owned(),
                            state: RunState::Cancelled,
                            output: None,
                        });
                    }
                    if current.attempt < task.retry.max_attempts && retryable_error(&error) {
                        self.store.transition_task(
                            run_id,
                            &task.id,
                            TaskState::RetryScheduled,
                            None,
                            Some(&error.to_string()),
                            None,
                            self.clock.now(),
                            trace_id,
                        )?;
                        self.trace(
                            TraceEvent::new(
                                SpanKind::Retry,
                                TracePhase::Waiting,
                                "task.retry",
                                trace_id,
                                run_id,
                                self.clock.now(),
                            )
                            .task(&task.id),
                        )?;
                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_millis(task.retry.backoff_ms)) => {}
                            () = cancellation.cancelled() => {
                                self.cancel_non_terminal(run_id, trace_id)?;
                                return Ok(RunOutcome {
                                    run_id: run_id.to_owned(),
                                    trace_id: trace_id.to_owned(),
                                    state: RunState::Cancelled,
                                    output: None,
                                });
                            },
                        }
                        self.store.transition_task(
                            run_id,
                            &task.id,
                            TaskState::Ready,
                            None,
                            None,
                            None,
                            self.clock.now(),
                            trace_id,
                        )?;
                        self.trace(
                            TraceEvent::new(
                                SpanKind::Task,
                                TracePhase::Failed,
                                "task.execute",
                                trace_id,
                                run_id,
                                self.clock.now(),
                            )
                            .task(&task.id)
                            .attributes(serde_json::json!({"error": error.to_string()}), &[]),
                        )?;
                    } else {
                        self.store.transition_task(
                            run_id,
                            &task.id,
                            TaskState::Failed,
                            None,
                            Some(&error.to_string()),
                            None,
                            self.clock.now(),
                            trace_id,
                        )?;
                        if task.failure == FailureBehavior::Stop {
                            self.store.update_run_state(
                                run_id,
                                RunState::Failed,
                                None,
                                self.clock.now(),
                                trace_id,
                            )?;
                            return Err(RuntimeError::RunFailed {
                                run_id: run_id.to_owned(),
                                trace_id: trace_id.to_owned(),
                                task: task.id.clone(),
                                message: error.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_task(
        &self,
        workflow: &Workflow,
        run: &agentctl_store::RunRecord,
        record: &TaskRecord,
        task: &agentctl_core::CompiledTask,
        policy: &PolicyEngine,
        trace_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<TaskExecution, RuntimeError> {
        let tasks = self.store.list_tasks(&run.run_id)?;
        let mut context = context_for(run, &tasks)?;
        context.vars = task
            .vars
            .iter()
            .map(|(name, value)| render(value, &context).map(|value| (name.clone(), value)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let raw_input = serde_json::to_value(&task.input)?;
        let input = render(&raw_input, &context)?;
        match &task.uses {
            TaskUse::Action(name) => {
                let action = workflow.spec.actions.get(name).ok_or_else(|| {
                    RuntimeError::InvalidState(format!("action `{name}` disappeared after compile"))
                })?;
                if action.kind == ActionKind::MemoryWrite {
                    let key = required_string(&input, "key")?;
                    if (workflow.spec.runtime.max_concurrency > 1 || !task.memory_writes.is_empty())
                        && !task.memory_writes.contains(&key)
                    {
                        return Err(RuntimeError::Task {
                            task: task.id.clone(),
                            message: format!(
                                "resolved working-memory key `{key}` is not declared in memoryWrites"
                            ),
                        });
                    }
                }
                self.execute_action(
                    workflow,
                    run,
                    record,
                    action,
                    input,
                    policy,
                    trace_id,
                    options,
                    cancellation,
                )
                .await
            }
            TaskUse::Agent(name) => {
                if options.check {
                    return Ok(TaskExecution::Complete {
                        output: serde_json::json!({
                            "status": "requires_execution",
                            "changed": false,
                            "provider": workflow.spec.agents[name].provider,
                        }),
                        memory: None,
                    });
                }
                self.execute_agent(
                    workflow,
                    run,
                    record,
                    name,
                    input,
                    policy,
                    trace_id,
                    options.interactive,
                    cancellation,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_action(
        &self,
        workflow: &Workflow,
        run: &agentctl_store::RunRecord,
        task: &TaskRecord,
        action: &ActionDefinition,
        input: Value,
        policy: &PolicyEngine,
        trace_id: &str,
        options: RunOptions,
        cancellation: &CancellationToken,
    ) -> Result<TaskExecution, RuntimeError> {
        match action.kind {
            ActionKind::Assign => Ok(TaskExecution::Complete {
                output: serde_json::to_value(ActionResult {
                    status: ChangeStatus::Unchanged,
                    changed: false,
                    before: None,
                    after: Some(input.clone()),
                    diff: None,
                    output: input,
                    predictability: PlanPredictability::FullyPredictable,
                })?,
                memory: None,
            }),
            ActionKind::Assert => {
                let passed = input.get("that").and_then(Value::as_bool).ok_or_else(|| {
                    RuntimeError::InvalidState("assert input requires boolean `that`".to_owned())
                })?;
                if passed {
                    Ok(TaskExecution::Complete {
                        output: serde_json::json!({"status": "unchanged", "changed": false, "passed": true}),
                        memory: None,
                    })
                } else {
                    Err(RuntimeError::Task {
                        task: task.task_id.clone(),
                        message: input
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("assertion failed")
                            .to_owned(),
                    })
                }
            }
            ActionKind::MemoryRead => {
                let key = required_string(&input, "key")?;
                let value = run.working_memory.get(&key).cloned().ok_or_else(|| {
                    RuntimeError::InvalidState(format!("working memory key `{key}` is missing"))
                })?;
                Ok(TaskExecution::Complete {
                    output: serde_json::json!({"status": "unchanged", "changed": false, "value": value}),
                    memory: None,
                })
            }
            ActionKind::MemoryWrite => {
                let key = required_string(&input, "key")?;
                let value = input.get("value").cloned().unwrap_or(Value::Null);
                let mut memory = run.working_memory.clone();
                let object = memory.as_object_mut().ok_or_else(|| {
                    RuntimeError::InvalidState("working memory must be an object".to_owned())
                })?;
                let before = object.insert(key.clone(), value.clone());
                let changed = before.as_ref() != Some(&value);
                let output = serde_json::json!({
                    "status": if changed {"changed"} else {"unchanged"},
                    "changed": changed,
                    "before": before,
                    "after": value,
                    "key": key,
                });
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    "builtin.memory.write",
                    EffectClass::InternalState,
                    Risk::Low,
                    Idempotency::Keyed,
                    input,
                    "update transactional run working memory",
                    trace_id,
                );
                match self.prepare_effect(
                    &request,
                    policy,
                    None,
                    "memory.write",
                    "internal_state",
                    options.interactive,
                )? {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(recorded) => Ok(TaskExecution::Complete {
                        output: recorded,
                        memory: Some(memory),
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        self.store
                            .complete_effect(&request.id, Ok(&output), self.clock.now())?;
                        Ok(TaskExecution::Complete {
                            output,
                            memory: Some(memory),
                        })
                    }
                }
            }
            ActionKind::Read => {
                let path = required_string(&input, "path")?;
                let resolved = policy.resolve_read_path(&path)?;
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    "builtin.read",
                    EffectClass::Observe,
                    Risk::Low,
                    Idempotency::Idempotent,
                    serde_json::json!({"path": path}),
                    "read a workspace file",
                    trace_id,
                );
                let execution = self.prepare_effect(
                    &request,
                    policy,
                    None,
                    "filesystem.read",
                    "observe",
                    options.interactive,
                )?;
                match execution {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(value) => Ok(TaskExecution::Complete {
                        output: value,
                        memory: None,
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        let content = read_bounded_text(&resolved).await;
                        match content {
                            Ok(content) => {
                                let output = serde_json::json!({"status": "unchanged", "changed": false, "content": content});
                                self.store.complete_effect(
                                    &request.id,
                                    Ok(&output),
                                    self.clock.now(),
                                )?;
                                Ok(TaskExecution::Complete {
                                    output,
                                    memory: None,
                                })
                            }
                            Err(error) => {
                                self.store.complete_effect(
                                    &request.id,
                                    Err(&error.to_string()),
                                    self.clock.now(),
                                )?;
                                Err(RuntimeError::Io(error))
                            }
                        }
                    }
                }
            }
            ActionKind::Write => {
                let path = required_string(&input, "path")?;
                let content = required_string(&input, "content")?;
                let resolved = policy.resolve_write_path(&path)?;
                let before = match read_bounded_text(&resolved).await {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(RuntimeError::Io(error)),
                };
                let changed = before.as_deref() != Some(&content);
                let diff = options
                    .diff
                    .then(|| unified_diff(before.as_deref(), &content));
                let output = serde_json::json!({
                    "status": if changed {"changed"} else {"unchanged"},
                    "changed": changed,
                    "before": before,
                    "after": content,
                    "diff": diff,
                    "path": path,
                    "predictability": "fully_predictable",
                });
                if options.check || !changed {
                    return Ok(TaskExecution::Complete {
                        output,
                        memory: None,
                    });
                }
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    "builtin.write",
                    EffectClass::WorkspaceMutate,
                    Risk::Medium,
                    Idempotency::Idempotent,
                    serde_json::json!({"path": path, "contentDigest": digest(content.as_bytes())}),
                    "write a workspace file",
                    trace_id,
                );
                match self.prepare_effect(
                    &request,
                    policy,
                    None,
                    "filesystem.write",
                    "mutate",
                    options.interactive,
                )? {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(value) => Ok(TaskExecution::Complete {
                        output: value,
                        memory: None,
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        let result = write_atomic(&resolved, content.as_bytes()).await;
                        match result {
                            Ok(()) => {
                                self.store.complete_effect(
                                    &request.id,
                                    Ok(&output),
                                    self.clock.now(),
                                )?;
                                Ok(TaskExecution::Complete {
                                    output,
                                    memory: None,
                                })
                            }
                            Err(error) => {
                                self.store.complete_effect(
                                    &request.id,
                                    Err(&error.to_string()),
                                    self.clock.now(),
                                )?;
                                Err(RuntimeError::Io(error))
                            }
                        }
                    }
                }
            }
            ActionKind::ShellExec => {
                if options.check {
                    return Ok(TaskExecution::Complete {
                        output: serde_json::json!({"status": "requires_execution", "changed": false, "predictability": "requires_execution"}),
                        memory: None,
                    });
                }
                let command = action.command.as_deref().ok_or_else(|| {
                    RuntimeError::InvalidState("shell action requires `command`".to_owned())
                })?;
                policy.authorize_process(command)?;
                let cwd = action
                    .cwd
                    .as_deref()
                    .map(|path| policy.resolve_read_path(path))
                    .transpose()?
                    .unwrap_or_else(|| self.base_path.clone());
                let mut resolved_environment = BTreeMap::new();
                let mut environment_digests = BTreeMap::new();
                let secret_resolver = secret::SecretResolver::restricted(policy.clone());
                for (name, reference) in &action.env {
                    policy.authorize_environment(name)?;
                    let value = secret_resolver
                        .resolve(reference, cancellation)
                        .await
                        .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
                    environment_digests.insert(
                        name.clone(),
                        serde_json::json!({
                            "source": reference.source_description(),
                            "valueDigest": digest(value.expose().as_bytes()),
                        }),
                    );
                    resolved_environment.insert(name.clone(), value);
                }
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    "builtin.shell.exec",
                    EffectClass::ProcessExecution,
                    Risk::High,
                    Idempotency::Unknown,
                    serde_json::json!({
                        "command": command,
                        "args": action.args,
                        "cwd": action.cwd,
                        "environment": environment_digests,
                        "stdoutLimitBytes": action.stdout_limit_bytes(),
                        "stderrLimitBytes": action.stderr_limit_bytes(),
                        "combinedOutputLimitBytes": action.combined_output_limit_bytes(),
                    }),
                    "execute an allowlisted subprocess",
                    trace_id,
                );
                match self.prepare_effect(
                    &request,
                    policy,
                    None,
                    "process.exec",
                    "act",
                    options.interactive,
                )? {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(value) => Ok(TaskExecution::Complete {
                        output: value,
                        memory: None,
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        let mut process = Command::new(command);
                        process
                            .args(&action.args)
                            .current_dir(cwd)
                            .env_clear()
                            .kill_on_drop(true);
                        for (name, value) in &resolved_environment {
                            process.env(name, value.expose());
                        }
                        let timeout = Duration::from_secs(
                            action
                                .timeout_seconds
                                .unwrap_or(task_timeout(workflow, &task.task_id)),
                        );
                        let limits = ProcessOutputLimits {
                            stdout_bytes: action.stdout_limit_bytes(),
                            stderr_bytes: action.stderr_limit_bytes(),
                            combined_bytes: action.combined_output_limit_bytes(),
                        };
                        let result =
                            run_bounded_process(process, limits, timeout, cancellation).await;
                        match result {
                            Ok(result) => {
                                let secrets = resolved_environment
                                    .values()
                                    .map(agentctl_core::secret::SecretValue::expose)
                                    .collect::<Vec<_>>();
                                let output = serde_json::json!({
                                    "status": if result.status.success() {"changed"} else {"failed"},
                                    "changed": result.status.success(),
                                    "exitCode": result.status.code(),
                                    "stdout": redact_text(
                                        &String::from_utf8_lossy(&result.stdout),
                                        &secrets,
                                    ),
                                    "stderr": redact_text(
                                        &String::from_utf8_lossy(&result.stderr),
                                        &secrets,
                                    ),
                                });
                                if result.status.success() {
                                    self.store.complete_effect(
                                        &request.id,
                                        Ok(&output),
                                        self.clock.now(),
                                    )?;
                                    Ok(TaskExecution::Complete {
                                        output,
                                        memory: None,
                                    })
                                } else {
                                    let message =
                                        format!("subprocess exited with {}", result.status);
                                    self.store.complete_effect(
                                        &request.id,
                                        Err(&message),
                                        self.clock.now(),
                                    )?;
                                    Err(RuntimeError::Task {
                                        task: task.task_id.clone(),
                                        message,
                                    })
                                }
                            }
                            Err(ProcessRunError::OutputLimitExceeded {
                                stream,
                                limit_bytes,
                                stdout,
                                stderr,
                            }) => {
                                let secrets = resolved_environment
                                    .values()
                                    .map(agentctl_core::secret::SecretValue::expose)
                                    .collect::<Vec<_>>();
                                let diagnostic = serde_json::json!({
                                    "code": "subprocess_output_limit_exceeded",
                                    "stream": stream,
                                    "limitBytes": limit_bytes,
                                    "stdoutPrefix": redacted_process_diagnostic(&stdout, &secrets),
                                    "stderrPrefix": redacted_process_diagnostic(&stderr, &secrets),
                                    "remediation": "reduce subprocess output or raise the action output limit within the 16777216-byte maximum",
                                })
                                .to_string();
                                self.store.complete_effect(
                                    &request.id,
                                    Err(&diagnostic),
                                    self.clock.now(),
                                )?;
                                Err(RuntimeError::Task {
                                    task: task.task_id.clone(),
                                    message: diagnostic,
                                })
                            }
                            Err(error) => {
                                let error = match error {
                                    ProcessRunError::Timeout { seconds } => RuntimeError::Task {
                                        task: task.task_id.clone(),
                                        message: format!(
                                            "subprocess timed out after {seconds} seconds and was terminated"
                                        ),
                                    },
                                    ProcessRunError::Cancelled => RuntimeError::Cancelled,
                                    ProcessRunError::Spawn(error)
                                    | ProcessRunError::Wait(error) => RuntimeError::Io(error),
                                    ProcessRunError::Read { stream, message } => {
                                        RuntimeError::Task {
                                            task: task.task_id.clone(),
                                            message: format!(
                                                "failed to capture subprocess {stream}: {message}"
                                            ),
                                        }
                                    }
                                    ProcessRunError::OutputLimitExceeded { .. } => unreachable!(),
                                };
                                self.store.mark_effect_uncertain(
                                    &request.id,
                                    &error.to_string(),
                                    self.clock.now(),
                                )?;
                                Err(error)
                            }
                        }
                    }
                }
            }
            ActionKind::LongTermMemoryRead => {
                let namespace = workflow
                    .spec
                    .memory
                    .long_term
                    .as_ref()
                    .map_or("default", |memory| memory.namespace.as_str());
                let key = required_string(&input, "key")?;
                let value = self
                    .store
                    .get_long_term_memory(namespace, &key, self.clock.now())?;
                Ok(TaskExecution::Complete {
                    output: serde_json::json!({"status": "unchanged", "changed": false, "value": value}),
                    memory: None,
                })
            }
            ActionKind::LongTermMemoryWrite => {
                if options.check {
                    return Ok(TaskExecution::Complete {
                        output: serde_json::json!({"status": "requires_execution", "changed": false}),
                        memory: None,
                    });
                }
                let namespace = workflow
                    .spec
                    .memory
                    .long_term
                    .as_ref()
                    .map_or("default", |memory| memory.namespace.as_str());
                let key = required_string(&input, "key")?;
                let value = input.get("value").cloned().unwrap_or(Value::Null);
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    "builtin.long_term_memory.write",
                    EffectClass::ExternalMutate,
                    Risk::Medium,
                    Idempotency::Keyed,
                    serde_json::json!({"namespace": namespace, "key": key, "value": value}),
                    "write cross-run memory",
                    trace_id,
                );
                match self.prepare_effect(
                    &request,
                    policy,
                    None,
                    "memory.write",
                    "mutate",
                    options.interactive,
                )? {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(output) => Ok(TaskExecution::Complete {
                        output,
                        memory: None,
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        self.store.put_long_term_memory(
                            namespace,
                            &key,
                            &value,
                            None,
                            self.clock.now(),
                        )?;
                        let output =
                            serde_json::json!({"status": "changed", "changed": true, "key": key});
                        self.store
                            .complete_effect(&request.id, Ok(&output), self.clock.now())?;
                        Ok(TaskExecution::Complete {
                            output,
                            memory: None,
                        })
                    }
                }
            }
            ActionKind::McpCall | ActionKind::A2aDelegate => {
                if options.check {
                    return Ok(TaskExecution::Complete {
                        output: serde_json::json!({"status": "requires_execution", "changed": false}),
                        memory: None,
                    });
                }
                let handler = self.registry.external_actions.as_ref().ok_or_else(|| {
                    RuntimeError::InvalidState(format!(
                        "no handler is registered for {:?}",
                        action.kind
                    ))
                })?;
                let remote_url = if action.kind == ActionKind::McpCall {
                    let server = required_string(&input, "server")?;
                    workflow
                        .spec
                        .mcp_servers
                        .get(&server)
                        .map(|definition| definition.url.as_str())
                        .ok_or_else(|| {
                            RuntimeError::InvalidState(format!("unknown MCP server `{server}`"))
                        })?
                } else {
                    let peer = required_string(&input, "peer")?;
                    workflow
                        .spec
                        .a2a_peers
                        .get(&peer)
                        .map(|definition| definition.card_url.as_str())
                        .ok_or_else(|| {
                            RuntimeError::InvalidState(format!("unknown A2A peer `{peer}`"))
                        })?
                };
                let remote_url = Url::parse(remote_url).map_err(|error| {
                    RuntimeError::InvalidState(format!("remote URL is invalid: {error}"))
                })?;
                policy.authorize_network(&remote_url)?;
                let (class, risk, operation) = if action.kind == ActionKind::McpCall {
                    (EffectClass::Network, Risk::Medium, "mcp.call")
                } else {
                    (EffectClass::RemoteAgent, Risk::High, "a2a.delegate")
                };
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    1,
                    operation,
                    class,
                    risk,
                    Idempotency::Unknown,
                    input.clone(),
                    operation,
                    trace_id,
                );
                match self.prepare_effect(
                    &request,
                    policy,
                    None,
                    operation,
                    "network",
                    options.interactive,
                )? {
                    PreparedEffect::Paused => Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(output) => Ok(TaskExecution::Complete {
                        output,
                        memory: None,
                    }),
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        let result = handler.execute(action.kind, &input, cancellation).await;
                        match result {
                            Ok(output) => {
                                self.store.complete_effect(
                                    &request.id,
                                    Ok(&output),
                                    self.clock.now(),
                                )?;
                                Ok(TaskExecution::Complete {
                                    output,
                                    memory: None,
                                })
                            }
                            Err(error) => {
                                if matches!(
                                    error,
                                    RuntimeError::Cancelled
                                        | RuntimeError::ExternalEffectUncertain(_)
                                ) {
                                    self.store.mark_effect_uncertain(
                                        &request.id,
                                        &error.to_string(),
                                        self.clock.now(),
                                    )?;
                                } else {
                                    self.store.complete_effect(
                                        &request.id,
                                        Err(&error.to_string()),
                                        self.clock.now(),
                                    )?;
                                }
                                Err(error)
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_agent(
        &self,
        workflow: &Workflow,
        run: &agentctl_store::RunRecord,
        task: &TaskRecord,
        agent_name: &str,
        input: Value,
        policy: &PolicyEngine,
        trace_id: &str,
        interactive: bool,
        cancellation: &CancellationToken,
    ) -> Result<TaskExecution, RuntimeError> {
        let agent = workflow.spec.agents.get(agent_name).ok_or_else(|| {
            RuntimeError::InvalidState(format!("agent `{agent_name}` disappeared after compile"))
        })?;
        let provider_definition =
            workflow
                .spec
                .providers
                .get(&agent.provider)
                .ok_or_else(|| {
                    RuntimeError::InvalidState(format!(
                        "provider `{}` disappeared after compile",
                        agent.provider
                    ))
                })?;
        let endpoint = provider_definition
            .endpoint
            .as_deref()
            .unwrap_or(match provider_definition.kind {
                agentctl_core::dsl::ProviderKind::Fake => "http://127.0.0.1",
                agentctl_core::dsl::ProviderKind::Openai => "https://api.openai.com/v1/responses",
                agentctl_core::dsl::ProviderKind::Anthropic => {
                    "https://api.anthropic.com/v1/messages"
                }
                agentctl_core::dsl::ProviderKind::Google => {
                    "https://generativelanguage.googleapis.com/v1beta/models"
                }
                agentctl_core::dsl::ProviderKind::AzureOpenai => {
                    return Err(RuntimeError::InvalidState(
                        "Azure OpenAI provider requires an explicit endpoint".to_owned(),
                    ));
                }
            });
        if provider_definition.kind != agentctl_core::dsl::ProviderKind::Fake {
            let endpoint = Url::parse(endpoint).map_err(|error| {
                RuntimeError::InvalidState(format!("provider endpoint is invalid: {error}"))
            })?;
            policy.authorize_network(&endpoint)?;
        }
        let provider = self
            .registry
            .providers
            .get(&agent.provider)
            .ok_or_else(|| {
                RuntimeError::InvalidState(format!(
                    "provider `{}` is not registered",
                    agent.provider
                ))
            })?;
        let mut ordinal = 0_u16;
        let instructions = match (&agent.instructions, &agent.instructions_file) {
            (Some(value), None) => value.clone(),
            (None, Some(path)) => {
                let resolved = policy.resolve_read_path(path)?;
                ordinal = ordinal.saturating_add(1);
                let request = EffectRequest::new(
                    &run.run_id,
                    &task.task_id,
                    task.attempt,
                    ordinal,
                    "agent.instructions.read",
                    EffectClass::Observe,
                    Risk::Low,
                    Idempotency::Idempotent,
                    serde_json::json!({"path": path}),
                    "read the agent instruction file",
                    trace_id,
                );
                let output = match self.prepare_effect(
                    &request,
                    policy,
                    Some(agent_name),
                    "filesystem.read",
                    "observe",
                    interactive,
                )? {
                    PreparedEffect::Paused => return Ok(TaskExecution::Paused),
                    PreparedEffect::Recorded(value) => value,
                    PreparedEffect::Execute => {
                        self.store
                            .mark_effect_started(&request.id, self.clock.now())?;
                        match read_bounded_text(&resolved).await {
                            Ok(content) => {
                                let output = serde_json::json!({"content": content});
                                self.store.complete_effect(
                                    &request.id,
                                    Ok(&output),
                                    self.clock.now(),
                                )?;
                                output
                            }
                            Err(error) => {
                                self.store.complete_effect(
                                    &request.id,
                                    Err(&error.to_string()),
                                    self.clock.now(),
                                )?;
                                return Err(RuntimeError::Io(error));
                            }
                        }
                    }
                };
                output
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::InvalidState(
                            "recorded instruction-file effect has no string content".to_owned(),
                        )
                    })?
                    .to_owned()
            }
            _ => {
                return Err(RuntimeError::InvalidState(format!(
                    "agent `{agent_name}` must define exactly one instruction source"
                )));
            }
        };
        let prompt = input.get("prompt").and_then(Value::as_str).map_or_else(
            || serde_json::to_string(&input),
            |value| Ok(value.to_owned()),
        )?;
        let mut messages = vec![Message::User(vec![ContentBlock::Text { text: prompt }])];
        let contracts = agent
            .tools
            .iter()
            .map(|name| {
                self.registry
                    .tools
                    .get(name)
                    .map(|tool| tool.contract().clone())
                    .ok_or_else(|| {
                        RuntimeError::InvalidState(format!("tool `{name}` is not registered"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut continuation = None;
        let mut usage = Usage::default();
        let mut tool_call_count = 0_u16;
        for _turn in 0..agent.max_turns {
            ordinal = ordinal.saturating_add(1);
            let provider_request = ProviderRequest {
                model: agent.model.clone(),
                instructions: instructions.clone(),
                messages: messages.clone(),
                tools: contracts.clone(),
                max_output_tokens: agent.max_output_tokens,
                reasoning: agent.reasoning.clone(),
                structured_output: agent.structured_output.clone(),
                continuation: continuation.clone(),
                prompt_cache_key: Some(format!("{}:{}", workflow.metadata.name, agent_name)),
                safety_identifier: None,
                provider_options: agent.provider_options.clone(),
            };
            let effect = EffectRequest::new(
                &run.run_id,
                &task.task_id,
                task.attempt,
                ordinal,
                &agent.provider,
                EffectClass::Model,
                Risk::Medium,
                Idempotency::AtMostOnce,
                serde_json::to_value(&provider_request)?,
                "invoke a bounded model provider",
                trace_id,
            );
            let response: ProviderResponse = match self.prepare_effect(
                &effect,
                policy,
                Some(agent_name),
                &format!("provider.{}", provider.name()),
                "model",
                interactive,
            )? {
                PreparedEffect::Paused => return Ok(TaskExecution::Paused),
                PreparedEffect::Recorded(value) => serde_json::from_value(value)?,
                PreparedEffect::Execute => {
                    self.store
                        .mark_effect_started(&effect.id, self.clock.now())?;
                    let result = tokio::select! {
                        result = tokio::time::timeout(
                            Duration::from_secs(agent.timeout_seconds),
                            provider.complete(&provider_request, cancellation),
                        ) => result.unwrap_or(Err(ProviderError::Timeout)),
                        () = cancellation.cancelled() => Err(ProviderError::Cancelled),
                    };
                    match result {
                        Ok(response) => {
                            let value = serde_json::to_value(&response)?;
                            self.store
                                .complete_effect(&effect.id, Ok(&value), self.clock.now())?;
                            response
                        }
                        Err(ProviderError::Cancelled) => {
                            self.store.mark_effect_uncertain(
                                &effect.id,
                                "provider request was cancelled after dispatch",
                                self.clock.now(),
                            )?;
                            return Err(RuntimeError::Cancelled);
                        }
                        Err(error) => {
                            if provider_effect_is_uncertain(&error) {
                                self.store.mark_effect_uncertain(
                                    &effect.id,
                                    &error.to_string(),
                                    self.clock.now(),
                                )?;
                            } else {
                                self.store.complete_effect(
                                    &effect.id,
                                    Err(&error.to_string()),
                                    self.clock.now(),
                                )?;
                            }
                            return Err(RuntimeError::Provider(error));
                        }
                    }
                }
            };
            add_usage(&mut usage, &response.usage);
            enforce_usage(agent, &usage, &task.task_id)?;
            continuation = response.continuation.clone();
            self.store.put_provider_session(
                &run.run_id,
                &task.task_id,
                &agent.provider,
                &serde_json::to_value(&continuation)?,
                self.clock.now(),
            )?;
            if response.finish_reason == FinishReason::ToolCalls || !response.tool_calls.is_empty()
            {
                messages.push(Message::Assistant(response.assistant_content.clone()));
                let mut results = Vec::new();
                for call in response.tool_calls {
                    tool_call_count = tool_call_count.saturating_add(1);
                    if tool_call_count > agent.max_tool_calls {
                        return Err(RuntimeError::Task {
                            task: task.task_id.clone(),
                            message: "maximum tool-call count exceeded".to_owned(),
                        });
                    }
                    ordinal = ordinal.saturating_add(1);
                    let tool = self.registry.tools.get(&call.name).ok_or_else(|| {
                        RuntimeError::InvalidState(format!(
                            "provider requested unavailable tool `{}`",
                            call.name
                        ))
                    })?;
                    tool.contract().validate_input(&call.input)?;
                    let contract = tool.contract();
                    let call_id = call.id.clone();
                    let tool_effect = EffectRequest::new(
                        &run.run_id,
                        &task.task_id,
                        task.attempt,
                        ordinal,
                        &format!("tool.{}", contract.id),
                        contract.effect_class,
                        contract.risk,
                        contract.idempotency,
                        call.input.clone(),
                        &format!("execute tool {}", contract.id),
                        trace_id,
                    );
                    let output = match self.prepare_effect_with_approval(
                        &tool_effect,
                        policy,
                        Some(agent_name),
                        &contract.id,
                        &contract.capability,
                        contract.approval,
                        interactive,
                    )? {
                        PreparedEffect::Paused => return Ok(TaskExecution::Paused),
                        PreparedEffect::Recorded(value) => value,
                        PreparedEffect::Execute => {
                            self.store
                                .mark_effect_started(&tool_effect.id, self.clock.now())?;
                            self.store.start_tool_call(
                                &call_id,
                                &run.run_id,
                                &task.task_id,
                                &tool_effect.id,
                                &contract.id,
                                &tool_effect.input_digest,
                                self.clock.now(),
                            )?;
                            let result = tokio::select! {
                                result = tokio::time::timeout(
                                    Duration::from_secs(contract.timeout_seconds),
                                    tool.execute(call.input.clone(), cancellation),
                                ) => result.map_err(|_| ToolContractError::Execution(format!("tool `{}` timed out", contract.id))),
                                () = cancellation.cancelled() => Err(ToolContractError::Cancelled),
                            };
                            match result {
                                Ok(Ok(result)) => {
                                    if let Err(error) = contract.validate_output(&result.output) {
                                        self.store.complete_tool_effect(
                                            &tool_effect.id,
                                            &run.run_id,
                                            &call_id,
                                            Err(&error.to_string()),
                                            None,
                                            self.clock.now(),
                                        )?;
                                        return Err(RuntimeError::Tool(error));
                                    }
                                    self.store.complete_tool_effect(
                                        &tool_effect.id,
                                        &run.run_id,
                                        &call_id,
                                        Ok(&result.output),
                                        Some(&digest(&serde_json::to_vec(&result.output)?)),
                                        self.clock.now(),
                                    )?;
                                    result.output
                                }
                                Ok(Err(ToolContractError::Cancelled))
                                | Err(ToolContractError::Cancelled) => {
                                    self.store.mark_tool_effect_uncertain(
                                        &tool_effect.id,
                                        &run.run_id,
                                        &call_id,
                                        "tool execution was cancelled after dispatch",
                                        self.clock.now(),
                                    )?;
                                    return Err(RuntimeError::Cancelled);
                                }
                                Err(error) => {
                                    self.store.mark_tool_effect_uncertain(
                                        &tool_effect.id,
                                        &run.run_id,
                                        &call_id,
                                        &error.to_string(),
                                        self.clock.now(),
                                    )?;
                                    return Err(RuntimeError::Tool(error));
                                }
                                Ok(Err(error)) => {
                                    self.store.complete_tool_effect(
                                        &tool_effect.id,
                                        &run.run_id,
                                        &call_id,
                                        Err(&error.to_string()),
                                        None,
                                        self.clock.now(),
                                    )?;
                                    return Err(RuntimeError::Tool(error));
                                }
                            }
                        }
                    };
                    results.push(ContentBlock::ToolResult {
                        id: call.id,
                        output,
                        is_error: false,
                    });
                }
                messages.push(Message::User(results));
                continue;
            }
            match response.finish_reason {
                FinishReason::Complete => {
                    let output = if let Some(schema) = &agent.structured_output {
                        let structured: Value =
                            serde_json::from_str(&response.text).map_err(|error| {
                                RuntimeError::Task {
                                    task: task.task_id.clone(),
                                    message: format!(
                                        "provider structured output was not valid JSON: {error}"
                                    ),
                                }
                            })?;
                        validate_output_contract(schema, &structured).map_err(|message| {
                            RuntimeError::Task {
                                task: task.task_id.clone(),
                                message: format!(
                                    "provider structured output failed its contract: {message}"
                                ),
                            }
                        })?;
                        structured
                    } else {
                        serde_json::json!({"text": response.text, "usage": usage})
                    };
                    return Ok(TaskExecution::Complete {
                        output,
                        memory: None,
                    });
                }
                FinishReason::MaxTokens => {
                    return Err(RuntimeError::Task {
                        task: task.task_id.clone(),
                        message: "provider reached maximum output tokens".to_owned(),
                    });
                }
                FinishReason::Refusal => {
                    return Err(RuntimeError::Task {
                        task: task.task_id.clone(),
                        message: "provider refused the request".to_owned(),
                    });
                }
                FinishReason::Cancelled => return Err(RuntimeError::Cancelled),
                FinishReason::ToolCalls => {}
            }
        }
        Err(RuntimeError::Task {
            task: task.task_id.clone(),
            message: "maximum agent turns exceeded".to_owned(),
        })
    }

    fn prepare_effect(
        &self,
        request: &EffectRequest,
        policy: &PolicyEngine,
        agent: Option<&str>,
        tool: &str,
        capability: &str,
        interactive: bool,
    ) -> Result<PreparedEffect, RuntimeError> {
        self.prepare_effect_with_approval(
            request,
            policy,
            agent,
            tool,
            capability,
            ApprovalRequirement::Policy,
            interactive,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_effect_with_approval(
        &self,
        request: &EffectRequest,
        policy: &PolicyEngine,
        agent: Option<&str>,
        tool: &str,
        capability: &str,
        approval: ApprovalRequirement,
        interactive: bool,
    ) -> Result<PreparedEffect, RuntimeError> {
        match self.store.load_effect(&request.id) {
            Ok(record) => {
                if let Some(reconciliation) =
                    self.store.latest_effect_reconciliation(&request.id)?
                {
                    return match reconciliation.status {
                        ReconciliationStatus::Applied => reconciliation
                            .result
                            .map(PreparedEffect::Recorded)
                            .ok_or_else(|| {
                                RuntimeError::InvalidState(format!(
                                    "applied reconciliation for effect `{}` has no result",
                                    request.id
                                ))
                            }),
                        ReconciliationStatus::NotApplied | ReconciliationStatus::Compensated => {
                            Err(RuntimeError::InvalidState(format!(
                                "effect `{}` requires a fresh task attempt after {:?} reconciliation",
                                request.id, reconciliation.status
                            )))
                        }
                    };
                }
                return match record.status {
                    EffectStatus::Succeeded if record.confirmed => {
                        record.result.map(PreparedEffect::Recorded).ok_or_else(|| {
                            RuntimeError::InvalidState(format!(
                                "effect `{}` has no result",
                                request.id
                            ))
                        })
                    }
                    EffectStatus::Requested => Ok(PreparedEffect::Execute),
                    EffectStatus::WaitingForApproval => Ok(PreparedEffect::Paused),
                    EffectStatus::Started | EffectStatus::Uncertain => {
                        Err(RuntimeError::UncertainEffect {
                            run_id: request.run_id.clone(),
                            trace_id: request.trace_id.clone(),
                            effect_id: request.id.clone(),
                        })
                    }
                    EffectStatus::Failed => Err(RuntimeError::Task {
                        task: request.task_id.clone(),
                        message: record.error.unwrap_or_else(|| "effect failed".to_owned()),
                    }),
                    EffectStatus::Cancelled => Err(RuntimeError::Task {
                        task: request.task_id.clone(),
                        message: "effect was rejected or cancelled".to_owned(),
                    }),
                    EffectStatus::Succeeded => Err(RuntimeError::InvalidState(format!(
                        "effect `{}` is unconfirmed",
                        request.id
                    ))),
                };
            }
            Err(StoreError::EffectNotFound(_)) => {}
            Err(error) => return Err(RuntimeError::Store(error)),
        }
        let context = PolicyContext {
            run_id: request.run_id.clone(),
            trace_id: request.trace_id.clone(),
            task_id: request.task_id.clone(),
            agent: agent.map(ToOwned::to_owned),
            tool: tool.to_owned(),
            capability: capability.to_owned(),
            effect_class: request.effect_class,
            risk: request.risk,
            resource: None,
            provider: (request.effect_class == EffectClass::Model)
                .then(|| request.operation.clone()),
            input: request.input.clone(),
            interactive,
        };
        let decision = policy.decide_with_approval(&context, approval);
        match decision {
            PolicyDecision::Allow { .. } => {
                self.store
                    .record_effect_request(request, self.clock.now())?;
                self.trace_effect_request(request)?;
                Ok(PreparedEffect::Execute)
            }
            PolicyDecision::Deny { reason } => Err(RuntimeError::Task {
                task: request.task_id.clone(),
                message: format!("policy denied effect: {reason}"),
            }),
            PolicyDecision::RequireApproval { reason } => {
                let approval_id = format!("approval-{}", &request.id[..16]);
                self.store.create_approval(
                    request,
                    &ApprovalRequest {
                        approval_id,
                        run_id: request.run_id.clone(),
                        effect_id: request.id.clone(),
                        task_id: request.task_id.clone(),
                        agent: agent.map(ToOwned::to_owned),
                        tool: tool.to_owned(),
                        capability: capability.to_owned(),
                        risk: format!("{:?}", request.risk).to_ascii_lowercase(),
                        redacted_input: redact(&request.input, &[]),
                        expected_effect: request.expected_effect.clone(),
                        reason,
                        trace_id: request.trace_id.clone(),
                        requested_at: self.clock.now(),
                    },
                )?;
                self.trace_effect_request(request)?;
                Ok(PreparedEffect::Paused)
            }
        }
    }

    fn trace_effect_request(&self, request: &EffectRequest) -> Result<(), RuntimeError> {
        self.trace(
            TraceEvent::new(
                match request.effect_class {
                    EffectClass::Model => SpanKind::ProviderRequest,
                    EffectClass::RemoteAgent => SpanKind::A2aDelegation,
                    EffectClass::Network if request.operation.starts_with("mcp.") => {
                        SpanKind::McpRequest
                    }
                    _ => SpanKind::Effect,
                },
                TracePhase::Started,
                &request.operation,
                &request.trace_id,
                &request.run_id,
                self.clock.now(),
            )
            .task(&request.task_id)
            .effect(&request.id)
            .attributes(
                serde_json::json!({
                    "inputDigest": request.input_digest,
                    "effectClass": request.effect_class,
                    "risk": request.risk,
                }),
                &[],
            ),
        )
    }

    fn cancel_non_terminal(&self, run_id: &str, trace_id: &str) -> Result<(), RuntimeError> {
        for task in self.store.list_tasks(run_id)? {
            if !task.state.is_terminal() {
                let next = match task.state {
                    TaskState::Pending
                    | TaskState::Ready
                    | TaskState::Running
                    | TaskState::WaitingForApproval
                    | TaskState::WaitingForEffect
                    | TaskState::RetryScheduled => TaskState::Cancelled,
                    TaskState::Succeeded
                    | TaskState::Failed
                    | TaskState::Skipped
                    | TaskState::Cancelled => continue,
                };
                self.store.transition_task(
                    run_id,
                    &task.task_id,
                    next,
                    None,
                    Some("run cancelled"),
                    None,
                    self.clock.now(),
                    trace_id,
                )?;
            }
        }
        self.store.update_run_state(
            run_id,
            RunState::Cancelled,
            None,
            self.clock.now(),
            trace_id,
        )?;
        Ok(())
    }

    fn trace(&self, event: TraceEvent) -> Result<(), RuntimeError> {
        self.store.record_trace_event(
            &event.run_id,
            &event.trace_id,
            &serde_json::to_value(&event)?,
            event.timestamp,
        )?;
        self.traces.record(&event);
        Ok(())
    }
}

enum PreparedEffect {
    Execute,
    Recorded(Value),
    Paused,
}

enum TaskExecution {
    Complete {
        output: Value,
        memory: Option<Value>,
    },
    Paused,
}

struct PreparedBatchTask<'a> {
    task: &'a agentctl_core::CompiledTask,
    record: TaskRecord,
    run: agentctl_store::RunRecord,
    execution_contract: Value,
    execution_metadata: TaskExecutionMetadata,
}

fn repair_block(
    task_id: &str,
    rule: &str,
    message: String,
    source_fingerprint: Option<String>,
    target_fingerprint: Option<String>,
    suggested_repair_roots: Vec<String>,
    full_fork_required: bool,
) -> RepairBlock {
    RepairBlock {
        task_id: task_id.to_owned(),
        rule: rule.to_owned(),
        message,
        source_fingerprint,
        target_fingerprint,
        suggested_repair_roots,
        full_fork_required,
    }
}

fn blocked_task_plan(
    task_id: &str,
    source_state: Option<TaskState>,
    source_fingerprint: Option<String>,
    target_fingerprint: String,
) -> RepairTaskPlan {
    RepairTaskPlan {
        task_id: task_id.to_owned(),
        disposition: PlannedDisposition::Blocked,
        reason: "reuse compatibility check failed".to_owned(),
        source_state,
        source_fingerprint,
        target_fingerprint: Some(target_fingerprint),
    }
}

fn repair_effect_is_unsafe(
    effect: &EffectRecord,
    reconciliation: Option<&EffectReconciliationRecord>,
) -> bool {
    let potentially_mutating = matches!(
        effect.request.effect_class,
        EffectClass::WorkspaceMutate
            | EffectClass::ExternalMutate
            | EffectClass::ProcessExecution
            | EffectClass::Network
            | EffectClass::RemoteAgent
    );
    if !potentially_mutating {
        return false;
    }
    if let Some(reconciliation) = reconciliation {
        return match reconciliation.status {
            ReconciliationStatus::NotApplied | ReconciliationStatus::Compensated => false,
            ReconciliationStatus::Applied => !matches!(
                effect.request.idempotency,
                Idempotency::Pure | Idempotency::Idempotent | Idempotency::Keyed
            ),
        };
    }
    potentially_mutating
        && (matches!(
            effect.status,
            EffectStatus::Started | EffectStatus::Uncertain
        ) || (effect.status == EffectStatus::Succeeded
            && matches!(
                effect.request.idempotency,
                Idempotency::AtMostOnce | Idempotency::Unknown
            )))
}

fn unresolved_reuse_effects(
    store: &SqliteStore,
    task: &TaskRecord,
    source_effects: &[EffectRecord],
) -> Result<Vec<String>, String> {
    if task.disposition != TaskDisposition::Reused {
        let mut unresolved = Vec::new();
        for effect in source_effects
            .iter()
            .filter(|effect| effect.request.task_id == task.task_id)
        {
            let reconciliation = store
                .latest_effect_reconciliation(&effect.request.id)
                .map_err(|error| error.to_string())?;
            if reconciliation
                .as_ref()
                .is_some_and(|record| record.status == ReconciliationStatus::Compensated)
                || (matches!(
                    effect.status,
                    EffectStatus::Started | EffectStatus::Uncertain
                ) && !reconciliation
                    .as_ref()
                    .is_some_and(|record| record.status == ReconciliationStatus::Applied))
            {
                unresolved.push(effect.request.id.clone());
            }
        }
        return Ok(unresolved);
    }

    let summaries = task
        .reuse_decision
        .as_ref()
        .and_then(|decision| decision.get("sourceEffects"))
        .and_then(Value::as_array)
        .ok_or_else(|| "sourceEffects is missing or is not an array".to_owned())?;
    let mut unresolved = Vec::new();
    for summary in summaries {
        let effect_id = summary
            .get("effectId")
            .and_then(Value::as_str)
            .ok_or_else(|| "source effect summary has no effectId".to_owned())?;
        let status = summary
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("source effect `{effect_id}` has no status"))?;
        let reconciliation = store
            .latest_effect_reconciliation(effect_id)
            .map_err(|error| error.to_string())?;
        if reconciliation
            .as_ref()
            .is_some_and(|record| record.status == ReconciliationStatus::Compensated)
        {
            unresolved.push(effect_id.to_owned());
            continue;
        }
        match status {
            "started" | "uncertain"
                if !reconciliation
                    .as_ref()
                    .is_some_and(|record| record.status == ReconciliationStatus::Applied) =>
            {
                unresolved.push(effect_id.to_owned());
            }
            "started" | "uncertain" => {}
            "requested" | "waiting_for_approval" | "succeeded" | "failed" | "cancelled" => {}
            other => {
                return Err(format!(
                    "source effect `{effect_id}` has unsupported status `{other}`"
                ));
            }
        }
    }
    Ok(unresolved)
}

fn task_output_schema(workflow: &Workflow, task: &agentctl_core::CompiledTask) -> Option<Value> {
    task.output_schema.clone().or_else(|| match &task.uses {
        TaskUse::Agent(name) => workflow
            .spec
            .agents
            .get(name)
            .and_then(|agent| agent.structured_output.clone()),
        TaskUse::Action(_) => Some(serde_json::json!({"type": "object"})),
    })
}

fn validate_output_contract(schema: &Value, output: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema).map_err(|error| error.to_string())?;
    validator
        .validate(output)
        .map_err(|error| error.to_string())
}

fn task_definition_fingerprint(
    workflow: &Workflow,
    task: &agentctl_core::CompiledTask,
    policy: &PolicyEngine,
    recorded_effects: Option<&[EffectRecord]>,
) -> Result<String, RuntimeError> {
    let execution = match &task.uses {
        TaskUse::Action(name) => serde_json::json!({
            "kind": "action",
            "task": task,
            "action": workflow.spec.actions.get(name),
        }),
        TaskUse::Agent(name) => {
            let agent = workflow.spec.agents.get(name).ok_or_else(|| {
                RuntimeError::InvalidState(format!("agent `{name}` disappeared after compile"))
            })?;
            let provider = workflow
                .spec
                .providers
                .get(&agent.provider)
                .ok_or_else(|| {
                    RuntimeError::InvalidState(format!(
                        "provider `{}` disappeared after compile",
                        agent.provider
                    ))
                })?;
            let tools = agent
                .tools
                .iter()
                .map(|tool| {
                    workflow
                        .spec
                        .tools
                        .get(tool)
                        .map(|definition| (tool.clone(), definition.clone()))
                        .ok_or_else(|| {
                            RuntimeError::InvalidState(format!(
                                "tool `{tool}` disappeared after compile"
                            ))
                        })
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            let instruction_content_digest = if let Some(path) = &agent.instructions_file {
                let recorded = recorded_effects.and_then(|effects| {
                    effects
                        .iter()
                        .rev()
                        .find(|effect| {
                            effect.request.task_id == task.id
                                && effect.request.operation == "agent.instructions.read"
                                && effect.status == EffectStatus::Succeeded
                                && effect.confirmed
                        })
                        .and_then(|effect| effect.result.as_ref())
                        .and_then(|result| result.get("content"))
                        .and_then(Value::as_str)
                });
                let content = recorded.map(ToOwned::to_owned).map_or_else(
                    || read_bounded_text_sync(&policy.resolve_read_path(path)?),
                    Ok,
                )?;
                Some(format!("sha256:{}", digest(content.as_bytes())))
            } else {
                None
            };
            serde_json::json!({
                "kind": "agent",
                "task": task,
                "agent": agent,
                "provider": provider,
                "tools": tools,
                "instructionContentDigest": instruction_content_digest,
            })
        }
    };
    versioned_json_digest(&serde_json::json!({
        "formatVersion": 1,
        "execution": execution,
        "policy": workflow.spec.policy,
        "packs": workflow.spec.packs,
    }))
}

fn resolved_input_digest(
    inputs: &serde_json::Map<String, Value>,
    memory: &Value,
    outputs: &BTreeMap<String, Value>,
    task: &agentctl_core::CompiledTask,
) -> Result<String, RuntimeError> {
    let memory_object = memory
        .as_object()
        .ok_or_else(|| RuntimeError::InvalidState("working memory must be an object".to_owned()))?;
    let mut context = EvalContext {
        inputs: inputs
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        vars: BTreeMap::new(),
        memory: memory_object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        tasks: outputs.clone(),
    };
    context.vars = task
        .vars
        .iter()
        .map(|(name, value)| render(value, &context).map(|value| (name.clone(), value)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let input = render(&serde_json::to_value(&task.input)?, &context)?;
    let dependencies = task
        .needs
        .iter()
        .filter_map(|dependency| {
            outputs
                .get(dependency)
                .map(|output| (dependency.clone(), output.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    versioned_json_digest(&serde_json::json!({
        "formatVersion": 1,
        "input": input,
        "vars": context.vars,
        "workingMemory": memory,
        "dependencies": dependencies,
    }))
}

fn state_delta(before: &Value, after: Option<&Value>) -> Result<Value, RuntimeError> {
    let before = before
        .as_object()
        .ok_or_else(|| RuntimeError::InvalidState("working memory must be an object".to_owned()))?;
    let after = match after {
        Some(after) => after.as_object().ok_or_else(|| {
            RuntimeError::InvalidState("working memory must be an object".to_owned())
        })?,
        None => before,
    };
    let set = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let remove = before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "formatVersion": 1,
        "set": set,
        "remove": remove,
    }))
}

fn apply_state_delta(memory: &mut Value, delta: &Value) -> Result<(), RuntimeError> {
    if delta.get("formatVersion").and_then(Value::as_u64) != Some(1) {
        return Err(RuntimeError::InvalidState(
            "unsupported task state-delta format".to_owned(),
        ));
    }
    let object = memory
        .as_object_mut()
        .ok_or_else(|| RuntimeError::InvalidState("working memory must be an object".to_owned()))?;
    let set = delta
        .get("set")
        .and_then(Value::as_object)
        .ok_or_else(|| RuntimeError::InvalidState("state delta has no set object".to_owned()))?;
    for (key, value) in set {
        object.insert(key.clone(), value.clone());
    }
    let remove = delta
        .get("remove")
        .and_then(Value::as_array)
        .ok_or_else(|| RuntimeError::InvalidState("state delta has no remove array".to_owned()))?;
    for key in remove {
        let key = key.as_str().ok_or_else(|| {
            RuntimeError::InvalidState("state delta key is not a string".to_owned())
        })?;
        object.remove(key);
    }
    Ok(())
}

fn versioned_json_digest(value: &Value) -> Result<String, RuntimeError> {
    let canonical = canonical_json(value);
    Ok(format!(
        "sha256:v1:{}",
        digest(&serde_json::to_vec(&canonical)?)
    ))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_legacy_task_metadata(
    workflow: &Workflow,
    compiled: &agentctl_core::CompiledTask,
    task: &TaskRecord,
    policy: &PolicyEngine,
    inputs: &serde_json::Map<String, Value>,
    outputs: &BTreeMap<String, Value>,
    effects: &[EffectRecord],
    checkpoints: &[CheckpointRecord],
) -> (
    Option<TaskCompletionMetadata>,
    BTreeMap<String, String>,
    Vec<String>,
) {
    let mut provenance = BTreeMap::new();
    let mut reasons = Vec::new();

    let definition_fingerprint = match task_definition_fingerprint(
        workflow,
        compiled,
        policy,
        Some(effects),
    ) {
        Ok(fingerprint) => {
            provenance.insert(
                    "definitionFingerprint".to_owned(),
                    "stored workflow, compiled plan, policy, tools, provider, and recorded instruction read"
                        .to_owned(),
                );
            Some(fingerprint)
        }
        Err(error) => {
            reasons.push(format!("definition fingerprint cannot be proven: {error}"));
            None
        }
    };

    let contract = task_output_schema(workflow, compiled).unwrap_or_else(|| serde_json::json!({}));
    let output_contract_fingerprint = match versioned_json_digest(&contract) {
        Ok(fingerprint) => {
            provenance.insert(
                "outputContractFingerprint".to_owned(),
                "stored workflow task/agent output schema".to_owned(),
            );
            Some(fingerprint)
        }
        Err(error) => {
            reasons.push(format!("output contract cannot be hashed: {error}"));
            None
        }
    };

    let output_digest = match task.output.as_ref() {
        Some(output) => {
            if let Err(error) = validate_output_contract(&contract, output) {
                reasons.push(format!(
                    "stored successful output does not satisfy its contract: {error}"
                ));
                None
            } else {
                match versioned_json_digest(output) {
                    Ok(digest) => {
                        provenance.insert(
                            "outputDigest".to_owned(),
                            "stored successful task output".to_owned(),
                        );
                        Some(digest)
                    }
                    Err(error) => {
                        reasons.push(format!("stored output cannot be hashed: {error}"));
                        None
                    }
                }
            }
        }
        None => {
            reasons.push("successful task has no stored output".to_owned());
            None
        }
    };

    let boundary = match legacy_checkpoint_boundary(checkpoints, &task.task_id) {
        Ok(boundary) => {
            provenance.insert(
                "checkpointBoundary".to_owned(),
                format!(
                    "checksummed checkpoints {} and {} around the successful transition",
                    boundary.0, boundary.1
                ),
            );
            Some(boundary)
        }
        Err(error) => {
            reasons.push(error);
            None
        }
    };
    let input_digest = boundary.as_ref().and_then(|boundary| {
        match resolved_input_digest(inputs, &boundary.2, outputs, compiled) {
            Ok(digest) => {
                provenance.insert(
                    "inputDigest".to_owned(),
                    "stored inputs, dependency outputs, task variables, and pre-task checkpoint memory"
                        .to_owned(),
                );
                Some(digest)
            }
            Err(error) => {
                reasons.push(format!("resolved input boundary cannot be proven: {error}"));
                None
            }
        }
    });
    let state_delta =
        boundary.as_ref().and_then(
            |boundary| match state_delta(&boundary.2, Some(&boundary.3)) {
                Ok(delta) => {
                    provenance.insert(
                        "stateDelta".to_owned(),
                        "difference between checksummed pre/post-task working memory".to_owned(),
                    );
                    Some(delta)
                }
                Err(error) => {
                    reasons.push(format!("state delta cannot be reconstructed: {error}"));
                    None
                }
            },
        );
    let state_delta_digest =
        state_delta
            .as_ref()
            .and_then(|delta| match versioned_json_digest(delta) {
                Ok(digest) => Some(digest),
                Err(error) => {
                    reasons.push(format!("state delta cannot be hashed: {error}"));
                    None
                }
            });

    let artifact_manifest = match analyze_legacy_artifacts(task, effects, policy) {
        Ok(artifacts) => {
            provenance.insert(
                "artifactManifest".to_owned(),
                if task.artifact_manifest.is_empty() {
                    "confirmed workspace-mutation effects plus current policy-authorized bytes"
                        .to_owned()
                } else {
                    "legacy manifest identity plus current policy-authorized bytes".to_owned()
                },
            );
            Some(artifacts)
        }
        Err(error) => {
            reasons.push(format!("artifact manifest cannot be proven: {error}"));
            None
        }
    };

    let metadata = match (
        definition_fingerprint,
        input_digest,
        output_contract_fingerprint,
        output_digest,
        state_delta,
        state_delta_digest,
        artifact_manifest,
    ) {
        (
            Some(definition_fingerprint),
            Some(input_digest),
            Some(output_contract_fingerprint),
            Some(output_digest),
            Some(state_delta),
            Some(state_delta_digest),
            Some(artifact_manifest),
        ) if reasons.is_empty() => Some(TaskCompletionMetadata {
            execution: TaskExecutionMetadata {
                metadata_version: TASK_METADATA_VERSION,
                definition_fingerprint,
                input_digest,
                output_contract_fingerprint,
            },
            output_digest,
            state_delta,
            state_delta_digest,
            artifact_manifest,
        }),
        _ => None,
    };
    (metadata, provenance, reasons)
}

fn legacy_checkpoint_boundary(
    checkpoints: &[CheckpointRecord],
    task_id: &str,
) -> Result<(i64, i64, Value, Value), String> {
    for pair in checkpoints.windows(2) {
        let before_state = checkpoint_task_state(&pair[0].state, task_id);
        let after_state = checkpoint_task_state(&pair[1].state, task_id);
        if before_state.as_deref() != Some("succeeded")
            && after_state.as_deref() == Some("succeeded")
        {
            let before_memory = pair[0]
                .state
                .get("workingMemory")
                .filter(|memory| memory.is_object())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "checkpoint {} has no object workingMemory before task `{task_id}`",
                        pair[0].sequence
                    )
                })?;
            let after_memory = pair[1]
                .state
                .get("workingMemory")
                .filter(|memory| memory.is_object())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "checkpoint {} has no object workingMemory after task `{task_id}`",
                        pair[1].sequence
                    )
                })?;
            return Ok((
                pair[0].sequence,
                pair[1].sequence,
                before_memory,
                after_memory,
            ));
        }
    }
    Err(format!(
        "no consecutive checksummed checkpoints prove the successful transition for task `{task_id}`"
    ))
}

fn checkpoint_task_state(checkpoint: &Value, task_id: &str) -> Option<String> {
    checkpoint
        .get("tasks")
        .and_then(Value::as_array)?
        .iter()
        .find(|task| task.get("taskId").and_then(Value::as_str) == Some(task_id))?
        .get("state")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn analyze_legacy_artifacts(
    task: &TaskRecord,
    effects: &[EffectRecord],
    policy: &PolicyEngine,
) -> Result<Vec<ArtifactRecord>, String> {
    let mut expected = task
        .artifact_manifest
        .iter()
        .map(|artifact| (artifact.path.clone(), Some(artifact)))
        .collect::<BTreeMap<_, _>>();
    let mutations = effects
        .iter()
        .filter(|effect| effect.request.effect_class == EffectClass::WorkspaceMutate)
        .collect::<Vec<_>>();
    if expected.is_empty() {
        for effect in &mutations {
            if effect.status != EffectStatus::Succeeded || !effect.confirmed {
                return Err(format!(
                    "workspace mutation `{}` is not a confirmed success",
                    effect.request.id
                ));
            }
            let result = effect.result.as_ref().ok_or_else(|| {
                format!(
                    "confirmed workspace mutation `{}` has no stored result",
                    effect.request.id
                )
            })?;
            let mut paths = BTreeSet::new();
            collect_result_paths(result, &mut paths);
            if paths.is_empty() {
                return Err(format!(
                    "confirmed workspace mutation `{}` has no stored output path",
                    effect.request.id
                ));
            }
            for path in paths {
                expected.insert(path, None);
            }
        }
    }

    expected
        .into_iter()
        .map(|(path, legacy)| {
            let resolved = policy
                .resolve_artifact_path(&path)
                .map_err(|error| error.to_string())?;
            let (digest, size_bytes) = hash_bounded_artifact(&resolved)?;
            if let Some(legacy) = legacy
                && (legacy.digest != digest || legacy.size_bytes != size_bytes)
            {
                return Err(format!(
                    "legacy artifact `{path}` identity changed: expected {} bytes and `{}`, found {} bytes and `{digest}`",
                    legacy.size_bytes, legacy.digest, size_bytes
                ));
            }
            let logical_name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("artifact `{path}` has no valid logical name"))?;
            Ok(ArtifactRecord {
                path: path.clone(),
                digest,
                size_bytes,
                media_type: legacy
                    .map(|artifact| artifact.media_type.clone())
                    .filter(|media_type| !media_type.is_empty())
                    .unwrap_or_else(|| legacy_media_type(Path::new(&path)).to_owned()),
                logical_name: legacy
                    .map(|artifact| artifact.logical_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| logical_name.to_owned()),
                store_path: legacy
                    .map(|artifact| artifact.store_path.clone())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn hash_bounded_artifact(path: &Path) -> Result<(String, u64), String> {
    use std::io::Read as _;

    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "{} exceeds the {} byte artifact limit",
            path.display(),
            MAX_ARTIFACT_BYTES
        ));
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut reader = file.take(MAX_ARTIFACT_BYTES + 1);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "{} changed while reading and exceeded the {} byte artifact limit",
            path.display(),
            MAX_ARTIFACT_BYTES
        ));
    }
    Ok((
        format!("sha256:{}", digest(&bytes)),
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    ))
}

fn legacy_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "application/json",
        Some("yaml" | "yml") => "application/yaml",
        Some("md") => "text/markdown",
        Some("txt" | "log" | "csv") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn earliest_safe_repair_roots(plan: &CompiledPlan, unavailable: &[String]) -> Vec<String> {
    let unavailable = unavailable.iter().cloned().collect::<BTreeSet<_>>();
    if unavailable.is_empty() {
        return Vec::new();
    }
    let earliest = plan
        .order
        .iter()
        .position(|task_id| unavailable.contains(task_id))
        .unwrap_or(plan.order.len());
    let mut covered = BTreeSet::new();
    let mut roots = Vec::new();
    for task_id in plan.order.iter().skip(earliest) {
        if !covered.contains(task_id) {
            roots.push(task_id.clone());
            covered.insert(task_id.clone());
            loop {
                let before = covered.len();
                for candidate in &plan.order {
                    if plan
                        .tasks
                        .get(candidate)
                        .is_some_and(|task| task.needs.iter().any(|need| covered.contains(need)))
                    {
                        covered.insert(candidate.clone());
                    }
                }
                if covered.len() == before {
                    break;
                }
            }
        }
    }
    roots
}

fn read_bounded_text_sync(path: &Path) -> Result<String, RuntimeError> {
    use std::io::Read as _;

    let file = std::fs::File::open(path)?;
    let mut reader = file.take(MAX_WORKSPACE_FILE_BYTES + 1);
    let mut content = String::new();
    reader.read_to_string(&mut content)?;
    if content.len() as u64 > MAX_WORKSPACE_FILE_BYTES {
        return Err(RuntimeError::InvalidState(format!(
            "file {} exceeds {MAX_WORKSPACE_FILE_BYTES} bytes",
            path.display()
        )));
    }
    Ok(content)
}

fn collect_artifacts(
    store: &SqliteStore,
    policy: &PolicyEngine,
    effects: &[EffectRecord],
    run_id: &str,
    task_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<ArtifactRecord>, RuntimeError> {
    let mut paths = BTreeSet::new();
    for effect in effects.iter().filter(|effect| {
        effect.request.task_id == task_id
            && effect.request.effect_class == EffectClass::WorkspaceMutate
            && effect.status == EffectStatus::Succeeded
            && effect.confirmed
    }) {
        if let Some(result) = &effect.result {
            collect_result_paths(result, &mut paths);
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let resolved = policy.resolve_artifact_path(&path)?;
            store
                .ingest_artifact(run_id, task_id, &resolved, &path, 16 * 1024 * 1024, now)
                .map_err(RuntimeError::from)
        })
        .collect()
}

fn collect_result_paths(value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(path) = object.get("path").and_then(Value::as_str) {
                paths.insert(path.to_owned());
            }
            for value in object.values() {
                collect_result_paths(value, paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_result_paths(value, paths);
            }
        }
        _ => {}
    }
}

fn verify_artifacts(store: &SqliteStore, artifacts: &[ArtifactRecord]) -> Result<(), String> {
    for artifact in artifacts {
        let restoration = format!(
            "restore content-addressed blob `{}` for logical artifact `{}` with size {} bytes, import the legacy artifact, or select its producer as an earlier repair root",
            artifact.digest, artifact.path, artifact.size_bytes
        );
        store
            .verify_artifact_record(artifact)
            .map_err(|error| format!("{restoration}: {error}"))?;
    }
    Ok(())
}

fn next_task<'a>(
    plan: &'a CompiledPlan,
    records: &[TaskRecord],
) -> Option<&'a agentctl_core::CompiledTask> {
    plan.order.iter().find_map(|id| {
        let record = records.iter().find(|record| &record.task_id == id)?;
        if record.state.is_terminal() || record.state == TaskState::WaitingForApproval {
            return None;
        }
        let task = plan.tasks.get(id)?;
        let dependencies_terminal = task.needs.iter().all(|needed| {
            records
                .iter()
                .find(|record| &record.task_id == needed)
                .is_some_and(|record| record.state.is_terminal())
        });
        dependencies_terminal.then_some(task)
    })
}

fn ready_task_batch<'a>(
    plan: &'a CompiledPlan,
    records: &[TaskRecord],
    limit: usize,
) -> Vec<&'a agentctl_core::CompiledTask> {
    plan.order
        .iter()
        .filter_map(|id| {
            let record = records.iter().find(|record| &record.task_id == id)?;
            if !matches!(record.state, TaskState::Ready | TaskState::Running) {
                return None;
            }
            let task = plan.tasks.get(id)?;
            task.needs
                .iter()
                .all(|needed| {
                    records
                        .iter()
                        .find(|record| &record.task_id == needed)
                        .is_some_and(|record| record.state.is_terminal())
                })
                .then_some(task)
        })
        .take(limit)
        .collect()
}

fn validate_memory_delta(
    task: &agentctl_core::CompiledTask,
    delta: &Value,
) -> Result<(), RuntimeError> {
    let mut changed = delta
        .get("set")
        .and_then(Value::as_object)
        .map(|set| set.keys().cloned().collect::<BTreeSet<_>>())
        .ok_or_else(|| RuntimeError::InvalidState("state delta has no set object".to_owned()))?;
    let removed = delta
        .get("remove")
        .and_then(Value::as_array)
        .ok_or_else(|| RuntimeError::InvalidState("state delta has no remove array".to_owned()))?;
    for key in removed {
        changed.insert(
            key.as_str()
                .ok_or_else(|| {
                    RuntimeError::InvalidState("state delta key is not a string".to_owned())
                })?
                .to_owned(),
        );
    }
    let undeclared = changed
        .difference(&task.memory_writes.iter().cloned().collect())
        .cloned()
        .collect::<Vec<_>>();
    if undeclared.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::Task {
            task: task.id.clone(),
            message: format!(
                "task changed undeclared working-memory key(s): {}",
                undeclared.join(", ")
            ),
        })
    }
}

fn context_for(
    run: &agentctl_store::RunRecord,
    tasks: &[TaskRecord],
) -> Result<EvalContext, RuntimeError> {
    let inputs = run
        .inputs
        .as_object()
        .ok_or_else(|| RuntimeError::InvalidState("run inputs must be an object".to_owned()))?;
    let memory = run
        .working_memory
        .as_object()
        .ok_or_else(|| RuntimeError::InvalidState("working memory must be an object".to_owned()))?;
    let task_outputs = tasks
        .iter()
        .filter_map(|task| {
            task.output
                .clone()
                .map(|output| (task.task_id.clone(), output))
        })
        .collect::<BTreeMap<_, _>>();
    Ok(EvalContext {
        inputs: inputs
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        vars: BTreeMap::new(),
        memory: memory
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        tasks: task_outputs,
    })
}

fn collect_outputs(
    run: &agentctl_store::RunRecord,
    tasks: &[TaskRecord],
    declared: &BTreeMap<String, Value>,
) -> Result<Value, RuntimeError> {
    let outputs = tasks
        .iter()
        .filter_map(|task| {
            task.output
                .clone()
                .map(|output| (task.task_id.clone(), output))
        })
        .collect::<BTreeMap<_, _>>();
    if declared.is_empty() {
        return Ok(serde_json::to_value(outputs)?);
    }
    let mut context = context_for(run, tasks)?;
    context.tasks = outputs;
    render(&serde_json::to_value(declared)?, &context).map_err(RuntimeError::from)
}

fn required_string(input: &Value, name: &str) -> Result<String, RuntimeError> {
    input
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| RuntimeError::InvalidState(format!("input requires string `{name}`")))
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

fn redact_text(value: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
}

fn redacted_process_diagnostic(value: &[u8], secrets: &[&str]) -> String {
    const DIAGNOSTIC_PREFIX_BYTES: usize = 4 * 1024;
    if !secrets.is_empty() {
        return "[REDACTED: subprocess output omitted because secret environment values were present]"
            .to_owned();
    }
    let prefix = &value[..value.len().min(DIAGNOSTIC_PREFIX_BYTES)];
    redact_text(&String::from_utf8_lossy(prefix), secrets)
}

fn unified_diff(before: Option<&str>, after: &str) -> String {
    let before = before.unwrap_or("");
    if before == after {
        return String::new();
    }
    format!(
        "--- before\n+++ after\n-{}\n+{}",
        before.replace('\n', "\n-"),
        after.replace('\n', "\n+")
    )
}

async fn write_atomic(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agentctl-output");
    let temporary = path.with_file_name(format!(".{file_name}.agentctl.tmp"));
    let mut file = tokio::fs::File::create(&temporary).await?;
    file.write_all(content).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, path).await
}

async fn read_bounded_text(path: &Path) -> Result<String, std::io::Error> {
    let file = tokio::fs::File::open(path).await?;
    let mut reader = file.take(MAX_WORKSPACE_FILE_BYTES + 1);
    let mut content = String::new();
    reader.read_to_string(&mut content).await?;
    if content.len() as u64 > MAX_WORKSPACE_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file exceeds {MAX_WORKSPACE_FILE_BYTES} bytes"),
        ));
    }
    Ok(content)
}

fn add_usage(total: &mut Usage, current: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(current.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(current.output_tokens);
    total.reasoning_tokens = total
        .reasoning_tokens
        .saturating_add(current.reasoning_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(current.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(current.cache_write_tokens);
    total.cost_microusd = match (total.cost_microusd, current.cost_microusd) {
        (Some(total), Some(current)) => Some(total.saturating_add(current)),
        _ => None,
    };
}

const fn provider_effect_is_uncertain(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Timeout | ProviderError::Cancelled | ProviderError::Http { status: 0, .. }
    )
}

const fn retryable_error(error: &RuntimeError) -> bool {
    match error {
        RuntimeError::Provider(ProviderError::Http { retryable, .. }) => *retryable,
        RuntimeError::Io(_) => true,
        RuntimeError::Store(_)
        | RuntimeError::Policy(_)
        | RuntimeError::Template(_)
        | RuntimeError::Provider(_)
        | RuntimeError::Tool(_)
        | RuntimeError::InvalidState(_)
        | RuntimeError::Task { .. }
        | RuntimeError::RunFailed { .. }
        | RuntimeError::UncertainEffect { .. }
        | RuntimeError::ExternalEffectUncertain(_)
        | RuntimeError::Cancelled
        | RuntimeError::Json(_)
        | RuntimeError::RepairBlocked { .. }
        | RuntimeError::RetryBlocked { .. } => false,
    }
}

fn enforce_usage(
    agent: &agentctl_core::dsl::AgentDefinition,
    usage: &Usage,
    task_id: &str,
) -> Result<(), RuntimeError> {
    let Some(limit) = &agent.usage_limit else {
        return Ok(());
    };
    if limit
        .max_input_tokens
        .is_some_and(|limit| usage.input_tokens > limit)
        || limit
            .max_output_tokens
            .is_some_and(|limit| usage.output_tokens > limit)
        || limit.max_cost_usd.is_some_and(|limit| {
            usage
                .cost_microusd
                .is_some_and(|cost| cost as f64 / 1_000_000.0 > limit)
        })
    {
        Err(RuntimeError::Task {
            task: task_id.to_owned(),
            message: "agent usage limit exceeded".to_owned(),
        })
    } else {
        Ok(())
    }
}

fn task_timeout(workflow: &Workflow, task_id: &str) -> u64 {
    workflow
        .spec
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .and_then(|task| task.timeout_seconds)
        .unwrap_or(workflow.spec.runtime.default_timeout_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentctl_core::compile;
    use agentctl_core::dsl::{ApprovalRequirement, EffectClass, Idempotency, Risk, parse_workflow};
    use agentctl_core::effect::{ActionResult, ChangeStatus};
    use agentctl_core::provider::{ProviderRequest, ProviderResponse, ToolCall};
    use agentctl_core::tool::{ToolContract, ToolContractError, ToolExecutor};
    use agentctl_observability::BufferedTraceSink;
    use agentctl_store::ApprovalResolution;
    use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::sync::Notify;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        }
    }

    struct MutableClock(AtomicI64);

    impl MutableClock {
        fn new() -> Self {
            Self(AtomicI64::new(1_767_225_600))
        }

        fn advance(&self, seconds: i64) {
            self.0.fetch_add(seconds, Ordering::SeqCst);
        }
    }

    impl Clock for MutableClock {
        fn now(&self) -> DateTime<Utc> {
            DateTime::from_timestamp(self.0.load(Ordering::SeqCst), 0).unwrap_or_else(Utc::now)
        }
    }

    #[derive(Default)]
    struct SequenceIds(AtomicU64);

    impl IdGenerator for SequenceIds {
        fn next_id(&self, kind: &str) -> String {
            format!("{kind}-{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
        }
    }

    #[derive(Default)]
    struct CountingProvider(AtomicU64);

    #[async_trait]
    impl ModelProvider for CountingProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            if cancellation.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let call = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(ProviderResponse {
                response_id: Some(format!("fake-{call}")),
                text: format!("answer-{call}"),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text {
                    text: format!("answer-{call}"),
                }],
                continuation: None,
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..Usage::default()
                },
                finish_reason: FinishReason::Complete,
            })
        }
    }

    struct OverlapProvider {
        active: AtomicUsize,
        peak: AtomicUsize,
        delay: Duration,
    }

    impl OverlapProvider {
        fn new(delay: Duration) -> Self {
            Self {
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                delay,
            }
        }
    }

    #[async_trait]
    impl ModelProvider for OverlapProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            request: &ProviderRequest,
            cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            tokio::select! {
                () = tokio::time::sleep(self.delay) => {}
                () = cancellation.cancelled() => {
                    self.active.fetch_sub(1, Ordering::SeqCst);
                    return Err(ProviderError::Cancelled);
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            let text = request_text(request);
            Ok(ProviderResponse {
                response_id: Some(format!("overlap-{text}")),
                text: text.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text }],
                continuation: None,
                usage: Usage::default(),
                finish_reason: FinishReason::Complete,
            })
        }
    }

    #[derive(Default)]
    struct CancellationProvider {
        started: AtomicUsize,
        notify: Notify,
    }

    #[async_trait]
    impl ModelProvider for CancellationProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_waiters();
            cancellation.cancelled().await;
            Err(ProviderError::Cancelled)
        }
    }

    struct FailureSiblingProvider;

    #[async_trait]
    impl ModelProvider for FailureSiblingProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            request: &ProviderRequest,
            cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let text = request_text(request);
            if text == "fail" {
                return Err(ProviderError::Malformed(
                    "deterministic parallel failure".to_owned(),
                ));
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(40)) => {}
                () = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            }
            Ok(ProviderResponse {
                response_id: Some("sibling-success".to_owned()),
                text: text.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text }],
                continuation: None,
                usage: Usage::default(),
                finish_reason: FinishReason::Complete,
            })
        }
    }

    #[derive(Default)]
    struct ParallelRetryProvider {
        broken_calls: AtomicUsize,
        sibling_calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for ParallelRetryProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let prompt = request_text(request);
            if prompt == "broken" && self.broken_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ProviderError::Malformed(
                    "first parallel attempt fails".to_owned(),
                ));
            }
            if prompt == "sibling" {
                let prior = self.sibling_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(prior, 0, "successful sibling must be reused by retry");
            }
            Ok(ProviderResponse {
                response_id: Some(format!("parallel-retry-{prompt}")),
                text: prompt.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text: prompt }],
                continuation: None,
                usage: Usage::default(),
                finish_reason: FinishReason::Complete,
            })
        }
    }

    fn request_text(request: &ProviderRequest) -> String {
        match request.messages.first() {
            Some(Message::User(content)) => content
                .first()
                .and_then(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    struct PromptEchoProvider;

    #[async_trait]
    impl ModelProvider for PromptEchoProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let text = match request.messages.first() {
                Some(Message::User(content)) => content
                    .first()
                    .and_then(|block| match block {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default(),
                _ => String::new(),
            };
            Ok(ProviderResponse {
                response_id: Some("prompt-echo".to_owned()),
                text: text.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text }],
                continuation: None,
                usage: Usage::default(),
                finish_reason: FinishReason::Complete,
            })
        }
    }

    #[derive(Default)]
    struct TerminalRetryProvider(AtomicU64);

    #[async_trait]
    impl ModelProvider for TerminalRetryProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ProviderError::Malformed(
                    "transient terminal retry fixture".to_owned(),
                ));
            }
            let text = r#"{"value":"recovered"}"#.to_owned();
            Ok(ProviderResponse {
                response_id: Some("retry-recovered".to_owned()),
                text: text.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text }],
                continuation: None,
                usage: Usage::default(),
                finish_reason: FinishReason::Complete,
            })
        }
    }

    #[derive(Default)]
    struct SelectiveRepairProvider {
        first_calls: AtomicU64,
        second_calls: AtomicU64,
    }

    #[async_trait]
    impl ModelProvider for SelectiveRepairProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let prompt = match request.messages.first() {
                Some(Message::User(content)) => content
                    .first()
                    .and_then(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or_default(),
                _ => "",
            };
            let text = if prompt == "produce durable output" {
                assert_eq!(
                    self.first_calls.fetch_add(1, Ordering::SeqCst),
                    0,
                    "reused upstream provider must not execute during repair"
                );
                r#"{"value":"durable"}"#.to_owned()
            } else if prompt.starts_with("broken") {
                self.second_calls.fetch_add(1, Ordering::SeqCst);
                "not-json".to_owned()
            } else {
                assert_eq!(prompt, "fixed durable");
                self.second_calls.fetch_add(1, Ordering::SeqCst);
                r#"{"received":"durable"}"#.to_owned()
            };
            Ok(ProviderResponse {
                response_id: Some("repair-test".to_owned()),
                text: text.clone(),
                tool_calls: Vec::new(),
                assistant_content: vec![ContentBlock::Text { text }],
                continuation: None,
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..Usage::default()
                },
                finish_reason: FinishReason::Complete,
            })
        }
    }

    struct RetryableThenCancelProvider;

    #[async_trait]
    impl ModelProvider for RetryableThenCancelProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            cancellation.cancel();
            Err(ProviderError::Http {
                status: 503,
                message: "retry later".to_owned(),
                request_id: "retry-cancel".to_owned(),
                retryable: true,
            })
        }
    }

    #[derive(Default)]
    struct ToolCallingProvider(AtomicU64);

    #[async_trait]
    impl ModelProvider for ToolCallingProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ProviderResponse {
                    response_id: Some("tool-turn".to_owned()),
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_owned(),
                        name: "echo".to_owned(),
                        input: serde_json::json!({"text": "hello"}),
                    }],
                    assistant_content: vec![ContentBlock::ToolCall {
                        id: "call-1".to_owned(),
                        name: "echo".to_owned(),
                        input: serde_json::json!({"text": "hello"}),
                        provider_metadata: None,
                    }],
                    continuation: None,
                    usage: Usage::default(),
                    finish_reason: FinishReason::ToolCalls,
                })
            } else {
                Ok(ProviderResponse {
                    response_id: Some("final-turn".to_owned()),
                    text: "done".to_owned(),
                    tool_calls: Vec::new(),
                    assistant_content: vec![ContentBlock::Text {
                        text: "done".to_owned(),
                    }],
                    continuation: None,
                    usage: Usage::default(),
                    finish_reason: FinishReason::Complete,
                })
            }
        }
    }

    #[derive(Default)]
    struct RepairToolCallingProvider(AtomicU64);

    #[async_trait]
    impl ModelProvider for RepairToolCallingProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            let call = self.0.fetch_add(1, Ordering::SeqCst);
            if call % 2 == 0 {
                Ok(ProviderResponse {
                    response_id: Some(format!("repair-tool-{call}")),
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("repair-call-{call}"),
                        name: "echo".to_owned(),
                        input: serde_json::json!({"text": "durable"}),
                    }],
                    assistant_content: vec![ContentBlock::ToolCall {
                        id: format!("repair-call-{call}"),
                        name: "echo".to_owned(),
                        input: serde_json::json!({"text": "durable"}),
                        provider_metadata: None,
                    }],
                    continuation: None,
                    usage: Usage::default(),
                    finish_reason: FinishReason::ToolCalls,
                })
            } else {
                let text = r#"{"value":"durable"}"#.to_owned();
                Ok(ProviderResponse {
                    response_id: Some(format!("repair-final-{call}")),
                    text: text.clone(),
                    tool_calls: Vec::new(),
                    assistant_content: vec![ContentBlock::Text { text }],
                    continuation: None,
                    usage: Usage::default(),
                    finish_reason: FinishReason::Complete,
                })
            }
        }
    }

    struct PanicProvider;

    #[async_trait]
    impl ModelProvider for PanicProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn complete(
            &self,
            _request: &ProviderRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProviderResponse, ProviderError> {
            panic!("provider executor must not run during recorded replay")
        }
    }

    struct FixtureTool {
        contract: ToolContract,
        malformed: bool,
        delay: Duration,
    }

    impl FixtureTool {
        fn new(malformed: bool) -> Self {
            Self {
                contract: ToolContract {
                    id: "echo".to_owned(),
                    description: "echo input".to_owned(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"],
                        "additionalProperties": false
                    }),
                    output_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"],
                        "additionalProperties": false
                    }),
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
                },
                malformed,
                delay: Duration::ZERO,
            }
        }

        fn delayed(mut self, delay: Duration, timeout_seconds: u64) -> Self {
            self.delay = delay;
            self.contract.timeout_seconds = timeout_seconds;
            self
        }
    }

    #[async_trait]
    impl ToolExecutor for FixtureTool {
        fn contract(&self) -> &ToolContract {
            &self.contract
        }

        async fn execute(
            &self,
            _input: Value,
            cancellation: &CancellationToken,
        ) -> Result<ActionResult, ToolContractError> {
            if !self.delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(self.delay) => {}
                    () = cancellation.cancelled() => return Err(ToolContractError::Cancelled),
                }
            }
            Ok(ActionResult {
                status: ChangeStatus::Unchanged,
                changed: false,
                before: None,
                after: None,
                diff: None,
                output: if self.malformed {
                    Value::String("malicious success shape".to_owned())
                } else {
                    serde_json::json!({"text": "hello"})
                },
                predictability: PlanPredictability::RequiresExecution,
            })
        }
    }

    struct SingleUseRepairTool {
        inner: FixtureTool,
        calls: AtomicU64,
    }

    impl SingleUseRepairTool {
        fn new() -> Self {
            Self {
                inner: FixtureTool::new(false),
                calls: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for SingleUseRepairTool {
        fn contract(&self) -> &ToolContract {
            self.inner.contract()
        }

        async fn execute(
            &self,
            input: Value,
            cancellation: &CancellationToken,
        ) -> Result<ActionResult, ToolContractError> {
            assert_eq!(
                self.calls.fetch_add(1, Ordering::SeqCst),
                0,
                "reused upstream tool must not execute during repair"
            );
            self.inner.execute(input, cancellation).await
        }
    }

    struct PanicTool {
        contract: ToolContract,
    }

    #[async_trait]
    impl ToolExecutor for PanicTool {
        fn contract(&self) -> &ToolContract {
            &self.contract
        }

        async fn execute(
            &self,
            _input: Value,
            _cancellation: &CancellationToken,
        ) -> Result<ActionResult, ToolContractError> {
            panic!("tool executor must not run during recorded replay")
        }
    }

    struct RejectReconciliationHook;

    impl EffectReconciliationHook for RejectReconciliationHook {
        fn validate(
            &self,
            _effect: &EffectRecord,
            _evidence: &Value,
            _result: Option<&Value>,
        ) -> Result<(), String> {
            Err("external verifier did not confirm the record".to_owned())
        }
    }

    fn compile_fixture(source: &str) -> (Workflow, CompiledPlan) {
        let workflow = parse_workflow(source, "fixture.yaml")
            .expect("parse fixture")
            .workflow;
        let plan = compile(&workflow, "fixture.yaml").expect("compile fixture");
        (workflow, plan)
    }

    #[tokio::test]
    async fn parallel_scheduler_overlaps_work_caps_concurrency_and_commits_in_plan_order() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(OverlapProvider::new(Duration::from_millis(40)));
        let runtime = runtime(store.clone(), directory.path())
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider.clone()));
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-overlap }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: fake
      instructions: return the prompt
      maxTurns: 1
  tasks:
    - { id: first, uses: "agent:worker", with: { prompt: first } }
    - { id: second, uses: "agent:worker", with: { prompt: second } }
    - { id: third, uses: "agent:worker", with: { prompt: third } }
"#,
        );
        let outcome = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("parallel run");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(provider.peak.load(Ordering::SeqCst), 2);
        let tasks = store.list_tasks(&outcome.run_id).expect("tasks");
        assert_eq!(tasks[0].output.as_ref().expect("first")["text"], "first");
        assert_eq!(tasks[1].output.as_ref().expect("second")["text"], "second");
        assert_eq!(tasks[2].output.as_ref().expect("third")["text"], "third");
        let completion_order = store
            .audit_events(&outcome.run_id)
            .expect("audit")
            .into_iter()
            .filter(|event| {
                event.event_type == "task.transition" && event.payload["to"] == "succeeded"
            })
            .filter_map(|event| event.task_id)
            .collect::<Vec<_>>();
        assert_eq!(completion_order, ["first", "second", "third"]);
    }

    #[tokio::test]
    async fn parallel_memory_deltas_merge_atomically_and_replay_offline() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-memory }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  memory:
    working: { seed: kept }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - { id: left, uses: "action:remember", with: { key: left, value: one } }
    - { id: right, uses: "action:remember", with: { key: right, value: two } }
"#,
        );
        let runtime = runtime(store.clone(), directory.path());
        let outcome = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("parallel memory run");
        let source = store.load_run(&outcome.run_id).expect("source run");
        assert_eq!(
            source.working_memory,
            serde_json::json!({"seed": "kept", "left": "one", "right": "two"})
        );
        let replay = runtime
            .replay(&outcome.run_id)
            .await
            .expect("offline replay");
        let replayed = store.load_run(&replay.run_id).expect("replayed run");
        assert_eq!(replayed.working_memory, source.working_memory);
        assert!(
            store
                .list_effects(&replay.run_id)
                .expect("effects")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn parallel_dynamic_memory_write_is_rejected_before_effect_dispatch() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-write-declaration }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  inputs:
    selected: { type: string }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - id: remember
      uses: action:remember
      memoryWrites: [allowed]
      with: { key: "${{ inputs.selected }}", value: blocked }
"#,
        );
        let run_id = match runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({"selected": "undeclared"}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed {
                run_id, message, ..
            }) => {
                assert!(message.contains("not declared in memoryWrites"));
                run_id
            }
            other => panic!("expected declared-write failure, got {other:?}"),
        };
        assert!(store.list_effects(&run_id).expect("effects").is_empty());
    }

    #[tokio::test]
    async fn parallel_cancellation_propagates_to_every_running_task() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(CancellationProvider::default());
        let runtime = runtime(store.clone(), directory.path())
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider.clone()));
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-cancellation }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: fake
      instructions: wait
      maxTurns: 1
  tasks:
    - { id: first, uses: "agent:worker", with: { prompt: first } }
    - { id: second, uses: "agent:worker", with: { prompt: second } }
"#,
        );
        let cancellation = CancellationToken::new();
        let run_cancellation = cancellation.clone();
        let handle = tokio::spawn(async move {
            runtime
                .start(
                    &workflow,
                    &plan,
                    serde_json::json!({}),
                    RunOptions::default(),
                    &run_cancellation,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while provider.started.load(Ordering::SeqCst) != 2 {
                provider.notify.notified().await;
            }
        })
        .await
        .expect("both tasks started");
        cancellation.cancel();
        let outcome = handle.await.expect("join").expect("cancel outcome");
        assert_eq!(outcome.state, RunState::Cancelled);
        assert!(
            store
                .list_tasks(&outcome.run_id)
                .expect("tasks")
                .iter()
                .all(|task| task.state == TaskState::Cancelled)
        );
    }

    #[tokio::test]
    async fn stop_failure_waits_for_and_commits_successful_parallel_sibling() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default().with_provider("fake", Arc::new(FailureSiblingProvider)),
        );
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-failure }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: fake
      instructions: execute
      maxTurns: 1
  tasks:
    - { id: broken, uses: "agent:worker", with: { prompt: fail } }
    - { id: sibling, uses: "agent:worker", with: { prompt: keep } }
"#,
        );
        let run_id = match runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, task, .. }) => {
                assert_eq!(task, "broken");
                run_id
            }
            other => panic!("expected stop failure, got {other:?}"),
        };
        let tasks = store.list_tasks(&run_id).expect("tasks");
        assert_eq!(tasks[0].state, TaskState::Failed);
        assert_eq!(tasks[1].state, TaskState::Succeeded);
        assert_eq!(
            tasks[1].output.as_ref().expect("sibling output")["text"],
            "keep"
        );
        assert_eq!(
            store.load_run(&run_id).expect("run").state,
            RunState::Failed
        );
    }

    #[tokio::test]
    async fn continue_failure_preserves_independent_parallel_branch() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default().with_provider("fake", Arc::new(FailureSiblingProvider)),
        );
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-continue }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: fake
      instructions: execute
      maxTurns: 1
  tasks:
    - id: broken
      uses: agent:worker
      failure: continue
      with: { prompt: fail }
    - { id: sibling, uses: "agent:worker", with: { prompt: keep } }
"#,
        );
        let outcome = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("continue returns terminal outcome");
        assert_eq!(outcome.state, RunState::Failed);
        let tasks = store.list_tasks(&outcome.run_id).expect("tasks");
        assert_eq!(tasks[0].state, TaskState::Failed);
        assert_eq!(tasks[1].state, TaskState::Succeeded);
    }

    #[tokio::test]
    async fn parallel_approvals_pause_and_resume_all_tasks() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-approvals }
spec:
  runtime: { maxConcurrency: 2 }
  policy:
    approval: always
    nonInteractive: pause
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - { id: left, uses: "action:remember", with: { key: left, value: one } }
    - { id: right, uses: "action:remember", with: { key: right, value: two } }
"#,
        );
        let runtime = runtime(store.clone(), directory.path());
        let paused = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("paused run");
        assert_eq!(paused.state, RunState::Paused);
        let tasks = store.list_tasks(&paused.run_id).expect("paused tasks");
        assert!(tasks.iter().all(|task| {
            task.state == TaskState::WaitingForApproval && task.execution_memory.is_some()
        }));
        let approvals = store.pending_approvals(&paused.run_id).expect("approvals");
        assert_eq!(approvals.len(), 2);
        for approval in approvals {
            store
                .resolve_approval(
                    &approval.approval_id,
                    ApprovalResolution::Approved,
                    "test",
                    "approved parallel task",
                    FixedClock.now(),
                )
                .expect("approval");
        }
        let resumed = runtime
            .resume(
                &paused.run_id,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("resume");
        assert_eq!(resumed.state, RunState::Succeeded);
        assert_eq!(
            store.load_run(&resumed.run_id).expect("run").working_memory,
            serde_json::json!({"left": "one", "right": "two"})
        );
    }

    #[tokio::test]
    async fn failed_only_retry_reuses_successful_parallel_sibling() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(ParallelRetryProvider::default());
        let runtime = runtime(store.clone(), directory.path())
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider.clone()));
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-retry }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: fake
      instructions: execute
      maxTurns: 1
  tasks:
    - { id: broken, uses: "agent:worker", with: { prompt: broken } }
    - { id: sibling, uses: "agent:worker", with: { prompt: sibling } }
"#,
        );
        let source_run_id = match runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let retry_plan = runtime
            .plan_retry(&source_run_id, &workflow, &plan, &[], true, false)
            .expect("retry plan");
        assert!(retry_plan.compatible, "{:?}", retry_plan.blocked_reuse);
        assert_eq!(retry_plan.retry_roots, ["broken"]);
        assert_eq!(retry_plan.reused_tasks, ["sibling"]);
        let outcome = runtime
            .retry(
                &workflow,
                &plan,
                retry_plan,
                Some("retry failed parallel branch"),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("retry");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(provider.broken_calls.load(Ordering::SeqCst), 2);
        assert_eq!(provider.sibling_calls.load(Ordering::SeqCst), 1);
        let retry_tasks = store.list_tasks(&outcome.run_id).expect("retry tasks");
        assert_eq!(retry_tasks[0].disposition, TaskDisposition::Executed);
        assert_eq!(retry_tasks[1].disposition, TaskDisposition::Reused);
    }

    #[tokio::test]
    async fn repair_reuses_successful_parallel_memory_branch() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-repair }
spec:
  runtime: { maxConcurrency: 2 }
  policy: { approval: never }
  actions:
    remember: { kind: builtin.memory.write }
    verify: { kind: builtin.assert }
  tasks:
    - { id: remember, uses: "action:remember", with: { key: durable, value: kept } }
    - { id: verify, uses: "action:verify", with: { that: false } }
"#;
        let target = source.replace("that: false", "that: true");
        let (source_workflow, source_plan) = compile_fixture(source);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let (target_workflow, target_plan) = compile_fixture(&target);
        let repair_plan = runtime
            .plan_repair(
                &source_run_id,
                &target_workflow,
                &target_plan,
                &["verify".to_owned()],
                false,
            )
            .expect("repair plan");
        assert!(repair_plan.compatible, "{:?}", repair_plan.blocked_reuse);
        assert_eq!(repair_plan.reused_tasks, ["remember"]);
        assert_eq!(repair_plan.rerun_tasks, ["verify"]);
        let outcome = runtime
            .repair(
                &target_workflow,
                &target_plan,
                repair_plan,
                Some("fix independent assertion"),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(
            store
                .load_run(&outcome.run_id)
                .expect("repair run")
                .working_memory,
            serde_json::json!({"durable": "kept"})
        );
        let tasks = store.list_tasks(&outcome.run_id).expect("repair tasks");
        assert_eq!(tasks[0].disposition, TaskDisposition::Reused);
        assert_eq!(tasks[1].disposition, TaskDisposition::Executed);
    }

    #[test]
    fn repair_fingerprints_include_tool_definitions_and_resolved_task_variables() {
        let directory = tempdir().expect("tempdir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: fingerprint-repair }
spec:
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: original echo
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: fake
      model: fake
      instructions: echo the selected value
      tools: [echo]
  tasks:
    - id: first
      uses: agent:worker
      vars: { selected: "${{ memory.selected }}" }
      with: { prompt: "${{ vars.selected }}" }
"#;
        let changed_tool = source.replace("original echo", "changed echo");
        let (source_workflow, source_plan) = compile_fixture(source);
        let (changed_workflow, changed_plan) = compile_fixture(&changed_tool);
        let source_policy =
            PolicyEngine::new(source_workflow.spec.policy.clone(), directory.path())
                .expect("source policy");
        let changed_policy =
            PolicyEngine::new(changed_workflow.spec.policy.clone(), directory.path())
                .expect("changed policy");
        let source_task = &source_plan.tasks["first"];
        let changed_task = &changed_plan.tasks["first"];
        assert_ne!(
            task_definition_fingerprint(&source_workflow, source_task, &source_policy, None)
                .expect("source fingerprint"),
            task_definition_fingerprint(&changed_workflow, changed_task, &changed_policy, None)
                .expect("changed fingerprint")
        );

        let inputs = serde_json::Map::new();
        let outputs = BTreeMap::new();
        assert_ne!(
            resolved_input_digest(
                &inputs,
                &serde_json::json!({"selected": "one"}),
                &outputs,
                source_task
            )
            .expect("first input digest"),
            resolved_input_digest(
                &inputs,
                &serde_json::json!({"selected": "two"}),
                &outputs,
                source_task
            )
            .expect("second input digest")
        );
    }

    fn runtime(store: SqliteStore, base: &Path) -> Runtime {
        Runtime::new(store, base)
            .with_clock(Arc::new(FixedClock))
            .with_ids(Arc::new(SequenceIds::default()))
    }

    fn running_memory_effect(
        store: &SqliteStore,
        base: &Path,
        run_id: &str,
        uncertain: bool,
    ) -> EffectRequest {
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: reconciled-resume }
spec:
  policy: { approval: never }
  memory:
    working: { value: old }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - id: remember
      uses: action:remember
      with: { key: value, value: new }
"#;
        let (workflow, plan) = compile_fixture(source);
        store
            .create_run(
                run_id,
                API_VERSION,
                &serde_json::to_value(workflow).expect("workflow"),
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({"value": "old"}),
                RunMode::Execute,
                None,
                base,
                FixedClock.now(),
                "trace-source",
            )
            .expect("create interrupted run");
        store
            .transition_task(
                run_id,
                "remember",
                TaskState::Ready,
                None,
                None,
                None,
                FixedClock.now(),
                "trace-source",
            )
            .expect("ready");
        store
            .transition_task(
                run_id,
                "remember",
                TaskState::Running,
                None,
                None,
                None,
                FixedClock.now(),
                "trace-source",
            )
            .expect("running");
        let effect = EffectRequest::new(
            run_id,
            "remember",
            1,
            1,
            "builtin.memory.write",
            EffectClass::InternalState,
            Risk::Low,
            Idempotency::Keyed,
            serde_json::json!({"key": "value", "value": "new"}),
            "update transactional run working memory",
            "trace-source",
        );
        store
            .record_effect_request(&effect, FixedClock.now())
            .expect("effect");
        store
            .mark_effect_started(&effect.id, FixedClock.now())
            .expect("started");
        if uncertain {
            store
                .mark_effect_uncertain(&effect.id, "interrupted", FixedClock.now())
                .expect("uncertain");
        } else {
            store
                .complete_effect(
                    &effect.id,
                    Ok(&serde_json::json!({
                        "status": "changed",
                        "changed": true,
                        "before": "old",
                        "after": "new",
                        "key": "value",
                    })),
                    FixedClock.now(),
                )
                .expect("complete");
        }
        effect
    }

    #[tokio::test]
    async fn applied_reconciliation_supplies_a_validated_result_to_resume() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let effect = running_memory_effect(&store, directory.path(), "resume-applied", true);
        let runtime = runtime(store.clone(), directory.path());
        let result = serde_json::json!({
            "status": "changed",
            "changed": true,
            "before": "old",
            "after": "new",
            "key": "value",
        });
        let reconciliation = runtime
            .reconcile_effect(EffectReconciliationInput {
                effect_id: effect.id.clone(),
                status: ReconciliationStatus::Applied,
                actor: "operator".to_owned(),
                reason: "checkpoint confirms the memory write".to_owned(),
                evidence: serde_json::json!({"checkpoint": "external-1"}),
                result: Some(result),
                result_schema: Some(serde_json::json!({
                    "type": "object",
                    "required": ["status", "changed", "key"],
                })),
                compensation_effect_id: None,
                approved: false,
            })
            .expect("reconcile applied");
        assert_eq!(reconciliation.status, ReconciliationStatus::Applied);
        let outcome = runtime
            .resume(
                "resume-applied",
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("resume");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(
            store
                .load_run("resume-applied")
                .expect("run")
                .working_memory["value"],
            "new"
        );
        assert_eq!(
            store.list_effects("resume-applied").expect("effects").len(),
            1
        );
        assert_eq!(
            store.load_effect(&effect.id).expect("source effect").status,
            EffectStatus::Uncertain
        );
    }

    #[tokio::test]
    async fn not_applied_reconciliation_resumes_with_a_fresh_task_and_effect_attempt() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let effect = running_memory_effect(&store, directory.path(), "resume-not-applied", true);
        let runtime = runtime(store.clone(), directory.path());
        runtime
            .reconcile_effect(EffectReconciliationInput {
                effect_id: effect.id.clone(),
                status: ReconciliationStatus::NotApplied,
                actor: "operator".to_owned(),
                reason: "checkpoint confirms no write".to_owned(),
                evidence: serde_json::json!({"checkpoint": "external-2"}),
                result: None,
                result_schema: None,
                compensation_effect_id: None,
                approved: false,
            })
            .expect("reconcile not applied");
        let outcome = runtime
            .resume(
                "resume-not-applied",
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("resume");
        assert_eq!(outcome.state, RunState::Succeeded);
        let task = store
            .list_tasks("resume-not-applied")
            .expect("tasks")
            .remove(0);
        assert_eq!(task.attempt, 2);
        let effects = store.list_effects("resume-not-applied").expect("effects");
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[0].request.id, effect.id);
        assert_eq!(effects[0].status, EffectStatus::Uncertain);
        assert_eq!(effects[1].status, EffectStatus::Succeeded);
        assert_ne!(effects[0].request.id, effects[1].request.id);
    }

    #[tokio::test]
    async fn compensated_reconciliation_resumes_a_completed_effect_with_a_fresh_attempt() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let effect = running_memory_effect(&store, directory.path(), "resume-compensated", false);
        let compensation = EffectRequest::new(
            "resume-compensated",
            "remember",
            1,
            2,
            "builtin.memory.restore",
            EffectClass::InternalState,
            Risk::Low,
            Idempotency::Keyed,
            serde_json::json!({"key": "value", "value": "old"}),
            "restore transactional run working memory",
            "trace-source",
        );
        store
            .record_effect_request(&compensation, FixedClock.now())
            .expect("compensation");
        store
            .mark_effect_started(&compensation.id, FixedClock.now())
            .expect("compensation started");
        store
            .complete_effect(
                &compensation.id,
                Ok(&serde_json::json!({"restored": true})),
                FixedClock.now(),
            )
            .expect("compensation complete");
        let runtime = runtime(store.clone(), directory.path());
        runtime
            .reconcile_effect(EffectReconciliationInput {
                effect_id: effect.id.clone(),
                status: ReconciliationStatus::Compensated,
                actor: "operator".to_owned(),
                reason: "the state write was reversed".to_owned(),
                evidence: serde_json::json!({"checkpoint": "external-3"}),
                result: None,
                result_schema: None,
                compensation_effect_id: Some(compensation.id),
                approved: false,
            })
            .expect("reconcile compensated");
        let outcome = runtime
            .resume(
                "resume-compensated",
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("resume");
        assert_eq!(outcome.state, RunState::Succeeded);
        let task = store
            .list_tasks("resume-compensated")
            .expect("tasks")
            .remove(0);
        assert_eq!(task.attempt, 2);
        let effects = store.list_effects("resume-compensated").expect("effects");
        assert_eq!(effects.len(), 3);
        assert_eq!(effects[0].status, EffectStatus::Succeeded);
        assert_eq!(effects[2].status, EffectStatus::Succeeded);
        assert_ne!(effects[0].request.id, effects[2].request.id);
    }

    #[test]
    fn reconciliation_enforces_tool_contract_hook_and_policy_approval() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: reconciliation-policy }
spec:
  policy:
    approval: high_risk
    nonInteractive: pause
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: one, uses: "action:assign", with: { value: one } }
"#;
        let (workflow, plan) = compile_fixture(source);
        store
            .create_run(
                "reconciliation-policy",
                API_VERSION,
                &serde_json::to_value(workflow).expect("workflow"),
                &plan,
                &serde_json::json!({}),
                &serde_json::json!({}),
                RunMode::Execute,
                None,
                directory.path(),
                FixedClock.now(),
                "trace",
            )
            .expect("run");
        let effect = EffectRequest::new(
            "reconciliation-policy",
            "one",
            1,
            1,
            "tool.echo",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"text": "hello"}),
            "external echo",
            "trace",
        );
        store
            .record_effect_request(&effect, FixedClock.now())
            .expect("effect");
        store
            .mark_effect_started(&effect.id, FixedClock.now())
            .expect("started");
        store
            .mark_effect_uncertain(&effect.id, "unknown", FixedClock.now())
            .expect("uncertain");
        let runtime_instance = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default().with_tool("echo", Arc::new(FixtureTool::new(false))),
        );
        let base = EffectReconciliationInput {
            effect_id: effect.id.clone(),
            status: ReconciliationStatus::Applied,
            actor: "operator".to_owned(),
            reason: "external verifier confirms output".to_owned(),
            evidence: serde_json::json!({"externalId": "echo-1"}),
            result: Some(serde_json::json!({"text": "hello"})),
            result_schema: None,
            compensation_effect_id: None,
            approved: false,
        };
        let mut invalid = base.clone();
        invalid.result = Some(serde_json::json!({"wrong": true}));
        assert!(matches!(
            runtime_instance.reconcile_effect(invalid),
            Err(RuntimeError::Tool(_))
        ));
        assert!(runtime_instance.reconcile_effect(base.clone()).is_err());
        let mut approved = base;
        approved.approved = true;
        assert_eq!(
            runtime_instance
                .reconcile_effect(approved)
                .expect("approved")
                .status,
            ReconciliationStatus::Applied
        );

        let hook_effect = EffectRequest::new(
            "reconciliation-policy",
            "one",
            1,
            2,
            "external.verify",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "verify record",
            "trace",
        );
        store
            .record_effect_request(&hook_effect, FixedClock.now())
            .expect("hook effect");
        store
            .mark_effect_started(&hook_effect.id, FixedClock.now())
            .expect("hook started");
        store
            .mark_effect_uncertain(&hook_effect.id, "unknown", FixedClock.now())
            .expect("hook uncertain");
        let hooked = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default()
                .with_reconciliation_hook("external.verify", Arc::new(RejectReconciliationHook)),
        );
        assert!(matches!(
            hooked.reconcile_effect(EffectReconciliationInput {
                effect_id: hook_effect.id.clone(),
                status: ReconciliationStatus::Applied,
                actor: "operator".to_owned(),
                reason: "manual claim".to_owned(),
                evidence: serde_json::json!({"claim": true}),
                result: Some(serde_json::json!({"record": "x"})),
                result_schema: None,
                compensation_effect_id: None,
                approved: true,
            }),
            Err(RuntimeError::InvalidState(message)) if message.contains("hook")
        ));
        assert!(
            store
                .effect_reconciliations(&hook_effect.id)
                .expect("no hook reconciliation")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn selective_repair_reuses_upstream_agent_output_without_dispatch() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(SelectiveRepairProvider::default());
        let clock = Arc::new(MutableClock::new());
        let runtime = Runtime::new(store.clone(), directory.path())
            .with_clock(clock.clone())
            .with_ids(Arc::new(SequenceIds::default()))
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider.clone()));
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: selective-repair }
spec:
  providers:
    fake: { kind: fake }
  agents:
    first:
      provider: fake
      model: fake
      instructions: produce structured output
      structuredOutput:
        type: object
        required: [value]
        additionalProperties: false
        properties:
          value: { type: string }
    second:
      provider: fake
      model: fake
      instructions: consume structured output
      structuredOutput:
        type: object
        required: [received]
        additionalProperties: false
        properties:
          received: { type: string }
  tasks:
    - id: first
      uses: agent:first
      with: { prompt: produce durable output }
    - id: second
      uses: agent:second
      needs: [first]
      with: { prompt: "broken ${{ tasks.first.output.value }}" }
"#;
        let repaired_yaml = source_yaml.replace(
            r#"with: { prompt: "broken ${{ tasks.first.output.value }}" }"#,
            r#"with: { prompt: "fixed ${{ tasks.first.output.value }}" }"#,
        );
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, task, .. }) => {
                assert_eq!(task, "second");
                run_id
            }
            other => panic!("expected source failure, got {other:?}"),
        };
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("repair plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        assert_eq!(plan.reused_tasks, ["first"]);
        assert_eq!(plan.rerun_tasks, ["second"]);
        let source_before = store.load_run(&source_run_id).expect("source before");
        let source_tasks_before = store
            .list_tasks(&source_run_id)
            .expect("source tasks before");

        let invalid_consumer_yaml =
            repaired_yaml.replace("tasks.first.output.value", "tasks.first.output.missing");
        let (invalid_consumer_workflow, invalid_consumer_plan) =
            compile_fixture(&invalid_consumer_yaml);
        let invalid_consumer = runtime
            .plan_repair(
                &source_run_id,
                &invalid_consumer_workflow,
                &invalid_consumer_plan,
                &["second".to_owned()],
                false,
            )
            .expect("invalid consumer plan");
        assert!(!invalid_consumer.compatible);
        assert!(
            invalid_consumer.blocked_reuse.iter().any(|block| {
                block.task_id == "second" && block.rule == "target_input_resolution"
            })
        );

        clock.advance(3_600);
        let outcome = runtime
            .repair(
                &repaired_workflow,
                &repaired_plan,
                plan,
                Some("fix second task"),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair succeeds");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(provider.first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.second_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            outcome.output.as_ref().expect("output")["second"]["received"],
            "durable"
        );
        let repaired_tasks = store.list_tasks(&outcome.run_id).expect("tasks");
        assert_eq!(
            repaired_tasks[0].disposition,
            agentctl_store::TaskDisposition::Reused
        );
        assert_eq!(
            repaired_tasks[1].disposition,
            agentctl_store::TaskDisposition::Executed
        );
        assert!(
            repaired_tasks[0]
                .reuse_decision
                .as_ref()
                .and_then(|decision| decision.get("sourceEffects"))
                .and_then(Value::as_array)
                .is_some_and(|effects| !effects.is_empty())
        );
        assert!(
            store
                .trace_events(&outcome.run_id)
                .expect("traces")
                .iter()
                .any(|event| {
                    event.event["name"] == "task.reused" && event.event["taskId"] == "first"
                })
        );
        assert!(
            store
                .provider_sessions(&outcome.run_id)
                .expect("sessions")
                .iter()
                .all(|session| session.task_id != "first")
        );
        assert_eq!(
            store
                .list_effects(&outcome.run_id)
                .expect("effects")
                .iter()
                .filter(|effect| effect.request.task_id == "first")
                .count(),
            0
        );
        assert_eq!(
            store.load_run(&source_run_id).expect("source").state,
            RunState::Failed
        );
        assert_eq!(
            store.load_run(&source_run_id).expect("source after"),
            source_before
        );
        assert_eq!(
            store
                .list_tasks(&source_run_id)
                .expect("source tasks after"),
            source_tasks_before
        );
        let cutoff = DateTime::from_timestamp(1_767_227_400, 0).expect("cutoff");
        store.garbage_collect(cutoff).expect("source gc");
        assert!(matches!(
            store.load_run(&source_run_id),
            Err(StoreError::RunNotFound(_))
        ));
        assert_eq!(
            store
                .load_run(&outcome.run_id)
                .expect("repair survives")
                .state,
            RunState::Succeeded
        );
        let replay = runtime
            .replay(&outcome.run_id)
            .await
            .expect("offline replay");
        assert_eq!(replay.state, RunState::Succeeded);
        assert_eq!(replay.output, outcome.output);
        assert!(
            store
                .list_effects(&replay.run_id)
                .expect("replay effects")
                .is_empty()
        );
        assert!(
            store
                .provider_sessions(&replay.run_id)
                .expect("replay sessions")
                .is_empty()
        );
        assert!(
            store
                .tool_calls(&replay.run_id)
                .expect("replay tool calls")
                .is_empty()
        );
        let replay_source_plan = runtime
            .plan_repair(
                &replay.run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                true,
            )
            .expect("replay-source plan");
        assert!(!replay_source_plan.compatible);
        assert!(
            replay_source_plan
                .blocked_reuse
                .iter()
                .any(|block| { block.rule == "recorded_replay_has_no_direct_effect_history" })
        );
    }

    #[tokio::test]
    async fn repair_reuses_tool_using_upstream_without_provider_or_tool_dispatch() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(RepairToolCallingProvider::default());
        let tool = Arc::new(SingleUseRepairTool::new());
        let runtime = Runtime::new(store.clone(), directory.path())
            .with_clock(Arc::new(FixedClock))
            .with_ids(Arc::new(SequenceIds::default()))
            .with_registry(
                RuntimeRegistry::default()
                    .with_provider("fake", provider.clone())
                    .with_tool("echo", tool.clone()),
            );
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: repair-tool-reuse }
spec:
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: echo input
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    first:
      provider: fake
      model: fake
      instructions: call echo, then return structured output
      tools: [echo]
      maxTurns: 2
      maxToolCalls: 1
      structuredOutput:
        type: object
        properties: { value: { type: string } }
        required: [value]
        additionalProperties: false
  actions:
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: agent:first
      with: { prompt: produce durable output }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let target_yaml = source_yaml.replace("that: false", "that: true");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        assert_eq!(tool.calls.load(Ordering::SeqCst), 1);

        let (target_workflow, target_plan) = compile_fixture(&target_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &target_workflow,
                &target_plan,
                &["second".to_owned()],
                false,
            )
            .expect("repair plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        let repair = runtime
            .repair(
                &target_workflow,
                &target_plan,
                plan,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair");
        assert_eq!(repair.state, RunState::Succeeded);
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
        assert!(
            store
                .list_effects(&repair.run_id)
                .expect("repair effects")
                .iter()
                .all(|effect| effect.request.task_id != "first")
        );
        assert!(
            store
                .tool_calls(&repair.run_id)
                .expect("repair tool calls")
                .iter()
                .all(|call| call.task_id != "first")
        );
    }

    #[tokio::test]
    async fn repair_plan_uses_minimal_branch_closure_and_blocks_changed_upstream() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: branch-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - id: prepare
      uses: action:assign
      with: { value: stable }
    - id: analyze_a
      uses: action:assign
      needs: [prepare]
      with: { branch: a }
    - id: analyze_b
      uses: action:assert
      needs: [prepare]
      with: { that: false, message: broken }
    - id: combine
      uses: action:assign
      needs: [analyze_a, analyze_b]
      with: { result: combined }
"#;
        let repaired_yaml = source_yaml.replace(
            "with: { that: false, message: broken }",
            "with: { that: true, message: fixed }",
        );
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, task, .. }) => {
                assert_eq!(task, "analyze_b");
                run_id
            }
            other => panic!("expected source failure, got {other:?}"),
        };
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["analyze_b".to_owned()],
                false,
            )
            .expect("plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        assert_eq!(plan.reused_tasks, ["prepare", "analyze_a"]);
        assert_eq!(plan.rerun_tasks, ["analyze_b", "combine"]);
        let outcome = runtime
            .repair(
                &repaired_workflow,
                &repaired_plan,
                plan,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair");
        assert_eq!(outcome.state, RunState::Succeeded);

        let changed_upstream_yaml =
            repaired_yaml.replace("with: { value: stable }", "with: { value: changed }");
        let (changed_workflow, changed_plan) = compile_fixture(&changed_upstream_yaml);
        let blocked = runtime
            .plan_repair(
                &source_run_id,
                &changed_workflow,
                &changed_plan,
                &["analyze_b".to_owned()],
                false,
            )
            .expect("blocked plan");
        assert!(!blocked.compatible);
        assert!(blocked.blocked_reuse.iter().any(|block| {
            block.task_id == "prepare" && block.rule == "definition_fingerprint_mismatch"
        }));

        let changed_dependency_yaml = repaired_yaml.replace(
            "    - id: analyze_a\n      uses: action:assign\n      needs: [prepare]",
            "    - id: analyze_a\n      uses: action:assign",
        );
        let (dependency_workflow, dependency_plan) = compile_fixture(&changed_dependency_yaml);
        let dependency_blocked = runtime
            .plan_repair(
                &source_run_id,
                &dependency_workflow,
                &dependency_plan,
                &["analyze_b".to_owned()],
                false,
            )
            .expect("dependency plan");
        assert!(!dependency_blocked.compatible);
        assert!(dependency_blocked.blocked_reuse.iter().any(|block| {
            block.task_id == "analyze_a" && block.rule == "dependency_set_mismatch"
        }));

        let changed_contract_yaml = repaired_yaml.replace(
            "      with: { value: stable }",
            concat!(
                "      with: { value: stable }\n",
                "      outputSchema:\n",
                "        type: object\n",
                "        properties: { value: { type: integer } }\n",
                "        required: [value]\n"
            ),
        );
        let (contract_workflow, contract_plan) = compile_fixture(&changed_contract_yaml);
        let contract_blocked = runtime
            .plan_repair(
                &source_run_id,
                &contract_workflow,
                &contract_plan,
                &["analyze_b".to_owned()],
                false,
            )
            .expect("contract plan");
        assert!(!contract_blocked.compatible);
        assert!(contract_blocked.blocked_reuse.iter().any(|block| {
            block.task_id == "prepare" && block.rule == "output_contract_mismatch"
        }));

        let unrelated_yaml = repaired_yaml.replace(
            "metadata: { name: branch-repair }",
            "metadata: { name: unrelated }",
        );
        let (unrelated_workflow, unrelated_plan) = compile_fixture(&unrelated_yaml);
        let unrelated = runtime
            .plan_repair(
                &source_run_id,
                &unrelated_workflow,
                &unrelated_plan,
                &["analyze_b".to_owned()],
                false,
            )
            .expect("unrelated plan");
        assert!(!unrelated.compatible);
        assert!(unrelated.blocked_reuse.iter().any(|block| {
            block.rule == "workflow_identity_mismatch" && block.full_fork_required
        }));
    }

    #[tokio::test]
    async fn repair_blocks_uncertain_mutation_until_not_applied_reconciliation() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: effect-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:assign
      with: { value: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let effect = EffectRequest::new(
            &source_run_id,
            "second",
            1,
            1,
            "external.publish",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "publish a record",
            "trace-uncertain",
        );
        store
            .record_effect_request(&effect, FixedClock.now())
            .expect("record effect");
        store
            .mark_effect_started(&effect.id, FixedClock.now())
            .expect("start effect");
        store
            .mark_effect_uncertain(&effect.id, "dispatch outcome unknown", FixedClock.now())
            .expect("uncertain effect");
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let blocked = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("blocked plan");
        assert!(!blocked.compatible);
        assert!(
            blocked
                .blocked_reuse
                .iter()
                .any(|block| { block.task_id == "second" && block.rule == "unreconciled_effect" })
        );

        store
            .reconcile_effect_not_applied(
                &effect.id,
                "operator",
                "remote system confirms no record",
                FixedClock.now(),
            )
            .expect("reconcile");
        let read_only = EffectRequest::new(
            &source_run_id,
            "second",
            1,
            2,
            "external.read",
            EffectClass::Observe,
            Risk::Low,
            Idempotency::Idempotent,
            serde_json::json!({"record": "x"}),
            "read a record",
            "trace-read-only",
        );
        store
            .record_effect_request(&read_only, FixedClock.now())
            .expect("record read-only effect");
        store
            .mark_effect_started(&read_only.id, FixedClock.now())
            .expect("start read-only effect");
        store
            .mark_effect_uncertain(&read_only.id, "read response was lost", FixedClock.now())
            .expect("uncertain read-only effect");
        let compatible = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("compatible plan");
        assert!(compatible.compatible, "{:?}", compatible.blocked_reuse);
        assert_eq!(compatible.fresh_effect_summary.uncertain_source_effects, 1);
    }

    #[tokio::test]
    async fn repair_rejects_unresolved_effect_on_otherwise_reusable_task() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: reused-effect-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - { id: first, uses: "action:assign", with: { value: durable } }
    - { id: second, uses: "action:assert", needs: [first], with: { that: false } }
"#;
        let repaired_yaml = source_yaml.replace("that: false", "that: true");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let unresolved = EffectRequest::new(
            &source_run_id,
            "first",
            1,
            99,
            "external.ambiguous",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "create external record",
            "trace-reused-uncertain",
        );
        store
            .record_effect_request(&unresolved, FixedClock.now())
            .expect("record effect");
        store
            .mark_effect_started(&unresolved.id, FixedClock.now())
            .expect("start effect");

        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let stats_before = store.stats().expect("stats");
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("blocked plan");
        assert!(!plan.compatible);
        assert!(plan.blocked_reuse.iter().any(|block| {
            block.task_id == "first"
                && block.rule == "unresolved_reused_effect"
                && block.message.contains(&unresolved.id)
        }));
        assert!(matches!(
            runtime
                .repair(
                    &repaired_workflow,
                    &repaired_plan,
                    plan,
                    None,
                    RunOptions::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(RuntimeError::RepairBlocked { .. })
        ));
        assert_eq!(store.stats().expect("stats"), stats_before);
    }

    #[tokio::test]
    async fn repair_detects_changed_upstream_prompt_file() {
        let directory = tempdir().expect("tempdir");
        std::fs::write(directory.path().join("first.txt"), "original instructions")
            .expect("prompt");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(SelectiveRepairProvider::default());
        let runtime = Runtime::new(store, directory.path())
            .with_clock(Arc::new(FixedClock))
            .with_ids(Arc::new(SequenceIds::default()))
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider));
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: prompt-repair }
spec:
  providers:
    fake: { kind: fake }
  agents:
    first:
      provider: fake
      model: fake
      instructionsFile: first.txt
      structuredOutput:
        type: object
        required: [value]
        properties:
          value: { type: string }
  actions:
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: agent:first
      with: { prompt: produce durable output }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        std::fs::write(directory.path().join("first.txt"), "changed instructions")
            .expect("changed prompt");
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("plan");
        assert!(!plan.compatible);
        assert!(plan.blocked_reuse.iter().any(|block| {
            block.task_id == "first" && block.rule == "definition_fingerprint_mismatch"
        }));
    }

    #[tokio::test]
    async fn repair_reconstructs_successful_memory_delta_and_excludes_failed_boundary() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: memory-repair }
spec:
  memory:
    working: { durable: false, failed: absent }
  policy: { approval: never }
  actions:
    memory: { kind: builtin.memory.write }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:memory
      with: { key: durable, value: true }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
    - id: verify
      uses: action:assert
      needs: [second]
      with: { that: "${{ memory.durable }}" }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        let outcome = runtime
            .repair(
                &repaired_workflow,
                &repaired_plan,
                plan,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair");
        assert_eq!(outcome.state, RunState::Succeeded);
        let run = store.load_run(&outcome.run_id).expect("run");
        assert_eq!(run.working_memory["durable"], true);
        assert_eq!(run.working_memory["failed"], "absent");
    }

    #[tokio::test]
    async fn repair_uses_cas_after_workspace_deletion_and_blocks_blob_corruption() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: artifact-repair }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [.]
    approval: never
  actions:
    write: { kind: builtin.write }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:write
      with: { path: artifact.txt, content: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let compatible = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("plan");
        assert!(compatible.compatible, "{:?}", compatible.blocked_reuse);
        let artifact = compatible.materialized_tasks[0].metadata.artifact_manifest[0].clone();
        std::fs::remove_file(directory.path().join("artifact.txt")).expect("remove artifact");
        let outcome = runtime
            .repair(
                &repaired_workflow,
                &repaired_plan,
                compatible,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("repair from CAS");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert!(!directory.path().join("artifact.txt").exists());
        assert!(
            store
                .verify_artifact_record(&artifact)
                .expect("CAS blob remains valid")
                .valid
        );
        let references = store
            .artifact_references(Some(&outcome.run_id), Some("first"))
            .expect("repair artifact references");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].digest, artifact.digest);

        let compatible_before_corruption = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("second compatible plan");
        assert!(compatible_before_corruption.compatible);
        let blob_path = store.artifact_root().join(&artifact.store_path);
        let mut permissions = std::fs::metadata(&blob_path)
            .expect("blob metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(&blob_path, permissions).expect("make blob writable");
        std::fs::write(&blob_path, b"corrupt").expect("corrupt blob");

        let stats_before = store.stats().expect("stats");
        assert!(matches!(
            runtime
                .repair(
                    &repaired_workflow,
                    &repaired_plan,
                    compatible_before_corruption,
                    None,
                    RunOptions::default(),
                    &CancellationToken::new(),
                )
                .await,
            Err(RuntimeError::RepairBlocked { .. })
        ));
        assert_eq!(store.stats().expect("stats"), stats_before);
        assert!(runtime.replay(&source_run_id).await.is_err());
        assert_eq!(store.stats().expect("stats"), stats_before);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("blocked corrupt plan");
        assert!(!plan.compatible);
        assert!(plan.blocked_reuse.iter().any(|block| {
            block.task_id == "first"
                && block.rule == "artifact_integrity"
                && block.message.contains("artifact.txt")
                && block.message.contains(&artifact.digest)
                && block.message.contains("earlier repair root")
        }));
        assert_eq!(store.stats().expect("stats"), stats_before);
    }

    #[tokio::test]
    async fn legacy_analysis_upgrade_imports_artifacts_and_enables_selective_repair() {
        let directory = tempdir().expect("tempdir");
        let database = directory.path().join("state").join("runtime.db");
        let store = SqliteStore::open(&database).expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: legacy-upgrade }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [.]
    approval: never
  actions:
    write: { kind: builtin.write }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:write
      with: { path: legacy.txt, content: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let raw = rusqlite::Connection::open(&database).expect("raw database");
        raw.execute(
            "UPDATE task_states SET metadata_version = NULL, definition_fingerprint = NULL, input_digest = NULL, output_contract_fingerprint = NULL, output_digest = NULL, state_delta_json = NULL, state_delta_digest = NULL, artifact_manifest_json = NULL, reuse_decision_json = NULL WHERE run_id = ?1 AND task_id = 'first'",
            [&source_run_id],
        )
        .expect("simulate pre-v5 task");
        raw.execute(
            "DELETE FROM artifact_refs WHERE run_id = ?1",
            [&source_run_id],
        )
        .expect("remove post-v5 references");
        raw.execute("DELETE FROM artifact_blobs", [])
            .expect("remove post-v5 blob index");
        drop(raw);

        let stats_before = store.stats().expect("stats");
        let analysis = runtime
            .analyze_legacy_run(&source_run_id)
            .expect("dry-run analysis");
        assert_eq!(analysis.upgradeable_tasks, ["first"]);
        assert_eq!(analysis.unavailable_tasks, ["second"]);
        assert_eq!(analysis.recommended_repair_roots, ["second"]);
        assert_eq!(store.stats().expect("stats"), stats_before);
        assert_eq!(
            store.list_tasks(&source_run_id).expect("tasks")[0].metadata_version,
            None
        );

        let upgrade = runtime
            .upgrade_legacy_run(&source_run_id)
            .expect("legacy upgrade");
        assert_eq!(upgrade.upgraded_tasks, ["first"]);
        assert!(upgrade.analysis_after.tasks[0].already_current);
        let upgraded = store.list_tasks(&source_run_id).expect("upgraded tasks");
        assert_eq!(upgraded[0].metadata_version, Some(TASK_METADATA_VERSION));
        assert_eq!(upgraded[0].artifact_manifest.len(), 1);
        assert_eq!(store.stats().expect("stats").run_upgrades, 1);

        std::fs::remove_file(directory.path().join("legacy.txt")).expect("remove workspace file");
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &upgrade.analysis_after.recommended_repair_roots,
                false,
            )
            .expect("repair plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        let outcome = runtime
            .repair(
                &repaired_workflow,
                &repaired_plan,
                plan,
                Some("repair from proven legacy boundary"),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("selective repair");
        assert_eq!(outcome.state, RunState::Succeeded);
        let tasks = store.list_tasks(&outcome.run_id).expect("repair tasks");
        assert_eq!(tasks[0].disposition, TaskDisposition::Reused);
        assert_eq!(tasks[1].disposition, TaskDisposition::Executed);
        assert!(runtime.replay(&outcome.run_id).await.is_ok());
    }

    #[tokio::test]
    async fn legacy_analysis_reports_the_earliest_safe_boundary_when_proof_is_missing() {
        let directory = tempdir().expect("tempdir");
        let database = directory.path().join("runtime.db");
        let store = SqliteStore::open(&database).expect("store");
        let runtime = runtime(store, directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: legacy-missing-proof }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: first, uses: "action:assign", with: { value: first } }
    - { id: second, uses: "action:assign", with: { value: second } }
"#;
        let (workflow, plan) = compile_fixture(source_yaml);
        let outcome = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("source");
        let raw = rusqlite::Connection::open(&database).expect("raw database");
        raw.execute(
            "UPDATE task_states SET metadata_version = NULL, definition_fingerprint = NULL, input_digest = NULL, output_contract_fingerprint = NULL, output_digest = NULL, state_delta_json = NULL, state_delta_digest = NULL, artifact_manifest_json = NULL, reuse_decision_json = NULL WHERE run_id = ?1",
            [&outcome.run_id],
        )
        .expect("simulate legacy tasks");
        raw.execute(
            "DELETE FROM checkpoints WHERE run_id = ?1 AND sequence > 1",
            [&outcome.run_id],
        )
        .expect("remove unavailable proof");
        drop(raw);

        let analysis = runtime
            .analyze_legacy_run(&outcome.run_id)
            .expect("analysis");
        assert!(!analysis.fully_upgradeable);
        assert_eq!(analysis.unavailable_tasks, ["first", "second"]);
        assert_eq!(analysis.recommended_repair_roots, ["first", "second"]);
        assert!(analysis.tasks.iter().all(|task| {
            task.proposed_metadata.is_none()
                && task
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("checksummed checkpoints"))
        }));
    }

    #[tokio::test]
    async fn terminal_failed_only_retry_reuses_success_and_replays_offline() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(TerminalRetryProvider::default());
        let runtime = Runtime::new(store.clone(), directory.path())
            .with_clock(Arc::new(FixedClock))
            .with_ids(Arc::new(SequenceIds::default()))
            .with_registry(RuntimeRegistry::default().with_provider("fake", provider.clone()));
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: terminal-retry }
spec:
  providers:
    fake: { kind: fake }
  agents:
    recover:
      provider: fake
      model: fake
      instructions: return a recovery object
      structuredOutput:
        type: object
        required: [value]
        additionalProperties: false
        properties:
          value: { type: string }
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: first, uses: "action:assign", with: { value: durable } }
    - { id: second, uses: "agent:recover", needs: [first], with: { prompt: recover } }
    - { id: third, uses: "action:assign", needs: [second], with: { value: "${{ tasks.second.output.value }}" } }
"#;
        let (workflow, compiled) = compile_fixture(source);
        let source_run_id = match runtime
            .start(
                &workflow,
                &compiled,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected terminal source failure, got {other:?}"),
        };
        let source_before = store.load_run(&source_run_id).expect("source");
        let plan = runtime
            .plan_retry(&source_run_id, &workflow, &compiled, &[], true, false)
            .expect("retry plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        assert_eq!(plan.retry_roots, ["second"]);
        assert_eq!(plan.reused_tasks, ["first"]);
        assert_eq!(plan.rerun_tasks, ["second", "third"]);
        assert_eq!(
            serde_json::to_value(&plan).expect("json")["failedOnly"],
            true
        );

        let outcome = runtime
            .retry(
                &workflow,
                &compiled,
                plan,
                Some("retry transient provider failure"),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("retry");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(outcome.reused_tasks, ["first"]);
        assert_eq!(outcome.executed_tasks, ["second", "third"]);
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        let retry = store.load_run(&outcome.run_id).expect("retry run");
        assert_eq!(retry.mode, RunMode::Retry);
        assert_eq!(retry.source_run_id.as_deref(), Some(source_run_id.as_str()));
        assert_eq!(retry.retry_roots, ["second"]);
        assert!(retry.retry_failed_only);
        assert_eq!(
            store.load_run(&source_run_id).expect("source unchanged"),
            source_before
        );
        let tasks = store.list_tasks(&outcome.run_id).expect("retry tasks");
        assert_eq!(tasks[0].disposition, TaskDisposition::Reused);
        assert_eq!(tasks[1].disposition, TaskDisposition::Executed);
        assert_eq!(tasks[1].attempt, 1);

        let replay = runtime
            .replay(&outcome.run_id)
            .await
            .expect("offline replay");
        assert_eq!(replay.state, RunState::Succeeded);
        assert!(
            store
                .list_effects(&replay.run_id)
                .expect("effects")
                .is_empty()
        );
        assert_eq!(replay.output, outcome.output);
    }

    #[tokio::test]
    async fn retry_planning_enforces_identity_roots_acknowledgement_and_reconciliation() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: retry-planning }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - { id: first, uses: "action:assign", with: { value: durable } }
    - { id: second, uses: "action:assert", needs: [first], with: { that: false } }
    - { id: third, uses: "action:assign", needs: [second], with: { value: done } }
"#;
        let (workflow, compiled) = compile_fixture(source);
        let source_run_id = match runtime
            .start(
                &workflow,
                &compiled,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let successful_without_ack = runtime
            .plan_retry(
                &source_run_id,
                &workflow,
                &compiled,
                &["first".to_owned()],
                false,
                false,
            )
            .expect("blocked successful root");
        assert!(!successful_without_ack.compatible);
        assert!(
            successful_without_ack
                .blocked_reuse
                .iter()
                .any(|block| { block.rule == "successful_root_requires_acknowledgement" })
        );
        let multiple = runtime
            .plan_retry(
                &source_run_id,
                &workflow,
                &compiled,
                &["first".to_owned(), "second".to_owned()],
                false,
                true,
            )
            .expect("multiple roots");
        assert!(multiple.compatible, "{:?}", multiple.blocked_reuse);
        assert_eq!(multiple.retry_roots, ["first", "second"]);

        let changed = source.replace("value: durable", "value: changed");
        let (changed_workflow, changed_plan) = compile_fixture(&changed);
        let mismatch = runtime
            .plan_retry(
                &source_run_id,
                &changed_workflow,
                &changed_plan,
                &[],
                true,
                false,
            )
            .expect("mismatch plan");
        assert!(!mismatch.compatible);
        assert!(
            mismatch
                .blocked_reuse
                .iter()
                .any(|block| { block.rule == "retry_workflow_definition_mismatch" })
        );

        let uncertain = EffectRequest::new(
            &source_run_id,
            "second",
            1,
            99,
            "external.publish",
            EffectClass::ExternalMutate,
            Risk::High,
            Idempotency::Unknown,
            serde_json::json!({"record": "x"}),
            "publish record",
            "trace-retry",
        );
        store
            .record_effect_request(&uncertain, FixedClock.now())
            .expect("effect");
        store
            .mark_effect_started(&uncertain.id, FixedClock.now())
            .expect("started");
        store
            .mark_effect_uncertain(&uncertain.id, "unknown", FixedClock.now())
            .expect("uncertain");
        let blocked = runtime
            .plan_retry(&source_run_id, &workflow, &compiled, &[], true, false)
            .expect("blocked effect");
        assert!(!blocked.compatible);
        assert!(
            blocked
                .blocked_reuse
                .iter()
                .any(|block| block.rule == "unreconciled_effect")
        );
        store
            .reconcile_effect_not_applied(
                &uncertain.id,
                "operator",
                "remote lookup found no record",
                FixedClock.now(),
            )
            .expect("reconcile");
        let compatible = runtime
            .plan_retry(&source_run_id, &workflow, &compiled, &[], true, false)
            .expect("compatible after reconciliation");
        assert!(compatible.compatible, "{:?}", compatible.blocked_reuse);
    }

    #[tokio::test]
    async fn repair_blocks_missing_state_delta_and_tampered_reused_output_digest() {
        let directory = tempdir().expect("tempdir");
        let database = directory.path().join("runtime.db");
        let store = SqliteStore::open(&database).expect("store");
        let runtime = runtime(store, directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: corrupt-output-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:assign
      with: { value: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false }
"#;
        let repaired_yaml = source_yaml.replace("with: { that: false }", "with: { that: true }");
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let tamper = rusqlite::Connection::open(&database).expect("tamper connection");
        tamper
            .execute(
                "UPDATE task_states SET state_delta_json = NULL WHERE run_id = ?1 AND task_id = ?2",
                rusqlite::params![source_run_id, "first"],
            )
            .expect("remove state delta");
        let (repaired_workflow, repaired_plan) = compile_fixture(&repaired_yaml);
        let missing_delta = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("missing state-delta plan");
        assert!(!missing_delta.compatible);
        assert!(
            missing_delta
                .blocked_reuse
                .iter()
                .any(|block| { block.task_id == "first" && block.rule == "state_delta_missing" })
        );

        tamper
            .execute(
                "UPDATE task_states SET state_delta_json = ?3 WHERE run_id = ?1 AND task_id = ?2",
                rusqlite::params![
                    source_run_id,
                    "first",
                    r#"{"formatVersion":1,"set":{},"remove":[]}"#
                ],
            )
            .expect("restore state delta");
        tamper
            .execute(
                "UPDATE task_states SET output_json = ?3 WHERE run_id = ?1 AND task_id = ?2",
                rusqlite::params![source_run_id, "first", r#"{"tampered":true}"#],
            )
            .expect("tamper output");
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &repaired_workflow,
                &repaired_plan,
                &["second".to_owned()],
                false,
            )
            .expect("plan");
        assert!(!plan.compatible);
        assert!(
            plan.blocked_reuse.iter().any(|block| {
                block.task_id == "first" && block.rule == "output_digest_mismatch"
            })
        );
    }

    #[tokio::test]
    async fn repair_handles_multiple_roots_new_descendants_removed_tasks_and_restart_acknowledgement()
     {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: graph-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - { id: prepare, uses: "action:assign", with: { value: stable } }
    - { id: removed, uses: "action:assign", with: { value: obsolete } }
    - { id: left, uses: "action:assert", needs: [prepare], with: { that: false } }
    - { id: right, uses: "action:assert", needs: [prepare], with: { that: false } }
    - { id: combine, uses: "action:assign", needs: [left, right], with: { value: combined } }
"#;
        let target_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: graph-repair }
spec:
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - { id: prepare, uses: "action:assign", with: { value: stable } }
    - { id: left, uses: "action:assert", needs: [prepare], with: { that: true } }
    - { id: right, uses: "action:assert", needs: [prepare], with: { that: true } }
    - { id: combine, uses: "action:assign", needs: [left, right], with: { value: combined } }
    - { id: verify, uses: "action:assert", needs: [combine], with: { that: true } }
"#;
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, task, .. }) => {
                assert_eq!(task, "left");
                run_id
            }
            other => panic!("expected source failure, got {other:?}"),
        };
        let (target_workflow, target_plan) = compile_fixture(target_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &target_workflow,
                &target_plan,
                &["left".to_owned(), "right".to_owned()],
                false,
            )
            .expect("multi-root plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        assert_eq!(plan.repair_roots, ["left", "right"]);
        assert_eq!(plan.reused_tasks, ["prepare"]);
        assert_eq!(plan.rerun_tasks, ["left", "right", "combine", "verify"]);
        assert_eq!(plan.new_tasks, ["verify"]);
        assert_eq!(plan.removed_tasks, ["removed"]);
        let outcome = runtime
            .repair(
                &target_workflow,
                &target_plan,
                plan,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("multi-root repair");
        assert_eq!(outcome.state, RunState::Succeeded);

        let unrelated_yaml = target_yaml.replace(
            "    - { id: verify, uses: \"action:assert\", needs: [combine], with: { that: true } }\n",
            concat!(
                "    - { id: verify, uses: \"action:assert\", needs: [combine], with: { that: true } }\n",
                "    - { id: unrelated, uses: \"action:assign\", with: { value: new } }\n"
            ),
        );
        let (unrelated_workflow, unrelated_plan) = compile_fixture(&unrelated_yaml);
        let blocked = runtime
            .plan_repair(
                &source_run_id,
                &unrelated_workflow,
                &unrelated_plan,
                &["left".to_owned(), "right".to_owned()],
                false,
            )
            .expect("blocked unrelated plan");
        assert!(!blocked.compatible);
        assert!(blocked.blocked_reuse.iter().any(|block| {
            block.task_id == "unrelated" && block.rule == "new_task_outside_repair_closure"
        }));

        let successful_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: restart-successful }
spec:
  actions: { assign: { kind: builtin.assign } }
  tasks: [{ id: done, uses: "action:assign", with: { value: old } }]
"#;
        let target_successful_yaml = successful_yaml.replace("value: old", "value: new");
        let (successful_workflow, successful_plan) = compile_fixture(successful_yaml);
        let successful_run = runtime
            .start(
                &successful_workflow,
                &successful_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("successful source");
        let (target_successful_workflow, target_successful_plan) =
            compile_fixture(&target_successful_yaml);
        let blocked = runtime
            .plan_repair(
                &successful_run.run_id,
                &target_successful_workflow,
                &target_successful_plan,
                &["done".to_owned()],
                false,
            )
            .expect("restart plan");
        assert!(!blocked.compatible);
        assert!(blocked.blocked_reuse.iter().any(|block| {
            block.task_id == "done" && block.rule == "successful_root_requires_acknowledgement"
        }));
        let acknowledged = runtime
            .plan_repair(
                &successful_run.run_id,
                &target_successful_workflow,
                &target_successful_plan,
                &["done".to_owned()],
                true,
            )
            .expect("acknowledged restart plan");
        assert!(acknowledged.compatible, "{:?}", acknowledged.blocked_reuse);
    }

    #[tokio::test]
    async fn repair_allows_confirmed_idempotent_effects_and_preserves_approval_gates() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let source_yaml = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: effect-safety }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [.]
  actions:
    assert: { kind: builtin.assert }
    write: { kind: builtin.write }
  tasks:
    - { id: publish, uses: "action:assert", with: { that: false } }
"#;
        let (source_workflow, source_plan) = compile_fixture(source_yaml);
        let source_run_id = match runtime
            .start(
                &source_workflow,
                &source_plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected source failure, got {other:?}"),
        };
        let prior_effect = EffectRequest::new(
            &source_run_id,
            "publish",
            1,
            1,
            "external.idempotent-put",
            EffectClass::ExternalMutate,
            Risk::Medium,
            Idempotency::Idempotent,
            serde_json::json!({"key": "stable", "value": 1}),
            "put stable value",
            "trace-idempotent",
        );
        store
            .record_effect_request(&prior_effect, FixedClock.now())
            .expect("record effect");
        store
            .mark_effect_started(&prior_effect.id, FixedClock.now())
            .expect("start effect");
        store
            .complete_effect(
                &prior_effect.id,
                Ok(&serde_json::json!({"stored": true})),
                FixedClock.now(),
            )
            .expect("complete effect");

        let target_yaml = source_yaml.replace(
            "    - { id: publish, uses: \"action:assert\", with: { that: false } }",
            "    - { id: publish, uses: \"action:write\", with: { path: approved.txt, content: repaired } }",
        );
        let (target_workflow, target_plan) = compile_fixture(&target_yaml);
        let plan = runtime
            .plan_repair(
                &source_run_id,
                &target_workflow,
                &target_plan,
                &["publish".to_owned()],
                false,
            )
            .expect("effect-safe plan");
        assert!(plan.compatible, "{:?}", plan.blocked_reuse);
        let outcome = runtime
            .repair(
                &target_workflow,
                &target_plan,
                plan,
                None,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("approval-paused repair");
        assert_eq!(outcome.state, RunState::Paused);
        assert!(!directory.path().join("approved.txt").exists());
        let tasks = store.list_tasks(&outcome.run_id).expect("repair tasks");
        assert_eq!(tasks[0].state, TaskState::WaitingForApproval);
        assert_eq!(
            tasks[0].disposition,
            agentctl_store::TaskDisposition::Executed
        );
        assert_eq!(
            store
                .pending_approvals(&outcome.run_id)
                .expect("approvals")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn deterministic_dataflow_condition_and_working_memory() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: dataflow }
spec:
  inputs: { enabled: true, greeting: hello }
  memory:
    working: { count: 0 }
  actions:
    assign: { kind: builtin.assign }
    remember: { kind: builtin.memory.write }
  tasks:
    - id: first
      uses: action:assign
      with: { message: "${{ inputs.greeting }}" }
    - id: remember
      uses: action:remember
      needs: [first]
      when: "${{ inputs.enabled == true }}"
      with: { key: result, value: "${{ tasks.first.output.output.message }}" }
"#,
        );
        let outcome = runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({"enabled": true, "greeting": "hello"}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("run succeeds");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(
            store.load_run(&outcome.run_id).expect("run").working_memory["result"],
            "hello"
        );
    }

    #[tokio::test]
    async fn task_output_contract_failure_is_durable() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: output-contract }
spec:
  actions: { assign: { kind: builtin.assign } }
  tasks:
    - id: typed
      uses: action:assign
      with: { value: not-an-integer }
      outputSchema:
        type: object
        properties: { value: { type: integer } }
        required: [value]
"#,
        );
        let run_id = match runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed {
                run_id, message, ..
            }) => {
                assert!(message.contains("task output contract failed"));
                run_id
            }
            other => panic!("expected contract failure, got {other:?}"),
        };
        let task = &store.list_tasks(&run_id).expect("tasks")[0];
        assert_eq!(task.state, TaskState::Failed);
        assert!(
            task.error
                .as_deref()
                .is_some_and(|error| error.contains("task output contract failed"))
        );
    }

    #[tokio::test]
    async fn task_vars_render_agent_defaults_overrides_and_dependency_outputs() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let registry =
            RuntimeRegistry::default().with_provider("fake", Arc::new(PromptEchoProvider));
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: task-vars }
spec:
  inputs: { service: checkout }
  providers: { fake: { kind: fake } }
  agents:
    reviewer:
      provider: fake
      model: scripted
      instructions: review
      vars: { severity: medium, service: default }
      maxTurns: 1
  actions: { assign: { kind: builtin.assign } }
  tasks:
    - id: prepare
      uses: action:assign
      with: { finding: restore-drill-missing }
    - id: review
      uses: agent:reviewer
      needs: [prepare]
      vars:
        service: "${{ inputs.service }}"
        finding: "${{ tasks.prepare.output.output.finding }}"
      with:
        prompt: "${{ vars.service }}:${{ vars.finding }}:${{ vars.severity }}"
"#,
        );
        let outcome = runtime(store.clone(), directory.path())
            .with_registry(registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({"service": "checkout"}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("run succeeds");
        assert_eq!(outcome.state, RunState::Succeeded);
        let review = store
            .list_tasks(&outcome.run_id)
            .expect("tasks")
            .into_iter()
            .find(|task| task.task_id == "review")
            .expect("review task");
        assert_eq!(
            review.output.expect("review output")["text"],
            "checkout:restore-drill-missing:medium"
        );
    }

    #[tokio::test]
    async fn direct_read_rejects_files_over_the_workspace_limit() {
        let directory = tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("oversized.txt"),
            vec![b'x'; MAX_WORKSPACE_FILE_BYTES as usize + 1],
        )
        .expect("oversized fixture");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: bounded-read }
spec:
  policy: { workspaceRoot: ., approval: never }
  actions: { read: { kind: builtin.read } }
  tasks:
    - id: read
      uses: action:read
      with: { path: oversized.txt }
"#,
        );
        let store = SqliteStore::open_memory().expect("store");
        let result = runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await;

        let run_id = match result {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected failed run, got {other:?}"),
        };
        let run = store.load_run(&run_id).expect("run");
        assert_eq!(run.state, RunState::Failed);
        assert_eq!(
            store.list_effects(&run.run_id).expect("effects")[0].status,
            EffectStatus::Failed
        );
    }

    #[tokio::test]
    async fn check_diff_does_not_mutate_and_interactive_approval_resumes() {
        let directory = tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("out")).expect("out dir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: write }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [out]
    approval: mutations
  actions:
    write:
      kind: builtin.write
  tasks:
    - id: write
      uses: action:write
      with: { path: out/result.txt, content: hello }
"#;
        let (workflow, plan) = compile_fixture(source);
        let check_store = SqliteStore::open_memory().expect("check store");
        let check = runtime(check_store, directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions {
                    check: true,
                    diff: true,
                    interactive: false,
                },
                &CancellationToken::new(),
            )
            .await
            .expect("check succeeds");
        assert_eq!(check.state, RunState::Succeeded);
        assert!(!directory.path().join("out/result.txt").exists());

        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let paused = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions {
                    check: false,
                    diff: true,
                    interactive: true,
                },
                &CancellationToken::new(),
            )
            .await
            .expect("pause");
        assert_eq!(paused.state, RunState::Paused);
        let approvals = store.pending_approvals(&paused.run_id).expect("approvals");
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].expected_effect, "write a workspace file");
        store
            .resolve_approval(
                &approvals[0].approval_id,
                ApprovalResolution::Approved,
                "tester",
                "fixture approval",
                Utc::now(),
            )
            .expect("approve");
        let resumed = runtime
            .resume(
                &paused.run_id,
                RunOptions {
                    check: false,
                    diff: true,
                    interactive: true,
                },
                &CancellationToken::new(),
            )
            .await
            .expect("resume");
        assert_eq!(resumed.state, RunState::Succeeded);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("out/result.txt")).expect("content"),
            "hello"
        );
    }

    #[tokio::test]
    async fn non_interactive_run_pauses_without_bypassing_required_approval() {
        let directory = tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("out")).expect("out dir");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: ci-safety }
spec:
  policy: { workspaceRoot: ., writableRoots: [out], approval: mutations }
  actions: { write: { kind: builtin.write } }
  tasks:
    - { id: write, uses: "action:write", with: { path: out/file, content: unsafe } }
"#,
        );
        let result = runtime(SqliteStore::open_memory().expect("store"), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("non-interactive run pauses");
        assert_eq!(result.state, RunState::Paused);
        assert!(!directory.path().join("out/file").exists());
    }

    #[tokio::test]
    async fn recorded_replay_never_calls_provider_and_fork_does() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let provider = Arc::new(CountingProvider::default());
        let registry = RuntimeRegistry::default().with_provider("fake", provider.clone());
        let traces = Arc::new(BufferedTraceSink::default());
        let runtime = runtime(store.clone(), directory.path())
            .with_registry(registry)
            .with_trace_sink(traces.clone());
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: provider }
spec:
  providers: { fake: { kind: fake } }
  agents:
    answer:
      provider: fake
      model: scripted
      instructions: answer briefly
      maxTurns: 1
  tasks:
    - { id: answer, uses: "agent:answer", with: { prompt: hello } }
"#,
        );
        let first = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("first run");
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        let replay = runtime.replay(&first.run_id).await.expect("replay");
        assert_eq!(replay.state, RunState::Succeeded);
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        let fork = runtime
            .fork(
                &first.run_id,
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("fork");
        assert_eq!(fork.state, RunState::Succeeded);
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        assert!(
            traces
                .events()
                .iter()
                .any(|event| event.kind == SpanKind::ProviderRequest)
        );
    }

    #[tokio::test]
    async fn recorded_replay_never_calls_provider_or_tool_executor() {
        let directory = tempdir().expect("tempdir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: replay-tools }
spec:
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: use the tool once
      tools: [echo]
      maxTurns: 2
      maxToolCalls: 1
  tasks: [{ id: work, uses: "agent:worker", with: { prompt: hello } }]
"#;
        let (workflow, plan) = compile_fixture(source);
        let provider = Arc::new(ToolCallingProvider::default());
        let store = SqliteStore::open_memory().expect("store");
        let execution_runtime = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default()
                .with_provider("fake", provider.clone())
                .with_tool("echo", Arc::new(FixtureTool::new(false))),
        );
        let first = execution_runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("first run");
        let replay_runtime = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default()
                .with_provider("fake", Arc::new(PanicProvider))
                .with_tool(
                    "echo",
                    Arc::new(PanicTool {
                        contract: FixtureTool::new(false).contract,
                    }),
                ),
        );
        let replay = replay_runtime.replay(&first.run_id).await.expect("replay");
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        assert_eq!(replay.output, first.output);
        assert!(
            store
                .list_effects(&replay.run_id)
                .expect("effects")
                .is_empty()
        );
        assert!(store.tool_calls(&replay.run_id).expect("calls").is_empty());
        let source_effect_ids = store
            .list_effects(&first.run_id)
            .expect("source effects")
            .into_iter()
            .map(|effect| effect.request.id)
            .collect::<Vec<_>>();
        let source_tool_call_ids = store
            .tool_calls(&first.run_id)
            .expect("source tool calls")
            .into_iter()
            .map(|call| call.call_id)
            .collect::<Vec<_>>();
        let replay_audit = store.audit_events(&replay.run_id).expect("replay audit");
        let reused = replay_audit
            .iter()
            .find(|event| event.event_type == "replay.effects_reused")
            .expect("reused-effect audit event");
        assert_eq!(reused.payload["sourceRunId"], first.run_id);
        assert_eq!(
            reused.payload["effects"]
                .as_array()
                .expect("effect references")
                .iter()
                .map(|effect| effect["effectId"].as_str().expect("effect id").to_owned())
                .collect::<Vec<_>>(),
            source_effect_ids
        );
        assert_eq!(
            reused.payload["toolCalls"]
                .as_array()
                .expect("tool-call references")
                .iter()
                .map(|call| call["callId"].as_str().expect("call id").to_owned())
                .collect::<Vec<_>>(),
            source_tool_call_ids
        );
    }

    #[tokio::test]
    async fn replay_rejects_nonterminal_source_without_creating_partial_state() {
        let directory = tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("out")).expect("out");
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: paused-replay }
spec:
  policy: { workspaceRoot: ., writableRoots: [out], approval: mutations }
  actions: { write: { kind: builtin.write } }
  tasks: [{ id: write, uses: "action:write", with: { path: out/file, content: value } }]
"#,
        );
        let paused = runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("paused run");
        assert_eq!(paused.state, RunState::Paused);
        assert!(matches!(
            runtime.replay(&paused.run_id).await,
            Err(RuntimeError::InvalidState(_))
        ));
        assert_eq!(store.stats().expect("stats").runs, 1);
    }

    #[tokio::test]
    async fn cancellation_is_durable() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: cancel }
spec:
  actions: { assign: { kind: builtin.assign } }
  tasks: [{ id: one, uses: "action:assign" }]
"#,
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let outcome = runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &cancellation,
            )
            .await
            .expect("cancelled outcome");
        assert_eq!(outcome.state, RunState::Cancelled);
        assert_eq!(
            store.load_run(&outcome.run_id).expect("run").state,
            RunState::Cancelled
        );
    }

    #[tokio::test]
    async fn cancellation_during_retry_backoff_is_durable() {
        let directory = tempdir().expect("tempdir");
        let store = SqliteStore::open_memory().expect("store");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: retry-cancel }
spec:
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: test
  tasks:
    - id: work
      uses: agent:worker
      retry: { maxAttempts: 2, backoffMs: 1000 }
      with: { prompt: hello }
"#,
        );
        let registry =
            RuntimeRegistry::default().with_provider("fake", Arc::new(RetryableThenCancelProvider));
        let outcome = runtime(store.clone(), directory.path())
            .with_registry(registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("cancelled outcome");

        assert_eq!(outcome.state, RunState::Cancelled);
        assert_eq!(
            store.load_run(&outcome.run_id).expect("run").state,
            RunState::Cancelled
        );
        assert_eq!(
            store.list_tasks(&outcome.run_id).expect("tasks")[0].state,
            TaskState::Cancelled
        );
    }

    #[tokio::test]
    async fn agent_tool_loop_validates_tool_output_before_model_continuation() {
        let directory = tempdir().expect("tempdir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: tools }
spec:
  policy: { providers: [fake] }
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: use the tool once
      tools: [echo]
      maxTurns: 2
      maxToolCalls: 1
  tasks:
    - { id: work, uses: "agent:worker", with: { prompt: hello } }
"#;
        let (workflow, plan) = compile_fixture(source);
        let provider = Arc::new(ToolCallingProvider::default());
        let registry = RuntimeRegistry::default()
            .with_provider("fake", provider.clone())
            .with_tool("echo", Arc::new(FixtureTool::new(false)));
        let store = SqliteStore::open_memory().expect("store");
        let outcome = runtime(store.clone(), directory.path())
            .with_registry(registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("valid tool output");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        let stats = store.stats().expect("stats");
        assert_eq!(stats.provider_sessions, 1);
        assert_eq!(stats.tool_calls, 1);
        let calls = store.tool_calls(&outcome.run_id).expect("tool calls");
        assert_eq!(calls[0].call_id, "call-1");

        let bad_registry = RuntimeRegistry::default()
            .with_provider("fake", Arc::new(ToolCallingProvider::default()))
            .with_tool("echo", Arc::new(FixtureTool::new(true)));
        let result = runtime(SqliteStore::open_memory().expect("store"), directory.path())
            .with_registry(bad_registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(RuntimeError::RunFailed { .. })));
    }

    #[tokio::test]
    async fn tool_approval_override_cannot_bypass_global_policy_denial() {
        let directory = tempdir().expect("tempdir");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: denied-tool }
spec:
  policy: { toolsDeny: [echo], approval: never }
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: use the tool
      tools: [echo]
      maxTurns: 2
      maxToolCalls: 1
  tasks: [{ id: work, uses: "agent:worker", with: { prompt: hello } }]
"#,
        );
        let registry = RuntimeRegistry::default()
            .with_provider("fake", Arc::new(ToolCallingProvider::default()))
            .with_tool("echo", Arc::new(FixtureTool::new(false)));
        let store = SqliteStore::open_memory().expect("store");
        let result = runtime(store.clone(), directory.path())
            .with_registry(registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(RuntimeError::RunFailed { .. })));
        assert_eq!(store.stats().expect("stats").tool_calls, 0);
    }

    #[tokio::test]
    async fn tool_timeout_and_cancellation_are_bounded_and_durable() {
        let directory = tempdir().expect("tempdir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: tool-bounds }
spec:
  providers: { fake: { kind: fake } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      outputSchema: { type: object, properties: { text: { type: string } }, required: [text], additionalProperties: false }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: use the tool
      tools: [echo]
      maxTurns: 2
      maxToolCalls: 1
  tasks: [{ id: work, uses: "agent:worker", with: { prompt: hello } }]
"#;
        let (workflow, plan) = compile_fixture(source);

        let timeout_store = SqliteStore::open_memory().expect("timeout store");
        let timeout_registry = RuntimeRegistry::default()
            .with_provider("fake", Arc::new(ToolCallingProvider::default()))
            .with_tool(
                "echo",
                Arc::new(FixtureTool::new(false).delayed(Duration::from_millis(1_100), 1)),
            );
        let timeout = runtime(timeout_store.clone(), directory.path())
            .with_registry(timeout_registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await;
        let timeout_run_id = match timeout {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected failed run, got {other:?}"),
        };
        assert_eq!(timeout_store.stats().expect("timeout stats").tool_calls, 1);
        assert_eq!(
            timeout_store.tool_calls(&timeout_run_id).expect("calls")[0].status,
            "uncertain"
        );
        assert_eq!(
            timeout_store
                .list_effects(&timeout_run_id)
                .expect("effects")
                .last()
                .expect("tool effect")
                .status,
            EffectStatus::Uncertain
        );

        let cancel_store = SqliteStore::open_memory().expect("cancel store");
        let cancel_registry = RuntimeRegistry::default()
            .with_provider("fake", Arc::new(ToolCallingProvider::default()))
            .with_tool(
                "echo",
                Arc::new(FixtureTool::new(false).delayed(Duration::from_secs(1), 5)),
            );
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            trigger.cancel();
        });
        let cancelled = runtime(cancel_store.clone(), directory.path())
            .with_registry(cancel_registry)
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &cancellation,
            )
            .await
            .expect("cancelled outcome");
        assert_eq!(cancelled.state, RunState::Cancelled);
        assert_eq!(
            cancel_store
                .load_run(&cancelled.run_id)
                .expect("cancelled run")
                .state,
            RunState::Cancelled
        );
        assert_eq!(
            cancel_store.tool_calls(&cancelled.run_id).expect("calls")[0].status,
            "uncertain"
        );
        assert_eq!(
            cancel_store
                .list_effects(&cancelled.run_id)
                .expect("effects")
                .last()
                .expect("tool effect")
                .status,
            EffectStatus::Uncertain
        );
        assert!(matches!(
            runtime(cancel_store, directory.path())
                .resume(
                    &cancelled.run_id,
                    RunOptions::default(),
                    &CancellationToken::new()
                )
                .await,
            Err(RuntimeError::UncertainEffect { .. })
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_timeout_is_durable_uncertainty_and_blocks_resume() {
        let directory = tempdir().expect("tempdir");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: process-timeout }
spec:
  policy:
    workspaceRoot: .
    processAllowlist: [sh]
    approval: never
  actions:
    wait:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, "sleep 5"]
      timeoutSeconds: 1
  tasks: [{ id: wait, uses: "action:wait" }]
"#,
        );
        let store = SqliteStore::open_memory().expect("store");
        let runtime = runtime(store.clone(), directory.path());
        let run_id = match runtime
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected failed run, got {other:?}"),
        };
        assert_eq!(
            store.list_effects(&run_id).expect("effects")[0].status,
            EffectStatus::Uncertain
        );
        assert!(matches!(
            runtime
                .resume(&run_id, RunOptions::default(), &CancellationToken::new())
                .await,
            Err(RuntimeError::UncertainEffect { .. })
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_output_limit_is_structured_durable_and_secret_safe() {
        let directory = tempdir().expect("tempdir");
        let secret = std::env::var("PATH").expect("PATH");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: process-output-limit }
spec:
  policy:
    workspaceRoot: .
    processAllowlist: [sh]
    environmentAllowlist: [SECRET, PATH]
    approval: never
  actions:
    noisy:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, 'while :; do printf "%s" "$SECRET"; done']
      env:
        SECRET: { env: PATH }
      stdoutLimitBytes: 64
      stderrLimitBytes: 64
      combinedOutputLimitBytes: 128
  tasks: [{ id: noisy, uses: "action:noisy" }]
"#,
        );
        let store = SqliteStore::open_memory().expect("store");
        let run_id = match runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected failed run, got {other:?}"),
        };
        let effect = &store.list_effects(&run_id).expect("effects")[0];
        assert_eq!(effect.status, EffectStatus::Failed);
        let diagnostic: Value = serde_json::from_str(effect.error.as_deref().expect("error"))
            .expect("structured diagnostic");
        assert_eq!(diagnostic["code"], "subprocess_output_limit_exceeded");
        assert_eq!(diagnostic["stream"], "stdout");
        assert_eq!(diagnostic["limitBytes"], 64);
        assert_eq!(
            diagnostic["stdoutPrefix"],
            "[REDACTED: subprocess output omitted because secret environment values were present]"
        );
        assert!(!effect.error.as_deref().expect("error").contains(&secret));
        assert_eq!(
            store.load_run(&run_id).expect("run").state,
            RunState::Failed
        );
        assert!(
            store.list_tasks(&run_id).expect("tasks")[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("subprocess_output_limit_exceeded"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_secret_subprocess_output_is_redacted_and_never_persisted() {
        let directory = tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("secrets")).expect("secret directory");
        let secret = "mounted-file-secret-marker";
        std::fs::write(
            directory.path().join("secrets/token"),
            format!("{secret}\n"),
        )
        .expect("secret file");
        let (workflow, plan) = compile_fixture(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: process-secret-redaction }
spec:
  policy:
    workspaceRoot: .
    processAllowlist: [sh]
    environmentAllowlist: [SECRET]
    secretFileRoots: [secrets]
    approval: never
  actions:
    print:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, 'printf "%s" "$SECRET"']
      env:
        SECRET: { file: secrets/token }
  tasks: [{ id: print, uses: "action:print" }]
"#,
        );
        let database = directory.path().join("runtime.db");
        let store = SqliteStore::open(&database).expect("store");
        let outcome = runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("successful run");
        assert_eq!(outcome.state, RunState::Succeeded);
        assert_eq!(
            outcome.output.as_ref().expect("output")["print"]["stdout"],
            "[REDACTED]"
        );
        let effect = &store.list_effects(&outcome.run_id).expect("effects")[0];
        assert_eq!(
            effect.request.input["environment"]["SECRET"]["source"],
            "secret file `secrets/token`"
        );
        assert!(
            !serde_json::to_string(effect)
                .expect("effect json")
                .contains(secret)
        );
        let raw = rusqlite::Connection::open(database).expect("raw database");
        let occurrences: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM (
                   SELECT workflow_json AS value FROM runs
                   UNION ALL SELECT inputs_json FROM runs
                   UNION ALL SELECT working_memory_json FROM runs
                   UNION ALL SELECT output_json FROM runs WHERE output_json IS NOT NULL
                   UNION ALL SELECT output_json FROM task_states WHERE output_json IS NOT NULL
                   UNION ALL SELECT error FROM task_states WHERE error IS NOT NULL
                   UNION ALL SELECT input_json FROM effects
                   UNION ALL SELECT result_json FROM effects WHERE result_json IS NOT NULL
                   UNION ALL SELECT error FROM effects WHERE error IS NOT NULL
                   UNION ALL SELECT state_json FROM checkpoints
                   UNION ALL SELECT payload_json FROM audit_events
                   UNION ALL SELECT event_json FROM trace_events
                 ) WHERE instr(value, ?1) > 0",
                [secret],
                |row| row.get(0),
            )
            .expect("secret scan");
        assert_eq!(occurrences, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pack_process_uses_the_same_output_limit_contract() {
        use agentctl_core::pack::PackManifest;

        let directory = tempdir().expect("tempdir");
        let mut workflow = parse_workflow(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: packed-process-output-limit }
spec:
  policy:
    workspaceRoot: .
    processAllowlist: [sh]
    approval: never
  actions:
    placeholder: { kind: builtin.assign }
  tasks: [{ id: noisy, uses: "action:example.utility.noisy" }]
"#,
            "fixture.yaml",
        )
        .expect("workflow")
        .workflow;
        let pack: PackManifest = serde_json::from_value(serde_json::json!({
            "apiVersion": "agentctl.dev/pack/v1alpha1",
            "name": "example.utility",
            "version": "1.0.0",
            "agentctl": ">=0.2.0, <1.0.0",
            "actions": {
                "noisy": {
                    "kind": "builtin.shell.exec",
                    "command": "/bin/sh",
                    "args": ["-c", "while :; do printf packed; done"],
                    "stdoutLimitBytes": 32,
                    "stderrLimitBytes": 32,
                    "combinedOutputLimitBytes": 64
                }
            }
        }))
        .expect("pack");
        pack.validate().expect("valid pack");
        workflow.spec.actions.insert(
            "example.utility.noisy".to_owned(),
            pack.actions["noisy"].clone(),
        );
        let plan = compile(&workflow, "fixture.yaml").expect("plan");
        let store = SqliteStore::open_memory().expect("store");
        let run_id = match runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &CancellationToken::new(),
            )
            .await
        {
            Err(RuntimeError::RunFailed { run_id, .. }) => run_id,
            other => panic!("expected failed run, got {other:?}"),
        };
        let error = store.list_effects(&run_id).expect("effects")[0]
            .error
            .clone()
            .expect("error");
        let diagnostic: Value = serde_json::from_str(&error).expect("structured diagnostic");
        assert_eq!(diagnostic["code"], "subprocess_output_limit_exceeded");
        assert_eq!(diagnostic["limitBytes"], 32);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_cancellation_is_durable_uncertainty() {
        let directory = tempdir().expect("tempdir");
        let marker = directory.path().join("started");
        let source = format!(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: process-cancellation }}
spec:
  policy:
    workspaceRoot: .
    processAllowlist: [sh]
    approval: never
  actions:
    wait:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, "echo started > '{}'; sleep 5"]
      timeoutSeconds: 10
  tasks: [{{ id: wait, uses: "action:wait" }}]
"#,
            marker.display()
        );
        let (workflow, plan) = compile_fixture(&source);
        let store = SqliteStore::open_memory().expect("store");
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let observed_marker = marker.clone();
        tokio::spawn(async move {
            while !observed_marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            trigger.cancel();
        });
        let cancelled = runtime(store.clone(), directory.path())
            .start(
                &workflow,
                &plan,
                serde_json::json!({}),
                RunOptions::default(),
                &cancellation,
            )
            .await
            .expect("cancelled outcome");
        assert_eq!(cancelled.state, RunState::Cancelled);
        assert_eq!(
            store.list_effects(&cancelled.run_id).expect("effects")[0].status,
            EffectStatus::Uncertain
        );
        assert!(matches!(
            runtime(store, directory.path())
                .resume(
                    &cancelled.run_id,
                    RunOptions::default(),
                    &CancellationToken::new()
                )
                .await,
            Err(RuntimeError::UncertainEffect { .. })
        ));
    }

    #[test]
    fn subprocess_output_redaction_removes_every_known_secret_value() {
        let output = redact_text("token=top-secret; repeated=top-secret", &["top-secret"]);
        assert_eq!(output, "token=[REDACTED]; repeated=[REDACTED]");
    }
}
