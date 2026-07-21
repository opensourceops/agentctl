import type {
	AgentDecision,
	AgentDecisionToolCall,
	AgentDefinition,
	AgentModel,
	AgentModelContext,
	AgentProviderMetric,
	AgentSessionState,
	AgentToolDefinition,
	JsonObject,
	ResolvedPromptCacheConfig,
	RuntimeSnapshot,
	TaskOutput,
} from "./types.js";
import { ApprovalRequiredError } from "./approvals.js";
import type { ModuleExecutionResult } from "./modules.js";
import type { ModelRegistry } from "./model-registry.js";
import { buildTaskTemplateContext, deepClone, isJsonObject, resolveTemplates, stableStringify } from "./utils.js";
import { resolveTemplatesStrict } from "./template-utils.js";
import OpenAI from "openai";
import { AzureOpenAI } from "openai";
import type { Responses as OpenAIResponses } from "openai/resources/responses/responses";

export interface AgentExecutionCallbacks {
	onTurn(
		session: AgentSessionState,
		decision: AgentDecision,
		observation?: TaskOutput,
		snapshot?: RuntimeSnapshot,
	): Promise<void> | void;
}

export interface AgentExecutionResult {
	output: TaskOutput;
	session: AgentSessionState;
	snapshot: RuntimeSnapshot;
}

export interface AgentToolRuntime {
	executeTool(
		tool: AgentToolDefinition,
		taskSnapshot: RuntimeSnapshot,
		toolCallId: string,
	): Promise<ModuleExecutionResult>;
}

export class HeuristicAgentModel implements AgentModel {
	readonly name = "builtin.heuristic";

	async decide(context: AgentModelContext): Promise<AgentDecision> {
		if (context.tools.length === 0) {
			return {
				kind: "finish",
				reason: "no tools configured",
				output: {
					finalText: context.instructions,
				},
			};
		}

		if (context.turns.length < context.tools.length) {
			return {
				kind: "call_tool",
				toolIndex: context.turns.length,
				reason: "run the next configured tool",
			};
		}

		const observations = context.turns
			.map((turn) => turn.observation)
			.filter((observation): observation is TaskOutput => observation !== undefined);
		const lastObservation = observations[observations.length - 1] ?? {};
		const finalText =
			typeof lastObservation.stdout === "string"
				? lastObservation.stdout
				: observations.length > 0
					? stableStringify(lastObservation)
					: context.instructions;

		return {
			kind: "finish",
			reason: "tool observations collected",
			output: {
				finalText,
				observations,
			},
		};
	}
}

interface OpenAIProviderState {
	previousResponseId?: string;
	pendingToolCalls: PendingOpenAIToolCall[];
	completedToolOutputs: PendingOpenAIToolOutput[];
}

interface PendingOpenAIToolCall {
	toolName: string;
	callId: string;
	arguments: JsonObject;
}

interface PendingOpenAIToolOutput {
	callId: string;
	output: TaskOutput;
}

function createProviderState(
	previousResponseId: string | undefined,
	pendingToolCalls: PendingOpenAIToolCall[],
	completedToolOutputs: PendingOpenAIToolOutput[],
): OpenAIProviderState {
	return {
		...(previousResponseId ? { previousResponseId } : {}),
		pendingToolCalls,
		completedToolOutputs,
	};
}

function getProviderState(session: AgentSessionState): OpenAIProviderState {
	if (!session.providerState) {
		return { pendingToolCalls: [], completedToolOutputs: [] };
	}
	const previousResponseId = session.providerState.previousResponseId;
	const pendingToolCalls = Array.isArray(session.providerState.pendingToolCalls)
		? session.providerState.pendingToolCalls.flatMap((entry) => {
				if (!isJsonObject(entry)) {
					return [];
				}
				if (
					typeof entry.toolName !== "string" ||
					typeof entry.callId !== "string" ||
					!isJsonObject(entry.arguments)
				) {
					return [];
				}
				return [
					{
						toolName: entry.toolName,
						callId: entry.callId,
						arguments: entry.arguments,
					},
				];
			})
		: [];
	const completedToolOutputs = Array.isArray(session.providerState.completedToolOutputs)
		? session.providerState.completedToolOutputs.flatMap((entry) => {
				if (!isJsonObject(entry) || typeof entry.callId !== "string" || !isJsonObject(entry.output)) {
					return [];
				}
				return [
					{
						callId: entry.callId,
						output: entry.output,
					},
				];
			})
		: [];
	return {
		...(typeof previousResponseId === "string" ? { previousResponseId } : {}),
		pendingToolCalls,
		completedToolOutputs,
	};
}

