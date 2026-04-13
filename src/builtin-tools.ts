import { basename } from "node:path";
import type { AgentProfileName, BuiltinModuleDefinition, ModuleDefinition, ModuleKind, ToolCapability, ToolPolicySpec } from "./types.js";

const BUILTIN_TOOL_ALIASES: Record<string, BuiltinModuleDefinition["kind"]> = {
	"builtin/read": "builtin.read",
	"builtin/write": "builtin.write",
	"builtin/edit": "builtin.edit",
	"builtin/grep": "builtin.grep",
	"builtin/find": "builtin.find",
	"builtin/ls": "builtin.ls",
	"builtin/bash": "builtin.shell.exec",
	"builtin/memory-read": "builtin.memory.read",
	"builtin/memory-write": "builtin.memory.write",
	"builtin/long-term-memory-write": "builtin.long_term_memory.write",
	"builtin/long-term-memory-search": "builtin.long_term_memory.search",
	"builtin/long-term-memory-retrieve": "builtin.long_term_memory.retrieve",
};

const MODULE_POLICY_SPECS: Record<BuiltinModuleDefinition["kind"], Omit<ToolPolicySpec, "ref">> = {
	"builtin.assign": {
		provider: "builtin",
		label: "assign",
		capability: "internal",
		risk: "low",
	},
	"builtin.assert": {
		provider: "builtin",
		label: "assert",
		capability: "internal",
		risk: "low",
	},
	"builtin.shell.exec": {
		provider: "builtin",
		label: "bash",
		capability: "act",
		risk: "high",
	},
	"builtin.read": {
		provider: "builtin",
		label: "read",
		capability: "observe",
		risk: "low",
	},
	"builtin.write": {
		provider: "builtin",
		label: "write",
		capability: "mutate",
		risk: "medium",
	},
	"builtin.edit": {
		provider: "builtin",
		label: "edit",
		capability: "mutate",
		risk: "medium",
	},
	"builtin.grep": {
		provider: "builtin",
		label: "grep",
		capability: "observe",
		risk: "low",
	},
	"builtin.find": {
		provider: "builtin",
		label: "find",
		capability: "observe",
		risk: "low",
	},
	"builtin.ls": {
		provider: "builtin",
		label: "ls",
		capability: "observe",
		risk: "low",
	},
	"builtin.memory.read": {
		provider: "builtin",
		label: "memory.read",
		capability: "internal",
		risk: "low",
	},
	"builtin.memory.write": {
		provider: "builtin",
		label: "memory.write",
		capability: "internal",
		risk: "low",
	},
	"builtin.long_term_memory.write": {
		provider: "builtin",
		label: "long_term_memory.write",
		capability: "mutate",
		risk: "medium",
	},
	"builtin.long_term_memory.search": {
		provider: "builtin",
		label: "long_term_memory.search",
		capability: "observe",
		risk: "low",
	},
	"builtin.long_term_memory.retrieve": {
		provider: "builtin",
		label: "long_term_memory.retrieve",
		capability: "internal",
		risk: "low",
	},
};

const AGENT_PROFILE_CAPABILITIES: Record<AgentProfileName, ToolCapability[]> = {
	none: ["internal"],
	inspect: ["internal", "observe"],
	workspace_write: ["internal", "observe", "mutate"],
	workspace_exec: ["internal", "observe", "mutate", "act"],
};

function getBuiltinPolicySpec(kind: BuiltinModuleDefinition["kind"], ref?: string): ToolPolicySpec {
	return {
		ref: ref ?? kind,
		...MODULE_POLICY_SPECS[kind],
	};
}

export function getModulePolicySpec(definition: ModuleDefinition, ref?: string): ToolPolicySpec {
	if (definition.kind !== "pack.process") {
		return getBuiltinPolicySpec(definition.kind, ref);
	}
	return {
		ref: ref ?? definition.command,
		provider: "module",
		label: definition.policy?.label ?? basename(definition.command),
		capability: definition.policy?.capability ?? "act",
		risk: definition.policy?.risk ?? "high",
	};
}

export function agentProfileAllowsCapability(profile: AgentProfileName, capability: ToolCapability): boolean {
	return AGENT_PROFILE_CAPABILITIES[profile].includes(capability);
}

export function resolveBuiltinToolRef(ref: string): BuiltinModuleDefinition["kind"] | undefined {
	return BUILTIN_TOOL_ALIASES[ref];
}

export function isBuiltinToolRef(ref: string): boolean {
	return ref in BUILTIN_TOOL_ALIASES || ref.startsWith("builtin.");
}
