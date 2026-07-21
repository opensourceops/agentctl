import { existsSync, readFileSync } from "node:fs";
import { LineCounter, parseDocument } from "yaml";
import { z, type ZodType } from "zod";
import { compilePlaybook } from "./compiler.js";
import { loadPlaybook, loadPlaybookWithPacks, loadPack } from "./parser.js";
import { packSchema, playbookSchema } from "./schema.js";
import { extractTemplatePaths } from "./template-utils.js";
import type { JsonObject, JsonValue, PackDefinition, PlaybookDefinition } from "./types.js";
import { resolveRelativePath } from "./utils.js";

export type CheckPhase = "yaml_syntax" | "schema" | "load" | "compile" | "template";

export interface CheckDiagnostic {
	readonly file: string;
	readonly phase: CheckPhase;
	readonly detail: string;
	readonly path?: string;
	readonly line?: number;
	readonly column?: number;
}

export interface CheckResult {
	readonly ok: boolean;
	readonly playbook: string;
	readonly packs: readonly string[];
	readonly diagnostics: readonly CheckDiagnostic[];
}

function createYamlDiagnostic(filePath: string, error: unknown): CheckDiagnostic {
	const yamlError = error as {
		message?: string;
		linePos?: Array<{ line: number; col: number }>;
	};
	return {
		file: filePath,
		phase: "yaml_syntax",
		detail: yamlError.message ?? "Invalid YAML",
		...(yamlError.linePos?.[0]
			? {
					line: yamlError.linePos[0].line,
					column: yamlError.linePos[0].col,
				}
			: {}),
	};
}

function parseYamlForCheck(filePath: string): unknown {
	const source = readFileSync(filePath, "utf8");
	const lineCounter = new LineCounter();
	const document = parseDocument(source, { lineCounter, prettyErrors: true });
	if (document.errors.length > 0) {
		throw document.errors[0];
	}
	return document.toJS();
}

function schemaDiagnostic(filePath: string, error: z.ZodError): CheckDiagnostic {
	const issue = error.issues[0];
	return {
		file: filePath,
		phase: "schema",
		detail: issue?.message ?? "Schema validation failed",
		...(issue && issue.path.length > 0
			? {
					path: issue.path
						.filter(
							(segment: PropertyKey): segment is string | number =>
								typeof segment === "string" || typeof segment === "number",
						)
						.join("."),
				}
			: {}),
	};
}

function validateYamlFile<T>(filePath: string, schema: ZodType<T>): CheckDiagnostic | undefined {
	try {
		const parsed = parseYamlForCheck(filePath);
		const validated = schema.safeParse(parsed);
		if (!validated.success) {
			return schemaDiagnostic(filePath, validated.error);
		}
		return undefined;
	} catch (error) {
		return createYamlDiagnostic(filePath, error);
	}
}

const EXPLICIT_TEMPLATE_ROOTS = new Set(["inputs", "memory", "tasks", "run", "vars", "input"]);

function validateTemplateReference(
	filePath: string,
	templatePath: string,
	availableVars: ReadonlySet<string>,
	taskIds: ReadonlySet<string>,
	detail: (name: string) => string,
	locationPath: string,
): CheckDiagnostic | undefined {
	const [root, second] = templatePath.split(".");
	if (!root) {
		return undefined;
	}
	if (root === "tasks") {
		if (!second || !taskIds.has(second)) {
			return {
				file: filePath,
				phase: "template",
				detail: `Template references unknown task "${second ?? ""}"`,
				path: locationPath,
			};
		}
		return undefined;
	}
	if (root === "vars") {
		if (!second || !availableVars.has(second)) {
			return {
				file: filePath,
				phase: "template",
				detail: detail(second ?? root),
				path: locationPath,
			};
		}
		return undefined;
	}
	if (EXPLICIT_TEMPLATE_ROOTS.has(root)) {
		return undefined;
	}
	if (!availableVars.has(root)) {
		return {
			file: filePath,
			phase: "template",
			detail: detail(root),
			path: locationPath,
		};
	}
	return undefined;
}

function validateTaskTemplateValue(
	filePath: string,
	value: JsonValue,
	availableVars: ReadonlySet<string>,
	taskIds: ReadonlySet<string>,
	locationPath: string,
	detail: (name: string) => string,
): CheckDiagnostic | undefined {
	for (const templatePath of extractTemplatePaths(value)) {
		const diagnostic = validateTemplateReference(
			filePath,
			templatePath,
			availableVars,
			taskIds,
			detail,
			locationPath,
		);
		if (diagnostic) {
			return diagnostic;
		}
	}
	return undefined;
}

