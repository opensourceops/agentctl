import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { AuthStorage } from "../src/auth-storage.js";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { getEnvApiKey, getEnvProviderConfig } from "../src/env-api-keys.js";
import { ModelRegistry } from "../src/model-registry.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

function createTempDir(name: string): string {
	return mkdtempSync(join(tmpdir(), `${name}-`));
}

async function readJson(request: IncomingMessage): Promise<unknown> {
	const chunks: Buffer[] = [];
	for await (const chunk of request) {
		chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
	}
	return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
}

async function withServer(
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
		throw new Error("Unable to determine server address");
	}

	try {
		await run(`http://127.0.0.1:${address.port}`);
	} finally {
		await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
	}
}

describe("provider auth and model-backed agents", () => {
	test("runtime override beats stored and env auth", () => {
		const previousEnv = process.env.OPENAI_API_KEY;
		process.env.OPENAI_API_KEY = "env-key";
		const storage = AuthStorage.inMemory({ openai: "stored-key" });
		storage.setRuntimeApiKey("openai", "runtime-key");
		const registry = new ModelRegistry(storage);

		try {
			expect(
				registry.resolveAgent({
					kind: "openai.responses",
					instructions: "test",
					model: "gpt-5-mini",
				}).apiKey,
			).toBe("runtime-key");
		} finally {
			if (previousEnv === undefined) {
				delete process.env.OPENAI_API_KEY;
			} else {
				process.env.OPENAI_API_KEY = previousEnv;
			}
		}
	});

	test("empty provider env vars are treated as missing auth", () => {
		const previousEnv = {
			OPENAI_API_KEY: process.env.OPENAI_API_KEY,
			OPENAI_ORG_ID: process.env.OPENAI_ORG_ID,
			OPENAI_PROJECT_ID: process.env.OPENAI_PROJECT_ID,
		};
		process.env.OPENAI_API_KEY = "   ";
		process.env.OPENAI_ORG_ID = "org_test";
		process.env.OPENAI_PROJECT_ID = "proj_test";

		try {
			expect(getEnvApiKey("openai")).toBeUndefined();
			expect(getEnvProviderConfig("openai")).toEqual({
				organization: "org_test",
				project: "proj_test",
			});
			expect(AuthStorage.inMemory().inspect("openai")).toEqual({
				provider: "openai",
				configured: false,
				source: "missing",
			});
		} finally {
			for (const [key, value] of Object.entries(previousEnv)) {
				if (value === undefined) delete process.env[key];
				else process.env[key] = value;
			}
		}
	});

	test("model registry resolves stored OpenAI organization and project metadata", () => {
		const registry = new ModelRegistry(
			AuthStorage.inMemory({
				openai: {
					type: "api_key",
					key: "stored-key",
					organization: "org_stored",
					project: "proj_stored",
				},
			}),
		);

		expect(
			registry.resolveAgent({
				kind: "openai.responses",
				instructions: "test",
				model: "gpt-5-mini",
			}),
		).toMatchObject({
			provider: "openai",
			apiKey: "stored-key",
			organization: "org_stored",
			project: "proj_stored",
		});
	});

	test("openai.responses agent can call a tool and finish through the Responses API", async () => {
		const dir = createTempDir("agentctl-openai-agent");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "provider.playbook.yaml");
		let callCount = 0;

		await withServer(async (request, response) => {
			expect(request.method).toBe("POST");
			expect(request.url).toBe("/responses");
			expect(request.headers.authorization).toBe("Bearer runtime-key");
			const body = (await readJson(request)) as Record<string, unknown>;
			callCount += 1;

			if (callCount === 1) {
				expect(body.model).toBe("gpt-5-mini");
				expect(Array.isArray(body.tools)).toBe(true);
				expect("temperature" in body).toBe(false);
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						id: "resp_1",
						object: "response",
						created_at: 0,
						status: "completed",
						model: "gpt-5-mini",
						output_text: "",
						output: [
							{
								id: "fc_1",
								type: "function_call",
								call_id: "call_1",
								name: "assign",
								arguments: JSON.stringify({
									values: {
										summary: "investigated",
									},
								}),
								status: "completed",
							},
						],
					}),
				);
				return;
			}

			expect(body.previous_response_id).toBe("resp_1");
			expect(Array.isArray(body.input)).toBe(true);
			const toolOutput = (body.input as Array<Record<string, unknown>>)[0];
			expect(toolOutput).toMatchObject({
				type: "function_call_output",
				call_id: "call_1",
			});
			const parsedToolOutput = JSON.parse(String(toolOutput?.output)) as Record<string, unknown>;
			expect(parsedToolOutput.values).toEqual({ summary: "investigated" });
			expect(parsedToolOutput.call_id).toBe("call_1");
			expect(typeof parsedToolOutput.assignedAt).toBe("string");
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_2",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					output_text: "",
					output: [
						{
							id: "msg_1",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [
								{
									type: "output_text",
									text: "final report: investigated",
									annotations: [],
								},
							],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: provider-run\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: investigate and write a final report\n` +
					`    tools:\n` +
					`      - tool: builtin.assign\n` +
					`        name: assign\n` +
					`        with:\n` +
					`          values:\n` +
					`            summary: pending\n` +
					`tasks:\n` +
					`  - id: review\n` +
					`    uses: agent:real/reviewer\n`,
				"utf8",
			);

			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
					authStorage,
				});
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.vars.summary).toBe("investigated");
				expect(result.run.snapshot.tasks.review.output?.finalText).toBe("final report: investigated");
				expect(callCount).toBe(2);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("openai.responses batches outputs for multiple function calls from one response", async () => {
		const dir = createTempDir("agentctl-openai-multi-tool");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "provider.playbook.yaml");
		let callCount = 0;

		await withServer(async (request, response) => {
			expect(request.method).toBe("POST");
			expect(request.url).toBe("/responses");
			const body = (await readJson(request)) as Record<string, unknown>;
			callCount += 1;

			if (callCount === 1) {
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						id: "resp_multi_1",
						object: "response",
						created_at: 0,
						status: "completed",
						model: "gpt-5-mini",
						output_text: "",
						output: [
							{
								id: "fc_a",
								type: "function_call",
								call_id: "call_a",
								name: "assign_one",
								arguments: JSON.stringify({
									values: { first: "alpha" },
								}),
								status: "completed",
							},
							{
								id: "fc_b",
								type: "function_call",
								call_id: "call_b",
								name: "assign_two",
								arguments: JSON.stringify({
									values: { second: "beta" },
								}),
								status: "completed",
							},
						],
					}),
				);
				return;
			}

			expect(body.previous_response_id).toBe("resp_multi_1");
			expect(Array.isArray(body.input)).toBe(true);
			const outputs = body.input as Array<Record<string, unknown>>;
			expect(outputs).toHaveLength(2);
			expect(outputs.map((entry) => entry.call_id)).toEqual(["call_a", "call_b"]);
			const parsedA = JSON.parse(String(outputs[0]!.output)) as Record<string, unknown>;
			const parsedB = JSON.parse(String(outputs[1]!.output)) as Record<string, unknown>;
			expect(parsedA.values).toEqual({ first: "alpha" });
			expect(parsedB.values).toEqual({ second: "beta" });

			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_multi_2",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					output_text: "",
					output: [
						{
							id: "msg_multi",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [
								{
									type: "output_text",
									text: "multi tool complete",
									annotations: [],
								},
							],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: provider-run\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: use both tools then finish\n` +
					`    tools:\n` +
					`      - tool: builtin.assign\n` +
					`        name: assign_one\n` +
					`      - tool: builtin.assign\n` +
					`        name: assign_two\n` +
					`tasks:\n` +
					`  - id: review\n` +
					`    uses: agent:real/reviewer\n`,
				"utf8",
			);

			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
					authStorage,
				});
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.vars.first).toBe("alpha");
				expect(result.run.snapshot.vars.second).toBe("beta");
				expect(result.run.snapshot.tasks.review.output?.finalText).toBe("multi tool complete");
				expect(callCount).toBe(2);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("openai.responses forwards organization and project headers", async () => {
		const dir = createTempDir("agentctl-openai-org-project");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "provider.playbook.yaml");

		await withServer(async (request, response) => {
			expect(request.method).toBe("POST");
			expect(request.url).toBe("/responses");
			expect(request.headers.authorization).toBe("Bearer stored-key");
			expect(request.headers["openai-organization"]).toBe("org_stored");
			expect(request.headers["openai-project"]).toBe("proj_stored");
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_org_project",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					output_text: "",
					output: [
						{
							id: "msg_org_project",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [
								{
									type: "output_text",
									text: "done",
									annotations: [],
								},
							],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: provider-run\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: finish immediately\n` +
					`tasks:\n` +
					`  - id: review\n` +
					`    uses: agent:real/reviewer\n`,
				"utf8",
			);

			const authStorage = AuthStorage.inMemory({
				openai: {
					type: "api_key",
					key: "stored-key",
					organization: "org_stored",
					project: "proj_stored",
				},
			});
			const store = new CheckpointStore(dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
					authStorage,
				});
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.tasks.review.output?.finalText).toBe("done");
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("azure openai responses uses api-version and api-key auth", async () => {
		const dir = createTempDir("agentctl-azure-openai");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "provider.playbook.yaml");

		await withServer(async (request, response) => {
			expect(request.method).toBe("POST");
			expect(request.url).toBe("/openai/responses?api-version=2024-10-01-preview");
			expect(request.headers["api-key"]).toBe("azure-key");
			expect(request.headers.authorization).toBeUndefined();
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_azure",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-4.1-mini",
					output_text: "",
					output: [
						{
							id: "msg_azure",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [
								{
									type: "output_text",
									text: "azure-finished",
									annotations: [],
								},
							],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: azure-provider-run\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: azure-openai-responses\n` +
					`    model: gpt-4.1-mini\n` +
					`    endpoint: ${baseUrl}\n` +
					`    apiVersion: 2024-10-01-preview\n` +
					`    instructions: finish immediately\n` +
					`tasks:\n` +
					`  - id: review\n` +
					`    uses: agent:real/reviewer\n`,
				"utf8",
			);

			const authStorage = AuthStorage.inMemory({ "azure-openai-responses": "azure-key" });
			const store = new CheckpointStore(dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
					authStorage,
				});
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(result.run.snapshot.tasks.review.output?.finalText).toBe("azure-finished");
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});
});
