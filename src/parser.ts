import { readFileSync } from "node:fs";
import YAML from "yaml";
import { z } from "zod";
import {
	agentSchema,
	agentToolSchema,
	a2aAgentSchema,
	defaultsSchema,
	longTermMemorySchema,
	memorySchema,
	mcpServerSchema,
	moduleSchema,
	outputSchema,
	packSchema,
	playbookSchema,
	policySchema,
	taskSchema,
	workingMemorySchema,
} from "./schema.js";
import type {
	AgentDefinition,
	AgentToolDefinition,
	A2AAgentDefinition,
	GuardrailsPolicy,
	LongTermMemoryDefinition,
	MemoryDefinition,
	McpServerDefinition,
	ModuleDefinition,
	OutputDefinition,
	ProcessModuleDefinition,
	PackDefinition,
	PlaybookDefaults,
	PlaybookDefinition,
	TaskDefinition,
	WorkingMemoryDefinition,
} from "./types.js";
import { resolveRelativePath } from "./utils.js";

function readYamlFile(filePath: string): unknown {
	return YAML.parse(readFileSync(filePath, "utf8"));
}

function normalizeModuleDefinition(module: z.infer<typeof moduleSchema>): ModuleDefinition {
	if (module.kind === "pack.process") {
		return {
			kind: "pack.process",
			command: module.command,
			...(module.args ? { args: module.args } : {}),
			...(module.cwd ? { cwd: module.cwd } : {}),
			...(module.env ? { env: module.env } : {}),
			...(module.runtime
				? {
						runtime: {
							...(module.runtime.requires
								? {
										requires: module.runtime.requires.map((requirement) => ({
											command: requirement.command,
											...(requirement.version ? { version: requirement.version } : {}),
											...(requirement.versionArgs ? { versionArgs: requirement.versionArgs } : {}),
										})),
									}
								: {}),
						},
					}
				: {}),
			...(module.policy
				? {
						policy: {
							...(module.policy.label ? { label: module.policy.label } : {}),
							...(module.policy.capability ? { capability: module.policy.capability } : {}),
							...(module.policy.risk ? { risk: module.policy.risk } : {}),
						},
					}
				: {}),
			...(module.description ? { description: module.description } : {}),
			...(module.with ? { with: module.with } : {}),
			...(module.deterministic !== undefined ? { deterministic: module.deterministic } : {}),
		} satisfies ProcessModuleDefinition;
	}
	return {
		kind: module.kind,
		...(module.description ? { description: module.description } : {}),
		...(module.with ? { with: module.with } : {}),
		...(module.deterministic !== undefined ? { deterministic: module.deterministic } : {}),
	};
}

function normalizeModulePath(filePath: string, moduleDefinition: ModuleDefinition): ModuleDefinition {
	if (moduleDefinition.kind !== "pack.process") {
		return moduleDefinition;
	}
	const isPathLike = (value: string): boolean => value.startsWith(".") || value.includes("/") || value.includes("\\");
	return {
		...moduleDefinition,
		command: isPathLike(moduleDefinition.command)
			? resolveRelativePath(filePath, moduleDefinition.command)
			: moduleDefinition.command,
		...(moduleDefinition.cwd ? { cwd: resolveRelativePath(filePath, moduleDefinition.cwd) } : {}),
	};
}

function normalizeAgentToolDefinition(tool: z.infer<typeof agentToolSchema>): AgentToolDefinition {
	return {
		tool: tool.tool,
		...(tool.name ? { name: tool.name } : {}),
		...(tool.with ? { with: tool.with } : {}),
	};
}

function normalizeAgentDefinition(agent: z.infer<typeof agentSchema>): AgentDefinition {
	return {
		kind: agent.kind,
		instructions: agent.instructions,
		...(agent.description ? { description: agent.description } : {}),
		...(agent.maxTurns !== undefined ? { maxTurns: agent.maxTurns } : {}),
		...(agent.profile ? { profile: agent.profile } : {}),
		...(agent.tools ? { tools: agent.tools.map((tool) => normalizeAgentToolDefinition(tool)) } : {}),
		...(agent.provider ? { provider: agent.provider } : {}),
		...(agent.model ? { model: agent.model } : {}),
		...(agent.baseUrl ? { baseUrl: agent.baseUrl } : {}),
		...(agent.organization ? { organization: agent.organization } : {}),
		...(agent.project ? { project: agent.project } : {}),
		...(agent.endpoint ? { endpoint: agent.endpoint } : {}),
		...(agent.apiVersion ? { apiVersion: agent.apiVersion } : {}),
		...(agent.deployment ? { deployment: agent.deployment } : {}),
		...(agent.temperature !== undefined ? { temperature: agent.temperature } : {}),
		...(agent.maxOutputTokens !== undefined ? { maxOutputTokens: agent.maxOutputTokens } : {}),
		...(agent.reasoningEffort ? { reasoningEffort: agent.reasoningEffort } : {}),
	};
}

