import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { LongTermMemoryStore } from "../src/long-term-memory.js";
import {
	createLongTermMemoryAdapter,
	MongoDbAtlasLongTermMemoryAdapter,
	PostgresLongTermMemoryAdapter,
	SqliteLongTermMemoryAdapter,
} from "../src/long-term-memory-adapters/index.js";

describe("long-term memory adapters", () => {
	test("sqlite adapter delegates to the concrete store", () => {
		const dir = mkdtempSync(join(tmpdir(), "agentctl-ltm-adapter-"));
		const dbPath = join(dir, "long-term.db");
		const store = new LongTermMemoryStore(dbPath);
		const adapter = new SqliteLongTermMemoryAdapter(store);

		try {
			const written = adapter.write("service-a", "finding", "restore-drill-missing", ["readiness"]);
			expect(written).toEqual(
				expect.objectContaining({
					namespace: "service-a",
					key: "finding",
					value: "restore-drill-missing",
					tags: ["readiness"],
				}),
			);

			expect(adapter.get("service-a", "finding")).toEqual(
				expect.objectContaining({
					namespace: "service-a",
					key: "finding",
					value: "restore-drill-missing",
				}),
			);

			expect(adapter.search(undefined, undefined, "finding", 10).entries).toHaveLength(1);
			expect(adapter.getStats().totalEntries).toBe(1);
		} finally {
			adapter.close();
			rmSync(dir, { recursive: true, force: true });
		}
	});

	test("placeholder adapters fail explicitly when used", () => {
		const adapter = new PostgresLongTermMemoryAdapter();

		expect(() => adapter.write("service-a", "finding", "value")).toThrow(
			"postgres long-term memory adapter is a placeholder",
		);
		expect(() => adapter.search("service-a", "value", undefined, 10)).toThrow(
			"postgres long-term memory adapter is a placeholder",
		);
	});

	test("mongodb-atlas factory requires a connection string", () => {
		expect(() =>
			createLongTermMemoryAdapter({
				provider: "mongodb-atlas",
				database: "agentctl",
				collection: "long_term_memories",
			}),
		).toThrow('MongoDB Atlas long-term memory requires "connectionString" or "connectionStringEnv"');
	});

	test("mongodb-atlas stats map aggregate rows into framework stats shape", async () => {
		const adapter = new MongoDbAtlasLongTermMemoryAdapter({
			connectionString: "mongodb://example.invalid",
			database: "agentctl",
			collection: "long_term_memories",
		});
		const pipelines: object[][] = [];
		const fakeCollection = {
			aggregate<T>(pipeline: object[]) {
				pipelines.push(pipeline);
				if (pipelines.length === 1) {
					return {
						toArray: async () =>
							[
								{
									totalEntries: 2,
									oldestCreatedAt: "2026-04-12T00:00:00.000Z",
									newestUpdatedAt: "2026-04-12T01:00:00.000Z",
								},
							] satisfies T[],
					};
				}
				return {
					toArray: async () =>
						[
							{
								_id: "service-a",
								entries: 2,
								oldestCreatedAt: "2026-04-12T00:00:00.000Z",
								newestUpdatedAt: "2026-04-12T01:00:00.000Z",
							},
						] satisfies T[],
				};
			},
		};

		Object.defineProperty(adapter, "getCollection", {
			value: async () => fakeCollection,
		});

		try {
			const stats = await adapter.getStats();
			expect(stats.totalEntries).toBe(2);
			expect(stats.totalNamespaces).toBe(1);
			expect(stats.oldestCreatedAt).toBe("2026-04-12T00:00:00.000Z");
			expect(stats.newestUpdatedAt).toBe("2026-04-12T01:00:00.000Z");
			expect(stats.namespaces).toEqual([
				{
					namespace: "service-a",
					entries: 2,
					oldestCreatedAt: "2026-04-12T00:00:00.000Z",
					newestUpdatedAt: "2026-04-12T01:00:00.000Z",
				},
			]);
		} finally {
			await adapter.close();
		}
	});
});
