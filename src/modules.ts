import { execFile } from "node:child_process";
import { access, mkdir, readdir, readFile, stat, writeFile } from "node:fs/promises";
import { basename, dirname, relative, resolve } from "node:path";
import { promisify } from "node:util";
import type { LongTermMemoryEntry } from "./long-term-memory.js";
import type { LongTermMemoryAdapter } from "./long-term-memory-adapters/types.js";
import type {
	JsonObject,
	JsonValue,
	ModuleDefinition,
	ProcessModuleDefinition,
	ProcessRequirementDefinition,
	RuntimeSnapshot,
	TaskOutput,
} from "./types.js";
import {
	buildTemplateContext,
	getJsonObjectValue,
	getStringArrayValue,
	getStringValue,
	isJsonObject,
	nowIso,
	resolveTemplates,
} from "./utils.js";

const execFileAsync = promisify(execFile);

export interface ModuleExecutionResult {
	output: TaskOutput;
	stateUpdates?: JsonObject;
}

export interface ModuleExecutionContext {
	runId: string;
	taskId: string;
	definition: ModuleDefinition;
	input: JsonObject;
	snapshot: RuntimeSnapshot;
	workspaceRoot: string;
	longTermMemory?: LongTermMemoryAdapter;
	longTermNamespace?: string;
}

export interface ModuleExecutor {
	run(context: ModuleExecutionContext): Promise<ModuleExecutionResult>;
}

function evaluateAssertInput(input: JsonObject): { ok: boolean; details: TaskOutput } {
	const message = getStringValue(input, "message");
	if ("condition" in input) {
		const ok = Boolean(input.condition);
		return {
			ok,
			details: {
				ok,
				condition: input.condition,
				message: message ?? (ok ? "assertion passed" : "assertion failed"),
			},
		};
	}

	const equals = getJsonObjectValue(input, "equals");
	if (equals) {
		const ok = JSON.stringify(equals.left ?? null) === JSON.stringify(equals.right ?? null);
		return {
			ok,
			details: {
				ok,
				left: equals.left ?? null,
				right: equals.right ?? null,
				message: message ?? (ok ? "assertion passed" : "values are not equal"),
			},
		};
	}

	throw new Error('builtin.assert requires either "condition" or "equals"');
}

class AssignModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const values = context.input.values;
		if (!isJsonObject(values)) {
			throw new Error('builtin.assign requires an object "values" field');
		}

		return {
			output: {
				assignedAt: nowIso(),
				values,
			},
			stateUpdates: values,
		};
	}
}

class AssertModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const result = evaluateAssertInput(context.input);
		if (!result.ok) {
			throw new Error(String(result.details.message));
		}

		return { output: result.details };
	}
}

class ShellExecModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const command = context.input.command;
		if (typeof command !== "string" || command.trim() === "") {
			throw new Error('builtin.shell.exec requires a non-empty "command"');
		}

		const cwd =
			typeof context.input.cwd === "string" ? resolve(context.workspaceRoot, context.input.cwd) : context.workspaceRoot;
		const envObject = getJsonObjectValue(context.input, "env");
		const env = envObject
			? Object.fromEntries(Object.entries(envObject).map(([key, value]) => [key, String(value)]))
			: undefined;

		try {
			const { stdout, stderr } = await execFileAsync("/bin/sh", ["-lc", command], {
				cwd,
				env: env ? { ...process.env, ...env } : process.env,
				maxBuffer: 10 * 1024 * 1024,
			});
			return {
				output: {
					command,
					cwd,
					exitCode: 0,
					stdout: stdout.trimEnd(),
					stderr: stderr.trimEnd(),
				},
			};
		} catch (error) {
			const execError = error as Error & { stdout?: string; stderr?: string; code?: number };
			throw new Error(
				[
					`Command failed: ${command}`,
					`exitCode=${execError.code ?? "unknown"}`,
					execError.stdout ? `stdout=${execError.stdout.trimEnd()}` : "",
					execError.stderr ? `stderr=${execError.stderr.trimEnd()}` : "",
				]
					.filter(Boolean)
					.join("\n"),
			);
		}
	}
}

class ProcessModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		if (context.definition.kind !== "pack.process") {
			throw new Error(`ProcessModuleExecutor received unsupported module kind "${context.definition.kind}"`);
		}

		const source = {
			...buildTemplateContext(context.snapshot),
			input: context.input,
		};
		const commandValue = resolveTemplates(context.definition.command, source);
		if (typeof commandValue !== "string" || commandValue.trim() === "") {
			throw new Error('pack.process requires a non-empty "command"');
		}
		const argsValue = context.definition.args ? resolveTemplates([...context.definition.args], source) : [];
		if (!Array.isArray(argsValue) || !argsValue.every((item) => typeof item === "string")) {
			throw new Error('pack.process requires string "args" values');
		}
		const cwdValue =
			context.definition.cwd !== undefined ? resolveTemplates(context.definition.cwd, source) : context.workspaceRoot;
		if (typeof cwdValue !== "string" || cwdValue.trim() === "") {
			throw new Error('pack.process requires string "cwd" when provided');
		}
		const envValue = context.definition.env ? resolveTemplates({ ...context.definition.env }, source) : undefined;
		if (envValue !== undefined && (!isJsonObject(envValue) || !Object.values(envValue).every((item) => typeof item === "string"))) {
			throw new Error('pack.process requires string "env" values');
		}

		try {
			const { stdout, stderr } = await execFileAsync(commandValue, argsValue, {
				cwd: cwdValue,
				env: envValue ? { ...process.env, ...envValue } : process.env,
				maxBuffer: 10 * 1024 * 1024,
			});
			return {
				output: {
					command: commandValue,
					args: argsValue,
					cwd: cwdValue,
					exitCode: 0,
					stdout: stdout.trimEnd(),
					stderr: stderr.trimEnd(),
				},
			};
		} catch (error) {
			const execError = error as Error & { stdout?: string; stderr?: string; code?: number };
			throw new Error(
				[
					`Command failed: ${commandValue}`,
					`exitCode=${execError.code ?? "unknown"}`,
					execError.stdout ? `stdout=${execError.stdout.trimEnd()}` : "",
					execError.stderr ? `stderr=${execError.stderr.trimEnd()}` : "",
				]
					.filter(Boolean)
					.join("\n"),
			);
		}
	}
}

class ReadModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const path = getRequiredPath(context.input, context.workspaceRoot);
		const offset = getOptionalInteger(context.input.offset, 0);
		const limit = getOptionalInteger(context.input.limit, 200);
		const content = await readFile(path, "utf8");
		const lines = content.split("\n");
		const selected = lines.slice(offset, offset + limit);
		return {
			output: {
				path,
				offset,
				limit,
				totalLines: lines.length,
				content: selected.join("\n"),
			},
		};
	}
}

class WriteModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const path = getRequiredPath(context.input, context.workspaceRoot);
		const content = context.input.content;
		if (typeof content !== "string") {
			throw new Error('builtin.write requires string "content"');
		}
		const mode = context.input.mode === "append" ? "append" : "overwrite";
		await mkdir(dirname(path), { recursive: true });
		if (mode === "append") {
			const existing = await readFile(path, "utf8").catch(() => "");
			await writeFile(path, `${existing}${content}`, "utf8");
		} else {
			await writeFile(path, content, "utf8");
		}
		return {
			output: {
				path,
				mode,
				bytesWritten: Buffer.byteLength(content, "utf8"),
			},
		};
	}
}

class EditModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const path = getRequiredPath(context.input, context.workspaceRoot);
		const find = context.input.find;
		const replace = context.input.replace;
		if (typeof find !== "string" || find.length === 0) {
			throw new Error('builtin.edit requires non-empty string "find"');
		}
		if (typeof replace !== "string") {
			throw new Error('builtin.edit requires string "replace"');
		}

		const replaceAll = context.input.replaceAll === true;
		const expectedMatches = typeof context.input.expectedMatches === "number" ? context.input.expectedMatches : undefined;
		const original = await readFile(path, "utf8");
		const matchCount = original.split(find).length - 1;
		if (matchCount === 0) {
			throw new Error(`builtin.edit could not find "${find}" in ${path}`);
		}
		if (expectedMatches !== undefined && matchCount !== expectedMatches) {
			throw new Error(`builtin.edit expected ${expectedMatches} matches, found ${matchCount}`);
		}
		const updated = replaceAll ? original.split(find).join(replace) : original.replace(find, replace);
		const replacements = replaceAll ? matchCount : 1;
		await writeFile(path, updated, "utf8");
		return {
			output: {
				path,
				find,
				replace,
				replacements,
			},
		};
	}
}

class GrepModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const root = getOptionalPath(context.input.path, context.workspaceRoot) ?? context.workspaceRoot;
		const patternValue = context.input.pattern;
		if (typeof patternValue !== "string" || patternValue.length === 0) {
			throw new Error('builtin.grep requires non-empty string "pattern"');
		}
		const limit = getOptionalInteger(context.input.limit, 50);
		const matcher = new RegExp(patternValue, "i");
		const files = await walkFiles(root);
		const matches: JsonValue[] = [];

		for (const file of files) {
			const content = await readFile(file, "utf8").catch(() => null);
			if (content === null) continue;
			for (const [index, line] of content.split("\n").entries()) {
				if (matcher.test(line)) {
					matches.push({ path: file, line: index + 1, text: line });
					if (matches.length >= limit) {
						return { output: { path: root, pattern: patternValue, matches } };
					}
				}
			}
		}

		return { output: { path: root, pattern: patternValue, matches } };
	}
}

class FindModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const root = getOptionalPath(context.input.path, context.workspaceRoot) ?? context.workspaceRoot;
		const patternValue = context.input.pattern;
		if (typeof patternValue !== "string" || patternValue.length === 0) {
			throw new Error('builtin.find requires non-empty string "pattern"');
		}
		const limit = getOptionalInteger(context.input.limit, 50);
		const files = await walkFiles(root);
		const matches = files.filter((file) => matchesFindPattern(file, root, patternValue)).slice(0, limit);
		return {
			output: {
				path: root,
				pattern: patternValue,
				matches,
			},
		};
	}
}

function matchesFindPattern(filePath: string, root: string, pattern: string): boolean {
	const normalizedPattern = normalizePathPattern(pattern);
	if (
		normalizedPattern === "." ||
		normalizedPattern === "*" ||
		normalizedPattern === "**" ||
		normalizedPattern === "**/*"
	) {
		return true;
	}

	const relativePath = normalizePathPattern(relative(root, filePath));
	const fileName = normalizePathPattern(basename(filePath));
	if (!containsGlobToken(normalizedPattern)) {
		return relativePath.includes(normalizedPattern) || fileName.includes(normalizedPattern);
	}

	const matcher = new RegExp(globPatternToRegex(normalizedPattern));
	return matcher.test(relativePath) || matcher.test(fileName);
}

function normalizePathPattern(value: string): string {
	return value.replaceAll("\\", "/");
}

function containsGlobToken(pattern: string): boolean {
	return pattern.includes("*") || pattern.includes("?");
}

function globPatternToRegex(pattern: string): string {
	let source = "";
	for (let index = 0; index < pattern.length; index += 1) {
		const char = pattern[index];
		if (char === undefined) {
			continue;
		}
		if (char === "*") {
			const nextChar = pattern[index + 1];
			if (nextChar === "*") {
				const afterDoubleStar = pattern[index + 2];
				if (afterDoubleStar === "/") {
					source += "(?:.*/)?";
					index += 2;
					continue;
				}
				source += ".*";
				index += 1;
				continue;
			}
			source += "[^/]*";
			continue;
		}
		if (char === "?") {
			source += "[^/]";
			continue;
		}
		source += escapeRegexCharacter(char);
	}
	return `^${source}$`;
}

function escapeRegexCharacter(char: string): string {
	return /[\\^$+?.()|[\]{}]/.test(char) ? `\\${char}` : char;
}

class LsModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const path = getOptionalPath(context.input.path, context.workspaceRoot) ?? context.workspaceRoot;
		const entries = await readdir(path, { withFileTypes: true });
		return {
			output: {
				path,
				entries: entries.map((entry) => ({
					name: entry.name,
					type: entry.isDirectory() ? "directory" : entry.isFile() ? "file" : "other",
				})),
			},
		};
	}
}

class WorkingMemoryReadModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const key = context.input.key;
		if (typeof key !== "string" || key.length === 0) {
			throw new Error('builtin.memory.read requires non-empty string "key"');
		}
		const value = context.snapshot.memory.working[key] ?? null;
		return {
			output: {
				key,
				value,
				found: key in context.snapshot.memory.working,
			},
		};
	}
}

class WorkingMemoryWriteModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		const key = context.input.key;
		if (typeof key !== "string" || key.length === 0) {
			throw new Error('builtin.memory.write requires non-empty string "key"');
		}
		if (!("value" in context.input)) {
			throw new Error('builtin.memory.write requires "value"');
		}
		return {
			output: {
				key,
				value: context.input.value ?? null,
				scope: "working",
			},
			stateUpdates: {
				[key]: context.input.value ?? null,
			},
		};
	}
}

class LongTermMemoryWriteModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		if (!context.longTermMemory) {
			throw new Error("builtin.long_term_memory.write requires a configured long-term memory store");
		}
		const key = context.input.key;
		if (typeof key !== "string" || key.length === 0) {
			throw new Error('builtin.long_term_memory.write requires non-empty string "key"');
		}
		if (!("value" in context.input)) {
			throw new Error('builtin.long_term_memory.write requires "value"');
		}
		const namespace =
			typeof context.input.namespace === "string" && context.input.namespace.length > 0
				? context.input.namespace
				: (context.longTermNamespace ?? "default");
		const tags = getStringArrayValue(context.input, "tags") ?? [];
		const entry = await context.longTermMemory.write(namespace, key, context.input.value ?? null, tags);
		return {
			output: {
				namespace: entry.namespace,
				key: entry.key,
				value: entry.value,
				tags: entry.tags,
				updatedAt: entry.updatedAt,
			},
		};
	}
}

class LongTermMemorySearchModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		if (!context.longTermMemory) {
			throw new Error("builtin.long_term_memory.search requires a configured long-term memory store");
		}
		const namespace =
			typeof context.input.namespace === "string" && context.input.namespace.length > 0
				? context.input.namespace
				: (context.longTermNamespace ?? "default");
		const query = typeof context.input.query === "string" && context.input.query.length > 0 ? context.input.query : undefined;
		const key = typeof context.input.key === "string" && context.input.key.length > 0 ? context.input.key : undefined;
		const limit = getOptionalInteger(context.input.limit, 10);
		const result = await context.longTermMemory.search(namespace, query, key, limit);
		return {
			output: {
				namespace,
				query: query ?? null,
				key: key ?? null,
				matchCount: result.entries.length,
				matches: result.entries.map((entry) => toJsonMemoryEntry(entry)),
			},
		};
	}
}

class LongTermMemoryRetrieveModuleExecutor implements ModuleExecutor {
	async run(context: ModuleExecutionContext): Promise<ModuleExecutionResult> {
		if (!context.longTermMemory) {
			throw new Error("builtin.long_term_memory.retrieve requires a configured long-term memory store");
		}
		const namespace =
			typeof context.input.namespace === "string" && context.input.namespace.length > 0
				? context.input.namespace
				: (context.longTermNamespace ?? "default");
		const query = typeof context.input.query === "string" && context.input.query.length > 0 ? context.input.query : undefined;
		const key = typeof context.input.key === "string" && context.input.key.length > 0 ? context.input.key : undefined;
		const limit = getOptionalInteger(context.input.limit, 10);
		const select = context.input.select === "all" ? "all" : "first";
		const promoteKey = context.input.promoteKey;
		if (typeof promoteKey !== "string" || promoteKey.length === 0) {
			throw new Error('builtin.long_term_memory.retrieve requires non-empty string "promoteKey"');
		}
		const promoteMode =
			context.input.promoteMode === "entry" || context.input.promoteMode === "matches" || context.input.promoteMode === "values"
				? context.input.promoteMode
				: "value";
		const includeMetadata = context.input.includeMetadata === true;
		const result = await context.longTermMemory.search(namespace, query, key, limit);
		const selected = select === "all" ? result.entries : result.entries.slice(0, 1);
		const promotedValue = buildPromotedValue(selected, {
			select,
			promoteMode,
			includeMetadata,
		});
		return {
			output: {
				namespace,
				query: query ?? null,
				key: key ?? null,
				select,
				promoteKey,
				promoteMode,
				matchCount: result.entries.length,
				promoted: selected.length > 0,
				selected: selected.map((entry) => toJsonMemoryEntry(entry)),
				promotedValue,
			},
			...(selected.length > 0 ? { stateUpdates: { [promoteKey]: promotedValue } } : {}),
		};
	}
}

