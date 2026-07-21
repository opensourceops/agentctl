#!/usr/bin/env node
import { createInterface } from "node:readline/promises";
import { homedir } from "node:os";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import YAML from "yaml";
import { AuthStorage } from "./auth-storage.js";
import { runInteractiveApprovalLoop } from "./cli-approval-loop.js";
import { getCommandDoc, renderCommandHelp, renderRootHelp } from "./cli-docs.js";
import { compilePlaybook } from "./compiler.js";
import { loadPlaybookWithPacks } from "./parser.js";
import { CheckpointStore } from "./checkpoint-store.js";
import { createLongTermMemoryAdapter } from "./long-term-memory-adapters/factory.js";
import type { LongTermMemoryAdapterKind } from "./long-term-memory-adapters/types.js";
import type { LongTermMemoryStats } from "./long-term-memory.js";
import type { LongTermMemoryEntry } from "./long-term-memory.js";
import { checkPlaybook } from "./playbook-check.js";
import { inspectPromptCacheSupport, resolvePromptCacheConfig } from "./prompt-cache.js";
import { PlaybookRuntime } from "./runtime.js";
import type {
	AgentDefinition,
	ApprovalRecord,
	CheckpointRecord,
	ExecutionResult,
	JsonValue,
	OutputColorMode,
	OutputFormat,
	RunRecord,
	RuntimeSnapshot,
	TaskOutput,
	TaskState,
} from "./types.js";
import { ModelRegistry } from "./model-registry.js";

type MemoryProviderFlag = "sqlite" | "mongodb-atlas";

function getVersion(): string {
	const packageJsonPath = new URL("../package.json", import.meta.url);
	const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as { version?: string };
	if (!packageJson.version) {
		throw new Error("package.json is missing version");
	}
	return packageJson.version;
}

function printCommandHelp(name: string): void {
	const command = getCommandDoc(name);
	if (!command) {
		throw new Error(`Unknown command "${name}"`);
	}
	console.log(renderCommandHelp(command));
}

function printUpdateInstructions(): void {
	console.log(`agentctl update

This build is intended to run from a source checkout.
Update the local checkout with:
  git pull --rebase
  npm install`);
}

function getFlag(name: string, args: string[], fallback: string): string {
	const index = args.indexOf(name);
	if (index === -1 || index === args.length - 1) return fallback;
	return args[index + 1]!;
}

function isProviderBackedAgent(definition: AgentDefinition): boolean {
	return definition.kind === "openai.responses";
}

function getCommandArg(args: string[], index: number): string | undefined {
	const value = args[index];
	if (!value || value.startsWith("-")) {
		return undefined;
	}
	return value;
}

function isHelpArg(value: string | undefined): boolean {
	return value === "-h" || value === "--help" || value === "help";
}

function isOutputFormat(value: string): value is OutputFormat {
	return value === "yaml" || value === "json";
}

class OutputWriter {
	private yamlDocumentCount = 0;

	constructor(
		private readonly format: OutputFormat,
		private readonly colorMode: OutputColorMode,
		private readonly isTty: boolean,
	) {}

	write(value: unknown): void {
		if (this.format === "json") {
			process.stdout.write(`${JSON.stringify(value)}\n`);
			return;
		}
		if (this.yamlDocumentCount > 0) {
			process.stdout.write(this.shouldColorize() ? `${Ansi.Dim}---${Ansi.Reset}\n` : "---\n");
		}
		const yaml = YAML.stringify(value);
		process.stdout.write(this.shouldColorize() ? colorizeYaml(yaml) : yaml);
		this.yamlDocumentCount += 1;
	}

	private shouldColorize(): boolean {
		if (this.format !== "yaml") {
			return false;
		}
		if (this.colorMode === "always") {
			return true;
		}
		if (this.colorMode === "never") {
			return false;
		}
		return this.isTty;
	}
}

const Ansi = {
	Reset: "\u001b[0m",
	Bold: "\u001b[1m",
	Dim: "\u001b[2m",
	Cyan: "\u001b[36m",
	Blue: "\u001b[34m",
	Magenta: "\u001b[35m",
	Green: "\u001b[32m",
	Yellow: "\u001b[33m",
	Red: "\u001b[31m",
} as const;

function isVerboseRequested(args: string[]): boolean {
	return args.includes("--verbose") || args.includes("-v");
}

function isOutputColorMode(value: string): value is OutputColorMode {
	return value === "auto" || value === "always" || value === "never";
}

function getRequestedColorMode(args: string[]): OutputColorMode | undefined {
	const value = getFlag("--color", args, "");
	if (!value) {
		return undefined;
	}
	if (!isOutputColorMode(value)) {
		throw new Error(`Unsupported color mode "${value}"`);
	}
	return value;
}

