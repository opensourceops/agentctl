export type KnownProvider = "openai" | "azure-openai-responses" | "anthropic" | "google" | "google-vertex" | "amazon-bedrock";

export interface ProviderEnvConfig {
	apiKey?: string;
	baseUrl?: string;
	organization?: string;
	project?: string;
	endpoint?: string;
	apiVersion?: string;
	deployment?: string;
}

function withField<K extends keyof ProviderEnvConfig>(
	key: K,
	value: ProviderEnvConfig[K] | undefined,
): Partial<ProviderEnvConfig> {
	return value === undefined ? {} : { [key]: value };
}

function normalizeEnvCredential(value: string | undefined): string | undefined {
	if (value === undefined) {
		return undefined;
	}
	return value.trim() === "" ? undefined : value;
}

export function getEnvApiKey(provider: string): string | undefined {
	return getEnvProviderConfig(provider).apiKey;
}

export function getEnvProviderConfig(provider: string): ProviderEnvConfig {
	if (provider === "anthropic") {
		const apiKey =
			normalizeEnvCredential(process.env.ANTHROPIC_OAUTH_TOKEN) ?? normalizeEnvCredential(process.env.ANTHROPIC_API_KEY);
		return {
			...withField("apiKey", apiKey),
		};
	}

	if (provider === "google-vertex") {
		const apiKey = normalizeEnvCredential(process.env.GOOGLE_CLOUD_API_KEY);
		if (apiKey) {
			return { apiKey };
		}
		const hasProject = !!(process.env.GOOGLE_CLOUD_PROJECT || process.env.GCLOUD_PROJECT);
		const hasLocation = !!process.env.GOOGLE_CLOUD_LOCATION;
		if (hasProject && hasLocation) {
			return { apiKey: "<authenticated>" };
		}
	}

	if (provider === "amazon-bedrock") {
		if (
			process.env.AWS_PROFILE ||
			(process.env.AWS_ACCESS_KEY_ID && process.env.AWS_SECRET_ACCESS_KEY) ||
			process.env.AWS_BEARER_TOKEN_BEDROCK ||
			process.env.AWS_CONTAINER_CREDENTIALS_RELATIVE_URI ||
			process.env.AWS_CONTAINER_CREDENTIALS_FULL_URI ||
			process.env.AWS_WEB_IDENTITY_TOKEN_FILE
		) {
			return { apiKey: "<authenticated>" };
		}
	}

	const envMap: Record<string, string> = {
		openai: "OPENAI_API_KEY",
		"azure-openai-responses": "AZURE_OPENAI_API_KEY",
		google: "GEMINI_API_KEY",
	};

	const envVar = envMap[provider];
	const apiKey = envVar ? normalizeEnvCredential(process.env[envVar]) : undefined;

	if (provider === "openai") {
		const baseUrl = normalizeEnvCredential(process.env.OPENAI_BASE_URL);
		const organization = normalizeEnvCredential(process.env.OPENAI_ORG_ID);
		const project = normalizeEnvCredential(process.env.OPENAI_PROJECT_ID);
		return {
			...withField("apiKey", apiKey),
			...withField("baseUrl", baseUrl),
			...withField("organization", organization),
			...withField("project", project),
		};
	}

	if (provider === "azure-openai-responses") {
		const baseUrl = normalizeEnvCredential(process.env.OPENAI_BASE_URL);
		const endpoint = normalizeEnvCredential(process.env.AZURE_OPENAI_ENDPOINT);
		const apiVersion = normalizeEnvCredential(process.env.OPENAI_API_VERSION);
		const organization = normalizeEnvCredential(process.env.OPENAI_ORG_ID);
		const project = normalizeEnvCredential(process.env.OPENAI_PROJECT_ID);
		return {
			...withField("apiKey", apiKey),
			...withField("baseUrl", baseUrl),
			...withField("endpoint", endpoint),
			...withField("apiVersion", apiVersion),
			...withField("organization", organization),
			...withField("project", project),
		};
	}

	return withField("apiKey", apiKey);
}