function buildPromotedValue(
	entries: LongTermMemoryEntry[],
	options: {
		select: "first" | "all";
		promoteMode: "value" | "entry" | "matches" | "values";
		includeMetadata: boolean;
	},
): JsonValue {
	if (options.select === "all") {
		if (options.promoteMode === "values" || options.promoteMode === "value") {
			return entries.map((entry) => entry.value);
		}
		return entries.map((entry) => toJsonMemoryEntry(entry));
	}
	const entry = entries[0];
	if (!entry) {
		return null;
	}
	if (options.promoteMode === "entry" || options.promoteMode === "matches") {
		return toJsonMemoryEntry(entry);
	}
	return entry.value;
}

function toJsonMemoryEntry(entry: LongTermMemoryEntry): JsonObject {
	return {
		namespace: entry.namespace,
		key: entry.key,
		value: entry.value,
		tags: entry.tags,
		createdAt: entry.createdAt,
		updatedAt: entry.updatedAt,
	};
}

function getRequiredPath(value: JsonObject, workspaceRoot: string): string {
	if (typeof value.path !== "string" || value.path.length === 0) {
		throw new Error('module requires non-empty string "path"');
	}
	return resolve(workspaceRoot, value.path);
}

function getOptionalPath(value: JsonValue | undefined, workspaceRoot: string): string | undefined {
	return typeof value === "string" && value.length > 0 ? resolve(workspaceRoot, value) : undefined;
}

function getOptionalInteger(value: JsonValue | undefined, fallback: number): number {
	return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : fallback;
}

async function walkFiles(root: string): Promise<string[]> {
	const rootStats = await stat(root);
	if (!rootStats.isDirectory()) {
		return [root];
	}
	const files: string[] = [];
	const queue = [root];
	while (queue.length > 0) {
		const current = queue.shift();
		if (!current) continue;
		const entries = await readdir(current, { withFileTypes: true });
		for (const entry of entries) {
			const nextPath = resolve(current, entry.name);
			if (entry.isDirectory()) {
				queue.push(nextPath);
				continue;
			}
			if (entry.isFile()) {
				files.push(nextPath);
			}
		}
	}
	return files;
}

export class BuiltinModuleRegistry {
	private readonly executors = new Map<string, ModuleExecutor>([
		["builtin.assign", new AssignModuleExecutor()],
		["builtin.assert", new AssertModuleExecutor()],
		["builtin.shell.exec", new ShellExecModuleExecutor()],
		["builtin.read", new ReadModuleExecutor()],
		["builtin.write", new WriteModuleExecutor()],
		["builtin.edit", new EditModuleExecutor()],
			["builtin.grep", new GrepModuleExecutor()],
			["builtin.find", new FindModuleExecutor()],
			["builtin.ls", new LsModuleExecutor()],
			["builtin.memory.read", new WorkingMemoryReadModuleExecutor()],
			["builtin.memory.write", new WorkingMemoryWriteModuleExecutor()],
			["builtin.long_term_memory.write", new LongTermMemoryWriteModuleExecutor()],
			["builtin.long_term_memory.search", new LongTermMemorySearchModuleExecutor()],
			["builtin.long_term_memory.retrieve", new LongTermMemoryRetrieveModuleExecutor()],
			["pack.process", new ProcessModuleExecutor()],
		]);

	constructor(private readonly longTermMemory?: LongTermMemoryAdapter, private readonly longTermNamespace?: string) {}

	register(kind: string, executor: ModuleExecutor): void {
		this.executors.set(kind, executor);
	}

	async execute(
		runId: string,
		taskId: string,
		definition: ModuleDefinition,
		taskInput: Record<string, JsonValue>,
		snapshot: RuntimeSnapshot,
		workspaceRoot: string,
	): Promise<ModuleExecutionResult> {
		const resolvedInput = this.resolveInput(definition, taskInput, snapshot);
		return this.executeResolved(runId, taskId, definition, resolvedInput, snapshot, workspaceRoot);
	}