function summarizeTaskOutput(output: TaskOutput | undefined): Record<string, JsonValue> | undefined {
	if (!output) {
		return undefined;
	}
	const summary: Record<string, JsonValue> = {};
	if (typeof output.finalText === "string") {
		summary.finalTextPreview = output.finalText.length > 240 ? `${output.finalText.slice(0, 240)}...` : output.finalText;
	}
	if (typeof output.stdout === "string") {
		summary.stdout = output.stdout;
	}
	if (typeof output.stderr === "string" && output.stderr.length > 0) {
		summary.stderr = output.stderr;
	}
	if (typeof output.path === "string") {
		summary.path = output.path;
	}
	if (typeof output.bytesWritten === "number") {
		summary.bytesWritten = output.bytesWritten;
	}
	if (typeof output.exitCode === "number") {
		summary.exitCode = output.exitCode;
	}
	if (typeof output.ok === "boolean") {
		summary.ok = output.ok;
	}
	if (typeof output.message === "string") {
		summary.message = output.message;
	}
	if (typeof output.mode === "string") {
		summary.mode = output.mode;
	}
	return Object.keys(summary).length > 0 ? summary : undefined;
}

function summarizeTaskState(taskState: TaskState): Record<string, JsonValue> {
	return {
		status: taskState.status,
		attempts: taskState.attempts,
		...(taskState.approvalId ? { approvalId: taskState.approvalId } : {}),
		...(taskState.error ? { error: taskState.error } : {}),
		...(summarizeTaskOutput(taskState.output) ? { output: summarizeTaskOutput(taskState.output)! } : {}),
	};
}

function summarizeSnapshot(snapshot: RuntimeSnapshot): Record<string, JsonValue> {
	return {
		inputs: snapshot.inputs,
		vars: snapshot.vars,
		tasks: Object.fromEntries(
			Object.entries(snapshot.tasks).map(([taskId, taskState]) => [taskId, summarizeTaskState(taskState)]),
		),
		agentSessions: Object.keys(snapshot.agents).length,
	};
}

function getRequestedOutputFormat(args: string[]): OutputFormat | undefined {
	const value = getFlag("--output", args, "");
	if (!value) {
		return undefined;
	}
	if (!isOutputFormat(value)) {
		throw new Error(`Unsupported output format "${value}"`);
	}
	return value;
}

function getIntegerFlag(name: string, args: string[], fallback: number): number {
	const value = getFlag(name, args, "");
	if (!value) {
		return fallback;
	}
	const parsed = Number(value);
	if (!Number.isInteger(parsed) || parsed < 0) {
		throw new Error(`Flag ${name} requires a non-negative integer`);
	}
	return parsed;
}

function getDefaultDbPath(): string {
	return join(homedir(), ".agentctl", "runtime", "runtime.db");
}

function getDefaultMemoryDbPath(): string {
	return join(homedir(), ".agentctl", "memory", "long-term.db");
}

function getMemoryProvider(args: string[]): MemoryProviderFlag {
	const value = getFlag("--provider", args, "sqlite");
	if (value === "sqlite" || value === "mongodb-atlas") {
		return value;
	}
	throw new Error(`Unsupported memory provider "${value}"`);
}

function getOptionalStringFlag(name: string, args: string[]): string | undefined {
	const value = getFlag(name, args, "");
	return value === "" ? undefined : value;
}

function parseMemoryValue(args: string[]): JsonValue {
	const jsonValue = getOptionalStringFlag("--value", args);
	const stringValue = getOptionalStringFlag("--string", args);
	if (jsonValue && stringValue) {
		throw new Error('memory write accepts either "--value" or "--string", not both');
	}
	if (stringValue !== undefined) {
		return stringValue;
	}
	if (jsonValue === undefined) {
		throw new Error('memory write requires "--value" or "--string"');
	}
	try {
		return JSON.parse(jsonValue) as JsonValue;
	} catch (error) {
		throw new Error(`Invalid JSON for --value: ${error instanceof Error ? error.message : String(error)}`);
	}
}

function parseTags(args: string[]): string[] {
	const value = getOptionalStringFlag("--tags", args);
	if (!value) {
		return [];
	}
	return value
		.split(",")
		.map((tag) => tag.trim())
		.filter((tag) => tag.length > 0);
}

function createDbStatsPayload(
	stats: ReturnType<CheckpointStore["getStats"]>,
	verbose: boolean,
): Record<string, unknown> {
	return {
		type: "db_stats",
		dbPath: stats.dbPath,
		fileSizeBytes: stats.fileSizeBytes,
		runs: stats.runs,
		records: stats.records,
		...(stats.latestRun ? { latestRun: stats.latestRun } : {}),
		...(verbose ? { fileSizeKiB: Number((stats.fileSizeBytes / 1024).toFixed(2)) } : {}),
	};
}

function createMemoryGetPayload(input: {
	dbPath: string;
	namespace?: string;
	key: string;
	limit: number;
	matches: LongTermMemoryEntry[];
}): Record<string, unknown> {
	return {
		type: "memory_get",
		dbPath: input.dbPath,
		...(input.namespace ? { namespace: input.namespace } : { namespace: null }),
		key: input.key,
		limit: input.limit,
		found: input.matches.length > 0,
		matchCount: input.matches.length,
		matches: input.matches,
	};
}

