import { MongoClient } from "mongodb";
import type { JsonValue } from "../types.js";
import type { LongTermMemoryStats } from "../long-term-memory.js";
import type {
	LongTermMemoryAdapter,
	LongTermMemoryAdapterConfig,
	LongTermMemoryDeletedEntry,
	LongTermMemoryGarbageCollectResult,
} from "./types.js";

interface LongTermMemoryDocument {
	namespace: string;
	key: string;
	value: JsonValue;
	textValue: string;
	tags: string[];
	createdAt: string;
	updatedAt: string;
}

export interface MongoDbAtlasLongTermMemoryAdapterOptions {
	connectionString: string;
	database: string;
	collection: string;
	appName?: string;
}

export const MONGODB_ATLAS_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "mongodb-atlas",
	name: "mongodb-atlas",
	description: "MongoDB Atlas adapter for long-term memory.",
};

export class MongoDbAtlasLongTermMemoryAdapter implements LongTermMemoryAdapter {
	readonly config = MONGODB_ATLAS_LONG_TERM_MEMORY_ADAPTER;
	private readonly client: MongoClient;
	private initialized = false;

	constructor(private readonly options: MongoDbAtlasLongTermMemoryAdapterOptions) {
		this.client = new MongoClient(options.connectionString, {
			appName: options.appName ?? "agentctl",
		});
	}

	async close(): Promise<void> {
		await this.client.close();
	}

	async write(namespace: string, key: string, value: JsonValue, tags: string[] = []) {
		const collection = await this.getCollection();
		const existing = await this.get(namespace, key);
		const createdAt = existing?.createdAt ?? new Date().toISOString();
		const updatedAt = new Date().toISOString();
		await collection.updateOne(
			{ namespace, key },
			{
				$set: {
					value,
					textValue: JSON.stringify(value),
					tags,
					updatedAt,
				},
				$setOnInsert: {
					namespace,
					key,
					createdAt,
				},
			},
			{ upsert: true },
		);
		return {
			namespace,
			key,
			value,
			tags,
			createdAt,
			updatedAt,
		};
	}

	async get(namespace: string, key: string) {
		const collection = await this.getCollection();
		const document = await collection.findOne(
			{ namespace, key },
			{ projection: { _id: 0 } },
		);
		return document ? this.toEntry(document) : undefined;
	}

	async search(namespace: string | undefined, query: string | undefined, key: string | undefined, limit: number) {
		const collection = await this.getCollection();
		const filter: Record<string, unknown> = {};
		if (namespace) {
			filter.namespace = namespace;
		}
		if (key) {
			filter.key = key;
		} else if (query) {
			filter.$or = [
				{ key: { $regex: query, $options: "i" } },
				{ textValue: { $regex: query, $options: "i" } },
				{ tags: { $elemMatch: { $regex: query, $options: "i" } } },
			];
		}
		const documents = await collection
			.find(filter, { projection: { _id: 0 } })
			.sort({ updatedAt: -1, namespace: 1, key: 1 })
			.limit(limit)
			.toArray();
		return {
			entries: documents.map((document) => this.toEntry(document)),
		};
	}

	async getStats(namespace?: string): Promise<LongTermMemoryStats> {
		const collection = await this.getCollection();
		const filter = namespace ? { namespace } : {};
		const [summary] = await collection
			.aggregate<{
				totalEntries: number;
				oldestCreatedAt: string | null;
				newestUpdatedAt: string | null;
			}>([
				{ $match: filter },
				{
					$group: {
						_id: null,
						totalEntries: { $sum: 1 },
						oldestCreatedAt: { $min: "$createdAt" },
						newestUpdatedAt: { $max: "$updatedAt" },
					},
				},
				{
					$project: {
						_id: 0,
						totalEntries: 1,
						oldestCreatedAt: 1,
						newestUpdatedAt: 1,
					},
				},
			])
			.toArray();
		const namespaceRows = await collection
			.aggregate<{
				_id: string;
				entries: number;
				oldestCreatedAt: string | null;
				newestUpdatedAt: string | null;
			}>([
				...(namespace ? [{ $match: { namespace } }] : []),
				{
					$group: {
						_id: "$namespace",
						entries: { $sum: 1 },
						oldestCreatedAt: { $min: "$createdAt" },
						newestUpdatedAt: { $max: "$updatedAt" },
					},
				},
				{ $sort: { _id: 1 } },
			])
			.toArray();
		return {
			dbPath: `mongodb-atlas://${this.options.database}/${this.options.collection}`,
			fileSizeBytes: null,
			totalEntries: summary?.totalEntries ?? 0,
			totalNamespaces: namespace ? ((summary?.totalEntries ?? 0) > 0 ? 1 : 0) : namespaceRows.length,
			oldestCreatedAt: summary?.oldestCreatedAt ?? null,
			newestUpdatedAt: summary?.newestUpdatedAt ?? null,
			...(namespace ? { namespace } : {}),
			namespaces: namespaceRows.map((row) => ({
				namespace: row._id,
				entries: row.entries,
				oldestCreatedAt: row.oldestCreatedAt,
				newestUpdatedAt: row.newestUpdatedAt,
			})),
		};
	}

	async garbageCollect(options: {
		namespace?: string;
		olderThanDays: number;
		keepEntries: number;
		vacuum?: boolean;
	}): Promise<LongTermMemoryGarbageCollectResult> {
		const collection = await this.getCollection();
		const cutoff = new Date(Date.now() - options.olderThanDays * 24 * 60 * 60 * 1000).toISOString();
		const scoped = await collection
			.find(options.namespace ? { namespace: options.namespace } : {}, { projection: { _id: 0, namespace: 1, key: 1, updatedAt: 1 } })
			.sort({ updatedAt: -1, namespace: 1, key: 1 })
			.toArray() as Array<{ namespace: string; key: string; updatedAt: string }>;
		const retained = new Set(
			scoped.slice(0, options.keepEntries).map((row) => `${row.namespace}\u0000${row.key}`),
		);
		const deletedEntries: LongTermMemoryDeletedEntry[] = scoped
			.filter((row) => row.updatedAt < cutoff)
			.filter((row) => !retained.has(`${row.namespace}\u0000${row.key}`))
			.map((row) => ({
				namespace: row.namespace,
				key: row.key,
				updatedAt: row.updatedAt,
			}));
		if (deletedEntries.length > 0) {
			await collection.deleteMany({
				$or: deletedEntries.map((row) => ({ namespace: row.namespace, key: row.key })),
			});
		}
		return {
			deletedEntries,
			vacuumed: false,
		};
	}

	private async getCollection() {
		if (!this.initialized) {
			await this.client.connect();
			const collection = this.client.db(this.options.database).collection<LongTermMemoryDocument>(this.options.collection);
			await Promise.all([
				collection.createIndex({ namespace: 1, key: 1 }, { unique: true }),
				collection.createIndex({ namespace: 1, updatedAt: -1 }),
			]);
			this.initialized = true;
		}
		return this.client.db(this.options.database).collection<LongTermMemoryDocument>(this.options.collection);
	}

	private toEntry(document: LongTermMemoryDocument) {
		return {
			namespace: document.namespace,
			key: document.key,
			value: document.value,
			tags: document.tags,
			createdAt: document.createdAt,
			updatedAt: document.updatedAt,
		};
	}
}
