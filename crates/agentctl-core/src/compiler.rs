use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::PLAN_FORMAT_VERSION;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::dsl::{
    ActionKind, EffectClass, Idempotency, JsonMap, ProviderKind, RetryDefinition, ToolKind,
    Workflow,
};
use crate::template::{TemplateError, referenced_tasks, validate_expression};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPredictability {
    FullyPredictable,
    PartiallyPredictable,
    RequiresExecution,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledTask {
    pub id: String,
    pub uses: TaskUse,
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_writes: Vec<String>,
    pub when: Option<String>,
    pub vars: JsonMap,
    pub input: JsonMap,
    pub retry: RetryDefinition,
    pub timeout_seconds: u64,
    pub failure: crate::dsl::FailureBehavior,
    pub output_schema: Option<Value>,
    pub predictability: PlanPredictability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum TaskUse {
    Action(String),
    Agent(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledPlan {
    pub format_version: u32,
    pub workflow_name: String,
    pub workflow_digest: String,
    pub plan_digest: String,
    pub order: Vec<String>,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    pub tasks: BTreeMap<String, CompiledTask>,
    pub predictability: PlanPredictability,
    pub requirements: PlanRequirements,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRequirements {
    pub providers: Vec<ProviderRequirement>,
    pub tools: Vec<ToolRequirement>,
    pub effects: Vec<EffectRequirement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequirement {
    pub name: String,
    pub kind: ProviderKind,
    pub agents: Vec<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequirement {
    pub name: String,
    pub capability: String,
    pub effect_class: EffectClass,
    pub risk: crate::dsl::Risk,
    pub approval: crate::dsl::ApprovalRequirement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectRequirement {
    pub task: String,
    pub operation: String,
    pub effect_class: EffectClass,
    pub approval_possible: bool,
    pub predictability: PlanPredictability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderCapability {
    Text,
    FunctionTools,
    StructuredOutput,
    ReasoningEffort,
    ReasoningMode,
    Continuation,
    Usage,
    CostMetadata,
    Cancellation,
    PersistedReasoning,
    PromptCaching,
    MultipleFunctionCalls,
    ResponseStorage,
}

impl ProviderCapability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::FunctionTools => "function_tools",
            Self::StructuredOutput => "structured_output",
            Self::ReasoningEffort => "reasoning_effort",
            Self::ReasoningMode => "reasoning_mode",
            Self::Continuation => "continuation",
            Self::Usage => "usage",
            Self::CostMetadata => "cost_metadata",
            Self::Cancellation => "cancellation",
            Self::PersistedReasoning => "persisted_reasoning",
            Self::PromptCaching => "prompt_caching",
            Self::MultipleFunctionCalls => "multiple_function_calls",
            Self::ResponseStorage => "response_storage",
        }
    }
}

pub fn compile(workflow: &Workflow, file: &str) -> Result<CompiledPlan, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut tasks = BTreeMap::new();
    let mut declaration_order = Vec::new();

    for (position, task) in workflow.spec.tasks.iter().enumerate() {
        if tasks.contains_key(&task.id) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::DuplicateTask,
                    file,
                    format!("duplicate task id `{}`", task.id),
                )
                .with_path(format!("spec.tasks[{position}].id")),
            );
            continue;
        }
        if !valid_identifier(&task.id) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("invalid task id `{}`", task.id),
                )
                .with_path(format!("spec.tasks[{position}].id")),
            );
        }
        let task_use = match parse_use(&task.uses) {
            Some(TaskUse::Action(name)) if workflow.spec.actions.contains_key(&name) => {
                TaskUse::Action(name)
            }
            Some(TaskUse::Agent(name)) if workflow.spec.agents.contains_key(&name) => {
                TaskUse::Agent(name)
            }
            _ => {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::MissingReference,
                        file,
                        format!("task `{}` refers to unknown `{}`", task.id, task.uses),
                    )
                    .with_path(format!("spec.tasks[{position}].uses")),
                );
                continue;
            }
        };
        let mut input = match &task_use {
            TaskUse::Action(name) => workflow
                .spec
                .actions
                .get(name)
                .map(|action| action.defaults.clone())
                .unwrap_or_default(),
            TaskUse::Agent(_) => JsonMap::new(),
        };
        input.extend(task.input.clone());
        let memory_writes = task_memory_writes(
            workflow,
            &task_use,
            &input,
            &task.memory_writes,
            file,
            position,
            &mut diagnostics,
        );
        let mut vars = match &task_use {
            TaskUse::Agent(name) => workflow
                .spec
                .agents
                .get(name)
                .map(|agent| agent.vars.clone())
                .unwrap_or_default(),
            TaskUse::Action(_) => JsonMap::new(),
        };
        vars.extend(task.vars.clone());
        declaration_order.push(task.id.clone());
        tasks.insert(
            task.id.clone(),
            CompiledTask {
                id: task.id.clone(),
                uses: task_use,
                needs: task.needs.clone(),
                memory_writes,
                when: task.when.clone(),
                vars,
                input,
                retry: task.retry.clone(),
                timeout_seconds: task
                    .timeout_seconds
                    .unwrap_or(workflow.spec.runtime.default_timeout_seconds),
                failure: task.failure,
                output_schema: task.output_schema.clone(),
                predictability: PlanPredictability::FullyPredictable,
            },
        );
    }

    for (position, id) in declaration_order.iter().enumerate() {
        let Some(task) = tasks.get(id) else {
            continue;
        };
        for dependency in &task.needs {
            if !tasks.contains_key(dependency) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::MissingReference,
                        file,
                        format!("task `{id}` needs unknown task `{dependency}`"),
                    )
                    .with_path(format!("spec.tasks[{position}].needs")),
                );
            }
        }
        validate_task_templates(task, &tasks, file, position, &mut diagnostics);
        if let Some(schema) = &task.output_schema
            && let Err(error) = jsonschema::validator_for(schema)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("task `{id}` outputSchema is not a valid JSON Schema: {error}"),
                )
                .with_path(format!("spec.tasks[{position}].outputSchema")),
            );
        }
    }

    validate_tools(workflow, file, &mut diagnostics);
    validate_agents(workflow, file, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let order = stable_topological_order(&declaration_order, &tasks).map_err(|cycle| {
        vec![Diagnostic::error(
            DiagnosticCode::DependencyCycle,
            file,
            format!("task dependency cycle: {}", cycle.join(" -> ")),
        )]
    })?;
    validate_parallel_memory_writes(workflow, &order, &tasks, file, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    for task in tasks.values_mut() {
        task.predictability = match &task.uses {
            TaskUse::Agent(_) => PlanPredictability::RequiresExecution,
            TaskUse::Action(name) => workflow
                .spec
                .actions
                .get(name)
                .map_or(PlanPredictability::RequiresExecution, |action| {
                    action_predictability(action.kind)
                }),
        };
    }
    let predictability = tasks
        .values()
        .map(|task| task.predictability)
        .max_by_key(|value| match value {
            PlanPredictability::FullyPredictable => 0,
            PlanPredictability::PartiallyPredictable => 1,
            PlanPredictability::RequiresExecution => 2,
        })
        .unwrap_or(PlanPredictability::FullyPredictable);
    let workflow_json = serde_json::to_vec(workflow).map_err(|error| {
        vec![Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            error.to_string(),
        )]
    })?;
    let workflow_digest = sha256(&workflow_json);
    let requirements = plan_requirements(workflow, &tasks);
    let plan_seed = serde_json::to_vec(&(
        &workflow.metadata.name,
        &workflow_digest,
        &order,
        workflow.spec.runtime.max_concurrency,
        &tasks,
        &requirements,
    ))
    .map_err(|error| {
        vec![Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            error.to_string(),
        )]
    })?;
    let plan_digest = sha256(&plan_seed);

    Ok(CompiledPlan {
        format_version: PLAN_FORMAT_VERSION,
        workflow_name: workflow.metadata.name.clone(),
        workflow_digest,
        plan_digest,
        order,
        max_concurrency: workflow.spec.runtime.max_concurrency,
        tasks,
        predictability,
        requirements,
    })
}

