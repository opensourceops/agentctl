import type {
	AgentDefinition,
	CheckpointRecord,
	CompiledPlaybook,
	CompiledTask,
	ExecutionResult,
	JsonValue,
	ModuleDefinition,
	RunRecord,
	RunStatus,
	RuntimeSnapshot,
	TaskAttemptRecord,
	TaskState,
	ToolPolicySpec,
} from "./types.js";
import { A2ARegistry, createA2ATransportMap, type A2AAgentTransport } from "./a2a.js";
import { AuthStorage } from "./auth-storage.js";
import { BuiltinAgentRegistry } from "./agents.js";
import { getModulePolicySpec, resolveBuiltinToolRef } from "./builtin-tools.js";
import { CheckpointStore } from "./checkpoint-store.js";
import { createLongTermMemoryAdapter } from "./long-term-memory-adapters/factory.js";
import type { LongTermMemoryAdapter } from "./long-term-memory-adapters/types.js";
import { createMcpTransportMap, McpRegistry, type McpServerTransport } from "./mcp.js";
import { ModelRegistry } from "./model-registry.js";
import { BuiltinModuleRegistry, preflightProcessModule } from "./modules.js";
import { PolicyEngine } from "./policy.js";
import { ActiveSpan, OtelTraceSink, TraceRecorder, type TraceSink } from "./tracing.js";
import { buildTemplateContext, deepClone, nowIso, resolveTemplates } from "./utils.js";

export interface EngineHooks {
	afterCheckpoint?(checkpoint: CheckpointRecord): void | Promise<void>;
}

export interface RuntimeOptions {
	traceSinks?: TraceSink[];
	hooks?: EngineHooks;
	mcpServers?: Record<string, McpServerTransport>;
	a2aAgents?: Record<string, A2AAgentTransport>;
	authStorage?: AuthStorage;
	longTermMemory?: LongTermMemoryAdapter;
}

class CheckpointInterruptError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "CheckpointInterruptError";
	}
}

function createInitialSnapshot(plan: CompiledPlaybook, overrides: Record<string, JsonValue>): RuntimeSnapshot {
	const tasks: Record<string, TaskState> = {};
	for (const task of plan.tasks) {
		tasks[task.id] = {
			status: "pending",
			attempts: 0,
		};
	}

	const workingMemory = deepClone(plan.memory.working.initial);
	return {
		inputs: { ...plan.inputs, ...overrides },
		vars: deepClone(workingMemory),
		memory: {
			working: workingMemory,
		},
		tasks,
		agents: {},
	};
}

function cloneSnapshot(snapshot: RuntimeSnapshot): RuntimeSnapshot {
	return deepClone(snapshot);
}

function canRunTask(task: CompiledTask, snapshot: RuntimeSnapshot): boolean {
	return task.needs.every((dependency) => snapshot.tasks[dependency]?.status === "succeeded");
}

function isRunComplete(snapshot: RuntimeSnapshot): RunStatus | undefined {
	const states = Object.values(snapshot.tasks).map((task) => task.status);
	if (states.every((status) => status === "succeeded")) return "succeeded";
	if (states.some((status) => status === "failed")) return "failed";
	return undefined;
}

function normalizeSnapshotForResume(snapshot: RuntimeSnapshot): RuntimeSnapshot {
	const next = cloneSnapshot(snapshot);
	if (!("memory" in next) || !next.memory || typeof next.memory !== "object") {
		next.memory = { working: deepClone(next.vars) };
	}
	if (!("working" in next.memory) || typeof next.memory.working !== "object" || Array.isArray(next.memory.working)) {
		next.memory.working = deepClone(next.vars);
	}
	next.vars = deepClone(next.memory.working);
	for (const [taskId, taskState] of Object.entries(next.tasks)) {
		if (taskState.status === "running" && !next.agents[taskId]) {
			taskState.status = "pending";
		}
	}
	return next;
}

export class PlaybookRuntime {
	private readonly modules: BuiltinModuleRegistry;
	private readonly agents: BuiltinAgentRegistry;
	private readonly policy: PolicyEngine;
	private readonly mcpRegistry: McpRegistry;
	private readonly a2aRegistry: A2ARegistry;
	private readonly hooks: EngineHooks;
	private readonly traceSinks: TraceSink[];
	private readonly longTermMemory: LongTermMemoryAdapter;

