import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

function createTempDb(name: string): { dir: string; dbPath: string } {
	const dir = mkdtempSync(join(tmpdir(), `${name}-`));
	return { dir, dbPath: join(dir, "runtime.db") };
}

describe("example smoke tests", () => {
	test("dataflow example succeeds and preserves structured outputs across task boundaries", async () => {
		const { dir, dbPath } = createTempDb("agentctl-example-dataflow");
		const store = new CheckpointStore(dbPath);

		try {
			const runtime = new PlaybookRuntime(
				compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/dataflow/mission.playbook.yaml"))),
				store,
			);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.consume_scalar.output?.values).toEqual({ copiedStatus: "ready" });
			expect(result.run.snapshot.tasks.consume_object.output?.values).toEqual({
				copiedPayload: {
					service: "checkout",
					findings: ["restore-drill-missing", "escalation-policy-missing"],
					metadata: {
						source: "fixture",
						severity: "high",
					},
				},
			});
			expect(result.run.snapshot.tasks.assert_object.status).toBe("succeeded");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("hello example succeeds and proves pack-defined agent/module wiring", async () => {
		const { dir, dbPath } = createTempDb("agentctl-example-hello");
		const store = new CheckpointStore(dbPath);

		try {
			const runtime = new PlaybookRuntime(
				compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/hello.playbook.yaml"))),
				store,
			);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.review.output?.finalText).toBe("status=ready\nframework=agentctl");
			expect(result.run.snapshot.tasks.verify.status).toBe("succeeded");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("memory-flow example succeeds and verifies retrieval promotion", async () => {
		const tempRoot = mkdtempSync(join(tmpdir(), "agentctl-example-memory-flow-"));
		const exampleDir = join(tempRoot, "memory-flow");
		cpSync(join(process.cwd(), "examples/memory-flow"), exampleDir, { recursive: true });
		const playbookFile = join(exampleDir, "mission.playbook.yaml");
		const dbPath = join(tempRoot, "runtime.db");
		const longTermDbPath = join(exampleDir, "state", "long-term.db");
		const store = new CheckpointStore(dbPath);

		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.read_long_term.output?.matchCount).toBe(1);
			expect(result.run.snapshot.memory.working.recalled).toBe("restore-drill-missing");
			expect(result.run.snapshot.tasks.assert_recalled.status).toBe("succeeded");
			expect(existsSync(longTermDbPath)).toBe(true);
		} finally {
			store.close();
			rmSync(tempRoot, { recursive: true, force: true });
		}
	});

	test("custom-pack-tools example succeeds and persists the verified report", async () => {
		const tempRoot = mkdtempSync(join(tmpdir(), "agentctl-example-custom-pack-"));
		const exampleDir = join(tempRoot, "custom-pack-tools");
		cpSync(join(process.cwd(), "examples/custom-pack-tools"), exampleDir, { recursive: true });
		const playbookFile = join(exampleDir, "mission.playbook.yaml");
		const dbPath = join(tempRoot, "runtime.db");
		const artifactPath = join(exampleDir, "artifacts", "custom-pack-report.md");
		const store = new CheckpointStore(dbPath);

		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.verify_report.output?.stdout).toBe("verified");
			const report = readFileSync(artifactPath, "utf8");
			expect(report).toContain("# Custom Pack Report");
			expect(report).toContain("Node.js version:");
			expect(report).toContain("rollback owner");
			expect(report).toContain("restore drill");
		} finally {
			store.close();
			rmSync(tempRoot, { recursive: true, force: true });
		}
	});
});
