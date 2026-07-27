use std::collections::BTreeMap;
use std::path::Path;

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::pack::{PackSignature, PackSource};

pub const API_VERSION: &str = "agentctl.dev/v1alpha1";

pub type JsonMap = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workflow {
    pub api_version: String,
    pub kind: WorkflowKind,
    pub metadata: Metadata,
    pub spec: WorkflowSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum WorkflowKind {
    Workflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowSpec {
    #[serde(default)]
    pub inputs: JsonMap,
    #[serde(default)]
    pub outputs: JsonMap,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderDefinition>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentDefinition>,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDefinition>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolDefinition>,
    #[serde(default)]
    pub subworkflows: BTreeMap<String, SubworkflowDefinition>,
    #[serde(default)]
    pub compensation: CompensationPolicyDefinition,
    pub tasks: Vec<TaskDefinition>,
    #[serde(default)]
    pub policy: PolicyDefinition,
    #[serde(default)]
    pub memory: MemoryDefinition,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerDefinition>,
    #[serde(default)]
    pub a2a_peers: BTreeMap<String, A2aPeerDefinition>,
    #[serde(default)]
    pub packs: Vec<PackReference>,
    #[serde(default)]
    pub pack_trust: PackTrustDefinition,
    #[serde(default)]
    pub runtime: RuntimeDefinition,
    #[serde(default)]
    pub output: OutputDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Fake,
    Openai,
    Anthropic,
    Google,
    AzureOpenai,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDefinition {
    pub kind: ProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<SecretReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, SecretReference>,
}

pub const DEFAULT_SECRET_PROCESS_TIMEOUT_SECONDS: u64 = 5;
pub const DEFAULT_SECRET_OUTPUT_LIMIT_BYTES: u64 = 16 * 1024;
pub const MAX_SECRET_OUTPUT_LIMIT_BYTES: u64 = 64 * 1024;
pub const MAX_SECRET_PROCESS_TIMEOUT_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum SecretReference {
    Environment { env: String },
    File { file: String },
    Process { process: SecretProcessReference },
}

impl SecretReference {
    #[must_use]
    pub fn environment(name: impl Into<String>) -> Self {
        Self::Environment { env: name.into() }
    }

    #[must_use]
    pub fn source_description(&self) -> String {
        match self {
            Self::Environment { env } => format!("environment variable `{env}`"),
            Self::File { file } => format!("secret file `{file}`"),
            Self::Process { process } => {
                format!("secret process `{}`", process.command)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretProcessReference {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_secret_process_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_secret_output_limit_bytes")]
    pub output_limit_bytes: u64,
}

const fn default_secret_process_timeout_seconds() -> u64 {
    DEFAULT_SECRET_PROCESS_TIMEOUT_SECONDS
}

const fn default_secret_output_limit_bytes() -> u64 {
    DEFAULT_SECRET_OUTPUT_LIMIT_BYTES
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefinition {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_file: Option<String>,
    #[serde(default)]
    pub vars: JsonMap,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_max_turns")]
    pub max_turns: u16,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u16,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub retry: RetryDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<UsageLimitDefinition>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub provider_options: JsonMap,
}

const fn default_max_turns() -> u16 {
    8
}
const fn default_max_tool_calls() -> u16 {
    16
}
const fn default_max_output_tokens() -> u32 {
    2_048
}
const fn default_timeout_seconds() -> u64 {
    120
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningDefinition {
    pub effort: ReasoningEffort,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageLimitDefinition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionDefinition {
    pub kind: ActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "with")]
    pub defaults: JsonMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<Idempotency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, SecretReference>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_limit_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_limit_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combined_output_limit_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "ProcessIsolation::is_process")]
    pub isolation: ProcessIsolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerIsolationDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessIsolation {
    #[default]
    Process,
    Container,
}

impl ProcessIsolation {
    #[must_use]
    pub const fn is_process(&self) -> bool {
        matches!(self, Self::Process)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Container => "container",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContainerRuntime {
    #[default]
    Auto,
    Docker,
    Podman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerIsolationDefinition {
    pub image: String,
    #[serde(default)]
    pub runtime: ContainerRuntime,
    #[serde(default = "default_container_memory_limit_bytes")]
    pub memory_limit_bytes: u64,
    #[serde(default = "default_container_cpu_limit_millis")]
    pub cpu_limit_millis: u32,
    #[serde(default = "default_container_pids_limit")]
    pub pids_limit: u32,
}

pub const PROCESS_EXTENSION_PROTOCOL_VERSION: &str = "agentctl.dev/process-extension/v1";
pub const DEFAULT_PROCESS_STREAM_LIMIT_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PROCESS_COMBINED_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_PROCESS_OUTPUT_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PROCESS_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_TASK_CONCURRENCY: usize = 64;
pub const MIN_CONTAINER_MEMORY_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_CONTAINER_MEMORY_LIMIT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_CONTAINER_CPU_LIMIT_MILLIS: u32 = 64_000;
pub const MAX_CONTAINER_PIDS_LIMIT: u32 = 4096;

const fn default_container_memory_limit_bytes() -> u64 {
    256 * 1024 * 1024
}

const fn default_container_cpu_limit_millis() -> u32 {
    1000
}

const fn default_container_pids_limit() -> u32 {
    64
}

impl ActionDefinition {
    #[must_use]
    pub fn stdout_limit_bytes(&self) -> u64 {
        self.stdout_limit_bytes
            .unwrap_or(DEFAULT_PROCESS_STREAM_LIMIT_BYTES)
    }

    #[must_use]
    pub fn stderr_limit_bytes(&self) -> u64 {
        self.stderr_limit_bytes
            .unwrap_or(DEFAULT_PROCESS_STREAM_LIMIT_BYTES)
    }

    #[must_use]
    pub fn combined_output_limit_bytes(&self) -> u64 {
        self.combined_output_limit_bytes
            .unwrap_or(DEFAULT_PROCESS_COMBINED_LIMIT_BYTES)
    }

    pub fn validate_process_bounds(&self) -> Result<(), &'static str> {
        let has_output_limit = self.stdout_limit_bytes.is_some()
            || self.stderr_limit_bytes.is_some()
            || self.combined_output_limit_bytes.is_some();
        let process_action = matches!(
            self.kind,
            ActionKind::ShellExec | ActionKind::ProcessExtension
        );
        if !process_action && (!self.isolation.is_process() || self.container.is_some()) {
            return Err(
                "process isolation is only valid for builtin.shell.exec or extension.process actions",
            );
        }
        if !process_action && has_output_limit {
            return Err(
                "process output limits are only valid for builtin.shell.exec or extension.process actions",
            );
        }
        if self.idempotency.is_some()
            && !matches!(
                self.kind,
                ActionKind::McpCall | ActionKind::ProcessExtension
            )
        {
            return Err("idempotency is only valid for mcp.call or extension.process actions");
        }
        if !process_action {
            return Ok(());
        }
        match (self.isolation, &self.container) {
            (ProcessIsolation::Process, Some(_)) => {
                return Err("container configuration requires isolation: container");
            }
            (ProcessIsolation::Container, None) => {
                return Err("isolation: container requires container configuration");
            }
            (ProcessIsolation::Container, Some(container)) => {
                if !container_image_is_digest_pinned(&container.image) {
                    return Err(
                        "container image must be pinned as NAME@sha256:DIGEST or sha256:IMAGE_ID",
                    );
                }
                if !(MIN_CONTAINER_MEMORY_LIMIT_BYTES..=MAX_CONTAINER_MEMORY_LIMIT_BYTES)
                    .contains(&container.memory_limit_bytes)
                {
                    return Err(
                        "container memoryLimitBytes must be between 16777216 and 17179869184",
                    );
                }
                if container.cpu_limit_millis == 0
                    || container.cpu_limit_millis > MAX_CONTAINER_CPU_LIMIT_MILLIS
                {
                    return Err("container cpuLimitMillis must be between 1 and 64000");
                }
                if container.pids_limit == 0 || container.pids_limit > MAX_CONTAINER_PIDS_LIMIT {
                    return Err("container pidsLimit must be between 1 and 4096");
                }
                if self
                    .env
                    .keys()
                    .any(|name| container_engine_environment(name))
                {
                    return Err(
                        "container action environment must not override container-engine variables",
                    );
                }
            }
            (ProcessIsolation::Process, None) => {}
        }
        if self.timeout_seconds.is_some_and(|value| value == 0) {
            return Err("timeoutSeconds must be greater than zero");
        }
        if self
            .timeout_seconds
            .is_some_and(|value| value > MAX_PROCESS_TIMEOUT_SECONDS)
        {
            return Err("timeoutSeconds must not exceed 86400");
        }
        for (value, message) in [
            (
                self.stdout_limit_bytes,
                "stdoutLimitBytes must be between 1 and 16777216",
            ),
            (
                self.stderr_limit_bytes,
                "stderrLimitBytes must be between 1 and 16777216",
            ),
            (
                self.combined_output_limit_bytes,
                "combinedOutputLimitBytes must be between 1 and 16777216",
            ),
        ] {
            if value.is_some_and(|value| value == 0 || value > MAX_PROCESS_OUTPUT_LIMIT_BYTES) {
                return Err(message);
            }
        }
        if self.kind == ActionKind::ProcessExtension {
            if self.command.as_deref().is_none_or(str::is_empty) {
                return Err("extension.process requires command");
            }
            if self.protocol_version.as_deref() != Some(PROCESS_EXTENSION_PROTOCOL_VERSION) {
                return Err(
                    "extension.process protocolVersion must be agentctl.dev/process-extension/v1",
                );
            }
            if self.idempotency.is_none() {
                return Err("extension.process requires explicit idempotency");
            }
            let Some(input_schema) = &self.input_schema else {
                return Err("extension.process requires inputSchema");
            };
            if jsonschema::validator_for(input_schema).is_err() {
                return Err("extension.process inputSchema must be valid JSON Schema");
            }
            let Some(output_schema) = &self.output_schema else {
                return Err("extension.process requires outputSchema");
            };
            if jsonschema::validator_for(output_schema).is_err() {
                return Err("extension.process outputSchema must be valid JSON Schema");
            }
            if self.capabilities.iter().any(String::is_empty) {
                return Err("extension.process capabilities must not contain empty values");
            }
            let unique = self
                .capabilities
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            if unique.len() != self.capabilities.len() {
                return Err("extension.process capabilities must be unique");
            }
        } else if self.protocol_version.is_some()
            || self.input_schema.is_some()
            || self.output_schema.is_some()
            || !self.capabilities.is_empty()
        {
            return Err("extension protocol fields are only valid for extension.process actions");
        }
        Ok(())
    }
}

fn container_image_is_digest_pinned(image: &str) -> bool {
    let digest = image.strip_prefix("sha256:").or_else(|| {
        image
            .rsplit_once("@sha256:")
            .filter(|(name, _)| !name.is_empty() && !name.chars().any(char::is_whitespace))
            .map(|(_, digest)| digest)
    });
    digest.is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn container_engine_environment(name: &str) -> bool {
    matches!(
        name,
        "CONTAINER_CONNECTION"
            | "CONTAINER_HOST"
            | "DOCKER_CERT_PATH"
            | "DOCKER_CONFIG"
            | "DOCKER_CONTEXT"
            | "DOCKER_HOST"
            | "DOCKER_TLS_VERIFY"
            | "HOME"
            | "PATH"
            | "PODMAN_CONNECTIONS_CONF"
            | "TEMP"
            | "TMP"
            | "TMPDIR"
            | "XDG_CONFIG_HOME"
            | "XDG_RUNTIME_DIR"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ActionKind {
    #[serde(rename = "builtin.assign")]
    Assign,
    #[serde(rename = "builtin.assert")]
    Assert,
    #[serde(rename = "builtin.read")]
    Read,
    #[serde(rename = "builtin.write")]
    Write,
    #[serde(rename = "builtin.shell.exec")]
    ShellExec,
    #[serde(rename = "extension.process")]
    ProcessExtension,
    #[serde(rename = "builtin.memory.read")]
    MemoryRead,
    #[serde(rename = "builtin.memory.write")]
    MemoryWrite,
    #[serde(rename = "builtin.long_term_memory.read")]
    LongTermMemoryRead,
    #[serde(rename = "builtin.long_term_memory.search")]
    LongTermMemorySearch,
    #[serde(rename = "builtin.long_term_memory.write")]
    LongTermMemoryWrite,
    #[serde(rename = "builtin.long_term_memory.promote")]
    LongTermMemoryPromote,
    #[serde(rename = "mcp.call")]
    McpCall,
    #[serde(rename = "a2a.delegate")]
    A2aDelegate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDefinition {
    pub kind: ToolKind,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capability: String,
    pub risk: Risk,
    pub effect_class: EffectClass,
    pub idempotency: Idempotency,
    pub retry_safe: bool,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub secrets: Vec<SecretReference>,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub approval: ApprovalRequirement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ToolKind {
    #[serde(rename = "builtin.workspace.read")]
    WorkspaceRead,
    #[serde(rename = "builtin.workspace.write")]
    WorkspaceWrite,
    #[serde(rename = "builtin.echo")]
    Echo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Pure,
    InternalState,
    Observe,
    WorkspaceMutate,
    ExternalMutate,
    ProcessExecution,
    Network,
    Model,
    RemoteAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    Pure,
    Idempotent,
    Keyed,
    AtMostOnce,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    #[default]
    Policy,
    Never,
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskDefinition {
    pub id: String,
    pub uses: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreach: Option<ForeachDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<MatrixDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "loop")]
    pub loop_definition: Option<LoopDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_writes: Vec<String>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub vars: JsonMap,
    #[serde(default, rename = "with")]
    pub input: JsonMap,
    #[serde(default)]
    pub retry: RetryDefinition,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub failure: FailureBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensate: Option<CompensationDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompensationDefinition {
    pub uses: String,
    #[serde(default, rename = "with")]
    pub input: JsonMap,
    #[serde(default)]
    pub retry: RetryDefinition,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompensationTrigger {
    #[default]
    Manual,
    Automatic,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompensationPolicyDefinition {
    #[serde(default)]
    pub on_failure: CompensationTrigger,
    #[serde(default)]
    pub approval: ApprovalRequirement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubworkflowDefinition {
    pub version: String,
    #[serde(default)]
    pub inputs: JsonMap,
    pub input_schema: Value,
    #[serde(default)]
    pub outputs: JsonMap,
    pub output_schema: Value,
    pub tasks: Vec<TaskDefinition>,
}

pub const DEFAULT_MAX_EXPANSION_ITEMS: usize = 32;
pub const MAX_EXPANSION_ITEMS: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForeachDefinition {
    pub items: Vec<Value>,
    #[serde(default = "default_item_binding", rename = "as")]
    pub binding: String,
    #[serde(default = "default_max_expansion_items")]
    pub max_items: usize,
}

fn default_item_binding() -> String {
    "item".to_owned()
}

const fn default_max_expansion_items() -> usize {
    DEFAULT_MAX_EXPANSION_ITEMS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatrixDefinition {
    pub axes: BTreeMap<String, Vec<Value>>,
    #[serde(default = "default_max_expansion_items")]
    pub max_items: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteDefinition {
    pub select: String,
    pub cases: Vec<RouteCaseDefinition>,
    #[serde(default)]
    pub default: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteCaseDefinition {
    pub equals: Value,
    pub tasks: Vec<String>,
}

pub const MAX_LOOP_ITERATIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopDefinition {
    pub max_iterations: usize,
    #[serde(rename = "while")]
    pub condition: String,
    #[serde(default)]
    pub initial: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryDefinition {
    #[serde(default = "one")]
    pub max_attempts: u16,
    #[serde(default)]
    pub backoff_ms: u64,
}

impl Default for RetryDefinition {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_ms: 0,
        }
    }
}

const fn one() -> u16 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureBehavior {
    #[default]
    Stop,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyDefinition {
    #[serde(default = "default_workspace_root")]
    pub workspace_root: String,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    #[serde(default)]
    pub environment_allowlist: Vec<String>,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    #[serde(default)]
    pub network: NetworkPolicyDefinition,
    #[serde(default)]
    pub process_allowlist: Vec<String>,
    #[serde(default)]
    pub secret_file_roots: Vec<String>,
    #[serde(default)]
    pub secret_process_allowlist: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub tools_allow: Vec<String>,
    #[serde(default)]
    pub tools_deny: Vec<String>,
    #[serde(default)]
    pub approval: ApprovalMode,
    #[serde(default)]
    pub non_interactive: NonInteractiveMode,
}

impl Default for PolicyDefinition {
    fn default() -> Self {
        Self {
            workspace_root: default_workspace_root(),
            writable_roots: Vec::new(),
            environment_allowlist: Vec::new(),
            network_allowlist: Vec::new(),
            network: NetworkPolicyDefinition::default(),
            process_allowlist: Vec::new(),
            secret_file_roots: Vec::new(),
            secret_process_allowlist: Vec::new(),
            providers: Vec::new(),
            tools_allow: Vec::new(),
            tools_deny: Vec::new(),
            approval: ApprovalMode::default(),
            non_interactive: NonInteractiveMode::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkPolicyDefinition {
    #[serde(default = "default_network_schemes")]
    pub allowed_schemes: Vec<String>,
    #[serde(default)]
    pub allowed_ports: Vec<u16>,
    #[serde(default)]
    pub allow_private: bool,
    #[serde(default)]
    pub allow_proxy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_ca: Option<SecretReference>,
    #[serde(default = "default_network_connect_timeout_seconds")]
    pub connect_timeout_seconds: u64,
    #[serde(default = "default_network_response_limit_bytes")]
    pub max_response_bytes: u64,
}

impl Default for NetworkPolicyDefinition {
    fn default() -> Self {
        Self {
            allowed_schemes: default_network_schemes(),
            allowed_ports: Vec::new(),
            allow_private: false,
            allow_proxy: false,
            custom_ca: None,
            connect_timeout_seconds: default_network_connect_timeout_seconds(),
            max_response_bytes: default_network_response_limit_bytes(),
        }
    }
}

fn default_network_schemes() -> Vec<String> {
    vec!["https".to_owned(), "http".to_owned()]
}

const fn default_network_connect_timeout_seconds() -> u64 {
    crate::network::DEFAULT_NETWORK_CONNECT_TIMEOUT_SECONDS
}

const fn default_network_response_limit_bytes() -> u64 {
    crate::network::DEFAULT_NETWORK_RESPONSE_LIMIT_BYTES
}

fn default_workspace_root() -> String {
    ".".to_owned()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Never,
    #[default]
    Mutations,
    HighRisk,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NonInteractiveMode {
    #[default]
    Pause,
    DenyApproval,
    Fail,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryDefinition {
    #[serde(default)]
    pub working: JsonMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_term: Option<LongTermMemoryDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LongTermMemoryDefinition {
    #[serde(default = "default_sqlite")]
    pub provider: String,
    #[serde(default = "default_memory_namespace")]
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
    #[serde(default)]
    pub embedding: MemoryEmbeddingDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryEmbeddingDefinition {
    #[serde(default = "default_local_hash_embedding")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_embedding_dimensions")]
    pub dimensions: u16,
}

impl Default for MemoryEmbeddingDefinition {
    fn default() -> Self {
        Self {
            provider: default_local_hash_embedding(),
            model: None,
            dimensions: default_embedding_dimensions(),
        }
    }
}

fn default_sqlite() -> String {
    "sqlite".to_owned()
}
fn default_memory_namespace() -> String {
    "default".to_owned()
}
fn default_local_hash_embedding() -> String {
    "local_hash".to_owned()
}
const fn default_embedding_dimensions() -> u16 {
    64
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerDefinition {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, SecretReference>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_mcp_version")]
    pub protocol_version: String,
}

fn default_mcp_version() -> String {
    "2025-11-25".to_owned()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct A2aPeerDefinition {
    pub card_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, SecretReference>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_a2a_max_polls")]
    pub max_polls: usize,
    #[serde(default = "default_a2a_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_a2a_version")]
    pub protocol_version: String,
}

const fn default_a2a_max_polls() -> usize {
    100
}

const fn default_a2a_poll_interval_ms() -> u64 {
    100
}

fn default_a2a_version() -> String {
    "1.0".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackReference {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PackSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<PackSignature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackTrustDefinition {
    #[serde(default)]
    pub unsigned: UnsignedPackPolicy,
    #[serde(default)]
    pub identities: Vec<PackIdentity>,
    #[serde(default)]
    pub allow_unsigned_process: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackIdentity {
    pub identity: String,
    pub issuer: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnsignedPackPolicy {
    Deny,
    #[default]
    Warn,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDefinition {
    #[serde(default = "one_usize")]
    pub max_concurrency: usize,
    #[serde(default = "default_timeout_seconds")]
    pub default_timeout_seconds: u64,
}

impl Default for RuntimeDefinition {
    fn default() -> Self {
        Self {
            max_concurrency: 1,
            default_timeout_seconds: default_timeout_seconds(),
        }
    }
}

const fn one_usize() -> usize {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputDefinition {
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub show_diff: bool,
}

impl Default for OutputDefinition {
    fn default() -> Self {
        Self {
            verbose: false,
            show_diff: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseOutcome {
    pub workflow: Workflow,
    pub diagnostics: Vec<Diagnostic>,
    pub migrated_legacy: bool,
}

/// Parse a strict v1 workflow or translate the prototype's unversioned envelope.
pub fn parse_workflow(source: &str, file: &str) -> Result<ParseOutcome, Vec<Diagnostic>> {
    if source.len() > 1_048_576 {
        return Err(vec![Diagnostic::error(
            DiagnosticCode::SchemaViolation,
            file,
            "workflow exceeds the 1 MiB parser limit",
        )]);
    }
    let raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(source).map_err(|error| {
        let mut diagnostic = Diagnostic::error(DiagnosticCode::YamlSyntax, file, error.to_string());
        if let Some(location) = error.location() {
            diagnostic = diagnostic.with_location(location.line(), location.column());
        }
        vec![diagnostic]
    })?;

    let is_legacy = raw.get("apiVersion").is_none();
    let normalized = if is_legacy {
        translate_legacy(raw, file)?
    } else {
        raw
    };

    let normalized_source;
    let parse_source = if is_legacy {
        normalized_source = serde_yaml_ng::to_string(&normalized).map_err(|error| {
            vec![Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                error.to_string(),
            )]
        })?;
        normalized_source.as_str()
    } else {
        source
    };
    let deserializer = serde_yaml_ng::Deserializer::from_str(parse_source);
    let workflow: Workflow = serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let path = error.path().to_string();
        let inner = error.into_inner();
        let mut diagnostic =
            Diagnostic::error(DiagnosticCode::SchemaViolation, file, inner.to_string())
                .with_path(path);
        if let Some(location) = inner.location() {
            diagnostic = diagnostic.with_location(location.line(), location.column());
        }
        vec![diagnostic]
    })?;

    let mut diagnostics = validate_document(&workflow, file);
    if diagnostics
        .iter()
        .any(|item| item.severity == Severity::Error)
    {
        return Err(diagnostics);
    }
    if is_legacy {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::MigrationRequired,
            severity: Severity::Warning,
            message: "translated an unversioned TypeScript-era workflow".to_owned(),
            file: file.to_owned(),
            line: Some(1),
            column: Some(1),
            path: None,
            help: Some("run `agentctl migrate <file>` and commit the versioned form".to_owned()),
        });
    }
    Ok(ParseOutcome {
        workflow,
        diagnostics,
        migrated_legacy: is_legacy,
    })
}

fn translate_legacy(
    raw: serde_yaml_ng::Value,
    file: &str,
) -> Result<serde_yaml_ng::Value, Vec<Diagnostic>> {
    let mut json: Value = serde_json::to_value(raw).map_err(|error| {
        vec![Diagnostic::error(
            DiagnosticCode::MigrationRequired,
            file,
            error.to_string(),
        )]
    })?;
    let Some(root) = json.as_object_mut() else {
        return Err(vec![Diagnostic::error(
            DiagnosticCode::MigrationRequired,
            file,
            "workflow root must be a mapping",
        )]);
    };
    let name = root
        .remove("playbook")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| {
            vec![Diagnostic::error(
                DiagnosticCode::MigrationRequired,
                file,
                "legacy workflow requires `playbook`",
            )]
        })?;
    root.remove("version");
    let description = root.remove("description");
    if let Some(modules) = root.remove("modules") {
        root.insert("actions".to_owned(), modules);
    }
    if let Some(tasks) = root.get_mut("tasks").and_then(Value::as_array_mut) {
        for task in tasks {
            if let Some(uses) = task.get_mut("uses")
                && let Some(text) = uses.as_str()
                && let Some(reference) = text.strip_prefix("module:")
            {
                *uses = Value::String(format!("action:{reference}"));
            }
        }
    }
    if let Some(agents) = root.get_mut("agents").and_then(Value::as_object_mut) {
        for agent in agents.values_mut() {
            let Some(object) = agent.as_object_mut() else {
                continue;
            };
            let kind = object
                .remove("kind")
                .and_then(|value| value.as_str().map(ToOwned::to_owned));
            let provider = object
                .remove("provider")
                .and_then(|value| value.as_str().map(ToOwned::to_owned));
            let resolved = match (kind.as_deref(), provider) {
                (Some("builtin.heuristic"), _) => "fake".to_owned(),
                (_, Some(provider)) => provider,
                _ => "openai".to_owned(),
            };
            object.insert("provider".to_owned(), Value::String(resolved));
            object
                .entry("model")
                .or_insert_with(|| Value::String("scripted".to_owned()));
            object.remove("profile");
            object.remove("promptCache");
            object.remove("baseUrl");
            object.remove("organization");
            object.remove("project");
            object.remove("endpoint");
            object.remove("apiVersion");
            object.remove("deployment");
            object.remove("temperature");
            object.remove("reasoningEffort");
        }
    }
    if !root.contains_key("providers") {
        let providers = serde_json::json!({
            "fake": { "kind": "fake" },
            "openai": { "kind": "openai", "credential": { "env": "OPENAI_API_KEY" } }
        });
        root.insert("providers".to_owned(), providers);
    }
    root.remove("defaults");
    root.remove("promptCache");
    if let Some(memory) = root.get_mut("memory").and_then(Value::as_object_mut) {
        if let Some(working) = memory.remove("working") {
            let initial = working
                .get("initial")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            root.insert("memory".to_owned(), serde_json::json!({"working": initial}));
        } else {
            root.remove("memory");
        }
    }
    root.remove("mcpServers");
    root.remove("a2aAgents");
    root.remove("packs");
    if let Some(policy) = root.get_mut("policy").and_then(Value::as_object_mut) {
        if let Some(mode) = policy.remove("approvalMode") {
            let mapped = match mode.as_str() {
                Some("never") => "never",
                Some("always") => "always",
                _ => "mutations",
            };
            policy.insert("approval".to_owned(), Value::String(mapped.to_owned()));
        }
    }
    let spec = Value::Object(std::mem::take(root));
    let mut metadata = serde_json::Map::new();
    metadata.insert("name".to_owned(), Value::String(name));
    if let Some(description) = description {
        metadata.insert("description".to_owned(), description);
    }
    json = serde_json::json!({
        "apiVersion": API_VERSION,
        "kind": "Workflow",
        "metadata": metadata,
        "spec": spec
    });
    serde_json::from_value(json).map_err(|error| {
        vec![Diagnostic::error(
            DiagnosticCode::MigrationRequired,
            file,
            error.to_string(),
        )]
    })
}

fn validate_document(workflow: &Workflow, file: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if workflow.api_version != API_VERSION {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::UnsupportedVersion,
                file,
                format!("unsupported apiVersion `{}`", workflow.api_version),
            )
            .with_path("apiVersion")
            .with_help(format!("use `{API_VERSION}`")),
        );
    }
    if workflow.metadata.name.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "metadata.name must not be empty",
            )
            .with_path("metadata.name"),
        );
    }
    if workflow.spec.tasks.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "spec.tasks must contain at least one task",
            )
            .with_path("spec.tasks"),
        );
    }
    if workflow.spec.runtime.max_concurrency == 0
        || workflow.spec.runtime.max_concurrency > MAX_TASK_CONCURRENCY
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                format!("runtime.maxConcurrency must be between 1 and {MAX_TASK_CONCURRENCY}"),
            )
            .with_path("spec.runtime.maxConcurrency"),
        );
    }
    if workflow.spec.policy.network.allowed_schemes.is_empty()
        || workflow
            .spec
            .policy
            .network
            .allowed_schemes
            .iter()
            .any(|scheme| !matches!(scheme.as_str(), "http" | "https"))
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "policy.network.allowedSchemes must contain only http or https",
            )
            .with_path("spec.policy.network.allowedSchemes"),
        );
    }
    if workflow.spec.policy.network.allowed_ports.contains(&0) {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "policy.network.allowedPorts must not contain port 0",
            )
            .with_path("spec.policy.network.allowedPorts"),
        );
    }
    if workflow.spec.policy.network.connect_timeout_seconds == 0
        || workflow.spec.policy.network.connect_timeout_seconds
            > crate::network::MAX_NETWORK_CONNECT_TIMEOUT_SECONDS
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                format!(
                    "policy.network.connectTimeoutSeconds must be between 1 and {}",
                    crate::network::MAX_NETWORK_CONNECT_TIMEOUT_SECONDS
                ),
            )
            .with_path("spec.policy.network.connectTimeoutSeconds"),
        );
    }
    if workflow.spec.policy.network.max_response_bytes == 0
        || workflow.spec.policy.network.max_response_bytes
            > crate::network::MAX_NETWORK_RESPONSE_LIMIT_BYTES
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                format!(
                    "policy.network.maxResponseBytes must be between 1 and {}",
                    crate::network::MAX_NETWORK_RESPONSE_LIMIT_BYTES
                ),
            )
            .with_path("spec.policy.network.maxResponseBytes"),
        );
    }
    if let Some(custom_ca) = &workflow.spec.policy.network.custom_ca {
        validate_secret_reference(
            custom_ca,
            "network custom CA",
            "spec.policy.network.customCa",
            &workflow.spec.policy,
            file,
            &mut diagnostics,
        );
        if let SecretReference::Environment { env } = custom_ca
            && !workflow.spec.policy.environment_allowlist.contains(env)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidSecretReference,
                    file,
                    "network custom CA environment variable is not in policy.environmentAllowlist",
                )
                .with_path("spec.policy.network.customCa"),
            );
        }
    }
    if workflow.spec.runtime.default_timeout_seconds == 0
        || workflow.spec.runtime.default_timeout_seconds > MAX_PROCESS_TIMEOUT_SECONDS
    {
        diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::SchemaViolation,
                file,
                "runtime.defaultTimeoutSeconds must be between 1 and 86400",
            )
            .with_path("spec.runtime.defaultTimeoutSeconds"),
        );
    }
    for (position, task) in workflow.spec.tasks.iter().enumerate() {
        if task.foreach.is_some() && task.matrix.is_some() {
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
        }
        if let Some(foreach) = &task.foreach
            && (foreach.max_items == 0 || foreach.max_items > MAX_EXPANSION_ITEMS)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("foreach.maxItems must be between 1 and {MAX_EXPANSION_ITEMS}"),
                )
                .with_path(format!("spec.tasks[{position}].foreach.maxItems")),
            );
        }
        if let Some(matrix) = &task.matrix
            && (matrix.max_items == 0 || matrix.max_items > MAX_EXPANSION_ITEMS)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("matrix.maxItems must be between 1 and {MAX_EXPANSION_ITEMS}"),
                )
                .with_path(format!("spec.tasks[{position}].matrix.maxItems")),
            );
        }
        if let Some(loop_definition) = &task.loop_definition
            && (loop_definition.max_iterations == 0
                || loop_definition.max_iterations > MAX_LOOP_ITERATIONS)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("loop.maxIterations must be between 1 and {MAX_LOOP_ITERATIONS}"),
                )
                .with_path(format!("spec.tasks[{position}].loop.maxIterations")),
            );
        }
    }
    for (name, action) in &workflow.spec.actions {
        if let Err(message) = action.validate_process_bounds() {
            diagnostics.push(
                Diagnostic::error(DiagnosticCode::SchemaViolation, file, message)
                    .with_path(format!("spec.actions.{name}")),
            );
        }
    }
    for (name, provider) in &workflow.spec.providers {
        if let Some(secret) = &provider.credential {
            validate_secret_reference(
                secret,
                &format!("provider `{name}` credential"),
                &format!("spec.providers.{name}.credential"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
        for (header, secret) in &provider.headers {
            validate_secret_reference(
                secret,
                &format!("provider `{name}` header `{header}`"),
                &format!("spec.providers.{name}.headers.{header}"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
    }
    for (name, action) in &workflow.spec.actions {
        for (environment, secret) in &action.env {
            validate_secret_reference(
                secret,
                &format!("action `{name}` environment `{environment}`"),
                &format!("spec.actions.{name}.env.{environment}"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
    }
    for (name, tool) in &workflow.spec.tools {
        for (position, secret) in tool.secrets.iter().enumerate() {
            validate_secret_reference(
                secret,
                &format!("tool `{name}` secret {position}"),
                &format!("spec.tools.{name}.secrets[{position}]"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
    }
    for (name, agent) in &workflow.spec.agents {
        if agent.instructions.is_some() == agent.instructions_file.is_some() {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!(
                        "agent `{name}` must define exactly one of instructions or instructionsFile"
                    ),
                )
                .with_path(format!("spec.agents.{name}")),
            );
        }
        if agent.max_turns == 0 || agent.max_turns > 64 {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("agent `{name}` maxTurns must be between 1 and 64"),
                )
                .with_path(format!("spec.agents.{name}.maxTurns")),
            );
        }
        if agent.max_tool_calls > 256 || agent.max_output_tokens == 0 || agent.timeout_seconds == 0
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("agent `{name}` has invalid execution bounds"),
                )
                .with_path(format!("spec.agents.{name}")),
            );
        }
    }
    for (position, task) in workflow.spec.tasks.iter().enumerate() {
        if task
            .timeout_seconds
            .is_some_and(|value| value == 0 || value > MAX_PROCESS_TIMEOUT_SECONDS)
        {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "task timeoutSeconds must be between 1 and 86400",
                )
                .with_path(format!("spec.tasks[{position}].timeoutSeconds")),
            );
        }
        if task.retry.max_attempts == 0 || task.retry.max_attempts > 20 {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "retry.maxAttempts must be between 1 and 20",
                )
                .with_path(format!("spec.tasks[{position}].retry.maxAttempts")),
            );
        }
        if task.retry.backoff_ms > 60_000 {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    "retry.backoffMs must not exceed 60000",
                )
                .with_path(format!("spec.tasks[{position}].retry.backoffMs")),
            );
        }
        if let Some(compensate) = &task.compensate {
            if compensate
                .timeout_seconds
                .is_some_and(|value| value == 0 || value > MAX_PROCESS_TIMEOUT_SECONDS)
            {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        "compensate.timeoutSeconds must be between 1 and 86400",
                    )
                    .with_path(format!("spec.tasks[{position}].compensate.timeoutSeconds")),
                );
            }
            if compensate.retry.max_attempts == 0 || compensate.retry.max_attempts > 20 {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        "compensate.retry.maxAttempts must be between 1 and 20",
                    )
                    .with_path(format!(
                        "spec.tasks[{position}].compensate.retry.maxAttempts"
                    )),
                );
            }
            if compensate.retry.backoff_ms > 60_000 {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::SchemaViolation,
                        file,
                        "compensate.retry.backoffMs must not exceed 60000",
                    )
                    .with_path(format!("spec.tasks[{position}].compensate.retry.backoffMs")),
                );
            }
        }
    }
    for (name, server) in &workflow.spec.mcp_servers {
        if server.timeout_seconds == 0 || server.timeout_seconds > MAX_PROCESS_TIMEOUT_SECONDS {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!(
                        "MCP server `{name}` timeoutSeconds must be between 1 and {MAX_PROCESS_TIMEOUT_SECONDS}"
                    ),
                )
                .with_path(format!("spec.mcpServers.{name}.timeoutSeconds")),
            );
        }
        for (header, secret) in &server.headers {
            validate_secret_reference(
                secret,
                &format!("MCP server `{name}` header `{header}`"),
                &format!("spec.mcpServers.{name}.headers.{header}"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
    }
    for (name, peer) in &workflow.spec.a2a_peers {
        if peer.timeout_seconds == 0 || peer.timeout_seconds > MAX_PROCESS_TIMEOUT_SECONDS {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!(
                        "A2A peer `{name}` timeoutSeconds must be between 1 and {MAX_PROCESS_TIMEOUT_SECONDS}"
                    ),
                )
                .with_path(format!("spec.a2aPeers.{name}.timeoutSeconds")),
            );
        }
        if peer.max_polls == 0 || peer.max_polls > 1_000 {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("A2A peer `{name}` maxPolls must be between 1 and 1000"),
                )
                .with_path(format!("spec.a2aPeers.{name}.maxPolls")),
            );
        }
        if peer.poll_interval_ms == 0 || peer.poll_interval_ms > 60_000 {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::SchemaViolation,
                    file,
                    format!("A2A peer `{name}` pollIntervalMs must be between 1 and 60000"),
                )
                .with_path(format!("spec.a2aPeers.{name}.pollIntervalMs")),
            );
        }
        for (header, secret) in &peer.headers {
            validate_secret_reference(
                secret,
                &format!("A2A peer `{name}` header `{header}`"),
                &format!("spec.a2aPeers.{name}.headers.{header}"),
                &workflow.spec.policy,
                file,
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

fn validate_secret_reference(
    reference: &SecretReference,
    label: &str,
    path: &str,
    policy: &PolicyDefinition,
    file: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let error = match reference {
        SecretReference::Environment { env } if !valid_env_name(env) => {
            Some(format!("{label} uses an invalid environment variable name"))
        }
        SecretReference::File { file } if file.trim().is_empty() || file.contains('\0') => {
            Some(format!("{label} uses an invalid secret file path"))
        }
        SecretReference::File { .. } if policy.secret_file_roots.is_empty() => Some(format!(
            "{label} requires at least one policy.secretFileRoots entry"
        )),
        SecretReference::Process { process } if process.command.trim().is_empty() => {
            Some(format!("{label} uses an empty secret process command"))
        }
        SecretReference::Process { process }
            if process.args.len() > 64
                || process.args.iter().any(|argument| argument.len() > 4096) =>
        {
            Some(format!(
                "{label} secret process accepts at most 64 arguments of 4096 bytes each"
            ))
        }
        SecretReference::Process { process }
            if process.timeout_seconds == 0
                || process.timeout_seconds > MAX_SECRET_PROCESS_TIMEOUT_SECONDS =>
        {
            Some(format!(
                "{label} secret process timeoutSeconds must be between 1 and {MAX_SECRET_PROCESS_TIMEOUT_SECONDS}"
            ))
        }
        SecretReference::Process { process }
            if process.output_limit_bytes == 0
                || process.output_limit_bytes > MAX_SECRET_OUTPUT_LIMIT_BYTES =>
        {
            Some(format!(
                "{label} secret process outputLimitBytes must be between 1 and {MAX_SECRET_OUTPUT_LIMIT_BYTES}"
            ))
        }
        SecretReference::Process { process }
            if !policy.secret_process_allowlist.iter().any(|allowed| {
                Path::new(&process.command)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|basename| basename == allowed)
            }) =>
        {
            Some(format!(
                "{label} process is not in policy.secretProcessAllowlist"
            ))
        }
        _ => None,
    };
    if let Some(error) = error {
        diagnostics.push(
            Diagnostic::error(DiagnosticCode::InvalidSecretReference, file, error)
                .with_path(path.to_owned()),
        );
    }
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[must_use]
pub fn schema_json() -> Value {
    serde_json::to_value(schema_for!(Workflow)).unwrap_or_else(|_| Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata:
  name: hello
spec:
  actions:
    greet:
      kind: builtin.assign
  tasks:
    - id: greet
      uses: action:greet
      with:
        message: hello
"#;

    #[test]
    fn parses_versioned_workflow() {
        let outcome = parse_workflow(MINIMAL, "hello.yaml").expect("valid fixture");
        assert_eq!(outcome.workflow.metadata.name, "hello");
        assert!(!outcome.migrated_legacy);
    }

    #[test]
    fn rejects_unknown_fields_with_location_and_path() {
        let source = MINIMAL.replace("  name: hello", "  name: hello\n  surprise: true");
        let diagnostics = parse_workflow(&source, "bad.yaml").expect_err("unknown field");
        assert_eq!(diagnostics[0].code, DiagnosticCode::SchemaViolation);
        assert!(diagnostics[0].line.is_some());
        assert!(
            diagnostics[0]
                .path
                .as_deref()
                .is_some_and(|path| path.contains("metadata"))
        );
    }

    #[test]
    fn rejects_removed_tool_level_compensation_metadata() {
        let source = MINIMAL.replace(
            "  actions:",
            r#"  tools:
    legacy:
      kind: builtin.echo
      description: echo
      inputSchema: { type: object }
      outputSchema: { type: object }
      capability: observe
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      compensation: undo
  actions:"#,
        );
        let diagnostics =
            parse_workflow(&source, "legacy-compensation.yaml").expect_err("removed field");
        assert_eq!(diagnostics[0].code, DiagnosticCode::SchemaViolation);
        assert!(diagnostics[0].message.contains("compensation"));
    }

    #[test]
    fn validates_secret_reference_name() {
        let source = MINIMAL.replace(
            "  actions:",
            "  providers:\n    openai:\n      kind: openai\n      credential:\n        env: bad-key\n  actions:",
        );
        let diagnostics = parse_workflow(&source, "bad.yaml").expect_err("bad env name");
        assert_eq!(diagnostics[0].code, DiagnosticCode::InvalidSecretReference);
    }

    #[test]
    fn validates_network_security_bounds_and_custom_ca_policy() {
        let invalid = MINIMAL.replace(
            "  actions:",
            "  policy:\n    network:\n      allowedSchemes: [ftp]\n      allowedPorts: [0]\n      connectTimeoutSeconds: 0\n      maxResponseBytes: 0\n      customCa: { env: CORP_CA_PEM }\n  actions:",
        );
        let diagnostics =
            parse_workflow(&invalid, "network-invalid.yaml").expect_err("invalid network policy");
        let paths = diagnostics
            .iter()
            .filter_map(|diagnostic| diagnostic.path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "spec.policy.network.allowedSchemes",
            "spec.policy.network.allowedPorts",
            "spec.policy.network.connectTimeoutSeconds",
            "spec.policy.network.maxResponseBytes",
            "spec.policy.network.customCa",
        ] {
            assert!(
                paths.contains(&expected),
                "missing diagnostic for {expected}"
            );
        }

        let valid = MINIMAL.replace(
            "  actions:",
            "  policy:\n    environmentAllowlist: [CORP_CA_PEM]\n    network:\n      allowedSchemes: [https]\n      allowedPorts: [443]\n      connectTimeoutSeconds: 5\n      maxResponseBytes: 4096\n      customCa: { env: CORP_CA_PEM }\n  actions:",
        );
        parse_workflow(&valid, "network-valid.yaml").expect("valid network policy");
    }

    #[test]
    fn parses_bounded_file_and_process_secret_references() {
        let source = MINIMAL.replace(
            "  actions:",
            "  policy:\n    secretFileRoots: [secrets]\n    secretProcessAllowlist: [secret-helper]\n  providers:\n    file:\n      kind: openai\n      credential: { file: secrets/openai }\n    process:\n      kind: anthropic\n      credential:\n        process:\n          command: /usr/local/bin/secret-helper\n          args: [read, anthropic]\n          timeoutSeconds: 3\n          outputLimitBytes: 128\n  actions:",
        );
        let workflow = parse_workflow(&source, "secrets.yaml")
            .expect("valid secret references")
            .workflow;
        assert!(matches!(
            workflow.spec.providers["file"].credential.as_ref(),
            Some(SecretReference::File { .. })
        ));
        assert!(matches!(
            workflow.spec.providers["process"].credential.as_ref(),
            Some(SecretReference::Process { .. })
        ));
    }

    #[test]
    fn protocol_recovery_bounds_and_idempotency_are_strict() {
        let mcp = MINIMAL.replace(
            "    greet:\n      kind: builtin.assign",
            "    greet:\n      kind: mcp.call\n      idempotency: keyed",
        );
        let workflow = parse_workflow(&mcp, "mcp.yaml")
            .expect("MCP idempotency")
            .workflow;
        assert_eq!(
            workflow.spec.actions["greet"].idempotency,
            Some(Idempotency::Keyed)
        );

        let invalid_action = MINIMAL.replace(
            "      kind: builtin.assign",
            "      kind: builtin.assign\n      idempotency: idempotent",
        );
        let diagnostics =
            parse_workflow(&invalid_action, "action.yaml").expect_err("invalid action field");
        assert!(diagnostics[0].message.contains("only valid for mcp.call"));

        let invalid_a2a = MINIMAL.replace(
            "  actions:",
            "  a2aPeers:\n    remote:\n      cardUrl: https://agent.example/card.json\n      maxPolls: 0\n      pollIntervalMs: 0\n  actions:",
        );
        let diagnostics =
            parse_workflow(&invalid_a2a, "a2a.yaml").expect_err("invalid polling bounds");
        assert!(
            diagnostics
                .iter()
                .any(|item| item.message.contains("maxPolls"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|item| item.message.contains("pollIntervalMs"))
        );
    }

    #[test]
    fn secret_file_and_process_references_require_explicit_policy() {
        let file_source = MINIMAL.replace(
            "  actions:",
            "  providers:\n    openai:\n      kind: openai\n      credential: { file: /run/secrets/openai }\n  actions:",
        );
        let diagnostics =
            parse_workflow(&file_source, "bad.yaml").expect_err("missing secret root");
        assert_eq!(diagnostics[0].code, DiagnosticCode::InvalidSecretReference);
        assert!(diagnostics[0].message.contains("secretFileRoots"));

        let process_source = MINIMAL.replace(
            "  actions:",
            "  providers:\n    openai:\n      kind: openai\n      credential:\n        process:\n          command: secret-helper\n          timeoutSeconds: 0\n  actions:",
        );
        let diagnostics =
            parse_workflow(&process_source, "bad.yaml").expect_err("invalid secret process");
        assert_eq!(diagnostics[0].code, DiagnosticCode::InvalidSecretReference);
        assert!(diagnostics[0].message.contains("timeoutSeconds"));
    }

    #[test]
    fn rejects_process_limits_on_non_process_actions() {
        let source = MINIMAL.replace(
            "      kind: builtin.assign",
            "      kind: builtin.assign\n      stdoutLimitBytes: 64",
        );
        let diagnostics = parse_workflow(&source, "bad.yaml").expect_err("invalid bound");
        assert!(
            diagnostics[0]
                .message
                .contains("only valid for builtin.shell.exec")
        );
    }

    #[test]
    fn rejects_zero_and_unreasonably_large_process_bounds() {
        let source = MINIMAL.replace(
            "      kind: builtin.assign",
            "      kind: builtin.shell.exec\n      command: sh\n      stdoutLimitBytes: 0",
        );
        let diagnostics = parse_workflow(&source, "bad.yaml").expect_err("invalid bound");
        assert!(diagnostics[0].message.contains("between 1 and 16777216"));
    }

    #[test]
    fn process_isolation_defaults_to_host_and_validates_container_contract() {
        let host = MINIMAL.replace(
            "      kind: builtin.assign",
            "      kind: builtin.shell.exec\n      command: sh",
        );
        let workflow = parse_workflow(&host, "host.yaml")
            .expect("host process")
            .workflow;
        assert_eq!(
            workflow.spec.actions["greet"].isolation,
            ProcessIsolation::Process
        );

        let digest = "0".repeat(64);
        let container = MINIMAL.replace(
            "      kind: builtin.assign",
            &format!(
                "      kind: builtin.shell.exec\n      command: /bin/fixture\n      isolation: container\n      container:\n        image: example.invalid/fixture@sha256:{digest}\n        runtime: podman\n        memoryLimitBytes: 33554432\n        cpuLimitMillis: 500\n        pidsLimit: 16"
            ),
        );
        let workflow = parse_workflow(&container, "container.yaml")
            .expect("container process")
            .workflow;
        assert_eq!(
            workflow.spec.actions["greet"].isolation,
            ProcessIsolation::Container
        );
        assert_eq!(
            workflow.spec.actions["greet"]
                .container
                .as_ref()
                .expect("container")
                .runtime,
            ContainerRuntime::Podman
        );
    }

    #[test]
    fn rejects_weak_or_misapplied_container_isolation() {
        for replacement in [
            "      kind: builtin.shell.exec\n      command: sh\n      isolation: container",
            "      kind: builtin.shell.exec\n      command: sh\n      isolation: container\n      container:\n        image: example.invalid/fixture:latest",
            "      kind: builtin.assign\n      isolation: container\n      container:\n        image: example.invalid/fixture@sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "      kind: builtin.shell.exec\n      command: sh\n      container:\n        image: example.invalid/fixture@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let source = MINIMAL.replace("      kind: builtin.assign", replacement);
            parse_workflow(&source, "bad-isolation.yaml").expect_err("invalid isolation must fail");
        }
    }

    #[test]
    fn rejects_loop_iteration_bounds_outside_framework_limits() {
        for max_iterations in [0, MAX_LOOP_ITERATIONS + 1] {
            let source = format!(
                r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: invalid-loop-bound }}
spec:
  actions:
    assign: {{ kind: builtin.assign }}
  tasks:
    - id: bounded
      uses: action:assign
      loop:
        maxIterations: {max_iterations}
        while: "${{{{ vars.loopIndex < 1 }}}}"
"#
            );
            let diagnostics = parse_workflow(&source, "bad.yaml").expect_err("invalid loop bound");
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("loop.maxIterations must be between 1 and 64")
            }));
        }
    }
}
