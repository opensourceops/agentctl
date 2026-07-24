# State and memory

Four mechanisms remain intentionally separate:

- Run state is authoritative lifecycle data: inputs, task states, attempts, outputs, cancellations, effects, approvals, and checkpoints.
- Working memory is a JSON object owned by one run. Writes are explicit keyed internal-state effects. Parallel tasks read durable isolated snapshots; disjoint deltas commit in compiled order with task transitions and the checkpoint. Unordered conflicting write sets fail compilation.
- Long-term memory is namespaced SQLite data across runs with optional expiry. Reads/writes are explicit actions; `memory get/put` and `gc` provide administration. Replay never rolls it back or treats it as history.
- Provider prompt cache is an optional performance optimization. Cache keys/options and usage counts are provider metadata, never correctness or memory.

Long-term retrieval is exact namespace/key lookup in this release. Vector search and automatic promotion are not implemented. A workflow promotes a value explicitly by reading long-term memory and then writing working memory. Retention is applied by expiration/GC, not by replay.
