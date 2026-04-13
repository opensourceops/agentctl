import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { AuthStorage } from "../src/auth-storage.js";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

async function readJson(request: IncomingMessage): Promise<unknown> {
	const chunks: Buffer[] = [];
	for await (const chunk of request) {
		chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
	}
	return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
}

async function withOpenAIServer(
	reportMarkdown: string,
	run: (baseUrl: string) => Promise<void>,
): Promise<void> {
	const server = createServer((request, response) => {
		Promise.resolve(handleOpenAIRequest(request, response, reportMarkdown)).catch((error: unknown) => {
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
		throw new Error("Unable to determine OpenAI test server address");
	}

	try {
		await run(`http://127.0.0.1:${address.port}`);
	} finally {
		await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
	}
}

async function handleOpenAIRequest(
	request: IncomingMessage,
	response: ServerResponse,
	reportMarkdown: string,
): Promise<void> {
	expect(request.method).toBe("POST");
	expect(request.url).toBe("/responses");
	expect(request.headers.authorization).toBe("Bearer runtime-key");
	await readJson(request);

	response.statusCode = 200;
	response.setHeader("content-type", "application/json");
	response.end(
		JSON.stringify({
			id: "resp_real_autonomy",
			object: "response",
			created_at: 0,
			status: "completed",
			model: "gpt-5-mini",
			output_text: "",
			output: [
				{
					id: "msg_real_autonomy",
					type: "message",
					role: "assistant",
					status: "completed",
					content: [
						{
							type: "output_text",
							text: reportMarkdown,
							annotations: [],
						},
					],
				},
			],
		}),
	);
}

function copyRealAutonomyExample(name: string): { dir: string; playbookPath: string; dbPath: string; artifactPath: string } {
	const dir = mkdtempSync(join(tmpdir(), `${name}-`));
	cpSync(join(process.cwd(), "examples/real-autonomy"), dir, { recursive: true });
	return {
		dir,
		playbookPath: join(dir, "mission.playbook.yaml"),
		dbPath: join(dir, "runtime.db"),
		artifactPath: join(dir, "artifacts", "ops-readiness-report.md"),
	};
}

function groundedReport(): string {
	return `# Ops Readiness Report

## Summary
- The service has documented operational notes, but it is missing a backup restore drill and a documented on-call escalation policy.

## Evidence
- README.md: "There is no documented on-call escalation policy."
- README.md: "Backups exist at the database layer, but there is no backup restore drill."
- docs/runbook.md: "No restore validation drill is scheduled."

## Risks
- Recovery procedures are not validated in practice, which increases outage and data-loss risk.
- Incident escalation is unclear, which increases coordination delays during incidents.

## Recommended Next Steps
- Create and schedule a backup restore drill with explicit verification steps.
- Publish a documented on-call escalation policy and link it from the service docs.
- Extend the runbook with recovery validation procedures.`;
}

function ungroundedReport(): string {
	return `# Ops Readiness Report

## Summary
- The service needs better operations documentation.

## Evidence
- No direct evidence captured.

## Risks
- Recovery and incident response could be weak.

## Recommended Next Steps
- Improve operations procedures.`;
}

describe("real autonomy example", () => {
	test("succeeds when the provider returns evidence-grounded markdown", async () => {
		const example = copyRealAutonomyExample("agentctl-real-autonomy-ok");

		await withOpenAIServer(groundedReport(), async (baseUrl) => {
			const definition = loadPlaybookWithPacks(example.playbookPath);
			const auditAgent = definition.agents?.["real/ops_auditor"];
			if (!auditAgent) {
				throw new Error("real/ops_auditor not found");
			}
			auditAgent.baseUrl = baseUrl;

			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(example.dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(definition), store, { authStorage });
				const result = await runtime.start();
				expect(result.run.status).toBe("succeeded");
				expect(readFileSync(example.artifactPath, "utf8")).toContain("docs/runbook.md");
			} finally {
				store.close();
			}
		});

		rmSync(example.dir, { recursive: true, force: true });
	});

	test("fails verification when the report omits required source evidence", async () => {
		const example = copyRealAutonomyExample("agentctl-real-autonomy-fail");

		await withOpenAIServer(ungroundedReport(), async (baseUrl) => {
			const definition = loadPlaybookWithPacks(example.playbookPath);
			const auditAgent = definition.agents?.["real/ops_auditor"];
			if (!auditAgent) {
				throw new Error("real/ops_auditor not found");
			}
			auditAgent.baseUrl = baseUrl;

			const authStorage = AuthStorage.inMemory();
			authStorage.setRuntimeApiKey("openai", "runtime-key");
			const store = new CheckpointStore(example.dbPath);

			try {
				const runtime = new PlaybookRuntime(compilePlaybook(definition), store, { authStorage });
				const result = await runtime.start();
				expect(result.run.status).toBe("failed");
				expect(result.run.snapshot.tasks.verify_report.error).toContain("Command failed");
			} finally {
				store.close();
			}
		});

		rmSync(example.dir, { recursive: true, force: true });
	});
});