function createMemorySearchPayload(input: {
	dbPath: string;
	namespace?: string;
	query?: string;
	key?: string;
	limit: number;
	matches: LongTermMemoryEntry[];
}): Record<string, unknown> {
	return {
		type: "memory_search",
		dbPath: input.dbPath,
		...(input.namespace ? { namespace: input.namespace } : { namespace: null }),
		...(input.query ? { query: input.query } : { query: null }),
		...(input.key ? { key: input.key } : { key: null }),
		limit: input.limit,
		matchCount: input.matches.length,
		matches: input.matches,
	};
}

function createMemoryWritePayload(input: {
	dbPath: string;
	namespace: string;
	entry: LongTermMemoryEntry;
}): Record<string, unknown> {
	return {
		type: "memory_write",
		dbPath: input.dbPath,
		namespace: input.namespace,
		entry: input.entry,
	};
}

function createMemoryStatsPayload(
	stats: LongTermMemoryStats,
	verbose: boolean,
): Record<string, unknown> {
	return {
		type: "memory_stats",
		dbPath: stats.dbPath,
		fileSizeBytes: stats.fileSizeBytes,
		totalEntries: stats.totalEntries,
		totalNamespaces: stats.totalNamespaces,
		oldestCreatedAt: stats.oldestCreatedAt,
		newestUpdatedAt: stats.newestUpdatedAt,
		...(stats.namespace ? { namespace: stats.namespace } : {}),
		namespaces: stats.namespaces,
		...(verbose && typeof stats.fileSizeBytes === "number"
			? { fileSizeKiB: Number((stats.fileSizeBytes / 1024).toFixed(2)) }
			: {}),
	};
}

function createMemoryGcPayload(input: {
	provider: LongTermMemoryAdapterKind;
	before: LongTermMemoryStats;
	after: LongTermMemoryStats;
	deletedEntries: Array<{ namespace: string; key: string; updatedAt: string }>;
	olderThanDays: number;
	keepEntries: number;
	vacuumed: boolean;
	verbose: boolean;
}): Record<string, unknown> {
	return {
		type: "memory_gc",
		provider: input.provider,
		olderThanDays: input.olderThanDays,
		keepEntries: input.keepEntries,
		deletedEntries: input.deletedEntries.length,
		vacuumed: input.vacuumed,
		before: input.before,
		after: input.after,
		...(input.verbose ? { deletedKeys: input.deletedEntries } : {}),
	};
}

function createGcPayload(input: {
	dbPath: string;
	olderThanDays: number;
	keepRuns: number;
	before: ReturnType<CheckpointStore["getStats"]>;
	after: ReturnType<CheckpointStore["getStats"]>;
	deletedRunIds: string[];
	vacuumed: boolean;
	verbose: boolean;
}): Record<string, unknown> {
	return {
		type: "gc",
		dbPath: input.dbPath,
		olderThanDays: input.olderThanDays,
		keepRuns: input.keepRuns,
		deletedRuns: input.deletedRunIds.length,
		vacuumed: input.vacuumed,
		before: {
			fileSizeBytes: input.before.fileSizeBytes,
			runs: input.before.runs,
			records: input.before.records,
		},
		after: {
			fileSizeBytes: input.after.fileSizeBytes,
			runs: input.after.runs,
			records: input.after.records,
		},
		...(input.verbose ? { deletedRunIds: input.deletedRunIds } : {}),
	};
}

function isApprovalStatus(value: string): value is ApprovalRecord["status"] {
	return value === "pending" || value === "approved" || value === "rejected";
}

function createApprovalListPayload(approvals: ApprovalRecord[]): Record<string, unknown> {
	return {
		type: "approval_list",
		count: approvals.length,
		approvals,
	};
}

function createApprovalPayload(
	type: "approval_show" | "approval_approve" | "approval_reject",
	approval: ApprovalRecord,
): Record<string, unknown> {
	return {
		type,
		approval,
	};
}

function createInteractiveApprovalPrompt(): {
	ask(approval: ApprovalRecord): Promise<Extract<ApprovalRecord["status"], "approved" | "rejected">>;
	close(): Promise<void>;
} {
	const rl = createInterface({
		input: process.stdin,
		output: process.stderr,
	});
	return {
		async ask(approval) {
			const context = [
				"",
				`Approval required for run ${approval.runId}, task ${approval.taskId}.`,
				`Tool: ${approval.toolRef} (${approval.capability}, risk=${approval.risk})`,
				`Reason: ${approval.reason}`,
				"Input:",
				YAML.stringify(approval.requestInput).trimEnd(),
			].join("\n");
			process.stderr.write(`${context}\n`);
			while (true) {
				const answer = (await rl.question("Approve? [y/n]: ")).trim().toLowerCase();
				if (answer === "y" || answer === "yes") {
					return "approved";
				}
				if (answer === "n" || answer === "no") {
					return "rejected";
				}
				process.stderr.write('Please answer "y" or "n".\n');
			}
		},
		async close() {
			rl.close();
		},
	};
}

