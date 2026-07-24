use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::PLAN_FORMAT_VERSION;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::dsl::{
    ActionKind, EffectClass, Idempotency, JsonMap, MAX_EXPANSION_ITEMS, MAX_LOOP_ITERATIONS,
    ProviderKind, RetryDefinition, TaskDefinition, ToolKind, Workflow,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<CompiledExpansion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_guards: Vec<RouteGuard>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledExpansion {
    pub parent: String,
    pub index: usize,
    pub bindings: JsonMap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledRouter {
    pub select: String,
    pub cases: Vec<CompiledRouteCase>,
    pub default: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledRouteCase {
    pub equals: Value,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteGuard {
    pub router: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledLoopAggregate {
    pub children: Vec<String>,
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledSubworkflowInput {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledSubworkflowAggregate {
    pub name: String,
    pub version: String,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum TaskUse {
    Action(String),
    Agent(String),
    Aggregate(Vec<String>),
    Router(CompiledRouter),
    LoopAggregate(CompiledLoopAggregate),
    SubworkflowInput(CompiledSubworkflowInput),
    SubworkflowAggregate(CompiledSubworkflowAggregate),
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

#[derive(Debug, Clone)]
struct ExpandedTaskDefinition {
    definition: TaskDefinition,
    source_position: usize,
    expansion: Option<CompiledExpansion>,
    synthetic_use: Option<TaskUse>,
}

fn expand_tasks(
    workflow: &Workflow,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
    synthetic_uses: &BTreeMap<String, TaskUse>,
) -> Vec<ExpandedTaskDefinition> {
    let mut expanded = Vec::new();
    for (position, task) in workflow.spec.tasks.iter().enumerate() {
        if let Some(task_use) = synthetic_uses.get(&task.id) {
            expanded.push(ExpandedTaskDefinition {
                definition: task.clone(),
                source_position: position,
                expansion: None,
                synthetic_use: Some(task_use.clone()),
            });
            continue;
        }
        if let Some(loop_definition) = &task.loop_definition {
            let loop_path = format!("spec.tasks[{position}].loop");
            if task.foreach.is_some() || task.matrix.is_some() || task.route.is_some() {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "loop task `{}` cannot also declare foreach, matrix, or route",
                            task.id
                        ),
                    )
                    .with_path(loop_path),
                );
                continue;
            }
            if task.uses == "router" {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("router task `{}` cannot be a loop body", task.id),
                    )
                    .with_path(format!("spec.tasks[{position}].uses")),
                );
                continue;
            }
            if task.when.is_some() {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "loop task `{}` uses loop.while as its iteration guard and cannot also declare when",
                            task.id
                        ),
                    )
                    .with_path(format!("spec.tasks[{position}].when")),
                );
                continue;
            }
            if task.vars.contains_key("loopIndex") || task.vars.contains_key("loopPrevious") {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("loop task `{}` bindings conflict with task vars", task.id),
                    )
                    .with_path(format!("spec.tasks[{position}].vars")),
                );
                continue;
            }
            if loop_definition.max_iterations == 0
                || loop_definition.max_iterations > MAX_LOOP_ITERATIONS
            {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "loop task `{}` maxIterations must be between 1 and {MAX_LOOP_ITERATIONS}",
                            task.id
                        ),
                    )
                    .with_path(format!("spec.tasks[{position}].loop.maxIterations")),
                );
                continue;
            }
            let condition = loop_definition
                .condition
                .replace("loop.output", "vars.loopPrevious");
            if !is_exact_template(&condition) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::InvalidTemplate,
                        file,
                        format!(
                            "loop task `{}` while must be one exact typed condition",
                            task.id
                        ),
                    )
                    .with_path(format!("spec.tasks[{position}].loop.while")),
                );
                continue;
            }
            if let Err(error) = validate_expression(&condition) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::InvalidTemplate,
                        file,
                        format!("loop task `{}`: {error}", task.id),
                    )
                    .with_path(format!("spec.tasks[{position}].loop.while")),
                );
                continue;
            }

            let mut children: Vec<String> = Vec::with_capacity(loop_definition.max_iterations);
            for index in 0..loop_definition.max_iterations {
                let mut bindings = JsonMap::new();
                bindings.insert("loopIndex".to_owned(), Value::from(index));
                bindings.insert(
                    "loopPrevious".to_owned(),
                    if let Some(previous) = children.last() {
                        Value::String(format!("${{{{ tasks.{previous}.output }}}}"))
                    } else {
                        loop_definition.initial.clone()
                    },
                );
                let id = expanded_task_id(&task.id, index, &bindings);
                let mut child = task.clone();
                child.id.clone_from(&id);
                child.loop_definition = None;
                child.vars.extend(bindings.clone());
                child.when = Some(condition.clone());
                if let Some(previous) = children.last() {
                    child.needs.push(previous.clone());
                }
                children.push(id.clone());
                expanded.push(ExpandedTaskDefinition {
                    definition: child,
                    source_position: position,
                    expansion: Some(CompiledExpansion {
                        parent: task.id.clone(),
                        index,
                        bindings,
                    }),
                    synthetic_use: None,
                });
            }

            let mut aggregate = task.clone();
            aggregate.needs.clone_from(&children);
            aggregate.foreach = None;
            aggregate.matrix = None;
            aggregate.route = None;
            aggregate.loop_definition = None;
            aggregate.memory_writes.clear();
            aggregate.when = None;
            aggregate.vars.clear();
            aggregate.input.clear();
            aggregate.retry = RetryDefinition::default();
            aggregate.timeout_seconds = None;
            aggregate.output_schema = None;
            expanded.push(ExpandedTaskDefinition {
                definition: aggregate,
                source_position: position,
                expansion: None,
                synthetic_use: Some(TaskUse::LoopAggregate(CompiledLoopAggregate {
                    children,
                    condition,
                })),
            });
            continue;
        }
        if task.route.is_some() && (task.foreach.is_some() || task.matrix.is_some()) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("router task `{}` cannot be expanded", task.id),
                )
                .with_path(format!("spec.tasks[{position}]")),
            );
            continue;
        }
        let bindings = match (&task.foreach, &task.matrix) {
            (None, None) => {
                expanded.push(ExpandedTaskDefinition {
                    definition: task.clone(),
                    source_position: position,
                    expansion: None,
                    synthetic_use: None,
                });
                continue;
            }
            (Some(_), Some(_)) => {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "task `{}` cannot declare both foreach and matrix expansion",
                            task.id
                        ),
                    )
                    .with_path(format!("spec.tasks[{position}]")),
                );
                continue;
            }
            (Some(foreach), None) => {
                if !valid_identifier(&foreach.binding) {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!("invalid foreach binding `{}`", foreach.binding),
                        )
                        .with_path(format!("spec.tasks[{position}].foreach.as")),
                    );
                    continue;
                }
                if task.vars.contains_key(&foreach.binding)
                    || task.vars.contains_key("foreachIndex")
                {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!(
                                "task `{}` foreach bindings conflict with existing task vars",
                                task.id
                            ),
                        )
                        .with_path(format!("spec.tasks[{position}].vars")),
                    );
                    continue;
                }
                if foreach.max_items == 0
                    || foreach.max_items > MAX_EXPANSION_ITEMS
                    || foreach.items.len() > foreach.max_items
                {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!(
                                "task `{}` foreach expands to {} items, exceeding maxItems {} or framework maximum {MAX_EXPANSION_ITEMS}",
                                task.id,
                                foreach.items.len(),
                                foreach.max_items
                            ),
                        )
                        .with_path(format!("spec.tasks[{position}].foreach")),
                    );
                    continue;
                }
                foreach
                    .items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let mut values = JsonMap::new();
                        values.insert(foreach.binding.clone(), item.clone());
                        values.insert("foreachIndex".to_owned(), Value::from(index));
                        values
                    })
                    .collect::<Vec<_>>()
            }
            (None, Some(matrix)) => {
                if matrix.axes.is_empty() {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!("task `{}` matrix must declare at least one axis", task.id),
                        )
                        .with_path(format!("spec.tasks[{position}].matrix.axes")),
                    );
                    continue;
                }
                if task.vars.contains_key("matrix") || task.vars.contains_key("matrixIndex") {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!(
                                "task `{}` matrix bindings conflict with existing task vars",
                                task.id
                            ),
                        )
                        .with_path(format!("spec.tasks[{position}].vars")),
                    );
                    continue;
                }
                if let Some(axis) = matrix.axes.keys().find(|name| !valid_identifier(name)) {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!("invalid matrix axis `{axis}`"),
                        )
                        .with_path(format!("spec.tasks[{position}].matrix.axes")),
                    );
                    continue;
                }
                let count = matrix
                    .axes
                    .values()
                    .try_fold(1_usize, |count, values| count.checked_mul(values.len()));
                if matrix.max_items == 0
                    || matrix.max_items > MAX_EXPANSION_ITEMS
                    || count.is_none_or(|count| count > matrix.max_items)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::SchemaViolation,
                            file,
                            format!(
                                "task `{}` matrix exceeds maxItems {} or framework maximum {MAX_EXPANSION_ITEMS}",
                                task.id, matrix.max_items
                            ),
                        )
                        .with_path(format!("spec.tasks[{position}].matrix")),
                    );
                    continue;
                }
                let mut combinations = vec![JsonMap::new()];
                for (axis, values) in &matrix.axes {
                    let mut next = Vec::with_capacity(combinations.len() * values.len());
                    for combination in &combinations {
                        for value in values {
                            let mut candidate = combination.clone();
                            candidate.insert(axis.clone(), value.clone());
                            next.push(candidate);
                        }
                    }
                    combinations = next;
                }
                combinations
                    .into_iter()
                    .enumerate()
                    .map(|(index, matrix)| {
                        let mut values = JsonMap::new();
                        values.insert(
                            "matrix".to_owned(),
                            Value::Object(matrix.into_iter().collect()),
                        );
                        values.insert("matrixIndex".to_owned(), Value::from(index));
                        values
                    })
                    .collect::<Vec<_>>()
            }
        };

        let mut children = Vec::with_capacity(bindings.len());
        for (index, values) in bindings.into_iter().enumerate() {
            let id = expanded_task_id(&task.id, index, &values);
            let mut child = task.clone();
            child.id.clone_from(&id);
            child.foreach = None;
            child.matrix = None;
            child.vars.extend(values.clone());
            children.push(id.clone());
            expanded.push(ExpandedTaskDefinition {
                definition: child,
                source_position: position,
                expansion: Some(CompiledExpansion {
                    parent: task.id.clone(),
                    index,
                    bindings: values,
                }),
                synthetic_use: None,
            });
        }

        let mut aggregate = task.clone();
        aggregate.needs.clone_from(&children);
        aggregate.foreach = None;
        aggregate.matrix = None;
        aggregate.route = None;
        aggregate.memory_writes.clear();
        aggregate.when = None;
        aggregate.vars.clear();
        aggregate.input.clear();
        aggregate.retry = RetryDefinition::default();
        aggregate.timeout_seconds = None;
        aggregate.output_schema = None;
        expanded.push(ExpandedTaskDefinition {
            definition: aggregate,
            source_position: position,
            expansion: None,
            synthetic_use: Some(TaskUse::Aggregate(children)),
        });
    }
    expanded
}

