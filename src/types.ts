export type JsonPrimitive = string | number | boolean | null;
export type JsonArray = JsonValue[];
export interface JsonObject {
	[key: string]: JsonValue;
}
export type JsonValue = JsonPrimitive | JsonArray | JsonObject;

export type ModuleKind =
	| "builtin.assign"
	| "builtin.assert"
	| "builtin.shell.exec"
	| "builtin.read"
	| "builtin.write"
	| "builtin.edit"
	| "builtin.grep"
	| "builtin.find"
	| "builtin.ls"
	| "builtin.memory.read"
	| "builtin.memory.write"
	| "builtin.long_term_memory.write"
	| "builtin.long_term_memory.search"
	| "builtin.long_term_memory.retrieve"
	| "pack.process";
export type AgentKind = "builtin.heuristic" | "openai.responses";
export type AgentProfileName = "none" | "inspect" | "workspace_write" | "workspace_exec";
export type ToolCapability = "internal" | "observe" | "mutate" | "act";
export type ToolRisk = "low" | "medium" | "high";
export type ApprovalMode = "never" | "on-mutate" | "on-act" | "always";
export type ApprovalStatus = "pending" | "approved" | "rejected";
export type ToolProviderKind = "builtin" | "module" | "mcp" | "a2a";
export type ReasoningEffort = "minimal" | "low" | "medium" | "high";
export type OutputFormat = "yaml" | "json";
export type OutputColorMode = "auto" | "always" | "never";
export type PromptCacheRetention = "in_memory" | "24h";
export type PromptCacheKeyScope = "agent" | "run" | "playbook" | "provider" | "custom";
export type PromptCacheShareMode = "isolated" | "group";

export interface RetryPolicy {
	maxAttempts: number;
	backoffMs: number;
}

export interface ProcessRequirementDefinition {
	readonly command: string;
	readonly version?: string;
	readonly versionArgs?: ReadonlyArray<string>;
}

export interface ProcessRuntimeDefinition {
	readonly requires?: ReadonlyArray<ProcessRequirementDefinition>;
}

export interface ModuleToolPolicyDefinition {
	readonly label?: string;
	readonly capability?: ToolCapability;
	readonly risk?: ToolRisk;
}

interface BaseModuleDefinition {
	readonly description?: string;
	readonly with?: JsonObject;
	readonly deterministic?: boolean;
}

export interface BuiltinModuleDefinition extends BaseModuleDefinition {
	readonly kind:
		| "builtin.assign"
		| "builtin.assert"
		| "builtin.shell.exec"
		| "builtin.read"
		| "builtin.write"
		| "builtin.edit"
		| "builtin.grep"
		| "builtin.find"
		| "builtin.ls"
		| "builtin.memory.read"
		| "builtin.memory.write"
		| "builtin.long_term_memory.write"
		| "builtin.long_term_memory.search"
		| "builtin.long_term_memory.retrieve";
}

export interface ProcessModuleDefinition extends BaseModuleDefinition {
	readonly kind: "pack.process";
	readonly command: string;
	readonly args?: ReadonlyArray<string>;
	readonly cwd?: string;
	readonly env?: Readonly<Record<string, string>>;
	readonly runtime?: ProcessRuntimeDefinition;
	readonly policy?: ModuleToolPolicyDefinition;
}

export type ModuleDefinition = BuiltinModuleDefinition | ProcessModuleDefinition;

export interface AgentToolDefinition {
	tool: string;
	name?: string;
	with?: JsonObject;
}

export interface PromptCacheDefinition {
	enabled?: boolean;
	force?: boolean;
	retention?: PromptCacheRetention;
	keyScope?: PromptCacheKeyScope;
	shareMode?: PromptCacheShareMode;
	group?: string;
	keyTemplate?: string;
}

export interface AgentDefinition {
	kind: AgentKind;
	description?: string;
	instructions: string;
	instructionsFile?: string;
	vars?: JsonObject;
	promptCache?: PromptCacheDefinition;
	maxTurns?: number;
	profile?: AgentProfileName;
	tools?: AgentToolDefinition[];
	provider?: string;
	model?: string;
	baseUrl?: string;
	organization?: string;
	project?: string;
	endpoint?: string;
	apiVersion?: string;
	deployment?: string;
	temperature?: number;
	maxOutputTokens?: number;
	reasoningEffort?: ReasoningEffort;
}