function createPromptCacheStatsPayload(
	stats: ReturnType<CheckpointStore["getPromptCacheStats"]>,
): Record<string, unknown> {
	return {
		type: "prompt_cache_stats",
		dbPath: stats.dbPath,
		totalResponses: stats.totalResponses,
		hitResponses: stats.hitResponses,
		totalCachedTokens: stats.totalCachedTokens,
		totalInputTokens: stats.totalInputTokens,
		totalUncachedInputTokens: stats.totalUncachedInputTokens,
		totalOutputTokens: stats.totalOutputTokens,
		latestResponseAt: stats.latestResponseAt,
		providers: stats.providers,
		agents: stats.agents,
		runs: stats.runs,
		...(stats.responses ? { responses: stats.responses } : {}),
	};
}

function createPromptCacheExplainPayload(input: {
	playbook: string;
	agents: Array<Record<string, unknown>>;
}): Record<string, unknown> {
	return {
		type: "prompt_cache_explain",
		playbook: input.playbook,
		agents: input.agents,
	};
}

function createCheckpointPayload(checkpoint: CheckpointRecord, verbose: boolean): Record<string, unknown> {
	const taskId = checkpoint.taskId;
	const task = taskId ? checkpoint.snapshot.tasks[taskId] : undefined;
	return {
		type: "checkpoint",
		runId: checkpoint.runId,
		seq: checkpoint.seq,
		status: checkpoint.status,
		createdAt: checkpoint.createdAt,
		...(taskId
			? {
					taskId,
					task: task ? (verbose ? task : summarizeTaskState(task)) : undefined,
				}
			: {}),
		...(verbose ? { snapshot: checkpoint.snapshot } : {}),
	};
}

function summarizeRun(run: RunRecord): Record<string, unknown> {
	return {
		id: run.id,
		playbookName: run.playbookName,
		status: run.status,
		traceId: run.traceId,
		createdAt: run.createdAt,
		updatedAt: run.updatedAt,
	};
}

function colorizeYaml(yaml: string): string {
	return yaml
		.split("\n")
		.map((line) => colorizeYamlLine(line))
		.join("\n");
}

function colorizeYamlLine(line: string): string {
	if (line.length === 0) {
		return line;
	}
	const keyMatch = /^(\s*)([^:\n]+):(.*)$/.exec(line);
	if (!keyMatch) {
		return line;
	}
	const [, indent, key, remainder] = keyMatch;
	if (key === undefined || remainder === undefined) {
		return line;
	}
	const coloredKey = `${Ansi.Cyan}${key}${Ansi.Reset}`;
	const trimmedRemainder = remainder.trimStart();
	if (trimmedRemainder.length === 0) {
		return `${indent}${coloredKey}:`;
	}
	return `${indent}${coloredKey}:${colorizeYamlValue(trimmedRemainder)}`;
}

function colorizeYamlValue(value: string): string {
	if (value === " running" || value === "running") {
		return ` ${Ansi.Yellow}running${Ansi.Reset}`;
	}
	if (value === " succeeded" || value === "succeeded") {
		return ` ${Ansi.Green}succeeded${Ansi.Reset}`;
	}
	if (value === " failed" || value === "failed") {
		return ` ${Ansi.Red}failed${Ansi.Reset}`;
	}
	if (value === " paused" || value === "paused") {
		return ` ${Ansi.Magenta}paused${Ansi.Reset}`;
	}
	if (value === " waiting_approval" || value === "waiting_approval") {
		return ` ${Ansi.Magenta}waiting_approval${Ansi.Reset}`;
	}
	if (value === " checkpoint" || value === "checkpoint") {
		return ` ${Ansi.Magenta}checkpoint${Ansi.Reset}`;
	}
	if (value === " result" || value === "result") {
		return ` ${Ansi.Blue}result${Ansi.Reset}`;
	}
	if (/^\s*[0-9]+\s*$/.test(value)) {
		return ` ${Ansi.Blue}${value.trim()}${Ansi.Reset}`;
	}
	if (/^\s*[0-9a-f]{8}-[0-9a-f-]{27}\s*$/i.test(value)) {
		return ` ${Ansi.Dim}${value.trim()}${Ansi.Reset}`;
	}
	return value;
}

function createResultPayload(result: ExecutionResult, verbose: boolean): Record<string, unknown> {
	return {
		type: "result",
		run: verbose
			? result.run
			: {
					...summarizeRun(result.run),
					snapshot: summarizeSnapshot(result.run.snapshot),
				},
		latestCheckpoint: verbose
			? result.latestCheckpoint
			: {
					runId: result.latestCheckpoint.runId,
					seq: result.latestCheckpoint.seq,
					status: result.latestCheckpoint.status,
					createdAt: result.latestCheckpoint.createdAt,
				},
	};
}

