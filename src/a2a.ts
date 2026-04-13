import { randomUUID } from "node:crypto";
import type { A2AAgentDefinition, JsonValue, TaskOutput, ToolPolicySpec } from "./types.js";

const DEFAULT_A2A_POLL_INTERVAL_MS = 25;

interface JsonRpcSuccessResponse {
	jsonrpc: "2.0";
	id: string | number | null;
	result: JsonValue;
}

interface JsonRpcErrorResponse {
	jsonrpc: "2.0";
	id: string | number | null;
	error: {
		code: number;
		message: string;
		data?: JsonValue;
	};
}

type JsonRpcResponse = JsonRpcSuccessResponse | JsonRpcErrorResponse;

interface A2AAgentCard {
	url?: string;
	skills?: Array<{
		id?: string;
		name?: string;
		description?: string;
	}>;
}

interface A2AProtocolTask {
	id?: string;
	contextId?: string;
	state?: string;
	status?: {
		state?: string;
		message?: {
			parts?: Array<{
				text?: string;
			}>;
		};
	};
	artifacts?: Array<{
		parts?: Array<{
			text?: string;
		}>;
	}>;
	metadata?: Record<string, JsonValue>;
}

export interface A2ATaskRequest {
	taskId?: string;
	contextId?: string;
	message: string;
	input: Record<string, JsonValue>;
}

export interface A2ATaskResponse {
	taskId: string;
	contextId: string;
	state: "COMPLETED" | "FAILED";
	output: TaskOutput;
}

export interface A2AAgentTransport {
	sendTask(request: A2ATaskRequest): Promise<A2ATaskResponse>;
}

export interface ResolvedA2AAgent {
	agentName: string;
	spec: ToolPolicySpec;
}

interface RemoteA2ATransportConfig {
	url?: string;
	cardUrl?: string;
	headers?: Record<string, string>;
	bearerTokenEnv?: string;
}

function buildAuthHeaders(config: { headers?: Record<string, string>; bearerTokenEnv?: string }): Record<string, string> {
	const headers = { ...(config.headers ?? {}) };
	if (config.bearerTokenEnv) {
		const token = process.env[config.bearerTokenEnv];
		if (!token) {
			throw new Error(`Environment variable "${config.bearerTokenEnv}" is required for remote authentication`);
		}
		headers.Authorization = `Bearer ${token}`;
	}
	return headers;
}

function ensureRecord(value: JsonValue | undefined, context: string): Record<string, JsonValue> {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error(`${context} must be an object`);
	}
	return value as Record<string, JsonValue>;
}

function isJsonRpcErrorResponse(response: JsonRpcResponse): response is JsonRpcErrorResponse {
	return "error" in response;
}

function normalizeTaskState(value: string | undefined): string {
	if (!value) {
		return "unknown";
	}
	return value.replace(/^TASK_STATE_/, "").toLowerCase();
}

function isCompletedState(value: string): boolean {
	return value === "completed";
}

function isFailedState(value: string): boolean {
	return value === "failed" || value === "canceled" || value === "cancelled" || value === "rejected";
}

function extractTextParts(parts: Array<{ text?: string }> | undefined): string[] {
	return (parts ?? []).flatMap((part) => (typeof part.text === "string" && part.text.length > 0 ? [part.text] : []));
}

function normalizeTaskOutput(task: A2AProtocolTask): TaskOutput {
	const metadataOutput = task.metadata?.output;
	if (metadataOutput && typeof metadataOutput === "object" && !Array.isArray(metadataOutput)) {
		return metadataOutput as TaskOutput;
	}

	const artifactText = (task.artifacts ?? [])
		.flatMap((artifact) => extractTextParts(artifact.parts))
		.join("\n")
		.trim();
	if (artifactText.length > 0) {
		return { finalText: artifactText };
	}

	const statusText = extractTextParts(task.status?.message?.parts).join("\n").trim();
	if (statusText.length > 0) {
		return { finalText: statusText };
	}

	return {};
}

export class RemoteA2AHttpTransport implements A2AAgentTransport {
	private endpointUrl: string | undefined;
	private cardLoaded = false;

	constructor(private readonly config: RemoteA2ATransportConfig) {
		if (config.url) {
			this.endpointUrl = config.url;
		}
	}

	async sendTask(request: A2ATaskRequest): Promise<A2ATaskResponse> {
		await this.ensureDiscovered();

		const initialTask = await this.sendMessage(request);
		const terminalTask = await this.waitForTerminalState(initialTask);
		const state = normalizeTaskState(terminalTask.status?.state ?? terminalTask.state);
		if (isFailedState(state)) {
			throw new Error(`A2A agent returned ${state}`);
		}
		if (!isCompletedState(state)) {
			throw new Error(`A2A agent returned non-terminal state ${state}`);
		}

		return {
			taskId: terminalTask.id ?? request.taskId ?? "missing-task-id",
			contextId: terminalTask.contextId ?? request.contextId ?? "missing-context-id",
			state: "COMPLETED",
			output: normalizeTaskOutput(terminalTask),
		};
	}

