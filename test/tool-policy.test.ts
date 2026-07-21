import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
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

async function runPlaybook(
	playbookFile: string,
	dbPath: string,
	options: ConstructorParameters<typeof PlaybookRuntime>[2] = {},
) {
	const store = new CheckpointStore(dbPath);
	const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, options);
	const result = await runtime.start();
	return { store, result };
}

describe("tool policy", () => {
	test("none profile allows internal memory reads", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-none-internal");
		const playbookFile = join(dir, "none-internal.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: none-internal\n` +
				`defaults:\n` +
				`  agentProfile: none\n` +
				`memory:\n` +
				`  working:\n` +
				`    initial:\n` +
				`      finding: grounded\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: read memory\n` +
				`    tools:\n` +
				`      - tool: builtin/memory-read\n` +
				`        with:\n` +
				`          key: finding\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("succeeded");
			expect(JSON.stringify(result.run.snapshot.tasks.reader.output)).toContain("grounded");
			expect(store.listAuditEvents(result.run.id)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						scope: "policy",
						name: "policy.allow",
						attributes: expect.objectContaining({
							tool_ref: "builtin/memory-read",
							capability: "internal",
						}),
					}),
				]),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

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

	test("none profile denies builtin/read", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-none-deny-read");
		const playbookFile = join(dir, "none-deny-read.playbook.yaml");
		writeFileSync(join(dir, "note.txt"), "policy-check\n", "utf8");
		writeFileSync(
			playbookFile,
			`playbook: none-deny-read\n` +
				`defaults:\n` +
				`  agentProfile: none\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: attempt a blocked read\n` +
				`    tools:\n` +
				`      - tool: builtin/read\n` +
				`        with:\n` +
				`          path: ./note.txt\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.reader.error).toContain("does not allow read");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
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

	test("workspace_write allows builtin/write inside writableRoots", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-write-ok");
		const workspaceDir = join(dir, "workspace");
		const targetFile = join(workspaceDir, "allowed.txt");
		const playbookFile = join(dir, "write-ok.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		writeFileSync(
			playbookFile,
			`playbook: write-ok\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`  writableRoots:\n` +
				`    - ./workspace\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: write the file\n` +
				`    tools:\n` +
				`      - tool: builtin/write\n` +
				`        with:\n` +
				`          path: ./allowed.txt\n` +
				`          content: allowed\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("succeeded");
			expect(readFileSync(targetFile, "utf8")).toBe("allowed");
			expect(store.listAuditEvents(result.run.id)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						scope: "policy",
						name: "policy.allow",
						attributes: expect.objectContaining({
							tool_ref: "builtin/write",
							capability: "mutate",
						}),
					}),
				]),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("absolute read path outside workspaceRoot is denied", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-absolute-read-escape");
		const workspaceDir = join(dir, "workspace");
		const outsideFile = join(dir, "outside.txt");
		const playbookFile = join(dir, "absolute-read.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		writeFileSync(outsideFile, "outside\n", "utf8");
		writeFileSync(
			playbookFile,
			`playbook: absolute-read-escape\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to read outside\n` +
				`    tools:\n` +
				`      - tool: builtin/read\n` +
				`        with:\n` +
				`          path: ${outsideFile}\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.reader.error).toContain("escapes workspaceRoot");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("symlinked read path escaping workspaceRoot is denied", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-symlink-read-escape");
		const workspaceDir = join(dir, "workspace");
		const outsideDir = join(dir, "outside");
		const symlinkPath = join(workspaceDir, "linked-outside");
		const playbookFile = join(dir, "symlink-read.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		await mkdir(outsideDir, { recursive: true });
		writeFileSync(join(outsideDir, "secret.txt"), "secret\n", "utf8");
		symlinkSync(outsideDir, symlinkPath);
		writeFileSync(
			playbookFile,
			`playbook: symlink-read-escape\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to traverse a symlink\n` +
				`    tools:\n` +
				`      - tool: builtin/read\n` +
				`        with:\n` +
				`          path: ./linked-outside/secret.txt\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.reader.error).toContain("escapes workspaceRoot");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("symlinked write path escaping writableRoots is denied", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-symlink-write-escape");
		const workspaceDir = join(dir, "workspace");
		const outsideDir = join(dir, "outside");
		const escapedFile = join(outsideDir, "escaped.txt");
		const symlinkPath = join(workspaceDir, "linked-outside");
		const playbookFile = join(dir, "symlink-write.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		await mkdir(outsideDir, { recursive: true });
		symlinkSync(outsideDir, symlinkPath);
		writeFileSync(
			playbookFile,
			`playbook: symlink-write-escape\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`  writableRoots:\n` +
				`    - ./workspace\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to write through a symlink\n` +
				`    tools:\n` +
				`      - tool: builtin/write\n` +
				`        with:\n` +
				`          path: ./linked-outside/escaped.txt\n` +
				`          content: escaped\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(existsSync(escapedFile)).toBe(false);
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("writableRoots");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("bash cwd absolute escape is denied", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-bash-cwd-escape");
		const workspaceDir = join(dir, "workspace");
		const outsideDir = join(dir, "outside");
		const playbookFile = join(dir, "bash-cwd.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		await mkdir(outsideDir, { recursive: true });
		writeFileSync(
			playbookFile,
			`playbook: bash-cwd-escape\n` +
				`defaults:\n` +
				`  agentProfile: workspace_exec\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`agents:\n` +
				`  local/basher:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to run bash outside\n` +
				`    tools:\n` +
				`      - tool: builtin/bash\n` +
				`        with:\n` +
				`          cwd: ${outsideDir}\n` +
				`          command: pwd\n` +
				`tasks:\n` +
				`  - id: basher\n` +
				`    uses: agent:local/basher\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.basher.error).toContain("bash cwd");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("approvalMode on-mutate pauses builtin/write under workspace_write", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-mutate-approval");
		const workspaceDir = join(dir, "workspace");
		const targetFile = join(workspaceDir, "blocked.txt");
		const playbookFile = join(dir, "mutate-approval.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		writeFileSync(
			playbookFile,
			`playbook: mutate-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`  writableRoots:\n` +
				`    - ./workspace\n` +
				`  approvalMode: on-mutate\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: ask for approval\n` +
				`    tools:\n` +
				`      - tool: builtin/write\n` +
				`        with:\n` +
				`          path: ./blocked.txt\n` +
				`          content: blocked\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("paused");
			expect(existsSync(targetFile)).toBe(false);
			expect(result.run.snapshot.tasks.writer.status).toBe("waiting_approval");
			expect(result.run.snapshot.tasks.writer.approvalId).toBeTruthy();
			expect(store.listAuditEvents(result.run.id)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						scope: "policy",
						name: "policy.require_approval",
						attributes: expect.objectContaining({
							tool_ref: "builtin/write",
							capability: "mutate",
						}),
					}),
					expect.objectContaining({
						scope: "approval",
						name: "approval.created",
						attributes: expect.objectContaining({
							tool_ref: "builtin/write",
						}),
					}),
				]),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("approvalMode always pauses builtin/read even for inspect profile", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-always-approval");
		const playbookFile = join(dir, "always-approval.playbook.yaml");
		writeFileSync(join(dir, "note.txt"), "approval\n", "utf8");
		writeFileSync(
			playbookFile,
			`playbook: always-approval\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`policy:\n` +
				`  approvalMode: always\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: ask for approval on read\n` +
				`    tools:\n` +
				`      - tool: builtin/read\n` +
				`        with:\n` +
				`          path: ./note.txt\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("paused");
			expect(result.run.snapshot.tasks.reader.status).toBe("waiting_approval");
			expect(store.listApprovals({ runId: result.run.id, status: "pending" })).toHaveLength(1);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("approvalMode on-act pauses builtin/bash even for workspace_exec profile", async () => {
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
		expect(result.run.status).toBe("paused");
		expect(existsSync(targetFile)).toBe(false);
		expect(readFileSync(playbookFile, "utf8")).toContain("builtin/bash");
		expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.basher.status).toBe("waiting_approval");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("builtin/bash from an agent requires approval even when approvalMode is never", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-bash-agent-approval");
		const playbookFile = join(dir, "bash-agent-approval.playbook.yaml");
		const targetFile = join(dir, "blocked.txt");
		writeFileSync(
			playbookFile,
			`playbook: bash-agent-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_exec\n` +
				`policy:\n` +
				`  approvalMode: never\n` +
				`agents:\n` +
				`  local/basher:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try to run bash\n` +
				`    tools:\n` +
				`      - tool: builtin/bash\n` +
				`        with:\n` +
				`          command: "printf blocked > ./blocked.txt"\n` +
				`tasks:\n` +
				`  - id: basher\n` +
				`    uses: agent:local/basher\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("paused");
			expect(existsSync(targetFile)).toBe(false);
			expect(result.run.snapshot.tasks.basher.status).toBe("waiting_approval");
			expect(store.listAuditEvents(result.run.id)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						scope: "policy",
						name: "policy.require_approval",
						attributes: expect.objectContaining({
							tool_ref: "builtin/bash",
							reason: expect.stringContaining("subprocess"),
						}),
					}),
				]),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("mcp observe tools are denied under none profile and never called", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-mcp-deny");
		const playbookFile = join(dir, "mcp-deny.playbook.yaml");
		let callCount = 0;
		writeFileSync(
			playbookFile,
			`playbook: mcp-deny\n` +
				`defaults:\n` +
				`  agentProfile: none\n` +
				`mcpServers:\n` +
				`  docs:\n` +
				`    url: https://example.invalid\n` +
				`agents:\n` +
				`  local/reader:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try mcp\n` +
				`    tools:\n` +
				`      - tool: mcp:docs/read_note\n` +
				`tasks:\n` +
				`  - id: reader\n` +
				`    uses: agent:local/reader\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath, {
			mcpServers: {
				docs: {
					async listTools() {
						return [{ name: "read_note", capability: "observe", risk: "low" }];
					},
					async callTool() {
						callCount += 1;
						return { content: "should not run" };
					},
				},
			},
		});
		try {
			expect(result.run.status).toBe("failed");
			expect(callCount).toBe(0);
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.reader.error).toContain("does not allow docs/read_note");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("a2a tools are denied under workspace_write and never called", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-a2a-deny");
		const playbookFile = join(dir, "a2a-deny.playbook.yaml");
		let callCount = 0;
		writeFileSync(
			playbookFile,
			`playbook: a2a-deny\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`a2aAgents:\n` +
				`  delegate:\n` +
				`    url: https://example.invalid\n` +
				`agents:\n` +
				`  local/delegator:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try a2a\n` +
				`    tools:\n` +
				`      - tool: a2a:delegate\n` +
				`tasks:\n` +
				`  - id: delegator\n` +
				`    uses: agent:local/delegator\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath, {
			a2aAgents: {
				delegate: {
					async sendTask() {
						callCount += 1;
						return {
							taskId: "a2a-task",
							contextId: "a2a-context",
							state: "COMPLETED",
							output: { finalText: "should not run" },
						};
					},
				},
			},
		});
		try {
			expect(result.run.status).toBe("failed");
			expect(callCount).toBe(0);
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.delegator.error).toContain("does not allow delegate");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("pack.process policy overrides are enforced for agent tool calls", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-pack-process-override");
		const playbookFile = join(dir, "pack-process-override.playbook.yaml");
		const targetFile = join(dir, "should-not-exist.txt");
		writeFileSync(
			playbookFile,
			`playbook: pack-process-override\n` +
				`defaults:\n` +
				`  agentProfile: inspect\n` +
				`modules:\n` +
				`  local/write_file:\n` +
				`    kind: pack.process\n` +
				`    command: node\n` +
				`    args:\n` +
				`      - -e\n` +
				`      - "require('node:fs').writeFileSync(process.argv[1], 'blocked')"\n` +
				`      - ${targetFile}\n` +
				`    policy:\n` +
				`      label: custom-write\n` +
				`      capability: mutate\n` +
				`      risk: high\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try custom tool\n` +
				`    tools:\n` +
				`      - tool: local/write_file\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(existsSync(targetFile)).toBe(false);
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("does not allow custom-write");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("pack.process task cwd escaping workspaceRoot is denied before execution", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-pack-process-cwd-escape");
		const workspaceDir = join(dir, "workspace");
		const outsideDir = join(dir, "outside");
		const targetFile = join(outsideDir, "should-not-exist.txt");
		const playbookFile = join(dir, "pack-process-cwd-escape.playbook.yaml");
		await mkdir(workspaceDir, { recursive: true });
		await mkdir(outsideDir, { recursive: true });
		writeFileSync(
			playbookFile,
			`playbook: pack-process-cwd-escape\n` +
				`policy:\n` +
				`  workspaceRoot: ./workspace\n` +
				`modules:\n` +
				`  local/write_outside:\n` +
				`    kind: pack.process\n` +
				`    command: node\n` +
				`    args:\n` +
				`      - -e\n` +
				`      - "require('node:fs').writeFileSync('should-not-exist.txt', 'blocked')"\n` +
				`    cwd: ${outsideDir}\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: module:local/write_outside\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("failed");
			expect(existsSync(targetFile)).toBe(false);
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("cwd");
			expect(store.getLatestCheckpoint(result.run.id).snapshot.tasks.writer.error).toContain("workspaceRoot");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("pack.process exposed to an agent requires approval even when approvalMode is never", async () => {
		const { dir, dbPath } = createTempDb("agentctl-policy-pack-process-agent-approval");
		const playbookFile = join(dir, "pack-process-agent-approval.playbook.yaml");
		const targetFile = join(dir, "blocked.txt");
		writeFileSync(
			playbookFile,
			`playbook: pack-process-agent-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_exec\n` +
				`policy:\n` +
				`  approvalMode: never\n` +
				`modules:\n` +
				`  local/write_file:\n` +
				`    kind: pack.process\n` +
				`    command: node\n` +
				`    args:\n` +
				`      - -e\n` +
				`      - "require('node:fs').writeFileSync(process.argv[1], 'blocked')"\n` +
				`      - ./blocked.txt\n` +
				`agents:\n` +
				`  local/writer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: try process tool\n` +
				`    tools:\n` +
				`      - tool: local/write_file\n` +
				`tasks:\n` +
				`  - id: writer\n` +
				`    uses: agent:local/writer\n`,
			"utf8",
		);

		const { store, result } = await runPlaybook(playbookFile, dbPath);
		try {
			expect(result.run.status).toBe("paused");
			expect(existsSync(targetFile)).toBe(false);
			expect(result.run.snapshot.tasks.writer.status).toBe("waiting_approval");
			expect(store.listAuditEvents(result.run.id)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						scope: "policy",
						name: "policy.require_approval",
						attributes: expect.objectContaining({
							tool_ref: "local/write_file",
							tool_provider: "module",
							reason: expect.stringContaining("subprocess"),
						}),
					}),
				]),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
