import { z } from "zod";
import type { JsonValue } from "./types.js";

const jsonLiteralSchema = z.union([z.string(), z.number(), z.boolean(), z.null()]);

export const jsonValueSchema: z.ZodType<JsonValue> = z.lazy(() =>
	z.union([jsonLiteralSchema, z.array(jsonValueSchema), z.record(z.string(), jsonValueSchema)]),
);

const retrySchema = z.object({
	maxAttempts: z.number().int().positive().max(20).default(1),
	backoffMs: z.number().int().nonnegative().max(60_000).default(0),
});

const moduleKindSchema = z.enum([
	"builtin.assign",
	"builtin.assert",
	"builtin.shell.exec",
	"builtin.read",
	"builtin.write",
	"builtin.edit",
	"builtin.grep",
	"builtin.find",
	"builtin.ls",
	"builtin.memory.read",
	"builtin.memory.write",
	"builtin.long_term_memory.write",
	"builtin.long_term_memory.search",
	"builtin.long_term_memory.retrieve",
	"pack.process",
]);
const agentKindSchema = z.enum(["builtin.heuristic", "openai.responses"]);
const agentProfileSchema = z.enum(["none", "inspect", "workspace_write", "workspace_exec"]);
const approvalModeSchema = z.enum(["never", "on-mutate", "on-act", "always"]);
const reasoningEffortSchema = z.enum(["minimal", "low", "medium", "high"]);
const outputFormatSchema = z.enum(["yaml", "json"]);
const outputColorModeSchema = z.enum(["auto", "always", "never"]);
const longTermProviderSchema = z.enum(["sqlite", "mongodb-atlas"]);

const processRequirementSchema = z.object({
	command: z.string().min(1),
	version: z.string().min(1).optional(),
	versionArgs: z.array(z.string().min(1)).optional(),
});

const processRuntimeSchema = z.object({
	requires: z.array(processRequirementSchema).optional(),
});

const modulePolicyOverrideSchema = z.object({
	label: z.string().min(1).optional(),
	capability: z.enum(["internal", "observe", "mutate", "act"]).optional(),
	risk: z.enum(["low", "medium", "high"]).optional(),
});

const builtinModuleSchema = z.object({
	kind: moduleKindSchema.exclude(["pack.process"]),
	description: z.string().optional(),
	with: z.record(z.string(), jsonValueSchema).optional(),
	deterministic: z.boolean().optional(),
});

const processModuleSchema = z.object({
	kind: z.literal("pack.process"),
	description: z.string().optional(),
	with: z.record(z.string(), jsonValueSchema).optional(),
	deterministic: z.boolean().optional(),
	command: z.string().min(1),
	args: z.array(z.string()).optional(),
	cwd: z.string().min(1).optional(),
	env: z.record(z.string(), z.string()).optional(),
	runtime: processRuntimeSchema.optional(),
	policy: modulePolicyOverrideSchema.optional(),
});

export const moduleSchema = z.union([builtinModuleSchema, processModuleSchema]);

export const agentToolSchema = z.object({
	tool: z.string().min(1),
	name: z.string().min(1).optional(),
	with: z.record(z.string(), jsonValueSchema).optional(),
});

export const agentSchema = z.object({
	kind: agentKindSchema,
	description: z.string().optional(),
	instructions: z.string().min(1),
	maxTurns: z.number().int().positive().max(20).optional(),
	profile: agentProfileSchema.optional(),
	tools: z.array(agentToolSchema).optional(),
	provider: z.string().min(1).optional(),
	model: z.string().min(1).optional(),
	baseUrl: z.string().url().optional(),
	organization: z.string().min(1).optional(),
	project: z.string().min(1).optional(),
	endpoint: z.string().url().optional(),
	apiVersion: z.string().min(1).optional(),
	deployment: z.string().min(1).optional(),
	temperature: z.number().min(0).max(2).optional(),
	maxOutputTokens: z.number().int().positive().optional(),
	reasoningEffort: reasoningEffortSchema.optional(),
});

export const defaultsSchema = z.object({
	agentProfile: agentProfileSchema.optional(),
});

export const workingMemorySchema = z.object({
	initial: z.record(z.string(), jsonValueSchema).optional(),
});

export const longTermMemorySchema = z.object({
	provider: longTermProviderSchema.optional(),
	dbPath: z.string().min(1).optional(),
	namespace: z.string().min(1).optional(),
	connectionString: z.string().min(1).optional(),
	connectionStringEnv: z.string().min(1).optional(),
	database: z.string().min(1).optional(),
	collection: z.string().min(1).optional(),
});

export const memorySchema = z.object({
	working: workingMemorySchema.optional(),
	longTerm: longTermMemorySchema.optional(),
});

export const outputSchema = z.object({
	format: outputFormatSchema.optional(),
	verbose: z.boolean().optional(),
	color: outputColorModeSchema.optional(),
});

export const policySchema = z.object({
	workspaceRoot: z.string().min(1).optional(),
	writableRoots: z.array(z.string().min(1)).optional(),
	approvalMode: approvalModeSchema.optional(),
});

export const mcpServerSchema = z.object({
	description: z.string().optional(),
	url: z.string().url().optional(),
	headers: z.record(z.string(), z.string()).optional(),
	bearerTokenEnv: z.string().min(1).optional(),
});

export const a2aAgentSchema = z.object({
	description: z.string().optional(),
	url: z.string().url().optional(),
	cardUrl: z.string().url().optional(),
	headers: z.record(z.string(), z.string()).optional(),
	bearerTokenEnv: z.string().min(1).optional(),
});

export const taskSchema = z.object({
	id: z.string().min(1).regex(/^[a-zA-Z][a-zA-Z0-9_-]*$/),
	uses: z.string().min(1).regex(/^(module|agent):.+$/),
	needs: z.array(z.string().min(1)).default([]),
	with: z.record(z.string(), jsonValueSchema).default({}),
	retry: retrySchema.partial().optional(),
});

export const packSchema = z.object({
	pack: z.string().min(1),
	version: z.string().min(1),
	description: z.string().optional(),
	modules: z.record(z.string(), moduleSchema).optional(),
	agents: z.record(z.string(), agentSchema).optional(),
});

export const playbookSchema = z.object({
	playbook: z.string().min(1),
	version: z.string().optional(),
	description: z.string().optional(),
	packs: z.array(z.string().min(1)).optional(),
	inputs: z.record(z.string(), jsonValueSchema).optional(),
	defaults: defaultsSchema.optional(),
	memory: memorySchema.optional(),
	output: outputSchema.optional(),
	policy: policySchema.optional(),
	mcpServers: z.record(z.string(), mcpServerSchema).optional(),
	a2aAgents: z.record(z.string(), a2aAgentSchema).optional(),
	modules: z.record(z.string(), moduleSchema).optional(),
	agents: z.record(z.string(), agentSchema).optional(),
	tasks: z.array(taskSchema).min(1),
});
