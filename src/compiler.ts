import { homedir } from "node:os";
import type { CompiledPlaybook, CompiledTask, PlaybookDefinition } from "./types.js";
import { isBuiltinToolRef } from "./builtin-tools.js";
import { parseTaskUse } from "./utils.js";

function topologicalSort(tasks: CompiledTask[]): CompiledTask[] {
	const inDegree = new Map<string, number>();
	const dependents = new Map<string, string[]>();

	for (const task of tasks) {
		inDegree.set(task.id, task.needs.length);
		for (const dependency of task.needs) {
			const current = dependents.get(dependency) ?? [];
			current.push(task.id);
			dependents.set(dependency, current);
		}
	}

	const ordered: CompiledTask[] = [];
	const queue = tasks.filter((task) => task.needs.length === 0).map((task) => task.id);
	const taskIndex = new Map(tasks.map((task) => [task.id, task]));

	while (queue.length > 0) {
		const currentId = queue.shift()!;
		const task = taskIndex.get(currentId);
		if (!task) continue;
		ordered.push(task);

		for (const dependentId of dependents.get(currentId) ?? []) {
			const nextDegree = (inDegree.get(dependentId) ?? 0) - 1;
			inDegree.set(dependentId, nextDegree);
			if (nextDegree === 0) queue.push(dependentId);
		}
	}

	if (ordered.length !== tasks.length) {
		throw new Error("Task graph contains a cycle");
	}

	return ordered;
}

export function compilePlaybook(definition: PlaybookDefinition): CompiledPlaybook {
	const seen = new Set<string>();
	const compiledTasks: CompiledTask[] = definition.tasks.map((task) => {
		if (seen.has(task.id)) {
			throw new Error(`Duplicate task id "${task.id}"`);
		}
		seen.add(task.id);

		const use = parseTaskUse(task.uses);
		return {
			id: task.id,
			use,
			needs: task.needs ?? [],
			with: task.with ?? {},
			retry: {
				maxAttempts: task.retry?.maxAttempts ?? 1,
				backoffMs: task.retry?.backoffMs ?? 0,
			},
		};
	});

	const taskIds = new Set(compiledTasks.map((task) => task.id));
	for (const task of compiledTasks) {
		for (const dependency of task.needs) {
			if (!taskIds.has(dependency)) {
				throw new Error(`Task "${task.id}" depends on missing task "${dependency}"`);
			}
		}

		if (task.use.kind === "module" && !(definition.modules ?? {})[task.use.ref] && !isBuiltinToolRef(task.use.ref)) {
			throw new Error(`Task "${task.id}" references missing module "${task.use.ref}"`);
		}
		if (task.use.kind === "agent" && !(definition.agents ?? {})[task.use.ref]) {
			throw new Error(`Task "${task.id}" references missing agent "${task.use.ref}"`);
		}
	}

	for (const [agentName, agentDefinition] of Object.entries(definition.agents ?? {})) {
		for (const tool of agentDefinition.tools ?? []) {
			if (isBuiltinToolRef(tool.tool)) {
				continue;
			}
			if (tool.tool.startsWith("mcp:")) {
				const match = /^mcp:([^/]+)\/.+$/.exec(tool.tool);
				if (!match) {
					throw new Error(`Agent "${agentName}" references invalid MCP tool "${tool.tool}"`);
				}
				if (!(definition.mcpServers ?? {})[match[1]!]) {
					throw new Error(`Agent "${agentName}" references undeclared MCP server "${match[1]!}"`);
				}
				continue;
			}
			if (tool.tool.startsWith("a2a:")) {
				const match = /^a2a:(.+)$/.exec(tool.tool);
				if (!match) {
					throw new Error(`Agent "${agentName}" references invalid A2A agent "${tool.tool}"`);
				}
				if (!(definition.a2aAgents ?? {})[match[1]!]) {
					throw new Error(`Agent "${agentName}" references undeclared A2A agent "${match[1]!}"`);
				}
				continue;
			}
			if (!(definition.modules ?? {})[tool.tool]) {
				throw new Error(`Agent "${agentName}" references missing tool "${tool.tool}"`);
			}
		}
	}

	const orderedTasks = topologicalSort(compiledTasks);
	const taskIndex = new Map<string, CompiledTask>();
	const dependents = new Map<string, string[]>();
	for (const task of orderedTasks) {
		taskIndex.set(task.id, task);
		for (const dependency of task.needs) {
			const current = dependents.get(dependency) ?? [];
			current.push(task.id);
			dependents.set(dependency, current);
		}
	}

		return {
			name: definition.playbook,
			inputs: definition.inputs ?? {},
			defaults: {
				agentProfile: definition.defaults?.agentProfile ?? "none",
			},
			memory: {
				working: {
					initial: definition.memory?.working?.initial ?? {},
				},
				longTerm: {
					provider: definition.memory?.longTerm?.provider ?? "sqlite",
					dbPath:
						(definition.memory?.longTerm?.provider ?? "sqlite") === "sqlite"
							? (definition.memory?.longTerm?.dbPath ?? `${homedir()}/.agentctl/memory/long-term.db`)
							: (definition.memory?.longTerm?.dbPath ?? ""),
					namespace: definition.memory?.longTerm?.namespace ?? definition.playbook,
					connectionString: definition.memory?.longTerm?.connectionString ?? "",
					connectionStringEnv: definition.memory?.longTerm?.connectionStringEnv ?? "",
					database: definition.memory?.longTerm?.database ?? "agentctl",
					collection: definition.memory?.longTerm?.collection ?? "long_term_memories",
				},
			},
			output: {
				format: definition.output?.format ?? "yaml",
			verbose: definition.output?.verbose ?? false,
			color: definition.output?.color ?? "auto",
		},
		policy: {
			workspaceRoot: definition.policy?.workspaceRoot ?? process.cwd(),
			writableRoots: definition.policy?.writableRoots ?? [definition.policy?.workspaceRoot ?? process.cwd()],
			approvalMode: definition.policy?.approvalMode ?? "never",
		},
		mcpServers: definition.mcpServers ?? {},
		a2aAgents: definition.a2aAgents ?? {},
		modules: definition.modules ?? {},
		agents: definition.agents ?? {},
		tasks: orderedTasks,
		taskIndex,
		dependents,
		...(definition.description ? { description: definition.description } : {}),
	};
}