export interface PlaybookDefaults {
	agentProfile?: AgentProfileName;
}

export interface WorkingMemoryDefinition {
	initial?: JsonObject;
}

export interface LongTermMemoryDefinition {
	provider?: "sqlite" | "mongodb-atlas";
	dbPath?: string;
	namespace?: string;
	connectionString?: string;
	connectionStringEnv?: string;
	database?: string;
	collection?: string;
}

export interface MemoryDefinition {
	working?: WorkingMemoryDefinition;
	longTerm?: LongTermMemoryDefinition;
}

export interface OutputDefinition {
	format?: OutputFormat;
	verbose?: boolean;
	color?: OutputColorMode;
}

export interface GuardrailsPolicy {
	workspaceRoot?: string;
	writableRoots?: string[];
	approvalMode?: ApprovalMode;
}

export interface McpServerDefinition {
	description?: string;
	url?: string;
	headers?: Record<string, string>;
	bearerTokenEnv?: string;
}

export interface A2AAgentDefinition {
	description?: string;
	url?: string;
	cardUrl?: string;
	headers?: Record<string, string>;
	bearerTokenEnv?: string;
}

export interface PackDefinition {
	pack: string;
	version: string;
	description?: string;
	modules?: Record<string, ModuleDefinition>;
	agents?: Record<string, AgentDefinition>;
}

export interface TaskDefinition {
	id: string;
	uses: string;
	needs?: string[];
	vars?: JsonObject;
	with?: JsonObject;
	retry?: Partial<RetryPolicy>;
}

export interface PlaybookDefinition {
	playbook: string;
	version?: string;
	description?: string;
	packs?: string[];
	inputs?: JsonObject;
	defaults?: PlaybookDefaults;
	promptCache?: PromptCacheDefinition;
	memory?: MemoryDefinition;
	output?: OutputDefinition;
	policy?: GuardrailsPolicy;
	mcpServers?: Record<string, McpServerDefinition>;
	a2aAgents?: Record<string, A2AAgentDefinition>;
	modules?: Record<string, ModuleDefinition>;
	agents?: Record<string, AgentDefinition>;
	tasks: TaskDefinition[];
}

export type TaskUseKind = "module" | "agent";

export interface TaskReference {
	kind: TaskUseKind;
	ref: string;
}

export interface CompiledTask {
	id: string;
	use: TaskReference;
	needs: string[];
	vars: JsonObject;
	with: JsonObject;
	retry: RetryPolicy;
}

export interface CompiledPlaybook {
	name: string;
	description?: string;
	inputs: JsonObject;
	defaults: Required<PlaybookDefaults>;
	promptCache: Required<Omit<PromptCacheDefinition, "group" | "keyTemplate">> & Pick<PromptCacheDefinition, "group" | "keyTemplate">;
	memory: {
		working: Required<WorkingMemoryDefinition>;
		longTerm: Required<LongTermMemoryDefinition>;
	};
	output: Required<OutputDefinition>;
	policy: Required<GuardrailsPolicy>;
	mcpServers: Record<string, McpServerDefinition>;
	a2aAgents: Record<string, A2AAgentDefinition>;
	modules: Record<string, ModuleDefinition>;
	agents: Record<string, AgentDefinition>;
	tasks: CompiledTask[];
	taskIndex: Map<string, CompiledTask>;
	dependents: Map<string, string[]>;
}

export interface TaskOutput {
	readonly [key: string]: JsonValue;
}

export interface TaskState {
	status: "pending" | "running" | "waiting_approval" | "succeeded" | "failed";
	attempts: number;
	output?: TaskOutput;
	error?: string;
	approvalId?: string;
}

export interface AgentSessionState {
	attempt: number;
	input: JsonObject;
	resolvedVars?: JsonObject;
	turns: AgentTurnRecord[];
	pendingToolCall?: AgentDecisionToolCall;
	providerState?: JsonObject;
}

export interface RuntimeSnapshot {
	inputs: JsonObject;
	vars: JsonObject;
	memory: {
		working: JsonObject;
	};
	tasks: Record<string, TaskState>;
	agents: Record<string, AgentSessionState>;
}