fn expanded_task_id(parent: &str, index: usize, bindings: &JsonMap) -> String {
    let encoded = serde_json::to_vec(bindings).unwrap_or_default();
    let digest = sha256(&encoded);
    format!("{parent}--{index:04}-{}", &digest[..12])
}

fn is_exact_template(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("${{")
        && trimmed
            .get(3..)
            .and_then(|value| value.find("}}"))
            .is_some_and(|closing| closing + 3 == trimmed.len() - 2)
}

fn expand_subworkflow_calls(
    workflow: &Workflow,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Workflow, BTreeMap<String, TaskUse>) {
    let mut flattened = workflow.clone();
    let mut tasks = Vec::new();
    let mut synthetic = BTreeMap::new();
    for task in &workflow.spec.tasks {
        instantiate_subworkflow_task(
            workflow,
            task.clone(),
            file,
            &mut Vec::new(),
            &mut tasks,
            &mut synthetic,
            diagnostics,
        );
    }
    flattened.spec.tasks = tasks;
    (flattened, synthetic)
}

#[allow(clippy::too_many_arguments)]
fn instantiate_subworkflow_task(
    workflow: &Workflow,
    task: TaskDefinition,
    file: &str,
    stack: &mut Vec<String>,
    tasks: &mut Vec<TaskDefinition>,
    synthetic: &mut BTreeMap<String, TaskUse>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(name) = task.uses.strip_prefix("workflow:") else {
        tasks.push(task);
        return;
    };
    let Some(definition) = workflow.spec.subworkflows.get(name) else {
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::MissingReference,
            file,
            format!(
                "sub-workflow invocation `{}` refers to unknown workflow `{name}`",
                task.id
            ),
        ));
        return;
    };
    if stack.iter().any(|active| active == name) {
        let mut cycle = stack.clone();
        cycle.push(name.to_owned());
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::DependencyCycle,
            file,
            format!("sub-workflow cycle: {}", cycle.join(" -> ")),
        ));
        return;
    }
    if semver::Version::parse(&definition.version).is_err() {
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            format!("sub-workflow `{name}` version must be semantic versioning"),
        ));
        return;
    }
    for (label, schema) in [
        ("inputSchema", &definition.input_schema),
        ("outputSchema", &definition.output_schema),
    ] {
        if let Err(error) = jsonschema::validator_for(schema) {
            diagnostics.push(Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                format!("sub-workflow `{name}` {label} is not valid JSON Schema: {error}"),
            ));
            return;
        }
    }
    if task.foreach.is_some()
        || task.matrix.is_some()
        || task.route.is_some()
        || task.loop_definition.is_some()
        || task.when.is_some()
        || !task.vars.is_empty()
        || !task.memory_writes.is_empty()
        || task.retry != RetryDefinition::default()
        || task.timeout_seconds.is_some()
        || task.output_schema.is_some()
    {
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            format!(
                "sub-workflow invocation `{}` accepts needs, with, and failure only",
                task.id
            ),
        ));
        return;
    }

    let identity = sha256(format!("{name}@{}", definition.version).as_bytes());
    let input_id = format!("{}--inputs-{}", task.id, &identity[..8]);
    let namespace = format!("{}--", task.id);
    let local_ids = definition
        .tasks
        .iter()
        .map(|child| (child.id.clone(), format!("{namespace}{}", child.id)))
        .collect::<BTreeMap<_, _>>();
    if local_ids.values().any(|id| id == &input_id) {
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::DuplicateTask,
            file,
            format!("sub-workflow `{name}` produces reserved task ID `{input_id}`"),
        ));
        return;
    }

    let mut values = definition.inputs.clone();
    values.extend(task.input.clone());
    let mut input_boundary = synthetic_task(&task, input_id.clone(), task.needs.clone());
    input_boundary.input = values;
    input_boundary.output_schema = Some(definition.input_schema.clone());
    tasks.push(input_boundary);
    synthetic.insert(
        input_id.clone(),
        TaskUse::SubworkflowInput(CompiledSubworkflowInput {
            name: name.to_owned(),
            version: definition.version.clone(),
        }),
    );

    stack.push(name.to_owned());
    for definition_task in &definition.tasks {
        let mut child = definition_task.clone();
        child.id = local_ids[&definition_task.id].clone();
        child.needs = definition_task
            .needs
            .iter()
            .map(|needed| {
                local_ids
                    .get(needed)
                    .cloned()
                    .unwrap_or_else(|| needed.clone())
            })
            .collect();
        if !child.needs.contains(&input_id) {
            child.needs.push(input_id.clone());
        }
        rewrite_subworkflow_task(
            &mut child,
            &local_ids,
            &input_id,
            &format!("{}__", task.id.replace('-', "_")),
        );
        namespace_subworkflow_action_state(
            workflow,
            &mut child,
            &format!("{}__", task.id.replace('-', "_")),
            file,
            diagnostics,
        );
        instantiate_subworkflow_task(workflow, child, file, stack, tasks, synthetic, diagnostics);
    }
    stack.pop();

    let children = definition
        .tasks
        .iter()
        .map(|child| local_ids[&child.id].clone())
        .collect::<Vec<_>>();
    let outputs = rewrite_subworkflow_value(
        &Value::Object(definition.outputs.clone().into_iter().collect()),
        &local_ids,
        &input_id,
        &format!("{}__", task.id.replace('-', "_")),
    );
    let mut aggregate_needs = children.clone();
    aggregate_needs.push(input_id);
    let mut aggregate = synthetic_task(&task, task.id.clone(), aggregate_needs);
    aggregate.input = outputs
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    aggregate.output_schema = Some(definition.output_schema.clone());
    tasks.push(aggregate);
    synthetic.insert(
        task.id.clone(),
        TaskUse::SubworkflowAggregate(CompiledSubworkflowAggregate {
            name: name.to_owned(),
            version: definition.version.clone(),
            children,
        }),
    );
}

