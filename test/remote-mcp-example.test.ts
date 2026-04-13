import { spawn } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

const serverProcesses = new Set<ReturnType<typeof spawn>>();

async function startExampleServer(scriptPath: string, cwd: string): Promise<ReturnType<typeof spawn>> {
	const server = spawn("node", [scriptPath], {
		cwd,
		stdio: ["ignore", "pipe", "pipe"],
	});
	serverProcesses.add(server);

	await new Promise<void>((resolve, reject) => {
		let settled = false;
		const onData = (chunk: Buffer | string) => {
			const text = String(chunk);
			if (text.includes("agentctl remote MCP server listening")) {
				settled = true;
				server.stdout?.off("data", onData);
				server.stderr?.off("data", onErrorData);
				resolve();
			}
		};
		const onErrorData = (chunk: Buffer | string) => {
			if (!settled) {
				reject(new Error(`remote MCP server failed to start: ${String(chunk)}`));
			}
		};
		server.once("exit", (code) => {
			if (!settled) {
				reject(new Error(`remote MCP server exited before readiness with code ${code ?? -1}`));
			}
		});
		server.stdout?.on("data", onData);
		server.stderr?.on("data", onErrorData);
	});

	return server;
}

async function stopServer(server: ReturnType<typeof spawn>): Promise<void> {
	if (server.exitCode !== null) {
		serverProcesses.delete(server);
		return;
	}
	await new Promise<void>((resolve) => {
		server.once("exit", () => resolve());
		server.kill("SIGTERM");
	});
	serverProcesses.delete(server);
}

afterEach(async () => {
	for (const server of [...serverProcesses]) {
		await stopServer(server);
	}
});

describe("remote MCP example", () => {
	test("real example runs against the standalone MCP server and writes the verified report", async () => {
		const tempRoot = mkdtempSync(join(tmpdir(), "agentctl-remote-mcp-example-"));
		const exampleDir = join(tempRoot, "remote-mcp-autonomy");
		cpSync(join(process.cwd(), "examples/remote-mcp-autonomy"), exampleDir, { recursive: true });

		const server = await startExampleServer(join(exampleDir, "mock-mcp-server.mjs"), exampleDir);
		const playbookFile = join(exampleDir, "mission.playbook.yaml");
		const dbPath = join(tempRoot, "runtime.db");
		const artifactPath = join(exampleDir, "artifacts", "remote-mcp-report.md");
		const store = new CheckpointStore(dbPath);

		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const result = await runtime.start();

			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.verify_report.output?.stdout).toBe("verified");

			const report = readFileSync(artifactPath, "utf8");
			expect(report).toContain("# Remote MCP Ops Report");
			expect(report).toContain("rollback validation drill");
			expect(report).toContain("incident communication owner");
			expect(report).toContain("Remote MCP audit inspected ./fixtures/service.");
		} finally {
			store.close();
			await stopServer(server);
			rmSync(tempRoot, { recursive: true, force: true });
		}
	});
});
