import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import type { JsonObject, JsonValue, RuntimeSnapshot, TaskReference } from "./types.js";

const TASK_TEMPLATE_VARS_KEY = "__agentctlTaskVars";
const TASK_TEMPLATE_BASE_KEY = "__agentctlTaskBase";

export function nowIso(): string {
	return new Date().toISOString();
}

export function parseTaskUse(uses: string): TaskReference {
	const index = uses.indexOf(":");
	if (index === -1) throw new Error(`Invalid task reference "${uses}"`);
	const kind = uses.slice(0, index);
	const ref = uses.slice(index + 1);
	if (kind !== "module" && kind !== "agent") {
		throw new Error(`Unsupported task reference kind "${kind}" in "${uses}"`);
	}
	return { kind, ref };
}

export function ensureParentDir(filePath: string): void {
	mkdirSync(dirname(filePath), { recursive: true });
}

export function deepClone<T>(value: T): T {
	return JSON.parse(JSON.stringify(value)) as T;
}

export function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isJsonObject(value: unknown): value is JsonObject {
	return isObject(value);
}

export function getStringValue(record: JsonObject, key: string): string | undefined {
	const value = record[key];
	return typeof value === "string" ? value : undefined;
}

export function getJsonObjectValue(record: JsonObject, key: string): JsonObject | undefined {
	const value = record[key];
	return isJsonObject(value) ? value : undefined;
}

export function getStringArrayValue(record: JsonObject, key: string): string[] | undefined {
	const value = record[key];
	if (!Array.isArray(value) || !value.every((item) => typeof item === "string")) {
		return undefined;
	}
	return value;
}

export function mergeRecords<T extends Record<string, unknown>>(base: T, override: T): T {
	return { ...base, ...override };
}

function getNestedPathValue(pathSegments: string[], source: Record<string, unknown>): unknown {
	return pathSegments.reduce<unknown>((current, key) => {
		if (!isObject(current)) return undefined;
		return current[key];
	}, source);
}

function getPathValue(path: string, source: Record<string, unknown>): unknown {
	const segments = path.split(".");
	const [root, ...rest] = segments;
	if (!root) {
		return undefined;
	}
	const taskVars = isObject(source[TASK_TEMPLATE_VARS_KEY]) ? source[TASK_TEMPLATE_VARS_KEY] : undefined;
	const taskBase = isObject(source[TASK_TEMPLATE_BASE_KEY]) ? source[TASK_TEMPLATE_BASE_KEY] : undefined;
	if (root === "vars" && taskVars) {
		return rest.length === 0 ? taskVars : getNestedPathValue(rest, taskVars);
	}
	if (taskVars && rest.length === 0 && root in taskVars) {
		return taskVars[root];
	}
	const target = taskBase && (root === "inputs" || root === "memory" || root === "tasks" || root === "run")
		? taskBase
		: source;
	return getNestedPathValue(segments, target);
}

function renderTemplateString(template: string, source: Record<string, unknown>): JsonValue {
	const exact = template.match(/^{{\s*([^}]+?)\s*}}$/);
	if (exact) {
		const path = exact[1];
		if (!path) return "";
		const resolved = getPathValue(path, source);
		return normalizeJson(resolved);
	}

	return template.replace(/{{\s*([^}]+?)\s*}}/g, (_match, path) => {
		const resolved = getPathValue(path, source);
		return resolved === undefined ? "" : String(resolved);
	});
}

export function resolveTemplates<T extends JsonValue | Record<string, JsonValue>>(
	value: T,
	source: Record<string, unknown>,
): T {
	if (typeof value === "string") {
		return renderTemplateString(value, source) as T;
	}
	if (Array.isArray(value)) {
		return value.map((item) => resolveTemplates(item, source)) as T;
	}
	if (isObject(value)) {
		const resolved: Record<string, JsonValue> = {};
		for (const [key, item] of Object.entries(value)) {
			resolved[key] = resolveTemplates(item as JsonValue, source);
		}
		return resolved as T;
	}
	return value;
}

export function normalizeJson(value: unknown): JsonValue {
	if (
		value === null ||
		typeof value === "string" ||
		typeof value === "number" ||
		typeof value === "boolean"
	) {
		return value;
	}
	if (Array.isArray(value)) {
		return value.map((item) => normalizeJson(item));
	}
	if (isObject(value)) {
		const normalized: Record<string, JsonValue> = {};
		for (const [key, item] of Object.entries(value)) {
			normalized[key] = normalizeJson(item);
		}
		return normalized;
	}
	return String(value);
}

export function buildTemplateContext(snapshot: RuntimeSnapshot): Record<string, unknown> {
	const tasks: Record<string, unknown> = {};
	for (const [taskId, taskState] of Object.entries(snapshot.tasks)) {
		tasks[taskId] = {
			status: taskState.status,
			attempts: taskState.attempts,
			output: taskState.output ?? null,
			error: taskState.error ?? null,
		};
	}

	return {
		inputs: snapshot.inputs,
		vars: snapshot.vars,
		memory: {
			working: snapshot.memory.working,
		},
		tasks,
		run: {
			inputs: snapshot.inputs,
			vars: snapshot.vars,
			memory: {
				working: snapshot.memory.working,
			},
		},
	};
}

export function buildTaskTemplateContext(
	snapshot: RuntimeSnapshot,
	resolvedVars: JsonObject = {},
	input: JsonObject = {},
): Record<string, unknown> {
	const base = buildTemplateContext(snapshot);
	return {
		...base,
		vars: resolvedVars,
		input,
		[TASK_TEMPLATE_VARS_KEY]: resolvedVars,
		[TASK_TEMPLATE_BASE_KEY]: base,
	};
}

export function stableStringify(value: unknown): string {
	if (Array.isArray(value)) {
		return `[${value.map((item) => stableStringify(item)).join(",")}]`;
	}
	if (isObject(value)) {
		return `{${Object.keys(value)
			.sort()
			.map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`)
			.join(",")}}`;
	}
	return JSON.stringify(value);
}

export function resolveRelativePath(fromFile: string, target: string): string {
	return resolve(dirname(fromFile), target);
}