const fn default_max_concurrency() -> usize {
    1
}

#[allow(clippy::too_many_arguments)]
fn task_memory_writes(
    workflow: &Workflow,
    task_use: &TaskUse,
    input: &JsonMap,
    declared: &[String],
    file: &str,
    position: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let path = format!("spec.tasks[{position}].memoryWrites");
    let mut writes = BTreeSet::new();
    for key in declared {
        if key.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "working-memory write keys must not be empty",
                )
                .with_path(path.clone()),
            );
        } else if !writes.insert(key.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("duplicate working-memory write key `{key}`"),
                )
                .with_path(path.clone()),
            );
        }
    }

    let is_memory_write = match task_use {
        TaskUse::Action(name) => workflow
            .spec
            .actions
            .get(name)
            .is_some_and(|action| action.kind == ActionKind::MemoryWrite),
        TaskUse::Agent(_) => false,
    };
    if !is_memory_write {
        if !declared.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "memoryWrites is only valid for builtin.memory.write tasks",
                )
                .with_path(path),
            );
        }
        return writes.into_iter().collect();
    }

    let Some(key) = input.get("key").and_then(Value::as_str) else {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "builtin.memory.write requires a string `key`",
            )
            .with_path(format!("spec.tasks[{position}].with.key")),
        );
        return writes.into_iter().collect();
    };
    if key.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "builtin.memory.write key must not be empty",
            )
            .with_path(format!("spec.tasks[{position}].with.key")),
        );
    } else if key.contains("${{") {
        if writes.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "a templated builtin.memory.write key requires an explicit memoryWrites set",
                )
                .with_path(path),
            );
        }
    } else if writes.is_empty() {
        writes.insert(key.to_owned());
    } else if !writes.contains(key) {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                format!("literal working-memory key `{key}` is missing from memoryWrites"),
            )
            .with_path(path),
        );
    }
    writes.into_iter().collect()
}