function normalizeTaskDefinition(task: z.infer<typeof taskSchema>): TaskDefinition {
	return {
		id: task.id,
		uses: task.uses,
		...(task.needs.length > 0 ? { needs: task.needs } : {}),
		...(Object.keys(task.with).length > 0 ? { with: task.with } : {}),
		...(task.retry
			? {
					retry: {
						...(task.retry.maxAttempts !== undefined ? { maxAttempts: task.retry.maxAttempts } : {}),
						...(task.retry.backoffMs !== undefined ? { backoffMs: task.retry.backoffMs } : {}),
					},
				}
			: {}),
	};
}

function normalizeMcpServerDefinition(definition: z.infer<typeof mcpServerSchema>): McpServerDefinition {
	return {
		...(definition.description ? { description: definition.description } : {}),
		...(definition.url ? { url: definition.url } : {}),
		...(definition.headers ? { headers: definition.headers } : {}),
		...(definition.bearerTokenEnv ? { bearerTokenEnv: definition.bearerTokenEnv } : {}),
	};
}

function normalizeA2AAgentDefinition(definition: z.infer<typeof a2aAgentSchema>): A2AAgentDefinition {
	return {
		...(definition.description ? { description: definition.description } : {}),
		...(definition.url ? { url: definition.url } : {}),
		...(definition.cardUrl ? { cardUrl: definition.cardUrl } : {}),
		...(definition.headers ? { headers: definition.headers } : {}),
		...(definition.bearerTokenEnv ? { bearerTokenEnv: definition.bearerTokenEnv } : {}),
	};
}

function normalizeDefaults(definition: z.infer<typeof defaultsSchema>): PlaybookDefaults {
	return {
		...(definition.agentProfile ? { agentProfile: definition.agentProfile } : {}),
	};
}

function normalizeWorkingMemory(definition: z.infer<typeof workingMemorySchema>): WorkingMemoryDefinition {
	return {
		...(definition.initial ? { initial: definition.initial } : {}),
	};
}

function normalizeLongTermMemory(definition: z.infer<typeof longTermMemorySchema>, playbookPath: string): LongTermMemoryDefinition {
	return {
		...(definition.provider ? { provider: definition.provider } : {}),
		...(definition.dbPath ? { dbPath: resolveRelativePath(playbookPath, definition.dbPath) } : {}),
		...(definition.namespace ? { namespace: definition.namespace } : {}),
		...(definition.connectionString ? { connectionString: definition.connectionString } : {}),
		...(definition.connectionStringEnv ? { connectionStringEnv: definition.connectionStringEnv } : {}),
		...(definition.database ? { database: definition.database } : {}),
		...(definition.collection ? { collection: definition.collection } : {}),
	};
}

function normalizeMemory(definition: z.infer<typeof memorySchema>, playbookPath: string): MemoryDefinition {
	return {
		...(definition.working ? { working: normalizeWorkingMemory(definition.working) } : {}),
		...(definition.longTerm ? { longTerm: normalizeLongTermMemory(definition.longTerm, playbookPath) } : {}),
	};
}

function normalizeOutput(definition: z.infer<typeof outputSchema>): OutputDefinition {
	return {
		...(definition.format ? { format: definition.format } : {}),
		...(definition.verbose !== undefined ? { verbose: definition.verbose } : {}),
		...(definition.color ? { color: definition.color } : {}),
	};
}

function normalizePolicy(definition: z.infer<typeof policySchema>, playbookPath: string): GuardrailsPolicy {
	const workspaceRoot = definition.workspaceRoot
		? resolveRelativePath(playbookPath, definition.workspaceRoot)
		: resolveRelativePath(playbookPath, ".");
	return {
		workspaceRoot,
		...(definition.writableRoots
			? {
					writableRoots: definition.writableRoots.map((root) => resolveRelativePath(playbookPath, root)),
				}
			: { writableRoots: [workspaceRoot] }),
		...(definition.approvalMode ? { approvalMode: definition.approvalMode } : { approvalMode: "never" }),
	};
}

