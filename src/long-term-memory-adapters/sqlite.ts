import type { JsonValue } from "../types.js";
import {
	LongTermMemoryStore,
	type LongTermMemoryEntry,
	type LongTermMemorySearchResult,
	type LongTermMemoryStats,
} from "../long-term-memory.js";
import type {
	LongTermMemoryAdapter,
	LongTermMemoryAdapterConfig,
	LongTermMemoryGarbageCollectResult,
} from "./types.js";

export const SQLITE_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "sqlite",
	name: "sqlite",
	description: "Built-in local SQLite adapter for long-term memory.",
};

export class SqliteLongTermMemoryAdapter implements LongTermMemoryAdapter {
	readonly config = SQLITE_LONG_TERM_MEMORY_ADAPTER;

	constructor(private readonly store: LongTermMemoryStore) {}

	close(): void {
		this.store.close();
	}

	write(namespace: string, key: string, value: JsonValue, tags: string[] = []): LongTermMemoryEntry {
		return this.store.write(namespace, key, value, tags);
	}

	get(namespace: string, key: string): LongTermMemoryEntry | undefined {
		return this.store.get(namespace, key);
	}

	search(namespace: string | undefined, query: string | undefined, key: string | undefined, limit: number): LongTermMemorySearchResult {
		return this.store.search(namespace, query, key, limit);
	}

	getStats(namespace?: string): LongTermMemoryStats {
		return this.store.getStats(namespace);
	}

	garbageCollect(options: {
		namespace?: string;
		olderThanDays: number;
		keepEntries: number;
		vacuum?: boolean;
	}): LongTermMemoryGarbageCollectResult {
		return this.store.garbageCollect(options);
	}
}