fn validate_parallel_memory_writes(
    workflow: &Workflow,
    order: &[String],
    tasks: &BTreeMap<String, CompiledTask>,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if workflow.spec.runtime.max_concurrency == 1 {
        return;
    }
    for (position, left_id) in order.iter().enumerate() {
        let left = &tasks[left_id];
        for right_id in order.iter().skip(position + 1) {
            let right = &tasks[right_id];
            if depends_on(left_id, right_id, tasks) || depends_on(right_id, left_id, tasks) {
                continue;
            }
            let conflicts = left
                .memory_writes
                .iter()
                .filter(|key| right.memory_writes.contains(key))
                .cloned()
                .collect::<Vec<_>>();
            if !conflicts.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "parallel tasks `{left_id}` and `{right_id}` have conflicting working-memory writes: {}",
                            conflicts.join(", ")
                        ),
                    )
                    .with_path("spec.runtime.maxConcurrency")
                    .with_help(
                        "order the tasks with needs or give them disjoint memoryWrites keys",
                    ),
                );
            }
        }
    }
}

fn depends_on(task_id: &str, dependency_id: &str, tasks: &BTreeMap<String, CompiledTask>) -> bool {
    let mut pending = tasks
        .get(task_id)
        .map_or_else(Vec::new, |task| task.needs.clone());
    let mut visited = BTreeSet::new();
    while let Some(candidate) = pending.pop() {
        if candidate == dependency_id {
            return true;
        }
        if visited.insert(candidate.clone())
            && let Some(task) = tasks.get(&candidate)
        {
            pending.extend(task.needs.iter().cloned());
        }
    }
    false
}

fn plan_requirements(
    workflow: &Workflow,
    tasks: &BTreeMap<String, CompiledTask>,
) -> PlanRequirements {
    let providers = workflow
        .spec
        .providers
        .iter()
        .filter_map(|(name, definition)| {
            let agents = workflow
                .spec
                .agents
                .iter()
                .filter(|(_, agent)| agent.provider == *name)
                .map(|(agent, _)| agent.clone())
                .collect::<Vec<_>>();
            (!agents.is_empty()).then(|| ProviderRequirement {
                name: name.clone(),
                kind: definition.kind.clone(),
                agents,
                capabilities: provider_capabilities(definition.kind.clone())
                    .into_iter()
                    .map(|capability| capability.as_str().to_owned())
                    .collect(),
            })
        })
        .collect();
    let tools = workflow
        .spec
        .tools
        .iter()
        .map(|(name, tool)| ToolRequirement {
            name: name.clone(),
            capability: tool.capability.clone(),
            effect_class: tool.effect_class,
            risk: tool.risk,
            approval: tool.approval,
        })
        .collect();
    let effects = tasks
        .values()
        .flat_map(|task| match &task.uses {
            TaskUse::Agent(agent_name) => {
                let mut effects = vec![EffectRequirement {
                    task: task.id.clone(),
                    operation: format!("agent:{agent_name}"),
                    effect_class: EffectClass::Model,
                    approval_possible: workflow.spec.policy.approval
                        != crate::dsl::ApprovalMode::Never,
                    predictability: task.predictability,
                }];
                if let Some(agent) = workflow.spec.agents.get(agent_name) {
                    effects.extend(agent.tools.iter().filter_map(|name| {
                        workflow.spec.tools.get(name).map(|tool| EffectRequirement {
                            task: task.id.clone(),
                            operation: format!("tool:{name}"),
                            effect_class: tool.effect_class,
                            approval_possible: tool.approval
                                != crate::dsl::ApprovalRequirement::Never
                                || workflow.spec.policy.approval != crate::dsl::ApprovalMode::Never,
                            predictability: PlanPredictability::RequiresExecution,
                        })
                    }));
                }
                effects
            }
            TaskUse::Action(name) => {
                let effect_class = workflow
                    .spec
                    .actions
                    .get(name)
                    .map_or(EffectClass::ExternalMutate, |action| {
                        action_effect_class(action.kind)
                    });
                vec![EffectRequirement {
                    task: task.id.clone(),
                    operation: format!("action:{name}"),
                    effect_class,
                    approval_possible: workflow.spec.policy.approval
                        != crate::dsl::ApprovalMode::Never,
                    predictability: task.predictability,
                }]
            }
        })
        .collect();
    PlanRequirements {
        providers,
        tools,
        effects,
    }
}