function setProviderState(session: AgentSessionState, state: OpenAIProviderState): void {
	if (state.previousResponseId || state.pendingToolCalls.length > 0 || state.completedToolOutputs.length > 0) {
		session.providerState = {
			...(state.previousResponseId ? { previousResponseId: state.previousResponseId } : {}),
			...(state.pendingToolCalls.length > 0
				? {
						pendingToolCalls: state.pendingToolCalls.map((call) => ({
							toolName: call.toolName,
							callId: call.callId,
							arguments: call.arguments,
						})),
					}
				: {}),
			...(state.completedToolOutputs.length > 0
				? {
						completedToolOutputs: state.completedToolOutputs.map((output) => ({
							callId: output.callId,
							output: output.output,
						})),
					}
				: {}),
		};
		return;
	}
	delete session.providerState;
}

function recordToolOutput(session: AgentSessionState, callId: string, output: TaskOutput): void {
	const providerState = getProviderState(session);
	const nextOutputs = [
		...providerState.completedToolOutputs.filter((entry) => entry.callId !== callId),
		{ callId, output },
	];
	setProviderState(
		session,
		createProviderState(providerState.previousResponseId, providerState.pendingToolCalls, nextOutputs),
	);
}

function getToolName(tool: AgentToolDefinition): string {
	if (tool.name) {
		return tool.name.replace(/[^a-zA-Z0-9_]/g, "_");
	}
	const match = /^(?:builtin\/|builtin\.|mcp:[^/]+\/|a2a:)?(.+)$/.exec(tool.tool);
	if (!match) {
		return tool.tool.replace(/[^a-zA-Z0-9_]/g, "_");
	}
	return match[1]!.replace(/[^a-zA-Z0-9_]/g, "_");
}

function buildToolDescription(tool: AgentToolDefinition): string {
	return tool.name ?? `Call the agentctl tool ${tool.tool}`;
}

function buildToolParameters(tool: AgentToolDefinition): Record<string, unknown> {
	const properties: Record<string, { type: string }> = {};
	const required: string[] = [];
	for (const [key, value] of Object.entries(tool.with ?? {})) {
		properties[key] = { type: typeof value === "number" ? "number" : typeof value === "boolean" ? "boolean" : "string" };
		required.push(key);
	}
	return {
		type: "object",
		properties,
		additionalProperties: true,
		required,
	};
}

function extractResponseText(response: OpenAIResponses.Response): string {
	if (response.output_text.length > 0) {
		return response.output_text;
	}
	const text = response.output
		.flatMap((item) => {
			if (item.type !== "message") {
				return [];
			}
			return item.content.flatMap((part) => (part.type === "output_text" ? [part.text] : []));
		})
		.join("\n")
		.trim();
	return text;
}

function supportsTemperature(model: string, reasoningEffort: "minimal" | "low" | "medium" | "high" | undefined): boolean {
	const normalizedModel = model.toLowerCase();
	if (
		normalizedModel === "gpt-5" ||
		normalizedModel === "gpt-5-mini" ||
		normalizedModel === "gpt-5-nano" ||
		normalizedModel.startsWith("gpt-5-")
	) {
		return false;
	}
	if (normalizedModel === "gpt-5.1" || normalizedModel.startsWith("gpt-5.1-")) {
		return false;
	}
	return reasoningEffort === undefined;
}

function resolveProviderPromptCacheRetention(
	directOpenAiBaseUrl: boolean | undefined,
	retention: "in-memory" | "24h",
): "in-memory" | "24h" {
	if (retention === "24h" && directOpenAiBaseUrl === false) {
		return "in-memory";
	}
	return retention;
}

