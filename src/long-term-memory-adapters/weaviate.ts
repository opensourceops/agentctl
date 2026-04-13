import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const WEAVIATE_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "weaviate",
	name: "weaviate",
	description: "Placeholder adapter for hybrid semantic retrieval in Weaviate.",
};

export class WeaviateLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(WEAVIATE_LONG_TERM_MEMORY_ADAPTER);
	}
}