const fn action_effect_class(kind: ActionKind) -> EffectClass {
    match kind {
        ActionKind::Assign | ActionKind::Assert => EffectClass::Pure,
        ActionKind::Read => EffectClass::Observe,
        ActionKind::Write => EffectClass::WorkspaceMutate,
        ActionKind::ShellExec => EffectClass::ProcessExecution,
        ActionKind::MemoryRead | ActionKind::MemoryWrite => EffectClass::InternalState,
        ActionKind::LongTermMemoryRead => EffectClass::Observe,
        ActionKind::LongTermMemoryWrite => EffectClass::ExternalMutate,
        ActionKind::McpCall => EffectClass::Network,
        ActionKind::A2aDelegate => EffectClass::RemoteAgent,
    }
}

fn validate_task_templates(
    task: &CompiledTask,
    tasks: &BTreeMap<String, CompiledTask>,
    file: &str,
    position: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut values: Vec<&Value> = task.input.values().chain(task.vars.values()).collect();
    if let Some(condition) = &task.when {
        if let Err(error) = validate_expression(condition) {
            push_template_error(error, task, file, position, "when", diagnostics);
        }
    }
    while let Some(value) = values.pop() {
        match value {
            Value::String(template) => {
                if let Err(error) = validate_expression(template) {
                    push_template_error(error, task, file, position, "with", diagnostics);
                }
                for reference in referenced_tasks(template) {
                    if !tasks.contains_key(&reference) {
                        diagnostics.push(
                            Diagnostic::error(
                                DiagnosticCode::MissingReference,
                                file,
                                format!(
                                    "task `{}` template refers to unknown task `{reference}`",
                                    task.id
                                ),
                            )
                            .with_path(format!("spec.tasks[{position}].with")),
                        );
                    } else if !task.needs.contains(&reference) {
                        diagnostics.push(
                            Diagnostic::error(
                                DiagnosticCode::InvalidTemplate,
                                file,
                                format!(
                                    "task `{}` must declare `{reference}` in needs before reading its output",
                                    task.id
                                ),
                            )
                            .with_path(format!("spec.tasks[{position}].with")),
                        );
                    }
                }
            }
            Value::Array(items) => values.extend(items),
            Value::Object(map) => values.extend(map.values()),
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
}

fn push_template_error(
    error: TemplateError,
    task: &CompiledTask,
    file: &str,
    position: usize,
    field: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    diagnostics.push(
        Diagnostic::error(
            DiagnosticCode::InvalidTemplate,
            file,
            format!("task `{}`: {error}", task.id),
        )
        .with_path(format!("spec.tasks[{position}].{field}")),
    );
}

fn validate_agents(workflow: &Workflow, file: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (name, agent) in &workflow.spec.agents {
        let Some(provider) = workflow.spec.providers.get(&agent.provider) else {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::MissingReference,
                    file,
                    format!(
                        "agent `{name}` refers to unknown provider `{}`",
                        agent.provider
                    ),
                )
                .with_path(format!("spec.agents.{name}.provider")),
            );
            continue;
        };
        let capabilities = provider_capabilities(provider.kind.clone());
        validate_provider_options(
            name,
            provider.kind.clone(),
            &agent.provider_options,
            file,
            diagnostics,
        );
        if matches!(
            provider.kind,
            ProviderKind::Openai | ProviderKind::AzureOpenai
        ) && !agent.tools.is_empty()
            && agent.provider_options.get("store") == Some(&Value::Bool(false))
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnsupportedCapability,
                    file,
                    format!(
                        "provider option `store: false` is unsupported for tool-using agent `{name}` because stateless continuation replay is not implemented"
                    ),
                )
                .with_path(format!("spec.agents.{name}.providerOptions.store")),
            );
        }
        let mut required = BTreeSet::from([
            ProviderCapability::Text,
            ProviderCapability::Usage,
            ProviderCapability::Cancellation,
        ]);
        if !agent.tools.is_empty() {
            required.insert(ProviderCapability::FunctionTools);
            required.insert(ProviderCapability::Continuation);
        }
        if agent.structured_output.is_some() {
            required.insert(ProviderCapability::StructuredOutput);
        }
        if let Some(schema) = &agent.structured_output
            && let Err(error) = jsonschema::validator_for(schema)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("agent `{name}` structuredOutput is not a valid JSON Schema: {error}"),
                )
                .with_path(format!("spec.agents.{name}.structuredOutput")),
            );
        }
        if let Some(reasoning) = &agent.reasoning {
            required.insert(ProviderCapability::ReasoningEffort);
            if reasoning.mode.is_some() {
                required.insert(ProviderCapability::ReasoningMode);
            }
        }
        if agent
            .usage_limit
            .as_ref()
            .is_some_and(|limit| limit.max_cost_usd.is_some())
        {
            required.insert(ProviderCapability::CostMetadata);
        }
        if agent.provider_options.contains_key("reasoningContext") {
            required.insert(ProviderCapability::PersistedReasoning);
        }
        if agent.provider_options.contains_key("promptCacheMode")
            || agent.provider_options.contains_key("promptCacheTtl")
        {
            required.insert(ProviderCapability::PromptCaching);
        }
        if agent.provider_options.contains_key("parallelToolCalls") {
            required.insert(ProviderCapability::MultipleFunctionCalls);
        }
        if agent.provider_options.contains_key("store") {
            required.insert(ProviderCapability::ResponseStorage);
        }
        for tool in &agent.tools {
            if !workflow.spec.tools.contains_key(tool) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::MissingReference,
                        file,
                        format!("agent `{name}` refers to unknown tool `{tool}`"),
                    )
                    .with_path(format!("spec.agents.{name}.tools")),
                );
            }
        }
        if let Some(missing) = required.difference(&capabilities).next() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnsupportedCapability,
                    file,
                    format!(
                        "provider `{}` does not support requested capability `{}` for agent `{name}`",
                        agent.provider,
                        missing.as_str()
                    ),
                )
                .with_path(format!("spec.agents.{name}")),
            );
        }
    }
}

