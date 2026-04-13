import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import type { A2AAgentTransport } from "../src/a2a.js";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import type { JsonValue } from "../src/types.js";
import type { McpServerTransport } from "../src/mcp.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

interface JsonRpcRequest {
	jsonrpc: "2.0";
	id?: string;
	method: string;
	params?: Record<string, JsonValue>;
}

function createTempDb(name: string): { dir: string; dbPath: string } {
	const dir = mkdtempSync(join(tmpdir(), `${name}-`));
	return { dir, dbPath: join(dir, "runtime.db") };
}

async function readJsonRequest(request: IncomingMessage): Promise<JsonRpcRequest> {
	const chunks: Buffer[] = [];
	for await (const chunk of request) {
		chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
	}
	return JSON.parse(Buffer.concat(chunks).toString("utf8")) as JsonRpcRequest;
}

async function withHttpServer(
	handler: (request: IncomingMessage, response: ServerResponse) => Promise<void> | void,
	run: (baseUrl: string) => Promise<void>,
): Promise<void> {
	const server = createServer((request, response) => {
		Promise.resolve(handler(request, response)).catch((error: unknown) => {
			response.statusCode = 500;
			response.setHeader("content-type", "application/json");
			response.end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) }));
		});
	});

	await new Promise<void>((resolve, reject) => {
		server.listen(0, "127.0.0.1", () => resolve());
		server.once("error", reject);
	});

	const address = server.address();
	if (!address || typeof address === "string") {
		server.close();
		throw new Error("Unable to determine test server address");
	}

	try {
		await run(`http://127.0.0.1:${address.port}`);
	} finally {
		await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
	}
}