fn synthetic_task(source: &TaskDefinition, id: String, needs: Vec<String>) -> TaskDefinition {
    let mut task = source.clone();
    task.id = id;
    task.needs = needs;
    task.foreach = None;
    task.matrix = None;
    task.route = None;
    task.loop_definition = None;
    task.memory_writes.clear();
    task.when = None;
    task.vars.clear();
    task.input.clear();
    task.retry = RetryDefinition::default();
    task.timeout_seconds = None;
    task.output_schema = None;
    task
}

fn namespace_subworkflow_action_state(
    workflow: &Workflow,
    task: &mut TaskDefinition,
    prefix: &str,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(name) = task.uses.strip_prefix("action:") else {
        return;
    };
    let Some(action) = workflow.spec.actions.get(name) else {
        return;
    };
    let field = match action.kind {
        ActionKind::MemoryRead | ActionKind::MemoryWrite => "key",
        ActionKind::LongTermMemoryRead | ActionKind::LongTermMemoryWrite => "namespace",
        _ => return,
    };
    let Some(Value::String(value)) = task.input.get_mut(field) else {
        return;
    };
    if value.contains("${{") {
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnsupportedCapability,
            file,
            format!(
                "sub-workflow task `{}` requires a static `{field}` for isolated state",
                task.id
            ),
        ));
    } else {
        value.insert_str(0, prefix);
    }
}