function validateTemplatePaths(filePath: string, definition: PlaybookDefinition, taskIds: ReadonlySet<string>): CheckDiagnostic | undefined {
	for (const task of definition.tasks) {
		const agentName = task.uses.startsWith("agent:") ? task.uses.slice("agent:".length) : undefined;
		const agent = agentName ? definition.agents?.[agentName] : undefined;
		const availableVars = new Set<string>([
			...Object.keys(agent?.vars ?? {}),
			...Object.keys(task.vars ?? {}),
		]);

		for (const [varName, value] of Object.entries(agent?.vars ?? {})) {
			const diagnostic = validateTaskTemplateValue(
				filePath,
				value as JsonValue,
				availableVars,
				taskIds,
				`agents.${agentName}.vars.${varName}`,
				(name) => `Agent "${agentName}" default var "${varName}" references undefined variable "${name}"`,
			);
			if (diagnostic) {
				return diagnostic;
			}
		}

		for (const [varName, value] of Object.entries(task.vars ?? {})) {
			const diagnostic = validateTaskTemplateValue(
				filePath,
				value as JsonValue,
				availableVars,
				taskIds,
				`tasks.${task.id}.vars.${varName}`,
				(name) => `Task "${task.id}" var "${varName}" references undefined variable "${name}"`,
			);
			if (diagnostic) {
				return diagnostic;
			}
		}

		const taskInputDiagnostic = validateTaskTemplateValue(
			filePath,
			task.with ?? {},
			availableVars,
			taskIds,
			`tasks.${task.id}.with`,
			(name) => `Task "${task.id}" input references undefined variable "${name}"`,
		);
		if (taskInputDiagnostic) {
			return taskInputDiagnostic;
		}

		if (!agentName || !agent?.instructions) {
			continue;
		}
		const promptDiagnostic = validateTaskTemplateValue(
			filePath,
			agent.instructions,
			availableVars,
			taskIds,
			`tasks.${task.id}.uses`,
			(name) => `Task "${task.id}" references undefined prompt variable "${name}" for agent "${agentName}"`,
		);
		if (promptDiagnostic) {
			return promptDiagnostic;
		}
	}

	return undefined;
}

function getPackPaths(definition: PlaybookDefinition, playbookPath: string): string[] {
	return (definition.packs ?? []).map((packPath) => resolveRelativePath(playbookPath, packPath));
}

export function checkPlaybook(filePath: string): CheckResult {
	const resolvedFile = filePath;
	if (!existsSync(resolvedFile)) {
		return {
			ok: false,
			playbook: resolvedFile,
			packs: [],
			diagnostics: [
				{
					file: resolvedFile,
					phase: "load",
					detail: `Playbook file not found: ${resolvedFile}`,
				},
			],
		};
	}

	const playbookDiagnostic = validateYamlFile(resolvedFile, playbookSchema);
	if (playbookDiagnostic) {
		return {
			ok: false,
			playbook: resolvedFile,
			packs: [],
			diagnostics: [playbookDiagnostic],
		};
	}

	let playbookDefinition: PlaybookDefinition;
	try {
		playbookDefinition = loadPlaybook(resolvedFile);
	} catch (error) {
		return {
			ok: false,
			playbook: resolvedFile,
			packs: [],
			diagnostics: [
				{
					file: resolvedFile,
					phase: "load",
					detail: error instanceof Error ? error.message : String(error),
				},
			],
		};
	}

	const packPaths = getPackPaths(playbookDefinition, resolvedFile);
	for (const packPath of packPaths) {
		const packDiagnostic = validateYamlFile(packPath, packSchema);
		if (packDiagnostic) {
			return {
				ok: false,
				playbook: resolvedFile,
				packs: packPaths,
				diagnostics: [packDiagnostic],
			};
		}
		try {
			loadPack(packPath);
		} catch (error) {
			return {
				ok: false,
				playbook: resolvedFile,
				packs: packPaths,
				diagnostics: [
					{
						file: packPath,
						phase: "load",
						detail: error instanceof Error ? error.message : String(error),
					},
				],
			};
		}
	}

	try {
		const merged = loadPlaybookWithPacks(resolvedFile);
		const plan = compilePlaybook(merged);
			const templateDiagnostic = validateTemplatePaths(resolvedFile, merged, new Set(plan.taskIndex.keys()));
		if (templateDiagnostic) {
			return {
				ok: false,
				playbook: resolvedFile,
				packs: packPaths,
				diagnostics: [templateDiagnostic],
			};
		}
		return {
			ok: true,
			playbook: resolvedFile,
			packs: packPaths,
			diagnostics: [],
		};
	} catch (error) {
		return {
			ok: false,
			playbook: resolvedFile,
			packs: packPaths,
			diagnostics: [
				{
					file: resolvedFile,
					phase: "compile",
					detail: error instanceof Error ? error.message : String(error),
				},
			],
		};
	}
}
