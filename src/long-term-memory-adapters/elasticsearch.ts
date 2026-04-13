import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const ELASTICSEARCH_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "elasticsearch",
	name: "elasticsearch",
	description: "Placeholder adapter for document and keyword retrieval in Elasticsearch/OpenSearch.",
};

export class ElasticsearchLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(ELASTICSEARCH_LONG_TERM_MEMORY_ADAPTER);
	}
}
