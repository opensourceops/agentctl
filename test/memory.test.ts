import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

describe("memory", () => {
	test("long-term memory persists across independent runs in a separate store", async () => {
		const { dir, dbPath } = createTempDb("agentctl-long-term-memory");
		const longTermDbPath = join(dir, "long-term.db");
		const writerPlaybook = join(dir, "writer.playbook.yaml");
		const readerPlaybook = join(dir, "reader.playbook.yaml");

		writeFileSync(
			writerPlaybook,
			`playbook: writer\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: service-audit\n` +
				`tasks:\n` +
				`  - id: write_memory\n` +
				`    uses: module:builtin.long_term_memory.write\n` +
				`    with:\n` +
				`      key: incident-owner\n` +
				`      value: pager-duty\n` +
				`      tags:\n` +
				`        - readiness\n`,
			"utf8",
		);

		writeFileSync(
			readerPlaybook,
			`playbook: reader\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: service-audit\n` +
				`tasks:\n` +
				`  - id: search_memory\n` +
				`    uses: module:builtin.long_term_memory.search\n` +
				`    with:\n` +
				`      query: pager\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const writer = await new PlaybookRuntime(
				compilePlaybook(loadPlaybookWithPacks(writerPlaybook)),
				store,
			).start();
			expect(writer.run.status).toBe("succeeded");
			expect(readFileSync(longTermDbPath).length).toBeGreaterThan(0);

			const reader = await new PlaybookRuntime(
				compilePlaybook(loadPlaybookWithPacks(readerPlaybook)),
				store,
			).start();
			expect(reader.run.status).toBe("succeeded");
				expect(reader.run.snapshot.tasks.search_memory.output).toEqual({
					namespace: "service-audit",
					query: "pager",
					key: null,
					matchCount: 1,
					matches: [
						{
						namespace: "service-audit",
						key: "incident-owner",
						value: "pager-duty",
						tags: ["readiness"],
						createdAt: expect.any(String),
						updatedAt: expect.any(String),
					},
				],
			});
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("inspect profile denies long-term memory writes from agents", async () => {
		const { dir, dbPath } = createTempDb("agentctl-long-term-policy");
		const playbookFile = join(dir, "long-term-policy.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: long-term-policy\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: policy-test\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to persist a long-term fact\n` +
				`    tools:\n` +
				`      - tool: builtin/long-term-memory-write\n` +
				`        with:\n` +
				`          key: forbidden\n` +
				`          value: true\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const result = await new PlaybookRuntime(
				compilePlaybook(loadPlaybookWithPacks(playbookFile)),
				store,
			).start();
			expect(result.run.status).toBe("failed");
			expect(result.run.snapshot.tasks.writer.error).toContain('does not allow long_term_memory.write');
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