	constructor(
		private readonly plan: CompiledPlaybook,
		private readonly store: CheckpointStore,
		options: RuntimeOptions = {},
	) {
		this.traceSinks = options.traceSinks ?? [new OtelTraceSink()];
		this.policy = new PolicyEngine(plan.policy);
		const authStorage = options.authStorage ?? AuthStorage.create();
		this.longTermMemory = options.longTermMemory ?? createLongTermMemoryAdapter(plan.memory.longTerm);
		this.modules = new BuiltinModuleRegistry(this.longTermMemory, plan.memory.longTerm.namespace);
		this.agents = new BuiltinAgentRegistry(new ModelRegistry(authStorage));
		this.mcpRegistry = new McpRegistry(createMcpTransportMap(plan.mcpServers, options.mcpServers));
		this.a2aRegistry = new A2ARegistry(createA2ATransportMap(plan.a2aAgents, options.a2aAgents));
		this.hooks = options.hooks ?? {};
	}

	async start(inputs: Record<string, JsonValue> = {}): Promise<ExecutionResult> {
		await this.preflightProcessModules();
		const snapshot = createInitialSnapshot(this.plan, inputs);
		const run = this.store.createRun(this.plan.name, snapshot);
		await this.hooks.afterCheckpoint?.(this.store.getLatestCheckpoint(run.id));
		return this.execute(run.id, run, this.store.getLatestCheckpoint(run.id).seq);
	}

	async resume(runId: string): Promise<ExecutionResult> {
		await this.preflightProcessModules();
		const run = this.store.getRun(runId);
		if (run.status === "succeeded" || run.status === "failed") {
			throw new Error(`Run "${runId}" is already ${run.status}; use replay to fork from an earlier checkpoint`);
		}
		run.snapshot = normalizeSnapshotForResume(run.snapshot);
		const updatedRun = this.store.updateRun(run.id, "running", run.snapshot);
		const latest = this.store.getLatestCheckpoint(run.id);
		const resumeCheckpoint: CheckpointRecord = {
			runId: run.id,
			seq: latest.seq + 1,
			status: "running",
			snapshot: cloneSnapshot(run.snapshot),
			createdAt: nowIso(),
		};
		this.store.saveCheckpoint(run.id, resumeCheckpoint);
		await this.hooks.afterCheckpoint?.(resumeCheckpoint);
		return this.execute(run.id, updatedRun, resumeCheckpoint.seq);
	}

	async replay(runId: string, checkpointSeq: number): Promise<ExecutionResult> {
		await this.preflightProcessModules();
		const replayRun = this.store.createReplayRun(runId, checkpointSeq);
		replayRun.snapshot = normalizeSnapshotForResume(replayRun.snapshot);
		this.store.updateRun(replayRun.id, "running", replayRun.snapshot);
		await this.hooks.afterCheckpoint?.(this.store.getLatestCheckpoint(replayRun.id));
		return this.execute(replayRun.id, replayRun, this.store.getLatestCheckpoint(replayRun.id).seq);
	}

	private resolveModuleDefinition(ref: string): ModuleDefinition {
		const builtinKind = resolveBuiltinToolRef(ref);
		if (builtinKind) {
			return { kind: builtinKind };
		}
		if (ref.startsWith("builtin.")) {
			return { kind: ref as Exclude<ModuleDefinition["kind"], "pack.process"> };
		}
		const definition = this.plan.modules[ref];
		if (!definition) throw new Error(`Module "${ref}" not found`);
		return definition;
	}

	private async preflightProcessModules(): Promise<void> {
		const moduleRefs = new Set<string>();
		for (const task of this.plan.tasks) {
			if (task.use.kind === "module") {
				moduleRefs.add(task.use.ref);
			}
		}
		for (const agentDefinition of Object.values(this.plan.agents)) {
			for (const tool of agentDefinition.tools ?? []) {
				if (tool.tool.startsWith("mcp:") || tool.tool.startsWith("a2a:")) {
					continue;
				}
				moduleRefs.add(tool.tool);
			}
		}
		for (const ref of moduleRefs) {
			const definition = this.resolveModuleDefinition(ref);
			if (definition.kind === "pack.process") {
				await preflightProcessModule(definition);
			}
		}
	}

	private resolveAgentDefinition(ref: string): AgentDefinition {
		const definition = this.plan.agents[ref];
		if (!definition) throw new Error(`Agent "${ref}" not found`);
		return definition;
	}

