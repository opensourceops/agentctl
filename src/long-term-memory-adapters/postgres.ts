import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const POSTGRES_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "postgres",
	name: "postgres",
	description: "Placeholder adapter for SQL-backed cross-run memory in PostgreSQL.",
};

export class PostgresLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(POSTGRES_LONG_TERM_MEMORY_ADAPTER);
	}
}
