import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const PINECONE_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "pinecone",
	name: "pinecone",
	description: "Placeholder adapter for semantic vector retrieval in Pinecone.",
};

export class PineconeLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(PINECONE_LONG_TERM_MEMORY_ADAPTER);
	}
}