	private nextRunnableTask(snapshot: RuntimeSnapshot): CompiledTask | undefined {
		const runningAgent = this.plan.tasks.find(
			(task) => snapshot.tasks[task.id]?.status === "running" && snapshot.agents[task.id],
		);
		if (runningAgent) return runningAgent;

		return this.plan.tasks.find(
			(task) => snapshot.tasks[task.id]?.status === "pending" && canRunTask(task, snapshot),
		);
	}

	private async execute(runId: string, run: RunRecord, checkpointSeq: number): Promise<ExecutionResult> {
		const trace = new TraceRecorder(runId, this.store, this.traceSinks);
		const runSpan = trace.startSpan("playbook.run", "run", { playbook: this.plan.name });
		let snapshot = cloneSnapshot(run.snapshot);
		let seq = checkpointSeq;

		try {
			while (true) {
				const completionStatus = isRunComplete(snapshot);
				if (completionStatus) {
					run = this.store.updateRun(runId, completionStatus, snapshot);
					seq = await this.saveCheckpoint(run, seq, completionStatus, snapshot, undefined, trace, runSpan.id);
					runSpan.end("ok", { final_status: completionStatus });
					return {
						run,
						latestCheckpoint: this.store.getLatestCheckpoint(run.id),
					};
				}

				const task = this.nextRunnableTask(snapshot);
				if (!task) {
					throw new Error("No runnable task found and run is not complete");
				}

				const taskSpan = trace.startSpan("playbook.task", "task", { task_id: task.id, uses: task.use.ref }, runSpan.id);
				snapshot = await this.executeTask(runId, snapshot, seq, task, trace, taskSpan);
				seq = this.store.getLatestCheckpoint(runId).seq;
				taskSpan.end(snapshot.tasks[task.id]?.status === "failed" ? "error" : "ok");
			}
		} catch (error) {
			if (error instanceof CheckpointInterruptError) {
				runSpan.end("error", { interrupted: true, error: error.message });
				throw error;
			}
			run = this.store.updateRun(runId, "failed", snapshot);
			seq = await this.saveCheckpoint(run, seq, "failed", snapshot, undefined, trace, runSpan.id);
			runSpan.end("error", { error: error instanceof Error ? error.message : String(error) });
			throw error;
		}
	}

