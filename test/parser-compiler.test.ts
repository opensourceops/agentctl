import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { compilePlaybook } from "../src/compiler.js";
import { loadPlaybookWithPacks } from "../src/parser.js";

describe("parser and compiler", () => {
	test("loads pack-scoped modules and agents into the playbook definition", () => {
		const definition = loadPlaybookWithPacks(join(process.cwd(), "examples/hello.playbook.yaml"));
		expect(definition.modules?.["demo/status_probe"]?.kind).toBe("builtin.shell.exec");
		expect(definition.agents?.["demo/reviewer"]?.kind).toBe("builtin.heuristic");
	});

	test("real autonomy example persists the agent report before verification", () => {
		const definition = loadPlaybookWithPacks(join(process.cwd(), "examples/real-autonomy/mission.playbook.yaml"));
		const plan = compilePlaybook(definition);
		const persistTask = plan.taskIndex.get("persist_report");
		const verifyTask = plan.taskIndex.get("verify_report");
		const auditAgent = definition.agents?.["real/ops_auditor"];

		expect(auditAgent?.kind).toBe("openai.responses");
		expect(auditAgent?.instructions).toContain("## Evidence");
		expect(auditAgent?.instructions).toContain('There is no documented on-call escalation policy.');
		expect(auditAgent?.instructions).toContain("No restore validation drill is scheduled.");
		expect(auditAgent?.tools?.find((tool) => tool.name === "find_files")?.with?.pattern).toBe("**/*");
		expect(persistTask?.use.ref).toBe("builtin.write");
		expect(persistTask?.needs).toEqual(["audit"]);
		expect(verifyTask?.needs).toEqual(["persist_report"]);
		expect(String(verifyTask?.with.command ?? "")).toContain("set -eu");
		expect(String(verifyTask?.with.command ?? "")).toContain("^## Evidence");
		expect(String(verifyTask?.with.command ?? "")).toContain("README.md");
		expect(String(verifyTask?.with.command ?? "")).toContain("docs/runbook.md");
	});

	test("remote MCP example persists the remote report and verifies required findings", () => {
		const definition = loadPlaybookWithPacks(join(process.cwd(), "examples/remote-mcp-autonomy/mission.playbook.yaml"));
		const plan = compilePlaybook(definition);
		const auditAgent = definition.agents?.["remote/service_auditor"];
		const persistTask = plan.taskIndex.get("persist_report");
		const verifyTask = plan.taskIndex.get("verify_report");

		expect(definition.mcpServers?.auditor?.url).toBe("http://127.0.0.1:43127/mcp");
		expect(auditAgent?.kind).toBe("builtin.heuristic");
		expect(auditAgent?.tools?.[0]?.tool).toBe("mcp:auditor/audit_service");
		expect(persistTask?.use.ref).toBe("builtin.write");
		expect(persistTask?.needs).toEqual(["audit"]);
		expect(verifyTask?.needs).toEqual(["persist_report"]);
		expect(String(verifyTask?.with.command ?? "")).toContain("set -eu");
		expect(String(verifyTask?.with.command ?? "")).toContain("rollback validation drill");
		expect(String(verifyTask?.with.command ?? "")).toContain("incident communication owner");
	});

	test("rejects cyclic task graphs", () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cycle-"));
		const file = join(dir, "cycle.playbook.yaml");
		writeFileSync(
			file,
			`playbook: cycle\n` +
				`tasks:\n` +
				`  - id: first\n` +
				`    uses: module:builtin.assign\n` +
				`    needs: [second]\n` +
				`    with:\n` +
				`      values:\n` +
				`        hello: world\n` +
				`  - id: second\n` +
				`    uses: module:builtin.assign\n` +
				`    needs: [first]\n` +
				`    with:\n` +
				`      values:\n` +
				`        hello: world\n`,
		);

		expect(() => compilePlaybook(loadPlaybookWithPacks(file))).toThrow("Task graph contains a cycle");
		rmSync(dir, { recursive: true, force: true });
	});

	test("compiles mongodb-atlas long-term memory config with explicit connection settings", () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-mongodb-memory-"));
		const file = join(dir, "mongodb-memory.playbook.yaml");
		writeFileSync(
			file,
			`playbook: mongodb-memory\n` +
				`memory:\n` +
				`  longTerm:\n` +
				`    provider: mongodb-atlas\n` +
				`    connectionStringEnv: AGENTCTL_MONGODB_URI\n` +
				`    database: ops\n` +
				`    collection: findings\n` +
				`    namespace: service-audit\n` +
				`tasks:\n` +
				`  - id: remember\n` +
				`    uses: module:builtin.long_term_memory.write\n` +
				`    with:\n` +
				`      key: incident-owner\n` +
				`      value: pager-duty\n`,
			"utf8",
		);

		try {
			const plan = compilePlaybook(loadPlaybookWithPacks(file));
			expect(plan.memory.longTerm).toEqual({
				provider: "mongodb-atlas",
				dbPath: "",
				namespace: "service-audit",
				connectionStringEnv: "AGENTCTL_MONGODB_URI",
				connectionString: "",
				database: "ops",
				collection: "findings",
			});
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