export class OpenAIResponsesAgentModel implements AgentModel {
	readonly name = "openai.responses";

	constructor(private readonly modelRegistry: ModelRegistry) {}

	async decide(context: AgentModelContext): Promise<AgentDecision> {
		const definition = context.agent;
		const config = this.modelRegistry.resolveAgent(definition);
		const promptCacheRetention = resolveProviderPromptCacheRetention(
			context.promptCache.directOpenAiBaseUrl,
			context.promptCache.retention,
		);
		const client =
			config.provider === "azure-openai-responses"
				? new AzureOpenAI({
						apiKey: config.apiKey,
						...(config.baseUrl ? { baseURL: config.baseUrl } : {}),
						...(config.endpoint ? { endpoint: config.endpoint } : {}),
						...(config.apiVersion ? { apiVersion: config.apiVersion } : {}),
						...(config.deployment ? { deployment: config.deployment } : {}),
						...(config.organization ? { organization: config.organization } : {}),
						...(config.project ? { project: config.project } : {}),
					})
				: new OpenAI({
						apiKey: config.apiKey,
						...(config.baseUrl ? { baseURL: config.baseUrl } : {}),
						...(config.organization ? { organization: config.organization } : {}),
						...(config.project ? { project: config.project } : {}),
					});

		const toolNameToIndex = new Map<string, number>();
		const tools: OpenAIResponses.FunctionTool[] = context.tools.map((tool, index) => {
			const toolName = getToolName(tool);
			toolNameToIndex.set(toolName, index);
			return {
				type: "function",
				name: toolName,
				description: buildToolDescription(tool),
				parameters: buildToolParameters(tool),
				strict: false,
			};
		});

		const providerState = getProviderState(context.session);
		if (providerState.pendingToolCalls.length > 0) {
			const nextCall = providerState.pendingToolCalls[0];
			if (!nextCall) {
				throw new Error("Provider state reported pending tool calls but none were available");
			}
			const remainingCalls = providerState.pendingToolCalls.slice(1);
			const toolIndex = toolNameToIndex.get(nextCall.toolName);
			if (toolIndex === undefined) {
				throw new Error(`Model selected unknown tool "${nextCall.toolName}"`);
			}
			setProviderState(
				context.session,
				createProviderState(providerState.previousResponseId, remainingCalls, providerState.completedToolOutputs),
			);
			return {
				kind: "call_tool",
				toolIndex,
				reason: `model requested ${nextCall.toolName}`,
				arguments: {
					...nextCall.arguments,
					call_id: nextCall.callId,
				},
			};
		}

		const response = await client.responses.create({
			model: config.model,
			instructions:
				`${context.instructions}\n\n` +
				`Use available tools when needed. Call at most one tool per response. ` +
				`When you are done, return the final answer directly.`,
			input: providerState.previousResponseId
				? providerState.completedToolOutputs.map((entry) => ({
						type: "function_call_output" as const,
						call_id: entry.callId,
						output: JSON.stringify({
							...entry.output,
							call_id: entry.callId,
						}),
					}))
				: [
						{
							role: "user",
							content: JSON.stringify({
								task_input: context.input,
								run_id: context.runId,
								task_id: context.taskId,
							}),
						},
					],
			...(providerState.previousResponseId ? { previous_response_id: providerState.previousResponseId } : {}),
			...(tools.length > 0 ? { tools } : {}),
			...(context.promptCache.enabled && context.promptCache.keyBase
				? {
						prompt_cache_key: context.promptCache.keyBase,
						prompt_cache_retention: promptCacheRetention,
					}
				: {}),
			...(config.temperature !== undefined && supportsTemperature(config.model, config.reasoningEffort)
				? { temperature: config.temperature }
				: {}),
			...(config.maxOutputTokens !== undefined ? { max_output_tokens: config.maxOutputTokens } : {}),
			...(config.reasoningEffort ? { reasoning: { effort: config.reasoningEffort } } : {}),
		});
		const cachedTokens = response.usage?.input_tokens_details?.cached_tokens ?? 0;
		const inputTokens = response.usage?.input_tokens ?? 0;
		const outputTokens = response.usage?.output_tokens ?? 0;
		if (context.promptCache.enabled) {
			context.recordProviderMetric({
				provider: config.provider,
				responseId: response.id,
				promptCache: {
					enabled: context.promptCache.enabled,
					...(context.promptCache.keyBase ? { key: context.promptCache.keyBase } : {}),
					retention: promptCacheRetention,
					cachedTokens,
					inputTokens,
					uncachedInputTokens: Math.max(0, inputTokens - cachedTokens),
					outputTokens,
				},
			});
		}

		const functionCalls = response.output.filter(
			(item): item is OpenAIResponses.ResponseFunctionToolCall => item.type === "function_call",
		);
		if (functionCalls.length > 0) {
			const pendingToolCalls = functionCalls.map((functionCall) => {
				const parsedArguments = JSON.parse(functionCall.arguments) as unknown;
				if (!isJsonObject(parsedArguments)) {
					throw new Error(`Tool arguments for "${functionCall.name}" must be a JSON object`);
				}
				return {
					toolName: functionCall.name,
					callId: functionCall.call_id,
					arguments: parsedArguments,
				};
			});
			const nextCall = pendingToolCalls[0];
			if (!nextCall) {
				throw new Error("Model returned function calls but none could be parsed");
			}
			const remainingCalls = pendingToolCalls.slice(1);
			const toolIndex = toolNameToIndex.get(nextCall.toolName);
			if (toolIndex === undefined) {
				throw new Error(`Model selected unknown tool "${nextCall.toolName}"`);
			}
			setProviderState(context.session, createProviderState(response.id, remainingCalls, []));
			return {
				kind: "call_tool",
				toolIndex,
				reason: `model requested ${nextCall.toolName}`,
				arguments: {
					...nextCall.arguments,
					call_id: nextCall.callId,
				},
			};
		}

		setProviderState(context.session, createProviderState(response.id, [], []));

		return {
			kind: "finish",
			reason: "model completed without tool call",
			output: {
				finalText: extractResponseText(response),
			},
		};
	}
}