	private async executeTask(
		runId: string,
		snapshot: RuntimeSnapshot,
		seq: number,
		task: CompiledTask,
		trace: TraceRecorder,
		taskSpan: ActiveSpan,
	): Promise<RuntimeSnapshot> {
		const next = cloneSnapshot(snapshot);
		const taskState = next.tasks[task.id];
		if (!taskState) throw new Error(`Task state for "${task.id}" not found`);
		let checkpointSeq = seq;

		const startingNewAttempt = taskState.status !== "running";
		if (startingNewAttempt) {
			taskState.status = "running";
			taskState.attempts += 1;
			delete taskState.error;
			checkpointSeq = await this.saveCheckpoint(
				this.store.updateRun(runId, "running", next),
				checkpointSeq,
				"running",
				next,
				task.id,
				trace,
				taskSpan.id,
			);
		}

		const attempt = taskState.attempts;
		const attemptRecord: TaskAttemptRecord = {
			taskId: task.id,
			attempt,
			status: "running",
			input: task.with,
			startedAt: nowIso(),
		};
		this.store.recordTaskAttemptForRun(runId, attemptRecord);

		try {
			if (task.use.kind === "module") {
				const definition = this.resolveModuleDefinition(task.use.ref);
				const resolvedInput = this.modules.resolveInput(definition, task.with, next);
				this.assertAuthorized(
					{
						origin: "task",
						spec: getModulePolicySpec(definition, task.use.ref),
						input: resolvedInput,
					},
					trace,
					task.id,
				);
				const result = await this.modules.executeResolved(
					runId,
					task.id,
					definition,
					resolvedInput,
					next,
					this.plan.policy.workspaceRoot,
				);
				taskState.status = "succeeded";
				taskState.output = result.output;
					if (result.stateUpdates) {
						for (const [key, value] of Object.entries(result.stateUpdates)) {
							next.vars[key] = value;
							next.memory.working[key] = value;
						}
					}
			} else {
				const agentDefinition = this.resolveAgentDefinition(task.use.ref);
				const existingSession = next.agents[task.id];
				const result = await this.agents.execute(
					runId,
					task.id,
					agentDefinition,
					task.with,
					next,
					{
						executeTool: async (tool, taskSnapshot, toolCallId) => {
							if (tool.tool.startsWith("mcp:")) {
								const resolved = await this.mcpRegistry.resolveTool(tool.tool);
								const resolvedInput = resolveTemplates(tool.with ?? {}, buildTemplateContext(taskSnapshot));
								this.assertAuthorized(
									{
										origin: "agent_tool",
										spec: resolved.spec,
										input: resolvedInput,
										agentProfile: agentDefinition.profile ?? this.plan.defaults.agentProfile,
									},
									trace,
									task.id,
								);
								const toolSpan = trace.startSpan(
									"playbook.tool",
									"tool",
									{
										task_id: task.id,
										tool_ref: tool.tool,
										tool_label: resolved.spec.label,
										provider: "mcp",
									},
									taskSpan.id,
								);
								try {
									const output = await this.mcpRegistry.callTool(
										resolved.serverName,
										resolved.toolName,
										resolvedInput,
									);
									toolSpan.end("ok");
									return { output };
								} catch (error) {
									toolSpan.end("error", { error: error instanceof Error ? error.message : String(error) });
									throw error;
								}
							}
							if (tool.tool.startsWith("a2a:")) {
								const resolved = this.a2aRegistry.resolveAgent(tool.tool);
								const resolvedInput = resolveTemplates(tool.with ?? {}, buildTemplateContext(taskSnapshot));
								this.assertAuthorized(
									{
										origin: "agent_tool",
										spec: resolved.spec,
										input: resolvedInput,
										agentProfile: agentDefinition.profile ?? this.plan.defaults.agentProfile,
									},
									trace,
									task.id,
								);
								const toolSpan = trace.startSpan(
									"playbook.tool",
									"tool",
									{
										task_id: task.id,
										tool_ref: tool.tool,
										tool_label: resolved.spec.label,
										provider: "a2a",
									},
									taskSpan.id,
								);
								try {
									const response = await this.a2aRegistry.sendTask(resolved.agentName, {
										message: tool.name ?? tool.tool,
										input: resolvedInput,
									});
									if (response.state !== "COMPLETED") {
										throw new Error(`A2A agent ${resolved.agentName} returned ${response.state}`);
									}
									toolSpan.end("ok");
									return {
										output: {
											taskId: response.taskId,
											contextId: response.contextId,
											state: response.state,
											...response.output,
										},
									};
								} catch (error) {
									toolSpan.end("error", { error: error instanceof Error ? error.message : String(error) });
									throw error;
								}
							}
							const definition = this.resolveModuleDefinition(tool.tool);
							const resolvedInput = this.modules.resolveInput(definition, tool.with ?? {}, taskSnapshot);
							this.assertAuthorized(
								{
									origin: "agent_tool",
								spec: getModulePolicySpec(definition, tool.tool),
									input: resolvedInput,
									agentProfile: agentDefinition.profile ?? this.plan.defaults.agentProfile,
								},
								trace,
								task.id,
							);
							const toolSpan = trace.startSpan(
								"playbook.tool",
								"tool",
								{
									task_id: task.id,
									tool_ref: tool.tool,
										tool_label: getModulePolicySpec(definition).label,
								},
								taskSpan.id,
							);
							try {
								const result = await this.modules.executeResolved(
									runId,
									toolCallId,
									definition,
									resolvedInput,
									taskSnapshot,
									this.plan.policy.workspaceRoot,
								);
								toolSpan.end("ok");
								return result;
							} catch (error) {
								toolSpan.end("error", { error: error instanceof Error ? error.message : String(error) });
								throw error;
							}
						},
					},
					existingSession,
					{
						onTurn: async (session, decision, observation, agentSnapshot) => {
							next.agents[task.id] = deepClone(session);
								if (agentSnapshot) {
									next.vars = deepClone(agentSnapshot.vars);
									next.memory = deepClone(agentSnapshot.memory);
								}
							this.store.recordAgentTurn(
								runId,
								task.id,
								attempt,
								session.turns.length,
								JSON.stringify(decision),
								observation ? JSON.stringify(observation) : null,
							);
							checkpointSeq = await this.saveCheckpoint(
								this.store.updateRun(runId, "running", next),
								checkpointSeq,
								"running",
								next,
								task.id,
								trace,
								taskSpan.id,
							);
						},
					},
				);
				taskState.status = "succeeded";
				taskState.output = result.output;
				delete next.agents[task.id];
			}

			this.store.recordTaskAttemptForRun(runId, {
				...attemptRecord,
				status: "succeeded",
				output: taskState.output,
				finishedAt: nowIso(),
			});
			trace.recordAudit("task", "task.succeeded", "info", {
				task_id: task.id,
				attempt,
			});
			await this.saveCheckpoint(
				this.store.updateRun(runId, "running", next),
				checkpointSeq,
				"running",
				next,
				task.id,
				trace,
				taskSpan.id,
			);
			return next;
		} catch (error) {
			if (error instanceof CheckpointInterruptError) {
				throw error;
			}
			const message = error instanceof Error ? error.message : String(error);
			this.store.recordTaskAttemptForRun(runId, {
				...attemptRecord,
				status: "failed",
				error: message,
				finishedAt: nowIso(),
			});
			trace.recordAudit("task", "task.failed", "warning", {
				task_id: task.id,
				attempt,
				error: message,
			});

			delete next.agents[task.id];
			if (attempt < task.retry.maxAttempts) {
				taskState.status = "pending";
				taskState.error = message;
				checkpointSeq = await this.saveCheckpoint(
					this.store.updateRun(runId, "running", next),
					checkpointSeq,
					"running",
					next,
					task.id,
					trace,
					taskSpan.id,
				);
				if (task.retry.backoffMs > 0) {
					await new Promise((resolve) => setTimeout(resolve, task.retry.backoffMs));
				}
				return next;
			}

			taskState.status = "failed";
			taskState.error = message;
			await this.saveCheckpoint(
				this.store.updateRun(runId, "running", next),
				checkpointSeq,
				"running",
				next,
				task.id,
				trace,
				taskSpan.id,
			);
			return next;
		}
	}