function normalizePackDefinition(parsed: z.infer<typeof packSchema>, packPath: string): PackDefinition {
	return {
		pack: parsed.pack,
		version: parsed.version,
		...(parsed.description ? { description: parsed.description } : {}),
		...(parsed.modules
			? {
					modules: Object.fromEntries(
						Object.entries(parsed.modules).map(([name, definition]) => [
							name,
							normalizeModulePath(packPath, normalizeModuleDefinition(definition)),
						]),
					),
				}
			: {}),
		...(parsed.agents
			? {
					agents: Object.fromEntries(
						Object.entries(parsed.agents).map(([name, definition]) => [name, normalizeAgentDefinition(definition)]),
					),
				}
			: {}),
	};
}

function normalizePlaybookDefinition(parsed: z.infer<typeof playbookSchema>, playbookPath: string): PlaybookDefinition {
	return {
		playbook: parsed.playbook,
		tasks: parsed.tasks.map((task) => normalizeTaskDefinition(task)),
		...(parsed.version ? { version: parsed.version } : {}),
		...(parsed.description ? { description: parsed.description } : {}),
		...(parsed.packs ? { packs: parsed.packs } : {}),
			...(parsed.inputs ? { inputs: parsed.inputs } : {}),
			...(parsed.defaults ? { defaults: normalizeDefaults(parsed.defaults) } : {}),
			...(parsed.memory ? { memory: normalizeMemory(parsed.memory, playbookPath) } : {}),
			...(parsed.output ? { output: normalizeOutput(parsed.output) } : {}),
		...(parsed.policy ? { policy: normalizePolicy(parsed.policy, playbookPath) } : { policy: normalizePolicy({}, playbookPath) }),
		...(parsed.mcpServers
			? {
					mcpServers: Object.fromEntries(
						Object.entries(parsed.mcpServers).map(([name, definition]) => [name, normalizeMcpServerDefinition(definition)]),
					),
				}
			: {}),
		...(parsed.a2aAgents
			? {
					a2aAgents: Object.fromEntries(
						Object.entries(parsed.a2aAgents).map(([name, definition]) => [name, normalizeA2AAgentDefinition(definition)]),
					),
				}
			: {}),
		...(parsed.modules
			? {
					modules: Object.fromEntries(
						Object.entries(parsed.modules).map(([name, definition]) => [
							name,
							normalizeModulePath(playbookPath, normalizeModuleDefinition(definition)),
						]),
					),
				}
			: {}),
		...(parsed.agents
			? {
					agents: Object.fromEntries(
						Object.entries(parsed.agents).map(([name, definition]) => [name, normalizeAgentDefinition(definition)]),
					),
				}
			: {}),
	};
}

export function loadPack(filePath: string): PackDefinition {
	const parsed = packSchema.safeParse(readYamlFile(filePath));
	if (!parsed.success) {
		throw new Error(`Invalid pack file ${filePath}: ${parsed.error.message}`);
	}
	return normalizePackDefinition(parsed.data, filePath);
}

export function loadPlaybook(filePath: string): PlaybookDefinition {
	const parsed = playbookSchema.safeParse(readYamlFile(filePath));
	if (!parsed.success) {
		throw new Error(`Invalid playbook file ${filePath}: ${parsed.error.message}`);
	}
	return normalizePlaybookDefinition(parsed.data, filePath);
}

export function loadPlaybookWithPacks(filePath: string): PlaybookDefinition {
	const playbook = loadPlaybook(filePath);
	const mergedModules = { ...(playbook.modules ?? {}) };
	const mergedAgents = { ...(playbook.agents ?? {}) };

	for (const packPath of playbook.packs ?? []) {
		const absolutePackPath = resolveRelativePath(filePath, packPath);
		const pack = loadPack(absolutePackPath);

		for (const [name, moduleDefinition] of Object.entries(pack.modules ?? {})) {
			mergedModules[`${pack.pack}/${name}`] = moduleDefinition;
		}
		for (const [name, agentDefinition] of Object.entries(pack.agents ?? {})) {
			mergedAgents[`${pack.pack}/${name}`] = agentDefinition;
		}
	}

	return {
		...playbook,
		modules: mergedModules,
		agents: mergedAgents,
	};
}
