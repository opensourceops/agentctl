import { existsSync, statSync } from "node:fs";
import Database from "better-sqlite3";
import type { JsonValue } from "./types.js";
import { ensureParentDir, nowIso } from "./utils.js";
import type {
	LongTermMemoryDeletedEntry,
	LongTermMemoryGarbageCollectResult,
} from "./long-term-memory-adapters/types.js";

const SCHEMA_SQL = `
CREATE TABLE IF NOT EXISTS long_term_memories (
	namespace TEXT NOT NULL,
	key TEXT NOT NULL,
	value_json TEXT NOT NULL,
	text_value TEXT NOT NULL,
	tags_json TEXT NOT NULL,
	created_at TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY (namespace, key)
);
`;

export interface LongTermMemoryEntry {
	namespace: string;
	key: string;
	value: JsonValue;
	tags: string[];
	createdAt: string;
	updatedAt: string;
}

export interface LongTermMemorySearchResult {
	entries: LongTermMemoryEntry[];
}

export interface LongTermMemoryNamespaceStats {
	namespace: string;
	entries: number;
	oldestCreatedAt: string | null;
	newestUpdatedAt: string | null;
}

export interface LongTermMemoryStats {
	dbPath: string;
	fileSizeBytes: number | null;
	totalEntries: number;
	totalNamespaces: number;
	oldestCreatedAt: string | null;
	newestUpdatedAt: string | null;
	namespace?: string;
	namespaces: LongTermMemoryNamespaceStats[];
}

export class LongTermMemoryStore {
	private readonly db: Database.Database;

	constructor(private readonly dbPath: string) {
		ensureParentDir(dbPath);
		this.db = new Database(dbPath);
		this.db.exec(SCHEMA_SQL);
	}

	close(): void {
		this.db.close();
	}

	getDbPath(): string {
		return this.dbPath;
	}

	write(namespace: string, key: string, value: JsonValue, tags: string[] = []): LongTermMemoryEntry {
		const existing = this.get(namespace, key);
		const createdAt = existing?.createdAt ?? nowIso();
		const updatedAt = nowIso();
		this.db
			.prepare(
				`INSERT OR REPLACE INTO long_term_memories
				 (namespace, key, value_json, text_value, tags_json, created_at, updated_at)
				 VALUES (?, ?, ?, ?, ?, ?, ?)`,
			)
			.run(namespace, key, JSON.stringify(value), JSON.stringify(value), JSON.stringify(tags), createdAt, updatedAt);
		return {
			namespace,
			key,
			value,
			tags,
			createdAt,
			updatedAt,
		};
	}

	get(namespace: string, key: string): LongTermMemoryEntry | undefined {
		const row = this.db
			.prepare(
				`SELECT namespace, key, value_json, tags_json, created_at, updated_at
				 FROM long_term_memories
				 WHERE namespace = ? AND key = ?`,
			)
			.get(namespace, key) as
			| {
					namespace: string;
					key: string;
					value_json: string;
					tags_json: string;
					created_at: string;
					updated_at: string;
			  }
			| undefined;
		if (!row) {
			return undefined;
		}
		return {
			namespace: row.namespace,
			key: row.key,
			value: JSON.parse(row.value_json) as JsonValue,
			tags: JSON.parse(row.tags_json) as string[],
			createdAt: row.created_at,
			updatedAt: row.updated_at,
		};
	}

	search(namespace: string | undefined, query: string | undefined, key: string | undefined, limit: number): LongTermMemorySearchResult {
		const rows = key
			? this.searchByKey(namespace, key, limit)
			: this.searchByQuery(namespace, query, limit);
		return {
			entries: rows.map((row) => ({
				namespace: row.namespace,
				key: row.key,
				value: JSON.parse(row.value_json) as JsonValue,
				tags: JSON.parse(row.tags_json) as string[],
				createdAt: row.created_at,
				updatedAt: row.updated_at,
			})),
		};
	}

	getStats(namespace?: string): LongTermMemoryStats {
		const summary = namespace
			? (this.db
					.prepare(
						`SELECT COUNT(*) AS total_entries,
						        MIN(created_at) AS oldest_created_at,
						        MAX(updated_at) AS newest_updated_at
						   FROM long_term_memories
						  WHERE namespace = ?`,
					)
					.get(namespace) as {
						total_entries: number;
						oldest_created_at: string | null;
						newest_updated_at: string | null;
					})
			: (this.db
					.prepare(
						`SELECT COUNT(*) AS total_entries,
						        MIN(created_at) AS oldest_created_at,
						        MAX(updated_at) AS newest_updated_at
						   FROM long_term_memories`,
					)
					.get() as {
						total_entries: number;
						oldest_created_at: string | null;
						newest_updated_at: string | null;
					});
		const namespaceRows = namespace
			? [this.getNamespaceStats(namespace)]
			: (this.db
					.prepare(
						`SELECT namespace,
						        COUNT(*) AS entries,
						        MIN(created_at) AS oldest_created_at,
						        MAX(updated_at) AS newest_updated_at
						   FROM long_term_memories
						  GROUP BY namespace
						  ORDER BY namespace ASC`,
					)
					.all() as Array<{
						namespace: string;
						entries: number;
						oldest_created_at: string | null;
						newest_updated_at: string | null;
					}>).map((row) => ({
						namespace: row.namespace,
						entries: row.entries,
						oldestCreatedAt: row.oldest_created_at,
						newestUpdatedAt: row.newest_updated_at,
					}));
		return {
			dbPath: this.dbPath,
			fileSizeBytes: existsSync(this.dbPath) ? statSync(this.dbPath).size : 0,
			totalEntries: summary.total_entries,
			totalNamespaces: namespace ? (summary.total_entries > 0 ? 1 : 0) : namespaceRows.length,
			oldestCreatedAt: summary.oldest_created_at,
			newestUpdatedAt: summary.newest_updated_at,
			...(namespace ? { namespace } : {}),
			namespaces: namespaceRows.filter((row) => row.entries > 0),
		};
	}