fn validate_tools(workflow: &Workflow, file: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (name, tool) in &workflow.spec.tools {
        if !valid_identifier(name) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("tool name `{name}` must start with a letter and contain only letters, digits, `_`, or `-`"),
                )
                .with_path(format!("spec.tools.{name}")),
            );
        }
        for (direction, schema) in [
            ("inputSchema", &tool.input_schema),
            ("outputSchema", &tool.output_schema),
        ] {
            if let Err(error) = jsonschema::validator_for(schema) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("tool `{name}` has invalid {direction}: {error}"),
                    )
                    .with_path(format!("spec.tools.{name}.{direction}")),
                );
            }
        }
        if tool.input_schema.get("type").and_then(Value::as_str) != Some("object")
            || tool
                .input_schema
                .get("additionalProperties")
                .and_then(Value::as_bool)
                != Some(false)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("tool `{name}` inputSchema must be a strict object with additionalProperties: false"),
                )
                .with_path(format!("spec.tools.{name}.inputSchema")),
            );
        }
        if let Some(properties) = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
        {
            let required = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>();
            if properties
                .keys()
                .any(|key| !required.contains(key.as_str()))
            {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("tool `{name}` strict inputSchema must require every property"),
                    )
                    .with_path(format!("spec.tools.{name}.inputSchema.required")),
                );
            }
        }
        let semantic_error = match tool.kind {
            ToolKind::WorkspaceRead
                if tool.effect_class != EffectClass::Observe
                    || tool.idempotency != Idempotency::Idempotent
                    || tool.capability != "filesystem.read" =>
            {
                Some(
                    "builtin.workspace.read requires capability filesystem.read, effectClass observe, and idempotency idempotent",
                )
            }
            ToolKind::WorkspaceWrite
                if tool.effect_class != EffectClass::WorkspaceMutate
                    || !matches!(
                        tool.idempotency,
                        Idempotency::Idempotent | Idempotency::Keyed
                    )
                    || tool.capability != "filesystem.write" =>
            {
                Some(
                    "builtin.workspace.write requires capability filesystem.write, effectClass workspace_mutate, and idempotency idempotent or keyed",
                )
            }
            ToolKind::Echo
                if tool.effect_class != EffectClass::Pure
                    || tool.idempotency != Idempotency::Pure
                    || tool.capability != "internal" =>
            {
                Some(
                    "builtin.echo requires capability internal, effectClass pure, and idempotency pure",
                )
            }
            _ => None,
        };
        if let Some(message) = semantic_error {
            diagnostics.push(
                Diagnostic::error(DiagnosticCode::SchemaViolation, file, message)
                    .with_path(format!("spec.tools.{name}")),
            );
        }
        if !tool.secrets.is_empty() || !tool.network.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnsupportedCapability,
                    file,
                    format!("built-in tool `{name}` cannot declare secret or network requirements"),
                )
                .with_path(format!("spec.tools.{name}")),
            );
        }
    }
}