	resolveInput(
		definition: ModuleDefinition,
		taskInput: JsonObject,
		snapshot: RuntimeSnapshot,
	): JsonObject {
		const baseInput = definition.with ?? {};
		const mergedInput = { ...baseInput, ...taskInput };
		return resolveTemplates(mergedInput, buildTemplateContext(snapshot));
	}

	async executeResolved(
		runId: string,
		taskId: string,
		definition: ModuleDefinition,
		resolvedInput: JsonObject,
		snapshot: RuntimeSnapshot,
		workspaceRoot: string,
	): Promise<ModuleExecutionResult> {
		const executor = this.executors.get(definition.kind);
		if (!executor) throw new Error(`No executor registered for module kind "${definition.kind}"`);
		return executor.run({
			runId,
			taskId,
			definition,
			input: resolvedInput,
			snapshot,
			workspaceRoot,
			...(this.longTermMemory ? { longTermMemory: this.longTermMemory } : {}),
			...(this.longTermNamespace ? { longTermNamespace: this.longTermNamespace } : {}),
		});
	}
}

export async function preflightProcessModule(definition: ProcessModuleDefinition): Promise<void> {
	await assertExecutableAvailable(definition.command);
	for (const requirement of definition.runtime?.requires ?? []) {
		await assertRequirement(requirement);
	}
}

async function assertExecutableAvailable(command: string): Promise<void> {
	const resolved = await resolveExecutable(command);
	if (!resolved) {
		throw new Error(`Required executable not found: ${command}`);
	}
}

async function assertRequirement(requirement: ProcessRequirementDefinition): Promise<void> {
	const executable = await resolveExecutable(requirement.command);
	if (!executable) {
		throw new Error(`Required executable not found: ${requirement.command}`);
	}
	if (!requirement.version) {
		return;
	}
	const versionArgs = requirement.versionArgs ? [...requirement.versionArgs] : ["--version"];
	const { stdout, stderr } = await execFileAsync(executable, versionArgs, {
		env: process.env,
		maxBuffer: 1024 * 1024,
	});
	const versionText = `${stdout}\n${stderr}`;
	const actualVersion = extractSemver(versionText);
	if (!actualVersion) {
		throw new Error(`Could not determine version for ${requirement.command}`);
	}
	if (!satisfiesMinimumVersion(actualVersion, requirement.version)) {
		throw new Error(`Executable ${requirement.command} version ${actualVersion} does not satisfy ${requirement.version}`);
	}
}

async function resolveExecutable(command: string): Promise<string | undefined> {
	if (command.includes("/") || command.includes("\\")) {
		try {
			await access(command);
			return command;
		} catch {
			return undefined;
		}
	}

	const pathEntries = (process.env.PATH ?? "").split(":").filter((entry) => entry.length > 0);
	for (const pathEntry of pathEntries) {
		const candidate = resolve(pathEntry, command);
		try {
			await access(candidate);
			return candidate;
		} catch {
			continue;
		}
	}
	return undefined;
}

function extractSemver(value: string): string | undefined {
	const match = /(\d+)\.(\d+)\.(\d+)/.exec(value) ?? /(\d+)\.(\d+)/.exec(value);
	if (!match) {
		return undefined;
	}
	const major = match[1];
	const minor = match[2];
	const patch = match[3] ?? "0";
	if (!major || !minor) {
		return undefined;
	}
	return `${major}.${minor}.${patch}`;
}

function satisfiesMinimumVersion(actual: string, constraint: string): boolean {
	const match = /^>=\s*(\d+(?:\.\d+){0,2})$/.exec(constraint.trim());
	const minimumVersion = match?.[1];
	if (!minimumVersion) {
		throw new Error(`Unsupported version constraint "${constraint}". Use >=x.y.z`);
	}
	const expected = normalizeVersion(minimumVersion);
	return compareVersions(normalizeVersion(actual), expected) >= 0;
}

function normalizeVersion(version: string): [number, number, number] {
	const [major = "0", minor = "0", patch = "0"] = version.split(".");
	return [Number(major), Number(minor), Number(patch)];
}

function compareVersions(left: [number, number, number], right: [number, number, number]): number {
	for (let index = 0; index < left.length; index += 1) {
		const leftPart = left[index] ?? 0;
		const rightPart = right[index] ?? 0;
		if (leftPart > rightPart) {
			return 1;
		}
		if (leftPart < rightPart) {
			return -1;
		}
	}
	return 0;
}
