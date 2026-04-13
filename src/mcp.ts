import { randomUUID } from "node:crypto";
import type {
	JsonValue,
	McpServerDefinition,
	TaskOutput,
	ToolCapability,
	ToolRisk,
	ToolPolicySpec,
} from "./types.js";

const MCP_PROTOCOL_VERSION = "2025-11-25";
const DEFAULT_MCP_TOOL_CAPABILITY: ToolCapability = "act";
const DEFAULT_MCP_TOOL_RISK: ToolRisk = "high";

interface JsonRpcSuccessResponse {
	jsonrpc: "2.0";
	id: string | number | null;
	result: JsonValue;
}

interface JsonRpcErrorObject {
	code: number;
	message: string;
	data?: JsonValue;
}

interface JsonRpcErrorResponse {
	jsonrpc: "2.0";
	id: string | number | null;
	error: JsonRpcErrorObject;
}

type JsonRpcResponse = JsonRpcSuccessResponse | JsonRpcErrorResponse;

interface McpListToolsResult {
	tools?: McpProtocolTool[];
}

interface McpInitializeResult {
	protocolVersion?: string;
}

interface McpProtocolTool {
	name?: string;
	description?: string;
	annotations?: {
		title?: string;
		readOnlyHint?: boolean;
		destructiveHint?: boolean;
		idempotentHint?: boolean;
		openWorldHint?: boolean;
	};
}

export interface McpToolDescriptor {
	name: string;
	description?: string;
	capability: ToolCapability;
	risk: ToolRisk;
}

export interface McpToolCallResult {
	content?: string;
	structuredContent?: TaskOutput;
	isError?: boolean;
}

export interface McpServerTransport {
	listTools(): Promise<McpToolDescriptor[]>;
	callTool(name: string, args: Record<string, JsonValue>): Promise<McpToolCallResult>;
}

export interface ResolvedMcpTool {
	serverName: string;
	toolName: string;
	spec: ToolPolicySpec;
}

interface RemoteMcpTransportConfig {
	url: string;
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

function isJsonRpcErrorResponse(response: JsonRpcResponse): response is JsonRpcErrorResponse {
	return "error" in response;
}

function ensureRecord(value: JsonValue | undefined, context: string): Record<string, JsonValue> {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error(`${context} must be an object`);
	}
	return value as Record<string, JsonValue>;
}

function parseSseJsonRpcPayload(body: string, requestId: string): JsonRpcResponse {
	for (const chunk of body.split("\n\n")) {
		const dataLines = chunk
			.split("\n")
			.filter((line) => line.startsWith("data:"))
			.map((line) => line.slice(5).trim())
			.filter((line) => line.length > 0);
		if (dataLines.length === 0) {
			continue;
		}
		const payload = JSON.parse(dataLines.join("\n")) as JsonRpcResponse;
		if ("id" in payload && String(payload.id) === requestId) {
			return payload;
		}
	}
	throw new Error(`MCP stream response did not include JSON-RPC result for request ${requestId}`);
}

function parseJsonRpcResponse(body: string, contentType: string | null, requestId: string): JsonRpcResponse {
	if (contentType?.includes("text/event-stream")) {
		return parseSseJsonRpcPayload(body, requestId);
	}
	return JSON.parse(body) as JsonRpcResponse;
}

function deriveCapability(tool: McpProtocolTool): ToolCapability {
	if (tool?.annotations?.readOnlyHint) {
		return "observe";
	}
	if (tool?.annotations?.destructiveHint) {
		return "mutate";
	}
	return DEFAULT_MCP_TOOL_CAPABILITY;
}

function deriveRisk(tool: McpProtocolTool): ToolRisk {
	if (tool?.annotations?.readOnlyHint) {
		return "low";
	}
	if (tool?.annotations?.destructiveHint) {
		return "high";
	}
	if (tool?.annotations?.idempotentHint) {
		return "medium";
	}
	return DEFAULT_MCP_TOOL_RISK;
}

function normalizeToolCallResult(result: Record<string, JsonValue>): McpToolCallResult {
	const structuredContent = result.structuredContent;
	if (structuredContent && typeof structuredContent === "object" && !Array.isArray(structuredContent)) {
		return {
			structuredContent: structuredContent as TaskOutput,
			...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
		};
	}

	const content = result.content;
	if (typeof content === "string") {
		return {
			content,
			...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
		};
	}

	if (Array.isArray(content)) {
		const text = content
			.map((part) => {
				if (!part || typeof part !== "object" || Array.isArray(part)) {
					return "";
				}
				const record = part as Record<string, JsonValue>;
				return typeof record.text === "string" ? record.text : "";
			})
			.filter((part) => part.length > 0)
			.join("\n");
		return {
			content: text,
			...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
		};
	}

	return {
		content: JSON.stringify(result),
		...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
	};
}

export class RemoteMcpHttpTransport implements McpServerTransport {
	private sessionId: string | undefined;
	private initialized = false;

	constructor(private readonly config: RemoteMcpTransportConfig) {}

	async listTools(): Promise<McpToolDescriptor[]> {
		await this.ensureInitialized();
		const result = await this.request("tools/list", {});
		const parsed = ensureRecord(result, "MCP tools/list result") as McpListToolsResult;
		return (parsed.tools ?? []).flatMap((tool) => {
			if (typeof tool?.name !== "string" || tool.name.length === 0) {
				return [];
			}
			return [
				{
					name: tool.name,
					...(tool.description ? { description: tool.description } : {}),
					capability: deriveCapability(tool),
					risk: deriveRisk(tool),
				},
			];
		});
	}

