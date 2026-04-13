import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { homedir } from "node:os";
import { getEnvProviderConfig, type ProviderEnvConfig } from "./env-api-keys.js";

export interface AuthStorageBackend {
	withLock<T>(fn: (current: string | undefined) => { result: T; next?: string }): T;
}

export class FileAuthStorageBackend implements AuthStorageBackend {
	constructor(private readonly authPath: string = join(homedir(), ".agentctl", "auth.json")) {}

	private ensureParentDir(): void {
		const dir = dirname(this.authPath);
		if (!existsSync(dir)) {
			mkdirSync(dir, { recursive: true, mode: 0o700 });
		}
	}

	private ensureFileExists(): void {
		if (!existsSync(this.authPath)) {
			writeFileSync(this.authPath, "{}", "utf8");
			chmodSync(this.authPath, 0o600);
		}
	}

	withLock<T>(fn: (current: string | undefined) => { result: T; next?: string }): T {
		this.ensureParentDir();
		this.ensureFileExists();
		const current = existsSync(this.authPath) ? readFileSync(this.authPath, "utf8") : undefined;
		const { result, next } = fn(current);
		if (next !== undefined) {
			writeFileSync(this.authPath, next, "utf8");
			chmodSync(this.authPath, 0o600);
		}
		return result;
	}
}

export class InMemoryAuthStorageBackend implements AuthStorageBackend {
	private value: string | undefined = "{}";

	withLock<T>(fn: (current: string | undefined) => { result: T; next?: string }): T {
		const { result, next } = fn(this.value);
		if (next !== undefined) {
			this.value = next;
		}
		return result;
	}
}

export interface ApiKeyCredential {
	type: "api_key";
	key: string;
	baseUrl?: string;
	organization?: string;
	project?: string;
	endpoint?: string;
	apiVersion?: string;
	deployment?: string;
}

export type AuthCredential = ApiKeyCredential;
type StoredCredentialInput = string | ApiKeyCredential;
type AuthStorageData = Record<string, StoredCredentialInput>;

export interface ResolvedProviderAuth extends ProviderEnvConfig {
	provider: string;
	source: "runtime_override" | "stored" | "env" | "missing";
	configured: boolean;
}

function normalizeCredential(credential: StoredCredentialInput): ApiKeyCredential {
	if (typeof credential === "string") {
		return { type: "api_key", key: credential };
	}
	return credential;
}

export class AuthStorage {
	private readonly runtimeOverrides = new Map<string, string>();
	private data: Record<string, ApiKeyCredential> = {};

	private constructor(private readonly backend: AuthStorageBackend) {
		this.reload();
	}

	static create(authPath?: string): AuthStorage {
		return new AuthStorage(new FileAuthStorageBackend(authPath));
	}

	static inMemory(data: Record<string, StoredCredentialInput> = {}): AuthStorage {
		const backend = new InMemoryAuthStorageBackend();
		const storage = new AuthStorage(backend);
		for (const [provider, credential] of Object.entries(data)) {
			storage.set(provider, credential);
		}
		return storage;
	}

	reload(): void {
		this.backend.withLock((current) => {
			const parsed = current ? (JSON.parse(current) as AuthStorageData) : {};
			this.data = Object.fromEntries(
				Object.entries(parsed).map(([provider, credential]) => [provider, normalizeCredential(credential)]),
			);
			return { result: undefined };
		});
	}

	setRuntimeApiKey(provider: string, apiKey: string): void {
		this.runtimeOverrides.set(provider, apiKey);
	}

	set(provider: string, credential: StoredCredentialInput): void {
		const normalized = normalizeCredential(credential);
		this.data[provider] = normalized;
		this.backend.withLock((current) => {
			const parsed = current ? (JSON.parse(current) as AuthStorageData) : {};
			parsed[provider] = normalized;
			return { result: undefined, next: JSON.stringify(parsed, null, 2) };
		});
	}

	getApiKey(provider: string): string | undefined {
		return this.getResolvedProviderAuth(provider).apiKey;
	}

	hasAuth(provider: string): boolean {
		return this.getApiKey(provider) !== undefined;
	}

	getCredential(provider: string): ApiKeyCredential | undefined {
		return this.data[provider];
	}

	getResolvedProviderAuth(provider: string): ResolvedProviderAuth {
		const env = getEnvProviderConfig(provider);
		const stored = this.data[provider];
		const runtimeKey = this.runtimeOverrides.get(provider);

		const apiKey = runtimeKey ?? stored?.key ?? env.apiKey;
		const source = runtimeKey
			? "runtime_override"
			: stored?.key
				? "stored"
				: env.apiKey
					? "env"
					: "missing";

		return {
			provider,
			configured: apiKey !== undefined,
			source,
			...(apiKey ? { apiKey } : {}),
			...(stored?.baseUrl ?? env.baseUrl ? { baseUrl: stored?.baseUrl ?? env.baseUrl } : {}),
			...(stored?.organization ?? env.organization ? { organization: stored?.organization ?? env.organization } : {}),
			...(stored?.project ?? env.project ? { project: stored?.project ?? env.project } : {}),
			...(stored?.endpoint ?? env.endpoint ? { endpoint: stored?.endpoint ?? env.endpoint } : {}),
			...(stored?.apiVersion ?? env.apiVersion ? { apiVersion: stored?.apiVersion ?? env.apiVersion } : {}),
			...(stored?.deployment ?? env.deployment ? { deployment: stored?.deployment ?? env.deployment } : {}),
		};
	}

	inspect(provider: string): {
		provider: string;
		configured: boolean;
		source: "runtime_override" | "stored" | "env" | "missing";
	} {
		const resolved = this.getResolvedProviderAuth(provider);
		return {
			provider: resolved.provider,
			configured: resolved.configured,
			source: resolved.source,
		};
	}
}
