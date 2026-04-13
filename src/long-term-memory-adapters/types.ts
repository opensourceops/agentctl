import type {
	JsonValue,
} from "../types.js";
import type {
	LongTermMemoryEntry,
	LongTermMemorySearchResult,
	LongTermMemoryStats,
} from "../long-term-memory.js";

export type LongTermMemoryAdapterKind =
	| "sqlite"
	| "mongodb-atlas"
	| "postgres"
	| "pgvector"
	| "elasticsearch"
	| "qdrant"
	| "weaviate"
	| "pinecone"
	| "document"
	| "graph";

export interface LongTermMemoryAdapterConfig {
	kind: LongTermMemoryAdapterKind;
	name: string;
	description: string;
}

export interface LongTermMemoryDeletedEntry {
	namespace: string;
	key: string;
	updatedAt: string;
}

export interface LongTermMemoryGarbageCollectResult {
	deletedEntries: LongTermMemoryDeletedEntry[];
	vacuumed: boolean;
}

export interface LongTermMemoryAdapter {
	readonly config: LongTermMemoryAdapterConfig;
	close(): void | Promise<void>;
	write(namespace: string, key: string, value: JsonValue, tags?: string[]): LongTermMemoryEntry | Promise<LongTermMemoryEntry>;
	get(namespace: string, key: string): LongTermMemoryEntry | undefined | Promise<LongTermMemoryEntry | undefined>;
	search(
		namespace: string | undefined,
		query: string | undefined,
		key: string | undefined,
		limit: number,
	): LongTermMemorySearchResult | Promise<LongTermMemorySearchResult>;
	getStats(namespace?: string): LongTermMemoryStats | Promise<LongTermMemoryStats>;
	garbageCollect(options: {
		namespace?: string;
		olderThanDays: number;
		keepEntries: number;
		vacuum?: boolean;
	}): LongTermMemoryGarbageCollectResult | Promise<LongTermMemoryGarbageCollectResult>;
}

export abstract class PlaceholderLongTermMemoryAdapter implements LongTermMemoryAdapter {
	constructor(public readonly config: LongTermMemoryAdapterConfig) {}

	close(): void {}

	write(): LongTermMemoryEntry {
		throw new Error(this.notImplementedMessage());
	}

	get(): LongTermMemoryEntry | undefined {
		throw new Error(this.notImplementedMessage());
	}

	search(): LongTermMemorySearchResult {
		throw new Error(this.notImplementedMessage());
	}

	getStats(): LongTermMemoryStats {
		throw new Error(this.notImplementedMessage());
	}

	garbageCollect(): LongTermMemoryGarbageCollectResult {
		throw new Error(this.notImplementedMessage());
	}

	protected notImplementedMessage(): string {
		return `${this.config.name} long-term memory adapter is a placeholder. Implement this adapter before enabling it in runtime or CLI flows.`;
	}
}
