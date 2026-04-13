import type { AgentDefinition } from "./types.js";
import { AuthStorage, type ResolvedProviderAuth } from "./auth-storage.js";

export interface ResolvedAgentModelConfig {
	provider: string;
	model: string;
	apiKey: string;
	baseUrl?: string;
	organization?: string;
	project?: string;
	endpoint?: string;
	apiVersion?: string;
	deployment?: string;
	temperature?: number;
	maxOutputTokens?: number;
	reasoningEffort?: "minimal" | "low" | "medium" | "high";
}

export interface AgentAuthInspection {
	provider: string;
	configured: boolean;
	source: "runtime_override" | "stored" | "env" | "missing";
	issues: string[];
}

function getProvider(definition: AgentDefinition): string {
	return definition.provider ?? "openai";
}

function getOptionalConfig(definition: AgentDefinition, auth: ResolvedProviderAuth): Omit<ResolvedAgentModelConfig, "provider" | "model" | "apiKey"> {
	return {
		...(definition.baseUrl ?? auth.baseUrl ? { baseUrl: definition.baseUrl ?? auth.baseUrl } : {}),
		...(definition.organization ?? auth.organization ? { organization: definition.organization ?? auth.organization } : {}),
		...(definition.project ?? auth.project ? { project: definition.project ?? auth.project } : {}),
		...(definition.endpoint ?? auth.endpoint ? { endpoint: definition.endpoint ?? auth.endpoint } : {}),
		...(definition.apiVersion ?? auth.apiVersion ? { apiVersion: definition.apiVersion ?? auth.apiVersion } : {}),
		...(definition.deployment ?? auth.deployment ? { deployment: definition.deployment ?? auth.deployment } : {}),
		...(definition.temperature !== undefined ? { temperature: definition.temperature } : {}),
		...(definition.maxOutputTokens !== undefined ? { maxOutputTokens: definition.maxOutputTokens } : {}),
		...(definition.reasoningEffort ? { reasoningEffort: definition.reasoningEffort } : {}),
	};
}

export class ModelRegistry {
	constructor(private readonly authStorage: AuthStorage) {}

	inspectAgent(definition: AgentDefinition): AgentAuthInspection {
		const provider = getProvider(definition);
		const auth = this.authStorage.getResolvedProviderAuth(provider);
		const issues: string[] = [];

		if (!definition.model) {
			issues.push(`Agent kind "${definition.kind}" requires a model`);
		}
		if (!auth.apiKey) {
			issues.push(`No API key configured for provider "${provider}"`);
		}
		if (provider === "azure-openai-responses") {
			const endpoint = definition.endpoint ?? auth.endpoint ?? definition.baseUrl ?? auth.baseUrl;
			const apiVersion = definition.apiVersion ?? auth.apiVersion;
			if (!endpoint) {
				issues.push('Azure OpenAI requires "endpoint" or "baseUrl"');
			}
			if (!apiVersion) {
				issues.push('Azure OpenAI requires "apiVersion"');
			}
		}

		return {
			provider,
			configured: issues.length === 0,
			source: auth.source,
			issues,
		};
	}

	resolveAgent(definition: AgentDefinition): ResolvedAgentModelConfig {
		const inspection = this.inspectAgent(definition);
		if (!definition.model) {
			throw new Error(`Agent kind "${definition.kind}" requires a model`);
		}
		if (!inspection.configured) {
			throw new Error(inspection.issues[0] ?? `Provider "${inspection.provider}" is not configured`);
		}

		const provider = getProvider(definition);
		const auth = this.authStorage.getResolvedProviderAuth(provider);
		if (!auth.apiKey) {
			throw new Error(`No API key configured for provider "${provider}"`);
		}

		return {
			provider,
			model: definition.model,
			apiKey: auth.apiKey,
			...getOptionalConfig(definition, auth),
		};
	}
}
