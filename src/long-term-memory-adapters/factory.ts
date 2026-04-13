import type { LongTermMemoryDefinition } from "../types.js";
import { LongTermMemoryStore } from "../long-term-memory.js";
import type { LongTermMemoryAdapter } from "./types.js";
import { MongoDbAtlasLongTermMemoryAdapter } from "./mongodb-atlas.js";
import { SqliteLongTermMemoryAdapter } from "./sqlite.js";

export interface LongTermMemoryConnectionOptions {
	provider?: "sqlite" | "mongodb-atlas";
	dbPath?: string;
	connectionString?: string;
	connectionStringEnv?: string;
	database?: string;
	collection?: string;
}

export function createLongTermMemoryAdapter(
	options: LongTermMemoryConnectionOptions | LongTermMemoryDefinition,
): LongTermMemoryAdapter {
	const provider = options.provider ?? "sqlite";
	if (provider === "mongodb-atlas") {
		const connectionString =
			options.connectionString ??
			(options.connectionStringEnv ? process.env[options.connectionStringEnv] : undefined);
		if (!connectionString) {
			throw new Error('MongoDB Atlas long-term memory requires "connectionString" or "connectionStringEnv"');
		}
		return new MongoDbAtlasLongTermMemoryAdapter({
			connectionString,
			database: options.database ?? "agentctl",
			collection: options.collection ?? "long_term_memories",
		});
	}
	return new SqliteLongTermMemoryAdapter(new LongTermMemoryStore(options.dbPath ?? ""));
}