fn rewrite_subworkflow_task(
    task: &mut TaskDefinition,
    local_ids: &BTreeMap<String, String>,
    input_id: &str,
    memory_prefix: &str,
) {
    task.when = task
        .when
        .as_ref()
        .map(|value| rewrite_subworkflow_string(value, local_ids, input_id, memory_prefix));
    task.vars = rewrite_subworkflow_map(&task.vars, local_ids, input_id, memory_prefix);
    task.input = rewrite_subworkflow_map(&task.input, local_ids, input_id, memory_prefix);
    task.memory_writes = task
        .memory_writes
        .iter()
        .map(|key| format!("{memory_prefix}{key}"))
        .collect();
    if let Some(foreach) = &mut task.foreach {
        foreach.items = foreach
            .items
            .iter()
            .map(|value| rewrite_subworkflow_value(value, local_ids, input_id, memory_prefix))
            .collect();
    }
    if let Some(matrix) = &mut task.matrix {
        for values in matrix.axes.values_mut() {
            *values = values
                .iter()
                .map(|value| rewrite_subworkflow_value(value, local_ids, input_id, memory_prefix))
                .collect();
        }
    }
    if let Some(route) = &mut task.route {
        route.select =
            rewrite_subworkflow_string(&route.select, local_ids, input_id, memory_prefix);
        for case in &mut route.cases {
            case.tasks = case
                .tasks
                .iter()
                .map(|id| local_ids.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect();
        }
        route.default = route
            .default
            .iter()
            .map(|id| local_ids.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect();
    }
    if let Some(loop_definition) = &mut task.loop_definition {
        loop_definition.condition = rewrite_subworkflow_string(
            &loop_definition.condition,
            local_ids,
            input_id,
            memory_prefix,
        );
        loop_definition.initial =
            rewrite_subworkflow_value(&loop_definition.initial, local_ids, input_id, memory_prefix);
    }
}

fn rewrite_subworkflow_map(
    values: &JsonMap,
    local_ids: &BTreeMap<String, String>,
    input_id: &str,
    memory_prefix: &str,
) -> JsonMap {
    values
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                rewrite_subworkflow_value(value, local_ids, input_id, memory_prefix),
            )
        })
        .collect()
}

fn rewrite_subworkflow_value(
    value: &Value,
    local_ids: &BTreeMap<String, String>,
    input_id: &str,
    memory_prefix: &str,
) -> Value {
    match value {
        Value::String(value) => Value::String(rewrite_subworkflow_string(
            value,
            local_ids,
            input_id,
            memory_prefix,
        )),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| rewrite_subworkflow_value(value, local_ids, input_id, memory_prefix))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        rewrite_subworkflow_value(value, local_ids, input_id, memory_prefix),
                    )
                })
                .collect(),
        ),
        value => value.clone(),
    }
}

fn rewrite_subworkflow_string(
    value: &str,
    local_ids: &BTreeMap<String, String>,
    input_id: &str,
    memory_prefix: &str,
) -> String {
    let mut rewritten = value.replace("inputs.", &format!("tasks.{input_id}.output."));
    rewritten = rewritten.replace("memory.", &format!("memory.{memory_prefix}"));
    for (local, namespaced) in local_ids {
        rewritten = rewritten.replace(&format!("tasks.{local}."), &format!("tasks.{namespaced}."));
    }
    rewritten
}