export class BuiltinAgentRegistry {
	private readonly models: Map<string, AgentModel>;

	constructor(modelRegistry?: ModelRegistry) {
		this.models = new Map<string, AgentModel>([["builtin.heuristic", new HeuristicAgentModel()]]);
		if (modelRegistry) {
			this.models.set("openai.responses", new OpenAIResponsesAgentModel(modelRegistry));
		}
	}

	register(kind: string, model: AgentModel): void {
		this.models.set(kind, model);
	}

	async execute(
		runId: string,
		playbookName: string,
		taskId: string,
		agentRef: string,
		definition: AgentDefinition,
		taskInput: JsonObject,
		resolvedVars: JsonObject,
		promptCache: ResolvedPromptCacheConfig,
		snapshot: RuntimeSnapshot,
		toolRuntime: AgentToolRuntime,
		existingSession: AgentSessionState | undefined,
		callbacks: AgentExecutionCallbacks,
		recordProviderMetric: (metric: AgentProviderMetric) => void,
	): Promise<AgentExecutionResult> {
		const model = this.models.get(definition.kind);
		if (!model) throw new Error(`No agent model registered for kind "${definition.kind}"`);

		const maxTurns = definition.maxTurns ?? 4;
		const resolvedInput = resolveTemplates(taskInput, buildTaskTemplateContext(snapshot, resolvedVars, taskInput));
		const session: AgentSessionState = existingSession
				? {
						attempt: existingSession.attempt,
						input: existingSession.input,
						resolvedVars: deepClone(existingSession.resolvedVars ?? resolvedVars),
						turns: [...existingSession.turns],
						...(existingSession.pendingToolCall ? { pendingToolCall: deepClone(existingSession.pendingToolCall) } : {}),
						...(existingSession.providerState ? { providerState: deepClone(existingSession.providerState) } : {}),
					}
				: {
						attempt: 1,
						input: resolvedInput,
						resolvedVars: deepClone(resolvedVars),
						turns: [],
					};
		let workingSnapshot = deepClone(snapshot);

		while (session.turns.length < maxTurns) {
			const tools = (definition.tools ?? []).map((tool) =>
				resolveTool(tool, workingSnapshot, session.resolvedVars ?? {}, session.input),
			);
			const templateContext = buildTaskTemplateContext(workingSnapshot, session.resolvedVars ?? {}, session.input);
			const resolvedInstructionsValue = resolveTemplatesStrict(definition.instructions, {
				...templateContext,
			});
			const resolvedInstructions =
				typeof resolvedInstructionsValue === "string"
					? resolvedInstructionsValue
					: stableStringify(resolvedInstructionsValue);
			const decision =
				session.pendingToolCall ??
				(await model.decide({
					runId,
					playbookName,
					taskId,
					agentRef,
					agent: definition,
					instructions: resolvedInstructions,
					maxTurns,
					input: session.input,
					profile: definition.profile ?? "none",
					tools,
					turns: session.turns,
					snapshot: workingSnapshot,
					session,
					promptCache,
					recordProviderMetric,
				}));

			if (decision.kind === "finish") {
				return {
					output: decision.output,
					session,
					snapshot: workingSnapshot,
				};
			}

			const selectedTool = tools[decision.toolIndex];
			if (!selectedTool) {
				throw new Error(`Agent selected missing tool index ${decision.toolIndex}`);
			}

			const toolInput = decision.arguments
				? (() => {
						const nextArguments = { ...decision.arguments };
						delete nextArguments.call_id;
						return {
							...selectedTool,
							with: {
								...(selectedTool.with ?? {}),
								...nextArguments,
							},
						};
					})()
				: selectedTool;
			let toolResult: ModuleExecutionResult;
			try {
				toolResult = await toolRuntime.executeTool(
					toolInput,
					workingSnapshot,
					`${taskId}::tool:${selectedTool.name ?? selectedTool.tool}`,
				);
			} catch (error) {
				if (error instanceof ApprovalRequiredError) {
					session.pendingToolCall = deepClone(decision as AgentDecisionToolCall);
					throw new ApprovalRequiredError(error.approval, {
						session: deepClone(session),
						snapshot: deepClone(workingSnapshot),
					});
				}
				throw error;
			}
			const observation =
				typeof decision.arguments?.call_id === "string"
					? {
							...toolResult.output,
							call_id: decision.arguments.call_id,
						}
					: toolResult.output;
				if (toolResult.stateUpdates) {
					for (const [key, value] of Object.entries(toolResult.stateUpdates)) {
						workingSnapshot.vars[key] = value;
						workingSnapshot.memory.working[key] = value;
					}
				}

			const turn = {
				turn: session.turns.length + 1,
				decision,
				observation,
				createdAt: new Date().toISOString(),
			};
			if (typeof decision.arguments?.call_id === "string") {
				recordToolOutput(session, decision.arguments.call_id, observation);
			}
			delete session.pendingToolCall;
			session.turns.push(turn);
			await callbacks.onTurn(session, decision, observation, workingSnapshot);
		}

		throw new Error(`Agent exceeded maxTurns=${maxTurns}`);
	}
}

function resolveTool(
	tool: AgentToolDefinition,
	snapshot: RuntimeSnapshot,
	resolvedVars: JsonObject,
	input: JsonObject,
): AgentToolDefinition {
	const resolvedTool = resolveTemplates(
		{
			tool: tool.tool,
			...(tool.name ? { name: tool.name } : {}),
			...(tool.with ? { with: tool.with } : {}),
		},
		buildTaskTemplateContext(snapshot, resolvedVars, input),
	);
	return {
		tool: String(resolvedTool.tool),
		...(typeof resolvedTool.name === "string" ? { name: resolvedTool.name } : {}),
		...(isJsonObject(resolvedTool.with) ? { with: resolvedTool.with } : {}),
	};
}
