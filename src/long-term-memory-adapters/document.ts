import {
	PlaceholderLongTermMemoryAdapter,
	type LongTermMemoryAdapterConfig,
} from "./types.js";

export const DOCUMENT_LONG_TERM_MEMORY_ADAPTER: LongTermMemoryAdapterConfig = {
	kind: "document",
	name: "document",
	description: "Placeholder adapter for document-store-backed long-term memory.",
};

export class DocumentLongTermMemoryAdapter extends PlaceholderLongTermMemoryAdapter {
	constructor() {
		super(DOCUMENT_LONG_TERM_MEMORY_ADAPTER);
	}
}