	private async ensureDiscovered(): Promise<void> {
		if (this.cardLoaded) {
			return;
		}
		if (this.config.cardUrl) {
			const response = await fetch(this.config.cardUrl, {
				method: "GET",
				headers: {
					Accept: "application/json",
					...buildAuthHeaders(this.config),
				},
			});
			if (!response.ok) {
				throw new Error(`A2A agent card request failed with HTTP ${response.status}`);
			}
			const card = (await response.json()) as A2AAgentCard;
			if (typeof card.url === "string" && card.url.length > 0) {
				this.endpointUrl = card.url;
			}
		}
		if (!this.endpointUrl) {
			throw new Error("A2A remote transport requires either url or cardUrl");
		}
		this.cardLoaded = true;
	}

	private async sendMessage(request: A2ATaskRequest): Promise<A2AProtocolTask> {
		const result = await this.callJsonRpc("message/send", {
			id: request.taskId ?? randomUUID(),
			sessionId: request.contextId ?? randomUUID(),
			message: {
				role: "user",
				parts: [{ type: "text", text: request.message }],
				messageId: request.taskId ?? randomUUID(),
				metadata: {
					input: request.input,
				},
			},
		}).catch(async (error) => {
			if (error instanceof Error && error.message.includes("Method not found")) {
				return this.callJsonRpc("tasks/send", {
					id: request.taskId ?? randomUUID(),
					sessionId: request.contextId ?? randomUUID(),
					message: {
						role: "user",
						parts: [{ type: "text", text: request.message }],
						messageId: request.taskId ?? randomUUID(),
						metadata: {
							input: request.input,
						},
					},
				});
			}
			throw error;
		});
		return this.unwrapTask(result, "A2A message/send result");
	}

	private async waitForTerminalState(task: A2AProtocolTask): Promise<A2AProtocolTask> {
		let current = task;
		while (true) {
			const state = normalizeTaskState(current.status?.state ?? current.state);
			if (isCompletedState(state) || isFailedState(state)) {
				return current;
			}
			if (!current.id) {
				throw new Error("A2A task polling requires a task id");
			}
			await new Promise((resolve) => setTimeout(resolve, DEFAULT_A2A_POLL_INTERVAL_MS));
			const result = await this.callJsonRpc("tasks/get", { id: current.id });
			current = this.unwrapTask(result, "A2A tasks/get result");
		}
	}

	private unwrapTask(value: JsonValue, context: string): A2AProtocolTask {
		const record = ensureRecord(value, context);
		if ("task" in record && record.task && typeof record.task === "object" && !Array.isArray(record.task)) {
			return record.task as A2AProtocolTask;
		}
		return record as A2AProtocolTask;
	}

	private async callJsonRpc(method: string, params: Record<string, JsonValue>): Promise<JsonValue> {
		if (!this.endpointUrl) {
			throw new Error("A2A endpoint URL is not configured");
		}
		const requestId = randomUUID();
		const response = await fetch(this.endpointUrl, {
			method: "POST",
			headers: {
				Accept: "application/json",
				"Content-Type": "application/json",
				...buildAuthHeaders(this.config),
			},
			body: JSON.stringify({
				jsonrpc: "2.0",
				id: requestId,
				method,
				params,
			}),
		});
		if (!response.ok) {
			const body = await response.text();
			throw new Error(`A2A request ${method} failed with HTTP ${response.status}: ${body}`);
		}
		const payload = (await response.json()) as JsonRpcResponse;
		if (isJsonRpcErrorResponse(payload)) {
			throw new Error(payload.error.message);
		}
		return payload.result;
	}
}

export function createRemoteA2ATransport(definition: A2AAgentDefinition): A2AAgentTransport | undefined {
	if (!definition.url && !definition.cardUrl) {
		return undefined;
	}
	return new RemoteA2AHttpTransport({
		...(definition.url ? { url: definition.url } : {}),
		...(definition.cardUrl ? { cardUrl: definition.cardUrl } : {}),
		...(definition.headers ? { headers: definition.headers } : {}),
		...(definition.bearerTokenEnv ? { bearerTokenEnv: definition.bearerTokenEnv } : {}),
	});
}

export function createA2ATransportMap(
	definitions: Record<string, A2AAgentDefinition>,
	overrides: Record<string, A2AAgentTransport> = {},
): Record<string, A2AAgentTransport> {
	const transports: Record<string, A2AAgentTransport> = { ...overrides };
	for (const [name, definition] of Object.entries(definitions)) {
		if (transports[name]) {
			continue;
		}
		const remote = createRemoteA2ATransport(definition);
		if (remote) {
			transports[name] = remote;
		}
	}
	return transports;
}

export class A2ARegistry {
	constructor(private readonly agents: Record<string, A2AAgentTransport> = {}) {}

	hasAgent(name: string): boolean {
		return name in this.agents;
	}

	resolveAgent(ref: string): ResolvedA2AAgent {
		const match = /^a2a:(.+)$/.exec(ref);
		if (!match) {
			throw new Error(`Invalid A2A agent reference "${ref}"`);
		}
		const agentName = match[1]!;
		if (!this.agents[agentName]) {
			throw new Error(`A2A agent "${agentName}" is not registered`);
		}
		return {
			agentName,
			spec: {
				ref,
				provider: "a2a",
				label: agentName,
				capability: "act",
				risk: "high",
			},
		};
	}

	async sendTask(agentName: string, request: Omit<A2ATaskRequest, "taskId" | "contextId">): Promise<A2ATaskResponse> {
		const agent = this.agents[agentName];
		if (!agent) {
			throw new Error(`A2A agent "${agentName}" is not registered`);
		}
		return agent.sendTask({
			taskId: randomUUID(),
			contextId: randomUUID(),
			...request,
		});
	}
}