describe("protocol providers", () => {
	test("agent can call an MCP tool through a declared MCP server", async () => {
		const { dir, dbPath } = createTempDb("agentctl-mcp");
		const playbookFile = join(dir, "mcp.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: mcp-call\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`mcpServers:\n` +
				`  math:\n` +
				`    description: local math server\n` +
				`agents:\n` +
				`  local/caller:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: call the math tool\n` +
				`    tools:\n` +
				`      - tool: mcp:math/add\n` +
				`        with:\n` +
				`          a: 2\n` +
				`          b: 3\n` +
				`tasks:\n` +
				`  - id: call\n` +
				`    uses: agent:local/caller\n`,
			"utf8",
		);

		const mathServer: McpServerTransport = {
			async listTools() {
				return [{ name: "add", capability: "observe", risk: "low" }];
			},
			async callTool(name, args) {
				expect(name).toBe("add");
				return {
					structuredContent: {
						sum: Number(args.a) + Number(args.b),
					},
				};
			},
		};

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
			mcpServers: { math: mathServer },
		});
		const result = await runtime.start();
		expect(result.run.status).toBe("succeeded");
		expect(result.run.snapshot.tasks.call.output?.finalText).toContain(`"sum":5`);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("agent can delegate to another agentctl runtime through A2A", async () => {
		const { dir, dbPath } = createTempDb("agentctl-a2a");
		const delegatorPlaybookFile = join(dir, "delegator.playbook.yaml");
		const helperPlaybookFile = join(dir, "helper.playbook.yaml");
		writeFileSync(
			helperPlaybookFile,
			`playbook: helper\n` +
				`inputs:\n` +
				`  text: default\n` +
				`agents:\n` +
				`  local/helper:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: delegate result\n` +
				`    tools:\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            delegated_text: "{{ inputs.text }}"\n` +
				`tasks:\n` +
				`  - id: helper_task\n` +
				`    uses: agent:local/helper\n`,
			"utf8",
		);
		writeFileSync(
			delegatorPlaybookFile,
			`playbook: delegator\n` +
				`defaults:\n` +
				`  agentProfile: workspace_exec\n` +
				`a2aAgents:\n` +
				`  helper:\n` +
				`    description: local helper runtime\n` +
				`agents:\n` +
				`  local/delegator:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: delegate work to helper\n` +
				`    tools:\n` +
				`      - tool: a2a:helper\n` +
				`        with:\n` +
				`          text: from-a2a\n` +
				`tasks:\n` +
				`  - id: delegate\n` +
				`    uses: agent:local/delegator\n`,
			"utf8",
		);

		const helperTransport: A2AAgentTransport = {
			async sendTask(request) {
				const helperStore = new CheckpointStore(join(dir, `${request.taskId}.db`));
				try {
					const helperRuntime = new PlaybookRuntime(
						compilePlaybook(loadPlaybookWithPacks(helperPlaybookFile)),
						helperStore,
					);
					const result = await helperRuntime.start({ text: request.input.text });
					return {
						taskId: request.taskId ?? "missing-task-id",
						contextId: request.contextId ?? "missing-context-id",
						state: "COMPLETED",
						output: {
							finalText: String(result.run.snapshot.vars.delegated_text ?? ""),
						},
					};
				} finally {
					helperStore.close();
				}
			},
		};

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(delegatorPlaybookFile)), store, {
			a2aAgents: { helper: helperTransport },
		});
		const result = await runtime.start();
		expect(result.run.status).toBe("succeeded");
		expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("from-a2a");
		expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("taskId");
		expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("contextId");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("remote MCP transport initializes the session and reuses MCP session headers", async () => {
		let initializeCalls = 0;
		let initializedNotificationCalls = 0;
		let listCalls = 0;
		let callCalls = 0;
		let sessionId = "";

		await withHttpServer(async (request, response) => {
			expect(request.url).toBe("/mcp");
			expect(request.headers.accept).toContain("application/json");
			expect(request.headers.accept).toContain("text/event-stream");
			const payload = await readJsonRequest(request);
			if (payload.method === "initialize") {
				initializeCalls += 1;
				sessionId = "mcp-test-session";
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.setHeader("MCP-Session-Id", sessionId);
				response.end(
					JSON.stringify({
						jsonrpc: "2.0",
						id: payload.id,
						result: {
							protocolVersion: "2025-11-25",
							capabilities: {},
							serverInfo: { name: "math", version: "1.0.0" },
						},
					}),
				);
				return;
			}

			expect(request.headers["mcp-session-id"]).toBe(sessionId);
			expect(request.headers["mcp-protocol-version"]).toBe("2025-11-25");

			if (payload.method === "notifications/initialized") {
				initializedNotificationCalls += 1;
				response.statusCode = 202;
				response.end();
				return;
			}

			if (payload.method === "tools/list") {
				listCalls += 1;
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						jsonrpc: "2.0",
						id: payload.id,
						result: {
							tools: [
								{
									name: "add",
									description: "add two numbers",
									annotations: { readOnlyHint: true },
								},
							],
						},
					}),
				);
				return;
			}

			if (payload.method === "tools/call") {
				callCalls += 1;
				expect(payload.params?.name).toBe("add");
				expect(payload.params?.arguments).toEqual({ a: 4, b: 5 });
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						jsonrpc: "2.0",
						id: payload.id,
						result: {
							structuredContent: {
								sum: 9,
							},
						},
					}),
				);
				return;
			}

			response.statusCode = 404;
			response.end();
		}, async (baseUrl) => {
			const { dir, dbPath } = createTempDb("agentctl-remote-mcp");
			const playbookFile = join(dir, "remote-mcp.playbook.yaml");
			writeFileSync(
				playbookFile,
				`playbook: remote-mcp\n` +
					`defaults:\n` +
					`  agentProfile: workspace_exec\n` +
					`mcpServers:\n` +
					`  math:\n` +
					`    url: ${baseUrl}/mcp\n` +
					`agents:\n` +
					`  remote/caller:\n` +
					`    kind: builtin.heuristic\n` +
					`    instructions: use the remote tool\n` +
					`    tools:\n` +
					`      - tool: mcp:math/add\n` +
					`        with:\n` +
					`          a: 4\n` +
					`          b: 5\n` +
					`tasks:\n` +
					`  - id: call\n` +
					`    uses: agent:remote/caller\n`,
				"utf8",
			);

			const store = new CheckpointStore(dbPath);
			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.tasks.call.output?.finalText).toContain(`"sum":9`);
				expect(initializeCalls).toBe(1);
				expect(initializedNotificationCalls).toBe(1);
				expect(listCalls).toBe(1);
				expect(callCalls).toBe(1);
			} finally {
				store.close();
				rmSync(dir, { recursive: true, force: true });
			}
		});
	});

	test("remote A2A transport discovers the agent card and polls until completion", async () => {
		let cardFetches = 0;
		let sendCalls = 0;
		let pollCalls = 0;

		await withHttpServer(async (request, response) => {
			if (request.method === "GET" && request.url === "/.well-known/agent-card.json") {
				cardFetches += 1;
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						name: "helper",
						url: "http://127.0.0.1:" + (request.socket.localPort ?? 0) + "/rpc",
					}),
				);
				return;
			}

			expect(cardFetches).toBeGreaterThan(0);
			expect(request.url).toBe("/rpc");
			const payload = await readJsonRequest(request);
			if (payload.method === "message/send") {
				sendCalls += 1;
				expect(payload.params?.message).toMatchObject({
					role: "user",
					metadata: {
						input: {
							text: "remote-a2a",
						},
					},
				});
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						jsonrpc: "2.0",
						id: payload.id,
						result: {
							task: {
								id: "remote-task",
								contextId: "remote-context",
								status: { state: "TASK_STATE_WORKING" },
							},
						},
					}),
				);
				return;
			}

			if (payload.method === "tasks/get") {
				pollCalls += 1;
				expect(payload.params).toEqual({ id: "remote-task" });
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						jsonrpc: "2.0",
						id: payload.id,
						result: {
							task: {
								id: "remote-task",
								contextId: "remote-context",
								status: { state: "TASK_STATE_COMPLETED" },
								artifacts: [
									{
										parts: [{ text: "delegated remote-a2a" }],
									},
								],
							},
						},
					}),
				);
				return;
			}

			response.statusCode = 404;
			response.end();
		}, async (baseUrl) => {
			const { dir, dbPath } = createTempDb("agentctl-remote-a2a");
			const playbookFile = join(dir, "remote-a2a.playbook.yaml");
			writeFileSync(
				playbookFile,
				`playbook: remote-a2a\n` +
					`defaults:\n` +
					`  agentProfile: workspace_exec\n` +
					`a2aAgents:\n` +
					`  helper:\n` +
					`    cardUrl: ${baseUrl}/.well-known/agent-card.json\n` +
					`agents:\n` +
					`  remote/delegator:\n` +
					`    kind: builtin.heuristic\n` +
					`    instructions: delegate to remote helper\n` +
					`    tools:\n` +
					`      - tool: a2a:helper\n` +
					`        with:\n` +
					`          text: remote-a2a\n` +
					`tasks:\n` +
					`  - id: delegate\n` +
					`    uses: agent:remote/delegator\n`,
				"utf8",
			);

			const store = new CheckpointStore(dbPath);
			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("delegated remote-a2a");
				expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("taskId");
				expect(result.run.snapshot.tasks.delegate.output?.finalText).toContain("contextId");
				expect(cardFetches).toBe(1);
				expect(sendCalls).toBe(1);
				expect(pollCalls).toBeGreaterThanOrEqual(1);
			} finally {
				store.close();
				rmSync(dir, { recursive: true, force: true });
			}
		});
	});
});