	private async saveCheckpoint(
		run: RunRecord,
		currentSeq: number,
		status: RunStatus,
		snapshot: RuntimeSnapshot,
		taskId: string | undefined,
		trace: TraceRecorder,
		parentSpanId?: string,
	): Promise<number> {
		const nextSeq = currentSeq + 1;
			const checkpoint: CheckpointRecord = {
			runId: run.id,
			seq: nextSeq,
			status,
			snapshot: cloneSnapshot(snapshot),
			createdAt: nowIso(),
			...(taskId ? { taskId } : {}),
		};
		const span = trace.startSpan("playbook.checkpoint", "checkpoint", { seq: nextSeq, task_id: taskId ?? "run" }, parentSpanId);
		this.store.saveCheckpoint(run.id, checkpoint);
		this.store.updateRun(run.id, status, snapshot);
		trace.recordAudit("checkpoint", "checkpoint.saved", "info", {
			seq: nextSeq,
			task_id: taskId ?? null,
			status,
		});
		span.end("ok");
		try {
			await this.hooks.afterCheckpoint?.(checkpoint);
		} catch (error) {
			throw new CheckpointInterruptError(error instanceof Error ? error.message : String(error));
		}
		return nextSeq;
	}

	private assertAuthorized(
		request: {
			origin: "task" | "agent_tool";
			spec: ToolPolicySpec;
			input: Record<string, JsonValue>;
			agentProfile?: AgentDefinition["profile"];
		},
		trace: TraceRecorder,
		taskId: string,
	): void {
		const decision = this.policy.authorize({
			origin: request.origin,
			spec: request.spec,
			input: request.input,
			...(request.agentProfile ? { agentProfile: request.agentProfile } : {}),
		});
		trace.recordAudit("policy", `policy.${decision.decision}`, decision.decision === "allow" ? "info" : "warning", {
			task_id: taskId,
			tool_ref: request.spec.ref,
			tool_provider: request.spec.provider,
			capability: decision.spec.capability,
			reason: decision.reason,
			origin: request.origin,
			agent_profile: request.agentProfile ?? null,
		});
		if (decision.decision === "allow") {
			return;
		}
		if (decision.decision === "require_approval") {
			throw new Error(`Tool call requires approval: ${decision.reason}`);
		}
		throw new Error(`Tool call denied: ${decision.reason}`);
	}
}
