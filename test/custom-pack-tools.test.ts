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

describe("custom pack process tools", () => {
	test("loads pack.process modules with pack-relative paths resolved", () => {
		const definition = loadPlaybookWithPacks(join(process.cwd(), "examples/custom-pack-tools/mission.playbook.yaml"));
		const fixtureAudit = definition.modules?.["custom/fixture_audit"];
		const nodeVersion = definition.modules?.["custom/node_version"];
		const auditor = definition.agents?.["custom/auditor"];

		expect(fixtureAudit?.kind).toBe("pack.process");
		expect(fixtureAudit?.cwd).toBe(join(process.cwd(), "examples/custom-pack-tools"));
		expect(nodeVersion?.kind).toBe("pack.process");
		expect(nodeVersion?.command).toBe("node");
		expect(auditor?.tools?.map((tool) => tool.tool)).toEqual(["custom/node_version", "custom/fixture_audit"]);
	});

	test("agent can call both an existing wrapped command and a pack-shipped script", async () => {
		const { dir, dbPath } = createTempDb("agentctl-custom-pack");
		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(
			compilePlaybook(loadPlaybookWithPacks(join(process.cwd(), "examples/custom-pack-tools/mission.playbook.yaml"))),
			store,
		);

		const result = await runtime.start();

		expect(result.run.status).toBe("succeeded");
		expect(result.run.snapshot.tasks.audit.output?.observations).toBeDefined();
		const observations = result.run.snapshot.tasks.audit.output?.observations;
		expect(Array.isArray(observations)).toBe(true);
		expect(observations).toHaveLength(2);
		expect(typeof observations?.[0]).toBe("object");
		expect(result.run.snapshot.tasks.audit.output?.finalText).toContain("# Custom Pack Report");
		expect(result.run.snapshot.tasks.verify_report.output?.stdout).toBe("verified");
		expect(
			readFileSync(join(process.cwd(), "examples/custom-pack-tools/artifacts/custom-pack-report.md"), "utf8"),
		).toContain("rollback owner");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("preflight fails before run creation when a required executable is missing", async () => {
		const { dir, dbPath } = createTempDb("agentctl-custom-pack-preflight");
		const playbookFile = join(dir, "missing-runtime.playbook.yaml");
		const packFile = join(dir, "missing-runtime.pack.yaml");
		writeFileSync(
			packFile,
			`pack: missing\n` +
				`version: "1"\n` +
				`modules:\n` +
				`  check:\n` +
				`    kind: pack.process\n` +
				`    command: node\n` +
				`    args:\n` +
				`      - --version\n` +
				`    runtime:\n` +
				`      requires:\n` +
				`        - command: missing-agentctl-tool\n` +
				`    policy:\n` +
				`      capability: observe\n` +
				`      risk: low\n` +
				`agents:\n` +
				`  runner:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructions: run the tool\n` +
				`    profile: inspect\n` +
				`    tools:\n` +
				`      - tool: missing/check\n`,
			"utf8",
		);
		writeFileSync(
			playbookFile,
			`playbook: missing-runtime\n` +
				`packs:\n` +
				`  - ./missing-runtime.pack.yaml\n` +
				`tasks:\n` +
				`  - id: audit\n` +
				`    uses: agent:missing/runner\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const runtime = new PlaybookRuntime(compilePlaybook(loadPlaybookWithPacks(playbookFile)), store);

		await expect(runtime.start()).rejects.toThrow("Required executable not found: missing-agentctl-tool");
		expect(store.getStats().runs.total).toBe(0);

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});

	test("resume does not rerun a succeeded pack.process side effect", async () => {
		const { dir, dbPath } = createTempDb("agentctl-custom-pack-resume");
		const counterFile = join(dir, "counter.txt");
		const scriptFile = join(dir, "bump.mjs");
		const packFile = join(dir, "resume.pack.yaml");
		const playbookFile = join(dir, "resume.playbook.yaml");
		writeFileSync(
			scriptFile,
			`import { existsSync, readFileSync, writeFileSync } from "node:fs";\n` +
				`const path = process.argv[2];\n` +
				`const count = existsSync(path) ? Number(readFileSync(path, "utf8")) : 0;\n` +
				`writeFileSync(path, String(count + 1));\n` +
				`process.stdout.write(String(count + 1));\n`,
			"utf8",
		);
		writeFileSync(
			packFile,
			`pack: resume\n` +
				`version: "1"\n` +
				`modules:\n` +
				`  bump:\n` +
				`    kind: pack.process\n` +
				`    command: node\n` +
				`    args:\n` +
				`      - ./bump.mjs\n` +
				`      - ${counterFile}\n` +
				`    cwd: .\n` +
				`    runtime:\n` +
				`      requires:\n` +
				`        - command: node\n` +
				`          version: \">=22\"\n` +
				`    policy:\n` +
				`      capability: act\n` +
				`      risk: high\n`,
			"utf8",
		);
		writeFileSync(
			playbookFile,
			`playbook: resume-pack-process\n` +
				`packs:\n` +
				`  - ./resume.pack.yaml\n` +
				`tasks:\n` +
				`  - id: bump\n` +
				`    uses: module:resume/bump\n` +
				`  - id: verify\n` +
				`    needs: [bump]\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        done: true\n`,
			"utf8",
		);

		const store = new CheckpointStore(dbPath);
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
		let interruptedRunId = "";
		const runtime = new PlaybookRuntime(plan, store, {
			hooks: {
				afterCheckpoint(checkpoint) {
					if (checkpoint.taskId === "bump" && checkpoint.snapshot.tasks.bump.status === "succeeded") {
						interruptedRunId = checkpoint.runId;
						throw new Error("interrupt after pack.process success");
					}
				},
			},
		});

		await expect(runtime.start()).rejects.toThrow("interrupt after pack.process success");
		expect(readFileSync(counterFile, "utf8")).toBe("1");

		const resumed = await new PlaybookRuntime(plan, store).resume(interruptedRunId);
		expect(resumed.run.status).toBe("succeeded");
		expect(readFileSync(counterFile, "utf8")).toBe("1");

		store.close();
		rmSync(dir, { recursive: true, force: true });
	});
});
