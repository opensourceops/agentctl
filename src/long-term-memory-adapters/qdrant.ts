import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const QDRANT_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "qdrant",
	name: "qdrant",
	description: "Placeholder adapter for vector search and metadata filtering in Qdrant.",
};

export class QdrantLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(QDRANT_LONG_TERM_MEMORY_ADAPTER);
	}
}
