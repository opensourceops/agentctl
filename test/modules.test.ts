import { describe, expect, test } from "vitest";
import { BuiltinModuleRegistry } from "../src/modules.js";
import type { LongTermMemoryEntry, LongTermMemoryStats } from "../src/long-term-memory.js";
import type { LongTermMemoryAdapter } from "../src/long-term-memory-adapters/types.js";
import type { JsonObject, JsonValue, ModuleDefinition, RuntimeSnapshot } from "../src/types.js";

class FakeLongTermMemoryAdapter implements LongTermMemoryAdapter {
	readonly config = {
		kind: "sqlite",
		name: "fake",
		description: "fake adapter for tests",
	} as const;

	constructor(private readonly entries: LongTermMemoryEntry[]) {}

	close(): void {}

	write(): LongTermMemoryEntry {
		throw new Error("write not implemented in fake adapter");
	}

	get(): LongTermMemoryEntry | undefined {
		return undefined;
	}

	search(
		namespace: string | undefined,
		query: string | undefined,
		key: string | undefined,
		limit: number,
	) {
		const filtered = this.entries
			.filter((entry) => (namespace ? entry.namespace === namespace : true))
			.filter((entry) => (key ? entry.key === key : true))
			.filter((entry) =>
				query
					? JSON.stringify(entry.value).includes(query) ||
					  entry.key.includes(query) ||
					  entry.tags.some((tag) => tag.includes(query))
					: true,
			)
			.slice(0, limit);
		return { entries: filtered };
	}

	getStats(): LongTermMemoryStats {
		return {
			dbPath: "fake",
			fileSizeBytes: null,
			totalEntries: this.entries.length,
			totalNamespaces: new Set(this.entries.map((entry) => entry.namespace)).size,
			oldestCreatedAt: this.entries[0]?.createdAt ?? null,
			newestUpdatedAt: this.entries.at(-1)?.updatedAt ?? null,
			namespaces: [],
		};
	}

	garbageCollect() {
		return {
			deletedEntries: [],
			vacuumed: false,
		};
	}
}

function createSnapshot(): RuntimeSnapshot {
	return {
		inputs: {},
		vars: {},
		memory: {
			working: {},
		},
		tasks: {},
		agents: {},
	};
}

async function executeRetrieveModule(
	input: JsonObject,
	entries: LongTermMemoryEntry[],
): Promise<{ output: Record<string, JsonValue>; stateUpdates?: JsonObject }> {
	const registry = new BuiltinModuleRegistry(new FakeLongTermMemoryAdapter(entries), "memory-test");
	const definition: ModuleDefinition = {
		kind: "builtin.long_term_memory.retrieve",
	};
	const result = await registry.executeResolved(
		"run-1",
		"task-1",
		definition,
		input,
		createSnapshot(),
		process.cwd(),
	);
	return result;
}

describe("builtin long-term memory retrieval", () => {
	const entryA: LongTermMemoryEntry = {
		namespace: "memory-test",
		key: "finding",
		value: "restore-drill-missing",
		tags: ["readiness"],
		createdAt: "2026-04-12T00:00:00.000Z",
		updatedAt: "2026-04-12T00:00:00.000Z",
	};
	const entryB: LongTermMemoryEntry = {
		namespace: "memory-test",
		key: "owner",
		value: "incident-commander-missing",
		tags: ["operations"],
		createdAt: "2026-04-12T00:01:00.000Z",
		updatedAt: "2026-04-12T00:01:00.000Z",
	};

	test("promoteMode=value promotes the first matched value into working memory", async () => {
		const result = await executeRetrieveModule(
			{
				query: "restore",
				promoteKey: "recalled",
			},
			[entryA, entryB],
		);

		expect(result.stateUpdates).toEqual({ recalled: "restore-drill-missing" });
		expect(result.output.promotedValue).toBe("restore-drill-missing");
	});

	test("promoteMode=values with select=all promotes all values as a json array", async () => {
		const result = await executeRetrieveModule(
			{
				select: "all",
				promoteMode: "values",
				promoteKey: "recalled",
			},
			[entryA, entryB],
		);

		expect(result.stateUpdates).toEqual({
			recalled: ["restore-drill-missing", "incident-commander-missing"],
		});
		expect(result.output.matchCount).toBe(2);
	});

	test("promoteMode=entry promotes metadata-rich entry objects", async () => {
		const result = await executeRetrieveModule(
			{
				key: "finding",
				promoteMode: "entry",
				promoteKey: "recalled",
			},
			[entryA],
		);

		expect(result.stateUpdates).toEqual({
			recalled: {
				namespace: "memory-test",
				key: "finding",
				value: "restore-drill-missing",
				tags: ["readiness"],
				createdAt: "2026-04-12T00:00:00.000Z",
				updatedAt: "2026-04-12T00:00:00.000Z",
			},
		});
	});

	test("no matches does not mutate working memory", async () => {
		const result = await executeRetrieveModule(
			{
				query: "does-not-exist",
				promoteKey: "recalled",
			},
			[entryA],
		);

		expect(result.stateUpdates).toBeUndefined();
		expect(result.output.promoted).toBe(false);
		expect(result.output.promotedValue).toBeNull();
	});
});