pub fn compile(workflow: &Workflow, file: &str) -> Result<CompiledPlan, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut tasks = BTreeMap::new();
    let mut declaration_order = Vec::new();
    let mut source_positions = BTreeMap::new();
    let (workflow, synthetic_uses) = expand_subworkflow_calls(workflow, file, &mut diagnostics);
    let expanded_tasks = expand_tasks(&workflow, file, &mut diagnostics, &synthetic_uses);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    for expanded in &expanded_tasks {
        let position = expanded.source_position;
        let task = &expanded.definition;
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
        let task_use = if let Some(synthetic_use) = &expanded.synthetic_use {
            synthetic_use.clone()
        } else if task.uses == "router" {
            let Some(route) = &task.route else {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("router task `{}` requires route configuration", task.id),
                    )
                    .with_path(format!("spec.tasks[{position}].route")),
                );
                continue;
            };
            TaskUse::Router(CompiledRouter {
                select: route.select.clone(),
                cases: route
                    .cases
                    .iter()
                    .map(|case| CompiledRouteCase {
                        equals: case.equals.clone(),
                        tasks: case.tasks.clone(),
                    })
                    .collect(),
                default: route.default.clone(),
            })
        } else {
            if task.route.is_some() {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "task `{}` declares route configuration but does not use `router`",
                            task.id
                        ),
                    )
                    .with_path(format!("spec.tasks[{position}].route")),
                );
                continue;
            }
            match parse_use(&task.uses) {
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
            }
        };
        let mut input = match &task_use {
            TaskUse::Action(name) => workflow
                .spec
                .actions
                .get(name)
                .map(|action| action.defaults.clone())
                .unwrap_or_default(),
            TaskUse::Agent(_)
            | TaskUse::Aggregate(_)
            | TaskUse::Router(_)
            | TaskUse::LoopAggregate(_)
            | TaskUse::SubworkflowInput(_)
            | TaskUse::SubworkflowAggregate(_) => JsonMap::new(),
        };
        input.extend(task.input.clone());
        let memory_writes = if matches!(
            task_use,
            TaskUse::Aggregate(_)
                | TaskUse::Router(_)
                | TaskUse::LoopAggregate(_)
                | TaskUse::SubworkflowInput(_)
                | TaskUse::SubworkflowAggregate(_)
        ) {
            Vec::new()
        } else {
            task_memory_writes(
                &workflow,
                &task_use,
                &input,
                &task.memory_writes,
                file,
                position,
                &mut diagnostics,
            )
        };
        let mut vars = match &task_use {
            TaskUse::Agent(name) => workflow
                .spec
                .agents
                .get(name)
                .map(|agent| agent.vars.clone())
                .unwrap_or_default(),
            TaskUse::Action(_)
            | TaskUse::Aggregate(_)
            | TaskUse::Router(_)
            | TaskUse::LoopAggregate(_)
            | TaskUse::SubworkflowInput(_)
            | TaskUse::SubworkflowAggregate(_) => JsonMap::new(),
        };
        vars.extend(task.vars.clone());
        declaration_order.push(task.id.clone());
        source_positions.insert(task.id.clone(), position);
        tasks.insert(
            task.id.clone(),
            CompiledTask {
                id: task.id.clone(),
                uses: task_use,
                needs: task.needs.clone(),
                expansion: expanded.expansion.clone(),
                route_guards: Vec::new(),
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

    for id in &declaration_order {
        let Some(task) = tasks.get(id) else {
            continue;
        };
        let position = source_positions.get(id).copied().unwrap_or_default();
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

    validate_routers(&mut tasks, &source_positions, file, &mut diagnostics);
    validate_tools(&workflow, file, &mut diagnostics);
    validate_agents(&workflow, file, &mut diagnostics);
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
    validate_parallel_memory_writes(&workflow, &order, &tasks, file, &mut diagnostics);
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
            TaskUse::Aggregate(_) => PlanPredictability::FullyPredictable,
            TaskUse::Router(_) => PlanPredictability::FullyPredictable,
            TaskUse::LoopAggregate(_) => PlanPredictability::FullyPredictable,
            TaskUse::SubworkflowInput(_) | TaskUse::SubworkflowAggregate(_) => {
                PlanPredictability::FullyPredictable
            }
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
    let workflow_json = serde_json::to_vec(&workflow).map_err(|error| {
        vec![Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            error.to_string(),
        )]
    })?;
    let workflow_digest = sha256(&workflow_json);
    let requirements = plan_requirements(&workflow, &tasks);
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
        TaskUse::Agent(_)
        | TaskUse::Aggregate(_)
        | TaskUse::Router(_)
        | TaskUse::LoopAggregate(_)
        | TaskUse::SubworkflowInput(_)
        | TaskUse::SubworkflowAggregate(_) => false,
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

fn validate_routers(
    tasks: &mut BTreeMap<String, CompiledTask>,
    source_positions: &BTreeMap<String, usize>,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let routers = tasks
        .iter()
        .filter_map(|(id, task)| match &task.uses {
            TaskUse::Router(router) => Some((id.clone(), router.clone(), task.needs.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (router_id, router, needs) in routers {
        let position = source_positions
            .get(&router_id)
            .copied()
            .unwrap_or_default();
        let route_path = format!("spec.tasks[{position}].route");
        if !is_exact_template(&router.select) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidTemplate,
                    file,
                    format!("router task `{router_id}` select must be one exact typed template"),
                )
                .with_path(format!("{route_path}.select")),
            );
        } else if let Err(error) = validate_expression(&router.select) {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidTemplate,
                    file,
                    format!("router task `{router_id}`: {error}"),
                )
                .with_path(format!("{route_path}.select")),
            );
        }
        for reference in referenced_tasks(&router.select) {
            if !tasks.contains_key(&reference) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::MissingReference,
                        file,
                        format!(
                            "router task `{router_id}` selector refers to unknown task `{reference}`"
                        ),
                    )
                    .with_path(format!("{route_path}.select")),
                );
            } else if !needs.contains(&reference) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::InvalidTemplate,
                        file,
                        format!(
                            "router task `{router_id}` must declare `{reference}` in needs before selecting from its output"
                        ),
                    )
                    .with_path(format!("{route_path}.select")),
                );
            }
        }
        if router.cases.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("router task `{router_id}` requires at least one case"),
                )
                .with_path(format!("{route_path}.cases")),
            );
        }
        for (index, case) in router.cases.iter().enumerate() {
            if case.tasks.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!("router task `{router_id}` case {index} has no destinations"),
                    )
                    .with_path(format!("{route_path}.cases[{index}].tasks")),
                );
            }
            if router.cases[..index]
                .iter()
                .any(|previous| previous.equals == case.equals)
            {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "router task `{router_id}` has duplicate typed case value {}",
                            case.equals
                        ),
                    )
                    .with_path(format!("{route_path}.cases[{index}].equals")),
                );
            }
        }

        let mut destinations = BTreeSet::new();
        for destination in router
            .cases
            .iter()
            .flat_map(|case| case.tasks.iter())
            .chain(router.default.iter())
        {
            if !destinations.insert(destination.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "router task `{router_id}` destination `{destination}` is declared more than once"
                        ),
                    )
                    .with_path(route_path.clone()),
                );
                continue;
            }
            let Some(target) = tasks.get(destination) else {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::MissingReference,
                        file,
                        format!(
                            "router task `{router_id}` refers to unknown destination `{destination}`"
                        ),
                    )
                    .with_path(route_path.clone()),
                );
                continue;
            };
            if !target.needs.contains(&router_id) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        format!(
                            "routed task `{destination}` must declare router `{router_id}` in needs"
                        ),
                    )
                    .with_path(route_path.clone()),
                );
                continue;
            }
            if let Some(target) = tasks.get_mut(destination) {
                target.route_guards.push(RouteGuard {
                    router: router_id.clone(),
                });
            }
        }
    }
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
            TaskUse::Aggregate(_)
            | TaskUse::Router(_)
            | TaskUse::LoopAggregate(_)
            | TaskUse::SubworkflowInput(_)
            | TaskUse::SubworkflowAggregate(_) => Vec::new(),
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
    fn foreach_and_matrix_expand_to_stable_children_and_aggregates() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: expansion }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: each
      uses: "action:assign"
      foreach:
        items: [alpha, beta]
        as: value
        maxItems: 2
      with: { value: "${{ vars.value }}", index: "${{ vars.foreachIndex }}" }
    - id: combinations
      uses: "action:assign"
      matrix:
        axes:
          os: [linux, macos]
          tier: [small, large]
        maxItems: 4
      with:
        os: "${{ vars.matrix.os }}"
        tier: "${{ vars.matrix.tier }}"
        index: "${{ vars.matrixIndex }}"
    - { id: done, uses: "action:assign", needs: [each, combinations] }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compiles");
        let second = compile(&workflow, "fixture.yaml").expect("compiles deterministically");
        assert_eq!(plan, second);

        let each_children = match &plan.tasks["each"].uses {
            TaskUse::Aggregate(children) => children,
            other => panic!("expected foreach aggregate, got {other:?}"),
        };
        assert_eq!(each_children.len(), 2);
        assert!(each_children[0].starts_with("each--0000-"));
        assert_eq!(
            plan.tasks[&each_children[1]]
                .expansion
                .as_ref()
                .expect("expansion")
                .bindings["value"],
            Value::String("beta".to_owned())
        );
        assert_eq!(plan.tasks["each"].needs, *each_children);

        let matrix_children = match &plan.tasks["combinations"].uses {
            TaskUse::Aggregate(children) => children,
            other => panic!("expected matrix aggregate, got {other:?}"),
        };
        assert_eq!(matrix_children.len(), 4);
        assert_eq!(
            plan.tasks[&matrix_children[2]]
                .expansion
                .as_ref()
                .expect("expansion")
                .bindings["matrix"],
            serde_json::json!({"os": "macos", "tier": "small"})
        );
        assert_eq!(
            plan.tasks["done"].needs,
            ["each".to_owned(), "combinations".to_owned()]
        );
    }

    #[test]
    fn expansion_bounds_and_binding_collisions_are_rejected() {
        let too_many = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: expansion-bound }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: each
      uses: "action:assign"
      foreach: { items: [a, b], maxItems: 1 }
