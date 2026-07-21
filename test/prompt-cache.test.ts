import { execFile } from "node:child_process";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { describe, expect, test } from "vitest";
import { AuthStorage } from "../src/auth-storage.js";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

const execFileAsync = promisify(execFile);

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

async function runCli(args: string[], env: NodeJS.ProcessEnv = {}): Promise<{
	stdout: string;
	stderr: string;
	exitCode: number;
}> {
	try {
		const result = await execFileAsync("node", ["./node_modules/tsx/dist/cli.mjs", "src/cli.ts", ...args], {
			cwd: process.cwd(),
			env: { ...process.env, ...env },
		});
		return { stdout: result.stdout, stderr: result.stderr, exitCode: 0 };
	} catch (error) {
		const failed = error as Error & { stdout?: string; stderr?: string; code?: number };
		return {
			stdout: failed.stdout ?? "",
			stderr: failed.stderr ?? "",
			exitCode: failed.code ?? 1,
		};
	}
}

describe("prompt cache", () => {
	test("prompt-cache --help prints command-specific usage", async () => {
		const { stdout, stderr, exitCode } = await runCli(["prompt-cache", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl prompt-cache stats [flags]");
		expect(stdout).toContain("agentctl prompt-cache explain <playbook.yaml> [flags]");
		expect(stdout).toContain("observability for provider-native caching");
	});

	test("compile rejects agent-level prompt cache on unsupported agent kinds", () => {
		const dir = createTempDir("agentctl-prompt-cache-invalid");
		const playbookFile = join(dir, "invalid.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: invalid-prompt-cache\n` +
				`agents:\n` +
				`  local/reviewer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: test\n` +
				`    promptCache:\n` +
				`      enabled: true\n` +
				`tasks:\n` +
				`  - id: review\n` +
				`    uses: agent:local/reviewer\n`,
			"utf8",
		);

		try {
			expect(() => compilePlaybook(loadPlaybookWithPacks(playbookFile))).toThrow(
				'Agent "local/reviewer" enables promptCache but only openai.responses with provider "openai" is supported',
			);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("custom OpenAI-compatible base URLs disable prompt cache unless forced", async () => {
		const dir = createTempDir("agentctl-prompt-cache-disabled");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "prompt-cache.playbook.yaml");
		let requestBody: Record<string, unknown> | undefined;

		await withServer(async (request, response) => {
			requestBody = (await readJson(request)) as Record<string, unknown>;
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_disabled_1",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1400,
						input_tokens_details: { cached_tokens: 900 },
						output_tokens: 80,
					},
					output_text: "",
					output: [
						{
							id: "msg_1",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "cache disabled", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-disabled\n` +
					`promptCache:\n` +
					`  enabled: true\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: cache disabled\n` +
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
				expect(requestBody).toBeDefined();
				expect("prompt_cache_key" in (requestBody ?? {})).toBe(false);
				expect(store.getPromptCacheStats().totalResponses).toBe(0);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("forced custom OpenAI-compatible base URLs send a stable key, downgrade 24h retention, and record stats", async () => {
		const dir = createTempDir("agentctl-prompt-cache-runtime");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "prompt-cache.playbook.yaml");
		const seenKeys: string[] = [];

		await withServer(async (request, response) => {
			expect(request.method).toBe("POST");
			expect(request.url).toBe("/responses");
			const body = (await readJson(request)) as Record<string, unknown>;
			expect(typeof body.prompt_cache_key).toBe("string");
			expect(body.prompt_cache_retention).toBe("in-memory");
			seenKeys.push(String(body.prompt_cache_key));
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_cache_1",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1400,
						input_tokens_details: { cached_tokens: 900 },
						output_tokens: 80,
					},
					output_text: "",
					output: [
						{
							id: "msg_1",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "cached review", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-runtime\n` +
				`promptCache:\n` +
					`  enabled: true\n` +
					`  force: true\n` +
					`  retention: 24h\n` +
					`  keyScope: agent\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: cached review\n` +
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
				expect(result.run.snapshot.tasks.review.output?.finalText).toBe("cached review");
				expect(seenKeys).toHaveLength(1);
				expect(seenKeys[0]).toContain("agentctl:openai:agent:prompt-cache-runtime:real/reviewer:");
				const stats = store.getPromptCacheStats();
				expect(stats.totalResponses).toBe(1);
				expect(stats.hitResponses).toBe(1);
				expect(stats.totalCachedTokens).toBe(900);
				expect(stats.totalInputTokens).toBe(1400);
				expect(stats.totalUncachedInputTokens).toBe(500);
				expect(stats.providers).toEqual([
					{
						provider: "openai",
						responses: 1,
						hitResponses: 1,
						cachedTokens: 900,
						inputTokens: 1400,
						uncachedInputTokens: 500,
						outputTokens: 80,
					},
				]);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("grouped multi-agent prompt cache uses the same key across agents", async () => {
		const dir = createTempDir("agentctl-prompt-cache-group");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "group.playbook.yaml");
		const seenKeys: string[] = [];

		await withServer(async (request, response) => {
			const body = (await readJson(request)) as Record<string, unknown>;
			seenKeys.push(String(body.prompt_cache_key));
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: `resp_${seenKeys.length}`,
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1200,
						input_tokens_details: { cached_tokens: seenKeys.length === 2 ? 1000 : 0 },
						output_tokens: 60,
					},
					output_text: "",
					output: [
						{
							id: `msg_${seenKeys.length}`,
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: `response ${seenKeys.length}`, annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-group\n` +
					`agents:\n` +
					`  real/first:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: shared cache review\n` +
					`    promptCache:\n` +
						`      enabled: true\n` +
						`      force: true\n` +
						`      shareMode: group\n` +
						`      group: review-shared\n` +
					`  real/second:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: shared cache review\n` +
					`    promptCache:\n` +
						`      enabled: true\n` +
						`      force: true\n` +
						`      shareMode: group\n` +
						`      group: review-shared\n` +
					`tasks:\n` +
					`  - id: one\n` +
					`    uses: agent:real/first\n` +
					`  - id: two\n` +
					`    needs: [one]\n` +
					`    uses: agent:real/second\n`,
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
				expect(seenKeys).toHaveLength(2);
				expect(seenKeys[0]).toBe(seenKeys[1]);
				expect(seenKeys[0]).toContain("group:review-shared");
				const stats = store.getPromptCacheStats();
				expect(stats.totalResponses).toBe(2);
				expect(stats.hitResponses).toBe(1);
				expect(stats.totalCachedTokens).toBe(1000);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("custom prompt cache key templates can use explicit runtime namespaces", async () => {
		const dir = createTempDir("agentctl-prompt-cache-custom");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "custom.playbook.yaml");
		const seenKeys: string[] = [];

		await withServer(async (request, response) => {
			const body = (await readJson(request)) as Record<string, unknown>;
			seenKeys.push(String(body.prompt_cache_key));
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_custom_1",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1024,
						input_tokens_details: { cached_tokens: 0 },
						output_tokens: 48,
					},
					output_text: "",
					output: [
						{
							id: "msg_custom",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "custom key", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-custom\n` +
					`inputs:\n` +
					`  service: checkout\n` +
					`promptCache:\n` +
						`  enabled: true\n` +
						`  force: true\n` +
						`  keyScope: custom\n` +
					`  keyTemplate: "{{ inputs.service }}:{{ run.id }}"\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: custom key\n` +
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
				expect(seenKeys).toHaveLength(1);
				expect(seenKeys[0]).toContain("custom:checkout:");
				expect(seenKeys[0]).toContain(result.run.id);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("resume preserves the same prompt cache key across a multi-turn provider run", async () => {
		const dir = createTempDir("agentctl-prompt-cache-resume");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "resume.playbook.yaml");
		const seenKeys: string[] = [];
		let interruptedRunId = "";
		let requestCount = 0;

		await withServer(async (request, response) => {
			const body = (await readJson(request)) as Record<string, unknown>;
			requestCount += 1;
			seenKeys.push(String(body.prompt_cache_key));

			if (requestCount === 1) {
				response.statusCode = 200;
				response.setHeader("content-type", "application/json");
				response.end(
					JSON.stringify({
						id: "resp_resume_1",
						object: "response",
						created_at: 0,
						status: "completed",
						model: "gpt-5-mini",
						usage: {
							input_tokens: 1100,
							input_tokens_details: { cached_tokens: 0 },
							output_tokens: 40,
						},
						output_text: "",
						output: [
							{
								id: "fc_1",
								type: "function_call",
								call_id: "call_1",
								name: "assign",
								arguments: JSON.stringify({ values: { status: "resumed" } }),
								status: "completed",
							},
						],
					}),
				);
				return;
			}

			expect(body.previous_response_id).toBe("resp_resume_1");
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_resume_2",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1200,
						input_tokens_details: { cached_tokens: 700 },
						output_tokens: 50,
					},
					output_text: "",
					output: [
						{
							id: "msg_resume",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "resume complete", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-resume\n` +
					`promptCache:\n` +
						`  enabled: true\n` +
						`  force: true\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: resume review\n` +
					`    tools:\n` +
					`      - tool: builtin.assign\n` +
					`        name: assign\n` +
					`        with:\n` +
					`          values:\n` +
					`            status: pending\n` +
					`tasks:\n` +
					`  - id: review\n` +
					`    uses: agent:real/reviewer\n`,
				"utf8",
			);

			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(dbPath);
			try {
				const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
				const interruptingRuntime = new PlaybookRuntime(plan, store, {
					authStorage,
					hooks: {
						afterCheckpoint(checkpoint) {
							if (checkpoint.taskId === "review" && checkpoint.snapshot.agents.review) {
								interruptedRunId = checkpoint.runId;
								throw new Error("interrupt after first tool turn");
							}
						},
					},
				});
				await expect(interruptingRuntime.start()).rejects.toThrow("interrupt after first tool turn");
				expect(interruptedRunId).not.toBe("");

				const resumed = await new PlaybookRuntime(plan, store, { authStorage }).resume(interruptedRunId);
				expect(resumed.run.status).toBe("succeeded");
				expect(resumed.run.snapshot.tasks.review.output?.finalText).toBe("resume complete");
				expect(seenKeys).toHaveLength(2);
				expect(seenKeys[0]).toBe(seenKeys[1]);
			} finally {
				store.close();
			}
		});

		rmSync(dir, { recursive: true, force: true });
	});

	test("prompt-cache stats command reports aggregated cache usage from the runtime db", async () => {
		const dir = createTempDir("agentctl-prompt-cache-cli");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "stats.playbook.yaml");

		await withServer(async (_request, response) => {
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: "resp_cli_1",
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1300,
						input_tokens_details: { cached_tokens: 650 },
						output_tokens: 55,
					},
					output_text: "",
					output: [
						{
							id: "msg_cli",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "cli cache stats", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-cli\n` +
					`promptCache:\n` +
						`  enabled: true\n` +
						`  force: true\n` +
					`agents:\n` +
					`  real/reviewer:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: cli cache stats\n` +
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
				await runtime.start();
			} finally {
				store.close();
			}
		});

		try {
			const { stdout, stderr, exitCode } = await runCli(["prompt-cache", "stats", "--db", dbPath, "--output", "json", "--verbose"]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const payload = JSON.parse(stdout.trim()) as Record<string, unknown>;
			expect(payload.type).toBe("prompt_cache_stats");
			expect(payload.totalResponses).toBe(1);
			expect(payload.hitResponses).toBe(1);
			expect(payload.totalCachedTokens).toBe(650);
			expect(payload.totalUncachedInputTokens).toBe(650);
			expect(payload.agents).toEqual([
				expect.objectContaining({
					agentRef: "real/reviewer",
					responses: 1,
					hitResponses: 1,
				}),
			]);
			expect(payload.runs).toEqual([
				expect.objectContaining({
					responses: 1,
					hitResponses: 1,
				}),
			]);
			expect(Array.isArray(payload.responses)).toBe(true);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("prompt-cache stats can filter by agent ref", async () => {
		const dir = createTempDir("agentctl-prompt-cache-agent-filter");
		const dbPath = join(dir, "runtime.db");
		const playbookFile = join(dir, "filter.playbook.yaml");

		await withServer(async (_request, response) => {
			response.statusCode = 200;
			response.setHeader("content-type", "application/json");
			response.end(
				JSON.stringify({
					id: `resp_${Date.now()}`,
					object: "response",
					created_at: 0,
					status: "completed",
					model: "gpt-5-mini",
					usage: {
						input_tokens: 1000,
						input_tokens_details: { cached_tokens: 400 },
						output_tokens: 50,
					},
					output_text: "",
					output: [
						{
							id: "msg_filter",
							type: "message",
							role: "assistant",
							status: "completed",
							content: [{ type: "output_text", text: "filter", annotations: [] }],
						},
					],
				}),
			);
		}, async (baseUrl) => {
			writeFileSync(
				playbookFile,
				`playbook: prompt-cache-filter\n` +
					`promptCache:\n` +
					`  enabled: true\n` +
					`  force: true\n` +
					`agents:\n` +
					`  real/first:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: first\n` +
					`  real/second:\n` +
					`    kind: openai.responses\n` +
					`    provider: openai\n` +
					`    model: gpt-5-mini\n` +
					`    baseUrl: ${baseUrl}\n` +
					`    instructions: second\n` +
					`tasks:\n` +
					`  - id: one\n` +
					`    uses: agent:real/first\n` +
					`  - id: two\n` +
					`    needs: [one]\n` +
					`    uses: agent:real/second\n`,
				"utf8",
			);
			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(dbPath);
			try {
				const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
					authStorage,
				});
				await runtime.start();
			} finally {
				store.close();
			}
		});

		try {
			const { stdout, stderr, exitCode } = await runCli([
				"prompt-cache",
				"stats",
				"--db",
				dbPath,
				"--agent-ref",
				"real/second",
				"--output",
				"json",
			]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const payload = JSON.parse(stdout.trim()) as Record<string, unknown>;
			expect(payload.totalResponses).toBe(1);
			expect(payload.agents).toEqual([
				expect.objectContaining({
					agentRef: "real/second",
					responses: 1,
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("prompt-cache explain reports eligibility and effective settings", async () => {
		const dir = createTempDir("agentctl-prompt-cache-explain");
		const playbookFile = join(dir, "explain.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: prompt-cache-explain\n` +
				`promptCache:\n` +
				`  enabled: true\n` +
				`  retention: in_memory\n` +
				`agents:\n` +
				`  real/reviewer:\n` +
				`    kind: openai.responses\n` +
				`    provider: openai\n` +
				`    model: gpt-5-mini\n` +
				`    baseUrl: http://127.0.0.1:9999\n` +
				`    instructions: explain me\n` +
				`  local/fallback:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: local fallback\n` +
				`tasks:\n` +
				`  - id: review\n` +
				`    uses: agent:real/reviewer\n` +
				`  - id: local\n` +
				`    uses: agent:local/fallback\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["prompt-cache", "explain", playbookFile, "--output", "json"]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const payload = JSON.parse(stdout.trim()) as {
				type: string;
				agents: Array<Record<string, unknown>>;
			};
			expect(payload.type).toBe("prompt_cache_explain");
			expect(payload.agents).toEqual([
				expect.objectContaining({
					taskId: "review",
					agentRef: "real/reviewer",
					requested: true,
					enabled: false,
					eligible: false,
					force: false,
					reason: "Custom OpenAI-compatible base URLs disable prompt cache by default; set promptCache.force: true to opt in",
				}),
				expect.objectContaining({
					taskId: "local",
					agentRef: "local/fallback",
					requested: true,
					enabled: false,
					eligible: false,
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
