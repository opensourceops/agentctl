import type { JsonObject, JsonValue, RuntimeSnapshot } from "./types.js";
import { buildTemplateContext } from "./utils.js";

const TASK_TEMPLATE_VARS_KEY = "__agentctlTaskVars";
const TASK_TEMPLATE_BASE_KEY = "__agentctlTaskBase";

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeJson(value: unknown): JsonValue {
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

function getPathValueStrict(path: string, source: Record<string, unknown>): unknown {
	const segments = path.split(".");
	const [root, ...rest] = segments;
	if (!root) {
		throw new Error(`Unresolved template path "${path}"`);
	}
	const taskVars = isObject(source[TASK_TEMPLATE_VARS_KEY]) ? source[TASK_TEMPLATE_VARS_KEY] : undefined;
	if (root === "vars" && taskVars) {
		return getPathValueStrictFromSegments(path, rest, taskVars);
	}
	if (taskVars && rest.length === 0 && root in taskVars) {
		return taskVars[root];
	}
	const taskBase = isObject(source[TASK_TEMPLATE_BASE_KEY]) ? source[TASK_TEMPLATE_BASE_KEY] : undefined;
	const target =
		taskBase && (root === "inputs" || root === "memory" || root === "tasks" || root === "run")
			? taskBase
			: source;
	return getPathValueStrictFromSegments(path, segments, target);
}

function getPathValueStrictFromSegments(
	path: string,
	segments: string[],
	source: Record<string, unknown>,
): unknown {
	let current: unknown = source;
	for (const segment of segments) {
		if (!isObject(current) || !(segment in current)) {
			throw new Error(`Unresolved template path "${path}"`);
		}
		current = current[segment];
	}
	return current;
}

function renderTemplateStringStrict(template: string, source: Record<string, unknown>): JsonValue {
	const exact = template.match(/^{{\s*([^}]+?)\s*}}$/);
	if (exact?.[1]) {
		return normalizeJson(getPathValueStrict(exact[1], source));
	}

	return template.replace(/{{\s*([^}]+?)\s*}}/g, (_match, rawPath) => {
		const path = String(rawPath).trim();
		const resolved = getPathValueStrict(path, source);
		return String(resolved);
	});
}

export function resolveTemplatesStrict<T extends JsonValue | Record<string, JsonValue>>(
	value: T,
	source: Record<string, unknown>,
): T {
	if (typeof value === "string") {
		return renderTemplateStringStrict(value, source) as T;
	}
	if (Array.isArray(value)) {
		return value.map((item) => resolveTemplatesStrict(item, source)) as T;
	}
	if (isObject(value)) {
		const resolved: Record<string, JsonValue> = {};
		for (const [key, item] of Object.entries(value)) {
			resolved[key] = resolveTemplatesStrict(item as JsonValue, source);
		}
		return resolved as T;
	}
	return value;
}

export function extractTemplatePaths(value: JsonValue | string): string[] {
	if (typeof value === "string") {
		return Array.from(value.matchAll(/{{\s*([^}]+?)\s*}}/g), (match) => String(match[1]).trim());
	}
	if (Array.isArray(value)) {
		return value.flatMap((item) => extractTemplatePaths(item));
	}
	if (isObject(value)) {
		return Object.values(value).flatMap((item) => extractTemplatePaths(item as JsonValue));
	}
	return [];
}

function resolveNamedVars(
	vars: JsonObject | undefined,
	source: Record<string, unknown>,
): JsonObject {
	if (!vars || Object.keys(vars).length === 0) {
		return {};
	}

	const resolved: Record<string, JsonValue> = {};
	const pending = new Map(Object.entries(vars));
	const maxPasses = Math.max(1, pending.size);

	for (let pass = 0; pass < maxPasses && pending.size > 0; pass += 1) {
		let progress = false;
		for (const [key, value] of Array.from(pending.entries())) {
			try {
				resolved[key] = resolveTemplatesStrict(value, {
					...source,
					vars: resolved,
					...resolved,
				});
				pending.delete(key);
				progress = true;
			} catch {
				// Retry on the next pass in case this depends on another agent var.
			}
		}
		if (!progress) {
			break;
		}
	}

	if (pending.size > 0) {
		const [key, value] = pending.entries().next().value as [string, JsonValue];
		const unresolved = extractTemplatePaths(value).join(", ");
		throw new Error(
			unresolved.length > 0
				? `Unable to resolve agent var "${key}" from template path(s): ${unresolved}`
				: `Unable to resolve agent var "${key}"`,
		);
	}

	return resolved;
}

export function resolveAgentVars(
	vars: JsonObject | undefined,
	source: Record<string, unknown>,
): JsonObject {
	return resolveNamedVars(vars, source);
}

export function resolveTaskVars(
	taskVars: JsonObject | undefined,
	agentVars: JsonObject | undefined,
	source: Record<string, unknown>,
): JsonObject {
	return resolveNamedVars(
		{
			...(agentVars ?? {}),
			...(taskVars ?? {}),
		},
		source,
	);
}

export function buildVarResolutionSource(snapshot: RuntimeSnapshot, input: JsonObject = {}): Record<string, unknown> {
	return {
		...buildTemplateContext(snapshot),
		input,
	};
}