fn validate_provider_options(
    agent_name: &str,
    kind: ProviderKind,
    options: &JsonMap,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let allowed: &[&str] = match kind {
        ProviderKind::Fake => &["toolInput", "finalText", "delayMs", "failFirst"],
        ProviderKind::Openai | ProviderKind::AzureOpenai => &[
            "store",
            "reasoningContext",
            "promptCacheMode",
            "promptCacheTtl",
            "parallelToolCalls",
            "safetyIdentifier",
        ],
        ProviderKind::Anthropic | ProviderKind::Google => &[],
    };
    for key in options.keys() {
        if !allowed.contains(&key.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnsupportedCapability,
                    file,
                    format!("provider option `{key}` is unsupported for agent `{agent_name}`"),
                )
                .with_path(format!("spec.agents.{agent_name}.providerOptions.{key}")),
            );
        }
    }
    let path = |key: &str| format!("spec.agents.{agent_name}.providerOptions.{key}");
    if let Some(value) = options.get("reasoningContext")
        && !matches!(value.as_str(), Some("auto" | "current_turn" | "all_turns"))
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "reasoningContext must be auto, current_turn, or all_turns",
            )
            .with_path(path("reasoningContext")),
        );
    }
    if let Some(value) = options.get("promptCacheMode")
        && !matches!(value.as_str(), Some("implicit" | "explicit"))
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "promptCacheMode must be implicit or explicit",
            )
            .with_path(path("promptCacheMode")),
        );
    }
    if let Some(value) = options.get("promptCacheTtl")
        && value.as_str() != Some("30m")
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "promptCacheTtl currently supports only 30m",
            )
            .with_path(path("promptCacheTtl")),
        );
    }
    for key in ["store", "parallelToolCalls"] {
        if options.get(key).is_some_and(|value| !value.is_boolean()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("{key} must be a boolean"),
                )
                .with_path(path(key)),
            );
        }
    }
    for key in ["delayMs", "failFirst"] {
        if options.get(key).is_some_and(|value| !value.is_u64()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("{key} must be a non-negative integer"),
                )
                .with_path(path(key)),
            );
        }
    }
    for key in ["finalText", "safetyIdentifier"] {
        if options.get(key).is_some_and(|value| !value.is_string()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("{key} must be a string"),
                )
                .with_path(path(key)),
            );
        }
    }
    if options
        .get("toolInput")
        .is_some_and(|value| !value.is_object())
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "toolInput must be a JSON object",
            )
            .with_path(path("toolInput")),
        );
    }
}

