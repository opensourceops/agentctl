import { execFile } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { promisify } from "node:util";
import YAML from "yaml";
import { describe, expect, test } from "vitest";
import { CheckpointStore } from "../src/checkpoint-store.js";
import { compilePlaybook } from "../src/compiler.js";
import { loadPlaybookWithPacks } from "../src/parser.js";
import { PlaybookRuntime } from "../src/runtime.js";

const execFileAsync = promisify(execFile);

interface CliResult {
	stdout: string;
	stderr: string;
	exitCode: number;
}

async function runCli(args: string[], env: NodeJS.ProcessEnv = {}): Promise<CliResult> {
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

function parseJsonLines(stdout: string): unknown[] {
	return stdout
		.trim()
		.split("\n")
		.filter((line) => line.length > 0)
		.map((line) => JSON.parse(line) as unknown);
}

function parseYamlDocuments(stdout: string): unknown[] {
	return YAML.parseAllDocuments(stdout).map((document) => document.toJSON());
}

function stripAnsi(text: string): string {
	return text.replace(/\u001b\[[0-9;]*m/g, "");
}

describe("cli", () => {
	test("-h prints help and exits successfully", async () => {
		const { stdout, stderr } = await runCli(["-h"]);
		expect(stderr).toBe("");
		expect(stdout).toContain("Usage:");
		expect(stdout).toContain("agentctl check <playbook.yaml> [flags]");
		expect(stdout).toContain("Use command-specific help for examples and command-specific flags.");
		expect(stdout).toContain("agentctl run <playbook.yaml> [flags]");
		expect(stdout).toContain("agentctl memory <subcommand> [flags]");
		expect(stdout).toContain("--verbose");
		expect(stdout).toContain("--output");
		expect(stdout).toContain("--color");
	});

	test("run --help prints command-specific examples and runtime flags", async () => {
		const { stdout, stderr, exitCode } = await runCli(["run", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl run <playbook.yaml> [flags]");
		expect(stdout).toContain("Streams checkpoint events progressively");
		expect(stdout).toContain("examples/real-autonomy/mission.playbook.yaml");
		expect(stdout).toContain("--api-key key");
		expect(stdout).toContain("--provider name");
	});

	test("check --help prints validation-specific usage and examples", async () => {
		const { stdout, stderr, exitCode } = await runCli(["check", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl check <playbook.yaml> [flags]");
		expect(stdout).toContain("Reports YAML syntax, schema, prompt-file, template-reference, and compile errors");
		expect(stdout).toContain("examples/prompt-file-vars/mission.playbook.yaml");
	});

	test("help memory prints memory backend specific help", async () => {
		const { stdout, stderr, exitCode } = await runCli(["help", "memory"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl memory write <key> (--value json | --string text) [flags]");
		expect(stdout).toContain("--provider sqlite|mongodb-atlas");
		expect(stdout).toContain("Long-term memory backend");
		expect(stdout).not.toContain("Provider for --api-key");
	});

	test("gc --help prints gc usage instead of executing gc", async () => {
		const { stdout, stderr, exitCode } = await runCli(["gc", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl gc");
		expect(stdout).toContain("older-than-days");
		expect(stdout).toContain("Running and paused runs are preserved.");
	});

	test("db --help prints db usage instead of trying to execute a subcommand", async () => {
		const { stdout, stderr, exitCode } = await runCli(["db", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl db stats");
		expect(stdout).toContain("Read-only runtime DB inspection.");
	});

	test("memory --help prints memory usage instead of executing a subcommand", async () => {
		const { stdout, stderr, exitCode } = await runCli(["memory", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl memory get");
		expect(stdout).toContain("agentctl memory write");
		expect(stdout).toContain("Reads fail on a missing SQLite memory DB path");
	});

	test("approvals --help prints approval workflow usage", async () => {
		const { stdout, stderr, exitCode } = await runCli(["approvals", "--help"]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl approvals list");
		expect(stdout).toContain("agentctl approvals approve <approval-id>");
		expect(stdout).toContain("agentctl approvals reject <approval-id>");
	});

	test("-V and --version print the package version", async () => {
		const longVersion = await runCli(["--version"]);
		const shortVersion = await runCli(["-V"]);
		expect(longVersion.stderr).toBe("");
		expect(shortVersion.stderr).toBe("");
		expect(longVersion.stdout.trim()).toBe("0.1.0");
		expect(shortVersion.stdout.trim()).toBe("0.1.0");
	});

	test("-v enables verbose auth output", async () => {
		const home = mkdtempSync(join(tmpdir(), "agentctl-auth-verbose-"));
		try {
			const { stdout, stderr, exitCode } = await runCli(
				["auth", "check", "examples/real-autonomy/mission.playbook.yaml", "-v", "--provider", "openai", "--api-key", "runtime-key"],
				{ HOME: home, OPENAI_API_KEY: "" },
			);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const documents = parseYamlDocuments(stdout) as Array<Record<string, unknown>>;
			expect(documents).toHaveLength(1);
			expect(documents[0]?.type).toBe("auth_check");
			expect(documents[0]?.plan).toBeDefined();
		} finally {
			rmSync(home, { recursive: true, force: true });
		}
	});

	test("update prints deterministic source-checkout instructions", async () => {
		const { stdout, stderr } = await runCli(["update"]);
		expect(stderr).toBe("");
		expect(stdout).toContain("agentctl update");
		expect(stdout).toContain("git pull --rebase");
		expect(stdout).toContain("npm install");
	});

	test("auth check exits nonzero when provider auth is missing", async () => {
		const home = mkdtempSync(join(tmpdir(), "agentctl-auth-missing-"));
		try {
			const { stdout, stderr, exitCode } = await runCli(["auth", "check", "--provider", "openai"], {
				HOME: home,
				OPENAI_API_KEY: "",
				OPENAI_BASE_URL: "",
			});
			expect(exitCode).toBe(1);
			expect(stderr).toBe("");
			expect(parseYamlDocuments(stdout)).toEqual([
				{
					type: "auth_check",
					ok: false,
					providers: [
						{
							provider: "openai",
							configured: false,
							source: "missing",
							issues: ['No API key configured for provider "openai"'],
						},
					],
				},
			]);
		} finally {
			rmSync(home, { recursive: true, force: true });
		}
	});

	test("run uses ~/.agentctl/runtime/runtime.db by default", async () => {
		const home = mkdtempSync(join(tmpdir(), "agentctl-home-default-db-"));
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-default-db-"));
		const playbookPath = join(dir, "default-db.playbook.yaml");
		writeFileSync(
			playbookPath,
			`playbook: default-db\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const result = await runCli(["run", playbookPath], { HOME: home });
			expect(result.exitCode).toBe(0);
			const defaultDb = join(home, ".agentctl", "runtime", "runtime.db");
			expect(existsSync(defaultDb)).toBe(true);
		} finally {
			rmSync(home, { recursive: true, force: true });
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("check succeeds for the prompt-file-vars example", async () => {
		const { stdout, stderr, exitCode } = await runCli([
			"check",
			"examples/prompt-file-vars/mission.playbook.yaml",
			"--output",
			"json",
		]);
		expect(exitCode).toBe(0);
		expect(stderr).toBe("");
		expect(parseJsonLines(stdout)).toEqual([
			{
				type: "check",
				ok: true,
				playbook: `${process.cwd()}/examples/prompt-file-vars/mission.playbook.yaml`,
				packs: [],
				compiled: true,
			},
		]);
	});

	test("check reports YAML syntax errors with exact line and column", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-check-yaml-"));
		const playbookPath = join(dir, "broken.playbook.yaml");
		writeFileSync(
			playbookPath,
			`playbook: broken\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`      with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["check", playbookPath, "--output", "json"]);
			expect(exitCode).toBe(1);
			expect(stderr).toBe("");
			expect(parseJsonLines(stdout)).toEqual([
				expect.objectContaining({
					type: "check",
					ok: false,
					packs: [],
					playbook: playbookPath,
					diagnostics: [
						expect.objectContaining({
							file: playbookPath,
							phase: "yaml_syntax",
							line: 4,
							column: 11,
							detail: expect.stringContaining("Nested mappings are not allowed in compact mappings"),
						}),
					],
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("check fails when an agent prompt file is missing", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-check-prompt-file-"));
		const playbookPath = join(dir, "missing-prompt.playbook.yaml");
		writeFileSync(
			playbookPath,
			`playbook: missing-prompt\n` +
				`agents:\n` +
				`  local/reviewer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructionsFile: ./prompts/review.md\n` +
				`tasks:\n` +
				`  - id: review\n` +
				`    uses: agent:local/reviewer\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["check", playbookPath, "--output", "json"]);
			expect(exitCode).toBe(1);
			expect(stderr).toBe("");
			expect(parseJsonLines(stdout)).toEqual([
				expect.objectContaining({
					type: "check",
					ok: false,
					diagnostics: [
						expect.objectContaining({
							file: playbookPath,
							phase: "load",
							detail: expect.stringContaining("Agent instructions file not found"),
						}),
					],
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

		test("check fails when a task using an agent does not supply a required prompt var", async () => {
			const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-check-prompt-vars-"));
			const promptDir = join(dir, "prompts");
			const playbookPath = join(dir, "undefined-var.playbook.yaml");
		mkdirSync(promptDir, { recursive: true });
		writeFileSync(join(promptDir, "review.md"), "Finding: {{ finding }}\n", "utf8");
		writeFileSync(
			playbookPath,
			`playbook: undefined-var\n` +
				`agents:\n` +
				`  local/reviewer:\n` +
				`    kind: builtin.heuristic\n` +
				`    instructionsFile: ./prompts/review.md\n` +
				`tasks:\n` +
				`  - id: review\n` +
				`    uses: agent:local/reviewer\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["check", playbookPath, "--output", "json"]);
			expect(exitCode).toBe(1);
			expect(stderr).toBe("");
				expect(parseJsonLines(stdout)).toEqual([
					expect.objectContaining({
						type: "check",
						ok: false,
						diagnostics: [
							expect.objectContaining({
								file: playbookPath,
								phase: "template",
								path: "tasks.review.uses",
								detail: 'Task "review" references undefined prompt variable "finding" for agent "local/reviewer"',
							}),
						],
					}),
				]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
			}
		});

		test("check validates prompt vars per task invocation for the same agent", async () => {
			const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-check-task-vars-"));
			const promptDir = join(dir, "prompts");
			const playbookPath = join(dir, "task-vars.playbook.yaml");
			mkdirSync(promptDir, { recursive: true });
			writeFileSync(join(promptDir, "review.md"), "Service: {{ service }}\nFinding: {{ finding }}\n", "utf8");
			writeFileSync(
				playbookPath,
				`playbook: task-vars\n` +
					`agents:\n` +
					`  local/reviewer:\n` +
					`    kind: builtin.heuristic\n` +
					`    instructionsFile: ./prompts/review.md\n` +
					`    vars:\n` +
					`      service: default-service\n` +
					`tasks:\n` +
					`  - id: prepare\n` +
					`    uses: module:builtin.assign\n` +
					`    with:\n` +
					`      values:\n` +
					`        finding: restore-drill-missing\n` +
					`  - id: review_ok\n` +
					`    needs: [prepare]\n` +
					`    uses: agent:local/reviewer\n` +
					`    vars:\n` +
					`      finding: "{{ tasks.prepare.output.values.finding }}"\n` +
					`  - id: review_bad\n` +
					`    needs: [prepare]\n` +
					`    uses: agent:local/reviewer\n`,
				"utf8",
			);

			try {
				const { stdout, stderr, exitCode } = await runCli(["check", playbookPath, "--output", "json"]);
				expect(exitCode).toBe(1);
				expect(stderr).toBe("");
				expect(parseJsonLines(stdout)).toEqual([
					expect.objectContaining({
						type: "check",
						ok: false,
						diagnostics: [
							expect.objectContaining({
								file: playbookPath,
								phase: "template",
								path: "tasks.review_bad.uses",
								detail: 'Task "review_bad" references undefined prompt variable "finding" for agent "local/reviewer"',
							}),
						],
					}),
				]);
			} finally {
				rmSync(dir, { recursive: true, force: true });
			}
		});

		test("check fails when task input references an undefined vars alias", async () => {
			const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-check-task-input-vars-"));
			const playbookPath = join(dir, "undefined-task-var.playbook.yaml");
			writeFileSync(
				playbookPath,
				`playbook: undefined-task-var\n` +
					`tasks:\n` +
					`  - id: project\n` +
					`    uses: module:builtin.assign\n` +
					`    with:\n` +
					`      values:\n` +
					`        rendered: "{{ vars.finding }}"\n`,
				"utf8",
			);

			try {
				const { stdout, stderr, exitCode } = await runCli(["check", playbookPath, "--output", "json"]);
				expect(exitCode).toBe(1);
				expect(stderr).toBe("");
				expect(parseJsonLines(stdout)).toEqual([
					expect.objectContaining({
						type: "check",
						ok: false,
						diagnostics: [
							expect.objectContaining({
								file: playbookPath,
								phase: "template",
								path: "tasks.project.with",
								detail: 'Task "project" input references undefined variable "finding"',
							}),
						],
					}),
				]);
			} finally {
				rmSync(dir, { recursive: true, force: true });
			}
		});

	test("run streams YAML checkpoints by default before the final result", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-yaml-run-"));
		const playbookPath = join(dir, "stream.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: stream-default-yaml\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["run", playbookPath, "--db", dbPath]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const documents = parseYamlDocuments(stdout) as Array<Record<string, unknown>>;
			expect(documents.length).toBeGreaterThanOrEqual(3);
			expect(documents[0]?.type).toBe("checkpoint");
			expect(documents[0]?.seq).toBe(1);
			expect(documents.at(-1)?.type).toBe("result");
			expect((documents.at(-1) as { run: { status: string } }).run.status).toBe("succeeded");
			expect(((documents.at(-1) as { run: { snapshot: { tasks: Record<string, { output?: Record<string, unknown> }> } } }).run.snapshot.tasks.init.output ?? {})).not.toHaveProperty("values");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("verbose run includes full task output payloads", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-yaml-run-verbose-"));
		const playbookPath = join(dir, "stream.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: stream-default-yaml\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["run", playbookPath, "--db", dbPath, "--verbose"]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			const documents = parseYamlDocuments(stdout) as Array<Record<string, unknown>>;
			const result = documents.at(-1) as { run: { snapshot: { tasks: Record<string, { output?: Record<string, unknown> }> } } };
			expect(result.run.snapshot.tasks.init.output?.values).toEqual({ status: "ready" });
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("yaml output can be colorized explicitly", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-color-yaml-"));
		const playbookPath = join(dir, "color.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: color-output\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli(["run", playbookPath, "--db", dbPath, "--color", "always"]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			expect(stdout).toContain("\u001b[");
			const documents = parseYamlDocuments(stripAnsi(stdout)) as Array<Record<string, unknown>>;
			expect(documents[0]?.type).toBe("checkpoint");
			expect(documents.at(-1)?.type).toBe("result");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("json output stays machine-parseable even when color is requested", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-color-json-"));
		const playbookPath = join(dir, "color-json.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: color-json-output\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const { stdout, stderr, exitCode } = await runCli([
				"run",
				playbookPath,
				"--db",
				dbPath,
				"--output",
				"json",
				"--color",
				"always",
			]);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			expect(stdout).not.toContain("\u001b[");
			const events = parseJsonLines(stdout) as Array<Record<string, unknown>>;
			expect(events[0]?.type).toBe("checkpoint");
			expect(events.at(-1)?.type).toBe("result");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("playbook output format defaults to json when configured and CLI override can force yaml", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-output-format-"));
		const playbookPath = join(dir, "output.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: output-format\n` +
				`output:\n` +
				`  format: json\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const jsonResult = await runCli(["run", playbookPath, "--db", dbPath]);
			expect(jsonResult.exitCode).toBe(0);
			expect(jsonResult.stderr).toBe("");
			const jsonEvents = parseJsonLines(jsonResult.stdout) as Array<Record<string, unknown>>;
			expect(jsonEvents[0]?.type).toBe("checkpoint");
			expect(jsonEvents.at(-1)?.type).toBe("result");

			const yamlResult = await runCli(["run", playbookPath, "--db", dbPath, "--output", "yaml"]);
			expect(yamlResult.exitCode).toBe(0);
			expect(yamlResult.stderr).toBe("");
			const yamlEvents = parseYamlDocuments(yamlResult.stdout) as Array<Record<string, unknown>>;
			expect(yamlEvents[0]?.type).toBe("checkpoint");
			expect(yamlEvents.at(-1)?.type).toBe("result");
			expect(yamlResult.stdout).toContain("type: checkpoint");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("resume rejects terminal runs with a concrete replay hint", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-resume-terminal-"));
		const playbookPath = join(dir, "resume-terminal.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: resume-terminal\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json"]);
			expect(runResult.exitCode).toBe(0);
			const runEvents = parseJsonLines(runResult.stdout) as Array<Record<string, unknown>>;
			const runId = ((runEvents.at(-1) as { run: { id: string } }).run.id);

			const resumeResult = await runCli(["resume", playbookPath, runId, "--db", dbPath]);
			expect(resumeResult.exitCode).toBe(1);
			expect(resumeResult.stdout).toBe("");
			expect(resumeResult.stderr).toContain(`Run "${runId}" is already succeeded; use replay to fork from an earlier checkpoint`);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("db stats reports runtime database counts and latest run metadata", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-db-stats-"));
		const playbookPath = join(dir, "db-stats.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: db-stats\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath]);
			expect(runResult.exitCode).toBe(0);

			const statsResult = await runCli(["db", "stats", "--db", dbPath, "--output", "json"]);
			expect(statsResult.exitCode).toBe(0);
			expect(statsResult.stderr).toBe("");
			expect(parseJsonLines(statsResult.stdout)).toEqual([
				expect.objectContaining({
					type: "db_stats",
					dbPath,
					runs: expect.objectContaining({
						total: 1,
						succeeded: 1,
						failed: 0,
						running: 0,
					}),
					records: expect.objectContaining({
						checkpoints: expect.any(Number),
						taskAttempts: expect.any(Number),
						agentTurns: 0,
						auditEvents: expect.any(Number),
						traceSpans: expect.any(Number),
					}),
					latestRun: expect.objectContaining({
						playbookName: "db-stats",
						status: "succeeded",
					}),
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("db stats fails for a missing database path instead of creating an empty database", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-db-stats-missing-"));
		const missingDbPath = join(dir, "missing.db");
		try {
			const result = await runCli(["db", "stats", "--db", missingDbPath]);
			expect(result.exitCode).toBe(1);
			expect(result.stdout).toBe("");
			expect(result.stderr).toContain(`Runtime DB not found: ${missingDbPath}`);
			expect(existsSync(missingDbPath)).toBe(false);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("memory get fails for a missing database path instead of creating an empty database", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-memory-missing-"));
		const missingDbPath = join(dir, "missing-memory.db");
		try {
			const result = await runCli(["memory", "get", "finding", "--db", missingDbPath]);
			expect(result.exitCode).toBe(1);
			expect(result.stdout).toBe("");
			expect(result.stderr).toContain(`Memory DB not found: ${missingDbPath}`);
			expect(existsSync(missingDbPath)).toBe(false);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("memory write/get/search/stats support namespaces and cross-namespace exact key lookup", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-memory-"));
		const dbPath = join(dir, "long-term.db");

		try {
			const writeOne = await runCli([
				"memory",
				"write",
				"finding",
				"--db",
				dbPath,
				"--namespace",
				"service-a",
				"--string",
				"restore-drill-missing",
				"--tags",
				"readiness,audit",
				"--output",
				"json",
			]);
			expect(writeOne.exitCode).toBe(0);
			const writeOnePayload = parseJsonLines(writeOne.stdout)[0] as {
				type: string;
				namespace: string;
				entry: { key: string; value: string; tags: string[] };
			};
			expect(writeOnePayload.type).toBe("memory_write");
			expect(writeOnePayload.namespace).toBe("service-a");
			expect(writeOnePayload.entry).toEqual(
				expect.objectContaining({
					key: "finding",
					value: "restore-drill-missing",
					tags: ["readiness", "audit"],
				}),
			);

			const writeTwo = await runCli([
				"memory",
				"write",
				"finding",
				"--db",
				dbPath,
				"--namespace",
				"service-b",
				"--value",
				'{"status":"present"}',
				"--output",
				"json",
			]);
			expect(writeTwo.exitCode).toBe(0);

			const getAll = await runCli(["memory", "get", "finding", "--db", dbPath, "--output", "json"]);
			expect(getAll.exitCode).toBe(0);
			expect(parseJsonLines(getAll.stdout)).toEqual([
				expect.objectContaining({
					type: "memory_get",
					namespace: null,
					key: "finding",
					found: true,
					matchCount: 2,
					matches: [
						expect.objectContaining({ namespace: "service-b", key: "finding", value: { status: "present" } }),
						expect.objectContaining({ namespace: "service-a", key: "finding", value: "restore-drill-missing" }),
					],
				}),
			]);

			const searchNamespace = await runCli([
				"memory",
				"search",
				"--db",
				dbPath,
				"--namespace",
				"service-a",
				"--query",
				"restore",
				"--output",
				"json",
			]);
			expect(searchNamespace.exitCode).toBe(0);
			expect(parseJsonLines(searchNamespace.stdout)).toEqual([
				expect.objectContaining({
					type: "memory_search",
					namespace: "service-a",
					query: "restore",
					matchCount: 1,
					matches: [expect.objectContaining({ namespace: "service-a", key: "finding", value: "restore-drill-missing" })],
				}),
			]);

			const statsAll = await runCli(["memory", "stats", "--db", dbPath, "--output", "json"]);
			expect(statsAll.exitCode).toBe(0);
			expect(parseJsonLines(statsAll.stdout)).toEqual([
				expect.objectContaining({
					type: "memory_stats",
					dbPath,
					totalEntries: 2,
					totalNamespaces: 2,
					namespaces: [
						expect.objectContaining({ namespace: "service-a", entries: 1 }),
						expect.objectContaining({ namespace: "service-b", entries: 1 }),
					],
				}),
			]);

			const statsNamespace = await runCli([
				"memory",
				"stats",
				"--db",
				dbPath,
				"--namespace",
				"service-a",
				"--output",
				"json",
			]);
			expect(statsNamespace.exitCode).toBe(0);
			expect(parseJsonLines(statsNamespace.stdout)).toEqual([
				expect.objectContaining({
					type: "memory_stats",
					namespace: "service-a",
					totalEntries: 1,
					totalNamespaces: 1,
					namespaces: [expect.objectContaining({ namespace: "service-a", entries: 1 })],
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("memory gc prunes old long-term memory entries while keeping the newest configured entries", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-memory-gc-"));
		const dbPath = join(dir, "long-term.db");

		try {
			await runCli(["memory", "write", "finding-a", "--db", dbPath, "--namespace", "service-a", "--string", "alpha"]);
			await runCli(["memory", "write", "finding-b", "--db", dbPath, "--namespace", "service-a", "--string", "beta"]);

			const gcResult = await runCli([
				"memory",
				"gc",
				"--db",
				dbPath,
				"--older-than-days",
				"0",
				"--keep-entries",
				"1",
				"--output",
				"json",
				"--verbose",
			]);
			expect(gcResult.exitCode).toBe(0);
			expect(gcResult.stderr).toBe("");
			expect(parseJsonLines(gcResult.stdout)).toEqual([
				expect.objectContaining({
					type: "memory_gc",
					provider: "sqlite",
					deletedEntries: 1,
					keepEntries: 1,
					after: expect.objectContaining({
						totalEntries: 1,
					}),
					deletedKeys: [expect.objectContaining({ namespace: "service-a", key: "finding-a" })],
				}),
			]);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("gc prunes old terminal runs while keeping the newest configured runs", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-gc-"));
		const playbookPath = join(dir, "gc.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		writeFileSync(
			playbookPath,
			`playbook: gc-playbook\n` +
				`tasks:\n` +
				`  - id: init\n` +
				`    uses: module:builtin.assign\n` +
				`    with:\n` +
				`      values:\n` +
				`        status: ready\n`,
			"utf8",
		);

		try {
			const firstRun = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json"]);
			const secondRun = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json"]);
			expect(firstRun.exitCode).toBe(0);
			expect(secondRun.exitCode).toBe(0);

			const gcResult = await runCli([
				"gc",
				"--db",
				dbPath,
				"--older-than-days",
				"0",
				"--keep-runs",
				"1",
				"--output",
				"json",
				"--verbose",
			]);
			expect(gcResult.exitCode).toBe(0);
			expect(gcResult.stderr).toBe("");
			const payload = parseJsonLines(gcResult.stdout)[0] as {
				type: string;
				deletedRuns: number;
				deletedRunIds: string[];
				before: { runs: { total: number } };
				after: { runs: { total: number; succeeded: number } };
				vacuumed: boolean;
			};
			expect(payload.type).toBe("gc");
			expect(payload.deletedRuns).toBe(1);
			expect(payload.deletedRunIds).toHaveLength(1);
			expect(payload.before.runs.total).toBe(2);
			expect(payload.after.runs.total).toBe(1);
			expect(payload.after.runs.succeeded).toBe(1);
			expect(payload.vacuumed).toBe(true);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("gc never deletes running runs", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-gc-running-"));
		const dbPath = join(dir, "runtime.db");
		const store = new CheckpointStore(dbPath);
		const runningRun = store.createRun("running-playbook", {
			inputs: {},
			vars: {},
			memory: {
				working: {},
			},
			tasks: {
				active: {
					status: "running",
					attempts: 1,
				},
			},
			agents: {},
		});
		store.close();

		try {
			const gcResult = await runCli([
				"gc",
				"--db",
				dbPath,
				"--older-than-days",
				"0",
				"--keep-runs",
				"0",
				"--output",
				"json",
				"--verbose",
			]);
			expect(gcResult.exitCode).toBe(0);
			expect(gcResult.stderr).toBe("");
			const payload = parseJsonLines(gcResult.stdout)[0] as {
				type: string;
				deletedRuns: number;
				after: { runs: { total: number; running: number } };
				deletedRunIds?: string[];
			};
			expect(payload.type).toBe("gc");
			expect(payload.deletedRuns).toBe(0);
			expect(payload.after.runs.total).toBe(1);
			expect(payload.after.runs.running).toBe(1);

			const reopenedStore = new CheckpointStore(dbPath);
			try {
				expect(reopenedStore.getRun(runningRun.id).status).toBe("running");
			} finally {
				reopenedStore.close();
			}
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("gc never deletes paused runs", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-gc-paused-"));
		const dbPath = join(dir, "runtime.db");
		const store = new CheckpointStore(dbPath);
		const pausedRun = store.createRun("paused-playbook", {
			inputs: {},
			vars: {},
			memory: {
				working: {},
			},
			tasks: {
				active: {
					status: "waiting_approval",
					attempts: 1,
					approvalId: "approval-1",
				},
			},
			agents: {},
		});
		store.updateRun(pausedRun.id, "paused", pausedRun.snapshot);
		store.close();

		try {
			const gcResult = await runCli([
				"gc",
				"--db",
				dbPath,
				"--older-than-days",
				"0",
				"--keep-runs",
				"0",
				"--output",
				"json",
				"--verbose",
			]);
			expect(gcResult.exitCode).toBe(0);
			expect(gcResult.stderr).toBe("");
			const payload = parseJsonLines(gcResult.stdout)[0] as {
				type: string;
				deletedRuns: number;
				after: { runs: { total: number; paused: number } };
			};
			expect(payload.type).toBe("gc");
			expect(payload.deletedRuns).toBe(0);
			expect(payload.after.runs.total).toBe(1);
			expect(payload.after.runs.paused).toBe(1);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("gc fails for a missing database path instead of creating an empty database", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-gc-missing-"));
		const missingDbPath = join(dir, "missing.db");
		try {
			const result = await runCli(["gc", "--db", missingDbPath]);
			expect(result.exitCode).toBe(1);
			expect(result.stdout).toBe("");
			expect(result.stderr).toContain(`Runtime DB not found: ${missingDbPath}`);
			expect(existsSync(missingDbPath)).toBe(false);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("replay from a succeeded checkpoint does not rerun completed side effects and streams a fresh run", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-replay-succeeded-"));
		const playbookPath = join(dir, "replay.playbook.yaml");
		const dbPath = join(dir, "runtime.db");
		const counterFile = join(dir, "counter.txt");
		writeFileSync(
			playbookPath,
			`playbook: replay-succeeded-checkpoint\n` +
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
			"utf8",
		);

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath]);
			expect(runResult.exitCode).toBe(0);
			const runEvents = parseYamlDocuments(runResult.stdout) as Array<Record<string, unknown>>;
			const finalRun = (runEvents.at(-1) as { run: { id: string } }).run.id;
			const sideEffectCheckpoint = runEvents.find(
				(event) =>
					event.type === "checkpoint" &&
					event.taskId === "side_effect" &&
					(event.task as { status?: string } | undefined)?.status === "succeeded",
			) as { seq: number } | undefined;

			expect(sideEffectCheckpoint).toBeDefined();
			expect(readFileSync(counterFile, "utf8")).toBe("1");

			const replayResult = await runCli(["replay", playbookPath, finalRun, String(sideEffectCheckpoint!.seq), "--db", dbPath]);
			expect(replayResult.exitCode).toBe(0);
			expect(replayResult.stderr).toBe("");
			const replayEvents = parseYamlDocuments(replayResult.stdout) as Array<Record<string, unknown>>;
			expect(replayEvents[0]?.type).toBe("checkpoint");
			expect(replayEvents[0]?.seq).toBe(1);
			const replayFinal = replayEvents.at(-1) as {
				run: {
					id: string;
					status: string;
					snapshot: { tasks: Record<string, { attempts: number; status: string }> };
				};
			};
			expect(replayFinal.run.id).not.toBe(finalRun);
			expect(replayFinal.run.status).toBe("succeeded");
			expect(replayFinal.run.snapshot.tasks.side_effect.attempts).toBe(1);
			expect(replayFinal.run.snapshot.tasks.finalize.status).toBe("succeeded");
			expect(readFileSync(counterFile, "utf8")).toBe("1");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("resume succeeds for an interrupted memory-heavy agent run and preserves prior memory writes", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-resume-memory-"));
		const dbPath = join(dir, "runtime.db");
		const playbookPath = join(dir, "resume-memory.playbook.yaml");
		writeFileSync(
			playbookPath,
			`playbook: resume-memory\n` +
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
		const plan = compilePlaybook(loadPlaybookWithPacks(playbookPath));
		let interruptedRunId = "";

		try {
			const seededRuntime = new PlaybookRuntime(plan, store, {
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

			await expect(seededRuntime.start()).rejects.toThrow("interrupt after first memory turn");
			expect(interruptedRunId).not.toBe("");

			const resumeResult = await runCli([
				"resume",
				playbookPath,
				interruptedRunId,
				"--db",
				dbPath,
				"--output",
				"json",
				"--verbose",
			]);
			expect(resumeResult.exitCode).toBe(0);
			expect(resumeResult.stderr).toBe("");
			const resumeEvents = parseJsonLines(resumeResult.stdout) as Array<Record<string, unknown>>;
			expect(resumeEvents[0]?.type).toBe("checkpoint");
			expect(resumeEvents[0]?.seq).toBeGreaterThan(0);
			const finalEvent = resumeEvents.at(-1) as {
				run: {
					status: string;
					snapshot: { memory: { working: Record<string, unknown> }; tasks: Record<string, { status: string }> };
				};
			};
			expect(finalEvent.run.status).toBe("succeeded");
			expect(finalEvent.run.snapshot.memory.working).toMatchObject({
				finding: "grounded",
				recalled: "grounded",
			});
			expect(finalEvent.run.snapshot.tasks.memory_agent.status).toBe("succeeded");
		} finally {
			store.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("replay from a mid-agent memory checkpoint preserves working memory and forks a fresh run", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-replay-memory-"));
		const dbPath = join(dir, "runtime.db");
		const playbookPath = join(dir, "replay-memory.playbook.yaml");
		writeFileSync(
			playbookPath,
			`playbook: replay-memory\n` +
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

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json", "--verbose"]);
			expect(runResult.exitCode).toBe(0);
			const runEvents = parseJsonLines(runResult.stdout) as Array<Record<string, unknown>>;
			const finalRun = (runEvents.at(-1) as { run: { id: string; snapshot: { memory: { working: Record<string, unknown> } } } }).run;
			const checkpoint = runEvents.find((event) => {
				const task = event.task as { status?: string } | undefined;
				const snapshot = event.snapshot as { agents?: Record<string, { turns?: unknown[] }> } | undefined;
				return (
					event.type === "checkpoint" &&
					event.taskId === "memory_agent" &&
					task?.status === "running" &&
					snapshot?.agents?.memory_agent?.turns?.length === 2
				);
			}) as { seq: number } | undefined;

			expect(checkpoint).toBeDefined();

			const replayResult = await runCli([
				"replay",
				playbookPath,
				finalRun.id,
				String(checkpoint!.seq),
				"--db",
				dbPath,
				"--output",
				"json",
				"--verbose",
			]);
			expect(replayResult.exitCode).toBe(0);
			expect(replayResult.stderr).toBe("");
			const replayEvents = parseJsonLines(replayResult.stdout) as Array<Record<string, unknown>>;
			expect(replayEvents[0]?.type).toBe("checkpoint");
			expect(replayEvents[0]?.seq).toBe(1);
			const replayInitial = replayEvents[0] as {
				snapshot: { agents: Record<string, { turns: unknown[] }>; memory: { working: Record<string, unknown> } };
			};
			const replayFinal = replayEvents.at(-1) as {
				run: { id: string; status: string; snapshot: { memory: { working: Record<string, unknown> } } };
			};
			expect(replayInitial.snapshot.agents.memory_agent.turns).toHaveLength(2);
			expect(replayInitial.snapshot.memory.working.finding).toBe("grounded");
			expect(replayFinal.run.id).not.toBe(finalRun.id);
			expect(replayFinal.run.status).toBe("succeeded");
			expect(replayFinal.run.snapshot.memory.working.recalled).toBe("grounded");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("auth check reports runtime override for provider-backed playbooks in json when requested", async () => {
		const home = mkdtempSync(join(tmpdir(), "agentctl-auth-runtime-"));
		try {
			const { stdout, stderr, exitCode } = await runCli(
				[
					"auth",
					"check",
					"examples/real-autonomy/mission.playbook.yaml",
					"--provider",
					"openai",
					"--api-key",
					"runtime-key",
					"--output",
					"json",
				],
				{ HOME: home, OPENAI_API_KEY: "" },
			);
			expect(exitCode).toBe(0);
			expect(stderr).toBe("");
			expect(parseJsonLines(stdout)).toEqual([
				{
					type: "auth_check",
					ok: true,
					playbook: `${process.cwd()}/examples/real-autonomy/mission.playbook.yaml`,
					providers: [{ provider: "openai", configured: true, source: "runtime_override", issues: [] }],
				},
			]);
		} finally {
			rmSync(home, { recursive: true, force: true });
		}
	});

	test("auth check reports azure openai configuration issues before a run in json when requested", async () => {
		const home = mkdtempSync(join(tmpdir(), "agentctl-auth-azure-"));
		try {
			const { stdout, stderr, exitCode } = await runCli(
				["auth", "check", "--provider", "azure-openai-responses", "--output", "json"],
				{
					HOME: home,
					AZURE_OPENAI_API_KEY: "azure-key",
				},
			);
			expect(exitCode).toBe(1);
			expect(stderr).toBe("");
			expect(parseJsonLines(stdout)).toEqual([
				{
					type: "auth_check",
					ok: false,
					providers: [
						{
							provider: "azure-openai-responses",
							configured: false,
							source: "env",
							issues: ['Azure OpenAI requires "endpoint" or "baseUrl"', 'Azure OpenAI requires "apiVersion"'],
						},
					],
				},
			]);
		} finally {
			rmSync(home, { recursive: true, force: true });
		}
	});

	test("approval CLI lists, shows, resolves, and resumes paused runs", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-approvals-"));
		const dbPath = join(dir, "runtime.db");
		const playbookPath = join(dir, "approval.playbook.yaml");
		const targetFile = join(dir, "approved.txt");
		writeFileSync(
			playbookPath,
			`playbook: approval-cli\n` +
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
				`      path: ./approved.txt\n` +
				`      content: approved\n`,
			"utf8",
		);

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json", "--verbose"]);
			expect(runResult.exitCode).toBe(0);
			const runEvents = parseJsonLines(runResult.stdout) as Array<Record<string, unknown>>;
			const pausedResult = runEvents.at(-1) as {
				run: {
					id: string;
					status: string;
					snapshot: { tasks: Record<string, { approvalId?: string; status: string }> };
				};
			};
			expect(pausedResult.run.status).toBe("paused");
			const approvalId = pausedResult.run.snapshot.tasks.write_note.approvalId;
			expect(approvalId).toBeTruthy();
			expect(existsSync(targetFile)).toBe(false);

			const listResult = await runCli(["approvals", "list", "--db", dbPath, "--output", "json"]);
			expect(listResult.exitCode).toBe(0);
			expect(parseJsonLines(listResult.stdout)).toEqual([
				expect.objectContaining({
					type: "approval_list",
					count: 1,
					approvals: [
						expect.objectContaining({
							id: approvalId,
							runId: pausedResult.run.id,
							taskId: "write_note",
							status: "pending",
							toolRef: "builtin.write",
						}),
					],
				}),
			]);

			const showResult = await runCli(["approvals", "show", approvalId!, "--db", dbPath, "--output", "json"]);
			expect(showResult.exitCode).toBe(0);
			expect(parseJsonLines(showResult.stdout)).toEqual([
				expect.objectContaining({
					type: "approval_show",
					approval: expect.objectContaining({
						id: approvalId,
						status: "pending",
						requestInput: {
							path: "./approved.txt",
							content: "approved",
						},
					}),
				}),
			]);

			const approveResult = await runCli([
				"approvals",
				"approve",
				approvalId!,
				"--db",
				dbPath,
				"--by",
				"tester",
				"--note",
				"approved for runtime test",
				"--output",
				"json",
			]);
			expect(approveResult.exitCode).toBe(0);
			expect(parseJsonLines(approveResult.stdout)).toEqual([
				expect.objectContaining({
					type: "approval_approve",
					approval: expect.objectContaining({
						id: approvalId,
						status: "approved",
						resolvedBy: "tester",
						resolutionNote: "approved for runtime test",
					}),
				}),
			]);

			const resumeResult = await runCli(["resume", playbookPath, pausedResult.run.id, "--db", dbPath, "--output", "json"]);
			expect(resumeResult.exitCode).toBe(0);
			const resumeEvents = parseJsonLines(resumeResult.stdout) as Array<Record<string, unknown>>;
			const finalResult = resumeEvents.at(-1) as { run: { status: string; snapshot: { tasks: Record<string, { status: string }> } } };
			expect(finalResult.run.status).toBe("succeeded");
			expect(finalResult.run.snapshot.tasks.write_note.status).toBe("succeeded");
			expect(readFileSync(targetFile, "utf8")).toBe("approved");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("approval CLI reject marks the blocked task failed on resume", async () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-cli-approval-reject-"));
		const dbPath = join(dir, "runtime.db");
		const playbookPath = join(dir, "approval-reject.playbook.yaml");
		const targetFile = join(dir, "rejected.txt");
		writeFileSync(
			playbookPath,
			`playbook: approval-reject-cli\n` +
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
				`      path: ./rejected.txt\n` +
				`      content: rejected\n`,
			"utf8",
		);

		try {
			const runResult = await runCli(["run", playbookPath, "--db", dbPath, "--output", "json"]);
			expect(runResult.exitCode).toBe(0);
			const pausedResult = parseJsonLines(runResult.stdout).at(-1) as {
				run: { id: string; snapshot: { tasks: Record<string, { approvalId?: string }> } };
			};
			const approvalId = pausedResult.run.snapshot.tasks.write_note.approvalId;
			expect(approvalId).toBeTruthy();

			const rejectResult = await runCli([
				"approvals",
				"reject",
				approvalId!,
				"--db",
				dbPath,
				"--by",
				"tester",
				"--note",
				"rejected for runtime test",
				"--output",
				"json",
			]);
			expect(rejectResult.exitCode).toBe(0);
			expect(parseJsonLines(rejectResult.stdout)).toEqual([
				expect.objectContaining({
					type: "approval_reject",
					approval: expect.objectContaining({
						id: approvalId,
						status: "rejected",
						resolvedBy: "tester",
						resolutionNote: "rejected for runtime test",
					}),
				}),
			]);

			const resumeResult = await runCli(["resume", playbookPath, pausedResult.run.id, "--db", dbPath, "--output", "json", "--verbose"]);
			expect(resumeResult.exitCode).toBe(0);
			const finalResult = parseJsonLines(resumeResult.stdout).at(-1) as {
				run: { status: string; snapshot: { tasks: Record<string, { status: string; error?: string }> } };
			};
			expect(finalResult.run.status).toBe("failed");
			expect(finalResult.run.snapshot.tasks.write_note.status).toBe("failed");
			expect(finalResult.run.snapshot.tasks.write_note.error).toContain("Tool call rejected");
			expect(existsSync(targetFile)).toBe(false);
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
