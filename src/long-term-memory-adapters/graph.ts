import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const GRAPH_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "graph",
	name: "graph",
	description: "Placeholder adapter for graph-backed long-term memory and relationship traversal.",
};

export class GraphLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(GRAPH_LONG_TERM_MEMORY_ADAPTER);
	}
}