function createCheckPayload(result: ReturnType<typeof checkPlaybook>): Record<string, unknown> {
	return {
		type: "check",
		ok: result.ok,
		playbook: result.playbook,
		packs: result.packs,
		...(result.ok ? { compiled: true } : {}),
		...(result.diagnostics.length > 0 ? { diagnostics: result.diagnostics } : {}),
	};
}

async function main(): Promise<void> {
	const args = process.argv.slice(2);
	const command = args[0];

	if (!command || command === "help" || command === "-h" || command === "--help") {
		if (command === "help" && args[1]) {
			printCommandHelp(args[1]);
			return;
		}
		console.log(renderRootHelp());
		return;
	}

	if (command === "version" || command === "-V" || command === "--version") {
		console.log(getVersion());
		return;
	}

	const runtimeApiKey = getFlag("--api-key", args, "");
	const runtimeApiProvider = getFlag("--provider", args, "openai");
	const requestedOutputFormat = getRequestedOutputFormat(args);
	const requestedColorMode = getRequestedColorMode(args);
	const verbose = isVerboseRequested(args);
	const dbPath = resolve(getFlag("--db", args, getDefaultDbPath()));

	if (command === "update") {
		if (isHelpArg(args[1])) {
			printCommandHelp("update");
			return;
		}
		printUpdateInstructions();
		return;
	}

	if (command === "check") {
		const playbookPath = args[1];
		if (isHelpArg(playbookPath)) {
			printCommandHelp("check");
			return;
		}
		if (!playbookPath) {
			printCommandHelp("check");
			process.exit(1);
		}
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		const result = checkPlaybook(resolve(playbookPath));
		output.write(createCheckPayload(result));
		if (!result.ok) {
			process.exit(1);
		}
		return;
	}

	if (command === "schema") {
		if (isHelpArg(args[1])) {
			printCommandHelp("schema");
			return;
		}
		console.log("Playbook DSL: playbook, packs, defaults.agentProfile, output.format, policy, modules, agents.tools, tasks, retry");
		return;
	}

	const authStorage = AuthStorage.create();
	if (runtimeApiKey) {
		authStorage.setRuntimeApiKey(runtimeApiProvider, runtimeApiKey);
	}

	if (command === "db") {
		const subcommand = args[1];
		if (isHelpArg(subcommand) || !subcommand) {
			printCommandHelp("db");
			return;
		}
		if (subcommand !== "stats") {
			throw new Error(`Unknown db subcommand "${subcommand ?? ""}"`);
		}
		if (isHelpArg(args[2])) {
			printCommandHelp("db");
			return;
		}
		if (!existsSync(dbPath)) {
			throw new Error(`Runtime DB not found: ${dbPath}`);
		}
		const store = new CheckpointStore(dbPath);
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		try {
			output.write(createDbStatsPayload(store.getStats(), verbose));
			return;
		} finally {
			store.close();
		}
	}

	if (command === "approvals") {
		const subcommand = args[1];
		if (isHelpArg(subcommand) || !subcommand) {
			printCommandHelp("approvals");
			return;
		}
		if (isHelpArg(args[2])) {
			printCommandHelp("approvals");
			return;
		}
		if (!existsSync(dbPath)) {
			throw new Error(`Runtime DB not found: ${dbPath}`);
		}
		const store = new CheckpointStore(dbPath);
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		try {
			if (subcommand === "list") {
				const statusValue = getOptionalStringFlag("--status", args);
				if (statusValue && !isApprovalStatus(statusValue)) {
					throw new Error(`Unsupported approval status "${statusValue}"`);
				}
				const runId = getOptionalStringFlag("--run-id", args);
				const filters: { runId?: string; status?: ApprovalRecord["status"] } = {};
				if (runId) {
					filters.runId = runId;
				}
				if (statusValue) {
					filters.status = statusValue as ApprovalRecord["status"];
				}
				output.write(
					createApprovalListPayload(
						store.listApprovals(filters),
					),
				);
				return;
			}
			if (subcommand === "show") {
				const approvalId = getCommandArg(args, 2);
				if (!approvalId) {
					throw new Error("approvals show requires an approval id");
				}
				output.write(createApprovalPayload("approval_show", store.getApproval(approvalId)));
				return;
			}
			if (subcommand === "approve" || subcommand === "reject") {
				const approvalId = getCommandArg(args, 2);
				if (!approvalId) {
					throw new Error(`approvals ${subcommand} requires an approval id`);
				}
				const nextStatus: Extract<ApprovalRecord["status"], "approved" | "rejected"> =
					subcommand === "approve" ? "approved" : "rejected";
				const resolveOptions: { resolvedBy?: string; resolutionNote?: string } = {};
				const resolvedBy = getOptionalStringFlag("--by", args);
				const resolutionNote = getOptionalStringFlag("--note", args);
				if (resolvedBy) {
					resolveOptions.resolvedBy = resolvedBy;
				}
				if (resolutionNote) {
					resolveOptions.resolutionNote = resolutionNote;
				}
				output.write(
					createApprovalPayload(
						subcommand === "approve" ? "approval_approve" : "approval_reject",
						store.resolveApproval(approvalId, nextStatus, resolveOptions),
					),
				);
				return;
			}
			throw new Error(`Unknown approvals subcommand "${subcommand}"`);
		} finally {
			store.close();
		}
	}

	if (command === "prompt-cache") {
		const subcommand = args[1];
		if (isHelpArg(subcommand) || !subcommand) {
			printCommandHelp("prompt-cache");
			return;
		}
		if (subcommand !== "stats" && subcommand !== "explain") {
			throw new Error(`Unknown prompt-cache subcommand "${subcommand ?? ""}"`);
		}
		if (isHelpArg(args[2])) {
			printCommandHelp("prompt-cache");
			return;
		}
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		if (subcommand === "stats") {
			if (!existsSync(dbPath)) {
				throw new Error(`Runtime DB not found: ${dbPath}`);
			}
			const store = new CheckpointStore(dbPath);
			try {
				const runId = getOptionalStringFlag("--run-id", args);
				const taskId = getOptionalStringFlag("--task-id", args);
				const agentRef = getOptionalStringFlag("--agent-ref", args);
				output.write(
					createPromptCacheStatsPayload(
						store.getPromptCacheStats({
							...(runId ? { runId } : {}),
							...(taskId ? { taskId } : {}),
							...(agentRef ? { agentRef } : {}),
							verbose,
						}),
					),
				);
				return;
			} finally {
				store.close();
			}
		}

		const playbookPath = getCommandArg(args, 2);
		if (!playbookPath) {
			printCommandHelp("prompt-cache");
			process.exit(1);
		}
		const plan = compilePlaybook(loadPlaybookWithPacks(resolve(playbookPath)));
		const agentFilter = getOptionalStringFlag("--agent-ref", args);
		const initialSnapshot: RuntimeSnapshot = {
			inputs: plan.inputs,
			vars: { ...plan.memory.working.initial },
			memory: { working: { ...plan.memory.working.initial } },
			tasks: Object.fromEntries(
				plan.tasks.map((task) => [task.id, { status: "pending" as const, attempts: 0 }]),
			),
			agents: {},
		};
		const explanations = plan.tasks
			.filter((task) => task.use.kind === "agent")
			.filter((task) => !agentFilter || task.use.ref === agentFilter)
			.map((task) => {
				const agent = plan.agents[task.use.ref]!;
				const support = inspectPromptCacheSupport({
					agent,
					agentRef: task.use.ref,
					playbookPromptCache: plan.promptCache,
				});
				const resolved = resolvePromptCacheConfig({
					playbook: plan,
					agent,
					agentRef: task.use.ref,
					runId: "<run-id>",
					snapshot: initialSnapshot,
					taskInput: task.with,
				});
				return {
					taskId: task.id,
					agentRef: task.use.ref,
					kind: agent.kind,
					provider: support.provider,
					model: agent.model ?? null,
					baseUrl: support.baseUrl ?? null,
					directOpenAiBaseUrl: support.directOpenAiBaseUrl,
					requested: support.requested,
					enabled: resolved.enabled,
					eligible: support.eligible,
					force: support.force,
					reason: support.reason,
					effective: {
						retention: resolved.retention,
						keyScope: resolved.keyScope,
						shareMode: resolved.shareMode,
						group: resolved.group ?? null,
						keyTemplate: resolved.keyTemplate ?? null,
						keyBasePreview: resolved.keyBase ?? null,
					},
				};
			});
		output.write(
			createPromptCacheExplainPayload({
				playbook: resolve(playbookPath),
				agents: explanations,
			}),
		);
		if (explanations.length === 0) {
			process.exit(1);
		}
		return;
	}

	if (command === "memory") {
		const subcommand = args[1];
		if (isHelpArg(subcommand) || !subcommand) {
			printCommandHelp("memory");
			return;
		}
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		const memoryProvider = getMemoryProvider(args);
		const memoryDbPath = resolve(getFlag("--db", args, getDefaultMemoryDbPath()));
		const namespace = getOptionalStringFlag("--namespace", args);
		const limit = getIntegerFlag("--limit", args, 50);
		const olderThanDays = getIntegerFlag("--older-than-days", args, 30);
		const keepEntries = getIntegerFlag("--keep-entries", args, 100);
		const openMemoryAdapter = () =>
			createLongTermMemoryAdapter({
				provider: memoryProvider,
				...(memoryProvider === "sqlite" ? { dbPath: memoryDbPath } : {}),
				...(getOptionalStringFlag("--connection-string", args) ? { connectionString: getOptionalStringFlag("--connection-string", args)! } : {}),
				database: getOptionalStringFlag("--database", args) ?? "agentctl",
				collection: getOptionalStringFlag("--collection", args) ?? "long_term_memories",
			});

		if (subcommand === "write") {
			const key = getCommandArg(args, 2);
			if (isHelpArg(key)) {
				printCommandHelp("memory");
				return;
			}
			if (!key) {
				throw new Error("memory write requires a key");
			}
			const effectiveNamespace = namespace ?? "default";
			const value = parseMemoryValue(args);
			const tags = parseTags(args);
			const store = openMemoryAdapter();
			try {
				output.write(
					createMemoryWritePayload({
						dbPath: memoryProvider === "sqlite" ? memoryDbPath : `mongodb-atlas://${getOptionalStringFlag("--database", args) ?? "agentctl"}/${getOptionalStringFlag("--collection", args) ?? "long_term_memories"}`,
						namespace: effectiveNamespace,
						entry: await store.write(effectiveNamespace, key, value, tags),
					}),
				);
				return;
			} finally {
				await store.close();
			}
		}

		if (memoryProvider === "sqlite" && !existsSync(memoryDbPath)) {
			throw new Error(`Memory DB not found: ${memoryDbPath}`);
		}
		const store = openMemoryAdapter();
		try {
			if (subcommand === "get") {
				const key = getCommandArg(args, 2);
				if (isHelpArg(key)) {
					printCommandHelp("memory");
					return;
				}
				if (!key) {
					throw new Error("memory get requires a key");
				}
				const matches = (await store.search(namespace, undefined, key, limit)).entries;
				output.write(
					createMemoryGetPayload({
						dbPath: memoryProvider === "sqlite" ? memoryDbPath : `mongodb-atlas://${getOptionalStringFlag("--database", args) ?? "agentctl"}/${getOptionalStringFlag("--collection", args) ?? "long_term_memories"}`,
						...(namespace ? { namespace } : {}),
						key,
						limit,
						matches,
					}),
				);
				return;
			}
			if (subcommand === "search") {
				if (isHelpArg(args[2])) {
					printCommandHelp("memory");
					return;
				}
				const query = getOptionalStringFlag("--query", args);
				const key = getOptionalStringFlag("--key", args);
				const matches = (await store.search(namespace, query, key, limit)).entries;
				output.write(
					createMemorySearchPayload({
						dbPath: memoryProvider === "sqlite" ? memoryDbPath : `mongodb-atlas://${getOptionalStringFlag("--database", args) ?? "agentctl"}/${getOptionalStringFlag("--collection", args) ?? "long_term_memories"}`,
						...(namespace ? { namespace } : {}),
						...(query ? { query } : {}),
						...(key ? { key } : {}),
						limit,
						matches,
					}),
				);
				return;
			}
			if (subcommand === "stats") {
				if (isHelpArg(args[2])) {
					printCommandHelp("memory");
					return;
				}
				output.write(createMemoryStatsPayload(await store.getStats(namespace), verbose));
				return;
			}
			if (subcommand === "gc") {
				if (isHelpArg(args[2])) {
					printCommandHelp("memory");
					return;
				}
				const before = await store.getStats(namespace);
				const result = await store.garbageCollect({
					...(namespace ? { namespace } : {}),
					olderThanDays,
					keepEntries,
					vacuum: true,
				});
				const after = await store.getStats(namespace);
				output.write(
					createMemoryGcPayload({
						provider: memoryProvider,
						before,
						after,
						deletedEntries: result.deletedEntries,
						olderThanDays,
						keepEntries,
						vacuumed: result.vacuumed,
						verbose,
					}),
				);
				return;
			}
			throw new Error(`Unknown memory subcommand "${subcommand}"`);
		} finally {
			await store.close();
		}
	}

	if (command === "gc") {
		if (isHelpArg(args[1])) {
			printCommandHelp("gc");
			return;
		}
		if (!existsSync(dbPath)) {
			throw new Error(`Runtime DB not found: ${dbPath}`);
		}
		const store = new CheckpointStore(dbPath);
		const output = new OutputWriter(requestedOutputFormat ?? "yaml", requestedColorMode ?? "auto", Boolean(process.stdout.isTTY));
		const olderThanDays = getIntegerFlag("--older-than-days", args, 30);
		const keepRuns = getIntegerFlag("--keep-runs", args, 100);
		try {
			const before = store.getStats();
			const gcResult = store.garbageCollect({ olderThanDays, keepRuns, vacuum: true });
			const after = store.getStats();
			output.write(
				createGcPayload({
					dbPath,
					olderThanDays,
					keepRuns,
					before,
					after,
					deletedRunIds: gcResult.deletedRunIds,
					vacuumed: gcResult.vacuumed,
					verbose,
				}),
			);
			return;
		} finally {
			store.close();
		}
	}

	if (command === "auth") {
		const subcommand = args[1];
		if (isHelpArg(subcommand) || !subcommand) {
			printCommandHelp("auth");
			return;
		}
		if (subcommand !== "check") {
			throw new Error(`Unknown auth subcommand "${subcommand ?? ""}"`);
		}
		if (isHelpArg(args[2])) {
			printCommandHelp("auth");
			return;
		}

		const playbookArg = getCommandArg(args, 2);
		const compiledPlan = playbookArg ? compilePlaybook(loadPlaybookWithPacks(resolve(playbookArg))) : undefined;
		const output = new OutputWriter(
			requestedOutputFormat ?? compiledPlan?.output.format ?? "yaml",
			requestedColorMode ?? compiledPlan?.output.color ?? "auto",
			Boolean(process.stdout.isTTY),
		);
		const modelRegistry = new ModelRegistry(authStorage);
		const inspections = compiledPlan
			? Object.values(compiledPlan.agents)
					.filter((definition) => isProviderBackedAgent(definition))
					.map((definition) => modelRegistry.inspectAgent(definition))
			: [modelRegistry.inspectAgent({ kind: "openai.responses", instructions: "auth check", model: "placeholder", provider: runtimeApiProvider })];
		const ok = inspections.every((inspection) => inspection.configured);
		output.write({
			type: "auth_check",
			ok,
			providers: inspections,
			...(playbookArg ? { playbook: resolve(playbookArg) } : {}),
			...(verbose && compiledPlan ? { plan: compiledPlan } : {}),
		});
		if (!ok) {
			process.exit(1);
		}
		return;
	}

	const playbookPath = args[1];
	if (isHelpArg(playbookPath)) {
		printCommandHelp(command);
		return;
	}
	if (!playbookPath) {
		printCommandHelp(command);
		process.exit(1);
	}

	const playbookFile = resolve(playbookPath);
	const plan = compilePlaybook(loadPlaybookWithPacks(playbookFile));
	const store = new CheckpointStore(dbPath);
	const output = new OutputWriter(
		requestedOutputFormat ?? plan.output.format,
		requestedColorMode ?? plan.output.color,
		Boolean(process.stdout.isTTY),
	);
	const outputVerbose = verbose || plan.output.verbose;
	const runtime = new PlaybookRuntime(plan, store, {
		authStorage,
		hooks: {
			afterCheckpoint(checkpoint) {
				output.write(createCheckpointPayload(checkpoint, outputVerbose));
			},
		},
	});

	try {
		if (command === "run") {
			await runInteractiveApprovalLoop({
				initialResult: await runtime.start(),
				outputFormat: requestedOutputFormat ?? plan.output.format,
				stdoutIsTty: Boolean(process.stdout.isTTY),
				stdinIsTty: Boolean(process.stdin.isTTY),
				listPendingApprovals: (runId) => store.listApprovals({ runId, status: "pending" }),
				resolveApproval: (approvalId, status) =>
					store.resolveApproval(approvalId, status, {
						resolvedBy: "interactive-cli",
					}),
				resumeRun: (runId) => runtime.resume(runId),
				writeResult: (result) => output.write(createResultPayload(result, outputVerbose)),
				writeApproval: (type, approval) => output.write(createApprovalPayload(type, approval)),
				prompt: createInteractiveApprovalPrompt(),
			});
			return;
		}
		if (command === "resume") {
			const runId = args[2];
			if (!runId) throw new Error("resume requires a run id");
			await runInteractiveApprovalLoop({
				initialResult: await runtime.resume(runId),
				outputFormat: requestedOutputFormat ?? plan.output.format,
				stdoutIsTty: Boolean(process.stdout.isTTY),
				stdinIsTty: Boolean(process.stdin.isTTY),
				listPendingApprovals: (currentRunId) => store.listApprovals({ runId: currentRunId, status: "pending" }),
				resolveApproval: (approvalId, status) =>
					store.resolveApproval(approvalId, status, {
						resolvedBy: "interactive-cli",
					}),
				resumeRun: (currentRunId) => runtime.resume(currentRunId),
				writeResult: (result) => output.write(createResultPayload(result, outputVerbose)),
				writeApproval: (type, approval) => output.write(createApprovalPayload(type, approval)),
				prompt: createInteractiveApprovalPrompt(),
			});
			return;
		}
		if (command === "replay") {
			const runId = args[2];
			const checkpointSeq = Number(args[3]);
			if (!runId || Number.isNaN(checkpointSeq)) throw new Error("replay requires run id and checkpoint seq");
			await runInteractiveApprovalLoop({
				initialResult: await runtime.replay(runId, checkpointSeq),
				outputFormat: requestedOutputFormat ?? plan.output.format,
				stdoutIsTty: Boolean(process.stdout.isTTY),
				stdinIsTty: Boolean(process.stdin.isTTY),
				listPendingApprovals: (currentRunId) => store.listApprovals({ runId: currentRunId, status: "pending" }),
				resolveApproval: (approvalId, status) =>
					store.resolveApproval(approvalId, status, {
						resolvedBy: "interactive-cli",
					}),
				resumeRun: (currentRunId) => runtime.resume(currentRunId),
				writeResult: (result) => output.write(createResultPayload(result, outputVerbose)),
				writeApproval: (type, approval) => output.write(createApprovalPayload(type, approval)),
				prompt: createInteractiveApprovalPrompt(),
			});
			return;
		}
		throw new Error(`Unknown command "${command}"`);
	} finally {
		store.close();
	}
}

main().catch((error) => {
	console.error(error instanceof Error ? error.message : String(error));
	process.exit(1);
});
