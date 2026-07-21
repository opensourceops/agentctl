import { createHash } from "node:crypto";
import { resolveTemplatesStrict } from "./template-utils.js";
import { buildTaskTemplateContext } from "./utils.js";
import type {
	AgentDefinition,
	CompiledPlaybook,
	JsonObject,
	PromptCacheDefinition,
	ResolvedPromptCacheConfig,
	RuntimeSnapshot,
} from "./types.js";

const TASK_TEMPLATE_BASE_KEY = "__agentctlTaskBase";

function toProviderRetention(retention: "in_memory" | "24h"): "in-memory" | "24h" {
	return retention === "in_memory" ? "in-memory" : "24h";
}

function mergePromptCacheDefinition(
	playbookPromptCache: CompiledPlaybook["promptCache"],
	agentPromptCache: PromptCacheDefinition | undefined,
): CompiledPlaybook["promptCache"] {
	return {
		...playbookPromptCache,
		...(agentPromptCache ?? {}),
	};
}

function fingerprintAgentPrefix(agent: AgentDefinition): string {
	return createHash("sha256")
		.update(
			JSON.stringify({
				kind: agent.kind,
				provider: agent.provider ?? "openai",
				model: agent.model ?? "",
				instructions: agent.instructions,
				tools: (agent.tools ?? []).map((tool) => ({
					tool: tool.tool,
					name: tool.name ?? "",
					withKeys: Object.keys(tool.with ?? {}).sort(),
				})),
			}),
		)
		.digest("hex")
		.slice(0, 12);
}

export function isDirectOpenAIBaseUrl(baseUrl: string | undefined): boolean {
	if (!baseUrl) {
		return true;
	}
	try {
		return new URL(baseUrl).hostname === "api.openai.com";
	} catch {
		return false;
	}
}

export function inspectPromptCacheSupport(input: {
	agent: AgentDefinition;
	agentRef: string;
	playbookPromptCache: CompiledPlaybook["promptCache"];
}): {
	requested: boolean;
	force: boolean;
	eligible: boolean;
	reason: string;
	provider: string;
	baseUrl?: string;
	directOpenAiBaseUrl: boolean;
	config: CompiledPlaybook["promptCache"];
} {
	const provider = input.agent.provider ?? "openai";
	const config = mergePromptCacheDefinition(input.playbookPromptCache, input.agent.promptCache);
	const requested = Boolean(config.enabled);
	const force = Boolean(config.force);
	const baseUrl = input.agent.baseUrl;
	const directOpenAiBaseUrl = isDirectOpenAIBaseUrl(baseUrl);

	if (input.agent.kind !== "openai.responses") {
		return {
			requested,
			force,
			eligible: false,
			reason: `Agent "${input.agentRef}" is not using openai.responses`,
			provider,
			...(baseUrl ? { baseUrl } : {}),
			directOpenAiBaseUrl,
			config,
		};
	}

	if (provider !== "openai") {
		return {
			requested,
			force,
			eligible: false,
			reason: `Provider "${provider}" does not support prompt cache in agentctl`,
			provider,
			...(baseUrl ? { baseUrl } : {}),
			directOpenAiBaseUrl,
			config,
		};
	}

	if (!directOpenAiBaseUrl && !force) {
		return {
			requested,
			force,
			eligible: false,
			reason:
				"Custom OpenAI-compatible base URLs disable prompt cache by default; set promptCache.force: true to opt in",
			provider,
			...(baseUrl ? { baseUrl } : {}),
			directOpenAiBaseUrl,
			config,
		};
	}

	return {
		requested,
		force,
		eligible: true,
		reason: directOpenAiBaseUrl
			? "Direct OpenAI base URL supports provider-native prompt cache"
			: "Prompt cache forced for a custom OpenAI-compatible base URL",
		provider,
		...(baseUrl ? { baseUrl } : {}),
		directOpenAiBaseUrl,
		config,
	};
}

function buildPromptCacheSubject(input: {
	config: CompiledPlaybook["promptCache"];
	playbookName: string;
	runId: string;
	agentRef: string;
	provider: string;
	snapshot: RuntimeSnapshot;
	taskInput: JsonObject;
}): string {
	if (input.config.shareMode === "group" && input.config.group) {
		return `group:${input.config.group}`;
	}

	if (input.config.keyScope === "custom" && input.config.keyTemplate) {
		const templateContext = buildTaskTemplateContext(input.snapshot, {}, input.taskInput);
		const runContext = templateContext.run;
		const nextRunContext = {
			...(runContext && typeof runContext === "object" ? runContext : {}),
			id: input.runId,
			playbook: input.playbookName,
		};
		const taskBase =
			templateContext[TASK_TEMPLATE_BASE_KEY] &&
			typeof templateContext[TASK_TEMPLATE_BASE_KEY] === "object"
				? (templateContext[TASK_TEMPLATE_BASE_KEY] as Record<string, unknown>)
				: {};
		const rendered = resolveTemplatesStrict(input.config.keyTemplate, {
			...templateContext,
			run: nextRunContext,
			[TASK_TEMPLATE_BASE_KEY]: {
				...taskBase,
				run: nextRunContext,
			},
		});
		return `custom:${String(rendered)}`;
	}

	switch (input.config.keyScope) {
		case "run":
			return `run:${input.playbookName}:${input.runId}`;
		case "playbook":
			return `playbook:${input.playbookName}`;
		case "provider":
			return `provider:${input.provider}`;
		case "custom":
			return `custom:${input.playbookName}:${input.agentRef}`;
		case "agent":
		default:
			return `agent:${input.playbookName}:${input.agentRef}`;
	}
}

export function resolvePromptCacheConfig(input: {
	playbook: CompiledPlaybook;
	agent: AgentDefinition;
	agentRef: string;
	runId: string;
	snapshot: RuntimeSnapshot;
	taskInput: JsonObject;
}): ResolvedPromptCacheConfig {
	const support = inspectPromptCacheSupport({
		agent: input.agent,
		agentRef: input.agentRef,
		playbookPromptCache: input.playbook.promptCache,
	});
	const keyBase = support.requested && support.eligible
		? [
				"agentctl",
				support.provider,
				buildPromptCacheSubject({
					config: support.config,
					playbookName: input.playbook.name,
					runId: input.runId,
					agentRef: input.agentRef,
					provider: support.provider,
					snapshot: input.snapshot,
					taskInput: input.taskInput,
				}),
				fingerprintAgentPrefix(input.agent),
			].join(":")
		: undefined;

	return {
		requested: support.requested,
		enabled: support.requested && support.eligible,
		force: support.force,
		eligible: support.eligible,
		...(!support.eligible ? { disabledReason: support.reason } : {}),
		provider: support.provider,
		...(support.baseUrl ? { baseUrl: support.baseUrl } : {}),
		directOpenAiBaseUrl: support.directOpenAiBaseUrl,
		retention: toProviderRetention(support.config.retention),
		keyScope: support.config.keyScope,
		shareMode: support.config.shareMode,
		...(support.config.group ? { group: support.config.group } : {}),
		...(support.config.keyTemplate ? { keyTemplate: support.config.keyTemplate } : {}),
		...(keyBase ? { keyBase } : {}),
	};
}