export interface TaskAttemptRecord {
	taskId: string;
	attempt: number;
	status: "running" | "waiting_approval" | "succeeded" | "failed";
	input: JsonObject;
	output?: TaskOutput;
	error?: string;
	startedAt: string;
	finishedAt?: string;
}

export interface AgentDecisionToolCall {
	kind: "call_tool";
	toolIndex: number;
	reason: string;
	arguments?: JsonObject;
}

export interface AgentDecisionFinish {
	kind: "finish";
	reason: string;
	output: TaskOutput;
}

export type AgentDecision = AgentDecisionToolCall | AgentDecisionFinish;

export interface AgentTurnRecord {
	turn: number;
	decision: AgentDecision;
	observation?: TaskOutput;
	createdAt: string;
}

export interface AgentModelContext {
	runId: string;
	playbookName: string;
	taskId: string;
	agentRef: string;
	agent: AgentDefinition;
	instructions: string;
	maxTurns: number;
	input: JsonObject;
	profile: AgentProfileName;
	tools: AgentToolDefinition[];
	turns: AgentTurnRecord[];
	snapshot: RuntimeSnapshot;
	session: AgentSessionState;
	promptCache: ResolvedPromptCacheConfig;
	recordProviderMetric(metric: AgentProviderMetric): void;
}

export interface AgentModel {
	name: string;
	decide(context: AgentModelContext): Promise<AgentDecision>;
}

export interface ResolvedPromptCacheConfig {
	requested: boolean;
	enabled: boolean;
	force: boolean;
	eligible: boolean;
	disabledReason?: string;
	provider: string;
	baseUrl?: string;
	directOpenAiBaseUrl?: boolean;
	retention: "in-memory" | "24h";
	keyScope: PromptCacheKeyScope;
	shareMode: PromptCacheShareMode;
	group?: string;
	keyTemplate?: string;
	keyBase?: string;
}

export interface AgentProviderMetric {
	readonly provider: string;
	readonly responseId?: string;
	readonly promptCache?: {
		readonly enabled: boolean;
		readonly key?: string;
		readonly retention: "in-memory" | "24h";
		readonly cachedTokens: number;
		readonly inputTokens: number;
		readonly uncachedInputTokens: number;
		readonly outputTokens: number;
	};
}

export interface ToolPolicySpec {
	ref: string;
	provider: ToolProviderKind;
	label: string;
	capability: ToolCapability;
	risk: ToolRisk;
}

export interface AuthorizationDecision {
	decision: "allow" | "deny" | "require_approval";
	reason: string;
	spec: ToolPolicySpec;
}

export interface ApprovalRecord {
	id: string;
	runId: string;
	taskId: string;
	origin: "task" | "agent_tool";
	toolRef: string;
	toolProvider: ToolProviderKind;
	toolLabel: string;
	capability: ToolCapability;
	risk: ToolRisk;
	requestInput: JsonObject;
	reason: string;
	agentProfile?: AgentProfileName;
	status: ApprovalStatus;
	createdAt: string;
	resolvedAt?: string;
	resolvedBy?: string;
	resolutionNote?: string;
}

export type TraceSpanKind = "run" | "task" | "agent" | "tool" | "checkpoint";
export type TraceStatus = "ok" | "error";

export interface TraceSpanRecord {
	id: string;
	runId: string;
	parentId?: string;
	name: string;
	kind: TraceSpanKind;
	status: TraceStatus;
	attributes: JsonObject;
	startedAt: string;
	endedAt?: string;
}

export interface AuditEventRecord {
	seq: number;
	runId: string;
	scope: string;
	name: string;
	level: "info" | "warning" | "error";
	attributes: JsonObject;
	createdAt: string;
}

export interface CheckpointRecord {
	seq: number;
	runId: string;
	taskId?: string;
	status: RunStatus;
	snapshot: RuntimeSnapshot;
	createdAt: string;
}

export type RunStatus = "running" | "paused" | "succeeded" | "failed";

export interface RunRecord {
	id: string;
	playbookName: string;
	status: RunStatus;
	snapshot: RuntimeSnapshot;
	traceId: string;
	createdAt: string;
	updatedAt: string;
}

export interface ExecutionResult {
	run: RunRecord;
	latestCheckpoint: CheckpointRecord;
}