#[must_use]
pub fn provider_capabilities(kind: ProviderKind) -> BTreeSet<ProviderCapability> {
    use ProviderCapability as C;
    let mut values = BTreeSet::from([C::Text, C::Usage, C::Cancellation]);
    match kind {
        ProviderKind::Fake => {
            values.extend([C::FunctionTools, C::StructuredOutput, C::Continuation]);
        }
        ProviderKind::Openai | ProviderKind::AzureOpenai => {
            values.extend([
                C::FunctionTools,
                C::StructuredOutput,
                C::ReasoningEffort,
                C::ReasoningMode,
                C::Continuation,
                C::PersistedReasoning,
                C::PromptCaching,
                C::MultipleFunctionCalls,
                C::ResponseStorage,
            ]);
        }
        ProviderKind::Anthropic => {
            values.extend([
                C::FunctionTools,
                C::StructuredOutput,
                C::ReasoningEffort,
                C::Continuation,
            ]);
        }
        ProviderKind::Google => {
            values.extend([
                C::FunctionTools,
                C::StructuredOutput,
                C::ReasoningEffort,
                C::Continuation,
            ]);
        }
    }
    values
}

fn parse_use(value: &str) -> Option<TaskUse> {
    value
        .strip_prefix("action:")
        .map(|name| TaskUse::Action(name.to_owned()))
        .or_else(|| {
            value
                .strip_prefix("agent:")
                .map(|name| TaskUse::Agent(name.to_owned()))
        })
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic())
        && chars
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn stable_topological_order(
    declaration_order: &[String],
    tasks: &BTreeMap<String, CompiledTask>,
) -> Result<Vec<String>, Vec<String>> {
    let position: BTreeMap<&str, usize> = declaration_order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut incoming: BTreeMap<&str, usize> = tasks
        .iter()
        .map(|(id, task)| (id.as_str(), task.needs.len()))
        .collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, task) in tasks {
        for dependency in &task.needs {
            dependents.entry(dependency).or_default().push(id);
        }
    }
    for items in dependents.values_mut() {
        items.sort_by_key(|id| position.get(id).copied().unwrap_or(usize::MAX));
    }
    let mut ready: VecDeque<&str> = declaration_order
        .iter()
        .filter(|id| incoming.get(id.as_str()).copied() == Some(0))
        .map(String::as_str)
        .collect();
    let mut result = Vec::with_capacity(tasks.len());
    while let Some(id) = ready.pop_front() {
        result.push(id.to_owned());
        if let Some(children) = dependents.get(id) {
            for child in children {
                if let Some(count) = incoming.get_mut(child) {
                    *count -= 1;
                    if *count == 0 {
                        let child_position = position.get(child).copied().unwrap_or(usize::MAX);
                        let insertion = ready
                            .iter()
                            .position(|queued| {
                                position.get(queued).copied().unwrap_or(usize::MAX) > child_position
                            })
                            .unwrap_or(ready.len());
                        ready.insert(insertion, child);
                    }
                }
            }
        }
    }
    if result.len() == tasks.len() {
        Ok(result)
    } else {
        Err(declaration_order
            .iter()
            .filter(|id| !result.contains(id))
            .cloned()
            .collect())
    }
}

const fn action_predictability(kind: ActionKind) -> PlanPredictability {
    match kind {
        ActionKind::Assign
        | ActionKind::Assert
        | ActionKind::MemoryRead
        | ActionKind::MemoryWrite => PlanPredictability::FullyPredictable,
        ActionKind::Read | ActionKind::Write => PlanPredictability::PartiallyPredictable,
        ActionKind::ShellExec
        | ActionKind::LongTermMemoryRead
        | ActionKind::LongTermMemoryWrite
        | ActionKind::McpCall
        | ActionKind::A2aDelegate => PlanPredictability::RequiresExecution,
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_workflow;
    use proptest::prelude::*;

    fn parse(source: &str) -> Workflow {
        parse_workflow(source, "fixture.yaml")
            .expect("fixture parses")
            .workflow
    }

    #[test]
    fn stable_order_uses_declaration_order_for_ready_tasks() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: ordering }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: b, uses: "action:assign" }
    - { id: a, uses: "action:assign" }
    - { id: c, uses: "action:assign", needs: [a, b] }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compiles");
        assert_eq!(plan.order, ["b", "a", "c"]);
    }

    #[test]
    fn parallel_memory_writes_are_inferred_and_conflicts_are_rejected() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-conflict }
spec:
  runtime: { maxConcurrency: 2 }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - { id: left, uses: "action:remember", with: { key: shared, value: left } }
    - { id: right, uses: "action:remember", with: { key: shared, value: right } }
"#,
        );
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("conflict rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("parallel tasks `left` and `right`")
                && diagnostic.message.contains("shared")
        }));
    }

    #[test]
    fn ordered_or_disjoint_parallel_memory_writes_compile() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: parallel-memory }