	garbageCollect(options: {
		namespace?: string;
		olderThanDays: number;
		keepEntries: number;
		vacuum?: boolean;
	}): LongTermMemoryGarbageCollectResult {
		const cutoff = new Date(Date.now() - options.olderThanDays * 24 * 60 * 60 * 1000).toISOString();
		const scopedRows = options.namespace
			? (this.db
					.prepare(
						`SELECT namespace, key, updated_at
						   FROM long_term_memories
						  WHERE namespace = ?
						  ORDER BY updated_at DESC, key ASC`,
					)
					.all(options.namespace) as Array<{ namespace: string; key: string; updated_at: string }>)
			: (this.db
					.prepare(
						`SELECT namespace, key, updated_at
						   FROM long_term_memories
						  ORDER BY updated_at DESC, namespace ASC, key ASC`,
					)
					.all() as Array<{ namespace: string; key: string; updated_at: string }>);
		const retained = new Set(
			scopedRows
				.slice(0, options.keepEntries)
				.map((row) => `${row.namespace}\u0000${row.key}`),
		);
		const toDelete = scopedRows
			.filter((row) => row.updated_at < cutoff)
			.filter((row) => !retained.has(`${row.namespace}\u0000${row.key}`));
		const deletedEntries: LongTermMemoryDeletedEntry[] = toDelete.map((row) => ({
			namespace: row.namespace,
			key: row.key,
			updatedAt: row.updated_at,
		}));
		const deleteStatement = this.db.prepare(
			`DELETE FROM long_term_memories WHERE namespace = ? AND key = ?`,
		);
		const transaction = this.db.transaction((rows: LongTermMemoryDeletedEntry[]) => {
			for (const row of rows) {
				deleteStatement.run(row.namespace, row.key);
			}
		});
		transaction(deletedEntries);
		let vacuumed = false;
		if ((options.vacuum ?? true) && deletedEntries.length > 0) {
			this.db.exec("VACUUM");
			vacuumed = true;
		}
		return {
			deletedEntries,
			vacuumed,
		};
	}

	private searchByKey(namespace: string | undefined, key: string, limit: number): Array<{
		namespace: string;
		key: string;
		value_json: string;
		tags_json: string;
		created_at: string;
		updated_at: string;
	}> {
		if (namespace) {
			const entry = this.get(namespace, key);
			if (!entry) {
				return [];
			}
			return [
				{
					namespace: entry.namespace,
					key: entry.key,
					value_json: JSON.stringify(entry.value),
					tags_json: JSON.stringify(entry.tags),
					created_at: entry.createdAt,
					updated_at: entry.updatedAt,
				},
			];
		}
		return this.db
			.prepare(
				`SELECT namespace, key, value_json, tags_json, created_at, updated_at
				   FROM long_term_memories
				  WHERE key = ?
				  ORDER BY updated_at DESC
				  LIMIT ?`,
			)
			.all(key, limit) as Array<{
				namespace: string;
				key: string;
				value_json: string;
				tags_json: string;
				created_at: string;
				updated_at: string;
			}>;
	}

	private searchByQuery(namespace: string | undefined, query: string | undefined, limit: number): Array<{
		namespace: string;
		key: string;
		value_json: string;
		tags_json: string;
		created_at: string;
		updated_at: string;
	}> {
		const sqlQuery = typeof query === "string" ? `%${query}%` : "%";
		if (namespace) {
			return this.db
				.prepare(
					`SELECT namespace, key, value_json, tags_json, created_at, updated_at
					   FROM long_term_memories
					  WHERE namespace = ?
					    AND (key LIKE ? OR text_value LIKE ? OR tags_json LIKE ?)
					  ORDER BY updated_at DESC
					  LIMIT ?`,
				)
				.all(namespace, sqlQuery, sqlQuery, sqlQuery, limit) as Array<{
					namespace: string;
					key: string;
					value_json: string;
					tags_json: string;
					created_at: string;
					updated_at: string;
				}>;
		}
		return this.db
			.prepare(
				`SELECT namespace, key, value_json, tags_json, created_at, updated_at
				   FROM long_term_memories
				  WHERE key LIKE ? OR text_value LIKE ? OR tags_json LIKE ?
				  ORDER BY updated_at DESC
				  LIMIT ?`,
			)
			.all(sqlQuery, sqlQuery, sqlQuery, limit) as Array<{
				namespace: string;
				key: string;
				value_json: string;
				tags_json: string;
				created_at: string;
				updated_at: string;
			}>;
	}

	private getNamespaceStats(namespace: string): LongTermMemoryNamespaceStats {
		const row = this.db
			.prepare(
				`SELECT COUNT(*) AS entries,
				        MIN(created_at) AS oldest_created_at,
				        MAX(updated_at) AS newest_updated_at
				   FROM long_term_memories
				  WHERE namespace = ?`,
			)
			.get(namespace) as {
				entries: number;
				oldest_created_at: string | null;
				newest_updated_at: string | null;
			};
		return {
			namespace,
			entries: row.entries,
			oldestCreatedAt: row.oldest_created_at,
			newestUpdatedAt: row.newest_updated_at,
		};
	}
}
