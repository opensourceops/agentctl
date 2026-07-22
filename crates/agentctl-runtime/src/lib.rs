//! Durable deterministic workflow runtime for agentctl.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentctl_core::compiler::{CompiledPlan, PlanPredictability, TaskUse};
use agentctl_core::dsl::{
    API_VERSION, ActionDefinition, ActionKind, ApprovalRequirement, EffectClass, FailureBehavior,
    Idempotency, Risk, ToolDefinition, ToolKind, Workflow,
};
use agentctl_core::effect::{ActionResult, ChangeStatus, EffectRequest, EffectStatus};
use agentctl_core::policy::{PolicyContext, PolicyDecision, PolicyEngine, PolicyError, redact};
use agentctl_core::provider::{
    ContentBlock, FinishReason, Message, ModelProvider, ProviderError, ProviderRequest,
    ProviderResponse, Usage,
};
use agentctl_core::state::{RunState, TaskState};
use agentctl_core::template::{EvalContext, TemplateError, evaluate_when, render};
use agentctl_core::tool::{ToolContract, ToolContractError, ToolExecutor};
use agentctl_observability::{NoopTraceSink, SpanKind, TraceEvent, TracePhase, TraceSink};
use agentctl_store::{ApprovalRequest, RunMode, SqliteStore, StoreError, TaskRecord};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

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