spec:
  runtime: { maxConcurrency: 3 }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - { id: left, uses: "action:remember", with: { key: left, value: one } }
    - { id: right, uses: "action:remember", with: { key: right, value: two } }
    - { id: ordered, uses: "action:remember", needs: [left], with: { key: left, value: three } }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compiles");
        assert_eq!(plan.tasks["left"].memory_writes, ["left"]);
        assert_eq!(plan.tasks["right"].memory_writes, ["right"]);
        assert_eq!(plan.tasks["ordered"].memory_writes, ["left"]);
    }

    #[test]
    fn templated_memory_key_requires_declared_write_set() {
        let source = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: dynamic-memory-key }
spec:
  inputs:
    selected: { type: string }
  actions:
    remember: { kind: builtin.memory.write }
  tasks:
    - id: remember
      uses: action:remember
      with: { key: "${{ inputs.selected }}", value: kept }
"#;
        let workflow = parse(source);
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("declaration required");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("requires an explicit memoryWrites set")
        }));

        let declared = source.replace(
            "uses: action:remember",
            "uses: action:remember\n      memoryWrites: [selected]",
        );
        compile(&parse(&declared), "fixture.yaml").expect("declared write compiles");
    }

    #[test]
    fn additive_parallel_plan_fields_preserve_old_sequential_plans() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: old-plan }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: one, uses: "action:assign" }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compile");
        let mut json = serde_json::to_value(plan).expect("plan json");
        json.as_object_mut()
            .expect("plan object")
            .remove("maxConcurrency");
        json["tasks"]["one"]
            .as_object_mut()
            .expect("task object")
            .remove("memoryWrites");
        let decoded: CompiledPlan = serde_json::from_value(json).expect("old plan decodes");
        assert_eq!(decoded.max_concurrency, 1);
        assert!(decoded.tasks["one"].memory_writes.is_empty());
    }

    #[test]
    fn rejects_cycle() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: cycle }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: a, uses: "action:assign", needs: [b] }
    - { id: b, uses: "action:assign", needs: [a] }
"#,
        );
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("cycle rejected");
        assert_eq!(diagnostics[0].code, DiagnosticCode::DependencyCycle);
    }

    #[test]
    fn rejects_stateless_openai_continuation_for_tool_using_agents() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: stateless-tools }
spec:
  providers: { openai: { kind: openai } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object }
      outputSchema: { type: object }
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: openai
      model: gpt-5.6
      instructions: use echo
      tools: [echo]
      providerOptions: { store: false }
  tasks: [{ id: work, uses: "agent:worker" }]
"#,
        );
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("must reject");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::UnsupportedCapability
                && diagnostic.message.contains("stateless continuation replay")
        }));
    }

    #[test]
    fn rejects_invalid_task_and_agent_output_contracts() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-output-contracts }
spec:
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: reply
      structuredOutput: { type: definitely-not-a-json-schema-type }
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: assign
      uses: action:assign
      outputSchema: { required: not-an-array }
    - id: work
      uses: agent:worker
"#,
        );
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("schemas must fail");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.path.as_deref() == Some("spec.tasks[0].outputSchema")
                && diagnostic.code == DiagnosticCode::SchemaViolation
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.path.as_deref() == Some("spec.agents.worker.structuredOutput")
                && diagnostic.code == DiagnosticCode::SchemaViolation
        }));
    }

    proptest! {
        #[test]
        fn chain_plans_are_stable(length in 1_usize..32) {
            let tasks = (0..length)
                .map(|index| {
                    let dependency = if index == 0 {
                        String::new()
                    } else {
                        format!("\n      needs: [task-{}]", index - 1)
                    };
                    format!("\n    - id: task-{index}\n      uses: action:assign{dependency}")
                })
                .collect::<String>();
            let source = format!(
                "apiVersion: agentctl.dev/v1alpha1\nkind: Workflow\nmetadata: {{ name: property }}\nspec:\n  actions:\n    assign: {{ kind: builtin.assign }}\n  tasks:{tasks}\n"
            );
            let workflow = parse(&source);
            let first = compile(&workflow, "property.yaml").expect("compile");
            let second = compile(&workflow, "property.yaml").expect("compile again");
            prop_assert_eq!(&first.order, &second.order);
            prop_assert_eq!(&first.plan_digest, &second.plan_digest);
            prop_assert_eq!(first.order.len(), length);
        }
    }
}