"#,
        );
        let diagnostics = compile(&too_many, "fixture.yaml").expect_err("bound rejected");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("foreach expands to 2 items"))
        );

        let collision = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: expansion-collision }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: each
      uses: "action:assign"
      vars: { item: existing }
      foreach: { items: [a] }
"#,
        );
        let diagnostics = compile(&collision, "fixture.yaml").expect_err("collision rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("foreach bindings conflict with existing task vars")
        }));
    }

    #[test]
    fn bounded_loop_expands_to_stable_sequential_iteration_tasks() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: bounded-loop }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: refine
      uses: "action:assign"
      loop:
        maxIterations: 3
        while: "${{ vars.loopIndex < 2 }}"
        initial: { value: seed }
      with:
        previous: "${{ vars.loopPrevious }}"
        iteration: "${{ vars.loopIndex }}"
    - { id: done, uses: "action:assign", needs: [refine] }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compiles");
        let second = compile(&workflow, "fixture.yaml").expect("compiles deterministically");
        assert_eq!(plan, second);

        let loop_aggregate = match &plan.tasks["refine"].uses {
            TaskUse::LoopAggregate(loop_aggregate) => loop_aggregate,
            other => panic!("expected loop aggregate, got {other:?}"),
        };
        assert_eq!(loop_aggregate.children.len(), 3);
        assert_eq!(loop_aggregate.condition, "${{ vars.loopIndex < 2 }}");
        assert!(loop_aggregate.children[0].starts_with("refine--0000-"));
        assert_eq!(
            plan.tasks[&loop_aggregate.children[0]].vars["loopPrevious"],
            serde_json::json!({"value": "seed"})
        );
        assert_eq!(
            plan.tasks[&loop_aggregate.children[1]].vars["loopPrevious"],
            Value::String(format!(
                "${{{{ tasks.{}.output }}}}",
                loop_aggregate.children[0]
            ))
        );
        assert_eq!(
            plan.tasks[&loop_aggregate.children[1]].needs,
            [loop_aggregate.children[0].clone()]
        );
        assert_eq!(
            plan.tasks[&loop_aggregate.children[2]].needs,
            [loop_aggregate.children[1].clone()]
        );
        assert_eq!(plan.tasks["refine"].needs, loop_aggregate.children);
        assert_eq!(plan.tasks["done"].needs, ["refine"]);
    }

    #[test]
    fn bounded_loop_rejects_ambiguous_or_invalid_declarations() {
        let invalid = [
            (
                r#"loop: { maxIterations: 2, while: "prefix ${{ vars.loopIndex < 1 }}" }"#,
                "while must be one exact typed condition",
            ),
            (
                r#"loop: { maxIterations: 2, while: "${{ vars.loopIndex < \"two\" }}" }"#,
                "unsupported expression",
            ),
        ];
        for (loop_definition, expected) in invalid {
            let source = format!(
                r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: invalid-loop }}
spec:
  actions:
    assign: {{ kind: builtin.assign }}
  tasks:
    - id: refine
      uses: "action:assign"
      {loop_definition}
"#
            );
            let workflow = parse(&source);
            let diagnostics = compile(&workflow, "fixture.yaml").expect_err("loop rejected");
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "{diagnostics:?}"
            );
        }

        let collision = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-loop-collision }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: refine
      uses: "action:assign"
      vars: { loopIndex: 1 }
      when: "${{ inputs.enabled }}"
      foreach: { items: [a] }
      loop: { maxIterations: 2, while: "${{ vars.loopIndex < 1 }}" }