const MAX_WORKSPACE_TOOL_BYTES: u64 = 1024 * 1024;

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
                let metadata = tokio::fs::metadata(&resolved)
                    .await
                    .map_err(|error| ToolContractError::Execution(error.to_string()))?;
                if metadata.len() > MAX_WORKSPACE_TOOL_BYTES {
                    return Err(ToolContractError::Execution(format!(
                        "workspace read exceeds {MAX_WORKSPACE_TOOL_BYTES} bytes"
                    )));
                }
                let content = tokio::fs::read_to_string(&resolved)
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

    pub async fn replay(&self, source_run_id: &str) -> Result<RunOutcome, RuntimeError> {
        let source = self.store.load_run(source_run_id)?;
        let source_tasks = self.store.list_tasks(source_run_id)?;
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

    async fn drive(
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
            match execution {
                Ok(TaskExecution::Complete { output, memory }) => {
                    self.store.transition_task(
                        run_id,
                        &task.id,
                        TaskState::Succeeded,
                        Some(&output),
                        None,
                        memory.as_ref(),
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
                            () = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
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
        let context = context_for(run, &tasks)?;
        let raw_input = serde_json::to_value(&task.input)?;
        let input = render(&raw_input, &context)?;
        match &task.uses {
            TaskUse::Action(name) => {
                let action = workflow.spec.actions.get(name).ok_or_else(|| {
                    RuntimeError::InvalidState(format!("action `{name}` disappeared after compile"))
                })?;
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
                        let content = tokio::fs::read_to_string(resolved).await;
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
                let before = tokio::fs::read_to_string(&resolved).await.ok();
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
                for (name, reference) in &action.env {
                    policy.authorize_environment(name)?;
                    policy.authorize_environment(&reference.env)?;
                    let value = std::env::var(&reference.env).map_err(|_| {
                        RuntimeError::InvalidState(format!(
                            "required environment variable `{}` is unavailable",
                            reference.env
                        ))
                    })?;
                    environment_digests.insert(
                        name.clone(),
                        serde_json::json!({
                            "source": reference.env,
                            "valueDigest": digest(value.as_bytes()),
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
                            process.env(name, value);
                        }
                        let timeout = Duration::from_secs(
                            action
                                .timeout_seconds
                                .unwrap_or(task_timeout(workflow, &task.task_id)),
                        );
                        let result = tokio::select! {
                            result = tokio::time::timeout(timeout, process.output()) => {
                                match result {
                                    Ok(result) => result.map_err(RuntimeError::Io),
                                    Err(_) => Err(RuntimeError::Task {
                                        task: task.task_id.clone(),
                                        message: "subprocess timed out".to_owned(),
                                    }),
                                }
                            }
                            () = cancellation.cancelled() => Err(RuntimeError::Cancelled),
                        };
                        match result {
                            Ok(result) => {
                                let secrets = resolved_environment
                                    .values()
                                    .map(String::as_str)
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
                            Err(error) => {
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
                        match tokio::fs::read_to_string(resolved).await {
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
                                        self.store.complete_effect(
                                            &tool_effect.id,
                                            Err(&error.to_string()),
                                            self.clock.now(),
                                        )?;
                                        self.store.complete_tool_call(
                                            &run.run_id,
                                            &call_id,
                                            None,
                                            false,
                                            self.clock.now(),
                                        )?;
                                        return Err(RuntimeError::Tool(error));
                                    }
                                    self.store.complete_effect(
                                        &tool_effect.id,
                                        Ok(&result.output),
                                        self.clock.now(),
                                    )?;
                                    let output_digest =
                                        digest(&serde_json::to_vec(&result.output)?);
                                    self.store.complete_tool_call(
                                        &run.run_id,
                                        &call_id,
                                        Some(&output_digest),
                                        true,
                                        self.clock.now(),
                                    )?;
                                    result.output
                                }
                                Ok(Err(ToolContractError::Cancelled))
                                | Err(ToolContractError::Cancelled) => {
                                    self.store.mark_effect_uncertain(
                                        &tool_effect.id,
                                        "tool execution was cancelled after dispatch",
                                        self.clock.now(),
                                    )?;
                                    self.store.mark_tool_call_uncertain(
                                        &run.run_id,
                                        &call_id,
                                        self.clock.now(),
                                    )?;
                                    return Err(RuntimeError::Cancelled);
                                }
                                Err(error) => {
                                    self.store.mark_effect_uncertain(
                                        &tool_effect.id,
                                        &error.to_string(),
                                        self.clock.now(),
                                    )?;
                                    self.store.mark_tool_call_uncertain(
                                        &run.run_id,
                                        &call_id,
                                        self.clock.now(),
                                    )?;
                                    return Err(RuntimeError::Tool(error));
                                }
                                Ok(Err(error)) => {
                                    self.store.complete_effect(
                                        &tool_effect.id,
                                        Err(&error.to_string()),
                                        self.clock.now(),
                                    )?;
                                    self.store.complete_tool_call(
                                        &run.run_id,
                                        &call_id,
                                        None,
                                        false,
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
                    return Ok(TaskExecution::Complete {
                        output: serde_json::json!({"text": response.text, "usage": usage}),
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
        self.store
            .record_effect_request(request, self.clock.now())?;
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
        )?;
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
            provider: (request.effect_class == EffectClass::Model).then(|| tool.to_owned()),
            input: request.input.clone(),
            interactive,
        };
        let decision = match approval {
            ApprovalRequirement::Never => PolicyDecision::Allow {
                reason: "tool contract does not require approval".to_owned(),
            },
            ApprovalRequirement::Always => PolicyDecision::RequireApproval {
                reason: "tool contract always requires approval".to_owned(),
            },
            ApprovalRequirement::Policy => policy.decide(&context),
        };
        match decision {
            PolicyDecision::Allow { .. } => Ok(PreparedEffect::Execute),
            PolicyDecision::Deny { reason } => Err(RuntimeError::Task {
                task: request.task_id.clone(),
                message: format!("policy denied effect: {reason}"),
            }),
            PolicyDecision::RequireApproval { reason } => {
                let approval_id = format!("approval-{}", &request.id[..16]);
                self.store.create_approval(&ApprovalRequest {
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
                })?;
                self.store.transition_task(
                    &request.run_id,
                    &request.task_id,
                    TaskState::WaitingForApproval,
                    None,
                    None,
                    None,
                    self.clock.now(),
                    &request.trace_id,
                )?;
                Ok(PreparedEffect::Paused)
            }
        }
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
        | RuntimeError::Json(_) => false,
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::tempdir;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
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

    fn compile_fixture(source: &str) -> (Workflow, CompiledPlan) {
        let workflow = parse_workflow(source, "fixture.yaml")
            .expect("parse fixture")
            .workflow;
        let plan = compile(&workflow, "fixture.yaml").expect("compile fixture");
        (workflow, plan)
    }

    fn runtime(store: SqliteStore, base: &Path) -> Runtime {
        Runtime::new(store, base)
            .with_clock(Arc::new(FixedClock))
            .with_ids(Arc::new(SequenceIds::default()))
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
        let runtime = runtime(store.clone(), directory.path()).with_registry(
            RuntimeRegistry::default()
                .with_provider("fake", provider.clone())
                .with_tool("echo", Arc::new(FixtureTool::new(false))),
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
        let replay = runtime.replay(&first.run_id).await.expect("replay");
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        assert_eq!(replay.output, first.output);
        assert!(
            store
                .list_effects(&replay.run_id)
                .expect("effects")
                .is_empty()
        );
        assert!(store.tool_calls(&replay.run_id).expect("calls").is_empty());
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
    async fn agent_tool_loop_validates_tool_output_before_model_continuation() {
        let directory = tempdir().expect("tempdir");
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: tools }
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

    #[test]
    fn subprocess_output_redaction_removes_every_known_secret_value() {
        let output = redact_text("token=top-secret; repeated=top-secret", &["top-secret"]);
        assert_eq!(output, "token=[REDACTED]; repeated=[REDACTED]");
    }
}
