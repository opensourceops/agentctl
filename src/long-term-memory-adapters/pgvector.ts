import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const PGVECTOR_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "pgvector",
	name: "pgvector",
	description: "Placeholder adapter for vector-backed long-term memory on PostgreSQL/pgvector.",
};

export class PgvectorLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(PGVECTOR_LONG_TERM_MEMORY_ADAPTER);
	}
}
