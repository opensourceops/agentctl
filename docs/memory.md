# State and memory

Four mechanisms remain intentionally separate:

- Run state is authoritative lifecycle data: inputs, task states, attempts, outputs, cancellations, effects, approvals, and checkpoints.
- Working memory is a JSON object owned by one run. Writes are explicit keyed internal-state effects. Parallel tasks read durable isolated snapshots; disjoint deltas commit in compiled order with task transitions and the checkpoint. Unordered conflicting write sets fail compilation.
- Long-term memory is typed, namespaced data across runs with optional expiry, exact lookup, metadata filters, and text, vector, or hybrid retrieval. SQLite is built in and a provider-neutral adapter trait supports external stores.
- Provider prompt cache is an optional performance optimization. Cache keys/options and usage counts are provider metadata, never correctness or memory.

## Long-term memory configuration

```yaml
spec:
  memory:
    working:
      recalled: []
    longTerm:
      provider: sqlite
      namespace: support
      retentionDays: 30
      embedding:
        provider: local_hash
        dimensions: 64
```

`provider` selects the memory store. The packaged CLI provides `sqlite`.
`embedding.provider` selects `local_hash` or a named provider. The local hash
provider is a deterministic lexical vector baseline suitable for offline tests,
not a neural semantic model. A named OpenAI provider can use
`model: text-embedding-3-small`; it follows the provider's credential, endpoint,
and header configuration. OpenAI dimensions require a
`text-embedding-3` model that supports the requested size.

Entries use format version 1 and contain either typed text or JSON plus
searchable text and exact-match metadata:

```yaml
content: { type: text, text: "customer prefers concise release notes" }
metadata: { team: docs, priority: 2 }
```

JSON entries may instead use `content: { type: json, value: ..., text: ... }`.
Legacy JSON values are read as versioned entries with derived searchable text.

## Workflow actions

- `builtin.long_term_memory.read` reads one namespace/key record.
- `builtin.long_term_memory.search` accepts `query`, `mode` (`text`, `vector`,
  or `hybrid`), `limit`, optional `namespace`, and exact metadata `filters`.
- `builtin.long_term_memory.write` accepts `key`, optional `namespace`,
  `content` or `entry`, metadata, and optional `retentionDays`.
- `builtin.long_term_memory.promote` explicitly copies its `value` into a
  declared working-memory `key`.

Search returns a stable ordered result set with typed records and integer
millionth scores. Text scoring is deterministic token overlap. Vector scoring
uses cosine similarity. Hybrid mode combines both scores. Equal scores are
ordered by key. Expired records are excluded.

Retrieval and writes are durable effects. Recorded replay reuses the recorded
result and performs no memory or embedding call. Selective repair re-executes a
selected retrieval boundary, so it can observe entries added after the source
run. Promotion is explicit and recorded; there is no automatic or hidden model
memory.

## Administration and bounds

```text
agentctl memory --db .agentctl/runtime.db get NAMESPACE KEY
agentctl memory --db .agentctl/runtime.db put NAMESPACE KEY JSON_VALUE --text TEXT --metadata JSON --retention-days 30
agentctl memory --db .agentctl/runtime.db search NAMESPACE QUERY --mode hybrid --limit 10 --filter team='"docs"'
agentctl memory --db .agentctl/runtime.db reindex NAMESPACE
```

The CLI administration path writes and rebuilds 64-dimensional `local_hash`
vectors. Workflow execution is the path for configured OpenAI or external
embedding providers.

Entries are capped at 1 MiB, queries at 64 KiB, results at 100, embedding
dimensions from 8 through 4096, and a local search scan at 10,000 active
candidates. Corrupt or mismatched vector dimensions fail closed. Retention is
applied during reads/search and by garbage collection, not by replay.