"#,
        );
        let diagnostics = compile(&collision, "fixture.yaml").expect_err("loop rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("cannot also declare foreach, matrix, or route")
        }));

        let existing_when = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-loop-when }
spec:
  inputs: { enabled: true }
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: refine
      uses: "action:assign"
      when: "${{ inputs.enabled }}"
      loop: { maxIterations: 2, while: "${{ vars.loopIndex < 1 }}" }
"#,
        );
        let diagnostics = compile(&existing_when, "fixture.yaml").expect_err("loop rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("uses loop.while as its iteration guard")
        }));

        let binding_collision = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-loop-binding }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: refine
      uses: "action:assign"
      vars: { loopPrevious: existing }
      loop: { maxIterations: 2, while: "${{ vars.loopIndex < 1 }}" }
"#,
        );
        let diagnostics = compile(&binding_collision, "fixture.yaml").expect_err("loop rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("bindings conflict with task vars")
        }));
    }

    #[test]
    fn subworkflow_compiles_to_typed_namespaced_boundaries() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: subworkflow }
spec:
  inputs: { message: hello }
  actions:
    assign: { kind: builtin.assign }
  subworkflows:
    summarize:
      version: 1.2.0
      inputs: { message: default }
      inputSchema:
        type: object
        required: [message]
        additionalProperties: false
        properties: { message: { type: string } }
      outputs:
        result: "${{ tasks.second.output.output.value }}"
      outputSchema:
        type: object
        required: [result]
        additionalProperties: false
        properties: { result: { type: string } }
      tasks:
        - { id: first, uses: "action:assign", with: { value: "${{ inputs.message }}" } }
        - id: second
          uses: action:assign
          needs: [first]
          with: { value: "${{ tasks.first.output.output.value }}" }
  tasks:
    - id: summary
      uses: workflow:summarize
      with: { message: "${{ inputs.message }}" }
    - { id: done, uses: "action:assign", needs: [summary] }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("sub-workflow compiles");
        let second = compile(&workflow, "fixture.yaml").expect("stable expansion");
        assert_eq!(plan, second);
        assert_eq!(plan.order.len(), 5);
        let input_id = plan
            .order
            .iter()
            .find(|id| id.starts_with("summary--inputs-"))
            .expect("input boundary")
            .clone();
        assert!(matches!(
            plan.tasks[&input_id].uses,
            TaskUse::SubworkflowInput(_)
        ));
        assert_eq!(
            plan.tasks["summary--first"].input["value"],
            Value::String(format!("${{{{ tasks.{input_id}.output.message }}}}"))
        );
        assert_eq!(
            plan.tasks["summary--second"].needs,
            ["summary--first".to_owned(), input_id.clone()]
        );
        let aggregate = match &plan.tasks["summary"].uses {
            TaskUse::SubworkflowAggregate(aggregate) => aggregate,
            other => panic!("expected sub-workflow aggregate, got {other:?}"),
        };
        assert_eq!(aggregate.name, "summarize");
        assert_eq!(aggregate.version, "1.2.0");
        assert_eq!(aggregate.children, ["summary--first", "summary--second"]);
        assert_eq!(
            plan.tasks["summary"].needs,
            [
                "summary--first".to_owned(),
                "summary--second".to_owned(),
                input_id.clone()
            ]
        );
        assert_eq!(
            plan.tasks["summary"].input["result"],
            "${{ tasks.summary--second.output.output.value }}"
        );
    }

    #[test]
    fn subworkflow_rejects_cycles_and_invalid_versions() {
        let cycle = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: subworkflow-cycle }
