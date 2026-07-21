import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { compilePlaybook } from "../src/compiler.js";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { BuiltinModuleRegistry } from "../src/modules.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

function createTempDb(name: string): { dir: string; dbPath: string } {
	const dir = mkdtempSync(join(tmpdir(), `${name}-`));
	return { dir, dbPath: join(dir, "runtime.db") };
}

describe("runtime", () => {
	test("executes the demo playbook successfully", async () => {
		const { dir, dbPath } = createTempDb("agentctl-success");
		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(
			compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/hello.playbook.yaml"))),
			store,
		);

		const result = await runtime.start();
		expect(result.run.status).toBe("succeeded");
		expect(result.run.snapshot.tasks.verify.status).toBe("succeeded");
		expect(result.run.snapshot.tasks.review.output?.finalText).toBe("status=ready\nframework=agentctl");
		expect(store.listCheckpoints(result.run.id).length).toBeGreaterThanOrEqual(5);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("resume does not rerun a task that was already checkpointed as succeeded", async () => {
		const { dir, dbPath } = createTempDb("agentctl-resume");
		const counterFile = join(dir, "counter.txt");
		const playbookFile = join(dir, "resume.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: resume-safety\n` +
				`tasks:\n` +
				`  - id: side_effect\n` +
				`    uses: module:builtin.shell.exec\n` +
				`    with:\n` +
				`      command: "node -e \\"const fs=require('fs'); const p='${counterFile}'; const n=fs.existsSync(p)?Number(fs.readFileSync(p,'utf8')):0; fs.writeFileSync(p, String(n+1)); console.log(n+1)\\""\n` +
				`  - id: finalize\n` +
				`    uses: module:builtin.assign\n` +
				`    needs: [side_effect]\n` +
				`    with:\n` +
				`      values:\n` +
				`        done: true\n`,
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		let interruptedRunId = "";
		const runtime = new PlaybookRuntime(plan, store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					if (
						checkpoint.taskId === "side_effect" &&
						checkpoint.snapshot.tasks.side_effect.status === "succeeded"
					) {
						interruptedRunId = checkpoint.runId;
						throw new Error("simulated crash after success checkpoint");
					}
				},
			},
		});

		await expect(runtime.start()).rejects.toThrow("simulated crash after success checkpoint");
		expect(readFileSync(counterFile, "utf8")).toBe("1");

		const resumed = await new PlaybookRuntime(plan, store).resume(interruptedRunId);
		expect(resumed.run.status).toBe("succeeded");
		expect(readFileSync(counterFile, "utf8")).toBe("1");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("replay forks a new run from an earlier checkpoint", async () => {
		const { dir, dbPath } = createTempDb("agentctl-replay");
		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/hello.playbook.yaml")));
		const runtime = new PlaybookRuntime(plan, store);
		const original = await runtime.start();
		const checkpoints = store.listCheckpoints(original.run.id);

		const replayed = await runtime.replay(original.run.id, checkpoints[1]!.seq);
		expect(replayed.run.id).not.toBe(original.run.id);
		expect(replayed.run.status).toBe("succeeded");
		expect(store.listCheckpoints(replayed.run.id).length).toBeGreaterThan(0);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("replay from a succeeded checkpoint does not rerun completed side effects", async () => {
		const { dir, dbPath } = createTempDb("agentctl-replay-no-rerun");
		const counterFile = join(dir, "counter.txt");
		const playbookFile = join(dir, "replay-no-rerun.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: replay-no-rerun\n` +
				`tasks:\n` +
				`  - id: side_effect\n` +
				`    uses: module:builtin.shell.exec\n` +
				`    with:\n` +
				`      cwd: ${JSON.stringify(dir)}\n` +
				`      command: "node -e \\"const fs=require('fs'); const p='${counterFile}'; const n=fs.existsSync(p)?Number(fs.readFileSync(p,'utf8')):0; fs.writeFileSync(p, String(n+1)); console.log(n+1)\\""\n` +
				`  - id: finalize\n` +
				`    needs: [side_effect]\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        done: true\n`,
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		const runtime = new PlaybookRuntime(plan, store);
		const original = await runtime.start();
		const succeededCheckpoint = store
			.listCheckpoints(original.run.id)
			.find((checkpoint) => checkpoint.taskId === "side_effect" && checkpoint.snapshot.tasks.side_effect.status === "succeeded");

		expect(succeededCheckpoint).toBeDefined();
		expect(readFileSync(counterFile, "utf8")).toBe("1");

		const replayed = await runtime.replay(original.run.id, succeededCheckpoint!.seq);
		expect(replayed.run.status).toBe("succeeded");
		expect(readFileSync(counterFile, "utf8")).toBe("1");
		expect(replayed.run.snapshot.tasks.side_effect.attempts).toBe(1);
		expect(replayed.run.snapshot.tasks.finalize.status).toBe("succeeded");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("resume rejects terminal runs instead of appending another terminal checkpoint", async () => {
		const { dir, dbPath } = createTempDb("agentctl-resume-terminal");
		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/hello.playbook.yaml")));
		const runtime = new PlaybookRuntime(plan, store);
		const result = await runtime.start();
		const before = store.listCheckpoints(result.run.id).length;

		await expect(runtime.resume(result.run.id)).rejects.toThrow(`Run "${result.run.id}" is already succeeded; use replay to fork from an earlier checkpoint`);
		expect(store.listCheckpoints(result.run.id)).toHaveLength(before);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("bounded agents fail when they exceed max turns", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-bound");
		const playbookFile = join(dir, "agent-bound.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: max-turns\n` +
				`agents:\n` +
				`  local/too-many-tools:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: keep running tools\n` +
				`    maxTurns: 2\n` +
				`    tools:\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            one: 1\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            two: 2\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            three: 3\n` +
				`tasks:\n` +
				`  - id: agent_task\n` +
				`    uses: agent:local/too-many-tools\n`,
		);

		const store = new CheckpointStore(dbPath);
		let failedRunId = "";
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					failedRunId = checkpoint.runId;
				},
			},
		});
		const result = await runtime.start();
		expect(result.run.status).toBe("failed");
		expect(failedRunId).not.toBe("");
		expect(store.getLatestCheckpoint(failedRunId).snapshot.tasks.agent_task.status).toBe("failed");
		expect(existsSync(dbPath)).toBe(true);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("agent checkpoints stay contiguous across multiple turns", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-seq");
		const playbookFile = join(dir, "agent-seq.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: agent-seq\n` +
				`agents:\n` +
				`  local/reviewer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: collect both observations then finish\n` +
				`    maxTurns: 3\n` +
				`    tools:\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            first: alpha\n` +
				`      - tool: builtin.assign\n` +
				`        with:\n` +
				`          values:\n` +
				`            second: beta\n` +
				`tasks:\n` +
				`  - id: review\n` +
				`    uses: agent:local/reviewer\n`,
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
		const result = await runtime.start();
		const checkpoints = store.listCheckpoints(result.run.id);

		expect(result.run.status).toBe("succeeded");
		expect(checkpoints.map((checkpoint) => checkpoint.seq)).toEqual(
			checkpoints.map((_, index) => index + 1),
		);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("builtin.find matches model-style glob patterns against discovered files", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-find-glob-"));
		const workspaceRoot = join(dir, "workspace");
		const nestedDir = join(workspaceRoot, "docs");
		mkdirSync(nestedDir, { recursive: true });
		writeFileSync(join(workspaceRoot, "README.md"), "# readme\n", "utf8");
		writeFileSync(join(nestedDir, "runbook.md"), "# runbook\n", "utf8");

		const registry = new BuiltinModuleRegistry();
		const result = await registry.executeResolved(
			"run-id",
			"find-task",
			{ kind: "builtin.find" },
			{ path: ".", pattern: "**/*" },
			{ inputs: {}, vars: {}, memory: { working: {} }, tasks: {}, agents: {} },
			workspaceRoot,
		);

		expect(result.output.matches).toEqual([
			join(workspaceRoot, "README.md"),
			join(workspaceRoot, "docs", "runbook.md"),
		]);

		rmSync(dir, { recursive: true, force: true });
	});

	test("working memory writes are checkpointed and resumed with vars mirrored", async () => {
		const { dir, dbPath } = createTempDb("agentctl-working-memory-resume");
		const playbookFile = join(dir, "working-memory.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: working-memory\n` +
				`memory:\n` +
				`  working:\n` +
				`    initial:\n` +
				`      seed: start\n` +
				`tasks:\n` +
				`  - id: write_fact\n` +
				`    uses: module:builtin.memory.write\n` +
				`    with:\n` +
				`      key: finding\n` +
				`      value: grounded\n` +
				`  - id: read_fact\n` +
				`    needs: [write_fact]\n` +
				`    uses: module:builtin.memory.read\n` +
				`    with:\n` +
				`      key: finding\n` +
				`  - id: assert_fact\n` +
				`    needs: [read_fact]\n` +
				`    uses: module:builtin.assert\n` +
				`    with:\n` +
				`      equals:\n` +
				`        left: "{{ memory.working.finding }}"\n` +
				`        right: grounded\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		let interruptedRunId = "";
		const runtime = new PlaybookRuntime(plan, store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					if (
						checkpoint.taskId === "write_fact" &&
						checkpoint.snapshot.tasks.write_fact.status === "succeeded"
					) {
						interruptedRunId = checkpoint.runId;
						throw new Error("interrupt after write");
					}
				},
			},
		});

		await expect(runtime.start()).rejects.toThrow("interrupt after write");
		const interrupted = store.getLatestCheckpoint(interruptedRunId);
		expect(interrupted.snapshot.memory.working).toEqual({
			seed: "start",
			finding: "grounded",
		});
		expect(interrupted.snapshot.vars).toEqual(interrupted.snapshot.memory.working);

		const resumed = await new PlaybookRuntime(plan, store).resume(interruptedRunId);
		expect(resumed.run.status).toBe("succeeded");
		expect(resumed.run.snapshot.memory.working.finding).toBe("grounded");
		expect(resumed.run.snapshot.vars.finding).toBe("grounded");
		expect(resumed.run.snapshot.tasks.read_fact.output).toEqual({
			key: "finding",
			value: "grounded",
			found: true,
		});

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("task output templates preserve structured JSON values across steps", async () => {
		const { dir, dbPath } = createTempDb("agentctl-structured-dataflow");
		const playbookFile = join(dir, "structured-dataflow.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: structured-dataflow\n` +
				`tasks:\n` +
				`  - id: produce\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        payload:\n` +
				`          items:\n` +
				`            - alpha\n` +
				`            - beta\n` +
				`          meta:\n` +
				`            status: ready\n` +
				`  - id: consume\n` +
				`    needs: [produce]\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        copied: "{{ tasks.produce.output.values.payload }}"\n` +
				`  - id: verify\n` +
				`    needs: [consume]\n` +
				`    uses: module:builtin.assert\n` +
				`    with:\n` +
				`      equals:\n` +
				`        left: "{{ tasks.consume.output.values.copied }}"\n` +
				`        right:\n` +
				`          items:\n` +
				`            - alpha\n` +
				`            - beta\n` +
				`          meta:\n` +
				`            status: ready\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.consume.output).toEqual({
				assignedAt: expect.any(String),
				values: {
					copied: {
						items: ["alpha", "beta"],
						meta: {
							status: "ready",
						},
					},
				},
			});
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
			}
		});

	test("task vars override agent defaults and support bare plus namespaced resolution", async () => {
		const { dir, dbPath } = createTempDb("agentctl-task-vars");
		const playbookFile = join(dir, "task-vars.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: task-vars\n` +
				`inputs:\n` +
				`  service: payments\n` +
				`agents:\n` +
				`  local/reviewer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: "service={{ service }} alias={{ vars.service }} input={{ inputs.service }} finding={{ finding }} severity={{ severity }}"\n` +
				`    vars:\n` +
				`      service: default-service\n` +
				`      severity: medium\n` +
				`tasks:\n` +
				`  - id: prepare\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        finding: restore-drill-missing\n` +
				`  - id: project\n` +
				`    needs: [prepare]\n` +
				`    uses: module:builtin.assign\n` +
				`    vars:\n` +
				`      service: checkout\n` +
				`      finding: "{{ tasks.prepare.output.values.finding }}"\n` +
				`    with:\n` +
				`      values:\n` +
				`        preview: "{{ service }}|{{ vars.finding }}|{{ inputs.service }}"\n` +
				`  - id: review\n` +
				`    needs: [prepare]\n` +
				`    uses: agent:local/reviewer\n` +
				`    vars:\n` +
				`      service: checkout\n` +
				`      finding: "{{ tasks.prepare.output.values.finding }}"\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const result = await runtime.start();
			expect(result.run.status).toBe("succeeded");
			expect(result.run.snapshot.tasks.project.output?.values).toEqual({
				preview: "checkout|restore-drill-missing|payments",
			});
			expect(result.run.snapshot.tasks.review.output?.finalText).toBe(
				"service=checkout alias=checkout input=payments finding=restore-drill-missing severity=medium",
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("resume preserves multi-turn agent memory workflows", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-memory-resume");
		const playbookFile = join(dir, "agent-memory-resume.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: agent-memory-resume\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: memory-agent\n` +
				`agents:\n` +
				`  local/memory_worker:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: progress through memory tools\n` +
				`    maxTurns: 4\n` +
				`    tools:\n` +
				`      - tool: builtin/memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: grounded\n` +
				`      - tool: builtin/long-term-memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ memory.working.finding }}"\n` +
				`      - tool: builtin/long-term-memory-retrieve\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          promoteKey: recalled\n` +
				`tasks:\n` +
				`  - id: memory_agent\n` +
				`    uses: agent:local/memory_worker\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		let interruptedRunId = "";
		const runtime = new PlaybookRuntime(plan, store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					const session = checkpoint.snapshot.agents.memory_agent;
					if (checkpoint.taskId === "memory_agent" && session && session.turns.length === 1) {
						interruptedRunId = checkpoint.runId;
						throw new Error("interrupt after first memory turn");
					}
				},
			},
		});

		await expect(runtime.start()).rejects.toThrow("interrupt after first memory turn");
		const interrupted = store.getLatestCheckpoint(interruptedRunId);
		expect(interrupted.snapshot.memory.working.finding).toBe("grounded");
		expect(interrupted.snapshot.agents.memory_agent?.turns).toHaveLength(1);

		const resumed = await new PlaybookRuntime(plan, store).resume(interruptedRunId);
		expect(resumed.run.status).toBe("succeeded");
		expect(resumed.run.snapshot.memory.working.finding).toBe("grounded");
		expect(resumed.run.snapshot.memory.working.recalled).toBe("grounded");
		expect(resumed.run.snapshot.vars.recalled).toBe("grounded");

		store.close();
			rmSync(dir, { recursive: true, force: true });
		});

	test("resume preserves task-scoped vars across multi-turn memory agent workflows", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-memory-task-vars-resume");
		const playbookFile = join(dir, "agent-memory-task-vars-resume.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: agent-memory-task-vars-resume\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: memory-agent\n` +
				`agents:\n` +
				`  local/memory_worker:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: progress through memory tools\n` +
				`    maxTurns: 4\n` +
				`    tools:\n` +
				`      - tool: builtin/memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ finding }}"\n` +
				`      - tool: builtin/long-term-memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ finding }}"\n` +
				`      - tool: builtin/long-term-memory-retrieve\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          promoteKey: recalled\n` +
				`tasks:\n` +
				`  - id: prepare\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        finding: grounded\n` +
				`  - id: memory_agent\n` +
				`    needs: [prepare]\n` +
				`    uses: agent:local/memory_worker\n` +
				`    vars:\n` +
				`      finding: "{{ tasks.prepare.output.values.finding }}"\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		let interruptedRunId = "";
		const runtime = new PlaybookRuntime(plan, store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					const session = checkpoint.snapshot.agents.memory_agent;
					if (checkpoint.taskId === "memory_agent" && session && session.turns.length === 1) {
						interruptedRunId = checkpoint.runId;
						throw new Error("interrupt after first task-var turn");
					}
				},
			},
		});

		await expect(runtime.start()).rejects.toThrow("interrupt after first task-var turn");
		const interrupted = store.getLatestCheckpoint(interruptedRunId);
		expect(interrupted.snapshot.memory.working.finding).toBe("grounded");
		expect(interrupted.snapshot.agents.memory_agent?.resolvedVars).toEqual({
			finding: "grounded",
		});

		const resumed = await new PlaybookRuntime(plan, store).resume(interruptedRunId);
		expect(resumed.run.status).toBe("succeeded");
		expect(resumed.run.snapshot.memory.working.recalled).toBe("grounded");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("replay from a mid-agent memory checkpoint preserves agent session and working memory", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-memory-replay");
		const playbookFile = join(dir, "agent-memory-replay.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: agent-memory-replay\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: memory-agent\n` +
				`agents:\n` +
				`  local/memory_worker:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: progress through memory tools\n` +
				`    maxTurns: 4\n` +
				`    tools:\n` +
				`      - tool: builtin/memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: grounded\n` +
				`      - tool: builtin/long-term-memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ memory.working.finding }}"\n` +
				`      - tool: builtin/long-term-memory-retrieve\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          promoteKey: recalled\n` +
				`tasks:\n` +
				`  - id: memory_agent\n` +
				`    uses: agent:local/memory_worker\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		const runtime = new PlaybookRuntime(plan, store);
		const original = await runtime.start();
		const checkpoint = store
			.listCheckpoints(original.run.id)
			.find((entry) => entry.taskId === "memory_agent" && entry.snapshot.agents.memory_agent?.turns.length === 2);

		expect(checkpoint).toBeDefined();

		const replayed = await runtime.replay(original.run.id, checkpoint!.seq);
		expect(replayed.run.status).toBe("succeeded");
		expect(replayed.run.id).not.toBe(original.run.id);
		expect(replayed.run.snapshot.memory.working.recalled).toBe("grounded");

		const replayedCheckpoints = store.listCheckpoints(replayed.run.id);
		expect(replayedCheckpoints[0]?.snapshot.agents.memory_agent?.turns).toHaveLength(2);
		expect(replayedCheckpoints[0]?.snapshot.memory.working.finding).toBe("grounded");

		store.close();
			rmSync(dir, { recursive: true, force: true });
		});

	test("replay from a mid-agent checkpoint preserves resolved task vars for later memory turns", async () => {
		const { dir, dbPath } = createTempDb("agentctl-agent-memory-task-vars-replay");
		const playbookFile = join(dir, "agent-memory-task-vars-replay.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: agent-memory-task-vars-replay\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    dbPath: ./long-term.db\n` +
				`    namespace: memory-agent\n` +
				`agents:\n` +
				`  local/memory_worker:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: progress through memory tools\n` +
				`    maxTurns: 4\n` +
				`    tools:\n` +
				`      - tool: builtin/memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ finding }}"\n` +
				`      - tool: builtin/long-term-memory-write\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          value: "{{ finding }}"\n` +
				`      - tool: builtin/long-term-memory-retrieve\n` +
				`        with:\n` +
				`          key: finding\n` +
				`          promoteKey: recalled\n` +
				`tasks:\n` +
				`  - id: prepare\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        finding: grounded\n` +
				`  - id: memory_agent\n` +
				`    needs: [prepare]\n` +
				`    uses: agent:local/memory_worker\n` +
				`    vars:\n` +
				`      finding: "{{ tasks.prepare.output.values.finding }}"\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		const runtime = new PlaybookRuntime(plan, store);
		const original = await runtime.start();
		const checkpoint = store
			.listCheckpoints(original.run.id)
			.find((entry) => entry.taskId === "memory_agent" && entry.snapshot.agents.memory_agent?.turns.length === 2);

		expect(checkpoint).toBeDefined();
		expect(checkpoint?.snapshot.agents.memory_agent?.resolvedVars).toEqual({
			finding: "grounded",
		});

		const replayed = await runtime.replay(original.run.id, checkpoint!.seq);
		expect(replayed.run.status).toBe("succeeded");
		expect(replayed.run.snapshot.memory.working.recalled).toBe("grounded");
		expect(store.listCheckpoints(replayed.run.id)[0]?.snapshot.agents.memory_agent?.resolvedVars).toEqual({
			finding: "grounded",
		});

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("approved module mutations resume successfully and execute exactly once", async () => {
		const { dir, dbPath } = createTempDb("agentctl-approval-resume-module");
		const playbookFile = join(dir, "approval-module.playbook.yaml");
		const targetFile = join(dir, "note.txt");
		writeFileSync(
			playbookFile,
			`playbook: approval-module\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ${JSON.stringify(dir)}\n` +
				`  writableRoots:\n` +
				`    - ${JSON.stringify(dir)}\n` +
				`  approvalMode: on-mutate\n` +
				`tasks:\n` +
				`  - id: write_note\n` +
				`    uses: module:builtin.write\n` +
				`    with:\n` +
				`      path: ./note.txt\n` +
				`      content: approved write\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const paused = await runtime.start();
			expect(paused.run.status).toBe("paused");
			expect(paused.run.snapshot.tasks.write_note.status).toBe("waiting_approval");
			expect(existsSync(targetFile)).toBe(false);

			const approvalId = paused.run.snapshot.tasks.write_note.approvalId;
			expect(approvalId).toBeTruthy();
			store.resolveApproval(approvalId!, "approved", { resolvedBy: "tester" });

			const resumed = await runtime.resume(paused.run.id);
			expect(resumed.run.status).toBe("succeeded");
			expect(resumed.run.snapshot.tasks.write_note.status).toBe("succeeded");
			expect(readFileSync(targetFile, "utf8")).toBe("approved write");
			expect(resumed.run.snapshot.tasks.write_note.attempts).toBe(1);

			const approvalAudit = store
				.listAuditEvents(resumed.run.id)
				.find((event) => event.name === "approval.applied");
			expect(approvalAudit).toBeDefined();
			expect(approvalAudit?.attributes).toEqual(
				expect.objectContaining({
					task_id: "write_note",
					approval_id: approvalId,
					tool_ref: "builtin.write",
				}),
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("rejected approvals fail the blocked task on resume without executing the tool", async () => {
		const { dir, dbPath } = createTempDb("agentctl-approval-reject-module");
		const playbookFile = join(dir, "reject-module.playbook.yaml");
		const targetFile = join(dir, "note.txt");
		writeFileSync(
			playbookFile,
			`playbook: reject-module\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ${JSON.stringify(dir)}\n` +
				`  writableRoots:\n` +
				`    - ${JSON.stringify(dir)}\n` +
				`  approvalMode: on-mutate\n` +
				`tasks:\n` +
				`  - id: write_note\n` +
				`    uses: module:builtin.write\n` +
				`    with:\n` +
				`      path: ./note.txt\n` +
				`      content: should-not-write\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const paused = await runtime.start();
			expect(paused.run.status).toBe("paused");
			const approvalId = paused.run.snapshot.tasks.write_note.approvalId;
			expect(approvalId).toBeTruthy();
			store.resolveApproval(approvalId!, "rejected", { resolvedBy: "tester", resolutionNote: "nope" });

			const resumed = await runtime.resume(paused.run.id);
			expect(resumed.run.status).toBe("failed");
			expect(resumed.run.snapshot.tasks.write_note.status).toBe("failed");
			expect(resumed.run.snapshot.tasks.write_note.error).toContain("Tool call rejected");
			expect(existsSync(targetFile)).toBe(false);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("resume refuses runs with pending approvals", async () => {
		const { dir, dbPath } = createTempDb("agentctl-approval-pending");
		const playbookFile = join(dir, "pending-approval.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: pending-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ${JSON.stringify(dir)}\n` +
				`  writableRoots:\n` +
				`    - ${JSON.stringify(dir)}\n` +
				`  approvalMode: on-mutate\n` +
				`tasks:\n` +
				`  - id: write_note\n` +
				`    uses: module:builtin.write\n` +
				`    with:\n` +
				`      path: ./note.txt\n` +
				`      content: pending\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const paused = await runtime.start();
			expect(paused.run.status).toBe("paused");
			await expect(runtime.resume(paused.run.id)).rejects.toThrow(
				`Run "${paused.run.id}" is paused with pending approval "${paused.run.snapshot.tasks.write_note.approvalId}" for task "write_note"`,
			);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("replay from a paused approval checkpoint forks a fresh run with a fresh approval", async () => {
		const { dir, dbPath } = createTempDb("agentctl-approval-replay");
		const playbookFile = join(dir, "replay-approval.playbook.yaml");
		writeFileSync(
			playbookFile,
			`playbook: replay-approval\n` +
				`defaults:\n` +
				`  agentProfile: workspace_write\n` +
				`policy:\n` +
				`  workspaceRoot: ${JSON.stringify(dir)}\n` +
				`  writableRoots:\n` +
				`    - ${JSON.stringify(dir)}\n` +
				`  approvalMode: on-mutate\n` +
				`tasks:\n` +
				`  - id: write_note\n` +
				`    uses: module:builtin.write\n` +
				`    with:\n` +
				`      path: ./note.txt\n` +
				`      content: replayed\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		try {
			const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);
			const paused = await runtime.start();
			expect(paused.run.status).toBe("paused");
			const pausedCheckpoint = store.getLatestCheckpoint(paused.run.id);
			const originalApprovalId = paused.run.snapshot.tasks.write_note.approvalId;
			expect(originalApprovalId).toBeTruthy();

			const replayed = await runtime.replay(paused.run.id, pausedCheckpoint.seq);
			expect(replayed.run.status).toBe("paused");
			expect(replayed.run.id).not.toBe(paused.run.id);
			expect(replayed.run.snapshot.tasks.write_note.status).toBe("waiting_approval");
			expect(replayed.run.snapshot.tasks.write_note.approvalId).toBeTruthy();
			expect(replayed.run.snapshot.tasks.write_note.approvalId).not.toBe(originalApprovalId);
			expect(store.getApproval(replayed.run.snapshot.tasks.write_note.approvalId!).runId).toBe(replayed.run.id);
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
