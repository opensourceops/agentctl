# ADR 0018: Typed semantic memory with recorded retrieval

- Status: accepted
- Date: 2026-07-27

## Context

Exact namespace/key values did not support relevant cross-run retrieval.
Implicit provider memory or automatic promotion would bypass workflow dataflow,
retention, effect inspection, and replay guarantees. A provider-specific vector
store would also make the core contract depend on one vendor.

## Decision

Long-term entries use a versioned provider-neutral text or JSON contract with
searchable text and exact-match metadata. Retrieval is explicit text, vector,
or hybrid search with bounded candidates and results, deterministic ranking,
stable key tie-breaking, namespaces, and expiry.

The core exposes embedding and memory adapter traits. SQLite provides the local
store and index. `local_hash` supplies deterministic lexical vectors for
offline operation. A packaged optional OpenAI adapter supplies neural
embeddings through a named provider and explicit model. External stores may
implement the public memory adapter.

Reads and searches are recorded observation effects. Writes are recorded
external mutations. Promotion into run working memory is a separate explicit
internal-state effect. Recorded replay reuses retrieval output without querying
the store or embedding provider; selective repair re-executes the selected
boundary.

## Consequences

Semantic quality is selectable without changing durability semantics. The
local hash provider is deterministic and cheap but lexical, so it cannot claim
neural semantic quality. SQLite search is intentionally local and bounded;
larger or specialized indexes belong behind the adapter trait.

Schema 14 preserves legacy values as typed entries and adds embedding identity,
dimension, vector, creation time, and search-index fields. Corrupt vectors,
dimension mismatches, invalid adapter results, expired records, and unsupported
provider configurations fail closed.