	async callTool(name: string, args: Record<string, JsonValue>): Promise<McpToolCallResult> {
		await this.ensureInitialized();
		const result = await this.request("tools/call", {
			name,
			arguments: args,
		});
		return normalizeToolCallResult(ensureRecord(result, "MCP tools/call result"));
	}

	private async ensureInitialized(): Promise<void> {
		if (this.initialized) {
			return;
		}

		const result = await this.request("initialize", {
			protocolVersion: MCP_PROTOCOL_VERSION,
			clientInfo: {
				name: "agentctl",
				version: "0.1.0",
			},
			capabilities: {},
		});
		const initializeResult = ensureRecord(result, "MCP initialize result") as McpInitializeResult;
		this.initialized = true;
		if (typeof initializeResult.protocolVersion === "string" && initializeResult.protocolVersion.length > 0) {
			await this.notify("notifications/initialized", {});
		}
	}

	private async request(method: string, params: Record<string, JsonValue>): Promise<JsonValue> {
		return this.sendJsonRpc(method, params, false);
	}

	private async notify(method: string, params: Record<string, JsonValue>): Promise<void> {
		await this.sendJsonRpc(method, params, true);
	}

	private async sendJsonRpc(method: string, params: Record<string, JsonValue>, notification: boolean): Promise<JsonValue> {
		const requestId = randomUUID();
		const headers: Record<string, string> = {
			Accept: "application/json, text/event-stream",
			"Content-Type": "application/json",
			...buildAuthHeaders(this.config),
		};
		if (this.sessionId) {
			headers["MCP-Session-Id"] = this.sessionId;
			headers["MCP-Protocol-Version"] = MCP_PROTOCOL_VERSION;
		}

		const response = await fetch(this.config.url, {
			method: "POST",
			headers,
			body: JSON.stringify({
				jsonrpc: "2.0",
				...(notification ? {} : { id: requestId }),
				method,
				params,
			}),
		});

		const nextSessionId = response.headers.get("MCP-Session-Id");
		if (nextSessionId) {
			this.sessionId = nextSessionId;
		}

		if (response.status === 404 && this.sessionId && method !== "initialize") {
			this.initialized = false;
			this.sessionId = undefined;
			await this.ensureInitialized();
			return this.sendJsonRpc(method, params, notification);
		}

		if (!response.ok && !(notification && response.status === 202)) {
			const body = await response.text();
			throw new Error(`MCP request ${method} failed with HTTP ${response.status}: ${body}`);
		}

		if (notification) {
			return null;
		}

		const body = await response.text();
		const rpcResponse = parseJsonRpcResponse(body, response.headers.get("content-type"), requestId);
		if (isJsonRpcErrorResponse(rpcResponse)) {
			throw new Error(`MCP ${method} failed: ${rpcResponse.error.message}`);
		}
		return rpcResponse.result;
	}
}

export function createRemoteMcpTransport(definition: McpServerDefinition): McpServerTransport | undefined {
	if (!definition.url) {
		return undefined;
	}
	return new RemoteMcpHttpTransport({
		url: definition.url,
		...(definition.headers ? { headers: definition.headers } : {}),
		...(definition.bearerTokenEnv ? { bearerTokenEnv: definition.bearerTokenEnv } : {}),
	});
}

export function createMcpTransportMap(
	definitions: Record<string, McpServerDefinition>,
	overrides: Record<string, McpServerTransport> = {},
): Record<string, McpServerTransport> {
	const transports: Record<string, McpServerTransport> = { ...overrides };
	for (const [name, definition] of Object.entries(definitions)) {
		if (transports[name]) {
			continue;
		}
		const remote = createRemoteMcpTransport(definition);
		if (remote) {
			transports[name] = remote;
		}
	}
	return transports;
}

export class McpRegistry {
	private readonly toolCache = new Map<string, Map<string, McpToolDescriptor>>();

	constructor(private readonly servers: Record<string, McpServerTransport> = {}) {}

	hasServer(name: string): boolean {
		return name in this.servers;
	}

	async resolveTool(ref: string): Promise<ResolvedMcpTool> {
		const match = /^mcp:([^/]+)\/(.+)$/.exec(ref);
		if (!match) {
			throw new Error(`Invalid MCP tool reference "${ref}"`);
		}
		const serverName = match[1]!;
		const toolName = match[2]!;
		const server = this.servers[serverName];
		if (!server) {
			throw new Error(`MCP server "${serverName}" is not registered`);
		}

		let tools = this.toolCache.get(serverName);
		if (!tools) {
			tools = new Map((await server.listTools()).map((tool) => [tool.name, tool]));
			this.toolCache.set(serverName, tools);
		}

		const descriptor = tools.get(toolName);
		if (!descriptor) {
			throw new Error(`MCP server "${serverName}" does not expose tool "${toolName}"`);
		}

		return {
			serverName,
			toolName,
			spec: {
				ref,
				provider: "mcp",
				label: `${serverName}/${toolName}`,
				capability: descriptor.capability,
				risk: descriptor.risk,
			},
		};
	}

	async callTool(serverName: string, toolName: string, args: Record<string, JsonValue>): Promise<TaskOutput> {
		const server = this.servers[serverName];
		if (!server) {
			throw new Error(`MCP server "${serverName}" is not registered`);
		}
		const result = await server.callTool(toolName, args);
		if (result.isError) {
			throw new Error(`MCP tool ${serverName}/${toolName} failed: ${result.content ?? "unknown error"}`);
		}
		if (result.structuredContent) {
			return result.structuredContent;
		}
		return {
			content: result.content ?? "",
		};
	}
}