spec:
  subworkflows:
    first:
      version: 1.0.0
      inputSchema: { type: object }
      outputSchema: { type: object }
      tasks: [{ id: nested, uses: "workflow:second" }]
    second:
      version: 1.0.0
      inputSchema: { type: object }
      outputSchema: { type: object }
      tasks: [{ id: nested, uses: "workflow:first" }]
  tasks: [{ id: invoke, uses: "workflow:first" }]
"#,
        );
        let diagnostics = compile(&cycle, "fixture.yaml").expect_err("cycle rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("sub-workflow cycle: first -> second -> first")
        }));

        let invalid = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-version }
spec:
  subworkflows:
    broken:
      version: latest
      inputSchema: { type: object }
      outputSchema: { type: object }
      tasks: []
  tasks: [{ id: invoke, uses: "workflow:broken" }]
"#,
        );
        let diagnostics = compile(&invalid, "fixture.yaml").expect_err("version rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("version must be semantic versioning")
        }));

        let output_override = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: output-override }
spec:
  subworkflows:
    typed:
      version: 1.0.0
      inputSchema: { type: object }
      outputSchema: { type: object }
      tasks: []
  tasks:
    - id: invoke
      uses: workflow:typed
      outputSchema: { type: string }
"#,
        );
        let diagnostics = compile(&output_override, "fixture.yaml").expect_err("override rejected");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("accepts needs, with, and failure only")
        }));
    }

    #[test]
    fn subworkflow_working_memory_is_namespaced_per_invocation() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: isolated-subworkflows }
spec:
  runtime: { maxConcurrency: 2 }
  actions:
    remember: { kind: builtin.memory.write }
  subworkflows:
    remember:
      version: 1.0.0
      inputSchema: { type: object }
      outputSchema: { type: object }
      tasks:
        - id: write
          uses: action:remember
          with: { key: result, value: stored }
  tasks:
    - { id: left, uses: "workflow:remember" }
    - { id: right, uses: "workflow:remember" }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("isolated calls compile");
        assert_eq!(plan.tasks["left--write"].memory_writes, ["left__result"]);
        assert_eq!(plan.tasks["right--write"].memory_writes, ["right__result"]);
        assert_eq!(plan.tasks["left--write"].input["key"], "left__result");
        assert_eq!(plan.tasks["right--write"].input["key"], "right__result");
    }

    #[test]
    fn typed_router_cases_compile_to_explicit_destination_guards() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: typed-router }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: decide, uses: "action:assign", with: { route: ship } }
    - id: route
      uses: router
      needs: [decide]
      route:
        select: "${{ tasks.decide.output.output.route }}"
        cases:
          - { equals: ship, tasks: [ship] }
          - { equals: 1, tasks: [numeric] }
        default: [hold]
    - { id: ship, uses: "action:assign", needs: [route] }
    - { id: numeric, uses: "action:assign", needs: [route] }
    - { id: hold, uses: "action:assign", needs: [route] }
"#,
        );
        let plan = compile(&workflow, "fixture.yaml").expect("compiles");
        let router = match &plan.tasks["route"].uses {
            TaskUse::Router(router) => router,
            other => panic!("expected router, got {other:?}"),
        };
        assert_eq!(router.cases[0].equals, "ship");
        assert_eq!(router.cases[1].equals, 1);
        assert_eq!(
            plan.tasks["ship"].route_guards,
            [RouteGuard {
                router: "route".to_owned()
            }]
        );
        assert_eq!(plan.order, ["decide", "route", "ship", "numeric", "hold"]);
    }

    #[test]
    fn router_rejects_ambiguous_cases_and_implicit_dependencies() {
        let workflow = parse(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: invalid-router }
spec:
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: decide, uses: "action:assign", with: { route: ship } }
    - id: route
      uses: router
      needs: [decide]
      route:
        select: "${{ tasks.decide.output.output.route }}"
        cases:
          - { equals: ship, tasks: [ship] }
          - { equals: ship, tasks: [ship] }
    - { id: ship, uses: "action:assign" }
"#,
        );
        let diagnostics = compile(&workflow, "fixture.yaml").expect_err("router rejected");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate typed case value"))
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("must declare router `route` in needs")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("destination `ship` is declared more than once")
        }));
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
