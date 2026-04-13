import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { mkdir } from "node:fs/promises";
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

describe("tool policy", () => {
	test("inspect profile can use builtin/read via builtin alias", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-read");
		const playbookFile = join(dir, "read.playbook.yaml");
		writeFileSync(join(dir, "note.txt"), "policy-check\n", "utf8");
		writeFileSync(
			playbookFile,
			`playbook: read-ok\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: read the target file\n` +
				`    tools:\n` +
				`      - tool: builtin/read\n` +
				`        with:\n` +
				`          path: ./note.txt\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
		const result = await runtime.start();
		expect(result.run.status).toBe("succeeded");
		expect(JSON.stringify(result.run.snapshot.tasks.reader.output)).toContain("policy-check");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("inspect profile denies builtin/write", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-deny-write");
		const playbookFile = join(dir, "deny-write.playbook.yaml");
		const targetFile = join(dir, "blocked.txt");
		writeFileSync(
			playbookFile,
			`playbook: deny-write\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: attempt a blocked write\n` +
				`    tools:\n` +
				`      - tool: builtin/write\n` +
				`        with:\n` +
				`          path: ./blocked.txt\n` +
				`          content: denied\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
		const result = await runtime.start();
		expect(result.run.status).toBe("failed");
		expect(existsSync(targetFile)).toBe(false);
		expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("does not allow write");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("workspace_write denies path escape outside writableRoots", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-path-escape");
		const workspaceDir = join(dir, "workspace");
		const escapedFile = join(dir, "escaped.txt");
		const playbookFile = join(dir, "escape.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		writeFileSync(
			playbookFile,
			`playbook: path-escape\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`  writableRoots:\n` +
				`    - ./workspace\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to escape the workspace\n` +
				`    tools:\n` +
				`      - tool: builtin/write\n` +
				`        with:\n` +
				`          path: ../escaped.txt\n` +
				`          content: escaped\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
		const result = await runtime.start();
		expect(result.run.status).toBe("failed");
		expect(existsSync(escapedFile)).toBe(false);
		expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("writableRoots");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("approvalMode on-act blocks builtin/bash even for workspace_exec profile", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-bash-approval");
		const playbookFile = join(dir, "bash.playbook.yaml");
		const targetFile = join(dir, "should-not-exist.txt");
		writeFileSync(
			playbookFile,
			`playbook: bash-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_exec\n` +
				`policy:\n` +
				`  approvalMode: on-act\n` +
				`agents:\n` +
				`  local/basher:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to run bash\n` +
				`    tools:\n` +
				`      - tool: builtin/bash\n` +
				`        with:\n` +
				`          command: "printf blocked > ./should-not-exist.txt"\n` +
				`tasks:\n` +
				`  - id: basher\n` +
				`    uses: agent:local/basher\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
		const result = await runtime.start();
		expect(result.run.status).toBe("failed");
		expect(existsSync(targetFile)).toBe(false);
		expect(readFileSync(playbookFile, "utf8")).toContain("builtin/bash");
		expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.basher.error).toContain("requires approval");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});
});
